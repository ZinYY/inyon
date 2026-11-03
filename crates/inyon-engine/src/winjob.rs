//! Windows jobs that end what's in them once nothing holds them any more. A child process on
//! Windows outlives the process that started it, so the link's tunnel (cloudflared) goes into a
//! job this process holds until it ends: however the link stops (Quit in the tray, Task Scheduler
//! ending its task, a crash), Windows closes the handle and cloudflared goes with it. The inyon
//! command uses the same kind of job for an engine it runs in a terminal.

use std::os::windows::io::{AsRawHandle, RawHandle};
use std::sync::OnceLock;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK,
};

/// A job that ends its processes when its last handle closes.
pub struct Job(HANDLE);

// SAFETY: a job handle can be used and closed from any thread
unsafe impl Send for Job {}
unsafe impl Sync for Job {}

impl Drop for Job {
    fn drop(&mut self) {
        // SAFETY: the handle is ours and closed once
        unsafe { CloseHandle(self.0) };
    }
}

impl Job {
    /// A new job whose processes end when it's closed, or None when Windows refused one.
    pub fn kill_on_close() -> Option<Job> {
        Self::with_limits(JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE)
    }

    /// The Inyon app's job: what the app adds (the link, the hub connection, a pairing) ends when
    /// the app does, even when it crashes, but what those start is left out of it unless it's
    /// added too (the link adds its tunnel to a job of its own). A psmux server an agent session
    /// started keeps running after Quit, with your sessions in it.
    pub fn owner() -> Option<Job> {
        Self::with_limits(JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_SILENT_BREAKAWAY_OK)
    }

    fn with_limits(flags: u32) -> Option<Job> {
        // SAFETY: plain calls with a zeroed limits struct the size Windows expects
        unsafe {
            let h = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if h.is_null() {
                return None;
            }
            let job = Job(h);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = flags;
            let size = std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
            if SetInformationJobObject(h, JobObjectExtendedLimitInformation, &info as *const _ as *const _, size) == 0 {
                return None;
            }
            Some(job)
        }
    }

    /// Puts the process behind `handle` in the job (with whatever it starts from now on).
    pub fn add(&self, handle: RawHandle) -> bool {
        // SAFETY: a process handle the caller owns, assigned to our job
        unsafe { AssignProcessToJobObject(self.0, handle as HANDLE) != 0 }
    }
}

/// A job holding `child` that Windows ends, with whatever it started, once this process has gone
/// and nothing holds the job any more.
pub fn ends_with_me(child: &std::process::Child) -> Option<Job> {
    let job = Job::kill_on_close()?;
    job.add(child.as_raw_handle()).then_some(job)
}

/// The job for what the link starts. It's never dropped, so it closes only when this process
/// ends, and takes those processes with it.
pub fn for_children() -> Option<&'static Job> {
    static JOB: OnceLock<Option<Job>> = OnceLock::new();
    JOB.get_or_init(Job::kill_on_close).as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    #[test]
    fn what_is_in_the_job_ends_when_the_job_is_closed() {
        // ping waits a second between tries, so this one would run for half a minute
        let mut child = Command::new("ping").args(["-n", "30", "127.0.0.1"]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let job = Job::kill_on_close().expect("a job");
        assert!(job.add(child.as_raw_handle()));
        assert!(child.try_wait().unwrap().is_none());
        drop(job);
        let started = Instant::now();
        let mut ended = None;
        while ended.is_none() && started.elapsed() < Duration::from_secs(10) {
            ended = child.try_wait().unwrap();
            std::thread::sleep(Duration::from_millis(100));
        }
        if ended.is_none() {
            let _ = child.kill();
        }
        assert!(ended.is_some(), "the process outlived its job");
    }

    #[test]
    fn the_apps_job_ends_what_it_holds_but_not_what_those_started() {
        // cmd starts ping and waits for it: cmd is in the job, ping only by inheritance, which the
        // app's job leaves out, as it leaves out a psmux server an agent session started
        let count = format!("{}", 41 + std::process::id() % 50);
        let mut child = Command::new("cmd").args(["/C", "ping", "-n", &count, "127.0.0.2"]).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
        let job = Job::owner().expect("a job");
        assert!(job.add(child.as_raw_handle()));
        let pings = || {
            let script = format!(
                "Get-CimInstance Win32_Process -Filter \"Name='PING.EXE'\" | Where-Object {{ $_.CommandLine -like '*-n {count} 127.0.0.2*' }} | ForEach-Object {{ $_.ProcessId }}"
            );
            let out = Command::new("powershell.exe").args(["-NoProfile", "-NonInteractive", "-Command", &script]).output().unwrap();
            String::from_utf8_lossy(&out.stdout).split_whitespace().map(String::from).collect::<Vec<_>>()
        };
        let started = Instant::now();
        while pings().is_empty() && started.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(200));
        }
        drop(job);
        let started = Instant::now();
        while child.try_wait().unwrap().is_none() && started.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(100));
        }
        assert!(child.try_wait().unwrap().is_some(), "what the app holds ends with it");
        let left = pings();
        for pid in &left {
            let _ = Command::new("taskkill").args(["/F", "/PID", pid]).stdout(Stdio::null()).stderr(Stdio::null()).status();
        }
        assert!(!left.is_empty(), "what it started keeps running");
    }

    #[test]
    fn the_link_has_one_job_for_what_it_starts() {
        let a = for_children().expect("a job") as *const Job;
        let b = for_children().expect("a job") as *const Job;
        assert_eq!(a, b);
    }
}
