//! A caller that puts a low-order point in message 1 (as its static or its ephemeral key) makes the
//! machine's DH with it all zero, a value anyone knows, so it needs no private key for that identity.
//! snow takes it; the machine must not. Message 1 is built here by hand, by the Noise spec, because
//! no real key pair has such a public half.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::ChaCha20Poly1305;
use curve25519_dalek::MontgomeryPoint;
use hmac::{Hmac, Mac};
use inyon_link::{accept, KeyPair, PROLOGUE};
use sha2::{Digest, Sha256};

fn hmac(key: &[u8], data: &[&[u8]]) -> [u8; 32] {
    let mut m = <Hmac<Sha256> as Mac>::new_from_slice(key).unwrap();
    for d in data {
        m.update(d);
    }
    m.finalize().into_bytes().into()
}

struct Symmetric {
    ck: [u8; 32],
    h: [u8; 32],
    k: Option<[u8; 32]>,
}

impl Symmetric {
    fn new() -> Self {
        let h: [u8; 32] = b"Noise_IK_25519_ChaChaPoly_SHA256".to_owned();
        Symmetric { ck: h, h, k: None }
    }
    fn mix_hash(&mut self, data: &[u8]) {
        self.h = Sha256::new().chain_update(self.h).chain_update(data).finalize().into();
    }
    fn mix_key(&mut self, ikm: &[u8]) {
        let temp = hmac(&self.ck, &[ikm]);
        self.ck = hmac(&temp, &[&[1]]);
        self.k = Some(hmac(&temp, &[&self.ck, &[2]]));
    }
    fn encrypt_and_hash(&mut self, plain: &[u8]) -> Vec<u8> {
        let k = self.k.unwrap();
        // each key is used once here, so the nonce is always 0
        let ct = ChaCha20Poly1305::new(&k.into()).encrypt(&[0u8; 12].into(), Payload { msg: plain, aad: &self.h }).unwrap();
        self.mix_hash(&ct);
        ct
    }
}

fn dh(secret: &[u8; 32], public: &[u8; 32]) -> [u8; 32] {
    MontgomeryPoint(*public).mul_clamped(*secret).to_bytes()
}

/// -> e, es, s, ss, payload, with the public keys and DH results given, so a key nobody holds the
/// private half of can be used.
fn message1(machine: &[u8; 32], e: &[u8; 32], es: &[u8; 32], s: &[u8; 32], ss: &[u8; 32], hello: &[u8]) -> Vec<u8> {
    let mut st = Symmetric::new();
    st.mix_hash(PROLOGUE);
    st.mix_hash(machine);
    st.mix_hash(e);
    let mut out = e.to_vec();
    st.mix_key(es);
    out.extend(st.encrypt_and_hash(s));
    st.mix_key(ss);
    out.extend(st.encrypt_and_hash(hello));
    out
}

/// The identity point (all zero), and a point of order 8.
fn low_points() -> [[u8; 32]; 2] {
    let order8: [u8; 32] = hex::decode("e0eb7a7c3b41b8ae1656e3faf19fc46ada098deb9c32b1fd866205165f49b800").unwrap().try_into().unwrap();
    [[0u8; 32], order8]
}

#[test]
fn a_message_1_built_by_hand_from_real_keys_is_accepted() {
    let machine = KeyPair::generate().unwrap();
    let (e, phone) = (KeyPair::generate().unwrap(), KeyPair::generate().unwrap());
    let msg = message1(&machine.public, &e.public, &dh(&e.secret, &machine.public), &phone.public, &dh(&phone.secret, &machine.public), b"{}");
    let a = accept(&machine, &msg).expect("the hand-built message follows the spec");
    assert_eq!(a.remote_static(), &phone.public);
    assert_eq!(a.hello(), b"{}");
}

#[test]
fn a_low_order_static_key_is_refused() {
    let machine = KeyPair::generate().unwrap();
    let e = KeyPair::generate().unwrap();
    for low in low_points() {
        // its DH with the machine is 0 whatever the machine's secret, so the caller needs no key
        assert_eq!(dh(&machine.secret, &low), [0u8; 32]);
        let msg = message1(&machine.public, &e.public, &dh(&e.secret, &machine.public), &low, &[0u8; 32], b"{}");
        assert!(accept(&machine, &msg).is_err(), "a static key of low order got in: {}", hex::encode(low));
    }
}

#[test]
fn a_low_order_ephemeral_key_is_refused() {
    let machine = KeyPair::generate().unwrap();
    let phone = KeyPair::generate().unwrap();
    for low in low_points() {
        let msg = message1(&machine.public, &low, &[0u8; 32], &phone.public, &dh(&phone.secret, &machine.public), b"{}");
        assert!(accept(&machine, &msg).is_err(), "an ephemeral key of low order got in: {}", hex::encode(low));
    }
}
