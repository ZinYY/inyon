//! `inyon sessions`, `inyon hub machines`, `inyon pages --hub` and `inyon send` on a joined
//! machine, against a stand-in hub: the machine's own token goes with every request, and a message
//! is held for the pane the target names.

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};

use serde_json::{json, Value};

const TOKEN: &str = "laptop-link-token-0123456789";

#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    auth: String,
    body: String,
}

/// A hub on a loopback port that answers from `answer` and keeps every request it got.
fn fake_hub(answer: fn(&str, &str) -> (u16, Value)) -> (String, Arc<Mutex<Vec<Seen>>>) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        for s in l.incoming().flatten() {
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut first = String::new();
            if r.read_line(&mut first).is_err() {
                continue;
            }
            let mut parts = first.split_whitespace();
            let (method, path) = (parts.next().unwrap_or("").to_string(), parts.next().unwrap_or("").to_string());
            let (mut auth, mut len) = (String::new(), 0usize);
            loop {
                let mut line = String::new();
                if r.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
                let (k, v) = line.split_once(':').unwrap_or((&line, ""));
                match k.to_ascii_lowercase().as_str() {
                    "authorization" => auth = v.trim().to_string(),
                    "content-length" => len = v.trim().parse().unwrap_or(0),
                    _ => {}
                }
            }
            let mut body = vec![0; len];
            let _ = r.read_exact(&mut body);
            let body = String::from_utf8_lossy(&body).into_owned();
            let (status, v) = answer(&method, &path);
            log.lock().unwrap().push(Seen { method, path, auth, body });
            let text = v.to_string();
            let mut s = s;
            let _ = write!(s, "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}", text.len());
        }
    });
    (url, seen)
}

fn overview() -> Value {
    let pane = |id: &str, kind: Option<&str>, state: &str| json!({ "id": id, "index": 0, "active": true, "title": "", "dead": false, "command": "x", "cwd": "C:\\demo", "agent": kind.map(|k| json!({ "kind": k, "state": state })) });
    json!({ "hosts": [
        { "config": { "id": "local", "name": "abhash-arm", "kind": "local" }, "status": "online", "sessions": [
            { "id": "$1", "name": "main", "windows": [{ "id": "@1", "index": 0, "name": "w", "active": true, "panes": [pane("%1", Some("claude"), "working")] }] }
        ] },
        { "config": { "id": "laptop", "name": "laptop", "kind": "node" }, "status": "online", "sessions": [
            { "id": "$2", "name": "experiment", "windows": [{ "id": "@2", "index": 0, "name": "w", "active": true, "panes": [pane("%3", Some("codex"), "idle")] }] }
        ] }
    ] })
}

fn answers(method: &str, path: &str) -> (u16, Value) {
    match (method, path) {
        ("GET", "/api/overview") => (200, overview()),
        ("GET", "/api/pages") => {
            (200, json!({ "pages": [{ "host": "laptop", "slug": "robot", "name": "Cardboard robot", "project": "C:\\demo\\robot", "by": "claude", "added": 5 }] }))
        }
        ("POST", "/api/hosts/local/panes/1/held") => (200, json!({ "id": "held-1", "text": "x", "at": 1 })),
        _ => (404, json!({ "error": "Not found" })),
    }
}

fn send_off(method: &str, path: &str) -> (u16, Value) {
    match (method, path) {
        ("POST", _) => (403, json!({ "error": "This hub doesn't take messages from its machines.", "reason": "send-off" })),
        _ => answers(method, path),
    }
}

struct Home(PathBuf);

impl Home {
    fn new(name: &str, hub: Option<&str>) -> Home {
        let dir = std::env::temp_dir().join(format!("inyon-hub-api-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join(".inyon/node")).unwrap();
        if let Some(hub) = hub {
            fs::write(dir.join(".inyon/node").join(if cfg!(windows) { ".env" } else { "env" }), format!("INYON_HUB={hub}\nINYON_TOKEN={TOKEN}\n")).unwrap();
        }
        Home(dir)
    }

    fn run(&self, args: &[&str], stdin: Option<&str>) -> (i32, String, String) {
        let mut c = Command::new(env!("CARGO_BIN_EXE_inyon"));
        c.args(args).env("HOME", &self.0).env("USERPROFILE", &self.0).current_dir(&self.0);
        for v in ["INYON_HUB", "INYON_TOKEN", "TETHER_HUB", "TETHER_TOKEN", "INYON_DATA_DIR", "XDG_DATA_HOME", "XDG_CONFIG_HOME"] {
            c.env_remove(v);
        }
        c.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped());
        let mut child = c.spawn().unwrap();
        if let Some(text) = stdin {
            child.stdin.take().unwrap().write_all(text.as_bytes()).unwrap();
        }
        drop(child.stdin.take());
        let out = child.wait_with_output().unwrap();
        (out.status.code().unwrap_or(-1), String::from_utf8_lossy(&out.stdout).into_owned(), String::from_utf8_lossy(&out.stderr).into_owned())
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_joined_machine_lists_the_hubs_sessions_machines_and_pages_with_its_own_token() {
    let (hub, seen) = fake_hub(answers);
    let h = Home::new("read", Some(&hub));
    let (code, out, err) = h.run(&["sessions"], None);
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(out, "abhash-arm/main    claude  working  C:\\demo\nlaptop/experiment  codex   idle     C:\\demo\n");
    let (code, out, err) = h.run(&["hub", "machines"], None);
    assert_eq!(code, 0, "{out}{err}");
    assert!(out.starts_with("abhash-arm  online  1 session, 1 agent, 1 working\nlaptop"), "{out}");
    let (code, out, err) = h.run(&["pages", "--hub"], None);
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(out, "Cardboard robot  laptop  robot  /pages/laptop/robot\n");
    let (code, out, _) = h.run(&["sessions", "--json"], None);
    assert_eq!(code, 0);
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["hosts"][1]["config"]["id"], "laptop");
    let seen = seen.lock().unwrap();
    assert!(!seen.is_empty());
    assert!(seen.iter().all(|s| s.auth == format!("Bearer {TOKEN}") && s.method == "GET"), "{seen:?}");
}

#[test]
fn a_message_is_held_for_the_pane_the_target_names() {
    let (hub, seen) = fake_hub(answers);
    let h = Home::new("send", Some(&hub));
    let (code, out, err) = h.run(&["send", "abhash-arm/main", "the", "report", "is", "in", "C:\\demo"], None);
    assert_eq!(code, 0, "{out}{err}");
    assert_eq!(out, "Held for abhash-arm/main (claude). It goes in once the agent is free, and shows in the app until then.\n");
    // from stdin, keeping its lines
    let (code, out, err) = h.run(&["send", "abhash-arm/main", "-"], Some("line one\nline two\n"));
    assert_eq!(code, 0, "{out}{err}");
    let posts: Vec<Seen> = seen.lock().unwrap().iter().filter(|s| s.method == "POST").cloned().collect();
    assert_eq!(posts.len(), 2, "{posts:?}");
    assert_eq!(posts[0].path, "/api/hosts/local/panes/1/held");
    assert_eq!(serde_json::from_str::<Value>(&posts[0].body).unwrap(), json!({ "text": "the report is in C:\\demo" }));
    assert_eq!(serde_json::from_str::<Value>(&posts[1].body).unwrap(), json!({ "text": "line one\nline two" }));
    assert_eq!(posts[0].auth, format!("Bearer {TOKEN}"));

    // a session with no such agent: nothing is sent
    let (code, _, err) = h.run(&["send", "laptop/nope", "hi"], None);
    assert_eq!((code, err.as_str()), (1, "laptop has no session called nope. `inyon sessions` lists them.\n"));
    assert_eq!(seen.lock().unwrap().iter().filter(|s| s.method == "POST").count(), 2);
}

#[test]
fn the_hubs_refusal_and_a_machine_that_isnt_joined_say_what_to_do() {
    let (hub, _) = fake_hub(send_off);
    let h = Home::new("off", Some(&hub));
    let (code, _, err) = h.run(&["send", "laptop/experiment", "hi"], None);
    assert_eq!((code, err.as_str()), (1, "This hub doesn't take messages from its machines.\n"));

    let h = Home::new("alone", None);
    for args in [&["sessions"][..], &["hub", "machines"], &["pages", "--hub"], &["send", "a/b", "hi"]] {
        let (code, _, err) = h.run(args, None);
        assert_eq!(code, 1, "{args:?}");
        assert!(err.starts_with("This machine doesn't run a hub and isn't joined to one."), "{args:?}: {err}");
    }
}
