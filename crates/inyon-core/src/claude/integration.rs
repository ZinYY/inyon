//! Port of claude/integration.ts: the hook and status line scripts Inyon puts on a machine, and
//! installing them into Claude Code's and Antigravity CLI's settings. The scripts are byte for
//! byte the TS's (fixtures/expected/scripts.json checks it), the marker included.

use serde_json::{json, Map, Value};

use crate::hosts::host::{shq, ExecOptions, Host};
use crate::hosts::http_error::{HubError, HubResult};
use crate::js::json::{self, spread, stringify_pretty, Js};
use crate::js::JsStr;
use crate::re;

/// The hook only appends the agent's own hook payload, tagged with the tmux pane, to a local log.
/// It always exits 0 and changes nothing the agent does: Claude Code's hook prints nothing, and
/// another agent's (`hook.sh copilot agentStop`) prints the empty answer those wait for.
pub const HOOK_SCRIPT: &str = r#"#!/bin/sh
# Inyon: records coding agents' events so Inyon knows which tmux pane runs which session.
# Safe to delete; turning Inyon's integration off removes it from each agent's settings.
# inyon-integration 9
# the events hold your prompts: readable by you only
umask 077
d="$HOME/.inyon"
mkdir -p "$d/panes" 2>/dev/null
# an agent other than Claude Code names itself and the event, and wants a JSON answer
agent=$(printf '%s' "${1:-}" | tr -cd 'a-z0-9_-')
event=$(printf '%s' "${2:-}" | tr -cd 'A-Za-z0-9_-')
[ -n "$agent" ] && echo '{}'
p=$(tr -d '\n\r')
[ -n "$p" ] || exit 0
pane=${TMUX_PANE:-}
if [ -n "$pane" ] && [ -n "${PSMUX_SESSION:-}" ]; then
  for _ in 1 2; do s=$(tmux display -p -t "$PSMUX_SESSION" '#{session_id}' 2>/dev/null | tr -d '$\r'); [ -n "$s" ] && break; done
  case $s in ''|*[!0-9]*) ;; *) pane="%$s.${pane#%}" ;; esac
fi
# a conversation Claude Code moved to the background runs outside tmux: its events go to the window it left
if [ -z "$pane" ] && [ -z "$agent" ]; then
  j=$(printf '%s' "$p" | sed -n 's/^{"session_id":"\([0-9a-f]\{8\}\).*/\1/p')
  w=$([ -n "$j" ] && grep -h "\"parkedJobId\":\"$j\"" "${CLAUDE_CONFIG_DIR:-$HOME/.claude}"/sessions/*.json 2>/dev/null | sed -n 's/.*"tmux":"\([^"]*\)".*/\1/p' | head -n 1)
  case $w in *:@*.%*)
    pane="%${w##*.%}"
    case $(tmux -V 2>/dev/null) in *psmux*)
      s=$(tmux display -p -t "${w%:@*}" '#{session_id}' 2>/dev/null | tr -d '$\r')
      case $s in ''|*[!0-9]*) pane= ;; *) pane="%$s.${pane#%}" ;; esac ;;
    esac ;;
  esac
fi
if [ -n "$agent" ]; then
  line=$(printf '{"pane":"%s","ts":%s,"agent":"%s","event":"%s","data":%s}' "$pane" "$(date +%s)" "$agent" "$event" "$p")
else
  line=$(printf '{"pane":"%s","ts":%s,"data":%s}' "$pane" "$(date +%s)" "$p")
fi
printf '%s\n' "$line" >> "$d/events.jsonl"
[ -n "$pane" ] && printf '%s\n' "$line" > "$d/panes/${pane#%}.json"
# after a compaction, so Inyon stops showing the context size from before it
case "$p" in
  *'"hook_event_name":"SessionStart"'*'"source":"compact"'*|*'"source":"compact"'*'"hook_event_name":"SessionStart"'*)
    [ -n "$pane" ] && date +%s > "$d/panes/${pane#%}.compact" ;;
esac
s=$(wc -c < "$d/events.jsonl" 2>/dev/null || echo 0)
[ "$s" -gt 2097152 ] && mv -f "$d/events.jsonl" "$d/events.old.jsonl"
exit 0
"#;
/// Claude Code hands its status line command a JSON payload with the model, context window, cost
/// and rate limits. The wrapper saves it per tmux pane, then runs the user's own status line.
pub const STATUS_SCRIPT: &str = r#"#!/bin/sh
# Inyon: saves Claude Code's status line data per tmux pane, then runs your own status line.
# inyon-integration 9
in=$(cat)
pane=${TMUX_PANE:-}
if [ -n "$pane" ] && [ -n "${PSMUX_SESSION:-}" ]; then
  for _ in 1 2; do s=$(tmux display -p -t "$PSMUX_SESSION" '#{session_id}' 2>/dev/null | tr -d '$\r'); [ -n "$s" ] && break; done
  case $s in ''|*[!0-9]*) ;; *) pane="%$s.${pane#%}" ;; esac
fi
# (in a subshell, so your own status line below keeps your umask)
if [ -n "$pane" ]; then (
  umask 077
  d="$HOME/.inyon/status"; f="$d/${pane#%}"
  mkdir -p "$d" 2>/dev/null
  printf '%s' "$in" | tr -d '\n\r' > "$f.json.$$" && mv -f "$f.json.$$" "$f.json"
  # the signed-in account, so limits reported for a previous account are never shown as this one's
  cfg="${CLAUDE_CONFIG_DIR:-$HOME}/.claude.json"
  printf '%s %s\n' "$(a() { v=$(tr -d '\n\r' < "$1" 2>/dev/null | grep -o '"oauthAccount": *{[^}]*' | grep -o '"accountUuid": *"[^"]*"' | head -n 1 | sed 's/.*"\([^"]*\)"$/\1/'); echo "${v:--}"; }; a "$cfg")" "$cfg" > "$f.acct"
)
fi
orig="$HOME/.inyon/statusline.orig"
# never this wrapper a second time: a saved command that leads back here would start itself forever
[ -s "$orig" ] && [ -z "${INYON_STATUSLINE:-}" ] && printf '%s' "$in" | INYON_STATUSLINE=1 sh "$orig"
exit 0
"#;
/// Antigravity CLI (agy) runs its status line command with a similar payload. Saved per pane in
/// its own folder, then your own agy status line runs.
pub const AGY_STATUS_SCRIPT: &str = r#"#!/bin/sh
# Inyon: saves Antigravity CLI's status line data per tmux pane, then runs your own status line.
# Safe to delete; remove "statusLine" from ~/.gemini/antigravity-cli/settings.json too.
# inyon-integration 9
in=$(cat)
pane=${TMUX_PANE:-}
if [ -n "$pane" ] && [ -n "${PSMUX_SESSION:-}" ]; then
  for _ in 1 2; do s=$(tmux display -p -t "$PSMUX_SESSION" '#{session_id}' 2>/dev/null | tr -d '$\r'); [ -n "$s" ] && break; done
  case $s in ''|*[!0-9]*) ;; *) pane="%$s.${pane#%}" ;; esac
fi
if [ -n "$pane" ]; then (
  umask 077
  d="$HOME/.inyon/agy-status"; f="$d/${pane#%}"
  mkdir -p "$d" 2>/dev/null
  printf '%s' "$in" | tr -d '\n\r' > "$f.json.$$" && mv -f "$f.json.$$" "$f.json"
)
fi
orig="$HOME/.inyon/agy-statusline.orig"
[ -s "$orig" ] && [ -z "${INYON_STATUSLINE:-}" ] && printf '%s' "$in" | INYON_STATUSLINE=1 sh "$orig"
exit 0
"#;

const HOOK_CMD: &str = "~/.inyon/hook.sh";
const STATUS_CMD: &str = "~/.inyon/statusline.sh";

/// Entries written before the rename to Inyon name ~/.tether: still ours, so they're replaced, not doubled.
fn ours(c: Option<&Value>, file: &str) -> bool {
    c.str().is_some_and(|c| c.contains(&format!(".inyon/{file}")) || c.contains(&format!(".tether/{file}")))
}
fn is_our_status(c: Option<&Value>) -> bool {
    ours(c, "statusline.sh")
}
fn is_our_status_str(c: &str) -> bool {
    is_our_status(Some(&Value::String(c.to_owned())))
}
fn is_ours(h: Option<&Value>) -> bool {
    ours(h.at("command"), "hook.sh")
}

const EVENTS: [(&str, Option<&str>); 6] =
    [("SessionStart", None), ("SessionEnd", None), ("UserPromptSubmit", None), ("Notification", None), ("PermissionRequest", Some("*")), ("Stop", None)];

/// A turn that ended on an error (a rate limit, signed out, overloaded). Only for Claude Code 2.1.78
/// and later, which have it: before 2.1.101 a hook event Claude Code doesn't know makes it ignore
/// the whole settings file.
const STOP_FAILURE: (&str, Option<&str>) = ("StopFailure", None);

/// Whether `claude --version` (e.g. "2.1.309 (Claude Code)") names a version with StopFailure.
pub fn knows_stop_failure(version: &str) -> bool {
    let Some(v) = re!(r"^\s*(\d+)\.(\d+)\.(\d+)").captures(version) else { return false };
    let n = |i: usize| v[i].parse::<u64>().unwrap_or(0);
    (n(1), n(2), n(3)) >= (2, 1, 78)
}

/// `JSON.stringify(settings, null, 2) + '\n'`
fn pretty(v: &Value) -> String {
    format!("{}\n", stringify_pretty(v))
}

fn or_message(stderr: &str, fallback: impl Into<String>) -> String {
    let t = stderr.jtrim();
    if t.is_empty() {
        fallback.into()
    } else {
        t.to_owned()
    }
}

/// What the TS gets from `JSON.parse` before it reads members off it: an object. Anything else
/// (an array, a number, null) the TS would mangle or trip over, so it's refused as invalid.
fn parse_object(text: &str) -> Option<Map<String, Value>> {
    match json::parse(text)? {
        Value::Object(m) => Some(m),
        _ => None,
    }
}

/// The script that reads ~/.claude/settings.json: its checksum on the first line (or "none"), then the file.
pub(crate) const READ_SETTINGS: &str = r#"f="$HOME/.claude/settings.json"; [ -f "$f" ] || { echo none; exit 0; }; s=$(sum "$f") || exit 1; echo "$s"; cat "$f""#;

/// The script that writes it, only if it still has the checksum it was read with (none: it wasn't
/// there), else exit 3. A settings.json that is a link to your dotfiles stays a link: the file it
/// points to is written. Private to you (it often holds API keys), and no half-written copy left if
/// the connection drops.
pub(crate) fn write_settings_command(sum: &str) -> String {
    let check = if sum.is_empty() { r#"[ -e "$f" ] && exit 3"#.to_owned() } else { format!(r#"{{ [ -f "$f" ] && [ "$(sum "$f")" = {} ]; }} || exit 3"#, shq(sum)) };
    [
        r#"umask 077; f="$HOME/.claude/settings.json"; t="$f.inyon-tmp"; mkdir -p "$HOME/.claude""#,
        &check,
        r#"trap 'rm -f "$t"' EXIT HUP INT TERM"#,
        r#"[ -f "$f" ] && [ ! -f "$f.tether-backup" ] && [ ! -f "$f.inyon-backup" ] && cp "$f" "$f.inyon-backup""#,
        r#"if [ -L "$f" ]; then cat > "$t" && cat "$t" > "$f"; else cat > "$t" && mv -f "$t" "$f"; fi"#,
    ]
    .join("\n")
}

/// The settings and the checksum they were read with ("" when there was no file).
async fn read_settings(host: &Host) -> HubResult<(Map<String, Value>, String)> {
    let r = host.run(READ_SETTINGS).await?;
    if r.code != 0 {
        return Err(HubError::http(500, or_message(&r.stderr, "Could not read ~/.claude/settings.json")));
    }
    let (first, text) = r.stdout.split_once('\n').unwrap_or((r.stdout.as_str(), ""));
    let sum = if first.jtrim() == "none" { String::new() } else { first.jtrim().to_owned() };
    if text.jtrim().is_empty() {
        return Ok((Map::new(), sum));
    }
    let settings = parse_object(text).ok_or_else(|| HubError::http(409, "~/.claude/settings.json on that machine is not valid JSON; fix it first"))?;
    Ok((settings, sum))
}

/// False when the file changed since it was read (Claude Code saved a setting meanwhile).
async fn write_settings(host: &Host, settings: &Map<String, Value>, sum: &str) -> HubResult<bool> {
    let r = host.exec(&write_settings_command(sum), ExecOptions::input(pretty(&Value::Object(settings.clone())))).await?;
    match r.code {
        0 => Ok(true),
        3 => Ok(false),
        _ => Err(HubError::http(500, or_message(&r.stderr, "Could not write settings"))),
    }
}

/// Settings that kept changing under each try.
fn settings_kept_changing() -> HubError {
    HubError::http(409, "~/.claude/settings.json kept changing while Inyon wrote it; try again")
}

async fn write_file(host: &Host, path: &str, content: &str, mode: &str) -> HubResult<()> {
    let r = host.exec(&format!(r#"umask 077; mkdir -p "$HOME/.inyon" && cat > "$HOME/{path}" && chmod {mode} "$HOME/{path}""#), ExecOptions::input(content)).await?;
    if r.code != 0 {
        return Err(HubError::http(500, or_message(&r.stderr, format!("Could not write ~/{path}"))));
    }
    Ok(())
}

async fn cat_trimmed(host: &Host, cmd: &str) -> HubResult<String> {
    Ok(host.run(cmd).await?.stdout.jtrim().to_owned())
}

/// The status line command in the settings backup made when Inyon was first installed, if it can run here.
async fn status_before_inyon(host: &Host) -> HubResult<String> {
    for f in ["settings.json.inyon-backup", "settings.json.tether-backup"] {
        let r = host.run(&format!(r#"cat "$HOME/.claude/{f}" 2>/dev/null"#)).await?;
        if r.stdout.jtrim().is_empty() {
            continue;
        }
        // `JSON.parse(..).statusLine` throws for null as for text that isn't JSON: the next file then
        let Some(parsed) = json::parse(&r.stdout).filter(|v| !v.is_null()) else { continue };
        let sl = Some(&parsed).at("statusLine");
        let cmd = match (sl.at("type").str(), sl.at("command").str()) {
            (Some("command"), Some(c)) => c.jtrim(),
            _ => "",
        };
        return Ok(if !cmd.is_empty() && !is_our_status_str(cmd) && !windows_path(cmd) { cmd.to_owned() } else { String::new() });
    }
    Ok(String::new())
}

fn windows_path(cmd: &str) -> bool {
    re!(r#"(^|[\s"'])[A-Za-z]:[\\/]"#).is_match(cmd)
}

/// The command of a `{ type: 'command', command }` status line, or "".
fn command_of(sl: Option<&Value>) -> String {
    match (sl.at("type").str(), sl.at("command").str()) {
        (Some("command"), Some(c)) => c.to_owned(),
        _ => String::new(),
    }
}

fn with_command(sl: Option<&Value>, command: &str) -> Value {
    let mut m = spread(sl);
    m.insert("command".into(), Value::String(command.into()));
    Value::Object(m)
}

/// Puts the status line and the hooks into ~/.claude/settings.json, read again and redone when it
/// changed in between; false when it changed every time.
async fn install_settings(host: &Host) -> HubResult<bool> {
    for _ in 0..3 {
        if install_settings_once(host).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn install_settings_once(host: &Host) -> HubResult<bool> {
    let (mut settings, sum) = read_settings(host).await?;
    let sl = settings.get("statusLine").cloned();
    let sl = sl.as_ref();
    if !is_our_status(sl.at("command")) {
        let cmd = command_of(sl);
        let prev = cat_trimmed(host, r#"cat "$HOME/.inyon/statusline.orig" 2>/dev/null"#).await?;
        // Settings synced from a Windows machine bring a status line that can't run here. Keep the one that does.
        if !(windows_path(&cmd) && !prev.is_empty() && !windows_path(&prev)) {
            write_file(host, ".inyon/statusline.orig", &if cmd.is_empty() { String::new() } else { format!("{cmd}\n") }, "600").await?;
        }
        // `{ ...(sl ?? { padding: 0 }), type: 'command', command }`
        let mut m = match sl {
            None | Some(Value::Null) => Map::from_iter([("padding".to_owned(), Value::from(0))]),
            sl => spread(sl),
        };
        m.insert("type".into(), Value::String("command".into()));
        m.insert("command".into(), Value::String(STATUS_CMD.into()));
        settings.insert("statusLine".into(), Value::Object(m));
    } else {
        if sl.at("command").str() != Some(STATUS_CMD) {
            settings.insert("statusLine".into(), with_command(sl, STATUS_CMD));
        }
        // a saved "own" status line that is this wrapper again would start itself forever: use the one
        // from before Inyon was installed instead, if there was one
        let prev = cat_trimmed(host, r#"cat "$HOME/.inyon/statusline.orig" 2>/dev/null"#).await?;
        if is_our_status_str(&prev) {
            let before = status_before_inyon(host).await?;
            write_file(host, ".inyon/statusline.orig", &if before.is_empty() { String::new() } else { format!("{before}\n") }, "600").await?;
        }
    }
    let version = host.run("claude --version 2>/dev/null").await?.stdout;
    let stop_failure = knows_stop_failure(&version).then_some(STOP_FAILURE);
    if settings.get("hooks").is_none_or(Value::is_null) {
        settings.insert("hooks".into(), Value::Object(Map::new()));
    }
    let Some(Value::Object(hooks)) = settings.get_mut("hooks") else {
        return Err(HubError::plain("~/.claude/settings.json has \"hooks\" that aren't an object"));
    };
    for (name, matcher) in EVENTS.into_iter().chain(stop_failure) {
        if hooks.get(name).is_none_or(Value::is_null) {
            hooks.insert(name.into(), Value::Array(vec![]));
        }
        let Some(Value::Array(groups)) = hooks.get_mut(name) else {
            return Err(HubError::plain(format!("~/.claude/settings.json has hooks for {name} that aren't a list")));
        };
        // an entry naming the old folder points at the new one
        for g in groups.iter_mut() {
            if let Some(Value::Array(hs)) = g.get_mut("hooks") {
                for h in hs.iter_mut() {
                    if is_ours(Some(h)) {
                        h["command"] = Value::String(HOOK_CMD.into());
                    }
                }
            }
        }
        if groups.iter().any(|g| Some(g).at("hooks").arr().is_some_and(|hs| hs.iter().any(|h| is_ours(Some(h))))) {
            continue;
        }
        let mut entry = Map::new();
        if let Some(m) = matcher {
            entry.insert("matcher".into(), Value::String(m.into()));
        }
        entry.insert("hooks".into(), json!([{ "type": "command", "command": HOOK_CMD }]));
        groups.push(Value::Object(entry));
    }
    // a Claude Code older than StopFailure (one downgraded since, say) ignores a settings file that has it
    if stop_failure.is_none() {
        drop_ours(hooks, STOP_FAILURE.0);
    }
    write_settings(host, &settings, &sum).await
}

pub async fn install_integration(host: &Host) -> HubResult<()> {
    write_file(host, ".inyon/hook.sh", HOOK_SCRIPT, "700").await?;
    write_file(host, ".inyon/statusline.sh", STATUS_SCRIPT, "700").await?;
    // ~/.inyon holds your prompts (events.jsonl, panes/): earlier versions left it readable by others
    host.run(r#"umask 077; mkdir -p "$HOME/.inyon/panes" "$HOME/.inyon/status" && chmod 700 "$HOME/.inyon" && chmod 600 "$HOME/.inyon/events.jsonl" "$HOME/.inyon/events.old.jsonl" 2>/dev/null; true"#).await?;
    if !install_settings(host).await? {
        return Err(settings_kept_changing());
    }
    install_agy_integration(host).await?;
    crate::claude::agent_hooks::install_agent_hooks(host).await?;
    Ok(())
}

const AGY_SETTINGS: &str = "$HOME/.gemini/antigravity-cli/settings.json";
const AGY_STATUS_CMD: &str = "~/.inyon/agy-statusline.sh";
fn is_our_agy_status(c: Option<&Value>) -> bool {
    ours(c, "agy-statusline.sh")
}

/// agy's settings, or None when agy isn't on the machine (nothing to install into).
async fn read_agy_settings(host: &Host) -> HubResult<Option<Map<String, Value>>> {
    let r = host.run(&format!(r#"if command -v agy >/dev/null 2>&1 || [ -d "$HOME/.gemini/antigravity-cli" ]; then echo @@; cat "{AGY_SETTINGS}" 2>/dev/null; fi"#)).await?;
    let Some(rest) = r.stdout.strip_prefix("@@") else { return Ok(None) };
    let text = rest.jtrim();
    if text.is_empty() {
        return Ok(Some(Map::new()));
    }
    parse_object(text).map(Some).ok_or_else(|| HubError::http(409, "~/.gemini/antigravity-cli/settings.json on that machine is not valid JSON; fix it first"))
}

async fn write_agy_settings(host: &Host, settings: &Map<String, Value>) -> HubResult<()> {
    let f = AGY_SETTINGS;
    let cmd = [
        format!(r#"umask 077; trap 'rm -f "{f}.inyon-tmp"' EXIT HUP INT TERM"#),
        r#"mkdir -p "$HOME/.gemini/antigravity-cli""#.to_owned(),
        format!(r#"[ -f "{f}" ] && [ ! -f "{f}.tether-backup" ] && [ ! -f "{f}.inyon-backup" ] && cp "{f}" "{f}.inyon-backup""#),
        format!(r#"cat > "{f}.inyon-tmp" && mv "{f}.inyon-tmp" "{f}""#),
    ]
    .join("; ");
    let r = host.exec(&cmd, ExecOptions::input(pretty(&Value::Object(settings.clone())))).await?;
    if r.code != 0 {
        return Err(HubError::http(500, or_message(&r.stderr, "Could not write agy settings")));
    }
    Ok(())
}

/// Adds the status line wrapper to agy's settings. Sessions already running pick it up when restarted.
pub async fn install_agy_integration(host: &Host) -> HubResult<bool> {
    let Some(mut settings) = read_agy_settings(host).await? else { return Ok(false) };
    write_file(host, ".inyon/agy-statusline.sh", AGY_STATUS_SCRIPT, "700").await?;
    let sl = settings.get("statusLine").cloned();
    let sl = sl.as_ref();
    if !is_our_agy_status(sl.at("command")) {
        let cmd = command_of(sl);
        write_file(host, ".inyon/agy-statusline.orig", &if cmd.is_empty() { String::new() } else { format!("{cmd}\n") }, "600").await?;
        // with no status line of your own, agy keeps drawing its default one (the wrapper prints nothing)
        let v = if !cmd.is_empty() {
            let mut m = spread(sl);
            m.insert("type".into(), Value::String("command".into()));
            m.insert("command".into(), Value::String(AGY_STATUS_CMD.into()));
            Value::Object(m)
        } else {
            json!({ "type": "command", "command": AGY_STATUS_CMD, "stack_with_default": true })
        };
        settings.insert("statusLine".into(), v);
        write_agy_settings(host, &settings).await?;
    } else {
        if sl.at("command").str() != Some(AGY_STATUS_CMD) {
            let mut s = settings.clone();
            s.insert("statusLine".into(), with_command(sl, AGY_STATUS_CMD));
            write_agy_settings(host, &s).await?;
        }
        let prev = cat_trimmed(host, r#"cat "$HOME/.inyon/agy-statusline.orig" 2>/dev/null"#).await?;
        if is_our_agy_status(Some(&Value::String(prev))) {
            write_file(host, ".inyon/agy-statusline.orig", "", "600").await?;
        }
    }
    Ok(true)
}

pub async fn uninstall_agy_integration(host: &Host) -> HubResult<()> {
    if let Some(mut settings) = read_agy_settings(host).await? {
        let sl = settings.get("statusLine").cloned();
        if is_our_agy_status(sl.as_ref().at("command")) {
            let orig = cat_trimmed(host, r#"cat "$HOME/.inyon/agy-statusline.orig" 2>/dev/null"#).await?;
            if !orig.is_empty() && !is_our_agy_status(Some(&Value::String(orig.clone()))) {
                settings.insert("statusLine".into(), with_command(sl.as_ref(), &orig));
            } else {
                settings.shift_remove("statusLine");
            }
            write_agy_settings(host, &settings).await?;
        }
    }
    host.run(r#"rm -rf "$HOME/.inyon/agy-statusline.sh" "$HOME/.inyon/agy-statusline.orig" "$HOME/.inyon/agy-status""#).await?;
    Ok(())
}

/// Project settings files Inyon added its status line to (see reconnect_status_line).
const PROJECT_LIST: &str = ".inyon/project-statuslines";

/// Gets a running Claude session reporting again without restarting it. A session whose user
/// settings were swapped under it stops reloading them, but it still watches its project's
/// .claude/settings.local.json; putting the status line there makes it pick it up within seconds.
/// The file stays out of git (via .git/info/exclude) and is listed so turning the integration
/// off removes it again.
pub async fn reconnect_status_line(host: &Host, cwd: &str) -> HubResult<()> {
    let dir = format!("{}/.claude", re!(r"/+$").replace(cwd, ""));
    let file = format!("{dir}/settings.local.json");
    let cur = host.run(&format!("cat {} 2>/dev/null", shq(&file))).await?.stdout;
    let mut settings = Map::new();
    if !cur.jtrim().is_empty() {
        settings = parse_object(&cur).ok_or_else(|| HubError::http(409, format!("{file} is not valid JSON")))?;
    }
    let sl = settings.get("statusLine").cloned();
    if Some(sl.as_ref()).flatten().truthy() && !is_our_status(sl.as_ref().at("command")) {
        return Err(HubError::http(409, "This project has its own status line"));
    }
    if !is_our_status(sl.as_ref().at("command")) {
        settings.insert("statusLine".into(), json!({ "type": "command", "command": STATUS_CMD, "padding": 0 }));
        let w = host.exec(&format!("umask 077; mkdir -p {} && cat > {}", shq(&dir), shq(&file)), ExecOptions::input(pretty(&Value::Object(settings)))).await?;
        if w.code != 0 {
            return Err(HubError::http(500, or_message(&w.stderr, format!("Could not write {file}"))));
        }
    }
    host.run(&format!(
        r#"cd {} 2>/dev/null && x=$(git rev-parse --git-path info/exclude 2>/dev/null) && {{ git check-ignore -q .claude/settings.local.json || {{ mkdir -p "$(dirname "$x")"; echo '.claude/settings.local.json' >> "$x"; }}; }}; l="$HOME/{PROJECT_LIST}"; grep -qxF {f} "$l" 2>/dev/null || echo {f} >> "$l""#,
        shq(cwd),
        f = shq(&file)
    ))
    .await?;
    Ok(())
}

/// Takes Inyon's status line back out of the project settings it was added to.
async fn remove_project_status_lines(host: &Host) -> HubResult<()> {
    let list = host.run(&format!(r#"cat "$HOME/{PROJECT_LIST}" 2>/dev/null"#)).await?.stdout;
    for file in list.split('\n').filter(|l| !l.is_empty()) {
        let cur = host.run(&format!("cat {} 2>/dev/null", shq(file))).await?.stdout;
        let Some(mut s) = parse_object(&cur) else { continue };
        if !is_our_status(s.get("statusLine").at("command")) {
            continue;
        }
        s.shift_remove("statusLine");
        if !s.is_empty() {
            host.exec(&format!("cat > {}", shq(file)), ExecOptions::input(pretty(&Value::Object(s)))).await?;
        } else {
            host.run(&format!("rm -f {}", shq(file))).await?;
        }
    }
    host.run(&format!(r#"rm -f "$HOME/{PROJECT_LIST}""#)).await?;
    Ok(())
}

/// Takes Inyon's hook out of one event's groups, leaving yours; a group left empty goes, and so
/// does the event when nothing is left.
fn drop_ours(hooks: &mut Map<String, Value>, name: &str) {
    let Some(Value::Array(groups)) = hooks.get(name) else { return };
    let kept: Vec<Value> = groups
        .iter()
        .map(|g| {
            let mut m = spread(Some(g));
            let hs: Vec<Value> = Some(g).at("hooks").arr().map(|hs| hs.iter().filter(|h| !is_ours(Some(h))).cloned().collect()).unwrap_or_default();
            m.insert("hooks".into(), Value::Array(hs));
            Value::Object(m)
        })
        .filter(|g| g["hooks"].as_array().is_some_and(|h| !h.is_empty()))
        .collect();
    if !kept.is_empty() {
        hooks.insert(name.to_owned(), Value::Array(kept));
    } else {
        hooks.shift_remove(name);
    }
}

/// Takes the status line and the hooks out of ~/.claude/settings.json, redone when it changed in
/// between; false when it changed every time.
async fn uninstall_settings(host: &Host) -> HubResult<bool> {
    for _ in 0..3 {
        if uninstall_settings_once(host).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn uninstall_settings_once(host: &Host) -> HubResult<bool> {
    let (mut settings, sum) = read_settings(host).await?;
    let sl = settings.get("statusLine").cloned();
    if is_our_status(sl.as_ref().at("command")) {
        let orig = cat_trimmed(host, r#"cat "$HOME/.inyon/statusline.orig" 2>/dev/null"#).await?;
        if !orig.is_empty() && !is_our_status_str(&orig) {
            settings.insert("statusLine".into(), with_command(sl.as_ref(), &orig));
        } else {
            settings.shift_remove("statusLine");
        }
    }
    if let Some(Value::Object(hooks)) = settings.get_mut("hooks") {
        let names: Vec<String> = hooks.keys().cloned().collect();
        for name in names {
            drop_ours(hooks, &name);
        }
        if hooks.is_empty() {
            settings.shift_remove("hooks");
        }
    }
    write_settings(host, &settings, &sum).await
}

pub async fn uninstall_integration(host: &Host) -> HubResult<()> {
    remove_project_status_lines(host).await?;
    if !uninstall_settings(host).await? {
        return Err(settings_kept_changing());
    }
    // the hook's log holds your prompts: it goes with the hook
    host.run(r#"rm -f "$HOME/.inyon/hook.sh" "$HOME/.inyon/statusline.sh" "$HOME/.inyon/statusline.orig" "$HOME/.inyon/events.jsonl" "$HOME/.inyon/events.old.jsonl""#).await?;
    crate::claude::agent_hooks::uninstall_agent_hooks(host).await?;
    uninstall_agy_integration(host).await
}

#[cfg(test)]
#[path = "integration_tests.rs"]
mod tests;
