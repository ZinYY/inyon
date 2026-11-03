//! The parts of the phone's SSH client (apps/mobile/.../ssh/SshCore.kt) that decide whether russh
//! can replace it: key loading, the login order, host key fingerprints in the same text form, many
//! sessions on one connection, exec and a PTY.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::client::{self, Handle, Msg};
use russh::keys::ssh_key::private::{Ed25519Keypair, KeypairData};
use russh::keys::{decode_secret_key, HashAlg, PrivateKey, PrivateKeyWithHashAlg, PublicKey, PublicKeyOrCertificate};
use russh::{Channel, ChannelMsg, Disconnect};

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(12);

/// Error codes as SshCore names them, so the web app's handling stays the same.
#[derive(Debug)]
pub enum SshError {
    BadRequest(String),
    HostKeyMismatch { expected: String, presented: String },
    Unreachable(String),
    AuthFailed(String),
    Error(String),
}

impl SshError {
    pub fn code(&self) -> &'static str {
        match self {
            SshError::BadRequest(_) => "BAD_REQUEST",
            SshError::HostKeyMismatch { .. } => "HOST_KEY_MISMATCH",
            SshError::Unreachable(_) => "UNREACHABLE",
            SshError::AuthFailed(_) => "AUTH_FAILED",
            SshError::Error(_) => "ERROR",
        }
    }
}

impl std::fmt::Display for SshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SshError::HostKeyMismatch { expected, presented } => write!(f, "host key mismatch: expected {expected}, server presented {presented}"),
            SshError::BadRequest(m) | SshError::Unreachable(m) | SshError::AuthFailed(m) | SshError::Error(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for SshError {}

impl From<russh::Error> for SshError {
    fn from(e: russh::Error) -> Self {
        SshError::Error(e.to_string())
    }
}

/// What to log in with. Tried in SshCore's order: none, ed25519 seed (the phone key), private key
/// text, password.
#[derive(Clone, Default)]
pub struct Credentials {
    pub username: String,
    pub password: Option<String>,
    pub private_key: Option<String>,
    pub passphrase: Option<String>,
    pub ed25519_seed: Option<[u8; 32]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostKey {
    pub fingerprint: String,
    pub key_type: String,
}

pub struct ExecResult {
    pub code: u32,
    pub stdout: String,
    pub stderr: String,
}

/// "SHA256:" and unpadded base64, as `ssh-keygen -l` prints it and SshCore shows it.
pub fn fingerprint(key: &PublicKey) -> String {
    key.fingerprint(HashAlg::Sha256).to_string()
}

/// SshCore.normalizeFingerprint: saved fingerprints may carry padding or spaces.
pub fn normalize_fingerprint(fp: &str) -> String {
    fp.trim().trim_end_matches('=').to_string()
}

fn host_key_of(key: &PublicKeyOrCertificate) -> HostKey {
    // A host certificate is fingerprinted by the key it certifies. SshCore hashes the whole blob
    // the server sent, so the two differ for certificates (rare for Inyon's hosts).
    let public = match key {
        PublicKeyOrCertificate::PublicKey { key, .. } => key.clone(),
        PublicKeyOrCertificate::Certificate(cert) => PublicKey::from(cert.public_key().clone()),
    };
    HostKey { fingerprint: fingerprint(&public), key_type: public.algorithm().as_str().to_string() }
}

/// A private key pasted by the user: OpenSSH format (encrypted or not), PKCS#8, or legacy PEM.
pub fn load_private_key(text: &str, passphrase: Option<&str>) -> Result<PrivateKey, SshError> {
    let passphrase = passphrase.filter(|p| !p.is_empty());
    decode_secret_key(text.trim(), passphrase).map_err(|e| SshError::AuthFailed(format!("private key could not be read (wrong passphrase or unsupported format): {e}")))
}

/// The phone's own key is a raw 32-byte ed25519 seed (the link identity), not a key file.
pub fn key_from_seed(seed: &[u8; 32]) -> Result<PrivateKey, SshError> {
    PrivateKey::new(KeypairData::Ed25519(Ed25519Keypair::from_seed(seed)), "inyon-phone").map_err(|e| SshError::Error(e.to_string()))
}

struct Checker {
    expected: Option<String>,
    seen: Arc<Mutex<Option<HostKey>>>,
}

impl client::Handler for Checker {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let hk = host_key_of(key);
        let ok = self.expected.as_deref() == Some(hk.fingerprint.as_str());
        *self.seen.lock().unwrap() = Some(hk);
        Ok(ok)
    }
}

fn config() -> Arc<client::Config> {
    Arc::new(client::Config { inactivity_timeout: None, keepalive_interval: Some(Duration::from_secs(15)), keepalive_max: 2, ..Default::default() })
}

async fn tcp(host: &str, port: u16) -> Result<tokio::net::TcpStream, SshError> {
    let addr = format!("{host}:{port}");
    match tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect(&addr)).await {
        Ok(Ok(s)) => {
            s.set_nodelay(true).ok();
            Ok(s)
        }
        Ok(Err(e)) => Err(SshError::Unreachable(format!("could not reach {addr}: {e}"))),
        Err(_) => Err(SshError::Unreachable(format!("could not reach {addr}: timed out"))),
    }
}

/// The server's host key, for the user to confirm before the first login. Refusing the key ends
/// the handshake right after the server has shown it.
pub async fn probe(host: &str, port: u16) -> Result<HostKey, SshError> {
    let seen = Arc::new(Mutex::new(None));
    let stream = tcp(host, port).await?;
    let res = client::connect_stream(config(), stream, Checker { expected: None, seen: seen.clone() }).await;
    drop(res);
    let hk = seen.lock().unwrap().clone();
    hk.ok_or_else(|| SshError::Unreachable(format!("no host key received from {host}:{port}")))
}

pub struct Connection {
    handle: Arc<Handle<Checker>>,
    /// which method let us in, for the tests and the log
    pub method: &'static str,
}

pub async fn connect(host: &str, port: u16, host_key: &str, creds: &Credentials) -> Result<Connection, SshError> {
    if host_key.trim().is_empty() {
        return Err(SshError::BadRequest("hostKey is required".into()));
    }
    if creds.username.is_empty() {
        return Err(SshError::BadRequest("username is required".into()));
    }
    let expected = normalize_fingerprint(host_key);
    let seen = Arc::new(Mutex::new(None));
    let stream = tcp(host, port).await?;
    let checker = Checker { expected: Some(expected.clone()), seen: seen.clone() };
    let mut handle = match client::connect_stream(config(), stream, checker).await {
        Ok(h) => h,
        Err(e) => {
            let presented = seen.lock().unwrap().clone();
            return Err(match presented {
                Some(hk) if hk.fingerprint != expected => SshError::HostKeyMismatch { expected, presented: hk.fingerprint },
                _ => SshError::Unreachable(format!("could not reach {host}:{port}: {e}")),
            });
        }
    };
    let method = authenticate(&mut handle, creds).await?;
    Ok(Connection { handle: Arc::new(handle), method })
}

async fn authenticate(h: &mut Handle<Checker>, c: &Credentials) -> Result<&'static str, SshError> {
    let user = c.username.as_str();
    let mut last = String::from("no credentials were accepted");

    // Tailscale SSH (and any server that trusts the network itself) lets you in with nothing
    if h.authenticate_none(user).await?.success() {
        return Ok("none");
    }

    if let Some(seed) = &c.ed25519_seed {
        let key = key_from_seed(seed)?;
        if h.authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), None)).await?.success() {
            return Ok("phone-key");
        }
        last = "the phone key was refused".into();
    }

    if let Some(text) = c.private_key.as_deref().filter(|t| !t.is_empty()) {
        match load_private_key(text, c.passphrase.as_deref()) {
            Ok(key) => {
                // RSA keys sign with rsa-sha2-512/256 when the server says it can (server-sig-algs)
                let hash = if key.algorithm().is_rsa() { h.best_supported_rsa_hash().await?.flatten() } else { None };
                if h.authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), hash)).await?.success() {
                    return Ok("private-key");
                }
                last = "the private key was refused".into();
            }
            Err(e) => last = e.to_string(),
        }
    }

    if let Some(pw) = &c.password {
        if h.authenticate_password(user, pw).await?.success() {
            return Ok("password");
        }
        last = "the password was refused".into();
    }

    Err(SshError::AuthFailed(last))
}

impl Connection {
    pub async fn open_session(&self) -> Result<Channel<Msg>, SshError> {
        self.handle.channel_open_session().await.map_err(|e| SshError::Error(format!("could not open a session: {e}")))
    }

    /// Opens `n` sessions at once on this one connection (SshCore needs a second connection past
    /// the server's MaxSessions, 10 by default).
    pub async fn open_sessions(&self, n: usize) -> Vec<Result<Channel<Msg>, SshError>> {
        let mut set = tokio::task::JoinSet::new();
        for i in 0..n {
            let h = self.handle.clone();
            set.spawn(async move { (i, h.channel_open_session().await.map_err(|e| SshError::Error(e.to_string()))) });
        }
        let mut out: Vec<Option<Result<Channel<Msg>, SshError>>> = (0..n).map(|_| None).collect();
        while let Some(r) = set.join_next().await {
            let (i, ch) = r.expect("open task panicked");
            out[i] = Some(ch);
        }
        out.into_iter().map(|c| c.unwrap()).collect()
    }

    pub async fn exec(&self, command: &str, timeout: Duration) -> Result<ExecResult, SshError> {
        let ch = self.open_session().await?;
        run_exec(ch, command, timeout).await
    }

    /// A shell (or `command`) on a PTY, like the terminal view's.
    pub async fn open_pty(&self, cols: u32, rows: u32, command: Option<&str>) -> Result<Pty, SshError> {
        let ch = self.open_session().await?;
        ch.request_pty(true, "xterm-256color", cols, rows, 0, 0, &[]).await?;
        match command {
            Some(c) => ch.exec(true, c).await?,
            None => ch.request_shell(true).await?,
        }
        Ok(Pty { channel: ch })
    }

    pub async fn disconnect(&self) {
        let _ = self.handle.disconnect(Disconnect::ByApplication, "", "en").await;
    }
}

pub async fn run_exec(mut ch: Channel<Msg>, command: &str, timeout: Duration) -> Result<ExecResult, SshError> {
    ch.exec(true, command).await?;
    let (mut out, mut err, mut code) = (Vec::new(), Vec::new(), None);
    let work = async {
        while let Some(msg) = ch.wait().await {
            match msg {
                ChannelMsg::Data { data } => out.extend_from_slice(&data),
                ChannelMsg::ExtendedData { data, ext: 1 } => err.extend_from_slice(&data),
                ChannelMsg::ExitStatus { exit_status } => code = Some(exit_status),
                _ => {}
            }
        }
    };
    if tokio::time::timeout(timeout, work).await.is_err() {
        return Err(SshError::Error(format!("`{command}` did not finish in {}s", timeout.as_secs())));
    }
    Ok(ExecResult { code: code.unwrap_or(255), stdout: String::from_utf8_lossy(&out).into_owned(), stderr: String::from_utf8_lossy(&err).into_owned() })
}

pub struct Pty {
    channel: Channel<Msg>,
}

impl Pty {
    pub async fn write(&self, data: &[u8]) -> Result<(), SshError> {
        self.channel.data_bytes(data.to_vec()).await.map_err(SshError::from)
    }

    pub async fn resize(&self, cols: u32, rows: u32) -> Result<(), SshError> {
        self.channel.window_change(cols, rows, 0, 0).await.map_err(SshError::from)
    }

    /// Reads output until `pred` holds for everything read so far, or the time is up.
    pub async fn read_until(&mut self, timeout: Duration, pred: impl Fn(&str) -> bool) -> Result<String, SshError> {
        let mut buf = Vec::new();
        let work = async {
            while let Some(msg) = self.channel.wait().await {
                if let ChannelMsg::Data { data } = msg {
                    buf.extend_from_slice(&data);
                    if pred(&String::from_utf8_lossy(&buf)) {
                        return true;
                    }
                }
            }
            false
        };
        match tokio::time::timeout(timeout, work).await {
            Ok(true) => Ok(String::from_utf8_lossy(&buf).into_owned()),
            _ => Err(SshError::Error(format!("PTY output never matched; got {:?}", String::from_utf8_lossy(&buf)))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_like_sshcore() {
        assert_eq!(normalize_fingerprint(" SHA256:abc= "), "SHA256:abc");
    }

    #[test]
    fn seed_key_is_ed25519() {
        let key = key_from_seed(&[7u8; 32]).unwrap();
        assert_eq!(key.algorithm().as_str(), "ssh-ed25519");
        // the same seed always gives the same key, so the phone keeps its identity
        assert_eq!(key.public_key(), key_from_seed(&[7u8; 32]).unwrap().public_key());
    }
}
