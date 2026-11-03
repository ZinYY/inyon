//! Find the Codex TUI that owns each thread writer lock on Windows. These are owner
//! queries only: never ask Restart Manager to stop or restart any process.

#[cfg(windows)]
pub fn threads() -> String {
    let Some(home) = std::env::var_os("CODEX_HOME").map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(|h| std::path::PathBuf::from(h).join(".codex"))) else { return String::new() };
    threads_at(&home)
}

#[cfg(windows)]
fn threads_at(home: &std::path::Path) -> String {
    use std::fmt::Write;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::RestartManager::{RmStartSession, RmEndSession, RmRegisterResources, RmGetList, RM_PROCESS_INFO};
    let Ok(files) = std::fs::read_dir(home.join("thread-writer-locks")) else { return String::new() };
    let mut out = String::new();
    for file in files.flatten() {
        let path = file.path();
        let Some(id) = path.file_stem().and_then(|s| s.to_str()) else { continue };
        if path.extension().and_then(|s| s.to_str()) != Some("lock") || id.len() != 36
            || !id.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-') { continue; }
        // Old lock files remain on disk. An exclusive read open cheaply skips those
        // with no live owner, so query cost depends on running sessions, not history.
        match std::fs::OpenOptions::new().read(true).share_mode(0).open(&path) {
            Ok(_) => continue,
            Err(e) if e.raw_os_error() == Some(32) => {}, // ERROR_SHARING_VIOLATION
            Err(_) => continue,
        }
        let mut handle = 0;
        let mut key = [0u16; 33];
        // Every successful session is closed even when a query fails.
        unsafe {
            if RmStartSession(&mut handle, 0, key.as_mut_ptr()) != 0 { continue; }
            let name: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            let names = [name.as_ptr()];
            if RmRegisterResources(handle, 1, names.as_ptr(), 0, std::ptr::null(), 0, std::ptr::null()) == 0 {
                let mut rows: Vec<RM_PROCESS_INFO> = vec![std::mem::zeroed(); 16];
                let mut needed = 0;
                let mut count = rows.len() as u32;
                let mut reason = 0;
                let mut result = RmGetList(handle, &mut needed, &mut count, rows.as_mut_ptr(), &mut reason);
                if result == 234 && needed <= 1024 {
                    rows.resize(needed as usize, std::mem::zeroed());
                    count = needed;
                    result = RmGetList(handle, &mut needed, &mut count, rows.as_mut_ptr(), &mut reason);
                }
                if result == 0 {
                    for row in rows.iter().take(count as usize) {
                        let _ = writeln!(out, "{} {id}", row.Process.dwProcessId);
                    }
                }
            }
            RmEndSession(handle);
        }
    }
    out
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn held_writer_lock_reports_its_owner_and_closed_locks_do_not() {
        let root = std::env::temp_dir().join(format!("inyon-codex-lock-{}-{}", std::process::id(), std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let locks = root.join("thread-writer-locks");
        std::fs::create_dir_all(&locks).unwrap();
        let id = "00000000-0000-0000-0000-000000000001";
        let held = std::fs::File::create(locks.join(format!("{id}.lock"))).unwrap();
        std::fs::write(locks.join("00000000-0000-0000-0000-000000000002.lock"), "").unwrap();
        assert_eq!(super::threads_at(&root).trim(), format!("{} {id}", std::process::id()));
        drop(held);
        assert_eq!(super::threads_at(&root), "");
        std::fs::remove_dir_all(root).unwrap();
    }
}
