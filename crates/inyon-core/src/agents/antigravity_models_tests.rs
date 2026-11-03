//! agents/antigravity-models.test.ts

use std::cell::Cell;
use std::rc::Rc;

use super::*;
use crate::hosts::host::ExecResult;
use crate::test_support::fake::{ok, Fake};
use crate::test_support::host;
use crate::test_support::rt::run;

const CATALOG: &str = "Fetching available models...\ngemini-3.1-pro-high\tGemini 3.1 Pro (High)\ngemini-3.1-pro-low\tGemini 3.1 Pro (Low)\nclaude-opus-4-6-thinking\tClaude Opus 4.6 (Thinking)\ngpt-oss-120b-medium\tGPT-OSS 120B (Medium)\n";

#[test]
fn catalog_groups_advertised_gemini_variants_without_inventing_unsupported_efforts() {
    let models = parse_agy_models(CATALOG);
    assert_eq!(models.len(), 3);
    assert_eq!(models[0].id, "gemini-3.1-pro");
    assert_eq!(models[0].efforts, Some(vec!["low".to_owned(), "high".to_owned()]));
    let v = models[0].variants.as_ref().unwrap();
    assert_eq!(v.get("high").map(String::as_str), Some("gemini-3.1-pro-high"));
    assert_eq!(v.get("low").map(String::as_str), Some("gemini-3.1-pro-low"));
    assert_eq!((models[1].id.as_str(), models[1].label.as_str(), models[1].efforts.as_deref()), ("claude-opus-4-6-thinking", "Claude Opus 4.6 (Thinking)", Some(&[][..])));
    assert_eq!((models[2].id.as_str(), models[2].efforts.as_deref()), ("gpt-oss-120b-medium", Some(&[][..])));
    assert!(parse_agy_models("login failed").is_empty());
}

#[test]
fn concurrent_discovery_uses_one_host_request_failure_remains_explicit() {
    run(|| async {
        let calls = Rc::new(Cell::new(0));
        let c = calls.clone();
        Fake::install(move |id, _, _| {
            if id == "models-error" {
                return ExecResult { code: 1, stdout: String::new(), stderr: "not signed in".into() };
            }
            c.set(c.get() + 1);
            ok(CATALOG)
        });
        let h = host("models-test", "local");
        let (first, second) = futures_util::future::join(agy_models(&h), agy_models(&h)).await;
        assert_eq!(calls.get(), 1);
        assert_eq!(first.source, CatalogSource::Cli);
        assert_eq!(second, first);
        assert_eq!(agy_models(&host("models-error", "local")).await.error.as_deref(), Some("not signed in"));
    });
}
