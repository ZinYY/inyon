//! Noise_IK_25519_ChaChaPoly_SHA256 (noiseprotocol.org, revision 34), through snow. The phone
//! already knows the machine's static key from pairing, so one round trip both authenticates the
//! two sides and agrees on transport keys. Messages travel over an ordered transport (a WebSocket,
//! directly or through a relay), so nonces are implicit counters.
//!
//! ```text
//!   <- s
//!   ...
//!   -> e, es, s, ss
//!   <- e, ee, se
//! ```

use std::fmt;

use curve25519_dalek::MontgomeryPoint;
use snow::{Builder, HandshakeState, TransportState};

pub const PROTOCOL: &str = "Noise_IK_25519_ChaChaPoly_SHA256";
const DH: usize = 32;
const TAG: usize = 16;
/// Noise's cap on one message, ciphertext included.
pub const MAX_MESSAGE: usize = 65535;

#[derive(Clone, PartialEq, Eq)]
pub struct KeyPair {
    pub secret: [u8; 32],
    pub public: [u8; 32],
}

impl fmt::Debug for KeyPair {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("KeyPair").field("public", &self.public).finish_non_exhaustive()
    }
}

impl KeyPair {
    pub fn from_secret(secret: [u8; 32]) -> Self {
        KeyPair { secret, public: MontgomeryPoint::mul_base_clamped(secret).to_bytes() }
    }

    pub fn generate() -> Result<Self, getrandom::Error> {
        let mut secret = [0u8; 32];
        getrandom::fill(&mut secret)?;
        Ok(Self::from_secret(secret))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoiseError {
    /// a message too short, too long, or out of turn
    BadMessage,
    /// the other side doesn't hold the key it claims, or the message was changed on the way
    Decrypt,
    /// anything else snow refuses (a bad key, an exhausted nonce)
    Failed,
}

impl fmt::Display for NoiseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            NoiseError::BadMessage => "Noise: bad message",
            NoiseError::Decrypt => "Noise: decryption failed",
            NoiseError::Failed => "Noise: handshake failed",
        })
    }
}

impl std::error::Error for NoiseError {}

impl From<snow::Error> for NoiseError {
    fn from(e: snow::Error) -> Self {
        match e {
            snow::Error::Decrypt => NoiseError::Decrypt,
            snow::Error::Input | snow::Error::State(_) => NoiseError::BadMessage,
            _ => NoiseError::Failed,
        }
    }
}

fn builder<'a>(s: &'a KeyPair, prologue: &'a [u8], e: Option<&'a [u8; 32]>) -> Result<Builder<'a>, NoiseError> {
    let params = PROTOCOL.parse().map_err(|_| NoiseError::Failed)?;
    let b = Builder::new(params).local_private_key(&s.secret)?.prologue(prologue)?;
    Ok(match e {
        Some(e) => b.fixed_ephemeral_key_for_testing_only(e),
        None => b,
    })
}

/// One end of a finished handshake: its two directions and the handshake hash.
pub struct Transport {
    state: TransportState,
    hash: [u8; 32],
}

impl Transport {
    fn new(hs: HandshakeState) -> Result<Self, NoiseError> {
        let hash = hs.get_handshake_hash().try_into().map_err(|_| NoiseError::Failed)?;
        Ok(Transport { state: hs.into_transport_mode()?, hash })
    }

    /// The handshake hash, the same on both ends (channel binding).
    pub fn hash(&self) -> &[u8; 32] {
        &self.hash
    }

    pub fn encrypt(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, NoiseError> {
        if plaintext.len() + TAG > MAX_MESSAGE {
            return Err(NoiseError::BadMessage);
        }
        let mut out = vec![0u8; plaintext.len() + TAG];
        let n = self.state.write_message(plaintext, &mut out)?;
        out.truncate(n);
        Ok(out)
    }

    /// A failed decrypt leaves the counter where it was, as the spec requires.
    pub fn decrypt(&mut self, ciphertext: &[u8]) -> Result<Vec<u8>, NoiseError> {
        if ciphertext.len() < TAG || ciphertext.len() > MAX_MESSAGE {
            return Err(NoiseError::BadMessage);
        }
        let mut out = vec![0u8; ciphertext.len()];
        let n = self.state.read_message(ciphertext, &mut out)?;
        out.truncate(n);
        Ok(out)
    }
}

/// The phone's side: it knows the machine's static key before it connects.
pub struct Initiator {
    hs: HandshakeState,
}

impl Initiator {
    pub fn new(s: &KeyPair, rs: &[u8; 32], prologue: &[u8]) -> Result<Self, NoiseError> {
        Self::build(s, rs, prologue, None)
    }

    /// A fixed ephemeral key, for the test vectors only.
    #[doc(hidden)]
    pub fn with_ephemeral(s: &KeyPair, rs: &[u8; 32], prologue: &[u8], e: &[u8; 32]) -> Result<Self, NoiseError> {
        Self::build(s, rs, prologue, Some(e))
    }

    fn build(s: &KeyPair, rs: &[u8; 32], prologue: &[u8], e: Option<&[u8; 32]>) -> Result<Self, NoiseError> {
        Ok(Initiator { hs: builder(s, prologue, e)?.remote_public_key(rs)?.build_initiator()? })
    }

    /// -> e, es, s, ss, payload
    pub fn write_message1(&mut self, payload: &[u8]) -> Result<Vec<u8>, NoiseError> {
        let len = DH + DH + TAG + payload.len() + TAG;
        if len > MAX_MESSAGE {
            return Err(NoiseError::BadMessage);
        }
        let mut out = vec![0u8; len];
        let n = self.hs.write_message(payload, &mut out)?;
        out.truncate(n);
        Ok(out)
    }

    /// <- e, ee, se, payload
    pub fn read_message2(mut self, msg: &[u8]) -> Result<(Vec<u8>, Transport), NoiseError> {
        if msg.len() < DH + TAG || msg.len() > MAX_MESSAGE {
            return Err(NoiseError::BadMessage);
        }
        let mut payload = vec![0u8; msg.len()];
        let n = self.hs.read_message(msg, &mut payload)?;
        payload.truncate(n);
        Ok((payload, Transport::new(self.hs)?))
    }
}

/// The machine's side: it learns who is calling from message 1.
pub struct Responder {
    hs: HandshakeState,
    secret: [u8; 32],
}

/// Whether X25519 of our secret with `public` comes out all zero: a low-order point, which makes
/// that DH a public value. snow doesn't refuse it, so a caller could claim a static key whose
/// private half nobody needs (noble, on the TS side, throws on it).
fn low_order(secret: &[u8; 32], public: &[u8]) -> bool {
    let Ok(p) = <[u8; 32]>::try_from(public) else { return true };
    MontgomeryPoint(p).mul_clamped(*secret).to_bytes() == [0u8; 32]
}

impl Responder {
    pub fn new(s: &KeyPair, prologue: &[u8]) -> Result<Self, NoiseError> {
        Ok(Responder { hs: builder(s, prologue, None)?.build_responder()?, secret: s.secret })
    }

    /// A fixed ephemeral key, for the test vectors only.
    #[doc(hidden)]
    pub fn with_ephemeral(s: &KeyPair, prologue: &[u8], e: &[u8; 32]) -> Result<Self, NoiseError> {
        Ok(Responder { hs: builder(s, prologue, Some(e))?.build_responder()?, secret: s.secret })
    }

    /// -> e, es, s, ss, payload
    pub fn read_message1(&mut self, msg: &[u8]) -> Result<Vec<u8>, NoiseError> {
        if msg.len() < DH + DH + TAG + TAG || msg.len() > MAX_MESSAGE {
            return Err(NoiseError::BadMessage);
        }
        // the ephemeral key travels in the clear at the start
        if low_order(&self.secret, &msg[..DH]) {
            return Err(NoiseError::BadMessage);
        }
        let mut payload = vec![0u8; msg.len()];
        let n = self.hs.read_message(msg, &mut payload)?;
        payload.truncate(n);
        if self.hs.get_remote_static().is_none_or(|rs| low_order(&self.secret, rs)) {
            return Err(NoiseError::BadMessage);
        }
        Ok(payload)
    }

    /// The caller's static key, known after message 1.
    pub fn remote_static(&self) -> Option<[u8; 32]> {
        self.hs.get_remote_static()?.try_into().ok()
    }

    /// <- e, ee, se, payload
    pub fn write_message2(mut self, payload: &[u8]) -> Result<(Vec<u8>, Transport), NoiseError> {
        let len = DH + payload.len() + TAG;
        if len > MAX_MESSAGE {
            return Err(NoiseError::BadMessage);
        }
        let mut out = vec![0u8; len];
        let n = self.hs.write_message(payload, &mut out)?;
        out.truncate(n);
        Ok((out, Transport::new(self.hs)?))
    }
}
