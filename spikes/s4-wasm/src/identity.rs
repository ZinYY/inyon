//! The identity from packages/link/src/identity.ts: one ed25519 seed, its public key in
//! z-base-32 as the id, and the matching X25519 key as the Noise static key.

use curve25519_dalek::edwards::CompressedEdwardsY;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

pub struct Identity {
    key: SigningKey,
    pub id: String,
    pub noise_secret: [u8; 32],
    pub noise_public: [u8; 32],
}

impl Identity {
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        let key = SigningKey::from_bytes(seed);
        let public = key.verifying_key();
        // noble's toMontgomerySecret clamps the hash; dalek leaves it raw. X25519 clamps either
        // way, but the stored bytes should match the TypeScript ones.
        let mut noise_secret = key.to_scalar_bytes();
        noise_secret[0] &= 248;
        noise_secret[31] &= 127;
        noise_secret[31] |= 64;
        Identity { id: z32encode(public.as_bytes()), noise_secret, noise_public: public.to_montgomery().to_bytes(), key }
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.key.verifying_key().to_bytes()
    }

    pub fn sign(&self, msg: &[u8]) -> [u8; 64] {
        self.key.sign(msg).to_bytes()
    }
}

/// The Noise static key that belongs to a machine id (noiseKeyOf in identity.ts).
pub fn noise_key_of(id: &str) -> Option<[u8; 32]> {
    let bytes: [u8; 32] = z32decode(id)?.try_into().ok()?;
    Some(CompressedEdwardsY(bytes).decompress()?.to_montgomery().to_bytes())
}

pub fn verify(id: &str, msg: &[u8], sig: &[u8]) -> bool {
    let Some(Ok(key)) = z32decode(id).and_then(|b| <[u8; 32]>::try_from(b).ok()).map(|b| VerifyingKey::from_bytes(&b)) else {
        return false;
    };
    let Ok(sig) = Signature::from_slice(sig) else {
        return false;
    };
    key.verify(msg, &sig).is_ok()
}

const ALPHABET: &[u8; 32] = b"ybndrfg8ejkmcpqxot1uwisza345h769";

pub fn z32encode(bytes: &[u8]) -> String {
    let mut out = String::new();
    let (mut bits, mut value) = (0u32, 0u32);
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

pub fn z32decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut bits, mut value) = (0u32, 0u32);
    for ch in text.to_ascii_lowercase().bytes() {
        let v = ALPHABET.iter().position(|&a| a == ch)? as u32;
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

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    /// identityFromSeed(0, 1, ..., 31) from packages/link, printed by Bun
    #[test]
    fn matches_the_typescript_identity() {
        let seed: [u8; 32] = std::array::from_fn(|i| i as u8);
        let id = Identity::from_seed(&seed);
        assert_eq!(id.id, "yqooxx9u3aemh8mo5wcqq16yufu6jitouq1o4za751dger1igghy");
        assert_eq!(hex(&id.public_key()), "03a107bff3ce10be1d70dd18e74bc09967e4d6309ba50d5f1ddc8664125531b8");
        assert_eq!(hex(&id.noise_public), "4701d08488451f545a409fb58ae3e58581ca40ac3f7f114698cd71deac73ca01");
        assert_eq!(hex(&id.noise_secret), "3894eea49c580aef816935762be049559d6d1440dede12e6a125f1841fff8e6f");
        assert_eq!(noise_key_of(&id.id), Some(id.noise_public));
    }

    #[test]
    fn signs_and_verifies() {
        let id = Identity::from_seed(&[7; 32]);
        let sig = id.sign(b"binding");
        assert!(verify(&id.id, b"binding", &sig));
        assert!(!verify(&id.id, b"other", &sig));
        assert!(!verify("not an id", b"binding", &sig));
    }

    #[test]
    fn z32_round_trips() {
        for n in 0..40 {
            let b: Vec<u8> = (0..n).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(z32decode(&z32encode(&b)).unwrap()[..n], b[..]);
        }
    }
}
