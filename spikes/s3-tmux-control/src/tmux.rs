//! The tmux commands Inyon runs (`apps/hub/src/tmux/tmux.ts`), sent over a control connection
//! instead of one shell round trip each.

use std::ffi::OsStr;
use std::io;
use std::path::PathBuf;
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;

use crate::client::{Control, Error};
use crate::escape::quote;
use crate::proto::Notification;

/// tmux's version as `tmux -V` prints it: `tmux 3.0a`, `tmux 3.6`, `tmux next-3.7`, `psmux 3.4.0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
}

impl Version {
    pub fn parse(s: &str) -> Option<Self> {
        // the word after the program's name: "psmux 3.4.0 (20aa1d7 2026-10-13)" has more after it
        let v = s.split_whitespace().nth(1)?;
        let v = v.trim_start_matches(|c: char| !c.is_ascii_digit());
        let mut it = v.split(|c: char| !c.is_ascii_digit());
        Some(Version { major: it.next()?.parse().ok()?, minor: it.next().and_then(|m| m.parse().ok()).unwrap_or(0) })
    }

    /// `attach-session -f` (client flags: read-only, ignore-size, pause-after...) arrived in 3.2.
    pub fn has_client_flags(self) -> bool {
        self >= Version { major: 3, minor: 2 }
    }
}

/// How to start tmux: which binary, which server.
#[derive(Debug, Clone)]
pub struct Tmux {
    pub bin: PathBuf,
    /// `-L name`: a private server, so tests never touch a real one
    pub socket: Option<String>,
    /// `-f file`; `/dev/null` keeps the user's config out
    pub config: Option<PathBuf>,
    /// add `-u` (UTF-8 whatever the locale), as the hub does for the real tmux; not for psmux
    pub utf8: bool,
}

impl Tmux {
    pub fn new(bin: impl Into<PathBuf>) -> Self {
        Tmux { bin: bin.into(), socket: None, config: None, utf8: true }
    }

    pub fn command(&self) -> Command {
        let mut c = Command::new(&self.bin);
        // a tmux started from inside tmux would otherwise talk to the outer server
        c.env_remove("TMUX");
        if let Some(s) = &self.socket {
            c.args(["-L", s]);
        }
        if let Some(f) = &self.config {
            c.arg("-f").arg(f);
        }
        if self.utf8 {
            c.arg("-u");
        }
        c
    }

    pub fn run<I: IntoIterator<Item = S>, S: AsRef<OsStr>>(&self, args: I) -> io::Result<Output> {
        self.command().args(args).output()
    }

    pub fn version(&self) -> Option<Version> {
        let out = Command::new(&self.bin).arg("-V").output().ok()?;
        Version::parse(&String::from_utf8_lossy(&out.stdout))
    }

    /// Attaches a control client to a session and waits until tmux has attached it.
    /// `ignore-size` keeps it out of window sizing; tmux before 3.2 has no client flags, so there
    /// only `-r` is passed for a read-only client (those versions don't size windows from a
    /// control client anyway).
    pub fn attach(&self, session: &str, read_only: bool) -> io::Result<(Control, Receiver<Notification>)> {
        let v = self.version().unwrap_or(Version { major: 3, minor: 2 });
        let mut c = self.command();
        c.args(attach_args(v, session, read_only));
        let (c, rx) = Control::spawn(c)?;
        if !c.wait_ready(std::time::Duration::from_secs(5)) {
            return Err(io::Error::other(format!("could not attach to {session}: {}", c.exit_reason().unwrap_or_default())));
        }
        Ok((c, rx))
    }

    /// The size of a session's active window as `WxH`, read with a one-off tmux command.
    pub fn window_size(&self, session: &str) -> Option<String> {
        let out = self.run(["list-windows", "-t", session, "-F", "#{window_active} #{window_width}x#{window_height}"]).ok()?;
        let text = String::from_utf8_lossy(&out.stdout).into_owned();
        text.lines().find_map(|l| l.strip_prefix("1 ")).map(|s| s.trim().to_string()).filter(|s| s.contains('x'))
    }
}

pub fn attach_args(v: Version, session: &str, read_only: bool) -> Vec<String> {
    let mut a = vec!["-C".to_string(), "attach-session".into(), "-t".into(), session.into()];
    if v.has_client_flags() {
        a.push("-f".into());
        a.push(if read_only { "read-only,ignore-size" } else { "ignore-size" }.into());
    } else if read_only {
        a.push("-r".into());
    }
    a
}

const SEP: char = '\u{1f}';
/// The hub's PANE_FORMAT (tmux.ts), field for field.
pub const PANE_FIELDS: [&str; 18] = [
    "session_id",
    "session_name",
    "session_attached",
    "window_id",
    "window_index",
    "window_name",
    "window_active",
    "window_activity",
    "pane_id",
    "pane_index",
    "pane_active",
    "pane_pid",
    "pane_current_command",
    "pane_current_path",
    "pane_title",
    "pane_width",
    "pane_height",
    "pane_dead",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pane {
    pub session_id: String,
    pub session_name: String,
    pub window_id: String,
    pub window_index: u32,
    pub window_name: String,
    pub window_active: bool,
    pub pane_id: String,
    pub pane_index: u32,
    pub active: bool,
    pub pid: u32,
    pub command: String,
    pub cwd: String,
    pub title: String,
    pub width: u32,
    pub height: u32,
    pub dead: bool,
}

/// Splits one line of PANE_FORMAT output. tmux 3.4 and older can write the separator as the
/// four characters `\037` (and a backslash as `\\`); those lines are unescaped first, as the hub's
/// `tmuxFields` does.
pub fn fields(line: &str) -> Vec<String> {
    let mut line = line.to_string();
    if !line.contains(SEP) && line.contains("\\037") {
        let mut out = String::new();
        let mut it = line.chars().peekable();
        while let Some(c) = it.next() {
            if c != '\\' {
                out.push(c);
                continue;
            }
            if it.peek() == Some(&'\\') {
                it.next();
                out.push('\\');
                continue;
            }
            let digits: String = it.clone().take(3).collect();
            match (digits.len() == 3).then(|| u8::from_str_radix(&digits, 8).ok()).flatten() {
                Some(b) => {
                    out.push(b as char);
                    it.nth(2);
                }
                None => out.push('\\'),
            }
        }
        line = out;
    }
    line.split(SEP).map(str::to_string).collect()
}

pub fn pane_format() -> String {
    PANE_FIELDS.iter().map(|f| format!("#{{{f}}}")).collect::<Vec<_>>().join(&SEP.to_string())
}

/// Every pane of every session, in the hub's shape.
pub fn list_panes(c: &Control) -> Result<Vec<Pane>, Error> {
    // single quotes: the parser takes the separator byte as it is
    let r = c.cmd(format!("list-panes -a -F '{}'", pane_format()))?;
    Ok(r.lines.iter().filter_map(|l| parse_pane(&String::from_utf8_lossy(l))).collect())
}

pub fn parse_pane(line: &str) -> Option<Pane> {
    let f = fields(line);
    if f.len() < PANE_FIELDS.len() {
        return None;
    }
    let n = |i: usize| f[i].parse().unwrap_or(0);
    Some(Pane {
        session_id: f[0].clone(),
        session_name: f[1].clone(),
        window_id: f[3].clone(),
        window_index: n(4),
        window_name: f[5].clone(),
        window_active: f[6] == "1",
        pane_id: f[8].clone(),
        pane_index: n(9),
        active: f[10] == "1",
        pid: n(11),
        command: f[12].clone(),
        cwd: f[13].clone(),
        title: f[14].clone(),
        width: n(15),
        height: n(16),
        dead: f[17] == "1",
    })
}

/// The visible screen of a pane with its styling (`capture-pane -p -e`), one entry per row.
pub fn capture(c: &Control, pane: &str) -> Result<Vec<Vec<u8>>, Error> {
    Ok(c.cmd(format!("capture-pane -p -e -t {}", quote(pane)))?.lines)
}

/// Several panes in one write (the hub's capturePanes does this in one shell round trip).
pub fn capture_many(c: &Control, panes: &[&str]) -> Vec<Result<Vec<Vec<u8>>, Error>> {
    let cmds: Vec<String> = panes.iter().map(|p| format!("capture-pane -p -e -t {}", quote(p))).collect();
    c.cmds(&cmds).into_iter().map(|r| r.map(|r| r.lines)).collect()
}

/// The key names the hub allows (tmux.ts KEY_RE), plus IC and DC, which the psmux mirror sends.
pub fn key_allowed(k: &str) -> bool {
    const NAMED: [&str; 18] =
        ["Enter", "Escape", "Tab", "BTab", "Up", "Down", "Left", "Right", "S-Left", "S-Right", "Space", "BSpace", "Home", "End", "PageUp", "PageDown", "IC", "DC"];
    let alnum = |s: &str| s.len() == 1 && s.chars().all(|c| c.is_ascii_alphanumeric());
    if alnum(k) || NAMED.contains(&k) {
        return true;
    }
    if let Some(n) = k.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
        return (1..=12).contains(&n) && !k[1..].starts_with('0');
    }
    ["C-M-", "C-", "M-"].iter().any(|p| k.strip_prefix(p).is_some_and(alnum))
}

pub fn send_keys(c: &Control, pane: &str, keys: &[&str]) -> Result<(), Error> {
    if let Some(k) = keys.iter().find(|k| !key_allowed(k)) {
        return Err(Error::Command(format!("key not allowed: {k}")));
    }
    c.cmd(format!("send-keys -t {} {}", quote(pane), keys.join(" "))).map(|_| ())
}

/// Types text as keystrokes (`send-keys -l`); newlines become C-j, as in the hub's typeText.
pub fn type_text(c: &Control, pane: &str, text: &str) -> Result<(), Error> {
    let p = quote(pane);
    let mut cmds = Vec::new();
    for (i, line) in text.replace("\r\n", "\n").replace('\r', "\n").split('\n').enumerate() {
        if i > 0 {
            cmds.push(format!("send-keys -t {p} C-j"));
        }
        for chunk in chunks(line, 2000) {
            cmds.push(format!("send-keys -t {p} -l -- {}", quote(chunk)));
        }
    }
    c.cmds(&cmds).into_iter().collect::<Result<Vec<_>, _>>().map(|_| ())
}

static PASTES: AtomicU64 = AtomicU64::new(0);

/// Pastes text into a pane as one bracketed paste: the text goes into a named buffer in quoted
/// chunks (set-buffer, then set-buffer -a), and paste-buffer -p wraps it in ESC[200~ ... ESC[201~
/// when the program asked for bracketed paste. No temp file and no shell, unlike the hub's
/// pasteCommand.
pub fn paste(c: &Control, pane: &str, text: &str) -> Result<(), Error> {
    let text = text.replace("\r\n", "\n").replace('\r', "\n");
    let buf = format!("inyon-{}-{}", std::process::id(), PASTES.fetch_add(1, Ordering::Relaxed));
    let mut cmds = Vec::new();
    for (i, chunk) in chunks(&text, 8192).enumerate() {
        let append = if i == 0 { "" } else { "-a " };
        cmds.push(format!("set-buffer {append}-b {buf} -- {}", quote(chunk)));
    }
    if cmds.is_empty() {
        return Ok(());
    }
    cmds.push(format!("paste-buffer -d -p -b {buf} -t {}", quote(pane)));
    let results = c.cmds(&cmds);
    if let Some(e) = results.into_iter().find_map(|r| r.err()) {
        let _ = c.cmd(format!("delete-buffer -b {buf}"));
        return Err(e);
    }
    Ok(())
}

/// Sets a window's size, as the psmux mirror does while a phone terminal is open.
pub fn resize_window(c: &Control, target: &str, cols: u32, rows: u32) -> Result<(), Error> {
    c.cmd(format!("resize-window -t {} -x {cols} -y {rows}", quote(target))).map(|_| ())
}

pub fn resize_pane(c: &Control, pane: &str, cols: u32, rows: u32) -> Result<(), Error> {
    c.cmd(format!("resize-pane -t {} -x {cols} -y {rows}", quote(pane))).map(|_| ())
}

/// Splits on char boundaries into pieces of at most `max` bytes.
fn chunks(s: &str, max: usize) -> impl Iterator<Item = &str> {
    let mut rest = s;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }
        let mut end = rest.len().min(max);
        while !rest.is_char_boundary(end) {
            end -= 1;
        }
        let (a, b) = rest.split_at(end);
        rest = b;
        Some(a)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_versions() {
        let v = |s| Version::parse(s).unwrap();
        assert_eq!(v("tmux 3.0a"), Version { major: 3, minor: 0 });
        assert_eq!(v("tmux 3.6\n"), Version { major: 3, minor: 6 });
        assert_eq!(v("tmux next-3.7"), Version { major: 3, minor: 7 });
        assert_eq!(v("psmux 3.4.0"), Version { major: 3, minor: 4 });
        assert_eq!(v("psmux 3.4.0 (20aa1d7 2026-10-13)"), Version { major: 3, minor: 4 });
        assert!(!v("tmux 3.1c").has_client_flags());
        assert!(v("tmux 3.2a").has_client_flags());
    }

    #[test]
    fn attach_flags_follow_the_version() {
        let old = Version { major: 3, minor: 0 };
        let new = Version { major: 3, minor: 4 };
        assert_eq!(attach_args(old, "$1", true), ["-C", "attach-session", "-t", "$1", "-r"]);
        assert_eq!(attach_args(old, "$1", false), ["-C", "attach-session", "-t", "$1"]);
        assert_eq!(attach_args(new, "$1", true), ["-C", "attach-session", "-t", "$1", "-f", "read-only,ignore-size"]);
    }

    #[test]
    fn splits_escaped_fields() {
        assert_eq!(fields("a\u{1f}b\u{1f}"), ["a", "b", ""]);
        assert_eq!(fields(r"a\037b\\c\037d"), ["a", r"b\c", "d"]);
    }

    #[test]
    fn checks_keys_like_the_hub() {
        for k in ["a", "Enter", "C-c", "M-x", "C-M-a", "F12", "BTab"] {
            assert!(key_allowed(k), "{k}");
        }
        for k in ["", "ab", "F13", "F01", "C-", "Enter; kill-server", "\"x\""] {
            assert!(!key_allowed(k), "{k}");
        }
    }

    #[test]
    fn chunks_on_char_boundaries() {
        let s = "aøb".repeat(3);
        let parts: Vec<_> = chunks(&s, 2).collect();
        assert_eq!(parts.concat(), s);
        assert!(parts.iter().all(|p| p.len() <= 2));
    }
}
