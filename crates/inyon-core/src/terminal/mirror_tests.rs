//! terminal/mirror.test.ts, the parts that need no tmux, and the psmux check from
//! hosts/win-prelude.test.ts. The end-to-end tests against a real tmux server are in
//! inyon-backends, which has real processes.

use super::*;
use crate::test_support::{sh, TempDir};

fn keys(k: &[&str]) -> Input {
    Input::Keys(k.iter().map(|s| s.to_string()).collect())
}

fn text(t: &str) -> Input {
    Input::Text(t.into())
}

#[test]
fn typing_becomes_literal_text_keys_by_name_and_raw_bytes_only_for_the_rest() {
    assert_eq!(to_input("hello"), [text("hello")]);
    assert_eq!(to_input("ls -la\r"), [text("ls -la"), keys(&["Enter"])]);
    assert_eq!(to_input("\x1b[A\x1b[B\x1bOC\x7f\t\x03"), [keys(&["Up", "Down", "Right", "BSpace", "Tab", "C-c"])]);
    assert_eq!(to_input("\x1b[1;5D\x1b[3~\x1b[5~\x1b[Z"), [keys(&["C-Left", "DC", "PPage", "BTab"])]);
    assert_eq!(to_input("\x1bb\x1b"), [keys(&["M-b", "Escape"])]);
    assert_eq!(to_input("\x1c\x1f\x00"), [keys(&["C-\\", "C-_", "C-Space"])]);
    assert_eq!(to_input("héllo ⏺ 日本"), [text("héllo ⏺ 日本")]);
    // a mouse report has no name: it goes as the bytes it is
    assert_eq!(to_input("\x1b[<0;3;4M"), [Input::Hex(["1b", "5b", "3c", "30", "3b", "33", "3b", "34", "4d"].map(str::to_owned).to_vec())]);
}

#[test]
fn a_bracketed_paste_goes_in_as_one_paste_with_the_text_around_it_kept_in_order() {
    assert_eq!(to_input("a\x1b[200~line 1\rline 2\x1b[201~\r"), [text("a"), Input::Paste("line 1\rline 2".into()), keys(&["Enter"])]);
    let (cmd, input) = input_command("%14.1", &Input::Paste("x\r\ny".into()));
    // through a file: psmux's load-buffer takes a path, not "-"
    assert!(regex::Regex::new(r#"load-buffer -b 'inyon-term-\w+' "\$f" && tmux paste-buffer -d -p -b 'inyon-term-\w+' -t '%14\.1'"#).unwrap().is_match(&cmd), "{cmd}");
    assert_eq!(input.as_deref(), Some("x\ny"));
}

#[test]
fn typed_text_goes_on_stdin_never_on_the_command_line() {
    // a password typed at a sudo prompt in the terminal view must not show in /proc/*/cmdline
    let (cmd, input) = input_command("%1", &text("'; rm -rf ~; echo '"));
    assert!(!cmd.contains("rm -rf"), "{cmd}");
    assert!(regex::Regex::new(r#"load-buffer -b 'inyon-term-\w+' "\$f" && tmux paste-buffer -d -b 'inyon-term-\w+' -t '%1'"#).unwrap().is_match(&cmd), "{cmd}");
    assert_eq!(input.as_deref(), Some("'; rm -rf ~; echo '"));
    assert_eq!(input_command("%1", &keys(&["C-\\"])).0, r"tmux send-keys -t '%1' 'C-\'");
}

#[test]
fn the_screen_is_read_with_its_size_and_cursor_one_entry_per_row() {
    let s = parse_screen("10 3 2 1 1\nab\n\x1b[31mred\x1b[0m\n").unwrap();
    assert_eq!(
        s,
        Screen { width: 10.0, height: 3.0, cursor: Cursor { x: 2.0, y: 1.0, visible: true }, lines: vec!["ab".into(), "\x1b[31mred\x1b[0m".into(), String::new()], mouse: false }
    );
    assert_eq!(parse_screen(""), None);
    assert_eq!(parse_screen("can't find pane\n"), None);
}

#[test]
fn only_the_rows_that_changed_are_drawn_again() {
    let a = parse_screen("10 3 0 0 1\none\ntwo\nthree").unwrap();
    let first = draw_screen(None, &a);
    // wrapping off, so a pane wider than the phone is cut at the edge
    assert!(first.starts_with("\x1b[?7l"));
    assert!(first.contains("\x1b[2;1H\x1b[0mtwo"));
    let b = parse_screen("10 3 3 1 1\none\ntwo!\nthree").unwrap();
    assert_eq!(draw_screen(Some(&a), &b), "\x1b[2;1H\x1b[0mtwo!\x1b[0m\x1b[K\x1b[2;4H\x1b[?25h");
    assert_eq!(draw_screen(Some(&b), &b), "");
    let hidden = Screen { cursor: Cursor { visible: false, ..b.cursor.clone() }, ..b.clone() };
    assert_eq!(draw_screen(Some(&b), &hidden), "\x1b[2;4H\x1b[?25l");
}

const PRELUDE: &str = include_str!("../../../inyon-engine/src/win-prelude.sh");

#[test]
fn the_terminals_commands_reach_the_right_psmux_pane() {
    // a stand-in for psmux's tmux.exe: logs its argv
    let bin = TempDir::new("psmux-");
    let log = bin.path("argv");
    bin.write("tmux", &format!("#!/bin/sh\nfor a in \"$@\"; do printf '[%s]' \"$a\" >> \"{}\"; done; echo >> \"{}\"\n", log.display(), log.display()));
    std::fs::set_permissions(bin.path("tmux"), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    bin.write("argv", "");
    let cmd = format!("{}; {}; {}", read_command("%14.1"), input_command("%14.1", &keys(&["C-c", "Up"])).0, input_command("%14.1", &Input::Hex(vec!["1b".into(), "5b".into()])).0);
    let path = format!("{}:/usr/bin:/bin", bin.s());
    sh(&format!("{PRELUDE}\n{cmd}"), None, &[("PATH", &path)]);
    assert_eq!(
        bin.read("argv"),
        "[display][-p][-t][$14:.%1][#{pane_width} #{pane_height} #{cursor_x} #{cursor_y} #{cursor_flag} #{alternate_on}]\n\
         [capture-pane][-p][-e][-t][$14:.%1]\n\
         [send-keys][-t][$14:.%1][C-c][Up]\n\
         [send-keys][-t][$14:.%1][-H][1b][5b]\n"
    );
}

#[test]
fn a_keystroke_batch_is_one_shell_and_each_more_piece_of_text_starts_the_next() {
    let b = batch_commands("%1", &to_input("ls -la\r"));
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].1.as_deref(), Some("ls -la"));
    assert!(b[0].0.starts_with("{ umask 077; f=$(mktemp) && "), "{}", b[0].0);
    assert!(b[0].0.ends_with("; [ $r = 0 ]; }; } && tmux send-keys -t '%1' 'Enter'"), "{}", b[0].0);
    // the file goes once: the trap starts no second rm
    assert_eq!(b[0].0.matches(r#"rm -f "$f""#).count(), 2);
    assert!(b[0].0.contains(r#"trap '[ -z "$f" ] || rm -f "$f"'"#) && b[0].0.contains(r#"rm -f "$f"; f=;"#));
    let inputs: Vec<_> = batch_commands("%1", &to_input("a\x1b[Ab")).into_iter().map(|(_, i)| i).collect();
    assert_eq!(inputs, [Some("a".to_string()), Some("b".to_string())]);
    assert_eq!(batch_commands("%1", &to_input("\x1b[A")), [("tmux send-keys -t '%1' 'Up'".to_string(), None)]);
}

/// Claude Code runs full screen, so its history scrolls with the wheel inside it. On Windows the
/// terminal view had no way to scroll at all: the browser terminal never reported the mouse, so the
/// wheel (and a finger drag on a phone, which becomes wheel steps) did nothing.
#[test]
fn a_full_screen_program_gets_the_mouse_wheel_and_a_shell_keeps_the_mouse_to_itself() {
    let shell = parse_screen("10 2 0 0 1 0\n$ ls\n").unwrap();
    let claude = parse_screen("10 2 0 0 1 1\n● Done.\n❯\n").unwrap();
    assert!(!shell.mouse && claude.mouse);
    let on = "\x1b[?1000h\x1b[?1006h";
    let off = "\x1b[?1000l\x1b[?1006l";
    assert!(!draw_screen(None, &shell).contains(on));
    // a full redraw after a resize turns it off for a shell, in case Claude had it on before
    assert!(draw_screen(None, &shell).contains(off));
    assert!(draw_screen(None, &claude).contains(on));
    assert!(draw_screen(Some(&shell), &claude).starts_with(on));
    // said once, not on every redraw
    assert!(!draw_screen(Some(&claude), &claude).contains(on));
    assert!(draw_screen(Some(&claude), &shell).starts_with(off));
    // psmux before alternate_on: no sixth number, so no mouse
    assert!(!parse_screen("10 2 0 0 1\nx\n").unwrap().mouse);
    // the wheel the browser terminal reports goes to the program as the bytes it is
    let up = to_input("\x1b[<64;10;5M");
    assert_eq!(up, [Input::Hex("\x1b[<64;10;5M".bytes().map(|b| format!("{b:02x}")).collect())]);
    assert_eq!(batch_commands("%55.2", &up)[0].0, "tmux send-keys -t '%55.2' -H 1b 5b 3c 36 34 3b 31 30 3b 35 4d");
}
