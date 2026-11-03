//! features/providers.test.ts

use super::*;
use crate::test_support::fake::{ok, Fake};
use crate::test_support::machine;
use crate::test_support::rt::run;

// what `opencode auth list` prints (1.19), colours and all
const LIST: &str = "\x1b[0m
\x1b[90m┌\x1b[39m  Credentials \x1b[90m/home/me/.local/share/opencode/auth.json\x1b[39m
\x1b[90m│\x1b[39m
\x1b[32m●\x1b[39m  GitHub Copilot \x1b[90moauth\x1b[39m
\x1b[90m│\x1b[39m
\x1b[32m●\x1b[39m  Anthropic api
\x1b[90m│\x1b[39m
\x1b[90m└\x1b[39m  2 credentials

\x1b[90m┌\x1b[39m  Environment
\x1b[90m│\x1b[39m
\x1b[32m●\x1b[39m  Groq \x1b[90mGROQ_API_KEY\x1b[39m
\x1b[90m│\x1b[39m
\x1b[90m└\x1b[39m  1 environment variable
";

fn catalogue() -> Catalogue {
    catalogue_from(&serde_json::json!({
        "connected": ["opencode", "anthropic", "github-copilot", "groq"],
        "all": [
            { "id": "opencode", "name": "OpenCode", "env": ["OPENCODE_API_KEY"], "key": "SECRET" },
            { "id": "anthropic", "name": "Anthropic", "env": ["ANTHROPIC_API_KEY"], "key": "SECRET" },
            { "id": "github-copilot", "name": "GitHub Copilot", "env": ["GITHUB_TOKEN"] },
            { "id": "groq", "name": "Groq", "env": ["GROQ_API_KEY"] },
            { "id": "zai", "name": "Z.AI", "env": ["ZHIPU_API_KEY"] },
            { "id": "openai", "name": "OpenAI", "env": ["OPENAI_API_KEY"] },
            { "id": "bad id", "name": "Nope", "env": [] },
        ],
    }))
}

#[test]
fn opencode_auth_list_is_read_as_credentials_and_environment_variables() {
    assert_eq!(
        serde_json::to_value(parse_auth_list(LIST)).unwrap(),
        serde_json::json!({
            "credentials": [{ "name": "GitHub Copilot", "type": "oauth" }, { "name": "Anthropic", "type": "api" }],
            "env": [{ "name": "Groq", "variable": "GROQ_API_KEY" }],
        })
    );
    assert_eq!(parse_auth_list("┌  Credentials x\n│\n└  0 credentials\n"), AuthList::default());
}

#[test]
fn the_catalogue_keeps_ids_names_and_variables_never_keys_and_drops_ids_a_shell_could_misread() {
    let cat = catalogue();
    assert!(!serde_json::to_string(&cat).unwrap().contains("SECRET"));
    assert_eq!(cat.providers.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), vec!["opencode", "anthropic", "github-copilot", "groq", "zai", "openai"]);
}

#[test]
fn connected_ones_first_with_how_they_connected_then_the_popular_ones_then_the_rest_by_name() {
    let list = merge_providers(&catalogue(), &parse_auth_list(LIST));
    let got: Vec<(&str, bool, Option<ProviderVia>, Option<&str>)> = list.iter().map(|p| (p.id.as_str(), p.connected, p.via, p.r#type.as_deref())).collect();
    assert_eq!(
        got,
        vec![
            ("anthropic", true, Some(ProviderVia::Credential), Some("api")),
            ("github-copilot", true, Some(ProviderVia::Credential), Some("oauth")),
            ("groq", true, Some(ProviderVia::Env), None),
            ("opencode", true, None, None),
            ("openai", false, None, None),
            ("zai", false, None, None),
        ]
    );
    assert_eq!(list.iter().find(|p| p.id == "groq").unwrap().env, Some(vec!["GROQ_API_KEY".to_owned()]));
    // a credential for a provider the catalogue doesn't list still shows
    let own = merge_providers(&Catalogue::default(), &AuthList { credentials: vec![AuthCredential { name: "My Proxy".into(), kind: "api".into() }], env: vec![] });
    assert_eq!(serde_json::to_value(own).unwrap(), serde_json::json!([{ "id": "my-proxy", "name": "My Proxy", "connected": true, "via": "credential", "type": "api" }]));
}

#[test]
fn disconnecting_runs_opencode_auth_logout_and_a_provider_from_the_environment_says_how_to_disconnect_it() {
    run(|| async {
        machine("h", "local", serde_json::json!([])).borrow_mut().hold_polls();
        let f = Fake::install(|_, cmd, _| ok(if cmd.contains("auth logout") { "┌  Remove credential\n│\n└  Logout successful\n" } else { LIST }));
        disconnect_provider("h", "anthropic").await.unwrap();
        assert!(f.cmds()[0].contains("opencode auth logout 'anthropic'"));
        Fake::install(|_, cmd, _| ok(if cmd.contains("auth logout") { "┌  Remove credential\n│\n■  No credentials found\n" } else { LIST }));
        assert!(disconnect_provider("h", "groq").await.unwrap_err().message.contains("GROQ_API_KEY"));
        assert!(disconnect_provider("h", "openai").await.unwrap_err().message.contains("no saved credential"));
        assert_eq!(disconnect_provider("h", "bad id").await.unwrap_err(), HubError::http(400, "Unknown provider"));
    });
}

#[test]
fn the_list_comes_back_with_an_error_line_instead_of_failing_when_opencodes_server_cant_start() {
    run(|| async {
        machine("h", "local", serde_json::json!([])).borrow_mut().hold_polls();
        Fake::install(|_, cmd, _| ok(if cmd.contains("auth list") { "" } else { "command not found" }));
        let r = list_providers("h", None).await.unwrap();
        assert_eq!(r.providers, vec![]);
        assert!(r.error.unwrap().contains("couldn't be read"));
    });
}

// ---------- beyond the TS tests ----------

#[test]
fn the_catalogue_is_read_once_in_ten_minutes_and_a_failure_is_not_kept() {
    run(|| async {
        machine("h", "local", serde_json::json!([])).borrow_mut().hold_polls();
        let body = serde_json::json!({ "all": [{ "id": "openai", "name": "OpenAI", "env": [] }], "connected": [] }).to_string();
        let f = Fake::install(move |_, cmd, _| ok(if cmd.contains("opencode serve") { &body } else { "" }));
        let serves = || f.cmds().iter().filter(|c| c.contains("opencode serve")).count();
        assert_eq!(list_providers("h", None).await.unwrap().providers.len(), 1);
        list_providers("h", None).await.unwrap();
        assert_eq!(serves(), 1);
        after_credential_change("h").await.unwrap();
        list_providers("h", None).await.unwrap();
        assert_eq!(serves(), 2);

        machine("h2", "local", serde_json::json!([])).borrow_mut().hold_polls();
        let g = Fake::install(|_, _, _| ok("nope"));
        assert!(list_providers("h2", None).await.unwrap().error.is_some());
        crate::test_support::rt::wait(0).await;
        list_providers("h2", None).await.unwrap();
        assert_eq!(g.cmds().iter().filter(|c| c.contains("opencode serve")).count(), 2);
    });
}

#[test]
fn connecting_checks_the_provider_id_first() {
    run(|| async {
        assert_eq!(connect_provider("h", "rm -rf").await.unwrap_err(), HubError::http(400, "Unknown provider"));
    });
}
