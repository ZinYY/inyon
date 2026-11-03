//! The few plain HTTP requests: pkarr relays, Web Push services, downloads of busybox and
//! cloudflared, and the hub's "were we removed?" answer. Blocking requests (ureq, rustls with
//! ring), run off the async threads.

use std::time::Duration;

pub struct Response {
    pub status: u16,
    pub body: Vec<u8>,
}

impl Response {
    pub fn ok(&self) -> bool {
        (200..300).contains(&self.status)
    }
}

/// NODE_TLS_REJECT_UNAUTHORIZED=0, set: certificate checks off for the hub this machine joined (a
/// hub on a self-signed certificate, for testing), as the Bun engine had them. See insecure_tls_for.
pub fn insecure_tls() -> bool {
    std::env::var("NODE_TLS_REJECT_UNAUTHORIZED").is_ok_and(|v| v == "0")
}

/// Whether to skip the certificate check for `url`: only with NODE_TLS_REJECT_UNAUTHORIZED=0, and
/// only for the saved hub. Everything else this program fetches (pkarr relays, push services,
/// GitHub, a hub's own downloads) is still checked, so a variable meant for a test hub can't open
/// all of them to whoever is on the network.
pub fn insecure_tls_for(url: &str) -> bool {
    insecure_tls() && crate::hub_tls::is_saved_hub(url)
}

/// The certificates in the file NODE_EXTRA_CA_CERTS names, trusted besides Mozilla's roots, as
/// the Bun engine did (a hub behind a company's own certificate authority). A file that can't be
/// read is said once and left out, as Node does. A hub's own certificate is never one of them: it's
/// pinned for that hub alone (crate::hub_tls).
pub fn extra_roots() -> Vec<rustls::pki_types::CertificateDer<'static>> {
    node_extra_ca_certs()
}

/// Where `inyon join` keeps the certificate of a hub that made it itself (see pinned_cert).
pub fn hub_cert_file() -> std::path::PathBuf {
    crate::paths::node_dir().join("hub.crt")
}

/// The certificate a hub served at /node/hub.crt, as PEM to keep, if its sha256 is `sha256` (hex,
/// colons allowed, as browsers show it). It may come over a connection nobody checked: the sha256
/// from the hub's "Add a machine" command is what vouches for it.
pub fn pinned_cert(served: &[u8], sha256: &str) -> Result<String, String> {
    use base64::Engine;
    use rustls::pki_types::pem::PemObject;
    use sha2::{Digest, Sha256};
    let want: String = sha256.chars().filter(|c| *c != ':').collect::<String>().to_ascii_lowercase();
    if want.len() != 64 || !want.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("INYON_HUB_CERT must be a certificate's SHA-256 (64 hex digits), not {sha256}."));
    }
    let cert = rustls::pki_types::CertificateDer::pem_slice_iter(served).next().and_then(Result::ok).ok_or("The hub didn't hand out a certificate.")?;
    let got: String = Sha256::digest(&cert).iter().map(|b| format!("{b:02x}")).collect();
    if got != want {
        return Err(format!(
            "The hub's certificate has SHA-256 {got}, not the {want} its command named, so this machine wasn't added. Get a new command from the hub, and check the address."
        ));
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(&cert);
    let lines: Vec<&str> = b64.as_bytes().chunks(64).map(|c| std::str::from_utf8(c).unwrap_or("")).collect();
    Ok(format!("-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n", lines.join("\n")))
}

fn node_extra_ca_certs() -> Vec<rustls::pki_types::CertificateDer<'static>> {
    static SAID: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    let Some(path) = std::env::var_os("NODE_EXTRA_CA_CERTS").filter(|p| !p.is_empty()) else {
        return Vec::new();
    };
    let path = std::path::PathBuf::from(path);
    match pem_certs(&path) {
        Ok(certs) => certs,
        Err(e) => {
            if !SAID.swap(true, std::sync::atomic::Ordering::Relaxed) {
                crate::output::log(&format!("Ignoring NODE_EXTRA_CA_CERTS: couldn't read certificates from {} ({e}).", path.display()));
            }
            Vec::new()
        }
    }
}

fn pem_certs(path: &std::path::Path) -> Result<Vec<rustls::pki_types::CertificateDer<'static>>, String> {
    use rustls::pki_types::pem::PemObject;
    let certs = rustls::pki_types::CertificateDer::pem_file_iter(path).map_err(|e| e.to_string())?.collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
    if certs.is_empty() {
        return Err("there are none in it".into());
    }
    Ok(certs)
}

/// The roots ureq checks a server's certificate against: Mozilla's, and NODE_EXTRA_CA_CERTS.
pub fn ureq_roots() -> ureq::tls::RootCerts {
    let extra = extra_roots();
    if extra.is_empty() {
        return ureq::tls::RootCerts::WebPki;
    }
    let all: Vec<ureq::tls::Certificate<'static>> =
        webpki_root_certs::TLS_SERVER_ROOT_CERTS.iter().chain(&extra).map(|c| ureq::tls::Certificate::from_der(c.as_ref()).to_owned()).collect();
    ureq::tls::RootCerts::new_with_certs(&all)
}

/// What may come back: more than cloudflared (about 40 MB), less than would fill memory.
const MAX_BODY: u64 = 256 * 1024 * 1024;

/// How long opening a connection may take, every address of the name and the TLS handshake
/// included. ureq gives the first address half of it and each next one half of what's left, so an
/// address that never answers (GitHub's CDN hands out a dead one now and then) costs a few seconds
/// before the next is tried, not the two minutes the system would wait.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// How long a request may wait for its next byte, either way, before it's given up: a download
/// that stalls fails after this rather than at the end of its whole time.
pub const STALL_TIMEOUT: Duration = Duration::from_secs(30);

/// A request's time limits: in all, to connect, and between two reads or writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub total: Duration,
    pub connect: Duration,
    pub stall: Duration,
}

impl Limits {
    /// `total` in all, with the usual connect and stall limits (never longer than `total`).
    pub fn new(total: Duration) -> Limits {
        Limits { total, connect: CONNECT_TIMEOUT.min(total), stall: STALL_TIMEOUT.min(total) }
    }
}

/// An agent for `config` that keeps to `limits`. Every request this program makes goes through
/// one (the inyon command's too), so none of them can hang on a dead address or a stalled answer.
pub fn timed_agent(config: ureq::config::ConfigBuilder<ureq::typestate::AgentScope>, limits: Limits) -> ureq::Agent {
    timed_agent_with(config, limits, ureq::unversioned::resolver::DefaultResolver::default())
}

/// `timed_agent` with a resolver of its own (the tests hand out a dead address first).
pub fn timed_agent_with(config: ureq::config::ConfigBuilder<ureq::typestate::AgentScope>, limits: Limits, resolver: impl ureq::unversioned::resolver::Resolver) -> ureq::Agent {
    timed_agent_pinned(config, limits, resolver, None)
}

/// `timed_agent_with`, and with `pin` the TLS of a hub whose certificate is pinned (crate::hub_tls).
pub fn timed_agent_pinned(
    config: ureq::config::ConfigBuilder<ureq::typestate::AgentScope>,
    limits: Limits,
    resolver: impl ureq::unversioned::resolver::Resolver,
    pin: Option<std::sync::Arc<rustls::ClientConfig>>,
) -> ureq::Agent {
    let config = config.timeout_global(Some(limits.total)).timeout_resolve(Some(limits.connect)).timeout_connect(Some(limits.connect)).build();
    ureq::Agent::with_parts(config, stall::Connector::new(limits.stall, pin), resolver)
}

/// An agent for a request to `url`: the hub's pin when `url` is the hub joined and its certificate
/// was kept, the usual roots for everything else.
pub fn timed_agent_for(url: &str, config: ureq::config::ConfigBuilder<ureq::typestate::AgentScope>, limits: Limits) -> ureq::Agent {
    timed_agent_to(url, config, limits, ureq::unversioned::resolver::DefaultResolver::default())
}

fn timed_agent_to(
    url: &str,
    config: ureq::config::ConfigBuilder<ureq::typestate::AgentScope>,
    limits: Limits,
    resolver: impl ureq::unversioned::resolver::Resolver,
) -> ureq::Agent {
    let pin = if insecure_tls_for(url) { None } else { crate::hub_tls::config_for(url) };
    timed_agent_pinned(config, limits, resolver, pin)
}

/// A name that resolves to these addresses only: ones already checked (crate::hub::check_name),
/// so a second lookup can't send the request somewhere else.
#[derive(Debug)]
pub struct Addrs(pub Vec<std::net::SocketAddr>);

impl ureq::unversioned::resolver::Resolver for Addrs {
    fn resolve(
        &self,
        _uri: &ureq::http::Uri,
        _config: &ureq::config::Config,
        _timeout: ureq::unversioned::transport::NextTimeout,
    ) -> Result<ureq::unversioned::resolver::ResolvedSocketAddrs, ureq::Error> {
        let mut v = self.empty();
        for a in &self.0 {
            v.push(*a);
        }
        Ok(v)
    }
}

/// A request that gave up because nothing came for a while says so, and a connection that
/// couldn't be opened says how long it was given.
fn describe(e: ureq::Error, limits: &Limits) -> String {
    match e {
        ureq::Error::Timeout(ureq::Timeout::Connect) => format!("couldn't connect within {} s", limits.connect.as_secs()),
        e => e.to_string(),
    }
}

/// Caps every wait for a byte at the stall limit. ureq's own timeouts count from the start of a
/// phase (the whole body, say), so a body that stops arriving halfway would otherwise hold the
/// request until its total time ran out.
mod stall {
    use std::time::Duration;

    use ureq::unversioned::transport::time::Duration as UDuration;
    use ureq::unversioned::transport::{Buffers, ChainedConnector, ConnectionDetails, Connector as _, DefaultConnector, NextTimeout, TcpConnector, Transport};

    use crate::hub_tls::ureq_tls::Tls;

    #[derive(Debug)]
    enum Inner {
        Default(DefaultConnector),
        /// a pinned hub: TCP, then TLS that accepts its certificate
        Pinned(ChainedConnector<(), TcpConnector, Tls>),
    }

    #[derive(Debug)]
    pub struct Connector {
        inner: Inner,
        stall: Duration,
    }

    impl Connector {
        pub fn new(stall: Duration, pin: Option<std::sync::Arc<rustls::ClientConfig>>) -> Connector {
            let inner = match pin {
                Some(c) => Inner::Pinned(TcpConnector::default().chain(Tls(c))),
                None => Inner::Default(DefaultConnector::new()),
            };
            Connector { inner, stall }
        }
    }

    impl ureq::unversioned::transport::Connector<()> for Connector {
        type Out = Stalling;

        fn connect(&self, details: &ConnectionDetails, chained: Option<()>) -> Result<Option<Stalling>, ureq::Error> {
            let inner: Option<Box<dyn Transport>> = match &self.inner {
                Inner::Default(c) => c.connect(details, chained)?,
                Inner::Pinned(c) => c.connect(details, chained)?.map(|t| t.boxed()),
            };
            Ok(inner.map(|inner| Stalling { inner, stall: self.stall }))
        }
    }

    #[derive(Debug)]
    pub struct Stalling {
        inner: Box<dyn Transport>,
        stall: Duration,
    }

    impl Stalling {
        /// The wait ureq asked for, or the stall limit when that's sooner; and whether it was cut.
        fn cap(&self, t: NextTimeout) -> (NextTimeout, bool) {
            if *t.after > self.stall {
                (NextTimeout { after: UDuration::Exact(self.stall), reason: t.reason }, true)
            } else {
                (t, false)
            }
        }

        fn stalled(&self, e: ureq::Error, cut: bool) -> ureq::Error {
            match e {
                ureq::Error::Timeout(_) if cut => ureq::Error::Io(std::io::Error::new(std::io::ErrorKind::TimedOut, format!("nothing arrived for {} s", self.stall.as_secs()))),
                e => e,
            }
        }
    }

    impl Transport for Stalling {
        fn buffers(&mut self) -> &mut dyn Buffers {
            self.inner.buffers()
        }

        fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), ureq::Error> {
            let (t, cut) = self.cap(timeout);
            self.inner.transmit_output(amount, t).map_err(|e| self.stalled(e, cut))
        }

        fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, ureq::Error> {
            let (t, cut) = self.cap(timeout);
            self.inner.await_input(t).map_err(|e| self.stalled(e, cut))
        }

        fn is_open(&mut self) -> bool {
            self.inner.is_open()
        }

        fn is_tls(&self) -> bool {
            self.inner.is_tls()
        }
    }
}

fn config(insecure: bool) -> ureq::config::ConfigBuilder<ureq::typestate::AgentScope> {
    let c = ureq::Agent::config_builder().http_status_as_error(false).user_agent(format!("inyon/{}", crate::VERSION));
    if insecure {
        // unchecked for the hub only: a redirect elsewhere would go unchecked too, so none is
        // followed (the 3xx comes back as the answer)
        return c.max_redirects(0).max_redirects_will_error(false).tls_config(ureq::tls::TlsConfig::builder().disable_verification(true).build());
    }
    c.tls_config(ureq::tls::TlsConfig::builder().root_certs(ureq_roots()).build())
}

/// A request and its whole answer. `headers` go with it; `body` makes it a request with one.
pub fn request(method: &str, url: &str, headers: &[(&str, &str)], body: Option<&[u8]>, timeout: Duration) -> Result<Response, String> {
    let limits = Limits::new(timeout);
    request_with(timed_agent_for(url, config(insecure_tls_for(url)), limits), &limits, method, url, headers, body, false)
}

/// `request`, sent only to `addrs` (the hub's name, already looked up and checked), with the name
/// kept for the Host header and the certificate.
pub fn request_to(method: &str, url: &str, headers: &[(&str, &str)], timeout: Duration, addrs: Vec<std::net::SocketAddr>) -> Result<Response, String> {
    let limits = Limits::new(timeout);
    request_with(timed_agent_to(url, config(insecure_tls_for(url)), limits, Addrs(addrs)), &limits, method, url, headers, None, false)
}

/// A GET to the hub on this machine over loopback (`inyon hub code`). Its certificate is made out
/// to the hub's name, not 127.0.0.1, so it isn't checked, as the Bun engine's
/// `rejectUnauthorized: false` didn't check it.
pub fn get_local(url: &str, timeout: Duration) -> Result<Response, String> {
    let limits = Limits::new(timeout);
    request_with(timed_agent(config(true), limits), &limits, "GET", url, &[], None, false)
}

/// A POST with no body to the hub on this machine, unchecked like `get_local` (`inyon hub
/// companion-code`). The local token goes in `headers`, never in the address, where a proxy's log
/// would keep it.
pub fn post_local(url: &str, headers: &[(&str, &str)], timeout: Duration) -> Result<Response, String> {
    let limits = Limits::new(timeout);
    request_with(timed_agent(config(true), limits), &limits, "POST", url, headers, Some(&[]), false)
}

/// Sends the request and reads the answer. With `whole`, a body that breaks off is an error
/// rather than whatever arrived (a download is checked against its checksum after this).
fn request_with(a: ureq::Agent, limits: &Limits, method: &str, url: &str, headers: &[(&str, &str)], body: Option<&[u8]>, whole: bool) -> Result<Response, String> {
    let mut res = match (method, body) {
        ("GET", _) => {
            let mut r = a.get(url);
            for (k, v) in headers {
                r = r.header(*k, *v);
            }
            r.call()
        }
        (m, b) => {
            let b = b.unwrap_or(&[]);
            let mut r = match m {
                "PUT" => a.put(url),
                _ => a.post(url),
            };
            for (k, v) in headers {
                r = r.header(*k, *v);
            }
            r.send(b)
        }
    }
    .map_err(|e| describe(e, limits))?;
    let status = res.status().as_u16();
    let read = res.body_mut().with_config().limit(MAX_BODY).read_to_vec();
    let body = match read {
        Ok(b) => b,
        Err(e) if whole => return Err(describe(e, limits)),
        Err(_) => Vec::new(),
    };
    Ok(Response { status, body })
}

/// `request` on a blocking thread, for async code.
pub async fn fetch(method: &'static str, url: String, headers: Vec<(String, String)>, body: Option<Vec<u8>>, timeout: Duration) -> Result<Response, String> {
    tokio::task::spawn_blocking(move || {
        let h: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        request(method, &url, &h, body.as_deref(), timeout)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// `request_to` on a blocking thread, for async code.
pub async fn fetch_to(url: String, headers: Vec<(String, String)>, timeout: Duration, addrs: Vec<std::net::SocketAddr>) -> Result<Response, String> {
    tokio::task::spawn_blocking(move || {
        let h: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        request_to("GET", &url, &h, timeout, addrs)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The longest a download may take in all, however steadily it arrives.
pub const DOWNLOAD_TIME: Duration = Duration::from_secs(600);

/// Downloads a file whole (a program to check before it's used).
pub fn download(url: &str) -> Result<Response, String> {
    download_with(url, Limits::new(DOWNLOAD_TIME), ureq::unversioned::resolver::DefaultResolver::default())
}

fn download_with(url: &str, limits: Limits, resolver: impl ureq::unversioned::resolver::Resolver) -> Result<Response, String> {
    request_with(timed_agent_to(url, config(insecure_tls_for(url)), limits, resolver), &limits, "GET", url, &[], None, true)
}

/// A server on a certificate from a certificate authority of its own, made with openssl for the
/// test (no key is kept in the repository), and whether a client trusts it.
#[cfg(all(test, target_os = "linux"))]
pub(crate) mod test_tls {
    use std::io::{Read, Write};
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex, MutexGuard};

    use rustls::pki_types::pem::PemObject;
    use rustls::pki_types::{CertificateDer, PrivateKeyDer};

    /// The tests that set NODE_EXTRA_CA_CERTS take turns.
    pub fn env_lock() -> MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The authority's certificate and the port of a server answering "ok" over TLS, as 127.0.0.1.
    pub fn server(dir: &Path) -> (PathBuf, u16) {
        std::fs::create_dir_all(dir).unwrap();
        let script = "set -e
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -keyout ca.key -out ca.pem -days 2 -subj /CN=inyon-test-ca -addext basicConstraints=critical,CA:TRUE -addext keyUsage=critical,keyCertSign,cRLSign
openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -keyout server.key -out server.csr -subj /CN=localhost
printf 'subjectAltName=DNS:localhost,IP:127.0.0.1\\nbasicConstraints=critical,CA:FALSE\\nkeyUsage=critical,digitalSignature\\nextendedKeyUsage=serverAuth\\n' > ext
openssl x509 -req -in server.csr -CA ca.pem -CAkey ca.key -CAcreateserial -out server.pem -days 2 -extfile ext";
        (dir.join("ca.pem"), serve(dir, script))
    }

    /// A server on a certificate it made itself, as `inyon setup` makes one for a hub on the LAN:
    /// the certificate (the one a machine keeps as hub.crt) and the port. Not marked as an
    /// authority, as setup's isn't: a certificate that is one can't also be the server's.
    pub fn self_signed_server(dir: &Path) -> (PathBuf, u16) {
        std::fs::create_dir_all(dir).unwrap();
        let script = "openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -keyout server.key -out server.pem -days 2 -subj /CN=hub -addext subjectAltName=DNS:localhost,IP:127.0.0.1 -addext extendedKeyUsage=serverAuth -addext basicConstraints=critical,CA:FALSE";
        (dir.join("server.pem"), serve(dir, script))
    }

    /// A server for localhost and 127.0.0.1 on a certificate signed with the key of the hub's own
    /// certificate in `hub_dir`, as whoever has that key could make one for any name.
    pub fn signed_by_hub_server(dir: &Path, hub_dir: &Path) -> u16 {
        std::fs::create_dir_all(dir).unwrap();
        let script = format!(
            "openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -keyout server.key -out server.csr -subj /CN=localhost && printf 'subjectAltName=DNS:localhost,IP:127.0.0.1\\nextendedKeyUsage=serverAuth\\n' > ext && openssl x509 -req -in server.csr -CA '{0}/server.pem' -CAkey '{0}/server.key' -CAcreateserial -out server.pem -days 2 -extfile ext",
            hub_dir.display()
        );
        serve(dir, &script)
    }

    /// Runs `script` in `dir` to make server.pem and server.key, and serves "ok" over TLS with them.
    fn serve(dir: &Path, script: &str) -> u16 {
        let made = std::process::Command::new("sh").arg("-c").arg(script).current_dir(dir).output().expect("sh");
        assert!(made.status.success(), "openssl: {}", String::from_utf8_lossy(&made.stderr));
        let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_file_iter(dir.join("server.pem")).unwrap().map(Result::unwrap).collect();
        let key = PrivateKeyDer::from_pem_file(dir.join("server.key")).unwrap();
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let config = Arc::new(
            rustls::ServerConfig::builder_with_provider(provider).with_safe_default_protocol_versions().unwrap().with_no_client_auth().with_single_cert(certs, key).unwrap(),
        );
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for tcp in listener.incoming().flatten() {
                let config = config.clone();
                std::thread::spawn(move || {
                    let Ok(conn) = rustls::ServerConnection::new(config) else { return };
                    let mut s = rustls::StreamOwned::new(conn, tcp);
                    let mut seen = Vec::new();
                    let mut buf = [0u8; 1024];
                    while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                        match s.read(&mut buf) {
                            Ok(0) | Err(_) => return,
                            Ok(n) => seen.extend_from_slice(&buf[..n]),
                        }
                    }
                    let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
                    let _ = s.flush();
                    s.conn.send_close_notify();
                    let _ = s.flush();
                });
            }
        });
        port
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn a_hub_on_a_certificate_from_node_extra_ca_certs_is_trusted() {
        let _env = test_tls::env_lock();
        let dir = std::env::temp_dir().join(format!("inyon-extra-ca-http-{}", std::process::id()));
        let (ca, port) = test_tls::server(&dir);
        let url = format!("https://127.0.0.1:{port}/");
        // SAFETY: the tests that touch NODE_EXTRA_CA_CERTS hold env_lock
        unsafe { std::env::remove_var("NODE_EXTRA_CA_CERTS") };
        let unknown = request("GET", &url, &[], None, Duration::from_secs(10));
        unsafe { std::env::set_var("NODE_EXTRA_CA_CERTS", &ca) };
        let trusted = request("GET", &url, &[], None, Duration::from_secs(10));
        // a file that isn't there leaves Mozilla's roots as they are
        unsafe { std::env::set_var("NODE_EXTRA_CA_CERTS", dir.join("missing.pem")) };
        let missing = request("GET", &url, &[], None, Duration::from_secs(10));
        unsafe { std::env::remove_var("NODE_EXTRA_CA_CERTS") };
        let _ = std::fs::remove_dir_all(&dir);
        assert!(unknown.is_err(), "an authority nobody named isn't trusted");
        let trusted = trusted.unwrap();
        assert_eq!((trusted.status, trusted.body.as_slice()), (200, &b"ok"[..]));
        assert!(missing.is_err());
    }

    #[test]
    fn join_keeps_only_a_certificate_with_the_sha256_the_command_named() {
        let dir = std::env::temp_dir().join(format!("inyon-kept-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ok = std::process::Command::new("sh")
            .arg("-c")
            .arg("openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -keyout server.key -out server.pem -days 2 -subj /CN=hub 2>/dev/null")
            .current_dir(&dir)
            .status()
            .unwrap();
        assert!(ok.success());
        let served = std::fs::read(dir.join("server.pem")).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let der = <rustls::pki_types::CertificateDer as rustls::pki_types::pem::PemObject>::from_pem_slice(&served).unwrap();
        let sha: String = <sha2::Sha256 as sha2::Digest>::digest(&der).iter().map(|b| format!("{b:02x}")).collect();
        assert!(pinned_cert(&served, &sha).is_ok());
        assert!(pinned_cert(&served, &"0".repeat(64)).is_err());
        assert!(pinned_cert(b"not a certificate", &sha).is_err());
    }

    #[test]
    fn the_certificate_join_kept_is_pinned_for_that_hub_alone_and_vouches_for_nothing_else() {
        let _env = test_tls::env_lock();
        let home = crate::paths::testing::home("hub-pin");
        // SAFETY: the tests that touch NODE_EXTRA_CA_CERTS hold env_lock
        unsafe { std::env::remove_var("NODE_EXTRA_CA_CERTS") };
        let (cert, port) = test_tls::self_signed_server(&home.dir.join("hub"));
        let forged_port = test_tls::signed_by_hub_server(&home.dir.join("forged"), &home.dir.join("hub"));
        let get = |url: String| request("GET", &url, &[], None, Duration::from_secs(10)).map(|r| r.status);
        let hub = format!("https://127.0.0.1:{port}");
        // before joining: nobody vouches for it
        assert!(get(format!("{hub}/")).is_err());
        // what `inyon join` keeps, checked against the command's sha256, and the hub it saved
        let served = std::fs::read(&cert).unwrap();
        let der = <rustls::pki_types::CertificateDer as rustls::pki_types::pem::PemObject>::from_pem_slice(&served).unwrap();
        let sha: String = <sha2::Sha256 as sha2::Digest>::digest(&der).iter().map(|b| format!("{b:02x}")).collect();
        std::fs::create_dir_all(crate::paths::node_dir()).unwrap();
        std::fs::write(hub_cert_file(), pinned_cert(&served, &sha).unwrap()).unwrap();
        let save = |hub: &str| std::fs::write(crate::paths::node_dir().join(if crate::WIN { ".env" } else { "env" }), format!("INYON_HUB={hub}\nINYON_TOKEN=t\n")).unwrap();
        save(&hub);
        assert_eq!(get(format!("{hub}/node")), Ok(200));
        // the same certificate under another name isn't the hub joined
        assert!(get(format!("https://localhost:{port}/")).is_err());
        // a certificate its key signed, for this machine's own name: no authority, anywhere
        assert!(get(format!("https://127.0.0.1:{forged_port}/")).is_err());
        assert!(get(format!("https://localhost:{forged_port}/")).is_err());
        // not even as the hub: only the very certificate counts
        save(&format!("https://127.0.0.1:{forged_port}"));
        assert!(get(format!("https://127.0.0.1:{forged_port}/")).is_err());
    }

    #[test]
    fn node_tls_reject_unauthorized_turns_checks_off_for_the_joined_hub_only() {
        let _env = test_tls::env_lock();
        let home = crate::paths::testing::home("insecure-tls");
        // SAFETY: the tests that touch these variables hold env_lock
        unsafe { std::env::remove_var("NODE_EXTRA_CA_CERTS") };
        let (_hub_cert, hub_port) = test_tls::self_signed_server(&home.dir.join("hub"));
        let (_other_cert, other_port) = test_tls::self_signed_server(&home.dir.join("relay"));
        let get = |url: String| request("GET", &url, &[], None, Duration::from_secs(10)).map(|r| r.status);
        let hub = format!("https://127.0.0.1:{hub_port}");
        std::fs::create_dir_all(crate::paths::node_dir()).unwrap();
        std::fs::write(crate::paths::node_dir().join(if crate::WIN { ".env" } else { "env" }), format!("INYON_HUB={hub}\nINYON_TOKEN=t\n")).unwrap();
        unsafe { std::env::set_var("NODE_TLS_REJECT_UNAUTHORIZED", "0") };
        let joined = get(format!("{hub}/node"));
        // a relay, a push service, a download: still checked
        let elsewhere = get(format!("https://127.0.0.1:{other_port}/"));
        let downloaded = download(&format!("https://127.0.0.1:{other_port}/inyon")).map(|r| r.status);
        unsafe { std::env::remove_var("NODE_TLS_REJECT_UNAUTHORIZED") };
        let checked = get(format!("{hub}/node"));
        assert_eq!(joined, Ok(200));
        assert!(elsewhere.is_err() && downloaded.is_err(), "{elsewhere:?} {downloaded:?}");
        assert!(checked.is_err());
        // and a redirect from the hub isn't followed with the checks off
        let unchecked = config(true).build();
        assert_eq!((unchecked.max_redirects(), unchecked.max_redirects_will_error()), (0, false));
        assert!(config(false).build().max_redirects() > 0);
    }

    #[test]
    fn the_hub_on_this_machine_is_asked_whatever_name_its_certificate_has() {
        let dir = std::env::temp_dir().join(format!("inyon-local-hub-http-{}", std::process::id()));
        let (_ca, port) = test_tls::server(&dir);
        let url = format!("https://127.0.0.1:{port}/");
        let got = get_local(&url, Duration::from_secs(10));
        let _ = std::fs::remove_dir_all(&dir);
        let got = got.unwrap();
        assert_eq!((got.status, got.body.as_slice()), (200, &b"ok"[..]));
    }
}

#[cfg(test)]
mod timeout_tests {
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpListener};
    use std::time::Instant;

    use super::*;

    /// A server on 127.0.0.1 that reads one request per connection and hands it to `answer`.
    fn server(answer: impl Fn(&mut std::net::TcpStream) + Send + Sync + 'static) -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let answer = std::sync::Arc::new(answer);
        std::thread::spawn(move || {
            for mut tcp in listener.incoming().flatten() {
                let answer = answer.clone();
                std::thread::spawn(move || {
                    let mut seen = Vec::new();
                    let mut buf = [0u8; 1024];
                    while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                        match tcp.read(&mut buf) {
                            Ok(0) | Err(_) => return,
                            Ok(n) => seen.extend_from_slice(&buf[..n]),
                        }
                    }
                    answer(&mut tcp);
                });
            }
        });
        port
    }

    #[test]
    fn a_request_to_checked_addresses_does_not_look_the_name_up_again_and_keeps_it_as_the_host() {
        let seen = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
        let s = seen.clone();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for mut c in listener.incoming().flatten() {
                let mut req = Vec::new();
                let mut buf = [0u8; 1024];
                while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                    match c.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => req.extend_from_slice(&buf[..n]),
                    }
                }
                *s.lock().unwrap() =
                    String::from_utf8_lossy(&req).lines().find_map(|l| l.strip_prefix("host: ").or(l.strip_prefix("Host: ")).map(String::from)).unwrap_or_default();
                let _ = c.write_all(b"HTTP/1.1 410 Gone\r\ncontent-length: 0\r\nconnection: close\r\n\r\n");
            }
        });
        let url = format!("http://hub.invalid:{port}/node");
        assert!(request("GET", &url, &[], None, Duration::from_secs(5)).is_err());
        let r = request_to("GET", &url, &[("authorization", "Bearer t")], Duration::from_secs(5), vec![format!("127.0.0.1:{port}").parse().unwrap()]).unwrap();
        assert_eq!(r.status, 410);
        assert_eq!(*seen.lock().unwrap(), format!("hub.invalid:{port}"));
    }

    #[test]
    fn a_download_moves_on_from_an_address_that_never_answers() {
        // 192.0.2.1 is kept for documentation: nothing answers it, so a connect there waits until
        // the system gives up (two minutes on Linux), as one of GitHub's addresses did in 0.19.0
        let port = server(|tcp| {
            let _ = tcp.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 11\r\nConnection: close\r\n\r\ncloudflared");
        });
        let addrs = vec![SocketAddr::from(([192, 0, 2, 1], port)), SocketAddr::from(([127, 0, 0, 1], port))];
        let started = Instant::now();
        let got = download_with(&format!("http://downloads.example:{port}/cloudflared"), Limits::new(DOWNLOAD_TIME), Addrs(addrs)).unwrap();
        let took = started.elapsed();
        assert_eq!((got.status, got.body.as_slice()), (200, &b"cloudflared"[..]));
        // the dead address gets about two thirds of the 10 s connect limit, then the next one is tried
        assert!(took < Duration::from_secs(12), "took {took:?}");
    }

    #[test]
    fn a_download_that_stops_arriving_fails_once_nothing_came_for_the_stall_limit() {
        let port = server(|tcp| {
            let _ = tcp.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\nConnection: close\r\n\r\npart of it");
            let _ = tcp.flush();
            std::thread::sleep(Duration::from_secs(20));
        });
        let limits = Limits { stall: Duration::from_secs(1), ..Limits::new(DOWNLOAD_TIME) };
        let started = Instant::now();
        let got = download_with(&format!("http://127.0.0.1:{port}/cloudflared"), limits, ureq::unversioned::resolver::DefaultResolver::default());
        let took = started.elapsed();
        let err = got.err().expect("a body that broke off is an error, not a short file");
        assert!(err.contains("nothing arrived for 1 s"), "{err}");
        assert!(took < Duration::from_secs(8), "took {took:?}");
    }

    #[test]
    fn a_server_that_never_answers_is_given_up_on_after_the_stall_limit() {
        let port = server(|_| std::thread::sleep(Duration::from_secs(20)));
        let limits = Limits { stall: Duration::from_secs(1), ..Limits::new(DOWNLOAD_TIME) };
        let started = Instant::now();
        let err = download_with(&format!("http://127.0.0.1:{port}/"), limits, ureq::unversioned::resolver::DefaultResolver::default()).err().unwrap();
        assert!(err.contains("nothing arrived for 1 s"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(8));
    }

    #[test]
    fn short_requests_keep_their_own_total() {
        assert_eq!(Limits::new(Duration::from_millis(700)), Limits { total: Duration::from_millis(700), connect: Duration::from_millis(700), stall: Duration::from_millis(700) });
        assert_eq!(Limits::new(DOWNLOAD_TIME), Limits { total: DOWNLOAD_TIME, connect: CONNECT_TIMEOUT, stall: STALL_TIMEOUT });
    }
}
