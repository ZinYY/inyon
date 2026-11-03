//! The web app, built (apps/web/dist) and carried inside the program, served the way index.ts
//! serves it: the file asked for, or index.html for any other path (the app routes itself), with
//! the page headers on both. The new app (apps/app, Dioxus) is carried the same way and served
//! under /next while it catches up with the web app.

use crate::http::Body;
use axum::http::{header, HeaderValue, Response, StatusCode};
use rust_embed::RustEmbed;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::http::{set_header, text, Resp};

/// Built with `bun run build` before the program; a build without it says so on every page. The
/// phone core's wasm stays out: only the phone app and the browser copy run it, from their own
/// files, and a hub's page can't compile wasm under PAGE_CSP anyway.
#[derive(RustEmbed)]
#[folder = "../../apps/web/dist"]
#[exclude = "assets/inyon_phone_bg-*.wasm"]
#[allow_missing = true]
struct Web;

/// No framing by other sites; images only from the hub itself, so agent output can't make the
/// browser fetch an outside URL (a prompt-injected agent could put data in one). Scripts only from
/// the app's own files: the page's `script-src 'self'` alone would also take a script an agent
/// wrote under /v/. A path needs a host in CSP; `*:*` is any host on any port, and the page's own
/// policy still holds it to this hub, so together they leave the app's files on this hub.
pub const PAGE_CSP: &str = "frame-ancestors 'none'; img-src 'self' data: blob:; object-src 'none'; base-uri 'none'; script-src *:*/assets/ *:*/theme.js *:*/sw.js";

/// The new app, built by apps/app/build.sh (dx and the wasm target). It is optional: a hub built
/// without it answers /next with a 404 that says so, and `cargo build` never needs dx.
#[derive(RustEmbed)]
#[folder = "../../apps/app/dist"]
#[allow_missing = true]
struct Next;

/// PAGE_CSP for the new app: its scripts are under /next/assets/, it shares the web app's theme
/// script and service worker (which shows push notifications), and it is wasm, which a page may
/// compile only with 'wasm-unsafe-eval'.
pub const NEXT_CSP: &str =
    "frame-ancestors 'none'; img-src 'self' data: blob:; object-src 'none'; base-uri 'none'; script-src *:*/next/assets/ *:*/theme.js *:*/sw.js 'wasm-unsafe-eval'";

/// What /next says on a hub built without the new app.
pub const NEXT_MISSING: &str = "The new app isn't in this build of the hub. Build it with apps/app/build.sh before the hub.";

pub fn page_headers(r: &mut Resp) {
    set_header(r, "content-security-policy", PAGE_CSP);
    set_header(r, "x-content-type-options", "nosniff");
    set_header(r, "referrer-policy", "same-origin");
}

/// The content types Bun gives these files.
fn content_type(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map_or("", |(_, e)| e).to_ascii_lowercase();
    match ext.as_str() {
        "js" | "mjs" => "text/javascript;charset=utf-8",
        "css" => "text/css;charset=utf-8",
        "html" | "htm" => "text/html;charset=utf-8",
        "json" | "map" => "application/json;charset=utf-8",
        "webmanifest" => "application/manifest+json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "txt" => "text/plain;charset=utf-8",
        "md" => "text/markdown",
        "xml" => "application/xml",
        "wasm" => "application/wasm",
        "mp3" => "audio/mpeg",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

/// `path.normalize(decoded).replace(/^(\.\.[/\\])+/, '')` for a path that starts with `/`: dot
/// segments resolved, and nothing above the root.
pub fn normalize(path: &str) -> String {
    let decoded = crate::http::decode_uri_component(path).unwrap_or_else(|| path.to_owned());
    let mut parts: Vec<&str> = vec![];
    for seg in decoded.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    let trailing = decoded.ends_with('/') && !parts.is_empty();
    format!("/{}{}", parts.join("/"), if trailing { "/" } else { "" })
}

/// Whether the browser takes gzip (RFC 9110): its own entry decides, else `*` does; `q=0` (the
/// name and the parameter in any case) refuses.
pub fn takes_gzip(accept: Option<&str>) -> bool {
    let mut star = None;
    for coding in accept.unwrap_or("").split(',') {
        let mut parts = coding.split(';').map(str::trim);
        let name = parts.next().unwrap_or("").to_ascii_lowercase();
        let q = parts.find_map(|p| p.split_once('=').filter(|(k, _)| k.trim().eq_ignore_ascii_case("q")).map(|(_, v)| v.trim().parse::<f32>().unwrap_or(1.0)));
        let ok = q.unwrap_or(1.0) > 0.0;
        match name.as_str() {
            "gzip" => return ok,
            "*" => star = Some(ok),
            _ => {}
        }
    }
    star.unwrap_or(false)
}

/// Text and wasm shrink to a third or less; pictures and fonts are compressed already.
fn compressible(name: &str) -> bool {
    let t = content_type(name);
    t.starts_with("text/") || (t.starts_with("application/") && !t.ends_with("octet-stream") && !t.ends_with("pdf")) || t == "image/svg+xml"
}

/// Files this small cost more in a gzip header than they save.
const GZIP_FROM: usize = 1024;

/// Gzip copies of the carried files, made on first request. The files are fixed in the program,
/// so the map holds at most one copy of each and never grows past them.
fn gzipped(key: String, data: &[u8]) -> Arc<[u8]> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<[u8]>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(z) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return z.clone();
    }
    let mut e = flate2::write::GzEncoder::new(Vec::with_capacity(data.len() / 3), flate2::Compression::best());
    let z: Arc<[u8]> = match std::io::Write::write_all(&mut e, data).and_then(|()| e.finish()) {
        Ok(z) => z.into(),
        Err(_) => return data.into(),
    };
    cache.lock().unwrap_or_else(|e| e.into_inner()).insert(key, z.clone());
    z
}

fn file(app: &str, name: &str, data: Vec<u8>, cache: &'static str, gzip: bool) -> Resp {
    let squeeze = compressible(name) && data.len() >= GZIP_FROM;
    let mut r = if squeeze && gzip {
        let mut r = Response::new(Body::from(gzipped(format!("{app}{name}"), &data).to_vec()));
        r.headers_mut().insert(header::CONTENT_ENCODING, HeaderValue::from_static("gzip"));
        r
    } else {
        Response::new(Body::from(data))
    };
    if squeeze {
        r.headers_mut().insert(header::VARY, HeaderValue::from_static("accept-encoding"));
    }
    r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type(name)));
    r.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    page_headers(&mut r);
    r
}

fn next_file(name: &str, data: Vec<u8>, cache: &'static str, gzip: bool) -> Resp {
    let mut r = file("next:", name, data, cache, gzip);
    set_header(&mut r, "content-security-policy", NEXT_CSP);
    r
}

/// Whether the web app was built into this program.
pub fn built() -> bool {
    Web::get("index.html").is_some()
}

/// The web app's file for `path`, gzipped when `gzip` (the browser takes it) and worth it.
pub fn serve(path: &str, gzip: bool) -> Resp {
    let rel = normalize(path);
    if rel != "/" {
        if let Some(f) = Web::get(&rel[1..]) {
            let immutable = rel.starts_with("/assets/");
            return file("web:", &rel, f.data.into_owned(), if immutable { "public, max-age=31536000, immutable" } else { "no-cache" }, gzip);
        }
    }
    match Web::get("index.html") {
        Some(f) => file("web:", "index.html", f.data.into_owned(), "no-cache", gzip),
        None => {
            let mut r = text(503, "Web app not built. Run `bun run build`.");
            *r.status_mut() = StatusCode::SERVICE_UNAVAILABLE;
            r
        }
    }
}

/// The new app at /next: its hashed files under /next/assets/, index.html for every other path
/// under /next (it will route itself), and a 404 when this hub was built without it.
pub fn serve_next(path: &str, gzip: bool) -> Resp {
    serve_next_from(path, gzip, |name| Next::get(name).map(|f| f.data.into_owned()))
}

fn serve_next_from(path: &str, gzip: bool, get: impl Fn(&str) -> Option<Vec<u8>>) -> Resp {
    let rel = normalize(path);
    let inner = rel.strip_prefix("/next").filter(|r| r.is_empty() || r.starts_with('/')).unwrap_or("");
    if inner.len() > 1 {
        if let Some(data) = get(&inner[1..]) {
            let immutable = inner.starts_with("/assets/");
            return next_file(inner, data, if immutable { "public, max-age=31536000, immutable" } else { "no-cache" }, gzip);
        }
        // a hashed file this build doesn't have (an old page asking after an update) isn't the page
        if inner.starts_with("/assets/") {
            return text(404, "Not found");
        }
    }
    match get("index.html") {
        Some(data) => next_file("index.html", data, "no-cache", gzip),
        None => {
            let mut r = text(404, NEXT_MISSING);
            page_headers(&mut r);
            r
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_inside_the_app() {
        assert_eq!(normalize("/"), "/");
        assert_eq!(normalize("/assets/a.js"), "/assets/a.js");
        assert_eq!(normalize("/../../etc/passwd"), "/etc/passwd");
        assert_eq!(normalize("/a/%2e%2e/b"), "/b");
        assert_eq!(normalize("/m/x/p/1"), "/m/x/p/1");
    }

    #[test]
    fn the_app_runs_only_its_own_scripts_never_a_pages_under_v() {
        let mut r = Resp::new(Body::empty());
        page_headers(&mut r);
        let csp = r.headers()["content-security-policy"].to_str().unwrap().to_owned();
        let scripts = csp.split("; ").find_map(|d| d.strip_prefix("script-src ")).unwrap();
        assert_eq!(scripts.split(' ').collect::<Vec<_>>(), ["*:*/assets/", "*:*/theme.js", "*:*/sw.js"]);
        assert!(!scripts.contains("'self'") && !scripts.contains("/v/"));
        // the files the app loads are all under those
        let index = include_str!("../../../apps/web/index.html");
        assert!(index.contains(r#"<script src="/theme.js">"#) && index.contains("script-src 'self'"));
        assert!(include_str!("../../../apps/web/src/main.tsx").contains("publicFile('sw.js')"));
    }

    #[test]
    fn the_phone_cores_wasm_is_left_out_and_the_rest_is_in() {
        let Ok(dir) = std::fs::read_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../apps/web/dist/assets")) else { return };
        for e in dir.flatten() {
            let name = format!("assets/{}", e.file_name().to_string_lossy());
            let phone = name.starts_with("assets/inyon_phone_bg-") && name.ends_with(".wasm");
            assert_eq!(Web::get(&name).is_some(), !phone, "{name}");
        }
    }

    async fn body(r: Resp) -> String {
        String::from_utf8_lossy(&r.into_body().bytes().await.unwrap()).into_owned()
    }

    fn built_next(name: &str) -> Option<Vec<u8>> {
        match name {
            "index.html" => Some(b"<div id=main></div><script type=module src=/next/assets/app-1.js></script>".to_vec()),
            "assets/app-1.js" => Some(b"export{}".to_vec()),
            "assets/app_bg-1.wasm" => Some(vec![0, 97, 115, 109]),
            _ => None,
        }
    }

    #[test]
    fn next_serves_the_new_apps_page_for_every_path_under_it() {
        crate::test_util::block(async {
            for path in ["/next", "/next/", "/next/m/box/p/1", "/next/../next/"] {
                let r = serve_next_from(path, false, built_next);
                assert_eq!(r.status().as_u16(), 200, "{path}");
                assert_eq!(r.headers()["content-type"], "text/html;charset=utf-8");
                assert_eq!(r.headers()["cache-control"], "no-cache");
                assert_eq!(r.headers()["content-security-policy"], NEXT_CSP);
                assert!(body(r).await.contains("/next/assets/app-1.js"), "{path}");
            }
        });
    }

    #[test]
    fn next_serves_its_files_and_refuses_ones_it_does_not_have() {
        let r = serve_next_from("/next/assets/app-1.js", false, built_next);
        assert_eq!(r.status().as_u16(), 200);
        assert_eq!(r.headers()["content-type"], "text/javascript;charset=utf-8");
        assert_eq!(r.headers()["cache-control"], "public, max-age=31536000, immutable");
        assert_eq!(serve_next_from("/next/assets/app_bg-1.wasm", false, built_next).headers()["content-type"], "application/wasm");
        // an old build's file is a 404, never the page passed off as a script
        assert_eq!(serve_next_from("/next/assets/app-0.js", false, built_next).status().as_u16(), 404);
        // nothing outside the new app's own files
        assert_eq!(serve_next_from("/next/../../etc/passwd", false, built_next).headers()["content-type"], "text/html;charset=utf-8");
    }

    #[test]
    fn a_hub_built_without_the_new_app_says_so_at_next() {
        crate::test_util::block(async {
            let r = serve_next_from("/next/", false, |_| None);
            assert_eq!(r.status().as_u16(), 404);
            assert_eq!(r.headers()["content-security-policy"], PAGE_CSP);
            assert_eq!(body(r).await, NEXT_MISSING);
        });
    }

    #[test]
    fn the_new_app_may_compile_wasm_and_runs_only_its_own_scripts() {
        let scripts = NEXT_CSP.split("; ").find_map(|d| d.strip_prefix("script-src ")).unwrap();
        assert_eq!(scripts.split(' ').collect::<Vec<_>>(), ["*:*/next/assets/", "*:*/theme.js", "*:*/sw.js", "'wasm-unsafe-eval'"]);
        // the rest of the policy is the web app's
        assert_eq!(NEXT_CSP.split("; script-src").next(), PAGE_CSP.split("; script-src").next());
        // the page loads nothing from elsewhere
        let index = include_str!("../../../apps/app/index.html");
        assert!(index.contains(r#"<script src="/theme.js">"#));
        assert!(include_str!("../../../apps/app/Dioxus.toml").contains(r#"base_path = "next""#));
    }

    #[test]
    fn the_new_app_is_carried_when_it_was_built() {
        let built = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../apps/app/dist/index.html")).exists();
        assert_eq!(Next::get("index.html").is_some(), built);
        if built {
            let index = String::from_utf8(Next::get("index.html").unwrap().data.into_owned()).unwrap();
            assert!(index.contains("/next/assets/"), "built with base_path next");
        }
    }

    #[test]
    fn the_hub_sends_next_to_the_new_app_and_keeps_the_web_app_at_the_root() {
        use crate::server::{dispatch, install_ctx, test_req, CtxInit};
        let next_built = Next::get("index.html").is_some();
        crate::test_util::block(async move {
            let ctx = CtxInit {
                data_dir: std::env::temp_dir().join(format!("inyon-next-{}", std::process::id())),
                bind: "127.0.0.1".into(),
                port: 0,
                tls: false,
                allowed_hosts: Some("hub.test".into()),
                allowed_origins: None,
                hostname: "test-box".into(),
                setup_code: None,
                local_token: "local".into(),
                hub_commit: String::new(),
                core: inyon_backends::core_here(),
            };
            install_ctx(ctx, crate::server::hub_routes());
            let get = |path: &'static str, host: &'static str| dispatch(test_req("GET", path, &[("host", host)], ""));
            for path in ["/next", "/next/", "/next/m/box/p/1"] {
                let r = get(path, "hub.test:4777").await;
                assert_eq!(r.status().as_u16(), if next_built { 200 } else { 404 }, "{path}");
                assert_eq!(r.headers()["content-security-policy"], if next_built { NEXT_CSP } else { PAGE_CSP }, "{path}");
            }
            // the web app keeps / and every path of its own, /nextcloud included
            for path in ["/", "/machines", "/nextcloud"] {
                let r = get(path, "hub.test:4777").await;
                assert_eq!(r.status().as_u16(), if built() { 200 } else { 503 }, "{path}");
                assert_ne!(r.headers().get("content-security-policy").map(|v| v.to_str().unwrap()), Some(NEXT_CSP), "{path}");
            }
            // a name the hub doesn't answer to is refused before either app
            assert_eq!(get("/next/", "evil.example").await.status().as_u16(), 421);
        });
    }

    #[test]
    fn gzip_is_taken_unless_refused() {
        assert!(takes_gzip(Some("gzip, deflate, br, zstd")));
        assert!(takes_gzip(Some("br;q=1.0, GZIP;q=0.5")));
        assert!(takes_gzip(Some("*")));
        assert!(!takes_gzip(Some("gzip;q=0, br")));
        assert!(!takes_gzip(Some("gzip;Q=0")));
        assert!(!takes_gzip(Some("gzip ; q = 0.000")));
        // its own entry wins over the wildcard, either way round
        assert!(!takes_gzip(Some("gzip;q=0, *;q=1")));
        assert!(!takes_gzip(Some("*;q=1, gzip;q=0")));
        assert!(takes_gzip(Some("*;q=0, gzip")));
        assert!(!takes_gzip(Some("*;q=0")));
        assert!(!takes_gzip(Some("identity")));
        assert!(!takes_gzip(None));
    }

    fn big_next(name: &str) -> Option<Vec<u8>> {
        match name {
            "assets/app_bg-1.wasm" => Some([0u8, 97, 115, 109].repeat(4096)),
            "assets/app-1.js" => Some(b"export const a = 1;\n".repeat(200)),
            "assets/geist.woff2" => Some(vec![7; 4096]),
            "assets/tiny.js" => Some(b"export{}".to_vec()),
            _ => built_next(name),
        }
    }

    #[test]
    fn the_new_apps_wasm_and_scripts_go_out_gzipped_to_a_browser_that_takes_it() {
        crate::test_util::block(async {
            for name in ["/next/assets/app_bg-1.wasm", "/next/assets/app-1.js"] {
                let plain = big_next(&name["/next/".len()..]).unwrap();
                let r = serve_next_from(name, true, big_next);
                assert_eq!(r.headers()["content-encoding"], "gzip", "{name}");
                assert_eq!(r.headers()["vary"], "accept-encoding");
                assert_eq!(r.headers()["content-security-policy"], NEXT_CSP);
                let z = r.into_body().bytes().await.unwrap();
                assert!(z.len() < plain.len() / 3, "{name}: {} of {}", z.len(), plain.len());
                let mut out = vec![];
                std::io::Read::read_to_end(&mut flate2::read::GzDecoder::new(&z[..]), &mut out).unwrap();
                assert_eq!(out, plain, "{name}");
                // a browser that doesn't take gzip gets the file as it is, and caches know it varies
                let r = serve_next_from(name, false, big_next);
                assert!(r.headers().get("content-encoding").is_none());
                assert_eq!(r.headers()["vary"], "accept-encoding");
                assert_eq!(r.into_body().bytes().await.unwrap(), plain);
            }
        });
    }

    #[test]
    fn fonts_and_tiny_files_go_out_as_they_are() {
        for name in ["/next/assets/geist.woff2", "/next/assets/tiny.js"] {
            let r = serve_next_from(name, true, big_next);
            assert!(r.headers().get("content-encoding").is_none(), "{name}");
        }
        assert!(compressible("index.html") && compressible("a.wasm") && compressible("a.svg") && compressible("m.webmanifest"));
        assert!(!compressible("a.woff2") && !compressible("a.png") && !compressible("a.pdf") && !compressible("a.bin"));
    }

    #[test]
    fn types_are_buns() {
        assert_eq!(content_type("/assets/index-x.js"), "text/javascript;charset=utf-8");
        assert_eq!(content_type("/manifest.webmanifest"), "application/manifest+json");
        assert_eq!(content_type("index.html"), "text/html;charset=utf-8");
    }
}
