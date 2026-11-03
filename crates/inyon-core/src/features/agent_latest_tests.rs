//! features/agent-latest.test.ts

use std::cell::RefCell;
use std::rc::Rc;

use futures_util::future::LocalBoxFuture;

use super::*;
use crate::clock::set_now;
use crate::platform::fetch::{use_fetcher, FetchResponse, Fetcher};
use crate::test_support::rt::run;

/// a registry that answers with the given version (or fails), counting the calls
#[derive(Default)]
struct Registry {
    version: Option<String>,
    status: Option<u16>,
    fail: bool,
    calls: RefCell<Vec<String>>,
}

impl Fetcher for Registry {
    fn fetch(&self, req: FetchRequest) -> LocalBoxFuture<'static, Result<FetchResponse, String>> {
        self.calls.borrow_mut().push(req.url.clone());
        // the request always carries a timeout
        assert_eq!(req.timeout_ms, 4000);
        assert!(req.headers.contains(&("accept".into(), "application/json".into())));
        let answer = if self.fail {
            Err("fetch failed".to_owned())
        } else {
            let body = match &self.version {
                Some(v) => serde_json::json!({ "version": v }),
                None => serde_json::json!({}),
            };
            Ok(FetchResponse { status: self.status.unwrap_or(200), body: body.to_string().into_bytes() })
        };
        Box::pin(async move {
            crate::platform::rt::yield_now().await;
            answer
        })
    }
}

fn registry(version: Option<&str>, status: Option<u16>, fail: bool) -> Rc<Registry> {
    let r = Rc::new(Registry { version: version.map(str::to_owned), status, fail, ..Default::default() });
    use_fetcher(Some(r.clone()));
    forget_latest();
    r
}

#[test]
fn reads_the_digits_out_of_what_each_agent_prints_for_version() {
    assert_eq!(parse_version(Some("2.1.296 (Claude Code)")), Some(vec![2.0, 1.0, 296.0]));
    assert_eq!(parse_version(Some("codex-cli 0.49.0")), Some(vec![0.0, 49.0, 0.0]));
    assert_eq!(parse_version(Some("GitHub Copilot CLI 1.0.95.")), Some(vec![1.0, 0.0, 95.0]));
    assert_eq!(parse_version(Some("v1.18.35")), Some(vec![1.0, 18.0, 35.0]));
    assert_eq!(parse_version(Some("")), None);
    assert_eq!(parse_version(Some("not installed")), None);
}

#[test]
fn compares_versions_by_number_not_by_text() {
    assert_eq!(compare_versions(Some("0.49.0"), Some("0.50.0")), -1);
    assert_eq!(compare_versions(Some("0.162.1"), Some("0.49.0")), 1);
    assert_eq!(compare_versions(Some("2.1.296 (Claude Code)"), Some("v2.1.296")), 0);
    assert_eq!(compare_versions(Some("1.2"), Some("1.2.0")), 0);
    assert_eq!(compare_versions(Some("1.2"), Some("1.2.1")), -1);
    // an unreadable side never counts as outdated
    assert_eq!(compare_versions(Some(""), Some("1.0.0")), 0);
    assert_eq!(compare_versions(Some("1.0.0"), None), 0);
}

#[test]
fn asks_the_registry_once_an_hour_per_package() {
    run(|| async {
        let r = registry(Some("0.50.0"), None, false);
        let mut t = 1_000_000;
        set_now(Some(t));
        assert_eq!(latest_npm_version("@openai/codex").await.as_deref(), Some("0.50.0"));
        assert_eq!(latest_npm_version("@openai/codex").await.as_deref(), Some("0.50.0"));
        assert_eq!(*r.calls.borrow(), vec!["https://registry.npmjs.org/@openai/codex/latest"]);
        t += 59 * 60_000;
        set_now(Some(t));
        latest_npm_version("@openai/codex").await;
        assert_eq!(r.calls.borrow().len(), 1);
        t += 2 * 60_000;
        set_now(Some(t));
        latest_npm_version("@openai/codex").await;
        assert_eq!(r.calls.borrow().len(), 2);
        set_now(None);
    });
}

#[test]
fn a_failed_lookup_is_unknown_and_not_retried_for_a_few_minutes() {
    run(|| async {
        let r = registry(None, None, true);
        let mut t = 1_000_000;
        set_now(Some(t));
        assert_eq!(latest_npm_version("opencode-ai").await, None);
        assert_eq!(latest_npm_version("opencode-ai").await, None);
        assert_eq!(r.calls.borrow().len(), 1);
        t += 6 * 60_000;
        set_now(Some(t));
        latest_npm_version("opencode-ai").await;
        assert_eq!(r.calls.borrow().len(), 2);
        set_now(None);
    });
}

#[test]
fn an_error_status_or_an_odd_body_is_unknown_too_never_a_failure() {
    run(|| async {
        registry(None, Some(503), false);
        assert_eq!(latest_npm_version("a").await, None);
        registry(Some("latest"), None, false);
        assert_eq!(latest_npm_version("b").await, None);
    });
}

#[test]
fn two_callers_at_once_share_one_request() {
    run(|| async {
        let r = registry(Some("1.0.0"), None, false);
        let (a, b) = futures_util::future::join(latest_npm_version("@github/copilot"), latest_npm_version("@github/copilot")).await;
        assert_eq!((a.as_deref(), b.as_deref()), (Some("1.0.0"), Some("1.0.0")));
        assert_eq!(r.calls.borrow().len(), 1);
    });
}

#[test]
fn looks_every_agent_with_a_package_up_in_parallel_and_skips_the_ones_without() {
    run(|| async {
        let r = registry(Some("9.9.9"), None, false);
        let all = latest_agent_versions().await;
        let got: Vec<(&str, &str)> = all.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let kinds: Vec<&str> = got
            .iter()
            .map(|(k, v)| {
                assert_eq!(*v, "9.9.9");
                *k
            })
            .collect();
        assert_eq!(kinds, ["claude", "codex", "opencode", "copilot", "gemini", "pi", "amp", "kimi", "qwen", "qoder", "codebuddy"]);
        assert!(r.calls.borrow().contains(&"https://registry.npmjs.org/@anthropic-ai/claude-code/latest".to_owned()));
        assert!(r.calls.borrow().contains(&"https://registry.npmjs.org/@ampcode/cli/latest".to_owned()));
        // no npm package: Antigravity and the agents that only ship an installer
        assert!(!r.calls.borrow().iter().any(|u| ["antigravity", "cursor", "grok", "kiro"].iter().any(|n| u.contains(n))));
    });
}

#[test]
fn without_a_network_every_lookup_is_unknown() {
    run(|| async {
        use_fetcher(None);
        forget_latest();
        assert_eq!(latest_npm_version("@openai/codex").await, None);
        assert!(latest_agent_versions().await.is_empty());
    });
}
