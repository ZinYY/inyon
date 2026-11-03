//! Where a machine can be reached right now, published under its own key with pkarr (signed DNS
//! records on the Mainline DHT, reached through public HTTP relays). The phone looks it up by the
//! machine id it paired with and checks the signature, so neither the relays nor the DHT can point
//! it somewhere else.
//!
//! This is the record format only; the HTTP PUT and GET belong to whoever has a client.

use std::fmt;

use serde::{Deserialize, Serialize};

use crate::identity::{verify, Identity};
use crate::js;

pub const PKARR_RELAYS: [&str; 2] = ["https://relay.pkarr.org", "https://dns.iroh.link/pkarr"];

/// What a machine publishes: its current addresses, best first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// wss:// or ws:// URLs that reach the machine's link listener (tunnel, LAN)
    pub urls: Vec<String>,
    /// relays it keeps a connection to
    pub relays: Vec<String>,
    /// a short pairing ID's record names the machine it stands for
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// A record as read back from a relay.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Published {
    #[serde(flatten)]
    pub record: Record,
    /// microseconds since the epoch; newer wins
    pub seq: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PkarrError {
    LabelTooLong,
    AddressTooLong,
    TooManyAddresses,
}

impl fmt::Display for PkarrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            PkarrError::LabelTooLong => "DNS label too long",
            PkarrError::AddressTooLong => "Address too long to publish",
            PkarrError::TooManyAddresses => "Too many addresses to publish",
        })
    }
}

impl std::error::Error for PkarrError {}

/// The bytes pkarr signs (BEP 44's bencoded seq and value).
pub fn signable(seq: u64, v: &[u8]) -> Vec<u8> {
    let mut out = format!("3:seqi{seq}e1:v{}:", v.len()).into_bytes();
    out.extend_from_slice(v);
    out
}

fn push_name(out: &mut Vec<u8>, labels: &[&str]) -> Result<(), PkarrError> {
    for l in labels {
        if l.len() > 63 {
            return Err(PkarrError::LabelTooLong);
        }
        out.push(l.len() as u8);
        out.extend_from_slice(l.as_bytes());
    }
    out.push(0);
    Ok(())
}

/// One TXT record "_tether.<id>" holding "m=", "n=", "u=<url>" and "r=<relay>" strings. The label
/// is a wire id from before the rename to Inyon: machines already publish under it and phones look
/// it up.
pub fn dns_packet(id: &str, p: &Record) -> Result<Vec<u8>, PkarrError> {
    let mut strings = Vec::new();
    // empty ones are left out, as the TS side's `p.machine ? ...` does
    if let Some(m) = p.machine.as_deref().filter(|m| !m.is_empty()) {
        strings.push(format!("m={m}"));
    }
    if let Some(n) = p.name.as_deref().filter(|n| !n.is_empty()) {
        strings.push(format!("n={}", js::slice(n, 64)));
    }
    strings.extend(p.urls.iter().map(|u| format!("u={u}")));
    strings.extend(p.relays.iter().map(|r| format!("r={r}")));
    let mut rdata = Vec::new();
    for s in strings {
        if s.len() > 255 {
            return Err(PkarrError::AddressTooLong);
        }
        rdata.push(s.len() as u8);
        rdata.extend_from_slice(s.as_bytes());
    }
    // id 0, flags: response + authoritative, 0 questions, 1 answer
    let mut out = vec![0, 0, 0x84, 0, 0, 0, 0, 1, 0, 0, 0, 0];
    push_name(&mut out, &["_tether", id])?;
    // TXT, IN, TTL 300
    out.extend_from_slice(&[0, 16, 0, 1, 0, 0, 1, 44]);
    out.extend_from_slice(&[(rdata.len() >> 8) as u8, rdata.len() as u8]);
    out.extend_from_slice(&rdata);
    if out.len() >= 1000 {
        return Err(PkarrError::TooManyAddresses);
    }
    Ok(out)
}

/// The body a pkarr relay takes: signature, seq (big-endian), DNS packet.
pub fn encode_published(identity: &Identity, p: &Record, seq: u64) -> Result<Vec<u8>, PkarrError> {
    let v = dns_packet(&identity.id, p)?;
    let sig = identity.sign(&signable(seq, &v));
    let mut body = Vec::with_capacity(72 + v.len());
    body.extend_from_slice(&sig);
    body.extend_from_slice(&seq.to_be_bytes());
    body.extend_from_slice(&v);
    Ok(body)
}

/// Checks a relay's answer against the machine id; None if it isn't signed by that machine.
pub fn decode_published(id: &str, body: &[u8]) -> Option<Published> {
    if body.len() < 72 + 12 {
        return None;
    }
    let seq = u64::from_be_bytes(body[64..72].try_into().ok()?);
    let v = &body[72..];
    if !verify(id, &signable(seq, v), &body[..64]) {
        return None;
    }
    let strings = read_packet(v)?;
    let all = |prefix: &str| strings.iter().filter_map(|s| s.strip_prefix(prefix)).map(str::to_string).collect::<Vec<_>>();
    let first = |prefix: &str| strings.iter().find_map(|s| s.strip_prefix(prefix)).map(str::to_string);
    Some(Published { record: Record { urls: all("u="), relays: all("r="), machine: first("m="), name: first("n=") }, seq })
}

/// The newest correctly signed record among the relays' answers.
pub fn newest<'a>(id: &str, bodies: impl IntoIterator<Item = &'a [u8]>) -> Option<Published> {
    bodies.into_iter().filter_map(|b| decode_published(id, b)).max_by_key(|p| p.seq)
}

/// The seq for a new record: the clock in microseconds, or one past the last record if the clock
/// went back (relays drop a record whose seq isn't above the one they hold).
pub fn next_seq(last_seq: u64, now_ms: u64) -> u64 {
    (now_ms * 1000).max(last_seq + 1)
}

/// Where a relay keeps a machine's record.
pub fn relay_url(relay: &str, id: &str) -> String {
    format!("{relay}/{id}")
}

/// `subarray`: indexes past the end are clamped, never an error.
fn sub(v: &[u8], from: usize, to: usize) -> &[u8] {
    let to = to.min(v.len());
    &v[from.min(to)..to]
}

fn u16_at(v: &[u8], at: usize) -> Option<usize> {
    Some(u16::from_be_bytes([*v.get(at)?, *v.get(at + 1)?]) as usize)
}

/// A name's labels and where the bytes after it start (following compression pointers).
fn read_name(v: &[u8], mut at: usize) -> Option<(Vec<String>, usize)> {
    let mut labels = Vec::new();
    let mut end = None;
    for _ in 0..32 {
        let len = *v.get(at)? as usize;
        if len == 0 {
            return Some((labels, end.unwrap_or(at + 1)));
        }
        if len & 0xc0 == 0xc0 {
            end.get_or_insert(at + 2);
            at = u16_at(v, at)? & 0x3fff;
            continue;
        }
        labels.push(String::from_utf8_lossy(sub(v, at + 1, at + 1 + len)).into_owned());
        at += 1 + len;
    }
    None
}

/// The TXT strings of every "_tether..." answer in a DNS packet; None if it doesn't parse.
fn read_packet(v: &[u8]) -> Option<Vec<String>> {
    let questions = u16_at(v, 4)?;
    let answers = u16_at(v, 6)?;
    let mut at = 12;
    for _ in 0..questions {
        at = read_name(v, at)?.1 + 4;
    }
    let mut out = Vec::new();
    for _ in 0..answers {
        let (labels, next) = read_name(v, at)?;
        let kind = u16_at(v, next)?;
        let len = u16_at(v, next + 8)?;
        let rdata = sub(v, next + 10, next + 10 + len);
        at = next + 10 + len;
        if kind != 16 || labels.first().map(String::as_str) != Some("_tether") {
            continue;
        }
        let mut j = 0;
        while j < rdata.len() {
            let n = rdata[j] as usize;
            out.push(String::from_utf8_lossy(sub(rdata, j + 1, j + 1 + n)).into_owned());
            j += 1 + n;
        }
    }
    Some(out)
}
