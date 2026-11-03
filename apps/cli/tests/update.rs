//! Nothing real is touched: home is a throwaway folder, a local server stands in for the releases
//! (the website's copy under /download, GitHub's under /releases) and the hub, and restarting the
//! services is a fake that records what it was asked.

mod common;

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashSet};
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use common::{lock, Home};
use inyon_cli::service::{Installed, Role};
use inyon_cli::signing;
use inyon_cli::sys::{Libc, Platform, RealProc};
use inyon_cli::update::{
    self, cleanup_old, find_install, latest_version, reinstall_command, release_names, swap_back, swap_in, wait_started, Arch, Error, Files, Host, Kind, RealFiles, WaitOptions,
};

fn sha(b: &[u8]) -> String {
    update::sha256(b)
}

/// The key the mock releases are signed with (tests/keys, no password), as its .key file text.
fn test_key() -> String {
    include_str!("keys/test.key").to_string()
}

fn test_pub() -> String {
    signing::key_line(include_str!("keys/test.pub")).unwrap()
}

/// A key that isn't the test key, in the same form.
fn other_key() -> String {
    minisign::KeyPair::generate_unencrypted_keypair().unwrap().sk.to_box(None).unwrap().into_string()
}

/// What `minisign -S` writes: the hashed kind of signature, which is the only kind accepted.
fn sign(key_text: &str, data: &[u8]) -> String {
    let sk = minisign::SecretKeyBox::from_string(key_text).unwrap().into_unencrypted_secret_key().unwrap();
    minisign::sign(None, &sk, data, Some("file:SHA256SUMS\thashed"), None).unwrap().into_string()
}

fn gzip(b: &[u8]) -> Vec<u8> {
    let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    e.write_all(b).unwrap();
    e.finish().unwrap()
}

/// A release: each file is attached plain and gzipped too, and SHA256SUMS lists both (or whatever
/// `sums` says). Files in `missing` are listed but not there to download. SHA256SUMS.minisig is
/// signed with `signer` (the test key unless a test says otherwise; none means no signature file)
/// over the list served, or over `signed_text` when a test wants the signature not to match.
/// The website's latest.json names `version` and lists the files, unless `listed` is off (not
/// there at all) or `listing` replaces it. Like a real release it carries inyon-<version>.apk, which
/// SHA256SUMS lists (custom `sums` too) unless `apk` is off.
#[derive(Clone)]
struct Release {
    version: String,
    files: BTreeMap<String, String>,
    gz: bool,
    sums: String,
    missing: Vec<String>,
    signer: Option<String>,
    signed_text: Option<String>,
    listed: bool,
    listing: Option<String>,
    apk: bool,
}

impl Release {
    fn signature(&self) -> Option<Vec<u8>> {
        let over = self.signed_text.clone().unwrap_or_else(|| self.sums());
        self.signer.as_deref().map(|k| sign(k, over.as_bytes()).into_bytes())
    }
    fn apk(&self) -> String {
        format!("inyon-{}.apk", self.version)
    }
    fn assets(&self) -> BTreeMap<String, Vec<u8>> {
        let mut a = BTreeMap::new();
        if self.apk {
            a.insert(self.apk(), b"the app".to_vec());
        }
        for (name, body) in &self.files {
            a.insert(name.clone(), body.as_bytes().to_vec());
            if self.gz {
                a.insert(format!("{name}.gz"), gzip(body.as_bytes()));
            }
        }
        a
    }
    fn sums(&self) -> String {
        if !self.sums.is_empty() {
            let apk = if self.apk { format!("{}  {}\n", sha(b"the app"), self.apk()) } else { String::new() };
            return format!("{}{apk}", self.sums);
        }
        self.assets().iter().map(|(name, body)| format!("{}  {name}\n", sha(body))).collect()
    }
    /// latest.json as the website's build writes it
    fn listing(&self) -> String {
        if let Some(l) = &self.listing {
            return l.clone();
        }
        let files: Vec<serde_json::Value> = self.assets().iter().map(|(name, body)| serde_json::json!({ "name": name, "size": body.len(), "sha256": sha(body) })).collect();
        serde_json::json!({ "version": self.version, "tag": format!("v{}", self.version), "date": "2026-10-13", "files": files }).to_string()
    }
}

/// Where the mock serves a release: the website's copy, or GitHub's newest.
const SITE: &str = "/download";
const GITHUB: &str = "/releases/latest/download";

/// What a hub hands out: the engine and the inyon command for each system (keyed os-arch; a
/// Windows engine is always the x64 one). A hub from before 0.13 (`split` off) serves one
/// program, the engine, under /node/inyon and nothing under /node/inyon-engine.
struct HubBuilds {
    engines: BTreeMap<String, String>,
    commands: BTreeMap<String, String>,
    sum_override: String,
    split: bool,
    /// whether it hands out the release's signed list under /node/release/ (a hub from 0.20 on)
    signed: bool,
    /// the list as it was signed, when a test changes the builds afterwards
    signed_list: Option<String>,
    /// the key the list is signed with
    signer: String,
    /// the version it says it hands out (/node/release/version), and its list names the APK of
    version: String,
    /// what /node/release/SHA256SUMS answers instead of the list, as a hub that can't reach GitHub
    list_status: Option<&'static str>,
}

impl HubBuilds {
    /// The release's SHA256SUMS for what the hub hands out.
    fn release_sums(&self) -> String {
        if let Some(l) = &self.signed_list {
            return l.clone();
        }
        let cli = self.commands.iter().map(|(t, b)| format!("{}  {}\n", sha(b.as_bytes()), update::release_files(t).cli));
        let engine = self.engines.iter().map(|(t, b)| format!("{}  {}\n", sha(b.as_bytes()), update::release_files(t).engine));
        let apk = format!("{}  inyon-{}.apk\n", sha(b"apk"), self.version);
        cli.chain(engine).chain([apk]).collect()
    }

    fn engine_for(&self, os: &str, arch: &str) -> Option<String> {
        self.engines.get(&format!("{os}-{}", if os == "windows" { "x64" } else { arch })).cloned()
    }
}

struct Server {
    base: String,
    release: Arc<Mutex<Release>>,
    hub: Arc<Mutex<HubBuilds>>,
    requests: Arc<Mutex<Vec<String>>>,
}

impl Server {
    fn start() -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let release = Arc::new(Mutex::new(Release {
            version: "9.9.9".into(),
            files: BTreeMap::new(),
            gz: true,
            sums: String::new(),
            missing: Vec::new(),
            signer: Some(test_key()),
            signed_text: None,
            listed: true,
            listing: None,
            apk: true,
        }));
        let hub = Arc::new(Mutex::new(HubBuilds {
            engines: [("linux-x64", "new linux build"), ("windows-x64", "new exe")].into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            commands: [("linux-x64", "new linux cli"), ("windows-x64", "new cli exe"), ("windows-arm64", "new arm cli exe")]
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            sum_override: String::new(),
            split: true,
            signed: true,
            signed_list: None,
            signer: test_key(),
            version: "0.20.0".into(),
            list_status: None,
        }));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (r, h, q) = (release.clone(), hub.clone(), requests.clone());
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
                let target = head.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("/").to_string();
                q.lock().unwrap().push(target.clone());
                let (path, query) = target.split_once('?').map(|(p, q)| (p.to_string(), q.to_string())).unwrap_or((target.clone(), String::new()));
                let param = |k: &str| query.split('&').find_map(|kv| kv.strip_prefix(&format!("{k}=")).map(String::from)).unwrap_or_default();
                let (status, extra, body): (&str, String, Vec<u8>) = {
                    let rel = r.lock().unwrap();
                    let hb = h.lock().unwrap();
                    let asset = path.strip_prefix(&format!("{SITE}/")).or_else(|| path.strip_prefix(&format!("{GITHUB}/")));
                    let not_found = ("404 Not Found", String::new(), b"not found".to_vec());
                    if path == "/releases/latest" {
                        ("302 Found", format!("Location: https://github.com/x/y/releases/tag/v{}\r\n", rel.version), Vec::new())
                    } else if path == format!("{SITE}/latest.json") {
                        if rel.listed {
                            ("200 OK", String::new(), rel.listing().into_bytes())
                        } else {
                            not_found
                        }
                    } else if asset == Some("SHA256SUMS") {
                        ("200 OK", String::new(), rel.sums().into_bytes())
                    } else if asset == Some("SHA256SUMS.minisig") {
                        match rel.signature() {
                            Some(sig) => ("200 OK", String::new(), sig),
                            None => not_found,
                        }
                    } else if let Some(asset) = asset {
                        match (!rel.missing.iter().any(|m| m == asset)).then(|| rel.assets().get(asset).cloned()).flatten() {
                            Some(b) => ("200 OK", String::new(), b),
                            None => not_found,
                        }
                    } else {
                        let (os, arch) = (param("os"), param("arch"));
                        let engine = hb.engine_for(&os, &arch);
                        let command = if hb.split { hb.commands.get(&format!("{os}-{arch}")).cloned() } else { engine.clone() };
                        let engine_sum = engine.as_ref().map(|b| if hb.sum_override.is_empty() { sha(b.as_bytes()) } else { hb.sum_override.clone() });
                        match (path.as_str(), hb.split) {
                            ("/node/release/SHA256SUMS", _) if hb.list_status.is_some() => {
                                (hb.list_status.unwrap(), String::new(), b"Could not get the checksums of Inyon 0.20.0 (503)\n".to_vec())
                            }
                            ("/node/release/SHA256SUMS", _) if hb.signed => ("200 OK", String::new(), hb.release_sums().into_bytes()),
                            ("/node/release/version", _) if hb.signed => ("200 OK", String::new(), format!("{}\n", hb.version).into_bytes()),
                            ("/node/release/SHA256SUMS.minisig", _) if hb.signed => ("200 OK", String::new(), sign(&hb.signer, hb.release_sums().as_bytes()).into_bytes()),
                            ("/node/inyon-engine.sha256", true) if engine.is_some() => ("200 OK", String::new(), engine_sum.unwrap().into_bytes()),
                            ("/node/inyon-engine.gz", true) if engine.is_some() => ("200 OK", String::new(), gzip(engine.unwrap().as_bytes())),
                            ("/node/inyon.sha256", split) if command.is_some() => {
                                ("200 OK", String::new(), if split { sha(command.unwrap().as_bytes()) } else { engine_sum.unwrap() }.into_bytes())
                            }
                            ("/node/inyon.gz", _) if command.is_some() => ("200 OK", String::new(), gzip(command.unwrap().as_bytes())),
                            _ => ("404 Not Found", String::new(), b"not found".to_vec()),
                        }
                    }
                };
                let _ = write!(s, "HTTP/1.1 {status}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = s.write_all(&body);
                let _ = s.flush();
            }
        });
        Server { base: format!("http://127.0.0.1:{port}"), release, hub, requests }
    }

    fn release(&self, f: impl FnOnce(&mut Release)) {
        f(&mut self.release.lock().unwrap());
    }

    fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }

    fn clear_requests(&self) {
        self.requests.lock().unwrap().clear();
    }

    /// the release files fetched, from either place, leaving out the list, its signature and latest.json
    fn downloads(&self) -> Vec<String> {
        self.requests()
            .iter()
            .filter_map(|r| r.strip_prefix(&format!("{SITE}/")).or_else(|| r.strip_prefix(&format!("{GITHUB}/"))).map(String::from))
            .filter(|n| !["SHA256SUMS", "SHA256SUMS.minisig", "latest.json"].contains(&n.as_str()))
            .collect()
    }
}

struct FakeHost {
    base: String,
    version: String,
    roles: Vec<Role>,
    restarts: RefCell<Vec<(Role, Vec<String>)>>,
    waited: RefCell<Vec<Role>>,
    started_ok: Cell<bool>,
    /// jobs still under tether-node's names, and what happened in which order
    old: Vec<Role>,
    moved: RefCell<Vec<Role>>,
    order: RefCell<Vec<String>>,
    platform: Platform,
    arch: Arch,
    libc: Libc,
}

/// The system the tests act as: this one, except that Windows acts as Linux unless a test says
/// Windows, since a Windows update makes the windowless copy from the program it fetched, and the
/// programs here are made up.
fn here() -> Platform {
    match Platform::current() {
        Platform::Windows => Platform::Linux,
        p => p,
    }
}

impl FakeHost {
    fn new(server: &Server, roles: &[Role]) -> FakeHost {
        FakeHost {
            base: format!("{}{SITE}", server.base),
            version: "0.6.3".into(),
            roles: roles.to_vec(),
            restarts: RefCell::new(Vec::new()),
            waited: RefCell::new(Vec::new()),
            started_ok: Cell::new(true),
            old: Vec::new(),
            moved: RefCell::new(Vec::new()),
            order: RefCell::new(Vec::new()),
            platform: here(),
            arch: Arch::current(),
            libc: Libc::Glibc,
        }
    }
    fn restarted_roles(&self) -> Vec<Role> {
        self.restarts.borrow().iter().map(|(r, _)| *r).collect()
    }
}

impl Host for FakeHost {
    fn version(&self) -> String {
        self.version.clone()
    }
    fn roles(&self) -> Vec<Role> {
        self.roles.clone()
    }
    fn restart(&self, role: Role, cmd: &[String]) -> Installed {
        self.restarts.borrow_mut().push((role, cmd.to_vec()));
        self.order.borrow_mut().push(format!("restart {role}"));
        Installed { ok: true, how: "fake".into() }
    }
    fn started(&self, role: Role, _version: Option<&str>, _before: Option<u32>) -> bool {
        self.waited.borrow_mut().push(role);
        self.order.borrow_mut().push(format!("started {role}"));
        self.started_ok.get()
    }
    fn old_roles(&self) -> Vec<Role> {
        self.old.clone()
    }
    fn move_old(&self, role: Role) -> Installed {
        self.moved.borrow_mut().push(role);
        self.order.borrow_mut().push(format!("move {role}"));
        Installed { ok: true, how: "a user service".into() }
    }
    fn log(&self, _line: &str) {}
    fn platform(&self) -> Platform {
        self.platform
    }
    fn arch(&self) -> Arch {
        self.arch
    }
    fn download_base(&self) -> String {
        self.base.clone()
    }
    fn signing_key(&self) -> String {
        test_pub()
    }
    fn libc(&self) -> Libc {
        self.libc
    }
}

struct Fixture {
    home: Home,
    server: Server,
    cli_name: String,
    engine_name: String,
}

impl Fixture {
    fn new() -> Fixture {
        let home = Home::new(&[]);
        let server = Server::start();
        // the fake host says glibc whatever this machine runs, so the release carries those names
        let names = release_names(here(), Arch::current(), Libc::Glibc).unwrap();
        fs::create_dir_all(home.dir.join(".inyon/bin")).unwrap();
        fs::write(home.dir.join(".inyon/bin/inyon"), "old build").unwrap();
        let (cli, engine) = (names.cli.clone(), names.engine.clone());
        server.release(|r| {
            r.files.insert(cli.clone(), "cli build".into());
            r.files.insert(engine.clone(), "engine build".into());
            r.files.insert("inyon-other".into(), "other".into());
        });
        Fixture { home, server, cli_name: names.cli, engine_name: names.engine }
    }
    fn installed(&self) -> PathBuf {
        self.home.dir.join(".inyon/bin/inyon")
    }
    fn engine(&self) -> PathBuf {
        self.home.dir.join(".inyon/bin/inyon-engine")
    }
    fn bin_names(&self) -> Vec<String> {
        let mut v: Vec<String> = fs::read_dir(self.home.dir.join(".inyon/bin")).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        v.sort();
        v
    }
    fn run(&self, host: &FakeHost) -> Result<update::Outcome, Error> {
        update::update(&find_install(&self.installed(), None), host, &RealFiles)
    }
    fn read(&self, p: &Path) -> String {
        fs::read_to_string(p).unwrap_or_default()
    }
}

#[test]
fn the_newest_version_comes_from_latest_json_where_github_redirects_or_from_a_pinned_tag() {
    let _g = lock();
    let f = Fixture::new();
    f.server.release(|r| r.version = "0.7.1".into());
    // the website (and any mirror) lists it in latest.json
    assert_eq!(latest_version(&format!("{}{SITE}", f.server.base)).unwrap(), "0.7.1");
    assert_eq!(latest_version(&format!("{}{SITE}/", f.server.base)).unwrap(), "0.7.1");
    assert_eq!(f.server.requests(), vec!["/download/latest.json", "/download/latest.json"]);
    // GitHub's newest says so in its redirect, and a pinned tag without a request
    f.server.clear_requests();
    assert_eq!(latest_version(&format!("{}{GITHUB}", f.server.base)).unwrap(), "0.7.1");
    assert_eq!(f.server.requests(), vec!["/releases/latest"]);
    assert_eq!(latest_version("https://example.invalid/x/releases/download/v0.6.9").unwrap(), "0.6.9");
    // somewhere without latest.json
    assert!(matches!(latest_version(&format!("{}/nothing-here/download", f.server.base)), Err(Error::Update(_))));
    // a "v" in front is fine, as in the tag
    f.server.release(|r| r.listing = Some(r#"{"version":"v0.7.2","files":[]}"#.into()));
    assert_eq!(latest_version(&format!("{}{SITE}", f.server.base)).unwrap(), "0.7.2");
}

#[test]
fn a_missing_or_unreadable_latest_json_is_an_error_that_names_the_host_and_changes_nothing() {
    let _g = lock();
    let f = Fixture::new();
    let host = f.server.base.trim_start_matches("http://").to_string();
    let site = format!("{}{SITE}", f.server.base);
    f.server.release(|r| r.listed = false);
    let e = latest_version(&site).unwrap_err().to_string();
    assert_eq!(e, format!("Couldn't tell the newest version: {host} answered 404 for {site}/latest.json."));
    for bad in ["not json", "{}", r#"{"tag":"v1.0.0"}"#, r#"{"version":""}"#, r#"{"version":10}"#, r#"{"version":"1.0.0 <b>"}"#, r#"{"version":"latest"}"#] {
        f.server.release(|r| {
            r.listed = true;
            r.listing = Some(bad.into());
        });
        let e = latest_version(&site).unwrap_err().to_string();
        assert_eq!(e, format!("Couldn't tell the newest version: {site}/latest.json doesn't name one."), "{bad}");
    }
    // nothing answers at all
    let e = latest_version("http://127.0.0.1:1/download").unwrap_err().to_string();
    assert!(e.starts_with("Couldn't reach 127.0.0.1:1 to look for a new version ("), "{e}");
    // and an update stops there, before anything is downloaded
    f.server.clear_requests();
    let host = FakeHost::new(&f.server, &[Role::Link]);
    assert!(f.run(&host).unwrap_err().to_string().contains("doesn't name one"));
    assert_eq!(f.server.requests(), vec!["/download/latest.json"]);
    assert_eq!(f.read(&f.installed()), "old build");
    assert!(host.restarts.borrow().is_empty());
}

#[test]
fn a_signed_list_that_is_not_the_release_latest_json_names_is_refused() {
    let _g = lock();
    let f = Fixture::new();
    let host = FakeHost::new(&f.server, &[Role::Link]);
    let refused = |err: String, version: &str| {
        assert_eq!(
            err,
            format!("The release's signed checksum list isn't the one for Inyon {version} (it doesn't list inyon-{version}.apk), so nothing was changed. Try again later.")
        );
        assert_eq!(f.read(&f.installed()), "old build");
        assert_eq!(f.bin_names(), vec!["inyon"]);
        assert!(f.server.downloads().is_empty());
        assert!(host.restarts.borrow().is_empty());
    };
    // latest.json names a newer version than the release whose signed list is served (9.9.9's)
    f.server.release(|r| r.listing = Some(r#"{"version":"9.9.10","files":[]}"#.into()));
    refused(f.run(&host).unwrap_err().to_string(), "9.9.10");
    // the right version, but a signed list that names no APK
    f.server.release(|r| {
        r.listing = None;
        r.apk = false;
    });
    refused(f.run(&host).unwrap_err().to_string(), "9.9.9");
    // the list the release signed, with its APK: the update goes through
    f.server.release(|r| r.apk = true);
    assert!(f.run(&host).unwrap().changed);
}

#[test]
fn a_release_from_github_still_updates_through_its_redirect() {
    let _g = lock();
    let f = Fixture::new();
    let mut host = FakeHost::new(&f.server, &[Role::Link]);
    host.base = format!("{}{GITHUB}", f.server.base);
    // GitHub has no latest.json, and it isn't asked for
    f.server.release(|r| r.listed = false);
    assert!(f.run(&host).unwrap().changed);
    assert_eq!(f.read(&f.installed()), "cli build");
    assert!(f.server.requests().iter().all(|r| !r.starts_with(SITE)), "{:?}", f.server.requests());
    assert_eq!(f.server.downloads(), vec![format!("{}.gz", f.cli_name)]);
}

#[test]
fn a_release_installs_the_new_inyon_alone_and_restarts_each_job_on_it() {
    let _g = lock();
    let f = Fixture::new();
    let host = FakeHost::new(&f.server, &[Role::Link, Role::Hub]);
    let r = f.run(&host).unwrap();
    assert!(r.changed);
    assert_eq!(r.version.as_deref(), Some("9.9.9"));
    assert_eq!(f.read(&f.installed()), "cli build");
    let cmd = vec![f.installed().to_string_lossy().into_owned()];
    assert_eq!(*host.restarts.borrow(), vec![(Role::Link, cmd.clone()), (Role::Hub, cmd)]);
    assert_eq!(*host.waited.borrow(), vec![Role::Link, Role::Hub]);
    // the old copy is gone once the new one runs, and no download is left behind; the release's
    // inyon-engine files are there for older versions, and a hub runs the one built in now
    assert_eq!(f.bin_names(), vec!["inyon"]);
    // the gzipped copy, since the release lists it
    assert_eq!(f.server.downloads(), vec![format!("{}.gz", f.cli_name)]);
}

#[test]
fn a_job_still_under_tether_nodes_name_moves_to_inyons_service_once_the_new_version_runs() {
    let _g = lock();
    let f = Fixture::new();
    let mut host = FakeHost::new(&f.server, &[Role::Link]);
    host.old = vec![Role::Link];
    let r = f.run(&host).unwrap();
    assert!(r.changed);
    assert_eq!(r.message, "inyon 9.9.9 is installed and running. The link moved from its tether-node service to inyon's.");
    // restarted under the old name first, checked, and only then moved: a move that fails would
    // otherwise leave nothing to go back to
    assert_eq!(*host.order.borrow(), vec!["restart link", "started link", "move link"]);
    assert_eq!(*host.moved.borrow(), vec![Role::Link]);
    // nothing moves when the new version didn't start: the old one is back
    host.started_ok.set(false);
    host.moved.borrow_mut().clear();
    f.server.release(|r| r.version = "9.9.10".into());
    assert!(f.run(&host).is_err());
    assert!(host.moved.borrow().is_empty());
}

#[test]
fn the_status_file_says_running_done_current_or_failed_as_the_hub_reads_it() {
    let _g = lock();
    let home = Home::new(&[]);
    let status = home.dir.join(".inyon/node/update-status");
    assert_eq!(update::status_file(), status);
    for line in ["running", "done 9.9.9", "current 9.9.8", "failed: The download didn't match the release's checksum, so nothing was changed. Try again."] {
        update::set_status(line);
        assert_eq!(fs::read_to_string(&status).unwrap(), format!("{line}\n"));
    }
    // update.log keeps every line, each after when it was written
    let log = fs::read_to_string(home.dir.join(".inyon/node/update.log")).unwrap();
    let lines: Vec<&str> = log.lines().collect();
    assert_eq!(lines.len(), 4);
    assert!(lines[0].ends_with(" running") && lines[1].ends_with(" done 9.9.9") && lines[2].ends_with(" current 9.9.8") && lines[3].ends_with("Try again."), "{log}");
}

#[test]
fn a_release_without_gzipped_files_is_downloaded_plain() {
    let _g = lock();
    let f = Fixture::new();
    f.server.release(|r| r.gz = false);
    let host = FakeHost::new(&f.server, &[Role::Link]);
    assert!(f.run(&host).unwrap().changed);
    assert_eq!(f.read(&f.installed()), "cli build");
    assert_eq!(f.server.downloads(), vec![f.cli_name.clone()]);
}

#[test]
fn a_download_that_does_not_match_sha256sums_changes_nothing() {
    let _g = lock();
    let f = Fixture::new();
    let (cli, engine) = (f.cli_name.clone(), f.engine_name.clone());
    f.server.release(|r| {
        r.gz = false;
        r.sums = format!("{}  {engine}\n{}  {cli}\n", sha(b"engine build"), sha(b"something else"));
    });
    let host = FakeHost::new(&f.server, &[Role::Link]);
    let err = f.run(&host).unwrap_err().to_string();
    assert!(err.contains("didn't match the release's checksum"), "{err}");
    assert_eq!(f.read(&f.installed()), "old build");
    assert_eq!(f.bin_names(), vec!["inyon"]);
    assert!(host.restarts.borrow().is_empty());
}

#[test]
fn the_gzipped_file_is_checked_before_it_is_unpacked_and_the_unpacked_file_after() {
    let _g = lock();
    let f = Fixture::new();
    let cli = f.cli_name.clone();
    let plain = sha(b"cli build");
    let gz = sha(&gzip(b"cli build"));
    // a .gz that isn't the one listed
    let s = format!("{plain}  {cli}\n{}  {cli}.gz\n", sha(b"tampered"));
    f.server.release(|r| r.sums = s);
    let host = FakeHost::new(&f.server, &[Role::Link]);
    assert!(f.run(&host).unwrap_err().to_string().contains("didn't match the release's checksum"));
    assert_eq!(f.read(&f.installed()), "old build");
    // the right .gz, but it unpacks to something other than the plain file listed
    let s = format!("{}  {cli}\n{gz}  {cli}.gz\n", sha(b"tampered"));
    f.server.release(|r| r.sums = s);
    assert!(f.run(&host).unwrap_err().to_string().contains("didn't match the release's checksum"));
    assert_eq!(f.read(&f.installed()), "old build");
    assert_eq!(f.server.downloads(), vec![format!("{cli}.gz"), format!("{cli}.gz")]);
    assert_eq!(f.bin_names(), vec!["inyon"]);
}

#[test]
fn a_file_missing_from_sha256sums_is_refused_too() {
    let _g = lock();
    let f = Fixture::new();
    let engine = f.engine_name.clone();
    f.server.release(|r| r.sums = format!("{}  {engine}\n{}  inyon-some-other-build\n", sha(b"engine build"), sha(b"cli build")));
    let host = FakeHost::new(&f.server, &[Role::Link]);
    let err = f.run(&host).unwrap_err().to_string();
    assert!(err.contains("doesn't list"), "{err}");
    assert_eq!(f.read(&f.installed()), "old build");
    assert_eq!(f.bin_names(), vec!["inyon"]);
}

#[test]
fn a_checksum_list_that_is_unsigned_signed_by_another_key_or_changed_after_signing_is_refused() {
    let _g = lock();
    let f = Fixture::new();
    let host = FakeHost::new(&f.server, &[Role::Link]);
    let refused = |err: String| {
        assert!(err.starts_with("The release's checksum list isn't signed by Inyon's key, so nothing was changed."), "{err}");
        assert!(err.ends_with(&format!("\n  {}", reinstall_command(here()))), "{err}");
        assert_eq!(f.read(&f.installed()), "old build");
        assert_eq!(f.bin_names(), vec!["inyon"]);
        assert!(f.server.downloads().is_empty());
        assert!(host.restarts.borrow().is_empty());
    };
    // no SHA256SUMS.minisig at all: a release from before 0.14, or a copy made elsewhere
    f.server.release(|r| r.signer = None);
    refused(f.run(&host).unwrap_err().to_string());
    // signed, but not with Inyon's key
    f.server.release(|r| r.signer = Some(other_key()));
    refused(f.run(&host).unwrap_err().to_string());
    // Inyon's signature over the list as it was, and a list changed since
    let (cli, engine) = (f.cli_name.clone(), f.engine_name.clone());
    f.server.release(|r| {
        r.signer = Some(test_key());
        let as_signed = r.sums();
        r.signed_text = Some(as_signed);
        r.sums = format!("{}  {engine}\n{}  {cli}\n", sha(b"engine build"), sha(b"tampered"));
    });
    refused(f.run(&host).unwrap_err().to_string());
    // the list as signed: the update goes through
    f.server.release(|r| {
        r.signed_text = None;
        r.sums.clear();
    });
    assert!(f.run(&host).unwrap().changed);
    assert_eq!(f.read(&f.installed()), "cli build");
}

#[test]
fn the_signing_key_from_the_environment_only_counts_for_releases_from_elsewhere() {
    let _g = lock();
    let _home = Home::new(&[]);
    let mirrors = ["http://127.0.0.1:1/download", "http://127.0.0.1:1/releases/latest/download"];
    assert_eq!(signing::signing_key(update::RELEASES), signing::RELEASE_KEY);
    assert_eq!(signing::signing_key(mirrors[0]), signing::RELEASE_KEY);
    // the whole .pub file or its base64 line alone
    std::env::set_var("INYON_SIGNING_KEY", include_str!("keys/test.pub"));
    for m in mirrors {
        assert_eq!(signing::signing_key(m), test_pub(), "{m}");
    }
    std::env::set_var("INYON_SIGNING_KEY", test_pub());
    assert_eq!(signing::signing_key(mirrors[0]), test_pub());
    // the name from before the rename counts too
    std::env::remove_var("INYON_SIGNING_KEY");
    std::env::set_var("TETHER_SIGNING_KEY", test_pub());
    assert_eq!(signing::signing_key(mirrors[0]), test_pub());
    std::env::remove_var("TETHER_SIGNING_KEY");
}

/// With INYON_SIGNING_KEY (or TETHER_SIGNING_KEY) naming the test key, `base` still takes the
/// release key: a variable alone can't point an install at another key.
fn ignores_the_key_from_the_environment(base: &str) {
    let _g = lock();
    let _home = Home::new(&[]);
    for var in ["INYON_SIGNING_KEY", "TETHER_SIGNING_KEY"] {
        std::env::set_var(var, test_pub());
        assert_eq!(signing::signing_key(base), signing::RELEASE_KEY, "{var} {base}");
        assert_eq!(signing::signing_key(&format!("{base}/")), signing::RELEASE_KEY, "{var} {base}/");
        std::env::remove_var(var);
    }
}

#[test]
fn the_websites_copy_ignores_a_signing_key_from_the_environment() {
    assert_eq!(update::RELEASES, "https://inyon.tech/download");
    ignores_the_key_from_the_environment(update::RELEASES);
}

#[test]
fn githubs_newest_release_ignores_a_signing_key_from_the_environment() {
    ignores_the_key_from_the_environment(&format!("https://github.com/{}/releases/latest/download", update::RELEASES_REPO));
}

#[test]
fn a_release_pinned_on_github_ignores_a_signing_key_from_the_environment() {
    ignores_the_key_from_the_environment(&format!("https://github.com/{}/releases/download/v0.13.3", update::RELEASES_REPO));
}

#[test]
fn alpine_takes_the_same_command_and_no_engine() {
    let _g = lock();
    let f = Fixture::new();
    f.server.release(|r| {
        r.files.insert("inyon-linux-x64".into(), "cli build".into());
        r.files.insert("inyon-engine-linux-x64-musl".into(), "musl engine".into());
    });
    let mut alpine = FakeHost::new(&f.server, &[Role::Link]);
    alpine.platform = Platform::Linux;
    alpine.arch = Arch::X64;
    alpine.libc = Libc::Musl;
    assert!(f.run(&alpine).unwrap().changed);
    assert!(!f.engine().exists());
    assert_eq!(f.read(&f.installed()), "cli build");
    assert_eq!(f.server.downloads(), vec!["inyon-linux-x64.gz"]);
}

#[test]
fn a_release_whose_inyon_is_bad_changes_nothing_and_says_how_to_install_instead() {
    let _g = lock();
    let f = Fixture::new();
    let cli_gz = format!("{}.gz", f.cli_name);
    f.server.release(|r| r.missing = vec![cli_gz]);
    let host = FakeHost::new(&f.server, &[Role::Link]);
    let err = f.run(&host).unwrap_err().to_string();
    // the line for this machine, whichever system the tests run on
    assert!(err.ends_with(&format!("\n  {}", reinstall_command(here()))), "{err}");
    assert!(err.contains("Couldn't download"), "{err}");
    assert_eq!(f.read(&f.installed()), "old build");
    assert_eq!(f.bin_names(), vec!["inyon"]);
    assert!(host.restarts.borrow().is_empty());

    // on Windows the line is the PowerShell one
    f.server.release(|r| {
        r.sums.clear();
        r.missing = vec!["inyon-windows-x64.exe.gz".into()];
        r.files = [("inyon-windows-x64.exe", "cli"), ("inyon-engine-windows-x64.exe", "engine")].into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    });
    let mut win = FakeHost::new(&f.server, &[Role::Link]);
    win.platform = Platform::Windows;
    win.arch = Arch::X64;
    let err = f.run(&win).unwrap_err().to_string();
    assert!(err.ends_with("\n  irm https://inyon.tech/get.ps1 | iex"), "{err}");
    assert_eq!(reinstall_command(Platform::Darwin), "curl -fsSL https://inyon.tech/get.sh | sh");
}

#[test]
fn a_release_that_does_not_start_goes_back_to_the_old_inyon_alone_and_says_how_to_install() {
    let _g = lock();
    let f = Fixture::new();
    let host = FakeHost::new(&f.server, &[Role::Link]);
    host.started_ok.set(false);
    let err = f.run(&host).unwrap_err().to_string();
    assert!(err.contains("the previous version is running again"), "{err}");
    assert!(err.ends_with(&reinstall_command(here())), "{err}");
    assert_eq!(f.read(&f.installed()), "old build");
    assert_eq!(host.restarted_roles(), vec![Role::Link, Role::Link]);
    // the engine went too, since nothing of the old version starts it
    assert!(!f.engine().exists());
}

#[test]
fn already_on_the_newest_version_says_so_and_downloads_nothing() {
    let _g = lock();
    let f = Fixture::new();
    f.server.release(|r| r.version = "0.6.3".into());
    let host = FakeHost::new(&f.server, &[Role::Link]);
    let r = f.run(&host).unwrap();
    assert_eq!(r, update::Outcome { changed: false, message: "inyon 0.6.3 is up to date.".into(), version: Some("0.6.3".into()) });
    assert_eq!(f.server.requests(), vec!["/download/latest.json"]);
    assert!(host.restarts.borrow().is_empty());
}

#[test]
fn a_new_version_that_does_not_start_is_swapped_back_out_and_every_job_restarted_on_the_old_one() {
    let _g = lock();
    let f = Fixture::new();
    fs::write(f.engine(), "old engine").unwrap();
    let host = FakeHost::new(&f.server, &[Role::Link, Role::Node]);
    host.started_ok.set(false);
    let err = f.run(&host).unwrap_err().to_string();
    assert!(err.contains("the previous version is running again"), "{err}");
    assert_eq!(f.read(&f.installed()), "old build");
    assert_eq!(f.read(&f.engine()), "old engine");
    assert_eq!(host.restarted_roles(), vec![Role::Link, Role::Node, Role::Link, Role::Node]);
}

#[test]
fn nothing_running_in_the_background_the_new_file_is_in_place_for_the_next_start() {
    let _g = lock();
    let f = Fixture::new();
    let host = FakeHost::new(&f.server, &[]);
    let r = f.run(&host).unwrap();
    assert!(r.message.contains("isn't running as a background service"), "{}", r.message);
    assert_eq!(f.read(&f.installed()), "cli build");
    assert!(!f.engine().exists());
}

#[test]
fn a_machine_added_to_a_hub_takes_the_hub_build_when_it_differs_checked_against_the_signed_list() {
    let _g = lock();
    let f = Fixture::new();
    let install = find_install(&f.installed(), Some(f.server.base.clone()));
    assert_eq!(install.kind, Kind::Hub(f.server.base.clone()));
    let mut linux = FakeHost::new(&f.server, &[Role::Node]);
    linux.platform = Platform::Linux;
    linux.arch = Arch::X64;
    // a hub from 0.19 on hands out an inyon with the engine built in, and nothing else is fetched
    let marked = |b: &str| format!("{b} {}", inyon_cli::engine::COPY_MARK);
    f.server.hub.lock().unwrap().commands.insert("linux-x64".into(), marked("new linux cli"));
    assert!(update::update(&install, &linux, &RealFiles).unwrap().changed);
    assert_eq!(f.read(&f.installed()), marked("new linux cli"));
    assert_eq!(*linux.restarts.borrow(), vec![(Role::Node, vec![f.installed().to_string_lossy().into_owned()])]);
    let hub_downloads: Vec<String> = f.server.requests().into_iter().filter(|r| r.ends_with(".gz?os=linux&arch=x64")).collect();
    assert_eq!(hub_downloads, vec!["/node/inyon.gz?os=linux&arch=x64"]);
    assert_eq!(f.bin_names(), vec!["inyon"]);

    // the same files as the hub's: nothing to do
    linux.restarts.borrow_mut().clear();
    assert!(!update::update(&install, &linux, &RealFiles).unwrap().changed);
    assert!(linux.restarts.borrow().is_empty());

    // a build swapped on the way, with the hub's own checksum swapped to match: the signed list
    // still says what the release has, and the running copy is left alone
    {
        let mut hub = f.server.hub.lock().unwrap();
        hub.commands.insert("linux-x64".into(), marked("newer linux cli"));
        hub.signed_list = Some(hub.release_sums());
        hub.commands.insert("linux-x64".into(), marked("tampered"));
    }
    let err = update::update(&install, &linux, &RealFiles).unwrap_err().to_string();
    assert!(err.contains("didn't match its checksum"), "{err}");
    assert_eq!(f.read(&f.installed()), marked("new linux cli"));
    assert_eq!(f.bin_names(), vec!["inyon"]);
}

#[test]
fn a_hub_with_no_signed_list_or_one_signed_by_another_key_changes_nothing_and_says_what_to_do() {
    let _g = lock();
    let f = Fixture::new();
    let install = find_install(&f.installed(), Some(f.server.base.clone()));
    let mut linux = FakeHost::new(&f.server, &[Role::Node]);
    linux.platform = Platform::Linux;
    linux.arch = Arch::X64;
    // a hub from before 0.20, or one running from a checkout: only its own checksums
    f.server.hub.lock().unwrap().signed = false;
    let err = update::update(&install, &linux, &RealFiles).unwrap_err().to_string();
    assert!(err.contains("doesn't hand out Inyon's signed list") && err.contains("Update the hub first") && err.contains("INYON_ALLOW_INSECURE=1"), "{err}");
    assert_eq!(f.read(&f.installed()), "old build");
    assert!(linux.restarts.borrow().is_empty());
    assert!(!f.server.requests().iter().any(|r| r.contains(".gz")), "{:?}", f.server.requests());
    // you can take them anyway
    std::env::set_var("INYON_ALLOW_INSECURE", "1");
    let r = update::update(&install, &linux, &RealFiles);
    std::env::remove_var("INYON_ALLOW_INSECURE");
    assert!(r.unwrap().changed);
    assert_eq!(f.read(&f.installed()), "new linux cli");

    // a list that's there but signed by someone else is refused, whatever INYON_ALLOW_INSECURE says
    fs::write(f.installed(), "old build").unwrap();
    {
        let mut hub = f.server.hub.lock().unwrap();
        hub.signed = true;
        hub.signer = other_key();
    }
    std::env::set_var("INYON_ALLOW_INSECURE", "1");
    let err = update::update(&install, &linux, &RealFiles).unwrap_err().to_string();
    std::env::remove_var("INYON_ALLOW_INSECURE");
    assert!(err.contains("isn't signed by Inyon's key"), "{err}");
    assert_eq!(f.read(&f.installed()), "old build");
}

#[test]
fn a_hub_list_for_another_version_or_one_the_hub_could_not_get_changes_nothing_and_says_why() {
    let _g = lock();
    let f = Fixture::new();
    let install = find_install(&f.installed(), Some(f.server.base.clone()));
    let mut linux = FakeHost::new(&f.server, &[Role::Node]);
    linux.platform = Platform::Linux;
    linux.arch = Arch::X64;
    // an older release's list, signed and all, passed off as the one for the version the hub runs
    {
        let mut hub = f.server.hub.lock().unwrap();
        hub.version = "0.19.0".into();
        hub.signed_list = Some(hub.release_sums());
        hub.version = "0.20.0".into();
    }
    let err = update::update(&install, &linux, &RealFiles).unwrap_err().to_string();
    assert!(err.contains("isn't the one for Inyon 0.20.0") && err.contains("inyon-0.20.0.apk"), "{err}");
    assert_eq!(f.read(&f.installed()), "old build");
    assert!(!f.server.requests().iter().any(|r| r.contains(".gz")), "{:?}", f.server.requests());

    // a hub from 0.20 that couldn't get the list is no older hub: INYON_ALLOW_INSECURE doesn't
    // make it take the hub's own checksums
    {
        let mut hub = f.server.hub.lock().unwrap();
        hub.signed_list = None;
        hub.list_status = Some("502 Bad Gateway");
    }
    std::env::set_var("INYON_ALLOW_INSECURE", "1");
    let err = update::update(&install, &linux, &RealFiles).unwrap_err().to_string();
    std::env::remove_var("INYON_ALLOW_INSECURE");
    assert!(err.contains("couldn't hand out Inyon's signed list") && err.contains("502: Could not get the checksums"), "{err}");
    assert!(!err.contains("before 0.20"), "{err}");
    assert_eq!(f.read(&f.installed()), "old build");
}

#[test]
fn a_hub_from_before_the_split_hands_out_one_program_so_nothing_is_changed_and_it_says_so() {
    let _g = lock();
    let f = Fixture::new();
    // and so from before the signed list too
    {
        let mut hub = f.server.hub.lock().unwrap();
        hub.split = false;
        hub.signed = false;
    }
    let install = find_install(&f.installed(), Some(f.server.base.clone()));
    let mut linux = FakeHost::new(&f.server, &[Role::Node]);
    linux.platform = Platform::Linux;
    linux.arch = Arch::X64;
    let err = update::update(&install, &linux, &RealFiles).unwrap_err().to_string();
    assert!(err.contains("Update the hub first"), "{err}");
    assert_eq!(f.read(&f.installed()), "old build");
    assert!(linux.restarts.borrow().is_empty());
    assert_eq!(f.bin_names(), vec!["inyon"]);
}

#[test]
fn a_windows_machine_asks_its_hub_for_its_own_command_and_the_x64_engine() {
    let _g = lock();
    let f = Fixture::new();
    let install = find_install(&f.installed(), Some(f.server.base.clone()));
    let mut win = FakeHost::new(&f.server, &[Role::Node]);
    win.platform = Platform::Windows;
    win.arch = Arch::Arm64;
    assert!(update::update(&install, &win, &RealFiles).unwrap().changed);
    assert_eq!(f.read(&f.engine()), "new exe");
    assert_eq!(f.read(&f.installed()), "new arm cli exe");
    assert!(f.server.requests().contains(&"/node/inyon-engine.gz?os=windows&arch=arm64".to_string()));
    assert!(f.server.requests().contains(&"/node/inyon.gz?os=windows&arch=arm64".to_string()));
}

#[test]
fn which_install_is_running() {
    let _g = lock();
    let f = Fixture::new();
    assert_eq!(find_install(&f.installed(), None).kind, Kind::Binary);
    assert_eq!(find_install(&f.installed(), Some("http://hub".into())).kind, Kind::Hub("http://hub".into()));
    // tether-node's old place, before it has moved
    assert_eq!(find_install(&f.home.dir.join(".tether-node/tether-node.exe"), Some("http://hub".into())).kind, Kind::Hub("http://hub".into()));
}

/// Acts like Windows: a running program can be renamed, but not deleted or overwritten.
struct WindowsFiles {
    locked: RefCell<HashSet<PathBuf>>,
    block_from: Option<PathBuf>,
}

impl Files for WindowsFiles {
    fn rename(&self, from: &Path, to: &Path) -> std::io::Result<()> {
        if self.block_from.as_deref() == Some(from) {
            return Err(std::io::Error::other("EPERM: blocked by antivirus"));
        }
        if self.locked.borrow().contains(to) {
            return Err(std::io::Error::other(format!("EPERM: {}", to.display())));
        }
        fs::rename(from, to)?;
        // the lock follows the running file to its new name
        let mut l = self.locked.borrow_mut();
        if l.remove(from) {
            l.insert(to.to_path_buf());
        }
        Ok(())
    }
    fn remove(&self, f: &Path) -> std::io::Result<()> {
        if self.locked.borrow().contains(f) {
            return Err(std::io::Error::other(format!("EBUSY: {}", f.display())));
        }
        RealFiles.remove(f)
    }
    fn exists(&self, f: &Path) -> bool {
        f.exists()
    }
    fn list(&self, dir: &Path) -> std::io::Result<Vec<String>> {
        RealFiles.list(dir)
    }
}

#[test]
fn windows_the_running_exe_is_renamed_aside_the_new_one_takes_its_name_the_old_one_goes_later() {
    let _g = lock();
    let f = Fixture::new();
    let exe = f.installed();
    let fresh = PathBuf::from(format!("{}.download", exe.display()));
    fs::write(&fresh, "new build").unwrap();
    let files = WindowsFiles { locked: RefCell::new([exe.clone()].into_iter().collect()), block_from: None };
    let t0: u64 = 1_000_000_000_000;
    let old = swap_in(&files, &exe, &fresh, t0).unwrap().unwrap();
    assert_eq!(old, PathBuf::from(format!("{}.old-{t0}", exe.display())));
    assert_eq!(f.read(&exe), "new build");
    assert!(files.locked.borrow().contains(&old));

    // still running: cleaning up leaves it, quietly
    cleanup_old(&files, &exe, t0 + 11 * 60_000);
    assert!(old.exists());
    // stopped, but from an update that may still be going on: kept for going back to
    files.locked.borrow_mut().clear();
    cleanup_old(&files, &exe, t0 + 5 * 60_000);
    assert!(old.exists());
    cleanup_old(&files, &exe, t0 + 11 * 60_000);
    assert!(!old.exists());
    assert!(exe.exists());
}

#[test]
fn windows_if_the_new_file_cannot_take_the_name_the_old_one_gets_it_back() {
    let _g = lock();
    let f = Fixture::new();
    let exe = f.installed();
    let fresh = PathBuf::from(format!("{}.download", exe.display()));
    fs::write(&fresh, "new build").unwrap();
    let files = WindowsFiles { locked: RefCell::new(HashSet::new()), block_from: Some(fresh.clone()) };
    let err = swap_in(&files, &exe, &fresh, 5).unwrap_err();
    assert!(err.to_string().contains("EPERM"));
    assert_eq!(f.read(&exe), "old build");
}

#[test]
fn windows_going_back_renames_the_new_copy_aside_too_in_case_it_is_still_running() {
    let _g = lock();
    let f = Fixture::new();
    let exe = f.installed();
    let fresh = PathBuf::from(format!("{}.download", exe.display()));
    fs::write(&fresh, "new build").unwrap();
    let old = swap_in(&RealFiles, &exe, &fresh, 1).unwrap().unwrap();
    let files = WindowsFiles { locked: RefCell::new([exe.clone()].into_iter().collect()), block_from: None };
    swap_back(&files, &exe, &old, 2).unwrap();
    assert_eq!(f.read(&exe), "old build");
    assert_eq!(f.read(&PathBuf::from(format!("{}.old-2", exe.display()))), "new build");
}

#[test]
fn the_restarted_copy_counts_once_it_names_the_new_version_is_a_new_process_and_stays_up() {
    let _g = lock();
    let f = Fixture::new();
    let state = f.home.dir.join(".inyon/node/state.json");
    fs::create_dir_all(state.parent().unwrap()).unwrap();
    let fast = WaitOptions { tries: 3, every_ms: 1, settle_ms: 1 };
    let me = std::process::id();
    fs::write(&state, format!(r#"{{"pid":{me},"version":"9.9.9"}}"#)).unwrap();
    assert!(wait_started(&RealProc, Role::Link, Some("9.9.9"), None, &fast));
    // still the copy from before the restart
    assert!(!wait_started(&RealProc, Role::Link, Some("9.9.9"), Some(me), &fast));
    assert!(!wait_started(&RealProc, Role::Link, Some("9.9.10"), None, &fast));
    fs::write(&state, format!(r#"{{"pid":{},"version":"9.9.9"}}"#, (1u32 << 22) + 12345)).unwrap();
    assert!(!wait_started(&RealProc, Role::Link, Some("9.9.9"), None, &fast));
}

/// A Windows program's first bytes, as text the mock release can carry: the DOS header pointing
/// at a PE header, its subsystem a console program's.
fn fake_exe_text() -> String {
    let mut exe = vec![b' '; 512];
    exe[..2].copy_from_slice(b"MZ");
    // the PE header right after the DOS one, at 0x40, so every byte is plain ASCII
    exe[0x3c..0x40].copy_from_slice(&0x40u32.to_le_bytes());
    exe[0x40..0x44].copy_from_slice(b"PE\0\0");
    let at = 0x40 + 4 + 20 + 68;
    exe[at..at + 2].copy_from_slice(&3u16.to_le_bytes());
    String::from_utf8(exe).unwrap()
}

/// inyon 0.19 or later for Windows: the engine is built in, and it carries the mark that says so.
fn fake_inyon_text() -> String {
    format!("{}{}", fake_exe_text(), inyon_cli::engine::COPY_MARK)
}

// Linux and macOS; Windows swaps in the windowless copy instead (below)
#[cfg(not(windows))]
#[test]
fn from_0_19_a_machine_without_a_hub_takes_inyon_alone_and_drops_the_bun_engine() {
    let _g = lock();
    let f = Fixture::new();
    // what an update by 0.18 left: the Bun engine next to inyon
    fs::write(f.engine(), "old engine").unwrap();
    let cli = f.cli_name.clone();
    f.server.release(move |r| {
        r.files.insert(cli, format!("cli build {}", inyon_cli::engine::COPY_MARK));
    });
    let host = FakeHost::new(&f.server, &[Role::Link, Role::Node]);
    let r = f.run(&host).unwrap();
    assert!(r.changed, "{}", r.message);
    assert!(f.read(&f.installed()).starts_with("cli build"));
    assert_eq!(f.server.downloads(), vec![format!("{}.gz", f.cli_name)]);
    assert!(!f.engine().exists(), "the engine nothing needs went");
    assert_eq!(f.bin_names(), vec!["inyon"]);
    assert_eq!(host.restarted_roles(), vec![Role::Link, Role::Node]);
}

// Linux and macOS; Windows swaps in the windowless copy instead (below)
#[cfg(not(windows))]
#[test]
fn a_release_without_the_engine_files_still_updates_a_hub() {
    let _g = lock();
    let f = Fixture::new();
    let engine = f.engine_name.clone();
    f.server.release(|r| {
        r.files.remove(&engine);
    });
    // the hub runs the one built into inyon from 0.23, so the Bun engine is never asked for
    let hub = FakeHost::new(&f.server, &[Role::Link, Role::Hub]);
    assert!(f.run(&hub).unwrap().changed);
    assert_eq!(f.read(&f.installed()), "cli build");
    assert!(!f.server.requests().iter().any(|r| r.contains(&engine)), "{:?}", f.server.requests());
}

#[test]
fn from_0_19_windows_gets_the_windowless_copy_of_the_new_inyon_in_place_of_the_bun_engine() {
    let _g = lock();
    let f = Fixture::new();
    fs::write(f.engine(), "the Bun engine").unwrap();
    let program = fake_inyon_text();
    let p = program.clone();
    f.server.release(move |r| {
        r.files = [("inyon-windows-x64.exe", p.as_str()), ("inyon-engine-windows-x64.exe", "engine")].into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    });
    let mut win = FakeHost::new(&f.server, &[Role::Link]);
    win.platform = Platform::Windows;
    win.arch = Arch::X64;
    let r = f.run(&win).unwrap();
    assert!(r.changed, "{}", r.message);
    assert_eq!(f.server.downloads(), vec!["inyon-windows-x64.exe.gz".to_string()]);
    assert_eq!(fs::read(f.installed()).unwrap(), program.as_bytes());
    assert_eq!(fs::read(f.engine()).unwrap(), inyon_cli::engine::make_copy(program.as_bytes()).unwrap());
    // a release without the app of its own: the app is the same windowless copy, under its name
    assert_eq!(fs::read(inyon_cli::app::app_beside(&f.installed())).unwrap(), inyon_cli::engine::make_copy(program.as_bytes()).unwrap());
    assert_eq!(f.bin_names(), vec!["inyon", "inyon-app", "inyon-engine"]);
    // and if the new version won't start, the Bun engine comes back with the old inyon
    let f = Fixture::new();
    fs::write(f.engine(), "the Bun engine").unwrap();
    let p = program.clone();
    f.server.release(move |r| {
        r.files = [("inyon-windows-x64.exe", p.as_str())].into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    });
    let mut win = FakeHost::new(&f.server, &[Role::Link]);
    win.platform = Platform::Windows;
    win.arch = Arch::X64;
    win.started_ok.set(false);
    assert!(f.run(&win).is_err());
    assert_eq!(f.read(&f.installed()), "old build");
    assert_eq!(f.read(&f.engine()), "the Bun engine");
}

// opening Inyon from the Start menu or at sign-in showed a console: the app is inyon marked to
// run without one, made from the new program on every update
#[test]
fn windows_makes_the_inyon_app_from_the_new_program_next_to_inyon() {
    let _g = lock();
    let f = Fixture::new();
    let program = fake_inyon_text();
    let p = program.clone();
    f.server.release(move |r| {
        r.files = [("inyon-windows-x64.exe", p.as_str())].into_iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
    });
    let mut win = FakeHost::new(&f.server, &[Role::Link]);
    win.platform = Platform::Windows;
    win.arch = Arch::X64;
    let r = f.run(&win).unwrap();
    assert!(r.changed, "{}", r.message);
    assert_eq!(f.server.downloads(), vec!["inyon-windows-x64.exe.gz".to_string()]);
    let windowless = inyon_cli::engine::make_copy(program.as_bytes()).unwrap();
    assert_eq!(fs::read(inyon_cli::app::app_beside(&f.installed())).unwrap(), windowless);
    assert_eq!(fs::read(f.engine()).unwrap(), windowless);
    assert_eq!(f.bin_names(), vec!["inyon", "inyon-app", "inyon-engine"]);
}

/// A machine added to a hub that still hands out an inyon from before 0.19 (Windows x64 here),
/// with no hub and no kill switch of its own.
fn older_hub(platform: Platform, program: &str) -> (Fixture, update::Install, FakeHost) {
    let f = Fixture::new();
    let (os, arch) = if platform == Platform::Windows { ("windows", Arch::X64) } else { ("linux", Arch::X64) };
    f.server.hub.lock().unwrap().commands.insert(format!("{os}-x64"), program.to_string());
    let install = find_install(&f.installed(), Some(f.server.base.clone()));
    let mut host = FakeHost::new(&f.server, &[Role::Link]);
    host.platform = platform;
    host.arch = arch;
    (f, install, host)
}

#[test]
fn windows_takes_the_hubs_engine_with_an_older_hubs_inyon_never_a_copy_of_it() {
    let _g = lock();
    // 0.18 doesn't answer to the engine's name: as its own windowless copy it would start itself forever
    let old = fake_exe_text();
    let (f, install, win) = older_hub(Platform::Windows, &old);
    fs::write(f.engine(), inyon_cli::engine::make_copy(fake_inyon_text().as_bytes()).unwrap()).unwrap();
    let r = update::update(&install, &win, &RealFiles).unwrap();
    assert!(r.changed, "{}", r.message);
    assert_eq!(f.read(&f.installed()), old);
    assert_eq!(f.read(&f.engine()), "new exe", "the hub's Bun engine, which 0.18 runs");
    // and a hub with 0.19 on it: the copy of that one again
    let new = fake_inyon_text();
    f.server.hub.lock().unwrap().commands.insert("windows-x64".into(), new.clone());
    assert!(update::update(&install, &win, &RealFiles).unwrap().changed);
    assert_eq!(fs::read(f.engine()).unwrap(), inyon_cli::engine::make_copy(new.as_bytes()).unwrap());
}

#[cfg(not(windows))]
#[test]
fn linux_keeps_the_bun_engine_when_an_older_hubs_inyon_comes_back() {
    let _g = lock();
    let (f, install, linux) = older_hub(Platform::Linux, "an inyon from 0.18");
    fs::write(f.engine(), "old engine").unwrap();
    let r = update::update(&install, &linux, &RealFiles).unwrap();
    assert!(r.changed, "{}", r.message);
    assert_eq!(f.read(&f.installed()), "an inyon from 0.18");
    assert_eq!(f.read(&f.engine()), "new linux build", "0.18 still runs the Bun engine, so it stays, as the hub's");
    assert_eq!(f.bin_names(), vec!["inyon", "inyon-engine"]);
}
