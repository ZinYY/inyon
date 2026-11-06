//! PSmux terminal bytes over one persistent control connection, pinned to one pane.
//! Capture only on attach/resize; normal output is pushed, and keys never start a process.
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use crate::hosts::host::{Host, PtyHandle};
use crate::hosts::http_error::{http, HubResult};
use crate::platform::rt::{spawn, sleep, Deferred};
use super::mirror::{draw_screen, parse_screen};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Reply { Ignore, Geometry, Capture, Observe, Input }

struct Decoder {
    pane: String,
    pending: Vec<u8>,
    replies: VecDeque<Reply>,
    frame: Option<(Reply, Vec<String>)>,
    geometry: String,
    ready: bool,
    observe_enter: bool,
    observations: Vec<Vec<String>>,
}

fn raw_pane(qualified: &str) -> Option<String> {
    let (session, pane) = qualified.strip_prefix('%')?.split_once('.')?;
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    (digits(session) && digits(pane)).then(|| format!("%{pane}"))
}

fn unescape(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' {
            if bytes.get(i + 1) == Some(&b'\\') { out.push(b'\\'); i += 2; continue; }
            if let Some(octal) = bytes.get(i + 1..i + 4).filter(|v| v.iter().all(|b| (b'0'..=b'7').contains(b))) {
                out.push(((octal[0] - b'0') as u16 * 64 + (octal[1] - b'0') as u16 * 8 + (octal[2] - b'0') as u16) as u8);
                i += 4; continue;
            }
        }
        out.push(bytes[i]); i += 1;
    }
    out
}

fn restore_snapshot(geometry: &str, body: &[String]) -> Result<Vec<u8>, String> {
    let height = parse_screen(&format!("{geometry}\n")).ok_or("Invalid PSmux screen")?.height as usize;
    let history = body.len().saturating_sub(height);
    let screen = parse_screen(&format!("{geometry}\n{}\n", body[history..].join("\n"))).ok_or("Invalid PSmux screen")?;
    // Rebuild retained server scrollback on attach and resize. Reset the browser's old
    // buffer first so reconnecting/resizing never duplicates history. Wrapping stays off
    // because capture-pane has already laid out each physical terminal row.
    let mut out = "\x1b[?7l\x1b[0m\x1b[r\x1b[H\x1b[2J\x1b[3J".to_owned();
    if history > 0 {
        for (i, line) in body.iter().enumerate() {
            if i > 0 { out.push_str("\r\n"); }
            out.push_str("\x1b[0m");
            out.push_str(line);
        }
    }
    out.push_str(&draw_screen(None, &screen));
    out.push_str("\x1b[?7h");
    Ok(out.into_bytes())
}

impl Decoder {
    fn new(pane: String) -> Self {
        Self { pane, pending: Vec::new(), replies: VecDeque::new(), frame: None, geometry: String::new(), ready: false, observe_enter: false, observations: Vec::new() }
    }

    fn snapshot(&mut self, cols: u16, rows: u16) -> String {
        self.ready = false;
        self.replies.extend([Reply::Ignore, Reply::Geometry, Reply::Capture]);
        let p = &self.pane;
        format!("resize-window -t {p} -x {cols} -y {rows}\ndisplay-message -p -t {p} '#{{pane_width}} #{{pane_height}} #{{cursor_x}} #{{cursor_y}} #{{cursor_flag}} #{{alternate_on}}'\ncapture-pane -p -e -S - -t {p}\n")
    }

    fn input(&mut self, text: &str) -> String {
        let mut commands = String::new();
        // Capture the confirmation *before* Enter dismisses it. Both commands use
        // the existing ordered connection: no new process or network round trip
        // before forwarding the key. A pasted newline must not trigger this.
        if self.observe_enter && matches!(text, "\r" | "\n") {
            self.replies.push_back(Reply::Observe);
            commands.push_str(&format!("capture-pane -p -e -t {}\n", self.pane));
        }
        // Hex is a byte channel: Unicode, bracketed paste and escape sequences remain exact.
        // A fixed target on every command prevents another client's window selection redirecting input.
        for chunk in text.as_bytes().chunks(4096) {
            self.replies.push_back(Reply::Input);
            commands.push_str(&format!("send-keys -t {} -H", self.pane));
            for b in chunk { commands.push_str(&format!(" {b:02x}")); }
            commands.push('\n');
        }
        commands
    }

    fn feed(&mut self, data: &[u8]) -> Result<Vec<Vec<u8>>, String> {
        self.pending.extend_from_slice(data);
        let mut output = Vec::new();
        while let Some(end) = self.pending.iter().position(|b| *b == b'\n') {
            let bytes: Vec<u8> = self.pending.drain(..=end).collect();
            let line = String::from_utf8_lossy(&bytes[..bytes.len() - 1]);
            let line = line.trim_end_matches('\r').strip_prefix("\x1bP1000p").unwrap_or(line.trim_end_matches('\r'));
            if let Some((kind, body)) = &mut self.frame {
                if line.starts_with("%end ") || line.starts_with("%error ") {
                    let kind = *kind;
                    let body = std::mem::take(body);
                    self.frame = None;
                    if line.starts_with("%error ") {
                        if kind == Reply::Observe { continue; }
                        // PSmux 3.4's SendBytes dispatch drops its response sender after enqueueing
                        // input. The control handler mislabels that immediate channel closure as a
                        // timeout. Treat only that known write-only response as fire-and-forget.
                        if kind == Reply::Input && body == ["command timed out"] { continue; }
                        return Err(body.join(" "));
                    }
                    match kind {
                        Reply::Geometry => self.geometry = body.join("\n"),
                        Reply::Capture => {
                            output.push(restore_snapshot(&self.geometry, &body)?);
                            self.ready = true;
                        }
                        Reply::Observe => self.observations.push(body),
                        _ => {}
                    }
                } else { body.push(line.to_owned()); }
                continue;
            }
            if line.starts_with("%begin ") {
                // The initial attach notification has flags=0 and is not a command reply.
                let kind = if line.split_whitespace().last() == Some("0") { Reply::Ignore } else { self.replies.pop_front().unwrap_or(Reply::Ignore) };
                self.frame = Some((kind, Vec::new()));
            } else if let Some(rest) = line.strip_prefix("%output ") {
                if let Some((pane, escaped)) = rest.split_once(' ') {
                    if self.ready && pane == self.pane { output.push(unescape(escaped)); }
                }
            } else if line == "%exit" || line.starts_with("%exit ") { return Err("PSmux connection closed".into()); }
        }
        Ok(output)
    }
}

pub fn open(host: Host, session: &str, pane: &str, cols: u16, rows: u16, mut on_data: impl FnMut(Vec<u8>) + 'static) -> HubResult<PtyHandle> {
    let raw = raw_pane(pane).ok_or_else(|| http(400, "Invalid PSmux pane"))?;
    let decoder = Rc::new(RefCell::new(Decoder::new(raw)));
    let host_id = host.config().id.clone();
    let pane_id = pane.to_owned();
    decoder.borrow_mut().observe_enter = crate::hosts::registry::registry().pane_agent(&host_id, pane)
        .is_some_and(|a| a.kind == inyon_protocol::AgentKind::Claude);
    let closed = Rc::new(Cell::new(false));
    let handle: Rc<RefCell<Option<PtyHandle>>> = Rc::new(RefCell::new(None));
    let (d, c, h) = (decoder.clone(), closed.clone(), handle.clone());
    let cmd = format!("inyon-psmux-control:{}", serde_json::to_string(session).expect("session string"));
    let pipe = host.pty(&cmd, cols, rows, move |bytes| {
        if c.get() { return; }
        let result = d.borrow_mut().feed(&bytes);
        for lines in std::mem::take(&mut d.borrow_mut().observations) {
            if let Some(w) = crate::claude::watcher::watchers().peek(&host_id, &pane_id) {
                w.observe_terminal_confirmation(&lines);
            }
        }
        match result {
            Ok(chunks) => { for bytes in chunks { on_data(bytes); } }
            Err(e) => {
                c.set(true);
                let text: String = e.chars().filter(|ch| !ch.is_control()).take(300).collect();
                on_data(format!("\r\n\x1b[31mInyon: {text}\x1b[0m\r\n").into_bytes());
                if let Some(p) = h.borrow().as_ref() { p.kill(); }
            }
        }
    })?;
    *handle.borrow_mut() = Some(pipe.clone());
    let initial = decoder.borrow_mut().snapshot(cols, rows);
    pipe.write(&initial);
    let (write, resize, kill) = (pipe.clone(), pipe.clone(), pipe.clone());
    let (wd, rd, wc, rc, kc) = (decoder.clone(), decoder, closed.clone(), closed.clone(), closed);
    // Same size restoration as the old mirror, including disconnects and remote exits.
    let exit = pipe.exited.clone();
    let target = crate::hosts::host::shq(pane);
    spawn(async move {
        exit.wait().await;
        handle.borrow_mut().take();
        let _ = host.run(&format!("tmux resize-window -t {target} -A 2>/dev/null; true")).await;
    });
    Ok(PtyHandle::new(
        move |text| { if !wc.get() { let cmd = wd.borrow_mut().input(text); write.write(&cmd); } },
        move |cols, rows| { if !rc.get() { let cmd = rd.borrow_mut().snapshot(cols, rows); resize.write(&cmd); } },
        move || { kc.set(true); kill.kill(); },
        pipe.exited,
    ))
}

pub(crate) fn available(host: &Host) -> bool {
    !crate::tmux::herdr::enabled(host)
        && crate::hosts::node_link::info(&host.config().id).and_then(|n| n.version)
            .is_some_and(|v| v == inyon_protocol::VERSION)
        && inyon_protocol::MANAGED_NODE_RELEASES
}

/// The same private byte channel as Terminal, without attaching or resizing a view.
/// Text is written on the control stream, never put in process arguments or a CLI debug log.
pub(crate) async fn send_input(host: &Host, pane: &str, text: &str) -> HubResult<()> {
    let raw = raw_pane(pane).ok_or_else(|| http(400, "Invalid PSmux pane"))?;
    let found = crate::hosts::registry::registry().find_pane(&host.config().id, pane)
        .ok_or_else(|| http(404, "Pane not found"))?;
    let mut decoder = Decoder::new(raw);
    let command = decoder.input(text);
    if command.is_empty() { return Ok(()); }
    let done = Deferred::<Result<(), String>>::new();
    let reply = done.clone();
    let cmd = format!("inyon-psmux-control:{}", serde_json::to_string(&found.session.name).expect("session string"));
    let pipe = host.pty(&cmd, 80, 24, move |bytes| {
        match decoder.feed(&bytes) {
            Err(e) => reply.resolve(Err(e)),
            Ok(_) if decoder.replies.is_empty() && decoder.frame.is_none() => reply.resolve(Ok(())),
            _ => {}
        }
    })?;
    pipe.write(&command);
    for _ in 0..100 {
        if done.is_settled() { break; }
        if pipe.exited.is_settled() {
            done.resolve(Err("Windows input connection closed before acknowledgement".into()));
            break;
        }
        sleep(50).await;
    }
    pipe.kill();
    done.value().unwrap_or_else(|| Err("Windows input acknowledgement timed out".into())).map_err(|e| http(409, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chat_byte_writes_keep_the_paste_exact_and_close_on_ack_error_or_timeout() {
        use crate::hosts::host::{set_host_backends, HostBackend, ExecOptions, ExecResult, OnData, OnLine, StreamHandle};
        use futures_util::future::LocalBoxFuture;
        use inyon_protocol::HostConfig;
        struct Control {
            writes: Rc<RefCell<Vec<String>>>,
            killed: Rc<Cell<bool>>,
            answer: Option<&'static str>,
        }
        impl HostBackend for Control {
            fn exec(&self, _: &HostConfig, _: &str, _: ExecOptions) -> LocalBoxFuture<'static, HubResult<ExecResult>> { panic!("text must not go through CLI arguments") }
            fn stream(&self, _: &HostConfig, _: &str, _: OnLine, _: bool) -> StreamHandle { panic!("no log-producing CLI stream") }
            fn pty(&self, _: &HostConfig, cmd: &str, _: u16, _: u16, on_data: OnData) -> HubResult<PtyHandle> {
                assert_eq!(cmd, "inyon-psmux-control:\"work\"");
                let data = Rc::new(RefCell::new(on_data));
                let (writes, killed, answer) = (self.writes.clone(), self.killed.clone(), self.answer);
                Ok(PtyHandle::new(move |s| {
                    writes.borrow_mut().push(s.to_owned());
                    if let Some(answer) = answer {
                        for (i, _) in s.lines().enumerate() {
                            (data.borrow_mut())(format!("%begin 1 {i} 1\n{answer}\n%error 1 {i} 1\n").into_bytes());
                        }
                    }
                }, |_, _| panic!("Chat must not resize the terminal"), move || killed.set(true), Deferred::new()))
            }
        }
        crate::test_support::rt::run(|| async {
            for answer in [Some("command timed out"), Some("can't find pane"), None] {
                let writes = Rc::new(RefCell::new(Vec::new()));
                let killed = Rc::new(Cell::new(false));
                let backend = Rc::new(Control { writes: writes.clone(), killed: killed.clone(), answer });
                set_host_backends(move |_| backend.clone());
                crate::test_support::machine("chat-bytes", "local", serde_json::json!([{ "id":"%14.1" }])).borrow_mut().hold_polls();
                let host = crate::hosts::registry::registry().host("chat-bytes").unwrap();
                let text = format!("\x1b[200~{}last customer's line\x1b[201~", "中文 row\n".repeat(900));
                let result = send_input(&host, "%14.1", &text).await;
                assert_eq!(result.is_ok(), answer == Some("command timed out"));
                assert!(killed.get(), "every one-shot channel is closed");
                let bytes = writes.borrow().iter().flat_map(|s| s.lines()).flat_map(|line| {
                    line.strip_prefix("send-keys -t %1 -H ").unwrap().split(' ').map(|b| u8::from_str_radix(b, 16).unwrap())
                }).collect::<Vec<_>>();
                assert_eq!(bytes, text.as_bytes());
                assert_eq!(writes.borrow().len(), 1, "no duplicate fallback after an uncertain write");
            }
        });
    }

    #[test]
    fn claude_confirmation_capture_precedes_enter_without_redrawing_the_terminal() {
        let mut d = Decoder::new("%3".into()); d.ready = true; d.observe_enter = true;
        assert_eq!(d.input("\r"), "capture-pane -p -e -t %3\nsend-keys -t %3 -H 0d\n");
        let out=d.feed(b"%begin 1 1 1\nConfirm you want to restore to the point before this message:\n| question\n%end 1 1 1\n%begin 1 2 1\ncommand timed out\n%error 1 2 1\n%output %3 draft\n").unwrap();
        assert_eq!(out,vec![b"draft".to_vec()]);
        assert_eq!(d.observations.len(),1);
        assert!(d.observations[0][0].starts_with("Confirm you want to restore"));
        assert!(!d.input("\x1b[200~line\nline\x1b[201~").contains("capture-pane"));
        let mut regular=Decoder::new("%3".into());
        assert_eq!(regular.input("\r"),"send-keys -t %3 -H 0d\n");
    }

    #[test]
    fn failed_observation_does_not_drop_input_or_close_terminal() {
        let mut d=Decoder::new("%3".into()); d.ready=true; d.observe_enter=true; d.input("\r");
        assert!(d.feed(b"%begin 1 1 1\ncapture unavailable\n%error 1 1 1\n%begin 1 2 1\n%end 1 2 1\n").is_ok());
        assert!(d.observations.is_empty());
        assert!(d.replies.is_empty());
    }

    #[test]
    fn fragmented_output_is_decoded_and_other_panes_are_ignored() {
        let mut d = Decoder::new("%2".into()); d.ready = true;
        assert!(d.feed(b"%output %1 wrong\n%output %2 \\344\\275").unwrap().is_empty());
        assert_eq!(d.feed(b"\\240\\345\\245\\275\\015\\012\\\\\\134\n").unwrap().concat(), "你好\r\n\\\\".as_bytes());
    }

    #[test]
    fn attach_snapshot_then_raw_bytes_preserve_cursor_and_wrapping() {
        let mut d = Decoder::new("%3".into());
        let cmd = d.snapshot(80, 24); assert!(cmd.contains("capture-pane -p -e -S - -t %3"));
        let data = b"\x1bP1000p%begin 1 1 0\n%end 1 1 0\n%begin 1 1 1\n%end 1 1 1\n%begin 1 2 1\n80 24 2 0 1 0\n%end 1 2 1\n%begin 1 3 1\nhi\n%end 1 3 1\n%output %3 !\n";
        let out = d.feed(data).unwrap();
        assert_eq!(out.len(), 2); assert!(out[0].ends_with(b"\x1b[?7h")); assert_eq!(out[1], b"!");
    }

    #[test]
    fn reconnect_restores_history_and_keeps_the_live_screen_at_the_bottom() {
        let body = ["oldest question", "earlier answer", "current answer", "prompt", ""].map(str::to_owned);
        let out = String::from_utf8(restore_snapshot("80 3 2 1 1 0", &body).unwrap()).unwrap();
        assert!(out.contains("\x1b[3J\x1b[0moldest question\r\n\x1b[0mearlier answer"));
        assert!(out.contains("\x1b[1;1H\x1b[0mcurrent answer"));
        assert!(out.contains("\x1b[2;1H\x1b[0mprompt"));
        assert!(out.ends_with("\x1b[2;3H\x1b[?25h\x1b[?7h"));
        // Rebuilding an unchanged snapshot is deterministic, with a fresh scrollback reset.
        assert_eq!(restore_snapshot("80 3 2 1 1 0", &body).unwrap(), out.as_bytes());
    }

    #[test]
    fn input_is_targeted_exactly_and_never_interpolated_as_control_commands() {
        let mut d = Decoder::new(raw_pane("%10.3").unwrap());
        let input = "你好\x1b[200~a\n'; kill-session\x1b[201~";
        let cmd = d.input(input);
        assert!(cmd.starts_with("send-keys -t %3 -H ")); assert!(!cmd.contains("kill-session"));
        let hex = cmd.trim().strip_prefix("send-keys -t %3 -H ").unwrap();
        assert_eq!(hex.split(' ').map(|b| u8::from_str_radix(b, 16).unwrap()).collect::<Vec<_>>(), input.as_bytes());
        assert!(raw_pane("%0.1;kill-session").is_none());
        assert!(d.feed(b"%begin 1 1 1\ncommand timed out\n%error 1 1 1\n").is_ok());
        d.input("x");
        assert!(d.feed(b"%begin 1 2 1\ncan't find pane\n%error 1 2 1\n").is_err());
    }
}
