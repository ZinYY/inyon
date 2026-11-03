//! Against a real OpenSSH server (start-sshd.sh). Skipped when INYON_SSH_TEST_HOST isn't set.

mod common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::read;
use inyon_ssh_spike::{connect, key_from_seed, probe, Connection, Credentials};

const T: Duration = Duration::from_secs(20);

struct Env {
    host: String,
    port: u16,
    port_default: u16,
    user: String,
    dir: PathBuf,
    host_key: String,
}

fn env() -> Option<Env> {
    let v = |k: &str| std::env::var(format!("INYON_SSH_TEST_{k}")).ok();
    let host = v("HOST")?;
    Some(Env { host, port: v("PORT")?.parse().unwrap(), port_default: v("PORT_DEFAULT")?.parse().unwrap(), user: v("USER")?, dir: v("DIR")?.into(), host_key: v("HOSTKEY")? })
}

macro_rules! need_sshd {
    () => {
        match env() {
            Some(e) => e,
            None => {
                eprintln!("skipped: no test sshd (see start-sshd.sh)");
                return;
            }
        }
    };
}

impl Env {
    fn key(&self, name: &str, pass: Option<&str>) -> Credentials {
        Credentials { username: self.user.clone(), private_key: Some(read(&self.dir.join(name))), passphrase: pass.map(str::to_string), ..Default::default() }
    }

    async fn login(&self, port: u16, creds: &Credentials) -> Connection {
        connect(&self.host, port, &self.host_key, creds).await.unwrap_or_else(|e| panic!("login failed: {} {e}", e.code()))
    }
}

#[tokio::test]
async fn probe_shows_the_host_key_like_ssh_keygen() {
    let e = need_sshd!();
    let hk = probe(&e.host, e.port).await.unwrap();
    assert_eq!(hk.fingerprint, e.host_key);
    assert_eq!(hk.key_type, "ssh-ed25519");
}

#[tokio::test]
async fn a_wrong_host_key_is_refused() {
    let e = need_sshd!();
    let fake = "SHA256:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";
    let Err(err) = connect(&e.host, e.port, fake, &e.key("client_ed25519", None)).await else { panic!("expected an error") };
    assert_eq!(err.code(), "HOST_KEY_MISMATCH", "{err}");
    assert!(err.to_string().contains(&e.host_key));
}

#[tokio::test]
async fn logs_in_with_each_key_format_after_trying_none() {
    let e = need_sshd!();
    for (name, pass) in [("client_ed25519", None), ("client_ed25519_pass", Some("pass phrase")), ("client_rsa_pass", Some("hunter2")), ("client_rsa_pem", None)] {
        let t = Instant::now();
        let c = e.login(e.port, &e.key(name, pass)).await;
        // OpenSSH refuses `none` here, so getting in at all shows the fallthrough works
        assert_eq!(c.method, "private-key", "{name}");
        println!("login with {name}: {} ms", t.elapsed().as_millis());
        let r = c.exec("echo ok", T).await.unwrap();
        assert_eq!(r.stdout.trim(), "ok");
        c.disconnect().await;
    }
}

#[tokio::test]
async fn a_wrong_passphrase_is_auth_failed() {
    let e = need_sshd!();
    let Err(err) = connect(&e.host, e.port, &e.host_key, &e.key("client_rsa_pass", Some("nope"))).await else { panic!("expected an error") };
    assert_eq!(err.code(), "AUTH_FAILED", "{err}");
}

/// The phone's own key: the ed25519 seed of its link identity, added to authorized_keys the way
/// `inyon phones` does.
#[tokio::test]
async fn logs_in_with_the_phone_seed() {
    let e = need_sshd!();
    let seed = [9u8; 32];
    let public = key_from_seed(&seed).unwrap().public_key().to_openssh().unwrap();
    let ak = e.dir.join("authorized_keys");
    let mut text = read(&ak);
    if !text.contains(&public) {
        text.push_str(&format!("{public} phone\n"));
        std::fs::write(&ak, text).unwrap();
    }
    let creds = Credentials { username: e.user.clone(), ed25519_seed: Some(seed), ..Default::default() };
    let c = e.login(e.port, &creds).await;
    assert_eq!(c.method, "phone-key");
    c.disconnect().await;
}

#[tokio::test]
async fn exec_returns_code_stdout_and_stderr() {
    let e = need_sshd!();
    let c = e.login(e.port, &e.key("client_ed25519", None)).await;
    let r = c.exec("printf out; printf err >&2; exit 3", T).await.unwrap();
    assert_eq!((r.code, r.stdout.as_str(), r.stderr.as_str()), (3, "out", "err"));
    // leading spaces survive (Ghostex #252 strips them in chat)
    let r = c.exec("printf '    indented\\n'", T).await.unwrap();
    assert_eq!(r.stdout, "    indented\n");
    c.disconnect().await;
}

/// Sixteen sessions at once on one connection, each running its own command.
#[tokio::test]
async fn sixteen_sessions_on_one_connection() {
    let e = need_sshd!();
    let c = e.login(e.port, &e.key("client_ed25519", None)).await;
    let t = Instant::now();
    let chans = c.open_sessions(16).await;
    let opened = t.elapsed();
    let mut set = tokio::task::JoinSet::new();
    for (i, ch) in chans.into_iter().enumerate() {
        let ch = ch.unwrap_or_else(|err| panic!("session {i}: {err}"));
        set.spawn(async move { (i, inyon_ssh_spike::run_exec(ch, &format!("sleep 0.2; echo {i}"), T).await) });
    }
    let mut done = 0;
    while let Some(r) = set.join_next().await {
        let (i, r) = r.unwrap();
        assert_eq!(r.unwrap().stdout.trim(), i.to_string());
        done += 1;
    }
    assert_eq!(done, 16);
    println!("16 sessions opened in {} ms, all ran in {} ms", opened.as_millis(), t.elapsed().as_millis());
    c.disconnect().await;
}

/// With OpenSSH's default MaxSessions 10, the 11th session on a connection is refused: the app
/// still needs SshCore's extra connections, and the refusal comes back as a plain error.
#[tokio::test]
async fn default_max_sessions_refuses_the_eleventh() {
    let e = need_sshd!();
    let c = e.login(e.port_default, &e.key("client_ed25519", None)).await;
    let mut held = Vec::new();
    for _ in 0..10 {
        held.push(c.open_session().await.expect("the first ten open"));
    }
    let Err(err) = c.open_session().await else { panic!("the eleventh is refused") };
    println!("11th session: {err}");
    // dropping a russh Channel does not close it on the server: each one has to be closed
    for ch in held {
        ch.close().await.unwrap();
    }
    tokio::time::sleep(Duration::from_millis(300)).await;
    // the connection itself is still fine, and has room again
    let r = c.exec("echo still here", T).await.unwrap();
    assert_eq!(r.stdout.trim(), "still here");
    c.disconnect().await;
}

#[tokio::test]
async fn pty_has_the_size_asked_for_and_follows_resizes() {
    let e = need_sshd!();
    let c = e.login(e.port, &e.key("client_ed25519", None)).await;
    let mut pty = c.open_pty(200, 50, Some("sh")).await.unwrap();
    pty.write(b"stty size\n").await.unwrap();
    pty.read_until(T, |s| s.contains("50 200")).await.unwrap();
    pty.resize(120, 30).await.unwrap();
    pty.write(b"stty size; echo typed-$((6*7))\n").await.unwrap();
    pty.read_until(T, |s| s.contains("30 120") && s.contains("typed-42")).await.unwrap();
    pty.write(b"exit\n").await.unwrap();
    c.disconnect().await;
}
