//! The phone link from packages/link/src/channel.ts, sans-io: the Noise IK handshake and the
//! chunked, encrypted JSON channel. The transport (a WebSocket here) feeds it whole messages.

use snow::{Builder, HandshakeState, TransportState};

/// A wire id from when Inyon was called Tether; both ends must agree on it (channel.ts).
pub const PROLOGUE: &[u8] = b"tether-link 1";
const PATTERN: &str = "Noise_IK_25519_ChaChaPoly_SHA256";
/// Noise caps a message at 65535 bytes; larger JSON goes in pieces
const CHUNK: usize = 60000;
const MAX_MESSAGE: usize = 16 * 1024 * 1024;
const NOISE_MAX: usize = 65535;

#[derive(Debug, PartialEq, Eq)]
pub enum LinkError {
    /// message 2 didn't decrypt: the machine isn't the one we paired with
    BadKey,
    /// the machine answered, but with ok: false
    Refused(String),
    BadMessage,
    TooLarge,
    Noise(String),
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LinkError::BadKey => write!(f, "The machine didn't prove it's the one you paired with"),
            LinkError::Refused(m) => write!(f, "{m}"),
            LinkError::BadMessage => write!(f, "bad message"),
            LinkError::TooLarge => write!(f, "message too large"),
            LinkError::Noise(e) => write!(f, "noise: {e}"),
        }
    }
}

fn noise(e: snow::Error) -> LinkError {
    LinkError::Noise(e.to_string())
}

/// The phone's side of the handshake (`connect` in channel.ts).
pub struct Initiator {
    hs: HandshakeState,
}

impl Initiator {
    pub fn new(noise_secret: &[u8; 32], machine_key: &[u8; 32]) -> Result<Self, LinkError> {
        let hs = Builder::new(PATTERN.parse().map_err(noise)?)
            .local_private_key(noise_secret)
            .map_err(noise)?
            .remote_public_key(machine_key)
            .map_err(noise)?
            .prologue(PROLOGUE)
            .map_err(noise)?
            .build_initiator()
            .map_err(noise)?;
        Ok(Initiator { hs })
    }

    /// -> e, es, s, ss, hello
    pub fn message1(&mut self, hello: &str) -> Result<Vec<u8>, LinkError> {
        let mut buf = vec![0; NOISE_MAX];
        let n = self.hs.write_message(hello.as_bytes(), &mut buf).map_err(noise)?;
        buf.truncate(n);
        Ok(buf)
    }

    /// <- e, ee, se, welcome. Returns the welcome JSON and the channel.
    pub fn message2(mut self, msg: &[u8]) -> Result<(String, Channel), LinkError> {
        let mut buf = vec![0; NOISE_MAX];
        let n = self.hs.read_message(msg, &mut buf).map_err(|_| LinkError::BadKey)?;
        let welcome = String::from_utf8(buf[..n].to_vec()).map_err(|_| LinkError::BadMessage)?;
        let ok = serde_json::from_str::<serde_json::Value>(&welcome).ok().and_then(|v| v.get("ok").and_then(|o| o.as_bool())).unwrap_or(false);
        if !ok {
            let error = serde_json::from_str::<serde_json::Value>(&welcome)
                .ok()
                .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
                .unwrap_or_else(|| "The machine refused the connection".into());
            return Err(LinkError::Refused(error));
        }
        let hash = self.hs.get_handshake_hash().to_vec();
        let t = self.hs.into_transport_mode().map_err(noise)?;
        Ok((welcome, Channel::new(t, hash)))
    }
}

/// The machine's side, for the native tests (the browser test uses the TypeScript one).
pub struct Responder {
    hs: HandshakeState,
}

impl Responder {
    pub fn new(noise_secret: &[u8; 32]) -> Result<Self, LinkError> {
        let hs =
            Builder::new(PATTERN.parse().map_err(noise)?).local_private_key(noise_secret).map_err(noise)?.prologue(PROLOGUE).map_err(noise)?.build_responder().map_err(noise)?;
        Ok(Responder { hs })
    }

    /// Returns the hello and the caller's static key.
    pub fn message1(&mut self, msg: &[u8]) -> Result<(String, Vec<u8>), LinkError> {
        let mut buf = vec![0; NOISE_MAX];
        let n = self.hs.read_message(msg, &mut buf).map_err(|_| LinkError::BadKey)?;
        let rs = self.hs.get_remote_static().unwrap_or_default().to_vec();
        Ok((String::from_utf8_lossy(&buf[..n]).into_owned(), rs))
    }

    pub fn message2(mut self, welcome: &str) -> Result<(Vec<u8>, Channel), LinkError> {
        let mut buf = vec![0; NOISE_MAX];
        let n = self.hs.write_message(welcome.as_bytes(), &mut buf).map_err(noise)?;
        buf.truncate(n);
        let hash = self.hs.get_handshake_hash().to_vec();
        Ok((buf, Channel::new(self.hs.into_transport_mode().map_err(noise)?, hash)))
    }
}

/// The encrypted JSON channel: each Noise message carries a flag byte (1 = last piece) and up to
/// 60000 bytes of the JSON text.
pub struct Channel {
    t: TransportState,
    hash: Vec<u8>,
    parts: Vec<u8>,
}

impl Channel {
    fn new(t: TransportState, hash: Vec<u8>) -> Self {
        Channel { t, hash, parts: Vec::new() }
    }

    /// The handshake hash, the same on both ends (channel binding).
    pub fn binding(&self) -> &[u8] {
        &self.hash
    }

    /// One JSON text as the messages to send.
    pub fn seal(&mut self, json: &str) -> Result<Vec<Vec<u8>>, LinkError> {
        let body = json.as_bytes();
        let mut out = Vec::with_capacity(body.len() / CHUNK + 1);
        let mut at = 0;
        loop {
            let piece = &body[at..body.len().min(at + CHUNK)];
            let last = at + CHUNK >= body.len();
            let mut plain = Vec::with_capacity(piece.len() + 1);
            plain.push(last as u8);
            plain.extend_from_slice(piece);
            let mut msg = vec![0; plain.len() + 16];
            let n = self.t.write_message(&plain, &mut msg).map_err(noise)?;
            msg.truncate(n);
            out.push(msg);
            at += CHUNK;
            if last {
                return Ok(out);
            }
        }
    }

    /// One received message; returns the JSON text once its last piece is in.
    pub fn open(&mut self, msg: &[u8]) -> Result<Option<String>, LinkError> {
        let mut plain = vec![0; msg.len()];
        // a forged or out-of-order message: the caller closes the session (4400)
        let n = self.t.read_message(msg, &mut plain).map_err(|_| LinkError::BadMessage)?;
        let Some((&flag, piece)) = plain[..n].split_first() else {
            return Err(LinkError::BadMessage);
        };
        self.parts.extend_from_slice(piece);
        if self.parts.len() > MAX_MESSAGE {
            self.parts.clear();
            return Err(LinkError::TooLarge);
        }
        if flag != 1 {
            return Ok(None);
        }
        let whole = std::mem::take(&mut self.parts);
        String::from_utf8(whole).map(Some).map_err(|_| LinkError::BadMessage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Identity;

    fn pair() -> (Channel, Channel) {
        let phone = Identity::from_seed(&[1; 32]);
        let machine = Identity::from_seed(&[2; 32]);
        let mut i = Initiator::new(&phone.noise_secret, &machine.noise_public).unwrap();
        let mut r = Responder::new(&machine.noise_secret).unwrap();
        let (hello, rs) = r.message1(&i.message1(r#"{"phone":"x"}"#).unwrap()).unwrap();
        assert_eq!(hello, r#"{"phone":"x"}"#);
        assert_eq!(rs, phone.noise_public);
        let (m2, rc) = r.message2(r#"{"ok":true}"#).unwrap();
        let (welcome, ic) = i.message2(&m2).unwrap();
        assert_eq!(welcome, r#"{"ok":true}"#);
        assert_eq!(ic.binding(), rc.binding());
        (ic, rc)
    }

    #[test]
    fn chunks_large_messages() {
        let (mut a, mut b) = pair();
        let big = format!("{{\"data\":\"{}\"}}", "x".repeat(200_000));
        let msgs = a.seal(&big).unwrap();
        assert_eq!(msgs.len(), 4);
        let mut got = None;
        for m in &msgs {
            got = b.open(m).unwrap();
        }
        assert_eq!(got.as_deref(), Some(big.as_str()));
        // an empty text is one message
        assert_eq!(a.seal("").unwrap().len(), 1);
    }

    #[test]
    fn a_wrong_machine_key_fails() {
        let phone = Identity::from_seed(&[1; 32]);
        let machine = Identity::from_seed(&[2; 32]);
        let other = Identity::from_seed(&[3; 32]);
        let mut i = Initiator::new(&phone.noise_secret, &other.noise_public).unwrap();
        let mut r = Responder::new(&machine.noise_secret).unwrap();
        assert_eq!(r.message1(&i.message1("{}").unwrap()).unwrap_err(), LinkError::BadKey);
    }

    #[test]
    fn a_refusal_is_an_error() {
        let phone = Identity::from_seed(&[1; 32]);
        let machine = Identity::from_seed(&[2; 32]);
        let mut i = Initiator::new(&phone.noise_secret, &machine.noise_public).unwrap();
        let mut r = Responder::new(&machine.noise_secret).unwrap();
        r.message1(&i.message1("{}").unwrap()).unwrap();
        let (m2, _) = r.message2(r#"{"ok":false,"code":"NOT_PAIRED","error":"not paired"}"#).unwrap();
        assert_eq!(i.message2(&m2).err(), Some(LinkError::Refused("not paired".into())));
    }

    #[test]
    fn a_tampered_message_is_rejected() {
        let (mut a, mut b) = pair();
        let mut m = a.seal("{}").unwrap().remove(0);
        m[3] ^= 1;
        assert_eq!(b.open(&m), Err(LinkError::BadMessage));
    }
}
