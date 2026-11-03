//! How sessions travel through a relay. A machine keeps one WebSocket to the relay at /m/<id>;
//! each phone opens its own at /p/<id>. On the machine's socket, binary frames carry a 4-byte
//! session number before the bytes, and text frames are control messages. The relay never sees
//! anything but Noise ciphertext.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum RelayToMachine {
    /// `v: 2` asks for a signature bound to the relay's origin ([auth_message_v2]).
    Challenge {
        nonce: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        v: Option<u32>,
    },
    Ready,
    Open {
        sid: u32,
    },
    Close {
        sid: u32,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "lowercase")]
pub enum MachineToRelay {
    Auth { id: String, sig: String },
    Close { sid: u32 },
}

/// What the machine signs to prove to the relay it owns its id (a wire id from before the rename
/// to Inyon).
pub fn auth_message(nonce: &str) -> Vec<u8> {
    format!("tether-relay-auth:{nonce}").into_bytes()
}

/// The same proof bound to the relay it's for, so one relay can't pass another relay's challenge
/// on and log in there as the machine with the answer. `origin` is [relay_origin] of the address
/// the machine dialed, never one the relay names.
pub fn auth_message_v2(origin: &str, nonce: &str) -> Vec<u8> {
    format!("tether-relay-auth:v2:{origin}:{nonce}").into_bytes()
}

/// The shortest challenge the machine answers: a short or missing nonce could be one a relay
/// asked for before.
pub const MIN_NONCE: usize = 16;

/// "https://Relay.Example:443/base/" → "https://relay.example": the scheme as http(s), the host in
/// lower case, the port only when it isn't the scheme's own. None for an address without a host.
pub fn relay_origin(relay: &str) -> Option<String> {
    let (scheme, rest) = relay.split_once("://")?;
    let scheme = match scheme.to_ascii_lowercase().as_str() {
        "https" | "wss" => "https",
        "http" | "ws" => "http",
        _ => return None,
    };
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?.to_ascii_lowercase();
    if authority.is_empty() {
        return None;
    }
    let own = if scheme == "https" { ":443" } else { ":80" };
    Some(format!("{scheme}://{}", authority.strip_suffix(own).unwrap_or(&authority)))
}

pub fn frame(sid: u32, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + data.len());
    out.extend_from_slice(&sid.to_be_bytes());
    out.extend_from_slice(data);
    out
}

/// None if the frame is too short to carry a session number.
pub fn unframe(buf: &[u8]) -> Option<(u32, &[u8])> {
    let (sid, data) = buf.split_first_chunk::<4>()?;
    Some((u32::from_be_bytes(*sid), data))
}

pub fn machine_path(id: &str) -> String {
    format!("/m/{id}")
}

pub fn phone_path(id: &str) -> String {
    format!("/p/{id}")
}

/// "https://relay.example.com" → its WebSocket URL for a path.
pub fn relay_ws(relay: &str, path: &str) -> String {
    let ws = match relay.strip_prefix("http") {
        Some(rest) => format!("ws{rest}"),
        None => relay.to_string(),
    };
    format!("{}{path}", ws.trim_end_matches('/'))
}
