//! Control-mode lines in, events out. No I/O here.

use crate::escape::unescape_output;

/// The answer to one command: the lines between `%begin` and `%end` (or `%error`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    /// tmux's own command number (not ours; replies come back in the order commands were sent)
    pub number: u64,
    /// tmux sets 1 for commands this client sent and 0 for the command that started it
    /// (`attach-session`, `new-session`)
    pub flags: u32,
    pub ok: bool,
    pub lines: Vec<Vec<u8>>,
}

impl Reply {
    pub fn text(&self) -> String {
        self.lines.iter().map(|l| String::from_utf8_lossy(l)).collect::<Vec<_>>().join("\n")
    }
}

/// The notifications Inyon reacts to; the rest are kept as [`Notification::Other`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Notification {
    /// `%output %3 data`, already decoded to the pane's bytes
    Output {
        pane: String,
        data: Vec<u8>,
    },
    /// `%extended-output %3 age ... : data`, sent instead of `%output` once `pause-after` is set
    ExtendedOutput {
        pane: String,
        age_ms: u64,
        data: Vec<u8>,
    },
    /// `%pause %3`: tmux stopped sending this pane's output (pause-after, tmux 3.2+)
    Pause {
        pane: String,
    },
    Continue {
        pane: String,
    },
    /// `%exit [reason]`: the client is being detached; EOF follows
    Exit {
        reason: Option<String>,
    },
    LayoutChange {
        window: String,
        layout: String,
    },
    WindowAdd {
        window: String,
    },
    WindowClose {
        window: String,
    },
    /// the same three for a window outside the client's session; a window that is killed is
    /// reported as `%unlinked-window-close`, since it has left the session by then
    UnlinkedWindowAdd {
        window: String,
    },
    UnlinkedWindowClose {
        window: String,
    },
    UnlinkedWindowRenamed {
        window: String,
        name: String,
    },
    WindowRenamed {
        window: String,
        name: String,
    },
    WindowPaneChanged {
        window: String,
        pane: String,
    },
    SessionChanged {
        session: String,
        name: String,
    },
    SessionRenamed {
        name: String,
    },
    SessionsChanged,
    PaneModeChanged {
        pane: String,
    },
    /// `%subscription-changed name $s @w idx %p ... : value` (refresh-client -B, tmux 3.2+)
    SubscriptionChanged {
        name: String,
        pane: String,
        value: String,
    },
    Other(String),
}

impl Notification {
    /// The events after which Inyon re-reads the session tree: the same list as `live.ts`.
    pub fn is_structure(&self) -> bool {
        matches!(
            self,
            Self::LayoutChange { .. }
                | Self::WindowAdd { .. }
                | Self::WindowClose { .. }
                | Self::UnlinkedWindowAdd { .. }
                | Self::UnlinkedWindowClose { .. }
                | Self::UnlinkedWindowRenamed { .. }
                | Self::WindowRenamed { .. }
                | Self::WindowPaneChanged { .. }
                | Self::SessionRenamed { .. }
                | Self::SessionsChanged
                | Self::PaneModeChanged { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Reply(Reply),
    Notification(Notification),
    /// A line outside any block that isn't a notification: psmux's `-C` echoes each command back
    Stray(Vec<u8>),
}

#[derive(Default)]
pub struct Parser {
    block: Option<(Vec<u8>, Reply)>,
}

/// `%begin 1700000000 12 1` → (`1700000000 12`, 12, 1)
fn guard(rest: &[u8]) -> Option<(Vec<u8>, u64, u32)> {
    let s = std::str::from_utf8(rest).ok()?;
    let mut it = s.split(' ');
    let (time, number, flags) = (it.next()?, it.next()?, it.next().unwrap_or("0"));
    let number = number.parse().ok()?;
    Some((format!("{time} {number}").into_bytes(), number, flags.trim().parse().unwrap_or(0)))
}

fn split2(s: &str) -> (&str, &str) {
    s.split_once(' ').unwrap_or((s, ""))
}

impl Parser {
    pub fn new() -> Self {
        Self::default()
    }

    /// True while inside a `%begin` block.
    pub fn in_block(&self) -> bool {
        self.block.is_some()
    }

    /// Feeds one line (without its `\n`). Returns an event when the line completes one.
    pub fn line(&mut self, mut line: &[u8]) -> Option<Event> {
        if let [rest @ .., b'\r'] = line {
            line = rest;
        }
        if let Some((key, reply)) = &mut self.block {
            // only the guard with the same time and number ends the block: a captured screen can
            // hold a line that starts with "%end" too
            for (tag, ok) in [(&b"%end "[..], true), (&b"%error "[..], false)] {
                if let Some(rest) = line.strip_prefix(tag) {
                    if guard(rest).is_some_and(|(k, _, _)| &k == key) {
                        let (_, mut reply) = self.block.take().unwrap();
                        reply.ok = ok;
                        return Some(Event::Reply(reply));
                    }
                }
            }
            // tmux writes %continue (and %pause) from inside the block of the refresh-client -A
            // that caused it, despite the manual saying notifications never come inside one.
            // Only that exact shape is taken out; a captured row could start with it, but it
            // would have to be the whole row.
            if is_flow_line(line) {
                return Some(Event::Notification(notification(line)));
            }
            reply.lines.push(line.to_vec());
            return None;
        }
        if let Some(rest) = line.strip_prefix(b"%begin ") {
            if let Some((key, number, flags)) = guard(rest) {
                self.block = Some((key, Reply { number, flags, ok: true, lines: Vec::new() }));
                return None;
            }
        }
        if line.first() != Some(&b'%') {
            return Some(Event::Stray(line.to_vec()));
        }
        Some(Event::Notification(notification(line)))
    }
}

/// `%pause %12` or `%continue %12`, nothing more
fn is_flow_line(line: &[u8]) -> bool {
    let rest = line.strip_prefix(b"%pause %").or_else(|| line.strip_prefix(b"%continue %"));
    rest.is_some_and(|r| !r.is_empty() && r.iter().all(u8::is_ascii_digit))
}

fn notification(line: &[u8]) -> Notification {
    // %output's data is the only part that can hold raw non-UTF-8 bytes; take it before decoding
    if let Some(rest) = line.strip_prefix(b"%output ") {
        let sp = rest.iter().position(|&b| b == b' ').unwrap_or(rest.len());
        let data = rest.get(sp + 1..).unwrap_or_default();
        return Notification::Output { pane: String::from_utf8_lossy(&rest[..sp]).into_owned(), data: unescape_output(data) };
    }
    if let Some(rest) = line.strip_prefix(b"%extended-output ") {
        // %extended-output %3 <age> [reserved ...] : <data>
        if let Some(colon) = rest.windows(3).position(|w| w == b" : ") {
            let head = String::from_utf8_lossy(&rest[..colon]);
            let mut it = head.split(' ');
            let pane = it.next().unwrap_or_default().to_string();
            let age_ms = it.next().and_then(|a| a.parse().ok()).unwrap_or(0);
            return Notification::ExtendedOutput { pane, age_ms, data: unescape_output(&rest[colon + 3..]) };
        }
    }
    let s = String::from_utf8_lossy(line).into_owned();
    let (name, rest) = split2(&s);
    let (a, b) = split2(rest);
    let (a, b) = (a.to_string(), b.to_string());
    match name {
        "%pause" => Notification::Pause { pane: a },
        "%continue" => Notification::Continue { pane: a },
        "%exit" => Notification::Exit { reason: (!rest.is_empty()).then(|| rest.to_string()) },
        "%layout-change" => Notification::LayoutChange { window: a, layout: split2(&b).0.to_string() },
        "%window-add" => Notification::WindowAdd { window: a },
        "%window-close" => Notification::WindowClose { window: a },
        "%window-renamed" => Notification::WindowRenamed { window: a, name: b },
        "%unlinked-window-add" => Notification::UnlinkedWindowAdd { window: a },
        "%unlinked-window-close" => Notification::UnlinkedWindowClose { window: a },
        "%unlinked-window-renamed" => Notification::UnlinkedWindowRenamed { window: a, name: b },
        "%window-pane-changed" => Notification::WindowPaneChanged { window: a, pane: b },
        "%session-changed" => Notification::SessionChanged { session: a, name: b },
        "%session-renamed" => Notification::SessionRenamed { name: rest.to_string() },
        "%sessions-changed" => Notification::SessionsChanged,
        "%pane-mode-changed" => Notification::PaneModeChanged { pane: a },
        "%subscription-changed" => {
            // name $1 @2 3 %4 ... : value
            let (head, value) = rest.split_once(" : ").unwrap_or((rest, ""));
            let pane = head.split(' ').find(|w| w.starts_with('%')).unwrap_or_default().to_string();
            Notification::SubscriptionChanged { name: a, pane, value: value.to_string() }
        }
        _ => Notification::Other(s),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(p: &mut Parser, lines: &[&[u8]]) -> Vec<Event> {
        lines.iter().filter_map(|l| p.line(l)).collect()
    }

    #[test]
    fn reads_reply_blocks() {
        let mut p = Parser::new();
        let ev = feed(&mut p, &[b"%begin 1700000000 7 1", b"main: 1 windows", b"%end 1700000000 7 1"]);
        assert_eq!(ev, vec![Event::Reply(Reply { number: 7, flags: 1, ok: true, lines: vec![b"main: 1 windows".to_vec()] })]);
        let ev = feed(&mut p, &[b"%begin 1700000001 8 1", b"unknown command: nope", b"%error 1700000001 8 1"]);
        assert!(matches!(&ev[..], [Event::Reply(Reply { ok: false, number: 8, .. })]));
    }

    #[test]
    fn a_captured_end_line_does_not_close_the_block() {
        let mut p = Parser::new();
        let ev = feed(&mut p, &[b"%begin 5 2 1", b"%end 4 1 1", b"%error 5 3 1", b"%end 5 2 1"]);
        let Event::Reply(r) = &ev[0] else { panic!() };
        assert_eq!(r.lines, vec![b"%end 4 1 1".to_vec(), b"%error 5 3 1".to_vec()]);
        assert!(!p.in_block());
    }

    #[test]
    fn decodes_output() {
        let mut p = Parser::new();
        let ev = p.line(b"%output %12 hi\\015\\012\\033[1m\xc3\xb8").unwrap();
        assert_eq!(ev, Event::Notification(Notification::Output { pane: "%12".into(), data: b"hi\r\n\x1b[1m\xc3\xb8".to_vec() }));
        // empty output (tmux never sends it, but psmux might)
        assert_eq!(p.line(b"%output %1").unwrap(), Event::Notification(Notification::Output { pane: "%1".into(), data: vec![] }));
    }

    #[test]
    fn decodes_extended_output() {
        let mut p = Parser::new();
        let ev = p.line(b"%extended-output %3 1520 : x\\040 : y").unwrap();
        assert_eq!(ev, Event::Notification(Notification::ExtendedOutput { pane: "%3".into(), age_ms: 1520, data: b"x  : y".to_vec() }));
    }

    #[test]
    fn reads_structure_and_exit() {
        let mut p = Parser::new();
        let n = |p: &mut Parser, l: &[u8]| match p.line(l) {
            Some(Event::Notification(n)) => n,
            e => panic!("{e:?}"),
        };
        assert_eq!(n(&mut p, b"%window-add @4"), Notification::WindowAdd { window: "@4".into() });
        assert_eq!(n(&mut p, b"%window-renamed @4 my build"), Notification::WindowRenamed { window: "@4".into(), name: "my build".into() });
        assert_eq!(n(&mut p, b"%layout-change @1 b25d,80x24,0,0,1 b25d,80x24,0,0,1 *"), Notification::LayoutChange { window: "@1".into(), layout: "b25d,80x24,0,0,1".into() });
        assert_eq!(n(&mut p, b"%exit"), Notification::Exit { reason: None });
        assert_eq!(n(&mut p, b"%exit too far behind"), Notification::Exit { reason: Some("too far behind".into()) });
        assert!(n(&mut p, b"%unlinked-window-add @9").is_structure());
        assert!(!n(&mut p, b"%client-detached /dev/pts/3").is_structure());
        assert_eq!(n(&mut p, b"%subscription-changed cmd $0 @1 0 %1 : vim"), Notification::SubscriptionChanged { name: "cmd".into(), pane: "%1".into(), value: "vim".into() });
    }

    #[test]
    fn continue_inside_a_block_is_a_notification() {
        let mut p = Parser::new();
        assert_eq!(p.line(b"%begin 9 285 1"), None);
        assert_eq!(p.line(b"%continue %1"), Some(Event::Notification(Notification::Continue { pane: "%1".into() })));
        assert_eq!(p.line(b"%continue %1 and more"), None);
        let Some(Event::Reply(r)) = p.line(b"%end 9 285 1") else { panic!() };
        assert_eq!(r.lines, vec![b"%continue %1 and more".to_vec()]);
    }

    #[test]
    fn echoed_commands_are_stray() {
        let mut p = Parser::new();
        assert_eq!(p.line(b"list-windows"), Some(Event::Stray(b"list-windows".to_vec())));
        // psmux -CC writes ST after %exit
        assert!(matches!(p.line(b"\x1b\\"), Some(Event::Stray(_))));
    }
}
