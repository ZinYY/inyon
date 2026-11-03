//! agents/opencode-questions.test.ts, test for test. Each test's name in the TS file is its doc
//! comment.

use super::*;
use serde_json::json;

fn tabs() -> OpenCodeQuestion {
    serde_json::from_value(json!({ "question": "Do you prefer tabs or spaces?", "header": "Indentation", "options": [{ "label": "Tabs", "description": "Indent with tab characters" }, { "label": "Spaces", "description": "Indent with spaces" }] })).unwrap()
}

fn colours() -> OpenCodeQuestion {
    serde_json::from_value(json!({ "question": "Which colours do you like?", "header": "Colours", "options": [{ "label": "Red", "description": "Red" }, { "label": "Green", "description": "Green" }, { "label": "Blue", "description": "Blue" }], "multiple": true })).unwrap()
}

fn strs(v: &[&[&str]]) -> Vec<Vec<String>> {
    v.iter().map(|a| a.iter().map(|s| s.to_string()).collect()).collect()
}

fn tab(label: &str, done: bool, current: bool) -> PromptTab {
    PromptTab { label: label.into(), done, current }
}

/// describe("OpenCode's questions through its API")
mod opencodes_questions_through_its_api {
    use super::*;

    /// one question: the options, a free-text choice, and a pick that ends it
    #[test]
    fn one_question_the_options_a_free_text_choice_and_a_pick_that_ends_it() {
        let mut flow = OpenCodeQuestionFlow::new("que_1", vec![tabs()]).unwrap();
        let p = flow.prompt();
        assert_eq!(p.body, ["Do you prefer tabs or spaces?"]);
        let rows: Vec<(u64, &str, Option<&str>)> = p.options.iter().map(|o| (o.n, o.label.as_str(), o.detail.as_deref())).collect();
        assert_eq!(rows, [(1, "Tabs", Some("Indent with tab characters")), (2, "Spaces", Some("Indent with spaces")), (3, "Type your own answer", None)]);
        assert_eq!(p.free_text_option, Some(3));
        assert_eq!(p.tabs, None);
        assert!(!p.multi);
        assert_eq!(p.api, Some(true));
        assert_eq!(flow.answer(2, None), Ok(true));
        assert_eq!(flow.answers(), strs(&[&["Spaces"]]));
        assert!(flow.done());
        assert!(flow.answer(1, None).unwrap_err().contains("answered already"));
    }

    /// a typed answer, and no free text when the question forbids it
    #[test]
    fn a_typed_answer_and_no_free_text_when_the_question_forbids_it() {
        let mut flow = OpenCodeQuestionFlow::new("que_1", vec![tabs()]).unwrap();
        assert!(flow.answer(3, None).unwrap_err().contains("Type an answer first"));
        assert_eq!(flow.answer(3, Some("  two spaces ")), Ok(true));
        assert_eq!(flow.answers(), strs(&[&["two spaces"]]));
        let mut strict = OpenCodeQuestionFlow::new("que_2", vec![OpenCodeQuestion { custom: Some(false), ..tabs() }]).unwrap();
        assert_eq!(strict.prompt().free_text_option, None);
        assert!(strict.answer(3, None).unwrap_err().contains("not in the question"));
    }

    /// checkboxes toggle and Confirm sends them; the tab strip follows along
    #[test]
    fn checkboxes_toggle_and_confirm_sends_them_the_tab_strip_follows_along() {
        let mut flow = OpenCodeQuestionFlow::new("que_1", vec![tabs(), colours()]).unwrap();
        assert_eq!(flow.prompt().tabs, Some(vec![tab("Indentation", false, true), tab("Colours", false, false)]));
        assert_eq!(flow.answer(1, None), Ok(false));
        let p = flow.prompt();
        assert_eq!(p.tabs, Some(vec![tab("Indentation", true, false), tab("Colours", false, true)]));
        assert!(p.multi);
        assert_eq!(p.confirm.as_deref(), Some("Confirm"));
        assert_eq!(p.options.iter().map(|o| o.checked).collect::<Vec<_>>(), [Some(false); 4]);
        assert!(flow.answer(CONFIRM_OPTION, None).unwrap_err().contains("Pick at least one"));
        for n in [1, 3, 1, 2] {
            flow.answer(n, None).unwrap();
        }
        let p = flow.prompt();
        let rows: Vec<(&str, Option<bool>)> = p.options.iter().map(|o| (o.label.as_str(), o.checked)).collect();
        assert_eq!(rows, [("Red", Some(false)), ("Green", Some(true)), ("Blue", Some(true)), ("Type your own answer", Some(false))]);
        assert_eq!(flow.answer(CONFIRM_OPTION, None), Ok(true));
        assert_eq!(flow.answers(), strs(&[&["Tabs"], &["Green", "Blue"]]));
    }

    /// a checkbox question that is not the last says Next, and a typed answer joins the ticks
    #[test]
    fn a_checkbox_question_that_is_not_the_last_says_next_and_a_typed_answer_joins_the_ticks() {
        let mut flow = OpenCodeQuestionFlow::new("que_1", vec![colours(), tabs()]).unwrap();
        assert_eq!(flow.prompt().confirm.as_deref(), Some("Next"));
        flow.answer(4, Some("Teal")).unwrap();
        flow.answer(1, None).unwrap();
        assert_eq!(flow.prompt().options[3].checked, Some(true));
        assert_eq!(flow.answer(CONFIRM_OPTION, None), Ok(false));
        assert_eq!(flow.answers()[0], ["Red", "Teal"]);
        assert!(flow.answer(CONFIRM_OPTION, None).unwrap_err().contains("one answer"));
        assert_eq!(flow.answer(2, None), Ok(true));
        assert_eq!(flow.answers(), strs(&[&["Red", "Teal"], &["Spaces"]]));
    }

    /// a request with no questions is refused
    #[test]
    fn a_request_with_no_questions_is_refused() {
        assert!(OpenCodeQuestionFlow::new("que_0", vec![]).is_err());
    }
}
