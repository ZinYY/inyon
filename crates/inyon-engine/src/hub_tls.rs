//! The certificate of a hub that made it itself (setup's LAN choice), pinned. `inyon join` keeps the
//! copy whose sha256 the hub's "Add a machine" command named (~/.inyon/node/hub.crt), and from then
//! on a connection to that hub, and to nothing else, accepts exactly that certificate. It's never a
//! root: nothing it could sign is trusted, for the hub or any other name. A certificate the hub gets
//! later from an authority (Let's Encrypt, a company's) still counts the usual way.

use std::sync::Arc;

use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::crypto::CryptoProvider;
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use sha2::{Digest, Sha256};
use url::Url;

/// The TLS settings for a connection to `url`: the pin when `url` is the saved hub and its
/// certificate was kept, else None (the usual roots).
pub fn config_for(url: &str) -> Option<Arc<rustls::ClientConfig>> {
    let want = pinned_sha256(&crate::http::hub_cert_file())?;
    Some(config(want, saved_hub_host(url)?))
}

/// Whether `url` goes to the saved hub: the same host and port.
pub fn is_saved_hub(url: &str) -> bool {
    saved_hub_host(url).is_some()
}

/// The saved hub's host, as a certificate's name is compared, when `url` goes to it.
fn saved_hub_host(url: &str) -> Option<String> {
    let hub = crate::paths::saved_hub()?;
    let target = Url::parse(url).ok()?;
    let saved = Url::parse(&hub).ok()?;
    let host = host_key(&saved)?;
    (host_key(&target)? == host && target.port_or_known_default() == saved.port_or_known_default()).then_some(host)
}

/// `host` lowercased, IPv6 without brackets, as a certificate's name is compared.
fn host_key(url: &Url) -> Option<String> {
    Some(match url.host()? {
        url::Host::Domain(d) => d.trim_end_matches('.').to_ascii_lowercase(),
        url::Host::Ipv4(ip) => ip.to_string(),
        url::Host::Ipv6(ip) => ip.to_string(),
    })
}

/// The sha256 of the first certificate in `file`.
pub fn pinned_sha256(file: &std::path::Path) -> Option<[u8; 32]> {
    use rustls::pki_types::pem::PemObject;
    let cert = CertificateDer::pem_file_iter(file).ok()?.next()?.ok()?;
    Some(Sha256::digest(&cert).into())
}

/// A client config that takes the certificate with sha256 `want` from `host`, and checks anything
/// else against the usual roots.
pub fn config(want: [u8; 32], host: String) -> Arc<rustls::ClientConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut roots = rustls::RootCertStore { roots: webpki_roots::TLS_SERVER_ROOTS.to_vec() };
    roots.add_parsable_certificates(crate::http::extra_roots());
    let fallback = rustls::client::WebPkiServerVerifier::builder_with_provider(Arc::new(roots), provider.clone()).build().expect("Mozilla's roots are valid");
    let pin = Pin { want, host, fallback, provider: provider.clone() };
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("ring supports TLS 1.2 and 1.3")
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(pin))
        .with_no_client_auth();
    Arc::new(config)
}

#[derive(Debug)]
struct Pin {
    want: [u8; 32],
    host: String,
    fallback: Arc<rustls::client::WebPkiServerVerifier>,
    provider: Arc<CryptoProvider>,
}

fn name_of(name: &ServerName<'_>) -> String {
    match name {
        ServerName::DnsName(d) => d.as_ref().trim_end_matches('.').to_ascii_lowercase(),
        ServerName::IpAddress(ip) => std::net::IpAddr::from(*ip).to_string(),
        _ => String::new(),
    }
}

impl ServerCertVerifier for Pin {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        if name_of(server_name) == self.host && <[u8; 32]>::from(Sha256::digest(end_entity.as_ref())) == self.want {
            return Ok(ServerCertVerified::assertion());
        }
        self.fallback.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
    }

    fn verify_tls12_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &rustls::DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn verify_tls13_signature(&self, message: &[u8], cert: &CertificateDer<'_>, dss: &rustls::DigitallySignedStruct) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(message, cert, dss, &self.provider.signature_verification_algorithms)
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider.signature_verification_algorithms.supported_schemes()
    }
}

/// ureq's last step for a request to a pinned hub: TLS with `config` over the TCP connection,
/// which ureq's own rustls step can't be given.
pub mod ureq_tls {
    use std::io::{Read, Write};
    use std::sync::Arc;

    use rustls::{ClientConnection, StreamOwned};
    use ureq::unversioned::transport::{Buffers, ConnectionDetails, Connector, Either, LazyBuffers, NextTimeout, Transport, TransportAdapter};

    #[derive(Debug)]
    pub struct Tls(pub Arc<rustls::ClientConfig>);

    impl<In: Transport> Connector<In> for Tls {
        type Out = Either<In, TlsTransport>;

        fn connect(&self, details: &ConnectionDetails, chained: Option<In>) -> Result<Option<Self::Out>, ureq::Error> {
            let Some(transport) = chained else { return Ok(None) };
            if !details.needs_tls() || transport.is_tls() {
                return Ok(Some(Either::A(transport)));
            }
            let host = details.uri.host().unwrap_or("").trim_start_matches('[').trim_end_matches(']').to_string();
            let name = rustls::pki_types::ServerName::try_from(host).map_err(|_| ureq::Error::Tls("not a name a certificate can have"))?;
            let mut conn = ClientConnection::new(self.0.clone(), name).map_err(|e| ureq::Error::Io(std::io::Error::other(e)))?;
            let mut sock = TransportAdapter::new(transport.boxed());
            sock.set_timeout(details.timeout);
            conn.complete_io(&mut sock).map_err(ureq::Error::Io)?;
            let buffers = LazyBuffers::new(details.config.input_buffer_size(), details.config.output_buffer_size());
            Ok(Some(Either::B(TlsTransport { buffers, stream: StreamOwned { conn, sock } })))
        }
    }

    pub struct TlsTransport {
        buffers: LazyBuffers,
        stream: StreamOwned<ClientConnection, TransportAdapter>,
    }

    impl std::fmt::Debug for TlsTransport {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("TlsTransport")
        }
    }

    impl Transport for TlsTransport {
        fn buffers(&mut self) -> &mut dyn Buffers {
            &mut self.buffers
        }

        fn transmit_output(&mut self, amount: usize, timeout: NextTimeout) -> Result<(), ureq::Error> {
            self.stream.get_mut().set_timeout(timeout);
            let output = &self.buffers.output()[..amount];
            self.stream.write_all(output).map_err(ureq::Error::Io)
        }

        fn await_input(&mut self, timeout: NextTimeout) -> Result<bool, ureq::Error> {
            self.stream.get_mut().set_timeout(timeout);
            let input = self.buffers.input_append_buf();
            let amount = self.stream.read(input).map_err(ureq::Error::Io)?;
            self.buffers.input_appended(amount);
            Ok(amount > 0)
        }

        fn is_open(&mut self) -> bool {
            self.stream.get_mut().get_mut().is_open()
        }

        fn is_tls(&self) -> bool {
            true
        }
    }
}
