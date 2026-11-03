//! Web Push without a push server of ours: a browser that allows notifications subscribes with its
//! own push service and gets an endpoint plus two keys. Anyone holding those can send it a
//! message, encrypted to the browser (RFC 8291), so the push service only ever carries ciphertext.
//! A VAPID key (RFC 8292) signs each request; the browser makes it and hands it out with the
//! subscription. Pure Rust (RustCrypto's P-256, AES-GCM and HKDF), so it builds for every system
//! the command ships for. (The hub's push/webpush.ts)

use std::time::Duration;

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::Aes128Gcm;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hkdf::Hkdf;
use p256::ecdsa::signature::Signer;
use p256::elliptic_curve::sec1::ToSec1Point;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::Sha256;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keys {
    pub p256dh: String,
    pub auth: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscription {
    pub endpoint: String,
    pub keys: Keys,
}

/// A P-256 private key as JWK, as the browser exports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EcJwk {
    pub kty: String,
    pub crv: String,
    pub d: String,
    pub x: String,
    pub y: String,
}

/// The browser's VAPID key pair.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Vapid {
    #[serde(rename = "privateJwk")]
    pub private_jwk: EcJwk,
    /// who to contact about these pushes: a mailto: or https: address (Apple requires one)
    pub subject: String,
}

pub fn b64u(b: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(b)
}

/// base64url as Node's Buffer reads it: padding, stray characters and a dangling last character
/// are let through.
pub fn unb64u(s: &str) -> Vec<u8> {
    use base64::engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig};
    const LENIENT: GeneralPurpose = GeneralPurpose::new(
        &base64::alphabet::URL_SAFE,
        GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent).with_decode_allow_trailing_bits(true),
    );
    let mut clean: String = s.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    if clean.len() % 4 == 1 {
        clean.pop();
    }
    LENIENT.decode(clean.as_bytes()).unwrap_or_default()
}

/// The 65-byte public key (0x04 ‖ x ‖ y) of a P-256 JWK, as the browser's applicationServerKey.
pub fn vapid_public_key(jwk: &EcJwk) -> String {
    let mut k = vec![4u8];
    k.extend(unb64u(&jwk.x));
    k.extend(unb64u(&jwk.y));
    b64u(&k)
}

/// A signed VAPID token for one push service (the endpoint's origin), good for 12 hours.
pub fn vapid_auth(endpoint: &str, v: &Vapid, now_ms: u64) -> Result<String, String> {
    let origin = url::Url::parse(endpoint).map_err(|e| e.to_string())?.origin().ascii_serialization();
    let header = b64u(br#"{"typ":"JWT","alg":"ES256"}"#);
    let claims = b64u(json!({ "aud": origin, "exp": now_ms / 1000 + 12 * 3600, "sub": v.subject }).to_string().as_bytes());
    let d = unb64u(&v.private_jwk.d);
    let key = p256::ecdsa::SigningKey::from_slice(&d).map_err(|_| "Bad VAPID key".to_string())?;
    let sig: p256::ecdsa::Signature = key.sign(format!("{header}.{claims}").as_bytes());
    Ok(format!("vapid t={header}.{claims}.{}, k={}", b64u(&sig.to_bytes()), vapid_public_key(&v.private_jwk)))
}

fn hkdf(ikm: &[u8], salt: &[u8], info: &[u8], len: usize) -> Vec<u8> {
    let mut out = vec![0u8; len];
    Hkdf::<Sha256>::new(Some(salt), ikm).expand(info, &mut out).expect("HKDF lengths here are small");
    out
}

/// Encrypts a message to the browser's keys (RFC 8291, aes128gcm, one record). `fixed` (salt,
/// sender key) is only for the RFC's test vector; every real message gets a new key and salt.
pub fn encrypt(sub: &Subscription, plaintext: &[u8], fixed: Option<(&[u8], &[u8])>) -> Result<Vec<u8>, String> {
    let ua_public = unb64u(&sub.keys.p256dh);
    let auth = unb64u(&sub.keys.auth);
    if ua_public.len() != 65 || auth.len() < 16 {
        return Err("Bad push subscription keys".into());
    }
    let ua = p256::PublicKey::from_sec1_bytes(&ua_public).map_err(|_| "Bad push subscription keys".to_string())?;
    let sender = match fixed {
        Some((_, key)) => p256::SecretKey::from_slice(key).map_err(|_| "Bad sender key".to_string())?,
        None => loop {
            if let Ok(k) = p256::SecretKey::from_slice(&crate::random_bytes(32)) {
                break k;
            }
        },
    };
    let salt = fixed.map(|(s, _)| s.to_vec()).unwrap_or_else(|| crate::random_bytes(16));
    let as_public = sender.public_key().to_sec1_point(false).as_bytes().to_vec();
    let shared = p256::ecdh::diffie_hellman(sender.to_nonzero_scalar(), ua.as_affine());
    let mut info = b"WebPush: info\0".to_vec();
    info.extend(&ua_public);
    info.extend(&as_public);
    let ikm = hkdf(shared.raw_secret_bytes(), &auth, &info, 32);
    let cek = hkdf(&ikm, &salt, b"Content-Encoding: aes128gcm\0", 16);
    let nonce = hkdf(&ikm, &salt, b"Content-Encoding: nonce\0", 12);
    let cipher = Aes128Gcm::new_from_slice(&cek).map_err(|e| e.to_string())?;
    // 0x02 marks the last (and only) record
    let mut padded = plaintext.to_vec();
    padded.push(2);
    let nonce: [u8; 12] = nonce.try_into().map_err(|_| "bad nonce".to_string())?;
    let body = cipher.encrypt(&nonce.into(), padded.as_slice()).map_err(|_| "Couldn't encrypt the notification".to_string())?;
    let mut out = salt;
    out.extend(4096u32.to_be_bytes());
    out.push(as_public.len() as u8);
    out.extend(&as_public);
    out.extend(body);
    Ok(out)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sent {
    pub ok: bool,
    pub status: u16,
    /// the subscription is gone (unsubscribed, expired): stop sending to it
    pub gone: bool,
}

impl Sent {
    /// What the push service's answer means for this subscription.
    pub fn from_status(status: u16) -> Sent {
        Sent { ok: (200..300).contains(&status), status, gone: status == 404 || status == 410 }
    }
}

/// What a send needs, apart from the network: the request it would make.
pub struct PushRequest {
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// The request for one message. `Urgency: high` lets it through a phone's battery saver.
pub fn push_request(sub: &Subscription, payload: &serde_json::Value, v: &Vapid, ttl: u64) -> Result<PushRequest, String> {
    let url = url::Url::parse(&sub.endpoint).map_err(|e| e.to_string())?;
    if url.scheme() != "https" {
        return Err("Push endpoints are https".into());
    }
    let headers = vec![
        ("Authorization".to_string(), vapid_auth(&sub.endpoint, v, crate::now_ms())?),
        ("Content-Encoding".to_string(), "aes128gcm".to_string()),
        ("Content-Type".to_string(), "application/octet-stream".to_string()),
        ("TTL".to_string(), ttl.to_string()),
        ("Urgency".to_string(), "high".to_string()),
    ];
    Ok(PushRequest { url: sub.endpoint.clone(), headers, body: encrypt(sub, payload.to_string().as_bytes(), None)? })
}

/// Sends one message.
pub async fn send_push(sub: &Subscription, payload: &serde_json::Value, v: &Vapid) -> Result<Sent, String> {
    send_push_with(sub, payload, v, |r| async move { Ok(crate::http::fetch("POST", r.url, r.headers, Some(r.body), Duration::from_secs(60)).await?.status) }).await
}

/// `send_push` with the request made by `fetch` (the TS's `o.fetch`), which answers its status.
pub async fn send_push_with<F, Fut>(sub: &Subscription, payload: &serde_json::Value, v: &Vapid, fetch: F) -> Result<Sent, String>
where
    F: FnOnce(PushRequest) -> Fut,
    Fut: std::future::Future<Output = Result<u16, String>>,
{
    let r = push_request(sub, payload, v, 4 * 3600)?;
    Ok(Sent::from_status(fetch(r).await?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use p256::ecdsa::signature::Verifier;

    // RFC 8291, Appendix A
    const PLAINTEXT: &str = "When I grow up, I want to be a watermelon";
    const SENDER_KEY: &str = "yfWPiYE-n46HLnH0KqZOF1fJJU3MYrct3AELtAQ-oRw";
    const UA_PUBLIC: &str = "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
    const AUTH: &str = "BTBZMqHH6r4Tts7J_aSIgg";
    const SALT: &str = "DGv6ra1nlYgDCS1FRnbzlw";
    const BODY: &str = "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A_yl95bQpu6cVPTpK4Mqgkf1CXztLVBSt2Ks3oZwbuwXPXLWyouBWLVWGNWQexSgSxsj_Qulcy4a-fN";

    fn sub() -> Subscription {
        Subscription { endpoint: "https://push.example.net/x".into(), keys: Keys { p256dh: UA_PUBLIC.into(), auth: AUTH.into() } }
    }

    #[test]
    fn encrypts_the_rfc_8291_example_byte_for_byte() {
        let out = encrypt(&sub(), PLAINTEXT.as_bytes(), Some((&unb64u(SALT), &unb64u(SENDER_KEY)))).unwrap();
        assert_eq!(b64u(&out), BODY);
    }

    #[test]
    fn every_message_gets_a_fresh_key_and_salt() {
        let a = encrypt(&sub(), &[1], None).unwrap();
        let b = encrypt(&sub(), &[1], None).unwrap();
        assert_ne!(a[..16], b[..16]);
        assert_ne!(a[21..86], b[21..86]);
    }

    fn jwk() -> (EcJwk, p256::ecdsa::VerifyingKey) {
        let secret = p256::SecretKey::from_slice(&unb64u(SENDER_KEY)).unwrap();
        let point = secret.public_key().to_sec1_point(false);
        let jwk = EcJwk { kty: "EC".into(), crv: "P-256".into(), d: SENDER_KEY.into(), x: b64u(point.x().unwrap()), y: b64u(point.y().unwrap()) };
        (jwk, p256::ecdsa::VerifyingKey::from(secret.public_key()))
    }

    #[test]
    fn a_vapid_token_for_the_push_service_signed_with_the_key_it_names() {
        let (jwk, verifying) = jwk();
        let auth = vapid_auth("https://fcm.googleapis.com/fcm/send/abc", &Vapid { private_jwk: jwk.clone(), subject: "https://example.com/".into() }, 1_700_000_000_000).unwrap();
        let rest = auth.strip_prefix("vapid t=").unwrap();
        let (t, k) = rest.split_once(", k=").unwrap();
        assert_eq!(k, vapid_public_key(&jwk));
        let parts: Vec<&str> = t.split('.').collect();
        assert_eq!(String::from_utf8(unb64u(parts[0])).unwrap(), r#"{"typ":"JWT","alg":"ES256"}"#);
        let claims: serde_json::Value = serde_json::from_slice(&unb64u(parts[1])).unwrap();
        assert_eq!(claims, json!({ "aud": "https://fcm.googleapis.com", "exp": 1_700_000_000u64 + 12 * 3600, "sub": "https://example.com/" }));
        let sig = p256::ecdsa::Signature::from_slice(&unb64u(parts[2])).unwrap();
        assert!(verifying.verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &sig).is_ok());
    }

    #[test]
    fn a_request_carries_the_headers_push_services_want() {
        let (key, _) = jwk();
        let r = push_request(&sub(), &json!({ "title": "x" }), &Vapid { private_jwk: key, subject: "mailto:me@example.com".into() }, 4 * 3600).unwrap();
        let h = |k: &str| r.headers.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).unwrap();
        assert_eq!(h("Content-Encoding"), "aes128gcm");
        assert_eq!(h("Urgency"), "high");
        assert_eq!(h("TTL"), "14400");
        assert!(h("Authorization").starts_with("vapid t="));
        let insecure = Subscription { endpoint: "http://push.example.net/x".into(), ..sub() };
        assert!(push_request(&insecure, &json!({}), &Vapid { private_jwk: jwk().0, subject: "mailto:a@b".into() }, 1).is_err());
    }

    #[test]
    fn a_gone_subscription_says_so() {
        let cell = std::cell::RefCell::new(vec![]);
        let sent = &cell;
        let v = Vapid { private_jwk: jwk().0, subject: "mailto:me@example.com".into() };
        let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
        let r = rt.block_on(send_push_with(&sub(), &json!({ "title": "x" }), &v, |r| async move {
            *sent.borrow_mut() = r.headers;
            Ok(410)
        }));
        assert_eq!(r.unwrap(), Sent { ok: false, status: 410, gone: true });
        let h = |k: &str| sent.borrow().iter().find(|(n, _)| n.eq_ignore_ascii_case(k)).map(|(_, v)| v.clone());
        assert_eq!(h("content-encoding").as_deref(), Some("aes128gcm"));
        assert_eq!(h("urgency").as_deref(), Some("high"));
        assert_eq!(Sent::from_status(410), Sent { ok: false, status: 410, gone: true });
        assert_eq!(Sent::from_status(404), Sent { ok: false, status: 404, gone: true });
        assert_eq!(Sent::from_status(201), Sent { ok: true, status: 201, gone: false });
        assert_eq!(Sent::from_status(500), Sent { ok: false, status: 500, gone: false });
    }
}
