//! Keeps ~/.inyon/uploads tidy on this machine by itself, so files sent from a phone or a hub
//! don't pile up when no hub is around to clean them: a computer paired only with a phone, or one
//! whose hub is gone. It follows the rules the hub last used here (~/.inyon/upload-policy, written
//! by the hub's cleanup), and otherwise deletes uploads after 7 days and keeps at most 1 GB. Files
//! an upload left half written (*.part, and a chunked upload's *.part.chunk) go after an hour. (tidy.ts)

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DAY: u64 = 86_400_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// 0: keep them, whatever their age
    pub days: u64,
    /// 0: no limit
    pub max_total_mb: u64,
}

pub fn policy(home: &Path) -> Policy {
    if let Ok(text) = fs::read_to_string(home.join(".inyon").join("upload-policy")) {
        let mut parts = text.split_whitespace().map(|p| p.parse::<f64>());
        if let (Some(Ok(days)), Some(Ok(max))) = (parts.next(), parts.next()) {
            if days >= 0.0 && max >= 0.0 {
                return Policy { days: days as u64, max_total_mb: max as u64 };
            }
        }
    }
    Policy { days: 7, max_total_mb: 1024 }
}

struct Entry {
    path: PathBuf,
    size: u64,
    mtime: u64,
}

fn mtime_ms(m: &fs::Metadata) -> u64 {
    m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_millis() as u64).unwrap_or(0)
}

fn files(dir: &Path, out: &mut Vec<Entry>) {
    let Ok(list) = fs::read_dir(dir) else { return };
    for d in list.flatten() {
        let path = d.path();
        let Ok(kind) = d.file_type() else { continue };
        if kind.is_dir() {
            files(&path, out);
        } else if kind.is_file() {
            if let Ok(st) = fs::metadata(&path) {
                out.push(Entry { path, size: st.len(), mtime: mtime_ms(&st) });
            }
        }
    }
}

fn drop_empty(dir: &Path, top: bool) -> bool {
    let mut empty = true;
    if let Ok(list) = fs::read_dir(dir) {
        for d in list.flatten() {
            let is_dir = d.file_type().is_ok_and(|t| t.is_dir());
            if !(is_dir && drop_empty(&d.path(), false)) {
                empty = false;
            }
        }
    }
    if empty && !top && fs::remove_dir(dir).is_err() {
        return false;
    }
    empty
}

/// Deletes what the policy says should go; returns how many files went.
pub fn tidy_uploads(home: &Path, p: Policy, now: u64) -> usize {
    let dir = home.join(".inyon").join("uploads");
    if !dir.exists() {
        return 0;
    }
    let mut gone = 0;
    let mut remove = |e: &Entry| {
        let ok = fs::remove_file(&e.path).is_ok();
        if ok {
            gone += 1;
        }
        ok
    };
    let mut all = Vec::new();
    files(&dir, &mut all);
    let part = |e: &Entry| {
        let p = e.path.to_string_lossy();
        p.ends_with(".part") || p.ends_with(".part.chunk")
    };
    let mut all: Vec<Entry> = all
        .into_iter()
        .filter(|e| {
            let age = now.saturating_sub(e.mtime);
            let old = (p.days > 0 && age > p.days * DAY) || (part(e) && age > 3_600_000);
            !(old && remove(e))
        })
        .collect();
    if p.max_total_mb > 0 {
        // oldest first, but never a file still being written
        all.retain(|e| !part(e));
        all.sort_by_key(|e| e.mtime);
        let mut total: u64 = all.iter().map(|e| e.size).sum();
        for e in &all {
            if total <= p.max_total_mb * 1024 * 1024 {
                break;
            }
            if remove(e) {
                total -= e.size;
            }
        }
    }
    drop_empty(&dir, true);
    gone
}

/// Tidies a minute after start, then every six hours, for as long as this inyon runs.
pub fn start_tidying(home: PathBuf) {
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(60));
        loop {
            let now = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
            tidy_uploads(&home, policy(&home), now);
            std::thread::sleep(Duration::from_secs(6 * 3600));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home(name: &str) -> PathBuf {
        let h = std::env::temp_dir().join(format!("inyon-tidy-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&h);
        fs::create_dir_all(&h).unwrap();
        h
    }

    fn file(h: &Path, rel: &str, kb: usize, age_ms: u64) -> PathBuf {
        let p = h.join(".inyon").join("uploads").join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, vec![0u8; kb * 1024]).unwrap();
        let t = SystemTime::now() - Duration::from_millis(age_ms);
        fs::File::options().write(true).open(&p).unwrap().set_modified(t).unwrap();
        p
    }

    fn now() -> u64 {
        crate::now_ms()
    }

    #[test]
    fn with_no_hub_around_old_and_half_written_uploads_go_after_a_week_and_an_hour() {
        let h = home("default");
        let old = file(&h, "a/old.png", 1, 8 * DAY);
        let cut = file(&h, "b/cut.mp4.part", 1, 2 * 3_600_000);
        let going = file(&h, "b/going.mp4.part", 1, 60_000);
        let chunk = file(&h, "b/cut.mp4.part.chunk", 1, 2 * 3_600_000);
        let keep = file(&h, "b/new.png", 1, DAY);
        assert_eq!(tidy_uploads(&h, policy(&h), now()), 3);
        assert_eq!([&old, &cut, &going, &chunk, &keep].map(|p| p.exists()), [false, false, true, false, true]);
        assert!(!h.join(".inyon/uploads/a").exists());
        fs::remove_dir_all(&h).unwrap();
    }

    #[test]
    fn it_follows_the_rules_the_hub_last_used_here() {
        let h = home("policy");
        fs::create_dir_all(h.join(".inyon")).unwrap();
        fs::write(h.join(".inyon/upload-policy"), "0 1\n").unwrap();
        assert_eq!(policy(&h), Policy { days: 0, max_total_mb: 1 });
        let ancient = file(&h, "a/one.bin", 600, 40 * DAY);
        let mid = file(&h, "a/two.bin", 600, 20 * DAY);
        let recent = file(&h, "a/three.bin", 300, DAY);
        tidy_uploads(&h, policy(&h), now());
        // never for their age, but past 1 MB the oldest go first
        assert_eq!([&ancient, &mid, &recent].map(|p| p.exists()), [false, true, true]);
        fs::remove_dir_all(&h).unwrap();
    }
}
