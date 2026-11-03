//! How a command answers: text for a person, or one JSON line (`{"v":1,"ok":true,…}`) with
//! `--json`, which the inyon command (and older ones, which ran the Bun engine) reads. With
//! `--json` anything meant for a person goes to stderr, so stdout stays one line of JSON. Run in
//! the background on Windows, where there's no console, the log lines go to a file. (output.ts)

use std::io::Write;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde_json::{json, Map, Value};

static JSON_OUT: AtomicBool = AtomicBool::new(false);
static LOG_TO: Mutex<Option<PathBuf>> = Mutex::new(None);

/// Whether answers are JSON (the command was given --json).
pub fn json() -> bool {
    JSON_OUT.load(Ordering::Relaxed)
}

pub fn set_json(on: bool) {
    JSON_OUT.store(on, Ordering::Relaxed);
}

/// The arguments without the --json flag, wherever it was given; notes whether it was.
pub fn take_json(args: &[String]) -> Vec<String> {
    if args.iter().any(|a| a == "--json") {
        set_json(true);
    }
    args.iter().filter(|a| *a != "--json").cloned().collect()
}

/// Sends log lines to `file` from now on, each stamped with the time (no console to print to).
pub fn log_to(file: PathBuf) {
    *LOG_TO.lock().unwrap_or_else(|e| e.into_inner()) = Some(file);
}

fn to_file(line: &str) -> bool {
    let guard = LOG_TO.lock().unwrap_or_else(|e| e.into_inner());
    let Some(file) = guard.as_ref() else { return false };
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(file) {
        let _ = writeln!(f, "{} {line}", crate::iso_now());
    }
    true
}

/// `console.log`: a line on stdout, or in the log file.
pub fn log(line: &str) {
    if !to_file(line) {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    }
}

/// `console.error`: a line on stderr, or in the log file.
pub fn error(line: &str) {
    if !to_file(line) {
        let _ = writeln!(std::io::stderr().lock(), "{line}");
    }
}

/// A line for a person: stdout, or stderr under --json.
pub fn say(line: &str) {
    if json() {
        error(line)
    } else {
        log(line)
    }
}

/// `{"v":1,"ok":true,…value}`.
pub fn answer(value: &Value) -> String {
    let mut m = Map::new();
    m.insert("v".into(), json!(1));
    m.insert("ok".into(), json!(true));
    if let Value::Object(fields) = value {
        for (k, v) in fields {
            m.insert(k.clone(), v.clone());
        }
    }
    Value::Object(m).to_string()
}

/// A line on stdout, wherever the log goes: an answer is for whoever reads this program's output.
fn answer_line(line: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}

/// A command's answer: `value` as JSON under --json, else `message` (when there is one).
pub fn emit(value: Value, message: Option<&str>) {
    if json() {
        answer_line(&answer(&value));
    } else if let Some(m) = message {
        log(m);
    }
}

/// `{"v":1,"ok":false,"error":…}`.
pub fn failure(message: &str) -> String {
    json!({ "v": 1, "ok": false, "error": message }).to_string()
}

/// Ends the command with an error: JSON on stdout and exit 1 under --json, else the message on
/// stderr and exit `code`.
pub fn fail(message: &str, code: i32) -> ! {
    if json() {
        answer_line(&failure(message));
        std::process::exit(1);
    }
    error(message);
    std::process::exit(code);
}

/// What `pair --json` answers.
pub struct PairOutput {
    /// the link for the app (inyon://pair?…)
    pub link: String,
    /// what the QR code carries: the web app's address with the pairing after #
    pub qr: String,
    /// the typed-in form, when the relays took it
    pub short: Option<(String, String)>,
    pub urls: Vec<String>,
    pub relays: Vec<String>,
    /// whether the link runs on its own (a service), so this process ends once the code is shown
    pub service: bool,
}

/// What `pair --json` prints. The short code is left out when it couldn't be published.
pub fn pair_result(o: &PairOutput, typed: bool) -> Value {
    let mut v = json!({ "link": o.link, "qr": o.qr, "urls": o.urls, "relays": o.relays, "service": o.service });
    if let (true, Some((id, code))) = (typed, &o.short) {
        v["short"] = json!({ "id": id, "code": code });
    }
    v
}

/// What the terminal says while the tunnel starts, before there's a code worth scanning.
pub const TUNNEL_STARTING: &str = "Starting the tunnel, so your phone can reach this computer from any network…";
/// And once it has taken longer than it should.
pub const TUNNEL_SLOW: &str = "The tunnel is still starting. The code shows here once it's up; `inyon logs` shows what the tunnel says.";
/// The last answer when the code ran out while the tunnel was still starting.
pub const PAIR_EXPIRED: &str = "The pairing code expired before the tunnel came up. `inyon logs` shows what the tunnel said.";
/// Under a code that works on the same network or VPN while the tunnel starts.
pub const TUNNEL_PENDING: &str = "The tunnel is still starting, so for now this code works on the same network or VPN. It's shown again once the tunnel is up.";

/// The first twelve characters of this computer's id (its public key) from its pairing link, in
/// groups of four, as the phone shows them before it pairs: "ybnd rfg8 ejkm".
pub fn computer_id(link: &str) -> Option<String> {
    let q = link.split_once('?')?.1;
    let id = q.split('&').find_map(|kv| kv.strip_prefix("m="))?;
    let id: String = id.chars().take(12).collect();
    (id.len() == 12 && id.chars().all(|c| c.is_ascii_alphanumeric())).then(|| format!("{} {} {}", &id[..4], &id[4..8], &id[8..]))
}

/// Whether `pair` leaves the link out: inside tmux, unless `--show-link` asked for it. Inyon shows
/// every tmux pane on other screens (the app, and a companion app reading an agent's screen), and
/// the link pairs a phone by itself, a shell on this machine for whoever reads it first.
pub fn link_hidden(show_link: bool) -> bool {
    link_hidden_in(show_link, std::env::var_os("TMUX").is_some_and(|v| !v.is_empty()))
}

fn link_hidden_in(show_link: bool, in_tmux: bool) -> bool {
    in_tmux && !show_link
}

/// Said in place of the link when it's left out.
pub const LINK_HIDDEN: &str =
    "This terminal is in tmux, which Inyon can show on other screens, so the link that pairs a phone by itself isn't printed (`inyon pair --show-link` prints it). The QR code and the code above work once, for 10 minutes.";

/// The pairing code as the terminal shows it, drawn from one of `pair --json`'s answers.
pub fn render_pair(v: &Value) -> String {
    render_pair_with(v, link_hidden(false))
}

/// `render_pair`, with the link left out when `hide_link`.
pub fn render_pair_with(v: &Value, hide_link: bool) -> String {
    let text = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    let list = |k: &str| v.get(k).and_then(Value::as_array).map(|a| a.len()).unwrap_or(0);
    if let Some(e) = v.get("error").and_then(Value::as_str) {
        return format!("\n! {e}\n");
    }
    let waiting = v.get("waiting").is_some();
    let no_address = list("urls") == 0 && list("relays") == 0;
    if waiting && v.get("qr").is_none() {
        return format!("\n{TUNNEL_STARTING}\n");
    }
    if waiting && no_address {
        return format!("\n{TUNNEL_SLOW}\n");
    }
    let mut out = String::new();
    out.push_str("\nScan this with your phone's camera, or in the Inyon app (Add a computer → Scan the code), within 10 minutes:\n\n");
    match crate::qr::render(&text("qr")) {
        Ok(code) => out.push_str(&code),
        Err(e) => out.push_str(&format!("(no QR code: {e})")),
    }
    out.push('\n');
    if let Some(short) = v.get("short") {
        let id = short.get("id").and_then(Value::as_str).unwrap_or("");
        let code = short.get("code").and_then(Value::as_str).unwrap_or("");
        let (a, b) = id.split_at(id.len().min(4));
        let (c, d) = code.split_at(code.len().min(3));
        out.push_str(&format!("\nCan't scan it? Choose \"Type a code\" in the app and enter\n\n    ID    {a}-{b}\n    Code  {c} {d}\n\n"));
    }
    if let Some(id) = computer_id(&text("link")) {
        out.push_str(&format!("\nThis computer's ID is {id}. The app shows it before it pairs: pair only if they match.\n"));
    }
    if hide_link {
        out.push_str(&format!("\n{LINK_HIDDEN}\n\n"));
    } else {
        out.push_str(&format!("\nOr paste this link into the app:\n{}\n\n", text("link")));
    }
    if waiting {
        out.push_str(&format!("{TUNNEL_PENDING}\n\n"));
    } else if no_address {
        out.push_str("No address yet: the phone will look this machine up once its tunnel is up.\n\n");
    }
    if v.get("service").and_then(Value::as_bool) == Some(false) {
        out.push_str("Keep this running so the phone can connect, or run `inyon service install` to keep it running in the background.\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_terminal_shows_the_computer_id_the_phone_shows() {
        let link = "inyon://pair?m=ybndrfg8ejkmcpqxot1uwisza345h769ybndrfg8ejkmcpqxot1u&s=secret&n=box";
        assert_eq!(computer_id(link).as_deref(), Some("ybnd rfg8 ejkm"));
        assert_eq!(computer_id("inyon://pair?n=box"), None);
        assert_eq!(computer_id("inyon://pair?m=short"), None);
        let shown = render_pair(&json!({ "link": link, "qr": "https://web/#pair?m=x", "urls": ["wss://x"], "relays": [], "service": true }));
        assert!(shown.contains("This computer's ID is ybnd rfg8 ejkm."), "{shown}");
    }

    #[test]
    fn in_tmux_the_link_that_pairs_a_phone_by_itself_is_printed_only_when_asked_for() {
        let link = "inyon://pair?m=ybndrfg8ejkmcpqxot1uwisza345h769ybndrfg8ejkmcpqxot1u&s=s3cret&n=box";
        let v = json!({ "link": link, "qr": "https://web/#pair?m=x", "urls": ["wss://x"], "relays": [], "service": true });
        assert!(link_hidden_in(false, true));
        assert!(!link_hidden_in(true, true) && !link_hidden_in(false, false));
        let hidden = render_pair_with(&v, true);
        assert!(!hidden.contains(link) && hidden.contains("--show-link"), "{hidden}");
        assert!(hidden.contains("This computer's ID is ybnd rfg8 ejkm."), "{hidden}");
        assert!(render_pair_with(&v, false).contains(&format!("Or paste this link into the app:\n{link}\n")));
    }

    #[test]
    fn the_pairing_answer_carries_the_short_code_only_once_published() {
        let base = PairOutput {
            link: "inyon://pair?m=a".into(),
            qr: "https://web/#pair?m=a".into(),
            short: Some(("K7Q4MX2T".into(), "493812".into())),
            urls: vec!["wss://x/link".into()],
            relays: vec![],
            service: true,
        };
        assert_eq!(
            answer(&pair_result(&base, true)),
            r#"{"v":1,"ok":true,"link":"inyon://pair?m=a","qr":"https://web/#pair?m=a","urls":["wss://x/link"],"relays":[],"service":true,"short":{"id":"K7Q4MX2T","code":"493812"}}"#
        );
        assert!(pair_result(&base, false).get("short").is_none());
    }

    #[test]
    fn answers_and_failures_are_one_line_with_v_first() {
        assert_eq!(answer(&json!({ "phones": [] })), r#"{"v":1,"ok":true,"phones":[]}"#);
        assert_eq!(failure(r#"No paired phone matches "x""#), r#"{"v":1,"ok":false,"error":"No paired phone matches \"x\""}"#);
    }

    #[test]
    fn the_flag_is_taken_out_wherever_it_is() {
        assert_eq!(take_json(&["forget".into(), "--json".into(), "a".into()]), vec!["forget".to_string(), "a".to_string()]);
        assert!(json());
        set_json(false);
    }
}
