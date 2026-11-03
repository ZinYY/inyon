//! `inyon join --no-autostart`, as install.ps1 runs it on Windows (and install.sh, with the
//! service): the env file it writes, and what it refuses.

use std::fs;
use std::path::PathBuf;
use std::process::Command;

struct Home(PathBuf);

impl Home {
    fn new(name: &str) -> Home {
        let dir = std::env::temp_dir().join(format!("inyon-join-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Home(dir)
    }

    fn join(&self, hub: &str, env: &[(&str, &str)]) -> (bool, String) {
        let mut c = Command::new(env!("CARGO_BIN_EXE_inyon"));
        c.args(["join", hub, "--no-autostart"]).env("HOME", &self.0).env("USERPROFILE", &self.0).current_dir(&self.0);
        for v in [
            "INYON_HUB",
            "INYON_ALLOW_INSECURE",
            "INYON_HUB_CERT",
            "INYON_LEGACY_ENGINE",
            "INYON_SH",
            "TETHER_HUB",
            "TETHER_ALLOW_INSECURE",
            "TETHER_LEGACY_ENGINE",
            "TETHER_TOKEN",
        ] {
            c.env_remove(v);
        }
        c.env("INYON_TOKEN", "t0k3n-for-the-join-test-0123456789");
        for (k, v) in env {
            c.env(k, v);
        }
        let out = c.output().unwrap();
        (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
    }

    fn saved(&self) -> Option<String> {
        let dir = self.0.join(".inyon").join("node");
        fs::read_to_string(dir.join(if cfg!(windows) { ".env" } else { "env" })).ok()
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn a_join_notes_its_version_and_refuses_plain_http_that_anyone_could_answer() {
    let version = env!("CARGO_PKG_VERSION");
    // a VPN address: saved, with the version that joined, so the connection holds it to this check
    let h = Home::new("vpn");
    let (ok, said) = h.join("http://100.70.1.2:4777", &[("INYON_SH", "C:\\inyon\\busybox.exe")]);
    assert!(ok, "{said}");
    let saved = h.saved().unwrap();
    assert!(saved.contains("INYON_HUB=http://100.70.1.2:4777\n"), "{saved}");
    assert!(saved.contains("INYON_TOKEN=t0k3n-for-the-join-test-0123456789\n"), "{saved}");
    assert!(saved.contains(&format!("INYON_JOINED={version}\n")), "{saved}");
    assert!(!saved.contains("INYON_ALLOW_INSECURE"), "{saved}");
    // Windows keeps the shell install.ps1 put there
    assert_eq!(saved.contains("INYON_SH=C:\\inyon\\busybox.exe"), cfg!(windows), "{saved}");

    // a LAN or public address on plain http: refused, nothing saved
    for hub in ["http://192.168.1.5:4777", "http://203.0.113.7:4777"] {
        let h = Home::new("refused");
        let (ok, said) = h.join(hub, &[]);
        assert!(!ok, "{hub}: {said}");
        assert!(said.contains("Refusing to send the token over plain http"), "{said}");
        assert_eq!(h.saved(), None, "{hub}");
    }

    // told to send it anyway: saved, and the choice noted for the connection
    let h = Home::new("insecure");
    let (ok, said) = h.join("http://192.168.1.5:4777", &[("INYON_ALLOW_INSECURE", "1")]);
    assert!(ok, "{said}");
    let saved = h.saved().unwrap();
    assert!(saved.contains(&format!("INYON_JOINED={version}\n")) && saved.contains("INYON_ALLOW_INSECURE=1\n"), "{saved}");
}

/// The token the test hub hands out: plain words, long enough to pass for one.
const FRESH: &str = "fresh-token-the-test-hub-hands-out-once";

/// A hub on 127.0.0.1 that swaps the test's token once, then refuses it as used, as /node/claim does.
fn claiming_hub() -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    std::thread::spawn(move || {
        let mut used = false;
        for conn in listener.incoming() {
            let Ok(mut conn) = conn else { continue };
            let mut buf = [0u8; 4096];
            let n = conn.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]).to_string();
            let line = req.lines().next().unwrap_or("").to_string();
            let bearer = req.lines().find_map(|l| l.to_ascii_lowercase().starts_with("authorization:").then(|| l[14..].trim().to_string())).unwrap_or_default();
            log.lock().unwrap().push(format!("{line} {bearer}"));
            let (status, body) = if line.starts_with("POST /node/claim ") && bearer == "Bearer t0k3n-for-the-join-test-0123456789" && !used {
                used = true;
                ("200 OK", format!(r#"{{"token":"{FRESH}"}}"#))
            } else {
                ("403 Forbidden", "Unknown token; the command was used already or replaced. Get a new one from the hub.\n".to_string())
            };
            let _ = write!(conn, "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
        }
    });
    (format!("http://127.0.0.1:{port}"), seen)
}

#[test]
fn the_pasted_token_is_swapped_for_the_hubs_once_and_running_the_command_again_keeps_the_swapped_one() {
    let (hub, seen) = claiming_hub();
    let h = Home::new("claim");
    let (ok, said) = h.join(&hub, &[]);
    assert!(ok, "{said}");
    let saved = h.saved().unwrap();
    assert!(saved.contains(&format!("INYON_TOKEN={FRESH}\n")), "{saved}");
    assert!(!saved.contains("t0k3n-for-the-join-test"), "{saved}");
    assert_eq!(seen.lock().unwrap()[0], "POST /node/claim HTTP/1.1 Bearer t0k3n-for-the-join-test-0123456789");

    // the same command again (install.sh run twice): the hub refuses the used token, and the one
    // saved the first time stays
    let (ok, said) = h.join(&hub, &[]);
    assert!(ok, "{said}");
    assert!(h.saved().unwrap().contains(&format!("INYON_TOKEN={FRESH}\n")));

    // on another machine the used command is refused, and nothing is saved
    let other = Home::new("claim-used");
    let (ok, said) = other.join(&hub, &[]);
    assert!(!ok, "{said}");
    assert!(said.contains("used already") && said.contains("Add a machine"), "{said}");
    assert_eq!(other.saved(), None);
}
