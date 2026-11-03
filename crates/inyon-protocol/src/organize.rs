// How sessions are organised (from 0.24): pins, parking, snoozes, tags and notes on sessions,
// projects that gather sessions by folder, and what the hub does for them on its own: messages sent
// later, idle agents put to sleep, sessions closed once done, and saved prompts. The hub owns it, so every device (phone, browser,
// desktop app) shows the same order. The TypeScript side is in packages/protocol/src/index.ts.
//
// A session here is a pane, as the overview lists it, found again by the anchors its mark keeps
// (see [`SessionMark`]); the hub matches marks to live panes and the apps join them by `host` and
// `pane` while `live` is set.

use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use ts_rs::TS;

/// The version of the `organize` record the hub writes. A hub that finds a later one reads what it
/// knows and refuses to change it, so going back to an older hub can't lose what a newer one kept.
pub const ORGANIZE_VERSION: u32 = 2;

/// Marks kept at most; past it the one seen longest ago goes.
pub const MAX_MARKS: usize = 1000;
/// Tags on one session at most.
pub const MAX_TAGS: usize = 8;
/// A tag's length at most (UTF-16 units, as JavaScript counts them).
pub const TAG_LEN: usize = 32;
/// A session note's length at most.
pub const NOTE_LEN: usize = 500;
/// A title, a section or a name, at most.
pub const TITLE_LEN: usize = 80;
pub const SECTION_LEN: usize = 40;
pub const NAME_LEN: usize = 60;
/// An icon: an emoji or an icon's name.
pub const ICON_LEN: usize = 32;
pub const MAX_PROJECTS: usize = 100;
pub const MAX_ROOTS: usize = 8;
pub const PATH_LEN: usize = 1024;
pub const MAX_GROUPS: usize = 30;
pub const MAX_SPACES: usize = 20;
/// Messages waiting to be sent later, on all sessions together.
pub const MAX_SENDS: usize = 50;
/// A message sent later at most; they travel in every v1 overview, so they're kept short.
pub const SEND_LEN: usize = 4_000;
pub const MAX_PROMPTS: usize = 200;
/// A saved prompt at most.
pub const PROMPT_LEN: usize = 10_000;
/// An idle agent can be put to sleep after this many minutes at the least, and at the most.
pub const SLEEP_MIN: u32 = 5;
pub const SLEEP_MAX: u32 = 24 * 60;
/// The [`SleepRule::scope`] that covers every session no project rule does.
pub const SLEEP_ALL: &str = "all";

/// A project's or a group's colour: the neutral ones, and a few muted ones. The app draws it on the
/// project's mark and nowhere else.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum ProjectColor {
    #[default]
    Silver,
    Ink,
    Sage,
    Teal,
    Blue,
    Violet,
    Rose,
}

/// Every colour, in the order a picker shows them.
pub const PROJECT_COLORS: [ProjectColor; 7] =
    [ProjectColor::Silver, ProjectColor::Ink, ProjectColor::Sage, ProjectColor::Teal, ProjectColor::Blue, ProjectColor::Violet, ProjectColor::Rose];

/// What the hub keeps about one session, and how it finds the session again. A session is matched
/// by its agent's conversation id when one is known (that follows a conversation resumed in another
/// pane), else by its tmux pane id together with its tmux session, so a pane id that tmux handed
/// out again after a restart of its server isn't taken for the old pane, else by its place (tmux
/// session name, window and pane numbers, folder) for a session put back after such a restart.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct SessionMark {
    /// the hub's id for it: "s-" and 12 hex digits
    pub id: String,
    /// the machine
    pub host: String,
    /// the tmux pane it was last matched to ("%12")
    pub pane: String,
    /// matched to a pane on its machine now; the apps join a mark to a pane only while this is set
    #[serde(default)]
    pub live: bool,
    /// the agent's conversation (Claude Code's session id, Codex's thread id), when known
    pub conv: Option<String>,
    /// the tmux session's id ("$3") and name where it was last seen
    pub tmux_id: Option<String>,
    pub tmux_name: Option<String>,
    /// tmux's window and pane numbers there, "2.0"
    pub tmux_place: Option<String>,
    /// its folder when last seen
    pub cwd: Option<String>,
    /// when it was last seen on its machine, unix ms
    #[serde(default)]
    pub seen: u64,
    /// pinned, at this place among the pinned (0 first)
    pub pinned: Option<u32>,
    pub parked: Option<bool>,
    /// hidden until then, unix ms; the hub clears it once it's past
    pub snoozed_until: Option<u64>,
    /// raise a notice when the snooze ends
    pub snooze_notice: Option<bool>,
    /// normalised (see [`normalize_tag`]), at most [`MAX_TAGS`]
    pub tags: Option<Vec<String>>,
    pub note: Option<String>,
    /// shown in place of the session's own name
    pub title: Option<String>,
    /// a section of its own within its project ("Move to group")
    pub section: Option<String>,
    /// the project it belongs to, whatever its folder; else it falls into one by its folder
    pub project: Option<String>,
    /// end the session (as End session does) once its agent has finished its next piece of work
    pub close_when_done: Option<bool>,
    /// the hub saw it working since `close_when_done` was set, so the next idle ends it
    pub close_saw_work: Option<bool>,
}

impl SessionMark {
    /// Nothing is kept about it any more, so the mark can go.
    pub fn is_blank(&self) -> bool {
        self.pinned.is_none()
            && self.parked != Some(true)
            && self.snoozed_until.is_none()
            && self.tags.as_ref().is_none_or(Vec::is_empty)
            && self.note.is_none()
            && self.title.is_none()
            && self.section.is_none()
            && self.project.is_none()
            && self.close_when_done != Some(true)
    }
}

/// A folder on one machine whose sessions belong to a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ProjectRoot {
    pub host: String,
    /// absolute; a session in it or any folder under it belongs to the project
    pub path: String,
}

/// A project: a name, a colour and the folders its sessions live in, on any machine.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
pub struct Project {
    /// "p-" and 12 hex digits
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub color: ProjectColor,
    pub icon: Option<String>,
    #[serde(default)]
    pub roots: Vec<ProjectRoot>,
    /// the group it's in
    pub group: Option<String>,
}

/// Projects shown together under one heading.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
pub struct ProjectGroup {
    /// "g-" and 12 hex digits
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub color: ProjectColor,
}

/// A named set of projects to look at on their own (all the "inyon" work, on every machine).
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
pub struct Space {
    /// "w-" and 12 hex digits
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    #[serde(default)]
    pub projects: Vec<String>,
}

/// A message the hub types into a session at a set time, through the same queue as a held message:
/// once the time comes it waits for the agent to be free, then goes once. Found again like a mark:
/// by the conversation, else by the pane in the same tmux session.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(optional_fields)]
pub struct DelayedSend {
    /// "d-" and 12 hex digits
    pub id: String,
    pub host: String,
    pub pane: String,
    pub conv: Option<String>,
    pub tmux_id: Option<String>,
    pub text: String,
    /// when it goes, unix ms
    pub at: u64,
    /// why it couldn't go; it stays until dismissed
    pub error: Option<String>,
}

/// A prompt kept to send again: for every session, or for one project's.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
pub struct SavedPrompt {
    /// "q-" and 12 hex digits
    pub id: String,
    pub name: String,
    pub text: String,
    /// the project it's for; none for every session
    pub project: Option<String>,
    /// shown first
    pub starred: Option<bool>,
}

/// After how long idle an agent is put to sleep (exits to its shell, keeping the pane, as Exit to
/// shell does), for one project's sessions or, with scope [`SLEEP_ALL`], every other session. Off
/// unless there's a rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct SleepRule {
    /// a project's id, or "all"
    pub scope: String,
    pub minutes: u32,
}

/// GET /api/organize, the hub's `organize` record, and the `organize` of a v1 overview (without
/// `prompts`, which only GET and the `/ws2` stream carry). Lists are in the order the apps show
/// them.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
pub struct Organize {
    /// [`ORGANIZE_VERSION`] when the hub wrote it
    #[serde(default)]
    pub v: u32,
    #[serde(default)]
    pub sessions: Vec<SessionMark>,
    #[serde(default)]
    pub projects: Vec<Project>,
    #[serde(default)]
    pub groups: Vec<ProjectGroup>,
    #[serde(default)]
    pub spaces: Vec<Space>,
    /// from version 2
    pub sends: Option<Vec<DelayedSend>>,
    pub prompts: Option<Vec<SavedPrompt>>,
    pub sleep: Option<Vec<SleepRule>>,
}

/// JavaScript's `s.length`: what the limits count.
pub fn js_len(s: &str) -> usize {
    s.encode_utf16().count()
}

/// Runs of white space as one space, trimmed.
pub fn squash_spaces(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A tag as the hub keeps it: trimmed, without a leading `#`, spaces squashed, in lower case.
/// None when nothing is left, or it's too long, or it holds a control character (a line break
/// inside it among them, which squashing the spaces would otherwise hide).
pub fn normalize_tag(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.chars().any(char::is_control) {
        return None;
    }
    let t = squash_spaces(raw.trim_start_matches('#')).to_lowercase();
    (!t.is_empty() && js_len(&t) <= TAG_LEN).then_some(t)
}

/// Whether `path` is `root` or a folder under it. Both are absolute; a trailing slash on the root
/// doesn't count, and Windows paths compare with either slash and in any case.
pub fn path_in(path: &str, root: &str) -> bool {
    let windows = root.contains('\\') || root.as_bytes().get(1) == Some(&b':');
    let norm = |s: &str| {
        let s = if windows { s.replace('\\', "/").to_lowercase() } else { s.to_owned() };
        let t = s.trim_end_matches('/');
        if t.is_empty() {
            "/".to_owned()
        } else {
            t.to_owned()
        }
    };
    let (p, r) = (norm(path), norm(root));
    // `/` holds every Unix path, never a Windows one
    p == r || (r == "/" && p.starts_with('/')) || p.strip_prefix(&r).is_some_and(|rest| rest.starts_with('/'))
}

/// The project a session belongs to: the one it was put in, else the one whose folder on its
/// machine holds its folder most closely.
pub fn project_of<'a>(org: &'a Organize, host: &str, cwd: &str, mark: Option<&SessionMark>) -> Option<&'a Project> {
    if let Some(pid) = mark.and_then(|m| m.project.as_deref()) {
        if let Some(p) = org.projects.iter().find(|p| p.id == pid) {
            return Some(p);
        }
    }
    org.projects
        .iter()
        .filter_map(|p| p.roots.iter().filter(|r| r.host == host && path_in(cwd, &r.path)).map(|r| js_len(&r.path)).max().map(|len| (len, p)))
        .max_by_key(|(len, _)| *len)
        .map(|(_, p)| p)
}

/// After how many idle minutes a session's agent is put to sleep: its project's rule, else the one
/// for all sessions. None when it's off for it.
pub fn sleep_minutes(org: &Organize, project: Option<&Project>) -> Option<u32> {
    let rules = org.sleep.as_deref().unwrap_or_default();
    let rule = |scope: &str| rules.iter().find(|r| r.scope == scope).map(|r| r.minutes);
    project.and_then(|p| rule(&p.id)).or_else(|| rule(SLEEP_ALL))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tags_come_out_the_same_however_they_were_typed() {
        assert_eq!(normalize_tag("  #Needs   Review "), Some("needs review".into()));
        assert_eq!(normalize_tag("#"), None);
        assert_eq!(normalize_tag(&"x".repeat(TAG_LEN)), Some("x".repeat(TAG_LEN)));
        assert_eq!(normalize_tag(&"x".repeat(TAG_LEN + 1)), None);
        assert_eq!(normalize_tag("a\u{7}b"), None);
        assert_eq!(normalize_tag("first\nsecond"), None, "one line");
        assert_eq!(normalize_tag(" review\n"), Some("review".into()), "a line break around it is trimmed");
        assert_eq!(normalize_tag("Überprüfen"), Some("überprüfen".into()));
    }

    #[test]
    fn a_folder_is_in_a_root_only_on_a_path_boundary() {
        assert!(path_in("/home/a/inyon", "/home/a/inyon"));
        assert!(path_in("/home/a/inyon/apps/web", "/home/a/inyon/"));
        assert!(!path_in("/home/a/inyon-plans", "/home/a/inyon"));
        assert!(path_in("/anything", "/"));
        assert!(!path_in("C:\\code\\other", "/"), "a Windows folder isn't under /");
        assert!(path_in("c:/Users/A/Code/x", "C:\\Users\\a\\code"));
        assert!(!path_in("C:\\Users\\a\\codex", "C:\\Users\\a\\code"));
    }

    #[test]
    fn a_mark_with_nothing_on_it_is_blank() {
        let mut m: SessionMark = serde_json::from_value(serde_json::json!({ "id": "s-1", "host": "h", "pane": "%1" })).unwrap();
        assert!(m.is_blank());
        m.tags = Some(vec![]);
        m.parked = Some(false);
        assert!(m.is_blank());
        m.note = Some("x".into());
        assert!(!m.is_blank());
    }
}
