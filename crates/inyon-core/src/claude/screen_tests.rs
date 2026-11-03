//! claude/screen.test.ts, test for test. Each test's name in the TS file is its doc comment.

use super::*;
use crate::test_support::{fixture, screen, screen_fixture};
use serde_json::json;

fn claude(lines: &[String], width: Option<u32>) -> ScreenState {
    parse_screen(lines, width, AgentKind::Claude)
}

fn labels(p: &ScreenPrompt) -> Vec<&str> {
    p.options.iter().map(|o| o.label.as_str()).collect()
}

/// toMatchObject on the fields given
fn matches(state: &ScreenState, want: serde_json::Value) {
    let got = serde_json::to_value(state).unwrap();
    for (k, v) in want.as_object().unwrap() {
        assert_eq!(got.get(k).unwrap_or(&serde_json::Value::Null), v, "{k}");
    }
}

fn r100() -> String {
    "─".repeat(100)
}

/// permission prompt
#[test]
fn permission_prompt() {
    let s = claude(&screen_fixture("claude", "permission-prompt"), None);
    assert!(!s.working);
    let p = s.prompt.unwrap();
    assert_eq!(p.options.iter().map(|o| o.n).collect::<Vec<_>>(), vec![1, 2, 3, 4]);
    assert_eq!((p.options[0].label.as_str(), p.options[0].selected), ("Yes", true));
    assert!(p.options[1].label.contains("-d507-4808"));
    assert_eq!(p.body[0], "Bash command");
    assert!(p.body.contains(&"echo hello-inyon > out.txt".to_owned()));
    assert_eq!(p.footer.as_deref(), Some("Esc to cancel · Tab to amend"));
    assert!(!p.multi);
}

/// ask user question, single select
#[test]
fn ask_user_question_single_select() {
    let p = claude(&screen_fixture("claude", "question-single"), None).prompt.unwrap();
    assert_eq!(p.body, vec!["What is your favorite color?"]);
    assert_eq!(labels(&p), vec!["Red", "Green", "Blue", "Type something.", "Chat about this"]);
    assert_eq!(p.options[0].detail.as_deref(), Some("The color red"));
    assert_eq!(p.free_text_option, Some(4));
    let tabs = p.tabs.unwrap();
    assert_eq!(tabs.iter().map(|t| t.label.as_str()).collect::<Vec<_>>(), vec!["Color", "Fruits", "Submit"]);
    assert!(tabs[0].current);
}

/// A question whose options carry previews draws the highlighted one's box beside the labels,
/// and there a number only moves the highlight: the labels come without the box, and it's answered
/// by moving the highlight and pressing Enter. Captured from Claude Code 2.1.312.
#[test]
fn ask_user_question_with_previews() {
    let p = claude(&screen_fixture("claude", "question-preview-tabs"), None).prompt.unwrap();
    assert_eq!(p.body, vec!["Where should the live name show?"]);
    assert_eq!(labels(&p), vec!["Heading (Recommended)", "On each row", "Rename the page too"]);
    assert!(p.options.iter().all(|o| o.detail.is_none()), "{:?}", p.options);
    assert_eq!(p.navigate, Some(true));
    assert!(p.tabs.unwrap()[0].current);
    // the box shows the highlighted option's preview, but what's asked stays the same question
    let first = claude(&screen_fixture("claude", "question-preview"), None).prompt.unwrap();
    let second = claude(&screen_fixture("claude", "question-preview-second"), None).prompt.unwrap();
    assert_eq!(labels(&first), vec!["Grid", "List", "Cards"]);
    assert_eq!(second.options.iter().find(|o| o.selected).map(|o| o.n), Some(2));
    assert_eq!(inyon_protocol::prompt_key(&first.body, &first.options), inyon_protocol::prompt_key(&second.body, &second.options));
    // one question: its strip has no arrows, and it isn't part of the question
    assert_eq!(first.body, vec!["Which layout?"]);
    assert_eq!(first.tabs.unwrap().iter().map(|t| t.label.as_str()).collect::<Vec<_>>(), vec!["Layout"]);
}

/// ask user question, multi select
#[test]
fn ask_user_question_multi_select() {
    let p = claude(&screen_fixture("claude", "question-multi"), None).prompt.unwrap();
    assert!(p.multi);
    assert_eq!(p.options[..3].iter().map(|o| (o.label.as_str(), o.checked)).collect::<Vec<_>>(), vec![("Apple", Some(true)), ("Mango", Some(false)), ("Kiwi", Some(true))]);
    assert_eq!(p.options[3].detail, None);
    assert!(p.tabs.unwrap()[1].current);
}

/// idle with draft, mode and finished spinner
#[test]
fn idle_with_draft_mode_and_finished_spinner() {
    let s = claude(&screen_fixture("claude", "idle-draft"), None);
    assert_eq!(s.prompt, None);
    assert!(!s.working);
    assert_eq!(s.draft.as_deref(), Some("half typed message"));
    assert_eq!(s.mode.as_deref(), Some("manual"));
}

/// a numbered list in the last reply isn't a question once the session has a name
#[test]
fn a_numbered_list_in_the_last_reply_isnt_a_question_once_the_session_has_a_name() {
    let lines = screen_fixture("claude", "renamed-session");
    let s = claude(&lines, Some(88));
    assert_eq!(s.prompt, None);
    assert_eq!(s.input_visible, Some(true));
    assert_eq!(s.mode.as_deref(), Some("auto"));
    // and what you type there is still read as a draft
    let typed: Vec<String> = lines.iter().map(|l| if l == "❯ " { "❯ 44".to_owned() } else { l.clone() }).collect();
    assert_eq!(claude(&typed, Some(88)).draft.as_deref(), Some("44"));
}

/// working
#[test]
fn working() {
    let s = claude(&screen_fixture("claude", "working"), None);
    assert!(s.working);
    assert_eq!(s.activity.as_deref(), Some("Undulating… (2s · thinking with high effort)"));
    assert_eq!(s.mode.as_deref(), Some("accept edits"));
    assert_eq!(s.draft, None);
}

/// Claude Code's spinner words can have accents ("Flambéing", "Sautéing"): the screen still says
/// it works. With letters read as ASCII only, it looked idle for the whole turn.
#[test]
fn a_spinner_word_with_an_accent_still_says_it_works() {
    let s = claude(&screen_fixture("claude", "reply-before-tool-dot-on"), None);
    assert!(s.working);
    assert_eq!(s.activity.as_deref(), Some("Flambéing… (3s · ↓ 54 tokens)"));
}

/// word-wrapped option label keeps its space
#[test]
fn word_wrapped_option_label_keeps_its_space() {
    let p = claude(&screen_fixture("claude", "permission-wrapped"), Some(80)).prompt.unwrap();
    assert_eq!(p.options[1].label, "Yes, and always allow access to /tmp/tester-1001/scratchpad/play from this project");
    assert_eq!(p.options[1].detail, None);
}

/// menu without numbers (remote control)
#[test]
fn menu_without_numbers_remote_control() {
    let p = claude(&screen_fixture("claude", "remote-control-menu"), Some(100)).prompt.unwrap();
    assert_eq!(p.navigate, Some(true));
    assert_eq!(labels(&p), vec!["Disconnect this session", "Show QR code", "Continue"]);
    assert_eq!(p.options[1].detail.as_deref(), Some("Scan with your phone to open this session"));
    assert!(p.options[2].selected);
    assert_eq!(p.body[0], "Remote Control");
}

/// login url dialog with code input
#[test]
fn login_url_dialog_with_code_input() {
    let url1 = "https://claude.com/cai/oauth/authorize?code=true&client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e&resp";
    let url2 = "onse_type=code&redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback&scope=org%3A";
    let url3 = "create_api_key&state=UwbdOhIwrMI5ooVjgLIvz5ysBPXoVHjRtqWSBhTnQO4";
    let p = claude(&screen_fixture("claude", "login-url"), Some(100)).prompt.unwrap();
    assert_eq!(p.input.as_ref().unwrap().label, "Paste code here if prompted");
    assert_eq!(p.links, Some(vec![format!("{url1}{url2}{url3}")]));
    assert_eq!(p.body[0], "Login");
    assert!(!p.body.join(" ").contains("https://"));
}

/// login method menu under a ▔ dialog top
#[test]
fn login_method_menu_under_a_dialog_top() {
    let p = claude(&screen_fixture("claude", "login-method"), Some(100)).prompt.unwrap();
    assert_eq!(p.body, vec!["Login", "Select login method:"]);
    assert_eq!(p.options.len(), 3);
}

/// describe('prompt suggestions (dimmed placeholder)')
mod prompt_suggestions_dimmed_placeholder {
    use super::*;
    const E: &str = "\x1b";

    fn bx(input: &str) -> Vec<String> {
        let rule = format!("{E}[37m{}{E}[39m", "─".repeat(60));
        vec![rule.clone(), input.to_owned(), rule, format!("  {E}[91m⏵⏵ bypass permissions on{E}[39m")]
    }

    /// a suggestion is not a draft
    #[test]
    fn a_suggestion_is_not_a_draft() {
        // Claude: first char inverted as the cursor, the rest dimmed
        let s = claude(&bx(&format!("{E}[39m❯ {E}[7mr{E}[27m{E}[2mun the tests and fix what fails{E}[22m")), Some(60));
        assert_eq!(s.draft, None);
        assert_eq!(s.mode.as_deref(), Some("bypass permissions"));
    }

    /// typed text is a draft, with the cursor at the end or inside it
    #[test]
    fn typed_text_is_a_draft_with_the_cursor_at_the_end_or_inside_it() {
        assert_eq!(claude(&bx(&format!("❯ fix the login bug{E}[7m {E}[27m")), Some(60)).draft.as_deref(), Some("fix the login bug"));
        assert_eq!(claude(&bx(&format!("❯ fix {E}[7mt{E}[27mhe bug")), Some(60)).draft.as_deref(), Some("fix the bug"));
        assert_eq!(claude(&bx(&format!("❯ {E}[7mx{E}[27m")), Some(60)).draft.as_deref(), Some("x"));
    }

    /// unstyle drops colours and links but keeps the text
    #[test]
    fn unstyle_drops_colours_and_links_but_keeps_the_text() {
        assert_eq!(unstyle(&format!("{E}[38;2;100;100;100mgrey{E}[0m {E}]8;;https://x.y{E}\\link{E}]8;;{E}\\")), Unstyled { text: "grey link".into(), ghost: false });
    }
}

/// describe('other agents')
mod other_agents {
    use super::*;

    fn codex(name: &str) -> ScreenState {
        parse_screen(&screen_fixture("codex", name), Some(120), AgentKind::Codex)
    }

    /// Codex folder trust menu (real capture, Codex 0.159, 156 columns)
    #[test]
    fn codex_folder_trust_menu() {
        let p = parse_screen(&screen_fixture("codex", "trust"), Some(156), AgentKind::Codex).prompt.unwrap();
        assert_eq!(p.options, vec![opt(1, "Trust and continue", true), opt(2, "Back to Agent Command Center", false)]);
        assert_eq!(p.navigate, Some(true));
        assert_eq!(p.body[0], "Folder access");
        assert!(p.body.iter().any(|l| l.starts_with("Trust this folder?")));
        assert_eq!(p.footer.as_deref(), Some("enter continue · esc back"));
    }

    /// Codex command approval (real capture, Codex 0.162)
    #[test]
    fn codex_command_approval() {
        let s = codex("approval");
        assert!(!s.working);
        let p = s.prompt.unwrap();
        assert_eq!(
            labels(&p),
            vec!["Yes, proceed", "Yes, and don't ask again for commands that start with `curl -sI https://example.com`", "No, and tell Codex what to do differently"]
        );
        assert_eq!(p.navigate, Some(true));
        assert!(p.body.contains(&"$ curl -sI https://example.com | head -1".to_owned()));
        assert_eq!(p.links, None);
    }

    /// Codex working with a queued message and a draft
    #[test]
    fn codex_working_with_a_queued_message_and_a_draft() {
        let s = codex("queued");
        assert!(s.working);
        assert_eq!(s.activity.as_deref(), Some("Working"));
        assert_eq!(s.queue, Some(vec!["second message queued".to_owned()]));
        assert_eq!(s.draft.as_deref(), Some("draft text here"));
        assert_eq!(s.model.as_deref(), Some("GPT-6-Luna"));
        assert_eq!(s.effort.as_deref(), Some("high"));
    }

    /// the dimmed suggestion in an empty Codex composer is not a draft
    #[test]
    fn the_dimmed_suggestion_in_an_empty_codex_composer_is_not_a_draft() {
        assert_eq!(codex("working-empty").draft, None);
    }

    /// a Codex settings picker is a menu, not the composer (50 columns)
    #[test]
    fn a_codex_settings_picker_is_a_menu_not_the_composer() {
        let s = parse_screen(&screen_fixture("codex", "effort-narrow"), Some(50), AgentKind::Codex);
        assert_eq!(s.model, None);
        assert_eq!(s.draft, None);
        let p = s.prompt.unwrap();
        assert_eq!(p.body, vec!["Select Reasoning Level for GPT-6.1-Sol"]);
        assert_eq!(labels(&p), vec!["Low (default)", "Medium", "High", "Extra high", "More reasoning…"]);
        assert_eq!(p.options[0].current, Some(true));
    }

    /// Codex idle with a background terminal
    #[test]
    fn codex_idle_with_a_background_terminal() {
        let s = codex("bg-idle");
        assert!(!s.working);
        assert_eq!(s.background, Some(Some("1 background terminal".into())));
    }

    /// a Codex footer that lists nothing in the background says so
    #[test]
    fn a_codex_footer_that_lists_nothing_in_the_background_says_so() {
        assert_eq!(codex("question-pending").background, Some(None));
        assert_eq!(codex("working-empty").background, Some(Some("1 background terminal".into())));
        // a menu replaces the footer, so there is nothing to read
        assert_eq!(codex("approval").background, None);
    }

    /// the time Codex has worked so far is read off the working line
    #[test]
    fn the_time_codex_has_worked_so_far_is_read_off_the_working_line() {
        let now = 1_792_320_000_437;
        crate::clock::set_now(Some(now));
        let s = codex("question-pending");
        crate::clock::set_now(None);
        assert!(s.working);
        // "Working (39s • esc to interrupt)": 39 s ago, to the second
        let since = s.since.unwrap();
        assert!(now - since >= 38_500);
        assert!(now - since < 41_000);
        assert_eq!(since % 1000, 0);
    }

    /// a question Codex asked mid-turn waits under the working line
    #[test]
    fn a_question_codex_asked_mid_turn_waits_under_the_working_line() {
        let s = codex("question-pending");
        assert!(s.working);
        assert_eq!(s.questions, Some(1));
        assert_eq!(s.queue, None);
        assert_eq!(s.prompt, None);
    }

    /// the opened question (shift+←) is a menu of its options
    #[test]
    fn the_opened_question_is_a_menu_of_its_options() {
        let s = codex("question");
        assert_eq!(s.model, None);
        let p = s.prompt.unwrap();
        assert_eq!(p.body, vec!["Colour"]);
        assert_eq!(p.options, vec![opt(1, "Red", true), opt(2, "Blue", false), opt(3, "Other", false)]);
        assert_eq!(p.footer.as_deref(), Some("enter submit   ^] skip   shift+→ main prompt"));
        assert_eq!(p.navigate, Some(true));
    }

    /// the plan-mode question once a plan is written
    #[test]
    fn the_plan_mode_question_once_a_plan_is_written() {
        let p = codex("plan-implement").prompt.unwrap();
        assert_eq!(p.body, vec!["Implement this plan?"]);
        assert_eq!(
            p.options.iter().map(|o| (o.label.as_str(), o.detail.as_deref())).collect::<Vec<_>>(),
            vec![
                ("Yes, implement this plan", Some("Switch to Default and start coding")),
                ("Yes, clear context and implement", Some("Start a fresh thread (current context: 2% used)")),
                ("No, stay in Plan mode", Some("Continue planning with the model")),
            ]
        );
        assert!(p.options[0].selected);
        assert_eq!(p.footer.as_deref(), Some("enter select · esc back"));
    }

    /// a command approval, with its reason, in a read-only workspace
    #[test]
    fn a_command_approval_with_its_reason_in_a_read_only_workspace() {
        let p = codex("command-approval").prompt.unwrap();
        assert_eq!(p.body[0], "Would you like to run the following command?");
        assert!(p.body.contains(&"Reason: May I create hello.txt in the workspace containing the word hi?".to_owned()));
        assert!(p.body.contains(&"$ printf 'hi\\n' > hello.txt".to_owned()));
        assert_eq!(
            labels(&p),
            vec!["Yes, proceed", "Yes, and don't ask again for commands that start with `printf 'hi\\n' > hello.txt`", "No, and tell Codex what to do differently"]
        );
        // the alert and the overview say what would run
        assert_eq!(prompt_summary(&p), "Run printf 'hi\\n' > hello.txt");
        // the line is the whole question: quick answers are offered
        assert!(prompt_whole(&p));
    }

    /// a file change approval names the file
    #[test]
    fn a_file_change_approval_names_the_file() {
        let p = codex("patch-approval").prompt.unwrap();
        assert_eq!(p.body[0], "Would you like to make the following edits?");
        assert!(p.body.contains(&"Description: Apply proposed file edits".to_owned()));
        assert_eq!(labels(&p), vec!["Yes, proceed", "Yes, and don't ask again for these files", "No, and tell Codex what to do differently"]);
        assert!(p.options[0].selected);
        // the path was wrapped by the terminal ("…/notes2" and ".txt"); the file is still named
        assert_eq!(prompt_summary(&p), "Edit notes2.txt?");
        // the line leaves out what an answer agrees to: no quick answers
        assert!(!prompt_whole(&p));
    }

    /// the review presets
    #[test]
    fn the_review_presets() {
        let p = codex("review").prompt.unwrap();
        assert_eq!(p.body, vec!["Select a review preset"]);
        assert_eq!(labels(&p), vec!["Review against a base branch", "Review uncommitted changes", "Review a commit", "Custom review instructions"]);
        assert_eq!(p.options[0].detail.as_deref(), Some("(PR Style)"));
    }

    /// the model list marks the model in use, and the reasoning step marks the default
    #[test]
    fn the_model_list_marks_the_model_in_use_and_the_reasoning_step_marks_the_default() {
        let models = codex("model").prompt.unwrap();
        assert_eq!(models.body, vec!["Select Model and Effort"]);
        assert_eq!(
            models.options.iter().map(|o| (o.label.as_str(), o.current.unwrap_or(false))).collect::<Vec<_>>(),
            vec![("GPT-6-Luna", true), ("GPT-5.6-Terra", false), ("GPT-5.6-Luna", false)]
        );
        assert_eq!(models.options[0].detail.as_deref(), Some("Fast and affordable model for easier tasks."));
        let levels = codex("reasoning").prompt.unwrap();
        assert_eq!(levels.body, vec!["Select Reasoning Level for GPT-6-Luna"]);
        assert_eq!(labels(&levels), vec!["Low", "Medium (default)", "High", "Extra high", "More reasoning…"]);
        assert!(levels.options[1].selected);
        assert_eq!(levels.footer.as_deref(), Some("enter default · s session · esc back"));
    }

    /// the permission presets, each with what it allows
    #[test]
    fn the_permission_presets_each_with_what_it_allows() {
        let p = codex("permissions").prompt.unwrap();
        assert_eq!(p.body, vec!["Update Model Permissions"]);
        assert_eq!(labels(&p), vec!["Ask for approval", "Approve for me", "Full Access", "Read Only"]);
        assert_eq!(
            p.options[0].detail.as_deref(),
            Some("Read and edit workspace files and run commands, with approval required for internet access or edits outside the workspace")
        );
        assert_eq!(p.options[3].detail.as_deref(), Some("Read workspace files, with approval required for edits or internet access"));
    }

    /// browsing the transcript to rewind: the highlighted message is known
    #[test]
    fn browsing_the_transcript_to_rewind_the_highlighted_message_is_known() {
        let cap = screen_fixture("codex", "rewind");
        assert_eq!(codex_browsing(&cap), Some(CodexBrowsing { selected: Some("> Colour".into()), screen: Some(cap.join("\n")) }));
        // not browsing: the composer is there as usual
        assert_eq!(codex_browsing(&screen_fixture("codex", "working-empty")), None);
        assert_eq!(parse_screen(&cap, Some(120), AgentKind::Codex).prompt, None);
    }

    /// a numbered list in a reply is not a prompt
    #[test]
    fn a_numbered_list_in_a_reply_is_not_a_prompt() {
        assert_eq!(parse_screen(&screen_fixture("codex", "reply-numbered-list"), Some(80), AgentKind::Codex).prompt, None);
    }
}

/// describe('Antigravity CLI (real captures, agy 1.3.3 at 150 columns)')
mod antigravity_cli {
    use super::*;

    fn cap(name: &str) -> Vec<String> {
        screen_fixture("antigravity", name)
    }
    fn agy(lines: &[String]) -> ScreenState {
        parse_screen(lines, Some(150), AgentKind::Antigravity)
    }

    /// idle
    #[test]
    fn idle() {
        assert_eq!(
            serde_json::to_value(agy(&cap("idle"))).unwrap(),
            json!({ "working": false, "width": 150, "model": "Gemini 3.8 Flash", "effort": "high", "mode": "default", "inputVisible": true })
        );
    }

    /// working: the spinner above the box, not the footer
    #[test]
    fn working_the_spinner_above_the_box_not_the_footer() {
        let s = agy(&cap("working"));
        assert!(s.working);
        assert_eq!(s.activity.as_deref(), Some("Generating"));
        assert_eq!(s.prompt, None);
        assert_eq!(s.draft, None);
    }

    /// a typed draft
    #[test]
    fn a_typed_draft() {
        assert_eq!(agy(&cap("draft")).draft.as_deref(), Some("fix the bug in main.go please"));
    }

    /// model and effort from the footer
    #[test]
    fn model_and_effort_from_the_footer() {
        let s = agy(&cap("approval"));
        assert_eq!((s.model.as_deref(), s.effort.as_deref()), (Some("Gemini 3.8 Flash"), Some("high")));
        assert_eq!(agy(&cap("plan")).model.as_deref(), Some("Gemini 3.8 Flash"));
    }

    /// mode from the footer
    #[test]
    fn mode_from_the_footer() {
        assert_eq!(agy(&cap("accept-edits")).mode.as_deref(), Some("accept-edits"));
        assert_eq!(agy(&cap("plan")).mode.as_deref(), Some("plan"));
        // the grey "Plan mode: … (shift+tab to cycle)" hint in the empty box is not a draft
        assert_eq!(agy(&cap("accept-edits")).draft, None);
        assert_eq!(agy(&cap("plan")).draft, None);
        assert_eq!(agy(&cap("idle")).mode.as_deref(), Some("default"));
    }

    /// the slash picker under the box is not a prompt
    #[test]
    fn the_slash_picker_under_the_box_is_not_a_prompt() {
        let s = agy(&cap("slash"));
        assert_eq!(s.prompt, None);
        assert!(!s.working);
        assert_eq!(s.draft.as_deref(), Some("/"));
    }

    /// command approval
    #[test]
    fn command_approval() {
        let p = agy(&cap("approval")).prompt.unwrap();
        assert_eq!(p.options.iter().map(|o| (o.n, o.selected)).collect::<Vec<_>>(), vec![(1, true), (2, false), (3, false), (4, false)]);
        assert_eq!(p.options[0].label, "Yes, run command");
        assert_eq!(p.options[3].label, "No, cancel");
        assert_eq!(p.navigate, Some(true));
        assert_eq!(p.body, vec!["Run this command?", "╌", "date +%Y", "╌"]);
    }

    /// approval whose long options wrap to column 0 in a narrow pane
    #[test]
    fn approval_whose_long_options_wrap_to_column_0_in_a_narrow_pane() {
        let p = parse_screen(&cap("approval-wrapped"), Some(115), AgentKind::Antigravity).prompt.unwrap();
        assert_eq!(p.options.iter().map(|o| o.n).collect::<Vec<_>>(), vec![1, 2, 3, 4]);
        assert!(p.options[2].label.contains("-maxdepth 3 -not -path '*/.*' -not -path"));
        assert!(p.options[2].label.ends_with("(Persist to settings.json)"));
        assert!(p.body[2].starts_with("find apps packages deploy"));
    }

    /// folder trust
    #[test]
    fn folder_trust() {
        let p = agy(&cap("trust")).prompt.unwrap();
        assert_eq!(p.options, vec![opt(1, "Yes, I trust this folder", true), opt(2, "No, exit", false)]);
        assert_eq!(p.navigate, Some(true));
        assert!(p.body.contains(&"Do you trust the contents of this project?".to_owned()));
    }

    /// queued messages while working
    #[test]
    fn queued_messages_while_working() {
        let s = agy(&cap("queued"));
        assert!(s.working);
        assert_eq!(s.activity.as_deref(), Some("Generating"));
        assert_eq!(s.queue, Some(vec!["Then say done.".to_owned()]));
    }

    /// wrapped queued message under a background task
    #[test]
    fn wrapped_queued_message_under_a_background_task() {
        let s = agy(&cap("queued-task"));
        assert!(s.working);
        assert_eq!(s.activity.as_deref(), Some("Running command"));
        assert_eq!(s.queue, Some(vec!["Then say done, and list every file in this folder with its size, newest first, so I can see what changed.".to_owned()]));
        assert_eq!(s.model.as_deref(), Some("Gemini 3.8 Flash"));
        assert_eq!(s.effort.as_deref(), Some("medium"));
        assert_eq!(s.background, Some(Some("1 background task".into())));
    }

    /// a folded thought is not a queued message
    #[test]
    fn a_folded_thought_is_not_a_queued_message() {
        let s = agy(&cap("folded-thought"));
        assert!(!s.working);
        assert_eq!(s.queue, None);
    }

    /// rewind picker: one option per message, its age as detail
    #[test]
    fn rewind_picker_one_option_per_message_its_age_as_detail() {
        let p = agy(&cap("rewind")).prompt.unwrap();
        assert_eq!(p.body, vec!["Rewind Conversation"]);
        assert_eq!(p.navigate, Some(true));
        let mut first = opt(1, "Then say done.", false);
        first.detail = Some("19m ago".into());
        assert_eq!(p.options[0], first);
        let last = p.options.last().unwrap();
        assert_eq!((last.label.as_str(), last.selected), ("(current)", true));
        assert!(!p.options.iter().any(|o| o.label.contains("items]")));
    }

    /// model picker: models only, without the search field or the effort slider
    #[test]
    fn model_picker_models_only_without_the_search_field_or_the_effort_slider() {
        let p = agy(&cap("model-menu")).prompt.unwrap();
        assert_eq!(p.body, vec!["Switch Model"]);
        let mut first = opt(1, "Gemini 3.8 Flash", true);
        first.detail = Some("current".into());
        assert_eq!(p.options[0], first);
        assert!(!labels(&p).contains(&"Search:"));
        assert!(!p.options.iter().any(|o| o.label.contains("Effort")));
        assert_eq!(p.options.len(), 7);
    }

    /// resume picker: table rows, not the heading or the search line
    #[test]
    fn resume_picker_table_rows_not_the_heading_or_the_search_line() {
        let p = agy(&cap("resume")).prompt.unwrap();
        assert_eq!(p.body, vec!["Conversations"]);
        let mut first = opt(1, "[CURRENT] Inyon QA run", true);
        first.detail = Some("27 steps · 8m ago".into());
        assert_eq!(p.options[0], first);
        assert_eq!((p.options[1].label.as_str(), p.options[1].detail.as_deref()), ("Simple Greeting", Some("custom-tmux · 47 steps · 23m ago")));
        assert_eq!(p.options.len(), 5);
    }

    /// remote control link, rejoined where the pane wrapped it, and off again
    #[test]
    fn remote_control_link_rejoined_where_the_pane_wrapped_it_and_off_again() {
        let on = fixture("screens/antigravity/remote-control.txt");
        assert_eq!(
            parse_screen(&screen(&on), Some(60), AgentKind::Antigravity).remote,
            Some(Some("https://antigravity.google.com/r/77637f21-f314-468d-9258-23b644a606f9-v2?p=c%2F5a9bbba1".into()))
        );
        let off = on.replacen("? for shortcuts", "> /remote-control off\n  ⎿  Remote control off. This machine is no longer reachable.\n? for shortcuts", 1);
        assert_eq!(parse_screen(&screen(&off), Some(60), AgentKind::Antigravity).remote, Some(None));
        assert_eq!(agy(&cap("idle")).remote, None);
    }

    /// approval options wrapped at a narrow width
    #[test]
    fn approval_options_wrapped_at_a_narrow_width() {
        let p = parse_screen(&cap("approval-narrow"), Some(60), AgentKind::Antigravity).prompt.unwrap();
        assert_eq!(labels(&p), vec!["Yes, run command", "Yes, and always allow in this conversation for commands that start with 'rm'", "No, cancel"]);
    }
}

/// describe('quiet foreground work')
mod quiet_foreground_work {
    use super::*;

    /// Codex spinner above delegation tip remains working
    #[test]
    fn codex_spinner_above_delegation_tip_remains_working() {
        let mut lines = screen_fixture("codex", "working-tip");
        matches(&parse_screen(&lines, Some(100), AgentKind::Codex), json!({ "working": true, "activity": "Working" }));
        lines.splice(0..2, ["• done".to_owned(), "  Worked for 2m 07s".to_owned()]);
        assert!(!parse_screen(&lines, Some(100), AgentKind::Codex).working);
    }

    /// Claude spinner survives long wrapped tool hints
    #[test]
    fn claude_spinner_survives_long_wrapped_tool_hints() {
        let mut lines = screen_fixture("claude", "working-tool-hints");
        assert!(claude(&lines, None).working);
        lines[0] = "● Command finished".into();
        matches(&claude(&lines, None), json!({ "working": false, "background": "1 shell" }));
    }

    /// Antigravity spinner survives live command output
    #[test]
    fn antigravity_spinner_survives_live_command_output() {
        let mut lines = screen_fixture("antigravity", "working-command-output");
        matches(&parse_screen(&lines, Some(100), AgentKind::Antigravity), json!({ "working": true, "activity": "Running command" }));
        lines[0] = "Command finished".into();
        assert!(!parse_screen(&lines, Some(100), AgentKind::Antigravity).working);
    }

    /// Codex does not reuse a prior turn spinner
    #[test]
    fn codex_does_not_reuse_a_prior_turn_spinner() {
        assert!(!parse_screen(&screen_fixture("codex", "finished-after-spinner"), Some(100), AgentKind::Codex).working);
    }
}

/// Codex reads a composer longer than fourteen terminal rows
#[test]
fn codex_reads_a_composer_longer_than_fourteen_terminal_rows() {
    let s = parse_screen(&screen_fixture("codex", "long-draft"), Some(100), AgentKind::Codex);
    assert_eq!(s.input_visible, Some(true));
    let draft = s.draft.unwrap();
    assert!(draft.contains("long draft"));
    assert_eq!(draft.matches("continued text").count(), 20);
}

/// Codex shell composer keeps the exclamation command prefix
#[test]
fn codex_shell_composer_keeps_the_exclamation_command_prefix() {
    let s = parse_screen(&screen_fixture("codex", "shell-composer"), Some(100), AgentKind::Codex);
    assert_eq!(s.input_visible, Some(true));
    assert_eq!(s.draft.as_deref(), Some("! printf hello"));
}

/// Codex's question to trust new hooks is buttons, without its banner
#[test]
fn codexs_question_to_trust_new_hooks_is_buttons_without_its_banner() {
    let p = parse_screen(&screen_fixture("codex", "hooks-review"), Some(120), AgentKind::Codex).prompt.unwrap();
    assert_eq!(p.body, vec!["Hooks need review", "2 hooks are new or changed.", "Hooks can run outside the sandbox after you trust them."]);
    assert_eq!(labels(&p), vec!["Review hooks", "Trust all and continue", "Continue without trusting (hooks won't run)"]);
}

/// a message from another session waiting to be let through is a question with buttons
#[test]
fn a_message_from_another_session_waiting_to_be_let_through_is_a_question_with_buttons() {
    let p = claude(&screen_fixture("claude", "held-peer-message"), Some(150)).prompt.unwrap();
    assert_eq!(p.navigate, Some(true));
    assert_eq!(
        p.options.iter().map(|o| (o.label.as_str(), o.detail.as_deref(), o.selected)).collect::<Vec<_>>(),
        vec![("Deny", Some("drop it and tell the sender it was declined"), true), ("Deliver this message to Claude", None, false)]
    );
    assert_eq!(p.body[0], "Held message from another session");
    assert!(p.body.contains(&"Test message from the Inyon development session: please reply with only".to_owned()));
    assert_eq!(p.body.iter().filter(|l| *l == "╌").count(), 2);
}

/// command prompt with a gutter keeps its lines and loses the gutter
#[test]
fn command_prompt_with_a_gutter_keeps_its_lines_and_loses_the_gutter() {
    let p = claude(&screen_fixture("claude", "bash-gutter"), Some(100)).prompt.unwrap();
    assert_eq!(
        p.body,
        vec![
            "Bash command",
            "Append empty-cart test and run the suite",
            "╌",
            "cat >> test/cart.test.js <<'EOF'",
            "",
            "test('empty cart costs nothing', () => {",
            "  assert.equal(subtotal([]), 0)",
            "})",
            "EOF",
            "╌",
            "Contains brace with quote character (expansion obfuscation)",
            "Ask rule Bash overrides auto mode for this command.",
            "/permissions to let auto mode decide",
            "Do you want to proceed?",
        ]
    );
    assert_eq!(p.whole_lines, Some(true));
    assert_eq!(labels(&p), vec!["Yes", "No"]);
    assert_eq!(prompt_summary(&p), "Append empty-cart test and run the suite: cat >> test/cart.test.js <<'EOF'");
    // the line leaves out what an answer agrees to: no quick answers
    assert!(!prompt_whole(&p));
}

/// a PowerShell prompt reads as what the command is for
#[test]
fn a_powershell_prompt_reads_as_what_the_command_is_for() {
    let p = claude(&screen_fixture("claude", "powershell-prompt"), Some(100)).prompt.unwrap();
    assert_eq!(p.body[3..], ["npm test", "╌", "This command requires approval", "Do you want to proceed?"]);
    assert_eq!(prompt_summary(&p), "Re-run the test suite: npm test");
    // the line is the whole question: quick answers are offered
    assert!(prompt_whole(&p));
}

/// lines Claude wrapped are joined, real line breaks stay
#[test]
fn lines_claude_wrapped_are_joined_real_line_breaks_stay() {
    let p = claude(&screen_fixture("claude", "switch-model"), Some(80)).prompt.unwrap();
    assert_eq!(
        p.body,
        vec![
            "Switch model?",
            "Your next response will be slower and use more tokens",
            "This conversation is cached for the current model. Switching to Sonnet 5.5 means the full history gets re-read on your next message.",
        ]
    );
    assert_eq!(prompt_summary(&p), "Switch model?");
    // the line is the whole question: quick answers are offered
    assert!(prompt_whole(&p));
}

/// an alert for a command names what it is for, not the tip
#[test]
fn an_alert_for_a_command_names_what_it_is_for_not_the_tip() {
    let p = claude(&screen_fixture("claude", "permission-tip"), None).prompt.unwrap();
    assert_eq!(prompt_summary(&p), "Write hello-inyon to out.txt: echo hello-inyon > out.txt");
    // the line is the whole question: quick answers are offered
    assert!(prompt_whole(&p));
}

/// an edit prompt names the file, not the first line of the diff
#[test]
fn an_edit_prompt_names_the_file_not_the_first_line_of_the_diff() {
    let p = claude(&screen_fixture("claude", "edit-prompt"), None).prompt.unwrap();
    assert_eq!(prompt_summary(&p), "Edit file: test\\cart.test.js");
    // the line leaves out what an answer agrees to: no quick answers
    assert!(!prompt_whole(&p));
}

/// describe('ultracode in Claude Code 2.1.299')
mod ultracode_in_claude_code_2_1_299 {
    use super::*;

    // captured with `claude --effort ultracode`: the box's top rule carries the label, and the
    // effort line above the box names it too
    fn bx(rule: &str, above: &str) -> Vec<String> {
        screen(&format!(
            "❯ /clear\n{above}\n{rule}\n❯\n{}\n  ⚠ Transcript saving is off — inherited CLAUDE_CODE_CHILD_SESSION marker · restart with CLAUDE_CODE_FORCE_…\n  ⏵⏵ auto mode on (shift+tab to cycle)",
            r100()
        ))
    }
    fn labelled() -> String {
        format!("{} ultracode ─", "─".repeat(98))
    }

    /// on: the rule says so, and the effort line names the level
    #[test]
    fn on_the_rule_says_so_and_the_effort_line_names_the_level() {
        let s = claude(&bx(&labelled(), "               ◉ xhigh · ultracode · /effort · current: 2.1.299 · lates… ✔ Update installed · Restart to up…"), Some(110));
        matches(&s, json!({ "inputVisible": true, "ultracode": true, "effort": "xhigh", "mode": "auto", "working": false }));
        assert_eq!(s.draft, None);
        // the label alone is enough (the effort line comes and goes)
        let bare = claude(&bx(&labelled(), "                                    0 tokens · current: 2.1.299 · lates… ✔ Update installed · Restart to up…"), Some(110));
        assert_eq!(bare.ultracode, Some(true));
        assert_eq!(bare.effort, None);
    }

    /// off: a bare rule, with or without the effort line; a renamed session is not ultracode
    #[test]
    fn off_a_bare_rule_with_or_without_the_effort_line_a_renamed_session_is_not_ultracode() {
        matches(
            &claude(&bx(&r100(), "                           ◉ xhigh · /effort · current: 2.1.299 · lates… ✔ Update installed · Restart to up…"), Some(110)),
            json!({ "ultracode": false, "effort": "xhigh" }),
        );
        matches(&claude(&bx(&r100(), ""), Some(110)), json!({ "ultracode": false }));
        matches(&claude(&bx(&format!("{} Abhash Website ─", "─".repeat(90)), ""), Some(110)), json!({ "ultracode": false, "inputVisible": true }));
    }

    /// unknown while a dialog covers the box
    #[test]
    fn unknown_while_a_dialog_covers_the_box() {
        assert_eq!(claude(&screen_fixture("claude", "effort-dialog"), Some(100)).ultracode, None);
    }
}

/// describe('background agents in the Claude footer')
mod background_agents_in_the_claude_footer {
    use super::*;

    // the ← is the hint to expand the agent list; the two rows under it are that list, opened
    fn footer(status: &str, rows: &[&str]) -> Vec<String> {
        let rows: String = rows.iter().map(|r| format!("\n  {r}")).collect();
        screen(&format!("● Started the agent in the background.\n{0}\n❯\n{0}\n  {status}{rows}", r100()))
    }
    const AGENTS: [&str; 2] = ["● main", "◯ general-purpose  Sleep briefly for a footer test                                                      6s · ↓ 37.9k tokens"];

    fn bg(status: &str) -> Option<Option<String>> {
        claude(&footer(status, &[]), Some(100)).background
    }

    /// a shell, with bypass mode
    #[test]
    fn a_shell_with_bypass_mode() {
        let s = claude(&footer("⏵⏵ bypass permissions on · 1 shell · ← 1 agent", &AGENTS), Some(100));
        matches(&s, json!({ "working": false, "inputVisible": true, "mode": "bypass permissions", "background": "1 shell" }));
        assert_eq!(s.draft, None);
        assert_eq!(s.prompt, None);
    }

    /// agents alone, without the mode segment
    #[test]
    fn agents_alone_without_the_mode_segment() {
        assert_eq!(bg("1 shell · 1 agent"), Some(Some("1 shell, 1 agent".into())));
    }

    /// the agents view's count is the machine's other sessions, not this pane's work: every Claude
    /// pane on a machine with an old background session looked busy, so held messages never went in
    #[test]
    fn the_agents_views_count_is_the_machines_other_sessions_not_this_panes_work() {
        assert_eq!(bg("← 1 agent"), Some(None));
        let rows = [AGENTS[0], AGENTS[1], "◯ Explore  Find the parser  12s · ↓ 4.1k tokens"];
        matches(&claude(&footer("← 2 agents", &rows), Some(100)), json!({ "working": false, "background": null }));
        // Claude Code 2.1.311 on Windows, idle and fresh, with one old session in its agents view
        let s = claude(&screen_fixture("claude", "idle-update-ready"), None);
        matches(&s, json!({ "working": false, "inputVisible": true, "mode": "auto", "background": null }));
        assert!(!inyon_protocol::is_busy(inyon_protocol::AgentState::Idle, s.background.flatten().as_deref()));
    }

    /// the agent list rows are not read as a footer
    #[test]
    fn the_agent_list_rows_are_not_read_as_a_footer() {
        let s = claude(&footer("⏵ plan mode on · ← 1 agent", &["● main", "◯ general-purpose  Start 2 shells, then ⏵⏵ bypass permissions on  6s · ↓ 1.2k tokens"]), Some(100));
        matches(&s, json!({ "mode": "plan", "background": null }));
    }

    /// nothing in the background once the footer is bare, which is not the same as no footer
    #[test]
    fn nothing_in_the_background_once_the_footer_is_bare_which_is_not_the_same_as_no_footer() {
        assert_eq!(bg("⏵⏵ bypass permissions on"), Some(None));
        // a picker over the input box: the footer can't be read, so nothing is known
        assert_eq!(claude(&screen_fixture("claude", "effort-dialog"), Some(100)).background, None);
    }

    /// every kind of background work Claude Code 2.1.299 names
    #[test]
    fn every_kind_of_background_work_claude_code_2_1_299_names() {
        assert_eq!(bg("⏵⏵ bypass permissions on · 1 local agent"), Some(Some("1 local agent".into())));
        assert_eq!(bg("⏵⏵ auto mode on (shift+tab to cycle) · 1 background dynamic workflow"), Some(Some("1 background dynamic workflow".into())));
        assert_eq!(bg("1 MCP task · 1 Artifact comment monitor · 1 cloud session"), Some(Some("1 MCP task, 1 Artifact comment monitor, 1 cloud session".into())));
        assert_eq!(bg("⏵⏵ bypass permissions on · 1 shell · /tasks to see subagents · ← 1 agent"), Some(Some("1 shell".into())));
        assert_eq!(bg("3 background tasks"), Some(Some("3 background tasks".into())));
    }
}

/// Claude Code 2.1.311's folder trust question (on Windows): no numbers, and "No, exit" first and
/// highlighted, so a choice has to be picked by moving to it, never by its place
#[test]
fn the_folder_trust_question_without_numbers_has_no_exit_first() {
    let s = claude(&screen_fixture("claude", "trust-folder"), None);
    let p = s.prompt.expect("a question");
    let options: Vec<(u64, &str, bool)> = p.options.iter().map(|o| (o.n, o.label.as_str(), o.selected)).collect();
    assert_eq!(options, vec![(1, "No, exit", true), (2, "Yes, I trust this folder", false)]);
    assert_eq!(p.navigate, Some(true));
    assert!(p.body.iter().any(|l| l.starts_with("Quick safety check")), "{:?}", p.body);
}
