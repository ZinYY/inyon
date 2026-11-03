//! Outbound WebSockets (to a hub's /node, to a relay), with TLS from rustls on ring and the
//! Mozilla root certificates, the same set the Bun engine trusted.

use std::sync::Arc;

use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::{Connector, MaybeTlsStream, WebSocketStream};

pub type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Accepts any certificate, for NODE_TLS_REJECT_UNAUTHORIZED=0 and the saved hub (crate::http::insecure_tls_for).
#[derive(Debug)]
struct NoCheck(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for NoCheck {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.0.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

fn tls(insecure: bool) -> Arc<rustls::ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider.clone()).with_safe_default_protocol_versions().expect("ring supports TLS 1.2 and 1.3");
    let config = if insecure {
        builder.dangerous().with_custom_certificate_verifier(Arc::new(NoCheck(provider))).with_no_client_auth()
    } else {
        let mut roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
        roots.add_parsable_certificates(crate::http::extra_roots());
        builder.with_root_certificates(roots).with_no_client_auth()
    };
    Arc::new(config)
}

/// Opens a WebSocket to `url` with extra `headers`. The error is a WebSocket close code the way a
/// browser reports a socket that never opened (1006), plus why.
pub async fn connect(url: &str, headers: &[(&str, &str)]) -> Result<Socket, (u16, String)> {
    connect_to(url, headers, None).await
}

/// `connect`, and with `addrs` only to those addresses (a name already looked up and checked), the
/// name kept for the Host header and the certificate. A hub whose certificate is pinned is held to
/// it (crate::hub_tls); everything else to the usual roots.
pub async fn connect_to(url: &str, headers: &[(&str, &str)], addrs: Option<Vec<std::net::SocketAddr>>) -> Result<Socket, (u16, String)> {
    let mut req = url.into_client_request().map_err(|e| (1006, e.to_string()))?;
    for (k, v) in headers {
        let name = tokio_tungstenite::tungstenite::http::HeaderName::from_bytes(k.as_bytes()).map_err(|e| (1006, e.to_string()))?;
        let value = tokio_tungstenite::tungstenite::http::HeaderValue::from_str(v).map_err(|e| (1006, e.to_string()))?;
        req.headers_mut().insert(name, value);
    }
    let config = WebSocketConfig::default().max_message_size(Some(64 << 20)).max_frame_size(Some(64 << 20));
    let insecure = crate::http::insecure_tls_for(url);
    let pin = if insecure { None } else { crate::hub_tls::config_for(url) };
    let tls = Connector::Rustls(pin.unwrap_or_else(|| tls(insecure)));
    let connect = async {
        match addrs {
            // no Nagle: keys typed into a terminal go out at once instead of waiting for more to send
            None => tokio_tungstenite::connect_async_tls_with_config(req, Some(config), true, Some(tls)).await.map_err(|e| e.to_string()),
            Some(addrs) => {
                let stream = TcpStream::connect(&addrs[..]).await.map_err(|e| e.to_string())?;
                let _ = stream.set_nodelay(true);
                tokio_tungstenite::client_async_tls_with_config(req, stream, Some(config), Some(tls)).await.map_err(|e| e.to_string())
            }
        }
    };
    match tokio::time::timeout(std::time::Duration::from_secs(30), connect).await {
        Ok(Ok((ws, _))) => Ok(ws),
        Ok(Err(e)) => Err((1006, e)),
        Err(_) => Err((1006, "timed out".into())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "linux")]
    #[test]
    fn a_hub_on_a_certificate_from_node_extra_ca_certs_is_trusted_over_websockets_too() {
        use std::io::{Read, Write};
        let _env = crate::http::test_tls::env_lock();
        let dir = std::env::temp_dir().join(format!("inyon-extra-ca-ws-{}", std::process::id()));
        let (ca, port) = crate::http::test_tls::server(&dir);
        let get = || -> Result<String, String> {
            let tcp = std::net::TcpStream::connect(("127.0.0.1", port)).map_err(|e| e.to_string())?;
            let conn = rustls::ClientConnection::new(tls(false), "localhost".try_into().unwrap()).map_err(|e| e.to_string())?;
            let mut s = rustls::StreamOwned::new(conn, tcp);
            s.write_all(b"GET / HTTP/1.1\r\nHost: localhost\r\n\r\n").map_err(|e| e.to_string())?;
            let mut out = String::new();
            let _ = s.read_to_string(&mut out);
            out.ends_with("ok").then_some(out).ok_or_else(|| "no answer".to_string())
        };
        // SAFETY: the tests that touch NODE_EXTRA_CA_CERTS hold env_lock
        unsafe { std::env::remove_var("NODE_EXTRA_CA_CERTS") };
        let unknown = get();
        unsafe { std::env::set_var("NODE_EXTRA_CA_CERTS", &ca) };
        let trusted = get();
        unsafe { std::env::remove_var("NODE_EXTRA_CA_CERTS") };
        let _ = std::fs::remove_dir_all(&dir);
        assert!(unknown.is_err());
        assert!(trusted.is_ok(), "{trusted:?}");
    }

    #[tokio::test]
    async fn a_name_already_checked_is_not_looked_up_again_and_stays_the_host() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let host = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
            let seen = host.clone();
            #[allow(clippy::result_large_err)] // the callback's type is tungstenite's
            let cb = move |req: &tokio_tungstenite::tungstenite::handshake::server::Request, res| {
                *seen.lock().unwrap() = req.headers().get("host").and_then(|h| h.to_str().ok()).unwrap_or("").to_string();
                Ok(res)
            };
            let _ws = tokio_tungstenite::accept_hdr_async(tcp, cb).await.unwrap();
            let h = host.lock().unwrap().clone();
            h
        });
        // .invalid never resolves: the socket opens only because it goes to the address given
        let url = format!("ws://hub.invalid:{port}/node");
        assert!(connect(&url, &[]).await.is_err());
        let ws = connect_to(&url, &[("authorization", "Bearer t")], Some(vec![format!("127.0.0.1:{port}").parse().unwrap()])).await;
        assert!(ws.is_ok(), "{:?}", ws.err());
        assert_eq!(server.await.unwrap(), format!("hub.invalid:{port}"));
    }

    #[tokio::test]
    async fn keys_go_out_at_once_without_nagle() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let _ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        });
        let ws = connect(&format!("ws://127.0.0.1:{port}/node"), &[]).await.unwrap();
        match ws.get_ref() {
            MaybeTlsStream::Plain(tcp) => assert!(tcp.nodelay().unwrap()),
            _ => panic!("a plain socket was expected"),
        }
        server.await.unwrap();
    }
}
