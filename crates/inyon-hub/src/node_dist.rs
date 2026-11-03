//! What a machine downloads from its hub to run Inyon (node-dist.ts and pair-command.ts, removed
//! in 0.23): the inyon command for its system (/node/inyon), the engine an inyon from 0.12 to 0.22
//! asks for (/node/inyon-engine, a copy of the command since 0.23), and on Windows busybox-w32,
//! the small sh the hub's commands run with.
//!
//! The hub in the inyon program hands out the release of its own version from GitHub, checked
//! against that release's SHA256SUMS, so a machine never needs GitHub itself. A hub built in a
//! checkout (see hub_update::checkout) hands out the commands cargo built into apps/cli/dist
//! (INYON_CLI_DIST), and the release's files for a system with none there. Files are kept under
//! ~/.inyon/node-dist.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use crate::http::Body;
use axum::http::{header, HeaderValue, Response, StatusCode};
use futures_util::future::{FutureExt, LocalBoxFuture, Shared};
use inyon_core::hosts::http_error::{http, HubError, HubResult};
use inyon_core::platform::console;
use inyon_protocol::{compressed, release_files, INSTALL_URL, RELEASES_REPO, RELEASE_KEY, VERSION};
use sha2::{Digest, Sha256};

use crate::http::{hex, Req, Resp};
use crate::server::{raw, Route, Routes};

pub const INSTALL_SH: &str = include_str!("../assets/install.sh");
pub const INSTALL_PS1: &str = include_str!("../assets/install.ps1");
pub const HUB_UPDATE_PS1: &str = include_str!("../assets/hub-update.ps1");

/// The systems a hub hands an engine to: Linux against glibc, macOS and Windows x64.
pub const HUB_TARGETS: [&str; 5] = ["linux-x64", "linux-arm64", "darwin-x64", "darwin-arm64", "windows-x64"];
/// The systems the inyon command ships for: the hub's and Windows on ARM.
pub const CLI_TARGETS: [&str; 6] = ["linux-x64", "linux-arm64", "darwin-x64", "darwin-arm64", "windows-x64", "windows-arm64"];

pub fn engine_file(t: &str) -> String {
    release_files(t).engine
}
pub fn cli_file(t: &str) -> String {
    release_files(t).cli
}

/// What install.sh can ask for: the hub's systems but Windows.
fn unix() -> impl Iterator<Item = &'static str> {
    HUB_TARGETS.into_iter().filter(|t| !t.starts_with("windows"))
}

/// busybox-w32 FRP-6075, checked against frippery.org/files/busybox/SHA256SUM
fn busybox_pin(arch: &str) -> (&'static str, &'static str) {
    if arch == "arm64" {
        ("busybox-w64a-FRP-6075-g169694ebd.exe", "e67f873d19d58c535cc9f0c4965ffd622e19b7bab87e3da89cb2185fb54464d7")
    } else {
        ("busybox-w64u-FRP-6075-g169694ebd.exe", "6e263d154d8548d1eb936f65d1d8312c80df31c45974e48d6335e4dcc0f4f34c")
    }
}

/// `?arch=`, x64 when it isn't given.
pub fn arch_of(query: &Query) -> HubResult<String> {
    let arch = query.get("arch").unwrap_or_else(|| "x64".into());
    if arch != "x64" && arch != "arm64" {
        return Err(http(400, "arch must be x64 or arm64"));
    }
    Ok(arch)
}

pub struct Query(pub String);
impl Query {
    fn get(&self, k: &str) -> Option<String> {
        url::form_urlencoded::parse(self.0.as_bytes()).find(|(n, _)| n == k).map(|(_, v)| v.into_owned())
    }
}

/// The engine a request asks for (?os=linux&arch=arm64). Windows on ARM runs the x64 engine.
pub fn target_of(q: &Query) -> HubResult<String> {
    let os = q.get("os").unwrap_or_else(|| "windows".into());
    if os == "windows" {
        return Ok("windows-x64".into());
    }
    let t = format!("{os}-{}", arch_of(q)?);
    if !HUB_TARGETS.contains(&t.as_str()) {
        return Err(http(400, "os must be linux, darwin or windows"));
    }
    Ok(t)
}

/// The inyon command a request asks for: the same systems, plus Windows on ARM as its own build.
pub fn cli_target_of(q: &Query) -> HubResult<String> {
    let os = q.get("os").unwrap_or_else(|| "windows".into());
    let t = format!("{os}-{}", arch_of(q)?);
    if !CLI_TARGETS.contains(&t.as_str()) {
        return Err(http(400, "os must be linux, darwin or windows"));
    }
    Ok(t)
}

/// A checkout's tag: the commands cargo built, by name, size and time, so a new build gets new
/// copies once the hub restarts.
pub fn built_tag(cli_dist: &Path) -> String {
    let mut h = Sha256::new();
    for t in CLI_TARGETS {
        let name = cli_file(t);
        if let Ok(m) = std::fs::metadata(cli_dist.join(&name)) {
            let at = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
            h.update(format!("{name} {} {at}\n", m.len()).as_bytes());
        }
    }
    format!("checkout-{}", &hex(&h.finalize())[..12])
}

/// Where a Windows program says which subsystem it runs in: the PE header (at the offset in its DOS
/// header), its signature and file header, then 68 bytes into the optional header.
fn subsystem_at(exe: &[u8]) -> Option<usize> {
    let pe = u32::from_le_bytes(exe.get(0x3c..0x40)?.try_into().ok()?) as usize;
    (exe.get(..2)? == b"MZ" && exe.get(pe..pe + 4)? == b"PE\0\0").then_some(pe + 4 + 20 + 68).filter(|at| at + 2 <= exe.len())
}

/// The windowless copy of a Windows program, the way the release makes inyon-engine-windows-x64.exe
/// (scripts/engine-copies.sh): the subsystem goes from console (3) to windows (2), nothing else changes.
pub fn windowless(mut exe: Vec<u8>) -> Result<Vec<u8>, String> {
    let at = subsystem_at(&exe).ok_or("it isn't a Windows program")?;
    exe[at..at + 2].copy_from_slice(&2u16.to_le_bytes());
    Ok(exe)
}

type Building = Shared<LocalBoxFuture<'static, Result<PathBuf, HubError>>>;
type Sums = Shared<LocalBoxFuture<'static, Result<Signed, HubError>>>;

/// A release's SHA256SUMS, and SHA256SUMS.minisig, which signs it.
#[derive(Clone)]
pub struct Signed {
    pub sums: String,
    pub sig: String,
}

/// The downloads, for one hub.
pub struct Dist {
    /// the checkout this hub was built in, when it runs from it
    pub repo: Option<PathBuf>,
    pub out: PathBuf,
    pub cli_dist: PathBuf,
    tag: String,
    building: RefCell<HashMap<PathBuf, Building>>,
    sums: RefCell<Option<Sums>>,
}

thread_local! {
    static DIST: RefCell<Option<Rc<Dist>>> = const { RefCell::new(None) };
}

pub fn dist() -> Rc<Dist> {
    DIST.with(|d| d.borrow().clone()).expect("node-dist is set up")
}

/// `sha256(data)` as hex
pub fn digest(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

/// The line of a SHA256SUMS list for `name`.
pub fn sum_for(sums: &str, name: &str) -> Option<String> {
    sums.lines().find_map(|l| {
        let l = l.trim();
        let mut parts = l.splitn(2, char::is_whitespace);
        let sum = parts.next()?;
        let rest = parts.next()?.trim_start().trim_start_matches('*');
        let file = rest.split_whitespace().next().unwrap_or("");
        (file == name).then(|| sum.to_lowercase())
    })
}

/// Fetches a URL whole: its status and bytes.
pub type Get = Rc<dyn Fn(String) -> LocalBoxFuture<'static, Result<(u16, Vec<u8>), String>>>;

pub fn net() -> Get {
    Rc::new(|url: String| {
        Box::pin(async move {
            let r = inyon_engine::http::fetch("GET", url, vec![], None, std::time::Duration::from_secs(600)).await?;
            Ok((r.status, r.body))
        })
    })
}

/// Whether `base` is where real releases come from: the website, or the releases on GitHub. Only
/// somewhere else (a mock release in tests and the container runs) may INYON_SIGNING_KEY name
/// another key, as for `inyon update`, so an environment variable alone can't change the key for
/// the real thing.
/// The website's earlier address, which redirects every path to INSTALL_URL, still counts as the website.
const OLD_SITE_HOST: &str = "inyon.abhashchakraborty.tech";

fn official(base: &str) -> bool {
    let Ok(u) = url::Url::parse(&base.trim().to_ascii_lowercase()) else { return false };
    let host = u.host_str().unwrap_or("");
    let site = url::Url::parse(INSTALL_URL).ok().and_then(|s| s.host_str().map(str::to_owned)).unwrap_or_default();
    host == site
        || host == OLD_SITE_HOST
        || (matches!(host, "github.com" | "www.github.com") && u.path().starts_with(&format!("/{}/releases/", RELEASES_REPO.to_ascii_lowercase())))
}

/// The key the release at `base` must be signed with.
pub fn signing_key(base: &str) -> String {
    if !official(base) {
        if let Some(k) =
            std::env::var("INYON_SIGNING_KEY").ok().and_then(|t| t.lines().map(str::trim).rfind(|l| !l.is_empty() && !l.starts_with("untrusted comment:")).map(str::to_owned))
        {
            return k;
        }
    }
    RELEASE_KEY.to_owned()
}

/// The release's SHA256SUMS, refused unless SHA256SUMS.minisig signs it with `key`: the hub hands
/// out what this list names, and puts its hash in every "Add a machine" command. Only the hashed
/// kind of signature, which the release workflow makes, counts, and only the list of `version`.
pub async fn signed_sums(base: &str, key: &str, version: &str, get: Get) -> HubResult<String> {
    Ok(signed_list(base, key, version, get).await?.sums)
}

/// signed_sums, with the signature: machines check it themselves before an update (/node/release/).
/// Each release lists its APK as inyon-<version>.apk, the rule `inyon update` holds the website
/// to: a list without the one for `version` (this hub's) is another release's, an old copy on a
/// mirror or one caught mid-update, and nothing from it is handed out.
pub async fn signed_list(base: &str, key: &str, version: &str, get: Get) -> HubResult<Signed> {
    let (status, sums) = get(format!("{base}/SHA256SUMS")).await.map_err(HubError::plain)?;
    if !(200..300).contains(&status) {
        return Err(http(502, format!("Could not get the checksums of Inyon {VERSION} ({status})")));
    }
    let (status, sig) = get(format!("{base}/SHA256SUMS.minisig")).await.map_err(HubError::plain)?;
    if !(200..300).contains(&status) {
        return Err(http(502, format!("Could not get the signature of Inyon {VERSION}'s checksums ({status}), so nothing from that release is handed out")));
    }
    let sig = String::from_utf8_lossy(&sig).into_owned();
    let checked = (|| {
        let key = minisign_verify::PublicKey::from_base64(key.trim()).ok()?;
        let sig = minisign_verify::Signature::decode(&sig).ok()?;
        key.verify(&sums, &sig, false).ok()
    })();
    if checked.is_none() {
        return Err(http(502, format!("The checksums of Inyon {VERSION} at {base} aren't signed with Inyon's release key, so nothing from that release is handed out")));
    }
    let sums = String::from_utf8(sums).map_err(|_| http(502, format!("The checksums of Inyon {VERSION} aren't text")))?;
    let apk = format!("inyon-{version}.apk");
    if sum_for(&sums, &apk).is_none() {
        return Err(http(502, format!("The signed checksums at {base} aren't the ones for Inyon {version} (they don't list {apk}), so nothing from that release is handed out")));
    }
    Ok(Signed { sums, sig })
}

/// What a machine hears when it asks a hub that runs from a checkout for the signed list.
pub const NOT_SIGNED: &str = "This hub runs from a checkout, so what it hands out isn't signed by Inyon's release key.\n";

/// Release file `name`, checked against the release's SHA256SUMS. The gzipped copy is taken when
/// the release lists one and checked before it's unpacked; machines get the plain bytes either way.
pub async fn release_file(name: &str, sums: &str, base: &str, get: Get) -> HubResult<Vec<u8>> {
    let want = sum_for(sums, name).ok_or_else(|| http(502, format!("Inyon {VERSION} has no {name} on {RELEASES_REPO}")))?;
    let gz = sum_for(sums, &compressed(name));
    let asset = if gz.is_some() { compressed(name) } else { name.to_owned() };
    let (status, mut data) = get(format!("{base}/{asset}")).await.map_err(HubError::plain)?;
    if !(200..300).contains(&status) {
        return Err(http(502, format!("Could not download {asset} ({status})")));
    }
    if let Some(gz) = gz {
        if digest(&data) != gz {
            return Err(http(502, format!("{asset} did not match its release checksum")));
        }
        let mut out = Vec::new();
        std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(&data[..]), &mut out).map_err(|e| HubError::plain(e.to_string()))?;
        data = out;
    }
    if digest(&data) != want {
        return Err(http(502, format!("{name} did not match its release checksum")));
    }
    Ok(data)
}

/// gzip as Node's zlib writes it at level 6 (no name, no time, Unix), so a .gz made by either hub
/// is the same file.
pub fn gzip(data: &[u8]) -> std::io::Result<Vec<u8>> {
    use std::io::Write;
    let mut e = flate2::GzBuilder::new().operating_system(3).write(Vec::new(), flate2::Compression::new(6));
    e.write_all(data)?;
    e.finish()
}

fn rename(from: &Path, to: &Path) -> HubResult<()> {
    std::fs::rename(from, to).map_err(|e| HubError::plain(e.to_string()))
}

fn write(p: &Path, data: &[u8]) -> HubResult<()> {
    std::fs::write(p, data).map_err(|e| HubError::plain(e.to_string()))
}

impl Dist {
    pub fn new(repo: Option<PathBuf>, data_dir: &Path) -> Rc<Self> {
        let cli_dist = std::env::var("INYON_CLI_DIST").ok().filter(|s| !s.is_empty()).map(PathBuf::from).unwrap_or_else(|| repo.clone().unwrap_or_default().join("apps/cli/dist"));
        let tag = if repo.is_some() { built_tag(&cli_dist) } else { format!("v{VERSION}") };
        Rc::new(Dist { repo, out: data_dir.join("node-dist"), cli_dist, tag, building: RefCell::default(), sums: RefCell::default() })
    }

    pub fn install(self: &Rc<Self>) {
        DIST.with(|d| *d.borrow_mut() = Some(self.clone()));
        // the update script for a Windows machine set up before the rename (see hosts/node_update)
        inyon_core::hosts::node_update::node_updates().use_scripts(|platform, arch| {
            let (platform, arch) = (platform.to_owned(), arch.to_owned());
            Box::pin(async move { dist().update_script(&platform, &arch).await.map_err(|e| e.message) })
        });
    }

    pub fn tag(&self) -> &str {
        &self.tag
    }

    fn runs_from_source(&self) -> bool {
        self.repo.is_some()
    }

    /// Runs `make` once for `key`, however many ask for it meanwhile.
    async fn once(self: &Rc<Self>, key: PathBuf, make: impl FnOnce() -> LocalBoxFuture<'static, Result<PathBuf, HubError>>) -> HubResult<PathBuf> {
        let existing = self.building.borrow().get(&key).cloned();
        let fut = match existing {
            Some(f) => f,
            None => {
                let f = make().shared();
                self.building.borrow_mut().insert(key.clone(), f.clone());
                f
            }
        };
        let r = fut.await;
        self.building.borrow_mut().remove(&key);
        r
    }

    /// Earlier builds and downloads are never served again: only this hub's own tag and release
    /// version stay, and files from before 0.13 go with the rest. busybox isn't touched.
    pub fn prune_old(&self, dir: &Path) {
        let keep = [self.tag.clone(), format!("v{VERSION}")];
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let f = e.file_name().to_string_lossy().into_owned();
            // /^(?:(.*?)-)?(?:inyon|tether-node)-/
            let Some(caps) = inyon_core::re!(r"^(?:(.*?)-)?(?:inyon|tether-node)-").captures(&f) else { continue };
            let prefix = caps.get(1).map_or("", |m| m.as_str());
            if !keep.iter().any(|k| k == prefix) {
                let _ = std::fs::remove_file(dir.join(&f));
            }
        }
    }

    /// Where the hub finds the release of its own version (INYON_DOWNLOAD for a mirror).
    fn release_base() -> String {
        let base = std::env::var("INYON_DOWNLOAD").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| format!("https://github.com/{RELEASES_REPO}/releases/download/v{VERSION}"));
        base.trim_end_matches('/').to_owned()
    }

    async fn release_sums(self: &Rc<Self>) -> HubResult<String> {
        Ok(self.release_signed().await?.sums)
    }

    async fn release_signed(self: &Rc<Self>) -> HubResult<Signed> {
        let existing = self.sums.borrow().clone();
        let fut = existing.unwrap_or_else(|| {
            let f: LocalBoxFuture<'static, Result<Signed, HubError>> = Box::pin(async {
                let base = Self::release_base();
                signed_list(&base, &signing_key(&base), VERSION, net()).await
            });
            let f = f.shared();
            *self.sums.borrow_mut() = Some(f.clone());
            f
        });
        let r = fut.await;
        if r.is_err() {
            *self.sums.borrow_mut() = None;
        }
        r
    }

    /// The engine for `t`: a copy of a checkout's command, or the release's file, made once.
    pub async fn engine(self: &Rc<Self>, t: &str) -> HubResult<PathBuf> {
        let file = self.out.join(format!("{}-{}", self.tag, engine_file(t)));
        if file.exists() {
            return Ok(file);
        }
        let (me, t, f) = (self.clone(), t.to_owned(), file.clone());
        self.once(file, move || {
            Box::pin(async move {
                std::fs::create_dir_all(&me.out).map_err(|e| HubError::plain(e.to_string()))?;
                let tmp = PathBuf::from(format!("{}.tmp", f.display()));
                let data = match me.built_command(&t).filter(|_| me.runs_from_source()) {
                    Some(built) => {
                        let data = std::fs::read(&built).map_err(|e| HubError::plain(e.to_string()))?;
                        if t.starts_with("windows") {
                            windowless(data).map_err(|e| http(500, format!("Could not make the engine for {t} from {}: {e}", built.display())))?
                        } else {
                            data
                        }
                    }
                    None => release_file(&engine_file(&t), &me.release_sums().await?, &Self::release_base(), net()).await?,
                };
                write(&tmp, &data)?;
                rename(&tmp, &f)?;
                me.prune_old(&me.out);
                Ok(f)
            })
        })
        .await
    }

    /// The cargo-built command for `t` next to a checkout, if there is one.
    fn built_command(&self, t: &str) -> Option<PathBuf> {
        let f = self.cli_dist.join(cli_file(t));
        f.exists().then_some(f)
    }

    /// The inyon command for `t`: the release's, or a checkout's (see source_command).
    pub async fn command(self: &Rc<Self>, t: &str) -> HubResult<PathBuf> {
        if self.runs_from_source() {
            self.source_command(t).await
        } else {
            self.release_command(t).await
        }
    }

    async fn release_command(self: &Rc<Self>, t: &str) -> HubResult<PathBuf> {
        let file = self.out.join(format!("v{VERSION}-{}", cli_file(t)));
        if file.exists() {
            return Ok(file);
        }
        let (me, t, f) = (self.clone(), t.to_owned(), file.clone());
        self.once(file, move || {
            Box::pin(async move {
                std::fs::create_dir_all(&me.out).map_err(|e| HubError::plain(e.to_string()))?;
                let sums = me.release_sums().await?;
                let tmp = PathBuf::from(format!("{}.tmp", f.display()));
                write(&tmp, &release_file(&cli_file(&t), &sums, &Self::release_base(), net()).await?)?;
                rename(&tmp, &f)?;
                me.prune_old(&me.out);
                Ok(f)
            })
        })
        .await
    }

    /// A checkout's command for `t`: what cargo built into apps/cli/dist, or else the release's.
    async fn source_command(self: &Rc<Self>, t: &str) -> HubResult<PathBuf> {
        if let Some(built) = self.built_command(t) {
            return Ok(built);
        }
        self.release_command(t).await.map_err(|e| {
            http(503, format!("This hub runs from a checkout and has no inyon command for {t}: none in {}, and the release's failed ({}).", self.cli_dist.display(), e.message))
        })
    }

    /// The checksums of the Linux and macOS files, which install.sh checks its downloads against.
    pub async fn unix_sums(self: &Rc<Self>) -> HubResult<String> {
        let sums = if self.runs_from_source() { None } else { Some(self.release_sums().await?) };
        let mut lines = vec![];
        for t in unix() {
            for name in [cli_file(t), engine_file(t)] {
                let sum = match &sums {
                    Some(s) => sum_for(s, &name),
                    None => {
                        let file = if name == cli_file(t) { self.command(t).await? } else { self.engine(t).await? };
                        Some(self.sha256(&file).await?)
                    }
                };
                let sum = sum.ok_or_else(|| http(502, format!("Inyon {VERSION} has no {name} on {RELEASES_REPO}")))?;
                lines.push(format!("{sum}  {name}"));
            }
        }
        Ok(format!("{}\n", lines.join("\n")))
    }

    /// A file gzipped (the engine comes down to about half), once per build, off the hub's thread.
    async fn gzipped(self: &Rc<Self>, file: &Path) -> HubResult<PathBuf> {
        let gz = PathBuf::from(format!("{}.gz", file.display()));
        if gz.exists() {
            return Ok(gz);
        }
        let (src, out) = (file.to_owned(), gz.clone());
        self.once(gz, move || {
            Box::pin(async move {
                let tmp = PathBuf::from(format!("{}.tmp", out.display()));
                let t2 = tmp.clone();
                tokio::task::spawn_blocking(move || -> std::io::Result<()> { std::fs::write(&t2, gzip(&std::fs::read(&src)?)?) })
                    .await
                    .map_err(|e| HubError::plain(e.to_string()))?
                    .map_err(|e| HubError::plain(e.to_string()))?;
                rename(&tmp, &out)?;
                Ok(out)
            })
        })
        .await
    }

    /// sha256 of a build, so a download can be told from a cut-off or swapped one
    async fn sha256(&self, file: &Path) -> HubResult<String> {
        let out = PathBuf::from(format!("{}.sha256", file.display()));
        if let Ok(s) = std::fs::read_to_string(&out) {
            return Ok(s);
        }
        let f = file.to_owned();
        let sum = tokio::task::spawn_blocking(move || std::fs::read(&f).map(|d| digest(&d)))
            .await
            .map_err(|e| HubError::plain(e.to_string()))?
            .map_err(|e| HubError::plain(e.to_string()))?;
        write(&out, sum.as_bytes())?;
        Ok(sum)
    }

    async fn busybox(self: &Rc<Self>, arch: &str) -> HubResult<PathBuf> {
        let (file, sha) = busybox_pin(arch);
        let out = self.out.join(file);
        if out.exists() {
            return Ok(out);
        }
        let (me, o) = (self.clone(), out.clone());
        self.once(out, move || {
            Box::pin(async move {
                std::fs::create_dir_all(&me.out).map_err(|e| HubError::plain(e.to_string()))?;
                let (status, buf) = net()(format!("https://frippery.org/files/busybox/{file}")).await.map_err(HubError::plain)?;
                if !(200..300).contains(&status) {
                    return Err(http(502, format!("Could not download busybox-w32 ({status})")));
                }
                if digest(&buf) != sha {
                    return Err(http(502, "busybox-w32 download did not match its pinned checksum"));
                }
                write(&o, &buf)?;
                Ok(o)
            })
        })
        .await
    }

    /// what tether-node's own installer and update script ask for: the Windows program, whole
    async fn windows(self: &Rc<Self>) -> HubResult<PathBuf> {
        self.engine("windows-x64").await
    }

    /// The update script for a Windows machine set up before the rename to Inyon, with the
    /// checksum of the program this hub serves it filled in.
    pub async fn update_script(self: &Rc<Self>, platform: &str, arch: &str) -> HubResult<String> {
        if platform != "win32" {
            return Err(http(409, REPLACED.trim()));
        }
        let sum = self.sha256(&self.windows().await?).await?;
        inyon_core::hosts::node_update::fill_update_script(HUB_UPDATE_PS1, &sum, VERSION, if arch == "arm64" { "arm64" } else { "x64" }).map_err(HubError::plain)
    }

    /// The one-liners the "Add machine" sheet shows.
    pub async fn pair_commands(self: &Rc<Self>, hub: &str, token: &str) -> HubResult<(String, String)> {
        let install_sh = digest(INSTALL_SH.as_bytes());
        let sums = digest(self.unix_sums().await?.as_bytes());
        let windows = format!(
            "powershell -NoProfile -ExecutionPolicy Bypass -Command \"[Net.ServicePointManager]::SecurityProtocol=3072; & ([scriptblock]::Create((irm '{hub}/node/install.ps1'))) -Hub '{hub}' -Token '{token}'\""
        );
        let cert = if hub.starts_with("https:") { crate::tls::self_signed(&tls_cert()).map(|c| c.sha256) } else { None };
        Ok((unix_pair_command(hub, token, &install_sh, &sums, cert.as_deref()), windows))
    }

    /// Builds what machines download in the background, so adding one doesn't wait for it.
    pub fn prebuild(self: &Rc<Self>) {
        // hubs started by tests never hand out the program
        if std::env::var("NODE_ENV").as_deref() == Ok("test") {
            return;
        }
        let me = self.clone();
        inyon_core::platform::rt::spawn(async move {
            let r: HubResult<()> = async {
                if !me.runs_from_source() {
                    me.release_sums().await?;
                    return Ok(());
                }
                for t in unix().chain(["windows-x64"]).filter(|t| me.built_command(t).is_some()) {
                    let f = me.engine(t).await?;
                    me.sha256(&f).await?;
                    me.gzipped(&f).await?;
                }
                Ok(())
            }
            .await;
            if let Err(e) = r {
                console::error(&format!("inyon build for machines: {}", e.message));
            }
        });
    }
}

/// Whether a request nobody signed may make this hub build or download Inyon for `os` (mayPrepare
/// in node-dist.ts). The /node routes are open (install.sh, install.ps1 and `inyon update` on
/// older machines send no token), so without this anyone who reaches the hub could make it fetch
/// Inyon for every system from GitHub. What's already here
/// is always handed out; a new system is prepared only while an "Add a machine" command is waiting
/// to be used, or when a machine of that system is paired (an update), or one whose system isn't
/// known yet.
pub fn may_prepare(os: &str, configs: &[inyon_protocol::HostConfig], os_of: impl Fn(&str) -> Option<String>, now: u64) -> bool {
    use inyon_protocol::HostKind;
    let nodes: Vec<_> = configs.iter().filter(|c| c.kind == HostKind::Node).collect();
    let paired = |c: &inyon_protocol::HostConfig| c.paired_at.is_some_and(|p| p != 0);
    if nodes.iter().any(|c| !paired(c) && c.token_issued_at.is_some_and(|t| t != 0) && !inyon_core::hosts::registry::token_expired(c, now)) {
        return true;
    }
    nodes.iter().any(|c| paired(c) && system_of(os_of(&c.id).as_deref()).is_none_or(|o| o == os))
}

/// `uname -s` as a machine's probe reports it (Linux, Darwin, Windows_NT, MINGW64_NT…), as a target's system
fn system_of(uname: Option<&str>) -> Option<String> {
    let u = uname.unwrap_or_default().to_lowercase();
    if u.is_empty() {
        return None;
    }
    Some(if u.starts_with("linux") {
        "linux".into()
    } else if u.starts_with("darwin") {
        "darwin".into()
    } else if ["windows", "win32", "mingw", "msys", "cygwin"].iter().any(|p| u.starts_with(p)) {
        "windows".into()
    } else {
        u
    })
}

fn not_ready(t: &str) -> String {
    format!("This hub hasn't fetched Inyon for {t} yet. Open \"Add a machine\" on the hub, then run the command again.")
}

/// The first of `targets` that isn't here and that no machine may be waiting for. The list of
/// checksums builds every one, so each counts on its own: a paired Linux machine doesn't open macOS.
pub fn unprepared<'a>(targets: impl IntoIterator<Item = &'a str>, ready: impl Fn(&str) -> bool, may: impl Fn(&str) -> bool) -> Option<&'a str> {
    targets.into_iter().find(|t| !ready(t) && !may(t.split('-').next().unwrap_or_default()))
}

/// 503 unless `ready` (the file is here already) or a machine may be waiting for it (may_prepare).
fn gate(t: &str, ready: bool) -> HubResult<()> {
    let reg = inyon_core::hosts::registry::registry();
    let os = t.split('-').next().unwrap_or_default();
    if ready || may_prepare(os, &reg.configs(), |id| reg.get(id).and_then(|r| r.borrow().state.os.clone()), inyon_core::clock::now_ms()) {
        Ok(())
    } else {
        Err(http(503, not_ready(t)))
    }
}

impl Dist {
    /// here already, or quickly copied from what cargo built
    fn engine_ready(&self, t: &str) -> bool {
        self.out.join(format!("{}-{}", self.tag, engine_file(t))).exists() || (self.runs_from_source() && self.built_command(t).is_some())
    }
    fn command_ready(&self, t: &str) -> bool {
        self.out.join(format!("v{VERSION}-{}", cli_file(t))).exists() || (self.runs_from_source() && self.built_command(t).is_some())
    }
    /// The engine a request asks for, once gate() lets it through.
    async fn engine_for(self: &Rc<Self>, t: &str) -> HubResult<PathBuf> {
        gate(t, self.engine_ready(t))?;
        self.engine(t).await
    }
    async fn command_for(self: &Rc<Self>, t: &str) -> HubResult<PathBuf> {
        gate(t, self.command_ready(t))?;
        self.command(t).await
    }
    async fn windows_program(self: &Rc<Self>) -> HubResult<PathBuf> {
        gate("windows-x64", self.engine_ready("windows-x64"))?;
        self.windows().await
    }
    async fn busybox_for(self: &Rc<Self>, arch: &str) -> HubResult<PathBuf> {
        gate(&format!("windows-{arch}"), self.out.join(busybox_pin(arch).0).exists())?;
        self.busybox(arch).await
    }
    /// a checkout works the list out from every Unix file, some from the release, so it waits for a machine to be added
    async fn unix_sums_for(self: &Rc<Self>) -> HubResult<String> {
        if self.runs_from_source() {
            let reg = inyon_core::hosts::registry::registry();
            let (configs, now) = (reg.configs(), inyon_core::clock::now_ms());
            let may = |os: &str| may_prepare(os, &configs, |id| reg.get(id).and_then(|r| r.borrow().state.os.clone()), now);
            if let Some(t) = unprepared(unix(), |t| self.engine_ready(t), may) {
                return Err(http(503, not_ready(t)));
            }
        }
        self.unix_sums().await
    }
}

const REPLACED: &str = "Inyon replaced tether-node with the inyon program. Copy the \"Add a machine\" command from the hub again and run it on this machine.\n";

/// The Unix "Add a machine" one-liner (pair-command.ts). It checks install.sh against its sha256
/// before running it, and install.sh checks the hub's list of checksums against the one given
/// here, then the program against its line. The token travels in the environment.
pub fn unix_pair_command(hub: &str, token: &str, install_sh_sha256: &str, sums_sha256: &str, cert_sha256: Option<&str>) -> String {
    // a hub on a certificate it made itself: curl can't check it, which is fine for files whose
    // sha256 the command already has; `inyon join` trusts only the certificate with this sha256
    let cert = cert_sha256.map(|c| format!(" INYON_HUB_CERT={c}")).unwrap_or_default();
    let curl = if cert_sha256.is_some() { "curl -fsSLk" } else { "curl -fsSL" };
    let run = concat!(
        "f=$(mktemp) && CURL \"$1/node/install.sh\" -o \"$f\" && ",
        "if [ \"$( (sha256sum \"$f\" 2>/dev/null || shasum -a 256 \"$f\") | cut -c1-64)\" = \"$2\" ]; then sh \"$f\" \"$1\"; ",
        "else echo \"install.sh did not match its checksum; get a new command from the hub\" >&2; false; fi; s=$?; rm -f \"$f\"; exit $s"
    );
    let run = run.replace("CURL", curl);
    format!("INYON_TOKEN={token} INYON_SUMS_SHA256={sums_sha256}{cert} sh -c '{run}' inyon {hub} {install_sh_sha256}")
}

/// TLS_CERT, the certificate file the hub serves.
fn tls_cert() -> String {
    std::env::var("TLS_CERT").unwrap_or_default()
}

fn plain(status: u16, body: impl Into<String>) -> Resp {
    let mut r = Response::new(Body::from(body.into()));
    *r.status_mut() = StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("text/plain; charset=utf-8"));
    r
}

/// A file as a download, read as it's sent.
async fn binary(file: &Path, name: &str) -> HubResult<Resp> {
    let f = tokio::fs::File::open(file).await.map_err(|e| HubError::plain(e.to_string()))?;
    let stream = futures_util::stream::unfold(f, |mut f| async move {
        use tokio::io::AsyncReadExt;
        let mut buf = vec![0u8; 256 * 1024];
        match f.read(&mut buf).await {
            Ok(0) => None,
            Ok(n) => {
                buf.truncate(n);
                Some((Ok::<_, std::io::Error>(axum::body::Bytes::from(buf)), f))
            }
            Err(e) => Some((Err(e), f)),
        }
    });
    let mut r = Response::new(Body::from_stream(stream));
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("application/octet-stream"));
    if let Ok(v) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
        r.headers_mut().insert(header::CONTENT_DISPOSITION, v);
    }
    Ok(r)
}

/// plain-text errors: these are fetched by install scripts, not the app
fn route<F, Fut>(f: F) -> Route
where
    F: Fn(Rc<Dist>, Query) -> Fut + 'static,
    Fut: std::future::Future<Output = HubResult<Resp>> + 'static,
{
    let f = Rc::new(f);
    Route::Any(raw(move |r: Req| {
        let f = f.clone();
        async move {
            match f(dist(), Query(r.query.clone())).await {
                Ok(res) => res,
                Err(e) => plain(e.status.unwrap_or(500), format!("{}\n", e.message)),
            }
        }
    }))
}

pub fn add(routes: &mut Routes) {
    routes.add("/node/install.sh", route(|_, _| async { Ok(plain(200, INSTALL_SH)) }));
    routes.add("/node/install.ps1", route(|_, _| async { Ok(plain(200, INSTALL_PS1)) }));
    routes.add("/node/SHA256SUMS", route(|d, _| async move { Ok(plain(200, d.unix_sums_for().await?)) }));
    // the release's own list and its signature, as the release has them: a machine checks what it
    // updates to against Inyon's key, never against a checksum the hub (or someone in between) gives
    routes.add("/node/release/SHA256SUMS", route(|d, _| async move { Ok(if d.runs_from_source() { plain(404, NOT_SIGNED) } else { plain(200, d.release_signed().await?.sums) }) }));
    // the version the list is for, which a machine holds the list to (it must name inyon-<version>.apk)
    routes.add("/node/release/version", route(|d, _| async move { Ok(if d.runs_from_source() { plain(404, NOT_SIGNED) } else { plain(200, format!("{VERSION}\n")) }) }));
    routes.add(
        "/node/release/SHA256SUMS.minisig",
        route(|d, _| async move { Ok(if d.runs_from_source() { plain(404, NOT_SIGNED) } else { plain(200, d.release_signed().await?.sig) }) }),
    );
    // the certificate this hub serves, for a machine added with its sha256 (see tls::self_signed)
    routes.add(
        "/node/hub.crt",
        route(|_, _| async move {
            Ok(match crate::tls::served_pem(&tls_cert()) {
                Some(pem) => plain(200, pem),
                None => plain(404, "This hub has no certificate of its own.\n"),
            })
        }),
    );
    // the inyon command: what install.sh, install.ps1 and `inyon update` on a machine take first
    routes.add(
        "/node/inyon",
        route(|d, q| async move {
            let t = cli_target_of(&q)?;
            binary(&d.command_for(&t).await?, &cli_file(&t)).await
        }),
    );
    routes.add(
        "/node/inyon.gz",
        route(|d, q| async move {
            let t = cli_target_of(&q)?;
            let f = d.command_for(&t).await?;
            binary(&d.gzipped(&f).await?, &format!("{}.gz", cli_file(&t))).await
        }),
    );
    routes.add(
        "/node/inyon.sha256",
        route(|d, q| async move {
            let t = cli_target_of(&q)?;
            Ok(plain(200, d.sha256(&d.command_for(&t).await?).await?))
        }),
    );
    // the engine behind it, which the command fetches from here rather than from GitHub
    routes.add(
        "/node/inyon-engine",
        route(|d, q| async move {
            let t = target_of(&q)?;
            binary(&d.engine_for(&t).await?, &engine_file(&t)).await
        }),
    );
    routes.add(
        "/node/inyon-engine.gz",
        route(|d, q| async move {
            let t = target_of(&q)?;
            let f = d.engine_for(&t).await?;
            binary(&d.gzipped(&f).await?, &format!("{}.gz", engine_file(&t))).await
        }),
    );
    routes.add(
        "/node/inyon-engine.sha256",
        route(|d, q| async move {
            let t = target_of(&q)?;
            Ok(plain(200, d.sha256(&d.engine_for(&t).await?).await?))
        }),
    );
    routes.add("/node/busybox.exe", route(|d, q| async move { binary(&d.busybox_for(&arch_of(&q)?).await?, "busybox.exe").await }));
    // its pinned checksum, which install.ps1 checks the download against
    routes.add("/node/busybox.exe.sha256", route(|_, q| async move { Ok(plain(200, busybox_pin(&arch_of(&q)?).1)) }));
    // a Windows machine set up before the rename updates through these (see hub-update.ps1)
    routes.add("/node/tether-node.exe", route(|d, _| async move { binary(&d.windows_program().await?, "tether-node.exe").await }));
    routes.add(
        "/node/tether-node.exe.gz",
        route(|d, _| async move {
            let f = d.windows_program().await?;
            binary(&d.gzipped(&f).await?, "tether-node.exe.gz").await
        }),
    );
    routes.add("/node/tether-node.exe.sha256", route(|d, _| async move { Ok(plain(200, d.sha256(&d.windows_program().await?).await?)) }));
    // tether-node as a Bun script (install.sh before the rename) has no update to the program
    routes.add("/node/tether-node.ts", route(|_, _| async { Ok(plain(410, REPLACED)) }));
    routes.add("/node/tether-node.ts.sha256", route(|_, _| async { Ok(plain(410, REPLACED)) }));
}

#[cfg(test)]
#[path = "node_dist_tests.rs"]
mod tests;
