//! Where the inyon program keeps itself and the node's state: the programs in ~/.inyon/bin,
//! pairings, keys and logs in ~/.inyon/node. Worked out on each use, so tests can point HOME at a
//! throwaway folder. (paths.ts)

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const NAME: &str = "inyon";
/// The engine's file name, next to `inyon` in ~/.inyon/bin (plus .exe on Windows).
pub const ENGINE: &str = "inyon-engine";

/// Values this process set for itself (INYON_SH once busybox is found), read before the
/// environment. Setting the real environment from a running multi-threaded program isn't safe.
static SET: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

/// Sets a variable for this process and the commands it starts.
pub fn set_env(name: &str, value: &str) {
    SET.lock().unwrap_or_else(|e| e.into_inner()).get_or_insert_with(HashMap::new).insert(name.to_string(), value.to_string());
}

/// What this process set with `set_env`.
pub fn set_vars() -> Vec<(String, String)> {
    SET.lock().unwrap_or_else(|e| e.into_inner()).as_ref().map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()).unwrap_or_default()
}

/// An INYON_* variable, or its TETHER_* name from before the rename to Inyon (this program was
/// tether-node then; installers, services and hubs set those). Empty counts as unset.
pub fn env_var(name: &str) -> Option<String> {
    if let Some(v) = SET.lock().unwrap_or_else(|e| e.into_inner()).as_ref().and_then(|m| m.get(name).cloned()) {
        return Some(v);
    }
    let old = name.strip_prefix("INYON_").map(|rest| format!("TETHER_{rest}"));
    std::env::var(name).ok().filter(|v| !v.is_empty()).or_else(|| old.and_then(|o| std::env::var(o).ok()).filter(|v| !v.is_empty()))
}

/// The user's home folder. On Windows that's USERPROFILE, where the installer puts ~/.inyon and
/// where the Bun engine (Node's os.homedir()) kept the node's keys and pairings, whatever HOME a
/// Git or MSYS shell sets; elsewhere HOME. The inyon command uses this too, so both read the same files.
pub fn home() -> PathBuf {
    home_from(cfg!(windows), |k| std::env::var_os(k))
}

fn home_from(windows: bool, var: impl Fn(&str) -> Option<std::ffi::OsString>) -> PathBuf {
    let order = if windows { ["USERPROFILE", "HOME"] } else { ["HOME", "USERPROFILE"] };
    order.into_iter().find_map(|k| var(k).filter(|h| !h.is_empty())).map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."))
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

/// The installed program, ~/.inyon/bin/inyon.
pub fn installed_binary() -> PathBuf {
    bin_dir().join(format!("{NAME}{}", exe_suffix()))
}

/// The file name of a program without its folder or .exe.
fn stem(p: &Path) -> String {
    let full = p.to_string_lossy();
    let name = full.rsplit(['/', '\\']).next().unwrap_or("");
    name.strip_suffix(".exe").or_else(|| name.strip_suffix(".EXE")).unwrap_or(name).to_string()
}

/// Whether this process was started under the engine's name (`inyon-engine`, or `tether-node`,
/// which a hub hands Windows machines set up before the rename): it then answers the engine's
/// commands, for whatever still runs it that way.
pub fn started_as_engine(argv0: &str) -> bool {
    let stem = stem(Path::new(argv0));
    stem.starts_with(ENGINE) || stem == "tether-node"
}

/// The inyon command this runs under: INYON_CLI (set by an inyon command that started this as a
/// program of its own), `inyon` next to a program called inyon-engine, or this program itself
/// when it is the inyon command. The command owns services and updates.
pub fn inyon_command() -> Option<PathBuf> {
    if let Some(given) = env_var("INYON_CLI").map(PathBuf::from).filter(|p| p.exists()) {
        return Some(given);
    }
    let exe = std::env::current_exe().ok()?;
    if stem(&exe).starts_with(ENGINE) {
        let beside = exe.with_file_name(format!("{NAME}{}", exe_suffix()));
        return beside.exists().then_some(beside);
    }
    Some(exe)
}

/// The three jobs this program runs: the phone link, the connection to a hub, the hub.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Link,
    Node,
    Hub,
}

/// What a started job writes with its pid and version: state.json (the link), node.json,
/// hub.json. The inyon command's update waits for it to name the new version.
pub fn running_file(role: Role) -> PathBuf {
    node_dir().join(match role {
        Role::Link => "state.json",
        Role::Node => "node.json",
        Role::Hub => "hub.json",
    })
}

/// Where a job writes its log when nothing else keeps it (launchd, nohup, a Windows task).
pub fn log_file(role: Role) -> PathBuf {
    node_dir().join(match role {
        Role::Link => "link.log",
        Role::Node => "node.log",
        Role::Hub => "hub.log",
    })
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
    // a junction needs no admin rights
    let mut c = std::process::Command::new("cmd");
    c.args(["/C", "mklink", "/J"]).arg(at).arg(target);
    crate::ops::hide_window_std(&mut c);
    if c.output().map(|o| o.status.success()).unwrap_or(false) && at.is_dir() {
        return Ok(());
    }
    std::os::windows::fs::symlink_dir(target, at)
}

/// Makes the node's folder, readable only by this user (it holds keys and pairings).
pub fn ensure_node_dir() -> std::io::Result<PathBuf> {
    let dir = node_dir();
    let made = !dir.exists();
    fs::create_dir_all(&dir)?;
    // as mkdirSync's mode: it counts for a folder made here, one that's there keeps its own
    #[cfg(unix)]
    if made {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
    }
    #[cfg(not(unix))]
    let _ = made;
    Ok(dir)
}

/// NAME=value lines, as the installers write them (`/^(\w+)=(.*)$/`, the value trimmed).
pub fn read_env_file(file: &Path) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Ok(text) = fs::read_to_string(file) else {
        return out;
    };
    for line in text.split('\n') {
        let line = line.strip_suffix('\r').unwrap_or(line);
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

/// The hub this machine connects to, if it was added to one.
pub fn saved_hub() -> Option<String> {
    env_var("INYON_HUB").or_else(|| saved_env().get("INYON_HUB").cloned()).map(|h| h.trim_end_matches('/').to_string()).filter(|h| !h.is_empty())
}

#[cfg(test)]
pub(crate) mod testing {
    use std::path::PathBuf;
    use std::sync::{Mutex, MutexGuard};

    /// Tests that point HOME somewhere run one at a time: the environment is the whole process's.
    static HOME_LOCK: Mutex<()> = Mutex::new(());

    pub struct Home {
        pub dir: PathBuf,
        old: [Option<std::ffi::OsString>; 2],
        _lock: MutexGuard<'static, ()>,
    }

    /// Both, so a test points the home somewhere on every system (Windows reads USERPROFILE).
    const VARS: [&str; 2] = ["HOME", "USERPROFILE"];

    impl Drop for Home {
        fn drop(&mut self) {
            // SAFETY: tests that touch HOME hold HOME_LOCK, so nothing reads it meanwhile
            unsafe {
                for (k, old) in VARS.iter().zip(&self.old) {
                    match old {
                        Some(h) => std::env::set_var(k, h),
                        None => std::env::remove_var(k),
                    }
                }
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// A throwaway home for one test.
    pub fn home(name: &str) -> Home {
        let lock = HOME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = std::env::temp_dir().join(format!("inyon-engine-{name}-{}-{}", std::process::id(), crate::now_ms()));
        std::fs::create_dir_all(&dir).unwrap();
        let old = VARS.map(std::env::var_os);
        for k in VARS {
            // SAFETY: as above
            unsafe { std::env::set_var(k, &dir) };
        }
        Home { dir, old, _lock: lock }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_finds_the_home_where_the_installer_and_the_bun_engine_did() {
        // a Git or MSYS shell sets HOME elsewhere; ~/.inyon with the keys and pairings is under USERPROFILE
        let vars = |k: &str| match k {
            "HOME" => Some("/c/Users/me/msys".into()),
            "USERPROFILE" => Some(r"C:\Users\me".into()),
            _ => None,
        };
        assert_eq!(home_from(true, vars), PathBuf::from(r"C:\Users\me"));
        assert_eq!(home_from(false, vars), PathBuf::from("/c/Users/me/msys"));
        // whichever is set, when the other isn't
        let only_home = |k: &str| (k == "HOME").then(|| "/home/me".into());
        assert_eq!(home_from(true, only_home), PathBuf::from("/home/me"));
    }

    #[test]
    fn tether_nodes_folder_moves_once_with_a_link_left_at_the_old_path() {
        let h = testing::home("paths-move");
        let old = h.dir.join(".tether-node");
        fs::create_dir_all(old.join("bin")).unwrap();
        fs::write(old.join("phones.json"), "{\"k\":1}").unwrap();
        assert_eq!(node_dir(), h.dir.join(".inyon").join("node"));
        assert_eq!(fs::read_to_string(h.dir.join(".inyon/node/phones.json")).unwrap(), "{\"k\":1}");
        #[cfg(unix)]
        assert!(fs::symlink_metadata(&old).unwrap().file_type().is_symlink());
        // what still points at the old path finds the same files
        assert_eq!(fs::read_to_string(old.join("phones.json")).unwrap(), "{\"k\":1}");
        assert_eq!(node_dir(), h.dir.join(".inyon").join("node"));
    }

    #[test]
    fn a_new_machine_gets_the_new_folder_and_nothing_is_made_until_written() {
        let h = testing::home("paths-new");
        assert_eq!(node_dir(), h.dir.join(".inyon").join("node"));
        assert!(!h.dir.join(".inyon").exists());
        assert!(!h.dir.join(".tether-node").exists());
    }

    #[test]
    fn the_hub_connections_saved_settings_are_read_under_either_name() {
        let h = testing::home("paths-env");
        let dir = h.dir.join(".tether-node");
        fs::create_dir_all(&dir).unwrap();
        // install.ps1 before the rename wrote .env; install.sh wrote env
        fs::write(dir.join(".env"), "TETHER_HUB=http://hub.lan:4777/\r\nTETHER_TOKEN=old\r\nTETHER_SH=C:\\bb.exe\r\n").unwrap();
        let e = saved_env();
        assert_eq!(e.get("INYON_HUB").unwrap(), "http://hub.lan:4777/");
        assert_eq!(e.get("INYON_TOKEN").unwrap(), "old");
        assert_eq!(e.get("INYON_SH").unwrap(), "C:\\bb.exe");
        assert_eq!(e.len(), 3);
        // a new name wins over the old one
        fs::write(node_dir().join("env"), "INYON_TOKEN=new\n").unwrap();
        assert_eq!(saved_env().get("INYON_TOKEN").unwrap(), "new");
    }

    #[test]
    fn the_engines_name_is_known_with_or_without_exe() {
        assert!(started_as_engine("/home/a/.inyon/bin/inyon-engine"));
        assert!(started_as_engine("C:\\Users\\a\\.inyon\\bin\\inyon-engine.exe"));
        assert!(started_as_engine("inyon-engine-linux-arm64"));
        assert!(!started_as_engine("/usr/bin/inyon"));
        assert!(!started_as_engine("inyon.exe"));
        // the name a hub hands it out under to a Windows machine from before the rename
        assert!(started_as_engine("C:\\Users\\a\\.tether-node\\tether-node.exe"));
        assert!(!started_as_engine("tether"));
    }
}
