//! features/workspace.test.ts

use serde_json::json;

use super::*;
use crate::test_support::rt::run;
use crate::test_support::sandbox::Sandbox;
use crate::test_support::{machine, sh, TempDir};

/// git with an identity of its own and no global config, as the TS test's helper runs it.
const GIT_ENV: &str = "export GIT_CONFIG_GLOBAL=/dev/null GIT_CONFIG_NOSYSTEM=1 GIT_AUTHOR_NAME=T GIT_AUTHOR_EMAIL=t@t GIT_COMMITTER_NAME=T GIT_COMMITTER_EMAIL=t@t; ";

fn git(dir: &TempDir, cmd: &str) -> (i32, String, String) {
    sh(&format!("{GIT_ENV}cd '{}' && git -c user.name=T -c user.email=t@t -c commit.gpgsign=false {cmd}", dir.s()), None, &[])
}

/// The machine "ws" with one pane, %1, working in `dir`; its commands run in a sandbox there.
fn setup(dir: &TempDir) {
    Sandbox::install_with(&dir.s(), GIT_ENV);
    machine("ws", "local", json!([{ "id": "%1", "cwd": dir.s() }]));
}

#[test]
fn paths_stay_inside_the_session_folder() {
    assert_eq!(inside("src/a.ts").unwrap(), "src/a.ts");
    assert_eq!(inside("./src/").unwrap(), "src");
    assert_eq!(inside("").unwrap(), "");
    for bad in ["../x", "a/../../b", "a\nb"] {
        assert_eq!(inside(bad).unwrap_err().message, "Bad path", "{bad}");
    }
    // a leading slash means the session folder, not the machine's root
    assert_eq!(inside("/etc/passwd").unwrap(), "etc/passwd");
}

#[test]
fn git_status_renames_and_counts_are_read_from_porcelain_output() {
    let status = "## main...origin/main [ahead 2, behind 1]\0 M src/a.ts\0R  new.ts\0old.ts\0?? notes.md\0D  gone.txt\0";
    let numstat = "3\t1\tsrc/a.ts\0".to_owned() + "0\t0\t\0old.ts\0new.ts\0" + "0\t4\tgone.txt\0";
    assert_eq!(
        serde_json::to_value(parse_status(status, &numstat)).unwrap(),
        json!({
            "branch": "main", "upstream": "origin/main", "ahead": 2, "behind": 1,
            "files": [
                { "path": "src/a.ts", "status": "M", "add": 3, "del": 1 },
                { "path": "new.ts", "from": "old.ts", "status": "R", "add": 0, "del": 0 },
                { "path": "notes.md", "status": "?", "add": 0, "del": 0 },
                { "path": "gone.txt", "status": "D", "add": 0, "del": 4 },
            ],
        })
    );
    assert_eq!(parse_status("## No commits yet on main\0", "").branch.as_deref(), Some("main"));
}

#[test]
fn unified_diff_hunks_keep_their_line_numbers() {
    let h = parse_hunks("diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1,2 +1,3 @@ fn\n a\n-b\n+c\n+d\n@@ -10 +11 @@\n-x\n+y\n\\ No newline at end of file\n");
    assert_eq!(
        serde_json::to_value(h).unwrap(),
        json!([
            { "oldStart": 1, "oldLines": 2, "newStart": 1, "newLines": 3, "lines": [" a", "-b", "+c", "+d"] },
            { "oldStart": 10, "oldLines": 1, "newStart": 11, "newLines": 1, "lines": ["-x", "+y", "\\ No newline at end of file"] },
        ])
    );
}

#[test]
fn a_folder_that_is_not_a_repository_says_so() {
    let dir = TempDir::new("inyon-ws-");
    run(|| async move {
        setup(&dir);
        assert_eq!(serde_json::to_value(git_status("ws", "%1").await.unwrap()).unwrap(), json!({ "repo": false, "files": [] }));
    });
}

#[test]
fn files_the_working_tree_and_a_commit_on_a_real_repository() {
    let dir = TempDir::new("inyon-ws-");
    run(|| async move {
        setup(&dir);
        git(&dir, "init -q -b main");
        dir.write("a.txt", "one\ntwo\n");
        dir.write("src/b.ts", "export {}\n");
        std::fs::write(dir.path("pic.png"), [137u8, 80, 78, 71]).unwrap();
        std::fs::write(dir.path("blob.bin"), [1u8, 0, 2, 0]).unwrap();
        git(&dir, "add -A && git -c user.name=T -c user.email=t@t -c commit.gpgsign=false commit -q -m first");

        let root = list_dir("ws", "%1", "").await.unwrap();
        let names: Vec<String> = root.entries.iter().map(|e| format!("{} {}", if e.dir { "d" } else { "f" }, e.name)).collect();
        assert_eq!(names, ["d src", "f a.txt", "f blob.bin", "f pic.png"]);
        let src = list_dir("ws", "%1", "src").await.unwrap();
        assert_eq!((src.entries[0].name.as_str(), src.entries[0].dir, src.entries[0].size), ("b.ts", false, 10));
        assert_eq!(list_dir("ws", "%1", "nope").await.unwrap_err().message, "No such folder");

        assert_eq!(
            serde_json::to_value(read_file("ws", "%1", "a.txt").await.unwrap()).unwrap(),
            json!({ "kind": "text", "path": "a.txt", "text": "one\ntwo\n", "size": 8, "truncated": false })
        );
        let pic = serde_json::to_value(read_file("ws", "%1", "pic.png").await.unwrap()).unwrap();
        assert_eq!((pic["kind"].as_str(), pic["type"].as_str(), pic["data"].as_str()), (Some("image"), Some("image/png"), Some("iVBORw==")));
        let blob = serde_json::to_value(read_file("ws", "%1", "blob.bin").await.unwrap()).unwrap();
        assert_eq!((blob["kind"].as_str(), blob["size"].as_u64()), (Some("binary"), Some(4)));

        dir.write("a.txt", "one\n2\nthree\n");
        dir.write("new file.md", "# hi\n");
        let s = git_status("ws", "%1").await.unwrap();
        assert_eq!((s.repo, s.status.branch.as_deref(), s.remote), (true, Some("main"), Some(false)));
        assert_eq!(
            serde_json::to_value(&s.status.files).unwrap(),
            json!([
                { "path": "a.txt", "status": "M", "add": 2, "del": 1 },
                { "path": "new file.md", "status": "?", "add": 0, "del": 0 },
            ])
        );
        assert_eq!(git_diff("ws", "%1", "a.txt").await.unwrap().hunks[0].lines, [" one", "-two", "+2", "+three"]);
        let added = &git_diff("ws", "%1", "new file.md").await.unwrap().hunks[0];
        assert_eq!((added.new_start, added.lines.clone()), (1, vec!["+# hi".to_owned()]));

        assert_eq!(git_commit("ws", "%1", Some(&json!("  "))).await.unwrap_err().message, "Write a commit message");
        let c = git_commit("ws", "%1", Some(&json!("Second commit\n\nWith a body"))).await.unwrap();
        assert!(crate::re!(r"^[0-9a-f]{7,}$").is_match(&c.commit), "{}", c.commit);
        assert!(git_status("ws", "%1").await.unwrap().status.files.is_empty());
        assert_eq!(git(&dir, "log -1 --format=%B").1.trim(), "Second commit\n\nWith a body");
        assert_eq!(dir.read("new file.md"), "# hi\n");
    });
}
