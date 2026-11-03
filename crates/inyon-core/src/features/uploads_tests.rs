use std::cell::{Cell, RefCell};
use std::rc::Rc;

use futures_util::future::LocalBoxFuture;
use inyon_protocol::HostConfig;
use serde_json::json;

use super::*;
use crate::hosts::host::{set_host_backends, ExecResult, HostBackend, OnData, OnLine, PtyHandle, StreamHandle};
use crate::platform::rt::Deferred;
use crate::test_support::rt::{run, wait};
use crate::test_support::sandbox::Sandbox;
use crate::test_support::{connected, sh, TempDir};

const MB: usize = 1024 * 1024;

fn bytes(n: usize) -> Vec<u8> {
    (0..n).map(|i| ((i * 7) ^ (i >> 11)) as u8).collect()
}

fn sha(b: &[u8]) -> String {
    hex(&Sha256::digest(b))
}

fn start(name: &str, size: usize) -> StartUpload {
    StartUpload { name: name.into(), mime: "video/mp4".into(), size: size as u64, sha256: None, pane: None }
}

/// Sends `data` a chunk at a time as the app does, then finishes with its hash.
async fn send(name: &str, data: &[u8]) -> Uploaded {
    let s = start_upload("up", &start(name, data.len())).await.unwrap();
    for at in (0..data.len()).step_by(s.chunk) {
        let r = put_chunk("up", &s.id, at as u64, &data[at..(at + s.chunk).min(data.len())]).await.unwrap();
        assert!(matches!(r, ChunkResult::Took(_)), "{r:?}");
    }
    finish_upload("up", &s.id, Some(sha(data))).await.unwrap()
}

fn files_under(dir: &std::path::Path) -> Vec<String> {
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(files_under(&p));
            } else {
                out.push(p.to_string_lossy().into_owned());
            }
        }
    }
    out
}

/// The machine's shell run for real, with every command's stdin size noted, and a way to make the
/// next command fail as a dropped connection does: before it runs, or after it ran with its
/// answer lost.
#[derive(Default)]
struct Machine {
    home: String,
    inputs: RefCell<Vec<usize>>,
    drop_before: Cell<bool>,
    drop_after: Cell<bool>,
}

impl HostBackend for Machine {
    fn exec(&self, _config: &HostConfig, cmd: &str, opts: ExecOptions) -> LocalBoxFuture<'static, HubResult<ExecResult>> {
        if self.drop_before.take() {
            return Box::pin(async { Ok(ExecResult { code: 255, stdout: String::new(), stderr: "the machine went away".into() }) });
        }
        self.inputs.borrow_mut().push(opts.input.as_ref().map_or(0, Vec::len));
        let (code, stdout, stderr) = sh(cmd, opts.input.as_deref(), &[("HOME", &self.home)]);
        if self.drop_after.take() {
            return Box::pin(async { Err(http(504, "timed out")) });
        }
        Box::pin(async move { Ok(ExecResult { code, stdout, stderr }) })
    }
    fn stream(&self, _config: &HostConfig, _cmd: &str, _on_line: OnLine, _keep_stdin: bool) -> StreamHandle {
        StreamHandle::new(|| {}, Deferred::resolved(0))
    }
    fn pty(&self, _config: &HostConfig, _cmd: &str, _cols: u16, _rows: u16, _on_data: OnData) -> HubResult<PtyHandle> {
        Ok(PtyHandle::new(|_| {}, |_, _| {}, || {}, Deferred::resolved(0)))
    }
}

fn machine(home: &TempDir) -> Rc<Machine> {
    connected("up", &home.s(), json!([]));
    let m = Rc::new(Machine { home: home.s(), ..Default::default() });
    let b = m.clone();
    set_host_backends(move |_| b.clone() as Rc<dyn HostBackend>);
    m
}

#[test]
fn a_5_mib_file_goes_a_chunk_at_a_time_and_takes_its_name_once_whole() {
    run(|| async {
        let home = TempDir::new("inyon-chunk-");
        let m = machine(&home);
        let v = bytes(5 * MB + 123);
        let u = send("clip.mp4", &v).await;
        assert!(u.path.starts_with(&format!("{}/.inyon/uploads/", home.s())), "{}", u.path);
        assert!(u.path.ends_with("-clip.mp4"), "{}", u.path);
        assert_eq!(std::fs::read(&u.path).unwrap(), v);
        assert_eq!((u.name.as_str(), u.kind.as_str(), u.size), ("clip.mp4", "video/mp4", inyon_protocol::Num(v.len() as f64)));
        // nothing left beside it, and no command was handed more than a chunk
        assert_eq!(files_under(&home.path(".inyon/uploads")), vec![u.path.clone()]);
        assert!(m.inputs.borrow().iter().all(|n| *n <= CHUNK), "{:?}", m.inputs.borrow());
    });
}

#[test]
fn a_50_mib_file_arrives_whole() {
    run(|| async {
        let home = TempDir::new("inyon-chunk-");
        machine(&home);
        let v = bytes(50 * MB);
        let u = send("big.bin", &v).await;
        assert_eq!(std::fs::metadata(&u.path).unwrap().len(), v.len() as u64);
        assert!(std::fs::read(&u.path).unwrap() == v);
        assert_eq!(open_uploads(), 1, "kept for a finish sent again");
    });
}

#[test]
fn a_dropped_connection_mid_way_resumes_from_what_the_machine_has() {
    run(|| async {
        let home = TempDir::new("inyon-chunk-");
        let m = machine(&home);
        let v = bytes(4 * MB + 10);
        let s = start_upload("up", &start("resume.mp4", v.len())).await.unwrap();
        let chunk = |i: usize| &v[i * CHUNK..((i + 1) * CHUNK).min(v.len())];
        put_chunk("up", &s.id, 0, chunk(0)).await.unwrap();
        // the machine can't be reached for this one: nothing written, and the hub says so
        m.drop_before.set(true);
        let e = put_chunk("up", &s.id, CHUNK as u64, chunk(1)).await.unwrap_err();
        assert_eq!(e.status, Some(503), "{}", e.message);
        assert_eq!(upload_status("up", &s.id).unwrap().received, CHUNK as u64);
        // this one got there, but its answer was lost on the way back
        m.drop_after.set(true);
        assert!(put_chunk("up", &s.id, CHUNK as u64, chunk(1)).await.is_err());
        assert_eq!(upload_status("up", &s.id).unwrap().received, CHUNK as u64);
        // sent again, it's found already there and counted once
        let again = put_chunk("up", &s.id, CHUNK as u64, chunk(1)).await.unwrap();
        assert!(matches!(&again, ChunkResult::Took(st) if st.received == 2 * CHUNK as u64), "{again:?}");
        // a chunk from further on is refused with where to carry on from
        let ahead = put_chunk("up", &s.id, 3 * CHUNK as u64, chunk(3)).await.unwrap();
        assert!(matches!(&ahead, ChunkResult::Elsewhere(st) if st.received == 2 * CHUNK as u64), "{ahead:?}");
        // and one already there in full changes nothing
        let old = put_chunk("up", &s.id, 0, chunk(0)).await.unwrap();
        assert!(matches!(&old, ChunkResult::Took(st) if st.received == 2 * CHUNK as u64), "{old:?}");
        for i in 2..5 {
            put_chunk("up", &s.id, (i * CHUNK) as u64, chunk(i)).await.unwrap();
        }
        let u = finish_upload("up", &s.id, Some(sha(&v))).await.unwrap();
        assert_eq!(std::fs::read(&u.path).unwrap(), v);
        // the finish sent again after its answer was lost gives the same file
        assert_eq!(finish_upload("up", &s.id, None).await.unwrap(), u);
    });
}

#[test]
fn a_chunk_that_overlaps_the_end_adds_only_its_new_bytes() {
    run(|| async {
        let home = TempDir::new("inyon-chunk-");
        machine(&home);
        let v = bytes(3000);
        let s = start_upload("up", &start("o.bin", v.len())).await.unwrap();
        put_chunk("up", &s.id, 0, &v[..1000]).await.unwrap();
        put_chunk("up", &s.id, 500, &v[500..2000]).await.unwrap();
        put_chunk("up", &s.id, 2000, &v[2000..]).await.unwrap();
        let u = finish_upload("up", &s.id, Some(sha(&v))).await.unwrap();
        assert_eq!(std::fs::read(&u.path).unwrap(), v);
    });
}

#[test]
fn a_wrong_hash_is_refused_and_nothing_is_left_behind() {
    run(|| async {
        let home = TempDir::new("inyon-chunk-");
        machine(&home);
        let v = bytes(2 * MB);
        let s = start_upload("up", &start("bad.bin", v.len())).await.unwrap();
        for at in (0..v.len()).step_by(CHUNK) {
            put_chunk("up", &s.id, at as u64, &v[at..at + CHUNK]).await.unwrap();
        }
        let e = finish_upload("up", &s.id, Some(sha(b"something else"))).await.unwrap_err();
        assert_eq!((e.status, e.message.as_str()), (Some(422), DAMAGED));
        assert_eq!(files_under(&home.path(".inyon/uploads")), Vec::<String>::new());
        assert_eq!(upload_status("up", &s.id).unwrap_err().status, Some(404));
        // a finish needs a hash, and the whole file
        let s = start_upload("up", &start("short.bin", 10)).await.unwrap();
        assert_eq!(finish_upload("up", &s.id, None).await.unwrap_err().status, Some(400));
        put_chunk("up", &s.id, 0, b"12345").await.unwrap();
        let e = finish_upload("up", &s.id, Some(sha(b"1234567890"))).await.unwrap_err();
        assert_eq!((e.status, e.message.as_str()), (Some(409), "Only 5 of 10 bytes arrived"));
    });
}

#[test]
fn the_machine_checks_the_hash_too() {
    run(|| async {
        let home = TempDir::new("inyon-chunk-");
        machine(&home);
        let v = bytes(1000);
        let s = start_upload("up", &start("m.bin", v.len())).await.unwrap();
        put_chunk("up", &s.id, 0, &v).await.unwrap();
        // the bytes change on the machine after they arrived, but before the finish
        let part = format!("{}.part", s.path);
        let mut on_disk = std::fs::read(&part).unwrap();
        on_disk[10] ^= 1;
        std::fs::write(&part, on_disk).unwrap();
        let e = finish_upload("up", &s.id, Some(sha(&v))).await.unwrap_err();
        if sh("command -v sha256sum || command -v shasum", None, &[]).0 == 0 {
            assert_eq!((e.status, e.message.as_str()), (Some(422), DAMAGED));
            assert!(!std::path::Path::new(&part).exists());
        }
    });
}

#[test]
fn too_large_is_refused_before_anything_is_written() {
    run(|| async {
        let home = TempDir::new("inyon-chunk-");
        machine(&home);
        let e = start_upload("up", &start("huge.mov", 101 * MB)).await.unwrap_err();
        assert_eq!((e.status, e.message.as_str()), (Some(413), "huge.mov is larger than 100 MB"));
        assert!(!home.exists(".inyon/uploads"));
        let s = start_upload("up", &start("small.bin", 10)).await.unwrap();
        assert_eq!(put_chunk("up", &s.id, 5, b"0123456789").await.unwrap_err().status, Some(400));
        assert_eq!(put_chunk("up", &s.id, 0, &vec![0; MAX_CHUNK + 1]).await.unwrap_err().status, Some(413));
    });
}

#[test]
fn names_cant_reach_outside_the_uploads_folder_and_ids_only_work_for_their_machine() {
    run(|| async {
        let home = TempDir::new("inyon-chunk-");
        machine(&home);
        for name in ["../../.bashrc", "/etc/passwd", "..", "a/../../b", "x\n; rm -rf ~", ".ssh/authorized_keys"] {
            let u = send(name, b"hello").await;
            let rest = u.path.strip_prefix(&format!("{}/.inyon/uploads/", home.s())).unwrap_or_else(|| panic!("{name:?} went to {}", u.path));
            // <date>/<session>/<file>, none of them a dot folder or hidden
            let parts: Vec<&str> = rest.split('/').collect();
            assert_eq!(parts.len(), 3, "{name:?} went to {}", u.path);
            assert!(parts.iter().all(|p| !p.is_empty() && !p.starts_with('.') && !p.contains('\n')), "{name:?} went to {}", u.path);
            assert_eq!(std::fs::read(&u.path).unwrap(), b"hello");
        }
        assert!(!home.exists(".bashrc") && !home.exists(".ssh"));
        // another machine's id, or a made-up one, finds nothing
        let s = start_upload("up", &start("mine.txt", 3)).await.unwrap();
        connected("other", &home.s(), json!([]));
        assert_eq!(upload_status("other", &s.id).unwrap_err().message, GONE);
        assert_eq!(put_chunk("other", &s.id, 0, b"abc").await.unwrap_err().status, Some(404));
        assert_eq!(upload_status("up", "../../etc").unwrap_err().status, Some(404));
    });
}

#[test]
fn an_upload_left_alone_for_an_hour_is_dropped_and_its_part_file_deleted() {
    run(|| async {
        let home = TempDir::new("inyon-chunk-");
        machine(&home);
        let s = start_upload("up", &start("gone.mp4", 2 * MB)).await.unwrap();
        put_chunk("up", &s.id, 0, &bytes(MB)).await.unwrap();
        let part = format!("{}.part", s.path);
        assert!(std::path::Path::new(&part).exists());
        wait(IDLE_MS - 1000).await;
        sweep();
        assert_eq!(upload_status("up", &s.id).unwrap().received, MB as u64, "still in use within the hour");
        wait(IDLE_MS + 1000).await;
        sweep();
        wait(10).await;
        assert_eq!(open_uploads(), 0);
        assert!(!std::path::Path::new(&part).exists());
        assert_eq!(put_chunk("up", &s.id, MB as u64, &bytes(MB)).await.unwrap_err().message, GONE);
    });
}

#[test]
fn a_cancelled_upload_leaves_nothing_and_an_empty_file_works() {
    run(|| async {
        let home = TempDir::new("inyon-chunk-");
        machine(&home);
        let s = start_upload("up", &start("stop.bin", 2 * MB)).await.unwrap();
        put_chunk("up", &s.id, 0, &bytes(MB)).await.unwrap();
        cancel_upload("up", &s.id).await.unwrap();
        assert_eq!(files_under(&home.path(".inyon/uploads")), Vec::<String>::new());
        let s = start_upload("up", &start("empty.txt", 0)).await.unwrap();
        let u = finish_upload("up", &s.id, Some(sha(b""))).await.unwrap();
        assert_eq!(std::fs::metadata(&u.path).unwrap().len(), 0);
    });
}

#[test]
fn only_so_many_uploads_are_open_at_once() {
    run(|| async {
        let home = TempDir::new("inyon-chunk-");
        Sandbox::install(&home.s());
        connected("up", &home.s(), json!([]));
        for i in 0..MAX_OPEN {
            start_upload("up", &start(&format!("f{i}"), 1)).await.unwrap();
        }
        assert_eq!(start_upload("up", &start("one-more", 1)).await.unwrap_err().status, Some(429));
    });
}

#[test]
fn hashes_are_64_hex_digits() {
    assert_eq!(parse_sha256(&"AB".repeat(32)), Some("ab".repeat(32)));
    assert_eq!(parse_sha256("abc"), None);
    assert_eq!(parse_sha256(&"zz".repeat(32)), None);
}
