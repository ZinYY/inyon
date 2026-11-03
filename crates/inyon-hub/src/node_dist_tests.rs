//! node-dist.test.ts, pair-command.test.ts (the one-liner run against the real install.sh) and
//! node-update-scripts.test.ts's checks of hub-update.ps1, which this hub still sends.

use std::cell::RefCell;
use std::rc::Rc;

use super::*;

fn q(s: &str) -> Query {
    Query(s.to_owned())
}

fn block<F: std::future::Future>(f: F) -> F::Output {
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    tokio::task::LocalSet::new().block_on(&rt, async {
        inyon_core::platform::rt::install(Rc::new(inyon_backends::runtime::TokioRuntime));
        f.await
    })
}

#[test]
fn a_download_names_its_system_windows_on_arm_gets_its_own_command_but_the_x64_engine() {
    assert_eq!(target_of(&q("os=linux&arch=arm64")).unwrap(), "linux-arm64");
    assert_eq!(target_of(&q("os=darwin&arch=x64")).unwrap(), "darwin-x64");
    assert_eq!(target_of(&q("os=windows&arch=arm64")).unwrap(), "windows-x64");
    assert_eq!(cli_target_of(&q("os=windows&arch=arm64")).unwrap(), "windows-arm64");
    assert_eq!(cli_target_of(&q("os=linux&arch=x64")).unwrap(), "linux-x64");
    // what tether-node's Windows installer asked with: no os at all
    assert_eq!(target_of(&q("arch=x64")).unwrap(), "windows-x64");
    assert_eq!(cli_target_of(&q("arch=x64")).unwrap(), "windows-x64");
    assert!(target_of(&q("os=plan9&arch=x64")).unwrap_err().message.starts_with("os must be"));
    assert!(cli_target_of(&q("os=plan9&arch=x64")).unwrap_err().message.starts_with("os must be"));
    assert!(target_of(&q("os=linux&arch=x86")).unwrap_err().message.starts_with("arch must be"));
}

#[test]
fn the_pinned_busybox_checksum_is_served_per_architecture_without_a_download() {
    let (x64, arm64) = (busybox_pin(&arch_of(&q("os=windows&arch=x64")).unwrap()).1, busybox_pin(&arch_of(&q("arch=arm64")).unwrap()).1);
    assert!(x64.len() == 64 && arm64.len() == 64 && x64 != arm64);
    assert_eq!(busybox_pin(&arch_of(&q("")).unwrap()).1, x64);
    assert_eq!(arch_of(&q("arch=x86")).unwrap_err().status, Some(400));
}

#[test]
fn tether_node_on_linux_is_told_to_run_add_a_machine_again() {
    let dir = std::env::temp_dir().join(format!("inyon-node-dist-{}", crate::http::random_hex(6)));
    let d = Dist::new(None, &dir);
    let e = block(d.update_script("linux", "x64")).unwrap_err();
    assert!(e.message.contains("Add a machine"));
    assert!(REPLACED.contains("Copy the \"Add a machine\" command from the hub again"));
}

#[test]
fn the_installers_are_served_as_they_are_in_the_repository() {
    assert!(INSTALL_SH.starts_with("#!/bin/sh\n"));
    assert!(INSTALL_PS1.contains("$TaskName = 'Inyon node'"));
    // the windowless copy follows from the inyon.exe it fetched, carrying the built-in engine's
    // mark (apps/cli's engine::COPY_MARK), not from whether the hub lists an engine file
    assert!(INSTALL_PS1.contains(".Contains('inyon: the built-in engine, windowless copy')"));
    assert!(!INSTALL_PS1.contains("Hub-Sum 'inyon-engine'"));
}

#[test]
fn earlier_builds_go_with_their_gz_and_sha256_this_build_this_release_and_busybox_stay() {
    let dir = std::env::temp_dir().join(format!("inyon-node-dist-{}", crate::http::random_hex(6)));
    std::fs::create_dir_all(&dir).unwrap();
    let d = Dist::new(None, &dir);
    // a hub run as the program: its tag is its release
    let tag = d.tag().to_owned();
    let stale = [
        "v0.12.0-inyon-engine-linux-x64",
        "v0.12.0-inyon-engine-linux-x64.gz",
        "v0.12.0-inyon-engine-linux-x64.sha256",
        "v0.12.0-inyon-windows-x64.exe",
        "0123456789abcdef-inyon-engine-darwin-arm64",
        "inyon-0123456789abcdef-inyon-engine-linux-x64",
        "tether-node-0123456789abcdef.js",
    ];
    let mut kept = vec![
        format!("{tag}-inyon-engine-linux-x64"),
        format!("{tag}-inyon-engine-linux-x64.gz"),
        format!("{tag}-inyon-engine-linux-x64.sha256"),
        format!("{tag}-inyon-engine-windows-x64.exe.tmp"),
        format!("v{VERSION}-inyon-windows-arm64.exe"),
        "busybox-w64u-FRP-6075-g169694ebd.exe".to_owned(),
        "notes.txt".to_owned(),
    ];
    for f in stale.iter().map(|s| s.to_string()).chain(kept.clone()) {
        std::fs::write(dir.join(&f), &f).unwrap();
    }
    d.prune_old(&dir);
    let mut left: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    left.sort();
    kept.sort();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(left, kept);
}

/// A stand-in for the GitHub release the hub takes the program from: names asked are noted.
fn release(asked: Rc<RefCell<Vec<String>>>, gz: Vec<u8>) -> Get {
    Rc::new(move |url: String| {
        let name = url.rsplit('/').next().unwrap_or("").to_owned();
        asked.borrow_mut().push(name.clone());
        let gz = gz.clone();
        Box::pin(async move {
            if url.contains("/gone/") {
                return Ok((404, b"no".to_vec()));
            }
            Ok(match name.as_str() {
                "inyon-linux-x64" => (200, BUILD.as_bytes().to_vec()),
                "inyon-linux-x64.gz" => (200, gz),
                _ => (404, b"no".to_vec()),
            })
        })
    })
}
const BUILD: &str = "the linux build";

#[test]
fn the_hub_takes_the_gzipped_release_file_when_sha256sums_lists_one_and_keeps_the_plain_bytes() {
    let gz = gzip(BUILD.as_bytes()).unwrap();
    let asked = Rc::new(RefCell::new(vec![]));
    let sums = format!("{}  inyon-linux-x64\n{}  inyon-linux-x64.gz\n", digest(BUILD.as_bytes()), digest(&gz));
    assert_eq!(block(release_file("inyon-linux-x64", &sums, "http://r", release(asked.clone(), gz.clone()))).unwrap(), BUILD.as_bytes());
    assert_eq!(*asked.borrow(), ["inyon-linux-x64.gz"]);
    // a release from before 0.11: the plain file
    asked.borrow_mut().clear();
    let plain_sums = format!("{}  inyon-linux-x64\n", digest(BUILD.as_bytes()));
    assert_eq!(block(release_file("inyon-linux-x64", &plain_sums, "http://r", release(asked.clone(), gz))).unwrap(), BUILD.as_bytes());
    assert_eq!(*asked.borrow(), ["inyon-linux-x64"]);
}

#[test]
fn both_the_gz_and_what_it_unpacks_to_are_checked_and_a_file_not_listed_is_refused() {
    let gz = gzip(BUILD.as_bytes()).unwrap();
    let get = || release(Rc::default(), gz.clone());
    let err = |name: &str, sums: String, base: &str| block(release_file(name, &sums, base, get())).unwrap_err().message;
    let (b, g, o) = (digest(BUILD.as_bytes()), digest(&gz), digest(b"other"));
    assert!(err("inyon-linux-x64", format!("{b}  inyon-linux-x64\n{o}  inyon-linux-x64.gz\n"), "http://r").contains("inyon-linux-x64.gz did not match"));
    assert!(err("inyon-linux-x64", format!("{o}  inyon-linux-x64\n{g}  inyon-linux-x64.gz\n"), "http://r").contains("inyon-linux-x64 did not match"));
    assert!(err("inyon-darwin-x64", format!("{b}  inyon-linux-x64\n"), "http://r").contains("has no inyon-darwin-x64"));
    assert_eq!(err("inyon-linux-x64", format!("{b}  inyon-linux-x64\n{g}  inyon-linux-x64.gz\n"), "http://r/gone"), "Could not download inyon-linux-x64.gz (404)");
}

#[test]
fn gzip_writes_the_bytes_nodes_zlib_does() {
    // zlib.gzipSync(…, { level: 6 }) in Bun, for the conformance harness's placeholder engine
    let gz = gzip(b"placeholder inyon-engine-linux-x64 for the conformance harness\n").unwrap();
    assert_eq!(digest(&gz), "6e053e6f46ad8c4e81d14ffa8a5fd73835ad6f5ef9a791a048569389d7f66e78");
}

#[test]
fn checksum_lists_are_read_as_sha256sum_writes_them() {
    let sums = "AB12  inyon-linux-x64\r\ncd34 *inyon-linux-x64.gz\n";
    assert_eq!(sum_for(sums, "inyon-linux-x64").as_deref(), Some("ab12"));
    assert_eq!(sum_for(sums, "inyon-linux-x64.gz").as_deref(), Some("cd34"));
    assert_eq!(sum_for(sums, "inyon"), None);
}

#[test]
fn the_unix_one_liner_carries_the_token_in_the_environment_and_both_checksums() {
    // unixPairCommand('http://hub:4777', 'tok', 'aaa', 'bbb') in Bun
    assert_eq!(
        unix_pair_command("http://hub:4777", "tok", "aaa", "bbb", None),
        r#"INYON_TOKEN=tok INYON_SUMS_SHA256=bbb sh -c 'f=$(mktemp) && curl -fsSL "$1/node/install.sh" -o "$f" && if [ "$( (sha256sum "$f" 2>/dev/null || shasum -a 256 "$f") | cut -c1-64)" = "$2" ]; then sh "$f" "$1"; else echo "install.sh did not match its checksum; get a new command from the hub" >&2; false; fi; s=$?; rm -f "$f"; exit $s' inyon http://hub:4777 aaa"#
    );
}

#[test]
fn a_hub_on_its_own_certificate_puts_its_sha256_in_the_command() {
    // unixPairCommand('https://192.168.1.5:4777', 'tok', 'aaa', 'bbb', 'cc') in Bun
    assert_eq!(
        unix_pair_command("https://192.168.1.5:4777", "tok", "aaa", "bbb", Some("cc")),
        r#"INYON_TOKEN=tok INYON_SUMS_SHA256=bbb INYON_HUB_CERT=cc sh -c 'f=$(mktemp) && curl -fsSLk "$1/node/install.sh" -o "$f" && if [ "$( (sha256sum "$f" 2>/dev/null || shasum -a 256 "$f") | cut -c1-64)" = "$2" ]; then sh "$f" "$1"; else echo "install.sh did not match its checksum; get a new command from the hub" >&2; false; fi; s=$?; rm -f "$f"; exit $s' inyon https://192.168.1.5:4777 aaa"#
    );
}

#[cfg(unix)]
#[test]
fn a_hub_on_its_own_certificate_hands_its_sha256_to_join_and_every_download_is_still_checked() {
    let (hub, state) = pair_hub(false);
    state.lock().unwrap().program = "#!/bin/sh\nprintf \"%s\\n\" \"$*\" \"$INYON_HUB_CERT\" > \"$HOME/ran\"\n".into();
    let sums = state.lock().unwrap().sums();
    let pin = "ab".repeat(32);
    let r = run_pair(&unix_pair_command(&hub, "tok", &sha(INSTALL_SH), &sha(&sums), Some(&pin)), &[]);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.ran.as_deref(), Some(format!("join {hub}\n{pin}\n").as_str()));
    // curl skips its own check only where the command's checksums cover every download
    let unchecked = run_pair(&format!("INYON_TOKEN=tok INYON_HUB_CERT={pin} sh -c 'curl -fsSL \"$1/node/install.sh\" | sh -s -- \"$1\"' inyon {hub}"), &[]);
    assert_eq!(unchecked.code, 2, "{}", unchecked.err);
    assert!(unchecked.err.contains("INYON_HUB_CERT needs INYON_SUMS_SHA256"), "{}", unchecked.err);
    assert_eq!(unchecked.ran, None);
}

/// A Windows console program, as small as one gets: the DOS header pointing at a PE header.
fn exe() -> Vec<u8> {
    let mut b = vec![0u8; 0x200];
    b[..2].copy_from_slice(b"MZ");
    b[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
    b[0x80..0x84].copy_from_slice(b"PE\0\0");
    b[0x80 + 92..0x80 + 94].copy_from_slice(&3u16.to_le_bytes());
    b
}

#[test]
fn the_windowless_copy_changes_only_the_subsystem_and_anything_else_is_refused() {
    let from = exe();
    let copy = windowless(from.clone()).unwrap();
    let changed: Vec<usize> = (0..from.len()).filter(|&i| from[i] != copy[i]).collect();
    assert_eq!(changed, [0x80 + 92]);
    assert_eq!(u16::from_le_bytes([copy[0x80 + 92], copy[0x80 + 93]]), 2);
    assert!(windowless(b"#!/bin/sh\n".to_vec()).is_err());
    let mut cut = exe();
    cut.truncate(0x90);
    assert!(windowless(cut).is_err());
}

/// A hub built in a throwaway checkout, cargo's builds in its own cli-dist folder.
fn checkout_dist(built: &[(&str, Vec<u8>)]) -> (std::path::PathBuf, Rc<Dist>) {
    let dir = std::env::temp_dir().join(format!("inyon-node-dist-{}", crate::http::random_hex(6)));
    let cli_dist = dir.join("cli-dist");
    std::fs::create_dir_all(&cli_dist).unwrap();
    for (t, data) in built {
        std::fs::write(cli_dist.join(cli_file(t)), data).unwrap();
    }
    let d = Rc::into_inner(Dist::new(Some(dir.join("repo")), &dir.join("data"))).unwrap();
    let tag = built_tag(&cli_dist);
    (dir.clone(), Rc::new(Dist { cli_dist, tag, ..d }))
}

/// The release's checksums, as a release that can't be had answers.
fn sums_fail(d: &Dist) {
    let f: LocalBoxFuture<'static, Result<Signed, HubError>> = Box::pin(async { Err(http(502, "Could not get the checksums of Inyon 0.17.0 (404)")) });
    *d.sums.borrow_mut() = Some(f.shared());
}

#[test]
fn a_checkout_hands_out_what_cargo_built_and_the_release_for_a_system_it_didnt_build() {
    let (dir, d) = checkout_dist(&[("linux-x64", b"linux build".to_vec()), ("windows-x64", exe())]);
    assert!(d.tag().starts_with("checkout-"), "{}", d.tag());
    std::fs::create_dir_all(&d.out).unwrap();
    let release = d.out.join(format!("v{VERSION}-{}", cli_file("darwin-arm64")));
    std::fs::write(&release, "release").unwrap();
    sums_fail(&d);
    let (linux, linux_engine, win_engine, darwin, arm, darwin_engine) = block(async {
        let linux = d.command("linux-x64").await;
        let linux_engine = d.engine("linux-x64").await;
        let win_engine = d.engine("windows-x64").await;
        let darwin = d.command("darwin-arm64").await;
        sums_fail(&d);
        let arm = d.command("windows-arm64").await;
        sums_fail(&d);
        let darwin_engine = d.engine("darwin-x64").await;
        (linux, linux_engine, win_engine, darwin, arm, darwin_engine)
    });
    let read = |p: &std::path::Path| std::fs::read(p).unwrap();
    assert_eq!(linux.unwrap(), d.cli_dist.join(cli_file("linux-x64")));
    // an older inyon's engine is a copy of the command, the Windows one without its console
    let linux_engine = linux_engine.unwrap();
    assert_eq!(linux_engine, d.out.join(format!("{}-{}", d.tag(), engine_file("linux-x64"))));
    assert_eq!(read(&linux_engine), b"linux build");
    assert_eq!(read(&win_engine.unwrap()), windowless(exe()).unwrap());
    // a system cargo didn't build gets the release's file, here already
    assert_eq!(darwin.unwrap(), release);
    let e = arm.unwrap_err();
    assert_eq!(e.status, Some(503));
    assert!(e.message.contains("has no inyon command for windows-arm64"), "{}", e.message);
    assert_eq!(darwin_engine.unwrap_err().status, Some(502));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_new_cargo_build_is_a_new_tag() {
    let dir = std::env::temp_dir().join(format!("inyon-node-dist-{}", crate::http::random_hex(6)));
    std::fs::create_dir_all(&dir).unwrap();
    let none = built_tag(&dir);
    std::fs::write(dir.join(cli_file("linux-arm64")), "one").unwrap();
    let one = built_tag(&dir);
    std::fs::write(dir.join(cli_file("linux-arm64")), "two!").unwrap();
    let two = built_tag(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(none != one && one != two, "{none} {one} {two}");
}

#[test]
fn a_checkouts_build_and_this_release_both_stay_when_their_tags_differ() {
    let dir = std::env::temp_dir().join(format!("inyon-node-dist-{}", crate::http::random_hex(6)));
    let out = dir.join("out");
    std::fs::create_dir_all(&out).unwrap();
    let d = Dist::new(Some(dir.join("repo")), &dir);
    // a hub run from a checkout: its tag is its builds', not the release
    let tag = d.tag().to_owned();
    assert_ne!(tag, format!("v{VERSION}"));
    let stale = [
        "v0.12.0-inyon-engine-linux-x64",
        "v0.12.0-inyon-engine-linux-x64.gz",
        "v0.12.0-inyon-engine-linux-x64.sha256",
        "v0.12.0-inyon-windows-x64.exe",
        "0123456789abcdef-inyon-engine-darwin-arm64",
        // an earlier cargo build
        "checkout-0123456789ab-inyon-engine-linux-x64",
        "inyon-0123456789abcdef-inyon-engine-linux-x64",
        "tether-node-0123456789abcdef.js",
    ];
    let mut kept = vec![
        format!("{tag}-inyon-engine-linux-x64"),
        format!("{tag}-inyon-engine-linux-x64.gz"),
        format!("{tag}-inyon-engine-linux-x64.sha256"),
        format!("{tag}-inyon-engine-windows-x64.exe.tmp"),
        format!("v{VERSION}-inyon-linux-x64"),
        format!("v{VERSION}-inyon-windows-arm64.exe"),
        "busybox-w64u-FRP-6075-g169694ebd.exe".to_owned(),
        "notes.txt".to_owned(),
    ];
    for f in stale.iter().map(|s| s.to_string()).chain(kept.clone()) {
        std::fs::write(out.join(&f), &f).unwrap();
    }
    d.prune_old(&out);
    let mut left: Vec<String> = std::fs::read_dir(&out).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    left.sort();
    kept.sort();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(left, kept);
}

// ---------- pair-command.test.ts: the one-liner run against the real install.sh ----------

/// A local stand-in for a hub: answers every request (its path and query) with `answer`.
fn serve(answer: impl Fn(&str) -> (u16, Vec<u8>) + Send + 'static) -> String {
    use std::io::{Read, Write};
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for s in l.incoming() {
            let Ok(mut s) = s else { continue };
            let (mut buf, mut b) = (vec![], [0u8; 4096]);
            while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                match s.read(&mut b) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => buf.extend_from_slice(&b[..n]),
                }
            }
            let target = String::from_utf8_lossy(&buf).split_whitespace().nth(1).unwrap_or("/").to_owned();
            let (status, body) = answer(&target);
            let _ = write!(s, "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            let _ = s.write_all(&body);
        }
    });
    format!("http://127.0.0.1:{port}")
}

const PROGRAM: &str = "#!/bin/sh\nprintf \"%s\\n\" \"$*\" \"$INYON_TOKEN\" > \"$HOME/ran\"\n";
const ENGINE: &str = "#!/bin/sh\necho engine\n";

/// What the stand-in hub serves: a program that writes down how it was run, and (`split`, a hub
/// from 0.13 on) the engine as a file of its own. `fixed_list` is served whatever the program is.
#[derive(Clone)]
struct PairHub {
    program: String,
    split: bool,
    fixed_list: Option<String>,
}

fn my_target() -> (&'static str, &'static str) {
    (if cfg!(target_os = "macos") { "darwin" } else { "linux" }, if cfg!(target_arch = "aarch64") { "arm64" } else { "x64" })
}

impl PairHub {
    fn sums(&self) -> String {
        let (os, arch) = my_target();
        let engine = if self.split { format!("{}  inyon-engine-{os}-{arch}\n", digest(ENGINE.as_bytes())) } else { String::new() };
        format!("{}  inyon-{os}-{arch}\n{}  inyon-other-x64\n{engine}", digest(self.program.as_bytes()), digest(b"another build"))
    }
}

fn pair_hub(split: bool) -> (String, std::sync::Arc<std::sync::Mutex<PairHub>>) {
    let state = std::sync::Arc::new(std::sync::Mutex::new(PairHub { program: PROGRAM.into(), split, fixed_list: None }));
    let s = state.clone();
    let hub = serve(move |target| {
        let h = s.lock().unwrap().clone();
        let (os, arch) = my_target();
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let mine = query.split('&').any(|p| p == format!("os={os}")) && query.split('&').any(|p| p == format!("arch={arch}"));
        match path {
            "/node/install.sh" => (200, INSTALL_SH.as_bytes().to_vec()),
            "/node/SHA256SUMS" => (200, h.fixed_list.clone().unwrap_or_else(|| h.sums()).into_bytes()),
            "/node/inyon" if mine => (200, h.program.into_bytes()),
            "/node/inyon-engine" if mine && h.split => (200, ENGINE.as_bytes().to_vec()),
            _ => (404, b"not found".to_vec()),
        }
    });
    (hub, state)
}

struct Ran {
    code: i32,
    err: String,
    ran: Option<String>,
    bin: std::path::PathBuf,
}

/// Runs `cmd` to its end, killing it and failing the test once it has run `secs` seconds.
fn run_for(cmd: &mut std::process::Command, secs: u64) -> std::process::Output {
    let mut child = cmd.spawn().unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    while child.try_wait().unwrap().is_none() {
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("{cmd:?} still ran after {secs}s");
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    child.wait_with_output().unwrap()
}

/// Runs a one-liner as a machine would, under a throwaway HOME.
fn run_pair(cmd: &str, env: &[(&str, &str)]) -> Ran {
    let home = std::env::temp_dir().join(format!("inyon-pair-{}", crate::http::random_hex(6)));
    std::fs::create_dir_all(&home).unwrap();
    let o = run_for(
        std::process::Command::new("sh")
            .arg("-c")
            .arg(cmd)
            .env_clear()
            .env("HOME", &home)
            .env("PATH", "/usr/local/bin:/usr/bin:/bin")
            .envs(env.iter().copied())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped()),
        30,
    );
    Ran {
        code: o.status.code().unwrap_or(-1),
        err: String::from_utf8_lossy(&o.stderr).into_owned(),
        ran: std::fs::read_to_string(home.join("ran")).ok(),
        bin: home.join(".inyon/bin"),
    }
}

impl Drop for Ran {
    fn drop(&mut self) {
        if let Some(home) = self.bin.parent().and_then(Path::parent) {
            let _ = std::fs::remove_dir_all(home);
        }
    }
}

fn sha(s: &str) -> String {
    digest(s.as_bytes())
}

#[cfg(unix)]
#[test]
fn the_one_liner_installs_what_the_hub_vouched_for_and_has_it_join_with_the_token_kept_out_of_argv() {
    let (hub, state) = pair_hub(false);
    let cmd = unix_pair_command(&hub, "tok_123-abc", &sha(INSTALL_SH), &sha(&state.lock().unwrap().sums()), None);
    assert!(!cmd.split(" sh -c '").nth(1).unwrap().contains("tok_123-abc"));
    let r = run_pair(&cmd, &[]);
    assert!(!r.err.contains("checksum"), "{}", r.err);
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(std::fs::read_to_string(r.bin.join("inyon")).unwrap(), PROGRAM);
    assert_eq!(r.ran.as_deref(), Some(format!("join {hub}\ntok_123-abc\n").as_str()));
    assert!(!r.bin.join("inyon.download").exists());
}

#[cfg(unix)]
#[test]
fn a_swapped_install_sh_checksum_list_or_program_is_refused_and_nothing_runs() {
    let (hub, state) = pair_hub(false);
    let sums = state.lock().unwrap().sums();
    let swapped_installer = run_pair(&unix_pair_command(&hub, "tok", &sha("something else"), &sha(&sums), None), &[]);
    assert_ne!(swapped_installer.code, 0);
    assert!(swapped_installer.err.contains("install.sh did not match"), "{}", swapped_installer.err);
    assert_eq!(swapped_installer.ran, None);

    let swapped_sums = run_pair(&unix_pair_command(&hub, "tok", &sha(INSTALL_SH), &sha("a different list"), None), &[]);
    assert_ne!(swapped_sums.code, 0);
    assert!(swapped_sums.err.contains("The hub's checksums did not match"), "{}", swapped_sums.err);
    assert_eq!(swapped_sums.ran, None);

    // the list as vouched for, but the program swapped on the way
    let fixed = {
        let mut h = state.lock().unwrap();
        h.fixed_list = Some(h.sums());
        h.program = "#!/bin/sh\necho \"not what the hub built\" > \"$HOME/ran\"\n".into();
        h.fixed_list.clone().unwrap()
    };
    let swapped_program = run_pair(&unix_pair_command(&hub, "tok", &sha(INSTALL_SH), &sha(&fixed), None), &[]);
    assert_ne!(swapped_program.code, 0);
    assert!(swapped_program.err.contains("inyon did not match its checksum"), "{}", swapped_program.err);
    assert_eq!(swapped_program.ran, None);
    assert!(!swapped_program.bin.join("inyon").exists());
}

#[cfg(unix)]
#[test]
fn from_0_19_a_machine_joins_with_the_command_alone_though_the_hub_lists_the_engine() {
    let (hub, state) = pair_hub(true);
    let sums = state.lock().unwrap().sums();
    // INYON_LEGACY_ENGINE=1 asked for the Bun engine before 0.23; it's no longer read
    for env in [&[][..], &[("INYON_LEGACY_ENGINE", "1")][..]] {
        let r = run_pair(&unix_pair_command(&hub, "tok_789", &sha(INSTALL_SH), &sha(&sums), None), env);
        assert_eq!(r.code, 0, "{}", r.err);
        assert_eq!(std::fs::read_to_string(r.bin.join("inyon")).unwrap(), PROGRAM);
        assert!(!r.bin.join("inyon-engine").exists());
        assert!(!r.err.contains("engine"), "{}", r.err);
        assert_eq!(r.ran.as_deref(), Some(format!("join {hub}\ntok_789\n").as_str()));
    }
}

// ---------- node-update-scripts.test.ts: the Windows script this hub still sends ----------

#[test]
fn the_windows_script_plain_ascii_launches_as_its_own_task_checks_the_download_before_touching_the_exe() {
    let ps1 = HUB_UPDATE_PS1;
    // Windows PowerShell 5.1 reads a file without a BOM as the local code page
    assert!(ps1.is_ascii());
    let at = |s: &str| ps1.find(s).unwrap_or_else(|| panic!("no {s:?} in hub-update.ps1"));
    // -Launch only re-starts it as a one-off task, then ends
    let launch = at("if ($Launch) {");
    assert!(at("Register-ScheduledTask -TaskName $UpdateTask") > launch);
    assert!(at("exit 0") < at("function Get-Hub"));
    // download, then the checksum, and only then the swap and restart
    let download = at("Get-File \"$hub/node/tether-node.exe.gz?arch=$Arch\" $gz");
    let check = at("(Get-FileHash -Algorithm SHA256 $new).Hash.ToLower() -ne $Want");
    let swap = at("Move-Retry $Exe $old");
    assert!(download < check && check < swap);
    assert!(at("Stop-Node\n  Start-Node") > swap);
    // a new copy that doesn't come up, or anything failing after the swap, brings the old one back,
    // and tether-node is left running
    let failed = at("} catch {\n  $msg");
    assert!(failed > at("if (-not (Test-Started)) { throw"));
    assert!(at("Move-Retry $old $Exe") > failed);
    assert!(at("if (-not (Get-Node)) { try { Start-Node }") > at("Move-Retry $old $Exe"));
    // the task removes itself, however it ended
    assert!(at("Unregister-ScheduledTask -TaskName $UpdateTask") > at("} finally {"));
    // the hub address comes from install.ps1's .env; nothing from the hub is put into a command
    assert!(ps1.contains("Get-Content (Join-Path $Dir '.env')"));
}

// Windows-only cmdlets as stubs that log what they're asked. Starting tether-node writes node.json
// for the new copy when NEW_STARTS=1, and throws when FAIL_START=1.
const PS_STUBS: &str = r#"
function Node-Started {
  $d = Join-Path $env:USERPROFILE '.tether-node'
  $isNew = (Get-Content (Join-Path $d 'tether-node.exe') -Raw) -match 'new'
  if ($isNew -and $env:FAIL_START -eq '1') { throw 'task start blocked' }
  if ($isNew -and $env:NEW_STARTS -eq '1') { Set-Content (Join-Path $d 'node.json') ('{"pid":' + $PID + ',"version":"0.6.3"}') }
}
function Get-ScheduledTask { param($TaskName, $ErrorAction) if ($TaskName -eq 'Tether node') { 'task' } }
function Stop-ScheduledTask { param($TaskName, $ErrorAction) Add-Content $env:CALLS "stop $TaskName" }
function Start-ScheduledTask { param($TaskName) Add-Content $env:CALLS "start $TaskName"; if ($TaskName -eq 'Tether node') { Node-Started } }
function Start-Process { param($FilePath, $ArgumentList, $WorkingDirectory, $WindowStyle) Add-Content $env:CALLS "start-process $FilePath" }
function Register-ScheduledTask { param($TaskName, $Action, $Principal, $Settings, [switch]$Force) Add-Content $env:CALLS "register $TaskName $($Action.Argument)" }
function New-ScheduledTaskAction { param($Execute, $Argument) [pscustomobject]@{ Execute = $Execute; Argument = $Argument } }
function New-ScheduledTaskPrincipal { param($UserId, $LogonType, $RunLevel) 'principal' }
function New-ScheduledTaskSettingsSet { param([switch]$AllowStartIfOnBatteries, [switch]$DontStopIfGoingOnBatteries, $ExecutionTimeLimit) 'settings' }
function Unregister-ScheduledTask { param($TaskName, $Confirm, $ErrorAction) Add-Content $env:CALLS "unregister $TaskName" }
function Get-Process { param($Name, $Id, $ErrorAction) if ($Id -and $Id -eq $PID) { [pscustomobject]@{ Id = $PID } } }
function Stop-Process { param([switch]$Force, $ErrorAction) }
function Start-Sleep { param($Seconds, $Milliseconds) }
"#;

/// PowerShell, where one is installed (PWSH=<path>, or pwsh on PATH); these tests skip without it.
fn pwsh() -> Option<std::path::PathBuf> {
    if let Some(p) = std::env::var_os("PWSH").filter(|p| !p.is_empty()) {
        return Some(p.into());
    }
    std::env::split_paths(&std::env::var_os("PATH")?).map(|d| d.join("pwsh")).find(|p| p.is_file())
}

struct Ps1Run {
    exe: String,
    status: String,
    files: Vec<String>,
    calls: Vec<String>,
}

/// Runs the filled-in Windows script under a throwaway USERPROFILE whose tether-node.exe is "old
/// exe", against a local server standing in for the hub. Nothing touches a real service.
fn run_ps1(pwsh: &Path, want: Option<&str>, env: &[(&str, &str)], launch: bool) -> Ps1Run {
    let gz = gzip(b"new exe").unwrap();
    let hub = serve(move |target| if target == "/node/tether-node.exe.gz?arch=x64" { (200, gz.clone()) } else { (404, b"no".to_vec()) });
    let root = std::env::temp_dir().join(format!("inyon-hub-update-{}", crate::http::random_hex(6)));
    // a home with a space and a quote in it: everything must stay quoted
    let home = root.join("home dir/o'brien");
    let profile = home.join("profile");
    let dir = profile.join(".tether-node");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("tether-node.exe"), "old exe").unwrap();
    std::fs::write(dir.join(".env"), format!("TETHER_HUB={hub}/\r\nTETHER_TOKEN=secret\r\n")).unwrap();
    std::fs::write(dir.join("update-status"), "running\n").unwrap();
    std::fs::write(root.join("stubs.ps1"), PS_STUBS).unwrap();
    let script = dir.join("update.ps1");
    let sum = want.map_or_else(|| sha("new exe"), str::to_owned);
    std::fs::write(&script, inyon_core::hosts::node_update::fill_update_script(HUB_UPDATE_PS1, &sum, "0.6.3", "x64").unwrap()).unwrap();
    let q = |p: &Path| p.to_string_lossy().replace('\'', "''");
    let cmd = format!(". '{}'; & '{}'{}", q(&root.join("stubs.ps1")), q(&script), if launch { " -Launch" } else { "" });
    // the TS gives a run 30 s, and the one that restores the old exe 60
    run_for(
        std::process::Command::new(pwsh)
            .args(["-NoProfile", "-NonInteractive", "-Command", &cmd])
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", &home)
            .env("USERPROFILE", &profile)
            .env("CALLS", root.join("calls"))
            .envs(env.iter().copied())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null()),
        60,
    );
    let read = |f: &str| std::fs::read_to_string(dir.join(f)).unwrap().trim().to_owned();
    let mut files: Vec<String> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    files.sort();
    let calls = std::fs::read_to_string(root.join("calls")).unwrap_or_default().trim().split('\n').map(str::to_owned).collect();
    let r = Ps1Run { exe: read("tether-node.exe"), status: read("update-status"), files, calls };
    let _ = std::fs::remove_dir_all(&root);
    r
}

#[test]
fn windows_script_under_powershell_a_mismatched_download_changes_nothing() {
    let Some(pwsh) = pwsh() else {
        eprintln!("no PowerShell here: skipped");
        return;
    };
    let r = run_ps1(&pwsh, Some(&sha("what the hub meant to send")), &[("NEW_STARTS", "1")], false);
    assert_eq!(r.exe, "old exe");
    assert_eq!(r.status, "failed: The download didn't match the hub's checksum, so nothing was changed");
    assert_eq!(r.files, [".env", "tether-node.exe", "update-status", "update.log", "update.ps1"]);
    assert!(!r.calls.iter().any(|c| c == "stop Tether node"));
}

#[test]
fn windows_script_under_powershell_replaced_restarted_and_done_once_the_new_version_runs() {
    let Some(pwsh) = pwsh() else {
        eprintln!("no PowerShell here: skipped");
        return;
    };
    let r = run_ps1(&pwsh, None, &[("NEW_STARTS", "1")], false);
    assert_eq!(r.exe, "new exe");
    assert_eq!(r.status, "done 0.6.3");
    assert!(!r.files.iter().any(|f| f.contains(".old-") || f.ends_with(".new") || f.ends_with(".gz")), "{:?}", r.files);
    assert_eq!(r.calls, ["stop Tether node", "start Tether node", "unregister Tether update"]);
}

#[test]
fn windows_script_under_powershell_a_new_version_that_doesnt_start_or_a_restart_that_throws_puts_the_old_one_back() {
    let Some(pwsh) = pwsh() else {
        eprintln!("no PowerShell here: skipped");
        return;
    };
    let failed = regex::Regex::new(r"^failed: (The new version didn't start, so the previous one is running again|task start blocked)$").unwrap();
    for env in [&[][..], &[("NEW_STARTS", "1"), ("FAIL_START", "1")][..]] {
        let r = run_ps1(&pwsh, None, env, false);
        assert_eq!(r.exe, "old exe");
        assert!(failed.is_match(&r.status), "{}", r.status);
        assert!(!r.files.iter().any(|f| f.contains(".old-")), "{:?}", r.files);
        assert_eq!(r.calls[r.calls.len() - 2], "start Tether node", "{:?}", r.calls);
    }
}

#[test]
fn windows_script_under_powershell_launch_only_starts_itself_as_a_one_off_task() {
    let Some(pwsh) = pwsh() else {
        eprintln!("no PowerShell here: skipped");
        return;
    };
    let r = run_ps1(&pwsh, None, &[], true);
    assert_eq!(r.exe, "old exe");
    assert_eq!(r.status, "running");
    assert_eq!(r.calls.len(), 2, "{:?}", r.calls);
    assert!(
        regex::Regex::new(r#"^register Tether update -NoProfile -NonInteractive -ExecutionPolicy Bypass -WindowStyle Hidden -File ".*update\.ps1"$"#)
            .unwrap()
            .is_match(&r.calls[0]),
        "{}",
        r.calls[0]
    );
    assert_eq!(r.calls[1], "start Tether update");
}

// made with the minisign binary and apps/cli/tests/keys, as in apps/cli/src/signing.rs
const TEST_KEY: &str = "RWQqNVK9mQOKMvLzyK5hNRZhdkrEQAMzJCRgzTyujgXyvmvHB0y3/zRm";
const SIGNED: &str = "abc  file-a\nabc  inyon-0.14.0.apk\n";
const SIGNED_SIG: &str = "untrusted comment: signature from minisign secret key\n\
    RUQqNVK9mQOKMkab6flmvsRkTJgrMwvlSr3HLaTaxl1/BKaIsjpexn6/YRrw9Nco1CyBNYXpD14RN4tyV+x91G0EBDlaQWFoVgg=\n\
    trusted comment: timestamp:1791906365\tfile:SUMS\thashed\n\
    hHmf6FdFURutVLVvePMp+3/HYEQlat81SrV37cY/HJXIVYmOh/WB1gOW4L2bQb9BQzY4ZopTYpjFX9UXykjRBw==\n\
";

/// A release with this list and signature (None: no .minisig); every name asked is noted.
fn signed_release(asked: Rc<RefCell<Vec<String>>>, sums: &'static str, sig: Option<&'static str>) -> Get {
    Rc::new(move |url: String| {
        asked.borrow_mut().push(url.rsplit('/').next().unwrap_or("").to_owned());
        Box::pin(async move {
            Ok(match (url.rsplit('/').next().unwrap_or(""), sig) {
                ("SHA256SUMS", _) => (200, sums.as_bytes().to_vec()),
                ("SHA256SUMS.minisig", Some(s)) => (200, s.as_bytes().to_vec()),
                _ => (404, b"no".to_vec()),
            })
        })
    })
}

#[test]
fn the_release_checksums_count_only_with_a_good_signature_from_the_release_key() {
    let asked = Rc::new(RefCell::new(Vec::new()));
    let ok = block(signed_sums("http://r", TEST_KEY, "0.14.0", signed_release(asked.clone(), SIGNED, Some(SIGNED_SIG))));
    assert_eq!(ok.unwrap(), SIGNED);
    // a good list of another release, an old copy on a mirror, isn't this hub's
    let e = block(signed_sums("http://r", TEST_KEY, "0.15.0", signed_release(asked.clone(), SIGNED, Some(SIGNED_SIG)))).unwrap_err();
    assert_eq!(e.status, Some(502));
    assert!(e.message.contains("aren't the ones for Inyon 0.15.0 (they don't list inyon-0.15.0.apk)"), "{}", e.message);
    // a list changed by one byte, no signature, another key, and a changed trusted comment are each refused, before any file
    let retimed: &'static str = Box::leak(SIGNED_SIG.replace("timestamp:1791906365", "timestamp:1791906366").into_boxed_str());
    for (sums, sig, key) in
        [("abd  file-a\nabc  inyon-0.14.0.apk\n", Some(SIGNED_SIG), TEST_KEY), (SIGNED, None, TEST_KEY), (SIGNED, Some(SIGNED_SIG), RELEASE_KEY), (SIGNED, Some(retimed), TEST_KEY)]
    {
        let asked = Rc::new(RefCell::new(Vec::new()));
        let e = block(signed_sums("http://r", key, "0.14.0", signed_release(asked.clone(), sums, sig))).unwrap_err();
        assert_eq!(e.status, Some(502), "{}", e.message);
        assert!(asked.borrow().iter().all(|n| n.starts_with("SHA256SUMS")), "{:?}", asked.borrow());
    }
}

#[test]
fn only_a_mock_release_may_name_another_signing_key() {
    // SAFETY: no other test in this binary reads INYON_SIGNING_KEY
    unsafe { std::env::set_var("INYON_SIGNING_KEY", format!("untrusted comment: test key\n{TEST_KEY}\n")) };
    assert_eq!(signing_key("http://127.0.0.1:8000/download"), TEST_KEY);
    for base in [
        format!("https://github.com/{RELEASES_REPO}/releases/download/v{VERSION}"),
        "https://inyon.tech/download".into(),
        "https://inyon.abhashchakraborty.tech/download".into(),
        "HTTPS://GitHub.com/abhash-chakraborty/inyon/releases/latest/download".into(),
    ] {
        assert_eq!(signing_key(&base), RELEASE_KEY, "{base}");
    }
    assert_eq!(signing_key("https://github.com/someone-else/inyon/releases/download/v1"), TEST_KEY);
    unsafe { std::env::remove_var("INYON_SIGNING_KEY") };
    assert_eq!(signing_key("http://127.0.0.1:8000/download"), RELEASE_KEY);
}

#[test]
fn a_new_system_is_prepared_while_an_add_a_machine_command_waits_or_for_a_paired_machine_of_that_system() {
    let now = 1_800_000_000_000u64;
    let node = |id: &str, v: serde_json::Value| -> inyon_protocol::HostConfig {
        let mut c = serde_json::json!({ "id": id, "name": id, "kind": "node" });
        c.as_object_mut().unwrap().extend(v.as_object().unwrap().clone());
        serde_json::from_value(c).unwrap()
    };
    let os = |m: &'static [(&'static str, &'static str)]| move |id: &str| m.iter().find(|(k, _)| *k == id).map(|(_, v)| v.to_string());
    let local: inyon_protocol::HostConfig = serde_json::from_value(serde_json::json!({ "id": "local", "name": "hub", "kind": "local" })).unwrap();
    // nothing paired, nothing waiting: no
    assert!(!may_prepare("linux", &[local], os(&[]), now));
    // an unused token from the last day: any system
    assert!(may_prepare("windows", &[node("new", serde_json::json!({ "tokenIssuedAt": now - 3_600_000 }))], os(&[]), now));
    assert!(!may_prepare("windows", &[node("old", serde_json::json!({ "tokenIssuedAt": now - 2 * 86_400_000 }))], os(&[]), now));
    // a paired Linux machine updates; it doesn't open Windows
    let paired = [node("box", serde_json::json!({ "pairedAt": now - 1000 }))];
    assert!(may_prepare("linux", &paired, os(&[("box", "Linux")]), now));
    assert!(!may_prepare("windows", &paired, os(&[("box", "Linux")]), now));
    assert!(may_prepare("darwin", &paired, os(&[("box", "Darwin")]), now));
    assert!(may_prepare("windows", &paired, os(&[("box", "Windows")]), now));
    // one that hasn't said what it runs yet could be anything
    assert!(may_prepare("windows", &paired, os(&[]), now));
}

#[test]
fn nobody_signed_in_can_make_the_hub_build_or_download_inyon_for_a_system_no_machine_needs() {
    let (dir, d) = checkout_dist(&[]);
    let asked = Rc::new(std::cell::Cell::new(false));
    let a = asked.clone();
    let f: LocalBoxFuture<'static, Result<Signed, HubError>> = Box::pin(async move {
        a.set(true);
        Err(http(502, "the release was asked"))
    });
    *d.sums.borrow_mut() = Some(f.shared());
    let results = block(async {
        vec![
            d.engine_for("linux-arm64").await.map(|_| ()),
            d.command_for("darwin-x64").await.map(|_| ()),
            d.windows_program().await.map(|_| ()),
            d.busybox_for("arm64").await.map(|_| ()),
            d.unix_sums_for().await.map(|_| ()),
        ]
    });
    let built = std::fs::read_dir(&d.out).map(|r| r.count()).unwrap_or(0);
    let _ = std::fs::remove_dir_all(&dir);
    for r in results {
        let e = r.unwrap_err();
        assert_eq!(e.status, Some(503));
        assert!(e.message.contains("Open \"Add a machine\" on the hub"), "{}", e.message);
    }
    assert!(!asked.get());
    assert_eq!(built, 0, "something was built");
}

#[test]
fn a_paired_linux_machine_doesnt_make_a_checkout_build_macos_engines_for_the_list_of_checksums() {
    let targets = ["linux-x64", "linux-arm64", "darwin-x64", "darwin-arm64"];
    let linux_only = |os: &str| os == "linux";
    assert_eq!(unprepared(targets, |_| false, linux_only), Some("darwin-x64"));
    // what's built already is handed out
    assert_eq!(unprepared(targets, |t| t.starts_with("darwin"), linux_only), None);
    // while an "Add a machine" command waits, any system
    assert_eq!(unprepared(targets, |_| false, |_| true), None);
}
