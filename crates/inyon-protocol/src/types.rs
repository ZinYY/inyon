// The types in packages/protocol/src/index.ts, field for field. The TypeScript file stays the one
// the Bun hub and the app compile against; `cargo test` writes these back out as TypeScript
// (src/gen/index.ts) and the protocol package's typecheck fails when the two disagree.

use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use ts_rs::TS;

use crate::agents::AgentKind;
use crate::js::{double_option, Num, True};

/// local: the hub itself · ssh: the hub connects out over SSH · node: the machine connects in to the hub (no open ports)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum HostKind {
    Local,
    Ssh,
    Node,
    Link,
}

/// How the phone app signs in to an SSH server ("none" for servers that let you in without a key or password).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum HostAuth {
    PhoneKey,
    Password,
    Key,
    None,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct HostConfig {
    pub id: String,
    pub name: String,
    pub kind: HostKind,
    /// ssh only: `user@host` or an alias from ~/.ssh/config
    pub target: Option<String>,
    pub port: Option<u16>,
    pub identity_file: Option<String>,
    /// ssh from the phone app: the server's host key fingerprint, confirmed when it was added
    pub host_key: Option<String>,
    /// ssh from the phone app: how it signs in (the secret itself stays in the phone's keystore)
    pub auth: Option<HostAuth>,
    /// ssh from the phone app: the id of a paired computer that opens the connection instead of the phone
    pub via: Option<String>,
    /// node only: sha256 of the pairing token (the token itself is never stored)
    pub token_hash: Option<String>,
    /// node only: when the machine first connected with its token (unset while the command hasn't run)
    pub paired_at: Option<u64>,
    /// node only: when its current token was made; one that's never used stops working after a day
    pub token_issued_at: Option<u64>,
    /// link only (a laptop the phone app paired with directly): its id, from `inyon pair`
    pub machine_id: Option<String>,
    /// link only: where it was last reachable, and the relays it uses
    pub urls: Option<Vec<String>>,
    pub relays: Option<Vec<String>>,
    /// put the Claude integration back when something replaces ~/.claude/settings.json
    pub keep_integration: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum HostStatus {
    Connecting,
    Online,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum AgentState {
    Idle,
    Working,
    Waiting,
    Unknown,
}

/// Working, or idle with shells, monitors or agents still running in the background: something is happening.
pub fn is_busy(state: AgentState, background: Option<&str>) -> bool {
    state == AgentState::Working || (state == AgentState::Idle && background.is_some_and(|b| !b.is_empty()))
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct RateWindow {
    pub pct: Num,
    /// unix seconds
    pub resets_at: Option<Num>,
}

/// Prepaid credits (Codex): a balance, or unlimited.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
pub struct Credits {
    pub balance: Option<String>,
    pub unlimited: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
pub enum StaleLimits {
    #[serde(rename = "account")]
    Account,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
pub enum StaleContext {
    #[serde(rename = "compacted")]
    Compacted,
}

/// Why some numbers are missing: the account changed since the limits were reported, or the
/// conversation was compacted since the context was. Both refill on the agent's next reply.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
pub struct Stale {
    pub limits: Option<StaleLimits>,
    pub context: Option<StaleContext>,
}

/// Live numbers the agent reports (Claude: its status line payload; Codex: token_count events).
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct AgentStatus {
    pub model: Option<String>,
    pub model_id: Option<String>,
    pub effort: Option<String>,
    pub fast: Option<bool>,
    pub version: Option<String>,
    pub context_pct: Option<Num>,
    pub context_used: Option<Num>,
    pub context_size: Option<Num>,
    pub cost_usd: Option<Num>,
    pub duration_ms: Option<Num>,
    pub api_duration_ms: Option<Num>,
    pub lines_added: Option<Num>,
    pub lines_removed: Option<Num>,
    pub five_hour: Option<RateWindow>,
    pub seven_day: Option<RateWindow>,
    pub credits: Option<Credits>,
    /// plan name when the agent reports it
    pub plan: Option<String>,
    pub output_style: Option<String>,
    pub stale: Option<Stale>,
    /// unix ms of the last update
    pub updated_at: u64,
}

/// Where the pane → session mapping came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum MappedBy {
    Hook,
    Cwd,
    Resume,
    Process,
}

/// An answer offered by the prompt on screen, for quick replies.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct QuickOption {
    pub n: u32,
    pub label: String,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct AgentPaneInfo {
    pub kind: AgentKind,
    /// a custom agent's name (Settings › Agents); `kind` is the agent it works like
    pub custom: Option<String>,
    pub pid: u32,
    pub session_id: Option<String>,
    pub transcript_path: Option<String>,
    /// where the pane → session mapping came from
    pub mapped_by: Option<MappedBy>,
    pub state: AgentState,
    /// e.g. "manual", "plan", "accept edits"
    pub mode: Option<String>,
    /// short text for the overview, e.g. the permission being asked
    pub waiting_for: Option<String>,
    /// answers offered by the prompt on screen (single-choice prompts only), for quick replies
    pub options: Option<Vec<QuickOption>>,
    /// working spinner text
    pub activity: Option<String>,
    /// background shells or tasks still running while the agent itself waits, e.g. "1 shell"
    pub background: Option<String>,
    /// Claude Code: ultracode (a dynamic workflow for every task) is on for the session
    pub ultracode: Option<bool>,
    pub status: Option<AgentStatus>,
    /// the integration is on but this session stopped sending live numbers
    pub status_stopped: Option<bool>,
    pub started_at: Option<u64>,
    /// the last turn ended on an error instead of finishing; gone with the next prompt
    pub failure: Option<TurnFailure>,
}

/// Why a turn stopped (Claude Code's StopFailure hook).
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct TurnFailure {
    /// "rate_limit", "billing_error", "authentication_failed", "overloaded", "server_error",
    /// "invalid_request", "max_output_tokens" or "unknown"
    pub error: String,
    /// rate limits: when the full limit resets, unix seconds
    pub resets_at: Option<u64>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
pub struct PaneInfo {
    /// tmux pane id, e.g. "%12"
    pub id: String,
    /// tmux's numbers as JavaScript reads them: NaN (null on the wire) where tmux printed no number
    pub index: Num,
    pub active: bool,
    pub command: String,
    pub cwd: String,
    pub title: String,
    /// the name you gave this session in Inyon
    pub label: Option<String>,
    pub width: Num,
    pub height: Num,
    pub dead: bool,
    pub agent: Option<AgentPaneInfo>,
    /// runs your companion agent (Settings › Companion); at most one pane across all machines
    pub companion: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct WindowInfo {
    /// "@3"
    pub id: String,
    pub index: Num,
    pub name: String,
    pub active: bool,
    pub activity: Num,
    pub panes: Vec<PaneInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct SessionInfo {
    /// "$1"
    pub id: String,
    pub name: String,
    pub attached: Num,
    pub windows: Vec<WindowInfo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum IntegrationState {
    Unknown,
    Installed,
    Missing,
    Removed,
    Outdated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum NodeUpdateState {
    Updating,
    Updated,
    Failed,
}

/// A machine's inyon that can be updated from the app, or an update the app started.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
pub struct NodeUpdate {
    pub latest: String,
    /// false for an inyon (or tether-node) too old to update itself
    pub supported: bool,
    pub state: Option<NodeUpdateState>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum MachineKind {
    Server,
    Laptop,
    Desktop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum TmuxState {
    Unknown,
    Ok,
    NoServer,
    Missing,
}

/// An agent's plan limits on a machine, read from the account it's signed in to, with or without a
/// session running.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct AccountLimits {
    pub kind: AgentKind,
    pub five_hour: Option<RateWindow>,
    pub seven_day: Option<RateWindow>,
    pub plan: Option<String>,
    pub credits: Option<Credits>,
    /// limit resets the account has banked (Codex): counted, never used
    pub resets: Option<u32>,
    /// unix ms of the reading
    pub updated_at: u64,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct HostState {
    pub config: HostConfig,
    pub status: HostStatus,
    pub error: Option<String>,
    /// hostname reported by the machine
    pub hostname: Option<String>,
    /// Claude Code integration
    pub integration: IntegrationState,
    /// Antigravity has its own settings and status wrapper.
    pub agy_integration: Option<IntegrationState>,
    pub tmux: TmuxState,
    /// agent CLIs found on PATH
    pub agents: Vec<AgentKind>,
    /// agents with a saved sign-in on the machine (unknown until the first probe)
    pub signed_in: Option<Vec<AgentKind>>,
    pub os: Option<String>,
    /// what the machine is, for its icon (detected on the machine)
    pub machine_kind: Option<MachineKind>,
    /// the inyon version a paired machine or laptop runs
    pub node_version: Option<String>,
    /// what that machine's inyon warns about its connection to the hub (plain http, from a machine
    /// added before 0.20), with how to fix it
    pub node_warning: Option<String>,
    /// set when that inyon is behind, or while an update runs
    pub node_update: Option<NodeUpdate>,
    pub latency_ms: Option<u64>,
    pub sessions: Vec<SessionInfo>,
    pub updated_at: u64,
    /// last successful check (ms); kept across hub restarts
    pub last_seen: Option<u64>,
    /// when the machine stopped answering; its sessions are the last ones seen, saved on disk
    pub offline_since: Option<u64>,
    /// plan limits of the accounts signed in there
    pub limits: Option<Vec<AccountLimits>>,
}

// ---------- Chat (normalized Claude Code transcript) ----------

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantBlock {
    Text {
        text: String,
    },
    Thinking {
        text: String,
    },
    ToolUse {
        id: String,
        name: String,
        #[ts(type = "any")]
        input: serde_json::Value,
    },
}

/// One unified hunk of an Edit/Write/MultiEdit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PatchHunk {
    pub old_start: Num,
    pub old_lines: Num,
    pub new_start: Num,
    pub new_lines: Num,
    pub lines: Vec<String>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct ToolResultMeta {
    /// Edit/Write/MultiEdit: unified hunks
    pub patch: Option<Vec<PatchHunk>>,
    pub file_path: Option<String>,
    /// Write: created a new file
    pub created: Option<bool>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
    pub interrupted: Option<bool>,
    /// AskUserQuestion
    pub answers: Option<StringMap>,
    /// ExitPlanMode
    pub plan: Option<String>,
    /// Read
    pub num_lines: Option<u64>,
    /// Bash/Monitor run in the background: the task it started (see ChatInfo.tasks)
    pub task_id: Option<String>,
}

/// `Record<string, string>`, in the order the keys came.
pub type StringMap = indexmap::IndexMap<String, String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum NoticeLevel {
    Info,
    Warn,
    Error,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case", rename_all_fields = "camelCase")]
#[ts(optional_fields)]
pub enum ChatItem {
    User { seq: i64, uuid: String, ts: String, text: String, images: u32, queued: Option<bool>, shell: Option<bool> },
    Assistant { seq: i64, uuid: String, ts: String, msg_id: String, model: Option<String>, block: AssistantBlock },
    ToolResult { seq: i64, uuid: String, ts: String, tool_use_id: String, is_error: bool, text: String, truncated: Option<bool>, meta: Option<ToolResultMeta> },
    Command { seq: i64, uuid: String, ts: String, name: String, args: String, output: Option<String> },
    Notice { seq: i64, uuid: String, ts: String, level: NoticeLevel, subtype: String, text: String },
    TurnEnd { seq: i64, uuid: String, ts: String, duration_ms: Num },
    Goal { seq: i64, uuid: String, ts: String, condition: String, met: bool, reason: Option<String>, cleared: Option<bool> },
}

impl ChatItem {
    /// The item's place in the conversation, shared by every kind.
    pub fn seq(&self) -> i64 {
        match self {
            ChatItem::User { seq, .. }
            | ChatItem::Assistant { seq, .. }
            | ChatItem::ToolResult { seq, .. }
            | ChatItem::Command { seq, .. }
            | ChatItem::Notice { seq, .. }
            | ChatItem::TurnEnd { seq, .. }
            | ChatItem::Goal { seq, .. } => *seq,
        }
    }

    pub fn uuid(&self) -> &str {
        match self {
            ChatItem::User { uuid, .. }
            | ChatItem::Assistant { uuid, .. }
            | ChatItem::ToolResult { uuid, .. }
            | ChatItem::Command { uuid, .. }
            | ChatItem::Notice { uuid, .. }
            | ChatItem::TurnEnd { uuid, .. }
            | ChatItem::Goal { uuid, .. } => uuid,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum TaskType {
    Shell,
    Monitor,
    Workflow,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    Running,
    Completed,
    Failed,
    Stopped,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct BackgroundTask {
    pub id: String,
    pub r#type: TaskType,
    pub description: String,
    pub command: Option<String>,
    pub status: TaskStatus,
    pub output_file: Option<String>,
    pub summary: Option<String>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum SubagentStatus {
    Running,
    Done,
    Error,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct SubagentInfo {
    pub tool_use_id: String,
    pub r#type: Option<String>,
    pub description: String,
    pub prompt: Option<String>,
    pub status: SubagentStatus,
    pub background: Option<bool>,
    pub started_at: Option<String>,
    pub transcript_path: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct ArtifactInfo {
    pub url: String,
    pub title: Option<String>,
    pub updated_at: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct SessionStats {
    pub context_tokens: Option<Num>,
    pub cost_usd: Option<Num>,
    pub duration_ms: Option<Num>,
    pub api_duration_ms: Option<Num>,
    pub lines_added: Option<Num>,
    pub lines_removed: Option<Num>,
    pub turns: u32,
    pub compactions: u32,
    /// unix ms of the last compaction
    pub compacted_at: Option<u64>,
    /// unix ms of the event `contextTokens` came from
    pub context_at: Option<u64>,
    pub started_at: Option<String>,
}

/// sending: being typed · sent: submitted, not yet in the agent's record · queued: in the agent's
/// queue while it works · delivered: in the conversation (`uuid` is that item) · failed: `error`
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum SendStatus {
    Sending,
    Sent,
    Queued,
    Delivered,
    Failed,
}

/// One message sent through Inyon. The hub knows the exact text it typed, so it is the one that
/// matches it to the agent's record; every device reads the outcome from here.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
pub struct SendState {
    pub id: String,
    pub text: String,
    pub at: u64,
    pub state: SendStatus,
    pub uuid: Option<String>,
    pub error: Option<String>,
    /// a change made with the app's controls (model, effort, goal…): followed, but not shown as a message
    pub setting: Option<bool>,
}

/// A message Inyon holds for a pane and types in once its agent is free (from 0.22): it can still
/// be edited, moved or dropped. `error` says why the last try didn't go, and holding stops there.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
pub struct HeldPrompt {
    pub id: String,
    pub text: String,
    pub at: u64,
    pub error: Option<String>,
}

/// Controls picked in the app while the agent couldn't take them (from 0.22): Inyon makes each
/// pick once the agent is free, the last of each kind winning. The values are what was picked:
/// a model as the agent's `/model` takes it, an effort level, a mode id. `error` says why the
/// last try didn't take, and nothing more is tried until it's picked again or dropped.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
pub struct ChosenControls {
    pub model: Option<String>,
    pub effort: Option<String>,
    pub mode: Option<String>,
    pub error: Option<String>,
}

/// A conversation the agent saved in a pane's folder, which it can be started again with.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
pub struct PastConversation {
    /// what the agent's command line takes to resume it
    pub id: String,
    /// the agent's own title for it, a name given in Inyon, or else the first thing you asked
    pub title: String,
    /// last activity, ms since the epoch
    pub at: u64,
    /// the conversation open in the pane now
    pub current: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum LiveKind {
    Text,
    Thinking,
    Plan,
    Output,
}

/// Something the agent is writing right now, before it's in its record. The record's item has the
/// same id, so the finished message takes the live one's place without a jump.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct LiveItem {
    pub id: String,
    pub kind: LiveKind,
    pub text: String,
    pub turn_id: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
pub struct ChatGoal {
    pub condition: String,
    pub met: bool,
    pub reason: Option<String>,
    pub ts: String,
}

/// Remote Control's link while it's on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct RemoteControl {
    pub url: String,
    pub ts: String,
}

/// The turn under way, for the working timer.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct Turn {
    pub id: Option<String>,
    /// unix ms, fractional when the agent wrote it so
    pub started_at: Num,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct ChatInfo {
    /// Live metrics from transcript/API adapters.
    pub status: Option<AgentStatus>,
    pub host_id: String,
    pub pane_id: String,
    pub agent: Option<AgentKind>,
    pub title: Option<String>,
    pub session_id: Option<String>,
    pub transcript_path: Option<String>,
    pub cwd: Option<String>,
    pub model: Option<String>,
    pub git_branch: Option<String>,
    pub version: Option<String>,
    /// the agent process is (still) running in the pane
    pub running: bool,
    pub mapped_by: Option<MappedBy>,
    pub error: Option<String>,
    pub goal: Option<ChatGoal>,
    pub rc: Option<RemoteControl>,
    /// the conversation's public link, while it's shared (OpenCode's /share)
    pub share: Option<String>,
    /// messages typed while the agent was busy, not yet picked up
    pub queue: Vec<String>,
    /// messages Inyon holds until the agent is free, in the order they go (from 0.22)
    pub held: Option<Vec<HeldPrompt>>,
    /// a model, effort or mode waiting for the agent to be free (from 0.22)
    pub chosen: Option<ChosenControls>,
    /// the turn under way, for the working timer (Codex: from its records and its live events)
    pub turn: Option<Turn>,
    /// recent sends and how far each got, matched by the hub against the agent's own record
    pub sends: Option<Vec<SendState>>,
    pub tasks: Vec<BackgroundTask>,
    pub subagents: Vec<SubagentInfo>,
    pub artifacts: Vec<ArtifactInfo>,
    pub stats: SessionStats,
    pub mode: Option<String>,
    /// Claude Code: ultracode is on for the session
    pub ultracode: Option<bool>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
pub struct PromptOption {
    pub n: u64,
    pub label: String,
    pub detail: Option<String>,
    pub selected: bool,
    /// multi-select checkbox state; undefined for single-select
    pub checked: Option<bool>,
    /// what's set now, in a settings picker (Codex marks it "(current)")
    pub current: Option<bool>,
}

/// A tab of AskUserQuestion's tab strip.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PromptTab {
    pub label: String,
    pub done: bool,
    pub current: bool,
}

/// The dialog asks for typed input (e.g. "Paste code here if prompted >").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PromptInput {
    pub label: String,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct ScreenPrompt {
    /// text above the options (title, command, question)
    pub body: Vec<String>,
    /// each body line is a whole line: the hub has joined the ones the terminal wrapped
    pub whole_lines: Option<bool>,
    pub options: Vec<PromptOption>,
    /// AskUserQuestion tab strip
    pub tabs: Option<Vec<PromptTab>>,
    pub footer: Option<String>,
    pub multi: bool,
    /// the option number of a free-text "Type something" entry
    pub free_text_option: Option<u64>,
    /// options aren't numbered on screen: answer by moving the cursor (↑/↓) and pressing Enter
    pub navigate: Option<bool>,
    /// the dialog asks for typed input (e.g. "Paste code here if prompted >")
    pub input: Option<PromptInput>,
    /// URLs shown in the dialog, re-joined when the terminal wrapped them
    pub links: Option<Vec<String>>,
    /// answered through the agent's API rather than keystrokes (OpenCode permissions and questions)
    pub api: Option<bool>,
    /// the label of the action that sends toggled choices together ("Confirm", "Next"); answered with option 0
    pub confirm: Option<String>,
}

/// Option number that stands for a prompt's `confirm` action.
pub const CONFIRM_OPTION: u32 = 0;

/// Which question a prompt is: its text and its choices, whitespace aside (the screen rewraps). An
/// answer carries the key of the question it was given for.
pub fn prompt_key(body: &[String], options: &[PromptOption]) -> String {
    let lines = body.iter().cloned().chain(options.iter().map(|o| format!("{}.{}", o.n, o.label)));
    // JavaScript's \s: Unicode white space plus the BOM, as String.prototype.replace(/\s+/g, '') removes it
    lines.collect::<Vec<_>>().join("\n").chars().filter(|c| !c.is_whitespace() && *c != '\u{feff}').collect()
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct ScreenState {
    /// A recognizable input box is present, even with a custom footer.
    pub input_visible: Option<bool>,
    /// a menu/prompt is waiting for an answer
    pub prompt: Option<ScreenPrompt>,
    pub working: bool,
    /// spinner line, e.g. "Undulating… (12s · thinking)"
    pub activity: Option<String>,
    /// when the current turn started (unix ms), read off the spinner's elapsed time (Codex)
    pub since: Option<u64>,
    pub mode: Option<String>,
    /// text sitting in Claude's input box (typed but not sent)
    pub draft: Option<String>,
    /// Remote Control as the screen last reported it: its link while on, null once turned off (agy)
    #[serde(default, deserialize_with = "double_option")]
    pub remote: Option<Option<String>>,
    /// background work listed under the input box: null when the footer lists none, left out when it wasn't visible
    #[serde(default, deserialize_with = "double_option")]
    pub background: Option<Option<String>>,
    /// messages waiting to be sent after the current turn, when only the screen shows them (Antigravity)
    pub queue: Option<Vec<String>>,
    /// questions Codex asked while it works, waiting to be answered (it opens them with shift+←)
    pub questions: Option<u32>,
    /// model and effort from the agent's own footer, for sessions that don't report live numbers
    pub model: Option<String>,
    pub effort: Option<String>,
    /// Claude Code's ultracode switch, as its input box shows it; undefined while the box isn't visible
    pub ultracode: Option<bool>,
    /// context and cost as the agent's own status row or sidebar prints them (OpenCode)
    pub context_pct: Option<Num>,
    pub context_used: Option<Num>,
    pub cost_usd: Option<Num>,
    /// a menu of the agent's own open over the box, by its title ("Select model", "Select variant")
    pub dialog: Option<String>,
    pub width: u32,
}

// ---------- OpenCode providers ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum ProviderVia {
    Credential,
    Env,
}

/// A model provider OpenCode knows, and whether this machine can use it.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
pub struct OpenCodeProvider {
    pub id: String,
    pub name: String,
    pub connected: bool,
    /// a saved credential (`opencode auth login`), or an API key in the machine's environment
    pub via: Option<ProviderVia>,
    /// the credential's kind, as `opencode auth list` prints it (api, oauth)
    pub r#type: Option<String>,
    /// environment variables that connect it without a saved credential
    pub env: Option<Vec<String>>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct ProvidersReport {
    pub providers: Vec<OpenCodeProvider>,
    pub updated_at: u64,
    /// the provider list couldn't be read (OpenCode missing, or its server didn't start)
    pub error: Option<String>,
}

// ---------- Phones paired with a hub ----------

/// A one-time code the hub's web app shows (as a QR, and as text to type) for a phone to pair with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct PhonePairCode {
    /// 8 characters of Crockford base32, shown as "K7Q4-MX2T"
    pub id: String,
    /// 6 digits
    pub code: String,
    /// what the QR holds: inyon://hub?u=<hub origin>&c=<id:code>&n=<hub name>
    pub link: String,
    /// unix ms; ten minutes after the code was made
    pub expires_at: u64,
}

/// POST /api/phones/claim: the phone hands in the code and says what to call it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PhoneClaim {
    /// "ID:DIGITS", the id in any case
    pub code: String,
    pub name: String,
}

/// The hub a phone paired with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct HubInfo {
    pub id: String,
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PhoneClaimResult {
    /// sent as `Authorization: Bearer` from then on; the hub keeps only its hash
    pub token: String,
    pub hub: HubInfo,
}

/// GET /api/phones: a phone that paired, as the hub lists it.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct PairedPhone {
    pub id: String,
    pub name: String,
    pub paired_at: u64,
    pub last_seen: u64,
    /// set on the phone that asked
    pub this_phone: Option<True>,
}

// ---------- Companion apps (Legato and the like), contract version 1 ----------

/// What a companion may do: read the overview, watch chats, answer prompts, send text. Nothing else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum CompanionScope {
    Overview,
    Chat,
    Answer,
    Send,
}

/// A short one-time code for a companion to type: six characters, no 0, O, 1, I or L.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct CompanionPairCode {
    pub code: String,
    /// unix ms; five minutes after the code was made
    pub expires_at: u64,
    /// from 1.1: the SHA-256 of the certificate the hub serves itself (its DER, lowercase hex), for the pairing link
    pub tls_fingerprint: Option<String>,
}

/// POST /api/companions/claim, sent with `X-Inyon-Contract`.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
pub struct CompanionClaim {
    /// any case, spaces and dashes ignored
    pub code: String,
    /// what the hub lists it as, e.g. "Legato on abhash-laptop"
    pub name: String,
    /// the app itself, e.g. "Legato for Windows"
    pub app: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct CompanionClaimResult {
    /// sent as `Authorization: Bearer` from then on; the hub keeps only its hash
    pub token: String,
    pub hub_id: String,
    pub scopes: Vec<CompanionScope>,
    pub contract_version: u32,
    /// from 1.1, when the hub serves HTTPS itself: the SHA-256 of its certificate's DER, lowercase hex, to pin
    pub tls_fingerprint: Option<String>,
}

/// GET /api/companions: a companion app that paired, as the hub lists it.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct PairedCompanion {
    pub id: String,
    pub name: String,
    pub app: Option<String>,
    pub scopes: Vec<CompanionScope>,
    pub contract_version: u32,
    pub paired_at: u64,
    pub last_seen: u64,
}

/// GET /api/companions/me: the companion asking, and the hub it paired with.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct CompanionMe {
    pub id: String,
    pub name: String,
    pub app: Option<String>,
    pub scopes: Vec<CompanionScope>,
    pub contract_version: u32,
    pub hub: HubInfo,
}

// ---------- Pages: sites an agent made, served by the hub under a token URL ----------

/// What a page's progress.json says, counted: tasks done of all tasks, and its `now` and `updated` lines.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
pub struct PageProgress {
    pub done: u32,
    pub total: u32,
    /// the "working on" line
    pub now: Option<String>,
    /// when progress.json was last written, as it says (an ISO time)
    pub updated: Option<String>,
}

/// GET /api/pages: one page, on one machine. Its id is `<host>/<slug>`.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
pub struct PageEntry {
    /// the machine's host id ("local" for the hub's own machine)
    pub host: String,
    /// `^[a-z0-9][a-z0-9-]{0,62}$`, unique on its machine
    pub slug: String,
    pub name: String,
    /// the project folder: the git top level of the page's folder, or the folder itself
    pub project: String,
    /// who added it: an agent ("claude", "codex"…), "cli" for the inyon command, or "app"
    pub by: String,
    /// unix ms
    pub added: u64,
    /// the site's folder, absolute; it holds index.html
    pub folder: String,
    /// left out when the site has no readable progress.json
    pub progress: Option<PageProgress>,
    /// the Markdown notes in the site's `research/` folder, by file name (from 0.22.4); left out
    /// when there are none
    pub notes: Option<Vec<String>>,
    /// the agent conversation that added or found it (Claude Code's session id, Codex's thread
    /// id), from 0.22.5: the app shows that session's name over it
    pub session: Option<String>,
    /// the name Inyon last gave that session, for when it isn't running
    #[serde(rename = "sessionName")]
    pub session_name: Option<String>,
}

/// GET /api/pages
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PageList {
    pub pages: Vec<PageEntry>,
}

/// POST /api/pages/:host/:slug/open. `url` is a path on the hub that called (`/v/<token>/`); the app
/// puts the hub's own address in front of it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PageOpen {
    pub url: String,
    /// unix ms; twelve hours after it was made
    pub expires: u64,
    /// how long the link lasts from when it was made, in ms; the app counts it on its own clock, which may not agree with the hub's
    pub ttl: u64,
}

/// POST /api/pages/:host: add a folder as a page. The slug comes from the folder's name when left out.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
pub struct PageRegister {
    /// absolute, and holds index.html
    pub path: String,
    pub name: Option<String>,
    pub slug: Option<String>,
}

/// One entry of ~/.inyon/pages.json, the machine's list of pages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PageRecord {
    pub slug: String,
    pub name: String,
    pub path: String,
    pub project: String,
    pub by: String,
    /// unix ms
    pub added: u64,
}

/// ~/.inyon/pages.json. Written to a temp file and renamed, under pages.json.lock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PagesFile {
    /// always 1
    pub v: u32,
    pub pages: Vec<PageRecord>,
}

/// What made a page's site; only Blueprint so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum PageKind {
    Blueprint,
}

/// `<site>/.inyon-page.json`, the marker in a site's own folder. It is a dot file, so never served.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PageMarker {
    /// always 1
    pub v: u32,
    pub slug: String,
    pub name: String,
    pub kind: PageKind,
    /// unix ms
    pub created: u64,
}

// ---------- WebSocket protocol ----------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "t")]
pub enum ClientMsg {
    #[serde(rename = "sub.overview")]
    SubOverview,
    #[serde(rename = "sub.chat")]
    SubChat { host: String, pane: String },
    #[serde(rename = "unsub.chat")]
    UnsubChat { host: String, pane: String },
    #[serde(rename = "chat.more")]
    ChatMore { host: String, pane: String, before: i64 },
    #[serde(rename = "draft")]
    Draft { host: String, pane: String, text: String },
    #[serde(rename = "term.open")]
    TermOpen { id: String, host: String, pane: String, cols: u16, rows: u16 },
    #[serde(rename = "term.in")]
    TermIn { id: String, d: String },
    #[serde(rename = "term.resize")]
    TermResize { id: String, cols: u16, rows: u16 },
    #[serde(rename = "term.close")]
    TermClose { id: String },
    #[serde(rename = "ping")]
    Ping,
    /// the app went to the background (true) or came back: overviews only when an agent's state
    /// changes while it's quiet. A hub before 0.23.1 ignores it.
    #[serde(rename = "quiet")]
    Quiet { on: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum DraftSource {
    App,
    Terminal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum NotifyKind {
    Waiting,
    Done,
    Offline,
    Online,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "t", rename_all_fields = "camelCase")]
#[ts(optional_fields)]
pub enum ServerMsg {
    /// `organize` (from 0.24): how the sessions are organised, without saved prompts; an older
    /// hub leaves it out
    #[serde(rename = "overview")]
    Overview { hosts: Vec<HostState>, organize: Option<Box<crate::Organize>> },
    #[serde(rename = "chat.reset")]
    ChatReset { key: String, info: Box<ChatInfo>, items: Vec<ChatItem>, has_more: bool },
    #[serde(rename = "chat.append")]
    ChatAppend { key: String, items: Vec<ChatItem> },
    #[serde(rename = "chat.more")]
    ChatMore { key: String, items: Vec<ChatItem>, has_more: bool },
    #[serde(rename = "chat.info")]
    ChatInfo { key: String, info: Box<ChatInfo> },
    #[serde(rename = "chat.screen")]
    ChatScreen { key: String, screen: Box<ScreenState> },
    /// what the agent is writing right now (the whole list each time; empty when nothing is)
    #[serde(rename = "chat.live")]
    ChatLive { key: String, items: Vec<LiveItem> },
    /// reading what was said while nobody was watching (bytes of the transcript read so far, of how many)
    #[serde(rename = "chat.progress")]
    ChatProgress { key: String, done: u64, total: u64 },
    /// the unsent message for a pane, shared by the app (every device) and the terminal's input box
    #[serde(rename = "chat.draft")]
    ChatDraft { key: String, text: String, source: DraftSource, at: u64 },
    #[serde(rename = "term.data")]
    TermData { id: String, d: String },
    #[serde(rename = "term.exit")]
    TermExit { id: String, reason: Option<String> },
    #[serde(rename = "notify")]
    Notify { host: String, pane: String, kind: NotifyKind, title: String, body: String, agent: Option<AgentKind> },
    /// A page's progress board moved on: a task on it is done (from 0.22.4). `title` is the
    /// page's name; `body` says how far it is, as "8 of 12 done · Payment sheet".
    #[serde(rename = "page.notify")]
    PageNotify { host: String, slug: String, title: String, body: String },
    #[serde(rename = "pong")]
    Pong,
    /// The companion agent finished a turn. `id` is unique per reply, so a reconnect never shows
    /// one twice; `text` is plain (markdown taken out), up to 2000 characters; `error` marks a turn
    /// that ended on an error, which `text` then names. `at` is unix ms.
    #[serde(rename = "companion.reply")]
    CompanionReply { id: String, host: String, pane: String, text: String, error: Option<bool>, at: u64 },
}

/// The key a chat's messages carry: `host:pane`.
pub fn chat_key(host: &str, pane: &str) -> String {
    format!("{host}:{pane}")
}
