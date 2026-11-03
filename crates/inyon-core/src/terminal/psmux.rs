//! PSmux terminal bytes over one persistent control connection, pinned to one pane.
//! Capture only on attach/resize; normal output is pushed, and keys never start a process.
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::rc::Rc;

use crate::hosts::host::{Host, PtyHandle};
use crate::hosts::http_error::{http, HubResult};
use crate::platform::rt::spawn;
use super::mirror::{draw_screen, parse_screen};

#[derive(Clone, Copy, Debug, PartialEq)]
enum Reply { Ignore, Geometry, Capture, Input }

struct Decoder {
    pane: String,
    pending: Vec<u8>,
    replies: VecDeque<Reply>,
    frame: Option<(Reply, Vec<String>)>,
    geometry: String,
    ready: bool,
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

impl Decoder {
    fn new(pane: String) -> Self {
        Self { pane, pending: Vec::new(), replies: VecDeque::new(), frame: None, geometry: String::new(), ready: false }
    }

    fn snapshot(&mut self, cols: u16, rows: u16) -> String {
        self.ready = false;
        self.replies.extend([Reply::Ignore, Reply::Geometry, Reply::Capture]);
        let p = &self.pane;
        format!("resize-window -t {p} -x {cols} -y {rows}\ndisplay-message -p -t {p} '#{{pane_width}} #{{pane_height}} #{{cursor_x}} #{{cursor_y}} #{{cursor_flag}} #{{alternate_on}}'\ncapture-pane -p -e -t {p}\n")
    }

    fn input(&mut self, text: &str) -> String {
        let mut commands = String::new();
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
                        // PSmux 3.4's SendBytes dispatch drops its response sender after enqueueing
                        // input. The control handler mislabels that immediate channel closure as a
                        // timeout. Treat only that known write-only response as fire-and-forget.
                        if kind == Reply::Input && body == ["command timed out"] { continue; }
                        return Err(body.join(" "));
                    }
                    match kind {
                        Reply::Geometry => self.geometry = body.join("\n"),
                        Reply::Capture => {
                            let screen = parse_screen(&format!("{}\n{}\n", self.geometry, body.join("\n"))).ok_or("Invalid PSmux screen")?;
                            output.push(format!("{}\x1b[?7h", draw_screen(None, &screen)).into_bytes());
                            self.ready = true;
                        }
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
    let closed = Rc::new(Cell::new(false));
    let handle: Rc<RefCell<Option<PtyHandle>>> = Rc::new(RefCell::new(None));
    let (d, c, h) = (decoder.clone(), closed.clone(), handle.clone());
    let cmd = format!("inyon-psmux-control:{}", serde_json::to_string(session).expect("session string"));
    let pipe = host.pty(&cmd, cols, rows, move |bytes| {
        if c.get() { return; }
        let result = d.borrow_mut().feed(&bytes);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fragmented_output_is_decoded_and_other_panes_are_ignored() {
        let mut d = Decoder::new("%2".into()); d.ready = true;
        assert!(d.feed(b"%output %1 wrong\n%output %2 \\344\\275").unwrap().is_empty());
        assert_eq!(d.feed(b"\\240\\345\\245\\275\\015\\012\\\\\\134\n").unwrap().concat(), "你好\r\n\\\\".as_bytes());
    }

    #[test]
    fn attach_snapshot_then_raw_bytes_preserve_cursor_and_wrapping() {
        let mut d = Decoder::new("%3".into());
        let cmd = d.snapshot(80, 24); assert!(cmd.contains("capture-pane -p -e -t %3"));
        let data = b"\x1bP1000p%begin 1 1 0\n%end 1 1 0\n%begin 1 1 1\n%end 1 1 1\n%begin 1 2 1\n80 24 2 0 1 0\n%end 1 2 1\n%begin 1 3 1\nhi\n%end 1 3 1\n%output %3 !\n";
        let out = d.feed(data).unwrap();
        assert_eq!(out.len(), 2); assert!(out[0].ends_with(b"\x1b[?7h")); assert_eq!(out[1], b"!");
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
