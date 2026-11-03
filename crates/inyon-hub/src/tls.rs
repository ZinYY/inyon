//! HTTPS served by the hub itself (TLS_CERT and TLS_KEY, PEM files). Browsers only treat the app
//! as installable, and allow clipboard images and notifications, on HTTPS. Each handshake runs on
//! its own, so a slow client doesn't hold up the others.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_rustls::server::TlsStream;
use tokio_rustls::TlsAcceptor;

/// The TLS config the hub serves, and the certificate's SHA-256 recorded for pairing links.
pub fn config(cert: &str, key: &str) -> Result<Arc<rustls::ServerConfig>, String> {
    let (c, sha256) = load(cert, key)?;
    *SERVED.lock().unwrap_or_else(|e| e.into_inner()) = Some(sha256);
    Ok(c)
}

/// The config, and the SHA-256 of the certificate in it (not of the file, which may change).
fn load(cert: &str, key: &str) -> Result<(Arc<rustls::ServerConfig>, String), String> {
    let certs: Vec<CertificateDer<'static>> =
        CertificateDer::pem_file_iter(cert).map_err(|e| format!("{cert}: {e}"))?.collect::<Result<_, _>>().map_err(|e| format!("{cert}: {e}"))?;
    if certs.is_empty() {
        return Err(format!("{cert} has no certificate"));
    }
    let key = PrivateKeyDer::from_pem_file(key).map_err(|e| format!("{key}: {e}"))?;
    let sha256 = {
        use sha2::{Digest, Sha256};
        crate::http::hex(&Sha256::digest(&certs[0]))
    };
    let mut c = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|e| e.to_string())?;
    c.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok((Arc::new(c), sha256))
}

/// The SHA-256 of the certificate in the TLS config the hub serves, set with that config.
static SERVED: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

pub struct TlsListener {
    rx: mpsc::Receiver<(TlsStream<TcpStream>, SocketAddr)>,
    local: SocketAddr,
}

impl TlsListener {
    pub fn new(listener: TcpListener, cfg: Arc<rustls::ServerConfig>) -> Self {
        let local = listener.local_addr().unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 0)));
        let (tx, rx) = mpsc::channel(64);
        let acceptor = TlsAcceptor::from(cfg);
        tokio::spawn(async move {
            loop {
                let (tcp, addr) = match listener.accept().await {
                    Ok(c) => c,
                    Err(_) => {
                        // out of file descriptors and the like: wait a moment rather than spin
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                };
                let (acceptor, tx) = (acceptor.clone(), tx.clone());
                tokio::spawn(async move {
                    if let Ok(Ok(tls)) = tokio::time::timeout(Duration::from_secs(10), acceptor.accept(tcp)).await {
                        let _ = tx.send((tls, addr)).await;
                    }
                });
            }
        });
        TlsListener { rx, local }
    }
}

impl axum::serve::Listener for TlsListener {
    type Io = TlsStream<TcpStream>;
    type Addr = SocketAddr;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.rx.recv().await {
            Some(c) => c,
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(self.local)
    }
}

/// The certificate a hub serves, as a machine joining it sees it.
pub struct Cert {
    pub pem: String,
    /// sha256 of its DER bytes, as hex: what browsers show as its SHA-256 fingerprint
    pub sha256: String,
}

fn first_cert(file: &str) -> Option<CertificateDer<'static>> {
    if file.is_empty() {
        return None;
    }
    CertificateDer::pem_file_iter(file).ok()?.next()?.ok()
}

fn pem_of(der: &[u8]) -> String {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(der);
    let lines: Vec<&str> = b64.as_bytes().chunks(64).map(|c| std::str::from_utf8(c).unwrap_or("")).collect();
    format!("-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n", lines.join("\n"))
}

/// The first certificate in TLS_CERT, as PEM (nothing else the file may hold).
pub fn served_pem(file: &str) -> Option<String> {
    first_cert(file).map(|c| pem_of(&c))
}

/// The SHA-256 of the certificate the hub serves itself (the first in TLS_CERT, as DER), lowercase
/// hex. None on plain http, and behind a proxy, which serves its own. Taken from the certificate
/// loaded with the TLS config, not the file now, so a file replaced while the hub runs can't put a
/// certificate it doesn't serve in a link. Phone pairing links and companion apps (contract 1.1)
/// carry it.
pub fn served_sha256() -> Option<String> {
    SERVED.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The SHA-256 of the first certificate in a PEM file, lowercase hex.
pub fn sha256_of(file: &str) -> Option<String> {
    use sha2::{Digest, Sha256};
    first_cert(file).map(|der| crate::http::hex(&Sha256::digest(&der)))
}

/// The hub's certificate when it made it itself (setup's LAN choice): a machine can't check it
/// against anyone, so the "Add a machine" command carries its sha256 and the machine trusts
/// exactly that certificate. One from an authority (Let's Encrypt, a company's) gets None.
pub fn self_signed(file: &str) -> Option<Cert> {
    use sha2::{Digest, Sha256};
    let der = first_cert(file)?;
    let (issuer, subject) = issuer_and_subject(&der)?;
    (issuer == subject).then(|| Cert { pem: pem_of(&der), sha256: crate::http::hex(&Sha256::digest(&der)) })
}

/// The issuer and subject names of a DER certificate, read from its TBSCertificate: version
/// (optional, [0]), serial, signature algorithm, issuer, validity, subject.
fn issuer_and_subject(der: &[u8]) -> Option<(&[u8], &[u8])> {
    /// one DER element: its tag, the whole of it, what's inside, and what follows it
    type Element<'a> = (u8, &'a [u8], &'a [u8], &'a [u8]);
    fn tlv(b: &[u8]) -> Option<Element<'_>> {
        let tag = *b.first()?;
        let first = *b.get(1)?;
        let (len, head) = if first < 0x80 {
            (first as usize, 2)
        } else {
            let n = (first & 0x7f) as usize;
            if n == 0 || n > 4 {
                return None;
            }
            let mut len = 0usize;
            for i in 0..n {
                len = (len << 8) | *b.get(2 + i)? as usize;
            }
            (len, 2 + n)
        };
        let end = head.checked_add(len).filter(|e| *e <= b.len())?;
        Some((tag, &b[..end], &b[head..end], &b[end..]))
    }
    let (_, _, cert, _) = tlv(der)?;
    let (_, _, tbs, _) = tlv(cert)?;
    let (tag, _, _, after) = tlv(tbs)?;
    let rest = if tag == 0xa0 { after } else { tbs };
    let (_, _, _, rest) = tlv(rest)?; // serial
    let (_, _, _, rest) = tlv(rest)?; // signature algorithm
    let (_, issuer, _, rest) = tlv(rest)?;
    let (_, _, _, rest) = tlv(rest)?; // validity
    let (_, subject, _, _) = tlv(rest)?;
    Some((issuer, subject))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn openssl(dir: &std::path::Path, script: &str) {
        std::fs::create_dir_all(dir).unwrap();
        let made = std::process::Command::new("sh").arg("-c").arg(script).current_dir(dir).output().expect("sh");
        assert!(made.status.success(), "openssl: {}", String::from_utf8_lossy(&made.stderr));
    }

    #[test]
    fn only_a_certificate_the_hub_made_itself_gets_its_sha256_into_the_command() {
        let dir = std::env::temp_dir().join(format!("inyon-hub-cert-{}", std::process::id()));
        openssl(
            &dir,
            "set -e
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -keyout self.key -out self.pem -days 2 -subj /CN=hub -addext subjectAltName=IP:192.168.1.5
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -keyout ca.key -out ca.pem -days 2 -subj /CN=inyon-test-ca
openssl req -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -keyout leaf.key -out leaf.csr -subj /CN=hub
openssl x509 -req -in leaf.csr -CA ca.pem -CAkey ca.key -CAcreateserial -out leaf.pem -days 2
openssl x509 -in self.pem -outform der > self.der
(sha256sum self.der 2>/dev/null || shasum -a 256 self.der) | cut -c1-64 > self.sha256",
        );
        let path = |f: &str| dir.join(f).to_string_lossy().into_owned();
        let own = self_signed(&path("self.pem")).expect("a self-signed certificate");
        assert_eq!(own.sha256, std::fs::read_to_string(dir.join("self.sha256")).unwrap().trim());
        // the PEM a machine downloads is the same certificate
        let back: Vec<CertificateDer> = CertificateDer::pem_slice_iter(own.pem.as_bytes()).map(Result::unwrap).collect();
        assert_eq!(back, vec![first_cert(&path("self.pem")).unwrap()]);
        assert_eq!(served_pem(&path("self.pem")), Some(own.pem));
        // one an authority signed is checked the usual way
        assert!(self_signed(&path("leaf.pem")).is_none());
        assert!(served_pem(&path("leaf.pem")).is_some());
        assert!(self_signed("").is_none() && served_pem(&path("nothing.pem")).is_none());
        // what phones and companion apps are shown: any served certificate's SHA-256, an authority's too
        assert_eq!(sha256_of(&path("self.pem")).as_deref(), Some(own.sha256.as_str()));
        assert_eq!(sha256_of(&path("leaf.pem")).map(|s| s.len()), Some(64));
        assert!(sha256_of(&path("nothing.pem")).is_none());
        // links carry the certificate the hub loaded, taken from the config rather than the file
        // (load, not config, so other tests never see a fingerprint in SERVED)
        let (_, loaded) = load(&path("self.pem"), &path("self.key")).expect("a TLS config");
        assert_eq!(loaded, own.sha256);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
