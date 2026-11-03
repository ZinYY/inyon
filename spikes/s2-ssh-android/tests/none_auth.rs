//! `none` auth against a server that lets anyone in (as Tailscale SSH does). OpenSSH can't be set
//! up that way without a passwordless account, so the server here is russh's own, in-process.

use std::sync::Arc;
use std::time::Duration;

use inyon_ssh_spike::{connect, fingerprint, key_from_seed, probe, Credentials};
use russh::server::{self, Auth, Msg, Session};
use russh::{Channel, ChannelId};

#[derive(Clone)]
struct Open;

impl server::Handler for Open {
    type Error = russh::Error;

    async fn auth_none(&mut self, _user: &str) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    async fn channel_open_session(&mut self, _ch: Channel<Msg>, reply: server::ChannelOpenHandle, _s: &mut Session) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }

    async fn exec_request(&mut self, ch: ChannelId, data: &[u8], s: &mut Session) -> Result<(), Self::Error> {
        s.channel_success(ch)?;
        s.data(ch, format!("ran {}", String::from_utf8_lossy(data)).into_bytes())?;
        s.exit_status_request(ch, 0)?;
        s.eof(ch)?;
        s.close(ch)?;
        Ok(())
    }
}

#[tokio::test]
async fn none_auth_logs_in_and_runs_a_command() {
    let host_key = key_from_seed(&[42u8; 32]).unwrap();
    let want_fp = fingerprint(host_key.public_key());
    let config = Arc::new(server::Config { keys: vec![host_key], auth_rejection_time: Duration::from_millis(10), ..Default::default() });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let (sock, _) = listener.accept().await.unwrap();
            let cfg = config.clone();
            tokio::spawn(async move {
                if let Ok(s) = server::run_stream(cfg, sock, Open).await {
                    let _ = s.await;
                }
            });
        }
    });

    let hk = probe("127.0.0.1", port).await.unwrap();
    assert_eq!(hk.fingerprint, want_fp);
    assert_eq!(hk.key_type, "ssh-ed25519");

    // no key and no password: only `none` can work
    let creds = Credentials { username: "anyone".into(), ..Default::default() };
    let conn = connect("127.0.0.1", port, &hk.fingerprint, &creds).await.unwrap();
    assert_eq!(conn.method, "none");
    let r = conn.exec("uptime", Duration::from_secs(5)).await.unwrap();
    assert_eq!(r.code, 0);
    assert_eq!(r.stdout, "ran uptime");
    conn.disconnect().await;
}
