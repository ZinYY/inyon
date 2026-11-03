//! The encrypted JSON channel between the phone and a machine, over anything that carries whole
//! binary messages in order: a WebSocket straight to the machine, through a tunnel, or through a
//! relay. The relay and the tunnel see only ciphertext.
//!
//! This is the channel without its socket: the caller moves the frames, so the same code runs on
//! tokio, in a browser, or in a test.

use std::fmt;

use serde::Serialize;
use serde_json::Value;

use crate::noise::{Initiator, KeyPair, NoiseError, Responder, Transport};

/// A wire id from when Inyon was called Tether. Both ends must agree on it, so it stays: changing it
/// would cut every paired phone off from every machine until both had updated.
pub const PROLOGUE: &[u8] = b"tether-link 1";
/// How long either side waits for the other's handshake message.
pub const HANDSHAKE_MS: u64 = 15000;
/// Noise caps a message at 65535 bytes; larger JSON goes in pieces.
pub const CHUNK: usize = 60000;
/// The most a peer can make this end hold while a message is still arriving in pieces.
pub const MAX_MESSAGE: usize = 16 * 1024 * 1024;

/// The WebSocket close codes each end uses.
pub mod close {
    pub const BAD_MESSAGE: u16 = 4400;
    pub const BAD_HANDSHAKE: u16 = 4401;
    pub const REFUSED: u16 = 4403;
    pub const HANDSHAKE_TIMEOUT: u16 = 4408;
    pub const TOO_LARGE: u16 = 4413;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LinkError {
    NotPaired(String),
    BadKey(String),
    Timeout(String),
    Closed(String),
    Refused(String),
}

impl LinkError {
    /// The code a refusal carries in the welcome (`NOT_PAIRED`, ...).
    pub fn code(&self) -> &'static str {
        match self {
            LinkError::NotPaired(_) => "NOT_PAIRED",
            LinkError::BadKey(_) => "BAD_KEY",
            LinkError::Timeout(_) => "TIMEOUT",
            LinkError::Closed(_) => "CLOSED",
            LinkError::Refused(_) => "REFUSED",
        }
    }

    pub fn message(&self) -> &str {
        match self {
            LinkError::NotPaired(m) | LinkError::BadKey(m) | LinkError::Timeout(m) | LinkError::Closed(m) | LinkError::Refused(m) => m,
        }
    }
}

impl fmt::Display for LinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for LinkError {}

/// Why a channel ended; the caller closes the socket with `close_code()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelError {
    /// a forged or out-of-order message: the session can't be trusted any more
    BadMessage,
    /// a peer sent more than `MAX_MESSAGE` before finishing a message
    TooLarge,
}

impl ChannelError {
    pub fn close_code(self) -> u16 {
        match self {
            ChannelError::BadMessage => close::BAD_MESSAGE,
            ChannelError::TooLarge => close::TOO_LARGE,
        }
    }

    pub fn reason(self) -> &'static str {
        match self {
            ChannelError::BadMessage => "bad message",
            ChannelError::TooLarge => "message too large",
        }
    }
}

impl fmt::Display for ChannelError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.reason())
    }
}

impl std::error::Error for ChannelError {}

/// Each frame is one Noise transport message whose plaintext is a flag byte (1 on a message's
/// last piece, else 0) and up to `CHUNK` bytes of the message.
pub struct Channel {
    t: Transport,
    parts: Vec<u8>,
}

impl Channel {
    pub fn new(t: Transport) -> Self {
        Channel { t, parts: Vec::new() }
    }

    /// The handshake hash, the same on both ends.
    pub fn binding(&self) -> &[u8; 32] {
        self.t.hash()
    }

    /// The frames that carry one message (UTF-8 JSON), in the order they must be sent.
    pub fn seal(&mut self, body: &[u8]) -> Result<Vec<Vec<u8>>, NoiseError> {
        let mut frames = Vec::with_capacity(body.len() / CHUNK + 1);
        let mut at = 0;
        loop {
            let end = (at + CHUNK).min(body.len());
            let mut plain = Vec::with_capacity(end - at + 1);
            plain.push(u8::from(at + CHUNK >= body.len()));
            plain.extend_from_slice(&body[at..end]);
            frames.push(self.t.encrypt(&plain)?);
            at += CHUNK;
            if at >= body.len() {
                return Ok(frames);
            }
        }
    }

    /// Takes one frame; returns a whole message once its last piece is in.
    pub fn open(&mut self, frame: &[u8]) -> Result<Option<Vec<u8>>, ChannelError> {
        let plain = self.t.decrypt(frame).map_err(|_| ChannelError::BadMessage)?;
        let Some((&flag, piece)) = plain.split_first() else { return Ok(None) };
        self.parts.extend_from_slice(piece);
        if self.parts.len() > MAX_MESSAGE {
            self.parts = Vec::new();
            return Err(ChannelError::TooLarge);
        }
        if flag != 1 {
            return Ok(None);
        }
        Ok(Some(std::mem::take(&mut self.parts)))
    }
}

/// JavaScript's idea of true, which the TS side uses on `welcome.ok`.
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0 && !f.is_nan()),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

/// The phone's side, between sending message 1 and the machine's answer.
pub struct Connecting {
    hs: Initiator,
}

/// Starts a connection: returns the state to finish it with and message 1 to send. `machine_key`
/// is the machine's Noise key from pairing; `hello` (JSON) goes to the machine encrypted.
pub fn connect(noise: &KeyPair, machine_key: &[u8; 32], hello: &[u8]) -> Result<(Connecting, Vec<u8>), NoiseError> {
    let mut hs = Initiator::new(noise, machine_key, PROLOGUE)?;
    let msg = hs.write_message1(hello)?;
    Ok((Connecting { hs }, msg))
}

impl Connecting {
    #[doc(hidden)]
    pub fn with_ephemeral(noise: &KeyPair, machine_key: &[u8; 32], hello: &[u8], e: &[u8; 32]) -> Result<(Connecting, Vec<u8>), NoiseError> {
        let mut hs = Initiator::with_ephemeral(noise, machine_key, PROLOGUE, e)?;
        let msg = hs.write_message1(hello)?;
        Ok((Connecting { hs }, msg))
    }

    /// Reads the machine's answer: the channel and its welcome, or why it refused. On `BadKey`
    /// close with 4401, on a refusal with 4403.
    pub fn finish(self, reply: &[u8]) -> Result<(Channel, Value), LinkError> {
        let (payload, transport) = self.hs.read_message2(reply).map_err(|_| LinkError::BadKey("The machine didn't prove it's the one you paired with".into()))?;
        let welcome: Value = serde_json::from_slice(&payload).map_err(|_| LinkError::Refused("The machine refused the connection".into()))?;
        if !truthy(welcome.get("ok")) {
            let error = welcome.get("error").and_then(Value::as_str).unwrap_or("The machine refused the connection").to_string();
            return Err(match welcome.get("code").and_then(Value::as_str) {
                Some("NOT_PAIRED") => LinkError::NotPaired(error),
                _ => LinkError::Refused(error),
            });
        }
        Ok((Channel::new(transport), welcome))
    }
}

/// The machine's side, after reading message 1 and before answering.
pub struct Accepting {
    hs: Responder,
    remote_static: [u8; 32],
    hello: Vec<u8>,
}

/// Reads a phone's message 1. On an error close with 4401.
pub fn accept(noise: &KeyPair, msg: &[u8]) -> Result<Accepting, NoiseError> {
    Accepting::read(Responder::new(noise, PROLOGUE)?, msg)
}

#[derive(Serialize)]
struct Refusal<'a> {
    ok: bool,
    code: &'a str,
    error: &'a str,
}

impl Accepting {
    #[doc(hidden)]
    pub fn with_ephemeral(noise: &KeyPair, msg: &[u8], e: &[u8; 32]) -> Result<Accepting, NoiseError> {
        Accepting::read(Responder::with_ephemeral(noise, PROLOGUE, e)?, msg)
    }

    fn read(mut hs: Responder, msg: &[u8]) -> Result<Accepting, NoiseError> {
        let hello = hs.read_message1(msg)?;
        let remote_static = hs.remote_static().ok_or(NoiseError::Failed)?;
        Ok(Accepting { hs, remote_static, hello })
    }

    /// Who is calling: their Noise static key.
    pub fn remote_static(&self) -> &[u8; 32] {
        &self.remote_static
    }

    /// What they sent (JSON: a pairing secret, the phone's name).
    pub fn hello(&self) -> &[u8] {
        &self.hello
    }

    /// Lets them in. `welcome` is the JSON object to send, `"ok": true` included.
    pub fn welcome(self, welcome: &[u8]) -> Result<(Channel, Vec<u8>), NoiseError> {
        let (msg, transport) = self.hs.write_message2(welcome)?;
        Ok((Channel::new(transport), msg))
    }

    /// Turns them away, telling them why (encrypted); send it, then close with 4403.
    pub fn refuse(self, err: &LinkError) -> Result<Vec<u8>, NoiseError> {
        let body = serde_json::to_vec(&Refusal { ok: false, code: err.code(), error: err.message() }).map_err(|_| NoiseError::Failed)?;
        Ok(self.hs.write_message2(&body)?.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pair() -> (Channel, Channel) {
        let phone = KeyPair::generate().unwrap();
        let machine = KeyPair::generate().unwrap();
        let (connecting, m1) = connect(&phone, &machine.public, br#"{"name":"Pixel"}"#).unwrap();
        let accepting = accept(&machine, &m1).unwrap();
        assert_eq!(accepting.remote_static(), &phone.public);
        let (machine_ch, m2) = accepting.welcome(br#"{"ok":true,"hostname":"box"}"#).unwrap();
        let (phone_ch, welcome) = connecting.finish(&m2).unwrap();
        assert_eq!(welcome["hostname"], "box");
        (phone_ch, machine_ch)
    }

    #[test]
    fn an_empty_message_is_one_final_frame() {
        let (mut a, mut b) = pair();
        let frames = a.seal(b"").unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(b.open(&frames[0]).unwrap(), Some(Vec::new()));
    }

    #[test]
    fn a_forged_or_reordered_frame_ends_the_channel() {
        let (mut a, mut b) = pair();
        let mut frames = a.seal(&vec![b'x'; CHUNK * 2]).unwrap();
        let err = b.open(&frames[1]).unwrap_err();
        assert_eq!((err, err.close_code()), (ChannelError::BadMessage, 4400));
        frames[0][3] ^= 1;
        assert_eq!(b.open(&frames[0]), Err(ChannelError::BadMessage));
    }

    #[test]
    fn a_frame_without_a_flag_byte_carries_nothing() {
        let (mut a, mut b) = pair();
        let empty = a.t.encrypt(&[]).unwrap();
        assert_eq!(b.open(&empty), Ok(None));
        let frames = a.seal(b"{}").unwrap();
        assert_eq!(b.open(&frames[0]), Ok(Some(b"{}".to_vec())));
    }

    #[test]
    fn a_peer_cannot_make_this_end_hold_more_than_the_cap() {
        let (mut a, mut b) = pair();
        let piece = [0u8; CHUNK + 1];
        let mut held = 0;
        loop {
            // pieces never marked final
            let f = a.t.encrypt(&piece).unwrap();
            held += CHUNK;
            match b.open(&f) {
                Ok(None) => assert!(held <= MAX_MESSAGE),
                Err(e) => {
                    assert_eq!((e, e.close_code()), (ChannelError::TooLarge, 4413));
                    assert!(held > MAX_MESSAGE);
                    return;
                }
                Ok(Some(_)) => panic!("an unfinished message came out"),
            }
        }
    }

    #[test]
    fn the_wrong_machine_key_fails_on_both_ends() {
        let phone = KeyPair::generate().unwrap();
        let machine = KeyPair::generate().unwrap();
        let someone = KeyPair::generate().unwrap();
        let (_, m1) = connect(&phone, &someone.public, b"{}").unwrap();
        assert!(accept(&machine, &m1).is_err());
        // an answer that doesn't come from the machine it was meant for
        let (connecting, m1) = connect(&phone, &machine.public, b"{}").unwrap();
        let (_, mut m2) = accept(&machine, &m1).unwrap().welcome(br#"{"ok":true}"#).unwrap();
        m2[40] ^= 1;
        assert!(matches!(connecting.finish(&m2), Err(LinkError::BadKey(_))));
    }

    #[test]
    fn a_welcome_without_ok_is_a_refusal() {
        let phone = KeyPair::generate().unwrap();
        let machine = KeyPair::generate().unwrap();
        let (connecting, m1) = connect(&phone, &machine.public, b"{}").unwrap();
        let (_, m2) = accept(&machine, &m1).unwrap().welcome(br#"{"error":"busy"}"#).unwrap();
        assert_eq!(connecting.finish(&m2).err(), Some(LinkError::Refused("busy".into())));
    }
}
