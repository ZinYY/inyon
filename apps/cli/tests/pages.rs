//! `inyon pages` end to end: the real binary with HOME a throwaway folder, and the slug and folder
//! rules checked against the vectors both hubs are tested with (fixtures/pages/rules.json).

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

use inyon_cli::pages;
use serde_json::Value;

const AGENT_VARS: [&str; 8] =
    ["CLAUDECODE", "CLAUDE_CODE_SESSION_ID", "CODEX_SANDBOX", "CODEX_SANDBOX_NETWORK_DISABLED", "CODEX_THREAD_ID", "GEMINI_CLI", "OPENCODE", "INYON_DATA_DIR"];

struct Home {
    dir: PathBuf,
}

impl Home {
    fn new(tag: &str) -> Home {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("inyon-pages-{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // the real path, so the folders below compare equal to what the command writes (macOS's /tmp is a link)
        // and on Windows in the form people type, not the `\\?\C:\…` canonicalize gives
        Home { dir: PathBuf::from(pages::strip_verbatim(s(&fs::canonicalize(&dir).unwrap()))) }
    }

    /// A site folder under the home, with an index.html titled `title`.
    fn site(&self, rel: &str, title: &str) -> PathBuf {
        // one part at a time, so a Windows path has no '/' in it
        let d = rel.split('/').fold(self.dir.clone(), |d, part| d.join(part));
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join("index.html"), format!("<!doctype html><html><head><title>{title}</title></head><body></body></html>")).unwrap();
        d
    }

    fn file(&self) -> PathBuf {
        self.dir.join(".inyon").join("pages.json")
    }

    fn list(&self) -> Value {
        serde_json::from_str(&fs::read_to_string(self.file()).unwrap()).unwrap()
    }

    fn cmd(&self, args: &[&str]) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_inyon"));
        c.args(args).env("HOME", &self.dir).env("USERPROFILE", &self.dir).env_remove("INYON_HUB").env_remove("TETHER_HUB").current_dir(&self.dir);
        for v in AGENT_VARS {
            c.env_remove(v);
        }
        c
    }

    fn run(&self, args: &[&str]) -> (i32, String, String) {
        out(self.cmd(args))
    }

    fn json(&self, args: &[&str]) -> (i32, Value) {
        let mut a = args.to_vec();
        a.push("--json");
        let (code, stdout, _) = self.run(&a);
        (code, serde_json::from_str(stdout.trim()).unwrap_or_else(|_| panic!("not JSON: {stdout}")))
    }
}

impl Drop for Home {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn out(mut c: Command) -> (i32, String, String) {
    let o = c.output().unwrap();
    (o.status.code().unwrap_or(-1), String::from_utf8_lossy(&o.stdout).into_owned(), String::from_utf8_lossy(&o.stderr).into_owned())
}

fn s(p: &Path) -> &str {
    p.to_str().unwrap()
}

fn rules() -> Value {
    let f = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pages/rules.json");
    serde_json::from_str(&fs::read_to_string(f).unwrap()).unwrap()
}

#[test]
fn slugs_and_folders_follow_the_shared_rules() {
    let r = rules();
    assert_eq!(r["listMax"].as_u64(), Some(pages::LIST_MAX));
    for c in r["slugify"].as_array().unwrap() {
        assert_eq!(pages::slugify(c["text"].as_str().unwrap()), c["slug"].as_str().unwrap(), "slugify {c}");
    }
    for c in r["isSlug"].as_array().unwrap() {
        assert_eq!(pages::is_slug(c["s"].as_str().unwrap()), c["ok"].as_bool().unwrap(), "isSlug {c}");
    }
    for c in r["uniqueSlug"].as_array().unwrap() {
        let taken: Vec<&str> = c["taken"].as_array().unwrap().iter().map(|t| t.as_str().unwrap()).collect();
        assert_eq!(pages::unique_slug(c["base"].as_str().unwrap(), |x| taken.contains(&x)), c["slug"].as_str().unwrap(), "uniqueSlug {c}");
    }
    for c in r["sitePath"].as_array().unwrap() {
        assert_eq!(pages::site_path_problem(c["path"].as_str().unwrap(), c["home"].as_str().unwrap()), c["problem"].as_str(), "sitePath {c}");
    }
    // and the messages are the hub's own
    let core = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates/inyon-core/src/features/pages_core.rs")).unwrap();
    for m in [pages::BAD_SLUG, pages::NOT_ABSOLUTE, pages::TOO_WIDE, pages::PRIVATE_FOLDER, pages::NO_INDEX] {
        assert!(core.contains(&format!("{m:?}")), "{m} isn't in pages_core.rs");
    }
}

#[test]
fn adding_a_folder_lists_it_and_marks_it() {
    let h = Home::new("add");
    let site = h.site("work/handy-redesign", "Handy &amp; redesign");
    let (code, stdout, stderr) = h.run(&["pages", "add", "work/handy-redesign/"]);
    assert_eq!(code, 0, "{stderr}");
    // no hub on this machine and none it's joined to: where it's listed, that nothing shows it yet,
    // and no app path that would open nothing
    assert!(stdout.contains("\"Handy & redesign\" is listed on "), "{stdout}");
    assert!(stdout.contains(", under handy-redesign.\n"), "{stdout}");
    assert!(stdout.contains("isn't joined to one, so the app can't open this page"), "{stdout}");
    assert!(!stdout.contains("joined to shows"), "{stdout}");
    assert!(!stdout.contains("/pages/local/"), "{stdout}");
    assert!(!stdout.contains("/v/"), "{stdout}");
    // joined to a hub: that hub shows it, under this machine
    fs::create_dir_all(h.dir.join(".inyon/node")).unwrap();
    fs::write(h.dir.join(".inyon/node/env"), "INYON_HUB=https://hub.example.ts.net\nINYON_TOKEN=t\n").unwrap();
    let (_, joined) = h.json(&["pages", "add", "work/handy-redesign/"]);
    let said = joined["message"].as_str().unwrap();
    assert!(said.starts_with("\"Handy & redesign\" is in Inyon: Pages > ") && said.contains(" > handy-redesign (already listed)\n"), "{said}");
    assert!(said.ends_with("This machine doesn't run a hub. The hub it's joined to shows its pages (hub 0.22 or later)."), "{said}");
    assert_eq!((joined["hub"].as_bool(), joined["joined"].as_bool()), (Some(false), Some(true)));
    fs::remove_file(h.dir.join(".inyon/node/env")).unwrap();

    let list = h.list();
    assert_eq!(list["v"], 1);
    let p = &list["pages"][0];
    assert_eq!(p["slug"], "handy-redesign");
    assert_eq!(p["name"], "Handy & redesign");
    assert_eq!(p["path"], s(&site));
    assert_eq!(p["project"], s(&site));
    assert_eq!(p["by"], "cli");
    assert!(p["added"].as_u64().unwrap() > 1_700_000_000_000);
    assert_eq!(p.as_object().unwrap().keys().collect::<Vec<_>>(), ["slug", "name", "path", "project", "by", "added"]);
    assert!(!h.dir.join(".inyon/pages.json.lock").exists());

    let marker: Value = serde_json::from_str(&fs::read_to_string(site.join(".inyon-page.json")).unwrap()).unwrap();
    assert_eq!(marker["v"], 1);
    assert_eq!(marker["slug"], "handy-redesign");
    assert_eq!(marker["name"], "Handy & redesign");
    assert_eq!(marker["kind"], "blueprint");
    assert_eq!(marker["created"], p["added"]);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(h.file()).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::metadata(h.dir.join(".inyon")).unwrap().permissions().mode() & 0o777, 0o700);
    }
}

#[test]
fn adding_again_changes_nothing_unless_asked() {
    let h = Home::new("again");
    let site = h.site("plans/ghostex", "Plan");
    assert_eq!(h.run(&["pages", "add", s(&site)]).0, 0);
    let before = fs::read(h.file()).unwrap();
    let marker_before = fs::read(site.join(".inyon-page.json")).unwrap();
    std::thread::sleep(std::time::Duration::from_millis(5));

    let (code, stdout, _) = h.run(&["pages", "add", s(&site)]);
    assert_eq!(code, 0);
    assert!(stdout.contains("\"Plan\" is listed on ") && stdout.contains("(already listed)."), "{stdout}");
    assert_eq!(fs::read(h.file()).unwrap(), before);
    assert_eq!(fs::read(site.join(".inyon-page.json")).unwrap(), marker_before);

    // a new name and slug: the entry keeps its place, its added time and who added it
    let (code, v) = h.json(&["pages", "add", s(&site), "--name", "  Inyon plan  ", "--slug", "inyon"]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["changed"], true);
    // no hub here: the app's path is the hub's to give
    assert!(v.get("app").is_none(), "{v}");
    let list = h.list();
    assert_eq!(list["pages"].as_array().unwrap().len(), 1);
    assert_eq!(list["pages"][0]["name"], "Inyon plan");
    assert_eq!(list["pages"][0]["slug"], "inyon");
    let old: Value = serde_json::from_slice(&before).unwrap();
    assert_eq!(list["pages"][0]["added"], old["pages"][0]["added"]);
    let marker: Value = serde_json::from_str(&fs::read_to_string(site.join(".inyon-page.json")).unwrap()).unwrap();
    assert_eq!(marker["slug"], "inyon");
    assert_eq!(marker["name"], "Inyon plan");
    assert_eq!(marker["created"], old["pages"][0]["added"]);
}

#[test]
fn the_marker_names_the_page_and_keeps_its_slug() {
    let h = Home::new("marker");
    let site = h.site("plans/site", "Plan");
    fs::write(site.join(".inyon-page.json"), r#"{"v":1,"slug":"ghostex","name":"Inyon plan","kind":"blueprint","created":1792320000000}"#).unwrap();
    let (code, v) = h.json(&["pages", "add", s(&site)]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["page"]["slug"], "ghostex");
    assert_eq!(v["page"]["name"], "Inyon plan");
    let marker: Value = serde_json::from_str(&fs::read_to_string(site.join(".inyon-page.json")).unwrap()).unwrap();
    assert_eq!(marker["created"], 1792320000000u64);
    assert_eq!(v["marker"], true);
}

#[test]
fn slugs_stay_unique_on_the_machine() {
    let h = Home::new("unique");
    let a = h.site("a/plan", "A");
    let b = h.site("b/plan", "B");
    assert_eq!(h.json(&["pages", "add", s(&a)]).1["page"]["slug"], "plan");
    assert_eq!(h.json(&["pages", "add", s(&b)]).1["page"]["slug"], "plan-2");
    let c = h.site("c/other", "C");
    let (code, _, stderr) = h.run(&["pages", "add", s(&c), "--slug", "plan"]);
    assert_eq!(code, 1);
    assert!(stderr.contains("The slug \"plan\" is taken by ") && stderr.contains("Pick another with --slug"), "{stderr}");
    let (code, _, stderr) = h.run(&["pages", "add", s(&c), "--slug", "Not_A_Slug"]);
    assert_eq!(code, 1);
    assert!(stderr.contains(pages::BAD_SLUG), "{stderr}");
    assert_eq!(h.list()["pages"].as_array().unwrap().len(), 2);
}

#[test]
fn folders_that_cannot_be_pages_are_refused() {
    let h = Home::new("refuse");
    fs::write(h.dir.join("index.html"), "<title>home</title>").unwrap();
    let cases: Vec<(PathBuf, &str)> = vec![
        (h.dir.clone(), pages::TOO_WIDE),
        (h.site(".ssh/site", "x"), pages::PRIVATE_FOLDER),
        (h.site(".inyon/site", "x"), pages::PRIVATE_FOLDER),
        (h.dir.join("missing"), "doesn't exist"),
    ];
    let empty = h.dir.join("empty");
    fs::create_dir_all(&empty).unwrap();
    for (dir, want) in cases.iter().chain([(empty.clone(), pages::NO_INDEX)].iter()) {
        let (code, _, stderr) = h.run(&["pages", "add", s(dir)]);
        assert_eq!(code, 1, "{}", dir.display());
        assert!(stderr.contains(want), "{}: {stderr}", dir.display());
    }
    let (code, v) = h.json(&["pages", "add", s(&empty)]);
    assert_eq!(code, 1);
    assert_eq!(v["ok"], false);
    assert!(v["error"].as_str().unwrap().starts_with(pages::NO_INDEX));
    assert!(!h.file().exists() || h.list()["pages"].as_array().unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn a_link_into_a_private_folder_is_refused() {
    let h = Home::new("link");
    let hidden = h.site(".ssh/keys", "x");
    std::os::unix::fs::symlink(&hidden, h.dir.join("innocent")).unwrap();
    let (code, _, stderr) = h.run(&["pages", "add", "innocent"]);
    assert_eq!(code, 1);
    assert!(stderr.contains(pages::PRIVATE_FOLDER), "{stderr}");
}

#[cfg(unix)]
#[test]
fn the_same_folder_through_a_link_is_one_page() {
    let h = Home::new("same");
    let site = h.site("real/site", "Site");
    std::os::unix::fs::symlink(&site, h.dir.join("alias")).unwrap();
    assert_eq!(h.run(&["pages", "add", s(&site)]).0, 0);
    let (code, stdout, _) = h.run(&["pages", "add", "alias"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("(already listed)"), "{stdout}");
    assert_eq!(h.list()["pages"].as_array().unwrap().len(), 1);
}

#[test]
fn the_agent_that_added_it_is_recorded() {
    let h = Home::new("agent");
    let site = h.site("s", "S");
    let mut c = h.cmd(&["pages", "add", s(&site), "--json"]);
    c.env("CLAUDECODE", "1");
    let (code, stdout, _) = out(c);
    assert_eq!(code, 0, "{stdout}");
    assert_eq!(h.list()["pages"][0]["by"], "claude");
}

// the app shows the session's name over its page, and follows it when the session is renamed
#[test]
fn the_agents_conversation_is_recorded_and_the_latest_add_wins() {
    let h = Home::new("session");
    let site = h.site("s", "S");
    let add = |vars: &[(&str, &str)]| {
        let mut c = h.cmd(&["pages", "add", s(&site), "--json"]);
        for (k, v) in vars {
            c.env(k, v);
        }
        assert_eq!(out(c).0, 0);
        h.list()["pages"][0].get("session").cloned()
    };
    // a plain add from a terminal has none
    assert_eq!(add(&[]), None);
    assert_eq!(add(&[("CLAUDECODE", "1"), ("CLAUDE_CODE_SESSION_ID", "7409eef9-d507-4808-a592-0bfabe896e3a")]), Some("7409eef9-d507-4808-a592-0bfabe896e3a".into()));
    // another agent working on it later takes it over; a terminal add leaves it be
    assert_eq!(add(&[("CODEX_THREAD_ID", "019a-thread")]), Some("019a-thread".into()));
    assert_eq!(add(&[]), Some("019a-thread".into()));
    // something that isn't an id is ignored
    assert_eq!(add(&[("CLAUDE_CODE_SESSION_ID", "a b; rm -rf")]), Some("019a-thread".into()));
}

#[test]
fn the_project_is_the_git_top_level() {
    if Command::new("git").arg("--version").output().is_err() {
        return;
    }
    let h = Home::new("git");
    let repo = h.dir.join("my-repo");
    fs::create_dir_all(&repo).unwrap();
    assert!(Command::new("git").args(["init", "-q"]).current_dir(&repo).status().unwrap().success());
    let site = h.site("my-repo/private/plans/plan", "Plan");
    let (code, stdout, _) = h.run(&["pages", "add", s(&site)]);
    assert_eq!(code, 0, "{stdout}");
    assert!(stdout.contains(", under my-repo."), "{stdout}");
    let project = h.list()["pages"][0]["project"].as_str().unwrap().to_string();
    assert_eq!(fs::canonicalize(project).unwrap(), fs::canonicalize(&repo).unwrap());
}

#[test]
fn the_project_of_a_folder_in_a_linked_worktree_is_the_main_checkout() {
    if Command::new("git").arg("--version").output().is_err() {
        return;
    }
    let h = Home::new("worktree");
    let repo = h.dir.join("my-repo");
    fs::create_dir_all(&repo).unwrap();
    let git = |args: &[&str]| assert!(Command::new("git").args(args).current_dir(&repo).status().unwrap().success(), "git {args:?}");
    git(&["init", "-q"]);
    git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "x"]);
    git(&["worktree", "add", "-q", ".claude/worktrees/feature"]);
    let site = h.site("my-repo/.claude/worktrees/feature/private/plans/plan", "Plan");
    let (code, stdout, _) = h.run(&["pages", "add", s(&site)]);
    assert_eq!(code, 0, "{stdout}");
    assert!(stdout.contains(", under my-repo.\n"), "{stdout}");
    let project = h.list()["pages"][0]["project"].as_str().unwrap().to_string();
    assert_eq!(fs::canonicalize(project).unwrap(), fs::canonicalize(&repo).unwrap());
}

#[test]
fn on_the_hub_machine_there_is_no_note() {
    let h = Home::new("hub");
    fs::create_dir_all(h.dir.join(".inyon")).unwrap();
    fs::write(h.dir.join(".inyon/hub.env"), "PORT=4777\n").unwrap();
    let site = h.site("s", "S");
    let (code, v) = h.json(&["pages", "add", s(&site)]);
    assert_eq!(code, 0);
    assert_eq!(v["hub"], true);
    let m = v["message"].as_str().unwrap();
    assert!(!m.contains("doesn't run a hub"), "{m}");
    // the app's path, never a link with a token in it
    assert!(m.contains("\"S\" is in Inyon: Pages > ") && m.contains(" > s\n"), "{m}");
    assert!(m.contains("Open it in the app at /pages/local/s"), "{m}");
    assert!(!m.contains("/v/"), "{m}");
    assert_eq!(v["app"], "/pages/local/s");
    let (_, list) = h.json(&["pages", "list"]);
    assert_eq!(list["pages"][0]["app"], "/pages/local/s");
    assert!(list["message"].as_str().unwrap().contains("Open one in the app under Pages, or at /pages/local/<slug>"), "{list}");
}

#[test]
fn the_list_stays_in_the_home_folder_with_a_data_dir_set() {
    let h = Home::new("datadir");
    let site = h.site("s", "S");
    let mut c = h.cmd(&["pages", "add", s(&site)]);
    c.env("INYON_DATA_DIR", h.dir.join("elsewhere"));
    assert_eq!(out(c).0, 0);
    assert!(h.file().exists());
    assert!(!h.dir.join("elsewhere/pages.json").exists());
}

#[test]
fn removing_a_page_leaves_its_folder() {
    let h = Home::new("rm");
    let a = h.site("a", "Alpha");
    let b = h.site("b", "Beta");
    h.run(&["pages", "add", s(&a)]);
    h.run(&["pages", "add", s(&b)]);
    let (code, stdout, _) = h.run(&["pages", "rm", "a"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("Removed \"Alpha\" from Inyon. Its folder stays where it is: ~/a"), "{stdout}");
    assert!(a.join("index.html").exists());
    let list = h.list();
    assert_eq!(list["pages"].as_array().unwrap().len(), 1);
    assert_eq!(list["pages"][0]["slug"], "b");
    let (code, _, stderr) = h.run(&["pages", "rm", "a"]);
    assert_eq!(code, 1);
    assert!(stderr.contains("There's no page called \"a\" on this machine"), "{stderr}");
    assert_eq!(h.run(&["pages", "remove", "b"]).0, 0);
    assert!(h.list()["pages"].as_array().unwrap().is_empty());
}

#[test]
fn the_list_shows_each_page_and_what_is_wrong() {
    let h = Home::new("ls");
    let (code, stdout, _) = h.run(&["pages"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("No pages on this machine yet"), "{stdout}");

    let a = h.site("sites/alpha", "Alpha");
    let b = h.site("sites/beta", "Beta");
    h.run(&["pages", "add", s(&a)]);
    h.run(&["pages", "add", s(&b)]);
    fs::remove_dir_all(&b).unwrap();
    // an entry this version can't read stays in the file and is counted
    let mut list = h.list();
    list["pages"].as_array_mut().unwrap().push(serde_json::json!({ "slug": "Broken!", "future": true }));
    list["extra"] = serde_json::json!("kept");
    fs::write(h.file(), serde_json::to_string(&list).unwrap()).unwrap();

    let (code, stdout, _) = h.run(&["pages", "ls"]);
    assert_eq!(code, 0);
    assert!(stdout.contains("alpha  Alpha  ~/sites/alpha"), "{stdout}");
    assert!(stdout.contains("beta   Beta   ~/sites/beta  ! the folder is gone"), "{stdout}");
    assert!(stdout.contains("1 entry in ~/.inyon/pages.json can't be read and is left out."), "{stdout}");

    let (code, v) = h.json(&["pages", "list"]);
    assert_eq!(code, 0);
    assert_eq!(v["ok"], true);
    assert_eq!(v["host"], "local");
    assert_eq!(v["skipped"], 1);
    let rows = v["pages"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows[0].get("app").is_none(), "{v}");
    assert!(v["message"].as_str().unwrap().contains("Open one in the app under Pages > "), "{v}");
    assert!(!v["message"].as_str().unwrap().contains("/pages/local/"), "{v}");
    assert!(rows[0].get("problem").is_none());
    assert_eq!(rows[1]["problem"], "the folder is gone");

    // writes keep what they don't understand
    let c = h.site("sites/gamma", "Gamma");
    h.run(&["pages", "add", s(&c)]);
    let list = h.list();
    assert_eq!(list["extra"], "kept");
    assert_eq!(list["pages"][2]["future"], true);
    assert_eq!(list["pages"][3]["slug"], "gamma");
}

#[cfg(windows)]
#[test]
fn a_folder_in_the_form_windows_prints_is_listed_as_people_type_it() {
    let h = Home::new("verbatim");
    let site = h.site("s", "S");
    let typed = format!(r"\\?\{}", s(&site));
    let (code, _, stderr) = h.run(&["pages", "add", &typed]);
    assert_eq!(code, 0, "{stderr}");
    assert_eq!(h.list()["pages"][0]["path"], s(&site));
}

#[test]
fn a_broken_list_is_never_overwritten() {
    let h = Home::new("broken");
    fs::create_dir_all(h.dir.join(".inyon")).unwrap();
    fs::write(h.file(), "{not json").unwrap();
    let site = h.site("s", "S");
    let (code, _, stderr) = h.run(&["pages", "add", s(&site)]);
    assert_eq!(code, 1);
    assert!(stderr.contains(pages::BROKEN_FILE), "{stderr}");
    assert_eq!(fs::read_to_string(h.file()).unwrap(), "{not json");
    assert_eq!(h.run(&["pages", "ls"]).0, 1);
    fs::write(h.file(), r#"{"v":2,"pages":[]}"#).unwrap();
    assert_eq!(h.run(&["pages", "add", s(&site)]).0, 1);
}

#[test]
fn a_list_bigger_than_the_cap_is_a_broken_list_at_once_a_sparse_one_claiming_terabytes_too() {
    // NTFS files aren't sparse unless asked, so Windows can't make the terabyte one
    let sizes: &[u64] = if cfg!(windows) { &[pages::LIST_MAX + 1] } else { &[4 << 40, pages::LIST_MAX + 1] };
    for &size in sizes {
        let h = Home::new("big");
        fs::create_dir_all(h.dir.join(".inyon")).unwrap();
        fs::write(h.file(), r#"{"v":1,"pages":[]}"#).unwrap();
        fs::File::options().write(true).open(h.file()).unwrap().set_len(size).unwrap();
        let site = h.site("s", "S");
        let at = std::time::Instant::now();
        let (code, _, stderr) = h.run(&["pages", "add", s(&site)]);
        assert_eq!(code, 1, "{size}");
        assert!(stderr.contains(pages::BROKEN_FILE), "{stderr}");
        assert_eq!(h.run(&["pages", "ls"]).0, 1);
        // reading the terabytes would take hours; two runs of the binary on a slow CI machine take seconds
        assert!(at.elapsed() < std::time::Duration::from_secs(30), "took {:?}", at.elapsed());
        assert_eq!(fs::metadata(h.file()).unwrap().len(), size);
    }
    // one at the cap is still read
    let h = Home::new("at-cap");
    fs::create_dir_all(h.dir.join(".inyon")).unwrap();
    let text = r#"{"v":1,"pages":[]}"#;
    fs::write(h.file(), format!("{text}{}", " ".repeat(pages::LIST_MAX as usize - text.len()))).unwrap();
    let site = h.site("s", "S");
    assert_eq!(h.run(&["pages", "add", s(&site)]).0, 0);
}

#[cfg(unix)]
#[test]
fn a_symlinked_list_a_dotfile_setup_is_read_and_written_through_the_link_stays_its_file_changes() {
    let h = Home::new("linked");
    let dotfiles = h.dir.join("dotfiles");
    fs::create_dir_all(&dotfiles).unwrap();
    fs::create_dir_all(h.dir.join(".inyon")).unwrap();
    // a relative link to a file not made yet, as a dotfile manager can leave it
    std::os::unix::fs::symlink("../dotfiles/pages.json", h.file()).unwrap();
    let a = h.site("a", "A");
    let b = h.site("b", "B");
    assert_eq!(h.run(&["pages", "add", s(&a)]).0, 0);
    assert_eq!(h.run(&["pages", "add", s(&b)]).0, 0);
    let target = dotfiles.join("pages.json");
    let paths = || -> Vec<String> {
        let v: Value = serde_json::from_str(&fs::read_to_string(&target).unwrap()).unwrap();
        v["pages"].as_array().unwrap().iter().map(|p| p["path"].as_str().unwrap().to_owned()).collect()
    };
    assert!(fs::symlink_metadata(h.file()).unwrap().file_type().is_symlink());
    assert_eq!(paths(), vec![s(&a).to_owned(), s(&b).to_owned()]);
    assert_eq!(h.run(&["pages", "rm", "a"]).0, 0);
    assert!(fs::symlink_metadata(h.file()).unwrap().file_type().is_symlink());
    assert_eq!(paths(), vec![s(&b).to_owned()]);
    assert_eq!(h.json(&["pages", "ls"]).1["pages"].as_array().unwrap().len(), 1);
    assert_eq!(fs::read_dir(&dotfiles).unwrap().count(), 1);
}

#[test]
fn a_lock_left_by_a_dead_writer_is_taken_over() {
    let h = Home::new("stale");
    fs::create_dir_all(h.dir.join(".inyon")).unwrap();
    let lock = h.dir.join(".inyon/pages.json.lock");
    fs::write(&lock, "99999\n").unwrap();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
    fs::File::options().write(true).open(&lock).unwrap().set_modified(old).unwrap();
    let site = h.site("s", "S");
    let (code, _, stderr) = h.run(&["pages", "add", s(&site)]);
    assert_eq!(code, 0, "{stderr}");
    assert!(!lock.exists());
}

#[test]
fn a_lock_held_by_another_writer_is_waited_for_then_refused() {
    let h = Home::new("held");
    fs::create_dir_all(h.dir.join(".inyon")).unwrap();
    let lock = h.dir.join(".inyon/pages.json.lock");
    fs::write(&lock, "1\n").unwrap();
    let site = h.site("s", "S");
    let start = std::time::Instant::now();
    let (code, _, stderr) = h.run(&["pages", "add", s(&site)]);
    assert_eq!(code, 1);
    assert!(stderr.contains(pages::LOCKED), "{stderr}");
    assert!(start.elapsed() >= std::time::Duration::from_secs(4));
    assert!(lock.exists(), "someone else's lock stays");
    assert!(!h.file().exists());
}

/// `inyon pages get` as the hub runs it.
fn get(h: &Home, args: &[&str]) -> Value {
    let mut a = vec!["pages", "get"];
    a.extend_from_slice(args);
    let (code, v) = h.json(&a);
    assert_eq!(code, 0, "{v}");
    v
}

fn data(v: &Value) -> Vec<u8> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(v["data"].as_str().unwrap_or_else(|| panic!("no data: {v}"))).unwrap()
}

// a laptop joined to a hub had pages its hub never showed: the hub now asks the machine for them
#[test]
fn a_hub_gets_a_pages_list_with_its_progress_and_each_file_from_the_machine() {
    let h = Home::new("get");
    let site = h.site("plans/robot", "Robot");
    fs::write(
        site.join("progress.json"),
        r#"{"now":"Wiring","updated":"2026-10-24T10:00:00Z","releases":[{"tasks":[{"status":"done"},{"status":"doing"}]},{"tasks":[{"status":"done"}]}]}"#,
    )
    .unwrap();
    fs::create_dir_all(site.join("mockups")).unwrap();
    fs::write(site.join("mockups/index.html"), "<p>mockups</p>").unwrap();
    fs::write(site.join("app.js"), "let x = 1").unwrap();
    fs::write(site.join(".env"), "SECRET=1").unwrap();
    assert_eq!(h.run(&["pages", "add", s(&site), "--slug", "robot"]).0, 0);

    let (_, list) = h.json(&["pages"]);
    assert_eq!(list["serves"], true);
    assert_eq!(list["pages"][0]["progress"], serde_json::json!({ "done": 2, "total": 3, "now": "Wiring", "updated": "2026-10-24T10:00:00Z" }));

    let root = get(&h, &["robot", "--slash"]);
    let real_site = pages::strip_verbatim(s(&fs::canonicalize(&site).unwrap())).replace('\\', "/");
    assert_eq!(root["root"], real_site.as_str());
    assert_eq!(root["items"][0]["dir"], true);
    assert_eq!(root["items"][1]["real"], format!("{real_site}/index.html").as_str());
    assert!(String::from_utf8(data(&root)).unwrap().contains("<title>Robot</title>"));

    let js = get(&h, &["robot", "app.js"]);
    assert_eq!((js["items"][0]["file"].as_bool(), js["items"][0]["size"].as_u64()), (Some(true), Some(9)));
    assert_eq!(data(&js), b"let x = 1");
    // what the browser has already isn't sent again, but progress.json always is
    let etag = format!("\"9-{:x}\"", js["items"][0]["mtime"].as_f64().unwrap().floor() as i64);
    assert!(get(&h, &["robot", "app.js", "--etag", &format!("W/{etag}")]).get("data").is_none());
    assert!(get(&h, &["robot", "progress.json", "--etag", "*"]).get("data").is_some());
    // a folder without its slash: listed, nothing sent (the hub redirects)
    let folder = get(&h, &["robot", "mockups"]);
    assert_eq!(folder["items"][0]["dir"], true);
    assert!(folder.get("data").is_none());
    assert_eq!(data(&get(&h, &["robot", "mockups", "--slash"])), b"<p>mockups</p>");
    // too big for what the hub asked: said, not sent
    let big = get(&h, &["robot", "app.js", "--max", "4"]);
    assert_eq!((big["big"].as_bool(), big.get("data")), (Some(true), None));

    // never a dot name, a path out of the site, a page that isn't listed, or another stream
    for args in
        [&["robot", ".env"][..], &["robot", ".."], &["robot", "..", "..", ".inyon", "pages.json"], &["other", "index.html"], &["robot", "index.html:x"], &["robot", "a/../../x"]]
    {
        let v = get(&h, args);
        assert!(v.get("data").is_none(), "{args:?} → {v}");
    }
}

#[cfg(unix)]
#[test]
fn a_symlink_out_of_the_site_is_listed_where_it_leads_and_never_sent() {
    let h = Home::new("get-link");
    let site = h.site("plans/robot", "Robot");
    fs::write(h.dir.join("secret.txt"), "private").unwrap();
    std::os::unix::fs::symlink(h.dir.join("secret.txt"), site.join("leak.txt")).unwrap();
    std::os::unix::fs::symlink(&h.dir, site.join("home")).unwrap();
    assert_eq!(h.run(&["pages", "add", s(&site), "--slug", "robot"]).0, 0);
    let v = get(&h, &["robot", "leak.txt"]);
    assert_eq!(v["items"][0]["real"], s(&h.dir.join("secret.txt")));
    assert!(v.get("data").is_none(), "{v}");
    assert!(get(&h, &["robot", "home", "secret.txt"]).get("data").is_none());
}

// a file over 8 MB on a joined machine was a 413: it had to fit in one message to the hub
#[test]
fn get_lines_sends_the_answer_then_the_file_as_base64_lines_of_48_kib() {
    use base64::Engine;
    let h = Home::new("get-lines");
    let site = h.site("plans/robot", "Robot");
    let film: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    fs::write(site.join("film.bin"), &film).unwrap();
    assert_eq!(h.run(&["pages", "add", s(&site), "--slug", "robot"]).0, 0);

    let (code, stdout, _) = h.run(&["pages", "get", "--json", "--lines", "--", "robot", "film.bin"]);
    assert_eq!(code, 0);
    let mut lines = stdout.lines();
    let answer: Value = serde_json::from_str(lines.next().unwrap()).unwrap();
    assert_eq!((answer["sends"].as_u64(), answer.get("data")), (Some(200_000), None), "{answer}");
    let parts: Vec<Vec<u8>> = lines.map(|l| base64::engine::general_purpose::STANDARD.decode(l).unwrap()).collect();
    assert_eq!(parts.iter().map(Vec::len).collect::<Vec<_>>(), [49152, 49152, 49152, 49152, 3392]);
    assert_eq!(parts.concat(), film);

    // over what the hub asked for: said, and nothing follows
    let (_, stdout, _) = h.run(&["pages", "get", "--json", "--lines", "--max", "1000", "--", "robot", "film.bin"]);
    let answer: Value = serde_json::from_str(stdout.trim()).unwrap();
    assert_eq!((answer["big"].as_bool(), answer.get("sends")), (Some(true), None));
    // a dot name is never sent
    let (_, stdout, _) = h.run(&["pages", "get", "--json", "--lines", "--", "robot", ".env"]);
    assert_eq!(stdout.lines().count(), 1, "{stdout}");
}

// the phone app with no hub asks a paired computer for each file over its link, with a 4 MiB cap
// (crates/inyon-phone/src/pages.rs): what it gets back for a file, a missing one, a bigger one and
// a path that tries to climb out
#[test]
fn a_phone_with_no_hub_gets_each_file_under_its_own_cap() {
    let h = Home::new("get-phone");
    let site = h.site("plans/robot", "Robot");
    fs::create_dir_all(site.join("kit")).unwrap();
    fs::write(site.join("kit/site.css"), "body{margin:0}").unwrap();
    fs::write(site.join("big.webp"), vec![1u8; 4 * 1024 * 1024 + 1]).unwrap();
    assert_eq!(h.run(&["pages", "add", s(&site), "--slug", "robot"]).0, 0);
    let phone = |segments: &[&str]| {
        let mut a = vec!["pages", "get", "--json", "--lines", "--max", "4194304", "--", "robot"];
        a.extend_from_slice(segments);
        let (code, stdout, _) = h.run(&a);
        assert_eq!(code, 0);
        let mut lines = stdout.lines();
        let answer: Value = serde_json::from_str(lines.next().unwrap()).unwrap();
        (answer, lines.count())
    };
    let (css, lines) = phone(&["kit", "site.css"]);
    assert_eq!((css["sends"].as_u64(), lines), (Some(14), 1), "{css}");
    let (missing, lines) = phone(&["kit", "nope.css"]);
    assert_eq!((missing["items"][0].is_null(), missing.get("sends"), lines), (true, None, 0), "{missing}");
    let (big, lines) = phone(&["big.webp"]);
    assert_eq!((big["big"].as_bool(), big.get("sends"), lines), (Some(true), None, 0), "{big}");
    for climb in [&["..", "..", ".ssh", "id_ed25519"][..], &["kit", "..", "..", "secret.txt"], &["%2e%2e", "x"]] {
        let (v, lines) = phone(climb);
        assert_eq!((v.get("sends"), lines), (None, 0), "{climb:?} → {v}");
    }
}

#[test]
fn a_sites_stamp_moves_on_when_a_file_it_serves_changes_and_not_for_dot_files() {
    let h = Home::new("stamp");
    let site = h.site("plans/robot", "Robot");
    let first = pages::site_stamp(&site).unwrap();
    assert_eq!(pages::site_stamp(&site).unwrap(), first);
    fs::write(site.join(".notes"), "private").unwrap();
    assert_eq!(pages::site_stamp(&site).unwrap(), first, "dot names aren't served, so they don't count");
    fs::create_dir_all(site.join("mockups")).unwrap();
    fs::write(site.join("mockups/a.html"), "a").unwrap();
    let second = pages::site_stamp(&site).unwrap();
    assert_ne!(second, first);
    fs::write(site.join("mockups/a.html"), "ab").unwrap();
    assert_ne!(pages::site_stamp(&site).unwrap(), second);
    // too many files to look at every 2 s: no stamp, and the hub asks for each file
    for i in 0..4000 {
        fs::write(site.join(format!("f{i}")), "").unwrap();
    }
    assert_eq!(pages::site_stamp(&site), None);
    assert_eq!(pages::site_stamp(&h.dir.join("nowhere")), None);
}

#[test]
fn watch_prints_the_list_with_stamps_and_again_only_when_something_changes() {
    use std::io::{BufRead, BufReader};
    let h = Home::new("watch");
    let site = h.site("plans/robot", "Robot");
    assert_eq!(h.run(&["pages", "add", s(&site), "--slug", "robot"]).0, 0);
    let mut child = h.cmd(&["pages", "watch", "--json", "--every", "200"]).stdout(std::process::Stdio::piped()).spawn().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let stdout = child.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let _ = tx.send(line.unwrap());
        }
    });
    let next = |wait: u64| rx.recv_timeout(std::time::Duration::from_secs(wait)).ok().map(|l| serde_json::from_str::<Value>(&l).unwrap());
    let first = next(10).expect("a first list");
    assert_eq!((first["serves"].as_bool(), first["pages"][0]["slug"].as_str()), (Some(true), Some("robot")));
    let stamp = first["pages"][0]["stamp"].as_str().unwrap().to_owned();
    assert!(next(1).is_none(), "nothing changed, nothing printed");
    fs::write(site.join("app.js"), "let x = 1").unwrap();
    let after = next(10).expect("printed again after a change");
    assert_ne!(after["pages"][0]["stamp"].as_str().unwrap(), stamp);
    let _ = child.kill();
    let _ = child.wait();
}

// a hub that went away without stopping it left `pages watch` looking for nobody, for good
#[test]
fn watch_ends_once_nobody_reads_it_even_when_nothing_changes() {
    use std::io::Read;
    let h = Home::new("watch-gone");
    h.site("plans/robot", "Robot");
    let mut child = h.cmd(&["pages", "watch", "--json", "--every", "200", "--alive", "500"]).stdout(std::process::Stdio::piped()).spawn().unwrap();
    let mut first = [0u8; 1];
    child.stdout.as_mut().unwrap().read_exact(&mut first).unwrap();
    drop(child.stdout.take());
    let start = std::time::Instant::now();
    while child.try_wait().unwrap().is_none() {
        assert!(start.elapsed() < std::time::Duration::from_secs(10), "still running");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

// the hub lists a page's research notes for the viewer's Notes drawer, as its own pages list them
#[test]
fn the_list_names_a_sites_markdown_notes() {
    let h = Home::new("notes");
    let site = h.site("plans/robot", "Robot");
    let bare = h.site("plans/bare", "Bare");
    fs::create_dir_all(site.join("research/old")).unwrap();
    for f in ["research/plan.md", "research/Internals.MD", "research/.draft.md", "research/data.json", "research/old/x.md"] {
        fs::write(site.join(f), "# x").unwrap();
    }
    assert_eq!(h.run(&["pages", "add", s(&site), "--slug", "robot"]).0, 0);
    assert_eq!(h.run(&["pages", "add", s(&bare), "--slug", "bare"]).0, 0);
    let (_, list) = h.json(&["pages"]);
    let page = |slug: &str| list["pages"].as_array().unwrap().iter().find(|p| p["slug"] == slug).unwrap().clone();
    assert_eq!(page("robot")["notes"], serde_json::json!(["Internals.MD", "plan.md"]));
    assert!(page("bare").get("notes").is_none(), "{list}");
}

/// A Blueprint site: an index.html with the marker Blueprint writes next to it.
fn blueprint(h: &Home, rel: &str, name: &str) -> PathBuf {
    let d = h.site(rel, name);
    fs::write(d.join(".inyon-page.json"), format!(r#"{{"v":1,"slug":"{}","name":"{name}","kind":"blueprint","created":1}}"#, pages::slugify(name))).unwrap();
    d
}

// an agent made a plan and never ran `inyon pages add`: the hub finds it where the agent works
#[test]
fn find_adds_the_sites_under_an_agents_folder_once_and_never_one_taken_off() {
    let h = Home::new("find");
    let shop = h.dir.join("code").join("shop");
    let plan = blueprint(&h, "code/shop/private/plans/checkout", "Checkout rewrite");
    blueprint(&h, "code/shop/node_modules/pkg/docs", "Dependency docs");
    // a site made for tests, as in Inyon's own repo
    blueprint(&h, "code/shop/fixtures/pages/site", "Fixture site");
    blueprint(&h, "code/shop/a/b/c/d/e", "Too deep");
    h.site("code/shop/site", "No marker");
    let src = shop.join("src");
    fs::create_dir_all(&src).unwrap();
    assert!(Command::new("git").args(["init", "-q"]).arg(&shop).status().unwrap().success());
    let find = |dir: &Path| {
        let (code, v) = h.json(&["pages", "find", "--by", "claude", s(dir)]);
        assert_eq!(code, 0, "{v}");
        v["found"].as_array().unwrap().iter().map(|p| p["name"].as_str().unwrap().to_owned()).collect::<Vec<_>>()
    };
    // the agent works in src: its project's private/plans is looked at too
    assert_eq!(find(&src), ["Checkout rewrite"]);
    assert_eq!(h.list()["pages"][0]["by"], "claude");
    assert_eq!(h.list()["pages"][0]["slug"], "checkout-rewrite");
    // listed now; no marker, too deep, inside node_modules or fixtures: nothing else
    assert!(find(&shop).is_empty());
    // taken off the list: it stays off
    assert_eq!(h.run(&["pages", "rm", "checkout-rewrite"]).0, 0);
    assert!(find(&shop).is_empty());
    assert!(h.list()["removed"].as_array().unwrap().iter().any(|p| p.as_str().is_some_and(|p| p.ends_with("checkout"))), "{}", h.list());
    // added again on purpose: no longer remembered as removed
    assert_eq!(h.run(&["pages", "add", s(&plan)]).0, 0);
    assert_eq!(h.list()["removed"], serde_json::json!([]));
    // the home folder itself isn't looked through, and a relative folder isn't either
    blueprint(&h, "notes", "Home notes");
    assert!(find(&h.dir).is_empty());
    assert!(find(Path::new("notes")).is_empty());
}

#[test]
fn find_shares_its_folder_budget_so_a_big_folder_leaves_room_for_the_next() {
    let h = Home::new("find-budget");
    let big = h.dir.join("big");
    for i in 0..4100 {
        fs::create_dir_all(big.join(format!("d{i:04}"))).unwrap();
    }
    blueprint(&h, "small/plans/board", "Board");
    let (code, v) = h.json(&["pages", "find", "--by", "codex", s(&big), s(&h.dir.join("small"))]);
    assert_eq!(code, 0, "{v}");
    assert_eq!(v["found"][0]["name"], "Board", "{v}");
}
