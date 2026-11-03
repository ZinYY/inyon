//! Port of features/pages-core.ts (the Bun hub's, removed in 0.23): the rules for serving an agent's site under
//! `/v/<token>/<path>`. Tokens, slugs, the path rules and the headers on every `/v/` answer live
//! here, with no I/O of their own: the key, the clock and the file system come in as arguments.
//! fixtures/pages (written by the Bun hub while it existed) holds what this must produce, byte for
//! byte, so a token an older hub minted still opens and every hub answers alike.
//!
//! The threat is the site itself: an agent wrote its HTML and JS and may have been talked into
//! something. So `/v/` never reads a cookie or an Authorization header, never sets a cookie, and
//! every answer (errors and redirects too) carries a CSP sandbox that gives the page an opaque
//! origin and lets it load only from its own token path.

use std::path::{Component, Path, PathBuf};

use base64::Engine;
use futures_util::future::LocalBoxFuture;
use hmac::{KeyInit, Mac};
use inyon_protocol::PageProgress;
use serde_json::{Map, Value};
use sha2::Sha256;

use crate::js::json::stringify;

/// The hub's own machine, as a host id.
pub const LOCAL_HOST: &str = "local";
/// How long a token opens its site.
pub const TOKEN_TTL_MS: u64 = 12 * 60 * 60 * 1000;
/// Longer than any token a hub makes; anything longer is refused before it is looked at.
const MAX_TOKEN: usize = 2048;
/// The most of ~/.inyon/pages.json any reader takes (both hubs, the CLI and Blueprint): a list
/// of a few thousand pages fits, and a bigger file (a sparse one claiming terabytes) counts as a
/// list that can't be read, at once.
pub const PAGES_LIST_MAX: u64 = 4 * 1024 * 1024;
/// How long the `git rev-parse` calls that find a page's project may take, together, before git is stopped.
pub const GIT_TIMEOUT_MS: u64 = 3000;
/// Set for those calls: no machine-wide git config, and no lock files taken just to look.
pub const GIT_ENV: [(&str, &str); 2] = [("GIT_CONFIG_NOSYSTEM", "1"), ("GIT_OPTIONAL_LOCKS", "0")];
/// `Number.MAX_SAFE_INTEGER`
const MAX_SAFE: f64 = 9_007_199_254_740_991.0;

// ---------- slugs, folders, progress ----------

/// `^[a-z0-9][a-z0-9-]{0,62}$`
pub fn is_slug(s: &str) -> bool {
    let b = s.as_bytes();
    !b.is_empty() && b.len() <= 63 && (b[0].is_ascii_lowercase() || b[0].is_ascii_digit()) && b.iter().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

pub const BAD_SLUG: &str = "A slug is lowercase letters, digits and dashes, up to 63 characters, starting with a letter or digit";
pub const NOT_ABSOLUTE: &str = "The folder must be an absolute path";
pub const TOO_WIDE: &str = "Pick the site's own folder, not / or your home folder";
pub const PRIVATE_FOLDER: &str = "Folders under ~/.ssh and ~/.inyon can't be pages";
pub const NO_INDEX: &str = "That folder has no index.html";
/// A 500 from register and delete when ~/.inyon can't be written: a read-only folder, or a stale lock that can't be removed.
pub const CANT_WRITE_LIST: &str =
    "The hub can't write in ~/.inyon, so it can't change pages.json there. Check that the folder and its pages.json.lock belong to the hub's user, then try again.";

/// A slug made from a name: ASCII letters lowercased, every run of anything else (other letters
/// too) one dash, no dash at either end, cut to 63 characters. "page" when nothing is left.
pub fn slugify(text: &str) -> String {
    let mut s = String::new();
    let mut dash = false;
    for c in text.chars() {
        let c = c.to_ascii_lowercase();
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            s.push(c);
            dash = false;
        } else if !dash {
            s.push('-');
            dash = true;
        }
    }
    let s = s.trim_matches('-');
    let s = &s[..s.len().min(63)];
    let s = s.trim_end_matches('-');
    if s.is_empty() {
        "page".into()
    } else {
        s.to_owned()
    }
}

/// `base`, or `base-2`, `base-3`… (cut so the whole stays 63 characters), whichever is free first.
pub fn unique_slug(base: &str, taken: impl Fn(&str) -> bool) -> String {
    if !taken(base) {
        return base.to_owned();
    }
    for i in 2u64.. {
        let suffix = format!("-{i}");
        let keep: String = base.chars().take(63usize.saturating_sub(suffix.len())).collect();
        let s = format!("{}{suffix}", keep.trim_end_matches('-'));
        if !taken(&s) {
            return s;
        }
    }
    unreachable!("some slug is free")
}

fn windows_path(p: &str) -> bool {
    let b = p.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
}

/// Whether a folder is written in full: from `/`, or from a drive letter (`C:\…`, `C:/…`). A
/// relative folder or `~/…` would land wherever the shell that reads it starts.
pub fn is_absolute(p: &str) -> bool {
    (p.starts_with('/') || windows_path(p)) && !p.contains('\0')
}

/// What is wrong with a folder someone wants as a page, before the disk is asked (the caller still
/// checks that `<path>/index.html` is a file, and says NO_INDEX when it isn't). None when it may be
/// one. `home` is the hub user's home folder. Paths are compared as written, with trailing slashes
/// dropped; a Windows path (`C:\…`) counts as absolute.
pub fn site_path_problem(p: &str, home: &str) -> Option<&'static str> {
    let win = windows_path(p);
    if !is_absolute(p) {
        return Some(NOT_ABSOLUTE);
    }
    let parts = |s: &str| -> Vec<String> { s.split(|c| c == '/' || (win && c == '\\')).filter(|x| !x.is_empty()).map(str::to_owned).collect() };
    let segs = parts(p);
    if segs.iter().any(|s| s == "." || s == "..") {
        return Some(NOT_ABSOLUTE);
    }
    let home_segs = parts(home);
    if segs.is_empty() || (win && segs.len() == 1) || (!home_segs.is_empty() && segs == home_segs) {
        return Some(TOO_WIDE);
    }
    let under = |dir: &str| {
        let mut d = home_segs.clone();
        d.push(dir.to_owned());
        segs.len() >= d.len() && segs[..d.len()] == d[..]
    };
    if !home_segs.is_empty() && (under(".ssh") || under(".inyon")) {
        return Some(PRIVATE_FOLDER);
    }
    None
}

/// A progress.json (as Blueprint writes it) counted for the list: tasks over every release, how
/// many are done, and its `now` and `updated` lines when they are strings. None when it isn't an
/// object with a `releases` array. Releases and tasks that aren't objects are skipped.
pub fn progress_summary(data: &Value) -> Option<PageProgress> {
    let releases = data.as_object()?.get("releases")?.as_array()?;
    let (mut done, mut total) = (0u32, 0u32);
    for r in releases {
        let Some(tasks) = r.as_object().and_then(|r| r.get("tasks")).and_then(Value::as_array) else { continue };
        for t in tasks.iter().filter_map(Value::as_object) {
            total += 1;
            if t.get("status").and_then(Value::as_str) == Some("done") {
                done += 1;
            }
        }
    }
    let text = |k: &str| data.get(k).and_then(Value::as_str).map(str::to_owned);
    Some(PageProgress { done, total, now: text("now"), updated: text("updated") })
}

// ---------- tokens ----------

/// What a token says: host, slug, expiry (unix ms), the site's epoch, and the phone that asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenClaims {
    pub h: String,
    pub s: String,
    pub x: u64,
    pub v: u64,
    pub p: Option<String>,
}

impl TokenClaims {
    /// The claims as the fixtures write them: `{h, s, x, v[, p]}`.
    pub fn to_json(&self) -> Value {
        let mut m = Map::new();
        m.insert("h".into(), Value::String(self.h.clone()));
        m.insert("s".into(), Value::String(self.s.clone()));
        m.insert("x".into(), Value::from(self.x));
        m.insert("v".into(), Value::from(self.v));
        if let Some(p) = &self.p {
            m.insert("p".into(), Value::String(p.clone()));
        }
        Value::Object(m)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenVerdict {
    Malformed,
    BadSignature,
    Expired,
    Revoked,
    Unpaired,
}

impl TokenVerdict {
    /// The name the fixtures use.
    pub fn as_str(self) -> &'static str {
        match self {
            TokenVerdict::Malformed => "malformed",
            TokenVerdict::BadSignature => "bad-signature",
            TokenVerdict::Expired => "expired",
            TokenVerdict::Revoked => "revoked",
            TokenVerdict::Unpaired => "unpaired",
        }
    }
}

pub struct VerifyContext<'a> {
    pub key: &'a [u8],
    /// unix ms
    pub now: u64,
    /// the site's current epoch; 0 for a site that never had one
    pub epoch_of: &'a dyn Fn(&str, &str) -> u64,
    /// whether a phone with this id is still paired
    pub phone_paired: &'a dyn Fn(&str) -> bool,
}

type HmacSha256 = hmac::Hmac<Sha256>;

fn hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut m = <HmacSha256 as KeyInit>::new_from_slice(key).expect("HMAC takes a key of any length");
    m.update(data);
    m.finalize().into_bytes().to_vec()
}

fn b64url(b: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(b)
}

/// `Buffer.from(s, 'base64url')`: lenient as Node is (no padding needed, trailing bits and a
/// dangling last character ignored). The token's shape check has already kept other characters out.
fn from_b64url(s: &str) -> Vec<u8> {
    let cfg = base64::engine::GeneralPurposeConfig::new().with_decode_allow_trailing_bits(true).with_decode_padding_mode(base64::engine::DecodePaddingMode::Indifferent);
    let engine = base64::engine::GeneralPurpose::new(&base64::alphabet::URL_SAFE, cfg);
    let s = s.trim_end_matches('=');
    let s = if s.len() % 4 == 1 { &s[..s.len() - 1] } else { s };
    engine.decode(s).unwrap_or_default()
}

/// The key tokens are signed with: HMAC-SHA256 keyed with the session secret (its UTF-8 bytes) over "inyon-pages-v1".
pub fn pages_key(session_secret: &str) -> Vec<u8> {
    hmac(session_secret.as_bytes(), b"inyon-pages-v1")
}

fn mac(key: &[u8], payload: &str) -> String {
    b64url(&hmac(key, format!("v1.{payload}").as_bytes()))
}

/// `v1.<payload>.<mac>`: the payload is base64url (no padding) of the claims as JSON, keys in the
/// order h, s, x, v, p; the mac is base64url of HMAC-SHA256(key, "v1." + payload).
pub fn sign_token(key: &[u8], claims: &TokenClaims) -> String {
    let payload = b64url(stringify(&claims.to_json()).as_bytes());
    let m = mac(key, &payload);
    format!("v1.{payload}.{m}")
}

/// A token for one site, good for TOKEN_TTL_MS from `now`: the token and when it expires.
pub fn mint_token(key: &[u8], h: &str, s: &str, v: u64, p: Option<&str>, now: u64) -> (String, u64) {
    let expires = now + TOKEN_TTL_MS;
    let claims = TokenClaims { h: h.to_owned(), s: s.to_owned(), x: expires, v, p: p.map(str::to_owned) };
    (sign_token(key, &claims), expires)
}

fn is_b64url_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_' || c == b'-'
}

/// `^v1\.([A-Za-z0-9_-]+)\.([A-Za-z0-9_-]{43})$`: the payload and the mac.
fn token_parts(token: &str) -> Option<(&str, &str)> {
    if token.len() > MAX_TOKEN {
        return None;
    }
    let rest = token.strip_prefix("v1.")?;
    let (payload, sig) = rest.split_once('.')?;
    let ok = !payload.is_empty() && payload.bytes().all(is_b64url_char) && sig.len() == 43 && sig.bytes().all(is_b64url_char);
    ok.then_some((payload, sig))
}

/// `crypto.timingSafeEqual` on two strings of the same length.
fn same(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// `Number.isSafeInteger(n) && n >= 0`
fn safe_int(v: Option<&Value>) -> Option<u64> {
    let n = v?.as_f64()?;
    (n.is_finite() && n.fract() == 0.0 && (0.0..=MAX_SAFE).contains(&n)).then_some(n as u64)
}

fn non_empty(v: Option<&Value>) -> Option<String> {
    v?.as_str().filter(|s| !s.is_empty()).map(str::to_owned)
}

/// Checks a token, in this order: its shape (malformed), the mac (bad-signature, compared in
/// constant time, before the payload is read), the claims (malformed: h a non-empty string, s a
/// slug, x and v whole numbers from 0 to 2^53-1, p a non-empty string when present; other keys are
/// ignored), the expiry (expired once now >= x), the epoch (revoked when v isn't the site's), and
/// the phone (unpaired).
pub fn verify_token(token: &str, ctx: &VerifyContext) -> Result<TokenClaims, TokenVerdict> {
    let (payload, sig) = token_parts(token).ok_or(TokenVerdict::Malformed)?;
    if !same(mac(ctx.key, payload).as_bytes(), sig.as_bytes()) {
        return Err(TokenVerdict::BadSignature);
    }
    let text = String::from_utf8_lossy(&from_b64url(payload)).into_owned();
    let c = crate::js::json::parse(&text).ok_or(TokenVerdict::Malformed)?;
    let c = c.as_object().ok_or(TokenVerdict::Malformed)?;
    let h = non_empty(c.get("h")).ok_or(TokenVerdict::Malformed)?;
    let s = c.get("s").and_then(Value::as_str).filter(|s| is_slug(s)).ok_or(TokenVerdict::Malformed)?.to_owned();
    let x = safe_int(c.get("x")).ok_or(TokenVerdict::Malformed)?;
    let v = safe_int(c.get("v")).ok_or(TokenVerdict::Malformed)?;
    let p = match c.get("p") {
        None => None,
        Some(p) => Some(non_empty(Some(p)).ok_or(TokenVerdict::Malformed)?),
    };
    let claims = TokenClaims { h, s, x, v, p };
    if ctx.now >= claims.x {
        return Err(TokenVerdict::Expired);
    }
    if claims.v != (ctx.epoch_of)(&claims.h, &claims.s) {
        return Err(TokenVerdict::Revoked);
    }
    if claims.p.as_deref().is_some_and(|p| !(ctx.phone_paired)(p)) {
        return Err(TokenVerdict::Unpaired);
    }
    Ok(claims)
}

// ---------- request targets and paths ----------

/// `/^(\.|%2e)$/i`
fn one_dot(seg: &str) -> bool {
    seg == "." || seg.eq_ignore_ascii_case("%2e")
}

/// `/^(\.|%2e)(\.|%2e)$/i`
fn two_dots(seg: &str) -> bool {
    let l = seg.to_ascii_lowercase();
    matches!(l.as_str(), ".." | ".%2e" | "%2e." | "%2e%2e")
}

/// The request target with dot segments resolved the way a browser's URL parser (and so Bun's
/// `req.url`) does: `\` counts as `/`, `.` `%2e` `..` `.%2e` `%2e.` `%2e%2e` in any case are dot
/// segments, `..` stops at the root, and a dot segment at the end leaves a trailing slash. The
/// query is split off unchanged. The hub runs this on the raw target before routing, so it sees
/// the path the Bun hub sees; `/v/<tok>/../x` is then `/v/x`, a token "x" that doesn't verify.
pub fn normalize_target(target: &str) -> (String, Option<String>) {
    let (raw, query) = match target.find('?') {
        Some(q) => (&target[..q], Some(target[q + 1..].to_owned())),
        None => (target, None),
    };
    let raw = raw.replace('\\', "/");
    let segs: Vec<&str> = raw.split('/').skip(1).collect();
    let mut out: Vec<&str> = vec![];
    for (i, seg) in segs.iter().enumerate() {
        let last = i == segs.len() - 1;
        if two_dots(seg) {
            out.pop();
            if last {
                out.push("");
            }
        } else if one_dot(seg) {
            if last {
                out.push("");
            }
        } else {
            out.push(seg);
        }
    }
    (format!("/{}", out.join("/")), query)
}

/// Whether a request belongs to `/v/` (after normalize_target); the route serves GET and HEAD and answers everything else 405.
pub fn is_view_path(target: &str) -> bool {
    normalize_target(target).0.starts_with("/v/")
}

/// `/v/<token><rest>?<query>` taken apart, after normalize_target. `rest` is "" or starts with "/".
pub struct ViewTarget {
    pub token: String,
    pub rest: String,
    pub query: Option<String>,
}

pub fn split_view_target(target: &str) -> Option<ViewTarget> {
    let (p, query) = normalize_target(target);
    let after = p.strip_prefix("/v/")?;
    Some(match after.find('/') {
        Some(i) => ViewTarget { token: after[..i].to_owned(), rest: after[i..].to_owned(), query },
        None => ViewTarget { token: after.to_owned(), rest: String::new(), query },
    })
}

/// `decodeURIComponent`, or None where it throws: a `%` without two hex digits after it, or bytes
/// that aren't UTF-8.
pub fn decode_uri_component(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let hex = |c: u8| (c as char).to_digit(16);
            let (Some(hi), Some(lo)) = (b.get(i + 1).copied().and_then(hex), b.get(i + 2).copied().and_then(hex)) else { return None };
            out.push((hi * 16 + lo) as u8);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The path after the token as file names, or None (a 404) when any of them is refused: an empty
/// segment (`a//b`), a bad percent escape or invalid UTF-8, a decoded name holding `/`, `\` or NUL,
/// or one starting with `.` (dot files and folders: `.git`, `.env`, the `.inyon-page.json` marker).
/// The flag is true when the path ends with `/`, which then means its index.html. `rest` is "/" or
/// longer; "" (no slash after the token) is a redirect, handled before this.
pub fn view_segments(rest: &str) -> Option<(Vec<String>, bool)> {
    let body = rest.get(1..).unwrap_or("");
    let dir = body.is_empty() || body.ends_with('/');
    let mut segments = vec![];
    if !body.is_empty() {
        let body = if dir { &body[..body.len() - 1] } else { body };
        for part in body.split('/') {
            let name = decode_uri_component(part)?;
            if name.is_empty() || name.starts_with('.') || name.contains(['/', '\\', '\0']) {
                return None;
            }
            segments.push(name);
        }
    }
    Some((segments, dir))
}

/// `encodeURIComponent`
pub fn encode_uri_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The query kept on a redirect: only when every character is printable ASCII other than
/// `"` `#` `%` `'` `<` `>`. A URL parser rewrites the others as percent escapes (Bun's req.url
/// turns `'` into `%27`), so refusing `%` as well makes a hub that sees the raw target and one
/// that sees the parsed URL agree.
fn kept_query(q: Option<&str>) -> String {
    let ok = |c: u8| matches!(c, 0x21 | 0x24 | 0x26 | 0x28..=0x3b | 0x3d | 0x3f..=0x7e);
    match q {
        Some(q) if !q.is_empty() && q.bytes().all(ok) => format!("?{q}"),
        _ => String::new(),
    }
}

// ---------- files ----------

/// The table index.ts and static_files.rs use for the app, with md and txt as plain text.
pub const CONTENT_TYPES: [(&str, &str); 28] = [
    ("html", "text/html;charset=utf-8"),
    ("htm", "text/html;charset=utf-8"),
    ("js", "text/javascript;charset=utf-8"),
    ("mjs", "text/javascript;charset=utf-8"),
    ("css", "text/css;charset=utf-8"),
    ("json", "application/json;charset=utf-8"),
    ("map", "application/json;charset=utf-8"),
    ("webmanifest", "application/manifest+json"),
    ("svg", "image/svg+xml"),
    ("png", "image/png"),
    ("jpg", "image/jpeg"),
    ("jpeg", "image/jpeg"),
    ("gif", "image/gif"),
    ("webp", "image/webp"),
    ("avif", "image/avif"),
    ("ico", "image/x-icon"),
    ("woff2", "font/woff2"),
    ("woff", "font/woff"),
    ("ttf", "font/ttf"),
    ("otf", "font/otf"),
    ("txt", "text/plain;charset=utf-8"),
    ("md", "text/plain;charset=utf-8"),
    ("xml", "application/xml"),
    ("wasm", "application/wasm"),
    ("mp3", "audio/mpeg"),
    ("mp4", "video/mp4"),
    ("webm", "video/webm"),
    ("pdf", "application/pdf"),
];

/// The content type for a file, by its extension (case ignored). Markdown and text are served as plain text.
pub fn content_type(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map_or(String::new(), |(_, e)| e.to_lowercase());
    CONTENT_TYPES.iter().find(|(e, _)| *e == ext).map_or("application/octet-stream", |(_, t)| t)
}

/// progress.json is read every 20 seconds and must never come from a cache; everything else is checked with its ETag.
pub fn no_store(name: &str) -> bool {
    name == "progress.json"
}

/// `(n).toString(16)` for a whole number, sign and all.
fn hex_signed(n: i128) -> String {
    if n < 0 {
        format!("-{:x}", -n)
    } else {
        format!("{n:x}")
    }
}

/// `"<size in hex>-<mtime in whole ms, hex>"`
pub fn etag_of(size: u64, mtime_ms: f64) -> String {
    format!("\"{size:x}-{}\"", hex_signed(mtime_ms.floor() as i128))
}

/// If-None-Match against an ETag: `*`, or any listed tag equal to it (a `W/` prefix ignored).
pub fn etag_matches(if_none_match: Option<&str>, etag: &str) -> bool {
    use crate::js::text::JsStr;
    let Some(inm) = if_none_match.filter(|s| !s.is_empty()) else { return false };
    if inm.jtrim() == "*" {
        return true;
    }
    inm.split(',').any(|t| {
        let t = t.jtrim();
        t.strip_prefix("W/").unwrap_or(t) == etag
    })
}

// ---------- headers ----------

/// The origin to write into the CSP, from the Host the hub answered to (X-Forwarded-Host first, as
/// same_origin reads it) and whether the request came over https. None for a host that isn't a
/// plain name, IPv4 or bracketed IPv6 with an optional port; lowercased otherwise.
pub fn view_origin(host: Option<&str>, https: bool) -> Option<String> {
    let host = host.filter(|h| !h.is_empty())?;
    if !crate::re!(r"^([A-Za-z0-9.-]+|\[[0-9A-Fa-f:.]+\])(:\d{1,5})?$").is_match(host) {
        return None;
    }
    Some(format!("{}://{}", if https { "https" } else { "http" }, host.to_ascii_lowercase()))
}

/// The sandbox flags. allow-forms lets a page's own script handle a form (a mockup's "Add" field):
/// without it the browser drops the submit before any handler runs. form-action 'none' in the CSP
/// still stops a form from sending anything anywhere.
///
/// There is no allow-same-origin, and never may be: the opaque origin is also what keeps password
/// managers away from a page served at the hub's own address. Chrome gives a frame with an opaque
/// origin no password manager at all (Blink's CanAccessPasswordManager), and Firefox's runs no code
/// in a document with a null principal, so neither fills or saves the hub password on /v/, framed,
/// in its own tab or in a popup (a popup keeps the sandbox: no allow-popups-to-escape-sandbox).
const SANDBOX: &str = "sandbox allow-scripts allow-forms allow-popups allow-downloads";

/// The Content-Security-Policy on every `/v/` answer. With an origin and a verified token, the page
/// may load only from `<origin>/v/<token>/`; without one (an error before the token checked out)
/// it may load nothing but inline styles.
///
/// There is no frame-ancestors on purpose. The sandbox gives every page an opaque origin, and a
/// browser matches no frame-ancestors source (not even `*`) against an opaque parent, so a site
/// framing its own files (Blueprint's mockup viewer and its screens) would break. Framing needs the
/// token URL anyway, and the framed page is still sandboxed.
pub fn view_csp(origin: Option<&str>, token: Option<&str>) -> String {
    let tail = "object-src 'none'; base-uri 'none'; form-action 'none'";
    let (Some(origin), Some(token)) = (origin, token) else {
        return format!("{SANDBOX}; default-src 'none'; style-src 'unsafe-inline'; {tail}");
    };
    let b = format!("{origin}/v/{token}/");
    [
        SANDBOX.to_owned(),
        "default-src 'none'".to_owned(),
        format!("script-src {b} 'unsafe-inline'"),
        format!("style-src {b} 'unsafe-inline'"),
        format!("img-src {b} data: blob:"),
        format!("font-src {b} data:"),
        format!("media-src {b}"),
        format!("connect-src {b}"),
        format!("frame-src {b}"),
        tail.to_owned(),
    ]
    .join("; ")
}

pub type HeaderList = Vec<(&'static str, String)>;

#[derive(Default)]
pub struct ViewHeaderInput<'a> {
    pub origin: Option<&'a str>,
    /// the token, once it verified; None before that
    pub token: Option<&'a str>,
    /// no-cache when false
    pub no_store: bool,
    pub content_type: Option<&'a str>,
    pub etag: Option<&'a str>,
    pub location: Option<&'a str>,
    pub allow: Option<&'a str>,
}

/// Every header a `/v/` answer carries, lowercase, in this order; the hub adds only what its server
/// adds by itself (date, content-length). The one place these are made, for every status.
pub fn view_headers(o: &ViewHeaderInput) -> HeaderList {
    let mut h: HeaderList = vec![
        ("content-security-policy", view_csp(o.origin, o.token)),
        ("access-control-allow-origin", "*".into()),
        ("referrer-policy", "no-referrer".into()),
        ("x-content-type-options", "nosniff".into()),
        // no DNS lookups for links the page shows: a host name can carry data out
        ("x-dns-prefetch-control", "off".into()),
        ("cache-control", if o.no_store { "no-store" } else { "no-cache" }.into()),
    ];
    for (k, v) in [("content-type", o.content_type), ("etag", o.etag), ("location", o.location), ("allow", o.allow)] {
        if let Some(v) = v.filter(|v| !v.is_empty()) {
            h.push((k, v.to_owned()));
        }
    }
    h
}

// ---------- error pages ----------

fn error_text(status: u16) -> (&'static str, &'static str) {
    match status {
        403 => ("Link expired", "Open the page again from Inyon."),
        405 => ("Read only", "Pages can be read, nothing else."),
        413 => ("Too big", "This file is too big to show here."),
        421 => ("Wrong address", "This hub doesn't answer to that address. If it's yours, add it to INYON_ALLOWED_HOSTS on the hub."),
        _ => ("Not found", "This page has no file at that address."),
    }
}

/// The small HTML page sent with an error (403, 404, 405, 413 or 421). Light or dark with the
/// system, no scripts.
pub fn error_page(status: u16) -> String {
    let (title, text) = error_text(status);
    format!(
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\"><title>{title}</title><style>:root{{color-scheme:light dark}}body{{margin:0;min-height:100vh;display:grid;place-items:center;font:15px/1.5 system-ui,sans-serif;background:Canvas;color:CanvasText}}main{{max-width:32rem;padding:24px}}h1{{font-size:18px;margin:0 0 4px}}p{{margin:0;opacity:.7}}</style><main><h1>{title}</h1><p>{text}</p></main></html>\n"
    )
}

// ---------- one request ----------

pub struct ViewRequest<'a> {
    pub method: &'a str,
    /// the request target: path and query as received
    pub target: &'a str,
    /// view_origin() of the request, or None when the hub doesn't answer to its Host (421)
    pub origin: Option<&'a str>,
    pub if_none_match: Option<&'a str>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FileStat {
    pub file: bool,
    pub dir: bool,
    pub size: u64,
    pub mtime_ms: f64,
}

/// What decide_view needs from the disk; None when the path doesn't exist. Both follow symlinks.
pub trait ViewFs {
    fn stat<'a>(&'a self, p: &'a Path) -> LocalBoxFuture<'a, Option<FileStat>>;
    fn realpath<'a>(&'a self, p: &'a Path) -> LocalBoxFuture<'a, Option<PathBuf>>;
}

pub struct ViewContext<'a> {
    pub verify: VerifyContext<'a>,
    /// the folder of a site this hub serves from its own disk, or None
    pub site_root: &'a dyn Fn(&str, &str) -> Option<PathBuf>,
    pub fs: &'a dyn ViewFs,
}

/// What to answer. A 200 names the file to stream (its real path) and the site's real folder it
/// was checked against; the others carry their whole body ("" for 301 and 304). On HEAD the hub
/// sends the same status and headers with no body.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewDecision {
    pub status: u16,
    pub headers: HeaderList,
    /// the 200's file and its size
    pub file: Option<(PathBuf, u64)>,
    /// the 200's site folder, as its real path
    pub site: Option<PathBuf>,
    pub body: String,
}

fn failure(origin: Option<&str>, status: u16, token: Option<&str>, allow: Option<&str>) -> ViewDecision {
    ViewDecision {
        status,
        headers: view_headers(&ViewHeaderInput { origin, token, no_store: true, content_type: Some("text/html;charset=utf-8"), allow, ..Default::default() }),
        file: None,
        site: None,
        body: error_page(status),
    }
}

/// The 404 for a 200 whose file, once opened, turned out not to be the one decide_view checked: a
/// folder on the way was swapped for a symlink in between. The hub opens the file and then asks the
/// system where the open file really is, so a swap can't lead it out of the site.
pub fn gone_after_open(req: &ViewRequest<'_>) -> ViewDecision {
    let token = split_view_target(req.target).map(|t| t.token);
    failure(req.origin, 404, token.as_deref(), None)
}

/// The 413 for a 200 whose file is on another machine and bigger than the hub fetches from it.
pub fn too_big(req: &ViewRequest<'_>) -> ViewDecision {
    let token = split_view_target(req.target).map(|t| t.token);
    failure(req.origin, 413, token.as_deref(), None)
}

/// The real path is the site's root or inside it, with no dot-named folder or file on the way.
pub fn within(root: &Path, real: &Path) -> bool {
    match real.strip_prefix(root) {
        Ok(rel) => rel.components().all(|c| matches!(c, Component::Normal(s) if !s.to_string_lossy().starts_with('.'))),
        Err(_) => false,
    }
}

/// One `/v/` request, start to end: the Host (421), the method (405), the token (403), the site
/// (404 when this hub doesn't serve it), the trailing slash after the token (301), the path rules
/// (404), the disk (a folder without its slash: 301; a folder without index.html, anything missing,
/// anything whose real path leaves the site or passes a dot name: 404), then the cache (304) or the
/// file (200).
pub async fn decide_view(req: &ViewRequest<'_>, ctx: &ViewContext<'_>) -> ViewDecision {
    let fail = |status: u16, token: Option<&str>, allow: Option<&str>| failure(req.origin, status, token, allow);
    if req.origin.is_none() {
        return fail(421, None, None);
    }
    if req.method != "GET" && req.method != "HEAD" {
        return fail(405, None, Some("GET, HEAD"));
    }
    let Some(t) = split_view_target(req.target) else { return fail(404, None, None) };
    let Ok(claims) = verify_token(&t.token, &ctx.verify) else { return fail(403, None, None) };
    let token = Some(t.token.as_str());
    let Some(root) = (ctx.site_root)(&claims.h, &claims.s) else { return fail(404, token, None) };
    let redirect = |segments: &[String]| {
        let path: String = segments.iter().map(|s| format!("{}/", encode_uri_component(s))).collect();
        let location = format!("/v/{}/{path}{}", t.token, kept_query(t.query.as_deref()));
        ViewDecision {
            status: 301,
            headers: view_headers(&ViewHeaderInput { origin: req.origin, token, no_store: true, location: Some(&location), ..Default::default() }),
            file: None,
            site: None,
            body: String::new(),
        }
    };
    if t.rest.is_empty() {
        return redirect(&[]);
    }
    let Some((segments, dir)) = view_segments(&t.rest) else { return fail(404, token, None) };

    let Some(site_real) = ctx.fs.realpath(&root).await else { return fail(404, token, None) };
    let joined = segments.iter().fold(root.clone(), |p, s| p.join(s));
    let mut real = match ctx.fs.realpath(&joined).await {
        Some(r) if within(&site_real, &r) => r,
        _ => return fail(404, token, None),
    };
    let mut st = ctx.fs.stat(&real).await;
    let mut name = segments.last().cloned().unwrap_or_else(|| "index.html".into());
    if st.is_some_and(|s| s.dir) {
        if !dir {
            return redirect(&segments);
        }
        real = match ctx.fs.realpath(&real.join("index.html")).await {
            Some(r) if within(&site_real, &r) => r,
            _ => return fail(404, token, None),
        };
        st = ctx.fs.stat(&real).await;
        name = "index.html".into();
    } else if dir {
        return fail(404, token, None);
    }
    let Some(st) = st.filter(|s| s.file) else { return fail(404, token, None) };

    let ty = content_type(&name);
    if no_store(&name) {
        let headers = view_headers(&ViewHeaderInput { origin: req.origin, token, no_store: true, content_type: Some(ty), ..Default::default() });
        return ViewDecision { status: 200, headers, file: Some((real, st.size)), site: Some(site_real), body: String::new() };
    }
    let etag = etag_of(st.size, st.mtime_ms);
    if etag_matches(req.if_none_match, &etag) {
        let headers = view_headers(&ViewHeaderInput { origin: req.origin, token, etag: Some(&etag), ..Default::default() });
        return ViewDecision { status: 304, headers, file: None, site: None, body: String::new() };
    }
    let headers = view_headers(&ViewHeaderInput { origin: req.origin, token, content_type: Some(ty), etag: Some(&etag), ..Default::default() });
    ViewDecision { status: 200, headers, file: Some((real, st.size)), site: Some(site_real), body: String::new() }
}

#[cfg(test)]
#[path = "pages_core_tests.rs"]
mod tests;
