//! The certificate `inyon setup` makes for a hub that answers on the LAN. Plain http there would
//! hand the password, the session and every machine's token to anyone on the Wi-Fi, so the hub
//! serves HTTPS on a certificate of its own: no authority vouches for it, so setup shows its
//! SHA-256 for you to compare in the browser, and the hub puts it in every "Add a machine" command.

use std::net::IpAddr;
use std::path::{Path, PathBuf};

pub struct Made {
    pub cert: PathBuf,
    pub key: PathBuf,
}

/// The names and addresses the certificate is good for: the machine's name (and name.local, which
/// is how macOS and most Linux machines are found on a LAN), localhost, and every address given.
pub fn names(hostname: &str, ips: &[IpAddr]) -> Vec<String> {
    let host = hostname.trim().trim_end_matches(".local").to_ascii_lowercase();
    let mut v = Vec::new();
    let valid = !host.is_empty() && host.split('.').all(|l| !l.is_empty() && l.len() <= 63 && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-'));
    if valid {
        v.push(host.clone());
        if !host.contains('.') {
            v.push(format!("{host}.local"));
        }
    }
    v.push("localhost".into());
    let mut addrs: Vec<IpAddr> = vec![IpAddr::from([127, 0, 0, 1]), IpAddr::from(std::net::Ipv6Addr::LOCALHOST)];
    addrs.extend(ips.iter().copied());
    for ip in addrs {
        let s = ip.to_string();
        if !v.contains(&s) {
            v.push(s);
        }
    }
    v
}

/// Makes a certificate and key for `names` in `dir` (hub.crt, hub.key, readable only by you),
/// good for ten years: a new one would mean adding every machine again.
pub fn make(dir: &Path, names: &[String]) -> Result<Made, String> {
    use rcgen::{CertificateParams, DnType, ExtendedKeyUsagePurpose, KeyPair, KeyUsagePurpose};
    let year = 1970 + (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) / 31_556_952) as i32;
    let mut params = CertificateParams::new(names.to_vec()).map_err(|e| e.to_string())?;
    let first = names.first().map(String::as_str).unwrap_or("localhost");
    params.distinguished_name.push(DnType::CommonName, format!("Inyon hub on {first}"));
    params.not_before = rcgen::date_time_ymd(year - 1, 1, 1);
    params.not_after = rcgen::date_time_ymd(year + 10, 1, 1);
    params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
    params.extended_key_usages = vec![ExtendedKeyUsagePurpose::ServerAuth];
    let key = KeyPair::generate().map_err(|e| e.to_string())?;
    let cert = params.self_signed(&key).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    let made = Made { cert: dir.join("hub.crt"), key: dir.join("hub.key") };
    write_private(&made.key, &key.serialize_pem()).map_err(|e| format!("{}: {e}", made.key.display()))?;
    write_private(&made.cert, &cert.pem()).map_err(|e| format!("{}: {e}", made.cert.display()))?;
    Ok(made)
}

/// The SHA-256 fingerprint of the first certificate in a PEM file, as browsers show it (AB:CD:…).
pub fn fingerprint(cert: &Path) -> Option<String> {
    use rustls_pki_types::pem::PemObject;
    use sha2::{Digest, Sha256};
    let der = rustls_pki_types::CertificateDer::pem_file_iter(cert).ok()?.next()?.ok()?;
    Some(Sha256::digest(&der).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":"))
}

/// The same fingerprint for a certificate given as PEM text (what WebView2 hands over).
pub fn fingerprint_pem(pem: &str) -> Option<String> {
    use rustls_pki_types::pem::PemObject;
    use sha2::{Digest, Sha256};
    let der = rustls_pki_types::CertificateDer::from_pem_slice(pem.as_bytes()).ok()?;
    Some(Sha256::digest(&der).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":"))
}

fn write_private(file: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    let _ = std::fs::remove_file(file);
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(file)?.write_all(text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hub answering 200 over TLS on the certificate, as the hub would serve it.
    fn serve(made: &Made) -> u16 {
        use rustls::pki_types::pem::PemObject;
        use std::io::{Read, Write};
        let certs = rustls::pki_types::CertificateDer::pem_file_iter(&made.cert).unwrap().map(Result::unwrap).collect();
        let key = rustls::pki_types::PrivateKeyDer::from_pem_file(&made.key).unwrap();
        let provider = std::sync::Arc::new(rustls::crypto::ring::default_provider());
        let config = std::sync::Arc::new(
            rustls::ServerConfig::builder_with_provider(provider).with_safe_default_protocol_versions().unwrap().with_no_client_auth().with_single_cert(certs, key).unwrap(),
        );
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for tcp in listener.incoming().flatten() {
                let Ok(conn) = rustls::ServerConnection::new(config.clone()) else { continue };
                let mut s = rustls::StreamOwned::new(conn, tcp);
                let mut seen = Vec::new();
                let mut buf = [0u8; 1024];
                while !seen.windows(4).any(|w| w == b"\r\n\r\n") {
                    match s.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => seen.extend_from_slice(&buf[..n]),
                    }
                }
                let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
                s.conn.send_close_notify();
                let _ = s.flush();
            }
        });
        port
    }

    #[test]
    fn the_certificate_covers_the_machines_name_and_addresses() {
        let n = names("Studio", &["192.168.1.20".parse().unwrap(), "100.97.1.2".parse().unwrap()]);
        assert_eq!(n, ["studio", "studio.local", "localhost", "127.0.0.1", "::1", "192.168.1.20", "100.97.1.2"]);
        assert_eq!(names("box.local", &[])[..2], ["box".to_string(), "box.local".to_string()]);
        // a name a certificate can't hold is left out, the addresses stay
        assert_eq!(names("my box!", &[])[0], "localhost");
    }

    #[test]
    fn it_is_self_signed_for_those_names_with_a_key_only_you_can_read() {
        let dir = std::env::temp_dir().join(format!("inyon-hub-cert-{}", std::process::id()));
        let made = make(&dir, &names("studio", &["192.168.1.20".parse().unwrap()])).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&made.key).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(std::fs::metadata(&made.cert).unwrap().permissions().mode() & 0o777, 0o600);
        }
        let fp = fingerprint(&made.cert).unwrap();
        assert_eq!(fp.len(), 95);
        // the window's view checks the PEM WebView2 hands over against the same fingerprint
        assert_eq!(fingerprint_pem(&std::fs::read_to_string(&made.cert).unwrap()).as_deref(), Some(fp.as_str()));
        assert_eq!(fingerprint_pem("not a certificate"), None);
        // a machine given its sha256 trusts it, and only it (inyon join, then every connection)
        let pem = std::fs::read(&made.cert).unwrap();
        let hex = fp.replace(':', "").to_lowercase();
        assert!(inyon_engine::http::pinned_cert(&pem, &fp).is_ok());
        assert!(inyon_engine::http::pinned_cert(&pem, &hex).is_ok());
        assert!(inyon_engine::http::pinned_cert(&pem, &"00".repeat(32)).unwrap_err().contains("wasn't added"));
        // and the engine connects to a hub serving it, through the pin join sets up for that hub
        let port = serve(&made);
        let url = format!("https://127.0.0.1:{port}/");
        let get = |pin: Option<[u8; 32]>| {
            let c = ureq::Agent::config_builder().http_status_as_error(false);
            let limits = inyon_engine::http::Limits::new(std::time::Duration::from_secs(10));
            let pin = pin.map(|want| inyon_engine::hub_tls::config(want, "127.0.0.1".into()));
            inyon_engine::http::timed_agent_pinned(c, limits, ureq::unversioned::resolver::DefaultResolver::default(), pin).get(&url).call().map(|r| r.status().as_u16())
        };
        let kept = dir.join("kept.crt");
        std::fs::write(&kept, inyon_engine::http::pinned_cert(&pem, &hex).unwrap()).unwrap();
        assert!(get(None).is_err());
        assert_eq!(get(inyon_engine::hub_tls::pinned_sha256(&kept)).ok(), Some(200));
        // a pin for another certificate doesn't let it through
        assert!(get(Some([0; 32])).is_err());
        let again = make(&dir, &names("studio", &[])).unwrap();
        assert_ne!(fingerprint(&again.cert).unwrap(), fp, "each certificate has its own key");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
