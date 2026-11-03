//! Where inyon keeps itself and the node's state: the programs in ~/.inyon/bin, pairings, keys
//! and logs in ~/.inyon/node. Worked out on each use, so tests can point HOME at a throwaway folder.

use std::collections::BTreeMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

pub const NAME: &str = "inyon";
pub const ENGINE: &str = "inyon-engine";

/// The user's home folder, as the engine works it out: USERPROFILE on Windows, HOME elsewhere
/// (what a test sets).
pub fn home() -> PathBuf {
    inyon_engine::paths::home()
}

/// Where the hub keeps its records (hub.env, setup-code): INYON_DATA_DIR, else ~/.inyon.
pub fn data_dir() -> PathBuf {
    env::var_os("INYON_DATA_DIR").filter(|d| !d.is_empty()).map(PathBuf::from).unwrap_or_else(|| home().join(".inyon"))
}

pub fn bin_dir() -> PathBuf {
    home().join(".inyon").join("bin")
}

pub const fn exe_suffix() -> &'static str {
    if cfg!(windows) {
        ".exe"
    } else {
        ""
    }
}

/// The installed command-line program, ~/.inyon/bin/inyon.
pub fn installed_binary() -> PathBuf {
    bin_dir().join(format!("{NAME}{}", exe_suffix()))
}

/// The engine next to it, ~/.inyon/bin/inyon-engine.
pub fn installed_engine() -> PathBuf {
    bin_dir().join(format!("{ENGINE}{}", exe_suffix()))
}

/// The folder before the rename to Inyon, when this program was tether-node.
pub fn old_dir() -> PathBuf {
    home().join(".tether-node")
}

fn new_dir() -> PathBuf {
    home().join(".inyon").join("node")
}

/// The node's folder. A machine set up as tether-node has ~/.tether-node: it moves to
/// ~/.inyon/node the first time, with a link left at the old path for whatever still points
/// there. Windows won't move a folder while a program in it runs; the old folder is used as it
/// is until it can be moved.
pub fn node_dir() -> PathBuf {
    let dir = new_dir();
    if dir.exists() {
        return dir;
    }
    let old = old_dir();
    match fs::symlink_metadata(&old) {
        Ok(m) if m.file_type().is_symlink() => return dir,
        Ok(_) => {}
        Err(_) => return dir,
    }
    if let Some(parent) = dir.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if fs::rename(&old, &dir).is_err() {
        return old;
    }
    let _ = link_dir(&dir, &old);
    dir
}

#[cfg(unix)]
fn link_dir(target: &Path, at: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(target, at)
}

#[cfg(windows)]
fn link_dir(target: &Path, at: &Path) -> std::io::Result<()> {
    // a junction needs no admin rights; a directory symlink wants them (or Developer Mode),
    // so it's only the fallback
    let mut c = std::process::Command::new("cmd");
    c.args(["/C", "mklink", "/J"]).arg(at).arg(target);
    if crate::sys::quiet(&mut c).output().map(|o| o.status.success()).unwrap_or(false) && at.is_dir() {
        return Ok(());
    }
    std::os::windows::fs::symlink_dir(target, at)
}

/// Makes the node's folder, readable only by this user (it holds keys and pairings).
pub fn ensure_node_dir() -> std::io::Result<PathBuf> {
    let dir = node_dir();
    fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    }
    Ok(dir)
}

/// NAME=value lines, as the installers write them.
pub fn read_env_file(file: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Ok(text) = fs::read_to_string(file) else {
        return out;
    };
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        if !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            out.insert(k.to_string(), v.trim().to_string());
        }
    }
    out
}

/// What the installers saved for the hub connection: env (Linux and macOS) or .env (Windows) in
/// the node's folder, with names from before the rename read as their INYON_* names.
pub fn saved_env() -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let dir = node_dir();
    for name in [".env", "env"] {
        for (k, v) in read_env_file(&dir.join(name)) {
            let key = match k.strip_prefix("TETHER_") {
                Some(rest) => format!("INYON_{rest}"),
                None => k.clone(),
            };
            if k.starts_with("INYON_") || !out.contains_key(&key) {
                out.insert(key, v);
            }
        }
    }
    out
}

/// An INYON_* variable, or its TETHER_* name from before the rename.
pub fn env_var(name: &str) -> Option<String> {
    env::var(name).ok().or_else(|| env::var(format!("TETHER_{}", name.trim_start_matches("INYON_"))).ok()).filter(|v| !v.is_empty())
}

/// The hub this machine connects to, if it was added to one.
pub fn saved_hub() -> Option<String> {
    env_var("INYON_HUB").or_else(|| saved_env().get("INYON_HUB").cloned()).map(|h| h.trim_end_matches('/').to_string()).filter(|h| !h.is_empty())
}

/// The file name of a path as text (a program's name, say).
pub fn basename(p: &str) -> &str {
    p.rsplit(['/', '\\']).next().unwrap_or(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_files_read_name_value_lines() {
        let dir = std::env::temp_dir().join(format!("inyon-paths-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let f = dir.join("env");
        fs::write(&f, "INYON_HUB=https://hub\nTETHER_TOKEN=abc \n# note\nbad line\n").unwrap();
        let e = read_env_file(&f);
        assert_eq!(e.get("INYON_HUB").unwrap(), "https://hub");
        assert_eq!(e.get("TETHER_TOKEN").unwrap(), "abc");
        assert_eq!(e.len(), 2);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn basenames() {
        assert_eq!(basename("/a/b/inyon"), "inyon");
        assert_eq!(basename("C:\\x\\inyon.exe"), "inyon.exe");
        assert_eq!(basename("inyon"), "inyon");
    }
}
