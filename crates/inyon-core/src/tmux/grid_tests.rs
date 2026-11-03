use super::*;

fn map(rows: &[(&str, (u32, u32))]) -> IndexMap<String, (u32, u32)> {
    rows.iter().map(|(w, s)| ((*w).to_owned(), *s)).collect()
}

#[test]
fn only_agent_windows_nobody_shows_rest_and_each_is_asked_once_per_size() {
    let agents: HashSet<String> = ["@1", "@2", "@3"].map(String::from).into();
    let mut tried = HashMap::new();
    // @2 is at the grid already, @4 holds no agent, @3 someone is looking at (not unwatched)
    let unwatched = map(&[("@1", (80, 24)), ("@2", (200, 50)), ("@4", (80, 24))]);
    assert_eq!(to_rest(&unwatched, &agents, &mut tried), ["@1"]);
    // still 80×24 on the next poll: tmux didn't take it, so it isn't asked again
    assert!(to_rest(&unwatched, &agents, &mut tried).is_empty());
    // a phone opened it and left it at 40 columns: asked again
    let after_phone = map(&[("@1", (40, 20)), ("@2", (200, 50))]);
    assert_eq!(to_rest(&after_phone, &agents, &mut tried), ["@1"]);
    // someone attached (it's watched now) and left: it rests again, even at the size it had
    assert!(to_rest(&map(&[]), &agents, &mut tried).is_empty());
    assert_eq!(to_rest(&after_phone, &agents, &mut tried), ["@1"]);
}

#[test]
fn the_command_sizes_and_puts_the_option_back_in_one_call_and_respects_set_sizes() {
    let c = rest_command(&["@1".into(), "@7".into()]);
    assert!(c.starts_with(r#"[ "$(tmux show -gv window-size 2>/dev/null)" = manual ] || { "#), "{c}");
    assert!(c.contains(r"tmux resize-window -t '@1' -x 200 -y 50 \; set-option -w -t '@1' -u window-size"), "{c}");
    assert!(c.contains(r#"[ -n "$(tmux show -wv -t '@7' window-size 2>/dev/null)" ] || tmux resize-window -t '@7'"#), "{c}");
    assert!(c.ends_with("; }; true"), "{c}");
}
