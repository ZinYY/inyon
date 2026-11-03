// The types and the AGENTS catalog of packages/protocol/src/agents.ts. The catalog's data is
// agents.json, written from the TypeScript one by `bun packages/protocol/scripts/catalog.ts`; the
// protocol package's tests fail when the two differ, so this copy never drifts.

use std::sync::OnceLock;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use ts_rs::TS;

use crate::js::Num;
use crate::types::StringMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum AgentKind {
    Claude,
    Codex,
    Opencode,
    Copilot,
    Gemini,
    Antigravity,
    Cursor,
    Grok,
    Pi,
    Amp,
    Goose,
    Hermes,
    Kimi,
    Kiro,
    Qwen,
    Qoder,
    Codebuddy,
}

/// Every agent, in the catalog's order (the order `detect_agent` tries them in).
pub const AGENT_KINDS: [AgentKind; 17] = [
    AgentKind::Claude,
    AgentKind::Codex,
    AgentKind::Opencode,
    AgentKind::Copilot,
    AgentKind::Gemini,
    AgentKind::Antigravity,
    AgentKind::Cursor,
    AgentKind::Grok,
    AgentKind::Pi,
    AgentKind::Amp,
    AgentKind::Goose,
    AgentKind::Hermes,
    AgentKind::Kimi,
    AgentKind::Kiro,
    AgentKind::Qwen,
    AgentKind::Qoder,
    AgentKind::Codebuddy,
];

/// The agents every app knows. Apps from before 0.23 crash on a kind they don't have, so the hub
/// shows them the others as plain terminals (see `AgentKind::known_to`).
pub const FIRST_KINDS: usize = 6;

impl AgentKind {
    /// The name on the wire ("claude", "opencode").
    pub fn as_str(self) -> &'static str {
        match self {
            AgentKind::Claude => "claude",
            AgentKind::Codex => "codex",
            AgentKind::Opencode => "opencode",
            AgentKind::Copilot => "copilot",
            AgentKind::Gemini => "gemini",
            AgentKind::Antigravity => "antigravity",
            AgentKind::Cursor => "cursor",
            AgentKind::Grok => "grok",
            AgentKind::Pi => "pi",
            AgentKind::Amp => "amp",
            AgentKind::Goose => "goose",
            AgentKind::Hermes => "hermes",
            AgentKind::Kimi => "kimi",
            AgentKind::Kiro => "kiro",
            AgentKind::Qwen => "qwen",
            AgentKind::Qoder => "qoder",
            AgentKind::Codebuddy => "codebuddy",
        }
    }

    /// Whether an app that knows `catalog` agents can show this one: 1 for the first six, 2 for
    /// all of them (from 0.23, when the app falls back on its own for a kind it doesn't have).
    pub fn known_to(self, catalog: u32) -> bool {
        catalog >= 2 || AGENT_KINDS[..FIRST_KINDS].contains(&self)
    }

    /// Whether Inyon reads its conversation as chat; the others stay in their terminal.
    pub fn chat(self) -> bool {
        self.def().chat
    }

    pub fn def(self) -> &'static AgentDef {
        &agents()[&self]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SkinTokens {
    pub bg: String,
    pub panel: String,
    pub raised: String,
    pub hover: String,
    pub line: String,
    pub line_strong: String,
    pub fg: String,
    pub fg2: String,
    pub fg3: String,
    pub accent: String,
    pub accent_fg: String,
    pub accent_soft: String,
    pub accent_text: String,
    pub bubble: String,
    pub code: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum CommandSource {
    BuiltIn,
    Skill,
    Project,
    User,
    Plugin,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
pub struct SlashCommand {
    pub name: String,
    pub description: String,
    pub source: CommandSource,
    /// runs immediately when picked (no arguments expected)
    pub instant: Option<bool>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct AgentModel {
    pub id: String,
    pub label: String,
    pub note: Option<String>,
    pub context: Option<Num>,
    /// Advertised choices for this model; empty means there is no adjustable effort.
    pub efforts: Option<Vec<String>>,
    /// The level the CLI uses when none is chosen.
    pub default_effort: Option<String>,
    /// CLI model ids for a family whose effort is encoded in the model name.
    pub variants: Option<StringMap>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum CatalogSource {
    Cli,
    Fallback,
}

/// A mode (permission preset, plan mode, OpenCode agent) the app can switch to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ModeOption {
    pub id: String,
    pub label: String,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct AgentCatalog {
    pub models: Vec<AgentModel>,
    pub source: CatalogSource,
    pub updated_at: u64,
    pub error: Option<String>,
    /// the agent's own list of modes when it can be asked (OpenCode's primary agents, custom ones included)
    pub modes: Option<Vec<ModeOption>>,
}

/// How to install an agent: its own installer when it has one, its npm package otherwise.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
pub struct AgentInstall {
    pub cmd: Option<String>,
    pub npm: Option<String>,
    pub docs: String,
}

/// Where an agent's newest version is published.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct LatestSource {
    pub npm: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum ReplyFont {
    Serif,
    Sans,
    Mono,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Skin {
    pub light: SkinTokens,
    pub dark: SkinTokens,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum ConfigFormat {
    Json,
    Toml,
    Md,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ConfigFile {
    pub path: String,
    pub label: String,
    pub format: ConfigFormat,
}

/// The commands that turn Claude Code's ultracode switch on and off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct UltracodeCommands {
    pub on: String,
    pub off: String,
}

/// How to set things from the outside. The TypeScript catalog has functions here; this has their
/// templates (`{id}`, `{level}`, `{text}`), filled in by the `AgentDef` methods.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct AgentSet {
    pub model: Option<String>,
    pub effort: Option<String>,
    pub goal: Option<String>,
    pub clear_goal: Option<String>,
    pub compact: Option<String>,
    pub clear: Option<String>,
    pub rc: Option<String>,
    pub ultracode: Option<UltracodeCommands>,
}

/// Everything Inyon knows about one coding agent. `args` holds the sources of the TypeScript
/// regular expressions (no flags), and `set` the templates of its functions.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct AgentDef {
    pub kind: AgentKind,
    pub install: AgentInstall,
    /// where its newest version is published (its npm package); none means Inyon can't tell
    pub latest: Option<LatestSource>,
    /// the agent's own update command; npm installs are updated with npm instead
    pub update: Option<String>,
    pub login: String,
    /// a command that prints the version
    pub version: String,
    /// files that exist once the user has signed in (any of them)
    pub auth_files: Vec<String>,
    pub name: String,
    pub vendor: String,
    /// process names that identify the agent inside a pane
    pub comm: Vec<String>,
    /// argv patterns that identify it when the process name is too generic
    pub args: Vec<String>,
    pub launch: String,
    /// reply typography
    pub reply_font: ReplyFont,
    pub spinner: Vec<String>,
    pub skin: Skin,
    /// conversation readable as chat (otherwise terminal-first)
    pub chat: bool,
    /// The running CLI supports direct ! shell commands.
    pub shell: Option<bool>,
    pub models: Vec<AgentModel>,
    pub efforts: Vec<String>,
    pub modes: Vec<ModeOption>,
    pub commands: Vec<SlashCommand>,
    /// The commands that open the agent's own list of past conversations in its terminal.
    pub conversations: Option<Vec<String>>,
    pub config_files: Vec<ConfigFile>,
    pub set: AgentSet,
}

impl AgentDef {
    /// The command that picks model `id`. `effort` is the session's current level, for agents that
    /// name both in one id (Antigravity).
    pub fn model_command(&self, id: &str, effort: Option<&str>) -> Option<String> {
        let id = if self.kind == AgentKind::Antigravity { agy_model_id(id, effort) } else { id.to_owned() };
        self.set.model.as_ref().map(|t| t.replace("{id}", &id))
    }

    pub fn effort_command(&self, level: &str) -> Option<String> {
        self.set.effort.as_ref().map(|t| t.replace("{level}", level))
    }

    pub fn goal_command(&self, text: &str) -> Option<String> {
        self.set.goal.as_ref().map(|t| t.replace("{text}", text))
    }

    pub fn ultracode_command(&self, on: bool) -> Option<&str> {
        self.set.ultracode.as_ref().map(|u| if on { u.on.as_str() } else { u.off.as_str() })
    }
}

/// The catalog, in the TypeScript `AGENTS` order.
pub fn agents() -> &'static IndexMap<AgentKind, AgentDef> {
    static AGENTS: OnceLock<IndexMap<AgentKind, AgentDef>> = OnceLock::new();
    AGENTS.get_or_init(|| serde_json::from_str(include_str!("agents.json")).expect("agents.json matches AgentDef"))
}

/// agy names a model and its effort together ("gemini-3.8-flash-high"). A bare family keeps the
/// session's effort when the model has it (Pro has only low and high), else gets high, agy's default.
pub fn agy_model_id(id: &str, effort: Option<&str>) -> String {
    let Some(family) = gemini_family(id) else { return id.to_owned() };
    let ok: &[&str] = if family == "pro" { &["low", "high"] } else { &["low", "medium", "high"] };
    let level = effort.filter(|e| ok.contains(e)).unwrap_or("high");
    format!("{id}-{level}")
}

/// `gemini-<version>-flash` or `gemini-<version>-pro` (`^gemini-[0-9.]+-(flash|pro)$`): the
/// family, else None.
fn gemini_family(id: &str) -> Option<&'static str> {
    let rest = id.strip_prefix("gemini-")?;
    let (version, family) = if let Some(v) = rest.strip_suffix("-flash") { (v, "flash") } else { (rest.strip_suffix("-pro")?, "pro") };
    (!version.is_empty() && version.bytes().all(|b| b.is_ascii_digit() || b == b'.')).then_some(family)
}

/// Which agent (if any) a process is. `comm` alone is too generic, so it needs a matching argv.
/// Only the engine needs it (the `detect` feature), which keeps regex out of the app.
#[cfg(feature = "detect")]
pub fn detect_agent(comm: &str, args: &str) -> Option<AgentKind> {
    use regex::Regex;
    static PATTERNS: OnceLock<Vec<(AgentKind, Vec<Regex>)>> = OnceLock::new();
    let patterns = PATTERNS.get_or_init(|| agents().values().map(|a| (a.kind, a.args.iter().map(|s| Regex::new(s).expect("agent args pattern")).collect())).collect());
    let shell = matches!(comm, "sh" | "bash" | "zsh" | "-zsh" | "-bash" | "fish" | "tmux");
    for (kind, res) in patterns {
        if kind.def().comm.iter().any(|c| c == comm) {
            return Some(*kind);
        }
        if shell {
            continue;
        }
        if res.iter().any(|r| r.is_match(args)) {
            return Some(*kind);
        }
    }
    None
}
