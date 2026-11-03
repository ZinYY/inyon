//! agents/opencode-screen.test.ts, test for test. Each test's name in the TS file is its doc comment.

use super::*;
use crate::claude::screen::parse_screen;
use crate::test_support::screen_fixture;
use inyon_protocol::{AgentKind, Num, PromptTab, ScreenPrompt};

// real OpenCode 1.19.3 captures (tmux capture-pane -p -e), wide ones with the sidebar
fn screen(name: &str, width: u32) -> ScreenState {
    parse_screen(&screen_fixture("opencode", name), Some(width), AgentKind::Opencode)
}

fn labels(p: &ScreenPrompt) -> Vec<&str> {
    p.options.iter().map(|o| o.label.as_str()).collect()
}

fn tab(label: &str, done: bool, current: bool) -> PromptTab {
    PromptTab { label: label.into(), done, current }
}

/// draft, agent, model and variant from the input box, sidebar cut off
#[test]
fn draft_agent_model_and_variant_from_the_input_box_sidebar_cut_off() {
    let s = screen("draft-wide", 160);
    assert_eq!(s.input_visible, Some(true));
    assert!(!s.working);
    assert_eq!(s.draft.as_deref(), Some("hello from the draft"));
    assert_eq!(s.mode.as_deref(), Some("plan"));
    assert_eq!(s.model.as_deref(), Some("DeepSeek V4.1 Flash Free OpenCode"));
    assert_eq!(s.effort.as_deref(), Some("max"));
    assert_eq!(s.prompt, None);
}

/// narrow window, no sidebar; the context percentage comes off the wrapped status row
#[test]
fn narrow_window_no_sidebar_the_context_percentage_comes_off_the_wrapped_status_row() {
    let s = screen("draft-narrow", 110);
    assert_eq!(s.draft.as_deref(), Some("second draft line"));
    assert_eq!(s.mode.as_deref(), Some("build"));
    assert_eq!(s.effort.as_deref(), Some("max"));
    assert_eq!(s.context_pct, Some(Num(8.0)));
    assert_eq!(s.context_used, None);
}

/// the sidebar gives the context tokens, the percentage and the cost
#[test]
fn the_sidebar_gives_the_context_tokens_the_percentage_and_the_cost() {
    let s = screen("idle-wide", 160);
    assert_eq!(s.input_visible, Some(true));
    assert_eq!(s.draft, None);
    assert_eq!(s.context_used, Some(Num(36898.0)));
    assert_eq!(s.context_pct, Some(Num(18.0)));
    assert_eq!(s.cost_usd, Some(Num(0.0)));
}

/// working while the status line offers esc interrupt
#[test]
fn working_while_the_status_line_offers_esc_interrupt() {
    let s = screen("working", 160);
    assert!(s.working);
    assert_eq!(s.activity.as_deref(), Some("Working"));
    assert_eq!(s.draft, None);
    assert_eq!(s.context_pct, Some(Num(18.0)));
}

/// what /undo leaves in the box isn't a draft
#[test]
fn what_undo_leaves_in_the_box_isnt_a_draft() {
    let s = screen("reverted", 160);
    assert_eq!(s.input_visible, Some(true));
    assert_eq!(s.draft, None);
    assert_eq!(s.model.as_deref(), Some("DeepSeek V4.1 Flash Free OpenCode"));
}

/// permission request becomes three buttons
#[test]
fn permission_request_becomes_three_buttons() {
    let s = screen("permission", 110);
    let p = s.prompt.unwrap();
    assert_eq!(labels(&p), vec!["Allow once", "Allow always", "Reject"]);
    assert!(p.body.join("\n").contains("Access external directory /etc"));
    assert!(p.body.join("\n").contains("- /etc/*"));
    assert_eq!(p.navigate, Some(true));
    assert_eq!(s.model, None);
}

/// "Allow always" asks once more
#[test]
fn allow_always_asks_once_more() {
    let p = screen("permission-always", 110).prompt.unwrap();
    assert_eq!(labels(&p), vec!["Confirm", "Cancel"]);
    assert_eq!(p.body[0], "Always allow this until OpenCode restarts?");
}

/// the agent's question with descriptions and a free-text choice
#[test]
fn the_agents_question_with_descriptions_and_a_free_text_choice() {
    let p = screen("question", 110).prompt.unwrap();
    assert_eq!(p.body, vec!["Do you prefer tabs or spaces?"]);
    assert_eq!(p.options.iter().map(|o| (o.n, o.label.as_str())).collect::<Vec<_>>(), vec![(1, "Tabs"), (2, "Spaces"), (3, "Type your own answer")]);
    assert_eq!(p.options[0].detail.as_deref(), Some("Use tab characters for indentation"));
    assert_eq!(p.free_text_option, Some(3));
    assert!(!p.multi);
    assert_eq!(p.confirm, None);
    assert_eq!(p.tabs, None);
}

/// a multi-select question: checkboxes, a tab strip and a Confirm step
#[test]
fn a_multi_select_question_checkboxes_a_tab_strip_and_a_confirm_step() {
    let p = screen("question-multi", 110).prompt.unwrap();
    assert_eq!(p.body, vec!["Which colours do you like? (select all that apply)"]);
    assert!(p.multi);
    assert_eq!(
        p.options.iter().map(|o| (o.label.as_str(), o.checked)).collect::<Vec<_>>(),
        vec![("Red", Some(false)), ("Green", Some(false)), ("Blue", Some(false)), ("Type your own answer", Some(false))]
    );
    assert_eq!(p.tabs, Some(vec![tab("Colour preference", false, true), tab("Confirm", false, false)]));
    assert_eq!(p.confirm.as_deref(), Some("Confirm"));
    let checked = screen("question-multi-checked", 110).prompt.unwrap();
    assert_eq!(checked.options.iter().map(|o| o.checked).collect::<Vec<_>>(), vec![Some(false), Some(true), Some(true), Some(false)]);
}

/// the Confirm tab reviews the answers and Enter sends them
#[test]
fn the_confirm_tab_reviews_the_answers_and_enter_sends_them() {
    let p = screen("question-review", 110).prompt.unwrap();
    assert_eq!(p.body, vec!["Review", "Colour preference: Green, Blue"]);
    assert_eq!(p.options, vec![]);
    assert_eq!(p.confirm.as_deref(), Some("Submit"));
    assert_eq!(p.tabs.unwrap().iter().map(|t| (t.label.as_str(), t.current, t.done)).collect::<Vec<_>>(), vec![("Colour preference", false, true), ("Confirm", true, false)]);
}

/// two questions at once: the first tab is open, and its answer confirms and moves on
#[test]
fn two_questions_at_once_the_first_tab_is_open_and_its_answer_confirms_and_moves_on() {
    let p = screen("question-two", 160).prompt.unwrap();
    assert_eq!(p.tabs.as_ref().unwrap().iter().map(|t| (t.label.as_str(), t.current)).collect::<Vec<_>>(), vec![("Indentation", true), ("Colours", false), ("Confirm", false)]);
    assert_eq!(p.body, vec!["Do you prefer tabs or spaces?"]);
    assert_eq!(labels(&p), vec!["Tabs", "Spaces", "Type your own answer"]);
    assert!(!p.multi);
    assert_eq!(p.confirm, None);
}

/// a menu over the box (the model list) hides the input but keeps the footer
#[test]
fn a_menu_over_the_box_the_model_list_hides_the_input_but_keeps_the_footer() {
    let s = screen("models-dialog", 160);
    assert_eq!(s.input_visible, Some(false));
    assert_eq!(s.prompt, None);
    assert_eq!(s.model.as_deref(), Some("DeepSeek V4.1 Flash Free OpenCode"));
}

/// the variant menu after picking a model is a menu too
#[test]
fn the_variant_menu_after_picking_a_model_is_a_menu_too() {
    let s = screen("select-variant", 120);
    assert_eq!(s.input_visible, Some(false));
    assert_eq!(s.prompt, None);
}

/// the slash popup above the box is not part of the draft
#[test]
fn the_slash_popup_above_the_box_is_not_part_of_the_draft() {
    let s = screen("slash-popup", 120);
    assert_eq!(s.draft.as_deref(), Some("/und"));
    assert_eq!(s.input_visible, Some(true));
    assert_eq!(s.effort.as_deref(), Some("high"));
}

/// home screen (no conversation yet): the box is centred under the logo
#[test]
fn home_screen_no_conversation_yet_the_box_is_centred_under_the_logo() {
    let s = screen("home", 110);
    assert_eq!(s.input_visible, Some(true));
    assert!(!s.working);
    assert_eq!(s.mode.as_deref(), Some("build"));
    assert_eq!(s.model.as_deref(), Some("DeepSeek V4.1 Flash Free OpenCode"));
    assert_eq!(s.effort.as_deref(), Some("max"));
    assert_eq!(s.draft, None);
    assert_eq!(s.context_pct, None);
}

/// home screen with the model menu over the box
#[test]
fn home_screen_with_the_model_menu_over_the_box() {
    let s = screen("home-models", 120);
    assert_eq!(s.input_visible, Some(false));
    assert_eq!(s.prompt, None);
}

/// the open tab is the one on its own background; without colours nothing is picked
#[test]
fn the_open_tab_is_the_one_on_its_own_background_without_colours_nothing_is_picked() {
    let row = "\x1b[48;2;1;22;39m  \x1b[48;2;11;37;58m┃  \x1b[48;2;199;146;234m One\x1b[48;2;11;37;58m   Two   Confirm";
    let three: Vec<String> = ["One", "Two", "Confirm"].map(String::from).to_vec();
    assert_eq!(current_tab(Some(row), &three), 0);
    assert_eq!(current_tab(Some(&row.replacen("48;2;199;146;234", "48;2;11;37;58", 1)), &three), -1);
    assert_eq!(current_tab(Some("┃  One   Two   Confirm"), &three), -1);
    assert_eq!(current_tab(None, &["One".to_owned()]), -1);
}

/// review: an option number past u32 stays its number, as Number() keeps it in the TS
#[test]
fn a_long_option_number_stays_its_number() {
    let lines: Vec<String> = screen_fixture("opencode", "question").into_iter().map(|l| l.replacen("m2.\u{1b}[", "m99999999999.\u{1b}[", 1)).collect();
    let p = parse_screen(&lines, Some(110), AgentKind::Opencode).prompt.unwrap();
    assert_eq!(p.options.iter().map(|o| o.n).collect::<Vec<_>>(), vec![1, 99_999_999_999, 3]);
}
