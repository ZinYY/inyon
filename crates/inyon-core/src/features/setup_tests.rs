//! features/setup.test.ts

use std::rc::Rc;

use futures_util::future::LocalBoxFuture;
use inyon_protocol::AgentKind;

use super::*;
use crate::features::agent_latest::forget_latest;
use crate::hosts::host::ExecResult;
use crate::platform::fetch::{use_fetcher, FetchRequest, FetchResponse, Fetcher};
use crate::test_support::fake::Fake;
use crate::test_support::rt::run;
use crate::test_support::{machine, sh, TempDir};

// The real install commands, run against a stand-in curl and npm: nothing is downloaded. The PATH
// holds only the stand-ins and a few basic tools, so a real npm on the test machine isn't found.

/// an installer that puts a working `codex` in ~/.local/bin, as the real one does
const TEST_PATHS: &str = r#"export PATH="$HOME/.local/bin:$PATH""#;
const INSTALLER: &str = r#"mkdir -p "$HOME/.local/bin"; printf '#!/bin/sh\necho codex 1.0\n' > "$HOME/.local/bin/codex"; chmod +x "$HOME/.local/bin/codex""#;

struct Machine {
    home: TempDir,
}

impl Machine {
    fn new(download: bool, npm: bool) -> Machine {
        let home = TempDir::new("inyon-setup-");
        let bin = home.path("tools");
        std::fs::create_dir_all(&bin).unwrap();
        for tool in ["sh", "cat", "mkdir", "chmod", "grep"] {
            let (_, found, _) = sh(&format!("command -v {tool}"), None, &[]);
            std::os::unix::fs::symlink(found.trim(), bin.join(tool)).unwrap();
        }
        let bin = bin.to_string_lossy().into_owned();
        home.write("tools/installer.sh", INSTALLER);
        // like `curl -fsSL`, a failed download prints nothing, so the piped shell still exits 0
        home.write("tools/curl", &if download { format!("#!/bin/sh\ncat '{bin}/installer.sh'\n") } else { "#!/bin/sh\nexit 22\n".into() });
        sh(&format!("chmod 755 '{bin}/curl'"), None, &[]);
        if npm {
            home.write("tools/npm", "#!/bin/sh\necho \"npm $*\" >> \"$HOME/npm-calls\"\n[ \"$1\" = config ] && [ \"$2\" = get ] && echo \"$HOME/.npm-global\"\nexit 0\n");
            sh(&format!("chmod 755 '{bin}/npm'"), None, &[]);
        }
        Machine { home }
    }

    fn run(&self, cmd: &str) -> (i32, String) {
        let bin = self.home.path("tools").to_string_lossy().into_owned();
        let home = self.home.s();
        let (code, out, err) = sh(&format!("cd \"$HOME\" && {cmd}"), None, &[("HOME", &home), ("PATH", &bin)]);
        (code, out + &err)
    }

    fn npm_calls(&self) -> Vec<String> {
        if !self.home.exists("npm-calls") {
            return vec![];
        }
        self.home.read("npm-calls").trim().split('\n').map(str::to_owned).collect()
    }
}

#[test]
fn an_agents_own_installer_is_used_and_npm_isnt_touched_when_it_works() {
    let m = Machine::new(true, true);
    let (code, _) = m.run(&install_command(AgentKind::Codex.def(), TEST_PATHS));
    assert_eq!(code, 0);
    assert!(m.home.exists(".local/bin/codex"));
    assert_eq!(m.npm_calls(), Vec::<String>::new());
}

#[test]
fn when_the_installer_fails_quietly_its_npm_package_is_installed_instead() {
    let m = Machine::new(false, true);
    let (code, out) = m.run(&install_command(AgentKind::Codex.def(), TEST_PATHS));
    assert_eq!(code, 0);
    assert!(out.contains("trying npm instead"), "{out}");
    assert!(m.npm_calls().contains(&"npm install -g @openai/codex".to_owned()));
}

#[test]
fn when_the_installer_fails_and_there_is_no_npm_the_step_fails_and_says_why() {
    let m = Machine::new(false, false);
    let (code, out) = m.run(&install_command(AgentKind::Codex.def(), TEST_PATHS));
    assert_ne!(code, 0);
    assert!(out.contains("there's no npm here"), "{out}");
}

#[test]
fn an_agent_with_no_installer_of_its_own_goes_straight_to_npm() {
    let m = Machine::new(true, true);
    assert_eq!(m.run(&install_command(AgentKind::Gemini.def(), TEST_PATHS)).0, 0);
    assert!(m.npm_calls().contains(&"npm install -g @google/gemini-cli".to_owned()));
}

// ---------- updates ----------

fn npm() -> InstalledAt {
    InstalledAt { via: InstalledVia::Npm, prefix: None }
}

#[test]
fn how_an_agent_was_installed_npm_when_its_package_is_listed_or_its_program_sits_in_node_modules() {
    let globals = "/usr/lib /usr/lib/node_modules/@openai/codex /usr/lib/node_modules/npm";
    let installer = InstalledAt { via: InstalledVia::Installer, prefix: None };
    assert_eq!(installed_via(AgentKind::Codex.def(), "/usr/bin/codex", globals), npm());
    assert_eq!(installed_via(AgentKind::Codex.def(), "/home/me/.local/bin/codex", ""), installer);
    assert_eq!(installed_via(AgentKind::Claude.def(), "/home/me/.npm-global/lib/node_modules/@anthropic-ai/claude-code/cli.js", ""), npm());
    assert_eq!(installed_via(AgentKind::Claude.def(), "/home/me/.local/bin/claude", globals), installer);
    assert_eq!(installed_via(AgentKind::Antigravity.def(), "/home/me/.local/bin/agy", globals), installer);
    // the global folder itself (Windows lays it out without lib/), and a copy under lib/ of a prefix
    assert_eq!(installed_via(AgentKind::Codex.def(), "/usr/lib/node_modules/@openai/codex/bin/codex.js", globals), npm());
    assert_eq!(installed_via(AgentKind::Codex.def(), "C:/Users/me/AppData/Roaming/npm/node_modules/@openai/codex/bin/codex.js", "C:/Users/me/AppData/Roaming/npm"), npm());
}

#[test]
fn an_npm_copy_in_a_folder_of_its_own_is_updated_there_not_as_a_second_global_copy() {
    // an older Inyon kept OpenCode this way, with ~/.local/bin/opencode pointing into it
    let bin = "/home/me/.local/share/tether-tools/opencode/node_modules/opencode-ai/bin/opencode";
    let def = AgentKind::Opencode.def();
    let at = installed_via(def, bin, "/usr/lib /usr/lib/node_modules/npm");
    assert_eq!(at, InstalledAt { via: InstalledVia::Npm, prefix: Some("/home/me/.local/share/tether-tools/opencode".into()) });
    assert_eq!(
        update_command(def, Some(InstalledVia::Npm), TEST_PATHS, at.prefix.as_deref()),
        format!("{TEST_PATHS}; npm install --prefix '/home/me/.local/share/tether-tools/opencode' opencode-ai@latest")
    );
    assert_eq!(
        manual_command(def, Step::Update, Some(InstalledVia::Npm), at.prefix.as_deref()),
        "npm install --prefix '/home/me/.local/share/tether-tools/opencode' opencode-ai@latest"
    );
    // without a prefix the global copy is the one
    assert!(update_command(def, Some(InstalledVia::Npm), TEST_PATHS, None).contains("npm install -g opencode-ai@latest"));
}

/// the registry answering every package with `version`
struct Answering(String);

impl Fetcher for Answering {
    fn fetch(&self, _req: FetchRequest) -> LocalBoxFuture<'static, Result<FetchResponse, String>> {
        let body = serde_json::json!({ "version": self.0 }).to_string().into_bytes();
        Box::pin(async move { Ok(FetchResponse { status: 200, body }) })
    }
}

fn registry_answering(version: &str) {
    use_fetcher(Some(Rc::new(Answering(version.into()))));
    forget_latest();
}

struct Step0 {
    code: i32,
    out: &'static str,
    after: &'static str,
}

/// machine "h", answering the report as a box with codex `installed` (through `via`) and every
/// other command as `step`
fn fake_machine(installed: &'static str, via: InstalledVia, step: Step0) -> Rc<Fake> {
    machine("h", "local", serde_json::json!([])).borrow_mut().hold_polls();
    Fake::install(move |_, cmd, _| {
        if cmd.contains("echo \"OS ") {
            let bin = if via == InstalledVia::Npm { "/usr/lib/node_modules/@openai/codex/bin/codex.js" } else { "/home/me/.local/bin/codex" };
            return ExecResult {
                code: 0,
                stdout: format!("OS Linux x86_64\nPKG apt\nSUDO 0\nTMUX tmux 3.4\nSESSIONS 1\nNODE v24.0.0\nNPM 11.0.0\nVER codex {installed}\nBIN codex {bin}\nNPMLS /usr/lib\n"),
                stderr: String::new(),
            };
        }
        ExecResult { code: step.code, stdout: format!("{}\nINYON_VERSION {}\n", step.out, step.after), stderr: String::new() }
    })
}

/// what ran besides the report and the registry's own polls
fn steps(f: &Fake) -> Vec<String> {
    f.cmds().into_iter().filter(|c| c.contains("INYON_VERSION")).collect()
}

#[test]
fn the_report_says_which_agents_are_behind_and_how_each_was_installed() {
    run(|| async {
        registry_answering("0.50.0");
        fake_machine("codex-cli 0.49.0", InstalledVia::Npm, Step0 { code: 0, out: "", after: "" });
        let report = setup_report("h").await.unwrap();
        let codex = report.agents.iter().find(|a| a.kind == AgentKind::Codex).unwrap();
        assert_eq!(
            serde_json::to_value(codex).unwrap(),
            serde_json::json!({ "kind": "codex", "installed": true, "version": "codex-cli 0.49.0", "signedIn": false, "latest": "0.50.0", "via": "npm", "outdated": true })
        );
        let claude = report.agents.iter().find(|a| a.kind == AgentKind::Claude).unwrap();
        assert_eq!(serde_json::to_value(claude).unwrap(), serde_json::json!({ "kind": "claude", "installed": false, "signedIn": false, "latest": "0.50.0" }));
    });
}

#[test]
fn an_update_runs_the_step_reads_the_new_version_and_reports_it() {
    run(|| async {
        registry_answering("0.50.0");
        let f = fake_machine("codex-cli 0.49.0", InstalledVia::Npm, Step0 { code: 0, out: "added 1 package", after: "codex-cli 0.50.0" });
        let r = run_setup("h", "update:codex").await.unwrap();
        assert_eq!(serde_json::to_value(&r).unwrap(), serde_json::json!({ "ok": true, "output": "added 1 package", "version": "codex-cli 0.50.0", "latest": "0.50.0" }));
        let ran = steps(&f);
        assert!(ran[0].contains("npm install -g @openai/codex@latest"));
        assert!(ran[0].contains("codex --version"));
    });
}

#[test]
fn an_installer_installed_agent_is_updated_with_its_own_updater() {
    run(|| async {
        registry_answering("0.50.0");
        let f = fake_machine("codex-cli 0.49.0", InstalledVia::Installer, Step0 { code: 0, out: "", after: "codex-cli 0.50.0" });
        run_setup("h", "update:codex").await.unwrap();
        let ran = steps(&f);
        assert!(ran[0].contains("; codex update\n"));
        assert!(!ran[0].contains("npm install"));
    });
}

#[test]
fn a_failed_update_says_why_and_what_to_run_by_hand() {
    run(|| async {
        registry_answering("0.50.0");
        fake_machine("codex-cli 0.49.0", InstalledVia::Npm, Step0 { code: 1, out: "npm ERR! code EACCES\nnpm ERR! syscall mkdir", after: "codex-cli 0.49.0" });
        let r = run_setup("h", "update:codex").await.unwrap();
        assert!(!r.ok);
        assert_eq!(r.version.as_deref(), Some("codex-cli 0.49.0"));
        assert_eq!(r.manual.as_deref(), Some("npm install -g @openai/codex@latest"));
        assert!(r.hint.unwrap().starts_with("npm can't write there"));
        assert_eq!(r.output.as_deref(), Some("npm ERR! code EACCES\nnpm ERR! syscall mkdir"));
    });
}

#[test]
fn an_install_that_ends_without_the_agent_is_a_failure_even_when_the_installer_said_nothing() {
    run(|| async {
        registry_answering("0.50.0");
        fake_machine("", InstalledVia::Installer, Step0 { code: 0, out: "", after: "" });
        let r = run_setup("h", "agent:claude").await.unwrap();
        assert!(!r.ok);
        assert_eq!(r.hint.as_deref(), Some("Claude Code still doesn't run after the step"));
        assert_eq!(r.manual, AgentKind::Claude.def().install.cmd);
    });
}

#[test]
fn opencodes_row_says_which_providers_are_connected_from_opencode_auth_list_carried_in_the_report() {
    use base64::Engine;
    let list = "┌  Credentials /x/auth.json\n│\n●  Anthropic api\n│\n└  1 credentials\n\n┌  Environment\n│\n●  Groq GROQ_API_KEY\n│\n└  1 environment variable\n";
    assert_eq!(connected_providers(&base64::engine::general_purpose::STANDARD.encode(list)), vec!["Anthropic", "Groq"]);
    assert_eq!(connected_providers(""), Vec::<String>::new());
    assert_eq!(connected_providers("not base64 at all!"), Vec::<String>::new());
}

#[test]
fn updating_an_agent_that_is_not_installed_installs_it() {
    run(|| async {
        registry_answering("0.50.0");
        let f = fake_machine("", InstalledVia::Installer, Step0 { code: 0, out: "", after: "codex-cli 0.50.0" });
        let r = run_setup("h", "update:codex").await.unwrap();
        assert!(r.ok);
        assert!(steps(&f)[0].contains("chatgpt.com/codex/install.sh"));
    });
}

// ---------- beyond the TS tests ----------

#[test]
fn the_hint_falls_back_to_the_last_line_that_sounds_like_an_error() {
    assert_eq!(failure_hint("", None), None);
    assert_eq!(failure_hint("bash: npm: command not found", None).as_deref(), Some("Install Node.js first (the Setup tab can)"));
    assert_eq!(failure_hint("curl: (6) Could not resolve host: claude.ai", None).as_deref(), Some("The machine couldn't reach the download; check its network and try again"));
    assert_eq!(failure_hint("step one\nthat failed here\nbye", None).as_deref(), Some("that failed here"));
    assert_eq!(failure_hint("all good\nbye\n", None).as_deref(), Some("bye"));
}

#[test]
fn unknown_steps_and_agents_are_refused() {
    run(|| async {
        registry_answering("0.50.0");
        fake_machine("", InstalledVia::Installer, Step0 { code: 0, out: "", after: "" });
        assert_eq!(run_setup("h", "agent:nope").await.unwrap_err(), HubError::http(400, "Unknown agent"));
        assert_eq!(run_setup("h", "reboot").await.unwrap_err(), HubError::http(400, "Unknown setup step"));
    });
}

#[test]
fn the_sign_in_tests_read_the_same_files_the_probe_does() {
    assert_eq!(signed_in_test(AgentKind::Codex), r#"{ [ -e "$HOME"/.codex/auth.json ]; }"#);
    assert!(signed_in_test(AgentKind::Claude).starts_with(r#"{ [ -s "${CLAUDE_CONFIG_DIR:-$HOME/.claude}"/.credentials.json ] || grep -q '"oauthAccount"' "#));
    assert!(signed_in_test(AgentKind::Copilot).starts_with("node -e '"));
    // the probe runs the same tests (Copilot's inside its five-minute cache)
    for k in AGENT_KINDS {
        let t = signed_in_test(k);
        let want = if k == AgentKind::Copilot { format!("elif {t}; then") } else { format!("$({t} && printf '{} ')", k.as_str()) };
        assert!(crate::tmux::tmux::PROBE.contains(&want), "{}", k.as_str());
    }
}

#[cfg(unix)]
#[test]
fn the_report_reads_opencodes_providers_from_a_listing_long_enough_for_base64_to_wrap() {
    // the line as the machine's shell runs it, with an opencode that lists two providers
    let dir = std::env::temp_dir().join(format!("inyon-ocauth-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let listing = "┌  Credentials ~/.local/share/opencode/auth.json\n│\n●  DeepSeek api\n│\n●  Anthropic oauth\n│\n└  2 credentials\n\n┌  Environment\n│\n●  Groq GROQ_API_KEY\n│\n└  1 environment variable\n";
    std::fs::write(dir.join("listing"), listing).unwrap();
    let stub = dir.join("opencode");
    std::fs::write(&stub, format!("#!/bin/sh\ncat '{}'\n", dir.join("listing").display())).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755)).unwrap();
    let out = std::process::Command::new("sh").arg("-c").arg(OCAUTH_LINE).env("PATH", format!("{}:/usr/bin:/bin", dir.display())).output().unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(stdout.lines().count(), 1, "{stdout}");
    let encoded = stdout.trim_end().strip_prefix("OCAUTH ").unwrap();
    assert_eq!(connected_providers(encoded), ["DeepSeek", "Anthropic", "Groq"]);
}

#[cfg(unix)]
#[test]
fn qwen_code_counts_as_signed_in_once_auth_picked_a_method_not_when_it_only_saved_preferences() {
    let home = TempDir::new("inyon-qwen-");
    let signed_in = || std::process::Command::new("sh").arg("-c").arg(signed_in_test(AgentKind::Qwen)).env("HOME", &home.0).status().unwrap().success();
    assert!(!signed_in());
    std::fs::create_dir_all(home.path(".qwen")).unwrap();
    std::fs::write(home.path(".qwen/settings.json"), r#"{ "ui": { "theme": "Default" } }"#).unwrap();
    assert!(!signed_in());
    std::fs::write(home.path(".qwen/settings.json"), r#"{ "security": { "auth": { "selectedType": "openai" } } }"#).unwrap();
    assert!(signed_in());
}
