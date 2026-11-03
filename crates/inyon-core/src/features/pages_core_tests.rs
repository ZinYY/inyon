//! features/pages-core.test.ts, against the shared vectors in fixtures/pages: the tokens the Bun
//! hub mints and how it judges them, the pure rules, and every `/v/` decision for the fixture site.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use futures_util::FutureExt;
use serde_json::{json, Value};

use super::*;
use crate::test_support::{fixture, TempDir};

fn vectors(name: &str) -> Value {
    serde_json::from_str(&fixture(&format!("pages/{name}"))).unwrap()
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

struct Hub {
    key: Vec<u8>,
    now: u64,
    epochs: HashMap<String, u64>,
    phones: Vec<String>,
}

fn hub(t: &Value) -> Hub {
    Hub {
        key: pages_key(t["secret"].as_str().unwrap()),
        now: t["now"].as_u64().unwrap(),
        epochs: t["epochs"].as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.as_u64().unwrap())).collect(),
        phones: t["phones"].as_array().unwrap().iter().map(|p| p.as_str().unwrap().to_owned()).collect(),
    }
}

#[test]
fn the_key_is_the_one_the_bun_hub_derives_from_the_same_session_secret() {
    let t = vectors("tokens.json");
    assert_eq!(hex(&pages_key(t["secret"].as_str().unwrap())), t["key"].as_str().unwrap());
    assert_eq!(t["ttl"].as_u64().unwrap(), TOKEN_TTL_MS);
}

#[test]
fn tokens_are_minted_byte_for_byte_as_the_bun_hub_mints_them() {
    let t = vectors("tokens.json");
    let h = hub(&t);
    for m in t["mint"].as_array().unwrap() {
        let site = &m["site"];
        let (token, expires) =
            mint_token(&h.key, site["h"].as_str().unwrap(), site["s"].as_str().unwrap(), site["v"].as_u64().unwrap(), site["p"].as_str(), m["now"].as_u64().unwrap());
        assert_eq!(token, m["token"].as_str().unwrap(), "{site}");
        assert_eq!(expires, m["expires"].as_u64().unwrap());
    }
}

#[test]
fn every_token_the_bun_hub_judged_is_judged_alike() {
    let t = vectors("tokens.json");
    let h = hub(&t);
    let epoch_of = |host: &str, slug: &str| h.epochs.get(&format!("{host}/{slug}")).copied().unwrap_or(0);
    let phone_paired = |id: &str| h.phones.iter().any(|p| p == id);
    let ctx = VerifyContext { key: &h.key, now: h.now, epoch_of: &epoch_of, phone_paired: &phone_paired };
    let cases = t["verify"].as_array().unwrap();
    assert_eq!(cases.len(), 32);
    for c in cases {
        let name = c["name"].as_str().unwrap();
        match verify_token(c["token"].as_str().unwrap(), &ctx) {
            Ok(claims) => {
                assert_eq!(c["verdict"], "ok", "{name}");
                assert_eq!(claims.to_json(), c["claims"], "{name}");
            }
            Err(v) => assert_eq!(v.as_str(), c["verdict"].as_str().unwrap(), "{name}"),
        }
    }
}

#[test]
fn a_new_session_secret_ends_every_token() {
    let t = vectors("tokens.json");
    let h = hub(&t);
    let (token, _) = mint_token(&h.key, "local", "fixture", 3, None, h.now);
    let other = pages_key("a new password made a new secret");
    let ctx = VerifyContext { key: &other, now: h.now, epoch_of: &|_, _| 3, phone_paired: &|_| true };
    assert_eq!(verify_token(&token, &ctx), Err(TokenVerdict::BadSignature));
}

#[test]
fn the_rules_match_the_vectors() {
    let r = vectors("rules.json");
    assert_eq!(r["listMax"].as_u64(), Some(PAGES_LIST_MAX));
    for c in r["slugify"].as_array().unwrap() {
        assert_eq!(slugify(c["text"].as_str().unwrap()), c["slug"].as_str().unwrap(), "{c}");
    }
    for c in r["isSlug"].as_array().unwrap() {
        assert_eq!(is_slug(c["s"].as_str().unwrap()), c["ok"].as_bool().unwrap(), "{c}");
    }
    for c in r["uniqueSlug"].as_array().unwrap() {
        let taken: Vec<&str> = c["taken"].as_array().unwrap().iter().map(|s| s.as_str().unwrap()).collect();
        assert_eq!(unique_slug(c["base"].as_str().unwrap(), |s| taken.contains(&s)), c["slug"].as_str().unwrap(), "{c}");
    }
    for c in r["sitePath"].as_array().unwrap() {
        assert_eq!(site_path_problem(c["path"].as_str().unwrap(), c["home"].as_str().unwrap()), c["problem"].as_str(), "{c}");
    }
    for c in r["progress"].as_array().unwrap() {
        let got = progress_summary(&c["data"]).map(|p| serde_json::to_value(p).unwrap()).unwrap_or(Value::Null);
        assert_eq!(got, c["summary"], "{c}");
    }
}

#[test]
fn dot_segments_resolve_as_a_url_parser_resolves_them() {
    assert_eq!(normalize_target("/v/t/../x"), ("/v/x".into(), None));
    assert_eq!(normalize_target("/v/t/%2E%2e/x?a=../b"), ("/v/x".into(), Some("a=../b".into())));
    assert_eq!(normalize_target("/a/./b/."), ("/a/b/".into(), None));
    assert_eq!(normalize_target("/a/b/.."), ("/a/".into(), None));
    assert_eq!(normalize_target("/../../x"), ("/x".into(), None));
    assert_eq!(normalize_target("/a\\..\\b"), ("/b".into(), None));
    assert_eq!(normalize_target("/a/.%2e/b"), ("/b".into(), None));
    assert_eq!(normalize_target("/a/...b"), ("/a/...b".into(), None));
    assert!(is_view_path("/v/x/") && !is_view_path("/v") && !is_view_path("/vv/x"));
    // what decodeURIComponent throws on, this refuses
    assert_eq!(decode_uri_component("%+f"), None);
    assert_eq!(decode_uri_component("%e2%82%ac").as_deref(), Some("€"));
    assert_eq!(decode_uri_component("%ED%A0%80"), None);
    assert_eq!(view_segments("//"), None);
    assert_eq!(view_segments("/"), Some((vec![], true)));
    assert_eq!(view_segments("/a%20b/c/"), Some((vec!["a b".into(), "c".into()], true)));
    assert_eq!(encode_uri_component("a b/ü!'()*~"), "a%20b%2F%C3%BC!'()*~");
}

#[test]
fn etags_and_types() {
    assert_eq!(etag_of(255, 1_792_233_600_000.9), "\"ff-1a149727400\"");
    assert_eq!(etag_of(0, -1.5), "\"0--2\"");
    assert!(etag_matches(Some(" * "), "\"a\""));
    assert!(etag_matches(Some("\"x\", W/\"a\""), "\"a\""));
    assert!(!etag_matches(Some(""), "\"a\"") && !etag_matches(None, "\"a\""));
    assert_eq!(content_type("A.MD"), "text/plain;charset=utf-8");
    assert_eq!(content_type("noext"), "application/octet-stream");
    assert_eq!(view_origin(Some("Hub.Test:4777"), true).as_deref(), Some("https://hub.test:4777"));
    assert_eq!(view_origin(Some("[::1]:80"), false).as_deref(), Some("http://[::1]:80"));
    assert_eq!(view_origin(Some("a b"), false), None);
    assert_eq!(view_origin(Some("hub.test, evil.test"), false), None);
    assert_eq!(view_origin(Some("hub.test:٤"), false), None);
}

#[test]
fn the_csp_lets_the_page_load_only_from_its_token_path_and_has_no_frame_ancestors() {
    let csp = view_csp(Some("http://hub.test"), Some("tok"));
    assert!(csp.starts_with("sandbox allow-scripts allow-forms allow-popups allow-downloads; default-src 'none'; script-src http://hub.test/v/tok/ 'unsafe-inline'"));
    // a sandboxed page has an opaque origin, which no frame-ancestors source matches
    assert!(!csp.contains("frame-ancestors"));
    assert!(csp.ends_with("object-src 'none'; base-uri 'none'; form-action 'none'"));
    assert_eq!(
        view_csp(None, None),
        "sandbox allow-scripts allow-forms allow-popups allow-downloads; default-src 'none'; style-src 'unsafe-inline'; object-src 'none'; base-uri 'none'; form-action 'none'"
    );
}

// ---------- every /v/ decision in responses.json ----------

/// decide_view's disk through std::fs (both calls follow symlinks).
pub struct StdFs;

impl ViewFs for StdFs {
    fn stat<'a>(&'a self, p: &'a Path) -> LocalBoxFuture<'a, Option<FileStat>> {
        Box::pin(async move {
            let m = std::fs::metadata(p).ok()?;
            let d = m.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?;
            Some(FileStat { file: m.is_file(), dir: m.is_dir(), size: m.len(), mtime_ms: d.as_secs() as f64 * 1e3 + f64::from(d.subsec_nanos()) / 1e6 })
        })
    }
    fn realpath<'a>(&'a self, p: &'a Path) -> LocalBoxFuture<'a, Option<PathBuf>> {
        Box::pin(async move { std::fs::canonicalize(p).ok() })
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let target = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &target);
        } else {
            std::fs::copy(e.path(), &target).unwrap();
        }
    }
}

fn touch_all(dir: &Path, at: std::time::SystemTime) {
    for e in std::fs::read_dir(dir).unwrap() {
        let e = e.unwrap();
        let ty = e.file_type().unwrap();
        if ty.is_symlink() {
            continue;
        }
        if ty.is_dir() {
            touch_all(&e.path(), at);
        } else {
            std::fs::OpenOptions::new().write(true).open(e.path()).unwrap().set_modified(at).unwrap();
        }
    }
}

/// responses.json's `setup` under `dir`: the site copied, the outside files and the symlinks, every
/// file's mtime the one it names. The site's folder.
#[cfg(unix)]
fn lay_out(dir: &Path, setup: &Value) -> PathBuf {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/pages");
    let site = dir.join("site");
    copy_dir(&root.join(setup["copy"].as_str().unwrap()), &site);
    std::fs::create_dir_all(dir.join("outside")).unwrap();
    for (name, text) in setup["outside"].as_object().unwrap() {
        std::fs::write(dir.join("outside").join(name), text.as_str().unwrap()).unwrap();
    }
    for (name, target) in setup["symlinks"].as_object().unwrap() {
        std::os::unix::fs::symlink(target.as_str().unwrap(), site.join(name)).unwrap();
    }
    let at = std::time::UNIX_EPOCH + std::time::Duration::from_millis(setup["mtimeMs"].as_u64().unwrap());
    touch_all(dir, at);
    site
}

#[cfg(unix)]
#[test]
fn every_view_decision_in_responses_json() {
    let t = vectors("tokens.json");
    let r = vectors("responses.json");
    let h = hub(&t);
    let tmp = TempDir::new("inyon-pages-core");
    let site = lay_out(&tmp.0, &r["setup"]);
    let site_real = std::fs::canonicalize(&site).unwrap();
    let cx = &r["context"];
    let (host, want_h, want_s) = (cx["host"].as_str().unwrap(), cx["site"]["h"].as_str().unwrap(), cx["site"]["s"].as_str().unwrap());
    let epoch_of = |host: &str, slug: &str| h.epochs.get(&format!("{host}/{slug}")).copied().unwrap_or(0);
    let phone_paired = |id: &str| h.phones.iter().any(|p| p == id);
    let site_root = |hh: &str, ss: &str| (hh == want_h && ss == want_s).then(|| site.clone());
    let ctx = ViewContext {
        verify: VerifyContext { key: &h.key, now: cx["now"].as_u64().unwrap(), epoch_of: &epoch_of, phone_paired: &phone_paired },
        site_root: &site_root,
        fs: &StdFs,
    };
    let cases = r["cases"].as_array().unwrap();
    assert!(cases.len() >= 59);
    for c in cases {
        let name = c["name"].as_str().unwrap();
        let q = &c["request"];
        let method = q["method"].as_str().unwrap();
        let h_host = q["headers"]["host"].as_str().unwrap();
        let origin = (h_host == host).then(|| view_origin(Some(h_host), false)).flatten();
        let req = ViewRequest { method, target: q["target"].as_str().unwrap(), origin: origin.as_deref(), if_none_match: q["headers"]["if-none-match"].as_str() };
        let d = decide_view(&req, &ctx).now_or_never().unwrap();
        let want = &c["response"];
        assert_eq!(u64::from(d.status), want["status"].as_u64().unwrap(), "{name}");
        let got: serde_json::Map<String, Value> = d.headers.iter().map(|(k, v)| ((*k).to_owned(), json!(v))).collect();
        assert_eq!(Value::Object(got), want["headers"], "{name}");
        let body = if method == "HEAD" {
            Value::Null
        } else if let Some((file, _)) = &d.file {
            json!({ "file": format!("site/{}", file.strip_prefix(&site_real).unwrap().to_string_lossy()) })
        } else if d.body.is_empty() {
            Value::Null
        } else {
            assert_eq!(d.body, r["errorPages"][d.status.to_string()].as_str().unwrap(), "{name}");
            json!({ "page": d.status })
        };
        assert_eq!(body, want["body"], "{name}");
    }
    for s in [403u16, 404, 405, 413, 421] {
        assert_eq!(error_page(s), r["errorPages"][s.to_string()].as_str().unwrap());
    }
}
