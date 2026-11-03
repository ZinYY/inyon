//! features/uploads.test.ts, features/config-write.test.ts, features/skill-commands.test.ts and
//! the lruCache test of caches.test.ts.

use std::cell::RefCell;
use std::rc::Rc;

use futures_util::future::LocalBoxFuture;
use inyon_protocol::{AgentKind, HostConfig};
use serde_json::json;

use super::*;
use crate::hosts::host::{set_host_backends, ExecResult, HostBackend, OnData, OnLine, PtyHandle, StreamHandle};
use crate::platform::rt::Deferred;
use crate::test_support::rt::{run, wait};
use crate::test_support::sandbox::Sandbox;
use crate::test_support::{connected, machine, sh, TempDir};

/// The machine's shell, run for real, with the size of what each command was handed on stdin noted.
#[derive(Default)]
struct Counting {
    inputs: RefCell<Vec<usize>>,
}

impl HostBackend for Counting {
    fn exec(&self, _config: &HostConfig, cmd: &str, opts: ExecOptions) -> LocalBoxFuture<'static, HubResult<ExecResult>> {
        self.inputs.borrow_mut().push(opts.input.as_ref().map_or(0, Vec::len));
        let (code, stdout, stderr) = sh(cmd, opts.input.as_deref(), &[("HOME", "/nonexistent")]);
        Box::pin(async move { Ok(ExecResult { code, stdout, stderr }) })
    }
    fn stream(&self, _config: &HostConfig, _cmd: &str, _on_line: OnLine, _keep_stdin: bool) -> StreamHandle {
        StreamHandle::new(|| {}, Deferred::resolved(0))
    }
    fn pty(&self, _config: &HostConfig, _cmd: &str, _cols: u16, _rows: u16, _on_data: OnData) -> HubResult<PtyHandle> {
        Ok(PtyHandle::new(|_| {}, |_, _| {}, || {}, Deferred::resolved(0)))
    }
}

fn counting() -> Rc<Counting> {
    let b = Rc::new(Counting::default());
    let c = b.clone();
    set_host_backends(move |_| c.clone() as Rc<dyn HostBackend>);
    b
}

fn video() -> Vec<u8> {
    (0..5 * 1024 * 1024 + 123).map(|i: usize| (i * 7) as u8).collect()
}

fn piece(v: &[u8], at: usize, n: usize, name: &str) -> UploadFile {
    UploadFile::new(name, "video/mp4", &v[at..(at + n).min(v.len())])
}

fn at(total: usize, offset: usize, path: Option<&str>) -> PieceAt {
    PieceAt { total: total as f64, offset: offset as f64, path: path.map(str::to_owned) }
}

// ---------- uploads.test.ts ----------

#[test]
fn a_large_file_is_written_a_few_megabytes_at_a_time_never_in_one_go() {
    run(|| async {
        let home = TempDir::new("inyon-up-");
        connected("up", &home.s(), json!([]));
        let b = counting();
        let v = video();
        let u = upload_files("up", None, &[UploadFile::new("clip.mp4", "video/mp4", v.clone())]).await.unwrap();
        let inputs = b.inputs.borrow().clone();
        assert_eq!(inputs.len(), 3);
        assert!(inputs.iter().max().unwrap() <= &(2 * 1024 * 1024));
        assert_eq!(std::fs::read(&u[0].path).unwrap(), v);
        assert_eq!((u[0].name.as_str(), u[0].kind.as_str(), u[0].size), ("clip.mp4", "video/mp4", Num(v.len() as f64)));
    });
}

#[test]
fn the_phone_sends_pieces_they_join_into_the_file_in_order_only() {
    run(|| async {
        let home = TempDir::new("inyon-up-");
        connected("up", &home.s(), json!([]));
        Sandbox::install(&home.s());
        let v = video();
        let first = upload_piece("up", None, &piece(&v, 0, 3_000_000, "clip.mp4"), &at(v.len(), 0, None)).await.unwrap();
        assert!(first.path.starts_with(&format!("{}/.inyon/uploads/", home.s())));
        // a piece that doesn't follow on from what's there is refused rather than corrupting the file
        assert!(upload_piece("up", None, &piece(&v, 4_000_000, 10, "clip.mp4"), &at(v.len(), 4_000_000, Some(&first.path))).await.is_err());
        let last = upload_piece("up", None, &piece(&v, 3_000_000, v.len() - 3_000_000, "clip.mp4"), &at(v.len(), 3_000_000, Some(&first.path))).await.unwrap();
        assert_eq!(last, Uploaded { size: Num(v.len() as f64), ..first.clone() });
        assert_eq!(std::fs::read(&first.path).unwrap(), v);
    });
}

#[test]
fn pieces_only_go_into_the_uploads_folder() {
    run(|| async {
        let home = TempDir::new("inyon-up-");
        connected("up", &home.s(), json!([]));
        Sandbox::install(&home.s());
        let p = UploadFile::new("a.txt", "", "x");
        let h = home.s();
        for path in [Some("/etc/passwd".to_owned()), Some(format!("{h}/.inyon/uploads/../../.bashrc")), Some(format!("{h}/.ssh/authorized_keys")), None] {
            let e = upload_piece("up", None, &p, &at(10, 5, path.as_deref())).await.unwrap_err();
            assert_eq!(e.message, "Bad upload path", "{path:?}");
        }
        let e = upload_piece("up", None, &p, &at(200 * 1024 * 1024, 0, None)).await.unwrap_err();
        assert!(e.message.contains("larger than"), "{}", e.message);
        assert_eq!(e.status, Some(413));
    });
}

#[test]
fn a_piece_sent_twice_is_written_once() {
    run(|| async {
        let home = TempDir::new("inyon-up-");
        connected("up", &home.s(), json!([]));
        Sandbox::install(&home.s());
        let v = video();
        let first = upload_piece("up", None, &piece(&v, 0, 2_000_000, "again.mp4"), &at(v.len(), 0, None)).await.unwrap();
        let second = at(v.len(), 2_000_000, Some(&first.path));
        upload_piece("up", None, &piece(&v, 2_000_000, 2_000_000, "again.mp4"), &second).await.unwrap();
        upload_piece("up", None, &piece(&v, 2_000_000, 2_000_000, "again.mp4"), &second).await.unwrap();
        upload_piece("up", None, &piece(&v, 4_000_000, v.len() - 4_000_000, "again.mp4"), &at(v.len(), 4_000_000, Some(&first.path))).await.unwrap();
        assert_eq!(std::fs::read(&first.path).unwrap(), v);
    });
}

#[test]
fn two_files_of_the_same_name_sent_in_the_same_second_both_keep_their_bytes() {
    run(|| async {
        let home = TempDir::new("inyon-up-");
        connected("up", &home.s(), json!([]));
        Sandbox::install(&home.s());
        let u = upload_files("up", Some("%3"), &[UploadFile::new("screenshot.png", "", "FIRST"), UploadFile::new("screenshot.png", "", "SECOND")]).await.unwrap();
        assert_ne!(u[0].path, u[1].path);
        assert_eq!(std::fs::read_to_string(&u[0].path).unwrap(), "FIRST");
        assert_eq!(std::fs::read_to_string(&u[1].path).unwrap(), "SECOND");
        // no pane %3 on the machine: its id names the folder
        assert!(u[0].path.contains("/pane-3/"), "{}", u[0].path);
    });
}

#[test]
fn a_file_in_progress_is_a_part_file_and_takes_its_name_only_once_the_last_piece_is_in() {
    run(|| async {
        let home = TempDir::new("inyon-up-");
        connected("up", &home.s(), json!([]));
        Sandbox::install(&home.s());
        let v = video();
        let first = upload_piece("up", None, &piece(&v, 0, 1_000_000, "half.mp4"), &at(2_000_000, 0, None)).await.unwrap();
        // cut off here: an agent can't be handed a file that is half there
        assert!(!std::path::Path::new(&first.path).exists());
        assert!(std::path::Path::new(&format!("{}.part", first.path)).exists());
        upload_piece("up", None, &piece(&v, 1_000_000, 1_000_000, "half.mp4"), &at(2_000_000, 1_000_000, Some(&first.path))).await.unwrap();
        assert!(!std::path::Path::new(&format!("{}.part", first.path)).exists());
        assert_eq!(std::fs::metadata(&first.path).unwrap().len(), 2_000_000);
        // the last piece sent again, after its answer was lost: the whole file stays as it is
        upload_piece("up", None, &piece(&v, 1_000_000, 1_000_000, "half.mp4"), &at(2_000_000, 1_000_000, Some(&first.path))).await.unwrap();
        assert_eq!(std::fs::metadata(&first.path).unwrap().len(), 2_000_000);
    });
}

#[test]
fn the_janitor_deletes_old_uploads_leftover_part_files_and_the_oldest_past_the_size_limit() {
    let dir = TempDir::new("inyon-clean-");
    let file = |name: &str, kb: usize, days_old: u64, mins_old: u64| {
        let p = dir.path(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, vec![0u8; kb * 1024]).unwrap();
        let t = std::time::SystemTime::now() - std::time::Duration::from_secs(days_old * 86_400 + mins_old * 60);
        std::fs::File::options().write(true).open(&p).unwrap().set_modified(t).unwrap();
        p
    };
    let ancient = file("2026-09-01/a/old.png", 10, 40, 0);
    let stale = file("2026-10-14/b/cut.mp4.part", 10, 0, 90);
    let fresh = file("2026-10-14/b/going.mp4.part", 10, 0, 5);
    let chunk = file("2026-10-14/b/cut.mp4.part.chunk", 10, 0, 90);
    let big1 = file("2026-10-10/c/one.bin", 600, 4, 0);
    let big2 = file("2026-10-13/c/two.bin", 600, 1, 0);
    let today = file("2026-10-14/c/three.bin", 600, 0, 0);
    let (code, _, err) = sh(&clean_script(&dir.s(), 30.0, 1.0), None, &[]);
    assert_eq!(code, 0, "{err}");
    assert_eq!([&ancient, &stale, &chunk, &big1, &big2].map(|p| p.exists()), [false; 5]);
    // still being written, and today's file under the 1 MB cap, both stay
    assert_eq!([&fresh, &today].map(|p| p.exists()), [true; 2]);
    assert!(!dir.exists("2026-09-01"));
}

#[test]
fn the_cleaning_script_names_its_policy_file_and_limits_as_whole_numbers() {
    let s = clean_script("/h/.inyon/uploads", 7.9, 1024.0);
    assert!(s.starts_with("printf '%s %s\\n' 7 1024 > '/h/.inyon/upload-policy' 2>/dev/null; [ -d '/h/.inyon/uploads' ] || exit 0; "), "{s}");
    assert!(s.contains("-mtime +7 -exec rm -f {} +;") && s.contains("-gt 1048576 ]"), "{s}");
    // all of them, and no policy written for a one-off clean
    let all = clean_script("/h/.inyon/uploads", -1.0, 0.0);
    assert!(all.starts_with(" [ -d '/h/.inyon/uploads' ] || exit 0; find '/h/.inyon/uploads' -type f -exec rm -f {} +;"), "{all}");
    assert!(!all.contains("while"));
}

// ---------- config-write.test.ts ----------

/// A home with a space in its name, next to an unrelated file named after its first word.
struct CfgHome {
    root: TempDir,
    home: String,
    file: String,
}

fn cfg_home() -> CfgHome {
    let root = TempDir::new("inyon-cfg-");
    let home = format!("{}/space home", root.s());
    std::fs::create_dir_all(format!("{home}/.claude")).unwrap();
    let rt = connected("cfg", &home, json!([]));
    rt.borrow_mut().state.agents = vec![AgentKind::Claude];
    Sandbox::install(&home);
    let file = format!("{home}/.claude/settings.json");
    CfgHome { root, home, file }
}

#[test]
fn clearing_old_backups_removes_only_backups_whatever_spaces_the_path_has() {
    run(|| async {
        let c = cfg_home();
        c.root.write("space", "not a backup");
        std::fs::write(&c.file, "{}").unwrap();
        for i in 0..7 {
            std::fs::write(format!("{}.inyon-bak-2026-01-0{}T00-00-00", c.file, i + 1), "{}").unwrap();
        }
        assert_eq!(write_config("cfg", &c.file, r#"{"new":true}"#).await.unwrap(), Saved { ok: true });
        assert_eq!(c.root.read("space"), "not a backup");
        let backups = std::fs::read_dir(format!("{}/.claude", c.home)).unwrap().filter(|e| e.as_ref().unwrap().file_name().to_string_lossy().contains("inyon-bak")).count();
        assert_eq!(backups, 5);
        assert_eq!(std::fs::read_to_string(&c.file).unwrap(), r#"{"new":true}"#);
    });
}

#[test]
fn a_write_that_fails_is_reported_not_passed_off_as_saved() {
    // root writes anywhere, so the folder can't be made read-only for it
    if sh("id -u", None, &[]).1.trim() == "0" {
        return;
    }
    run(|| async {
        use std::os::unix::fs::PermissionsExt;
        let c = cfg_home();
        std::fs::write(&c.file, r#"{"old":true}"#).unwrap();
        let dir = format!("{}/.claude", c.home);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
        let r = write_config("cfg", &c.file, r#"{"new":true}"#).await;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(r.is_err());
        assert_eq!(std::fs::read_to_string(&c.file).unwrap(), r#"{"old":true}"#);
        assert!(!std::path::Path::new(&format!("{}.inyon-tmp", c.file)).exists());
    });
}

#[test]
fn a_config_file_that_is_not_json_or_not_the_agents_is_refused() {
    run(|| async {
        let c = cfg_home();
        let e = write_config("cfg", &c.file, "{ nope").await.unwrap_err();
        assert_eq!(e.status, Some(400));
        assert!(e.message.starts_with("Not valid JSON: "), "{}", e.message);
        let e = write_config("cfg", "/etc/passwd", "x").await.unwrap_err();
        assert_eq!((e.status, e.message.as_str()), (Some(403), "That file is not an agent config file Inyon manages"));
        // an empty file is fine: it clears the settings
        assert!(write_config("cfg", &c.file, "  ").await.is_ok());
        let listed = list_configs("cfg").await.unwrap();
        let mine = listed.iter().find(|f| f.path == c.file).unwrap();
        assert_eq!((mine.label.as_str(), mine.exists, mine.size), ("User settings", true, Num(2.0)));
        assert_eq!(read_config("cfg", &c.file).await.unwrap().content, "  ");
    });
}

// ---------- skill-commands.test.ts ----------

fn skill(dir: &str, name: &str, description: &str) {
    std::fs::create_dir_all(format!("{dir}/{name}")).unwrap();
    std::fs::write(format!("{dir}/{name}/SKILL.md"), format!("---\nname: {name}\ndescription: {description}\n---\n\nBody\n")).unwrap();
}

/// A repo with the pane open in a folder inside it, and the machine's HOME the test folder.
fn skills_machine(kind: &str) -> (TempDir, String) {
    let home = TempDir::new("inyon-skills-");
    let repo = format!("{}/repo", home.s());
    std::fs::create_dir_all(format!("{repo}/apps/web")).unwrap();
    let (code, _, err) = sh(&format!("git init -q '{repo}'"), None, &[]);
    assert_eq!(code, 0, "{err}");
    connected("skills-test", &home.s(), json!([{ "id": "%9", "cwd": format!("{repo}/apps/web"), "agent": { "kind": kind, "pid": 7, "state": "idle" } }]));
    Sandbox::install(&home.s());
    (home, repo)
}

#[test]
fn codex_lists_its_skills_as_dollar_name_from_the_repo_root_its_home_and_plugins() {
    run(|| async {
        let (home, repo) = skills_machine("codex");
        let h = home.s();
        skill(&format!("{repo}/.agents/skills"), "impeccable", "Design work");
        skill(&format!("{h}/.codex/skills"), "mine", "My own skill");
        skill(&format!("{h}/.codex/skills/.system"), "imagegen", "Make images");
        skill(&format!("{h}/.codex/plugins/cache/market/google-drive/0.1.9/skills"), "google-slides", "Old copy");
        skill(&format!("{h}/.codex/plugins/cache/market/google-drive/0.1.20/skills"), "google-slides", "Slides");
        let list = list_commands("skills-test", "%9").await.unwrap();
        let get = |n: &str| list.iter().find(|c| c.name == n).cloned();
        let imp = get("$impeccable").unwrap();
        assert_eq!((imp.source, imp.description.as_str()), (CommandSource::Project, "Design work"));
        assert_eq!(get("$mine").unwrap().source, CommandSource::Skill);
        assert_eq!(get("$imagegen").unwrap().source, CommandSource::Skill);
        // the newest cached version of a plugin wins
        let slides = get("$google-drive:google-slides").unwrap();
        assert_eq!((slides.source, slides.description.as_str()), (CommandSource::Plugin, "Slides"));
        // built-in commands are still there
        assert!(get("/model").is_some());
        // sorted as the app shows them
        let names: Vec<&str> = list.iter().map(|c| c.name.as_str()).collect();
        let mut sorted = names.clone();
        sorted.sort_by(|a, b| locale_compare(a, b));
        assert_eq!(names, sorted);
    });
}

#[test]
fn claude_finds_a_project_skill_in_the_repo_root_while_the_pane_is_in_a_folder_inside_it() {
    run(|| async {
        let (_home, repo) = skills_machine("claude");
        skill(&format!("{repo}/.claude/skills"), "tether-ui", "UI rules");
        let list = list_commands("skills-test", "%9").await.unwrap();
        assert_eq!(list.iter().find(|c| c.name == "/tether-ui").map(|c| c.source), Some(CommandSource::Project));
    });
}

// ---------- caches.test.ts ----------

#[test]
fn the_command_and_file_caches_drop_old_entries_and_keep_to_a_size() {
    run(|| async {
        let mut c = lru_cache::<i32>(50, 3);
        for k in ["a", "b", "c"] {
            c.set(k, 1);
        }
        c.get("a"); // used, so b is now the oldest
        c.set("d", 1);
        assert_eq!([c.get("a"), c.get("b"), c.get("c"), c.get("d")], [Some(1), None, Some(1), Some(1)]);
        assert_eq!(c.size(), 3);
        wait(60).await;
        assert_eq!(c.get("a"), None);
        c.set("e", 1);
        assert_eq!(c.size(), 1);
    });
}

// ---------- the rest of features.ts ----------

#[test]
fn task_output_is_read_only_from_task_files_and_loses_its_colours() {
    run(|| async {
        connected("t", "/home/me", json!([]));
        let f = crate::test_support::fake::Fake::install(|_, _, _| crate::test_support::fake::ok("\x1b[32mok\x1b[0m done\n"));
        let e = task_output("t", "/etc/passwd").await.unwrap_err();
        assert_eq!((e.status, e.message.as_str()), (Some(400), "Not a task output file"));
        let out = task_output("t", "/tmp/claude-1000/-p/s1/tasks/b1.output").await.unwrap();
        assert_eq!(out.output, "ok done\n");
        assert_eq!(f.cmds(), ["tail -c 80000 '/tmp/claude-1000/-p/s1/tasks/b1.output' 2>/dev/null; true"]);
    });
}

#[test]
fn transcripts_and_subagents_come_only_from_the_projects_folder() {
    run(|| async {
        connected("t", "/home/me", json!([]));
        let f = crate::test_support::fake::Fake::install(|_, cmd, _| {
            crate::test_support::fake::ok(if cmd.starts_with("for m in") {
                "a1\t{\"agentType\":\"Explore\",\"description\":\"Look\",\"toolUseId\":\"tu1\"}\nb2\tnot json\n"
            } else {
                ""
            })
        });
        for bad in ["/home/me/.claude/projects/../x.jsonl", "/etc/x.jsonl", "/home/me/.claude/projects/p/s.json"] {
            assert_eq!(list_subagents("t", bad).await.unwrap_err().message, "Bad transcript path", "{bad}");
        }
        let subs = list_subagents("t", "/home/me/.claude/projects/p/s.jsonl").await.unwrap();
        assert_eq!(
            serde_json::to_value(&subs).unwrap(),
            json!([
                { "agentId": "a1", "toolUseId": "tu1", "type": "Explore", "description": "Look", "path": "/home/me/.claude/projects/p/s/subagents/a1.jsonl" },
                { "agentId": "b2", "path": "/home/me/.claude/projects/p/s/subagents/b2.jsonl" },
            ])
        );
        assert_eq!(
            f.cmds()[0],
            "for m in '/home/me/.claude/projects/p/s/subagents'/*.meta.json; do [ -f \"$m\" ] && echo \"$(basename \"$m\" .meta.json)\t$(tr -d '\\n' < \"$m\")\"; done"
        );
        assert!(read_transcript("t", "/home/me/.claude/projects/p/s.jsonl").await.unwrap().is_empty());
    });
}

#[test]
fn transcripts_are_read_from_claude_projects_folders_only() {
    run(|| async {
        // one session keeps its transcripts elsewhere (CLAUDE_CONFIG_DIR)
        connected(
            "paths",
            "/home/me",
            json!([{ "id": "%1", "agent": { "kind": "claude", "pid": 7, "state": "idle", "transcriptPath": "/home/me/.claude-work/projects/-w/abc.jsonl" } }]),
        );
        crate::test_support::fake::Fake::install(|_, _, _| crate::test_support::fake::ok(""));
        for ok in ["/home/me/.claude/projects/-x/s.jsonl", "/home/me/.claude-work/projects/-w/abc/subagents/agent-1.jsonl"] {
            read_transcript("paths", ok).await.unwrap();
            list_subagents("paths", ok).await.unwrap();
        }
        for bad in [
            "/home/me/.claude/projects/../../.ssh/id.jsonl",
            "/home/me/.claude/projects/-x/./s.jsonl",
            "/etc/.claude/projects/x.jsonl",
            "/home/me/other/.claude/projects/x.jsonl",
            "relative/.claude/projects/x.jsonl",
            "/home/me/.claude/projects/-x/s.txt",
        ] {
            assert_eq!(read_transcript("paths", bad).await.unwrap_err().message, "Bad transcript path", "{bad}");
            assert_eq!(list_subagents("paths", bad).await.unwrap_err().message, "Bad transcript path", "{bad}");
        }
    });
}

#[test]
fn a_machine_not_connected_yet_has_no_home_to_put_files_in() {
    run(|| async {
        let rt = machine("off", "local", json!([]));
        rt.borrow_mut().hold_polls();
        let e = upload_usage("off").await.unwrap_err();
        assert_eq!((e.status, e.message.as_str()), (Some(503), "Machine is not connected yet"));
        let e = upload_usage("nobody").await.unwrap_err();
        assert_eq!((e.status, e.message.as_str()), (Some(404), "Unknown machine"));
    });
}

#[test]
fn upload_usage_reads_sizes_counts_and_expired_files() {
    run(|| async {
        connected("u", "/home/me", json!([]));
        let f = crate::test_support::fake::Fake::install(|_, _, _| crate::test_support::fake::ok("12 3 1\n"));
        let u = upload_usage("u").await.unwrap();
        assert_eq!(serde_json::to_value(&u).unwrap(), json!({ "bytes": 12288, "files": 3, "expired": 1, "dir": "/home/me/.inyon/uploads", "days": 7 }));
        assert_eq!(
            f.cmds(),
            ["[ -d '/home/me/.inyon/uploads' ] || { echo \"0 0 0\"; exit 0; }; echo \"$(du -sk '/home/me/.inyon/uploads' | cut -f1) $(find '/home/me/.inyon/uploads' -type f | wc -l) $(find '/home/me/.inyon/uploads' -type f -mtime +7 | wc -l)\""]
        );
    });
}

#[test]
fn file_names_are_made_safe_for_the_shell_and_short() {
    assert_eq!(safe_name("my photo (1).png"), "my_photo_1_.png");
    assert_eq!(safe_name("..hidden"), "hidden");
    assert_eq!(safe_name(""), "file");
    assert_eq!(safe_name("…"), "_");
    assert_eq!(safe_name(&"a".repeat(200)).len(), 120);
}
