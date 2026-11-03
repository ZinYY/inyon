//! Port of platform/fs-store.ts: the hub's records, one JSON file per key under the data
//! directory, readable only by you.

use std::fs;
use std::path::{Path, PathBuf};

use inyon_core::platform::store::Store;
use serde_json::Value;

pub struct FsStore {
    dir: PathBuf,
}

#[cfg(unix)]
fn private_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new().recursive(true).mode(0o700).create(path)
}

#[cfg(not(unix))]
fn private_dir(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)
}

#[cfg(unix)]
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    f.write_all(text.as_bytes())
}

#[cfg(not(unix))]
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    fs::write(path, text)
}

impl FsStore {
    /// The store in `dir`, made (0700) if it isn't there.
    pub fn new(dir: impl Into<PathBuf>) -> std::io::Result<FsStore> {
        let dir = dir.into();
        private_dir(&dir)?;
        Ok(FsStore { dir })
    }

    fn file(&self, key: &str) -> PathBuf {
        self.dir.join(format!("{key}.json"))
    }
}

impl Store for FsStore {
    fn get(&self, key: &str) -> Option<Value> {
        fs::read_to_string(self.file(key)).ok().and_then(|t| serde_json::from_str(&t).ok())
    }

    fn set(&self, key: &str, value: &Value) -> Result<(), String> {
        let f = self.file(key);
        if let Some(parent) = f.parent() {
            private_dir(parent).map_err(|e| e.to_string())?;
        }
        // write then rename, so a crash never leaves half a file
        let tmp = PathBuf::from(format!("{}.{}.tmp", f.display(), std::process::id()));
        write_private(&tmp, &inyon_core::js::json::stringify(value)).map_err(|e| e.to_string())?;
        fs::rename(&tmp, &f).map_err(|e| e.to_string())
    }

    fn del(&self, key: &str) {
        let _ = fs::remove_file(self.file(key));
    }

    fn keys(&self, prefix: &str) -> Vec<String> {
        // path.dirname(`${prefix}x`): the folder the prefix points into
        let probe = format!("{prefix}x");
        let sub = match probe.rfind('/') {
            Some(0) => "/".to_owned(),
            Some(i) => probe[..i].to_owned(),
            None => ".".to_owned(),
        };
        let Ok(entries) = fs::read_dir(self.dir.join(&sub)) else { return vec![] };
        let mut out = vec![];
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            let Some(stem) = name.strip_suffix(".json") else { continue };
            let key = if sub == "." { stem.to_owned() } else { format!("{sub}/{stem}") };
            if key.starts_with(prefix) {
                out.push(key);
            }
        }
        out
    }
}
