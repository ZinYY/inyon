//! The Rust link against the bytes the TS link produced (fixtures/link, written by
//! packages/link's vectors.ts before it was removed in 0.23). Every vector in every file is checked.

use std::fs;

use inyon_link::channel::{close, Accepting, Channel, Connecting, LinkError, CHUNK, MAX_MESSAGE, PROLOGUE};
use inyon_link::code::{format_short_id, parse_short_code, short_code_identity, short_secret, ShortCode};
use inyon_link::identity::{is_machine_id, noise_key_of, verify, z32decode, z32encode, Identity};
use inyon_link::noise::{Initiator, KeyPair, Responder, Transport, PROTOCOL};
use inyon_link::pair::{pair_link, parse_pair_link, PairInfo};
use inyon_link::pkarr::{decode_published, encode_published, signable, Record, PKARR_RELAYS};
use inyon_link::relay::{auth_message, auth_message_v2, frame, machine_path, phone_path, relay_origin, relay_ws, unframe};
use serde_json::{json, Value};

fn read(name: &str) -> Value {
    let path = format!("{}/../../fixtures/link/{name}.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"))).unwrap()
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("not a string: {v}"))
}

fn bytes(v: &Value) -> Vec<u8> {
    hex::decode(s(v)).unwrap()
}

fn key(v: &Value) -> [u8; 32] {
    bytes(v).try_into().unwrap()
}

fn sha256(b: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    hex::encode(Sha256::digest(b))
}

#[test]
fn identities() {
    let v = read("identity");
    for i in v["identities"].as_array().unwrap() {
        let id = Identity::from_seed(key(&i["seed"]));
        assert_eq!(hex::encode(id.public_key), s(&i["public_key"]));
        assert_eq!(id.id, s(&i["id"]));
        assert_eq!(hex::encode(id.noise.secret), s(&i["x25519_secret"]));
        assert_eq!(hex::encode(id.noise.public), s(&i["x25519_public"]));
        assert_eq!(KeyPair::from_secret(id.noise.secret).public, id.noise.public);
        assert_eq!(noise_key_of(&id.id), Some(id.noise.public));
        for sig in i["signatures"].as_array().unwrap() {
            let msg = bytes(&sig["message"]);
            assert_eq!(hex::encode(id.sign(&msg)), s(&sig["signature"]));
            assert!(verify(&id.id, &msg, &bytes(&sig["signature"])));
        }
    }
    for z in v["z32"].as_array().unwrap() {
        assert_eq!(z32encode(&bytes(&z["bytes"])), s(&z["text"]));
        assert_eq!(z32decode(s(&z["text"])).unwrap(), bytes(&z["bytes"]));
    }
    for m in v["machine_ids"].as_array().unwrap() {
        assert_eq!(is_machine_id(s(&m["text"])), m["valid"].as_bool().unwrap(), "{}", m["text"]);
    }
    for c in v["verifications"].as_array().unwrap() {
        assert_eq!(verify(s(&c["id"]), &bytes(&c["message"]), &bytes(&c["signature"])), c["valid"].as_bool().unwrap(), "{c}");
    }
}

/// Runs a recorded handshake both ways; returns the two transports (initiator's, responder's).
fn handshake(c: &Value) -> (Transport, Transport) {
    let prologue = bytes(&c["prologue"]);
    let i_static = KeyPair::from_secret(key(&c["initiator_static"]));
    let r_static = KeyPair::from_secret(key(&c["responder_static"]));
    assert_eq!(hex::encode(i_static.public), s(&c["initiator_static_public"]));
    assert_eq!(hex::encode(r_static.public), s(&c["responder_static_public"]));

    let mut i = Initiator::with_ephemeral(&i_static, &r_static.public, &prologue, &key(&c["initiator_ephemeral"])).unwrap();
    let mut r = Responder::with_ephemeral(&r_static, &prologue, &key(&c["responder_ephemeral"])).unwrap();
    let m1 = i.write_message1(&bytes(&c["payload1"])).unwrap();
    assert_eq!(hex::encode(&m1), s(&c["message1"]), "{}: message 1", c["name"]);
    assert_eq!(hex::encode(r.read_message1(&m1).unwrap()), s(&c["payload1"]));
    assert_eq!(r.remote_static(), Some(i_static.public));

    let (m2, rt) = r.write_message2(&bytes(&c["payload2"])).unwrap();
    assert_eq!(hex::encode(&m2), s(&c["message2"]), "{}: message 2", c["name"]);
    let (payload, it) = i.read_message2(&m2).unwrap();
    assert_eq!(hex::encode(payload), s(&c["payload2"]));
    assert_eq!(hex::encode(it.hash()), s(&c["handshake_hash"]));
    assert_eq!(hex::encode(rt.hash()), s(&c["handshake_hash"]));
    (it, rt)
}

#[test]
fn noise_handshakes_and_transport() {
    let v = read("noise");
    assert_eq!(s(&v["protocol"]), PROTOCOL);
    assert_eq!(bytes(&v["prologue_used_by_inyon"]), PROLOGUE);
    let cases = v["cases"].as_array().unwrap();
    assert!(cases.len() >= 4);
    for c in cases {
        let (mut it, mut rt) = handshake(c);
        for m in c["transport"].as_array().unwrap() {
            let (from, to) = if s(&m["from"]) == "initiator" { (&mut it, &mut rt) } else { (&mut rt, &mut it) };
            let ct = from.encrypt(&bytes(&m["payload"])).unwrap();
            assert_eq!(hex::encode(&ct), s(&m["ciphertext"]), "{}: transport", c["name"]);
            assert_eq!(hex::encode(to.decrypt(&ct).unwrap()), s(&m["payload"]));
        }
    }
}

#[test]
fn noise_reads_the_recorded_messages() {
    // each side takes the other side's recorded bytes, not its own
    for c in read("noise")["cases"].as_array().unwrap() {
        let mut r = Responder::with_ephemeral(&KeyPair::from_secret(key(&c["responder_static"])), &bytes(&c["prologue"]), &key(&c["responder_ephemeral"])).unwrap();
        assert_eq!(hex::encode(r.read_message1(&bytes(&c["message1"])).unwrap()), s(&c["payload1"]));
        let i_static = KeyPair::from_secret(key(&c["initiator_static"]));
        let mut i = Initiator::with_ephemeral(&i_static, &key(&c["responder_static_public"]), &bytes(&c["prologue"]), &key(&c["initiator_ephemeral"])).unwrap();
        i.write_message1(&bytes(&c["payload1"])).unwrap();
        let (payload, mut t) = i.read_message2(&bytes(&c["message2"])).unwrap();
        assert_eq!(hex::encode(payload), s(&c["payload2"]));
        // the responder's recorded transport messages decrypt on the initiator's side
        for m in c["transport"].as_array().unwrap() {
            if s(&m["from"]) == "responder" {
                // the initiator's own sends don't touch its receive counter, so only the responder's are read here
                assert_eq!(hex::encode(t.decrypt(&bytes(&m["ciphertext"])).unwrap()), s(&m["payload"]));
            }
        }
    }
}

#[test]
fn a_tampered_handshake_or_transport_message_is_rejected() {
    let c = &read("noise")["cases"][0];
    let mut m1 = bytes(&c["message1"]);
    m1[40] ^= 1;
    let mut r = Responder::new(&KeyPair::from_secret(key(&c["responder_static"])), &bytes(&c["prologue"])).unwrap();
    assert!(r.read_message1(&m1).is_err());
    // the wrong prologue is another protocol
    let mut r = Responder::new(&KeyPair::from_secret(key(&c["responder_static"])), b"something else").unwrap();
    assert!(r.read_message1(&bytes(&c["message1"])).is_err());

    let (mut it, mut rt) = handshake(c);
    let mut ct = it.encrypt(b"hello").unwrap();
    ct[0] ^= 1;
    assert!(rt.decrypt(&ct).is_err());
}

fn body(b: &Value) -> Vec<u8> {
    match b.get("text") {
        Some(t) => s(t).as_bytes().to_vec(),
        None => format!("{}{}{}", s(&b["prefix"]), s(&b["fill"]).repeat(b["count"].as_u64().unwrap() as usize), s(&b["suffix"])).into_bytes(),
    }
}

#[test]
fn channel_frames() {
    let v = read("channel");
    assert_eq!(v["chunk"].as_u64().unwrap() as usize, CHUNK);
    assert_eq!(v["max_message"].as_u64().unwrap() as usize, MAX_MESSAGE);
    assert_eq!(
        v["close_codes"],
        json!({ "bad_message": close::BAD_MESSAGE, "too_large": close::TOO_LARGE, "bad_handshake": close::BAD_HANDSHAKE, "refused": close::REFUSED, "handshake_timeout": close::HANDSHAKE_TIMEOUT })
    );
    let noise = read("noise");
    let c = noise["cases"].as_array().unwrap().iter().find(|c| c["name"] == v["handshake"]).unwrap();
    assert_eq!(bytes(&c["prologue"]), PROLOGUE);

    // through the connect/accept steps the app uses, with the recorded ephemeral keys
    let phone = KeyPair::from_secret(key(&c["initiator_static"]));
    let machine = KeyPair::from_secret(key(&c["responder_static"]));
    let (connecting, m1) = Connecting::with_ephemeral(&phone, &machine.public, &bytes(&c["payload1"]), &key(&c["initiator_ephemeral"])).unwrap();
    assert_eq!(hex::encode(&m1), s(&c["message1"]));
    let accepting = Accepting::with_ephemeral(&machine, &m1, &key(&c["responder_ephemeral"])).unwrap();
    assert_eq!(accepting.remote_static(), &phone.public);
    assert_eq!(accepting.hello(), bytes(&c["payload1"]));
    let (mut machine_ch, m2) = accepting.welcome(&bytes(&c["payload2"])).unwrap();
    assert_eq!(hex::encode(&m2), s(&c["message2"]));
    let (mut phone_ch, welcome) = connecting.finish(&m2).unwrap();
    assert_eq!(welcome, serde_json::from_slice::<Value>(&bytes(&c["payload2"])).unwrap());
    assert_eq!(phone_ch.binding(), machine_ch.binding());

    for m in v["messages"].as_array().unwrap() {
        let b = body(&m["body"]);
        assert_eq!(b.len() as u64, m["body_len"].as_u64().unwrap());
        assert_eq!(sha256(&b), s(&m["body_sha256"]));
        let (from, to): (&mut Channel, &mut Channel) = if s(&m["from"]) == "phone" { (&mut phone_ch, &mut machine_ch) } else { (&mut machine_ch, &mut phone_ch) };
        let frames = from.seal(&b).unwrap();
        let want = m["frames"].as_array().unwrap();
        assert_eq!(frames.len(), want.len());
        for (n, (f, w)) in frames.iter().zip(want).enumerate() {
            assert_eq!(f.len() as u64, w["len"].as_u64().unwrap());
            assert_eq!(sha256(f), s(&w["sha256"]));
            if let Some(h) = w.get("hex") {
                assert_eq!(hex::encode(f), s(h));
            }
            let got = to.open(f).unwrap();
            assert_eq!(got.is_some(), w["final"].as_bool().unwrap());
            if n == frames.len() - 1 {
                assert_eq!(got.unwrap(), b);
            }
        }
    }
}

#[test]
fn a_refused_phone_hears_why() {
    let noise = read("noise");
    let c = noise["cases"].as_array().unwrap().iter().find(|c| c["name"] == "refused").unwrap();
    let phone = KeyPair::from_secret(key(&c["initiator_static"]));
    let machine = KeyPair::from_secret(key(&c["responder_static"]));
    let (connecting, m1) = Connecting::with_ephemeral(&phone, &machine.public, &bytes(&c["payload1"]), &key(&c["initiator_ephemeral"])).unwrap();
    let accepting = Accepting::with_ephemeral(&machine, &m1, &key(&c["responder_ephemeral"])).unwrap();
    // the refusal is written exactly as the TS machine writes it
    let m2 = accepting.refuse(&LinkError::NotPaired("This phone is not paired with build-box.".into())).unwrap();
    assert_eq!(hex::encode(&m2), s(&c["message2"]));
    let err = connecting.finish(&m2).err().unwrap();
    assert_eq!(err, LinkError::NotPaired("This phone is not paired with build-box.".into()));
}

#[test]
fn short_codes() {
    let v = read("code");
    assert_eq!(v["scrypt"], json!({ "salt": "inyon-pair-id:v2", "n": 16384, "r": 8, "p": 1, "len": 32 }));
    for c in v["codes"].as_array().unwrap() {
        let code = ShortCode { id: s(&c["id"]).into(), code: s(&c["code"]).into() };
        assert_eq!(format_short_id(&code.id), s(&c["formatted"]));
        assert_eq!(short_secret(&code), s(&c["secret"]));
        let id = short_code_identity(&code);
        assert_eq!(hex::encode(id.seed), s(&c["seed"]));
        assert_eq!(id.id, s(&c["machine_id"]));
        assert_eq!(parse_short_code(&format!("{} {}", s(&c["formatted"]), code.code)), Some(code));
    }
    for p in v["parse"].as_array().unwrap() {
        let got = parse_short_code(s(&p["text"])).map(|c| serde_json::to_value(c).unwrap()).unwrap_or(Value::Null);
        assert_eq!(got, p["result"], "{}", p["text"]);
    }
}

#[test]
fn pairing_links() {
    let v = read("pair");
    for l in v["links"].as_array().unwrap() {
        let info: PairInfo = serde_json::from_value(l["info"].clone()).unwrap();
        assert_eq!(pair_link(&info, l.get("web").map(s)), s(&l["link"]));
    }
    for p in v["parse"].as_array().unwrap() {
        let got = parse_pair_link(s(&p["text"])).map(|i| serde_json::to_value(i).unwrap()).unwrap_or(Value::Null);
        assert_eq!(got, p["result"], "{}", p["text"]);
    }
}

#[test]
fn pkarr_records() {
    let v = read("pkarr");
    assert_eq!(v["relays"], json!(PKARR_RELAYS));
    let machine = Identity::from_seed(key(&v["machine_seed"]));
    assert_eq!(machine.id, s(&v["machine_id"]));
    for r in v["records"].as_array().unwrap() {
        let record: Record = serde_json::from_value(r["published"].clone()).unwrap();
        let seq = r["seq"].as_u64().unwrap();
        let body = encode_published(&machine, &record, seq).unwrap();
        assert_eq!(hex::encode(&body[72..]), s(&r["dns_packet"]));
        assert_eq!(hex::encode(signable(seq, &body[72..])), s(&r["signable"]));
        assert_eq!(hex::encode(&body), s(&r["body"]));
        assert_eq!(decoded(decode_published(&machine.id, &body)), r["decoded"]);
    }
    for d in v["decode"].as_array().unwrap() {
        assert_eq!(decoded(decode_published(s(&d["id"]), &bytes(&d["body"]))), d["result"], "{}", d["name"]);
    }
}

fn decoded(p: Option<inyon_link::Published>) -> Value {
    p.map_or(Value::Null, |p| json!({ "urls": p.record.urls, "relays": p.record.relays, "seq": p.seq, "machine": p.record.machine, "name": p.record.name }))
}

#[test]
fn relay_frames_and_auth() {
    let v = read("relay");
    for f in v["frames"].as_array().unwrap() {
        let sid = f["sid"].as_u64().unwrap() as u32;
        let framed = frame(sid, &bytes(&f["data"]));
        assert_eq!(hex::encode(&framed), s(&f["framed"]));
        assert_eq!(unframe(&framed), Some((sid, &bytes(&f["data"])[..])));
    }
    assert_eq!(unframe(&[1, 2, 3]), None);
    for a in v["auth"].as_array().unwrap() {
        let id = Identity::from_seed(key(&a["seed"]));
        assert_eq!(id.id, s(&a["id"]));
        let msg = auth_message(s(&a["nonce"]));
        assert_eq!(hex::encode(&msg), s(&a["message"]));
        assert_eq!(hex::encode(id.sign(&msg)), s(&a["signature"]));
    }
    for a in v["auth_v2"].as_array().unwrap() {
        let id = Identity::from_seed(key(&a["seed"]));
        let msg = auth_message_v2(s(&a["origin"]), s(&a["nonce"]));
        assert_eq!(hex::encode(&msg), s(&a["message"]));
        assert_eq!(hex::encode(id.sign(&msg)), s(&a["signature"]));
        // made for one relay, it doesn't verify for another
        let other = auth_message_v2("https://other.example", s(&a["nonce"]));
        assert!(inyon_link::verify(&id.id, &msg, &id.sign(&msg)));
        assert!(!inyon_link::verify(&id.id, &other, &id.sign(&msg)));
    }
    for o in v["origins"].as_array().unwrap() {
        assert_eq!(relay_origin(s(&o["relay"])).as_deref(), o["origin"].as_str(), "{}", o["relay"]);
    }
    let id = s(&read("pkarr")["machine_id"]).to_string();
    assert_eq!(machine_path(&id), s(&v["paths"]["machine"]));
    assert_eq!(phone_path(&id), s(&v["paths"]["phone"]));
    for w in v["ws_urls"].as_array().unwrap() {
        assert_eq!(relay_ws(s(&w["relay"]), s(&w["path"])), s(&w["url"]));
    }
}
