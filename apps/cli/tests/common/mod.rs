//! What the service and update tests share: a throwaway HOME, fake programs on PATH that record
//! what they were asked, and a fake process table. Nothing real is installed, started or stopped.

#![allow(dead_code)]

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};

use inyon_cli::service::Sys;
use inyon_cli::sys::{Platform, Proc, Signal};

/// HOME and PATH are process-wide, so tests in one binary take turns.
pub fn lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(|e| e.into_inner())
}

pub struct Home {
    pub dir: PathBuf,
    pub fakes: PathBuf,
    pub log: PathBuf,
    pub crontab: PathBuf,
    pub procs: PathBuf,
}

impl Home {
    /// A fresh home with `fakes` on PATH (and nothing else).
    pub fn new(fakes: &[&str]) -> Home {
        let dir = std::env::temp_dir().join(format!("inyon-test-{}-{}", std::process::id(), unique()));
        let fakes_dir = dir.join("fakes");
        fs::create_dir_all(&fakes_dir).unwrap();
        let h = Home { log: dir.join("calls.log"), crontab: dir.join("crontab"), procs: dir.join("procs"), fakes: fakes_dir, dir };
        for name in fakes {
            h.fake(name);
        }
        std::env::set_var("HOME", &h.dir);
        // what Windows reads as the home
        std::env::set_var("USERPROFILE", &h.dir);
        std::env::set_var("PATH", &h.fakes);
        std::env::set_var("FAKE_LOG", &h.log);
        std::env::set_var("FAKE_CRONTAB", &h.crontab);
        std::env::set_var("FAKE_PROCS", &h.procs);
        std::env::set_var("FAKE_NEXT_PID", h.dir.join("next-pid"));
        std::env::remove_var("FAKE_REFUSE");
        std::env::remove_var("INYON_HUB");
        std::env::remove_var("INYON_DOWNLOAD");
        std::env::remove_var("INYON_SIGNING_KEY");
        // and the names from before the rename, which the program reads as well
        std::env::remove_var("TETHER_DOWNLOAD");
        std::env::remove_var("TETHER_SIGNING_KEY");
        h
    }

    /// A program on PATH that records its arguments. `crontab` keeps a crontab, `sh` starts a
    /// fake job and writes its pid.
    pub fn fake(&self, name: &str) {
        let record = "printf '%s' \"${0##*/}\" >> \"$FAKE_LOG\"; for a in \"$@\"; do printf '\\037%s' \"$a\" >> \"$FAKE_LOG\"; done; printf '\\035' >> \"$FAKE_LOG\"";
        let body = match name {
            "crontab" => format!(
                "#!/bin/sh\nPATH=/usr/bin:/bin\n{record}\nif [ \"$1\" = -l ]; then cat \"$FAKE_CRONTAB\" 2>/dev/null || {{ echo 'no crontab for me' >&2; exit 1; }}; exit 0; fi\nif [ \"$1\" = - ]; then cat > \"$FAKE_CRONTAB\"; exit 0; fi\nexit 1\n"
            ),
            "sh" => format!(
                "#!/bin/sh\nPATH=/usr/bin:/bin\n{record}\ncmd=\"$2\"\npid=$(cat \"$FAKE_NEXT_PID\" 2>/dev/null || echo 4242)\necho $((pid+1)) > \"$FAKE_NEXT_PID\"\npidfile=$(printf '%s' \"$cmd\" | sed -n \"s/.*echo \\$! > '\\([^']*\\)'.*/\\1/p\")\nargs=$(printf '%s' \"$cmd\" | sed -n 's/^nohup \\(.*\\) >> .*/\\1/p')\nprintf '%s\\n' \"$pid\" > \"$pidfile\"\nprintf '%s\\t%s\\n' \"$pid\" \"$args\" >> \"$FAKE_PROCS\"\n"
            ),
            "systemctl" => format!("#!/bin/sh\nPATH=/usr/bin:/bin\n{record}\nif [ -n \"$FAKE_REFUSE\" ] && [ \"$2\" = \"$FAKE_REFUSE\" ]; then echo 'Failed to enable'; exit 1; fi\nexit 0\n"),
            // FAKE_PS_NO: a script containing it fails (a task that isn't there, say)
            "powershell.exe" => format!("#!/bin/sh\nPATH=/usr/bin:/bin\n{record}\nif [ -n \"$FAKE_PS_NO\" ]; then case \"$*\" in *\"$FAKE_PS_NO\"*) exit 1;; esac; fi\nexit 0\n"),
            "inyon" => format!("#!/bin/sh\nPATH=/usr/bin:/bin\n{record}\necho \"inyon ran $*\"\n"),
            _ => format!("#!/bin/sh\nPATH=/usr/bin:/bin\n{record}\nexit 0\n"),
        };
        let p = self.fakes.join(name);
        fs::write(&p, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    /// What was run, each call as its program and arguments.
    pub fn calls(&self) -> Vec<Vec<String>> {
        let text = fs::read_to_string(&self.log).unwrap_or_default();
        text.split('\u{1d}').filter(|r| !r.is_empty()).map(|r| r.split('\u{1f}').map(String::from).collect()).collect()
    }

    pub fn clear_calls(&self) {
        let _ = fs::remove_file(&self.log);
    }

    pub fn put(&self, rel: &str, text: &str) {
        let p = self.dir.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, text).unwrap();
    }

    pub fn unit(&self, name: &str, exec: &str) {
        self.put(&format!(".config/systemd/user/{name}.service"), &format!("[Service]\nExecStart={exec}\n"));
    }

    pub fn read(&self, rel: &str) -> String {
        fs::read_to_string(self.dir.join(rel)).unwrap_or_default()
    }

    pub fn exists(&self, rel: &str) -> bool {
        self.dir.join(rel).exists()
    }

    pub fn crontab_text(&self) -> String {
        fs::read_to_string(&self.crontab).unwrap_or_default()
    }

    pub fn set_crontab(&self, text: &str) {
        fs::write(&self.crontab, text).unwrap();
    }

    pub fn refuse(&self, verb: &str) {
        std::env::set_var("FAKE_REFUSE", verb);
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn unique() -> u64 {
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    N.fetch_add(1, Ordering::Relaxed)
}

/// The process table: what the fake `sh` started, plus what a test puts there by hand.
pub struct FakeProc {
    file: PathBuf,
    pub signals: RefCell<Vec<(u32, String)>>,
    pub ignore_signals: Cell<bool>,
}

impl FakeProc {
    pub fn new(home: &Home) -> FakeProc {
        FakeProc { file: home.procs.clone(), signals: RefCell::new(Vec::new()), ignore_signals: Cell::new(false) }
    }

    pub fn table(&self) -> BTreeMap<u32, Vec<String>> {
        let mut out = BTreeMap::new();
        for line in fs::read_to_string(&self.file).unwrap_or_default().lines() {
            let Some((pid, args)) = line.split_once('\t') else { continue };
            let Ok(pid) = pid.parse::<u32>() else { continue };
            // a test writes arguments apart; the fake sh records them as "'a' 'b'", quoted
            let args: Vec<String> = if args.contains('\u{1f}') {
                args.split('\u{1f}').map(String::from).collect()
            } else {
                args.split("' '").map(|a| a.trim_matches('\'').to_string()).filter(|a| !a.is_empty()).collect()
            };
            out.insert(pid, args);
        }
        out
    }

    fn write(&self, table: &BTreeMap<u32, Vec<String>>) {
        let text: String = table.iter().map(|(pid, args)| format!("{pid}\t{}\n", args.join("\u{1f}"))).collect();
        fs::write(&self.file, text).unwrap();
    }

    pub fn set(&self, pid: u32, args: &[&str]) {
        let mut t = self.table();
        t.insert(pid, args.iter().map(|a| a.to_string()).collect());
        self.write(&t);
    }

    pub fn signal_names(&self) -> Vec<String> {
        self.signals.borrow().iter().map(|(_, s)| s.clone()).collect()
    }
}

impl Proc for FakeProc {
    fn cmdline(&self, pid: u32) -> Option<Vec<String>> {
        self.table().get(&pid).cloned()
    }
    fn signal(&self, pid: u32, sig: Signal) {
        self.signals.borrow_mut().push((pid, sig.name().to_string()));
        if !self.ignore_signals.get() {
            let mut t = self.table();
            t.remove(&pid);
            self.write(&t);
        }
    }
    fn alive(&self, pid: u32) -> bool {
        self.table().contains_key(&pid)
    }
    fn sleep_ms(&self, _ms: u64) {}
}

pub fn sys<'a>(proc: &'a FakeProc, platform: Platform, program: &Path) -> Sys<'a> {
    Sys { platform, proc, pid: std::process::id(), program: vec![program.to_string_lossy().into_owned()] }
}

pub fn bin(home: &Home) -> PathBuf {
    home.dir.join(".inyon").join("bin").join("inyon")
}
