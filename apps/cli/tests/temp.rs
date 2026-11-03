//! `inyon temp`: the listing, what may be deleted and why not, previews, and the command end to end
//! with HOME and the temp folder pointed at a throwaway folder. Nothing outside it is touched.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use inyon_cli::temp::{self, Root};
use serde_json::Value;

const SESSION: &str = "7409eef9-d507-4808-a592-0bfabe896e3a";
const DAY_MS: f64 = 86_400_000.0;

struct Box {
    dir: PathBuf,
    home: PathBuf,
    tmp: PathBuf,
}

impl Box {
    fn new(tag: &str) -> Box {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("inyon-temp-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // the real path, so what the listing says compares equal (macOS's /tmp is a link, Windows
        // may name it RUNNER~1), without the \\?\ Windows puts on it: no agent writes paths that way
        let dir = PathBuf::from(inyon_cli::pages::strip_verbatim(&s(&fs::canonicalize(&dir).unwrap())));
        let b = Box { home: dir.join("home"), tmp: dir.join("tmp"), dir };
        fs::create_dir_all(&b.home).unwrap();
        fs::create_dir_all(&b.tmp).unwrap();
        b
    }

    fn roots(&self) -> Vec<Root> {
        temp::known_roots(&self.home, &self.tmp)
    }

    /// Claude Code's temp folder in this box.
    fn claude(&self) -> PathBuf {
        self.roots()[0].path.clone()
    }

    fn file(&self, p: &Path, body: &[u8]) -> PathBuf {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
        p.to_path_buf()
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_inyon"));
        c.args(args).env("HOME", &self.home).env("USERPROFILE", &self.home).env("CLAUDE_CODE_TMPDIR", &self.tmp).env("TMPDIR", &self.tmp).current_dir(&self.dir);
        c
    }

    fn json(&self, args: &[&str]) -> (i32, Value) {
        let mut a = args.to_vec();
        a.push("--json");
        let o = self.cmd(&a).output().unwrap();
        let out = String::from_utf8_lossy(&o.stdout);
        (o.status.code().unwrap_or(-1), serde_json::from_str(out.trim()).unwrap_or_else(|_| panic!("not JSON: {out} {}", String::from_utf8_lossy(&o.stderr))))
    }
}

impl Drop for Box {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn s(p: &Path) -> String {
    p.to_string_lossy().into_owned()
}

/// Sets when `p` last changed. Windows wants the right to write attributes for that, which a
/// plain open for reading doesn't ask for, and a folder only opens with backup semantics.
fn touch(p: &Path, t: SystemTime) {
    let mut o = fs::OpenOptions::new();
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_WRITE_ATTRIBUTES, FILE_FLAG_BACKUP_SEMANTICS
        o.access_mode(0x100).custom_flags(0x0200_0000);
    }
    #[cfg(not(windows))]
    o.read(true);
    o.open(p).unwrap_or_else(|e| panic!("{}: {e}", p.display())).set_modified(t).unwrap();
}

/// Sets the time of `p` and everything in it `days` back, deepest first so folders keep theirs.
fn age(p: &Path, days: u64) {
    let t = SystemTime::now() - Duration::from_secs(days * 86_400);
    if p.is_dir() && !p.is_symlink() {
        for e in fs::read_dir(p).unwrap().flatten() {
            age(&e.path(), days);
        }
    }
    if !p.is_symlink() {
        touch(p, t);
    }
}

/// What Claude Code calls a project's temp folder: its path with everything but letters and digits as dashes.
fn slug(p: &Path) -> String {
    s(p).chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

/// A box with something in every root, the way agents leave them.
fn filled(tag: &str) -> Box {
    let b = Box::new(tag);
    let project = b.home.join("dev").join("custom-tmux");
    fs::create_dir_all(&project).unwrap();
    let session = b.claude().join(slug(&project)).join(SESSION);
    for i in 0..6 {
        b.file(&session.join("images").join(format!("{i}.png")), &[i as u8; 10]);
        // apart in time, so newest first means something
        touch(&session.join("images").join(format!("{i}.png")), SystemTime::now() - Duration::from_secs(600 - i * 60));
    }
    b.file(&session.join("scratchpad").join("notes.md"), b"# notes\n");
    // a project whose folder is gone since
    b.file(&b.claude().join(slug(&b.home.join("gone").join("old-app"))).join("0b2ac1de-0000-4000-8000-000000000000").join("tasks").join("t.output"), b"done\n");
    b.file(&b.claude().join("shots").join("a.png"), &[1; 100]);
    b.file(&b.claude().join("loose.txt"), b"hello");
    age(&b.claude().join("shots"), 10);
    b.file(&b.home.join(".claude/paste-cache/0a49e8c1.txt"), b"pasted");
    b.file(&b.home.join(".claude/shell-snapshots/snapshot-bash-1.sh"), b"export A=1\n");
    b.file(&b.home.join(".inyon/uploads/2026-10-20/sess/120000-ab12cd-shot.png"), &[2; 50]);
    age(&b.home.join(".inyon/uploads/2026-10-20"), 4);
    b.file(&b.home.join(".gemini/tmp/proj/chats/session-1.json"), b"{}");
    b.file(&b.home.join(".gemini/tmp/proj/logs.json"), b"[]");
    b.file(&b.home.join(".gemini/tmp/proj/scratch.txt"), b"scratch");
    b.file(&b.home.join(".codex/shell_snapshots/x.sh"), b"x");
    // history that must never show up
    b.file(&b.home.join(".claude/projects/p/s.jsonl"), b"{}");
    b.file(&b.home.join(".codex/sessions/2026/s.jsonl"), b"{}");
    b
}

#[test]
fn the_listing_has_one_item_per_session_folder_and_loose_thing() {
    let b = filled("list");
    let now = inyon_cli::state::now_ms();
    let l = temp::list_from(&b.roots(), now);
    assert_eq!(l.v, 1);
    let names: Vec<(&str, &str)> = l.items.iter().map(|i| (i.root.as_str(), i.name.as_str())).collect();
    for want in [
        ("claude-tmp", format!("custom-tmux · {}", &SESSION[..8])),
        ("claude-tmp", "gone-old-app · 0b2ac1de".to_string()),
        ("claude-tmp", "shots".into()),
        ("claude-tmp", "loose.txt".into()),
        ("claude-paste", "0a49e8c1.txt".into()),
        ("claude-shell", "snapshot-bash-1.sh".into()),
        ("inyon-uploads", "2026-10-20".into()),
        ("gemini-tmp", "proj · scratch.txt".into()),
        ("codex-shell", "x.sh".into()),
    ] {
        assert!(names.contains(&(want.0, want.1.as_str())), "{want:?} missing from {names:?}");
    }
    assert_eq!(l.items.len(), 9, "{names:?}");
    // transcripts, history and Gemini's chats are never items
    for i in &l.items {
        assert!(!i.path.contains("projects") && !i.path.contains("sessions") && !i.path.contains("chats") && !i.path.ends_with("logs.json"), "{}", i.path);
    }
    let session = l.items.iter().find(|i| i.name.starts_with("custom-tmux")).unwrap();
    assert_eq!((session.kind.as_str(), session.files, session.size), ("dir", 7, 6 * 10 + 8));
    assert!(session.recent);
    // the four newest images, newest first
    let thumbs: Vec<String> = session.images.iter().map(|p| Path::new(p).file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(thumbs, ["5.png", "4.png", "3.png", "2.png"]);
    assert_eq!(session.id, format!("claude-tmp:{}/{SESSION}", slug(&b.home.join("dev/custom-tmux"))));
    let shots = l.items.iter().find(|i| i.name == "shots").unwrap();
    assert!(!shots.recent && now - shots.modified > 9.0 * DAY_MS);
    let loose = l.items.iter().find(|i| i.name == "loose.txt").unwrap();
    assert_eq!((loose.kind.as_str(), loose.size, loose.files), ("file", 5, 1));
    // each root's totals add up its items; roots that aren't there say so
    let claude = l.roots.iter().find(|r| r.root.id == "claude-tmp").unwrap();
    assert_eq!(claude.items, 4);
    assert_eq!(claude.size, l.items.iter().filter(|i| i.root == "claude-tmp").map(|i| i.size).sum::<u64>());
    assert!(l.roots.iter().all(|r| r.exists));
    let bare = Box::new("bare");
    let empty = temp::list_from(&bare.roots(), now);
    assert!(empty.items.is_empty() && empty.roots.iter().all(|r| !r.exists || r.root.id == "claude-tmp"));
    assert!(temp::roots_in(&bare.home, &bare.tmp).is_empty());
}

#[test]
fn a_project_name_falls_back_to_the_slug_past_the_home_folder() {
    let home = Path::new("/nowhere-home/me");
    assert_eq!(temp::project_name("-nowhere-home-me-work-site-v2", home), "work-site-v2");
    assert_eq!(temp::project_name("-nowhere-home-me", home), "me");
    assert_eq!(temp::project_name("-elsewhere-x", home), "elsewhere-x");
}

#[test]
fn remove_refuses_whatever_is_not_strictly_inside_a_root() {
    let b = filled("refuse");
    let roots = b.roots();
    let outside = b.file(&b.dir.join("outside").join("keep.txt"), b"keep");
    let refused = |p: &str| {
        let r = temp::remove_in(&roots, &[p.to_string()]);
        assert!(r.removed.is_empty(), "{p} was removed");
        r.failed[0].1.clone()
    };
    assert_eq!(refused(""), temp::NO_PATH);
    assert_eq!(refused("loose.txt"), temp::NOT_ABSOLUTE);
    assert_eq!(refused(&format!("{}/shots/../loose.txt", s(&b.claude()))), temp::DOTS);
    assert_eq!(refused(&format!("{}/./loose.txt", s(&b.claude()))), temp::DOTS);
    assert_eq!(refused(&s(&b.claude().join("not-there"))), temp::NOT_FOUND);
    assert_eq!(refused(&s(&b.claude())), "That's the whole Claude Code temp files folder. Delete what's inside it instead");
    assert_eq!(refused(&s(&b.home.join(".inyon/uploads"))), "That's the whole Inyon uploads folder. Delete what's inside it instead");
    assert_eq!(refused(&s(&outside)), temp::OUTSIDE);
    assert_eq!(refused(&s(&b.home.join(".claude/projects/p/s.jsonl"))), temp::OUTSIDE);
    assert_eq!(refused(&s(&b.home)), temp::OUTSIDE);
    assert_eq!(refused(&s(&b.home.join(".gemini/tmp/proj/chats"))), temp::GEMINI_CHATS);
    assert_eq!(refused(&s(&b.home.join(".gemini/tmp/proj/chats/session-1.json"))), temp::GEMINI_CHATS);
    assert_eq!(refused(&s(&b.home.join(".gemini/tmp/proj/logs.json"))), temp::GEMINI_CHATS);
    // the project's folder holds the chats too
    assert_eq!(refused(&s(&b.home.join(".gemini/tmp/proj"))), temp::GEMINI_CHATS);
    assert!(outside.exists() && b.claude().exists() && b.home.join(".gemini/tmp/proj/chats/session-1.json").exists());
    // what's left of the gemini folder can go
    let r = temp::remove_in(&roots, &[s(&b.home.join(".gemini/tmp/proj/scratch.txt"))]);
    assert_eq!((r.removed.len(), r.freed), (1, 7));
}

#[cfg(unix)]
#[test]
fn a_link_is_deleted_as_a_link_and_never_followed_outside() {
    use std::os::unix::fs::symlink;
    let b = filled("links");
    let roots = b.roots();
    let outside = b.dir.join("outside");
    let keep = b.file(&outside.join("keep.txt"), b"keep");
    // a link inside a root, to a folder outside: the link goes, the folder stays
    let link = b.claude().join("to-outside");
    symlink(&outside, &link).unwrap();
    // a path through that link leads outside
    let r = temp::remove_in(&roots, &[s(&link.join("keep.txt"))]);
    assert_eq!(r.failed[0].1, temp::LINK_OUTSIDE);
    assert!(keep.exists());
    let r = temp::remove_in(&roots, &[s(&link)]);
    assert_eq!(r.removed, [s(&link)]);
    assert_eq!(r.freed, 0);
    assert!(!link.exists() && fs::symlink_metadata(&link).is_err());
    assert!(keep.exists(), "the link's target was deleted");
    // a link to a file outside, inside a session folder that is deleted whole
    let session = b.claude().join(slug(&b.home.join("dev/custom-tmux"))).join(SESSION);
    symlink(&keep, session.join("keep-link")).unwrap();
    let r = temp::remove_in(&roots, &[s(&session)]);
    assert_eq!(r.removed.len(), 1, "{:?}", r.failed);
    assert!(!session.exists() && keep.exists());
    // a root that is itself a link (a dotfiles setup): what's in it may go, the link and its folder stay
    let real_paste = b.dir.join("real-paste");
    fs::rename(b.home.join(".claude/paste-cache"), &real_paste).unwrap();
    symlink(&real_paste, b.home.join(".claude/paste-cache")).unwrap();
    let r = temp::remove_in(&roots, &[s(&b.home.join(".claude/paste-cache")), s(&b.home.join(".claude/paste-cache/0a49e8c1.txt"))]);
    assert_eq!(r.failed, [(s(&b.home.join(".claude/paste-cache")), "That's the whole Claude Code pasted text folder. Delete what's inside it instead".to_string())]);
    assert_eq!(r.removed.len(), 1);
    assert!(!real_paste.join("0a49e8c1.txt").exists() && real_paste.is_dir() && b.home.join(".claude/paste-cache").is_symlink());
    // previews don't follow links either
    symlink(&keep, b.claude().join("peek")).unwrap();
    assert_eq!(temp::preview_in(&roots, &s(&b.claude().join("peek")), temp::IMAGE_MAX).unwrap_err(), temp::A_LINK);
}

#[test]
fn remove_deletes_items_and_counts_what_it_freed() {
    let b = filled("remove");
    let roots = b.roots();
    let session = b.claude().join(slug(&b.home.join("dev/custom-tmux"))).join(SESSION);
    let inside = session.join("images").join("0.png");
    let r = temp::remove_in(&roots, &[s(&inside), s(&session), s(&b.claude().join("loose.txt")), s(&b.dir.join("nope"))]);
    assert_eq!(r.freed, 68 + 5);
    assert_eq!(r.removed.len(), 3, "{:?}", r);
    assert_eq!(r.failed, [(s(&b.dir.join("nope")), temp::OUTSIDE.to_string())]);
    assert!(!session.exists() && !b.claude().join("loose.txt").exists());
    // the project folder and the root stay
    assert!(session.parent().unwrap().exists() && b.claude().exists());
}

#[test]
fn files_lists_an_item_newest_first_with_kinds() {
    let b = filled("files");
    let roots = b.roots();
    let session = b.claude().join(slug(&b.home.join("dev/custom-tmux"))).join(SESSION);
    b.file(&session.join("tasks").join("b1.output"), b"build ok\n");
    b.file(&session.join("tasks").join("noext"), b"plain words");
    b.file(&session.join("tasks").join("blob.bin"), &[0, 159, 146, 150]);
    let (list, cut) = temp::files_in(&roots, &s(&session)).unwrap();
    assert!(!cut);
    assert_eq!(list.len(), 10);
    assert!(list.windows(2).all(|w| w[0].modified >= w[1].modified));
    let kind = |rel: &str| list.iter().find(|f| f.rel == rel).unwrap_or_else(|| panic!("{rel}")).kind.clone();
    assert_eq!(kind("images/3.png"), "image");
    assert_eq!(kind("scratchpad/notes.md"), "text");
    assert_eq!(kind("tasks/b1.output"), "text");
    assert_eq!(kind("tasks/noext"), "text");
    assert_eq!(kind("tasks/blob.bin"), "other");
    // a loose file is an item of one
    let (one, _) = temp::files_in(&roots, &s(&b.claude().join("loose.txt"))).unwrap();
    assert_eq!((one.len(), one[0].rel.as_str()), (1, "loose.txt"));
    assert_eq!(temp::files_in(&roots, &s(&b.home.join(".claude/projects"))).unwrap_err(), temp::OUTSIDE);
    assert_eq!(temp::files_in(&roots, &s(&b.claude())).unwrap_err(), "That's the whole Claude Code temp files folder. Delete what's inside it instead");
}

#[test]
fn previews_send_text_as_text_and_images_as_base64() {
    let b = filled("preview");
    let roots = b.roots();
    let dir = b.claude().join("pv");
    let html = b.file(&dir.join("page.html"), b"<script>alert(1)</script>");
    let p = temp::preview_in(&roots, &s(&html), temp::IMAGE_MAX).unwrap();
    assert_eq!((p.mime.as_str(), p.text.as_deref(), p.base64.is_none(), p.truncated), ("text/html", Some("<script>alert(1)</script>"), true, false));
    let png = b.file(&dir.join("a.png"), &[137, 80, 78, 71, 1, 2, 3]);
    let p = temp::preview_in(&roots, &s(&png), temp::IMAGE_MAX).unwrap();
    assert_eq!((p.mime.as_str(), p.base64.as_deref(), p.text.is_none(), p.size), ("image/png", Some("iVBORwECAw=="), true, 7));
    // too big to send
    let p = temp::preview_in(&roots, &s(&png), 4).unwrap();
    assert!(p.base64.is_none() && p.truncated);
    let big = b.file(&dir.join("big.log"), &vec![b'a'; temp::TEXT_MAX as usize + 10]);
    let p = temp::preview_in(&roots, &s(&big), temp::IMAGE_MAX).unwrap();
    assert_eq!((p.text.unwrap().len() as u64, p.truncated, p.size), (temp::TEXT_MAX, true, temp::TEXT_MAX + 10));
    // a character cut in half at the end isn't sent half
    let mut cut = vec![b'a'; temp::TEXT_MAX as usize - 1];
    cut.extend("é".as_bytes());
    let cut = b.file(&dir.join("cut.txt"), &cut);
    assert_eq!(temp::preview_in(&roots, &s(&cut), temp::IMAGE_MAX).unwrap().text.unwrap().len() as u64, temp::TEXT_MAX - 1);
    let bin = b.file(&dir.join("x.bin"), &[0, 1, 2]);
    let p = temp::preview_in(&roots, &s(&bin), temp::IMAGE_MAX).unwrap();
    assert!(p.text.is_none() && p.base64.is_none());
    assert_eq!(p.mime, "application/octet-stream");
    assert_eq!(temp::preview_in(&roots, &s(&dir), temp::IMAGE_MAX).unwrap_err(), temp::NOT_A_FILE);
    let secret = b.file(&b.home.join(".ssh/id_ed25519"), b"secret");
    assert_eq!(temp::preview_in(&roots, &s(&secret), temp::IMAGE_MAX).unwrap_err(), temp::OUTSIDE);
}

#[test]
fn ages_and_sizes_read_plainly() {
    assert_eq!(temp::parse_age("7d").unwrap(), 7.0 * DAY_MS);
    assert_eq!(temp::parse_age("24h").unwrap(), DAY_MS);
    assert_eq!(temp::parse_age("30m").unwrap(), 1_800_000.0);
    assert_eq!(temp::parse_age("2w").unwrap(), 14.0 * DAY_MS);
    for bad in ["", "7", "d", "7x", "-1d", "1.5d"] {
        assert_eq!(temp::parse_age(bad).unwrap_err(), temp::BAD_AGE, "{bad}");
    }
    assert_eq!(temp::size_text(12), "12 B");
    assert_eq!(temp::size_text(340 * 1024), "340 KB");
    assert_eq!(temp::size_text(3_543_348_019), "3.3 GB");
    assert_eq!(temp::age_text(10_000_000.0, 10_000_000.0 - 5.0 * 60_000.0), "5 min ago");
    assert_eq!(temp::age_text(DAY_MS * 3.0, 0.0), "3 d ago");
}

#[test]
fn the_command_lists_shows_and_deletes() {
    let b = filled("command");
    let (code, l) = b.json(&["temp"]);
    assert_eq!(code, 0);
    assert_eq!((l["v"].as_u64(), l["ok"].as_bool()), (Some(1), Some(true)));
    assert_eq!(l["items"].as_array().unwrap().len(), 9);
    assert!(l["roots"].as_array().unwrap().iter().any(|r| r["id"] == "claude-tmp" && r["path"] == s(&b.claude()) && r["exists"] == true));
    let plain = b.cmd(&["temp", "ls"]).output().unwrap();
    let text = String::from_utf8_lossy(&plain.stdout);
    assert!(text.starts_with("Agent temp files on ") && text.contains("Claude Code temp files"), "{text}");
    let session = b.claude().join(slug(&b.home.join("dev/custom-tmux"))).join(SESSION);
    let (code, f) = b.json(&["temp", "files", &s(&session)]);
    assert_eq!((code, f["files"].as_array().unwrap().len(), f["truncated"].as_bool()), (0, 7, Some(false)));
    // by id too
    let id = format!("claude-tmp:{}/{SESSION}", slug(&b.home.join("dev/custom-tmux")));
    assert_eq!(b.json(&["temp", "files", &id]).1["path"], s(&session));
    let (code, p) = b.json(&["temp", "show", &s(&session.join("scratchpad/notes.md"))]);
    assert_eq!((code, p["text"].as_str()), (0, Some("# notes\n")));
    let (code, e) = b.json(&["temp", "show", &s(&b.home.join(".claude/projects/p/s.jsonl"))]);
    assert_eq!((code, e["ok"].as_bool(), e["error"].as_str()), (1, Some(false), Some(temp::OUTSIDE)));
    // a refusal is in the answer and the exit code
    let (code, r) = b.json(&["temp", "rm", &s(&b.home)]);
    assert_eq!((code, r["ok"].as_bool(), r["failed"][0][1].as_str()), (1, Some(false), Some(temp::OUTSIDE)));
    // many paths at once on stdin, as the hubs send them
    let mut c = b.cmd(&["temp", "rm", "--stdin", "--json"]);
    let mut child = c.stdin(Stdio::piped()).stdout(Stdio::piped()).spawn().unwrap();
    let list = serde_json::to_string(&[s(&b.claude().join("loose.txt")), s(&b.home.join(".codex/shell_snapshots/x.sh"))]).unwrap();
    std::io::Write::write_all(child.stdin.as_mut().unwrap(), list.as_bytes()).unwrap();
    drop(child.stdin.take());
    let o = child.wait_with_output().unwrap();
    let r: Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!((o.status.code(), r["removed"].as_array().unwrap().len(), r["freed"].as_u64()), (Some(0), 2, Some(6)));
    assert!(!b.claude().join("loose.txt").exists());
}

#[test]
fn a_folder_too_deep_to_read_through_counts_as_recent_and_clean_keeps_it() {
    let b = filled("deep");
    let deep = b.claude().join("deep");
    let mut bottom = deep.clone();
    for _ in 0..temp::DEPTH_MAX + 2 {
        bottom = bottom.join("d");
    }
    let fresh = b.file(&bottom.join("fresh.txt"), b"still being written");
    age(&deep, 10);
    // everything the walk can reach is old; the one file past its depth was written just now
    touch(&fresh, SystemTime::now());
    let l = temp::list_from(&b.roots(), inyon_cli::state::now_ms());
    let item = |name: &str| l.items.iter().find(|i| i.name == name).unwrap_or_else(|| panic!("{name}"));
    assert!(item("deep").recent, "a walk that stopped short called the folder old");
    assert!(!item("shots").recent);
    let (code, r) = b.json(&["temp", "clean", "--older-than", "7d", "--yes"]);
    assert_eq!((code, r["removed"].as_array().unwrap().clone()), (0, vec![Value::from(s(&b.claude().join("shots")))]));
    assert!(fresh.exists());
}

#[test]
fn clean_lists_first_and_skips_recent_items() {
    let b = filled("clean");
    // without --yes and no terminal: only a list
    let (code, r) = b.json(&["temp", "clean", "--older-than", "3d"]);
    assert_eq!(code, 0);
    let would: Vec<String> = r["would"].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
    assert_eq!(would, [s(&b.claude().join("shots")), s(&b.home.join(".inyon").join("uploads").join("2026-10-20"))]);
    assert!(b.claude().join("shots").exists());
    let (code, r) = b.json(&["temp", "clean", "--older-than", "7d", "--yes"]);
    assert_eq!((code, r["removed"].as_array().unwrap().len(), r["freed"].as_u64()), (0, 1, Some(100)));
    assert!(!b.claude().join("shots").exists() && b.home.join(".inyon/uploads/2026-10-20").exists());
    // recent items stay even with an age of a minute
    let (_, r) = b.json(&["temp", "clean", "--older-than", "1m", "--yes"]);
    assert!(r["removed"].as_array().unwrap().iter().all(|p| !p.as_str().unwrap().contains(SESSION)));
    assert!(b.claude().join(slug(&b.home.join("dev/custom-tmux"))).join(SESSION).exists());
    let (code, e) = b.json(&["temp", "clean", "--older-than", "soon"]);
    assert_eq!((code, e["error"].as_str()), (1, Some(temp::BAD_AGE)));
}
