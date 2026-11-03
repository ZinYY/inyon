//! How the hub says sessions are organised (from 0.24), as the session list shows it: pinned
//! sessions first in their order, then the rest, with snoozed and parked ones in sections of their
//! own; projects found by folder; tags to filter by; and the times a snooze menu offers.
//!
//! The hub owns the marks and matches them to panes; here a mark counts for a pane only while it
//! is `live` and names the same machine and pane.

use std::collections::HashMap;

use inyon_protocol::{project_of, sleep_minutes, DelayedSend, Organize, Project, ProjectColor, ProjectGroup, SavedPrompt, SessionMark};

use crate::overview::{pane_title, PaneRef};

/// A colour's swatch on the light and the dark theme. Muted on purpose: a colour sits on a
/// project's mark and nowhere else.
pub fn color_hex(c: ProjectColor, dark: bool) -> &'static str {
    match (c, dark) {
        (ProjectColor::Silver, false) => "#bdbcb5",
        (ProjectColor::Silver, true) => "#6e6c66",
        (ProjectColor::Ink, false) => "#1d1c1a",
        (ProjectColor::Ink, true) => "#f1f0ec",
        (ProjectColor::Sage, false) => "#7f927b",
        (ProjectColor::Sage, true) => "#93a68f",
        (ProjectColor::Teal, false) => "#6b8e8c",
        (ProjectColor::Teal, true) => "#82a6a3",
        (ProjectColor::Blue, false) => "#71839c",
        (ProjectColor::Blue, true) => "#8a9bb3",
        (ProjectColor::Violet, false) => "#857a9b",
        (ProjectColor::Violet, true) => "#9d92b3",
        (ProjectColor::Rose, false) => "#a07c84",
        (ProjectColor::Rose, true) => "#b5939a",
    }
}

/// The mark the hub keeps for a pane, if any.
pub fn mark_for<'a>(org: &'a Organize, host: &str, pane: &str) -> Option<&'a SessionMark> {
    org.sessions.iter().find(|m| m.live && m.host == host && m.pane == pane)
}

/// Still snoozed at `now`. A snooze that's over counts as over before the hub has said so.
pub fn snoozed(m: &SessionMark, now: u64) -> bool {
    m.snoozed_until.is_some_and(|t| t > now)
}

/// Where a session goes in the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Place {
    Pinned,
    Sessions,
    Snoozed,
    Parked,
}

/// A snooze hides a session whatever else is set on it; parked comes next; then pinned.
pub fn place_of(m: Option<&SessionMark>, now: u64) -> Place {
    match m {
        Some(m) if snoozed(m, now) => Place::Snoozed,
        Some(m) if m.parked == Some(true) => Place::Parked,
        Some(m) if m.pinned.is_some() => Place::Pinned,
        _ => Place::Sessions,
    }
}

/// The session list in its sections.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Sorted<'a> {
    /// in the order they were pinned in
    pub pinned: Vec<PaneRef<'a>>,
    /// in the order they came
    pub sessions: Vec<PaneRef<'a>>,
    /// the one that wakes first first
    pub snoozed: Vec<PaneRef<'a>>,
    /// in the order they came
    pub parked: Vec<PaneRef<'a>>,
}

impl<'a> Sorted<'a> {
    /// Hidden from the main list: snoozed and parked.
    pub fn folded(&self) -> usize {
        self.snoozed.len() + self.parked.len()
    }
}

/// Puts panes (in the order the list shows them) into their sections.
pub fn sort_panes<'a>(panes: Vec<PaneRef<'a>>, org: &Organize, now: u64) -> Sorted<'a> {
    let mut out = Sorted::default();
    let mut pinned: Vec<(u32, PaneRef<'a>)> = vec![];
    let mut snoozed: Vec<(u64, PaneRef<'a>)> = vec![];
    for r in panes {
        let m = mark_for(org, &r.host.config.id, &r.pane.id);
        match place_of(m, now) {
            Place::Pinned => pinned.push((m.and_then(|m| m.pinned).unwrap_or(0), r)),
            Place::Sessions => out.sessions.push(r),
            Place::Snoozed => snoozed.push((m.and_then(|m| m.snoozed_until).unwrap_or(0), r)),
            Place::Parked => out.parked.push(r),
        }
    }
    pinned.sort_by_key(|(n, _)| *n);
    snoozed.sort_by_key(|(t, _)| *t);
    out.pinned = pinned.into_iter().map(|(_, r)| r).collect();
    out.snoozed = snoozed.into_iter().map(|(_, r)| r).collect();
    out
}

/// What a session is called: its title in Inyon, else what the pane says.
pub fn shown_title(r: &PaneRef, org: &Organize) -> String {
    mark_for(org, &r.host.config.id, &r.pane.id).and_then(|m| m.title.clone()).unwrap_or_else(|| pane_title(r))
}

/// The project a pane belongs to.
pub fn pane_project<'a>(r: &PaneRef, org: &'a Organize) -> Option<&'a Project> {
    project_of(org, &r.host.config.id, &r.pane.cwd, mark_for(org, &r.host.config.id, &r.pane.id))
}

/// Panes by project, in the projects' order, with those in none last (under `None`). Projects with
/// no session here are left out. Within a project, panes keep their order.
pub fn by_project<'a, 'o>(panes: &[PaneRef<'a>], org: &'o Organize) -> Vec<(Option<&'o Project>, Vec<PaneRef<'a>>)> {
    let mut at: HashMap<Option<&str>, Vec<PaneRef<'a>>> = HashMap::new();
    for r in panes {
        let p = pane_project(r, org).map(|p| p.id.as_str());
        at.entry(p).or_default().push(*r);
    }
    let mut out: Vec<(Option<&'o Project>, Vec<PaneRef<'a>>)> = org.projects.iter().filter_map(|p| at.remove(&Some(p.id.as_str())).map(|v| (Some(p), v))).collect();
    if let Some(rest) = at.remove(&None) {
        out.push((None, rest));
    }
    out
}

/// A project's group, if it's in one.
pub fn group_of<'o>(p: &Project, org: &'o Organize) -> Option<&'o ProjectGroup> {
    p.group.as_deref().and_then(|g| org.groups.iter().find(|x| x.id == g))
}

/// Whether a pane belongs to a project of the space.
pub fn in_space(r: &PaneRef, org: &Organize, space: &str) -> bool {
    let Some(s) = org.spaces.iter().find(|s| s.id == space) else { return false };
    pane_project(r, org).is_some_and(|p| s.projects.contains(&p.id))
}

/// Whether a session carries every one of `tags` (already normalised). No tags asked: yes.
pub fn has_tags(m: Option<&SessionMark>, tags: &[String]) -> bool {
    tags.iter().all(|t| m.and_then(|m| m.tags.as_ref()).is_some_and(|have| have.contains(t)))
}

/// Every tag in use on a live session, the most used first (then by name), for a filter.
pub fn tags_in_use(org: &Organize) -> Vec<String> {
    let mut n: HashMap<&str, usize> = HashMap::new();
    for t in org.sessions.iter().filter(|m| m.live).flat_map(|m| m.tags.iter().flatten()) {
        *n.entry(t.as_str()).or_default() += 1;
    }
    let mut v: Vec<(&str, usize)> = n.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    v.into_iter().map(|(t, _)| t.to_owned()).collect()
}

/// When the next snooze ends, for a timer that redraws the list then.
pub fn next_wake(org: &Organize, now: u64) -> Option<u64> {
    org.sessions.iter().filter_map(|m| m.snoozed_until).filter(|t| *t > now).min()
}

/// What the snooze menu offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnoozeFor {
    Hour,
    ThreeHours,
    /// tomorrow at 9:00
    Tomorrow,
    /// next Monday at 9:00
    NextWeek,
}

const HOUR: u64 = 3_600_000;
const DAY: u64 = 24 * HOUR;

/// When a snooze picked at `now` ends. `offset_min` is the device's distance from UTC in minutes
/// (east is positive: `-new Date().getTimezoneOffset()`), since "tomorrow" is the device's.
pub fn snooze_until(p: SnoozeFor, now: u64, offset_min: i32) -> u64 {
    let off = i64::from(offset_min) * 60_000;
    let local = now as i64 + off;
    // local midnight today, as unix ms
    let midnight = local.div_euclid(DAY as i64) * DAY as i64 - off;
    let nine = |days: i64| (midnight + days * DAY as i64 + 9 * HOUR as i64) as u64;
    match p {
        SnoozeFor::Hour => now + HOUR,
        SnoozeFor::ThreeHours => now + 3 * HOUR,
        SnoozeFor::Tomorrow => nine(1),
        SnoozeFor::NextWeek => {
            // 1970-01-01 was a Thursday: day 0 is Thursday, so Monday is 4
            let weekday = (local.div_euclid(DAY as i64) + 3).rem_euclid(7); // 0 = Monday
            nine(7 - weekday)
        }
    }
}

/// A paired hub's organize with its machines renamed as the app names them (`hub:<id>/<machine>`).
pub fn rename_hosts(org: &mut Organize, name: impl Fn(&str) -> String) {
    for m in &mut org.sessions {
        m.host = name(&m.host);
    }
    for s in org.sends.iter_mut().flatten() {
        s.host = name(&s.host);
    }
    for p in &mut org.projects {
        for r in &mut p.roots {
            r.host = name(&r.host);
        }
    }
}

/// Several hubs' organize as one list: each keeps its own ids, which can't clash (the hub makes them).
/// Sleep rules are each hub's own ("all" means all of that hub's sessions), so a merge leaves them
/// out: read them from the hub's own organize.
pub fn merge<'o>(parts: impl IntoIterator<Item = &'o Organize>) -> Organize {
    let mut out = Organize::default();
    for o in parts {
        out.v = out.v.max(o.v);
        out.sessions.extend(o.sessions.iter().cloned());
        out.projects.extend(o.projects.iter().cloned());
        out.groups.extend(o.groups.iter().cloned());
        out.spaces.extend(o.spaces.iter().cloned());
        out.sends.get_or_insert_with(Vec::new).extend(o.sends.iter().flatten().cloned());
        out.prompts.get_or_insert_with(Vec::new).extend(o.prompts.iter().flatten().cloned());
    }
    out.sends = out.sends.filter(|l| !l.is_empty());
    out.prompts = out.prompts.filter(|l| !l.is_empty());
    out
}

/// The messages waiting to go to a session, the soonest first (a failed one, with its reason, too).
pub fn sends_for<'a>(org: &'a Organize, host: &str, pane: &str) -> Vec<&'a DelayedSend> {
    let mut out: Vec<&DelayedSend> = org.sends.iter().flatten().filter(|s| s.host == host && s.pane == pane).collect();
    out.sort_by_key(|s| s.at);
    out
}

/// The saved prompts to offer in a session of `project`: the starred first, then the project's,
/// then those for every session, each in the order kept. Another project's aren't offered.
pub fn prompts_for<'a>(org: &'a Organize, project: Option<&str>) -> Vec<&'a SavedPrompt> {
    let mut out: Vec<&SavedPrompt> = org.prompts.iter().flatten().filter(|q| q.project.is_none() || q.project.as_deref() == project).collect();
    out.sort_by_key(|q| (q.starred != Some(true), q.project.is_none()));
    out
}

/// After how many idle minutes this hub puts a session's agent to sleep; None when it doesn't.
pub fn sleeps_after(r: &PaneRef, org: &Organize) -> Option<u32> {
    sleep_minutes(org, pane_project(r, org))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::overview::all_panes;
    use crate::test_support::{agent_pane, host};
    use inyon_protocol::HostState;
    use serde_json::{json, Value};

    /// a Tuesday, 12:00 UTC
    const NOW: u64 = 1_791_892_800_000;

    fn org(v: Value) -> Organize {
        serde_json::from_value(v).unwrap()
    }

    fn hosts() -> Vec<HostState> {
        vec![host(
            "box",
            vec![
                agent_pane("%1", "claude", json!({ "cwd": "/w/inyon" })),
                agent_pane("%2", "codex", json!({ "cwd": "/w/inyon/apps" })),
                agent_pane("%3", "claude", json!({ "cwd": "/w/site" })),
                agent_pane("%4", "claude", json!({ "cwd": "/tmp" })),
                agent_pane("%5", "claude", json!({ "cwd": "/w/inyon", "title": "✳ Fix login" })),
            ],
            json!({}),
        )]
    }

    fn panes(sorted: &[PaneRef]) -> Vec<String> {
        sorted.iter().map(|r| r.pane.id.clone()).collect()
    }

    fn sample() -> Organize {
        org(json!({ "v": 1,
            "sessions": [
                { "id": "s-a", "host": "box", "pane": "%3", "live": true, "pinned": 1 },
                { "id": "s-b", "host": "box", "pane": "%1", "live": true, "pinned": 0, "tags": ["review"] },
                { "id": "s-c", "host": "box", "pane": "%2", "live": true, "parked": true, "tags": ["review", "ui"] },
                { "id": "s-d", "host": "box", "pane": "%4", "live": true, "snoozedUntil": NOW + 60_000, "pinned": 2 },
                { "id": "s-e", "host": "box", "pane": "%5", "live": false, "pinned": 3, "title": "Old mark" },
            ],
            "projects": [
                { "id": "p-site", "name": "Site", "color": "rose", "roots": [{ "host": "box", "path": "/w/site" }] },
                { "id": "p-inyon", "name": "Inyon", "color": "sage", "group": "g-1", "roots": [{ "host": "box", "path": "/w/inyon" }] },
            ],
            "groups": [{ "id": "g-1", "name": "Mine", "color": "blue" }],
            "spaces": [{ "id": "w-1", "name": "Focus", "projects": ["p-inyon"] }],
        }))
    }

    #[test]
    fn pinned_come_first_in_order_and_snoozed_and_parked_fold_away() {
        let h = hosts();
        let o = sample();
        let s = sort_panes(all_panes(&h), &o, NOW);
        assert_eq!(panes(&s.pinned), ["%1", "%3"]);
        // a mark that isn't live says nothing about the pane that has its old id
        assert_eq!(panes(&s.sessions), ["%5"]);
        assert_eq!(panes(&s.snoozed), ["%4"], "snoozed beats pinned");
        assert_eq!(panes(&s.parked), ["%2"]);
        assert_eq!(s.folded(), 2);
        // the snooze is over: back where its pin puts it, before the hub has said so
        let s = sort_panes(all_panes(&h), &o, NOW + 60_000);
        assert_eq!(panes(&s.pinned), ["%1", "%3", "%4"]);
        assert_eq!(next_wake(&o, NOW), Some(NOW + 60_000));
        assert_eq!(next_wake(&o, NOW + 60_000), None);
    }

    #[test]
    fn sessions_fall_into_projects_by_folder_in_the_projects_order() {
        let h = hosts();
        let o = sample();
        let all = all_panes(&h);
        let groups: Vec<(Option<String>, Vec<String>)> = by_project(&all, &o).into_iter().map(|(p, v)| (p.map(|p| p.name.clone()), panes(&v))).collect();
        assert_eq!(groups, [(Some("Site".into()), vec!["%3".to_owned()]), (Some("Inyon".into()), vec!["%1".into(), "%2".into(), "%5".into()]), (None, vec!["%4".into()])]);
        assert_eq!(group_of(&o.projects[1], &o).map(|g| g.name.as_str()), Some("Mine"));
        assert!(in_space(&all[1], &o, "w-1"));
        assert!(!in_space(&all[2], &o, "w-1"));
        assert_eq!(shown_title(&all[4], &o), "Fix login", "an old mark's title isn't this pane's");
    }

    #[test]
    fn a_tag_filter_keeps_sessions_with_every_tag() {
        let o = sample();
        let m = |pane: &str| mark_for(&o, "box", pane);
        assert!(has_tags(m("%2"), &["review".into(), "ui".into()]));
        assert!(!has_tags(m("%1"), &["review".into(), "ui".into()]));
        assert!(has_tags(m("%4"), &[]));
        assert!(!has_tags(None, &["review".into()]));
        assert_eq!(tags_in_use(&o), ["review", "ui"]);
    }

    #[test]
    fn snooze_times_follow_the_devices_clock() {
        // Tuesday 12:00 UTC
        assert_eq!(snooze_until(SnoozeFor::Hour, NOW, 0), NOW + HOUR);
        assert_eq!(snooze_until(SnoozeFor::ThreeHours, NOW, 0), NOW + 3 * HOUR);
        let tue_midnight = NOW - 12 * HOUR;
        assert_eq!(snooze_until(SnoozeFor::Tomorrow, NOW, 0), tue_midnight + DAY + 9 * HOUR);
        assert_eq!(snooze_until(SnoozeFor::NextWeek, NOW, 0), tue_midnight + 6 * DAY + 9 * HOUR, "Monday");
        // in Kolkata (UTC+5:30) it's already 17:30 Tuesday; tomorrow 9:00 there is 3:30 UTC Wednesday
        assert_eq!(snooze_until(SnoozeFor::Tomorrow, NOW, 330), tue_midnight + DAY + 3 * HOUR + 30 * 60_000);
        // in Los Angeles (UTC-7) it's 5:00 Tuesday
        assert_eq!(snooze_until(SnoozeFor::Tomorrow, NOW, -420), tue_midnight + DAY + 16 * HOUR);
        // on a Monday, next week is seven days on
        let monday = NOW - DAY;
        assert_eq!(snooze_until(SnoozeFor::NextWeek, monday, 0), tue_midnight - DAY + 7 * DAY + 9 * HOUR);
    }

    #[test]
    fn a_paired_hubs_marks_are_renamed_and_merged() {
        let mut theirs = sample();
        rename_hosts(&mut theirs, |h| format!("hub:x/{h}"));
        assert_eq!(theirs.sessions[0].host, "hub:x/box");
        assert_eq!(theirs.projects[0].roots[0].host, "hub:x/box");
        let all = merge([&sample(), &theirs]);
        assert_eq!((all.sessions.len(), all.projects.len(), all.v), (10, 4, 1));
        assert!(mark_for(&all, "hub:x/box", "%1").is_some());
    }

    #[test]
    fn waiting_messages_prompts_and_sleep_rules_are_read_per_session() {
        let mut o = sample();
        o.sends = Some(
            serde_json::from_value(json!([
                { "id": "d-2", "host": "box", "pane": "%1", "text": "later", "at": NOW + 2 * HOUR },
                { "id": "d-1", "host": "box", "pane": "%1", "text": "sooner", "at": NOW + HOUR },
                { "id": "d-3", "host": "box", "pane": "%2", "text": "elsewhere", "at": NOW }
            ]))
            .unwrap(),
        );
        assert_eq!(sends_for(&o, "box", "%1").iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["d-1", "d-2"]);
        let pid = o.projects[0].id.clone();
        o.prompts = Some(
            serde_json::from_value(json!([
                { "id": "q-1", "name": "Mine", "text": "a" },
                { "id": "q-2", "name": "Theirs", "text": "b", "project": "p-other" },
                { "id": "q-3", "name": "Project", "text": "c", "project": pid },
                { "id": "q-4", "name": "Star", "text": "d", "starred": true }
            ]))
            .unwrap(),
        );
        assert_eq!(prompts_for(&o, Some(&pid)).iter().map(|q| q.id.as_str()).collect::<Vec<_>>(), ["q-4", "q-3", "q-1"]);
        assert_eq!(prompts_for(&o, None).iter().map(|q| q.id.as_str()).collect::<Vec<_>>(), ["q-4", "q-1"]);

        let hs = hosts();
        let all = all_panes(&hs);
        assert_eq!(sleeps_after(&all[0], &o), None, "off unless there's a rule");
        o.sleep = Some(vec![inyon_protocol::SleepRule { scope: "all".into(), minutes: 60 }, inyon_protocol::SleepRule { scope: pid.clone(), minutes: 15 }]);
        let in_project = all.iter().find(|r| pane_project(r, &o).is_some_and(|p| p.id == pid)).unwrap();
        let outside = all.iter().find(|r| pane_project(r, &o).is_none()).unwrap();
        assert_eq!((sleeps_after(in_project, &o), sleeps_after(outside, &o)), (Some(15), Some(60)));

        // a paired hub's messages carry its machines' new names; its rules stay its own
        let mut theirs = o.clone();
        rename_hosts(&mut theirs, |h| format!("hub:x/{h}"));
        let both = merge([&o, &theirs]);
        assert_eq!((sends_for(&both, "hub:x/box", "%1").len(), both.prompts.map(|l| l.len()), both.sleep), (2, Some(8), None));
    }

    #[test]
    fn every_colour_has_a_swatch_on_both_themes() {
        for c in inyon_protocol::PROJECT_COLORS {
            assert!(color_hex(c, false).starts_with('#') && color_hex(c, true).starts_with('#'));
        }
    }
}
