//! Port of env.ts and legacy.ts: the hub process's setup. Records as files under ~/.inyon (or
//! INYON_DATA_DIR), this machine as the "local" host, machines reached as processes or through
//! paired nodes. Inyon was called Tether, and what a machine kept from then still works: a
//! TETHER_* variable counts as its INYON_* name, and ~/.tether moves to ~/.inyon once.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use inyon_core::hosts::host::{set_host_backends, HostBackend};
use inyon_core::hosts::node_backend::node_backend;
use inyon_protocol::{HostConfig, HostKind};

use crate::fs_store::FsStore;
use crate::process::ProcessBackend;

/// The environment as the hub reads it: every TETHER_* variable also under its INYON_* name, unless
/// that is set already (legacy.ts does this to process.env, so children see it too).
#[derive(Debug, Clone, Default)]
pub struct HubEnv {
    vars: BTreeMap<String, String>,
    /// INYON_* names filled in from TETHER_* ones, which commands the hub runs get as well
    added: Vec<(String, String)>,
}

impl HubEnv {
    pub fn from_vars(vars: impl IntoIterator<Item = (String, String)>) -> HubEnv {
        let mut vars: BTreeMap<String, String> = vars.into_iter().collect();
        let mut added = vec![];
        let old: Vec<(String, String)> = vars.iter().filter_map(|(k, v)| k.strip_prefix("TETHER_").map(|rest| (format!("INYON_{rest}"), v.clone()))).collect();
        for (k, v) in old {
            if !vars.contains_key(&k) {
                vars.insert(k.clone(), v.clone());
                added.push((k, v));
            }
        }
        HubEnv { vars, added }
    }

    /// This process's environment.
    pub fn current() -> HubEnv {
        Self::from_vars(std::env::vars_os().filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?))))
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.vars.get(key).map(String::as_str)
    }

    /// set and not empty, as `process.env.X ||` reads it
    fn nonempty(&self, key: &str) -> Option<&str> {
        self.get(key).filter(|v| !v.is_empty())
    }

    pub fn added(&self) -> &[(String, String)] {
        &self.added
    }

    fn home(&self) -> PathBuf {
        PathBuf::from(self.nonempty("HOME").unwrap_or("/"))
    }

    /// `DATA_DIR`: INYON_DATA_DIR, else ~/.inyon
    pub fn data_dir(&self) -> PathBuf {
        match self.nonempty("INYON_DATA_DIR") {
            Some(d) => PathBuf::from(d),
            None => self.home().join(".inyon"),
        }
    }

    /// INYON_DEBUG, for the watcher's extra logging
    pub fn debug(&self) -> bool {
        self.nonempty("INYON_DEBUG").is_some()
    }

    /// `PORT` (4777 when unset)
    pub fn port(&self) -> u16 {
        self.nonempty("PORT").and_then(|p| p.parse().ok()).unwrap_or(4777)
    }

    /// `BIND` (127.0.0.1 when unset)
    pub fn bind(&self) -> String {
        self.nonempty("BIND").unwrap_or("127.0.0.1").to_owned()
    }
}

/// Moves ~/.tether to ~/.inyon once and leaves a link at the old path, so hooks, status lines and
/// services still pointing there keep working until they're rewritten. Does nothing when the new
/// folder exists already or the old one is a link (moved before). A move that fails is an error,
/// as the TS's `renameSync` throws: the hub mustn't start on an empty ~/.inyon beside the old one.
pub fn move_old_folder(home: &Path) -> std::io::Result<()> {
    let old = home.join(".tether");
    let dir = home.join(".inyon");
    match std::fs::symlink_metadata(&old) {
        Ok(m) if m.file_type().is_symlink() => return Ok(()),
        Ok(_) => {}
        Err(_) => return Ok(()),
    }
    if dir.exists() {
        return Ok(());
    }
    std::fs::rename(&old, &dir)?;
    #[cfg(unix)]
    let _ = std::os::unix::fs::symlink(&dir, &old);
    Ok(())
}

/// Sets the core up as the hub process (env.ts), on the core's thread: the store under the data
/// directory, this machine as "local", paired nodes through their links and every other machine
/// as a process. Returns the data directory.
pub fn install_hub(env: &HubEnv, hostname: &str) -> std::io::Result<PathBuf> {
    if env.nonempty("INYON_DATA_DIR").is_none() {
        move_old_folder(&env.home())?;
    }
    let dir = env.data_dir();
    inyon_core::platform::store::use_store(Rc::new(FsStore::new(&dir)?));
    let cm = dir.join("cm");
    FsStore::new(&cm)?;
    let local: HostConfig = serde_json::from_value(serde_json::json!({ "id": "local", "name": hostname, "kind": "local" })).expect("a host config");
    inyon_core::config::set_local_host(Some(local));
    inyon_core::platform::console::set_debug(env.debug());
    let processes: Rc<dyn HostBackend> = ProcessBackend::new(cm, env.added().to_vec());
    let nodes = node_backend();
    set_host_backends(move |c| if c.kind == HostKind::Node { nodes.clone() } else { processes.clone() });
    Ok(dir)
}
