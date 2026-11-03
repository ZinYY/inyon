#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// A fresh directory under the system temp dir (the tests make their own throwaway keys, so no
/// private key is ever checked in).
pub fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("inyon-ssh-spike-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// `ssh-keygen -t <kind> [-b bits] [-m format] -N <pass>`; returns the private key path.
pub fn keygen(dir: &Path, name: &str, kind: &str, bits: Option<u32>, format: Option<&str>, pass: &str) -> PathBuf {
    let path = dir.join(name);
    let mut cmd = Command::new("ssh-keygen");
    cmd.args(["-q", "-t", kind, "-N", pass, "-C", name, "-f"]).arg(&path);
    if let Some(b) = bits {
        cmd.args(["-b", &b.to_string()]);
    }
    if let Some(m) = format {
        cmd.args(["-m", m]);
    }
    let st = cmd.status().expect("ssh-keygen is needed for these tests");
    assert!(st.success(), "ssh-keygen failed for {name}");
    path
}

/// What `ssh-keygen -l -E sha256` prints for a public key file: "bits SHA256:... comment (TYPE)".
pub fn keygen_fingerprint(pub_path: &Path) -> String {
    let out = Command::new("ssh-keygen").args(["-l", "-E", "sha256", "-f"]).arg(pub_path).output().unwrap();
    assert!(out.status.success());
    String::from_utf8(out.stdout).unwrap().split_whitespace().nth(1).unwrap().to_string()
}

pub fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap()
}
