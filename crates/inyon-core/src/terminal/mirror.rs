//! Port of terminal/mirror.ts. A terminal for psmux (tmux on Windows), drawn from the pane's
//! screen rather than from a terminal of its own. psmux runs one server per session, so a view
//! session can't hold another session's window, and Bun has no pseudo-terminal on Windows to attach
//! a client in. So the screen is read with capture-pane whenever it changes and drawn line by line,
//! and what you type goes back through send-keys. While it's open the window takes the phone's
//! size, as a second tmux client would make it, and it goes back to fitting the laptop once the
//! terminal closes.

use std::cell::RefCell;
use std::rc::Rc;

use futures_util::future::LocalBoxFuture;

use crate::hosts::host::{shq, ExecOptions, Host, PtyHandle};
use crate::js::{fmt_number, to_number};
use crate::platform::rt::{clear, set_timeout, spawn, Deferred, Timer};
use crate::re;
use crate::tmux::tmux::{paste_command, paste_text, type_command};

/// xterm's sequences for the keys tmux knows by name.
fn named(seq: &str) -> Option<&'static str> {
    Some(match seq {
        "[A" | "OA" => "Up",
        "[B" | "OB" => "Down",
        "[C" | "OC" => "Right",
        "[D" | "OD" => "Left",
        "[H" | "OH" | "[1~" => "Home",
        "[F" | "OF" | "[4~" => "End",
        "[2~" => "IC",
        "[3~" => "DC",
        "[5~" => "PPage",
        "[6~" => "NPage",
        "[Z" => "BTab",
        "OP" => "F1",
        "OQ" => "F2",
        "OR" => "F3",
        "OS" => "F4",
        "[15~" => "F5",
        "[17~" => "F6",
        "[18~" => "F7",
        "[19~" => "F8",
        "[20~" => "F9",
        "[21~" => "F10",
        "[23~" => "F11",
        "[24~" => "F12",
        _ => return None,
    })
}

/// xterm's modifier numbers (1 + shift 1, alt 2, ctrl 4) as tmux prefixes
fn mods(n: &str) -> Option<&'static str> {
    Some(match n {
        "2" => "S-",
        "3" => "M-",
        "4" => "M-S-",
        "5" => "C-",
        "6" => "C-S-",
        "7" => "C-M-",
        "8" => "C-M-S-",
        _ => return None,
    })
}

/// One piece of what the browser terminal sent, as tmux takes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    Keys(Vec<String>),
    Text(String),
    Paste(String),
    Hex(Vec<String>),
}

/// What the browser terminal sent, as tmux input: printable runs as literal text, keys by name
/// (psmux knows names and -l for sure), a bracketed paste as one paste, and anything else as raw
/// bytes (-H).
pub fn to_input(d: &str) -> Vec<Input> {
    let mut out: Vec<Input> = vec![];
    fn key(out: &mut Vec<Input>, k: String) {
        if let Some(Input::Keys(keys)) = out.last_mut() {
            keys.push(k);
        } else {
            out.push(Input::Keys(vec![k]));
        }
    }
    let hex = |out: &mut Vec<Input>, s: &str| out.push(Input::Hex(s.bytes().map(|b| format!("{b:02x}")).collect()));
    let mut i = 0;
    while i < d.len() {
        let rest = &d[i..];
        let c = rest.chars().next().expect("a character");
        if let Some(body) = rest.strip_prefix("\x1b[200~") {
            match body.find("\x1b[201~") {
                Some(end) => {
                    out.push(Input::Paste(body[..end].to_owned()));
                    i += 6 + end + 6;
                }
                None => {
                    out.push(Input::Paste(body.to_owned()));
                    i = d.len();
                }
            }
            continue;
        }
        if c == '\x1b' {
            let after = &rest[1..];
            if let Some(m) = re!(r"^(?:\[[0-?]*[ -/]*[@-~]|O[A-Za-z])").find(after) {
                let seq = m.as_str();
                // modified keys: ESC [ 1 ; 5 A (Ctrl+Up), ESC [ 3 ; 2 ~ (Shift+Delete)
                let modded = re!(r"^\[(\d*);(\d)([A-Za-z~])$").captures(seq);
                let name = match &modded {
                    Some(m) => named(&if &m[3] == "~" { format!("[{}~", &m[1]) } else { format!("[{}", &m[3]) }),
                    None => named(seq),
                };
                let prefix = modded.as_ref().map(|m| mods(&m[2]));
                match (name, prefix) {
                    (Some(n), None) => key(&mut out, n.to_owned()),
                    (Some(n), Some(Some(p))) => key(&mut out, format!("{p}{n}")),
                    _ => hex(&mut out, &format!("\x1b{seq}")),
                }
                i += 1 + seq.len();
                continue;
            }
            // Alt+key arrives as ESC and the key
            match after.chars().next() {
                Some(next) if (' '..='~').contains(&next) => {
                    key(&mut out, format!("M-{next}"));
                    i += 2;
                }
                _ => {
                    key(&mut out, "Escape".into());
                    i += 1;
                }
            }
            continue;
        }
        let code = c as u32;
        match code {
            13 => key(&mut out, "Enter".into()),
            9 => key(&mut out, "Tab".into()),
            127 | 8 => key(&mut out, "BSpace".into()),
            0 => key(&mut out, "C-Space".into()),
            1..=31 => key(&mut out, format!("C-{}", char::from_u32(code + if code <= 26 { 96 } else { 64 }).expect("ASCII"))),
            _ => {
                let run: usize = rest.char_indices().find(|(_, ch)| (*ch as u32) < 32 || *ch == '\x7f').map(|(j, _)| j).unwrap_or(rest.len());
                let text = &rest[..run];
                if let Some(Input::Text(t)) = out.last_mut() {
                    t.push_str(text);
                } else {
                    out.push(Input::Text(text.to_owned()));
                }
                i += run;
                continue;
            }
        }
        i += c.len_utf8();
    }
    out
}

/// A buffer name no other paste is using (`Math.random().toString(36).slice(2, 10)` in the TS).
fn short_id(n: usize) -> String {
    crate::ids::random_uuid().replace('-', "")[..n].to_owned()
}

/// A batch of input as few commands as it can go in: one shell for the whole batch, keys and text
/// joined with &&, so it stops at the first that fails as one command per piece did. A shell has one
/// stdin, so each piece of text after the first starts the next command. On Windows every program
/// started is slow (sh, psmux, and busybox's mktemp, cat and rm), so a keystroke plus Enter is one
/// shell, not two.
pub fn batch_commands(pane: &str, inputs: &[Input]) -> Vec<(String, Option<String>)> {
    let mut out = Vec::new();
    let mut parts: Vec<String> = Vec::new();
    let mut input: Option<String> = None;
    let close = |parts: &mut Vec<String>, input: &mut Option<String>, out: &mut Vec<(String, Option<String>)>| {
        if parts.is_empty() {
            return;
        }
        let cmd = if parts.len() == 1 {
            parts.remove(0)
        } else {
            parts.drain(..).map(|p| if p.starts_with("umask ") { format!("{{ {p}; }}") } else { p }).collect::<Vec<_>>().join(" && ")
        };
        parts.clear();
        out.push((cmd, input.take()));
    };
    for x in inputs {
        let (cmd, piece) = input_command(pane, x);
        if piece.is_some() && input.is_some() {
            close(&mut parts, &mut input, &mut out);
        }
        parts.push(cmd);
        if piece.is_some() {
            input = piece;
        }
    }
    close(&mut parts, &mut input, &mut out);
    out
}

/// The tmux commands for one piece of input, and what to feed them.
pub fn input_command(pane: &str, x: &Input) -> (String, Option<String>) {
    let t = format!("tmux send-keys -t {}", shq(pane));
    match x {
        Input::Keys(keys) => (format!("{t} {}", keys.iter().map(|k| shq(k)).collect::<Vec<_>>().join(" ")), None),
        // through a buffer, never the command line, where other users could read it
        Input::Text(text) => (type_command(&format!("inyon-term-{}", short_id(8)), pane), Some(text.clone())),
        Input::Hex(hex) => (format!("{t} -H {}", hex.join(" ")), None),
        Input::Paste(p) => (paste_command(&format!("inyon-term-{}", short_id(8)), pane), Some(paste_text(p))),
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cursor {
    pub x: f64,
    pub y: f64,
    pub visible: bool,
}

/// A pane's screen as the mirror reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct Screen {
    pub width: f64,
    pub height: f64,
    pub cursor: Cursor,
    pub lines: Vec<String>,
    /// a full-screen program (the alternate screen), which takes the mouse wheel and clicks
    pub mouse: bool,
}

/// Reads the pane's size, cursor and screen (with colours) in one go.
pub fn read_command(pane: &str) -> String {
    format!(
        "tmux display -p -t {} '#{{pane_width}} #{{pane_height}} #{{cursor_x}} #{{cursor_y}} #{{cursor_flag}} #{{alternate_on}}' && tmux capture-pane -p -e -t {}",
        shq(pane),
        shq(pane)
    )
}

pub fn parse_screen(stdout: &str) -> Option<Screen> {
    let normal = stdout.replace("\r\n", "\n");
    let mut rows = normal.split('\n');
    let head = rows.next().unwrap_or("");
    let nums: Vec<f64> = crate::js::re::split_plain(crate::js::JsStr::jtrim(head), re!(r"\s+")).into_iter().map(to_number).collect();
    let at = |i: usize| nums.get(i).copied().unwrap_or(f64::NAN);
    let (w, h, x, y) = (at(0), at(1), at(2), at(3));
    if !(w > 0.0 && h > 0.0) {
        return None;
    }
    // capture-pane ends with a newline; keep exactly one entry per row
    let n = h as usize;
    let mut lines: Vec<String> = rows.take(n).map(str::to_owned).collect();
    lines.resize(n, String::new());
    let or0 = |v: f64| if v.is_nan() { 0.0 } else { v };
    Some(Screen { width: w, height: h, cursor: Cursor { x: or0(x), y: or0(y), visible: nums.get(4).is_none_or(|f| *f != 0.0) }, lines, mouse: nums.get(5) == Some(&1.0) })
}

/// What to write to the browser terminal to turn the last screen into this one: only the lines
/// that changed, each drawn from its first column with wrapping off, so a pane wider than the
/// phone is cut at the edge instead of spilling onto the next row.
pub fn draw_screen(prev: Option<&Screen>, next: &Screen) -> String {
    // bracketed paste on, so a pasted block arrives as one paste rather than lines that each submit
    let mut out = if prev.is_some() { String::new() } else { "\x1b[?7l\x1b[?2004h\x1b[0m\x1b[H\x1b[2J".to_owned() };
    // a full-screen program (Claude Code's, say) scrolls with the wheel: the browser terminal reports
    // the mouse while one runs, and those reports go to it as they are (psmux passes them on); a full
    // redraw (after a resize, say) says the mode again, since the browser terminal may still have it on
    if prev.is_none_or(|p| p.mouse != next.mouse) {
        out.push_str(if next.mouse { "\x1b[?1000h\x1b[?1006h" } else { "\x1b[?1000l\x1b[?1006l" });
    }
    for (i, line) in next.lines.iter().enumerate() {
        if prev.is_some_and(|p| p.lines.get(i) == Some(line) && p.width == next.width) {
            continue;
        }
        out.push_str(&format!("\x1b[{};1H\x1b[0m{line}\x1b[0m\x1b[K", i + 1));
    }
    // rows below a pane that got shorter
    if prev.is_some_and(|p| p.lines.len() > next.lines.len()) {
        out.push_str(&format!("\x1b[{};1H\x1b[0m\x1b[J", next.lines.len() + 1));
    }
    let c = &next.cursor;
    let moved = prev.is_none_or(|p| p.cursor != *c);
    if !out.is_empty() || moved {
        out.push_str(&format!("\x1b[{};{}H{}", fmt_number(c.y + 1.0), fmt_number(c.x + 1.0), if c.visible { "\x1b[?25h" } else { "\x1b[?25l" }));
    }
    out
}

/// Polling slows down while the screen stays the same, and speeds up the moment it changes.
const FAST_MS: u64 = 120;
const SLOW_MS: u64 = 1000;

/// How the mirror hears that the pane printed something: given the callback, it returns the
/// unsubscribe.
pub type OnOutput = Box<dyn FnOnce(Rc<dyn Fn()>) -> Box<dyn Fn()>>;

struct State {
    shown: Option<Screen>,
    closed: bool,
    size: (u16, u16),
    resized: bool,
    reading: bool,
    again: bool,
    delay: u64,
    timer: Option<Timer>,
    pending: String,
    writing: bool,
    off: Option<Box<dyn Fn()>>,
}

struct Mirror {
    host: Host,
    pane: String,
    p: String,
    st: RefCell<State>,
    on_data: RefCell<Box<dyn FnMut(Vec<u8>)>>,
    exited: Deferred<i32>,
}

fn close(m: &Rc<Mirror>, code: i32) {
    let (resized, off) = {
        let mut st = m.st.borrow_mut();
        if st.closed {
            return;
        }
        st.closed = true;
        clear(&mut st.timer);
        (st.resized, st.off.take())
    };
    if let Some(off) = off {
        off();
    }
    // let the window fit its own clients again
    if resized {
        let (host, p) = (m.host.clone(), m.p.clone());
        spawn(async move {
            let _ = host.run(&format!("tmux resize-window -t {p} -A 2>/dev/null; tmux set-option -w -t {p} -u window-size 2>/dev/null; true")).await;
        });
    }
    m.exited.resolve(code);
}

async fn fit(m: &Rc<Mirror>) {
    let (cols, rows) = {
        let mut st = m.st.borrow_mut();
        st.resized = true;
        st.size
    };
    let _ = m.host.run(&format!("tmux resize-window -t {} -x {cols} -y {rows}", m.p)).await;
}

fn schedule(m: &Rc<Mirror>, ms: u64) {
    let mut st = m.st.borrow_mut();
    if st.closed {
        return;
    }
    clear(&mut st.timer);
    let w = Rc::downgrade(m);
    st.timer = Some(set_timeout(ms, move || {
        if let Some(m) = w.upgrade() {
            spawn(async move { read(&m).await });
        }
    }));
}

async fn read(m: &Rc<Mirror>) {
    {
        let mut st = m.st.borrow_mut();
        if st.closed {
            return;
        }
        if st.reading {
            st.again = true;
            return;
        }
        st.reading = true;
    }
    match m.host.exec(&read_command(&m.pane), ExecOptions::timeout(10_000)).await {
        Ok(r) if !m.st.borrow().closed => match parse_screen(&r.stdout) {
            // the pane is gone (or the machine): end the terminal like a detach would
            None => {
                if r.code != 0 {
                    close(m, r.code);
                }
            }
            Some(next) => {
                let d = {
                    let mut st = m.st.borrow_mut();
                    let base = st.shown.as_ref().filter(|s| s.width == next.width && s.height == next.height);
                    let d = draw_screen(base, &next);
                    st.shown = Some(next);
                    st.delay = if d.is_empty() { SLOW_MS.min(st.delay * 2) } else { FAST_MS };
                    d
                };
                if !d.is_empty() {
                    (m.on_data.borrow_mut())(d.into_bytes());
                }
            }
        },
        Ok(_) => {}
        Err(_) => m.st.borrow_mut().delay = SLOW_MS,
    }
    let (again, delay) = {
        let mut st = m.st.borrow_mut();
        st.reading = false;
        (std::mem::take(&mut st.again), st.delay)
    };
    schedule(m, if again { 0 } else { delay });
}

/// Keystrokes go out in order, and whatever piled up during one round trip goes as one batch.
fn flush(m: Rc<Mirror>) -> LocalBoxFuture<'static, ()> {
    Box::pin(async move {
        let d = {
            let mut st = m.st.borrow_mut();
            if st.writing || st.closed || st.pending.is_empty() {
                return;
            }
            st.writing = true;
            std::mem::take(&mut st.pending)
        };
        for (cmd, input) in batch_commands(&m.pane, &to_input(&d)) {
            let opts = input.map(ExecOptions::input).unwrap_or_default();
            if m.host.exec(&cmd, opts).await.is_err() {
                break;
            }
        }
        let more = {
            let mut st = m.st.borrow_mut();
            st.writing = false;
            st.delay = FAST_MS;
            !st.pending.is_empty()
        };
        schedule(&m, 30);
        if more {
            spawn(flush(m));
        }
    })
}

/// Opens the mirror on a pane: `cols`×`rows` is the phone's terminal, `on_data` gets what to draw.
pub fn open_mirror(host: Host, pane_id: &str, cols: u16, rows: u16, on_data: impl FnMut(Vec<u8>) + 'static, on_output: Option<OnOutput>) -> PtyHandle {
    let m = Rc::new(Mirror {
        host,
        pane: pane_id.to_owned(),
        p: shq(pane_id),
        st: RefCell::new(State {
            shown: None,
            closed: false,
            size: (cols, rows),
            resized: false,
            reading: false,
            again: false,
            delay: FAST_MS,
            timer: None,
            pending: String::new(),
            writing: false,
            off: None,
        }),
        on_data: RefCell::new(Box::new(on_data)),
        exited: Deferred::new(),
    });
    if let Some(sub) = on_output {
        let w = Rc::downgrade(&m);
        let off = sub(Rc::new(move || {
            let Some(m) = w.upgrade() else { return };
            let reading = {
                let mut st = m.st.borrow_mut();
                st.delay = FAST_MS;
                st.reading
            };
            if !reading {
                schedule(&m, 40);
            }
        }));
        m.st.borrow_mut().off = Some(off);
    }
    let first = m.clone();
    spawn(async move {
        fit(&first).await;
        schedule(&first, 0);
    });
    let (wm, rm, km) = (m.clone(), m.clone(), m.clone());
    PtyHandle::new(
        move |d| {
            wm.st.borrow_mut().pending.push_str(d);
            spawn(flush(wm.clone()));
        },
        move |c, r| {
            {
                let mut st = rm.st.borrow_mut();
                if (c, r) == st.size {
                    return;
                }
                st.size = (c, r);
            }
            let m = rm.clone();
            spawn(async move {
                fit(&m).await;
                m.st.borrow_mut().shown = None;
                schedule(&m, 0);
            });
        },
        move || close(&km, 0),
        m.exited.clone(),
    )
}

#[cfg(test)]
#[path = "mirror_tests.rs"]
mod tests;
