//! `inyon skill` end to end: the real binary against a local server standing in for the website's
//! copy of a release, signed with the test key, with HOME a throwaway folder.

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;

fn sign(data: &[u8]) -> String {
    let sk = minisign::SecretKeyBox::from_string(include_str!("keys/test.key")).unwrap().into_unencrypted_secret_key().unwrap();
    minisign::sign(None, &sk, data, Some("file:SHA256SUMS\thashed"), None).unwrap().into_string()
}

fn tarball(top: &str, files: &[(&str, &str)]) -> Vec<u8> {
    let mut b = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
    for (path, body) in files {
        let mut h = tar::Header::new_gnu();
        h.set_size(body.len() as u64);
        h.set_mode(0o644);
        b.append_data(&mut h, format!("{top}/{path}"), body.as_bytes()).unwrap();
    }
    b.into_inner().unwrap().finish().unwrap()
}

/// Serves `files` under /download, with SHA256SUMS over them (signed unless `tamper` changes a
/// file after signing) and latest.json.
fn serve(files: BTreeMap<String, Vec<u8>>, tamper: Option<&str>) -> String {
    let mut files = files;
    let sums: String = files.iter().map(|(n, b)| format!("{}  {n}\n", inyon_cli::update::sha256(b))).collect();
    if let Some(name) = tamper {
        files.get_mut(name).unwrap().push(b'!');
    }
    files.insert("SHA256SUMS.minisig".into(), sign(sums.as_bytes()).into_bytes());
    files.insert("SHA256SUMS".into(), sums.into_bytes());
    files.insert("latest.json".into(), br#"{"version":"9.9.9","tag":"v9.9.9"}"#.to_vec());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut s) = stream else { continue };
            let mut buf = Vec::new();
            let mut chunk = [0u8; 4096];
            while let Ok(n) = s.read(&mut chunk) {
                buf.extend_from_slice(&chunk[..n]);
                if n == 0 || buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let head = String::from_utf8_lossy(&buf).into_owned();
            let path = head.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("/").to_string();
            let body = path.strip_prefix("/download/").and_then(|n| files.get(n));
            let (status, body) = match body {
                Some(b) => ("200 OK", b.clone()),
                None => ("404 Not Found", b"not found".to_vec()),
            };
            let _ = write!(s, "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = s.write_all(&body);
        }
    });
    format!("http://127.0.0.1:{port}/download")
}

fn home(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("inyon-skill-e2e-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

fn inyon(home: &Path, base: &str, args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_inyon"))
        .args(args)
        .env("HOME", home)
        .env("USERPROFILE", home)
        .env("INYON_DOWNLOAD", base)
        .env("INYON_SIGNING_KEY", include_str!("keys/test.pub"))
        .env_remove("TETHER_DOWNLOAD")
        .env_remove("TETHER_SIGNING_KEY")
        .output()
        .unwrap();
    (out.status.success(), format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr)))
}

fn release() -> BTreeMap<String, Vec<u8>> {
    let mut f = BTreeMap::new();
    f.insert("inyon-linux-x64".into(), b"cli".to_vec());
    f.insert("inyon-skill-blueprint.tar.gz".into(), tarball("blueprint", &[("SKILL.md", "---\nname: blueprint\n---\n"), ("scripts/serve.mjs", "// serve")]));
    f
}

#[test]
fn lists_adds_and_removes_a_skill() {
    let base = serve(release(), None);
    let h = home("add");
    fs::create_dir_all(h.join(".claude")).unwrap();
    fs::create_dir_all(h.join(".codex")).unwrap();

    let (ok, out) = inyon(&h, &base, &["skill", "list"]);
    assert!(ok, "{out}");
    assert!(out.contains("blueprint") && out.contains("not installed"), "{out}");

    let (ok, out) = inyon(&h, &base, &["skill", "add", "blueprint"]);
    assert!(ok, "{out}");
    for dir in [".claude/skills/blueprint", ".agents/skills/blueprint"] {
        assert_eq!(fs::read_to_string(h.join(dir).join("scripts/serve.mjs")).unwrap(), "// serve");
        assert_eq!(fs::read_to_string(h.join(dir).join(".inyon-skill")).unwrap(), "9.9.9\n");
    }
    assert!(!h.join(".gemini").exists());

    let (ok, out) = inyon(&h, &base, &["--json", "skill", "list"]);
    assert!(ok, "{out}");
    let v: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
    assert_eq!(v["skills"][0]["installed"].as_array().unwrap().len(), 2);

    let (ok, out) = inyon(&h, &base, &["skill", "remove", "blueprint"]);
    assert!(ok, "{out}");
    assert!(!h.join(".claude/skills/blueprint").exists());
    let (ok, _) = inyon(&h, &base, &["skill", "remove", "blueprint"]);
    assert!(!ok);
    fs::remove_dir_all(&h).unwrap();
}

#[test]
fn installs_into_a_chosen_folder() {
    let base = serve(release(), None);
    let h = home("to");
    let to = h.join("project/.claude/skills");
    let (ok, out) = inyon(&h, &base, &["skill", "add", "blueprint", "--to", to.to_str().unwrap()]);
    assert!(ok, "{out}");
    assert!(to.join("blueprint/SKILL.md").is_file());
    assert!(!h.join(".agents").exists());
    fs::remove_dir_all(&h).unwrap();
}

#[test]
fn refuses_a_download_that_does_not_match() {
    let base = serve(release(), Some("inyon-skill-blueprint.tar.gz"));
    let h = home("tamper");
    let (ok, out) = inyon(&h, &base, &["skill", "add", "blueprint"]);
    assert!(!ok);
    assert!(out.contains("checksum"), "{out}");
    assert!(!h.join(".agents/skills/blueprint").exists());
    fs::remove_dir_all(&h).unwrap();
}

#[test]
fn names_what_the_release_has() {
    let base = serve(release(), None);
    let h = home("unknown");
    let (ok, out) = inyon(&h, &base, &["skill", "add", "nope"]);
    assert!(!ok);
    assert!(out.contains("This release has: blueprint"), "{out}");
    let (ok, out) = inyon(&h, &base, &["skill", "add", "../x"]);
    assert!(!ok);
    assert!(out.contains("isn't a skill name"), "{out}");
    fs::remove_dir_all(&h).unwrap();
}

#[test]
fn keeps_a_skill_someone_added_by_hand() {
    let base = serve(release(), None);
    let h = home("own");
    fs::create_dir_all(h.join(".agents/skills/blueprint")).unwrap();
    fs::write(h.join(".agents/skills/blueprint/SKILL.md"), "mine").unwrap();
    let (ok, out) = inyon(&h, &base, &["skill", "add", "blueprint"]);
    assert!(!ok);
    assert!(out.contains("--force"), "{out}");
    assert_eq!(fs::read_to_string(h.join(".agents/skills/blueprint/SKILL.md")).unwrap(), "mine");
    let (ok, out) = inyon(&h, &base, &["skill", "add", "blueprint", "--force"]);
    assert!(ok, "{out}");
    fs::remove_dir_all(&h).unwrap();
}
