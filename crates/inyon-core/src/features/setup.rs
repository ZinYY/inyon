//! Port of features/setup.ts: everything Inyon needs on a machine, checked in one round trip, and
//! the one-tap steps that fix what's missing (tmux, Node.js, a session, each agent and its updates).

use inyon_protocol::{AgentDef, AgentKind, AGENT_KINDS};
use serde::Serialize;

use super::agent_latest::{compare_versions, latest_agent_versions};
use super::providers::parse_auth_list;
use crate::agents::copilot_rpc_sh::copilot_auth_script;
use crate::config::{load_settings, save_settings};
use crate::hosts::host::{shq, ExecOptions};
use crate::hosts::http_error::{HubError, HubResult};
use crate::hosts::registry::registry;
use crate::js::text::JsStr;
use crate::js::{fmt_number, to_number};
use crate::platform::rt::spawn;
use crate::re;
use crate::tmux::tmux::HIDDEN_PREFIXES;
use inyon_protocol::{HostStatus, TmuxState};

/// Puts the usual install folders on PATH, for commands run over a non-login shell.
pub const PATHS: &str = r#"export PATH="$HOME/.local/bin:$HOME/bin:$HOME/.npm-global/bin:$HOME/.bun/bin:$HOME/.opencode/bin:$HOME/.grok/bin:$HOME/.kimi-code/bin:/opt/homebrew/bin:/usr/local/bin:$PATH"; for d in "$HOME"/.nvm/versions/node/*/bin; do [ -d "$d" ] && PATH="$PATH:$d"; done"#;

/// A one-tap fix Inyon can run.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SetupFix {
    pub label: String,
    pub action: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SetupCheck {
    pub id: String,
    pub label: String,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    /// a one-tap fix Inyon can run
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<SetupFix>,
    /// a command for the user to run themselves (e.g. needs sudo or a browser)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manual: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub docs: Option<String>,
}

/// npm: an npm package (updated with npm, global or under its own prefix) · installer: the agent's own installer
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InstalledVia {
    Npm,
    Installer,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupAgent {
    pub kind: AgentKind,
    pub installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub signed_in: bool,
    /// the newest published version, when the registry answered
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest: Option<String>,
    /// OpenCode: the providers it's connected to on the machine (a saved credential or an environment variable)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub providers: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<InstalledVia>,
    /// an npm install that isn't global: the folder whose node_modules holds it, updated in place
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prefix: Option<String>,
    /// the installed version is older than `latest`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outdated: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupReport {
    pub os: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pkg: Option<String>,
    pub sudo: bool,
    pub checks: Vec<SetupCheck>,
    pub agents: Vec<SetupAgent>,
    pub keep_session: bool,
}

/// What an install or update step came back with.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
pub struct SetupResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// a sign-in that opened a terminal pane
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pane: Option<String>,
    /// the agent's version once the step ran
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest: Option<String>,
    /// when it failed: the command to run on the machine yourself
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manual: Option<String>,
    /// when it failed: the likely cause, in one line
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// How an installed agent got there, and the folder of an npm copy outside the global one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledAt {
    pub via: InstalledVia,
    pub prefix: Option<String>,
}

/// the program an agent's version command runs
fn binary(def: &AgentDef) -> &str {
    def.version.split(' ').next().unwrap_or("")
}

fn pkg_install(pkg: &str, p: &str) -> Option<String> {
    Some(match pkg {
        "apt" => format!("apt-get update -qq && apt-get install -y -qq {p}"),
        "dnf" => format!("dnf install -y -q {p}"),
        "yum" => format!("yum install -y -q {p}"),
        "pacman" => format!("pacman -S --noconfirm {p}"),
        "apk" => format!("apk add {p}"),
        "zypper" => format!("zypper -n install {p}"),
        "brew" => format!("brew install {p}"),
        _ => return None,
    })
}

// ---------- agents/auth-sh.ts ----------

fn file(f: &str) -> String {
    match f.strip_prefix('~') {
        Some(rest) => format!("\"$HOME\"{rest}"),
        None => f.to_owned(),
    }
}

/// A shell test that succeeds when the agent has a saved sign-in on the machine (`signedInTest`
/// from agents/auth-sh.ts). Only checks that files exist (or name a key); nothing is read out.
pub fn signed_in_test(kind: AgentKind) -> String {
    let def = kind.def();
    match kind {
        AgentKind::Copilot => {
            let quoted = format!("'{}'", copilot_auth_script().replace('\'', "'\\''"));
            format!("node -e {quoted} >/dev/null 2>&1")
        }
        AgentKind::Claude => {
            // ~/.claude.json exists from the first run; it only names an account once someone signed in (macOS keeps the token in the Keychain)
            let d = "\"${CLAUDE_CONFIG_DIR:-$HOME/.claude}\"";
            let cfg = "\"${CLAUDE_CONFIG_DIR:-$HOME}/.claude.json\"";
            format!("{{ [ -s {d}/.credentials.json ] || grep -q '\"oauthAccount\"' {cfg} 2>/dev/null; }}")
        }
        // a saved Google sign-in, or API-key mode ("modelProvider": "gemini" plus GEMINI_API_KEY in its environment)
        AgentKind::Antigravity => format!("{{ [ -s {} ] || grep -qs '\"modelProvider\"' \"$HOME/.gemini/antigravity-cli/settings.json\"; }}", file(&def.auth_files[0])),
        AgentKind::Gemini => {
            format!("{{ {} || grep -qs GEMINI_API_KEY \"$HOME/.gemini/.env\"; }}", def.auth_files.iter().map(|f| format!("[ -s {} ]", file(f))).collect::<Vec<_>>().join(" || "))
        }
        // Qwen Code writes settings.json for any preference; /auth adds the method it signed in with
        AgentKind::Qwen => format!("{{ grep -qs '\"selectedType\"' {}; }}", file(&def.auth_files[0])),
        // OpenCode's free Zen models need no sign-in: once it has run (its database exists) it can work
        AgentKind::Opencode => format!("{{ [ -e {} ] || [ -e \"$HOME/.local/share/opencode/opencode.db\" ]; }}", file(&def.auth_files[0])),
        _ => format!("{{ {}; }}", def.auth_files.iter().map(|f| format!("[ -e {} ]", file(f))).collect::<Vec<_>>().join(" || ")),
    }
}

// ---------- the report ----------

/// OpenCode's connected providers, as base64 on one line (GNU base64 wraps at 76 columns, and
/// only the report's first OCAUTH line is read).
pub const OCAUTH_LINE: &str = "command -v opencode >/dev/null 2>&1 && echo \"OCAUTH $(opencode auth list 2>/dev/null | base64 | tr -d '\\n')\"";

/// Everything Inyon needs on a machine, checked in one round trip.
pub async fn setup_report(host_id: &str) -> HubResult<SetupReport> {
    let host = registry().host(host_id)?;
    let auth_tests = AGENT_KINDS.iter().map(|k| format!("echo \"AUTH {} $({} && echo 1 || echo 0)\"", k.as_str(), signed_in_test(*k))).collect::<Vec<_>>().join("\n");
    let hidden = HIDDEN_PREFIXES.iter().map(|p| format!("-e '^{p}'")).collect::<Vec<_>>().join(" ");
    let vers = AGENT_KINDS.iter().map(|k| format!("echo \"VER {} $({} 2>/dev/null | head -1)\"", k.as_str(), k.def().version)).collect::<Vec<_>>().join("\n");
    let bins = AGENT_KINDS
        .iter()
        .map(|k| format!("echo \"BIN {} $(b=$(command -v {} 2>/dev/null) && {{ readlink -f \"$b\" 2>/dev/null || echo \"$b\"; }})\"", k.as_str(), binary(k.def())))
        .collect::<Vec<_>>()
        .join("\n");
    let script = format!(
        r#"{PATHS}
echo "OS $(uname -s) $(uname -m)"
for p in apt dnf yum pacman apk zypper brew; do command -v $p >/dev/null 2>&1 && {{ [ $p = apt ] && command -v apt-get >/dev/null || [ $p != apt ]; }} && {{ echo "PKG $p"; break; }}; done
sudo -n true >/dev/null 2>&1 && echo "SUDO 1" || echo "SUDO 0"
echo "TMUX $(tmux -V 2>/dev/null)"
# the same sessions the list shows: not Inyon's own terminal views and sign-ins
echo "SESSIONS $(tmux list-sessions -F '#{{session_name}}' 2>/dev/null | grep -c -v {hidden})"
echo "NODE $(node --version 2>/dev/null)"
echo "NPM $(npm --version 2>/dev/null)"
echo "CURL $(command -v curl >/dev/null && echo 1)"
echo "GIT $(git --version 2>/dev/null)"
{vers}
{bins}
echo "NPMLS $(npm ls -g --depth=0 --parseable 2>/dev/null | tr '\n' ' ')"
{OCAUTH_LINE}
{auth_tests}
"#
    );
    // the registry is asked while the machine runs the script, so the report isn't slower for it
    let (r, latest) = futures_util::future::join(host.exec(&script, ExecOptions::timeout(30000)), latest_agent_versions()).await;
    let r = r?;
    let lines: Vec<&str> = r.stdout.split('\n').collect();
    let get = |k: &str| -> String {
        let p = format!("{k} ");
        lines.iter().find(|l| l.starts_with(&p)).map(|l| l[p.len()..].jtrim().to_owned()).unwrap_or_default()
    };
    let os = get("OS");
    let pkg = Some(get("PKG")).filter(|p| !p.is_empty());
    let sudo = get("SUDO") == "1";
    let tmux = get("TMUX");
    let sessions = {
        let n = to_number(&get("SESSIONS"));
        if n.is_nan() {
            0.0
        } else {
            n
        }
    };
    let node = get("NODE");
    let keep_session = load_settings().keep_session().get(host_id).copied().unwrap_or(false);
    let install = |p: &str| pkg.as_deref().and_then(|k| pkg_install(k, p));
    let as_root = |cmd: Option<String>| cmd.map(|c| if pkg.as_deref() != Some("brew") { format!("sudo sh -c {}", shq(&c)) } else { c });
    let can_fix = pkg.is_some() && (sudo || pkg.as_deref() == Some("brew"));

    let checks = vec![
        SetupCheck {
            id: "tmux".into(),
            label: "tmux".into(),
            ok: !tmux.is_empty(),
            detail: Some(if tmux.is_empty() { "Not installed. Inyon lists and controls agents through tmux.".into() } else { tmux.clone() }),
            fix: (tmux.is_empty() && can_fix).then(|| SetupFix { label: "Install tmux".into(), action: "install:tmux".into() }),
            manual: tmux.is_empty().then(|| as_root(install("tmux")).unwrap_or_else(|| "Install tmux with your package manager".into())),
            docs: None,
        },
        SetupCheck {
            id: "session".into(),
            label: "A tmux session to work in".into(),
            ok: sessions > 0.0,
            detail: Some(if sessions != 0.0 { format!("{} running", fmt_number(sessions)) } else { "No tmux sessions yet".into() }),
            fix: (!tmux.is_empty() && sessions == 0.0).then(|| SetupFix { label: "Start a session".into(), action: "session".into() }),
            manual: None,
            docs: None,
        },
        SetupCheck {
            id: "node".into(),
            label: "Node.js (for npm-installed agents)".into(),
            ok: !node.is_empty(),
            detail: Some(if node.is_empty() { "Needed for Codex, Gemini CLI and GitHub Copilot".into() } else { node.clone() }),
            fix: (node.is_empty() && can_fix).then(|| SetupFix { label: "Install Node.js".into(), action: "install:node".into() }),
            manual: if node.is_empty() { as_root(install(if pkg.as_deref() == Some("brew") { "node" } else { "nodejs npm" })) } else { None },
            docs: None,
        },
    ];
    let npm_globals = get("NPMLS");
    let agents = AGENT_KINDS
        .iter()
        .map(|&k| {
            let version = get(&format!("VER {}", k.as_str()));
            let auth_prefix = format!("AUTH {} ", k.as_str());
            let auth = lines.iter().find(|l| l.starts_with(&auth_prefix)).is_some_and(|l| l.ends_with('1'));
            let mut a = SetupAgent {
                kind: k,
                installed: !version.is_empty(),
                version: Some(version.clone()).filter(|v| !v.is_empty()),
                signed_in: auth,
                latest: latest.get(&k).cloned(),
                providers: None,
                via: None,
                prefix: None,
                outdated: None,
            };
            if version.is_empty() {
                return a;
            }
            if k == AgentKind::Opencode {
                a.providers = Some(connected_providers(&get("OCAUTH")));
            }
            let at = installed_via(k.def(), &get(&format!("BIN {}", k.as_str())), &npm_globals);
            a.via = Some(at.via);
            a.prefix = at.prefix;
            if let Some(l) = &a.latest {
                a.outdated = Some(compare_versions(Some(&version), Some(l)) < 0);
            }
            a
        })
        .collect();
    Ok(SetupReport { os, pkg, sudo, checks, agents, keep_session })
}

/// Node's lenient base64 (`Buffer.from(s, 'base64')`): characters outside the alphabet are skipped,
/// the URL-safe ones count too, and decoding stops at the padding.
fn lenient_base64(s: &str) -> Vec<u8> {
    use base64::Engine;
    let mut clean: String = s
        .chars()
        .take_while(|c| *c != '=')
        .filter_map(|c| match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '+' | '/' => Some(c),
            '-' => Some('+'),
            '_' => Some('/'),
            _ => None,
        })
        .collect();
    // a lone character at the end carries no whole byte
    if clean.len() % 4 == 1 {
        clean.pop();
    }
    let engine = base64::engine::GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        base64::engine::GeneralPurposeConfig::new().with_decode_allow_trailing_bits(true).with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent),
    );
    engine.decode(clean.as_bytes()).unwrap_or_default()
}

/// The provider names `opencode auth list` (base64 in the report) says are connected, credentials first.
pub fn connected_providers(encoded: &str) -> Vec<String> {
    let text = if encoded.is_empty() { String::new() } else { String::from_utf8_lossy(&lenient_base64(encoded)).into_owned() };
    let auth = parse_auth_list(&text);
    let mut out: Vec<String> = vec![];
    for name in auth.credentials.iter().map(|c| &c.name).chain(auth.env.iter().map(|e| &e.name)) {
        if !out.contains(name) {
            out.push(name.clone());
        }
    }
    out
}

/// A global npm install into the user's own prefix when the global one isn't theirs to write.
fn npm_global(spec: &str) -> String {
    format!("{{ npm config get prefix | grep -q \"^$HOME\" || npm config set prefix \"$HOME/.npm-global\"; npm install -g {spec}; }}")
}

/// The agent's own installer when it has one. If the agent still doesn't run after it (a piped
/// installer reports success even when its download failed) and npm is there, its npm package.
/// npm installs into the user's prefix when the global one isn't writable.
pub fn install_command(def: &AgentDef, paths: &str) -> String {
    let via_npm = npm_global(def.install.npm.as_deref().unwrap_or("undefined"));
    let Some(cmd) = &def.install.cmd else { return format!("{paths}; {via_npm}") };
    if def.install.npm.is_none() {
        return format!("{paths}; {cmd}");
    }
    [
        format!("{paths}; {cmd}"),
        format!("if {} >/dev/null 2>&1; then :", def.version),
        format!("elif command -v npm >/dev/null 2>&1; then echo \"The installer didn't finish; trying npm instead.\"; {via_npm}"),
        "else echo \"The installer didn't finish, and there's no npm here to try instead.\"; false; fi".to_owned(),
    ]
    .join("\n")
}

fn npm_package(def: &AgentDef) -> Option<&str> {
    def.latest.as_ref().map(|l| l.npm.as_str()).or(def.install.npm.as_deref())
}

/// How an installed agent got there: through npm when `npm ls -g` lists its package or its program
/// lives in a node_modules or npm-global folder, otherwise through its own installer. An npm copy
/// outside the global folder (an older Inyon kept OpenCode under ~/.local/share/tether-tools, say)
/// comes with the folder it was installed into, so an update goes there and not to a second copy
/// the PATH would never reach.
pub fn installed_via(def: &AgentDef, bin_path: &str, npm_globals: &str) -> InstalledAt {
    let pkg = npm_package(def);
    let globals: Vec<&str> = npm_globals.split(|c: char| crate::js::text::is_space(c)).filter(|s| !s.is_empty()).collect();
    let npm = InstalledAt { via: InstalledVia::Npm, prefix: None };
    if let Some(pkg) = pkg {
        if globals.iter().any(|p| p.ends_with(&format!("/node_modules/{pkg}"))) {
            return npm;
        }
        let own = crate::js::re::dynamic(&format!("^(.*)/node_modules/{}/", crate::js::re::escape(pkg)), "");
        if let Some(m) = own.captures(bin_path) {
            // the first word of npm ls -g is the global folder; a package under it, or under lib/, is global
            let prefix = m.get(1).map_or("", |g| g.as_str());
            let global = globals.first().is_some_and(|root| prefix == *root || prefix == format!("{root}/lib")) || re!(r"/(lib|\.npm-global)$").is_match(prefix);
            return if global { npm } else { InstalledAt { via: InstalledVia::Npm, prefix: Some(prefix.to_owned()) } };
        }
    }
    if re!(r"node_modules|npm-global").is_match(bin_path) {
        return npm;
    }
    InstalledAt { via: InstalledVia::Installer, prefix: None }
}

/// npm install into a folder of its own, where an agent already lives, so the PATH keeps pointing at it.
fn npm_prefixed(prefix: &str, spec: &str) -> String {
    format!("npm install --prefix {} {spec}", shq(prefix))
}

/// The step that brings an agent to its newest version: npm for an npm install (into the user's
/// prefix, as install_command does, or the folder it already sits in), else the agent's own
/// updater, else its installer again.
pub fn update_command(def: &AgentDef, via: Option<InstalledVia>, paths: &str, prefix: Option<&str>) -> String {
    let pkg = npm_package(def);
    if let (Some(InstalledVia::Npm), Some(pkg)) = (via, pkg) {
        if let Some(prefix) = prefix.filter(|p| !p.is_empty()) {
            return format!("{paths}; {}", npm_prefixed(prefix, &format!("{pkg}@latest")));
        }
        return format!("{paths}; {}", npm_global(&format!("{pkg}@latest")));
    }
    if let Some(u) = &def.update {
        return format!("{paths}; {u}");
    }
    install_command(def, paths)
}

/// Which step a manual command is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Install,
    Update,
}

/// The same step as one line the user can paste into a terminal on the machine.
pub fn manual_command(def: &AgentDef, step: Step, via: Option<InstalledVia>, prefix: Option<&str>) -> String {
    let pkg = npm_package(def);
    if step == Step::Update && via == Some(InstalledVia::Npm) {
        if let Some(pkg) = pkg {
            if let Some(prefix) = prefix.filter(|p| !p.is_empty()) {
                return npm_prefixed(prefix, &format!("{pkg}@latest"));
            }
            return format!("npm install -g {pkg}@latest");
        }
    }
    if step == Step::Update {
        if let Some(u) = &def.update {
            return u.clone();
        }
    }
    def.install.cmd.clone().unwrap_or_else(|| format!("npm install -g {}", def.install.npm.as_deref().unwrap_or("undefined")))
}

/// The likely cause of a failed install or update, in one line, from what it printed.
pub fn failure_hint(output: &str, pkg: Option<&str>) -> Option<String> {
    let lines: Vec<&str> = output.split('\n').map(|l| l.jtrim()).filter(|l| !l.is_empty()).collect();
    if lines.is_empty() {
        return None;
    }
    if re!(r"EACCES|permission denied|EPERM", "i").is_match(output) {
        return Some(format!("npm can't write there; set a user prefix: npm config set prefix \"$HOME/.npm-global\" && npm install -g {}@latest", pkg.unwrap_or("<package>")));
    }
    if re!(r"(command not found: npm|npm: (command )?not found|node: (command )?not found|command not found: node|no npm here)", "i").is_match(output) {
        return Some("Install Node.js first (the Setup tab can)".into());
    }
    if re!(
        r"ENOTFOUND|ECONNREFUSED|ECONNRESET|ETIMEDOUT|EAI_AGAIN|timed out|Could not resolve host|Failed to connect|Connection refused|Network is unreachable|\bE(403|50[0-9])\b|returned error: (403|50[0-9])|HTTP(\/[\d.]+)? (403|50[0-9])\b|\b(403|50[0-9]) (Forbidden|Internal Server Error|Bad Gateway|Service Unavailable|Gateway Timeout)",
        "i"
    )
    .is_match(output)
    {
        return Some("The machine couldn't reach the download; check its network and try again".into());
    }
    let err = lines.iter().rev().find(|l| re!(r"error|fail|cannot|can't|not ", "i").is_match(l)).copied().unwrap_or(lines[lines.len() - 1]);
    Some(err.slice16(0, Some(200)).to_owned())
}

/// The last `n` UTF-16 units of `s` (`s.slice(-n)`).
fn tail16(s: &str, n: isize) -> String {
    s.slice16(-n, None).to_owned()
}

/// Runs one setup step. Installs run non-interactively; sign-ins go through features/signin.rs.
pub async fn run_setup(host_id: &str, action: &str) -> HubResult<SetupResult> {
    let host = registry().host(host_id)?;
    let report = setup_report(host_id).await?;
    let pkg = report.pkg.clone();
    let root = |cmd: String| if pkg.as_deref() == Some("brew") { cmd } else { format!("sudo -n sh -c {}", shq(&cmd)) };
    let cmd: Option<String>;
    let mut agent: Option<(&'static AgentDef, Step, Option<InstalledVia>, Option<String>)> = None;
    let mut state: Option<SetupAgent> = None;
    if let (true, Some(p)) = (action == "install:tmux", pkg.as_deref()) {
        cmd = pkg_install(p, "tmux").map(root);
    } else if let (true, Some(p)) = (action == "install:node", pkg.as_deref()) {
        cmd = pkg_install(p, if p == "brew" { "node" } else { "nodejs npm" }).map(root);
    } else if action == "session" {
        let r = host.run("tmux new-session -d -s main -c \"$HOME\" 2>&1 || tmux new-session -d -c \"$HOME\" 2>&1").await?;
        registry().refresh(host_id, 200);
        return Ok(SetupResult { ok: r.code == 0, output: Some(format!("{}{}", r.stdout, r.stderr)), ..Default::default() });
    } else if action.starts_with("agent:") || action.starts_with("update:") {
        let name = &action[action.find(':').map_or(0, |i| i + 1)..];
        let kind = AGENT_KINDS.iter().copied().find(|k| k.as_str() == name).ok_or_else(|| HubError::http(400, "Unknown agent"))?;
        let def = kind.def();
        state = report.agents.iter().find(|a| a.kind == kind).cloned();
        // updating something that isn't there is an install
        let step = if action.starts_with("update:") && state.as_ref().is_some_and(|s| s.installed) { Step::Update } else { Step::Install };
        let node = report.checks.iter().find(|c| c.id == "node").is_some_and(|c| c.ok);
        if step == Step::Install && def.install.cmd.is_none() && !node {
            return Err(HubError::http(409, "Install Node.js first"));
        }
        let via = state.as_ref().and_then(|s| s.via);
        let prefix = state.as_ref().and_then(|s| s.prefix.clone());
        cmd = Some(if step == Step::Update { update_command(def, via, PATHS, prefix.as_deref()) } else { install_command(def, PATHS) });
        agent = Some((def, step, via, prefix));
    } else {
        return Err(HubError::http(400, "Unknown setup step"));
    }
    let Some(cmd) = cmd else { return Err(HubError::http(409, "No package manager found on that machine")) };
    let Some((def, step, via, prefix)) = agent else {
        let r = host.exec(&cmd, ExecOptions::timeout(600_000)).await?;
        registry().refresh(host_id, 200);
        return Ok(SetupResult { ok: r.code == 0, output: Some(tail16(&format!("{}{}", r.stdout, r.stderr), 6000)), ..Default::default() });
    };
    // the agent's version is read in the same round trip, after the step
    let r = host
        .exec(&format!("{cmd}\n__inyon_code=$?; echo \"INYON_VERSION $({PATHS}; {} 2>/dev/null | head -1)\"; exit $__inyon_code", def.version), ExecOptions::timeout(600_000))
        .await?;
    registry().refresh(host_id, 200);
    let out = format!("{}{}", r.stdout, r.stderr);
    let version = out.split('\n').find(|l| l.starts_with("INYON_VERSION ")).map(|l| l["INYON_VERSION ".len()..].jtrim().to_owned()).filter(|v| !v.is_empty());
    let output = tail16(out.split('\n').filter(|l| !l.starts_with("INYON_VERSION ")).collect::<Vec<_>>().join("\n").jtrim(), 6000);
    let latest = state.and_then(|s| s.latest);
    // the step said it failed, or it ended and the agent still isn't there
    let ok = r.code == 0 && version.is_some();
    let mut result = SetupResult { ok, output: Some(output.clone()), version: version.clone(), latest, ..Default::default() };
    if !ok {
        result.manual = Some(manual_command(def, step, via, prefix.as_deref()));
        result.hint = failure_hint(&output, npm_package(def)).or_else(|| version.is_none().then(|| format!("{} still doesn't run after the step", def.name)));
    }
    Ok(result)
}

/// "Keep a tmux session running" on a machine, or not.
pub fn set_keep_session(host_id: &str, on: bool) -> HubResult<()> {
    let mut s = load_settings();
    let mut keep = match s.0.get("keepSession") {
        Some(serde_json::Value::Object(m)) => m.clone(),
        _ => serde_json::Map::new(),
    };
    keep.insert(host_id.to_owned(), serde_json::Value::Bool(on));
    s.0.insert("keepSession".into(), serde_json::Value::Object(keep));
    save_settings(&s).map_err(HubError::plain)?;
    if on {
        spawn(async {
            let _ = ensure_sessions().await;
        });
    }
    Ok(())
}

/// "Keep a tmux session running": make sure each opted-in machine has at least one session.
pub async fn ensure_sessions() -> HubResult<()> {
    let keep = load_settings().keep_session();
    for h in registry().overview() {
        if !keep.get(&h.config.id).copied().unwrap_or(false) || h.status != HostStatus::Online || h.tmux == TmuxState::Missing {
            continue;
        }
        if h.sessions.is_empty() {
            registry().host(&h.config.id)?.run("tmux new-session -d -s main -c \"$HOME\" 2>/dev/null || true").await?;
            registry().refresh(&h.config.id, 200);
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "setup_tests.rs"]
mod tests;
