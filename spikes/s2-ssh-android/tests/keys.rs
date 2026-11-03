//! Key formats a user may paste into the phone, read with the same code the app would use.

mod common;

use common::*;
use inyon_ssh_spike::{fingerprint, load_private_key};

fn check(name: &str, kind: &str, bits: Option<u32>, format: Option<&str>, pass: &str, algo: &str) {
    let dir = temp_dir(name);
    let path = keygen(&dir, name, kind, bits, format, pass);
    let text = read(&path);
    let key = load_private_key(&text, if pass.is_empty() { None } else { Some(pass) }).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(key.algorithm().as_str(), algo, "{name}");
    // the same text form as `ssh-keygen -l` and SshCore's probe
    let want = keygen_fingerprint(&path.with_extension("pub"));
    assert_eq!(fingerprint(key.public_key()), want, "{name}");
    if !pass.is_empty() {
        let Err(err) = load_private_key(&text, Some("not the passphrase")) else { panic!("a wrong passphrase must fail") };
        assert_eq!(err.code(), "AUTH_FAILED", "{name}");
        assert!(load_private_key(&text, None).is_err(), "{name}: no passphrase must fail");
    }
    std::fs::remove_dir_all(dir).ok();
}

#[test]
fn ed25519_openssh() {
    check("ed25519", "ed25519", None, None, "", "ssh-ed25519");
}

#[test]
fn ed25519_openssh_with_passphrase() {
    check("ed25519-pass", "ed25519", None, None, "hunter2 two", "ssh-ed25519");
}

#[test]
fn rsa_openssh_with_passphrase() {
    check("rsa-pass", "rsa", Some(3072), None, "hunter2", "ssh-rsa");
}

#[test]
fn rsa_openssh() {
    check("rsa", "rsa", Some(2048), None, "", "ssh-rsa");
}

#[test]
fn ecdsa_openssh() {
    check("ecdsa", "ecdsa", Some(256), None, "", "ecdsa-sha2-nistp256");
}

#[test]
fn rsa_pkcs8_with_passphrase() {
    check("rsa-pkcs8", "rsa", Some(2048), Some("PKCS8"), "hunter2", "ssh-rsa");
}

/// "-----BEGIN RSA PRIVATE KEY-----", what older guides and tools still produce.
#[test]
fn rsa_legacy_pem() {
    check("rsa-pem", "rsa", Some(2048), Some("PEM"), "", "ssh-rsa");
}

#[test]
fn rsa_legacy_pem_with_passphrase() {
    check("rsa-pem-pass", "rsa", Some(2048), Some("PEM"), "hunter2", "ssh-rsa");
}

#[test]
fn garbage_is_a_clear_error() {
    let Err(err) = load_private_key("not a key", None) else { panic!("expected an error") };
    assert_eq!(err.code(), "AUTH_FAILED");
}
