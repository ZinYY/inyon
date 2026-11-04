//! Fast, fail-open receiver for Claude's official MessageDisplay hook.
//! No shell, network, Python, or wait for stdin EOF on the critical path.
use std::fs::{self, OpenOptions, TryLockError};
use std::io::{self, Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};
use serde::Deserialize;
use serde_json::Value;
use inyon_protocol::claude_stream::{valid_id, DisplayEvent, DisplaySnapshot, MAX_TEXT};

fn read_event(input: impl Read) -> Option<Value> {
    let mut json = serde_json::Deserializer::from_reader(input.take((MAX_TEXT * 2) as u64));
    Value::deserialize(&mut json).ok()
}

fn private_file(path: &Path) -> io::Result<fs::File> {
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)] {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

fn replace_snapshot(temp: &Path, file: &Path) -> io::Result<()> {
    // Windows readers such as CRT/head may omit FILE_SHARE_DELETE. A brief read
    // must not drop a batch; never hold up Claude for an indefinitely open file.
    let until = Instant::now() + Duration::from_millis(200);
    loop {
        match fs::rename(temp, file) {
            Err(e) if cfg!(windows) && matches!(e.raw_os_error(), Some(5 | 32 | 33)) && Instant::now() < until => std::thread::sleep(Duration::from_millis(2)),
            result => return result,
        }
    }
}

pub fn read_snapshot(session: &str) {
    if !valid_id(session) { return; }
    let path = crate::paths::home().join(format!(".inyon/claude-live/{session}.json"));
    // Rust's Windows File reader shares deletion, so the normal polling reader
    // never prevents an atomic replacement. The size cap also bounds bad files.
    if let Ok(file) = fs::File::open(path) {
        let _ = io::copy(&mut file.take((MAX_TEXT * 2 + 1) as u64), &mut io::stdout().lock());
    }
}

fn save(home: &Path, input: Value) -> io::Result<()> {
    let session = input["session_id"].as_str().unwrap_or("");
    let name = input["hook_event_name"].as_str().unwrap_or("");
    if !valid_id(session) || !["MessageDisplay", "UserPromptSubmit", "SessionStart", "SessionEnd"].contains(&name) { return Ok(()); }
    let dir = home.join(".inyon/claude-live");
    fs::create_dir_all(&dir)?;
    #[cfg(unix)] {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700))?;
    }
    let file = dir.join(format!("{session}.json"));
    let lock = private_file(&file.with_extension("lock"))?;
    let until = Instant::now() + Duration::from_millis(200);
    loop {
        match lock.try_lock() {
            Ok(()) => break,
            Err(TryLockError::WouldBlock) if Instant::now() < until => std::thread::sleep(Duration::from_millis(2)),
            Err(_) => return Ok(()),
        }
    }
    if name != "MessageDisplay" {
        // /clear, /resume and a new prompt cannot replay the preceding turn's preview.
        let _ = fs::remove_file(&file);
        return Ok(());
    }
    let Ok(event) = serde_json::from_value::<DisplayEvent>(input) else { return Ok(()) };
    // Never let a damaged or unexpectedly large cache delay Claude.
    let previous = fs::metadata(&file).ok().filter(|m| m.len() <= (MAX_TEXT * 2) as u64)
        .and_then(|_| fs::read(&file).ok()).and_then(|b| serde_json::from_slice(&b).ok());
    let Some(value) = DisplaySnapshot::apply(previous, event, crate::state::now_ms() as u64) else { return Ok(()) };
    let temp = file.with_extension(format!("{}.tmp", std::process::id()));
    let result = (|| {
        let mut out = private_file(&temp)?;
        out.set_len(0)?;
        out.write_all(&serde_json::to_vec(&value)?)?;
        drop(out);
        replace_snapshot(&temp, &file)
    })();
    if result.is_err() { let _ = fs::remove_file(&temp); }
    result
}

pub fn run() {
    if let Some(event) = read_event(io::stdin().lock()) { let _ = save(&crate::paths::home(), event); }
    // An empty result preserves Claude's original display and transcript.
    let _ = writeln!(io::stdout().lock(), "{{}}");
}

#[cfg(test)]
mod tests {
    use super::*;
    const S: &str = "11111111-1111-4111-8111-111111111111";
    fn event(index: u64, text: &str, finished: bool) -> DisplayEvent {
        serde_json::from_value(serde_json::json!({"hook_event_name":"MessageDisplay","session_id":S,"turn_id":S,"message_id":S,"index":index,"delta":text,"final":finished})).unwrap()
    }

    #[cfg(windows)]
    #[test]
    fn a_brief_non_delete_sharing_reader_does_not_drop_a_batch() {
        use std::os::windows::fs::OpenOptionsExt;
        let dir=std::env::temp_dir().join(format!("inyon-display-reader-{}",std::process::id()));
        let input=|i,delta| serde_json::json!({"hook_event_name":"MessageDisplay","session_id":S,"turn_id":S,"message_id":S,"index":i,"delta":delta,"final":false});
        save(&dir,input(0,"one")).unwrap();
        let path=dir.join(format!(".inyon/claude-live/{S}.json"));
        let reader=OpenOptions::new().read(true).share_mode(3).open(&path).unwrap();
        let release=std::thread::spawn(move || { std::thread::sleep(Duration::from_millis(40)); drop(reader); });
        save(&dir,input(1," two")).unwrap();
        release.join().unwrap();
        let snapshot:DisplaySnapshot=serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(snapshot.text,"one two"); assert!(snapshot.valid); assert_eq!(snapshot.index,1);
        let held=OpenOptions::new().read(true).share_mode(3).open(&path).unwrap();
        let start=Instant::now(); assert!(save(&dir,input(2," blocked")).is_err());
        assert!(start.elapsed()<Duration::from_secs(1)); drop(held);
        save(&dir,input(2," three")).unwrap();
        let snapshot:DisplaySnapshot=serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(snapshot.text,"one two three"); assert!(snapshot.valid);
        fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn utf8_input_finishes_without_waiting_for_eof() {
        struct OpenPipe(std::io::Cursor<Vec<u8>>);
        impl Read for OpenPipe { fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
            assert!(self.0.position() < self.0.get_ref().len() as u64, "must not wait for EOF");
            self.0.read(bytes)
        }}
        let bytes = serde_json::to_vec(&serde_json::json!({"hook_event_name":"MessageDisplay","session_id":S,"turn_id":S,"message_id":S,"index":0,"delta":"你好 🎸\n","final":false})).unwrap();
        assert_eq!(read_event(OpenPipe(std::io::Cursor::new(bytes))).unwrap()["delta"], "你好 🎸\n");
    }
    #[test]
    fn ordered_unicode_batches_and_final_empty_batch_are_lossless() {
        let first = DisplaySnapshot::apply(None, event(0, "你好", false), 10);
        let dup = DisplaySnapshot::apply(first, event(0, "你好", false), 11);
        let next = DisplaySnapshot::apply(dup, event(1, " 🎸\n", false), 12);
        let done = DisplaySnapshot::apply(next, event(2, "", true), 13).unwrap();
        assert_eq!(done.text, "你好 🎸\n"); assert!(done.valid && done.finished); assert_eq!(done.started_at, 10);
    }
    #[test]
    fn gaps_never_join_unrelated_or_incomplete_text() {
        let first = DisplaySnapshot::apply(None, event(0, "one", false), 10);
        let bad = DisplaySnapshot::apply(first, event(2, "three", false), 12).unwrap();
        assert!(!bad.valid); assert!(bad.text.is_empty());
        assert!(DisplaySnapshot::apply(None, event(1, "missing start", false), 10).is_none());
        let mut wrong = event(0, "bad", false); wrong.session_id = "../../outside".into();
        assert!(DisplaySnapshot::apply(None, wrong, 10).is_none());
    }
    #[test]
    fn atomic_snapshot_replaces_existing_file_and_new_prompt_clears_it() {
        let dir=std::env::temp_dir().join(format!("inyon-display-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        let input=|i,delta,done| serde_json::json!({"hook_event_name":"MessageDisplay","session_id":S,"turn_id":S,"message_id":S,"index":i,"delta":delta,"final":done});
        save(&dir,input(0,"第一行\n",false)).unwrap();
        save(&dir,input(1,"第二行 🎸",true)).unwrap();
        let path=dir.join(format!(".inyon/claude-live/{S}.json"));
        let snapshot:DisplaySnapshot=serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(snapshot.text,"第一行\n第二行 🎸"); assert!(snapshot.finished && snapshot.valid);
        save(&dir,serde_json::json!({"session_id":S,"hook_event_name":"UserPromptSubmit"})).unwrap();
        assert!(!path.exists());
        // A late batch from the preceding turn cannot recreate its deleted preview.
        save(&dir,input(2,"late",true)).unwrap();
        assert!(!path.exists());
        fs::remove_dir_all(dir).unwrap();
    }
}
