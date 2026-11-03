//! Uploads sent a chunk at a time, from 0.24: the app starts one (name, size, the pane it's for),
//! sends its bytes in order as raw chunks, asks how much arrived after a dropped connection, and
//! finishes with the file's SHA-256. Each chunk goes on to the machine as it comes, appended to
//! `<path>.part` there, so the hub never holds more than a chunk of any file, and the file takes
//! its name only once its size and hash are right. The hub keeps a running hash of what it
//! appended; the machine checks the finished file too when it has `sha256sum` or `shasum`.
//!
//! What the hub knows of an upload lives in memory: after a restart the app starts the file over.
//! One left alone for an hour is dropped and its part file deleted, as the janitor does for the
//! single-request uploads (features.rs).

use std::cell::RefCell;

use indexmap::IndexMap;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::features::{host_and_home, janitor, max_mb, upload_path, Uploaded};
use crate::clock::now_ms;
use crate::hosts::host::{shq, ExecOptions, Host};
use crate::hosts::http_error::{http, HubError, HubResult};
use crate::hosts::registry::registry;
use crate::js::fmt_number;
use crate::js::text::JsStr;
use crate::platform::rt;

/// The chunk size the app is told to use: small enough that a dropped connection on a slow mobile
/// link loses a few seconds of work and that one chunk crosses the phone's encrypted link as about
/// two dozen frames, large enough that the shell started on the machine per chunk costs little.
pub const CHUNK: usize = 1024 * 1024;
/// The largest chunk the hub takes in one request.
pub const MAX_CHUNK: usize = 4 * 1024 * 1024;
/// Uploads open at once on this hub (or phone); past it, a new one waits for one to finish.
pub const MAX_OPEN: usize = 32;
/// An upload that gets nothing for this long is dropped, as the janitor drops `.part` files.
pub const IDLE_MS: u64 = 60 * 60 * 1000;

pub const GONE: &str = "This upload is no longer on the hub. Send the file again.";

struct Upload {
    host_id: String,
    name: String,
    mime: String,
    size: u64,
    sha256: Option<String>,
    path: String,
    received: u64,
    hasher: Sha256,
    touched: u64,
    /// a chunk or the finish is being written; another waits its turn
    busy: bool,
    /// the answer to a finish, for a finish sent again after its answer was lost
    done: Option<Uploaded>,
}

thread_local! {
    static UPLOADS: RefCell<IndexMap<String, Upload>> = RefCell::new(IndexMap::new());
}

/// How an upload stands, as the app reads it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UploadStatus {
    pub id: String,
    pub name: String,
    pub size: u64,
    /// bytes the machine has, in order from the start
    pub received: u64,
    /// the chunk size to send
    pub chunk: usize,
    /// where the file goes on the machine
    pub path: String,
    pub done: bool,
}

/// What a chunk did: taken (or already there), or refused because the upload stands elsewhere.
#[derive(Debug, Clone, PartialEq)]
pub enum ChunkResult {
    Took(UploadStatus),
    /// the chunk doesn't follow on from what arrived: send from `received`
    Elsewhere(UploadStatus),
}

/// What the app says when it starts an upload.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StartUpload {
    pub name: String,
    pub mime: String,
    pub size: u64,
    pub sha256: Option<String>,
    pub pane: Option<String>,
}

fn status_of(id: &str, u: &Upload) -> UploadStatus {
    UploadStatus { id: id.to_owned(), name: u.name.clone(), size: u.size, received: u.received, chunk: CHUNK, path: u.path.clone(), done: u.done.is_some() }
}

fn part(path: &str) -> String {
    format!("{path}.part")
}

/// A SHA-256 as 64 hex digits, lower case; None for anything else.
pub fn parse_sha256(s: &str) -> Option<String> {
    let s = s.trim().to_ascii_lowercase();
    (s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())).then_some(s)
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}

fn file_name(path: &str) -> &str {
    &path[path.rfind('/').map_or(0, |i| i + 1)..]
}

/// The upload, if it is this machine's. Uploads are found only by an id and the machine they
/// were started for, so a request for another machine's upload gets the same answer as a made-up id.
fn with_upload<T>(host_id: &str, id: &str, f: impl FnOnce(&mut Upload) -> T) -> HubResult<T> {
    UPLOADS.with(|all| match all.borrow_mut().get_mut(id) {
        Some(u) if u.host_id == host_id => Ok(f(u)),
        _ => Err(http(404, GONE)),
    })
}

fn forget(id: &str) -> Option<Upload> {
    UPLOADS.with(|all| all.borrow_mut().shift_remove(id))
}

/// Waits for the upload's chunk or finish in progress, then marks it busy for this one.
async fn take_turn(host_id: &str, id: &str) -> HubResult<()> {
    for _ in 0..2400 {
        if with_upload(host_id, id, |u| {
            if u.busy {
                return false;
            }
            u.busy = true;
            u.touched = now_ms();
            true
        })? {
            return Ok(());
        }
        rt::sleep(50).await;
    }
    Err(http(503, "The machine is still saving the last chunk. Try again."))
}

fn end_turn(id: &str) {
    UPLOADS.with(|all| {
        if let Some(u) = all.borrow_mut().get_mut(id) {
            u.busy = false;
            u.touched = now_ms();
        }
    });
}

/// The upload is beyond saving (its part file went or changed): it's dropped and the app told to
/// start the file over.
async fn broken(host: &Host, id: &str, path: &str, status: u16, why: &str) -> HubError {
    forget(id);
    let _ = host.run(&format!("rm -f {}", shq(&part(path)))).await;
    http(status, why)
}

const CHANGED: &str = "The partly sent file changed on the machine. Send it again.";
pub const DAMAGED: &str = "The file arrived damaged: its SHA-256 doesn't match. Send it again.";

/// Starts an upload: checks the size against the setting and makes the empty part file on the
/// machine, which also tells the app at once if it can't write there.
pub async fn start_upload(host_id: &str, s: &StartUpload) -> HubResult<UploadStatus> {
    let (host, home) = host_and_home(host_id)?;
    let max_mb = max_mb();
    if s.size as f64 > max_mb * 1024.0 * 1024.0 {
        return Err(http(413, format!("{} is larger than {} MB", s.name, fmt_number(max_mb))));
    }
    sweep();
    if UPLOADS.with(|all| all.borrow().values().filter(|u| u.done.is_none()).count()) >= MAX_OPEN {
        return Err(http(429, "Too many uploads at once. Try again when one has finished."));
    }
    let path = upload_path(&home, host_id, s.pane.as_deref(), &s.name);
    let dir = &path[..path.rfind('/').unwrap_or(0)];
    // private to your user on shared machines
    let r = host.exec(&format!("umask 077; mkdir -p {} && : > {}", shq(dir), shq(&part(&path))), ExecOptions::timeout(30_000)).await?;
    if r.code != 0 {
        let err = r.stderr.jtrim();
        return Err(http(500, if err.is_empty() { format!("Could not save {}", file_name(&path)) } else { err.to_owned() }));
    }
    let id = crate::ids::random_uuid().replace('-', "");
    let u = Upload {
        host_id: host_id.to_owned(),
        name: s.name.clone(),
        mime: s.mime.clone(),
        size: s.size,
        sha256: s.sha256.clone(),
        path,
        received: 0,
        hasher: Sha256::new(),
        touched: now_ms(),
        busy: false,
        done: None,
    };
    let status = status_of(&id, &u);
    UPLOADS.with(|all| all.borrow_mut().insert(id, u));
    Ok(status)
}

/// How much of the upload arrived.
pub fn upload_status(host_id: &str, id: &str) -> HubResult<UploadStatus> {
    with_upload(host_id, id, |u| {
        u.touched = now_ms();
        status_of(id, u)
    })
}

/// Appends a chunk that starts at `offset`. A chunk that's already there in full (sent again after
/// its answer was lost) is skipped; one that overlaps the end adds only its new bytes; one that
/// starts past the end is refused with where to send from.
pub async fn put_chunk(host_id: &str, id: &str, offset: u64, data: &[u8]) -> HubResult<ChunkResult> {
    if data.len() > MAX_CHUNK {
        return Err(http(413, format!("Upload chunks are limited to {} MB", MAX_CHUNK / 1024 / 1024)));
    }
    let (host, _) = host_and_home(host_id)?;
    take_turn(host_id, id).await?;
    let res = append(&host, host_id, id, offset, data).await;
    end_turn(id);
    res
}

async fn append(host: &Host, host_id: &str, id: &str, offset: u64, data: &[u8]) -> HubResult<ChunkResult> {
    let (size, received, path, done) = with_upload(host_id, id, |u| (u.size, u.received, u.path.clone(), u.done.is_some()))?;
    let end = offset + data.len() as u64;
    if end > size {
        return Err(http(400, "That chunk goes past the end of the file"));
    }
    if done || end <= received {
        return Ok(ChunkResult::Took(upload_status(host_id, id)?));
    }
    if offset > received {
        return Ok(ChunkResult::Elsewhere(upload_status(host_id, id)?));
    }
    let new = &data[(received - offset) as usize..];
    let (start, end) = (fmt_number(received as f64), fmt_number(end as f64));
    // The chunk lands in a file of its own first and goes on the end of the part file only once it
    // arrived whole, so a cut-off command never leaves half a chunk behind. Where the part file
    // already ends at this chunk's end, an earlier try got through after the hub gave up on it.
    let p = shq(&part(&path));
    let c = shq(&format!("{}.chunk", part(&path)));
    let cmd = format!(
        "n=\"$(wc -c < {p} | tr -d ' ')\" || exit 3; if [ \"$n\" = {end} ]; then cat > /dev/null; echo \"$n\"; exit 0; fi; \
         [ \"$n\" = {start} ] || {{ echo \"the part file has $n bytes, not {start}\" >&2; exit 3; }}; \
         cat > {c} && [ \"$(wc -c < {c} | tr -d ' ')\" = {len} ] && cat {c} >> {p}; r=$?; rm -f {c}; [ $r = 0 ] || exit 1; wc -c < {p} | tr -d ' '",
        len = new.len()
    );
    let r = host.exec(&cmd, ExecOptions { input: Some(new.to_vec()), timeout_ms: Some(120_000) }).await?;
    if r.code == 3 {
        return Err(broken(host, id, &path, 410, CHANGED).await);
    }
    if r.code != 0 {
        let err = r.stderr.jtrim();
        return Err(http(503, if err.is_empty() { format!("Could not save {}", file_name(&path)) } else { err.to_owned() }));
    }
    if r.stdout.jtrim() != end {
        return Err(broken(host, id, &path, 410, CHANGED).await);
    }
    with_upload(host_id, id, |u| {
        u.hasher.update(new);
        u.received += new.len() as u64;
    })?;
    Ok(ChunkResult::Took(upload_status(host_id, id)?))
}

/// Checks the whole file arrived with the hash the app gave (now or at the start) and gives it its
/// name on the machine.
pub async fn finish_upload(host_id: &str, id: &str, sha256: Option<String>) -> HubResult<Uploaded> {
    let (host, _) = host_and_home(host_id)?;
    take_turn(host_id, id).await?;
    let res = finish(&host, host_id, id, sha256).await;
    end_turn(id);
    res
}

async fn finish(host: &Host, host_id: &str, id: &str, sha256: Option<String>) -> HubResult<Uploaded> {
    let (u_done, size, received, path, want, got, name, mime) = with_upload(host_id, id, |u| {
        if let Some(s) = &sha256 {
            u.sha256 = Some(s.clone());
        }
        (u.done.clone(), u.size, u.received, u.path.clone(), u.sha256.clone(), hex(&u.hasher.clone().finalize()), u.name.clone(), u.mime.clone())
    })?;
    if let Some(done) = u_done {
        return Ok(done);
    }
    let Some(want) = want else { return Err(http(400, "Finishing an upload needs the file's SHA-256")) };
    if received != size {
        return Err(http(409, format!("Only {received} of {size} bytes arrived")));
    }
    if got != want {
        return Err(broken(host, id, &path, 422, DAMAGED).await);
    }
    let p = shq(&part(&path));
    let cmd = format!(
        "[ \"$(wc -c < {p} | tr -d ' ')\" = {size} ] || exit 3; \
         h=\"$({{ sha256sum {p} || shasum -a 256 {p}; }} 2>/dev/null | cut -c1-64)\"; [ -z \"$h\" ] || [ \"$h\" = {want} ] || exit 4; mv -f {p} {}",
        shq(&path),
        size = fmt_number(size as f64)
    );
    let r = host.exec(&cmd, ExecOptions::timeout(120_000)).await?;
    match r.code {
        0 => {}
        3 => return Err(broken(host, id, &path, 410, CHANGED).await),
        4 => return Err(broken(host, id, &path, 422, DAMAGED).await),
        _ => {
            let err = r.stderr.jtrim();
            return Err(http(503, if err.is_empty() { format!("Could not save {}", file_name(&path)) } else { err.to_owned() }));
        }
    }
    let done = Uploaded { name, path, size: inyon_protocol::Num(size as f64), kind: mime };
    with_upload(host_id, id, |u| u.done = Some(done.clone()))?;
    janitor::soon(host_id);
    Ok(done)
}

/// Stops an upload and deletes what arrived of it.
pub async fn cancel_upload(host_id: &str, id: &str) -> HubResult<()> {
    let (path, done) = with_upload(host_id, id, |u| (u.path.clone(), u.done.is_some()))?;
    forget(id);
    if !done {
        let (host, _) = host_and_home(host_id)?;
        host.run(&format!("rm -f {} {}", shq(&part(&path)), shq(&format!("{}.chunk", part(&path))))).await?;
    }
    Ok(())
}

/// Drops uploads nobody touched for an hour, deleting their part files on machines that answer.
pub fn sweep() {
    let now = now_ms();
    let stale: Vec<(String, Upload)> = UPLOADS.with(|all| {
        let mut all = all.borrow_mut();
        let ids: Vec<String> = all.iter().filter(|(_, u)| !u.busy && now.saturating_sub(u.touched) > IDLE_MS).map(|(id, _)| id.clone()).collect();
        ids.into_iter().filter_map(|id| all.shift_remove(&id).map(|u| (id, u))).collect()
    });
    for (_, u) in stale {
        if u.done.is_some() {
            continue;
        }
        let Some(rt) = registry().get(&u.host_id) else { continue };
        let host = rt.borrow().host.clone();
        rt::spawn(async move {
            let _ = host.run(&format!("rm -f {} {}", shq(&part(&u.path)), shq(&format!("{}.chunk", part(&u.path))))).await;
        });
    }
}

/// Uploads the hub is holding open, for tests and the log.
pub fn open_uploads() -> usize {
    UPLOADS.with(|all| all.borrow().len())
}

/// Sweeps every few minutes.
pub fn start_sweeping() {
    rt::spawn(async {
        loop {
            rt::sleep(5 * 60_000).await;
            sweep();
        }
    });
}

#[cfg(test)]
#[path = "uploads_tests.rs"]
mod tests;
