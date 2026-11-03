//! A machine's or phone's identity: one ed25519 seed. Its public key, in z-base-32, is the
//! machine's id (the same form pkarr uses, so the id is also where its addresses are published),
//! and the matching X25519 key is its Noise static key.

use std::fmt;

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

use crate::noise::KeyPair;

#[derive(Clone)]
pub struct Identity {
    pub seed: [u8; 32],
    pub public_key: [u8; 32],
    pub id: String,
    pub noise: KeyPair,
}

// the seed stays out of logs
impl fmt::Debug for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Identity").field("id", &self.id).finish_non_exhaustive()
    }
}

impl Identity {
    pub fn from_seed(seed: [u8; 32]) -> Self {
        let signing = SigningKey::from_bytes(&seed);
        let public_key = signing.verifying_key().to_bytes();
        // the X25519 secret is the clamped first half of sha512(seed), as noble's toMontgomerySecret
        // gives it; X25519 clamps anyway, but the stored bytes stay the same on both sides
        let mut secret = signing.to_scalar_bytes();
        secret[0] &= 248;
        secret[31] &= 127;
        secret[31] |= 64;
        let noise = KeyPair { secret, public: signing.verifying_key().to_montgomery().to_bytes() };
        Identity { seed, public_key, id: z32encode(&public_key), noise }
    }

    pub fn generate() -> Result<Self, getrandom::Error> {
        let mut seed = [0u8; 32];
        getrandom::fill(&mut seed)?;
        Ok(Self::from_seed(seed))
    }

    pub fn sign(&self, msg: &[u8]) -> [u8; 64] {
        SigningKey::from_bytes(&self.seed).sign(msg).to_bytes()
    }
}

/// The Noise static key that belongs to a machine id; None if the id isn't a public key.
pub fn noise_key_of(id: &str) -> Option<[u8; 32]> {
    let bytes: [u8; 32] = z32decode(id)?.try_into().ok()?;
    Some(VerifyingKey::from_bytes(&bytes).ok()?.to_montgomery().to_bytes())
}

/// Checks a signature by a machine id (z-base-32).
pub fn verify(id: &str, msg: &[u8], sig: &[u8]) -> bool {
    z32decode(id).is_some_and(|key| verify_key(&key, msg, sig))
}

/// Checks a signature by a raw ed25519 public key.
pub fn verify_key(key: &[u8], msg: &[u8], sig: &[u8]) -> bool {
    let Ok(key) = <[u8; 32]>::try_from(key) else { return false };
    let Ok(key) = VerifyingKey::from_bytes(&key) else { return false };
    let Ok(sig) = Signature::from_slice(sig) else { return false };
    key.verify(msg, &sig).is_ok()
}

// z-base-32 (philzimmermann.com/docs/human-oriented-base-32-encoding.txt)
const ALPHABET: &[u8; 32] = b"ybndrfg8ejkmcpqxot1uwisza345h769";

pub fn z32encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 8 / 5 + 1);
    let mut bits = 0u32;
    let mut value = 0u32;
    for &b in bytes {
        value = (value << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            out.push(ALPHABET[((value >> (bits - 5)) & 31) as usize] as char);
            bits -= 5;
        }
        value &= (1 << bits) - 1;
    }
    if bits > 0 {
        out.push(ALPHABET[((value << (5 - bits)) & 31) as usize] as char);
    }
    out
}

/// Case-insensitive; None on a character outside the alphabet. Leftover bits are dropped.
pub fn z32decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() * 5 / 8);
    let mut bits = 0u32;
    let mut value = 0u32;
    for ch in text.chars() {
        let ch = ch.to_ascii_lowercase();
        let v = ALPHABET.iter().position(|&a| a as char == ch)? as u32;
        value = (value << 5) | v;
        bits += 5;
        if bits >= 8 {
            out.push(((value >> (bits - 8)) & 255) as u8);
            bits -= 8;
        }
        value &= (1 << bits) - 1;
    }
    Some(out)
}

/// A 32-byte public key's id is 52 lowercase characters, spelt the way the key encodes: the last
/// character carries four padding bits, so 16 spellings decode to the same key and only the one
/// with them zero counts. Otherwise the same machine could be saved twice under different ids.
pub fn is_machine_id(s: &str) -> bool {
    s.len() == 52 && s.bytes().all(|b| ALPHABET.contains(&b)) && z32decode(s).is_some_and(|b| z32encode(&b) == s)
}
