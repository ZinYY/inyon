// The product's name and addresses, and release numbers for "Update available" checks: the
// constants and pure functions of packages/protocol/src/versions.ts.

use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use ts_rs::TS;

/// Inyon's version, the same for the app, the hub and the inyon program (the root package.json's).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// What the product is called, everywhere a person reads it.
pub const PRODUCT: &str = "Inyon";
/// The lowercase form, for folders, packages, services and storage keys.
pub const SLUG: &str = "inyon";
/// The name it had before, which migration code still reads (folders, env vars, storage keys).
pub const OLD_SLUG: &str = "tether";
pub const SOURCE_REPO: &str = "Abhash-Chakraborty/inyon";
/// The download page (static, on GitHub Pages at its own domain).
pub const WEB_URL: &str = "https://inyon.tech/";
/// The short address the one-line installers are typed with: …/get.sh and …/get.ps1
pub const INSTALL_URL: &str = "https://inyon.tech";
/// The Android application id.
pub const APP_ID: &str = "tech.abhashchakraborty.inyon";
/// The repository every release is published on.
pub const RELEASES_REPO: &str = SOURCE_REPO;
/// The minisign key every release's SHA256SUMS is signed with (key id 0F27F489FF0BBD53).
pub const RELEASE_KEY: &str = "RWRTvQv/ifQnD3jarY1oarS0AKdhogH+mdgn/o0NzT/Ro8L43nFqqfp6";
/// The website's copy of the newest release.
pub const DOWNLOAD_URL: &str = "https://inyon.tech/download";
/// What that copy is: {"version":"0.14.0","tag":"v0.14.0","date":…,"files":[{name,size,sha256}…]}
pub const LATEST_URL: &str = "https://inyon.tech/download/latest.json";
/// The engine's name once installed, next to `inyon` in ~/.inyon/bin (plus .exe on Windows).
pub const ENGINE: &str = "inyon-engine";

/// The files a release attaches for one system ("linux-x64", "windows-x64").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseFiles {
    pub cli: String,
    pub engine: String,
}

/// The release file names for one system. Updaters from 0.11 on depend on these names.
pub fn release_files(system: &str) -> ReleaseFiles {
    let exe = if system.starts_with("windows") { ".exe" } else { "" };
    ReleaseFiles { cli: format!("inyon-{system}{exe}"), engine: format!("inyon-engine-{system}{exe}") }
}

/// The gzipped copy of a release file; taken first when SHA256SUMS lists it.
pub fn compressed(file: &str) -> String {
    format!("{file}.gz")
}

/// A release number as [major, minor, patch]: "0.6.10" and "v0.6.10" both work. A prerelease
/// ("0.7.0-rc.1") or anything else gives None, so it never counts as an update.
pub fn parse_release(v: &str) -> Option<[u64; 3]> {
    let v = v.trim();
    let v = v.strip_prefix('v').unwrap_or(v);
    let mut parts = v.split('.');
    let mut out = [0u64; 3];
    for slot in &mut out {
        let p = parts.next()?;
        if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        // JavaScript's Number() never fails on digits; a huge one only loses precision
        *slot = p.parse().unwrap_or(u64::MAX);
    }
    parts.next().is_none().then_some(out)
}

/// Whether release `next` is newer than the running version `current`. A build between releases
/// ("0.6.3-build.41", from main after 0.6.3) counts as the release it's built on; a test build of
/// a release ("0.19.0-rc.1") comes before it, so 0.19.0 is an update for it.
pub fn newer_release(next: &str, current: &str) -> bool {
    let current = current.trim();
    let base = current.find(['-', '+']).map_or(current, |i| &current[..i]);
    match (parse_release(next), parse_release(base)) {
        (Some(a), Some(b)) if a != b => a > b,
        (Some(_), Some(_)) => test_build(current),
        _ => false,
    }
}

/// A signed test build of a release, published as a GitHub pre-release: "0.19.0-rc.1"
/// (`TEST_BUILD`, `/^v?\d+\.\d+\.\d+-rc\.\d+$/`).
pub fn test_build(v: &str) -> bool {
    let v = v.strip_prefix('v').unwrap_or(v);
    let Some((release, rc)) = v.split_once("-rc.") else { return false };
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let parts: Vec<&str> = release.split('.').collect();
    parts.len() == 3 && parts.iter().all(|p| digits(p)) && digits(rc)
}

/// The newest release among `versions` that is newer than `current`, if any.
pub fn newest_release<'a>(versions: impl IntoIterator<Item = &'a str>, current: &str) -> Option<String> {
    let mut best: Option<&str> = None;
    for v in versions {
        if newer_release(v, best.unwrap_or(current)) {
            best = Some(v);
        }
    }
    best.map(|b| b.strip_prefix('v').unwrap_or(b).to_owned())
}

/// One file of the newest release, as latest.json lists it.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
pub struct ReleaseFile {
    pub name: String,
    pub size: Option<u64>,
    pub sha256: Option<String>,
}
