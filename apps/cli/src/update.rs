//! `inyon update`: replaces the installed programs with the newest release and restarts each job
//! they run as (the phone link, the hub connection, the hub). The release comes from the website,
//! which keeps a copy of the newest one (RELEASES; INYON_DOWNLOAD points elsewhere). It checks
//! each file against the release's SHA256SUMS like get.sh does (the gzipped copy first); the list
//! itself must carry a signature by Inyon's release key. Nothing is replaced until every download
//! matches, and the old copies come back if the new version doesn't start. A machine added to a
//! hub takes what that hub serves, checked against the sha256 the hub publishes.
//!
//! From 0.19 the engine is built into this program, and from 0.23 the hub runs on it too, so an
//! update takes this program alone. Windows keeps a windowless copy of this program under the
//! engine's name for its logon tasks, made here from the new download.

use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::http;
use crate::paths::{node_dir, ENGINE};
use crate::service::{Installed, Role};
use crate::signing;
use crate::sys::{Libc, Platform, Proc};

/// The GitHub repository the releases are published on. Every release stays there; the website
/// keeps a copy of the newest one only.
pub const RELEASES_REPO: &str = "Abhash-Chakraborty/inyon";
pub const INSTALL_URL: &str = "https://inyon.tech";
/// Where the newest release is downloaded from: the website's copy of it (INSTALL_URL/download),
/// with latest.json naming its version next to the files.
pub const RELEASES: &str = "https://inyon.tech/download";

/// What the app reads to follow an update it started: running, done <version>, current, failed: …
pub fn status_file() -> PathBuf {
    node_dir().join("update-status")
}

#[derive(Debug)]
pub enum Error {
    /// something the update itself worked out, said in full
    Update(String),
    /// anything else (a file that couldn't be moved, say)
    Other(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Update(m) => f.write_str(m),
            Error::Other(m) => write!(f, "The update failed: {m}"),
        }
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Other(e.to_string())
    }
}

fn err(m: impl Into<String>) -> Error {
    Error::Update(m.into())
}

fn parse_version(v: &str) -> Option<(Vec<u64>, Option<String>)> {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let end = v.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(v.len());
    let nums: Vec<u64> = v[..end].split('.').map(|p| p.parse::<u64>().ok()).collect::<Option<Vec<_>>>()?;
    if nums.is_empty() {
        return None;
    }
    let rest = &v[end..];
    let pre = rest.strip_prefix('-').map(|p| p.chars().take_while(|c| c.is_alphanumeric() || *c == '.' || *c == '-').collect::<String>());
    Some((nums, pre))
}

/// Compares two prerelease tags the way `localeCompare` with numeric collation does.
fn compare_pre(a: &str, b: &str) -> std::cmp::Ordering {
    let chunks = |s: &str| -> Vec<(bool, String)> {
        let mut out: Vec<(bool, String)> = Vec::new();
        for c in s.chars() {
            let digit = c.is_ascii_digit();
            match out.last_mut() {
                Some((d, s)) if *d == digit => s.push(c),
                _ => out.push((digit, c.to_string())),
            }
        }
        out
    };
    let (x, y) = (chunks(a), chunks(b));
    for (p, q) in x.iter().zip(y.iter()) {
        let o = if p.0 && q.0 { p.1.parse::<u64>().unwrap_or(0).cmp(&q.1.parse::<u64>().unwrap_or(0)) } else { p.1.cmp(&q.1) };
        if o != std::cmp::Ordering::Equal {
            return o;
        }
    }
    x.len().cmp(&y.len())
}

/// 1 when `a` is newer than `b`, -1 when older, 0 when the same ("v0.6.3", "0.10.0", "0.7.0-rc.1").
pub fn compare_versions(a: &str, b: &str) -> i32 {
    let (Some(x), Some(y)) = (parse_version(a), parse_version(b)) else {
        return 0;
    };
    for i in 0..x.0.len().max(y.0.len()) {
        match x.0.get(i).copied().unwrap_or(0).cmp(&y.0.get(i).copied().unwrap_or(0)) {
            std::cmp::Ordering::Greater => return 1,
            std::cmp::Ordering::Less => return -1,
            std::cmp::Ordering::Equal => {}
        }
    }
    // a release is newer than its own pre-releases
    match (&x.1, &y.1) {
        (None, None) => 0,
        (None, Some(_)) => 1,
        (Some(_), None) => -1,
        (Some(p), Some(q)) if p == q => 0,
        (Some(p), Some(q)) => {
            if compare_pre(p, q) == std::cmp::Ordering::Greater {
                1
            } else {
                -1
            }
        }
    }
}

pub fn is_newer(a: &str, b: &str) -> bool {
    compare_versions(a, b) > 0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Arch {
    X64,
    Arm64,
    Other,
}

impl Arch {
    pub fn current() -> Arch {
        if cfg!(target_arch = "x86_64") {
            Arch::X64
        } else if cfg!(target_arch = "aarch64") {
            Arch::Arm64
        } else {
            Arch::Other
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Arch::X64 => "x64",
            Arch::Arm64 => "arm64",
            Arch::Other => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub os: &'static str,
    pub arch: &'static str,
}

/// This computer as the releases and the hub name it: linux, darwin or windows, x64 or arm64.
pub fn target(platform: Platform, arch: Arch) -> Result<Target, Error> {
    let os = match platform {
        Platform::Darwin => "darwin",
        Platform::Linux => "linux",
        Platform::Windows => "windows",
        Platform::Other => return Err(err(format!("There's no Inyon build for {} {}.", platform.name(), arch.name()))),
    };
    let a = match arch {
        Arch::X64 => "x64",
        Arch::Arm64 => "arm64",
        Arch::Other => return Err(err(format!("There's no Inyon build for {} {}.", platform.name(), arch.name()))),
    };
    Ok(Target { os, arch: a })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Names {
    pub cli: String,
    pub engine: String,
}

/// The files a release attaches for one system ("linux-x64", "windows-x64"): `cli` is this
/// program, `engine` the program that runs the link, the hub connection and the hub.
pub fn release_files(system: &str) -> Names {
    let exe = if system.starts_with("windows") { ".exe" } else { "" };
    Names { cli: format!("inyon-{system}{exe}"), engine: format!("inyon-engine-{system}{exe}") }
}

/// The release files for this computer. Windows on ARM gets the arm64 command but the x64
/// engine: Bun has no Windows on ARM build, and the x64 one runs there under Windows' own
/// emulation. Alpine gets the engine built against musl (inyon-engine-linux-x64-musl); the
/// command is static and has no such variant.
pub fn release_names(platform: Platform, arch: Arch, libc: Libc) -> Result<Names, Error> {
    let t = target(platform, arch)?;
    let engine_arch = if t.os == "windows" { "x64" } else { t.arch };
    let musl = if t.os == "linux" && libc == Libc::Musl { "-musl" } else { "" };
    Ok(Names { cli: release_files(&format!("{}-{}", t.os, t.arch)).cli, engine: release_files(&format!("{}-{engine_arch}{musl}", t.os)).engine })
}

/// The gzipped copy of a release file, under half the size; taken first when SHA256SUMS lists it.
pub fn compressed(file: &str) -> String {
    format!("{file}.gz")
}

/// The one line that installs the newest version from scratch.
pub fn reinstall_command(platform: Platform) -> String {
    if platform == Platform::Windows {
        format!("irm {INSTALL_URL}/get.ps1 | iex")
    } else {
        format!("curl -fsSL {INSTALL_URL}/get.sh | sh")
    }
}

/// The checksum SHA256SUMS lists for `name` (sha256sum writes "<hex>  name" or "<hex> *name").
pub fn sum_for(sums: &str, name: &str) -> Option<String> {
    for line in sums.lines() {
        let line = line.trim();
        // a blank line or a bare word is nobody's checksum; the lines after it still count
        let Some((hash, rest)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        if hash.len() != 64 || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
            continue;
        }
        let file = rest.trim_start().trim_start_matches('*');
        if file == name {
            return Some(hash.to_lowercase());
        }
    }
    None
}

/// The newest release's version, by where `base` points:
/// - a GitHub release pinned to a tag (…/releases/download/v1.2.3) says so itself;
/// - GitHub's newest (…/releases/latest/download): `…/releases/latest` redirects to
///   `…/releases/tag/v1.2.3`, so one request without following it says which;
/// - anything else (the website's copy, or a mirror) names it in latest.json next to the files.
pub fn latest_version(base: &str) -> Result<String, Error> {
    let base = base.trim_end_matches('/');
    if let Some(tag) = pinned_tag(base) {
        return Ok(tag);
    }
    let Some(repo) = base.strip_suffix("/releases/latest/download") else {
        return listed_version(base);
    };
    let page = format!("{repo}/releases/latest");
    let res = http::get_no_redirect(&page).map_err(|e| err(format!("Couldn't reach {} to look for a new version ({e}).", http::host_of(&page))))?;
    let location = res.location.unwrap_or_default();
    let tag = location.split("/tag/").nth(1).map(|t| t.split(['/', '?', '#']).next().unwrap_or("")).filter(|t| !t.is_empty());
    match tag {
        Some(t) => Ok(percent_decode(t.trim_start_matches('v'))),
        None => Err(err(format!("Couldn't tell the newest version from {page} ({}).", res.status))),
    }
}

/// The version a base pinned to a GitHub tag (…/releases/download/v1.2.3) stands for.
fn pinned_tag(base: &str) -> Option<String> {
    let rest = base.trim_end_matches('/').rsplit_once("/releases/download/")?.1;
    (!rest.is_empty() && !rest.contains('/')).then(|| rest.trim_start_matches('v').to_string())
}

/// Whether the newest version at `base` comes from latest.json (the website or a mirror) rather
/// than from GitHub's redirect or a pinned tag.
pub fn from_listing(base: &str) -> bool {
    let base = base.trim_end_matches('/');
    pinned_tag(base).is_none() && !base.ends_with("/releases/latest/download")
}

/// latest.json isn't signed, so it only chooses which release to fetch; the signed SHA256SUMS has
/// to agree. Each release lists its APK as inyon-<version>.apk, so a list without the one for
/// `version` belongs to another release: an old copy, or the website caught mid-update.
pub fn check_listed_version(sums: &str, version: &str) -> Result<(), Error> {
    let apk = format!("inyon-{version}.apk");
    if sum_for(sums, &apk).is_some() {
        return Ok(());
    }
    Err(err(format!("The release's signed checksum list isn't the one for Inyon {version} (it doesn't list {apk}), so nothing was changed. Try again later.")))
}

/// The version latest.json at `base` names: {"version":"0.14.0","tag":"v0.14.0",…}. Anything
/// that isn't a version number is refused rather than compared, since it ends up in messages and
/// in the status file the app reads.
fn listed_version(base: &str) -> Result<String, Error> {
    let url = format!("{base}/latest.json");
    let host = http::host_of(base);
    let res = http::get(&url).map_err(|e| err(format!("Couldn't reach {host} to look for a new version ({e}).")))?;
    if !res.ok() {
        return Err(err(format!("Couldn't tell the newest version: {host} answered {} for {url}.", res.status)));
    }
    let listed: Option<String> = serde_json::from_slice::<serde_json::Value>(&res.body)
        .ok()
        .and_then(|v| v.get("version").and_then(serde_json::Value::as_str).map(|s| s.trim().trim_start_matches('v').to_string()));
    match listed {
        Some(v) if parse_version(&v).is_some() && v.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+')) => Ok(v),
        _ => Err(err(format!("Couldn't tell the newest version: {url} doesn't name one."))),
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn sha256(data: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(data);
    hex(&h.finalize())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_file(file: &Path) -> io::Result<String> {
    use std::io::Read;
    let mut f = fs::File::open(file)?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex(&h.finalize()))
}

fn download(url: &str, log: &dyn Fn(&str)) -> Result<Vec<u8>, Error> {
    // one line per megabyte, not per packet: the line is redrawn on a terminal and kept in a log
    let last = std::cell::Cell::new(u64::MAX);
    let progress = |done: u64, total: Option<u64>| {
        if let Some(t) = total.filter(|t| *t > 4 << 20) {
            if last.replace(done >> 20) != done >> 20 {
                log(&format!("\r  {} of {} MB", done >> 20, t >> 20));
            }
        }
    };
    let res = http::get_with_progress(url, &progress).map_err(|e| err(format!("Couldn't download {url} ({e}).")))?;
    if !res.ok() {
        return Err(err(format!("Couldn't download {url} ({}).", res.status)));
    }
    Ok(res.body)
}

fn gunzip(data: &[u8]) -> io::Result<Vec<u8>> {
    use std::io::Read;
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(data).read_to_end(&mut out)?;
    Ok(out)
}

/// Downloads release file `name` to `to`, checked against the release's SHA256SUMS. The gzipped
/// copy is taken when the release lists one (under half the size), and checked before it's
/// unpacked; the unpacked file is checked like a plain download.
pub fn fetch_release(base: &str, name: &str, sums: &str, to: &Path, log: &dyn Fn(&str)) -> Result<(), Error> {
    let want = sum_for(sums, name).ok_or_else(|| err(format!("The release doesn't list {name} in SHA256SUMS, so nothing was changed.")))?;
    let mismatch = || err("The download didn't match the release's checksum, so nothing was changed. Try again.");
    let gz = sum_for(sums, &compressed(name));
    let file = if gz.is_some() { compressed(name) } else { name.to_string() };
    let mut data = download(&format!("{base}/{file}"), log)?;
    if let Some(gz) = gz {
        if sha256(&data) != gz {
            return Err(mismatch());
        }
        data = gunzip(&data).map_err(|e| err(format!("Couldn't unpack {} ({e}).", compressed(name))))?;
    }
    if sha256(&data) != want {
        return Err(mismatch());
    }
    fs::write(to, data)?;
    Ok(())
}

fn text(url: &str) -> Result<String, Error> {
    let res = http::get(url).map_err(|e| err(format!("Couldn't reach {url} ({e}).")))?;
    if !res.ok() {
        return Err(err(format!("{url} answered {}.", res.status)));
    }
    Ok(String::from_utf8_lossy(&res.body).into_owned())
}

pub const UNSIGNED: &str = "The release's checksum list isn't signed by Inyon's key, so nothing was changed.";

/// The release's SHA256SUMS, once SHA256SUMS.minisig next to it checks out against `key`. A
/// release without the signature (one from before 0.14, or a copy from somewhere else) is
/// refused before anything is downloaded.
pub fn release_sums(base: &str, key: &str, platform: Platform) -> Result<String, Error> {
    let sums = text(&format!("{base}/SHA256SUMS"))?;
    let refused = || err(format!("{UNSIGNED} Install the new version with:\n  {}", reinstall_command(platform)));
    let url = format!("{base}/SHA256SUMS.minisig");
    let res = http::get(&url).map_err(|e| err(format!("Couldn't reach {url} ({e}).")))?;
    // a release from before 0.14 has no signature file, and neither has a copy made elsewhere
    if !res.ok() {
        return Err(refused());
    }
    signing::verify_sums(&sums, &String::from_utf8_lossy(&res.body), key).map_err(|_| refused())?;
    Ok(sums.trim().to_string())
}

/// The file system calls the swap makes, so tests can act like Windows (a running .exe is locked).
pub trait Files {
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()>;
    /// removes `f`; a file that isn't there is fine
    fn remove(&self, f: &Path) -> io::Result<()>;
    fn exists(&self, f: &Path) -> bool;
    fn list(&self, dir: &Path) -> io::Result<Vec<String>>;
}

pub struct RealFiles;

impl Files for RealFiles {
    fn rename(&self, from: &Path, to: &Path) -> io::Result<()> {
        fs::rename(from, to)
    }
    fn remove(&self, f: &Path) -> io::Result<()> {
        match fs::remove_file(f) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            r => r,
        }
    }
    fn exists(&self, f: &Path) -> bool {
        f.exists()
    }
    fn list(&self, dir: &Path) -> io::Result<Vec<String>> {
        Ok(fs::read_dir(dir)?.filter_map(|e| e.ok()).map(|e| e.file_name().to_string_lossy().into_owned()).collect())
    }
}

fn with_suffix(p: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{suffix}", p.to_string_lossy()))
}

/// A free name to rename `target` aside to (two in the same millisecond don't collide).
fn aside(files: &dyn Files, target: &Path, now: u64) -> PathBuf {
    let mut n = now;
    while files.exists(&with_suffix(target, &format!(".old-{n}"))) {
        n += 1;
    }
    with_suffix(target, &format!(".old-{n}"))
}

/// Puts `fresh` in place of `target` and returns where the old copy went. Windows can't
/// overwrite or delete a running .exe, but it can rename one, so the old copy is always renamed
/// aside first (target.old-<time>); it's removed once the new version runs, or by a later start.
pub fn swap_in(files: &dyn Files, target: &Path, fresh: &Path, now: u64) -> io::Result<Option<PathBuf>> {
    let old = aside(files, target, now);
    let had = files.exists(target);
    if had {
        files.rename(target, &old)?;
    }
    if let Err(e) = files.rename(fresh, target) {
        if had {
            files.rename(&old, target)?;
        }
        return Err(e);
    }
    Ok(if had { Some(old) } else { None })
}

/// Puts the old copy back; the new one is renamed aside too, in case it's still running.
pub fn swap_back(files: &dyn Files, target: &Path, old: &Path, now: u64) -> io::Result<()> {
    if files.exists(target) {
        let away = aside(files, target, now);
        files.rename(target, &away)?;
    }
    files.rename(old, target)
}

/// Removes copies renamed aside by earlier updates. Only ones older than ten minutes: a copy
/// from the update running right now may still be needed to go back to.
pub fn cleanup_old(files: &dyn Files, target: &Path, now: u64) {
    let base = format!("{}.old-", target.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    let dir = target.parent().unwrap_or(Path::new("."));
    let Ok(names) = files.list(dir) else {
        return;
    };
    for name in names {
        let Some(stamp) = name.strip_prefix(&base).and_then(|s| s.parse::<u64>().ok()) else {
            continue;
        };
        if now.saturating_sub(stamp) < 10 * 60_000 {
            continue;
        }
        let _ = files.remove(&dir.join(name));
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// from the releases (get.sh, get.ps1), the website's copy unless INYON_DOWNLOAD says otherwise
    Binary,
    /// from the hub this machine was added to
    Hub(String),
}

/// Which install this program is: `file` is the installed command-line program.
#[derive(Debug, Clone)]
pub struct Install {
    pub kind: Kind,
    pub file: PathBuf,
}

pub fn find_install(file: &Path, hub: Option<String>) -> Install {
    Install { kind: hub.map(Kind::Hub).unwrap_or(Kind::Binary), file: file.to_path_buf() }
}

/// How to update a copy npm installed: npm owns that file, so `inyon update` leaves it alone.
pub const NPM_UPDATE: &str = "npm install -g @abhash-chakraborty/inyon@latest";

/// Whether `file` is the copy from the `inyon` npm package (it sits inside a node_modules folder).
pub fn from_npm(file: &Path) -> bool {
    file.components().any(|c| c.as_os_str() == "node_modules")
}

/// The engine next to the installed program.
pub fn engine_beside(file: &Path) -> PathBuf {
    let ext = file.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    file.with_file_name(format!("{ENGINE}{ext}"))
}

/// What the update needs from the machine: which jobs run and how to restart them. Tests fake it.
pub trait Host {
    fn version(&self) -> String;
    /// the jobs this machine runs inyon as in the background, each restarted after the swap
    fn roles(&self) -> Vec<Role>;
    /// restarts a job's background service so the new file runs
    fn restart(&self, role: Role, cmd: &[String]) -> Installed;
    /// waits for the restarted copy to come up (not the copy `before`); true once it runs and stays up
    fn started(&self, role: Role, version: Option<&str>, before: Option<u32>) -> bool;
    /// the jobs still set up under their names from before the rename to Inyon
    fn old_roles(&self) -> Vec<Role> {
        Vec::new()
    }
    /// moves such a job to inyon's own service (see service::move_old_service)
    fn move_old(&self, _role: Role) -> Installed {
        Installed { ok: false, how: "nothing to move".into() }
    }
    fn log(&self, line: &str);
    fn platform(&self) -> Platform {
        Platform::current()
    }
    fn arch(&self) -> Arch {
        Arch::current()
    }
    /// where releases come from (INYON_DOWNLOAD points it elsewhere)
    fn download_base(&self) -> String {
        crate::paths::env_var("INYON_DOWNLOAD").unwrap_or_else(|| RELEASES.to_string()).trim_end_matches('/').to_string()
    }
    /// the key the release's checksum list must be signed with
    fn signing_key(&self) -> String {
        signing::signing_key(&self.download_base())
    }
    /// the C library this system's engine is built against
    fn libc(&self) -> Libc {
        Libc::current()
    }
}

/// What the windowless copy of the program is called in progress lines (Windows).
const COPY: &str = "the windowless copy";
/// The Inyon app, made from the new program like the windowless copy (Windows).
const APP_COPY: &str = "the Inyon app";

/// Whether a swap's file is made here from the new program rather than downloaded.
fn derived(s: &Swap) -> bool {
    s.name == COPY || s.name == APP_COPY
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub changed: bool,
    pub message: String,
    pub version: Option<String>,
}

struct Swap {
    file: PathBuf,
    tmp: PathBuf,
    name: String,
}

/// Writes the windowless copy of `program` to the copy swap's download file (Windows).
fn make_copy_swap(program: &Path, s: &Swap) -> Result<(), Error> {
    let bytes = crate::engine::make_copy(&fs::read(program)?).map_err(err)?;
    fs::write(&s.tmp, bytes)?;
    Ok(())
}

/// Updates the install this program belongs to. The error says what went wrong.
pub fn update(install: &Install, host: &dyn Host, files: &dyn Files) -> Result<Outcome, Error> {
    let r = update_files(install, host, files)?;
    // a Bun engine left from before 0.23 goes (Windows keeps the windowless copy under its name),
    // unless the inyon now in place is one from before 0.19 (an older hub's), which still runs it
    let engine = engine_beside(&install.file);
    if r.changed && host.platform() != Platform::Windows && files.exists(&engine) && crate::engine::has_engine_built_in(&install.file) {
        let _ = files.remove(&engine);
    }
    Ok(r)
}

fn update_files(install: &Install, host: &dyn Host, files: &dyn Files) -> Result<Outcome, Error> {
    let platform = host.platform();
    let engine = engine_beside(&install.file);
    // Windows needs the windowless copy made from the new program
    let copy = (platform == Platform::Windows).then(|| Swap { file: engine.clone(), tmp: with_suffix(&engine, ".download"), name: COPY.into() });
    // and the Inyon app (inyon-app.exe) next to inyon, made from the new program the same way
    let app = crate::app::app_beside(&install.file);
    let app_swap = |name: String| Swap { file: app.clone(), tmp: with_suffix(&app, ".download"), name };
    match &install.kind {
        Kind::Binary => {
            let base = host.download_base();
            let version = latest_version(&base)?;
            let mine = host.version();
            if !is_newer(&version, &mine) {
                return Ok(Outcome { changed: false, message: format!("inyon {mine} is up to date."), version: Some(mine) });
            }
            let names = release_names(platform, host.arch(), host.libc())?;
            // the signature first: a list that isn't Inyon's says nothing about what the release has
            let sums = release_sums(&base, &host.signing_key(), platform)?;
            // then that the signed list is the release latest.json named
            if from_listing(&base) {
                check_listed_version(&sums, &version)?;
            }
            let program = Swap { file: install.file.clone(), tmp: with_suffix(&install.file, ".download"), name: names.cli.clone() };
            let mut swaps: Vec<Swap> = copy.into_iter().collect();
            // the program last: it's the one the restarts run
            swaps.push(program);
            let fetched: Result<(), Error> = (|| {
                for s in swaps.iter().filter(|s| !derived(s)) {
                    files.remove(&s.tmp)?;
                    host.log(&format!("Downloading inyon {version} ({})…", s.name));
                    fetch_release(&base, &s.name, &sums, &s.tmp, &|l| host.log(l))?;
                    runnable(&s.tmp, platform);
                }
                let fresh = &swaps[swaps.len() - 1].tmp;
                for s in swaps.iter().filter(|s| derived(s)) {
                    make_copy_swap(fresh, s)?;
                }
                Ok(())
            })();
            // the Inyon app, made from the new program once it's here: only an inyon with the engine
            // built in (0.19 and on) can be the app
            let fresh = swaps[swaps.len() - 1].tmp.clone();
            let fetched = fetched.and_then(|()| {
                if platform == Platform::Windows && crate::engine::has_engine_built_in(&fresh) {
                    let a = app_swap(APP_COPY.into());
                    make_copy_swap(&fresh, &a)?;
                    swaps.insert(swaps.len() - 1, a);
                }
                Ok(())
            });
            if let Err(e) = fetched {
                for s in &swaps {
                    let _ = files.remove(&s.tmp);
                }
                return Err(match e {
                    Error::Update(m) => err(format!("{m} Install inyon {version} with:\n  {}", reinstall_command(platform))),
                    other => other,
                });
            }
            put(install, host, files, &swaps, Some(&version)).map_err(|e| match e {
                Error::Update(m) if m.contains("didn't start") => err(format!("{m} Install the new version with:\n  {}", reinstall_command(platform))),
                other => other,
            })
        }
        Kind::Hub(hub) => {
            let t = target(platform, host.arch())?;
            let q = format!("?os={}&arch={}", t.os, t.arch);
            // what the hub hands out is checked against the release's signed list, as the hub has it
            let signed = hub_signed_sums(hub, &host.signing_key())?;
            // a hub from before 0.13 hands out one program under /node/inyon, which this
            // program can't run as its engine
            let engine_sum = || -> Result<String, Error> {
                hub_checksum(hub, signed.as_deref(), "inyon-engine", &hub_files(&t).engine, &q)?
                    .ok_or_else(|| err(format!("{hub} hands out one program, from before the engine and inyon were split. Update the hub first, then this machine.")))
            };
            let cli_sum = hub_checksum(hub, signed.as_deref(), "inyon", &hub_files(&t).cli, &q)?
                .ok_or_else(|| err(format!("{hub} didn't give a checksum for inyon. Update the hub first.")))?;
            let mine = host.version();
            let same = |file: &Path, want: &str| -> Result<bool, Error> { Ok(files.exists(file) && sha256_file(file)? == want) };
            let new_program = !same(&install.file, &cli_sum)?;
            let (program_tmp, engine_tmp) = (with_suffix(&install.file, ".download"), with_suffix(&engine, ".download"));
            let said = std::cell::Cell::new(false);
            // it comes gzipped (about half the size), which matters over a VPN
            let fetch = |route: &str, want: &str, to: &Path| -> Result<(), Error> {
                if !said.replace(true) {
                    host.log(&format!("Downloading inyon from {hub}…"));
                }
                files.remove(to)?;
                fetch_hub_file(hub, route, &q, want, to, &|l| host.log(l))?;
                runnable(to, platform);
                Ok(())
            };
            let hubs_engine = |want: &str| -> Result<Option<Swap>, Error> {
                if same(&engine, want)? {
                    return Ok(None);
                }
                fetch("inyon-engine", want, &engine_tmp)?;
                Ok(Some(Swap { tmp: engine_tmp.clone(), file: engine.clone(), name: "the hub's engine".into() }))
            };
            let fetched = (|| -> Result<Vec<Swap>, Error> {
                let mut swaps = Vec::new();
                if new_program {
                    fetch("inyon", &cli_sum, &program_tmp)?;
                }
                let program = if new_program { &program_tmp } else { &install.file };
                if crate::engine::has_engine_built_in(program) {
                    // the copy follows the program; one that isn't there yet (a Bun engine in its place) is made now
                    if platform == Platform::Windows && (new_program || !crate::engine::is_builtin_copy(&engine)) {
                        let c = Swap { file: engine.clone(), tmp: engine_tmp.clone(), name: COPY.into() };
                        make_copy_swap(program, &c)?;
                        swaps.push(c);
                    }
                } else {
                    // an older hub's inyon, from before 0.19, still runs the Bun engine: the hub's
                    // goes next to it, never a copy of a program that doesn't answer as the engine
                    swaps.extend(hubs_engine(&engine_sum()?)?);
                }
                // the hub hands out no app: it's made from the hub's inyon, once that has the engine built in
                let app_stale = || crate::app::build_of(&app).is_none() || crate::app::build_of(&app) != crate::app::build_of(program);
                if platform == Platform::Windows && crate::engine::has_engine_built_in(program) && (new_program || !files.exists(&app) || app_stale()) {
                    let a = app_swap(APP_COPY.into());
                    make_copy_swap(program, &a)?;
                    swaps.push(a);
                }
                if new_program {
                    swaps.push(Swap { tmp: program_tmp.clone(), file: install.file.clone(), name: "the hub's inyon".into() });
                }
                Ok(swaps)
            })();
            match fetched {
                Err(e) => {
                    let _ = files.remove(&program_tmp);
                    let _ = files.remove(&engine_tmp);
                    Err(e)
                }
                Ok(swaps) if swaps.is_empty() => Ok(Outcome { changed: false, message: format!("inyon {mine} is the same as the hub's."), version: Some(mine) }),
                Ok(swaps) => put(install, host, files, &swaps, None),
            }
        }
    }
}

/// The release files a hub hands this system: its own command, and the x64 engine on Windows.
pub fn hub_files(t: &Target) -> Names {
    let engine_arch = if t.os == "windows" { "x64" } else { t.arch };
    Names { cli: release_files(&format!("{}-{}", t.os, t.arch)).cli, engine: release_files(&format!("{}-{engine_arch}", t.os)).engine }
}

/// Why nothing is taken from a hub with no signed list.
pub fn hub_unsigned(hub: &str) -> String {
    format!(
        "{} doesn't hand out Inyon's signed list of checksums (a hub from before 0.20, or one running from a checkout), so nothing was changed. Update the hub first; or, to take the hub's own checksums anyway, set INYON_ALLOW_INSECURE=1 and run this again.",
        crate::http::host_of(hub)
    )
}

/// The release's SHA256SUMS as the hub has it (/node/release/), once SHA256SUMS.minisig signs it
/// with `key` and it lists the APK of the version the hub says it hands out (/node/release/version),
/// the rule `inyon update` holds the website to: a machine never takes a checksum only the hub, or
/// someone between, vouches for, nor an older release's list passed off as the hub's. None only
/// when the hub has no list to give (404 from a hub running from a checkout, or an older hub's web
/// page); any other failure is the hub's to fix and says so.
pub fn hub_signed_sums(hub: &str, key: &str) -> Result<Option<String>, Error> {
    let host = crate::http::host_of(hub);
    let base = format!("{hub}/node/release");
    let url = format!("{base}/SHA256SUMS");
    let res = http::get(&url).map_err(|e| err(format!("Couldn't reach {url} ({e}).")))?;
    let sums = String::from_utf8_lossy(&res.body).into_owned();
    if res.status == 404 {
        return Ok(None);
    }
    if !res.ok() {
        let why = sums.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").chars().take(200).collect::<String>();
        return Err(err(format!(
            "{host} couldn't hand out Inyon's signed list of checksums (it answered {}{}), so nothing was changed. Try again later, or look at the hub's log.",
            res.status,
            if why.is_empty() { String::new() } else { format!(": {why}") }
        )));
    }
    // an older hub answers with its web page, which doesn't list a single checksum
    if !sums.lines().any(|l| l.split_whitespace().next().is_some_and(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()))) {
        return Ok(None);
    }
    let sig_url = format!("{base}/SHA256SUMS.minisig");
    let sig = http::get(&sig_url).map_err(|e| err(format!("Couldn't reach {sig_url} ({e}).")))?;
    if !sig.ok() || signing::verify_sums(&sums, &String::from_utf8_lossy(&sig.body), key).is_err() {
        return Err(err(format!("The list of checksums {host} hands out isn't signed by Inyon's key, so nothing was changed.")));
    }
    let version_url = format!("{base}/version");
    let said = http::get(&version_url).map_err(|e| err(format!("Couldn't reach {version_url} ({e}).")))?;
    let version = String::from_utf8_lossy(&said.body).trim().trim_start_matches('v').to_string();
    if !said.ok() || parse_version(&version).is_none() || !version.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '+')) {
        return Err(err(format!("{host} didn't say which version of Inyon it hands out, so its list can't be matched to one and nothing was changed. Update the hub.")));
    }
    check_listed_version(&sums, &version)?;
    Ok(Some(sums))
}

/// The checksum to check one of a hub's files against: its line in the signed list (`name`), or
/// with no list and INYON_ALLOW_INSECURE set, the one the hub gives for `route`. None when the hub
/// doesn't hand that file out.
pub fn hub_checksum(hub: &str, signed: Option<&str>, route: &str, name: &str, q: &str) -> Result<Option<String>, Error> {
    match signed {
        Some(sums) => sum_for(sums, name).map(Some).ok_or_else(|| err(format!("The signed list from {} has no {name}, so nothing was changed.", crate::http::host_of(hub)))),
        None if crate::paths::env_var("INYON_ALLOW_INSECURE").is_some() => hub_sum(hub, route, q),
        None => Err(err(hub_unsigned(hub))),
    }
}

/// The checksum a hub publishes for one of its files (`inyon`, `inyon-engine`); None when the
/// hub doesn't serve that file.
pub fn hub_sum(hub: &str, route: &str, q: &str) -> Result<Option<String>, Error> {
    let url = format!("{hub}/node/{route}.sha256{q}");
    let res = http::get(&url).map_err(|e| err(format!("Couldn't reach {url} ({e}).")))?;
    if res.status == 404 {
        return Ok(None);
    }
    if !res.ok() {
        return Err(err(format!("{url} answered {}.", res.status)));
    }
    let want = String::from_utf8_lossy(&res.body).trim().to_lowercase();
    if want.len() != 64 || !want.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(err(format!("{hub} didn't give a checksum for {route}. Update the hub first.")));
    }
    Ok(Some(want))
}

/// Downloads one of the hub's files (gzipped) to `to`, checked against `want`.
pub fn fetch_hub_file(hub: &str, route: &str, q: &str, want: &str, to: &Path, log: &dyn Fn(&str)) -> Result<(), Error> {
    let data = gunzip(&download(&format!("{hub}/node/{route}.gz{q}"), log)?).map_err(|e| err(format!("Couldn't unpack the hub's download ({e}).")))?;
    if sha256(&data) != want {
        return Err(err("The download didn't match its checksum, so nothing was changed. Try again."));
    }
    fs::write(to, data)?;
    Ok(())
}

/// Makes a checked download a program this system will start.
fn runnable(file: &Path, platform: Platform) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(file, fs::Permissions::from_mode(0o755));
    }
    // Apple Silicon only runs signed programs; a local signature is enough (as get.sh does)
    if platform == Platform::Darwin {
        crate::sys::run(&["codesign", "--force", "--sign", "-", &file.to_string_lossy()]);
    }
}

fn now_ms() -> u64 {
    crate::state::now_ms() as u64
}

/// Swaps the checked downloads in (the program that's restarted last), restarts each job, and
/// puts everything back if the new copy won't run.
fn put(install: &Install, host: &dyn Host, files: &dyn Files, swaps: &[Swap], version: Option<&str>) -> Result<Outcome, Error> {
    let roles = host.roles();
    let mut done: Vec<(&Swap, Option<PathBuf>)> = Vec::new();
    for s in swaps {
        match swap_in(files, &s.file, &s.tmp, now_ms()) {
            Ok(old) => done.push((s, old)),
            Err(e) => {
                // what went in before this one comes back out
                for (d, old) in done.iter().rev() {
                    match old {
                        Some(o) => {
                            let _ = swap_back(files, &d.file, o, now_ms());
                        }
                        None => {
                            let _ = files.remove(&d.file);
                        }
                    }
                }
                return Err(Error::Other(format!("couldn't put {} in place ({e})", s.file.display())));
            }
        }
    }
    let program_old = done.last().and_then(|(_, old)| old.clone());
    let label = version.map(|v| format!("inyon {v}")).unwrap_or_else(|| "The new inyon".to_string());
    let cmd = vec![install.file.to_string_lossy().into_owned()];
    let before: Vec<(Role, Option<u32>)> = roles.iter().map(|r| (*r, read_running(*r).and_then(|s| s.pid))).collect();
    let restarted: Vec<Role> = roles.iter().copied().filter(|r| host.restart(*r, &cmd).ok).collect();
    if restarted.is_empty() {
        // nothing runs it in the background: the new file is in place for the next start
        return Ok(Outcome {
            changed: true,
            version: version.map(String::from),
            message: format!("{label} is installed. It isn't running as a background service, so start it again to use it."),
        });
    }
    let up = restarted.iter().all(|r| {
        let b = before.iter().find(|(role, _)| role == r).and_then(|(_, pid)| *pid);
        host.started(*r, version, b)
    });
    if up {
        for (_, old) in &done {
            if let Some(o) = old {
                let _ = files.remove(o);
            }
        }
        // a job still under tether-node's name was restarted under it; now that the new version
        // runs, it moves to inyon's own service (the engine did this a while after starting, before 0.14)
        let mut notes = Vec::new();
        for r in host.old_roles() {
            let m = host.move_old(r);
            notes.push(if m.ok {
                format!("The {r} moved from its tether-node service to inyon's.")
            } else {
                format!("The {r} still runs as its tether-node service ({}); `inyon migrate` tries again.", m.how)
            });
        }
        let mut message = format!("{label} is installed and running.");
        for n in notes {
            message.push(' ');
            message.push_str(&n);
        }
        return Ok(Outcome { changed: true, version: version.map(String::from), message });
    }
    if program_old.is_none() {
        return Err(err(format!("{label} didn't start.")));
    }
    // the program first, so the restart runs the old one; a file that had no old copy just goes
    for (s, old) in done.iter().rev() {
        match old {
            Some(o) => {
                let _ = swap_back(files, &s.file, o, now_ms());
            }
            None => {
                let _ = files.remove(&s.file);
            }
        }
    }
    for r in &restarted {
        host.restart(*r, &cmd);
    }
    Err(err(format!("{label} didn't start, so the previous version is running again.")))
}

/// What a started copy writes with its pid and version: state.json (the link), node.json, hub.json.
pub fn running_file(role: Role) -> PathBuf {
    node_dir().join(match role {
        Role::Link => "state.json".to_string(),
        r => format!("{r}.json"),
    })
}

pub fn read_running(role: Role) -> Option<crate::state::Running> {
    let text = fs::read_to_string(running_file(role)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Whether a copy doing `role` has started and still runs.
pub fn is_running(proc: &dyn Proc, role: Role) -> bool {
    read_running(role).and_then(|r| r.pid).is_some_and(|pid| proc.alive(pid))
}

pub struct WaitOptions {
    pub tries: u32,
    pub every_ms: u64,
    pub settle_ms: u64,
}

impl Default for WaitOptions {
    fn default() -> Self {
        WaitOptions { tries: 60, every_ms: 500, settle_ms: 4000 }
    }
}

/// Waits for the restarted copy: it writes its pid and version when it starts (a pid other than
/// the copy before it), and must still be running a few seconds later.
pub fn wait_started(proc: &dyn Proc, role: Role, version: Option<&str>, before: Option<u32>, o: &WaitOptions) -> bool {
    for _ in 0..o.tries {
        if let Some(s) = read_running(role) {
            let fresh = s.pid.is_some() && s.pid != before && s.pid.is_some_and(|p| proc.alive(p));
            let right = version.is_none() || s.version.as_deref() == version;
            if fresh && right {
                proc.sleep_ms(o.settle_ms);
                return s.pid.is_some_and(|p| proc.alive(p));
            }
        }
        proc.sleep_ms(o.every_ms);
    }
    false
}

/// Whether an update is under way: the status file says running and was written in the last 30
/// minutes, as long as the update's task may run (one that stopped without a word doesn't count
/// for ever).
pub fn under_way() -> bool {
    let file = status_file();
    let fresh = fs::metadata(&file).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() < 30 * 60);
    fresh && fs::read_to_string(&file).is_ok_and(|t| t.trim() == "running")
}

/// Records how the update went, for the app (and appends to update.log).
pub fn set_status(line: &str) {
    let dir = node_dir();
    let _ = fs::create_dir_all(&dir);
    let _ = fs::write(status_file(), format!("{line}\n"));
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(dir.join("update.log")) {
        use std::io::Write;
        let _ = writeln!(f, "{} {line}", crate::state::format_iso(crate::state::now_ms()));
    }
}

/// The link from before this version doesn't write its version into state.json: running setup
/// again restarts such a link, so re-running the installer is an update too.
pub fn stale_link(state: Option<&crate::state::LinkState>, version: &str) -> bool {
    state.is_some_and(|s| s.version.as_deref() != Some(version))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_by_number_and_a_release_beats_its_prereleases() {
        assert_eq!(compare_versions("0.6.3", "0.3.0"), 1);
        assert_eq!(compare_versions("0.3.0", "0.6.3"), -1);
        assert_eq!(compare_versions("0.10.0", "0.9.9"), 1);
        assert_eq!(compare_versions("v0.7.0", "0.7.0"), 0);
        assert_eq!(compare_versions("0.7", "0.7.0"), 0);
        assert_eq!(compare_versions("18446744073709551615.0.0", "1.0.0"), 1);
        assert_eq!(compare_versions("0.7.0", "0.7.0-rc.1"), 1);
        assert_eq!(compare_versions("0.7.0-rc.10", "0.7.0-rc.2"), 1);
        assert!(!is_newer("nonsense", "0.1.0"));
        assert!(!is_newer("0.6.3", "0.6.3"));
    }

    #[test]
    fn sha256sums_lines_match_the_exact_file_name_in_either_format() {
        let sums = format!("{}  inyon-linux-x64\n\nnotes\n{} *inyon-windows-x64.exe\n", "a".repeat(64), "b".repeat(64));
        assert_eq!(sum_for(&sums, "inyon-linux-x64").unwrap(), "a".repeat(64));
        // the blank line and the bare word before it don't hide this one
        assert_eq!(sum_for(&sums, "inyon-windows-x64.exe").unwrap(), "b".repeat(64));
        assert_eq!(sum_for(&sums, "inyon-linux"), None);
        assert_eq!(sum_for(&sums, "notes"), None);
    }

    #[test]
    fn the_release_files_for_each_system() {
        let names = |p, a| release_names(p, a, Libc::Glibc).unwrap();
        assert_eq!(names(Platform::Linux, Arch::Arm64).cli, "inyon-linux-arm64");
        assert_eq!(names(Platform::Linux, Arch::Arm64).engine, "inyon-engine-linux-arm64");
        assert_eq!(names(Platform::Darwin, Arch::X64).cli, "inyon-darwin-x64");
        // Windows on ARM gets its own command but the x64 engine, since Bun has no arm64 build for it
        let win_arm = names(Platform::Windows, Arch::Arm64);
        assert_eq!(win_arm.cli, "inyon-windows-arm64.exe");
        assert_eq!(win_arm.engine, "inyon-engine-windows-x64.exe");
        assert_eq!(names(Platform::Windows, Arch::X64).engine, "inyon-engine-windows-x64.exe");
        assert_eq!(target(Platform::Windows, Arch::Arm64).unwrap(), Target { os: "windows", arch: "arm64" });
        assert!(release_names(Platform::Other, Arch::X64, Libc::Glibc).is_err());
        assert_eq!(compressed("inyon-linux-x64"), "inyon-linux-x64.gz");
    }

    #[test]
    fn alpine_gets_the_musl_engine_and_the_same_command() {
        let alpine = release_names(Platform::Linux, Arch::X64, Libc::Musl).unwrap();
        assert_eq!(alpine, Names { cli: "inyon-linux-x64".into(), engine: "inyon-engine-linux-x64-musl".into() });
        assert_eq!(release_names(Platform::Linux, Arch::Arm64, Libc::Musl).unwrap().engine, "inyon-engine-linux-arm64-musl");
        // musl only means something on Linux
        assert_eq!(release_names(Platform::Darwin, Arch::Arm64, Libc::Musl).unwrap().engine, "inyon-engine-darwin-arm64");
        assert_eq!(release_names(Platform::Windows, Arch::X64, Libc::Musl).unwrap().engine, "inyon-engine-windows-x64.exe");
        // the generic helper names a system's files as given; the protocol package does the same
        assert_eq!(release_files("linux-x64-musl").engine, "inyon-engine-linux-x64-musl");
    }

    #[test]
    fn a_pinned_tag_names_its_version_without_a_request() {
        assert_eq!(latest_version("https://example.invalid/x/releases/download/v0.6.9").unwrap(), "0.6.9");
        assert_eq!(latest_version("https://example.invalid/x/releases/download/0.6.9/").unwrap(), "0.6.9");
    }

    #[test]
    fn the_releases_are_the_websites_copy() {
        assert_eq!(RELEASES, format!("{INSTALL_URL}/download"));
        assert!(from_listing(RELEASES));
        assert!(from_listing("http://127.0.0.1:8000/download/"));
        assert!(!from_listing("https://github.com/Abhash-Chakraborty/inyon/releases/latest/download"));
        assert!(!from_listing("https://github.com/Abhash-Chakraborty/inyon/releases/download/v0.13.3/"));
    }

    #[test]
    fn a_signed_list_belongs_to_the_release_whose_apk_it_names() {
        let sums = format!("{}  inyon-linux-x64\n{} *inyon-0.14.0.apk\n", "a".repeat(64), "b".repeat(64));
        assert!(check_listed_version(&sums, "0.14.0").is_ok());
        let e = check_listed_version(&sums, "0.14.1").unwrap_err().to_string();
        assert_eq!(e, "The release's signed checksum list isn't the one for Inyon 0.14.1 (it doesn't list inyon-0.14.1.apk), so nothing was changed. Try again later.");
        assert!(check_listed_version(&format!("{}  inyon-linux-x64\n", "a".repeat(64)), "0.14.0").is_err());
    }

    #[test]
    fn reinstall_lines() {
        assert_eq!(reinstall_command(Platform::Darwin), "curl -fsSL https://inyon.tech/get.sh | sh");
        assert_eq!(reinstall_command(Platform::Windows), "irm https://inyon.tech/get.ps1 | iex");
    }

    #[test]
    fn a_link_from_before_versions_were_recorded_counts_as_stale() {
        use crate::state::LinkState;
        let with = |v: Option<&str>| LinkState { version: v.map(String::from), ..Default::default() };
        assert!(stale_link(Some(&with(None)), "0.6.4"));
        assert!(stale_link(Some(&with(Some("0.6.3"))), "0.6.4"));
        assert!(!stale_link(Some(&with(Some("0.6.4"))), "0.6.4"));
        assert!(!stale_link(None, "0.6.4"));
    }

    #[test]
    fn a_copy_inside_node_modules_came_from_npm() {
        assert!(from_npm(Path::new("/usr/lib/node_modules/inyon/node_modules/@abhash-chakraborty/inyon-linux-x64/bin/inyon")));
        assert!(from_npm(Path::new("C:/Users/u/AppData/Roaming/npm/node_modules/@abhash-chakraborty/inyon-win32-x64/bin/inyon.exe")));
        assert!(!from_npm(Path::new("/h/.inyon/bin/inyon")));
        assert!(!from_npm(Path::new("/h/node_modules_old/inyon")));
    }

    #[test]
    fn the_engine_sits_next_to_the_program() {
        assert_eq!(engine_beside(Path::new("/h/.inyon/bin/inyon")), PathBuf::from("/h/.inyon/bin/inyon-engine"));
        assert_eq!(engine_beside(Path::new("C:/u/.inyon/bin/inyon.exe")), PathBuf::from("C:/u/.inyon/bin/inyon-engine.exe"));
    }
}
