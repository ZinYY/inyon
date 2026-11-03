//! agents/opencode-models.test.ts, test for test. Each test's name in the TS file is its doc
//! comment.

use super::*;
use crate::claude::delivery::input_contains;

/// the shape `opencode models --verbose` prints; headers and options stand in for secrets
const OUTPUT: &str = r#"opencode/big-pickle
{
  "id": "big-pickle",
  "providerID": "opencode",
  "name": "Big Pickle",
  "headers": { "x-secret": "never-keep-this" },
  "options": { "apiKey": "never-keep-this" },
  "limit": { "context": 200000, "output": 32000 },
  "status": "active",
  "variants": {}
}
opencode/ling-3.1-flash-free
{
  "id": "ling-3.1-flash-free",
  "name": "Ling 3.1 Flash Free",
  "limit": { "context": 262144 },
  "variants": { "low": { "reasoningEffort": "low" }, "medium": {}, "high": { "disabled": true } }
}
anthropic/old-model
{
  "name": "Old",
  "status": "deprecated",
  "limit": { "context": 1 }
}
"#;

fn model(id: &str, label: &str) -> AgentModel {
    AgentModel { id: id.into(), label: label.into(), note: None, context: None, efforts: None, default_effort: None, variants: None }
}

fn id(m: Option<&AgentModel>) -> Option<&str> {
    m.map(|m| m.id.as_str())
}

/// describe('OpenCode models')
mod opencode_models {
    use super::*;

    /// keeps id, name, context and variants, nothing else
    #[test]
    fn keeps_id_name_context_and_variants_nothing_else() {
        let models = parse_open_code_models(OUTPUT);
        assert_eq!(
            models,
            vec![
                AgentModel { context: Some(inyon_protocol::Num(200000.0)), efforts: Some(vec![]), ..model("opencode/big-pickle", "OpenCode · Big Pickle") },
                AgentModel {
                    context: Some(inyon_protocol::Num(262144.0)),
                    efforts: Some(vec!["low".into(), "medium".into()]),
                    ..model("opencode/ling-3.1-flash-free", "OpenCode · Ling 3.1 Flash Free")
                },
            ]
        );
        assert!(!serde_json::to_string(&models).unwrap().contains("never-keep-this"));
    }

    /// the footer names the model and its provider (1.19 says "OpenCode", older versions "OpenCode Zen")
    #[test]
    fn the_footer_names_the_model_and_its_provider_1_19_says_opencode_older_versions_opencode_zen() {
        let models = parse_open_code_models(OUTPUT);
        assert_eq!(id(model_from_footer(&models, "Ling 3.1 Flash Free OpenCode")), Some("opencode/ling-3.1-flash-free"));
        assert_eq!(id(model_from_footer(&models, "Big Pickle OpenCode Zen")), Some("opencode/big-pickle"));
        assert_eq!(id(model_from_footer(&models, "Something Else")), None);
        assert_eq!(without_provider("Big Pickle OpenCode"), "Big Pickle");
        assert_eq!(without_provider("Big Pickle OpenCode Zen"), "Big Pickle");
        assert_eq!(footer_model_name(Some(&models), "Ling 3.1 Flash Free OpenCode"), "Ling 3.1 Flash Free");
        assert_eq!(footer_model_name(None, "Mystery Model Anthropic"), "Mystery Model");
    }

    /// two providers offering the same model: the one the footer ends with
    #[test]
    fn two_providers_offering_the_same_model_the_one_the_footer_ends_with() {
        let models = [model("openai/gpt-5", "OpenAI · GPT-5"), model("openrouter/openai/gpt-5", "OpenRouter · GPT-5"), model("openai/gpt-5.1-codex", "OpenAI · GPT-5.1 Codex")];
        assert_eq!(id(model_from_footer(&models, "GPT-5 OpenRouter")), Some("openrouter/openai/gpt-5"));
        assert_eq!(id(model_from_footer(&models, "GPT-5 OpenAI")), Some("openai/gpt-5"));
        assert_eq!(id(model_from_footer(&models, "GPT-5.1 Codex OpenAI")), Some("openai/gpt-5.1-codex"));
    }

    /// provider names a machine reports are used from then on
    #[test]
    fn provider_names_a_machine_reports_are_used_from_then_on() {
        note_provider_names([("acme-cloud", "Acme Cloud")]);
        assert_eq!(without_provider("Some Model Acme Cloud"), "Some Model");
    }

    /// `opencode agent list` gives the agents a session can switch to
    #[test]
    fn opencode_agent_list_gives_the_agents_a_session_can_switch_to() {
        let out = "build (primary)\n  [\n  {\n    \"permission\": \"*\"\n  }\n  ]\nplan (primary)\n  []\nexplore (subagent)\n  []\nreviewer (all)\n  []\ntitle (primary)\n  []\n";
        let mode = |id: &str, label: &str| ModeOption { id: id.into(), label: label.into() };
        assert_eq!(parse_open_code_agents(out), vec![mode("build", "Build"), mode("plan", "Plan"), mode("reviewer", "Reviewer")]);
        assert_eq!(parse_open_code_agents(""), vec![]);
    }

    /// a multi-line paste shows as its line count, which counts only for text just put there
    #[test]
    fn a_multi_line_paste_shows_as_its_line_count_which_counts_only_for_text_just_put_there() {
        assert!(input_contains(Some("[Pasted ~3 lines]"), "one\ntwo\nthree", true));
        assert!(!input_contains(Some("[Pasted ~3 lines]"), "one\ntwo", true));
        // an older paste with as many lines is a different message
        assert!(!input_contains(Some("[Pasted ~3 lines]"), "one\ntwo\nthree", false));
    }
}

#[test]
fn unknown_providers_are_named_from_their_id_and_the_catalog_reads_both_lists() {
    assert_eq!(provider_name("acme-labs-x"), "Acme Labs X");
    assert_eq!(provider_name("xai"), "xAI");
    let cat = opencode_cli_catalog(&format!("{OUTPUT}@@AGENTS\nbuild (primary)\n"), 7);
    assert_eq!((cat.models.len(), cat.modes.map(|m| m.len()), cat.updated_at), (2, Some(1), 7));
    assert!(opencode_cli_catalog("", 7).error.is_some());
    assert!(opencode_cli_models_command().contains("opencode agent list"));
}

#[test]
fn a_provider_name_learned_on_one_thread_is_known_on_every_other() {
    std::thread::spawn(|| note_provider_names([("thread-cloud-9", "Thread Cloud Nine")])).join().unwrap();
    assert_eq!(provider_name("thread-cloud-9"), "Thread Cloud Nine");
    assert_eq!(std::thread::spawn(|| without_provider("Some Model Thread Cloud Nine")).join().unwrap(), "Some Model");
}

#[test]
fn crlf_output_reads_like_lf_output() {
    // JavaScript's multiline ^ and $ break at "\r" too, so a CRLF listing still splits per model
    let crlf = OUTPUT.replace('\n', "\r\n");
    let ids = |text: &str| parse_open_code_models(text).into_iter().map(|m| m.id).collect::<Vec<_>>();
    assert_eq!(ids(&crlf), ["opencode/big-pickle", "opencode/ling-3.1-flash-free"]);
    assert_eq!(ids(&crlf), ids(OUTPUT));
    let agents = parse_open_code_agents("build (primary)\r\n  []\r\nplan (primary)\r\n");
    assert_eq!(agents.len(), 2);
}

#[test]
fn a_context_size_passes_through_as_the_number_it_is() {
    let out = "x/a\n{ \"limit\": { \"context\": 1.5 } }\nx/b\n{ \"limit\": { \"context\": -1 } }\n";
    let got: Vec<_> = parse_open_code_models(out).into_iter().map(|m| m.context).collect();
    assert_eq!(got, [Some(inyon_protocol::Num(1.5)), Some(inyon_protocol::Num(-1.0))]);
}
