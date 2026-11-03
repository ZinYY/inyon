//! core.ts's API: the routes the app uses for machines, sessions, settings, uploads and the rest,
//! each a thin reading of the request around inyon-core's features. The hub puts every one behind
//! sign-in (see `routes::guarded`).

use http::Method;
use inyon_core::agents::custom as custom_agents;
use inyon_core::claude::actions::{self, LaunchOpts};
use inyon_core::claude::integration::{install_integration, uninstall_integration};
use inyon_core::claude::send_receipts::send_once;
use inyon_core::config::{load_settings, save_settings, HubSettings};
use inyon_core::features::uploads::{self as chunked, ChunkResult, StartUpload};
use inyon_core::features::{companion, conversations, features as feat, models, providers, setup, signin, temp, workspace};
use inyon_core::hosts::http_error::{http, HubError, HubResult};
use inyon_core::hosts::node_link;
use inyon_core::hosts::node_update::node_updates;
use inyon_core::hosts::registry::{public_config, registry};
use inyon_core::hosts::removed_nodes::remember_removed;
use inyon_core::hosts::tidy::{tidy_machine, TidyState};
use inyon_protocol::{HostConfig, HostKind};
use serde_json::{json, Map, Value};

use crate::http::{as_str, field, fields, js_number, json_of, ok, ok_json, pane_param, str_or_empty, str_raw, to_int32, truthy, Req, Resp};
use crate::multipart::{self, Part};
use crate::routes::{guarded, h, Routes};
use inyon_core::claude::{chosen, held};

/// `validateHost(b)`: an SSH machine's settings, checked.
pub fn validate_host(b: &Map<String, Value>) -> HubResult<HostConfig> {
    let name = str_or_empty(b.get("name")).trim().to_owned();
    let target = str_or_empty(b.get("target")).trim().to_owned();
    if name.is_empty() {
        return Err(http(400, "Give the machine a name"));
    }
    // a leading "-" would be read by ssh as an option
    if !inyon_core::re!(r"^[\w.@-]+$").is_match(&target) || target.starts_with('-') {
        return Err(http(400, "Use user@host or an alias from ~/.ssh/config"));
    }
    let port = if truthy(b.get("port")) { Some(js_number(b.get("port"))) } else { None };
    if let Some(p) = port {
        #[allow(clippy::neg_cmp_op_on_partial_ord)]
        if !(p > 0.0 && p < 65536.0) {
            return Err(http(400, "Port must be 1-65535"));
        }
    }
    let identity_file = match b.get("identityFile") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.trim().to_owned()).filter(|s| !s.is_empty()),
        Some(_) => return Err(HubError::plain("b.identityFile?.trim is not a function")),
    };
    if let Some(f) = &identity_file {
        if !inyon_core::re!(r"^[\w./~-]+$").is_match(f) {
            return Err(http(400, "Key path has unexpected characters"));
        }
    }
    Ok(HostConfig { name, kind: HostKind::Ssh, target: Some(target), port: port.map(|p| p as u16), identity_file, ..empty_config() })
}

/// A machine with nothing set, to fill in.
pub fn empty_config() -> HostConfig {
    HostConfig {
        id: String::new(),
        name: String::new(),
        kind: HostKind::Ssh,
        target: None,
        port: None,
        identity_file: None,
        host_key: None,
        auth: None,
        via: None,
        token_hash: None,
        paired_at: None,
        token_issued_at: None,
        machine_id: None,
        urls: None,
        relays: None,
        keep_integration: None,
    }
}

/// A readable id from a name: lower case, runs of anything else as one dash.
pub fn slug(name: &str, fallback: &str) -> String {
    let lower = name.to_lowercase();
    let dashed = inyon_core::re!(r"[^a-z0-9]+").replace_all(&lower, "-");
    let id = dashed.strip_prefix('-').unwrap_or(&dashed);
    let id = id.strip_suffix('-').unwrap_or(id);
    if id.is_empty() {
        fallback.to_owned()
    } else {
        id.to_owned()
    }
}

/// `newHostId(name)`: unique among the machines already added.
pub fn new_host_id(name: &str, fallback: &str) -> String {
    let id = slug(name, fallback);
    let mut unique = id.clone();
    let mut i = 2;
    while registry().get(&unique).is_some() {
        unique = format!("{id}-{i}");
        i += 1;
    }
    unique
}

/// Writes now what would otherwise be saved a few seconds from now: open chats, drafts, and what
/// each machine looked like. For when the hub stops.
pub fn save_everything() {
    let saves: [(&str, fn()); 3] =
        [("chats", || inyon_core::claude::watcher::watchers().save_all()), ("drafts", inyon_core::claude::drafts::save_drafts), ("machines", || registry().save_all())];
    for (what, save) in saves {
        if std::panic::catch_unwind(save).is_err() {
            inyon_core::platform::console::error(&format!("saving before closing: the {what} could not be saved"));
        }
    }
}

/// Starts watching machines and the housekeeping timers (`startCore`).
pub fn start_core() {
    crate::clients::listen();
    registry().start();
    feat::start_upload_janitor();
    inyon_core::features::organize::start();
    inyon_core::claude::chat_cache::prune_chats(None);
    signin::start();
    use inyon_core::platform::rt::{set_timeout, sleep, spawn};
    spawn(async {
        loop {
            sleep(30_000).await;
            let _ = setup::ensure_sessions().await;
        }
    });
    let _ = set_timeout(5_000, || {
        spawn(async {
            let _ = setup::ensure_sessions().await;
        })
    });
}

fn id(r: &Req) -> String {
    r.param("id")
}

fn pane(r: &Req) -> String {
    pane_param(&r.param("pane"))
}

/// `typeof v === 'string' ? v : undefined`, for strings handed on as they came; other values go as
/// their text, as the TS passes `any` through.
fn loose(v: Option<&Value>) -> Option<String> {
    match v {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.clone()),
        Some(v) => Some(crate::http::js_string(v)),
    }
}

/// `Math.max(lo, Math.min(hi, Number(x) | 0))`
fn clamp(x: f64, lo: i32, hi: i32) -> i32 {
    to_int32(x).clamp(lo, hi)
}

fn settings_put(b: &Value, cur: HubSettings) -> HubResult<Value> {
    let b = fields(b)?;
    let u = match b.get("uploads") {
        None | Some(Value::Null) => Map::new(),
        Some(Value::Object(m)) => m.clone(),
        Some(_) => Map::new(),
    };
    let cu = cur.0.get("uploads").cloned().unwrap_or(Value::Null);
    let pick = |k: &str| js_number(u.get(k).filter(|v| !v.is_null()).or_else(|| cu.get(k)));
    let mut next = cur.0.clone();
    if let Some(Value::Bool(a)) = b.get("autoResume") {
        next.insert("autoResume".into(), json!(a));
    }
    // joined machines may hold messages for agents with `inyon send` (the hub's machines.rs)
    if let Some(Value::Bool(a)) = b.get("machinesSend") {
        next.insert("machinesSend".into(), json!(a));
    }
    // checked whole: a bad entry is refused rather than dropped (inyon_core::agents::custom)
    let custom = match b.get("customAgents") {
        None | Some(Value::Null) => None,
        Some(v) => Some(custom_agents::parse_list(v).map_err(|e| http(400, &e))?),
    };
    if let Some(list) = &custom {
        next.insert("customAgents".into(), serde_json::to_value(list).map_err(|e| HubError::plain(e.to_string()))?);
    }
    next.insert(
        "uploads".into(),
        json!({
            "retentionDays": clamp(pick("retentionDays"), 0, 365),
            "maxMB": clamp(pick("maxMB"), 1, 2048),
            "maxTotalMB": clamp(pick("maxTotalMB"), 0, 102400),
            "longPasteLines": clamp(pick("longPasteLines"), 5, 1000),
        }),
    );
    save_settings(&HubSettings(next.clone())).map_err(HubError::plain)?;
    if let Some(list) = custom {
        custom_agents::set(list);
    }
    Ok(Value::Object(next))
}

async fn upload(mut r: Req) -> HubResult<Resp> {
    // refuse an oversized upload as soon as it passes the limit, chunked or not
    let max_mb = load_settings().uploads().map_or(100.0, |u| u.max_mb);
    let limit = ((max_mb + 1.0) * 1024.0 * 1024.0) as usize;
    let message = format!("Uploads are limited to {} MB", inyon_core::js::fmt_number(max_mb));
    let bytes = match r.read_limited(limit, &message).await {
        Ok(b) => b,
        Err(e) if e.status == Some(413) => return Err(e),
        Err(_) => return Err(http(400, "Expected multipart form data")),
    };
    let form = multipart::parse(&r.header("content-type").unwrap_or_default(), &bytes).ok_or_else(|| http(400, "Expected multipart form data"))?;
    let files: Vec<feat::UploadFile> = form
        .all("files")
        .filter_map(|p| match p {
            Part::File { name, mime, data } => Some(feat::UploadFile::new(name.clone(), mime.clone(), data.clone())),
            Part::Text(_) => None,
        })
        .collect();
    if files.is_empty() {
        return Err(http(400, "No files"));
    }
    let pane_id = form.text("pane").filter(|p| !p.is_empty()).map(pane_param);
    if let Some(total) = form.text("total") {
        let offset = match form.get("offset") {
            None => 0.0,
            Some(Part::Text(s)) => inyon_core::js::to_number(s),
            Some(Part::File { .. }) => f64::NAN,
        };
        let at = feat::PieceAt { total: inyon_core::js::to_number(total), offset, path: form.text("path").map(str::to_owned) };
        let one = feat::upload_piece(&id(&r), pane_id.as_deref(), &files[0], &at).await?;
        return Ok(json_of(&json!({ "files": [one] })));
    }
    let all = feat::upload_files(&id(&r), pane_id.as_deref(), &files).await?;
    Ok(json_of(&json!({ "files": all })))
}

// ---------- uploads a chunk at a time (from 0.24; see inyon_core::features::uploads) ----------

/// `POST /api/hosts/:id/uploads/start` `{ name, size, type?, pane?, sha256? }`: the upload's id,
/// the chunk size to send and where it goes.
async fn upload_start(mut r: Req) -> HubResult<Resp> {
    let b = r.json_body(16 * 1024).await?;
    let name = str_or_empty(field(&b, "name")?).trim().to_owned();
    if name.is_empty() {
        return Err(http(400, "An upload needs the file's name"));
    }
    let size = js_number(field(&b, "size")?);
    if !(size >= 0.0 && size.fract() == 0.0 && size <= 9e15) {
        return Err(http(400, "An upload needs the file's size"));
    }
    let sha256 = match as_str(field(&b, "sha256")?).filter(|s| !s.is_empty()) {
        Some(s) => Some(chunked::parse_sha256(&s).ok_or_else(|| http(400, "A SHA-256 is 64 hex digits"))?),
        None => None,
    };
    let pane = as_str(field(&b, "pane")?).filter(|p| !p.is_empty()).map(|p| pane_param(&p));
    let mime = as_str(field(&b, "type")?).unwrap_or_default().to_ascii_lowercase();
    let start = StartUpload { name, mime, size: size as u64, sha256, pane };
    Ok(json_of(&chunked::start_upload(&id(&r), &start).await?))
}

/// `PUT /api/hosts/:id/uploads/:upload/:offset` with the chunk's bytes as the body. A chunk that
/// doesn't follow on from what arrived gets 409 with the upload's state, to carry on from `received`.
async fn upload_chunk(mut r: Req) -> HubResult<Resp> {
    let offset = r.param("offset");
    if offset.is_empty() || offset.len() > 15 || !offset.bytes().all(|b| b.is_ascii_digit()) {
        return Err(http(400, "Bad upload offset"));
    }
    let message = format!("Upload chunks are limited to {} MB", chunked::MAX_CHUNK / 1024 / 1024);
    let data = match r.read_limited(chunked::MAX_CHUNK, &message).await {
        Ok(b) => b,
        Err(e) if e.status == Some(413) => return Err(e),
        // the connection dropped while the chunk was arriving: the app sends it again
        Err(_) => return Err(http(400, "The chunk didn't arrive whole")),
    };
    match chunked::put_chunk(&id(&r), &r.param("upload"), offset.parse().unwrap_or(0), &data).await? {
        ChunkResult::Took(s) => Ok(json_of(&s)),
        ChunkResult::Elsewhere(s) => {
            let mut body = serde_json::to_value(&s).map_err(|e| HubError::plain(e.to_string()))?;
            body["error"] = json!(format!("The upload carries on from byte {}", s.received));
            Ok(crate::http::json(409, &body))
        }
    }
}

/// `POST /api/hosts/:id/uploads/:upload/finish` `{ sha256 }`: the file as the old route answers
/// for one (`{ name, path, size, type }`), once its size and hash are right.
async fn upload_finish(mut r: Req) -> HubResult<Resp> {
    let b = r.json_body(16 * 1024).await?;
    let sha256 = match as_str(field(&b, "sha256")?).filter(|s| !s.is_empty()) {
        Some(s) => Some(chunked::parse_sha256(&s).ok_or_else(|| http(400, "A SHA-256 is 64 hex digits"))?),
        None => None,
    };
    Ok(json_of(&chunked::finish_upload(&id(&r), &r.param("upload"), sha256).await?))
}

async fn node_update(r: Req) -> HubResult<Resp> {
    let rt = registry().get(&id(&r)).ok_or_else(|| http(404, "Unknown machine"))?;
    let refused = {
        let b = rt.borrow();
        let (host, state) = (b.host.clone(), b.state.clone());
        drop(b);
        node_updates().start(&host, &state)
    };
    if let Some(why) = refused {
        return Err(http(409, why));
    }
    Ok(ok())
}

async fn temp_list(r: Req) -> HubResult<Resp> {
    Ok(json_of(&temp::temp_list(&id(&r)).await?))
}

async fn temp_files(r: Req) -> HubResult<Resp> {
    Ok(json_of(&temp::temp_files(&id(&r), &r.q("path")).await?))
}

async fn temp_preview(r: Req) -> HubResult<Resp> {
    // `Number(q) || undefined`: no limit asked for, or not a number, is the default
    let max = Some(inyon_core::js::to_number(&r.q("max"))).filter(|m| *m != 0.0 && !m.is_nan());
    Ok(json_of(&temp::temp_preview(&id(&r), &r.q("path"), max).await?))
}

async fn temp_delete(mut r: Req) -> HubResult<Resp> {
    let b = r.body().await?;
    Ok(json_of(&temp::temp_delete(&id(&r), field(&b, "paths")?).await?))
}

/// The routes, each behind sign-in.
/// What the overview shows, as the WebSocket's overview message carries it (a companion app reads this too).
pub fn overview(catalog: u32) -> Resp {
    let hosts = registry().overview();
    inyon_core::features::organize::reconcile(&hosts);
    ok_json(&json!({ "hosts": inyon_protocol::compat::hosts_for(&hosts, catalog), "organize": inyon_core::features::organize::for_overview() }))
}

pub fn add(routes: &mut Routes) {
    use Method as M;
    guarded(routes, "/api/overview", vec![(M::GET, h(|r| async move { Ok(overview(r.catalog())) }))]);
    // sends that failed, newest first, with the bottom of the screen each failed on (from 0.22)
    guarded(routes, "/api/send-failures", vec![(M::GET, h(|_r| async move { Ok(json_of(&inyon_core::claude::send_check::failures())) }))]);
    guarded(
        routes,
        "/api/refresh",
        vec![(
            M::POST,
            h(|_r| async {
                registry().refresh_all(6000).await;
                Ok(ok())
            }),
        )],
    );
    guarded(
        routes,
        "/api/hosts",
        vec![
            (M::GET, h(|_r| async { Ok(json_of(&registry().configs().iter().map(public_config).collect::<Vec<_>>())) })),
            (
                M::POST,
                h(|mut r| async move {
                    let mut cfg = validate_host(&fields(&r.body().await?)?)?;
                    let id = new_host_id(&cfg.name, "host");
                    cfg.id = id.clone();
                    registry().add(cfg, true)?;
                    Ok(ok_json(&json!({ "id": id })))
                }),
            ),
        ],
    );
    guarded(
        routes,
        "/api/hosts/:id",
        vec![
            (
                M::PATCH,
                h(|mut r| async move {
                    let id = id(&r);
                    let cur = registry().get(&id).map(|rt| rt.borrow().host.config()).ok_or_else(|| http(404, "Unknown machine"))?;
                    let b = r.body().await?;
                    if cur.kind != HostKind::Ssh {
                        let given = field(&b, "name")?.filter(|v| !v.is_null()).map(crate::http::js_string).unwrap_or_else(|| cur.name.clone());
                        let name = Some(given.trim().to_owned()).filter(|n| !n.is_empty()).unwrap_or_else(|| cur.name.clone());
                        registry().update(&id, |c| c.name = name)?;
                    } else {
                        let mut merged = match serde_json::to_value(&cur) {
                            Ok(Value::Object(m)) => m,
                            _ => Map::new(),
                        };
                        for (k, v) in fields(&b)? {
                            merged.insert(k, v);
                        }
                        let next = validate_host(&merged)?;
                        // a new address or key: the connection kept open to the old one goes
                        if next.target != cur.target || next.port != cur.port || next.identity_file != cur.identity_file {
                            if let Some(rt) = registry().get(&id) {
                                let host = rt.borrow().host.clone();
                                host.close().await?;
                            }
                        }
                        registry().update(&id, |c| {
                            c.name = next.name;
                            c.kind = HostKind::Ssh;
                            c.target = next.target;
                            c.port = next.port;
                            c.identity_file = next.identity_file;
                        })?;
                    }
                    Ok(ok())
                }),
            ),
            (
                M::DELETE,
                h(|r| async move {
                    let id = id(&r);
                    if id == "local" {
                        return Err(http(400, "This machine cannot be removed"));
                    }
                    let rt = registry().get(&id);
                    let token = rt.as_ref().and_then(|rt| rt.borrow().host.config().token_hash);
                    // hooks, uploads and the open connection come off the machine first, while it still answers
                    if let Some(rt) = rt {
                        let (host, state) = {
                            let b = rt.borrow();
                            (b.host.clone(), TidyState::from(&b.state))
                        };
                        tidy_machine(&host, Some(state), "").await;
                    }
                    remember_removed(token.as_deref());
                    node_link::disconnect(&id);
                    registry().remove(&id, true)?;
                    Ok(ok())
                }),
            ),
        ],
    );
    guarded(
        routes,
        "/api/hosts/:id/integration",
        vec![(
            M::POST,
            h(|mut r| async move {
                let b = r.body().await?;
                let enable = truthy(field(&b, "enable")?);
                let host = registry().host(&id(&r))?;
                if enable {
                    install_integration(&host).await?;
                } else {
                    uninstall_integration(&host).await?;
                }
                registry().set_keep_integration(&id(&r), enable)?;
                registry().refresh(&id(&r), 120);
                Ok(ok())
            }),
        )],
    );
    // one tap in the app: brings the machine's inyon up to date (see hosts/node_update)
    guarded(routes, "/api/hosts/:id/node/update", vec![(M::POST, h(node_update))]);
    guarded(
        routes,
        "/api/hosts/:id/launch",
        vec![(
            M::POST,
            h(|mut r| async move {
                let b = fields(&r.body().await?)?;
                let agent = loose(b.get("agent")).or_else(|| Some(if truthy(b.get("claude")) { "claude" } else { "shell" }.to_owned()));
                let opts = LaunchOpts {
                    session: loose(b.get("session")),
                    name: loose(b.get("name")),
                    cwd: loose(b.get("cwd")),
                    agent,
                    model: loose(b.get("model")),
                    mode: loose(b.get("mode")),
                    prompt: loose(b.get("prompt")),
                    custom: loose(b.get("custom")),
                    sign_in: truthy(b.get("signIn")),
                    ..Default::default()
                };
                let pane = actions::launch(&id(&r), &opts).await?;
                Ok(ok_json(&json!({ "pane": pane })))
            }),
        )],
    );
    // the companion agent (features/companion): whether it runs, start it with an agent, stop it
    guarded(
        routes,
        "/api/companion",
        vec![
            (M::GET, h(|_r| async { Ok(ok_json(&companion::status())) })),
            (
                M::POST,
                h(|mut r| async move {
                    let b = fields(&r.body().await?)?;
                    let agent = loose(b.get("agent")).unwrap_or_else(|| "claude".into());
                    Ok(ok_json(&companion::start(&agent).await?))
                }),
            ),
            (
                M::DELETE,
                h(|_r| async {
                    companion::stop().await?;
                    Ok(ok())
                }),
            ),
        ],
    );
    guarded(
        routes,
        "/api/settings",
        vec![
            (M::GET, h(|_r| async { Ok(ok_json(&Value::Object(load_settings().0))) })),
            (
                M::PUT,
                h(|mut r| async move {
                    let b = r.body().await?;
                    Ok(ok_json(&settings_put(&b, load_settings())?))
                }),
            ),
        ],
    );
    guarded(routes, "/api/hosts/:id/uploads", vec![(M::GET, h(|r| async move { Ok(json_of(&feat::upload_usage(&id(&r)).await?)) })), (M::POST, h(upload))]);
    // a chunk at a time, resumable (from 0.24); the route above stays for apps from before
    guarded(routes, "/api/hosts/:id/uploads/start", vec![(M::POST, h(upload_start))]);
    guarded(
        routes,
        "/api/hosts/:id/uploads/:upload",
        vec![
            (M::GET, h(|r| async move { Ok(json_of(&chunked::upload_status(&id(&r), &r.param("upload"))?)) })),
            (
                M::DELETE,
                h(|r| async move {
                    chunked::cancel_upload(&id(&r), &r.param("upload")).await?;
                    Ok(ok())
                }),
            ),
        ],
    );
    guarded(routes, "/api/hosts/:id/uploads/:upload/finish", vec![(M::POST, h(upload_finish))]);
    guarded(routes, "/api/hosts/:id/uploads/:upload/:offset", vec![(M::PUT, h(upload_chunk))]);
    guarded(
        routes,
        "/api/hosts/:id/uploads/clean",
        vec![(
            M::POST,
            h(|mut r| async move {
                let b = r.body().await?;
                let days = js_number(Some(field(&b, "olderThanDays")?.filter(|v| !v.is_null()).unwrap_or(&json!(0))));
                Ok(json_of(&feat::clean_uploads(&id(&r), days).await?))
            }),
        )],
    );
    // agent temp files, through the machine's own `inyon temp` (features/temp)
    guarded(routes, "/api/hosts/:id/temp", vec![(M::GET, h(temp_list))]);
    guarded(routes, "/api/hosts/:id/temp/files", vec![(M::GET, h(temp_files))]);
    guarded(routes, "/api/hosts/:id/temp/preview", vec![(M::GET, h(temp_preview))]);
    guarded(routes, "/api/hosts/:id/temp/delete", vec![(M::POST, h(temp_delete))]);
    guarded(
        routes,
        "/api/hosts/:id/setup",
        vec![
            (
                M::GET,
                h(|r| async move {
                    let mut report = setup::setup_report(&id(&r)).await?;
                    report.agents.retain(|a| a.kind.known_to(r.catalog()));
                    Ok(json_of(&report))
                }),
            ),
            (
                M::POST,
                h(|mut r| async move {
                    let b = r.body().await?;
                    if let Some(Value::Bool(keep)) = field(&b, "keepSession")? {
                        setup::set_keep_session(&id(&r), *keep)?;
                        return Ok(ok());
                    }
                    Ok(json_of(&setup::run_setup(&id(&r), &str_or_empty(field(&b, "action")?)).await?))
                }),
            ),
        ],
    );
    guarded(
        routes,
        "/api/hosts/:id/signin",
        vec![(
            M::POST,
            h(|mut r| async move {
                let b = r.body().await?;
                Ok(json_of(&signin::start_signin(&id(&r), &str_raw(field(&b, "agent")?), None).await?))
            }),
        )],
    );
    guarded(
        routes,
        "/api/hosts/:id/signin/:pane",
        vec![
            (M::GET, h(|r| async move { Ok(json_of(&signin::read_signin(&id(&r), &pane(&r)).await?)) })),
            (
                M::POST,
                h(|mut r| async move {
                    let b = r.body().await?;
                    signin::answer_signin(&id(&r), &pane(&r), &b).await?;
                    Ok(ok())
                }),
            ),
            (
                M::DELETE,
                h(|r| async move {
                    signin::stop_signin(&id(&r), &pane(&r)).await?;
                    Ok(ok())
                }),
            ),
        ],
    );
    // OpenCode's model providers on the machine; `pane` names a session whose own server can list them
    guarded(
        routes,
        "/api/hosts/:id/providers",
        vec![(
            M::GET,
            h(|r| async move {
                let p = r.q("pane");
                let pane = (!p.is_empty()).then(|| pane_param(&p));
                Ok(json_of(&providers::list_providers(&id(&r), pane.as_deref()).await?))
            }),
        )],
    );
    guarded(
        routes,
        "/api/hosts/:id/providers/:provider",
        vec![
            (M::POST, h(|r| async move { Ok(json_of(&providers::connect_provider(&id(&r), &r.param("provider")).await?)) })),
            (
                M::DELETE,
                h(|r| async move {
                    providers::disconnect_provider(&id(&r), &r.param("provider")).await?;
                    Ok(ok())
                }),
            ),
        ],
    );
    guarded(routes, "/api/hosts/:id/configs", vec![(M::GET, h(|r| async move { Ok(json_of(&feat::list_configs(&id(&r)).await?)) }))]);
    guarded(
        routes,
        "/api/hosts/:id/config",
        vec![
            (M::GET, h(|r| async move { Ok(json_of(&feat::read_config(&id(&r), &r.q("path")).await?)) })),
            (
                M::PUT,
                h(|mut r| async move {
                    let b = r.body().await?;
                    Ok(json_of(&feat::write_config(&id(&r), &str_raw(field(&b, "path")?), &str_or_empty(field(&b, "content")?)).await?))
                }),
            ),
        ],
    );
    guarded(routes, "/api/hosts/:id/subagents", vec![(M::GET, h(|r| async move { Ok(json_of(&feat::list_subagents(&id(&r), &r.q("transcript")).await?)) }))]);
    guarded(routes, "/api/hosts/:id/transcript", vec![(M::GET, h(|r| async move { Ok(json_of(&feat::read_transcript(&id(&r), &r.q("path")).await?)) }))]);
    guarded(
        routes,
        "/api/hosts/:id/task-output",
        vec![(
            M::GET,
            h(|r| async move {
                let given = r.q("path");
                let path = if given.is_empty() { actions::find_task_output(&id(&r), &r.q("id")).await?.unwrap_or_default() } else { given };
                if path.is_empty() {
                    return Err(http(404, "No output saved for that task"));
                }
                Ok(json_of(&feat::task_output(&id(&r), &path).await?))
            }),
        )],
    );
    guarded(routes, "/api/hosts/:id/panes/:pane/commands", vec![(M::GET, h(|r| async move { Ok(json_of(&feat::list_commands(&id(&r), &pane(&r)).await?)) }))]);
    guarded(routes, "/api/hosts/:id/panes/:pane/models", vec![(M::GET, h(|r| async move { Ok(json_of(&models::list_models(&id(&r), &pane(&r)).await?)) }))]);
    // the session's folder: its files (read only) and its git working tree
    guarded(routes, "/api/hosts/:id/panes/:pane/dir", vec![(M::GET, h(|r| async move { Ok(json_of(&workspace::list_dir(&id(&r), &pane(&r), &r.q("path")).await?)) }))]);
    guarded(routes, "/api/hosts/:id/panes/:pane/file", vec![(M::GET, h(|r| async move { Ok(json_of(&workspace::read_file(&id(&r), &pane(&r), &r.q("path")).await?)) }))]);
    guarded(routes, "/api/hosts/:id/panes/:pane/git", vec![(M::GET, h(|r| async move { Ok(json_of(&workspace::git_status(&id(&r), &pane(&r)).await?)) }))]);
    guarded(routes, "/api/hosts/:id/panes/:pane/git/diff", vec![(M::GET, h(|r| async move { Ok(json_of(&workspace::git_diff(&id(&r), &pane(&r), &r.q("path")).await?)) }))]);
    guarded(
        routes,
        "/api/hosts/:id/panes/:pane/git/commit",
        vec![(
            M::POST,
            h(|mut r| async move {
                let b = r.body().await?;
                let message = field(&b, "message")?.cloned();
                Ok(json_of(&workspace::git_commit(&id(&r), &pane(&r), message.as_ref()).await?))
            }),
        )],
    );
    guarded(routes, "/api/hosts/:id/panes/:pane/git/push", vec![(M::POST, h(|r| async move { Ok(json_of(&workspace::git_push(&id(&r), &pane(&r)).await?)) }))]);
    guarded(
        routes,
        "/api/hosts/:id/panes/:pane/fork",
        vec![(M::POST, h(|r| async move { Ok(ok_json(&json!({ "pane": actions::fork_conversation(&id(&r), &pane(&r)).await? }))) }))],
    );
    // a session's latest messages, condensed (`inyon read`)
    guarded(
        routes,
        "/api/hosts/:id/panes/:pane/history",
        vec![(M::GET, h(|r| async move { Ok(json_of(&companion::history(&id(&r), &pane(&r), history_limit(&r.q("limit"))).await?)) }))],
    );
    guarded(routes, "/api/hosts/:id/panes/:pane/files", vec![(M::GET, h(|r| async move { Ok(json_of(&feat::list_files(&id(&r), &pane(&r)).await?)) }))]);
    guarded(
        routes,
        "/api/hosts/:id/panes/:pane/input",
        vec![(
            M::POST,
            h(|mut r| async move {
                let b = r.body().await?;
                actions::input_answer(&id(&r), &pane(&r), &str_or_empty(field(&b, "text")?)).await?;
                Ok(ok())
            }),
        )],
    );
    guarded(
        routes,
        "/api/hosts/:id/tasks/stop",
        vec![(
            M::POST,
            h(|mut r| async move {
                let b = r.body().await?;
                let path = field(&b, "path")?.filter(|v| truthy(Some(v))).map(crate::http::js_string);
                let tid = loose(field(&b, "id")?);
                Ok(json_of(&actions::stop_task(&id(&r), path.as_deref(), tid.as_deref()).await?))
            }),
        )],
    );
    guarded(
        routes,
        "/api/hosts/:id/panes/:pane/rc",
        vec![(
            M::POST,
            h(|mut r| async move {
                let b = r.body().await?;
                actions::remote_control(&id(&r), &pane(&r), truthy(field(&b, "on")?)).await?;
                Ok(ok())
            }),
        )],
    );
    guarded(
        routes,
        "/api/hosts/:id/panes/:pane/mode",
        vec![(
            M::POST,
            h(|mut r| async move {
                let b = r.body().await?;
                let mode = str_raw(field(&b, "mode")?);
                if !chosen::hold_if_busy(&id(&r), &pane(&r), chosen::Pick::Mode, &mode)? {
                    actions::set_mode(&id(&r), &pane(&r), &mode).await?;
                }
                Ok(ok())
            }),
        )],
    );
    // a model, effort or mode waiting for the agent to be free (from 0.22; see inyon_core::claude::chosen)
    guarded(
        routes,
        "/api/hosts/:id/panes/:pane/chosen",
        vec![(
            M::DELETE,
            h(|r| async move {
                chosen::drop_all(&id(&r), &pane(&r));
                Ok(ok())
            }),
        )],
    );
    guarded(
        routes,
        "/api/hosts/:id/panes/:pane/ultracode",
        vec![(
            M::POST,
            h(|mut r| async move {
                let b = r.body().await?;
                actions::set_ultracode(&id(&r), &pane(&r), truthy(field(&b, "on")?)).await?;
                Ok(ok())
            }),
        )],
    );
    guarded(routes, "/api/hosts/:id/panes/:pane/send", vec![(M::POST, h(send))]);
    // messages the hub holds until the agent is free (from 0.22; see inyon_core::claude::held)
    guarded(
        routes,
        "/api/hosts/:id/panes/:pane/held",
        vec![(
            M::POST,
            h(|mut r| async move {
                let b = r.body().await?;
                let text = as_str(field(&b, "text")?).unwrap_or_default();
                Ok(json_of(&held::hold(&id(&r), &pane(&r), &text)?))
            }),
        )],
    );
    guarded(
        routes,
        "/api/hosts/:id/panes/:pane/held/:held",
        vec![
            (
                M::PATCH,
                h(|mut r| async move {
                    let b = r.body().await?;
                    if let Some(text) = as_str(field(&b, "text")?) {
                        held::edit(&id(&r), &pane(&r), &r.param("held"), &text)?;
                    }
                    let to = js_number(field(&b, "to")?);
                    if to.is_finite() && to >= 0.0 {
                        held::move_to(&id(&r), &pane(&r), &r.param("held"), to as usize)?;
                    }
                    Ok(ok())
                }),
            ),
            (
                M::DELETE,
                h(|r| async move {
                    held::remove(&id(&r), &pane(&r), &r.param("held"))?;
                    Ok(ok())
                }),
            ),
        ],
    );
    guarded(
        routes,
        "/api/hosts/:id/panes/:pane/held/:held/send",
        vec![(
            M::POST,
            h(|r| async move {
                held::send_now(&id(&r), &pane(&r), &r.param("held")).await?;
                Ok(ok())
            }),
        )],
    );
    guarded(
        routes,
        "/api/hosts/:id/panes/:pane/answer",
        vec![(
            M::POST,
            h(|mut r| async move {
                let b = r.body().await?;
                let option = js_number(field(&b, "option")?);
                let option = if option.is_finite() && option >= 0.0 { option as u32 } else { u32::MAX };
                let text = loose(field(&b, "text")?);
                let question = as_str(field(&b, "question")?);
                actions::answer(&id(&r), &pane(&r), option, text.as_deref(), question.as_deref()).await?;
                Ok(ok())
            }),
        )],
    );
    guarded(
        routes,
        "/api/hosts/:id/panes/:pane/keys",
        vec![(
            M::POST,
            h(|mut r| async move {
                let b = r.body().await?;
                let Some(Value::Array(list)) = field(&b, "keys")? else { return Err(http(400, "keys must be a short list")) };
                if list.len() > 20 {
                    return Err(http(400, "keys must be a short list"));
                }
                let keys: Vec<String> = list.iter().map(crate::http::js_string).collect();
                actions::keys(&id(&r), &pane(&r), &keys).await?;
                Ok(ok())
            }),
        )],
    );
    guarded(
        routes,
        "/api/hosts/:id/panes/:pane/interrupt",
        vec![(
            M::POST,
            h(|r| async move {
                actions::interrupt(&id(&r), &pane(&r)).await?;
                Ok(ok())
            }),
        )],
    );
    guarded(
        routes,
        "/api/hosts/:id/panes/:pane/name",
        vec![(
            M::PUT,
            h(|mut r| async move {
                let b = r.body().await?;
                actions::rename_pane(&id(&r), &pane(&r), &str_or_empty(field(&b, "name")?)).await?;
                Ok(ok())
            }),
        )],
    );
    guarded(
        routes,
        "/api/hosts/:id/sessions/:session/name",
        vec![(
            M::PUT,
            h(|mut r| async move {
                let b = r.body().await?;
                let name = actions::rename_tmux_session(&id(&r), &r.param("session"), &str_or_empty(field(&b, "name")?)).await?;
                Ok(json_of(&json!({ "name": name })))
            }),
        )],
    );
    guarded(
        routes,
        "/api/hosts/:id/panes/:pane/reconnect",
        vec![(
            M::POST,
            h(|r| async move {
                actions::reconnect_agent(&id(&r), &pane(&r)).await?;
                Ok(ok())
            }),
        )],
    );
    guarded(
        routes,
        "/api/hosts/:id/panes/:pane/restart",
        vec![(
            M::POST,
            h(|r| async move {
                actions::restart_agent(&id(&r), &pane(&r)).await?;
                Ok(ok())
            }),
        )],
    );
    guarded(
        routes,
        "/api/hosts/:id/panes/:pane/exit",
        vec![(
            M::POST,
            h(|r| async move {
                actions::exit_agent(&id(&r), &pane(&r), false).await?;
                Ok(ok())
            }),
        )],
    );
    guarded(routes, "/api/hosts/:id/panes/:pane/end", vec![(M::POST, h(|r| async move { Ok(json_of(&actions::end_session(&id(&r), &pane(&r)).await?)) }))]);
    guarded(
        routes,
        "/api/hosts/:id/panes/:pane/rewind",
        vec![(
            M::POST,
            h(|mut r| async move {
                let b = r.body().await?;
                let text = as_str(field(&b, "text")?);
                actions::codex_rewind(&id(&r), &pane(&r), text.as_deref()).await?;
                Ok(ok())
            }),
        )],
    );
    guarded(
        routes,
        "/api/hosts/:id/panes/:pane/start",
        vec![(
            M::POST,
            h(|mut r| async move {
                let b = r.body().await?;
                let agent = field(&b, "agent")?.cloned();
                let resume = field(&b, "resume")?.cloned();
                actions::start_agent(&id(&r), &pane(&r), agent.as_ref(), resume.as_ref()).await?;
                Ok(ok())
            }),
        )],
    );
    // the agent's past conversations in the pane's folder; `agent` says whose when none is running
    guarded(
        routes,
        "/api/hosts/:id/panes/:pane/conversations",
        vec![(
            M::GET,
            h(|r| async move {
                let a = r.q("agent");
                let asked = (!a.is_empty()).then(|| Value::String(a));
                Ok(json_of(&conversations::pane_conversations(&id(&r), &pane(&r), asked.as_ref()).await?))
            }),
        )],
    );
    // exits the running agent (the pane stays) and reopens it on another of its conversations
    guarded(
        routes,
        "/api/hosts/:id/panes/:pane/switch",
        vec![(
            M::POST,
            h(|mut r| async move {
                let b = r.body().await?;
                let agent = field(&b, "agent")?.filter(|v| truthy(Some(v))).cloned();
                let sid = field(&b, "id")?.cloned();
                Ok(json_of(&conversations::switch_conversation(&id(&r), &pane(&r), agent.as_ref(), sid.as_ref()).await?))
            }),
        )],
    );
    crate::organize_routes::add(routes);
}

async fn send(mut r: Req) -> HubResult<Resp> {
    let b = r.body().await?;
    let text = str_or_empty(field(&b, "text")?);
    let request_id = field(&b, "requestId")?.cloned();
    let setting = field(&b, "setting")? == Some(&Value::Bool(true));
    let steer = field(&b, "steer")? == Some(&Value::Bool(true));
    let request_id = match request_id {
        None => None,
        Some(Value::String(s)) if inyon_core::re!(r"^[\w-]{1,128}$").is_match(&s) => Some(s),
        Some(_) => return Err(http(400, "Invalid send id")),
    };
    let (host, pane_id) = (id(&r), pane(&r));
    // a model or effort picked while the agent can't take it waits for it
    if let Some((pick, value)) = chosen::from_setting(&text).filter(|_| setting) {
        if chosen::hold_if_busy(&host, &pane_id, pick, &value)? {
            return Ok(ok());
        }
    }
    match request_id {
        Some(rid) if !rid.is_empty() => {
            let key = inyon_core::js::stringify(&json!([host, r.param("pane"), rid]));
            let t = text.clone();
            send_once(&key, &text, move || {
                Box::pin(async move {
                    actions::send_message(&host, &pane_id, &t, true, Some(&rid), setting, steer).await?;
                    Ok(Value::Null)
                })
            })
            .await?;
        }
        _ => actions::send_message(&host, &pane_id, &text, false, None, setting, steer).await?,
    }
    Ok(ok())
}

/// `?limit=`: how many entries `inyon read` asked for, 20 when it didn't say.
pub fn history_limit(q: &str) -> usize {
    q.parse().unwrap_or(20)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    // saving.test.ts

    fn draft(text: &str, at: u32) -> Option<inyon_core::claude::drafts::Draft> {
        Some(serde_json::from_value(json!({ "text": text, "source": "app", "at": at })).unwrap())
    }

    fn saved(key: &str) -> Option<String> {
        let all: Map<String, Value> = inyon_core::platform::store::kv::get("drafts").unwrap_or_default();
        all.get(key).and_then(|d| d.get("text")).and_then(Value::as_str).map(str::to_owned)
    }

    #[test]
    fn a_draft_is_written_a_moment_later_or_at_once_when_the_hub_is_about_to_close() {
        crate::test_util::block(async {
            inyon_core::claude::drafts::set_draft("box:%7", draft("half a thought", 1));
            // typing writes on a short wait, not on every key
            assert_eq!(saved("box:%7"), None);
            save_everything();
            assert_eq!(saved("box:%7").as_deref(), Some("half a thought"));
        });
    }

    #[test]
    fn a_machines_latest_sessions_are_saved_when_the_hub_is_about_to_close() {
        crate::test_util::block(async {
            put(host_state("snap-test", "ssh", json!({ "sessions": [{ "id": "$1", "name": "new one", "attached": 0, "windows": [] }] })));
            save_everything();
            let saved: Value = inyon_core::platform::store::kv::get("snapshots/snap-test").unwrap();
            let names: Vec<&str> = saved["sessions"].as_array().unwrap().iter().filter_map(|s| s["name"].as_str()).collect();
            registry().remove("snap-test", true).unwrap();
            assert_eq!(names, ["new one"]);
        });
    }

    #[test]
    fn while_the_phone_app_is_in_the_background_every_draft_is_written_straight_away() {
        crate::test_util::block(async {
            inyon_core::claude::drafts::save_drafts_at_once(true);
            inyon_core::claude::drafts::set_draft("box:%8", draft("typed just before leaving", 2));
            assert_eq!(saved("box:%8").as_deref(), Some("typed just before leaving"));
            inyon_core::claude::drafts::set_draft("box:%8", None);
            assert_eq!(saved("box:%8"), None);
            inyon_core::claude::drafts::save_drafts_at_once(false);
        });
    }

    #[test]
    fn a_host_needs_a_name_a_target_and_a_sane_port() {
        crate::test_util::core();
        assert_eq!(validate_host(&obj(json!({ "target": "a@b" }))).unwrap_err().message, "Give the machine a name");
        assert_eq!(validate_host(&obj(json!({ "name": "x", "target": "-oProxy" }))).unwrap_err().message, "Use user@host or an alias from ~/.ssh/config");
        assert_eq!(validate_host(&obj(json!({ "name": "x", "target": "a b" }))).unwrap_err().message, "Use user@host or an alias from ~/.ssh/config");
        assert_eq!(validate_host(&obj(json!({ "name": "x", "target": "a@b", "port": 70000 }))).unwrap_err().message, "Port must be 1-65535");
        assert_eq!(validate_host(&obj(json!({ "name": "x", "target": "a@b", "identityFile": "~/k;rm" }))).unwrap_err().message, "Key path has unexpected characters");
        let ok = validate_host(&obj(json!({ "name": " Box ", "target": "me@box", "port": "2222", "identityFile": " " }))).unwrap();
        assert_eq!((ok.name.as_str(), ok.target.as_deref(), ok.port, ok.identity_file), ("Box", Some("me@box"), Some(2222), None));
    }

    #[test]
    fn ids_are_slugs() {
        crate::test_util::core();
        assert_eq!(slug("My Box!", "host"), "my-box");
        assert_eq!(slug("--", "host"), "host");
        assert_eq!(slug("Ünï", "node"), "n");
    }

    #[test]
    fn settings_are_clamped_and_other_keys_kept() {
        crate::test_util::core();
        let cur = HubSettings(obj(
            json!({ "keepSession": { "a": true }, "uploads": { "retentionDays": 7, "maxMB": 100, "maxTotalMB": 1024, "longPasteLines": 40, "extra": 1 }, "other": "x" }),
        ));
        let next = settings_put(&json!({ "autoResume": true, "machinesSend": "yes", "uploads": { "retentionDays": 999, "maxMB": "0", "longPasteLines": 2.9 } }), cur);
        // saving needs a store; the clamped value is what matters here
        let next = match next {
            Ok(v) => v,
            Err(e) => panic!("{e}"),
        };
        assert_eq!(
            inyon_core::js::stringify(&next),
            r#"{"keepSession":{"a":true},"uploads":{"retentionDays":365,"maxMB":1,"maxTotalMB":1024,"longPasteLines":5},"other":"x","autoResume":true}"#
        );
    }

    #[test]
    fn custom_agents_are_saved_checked_and_used_at_once() {
        crate::test_util::core();
        let yolo = json!([{ "name": "Claude YOLO", "command": "claude --dangerously-skip-permissions", "like": "claude" }]);
        let next = settings_put(&json!({ "customAgents": yolo }), load_settings()).unwrap();
        assert_eq!(next["customAgents"][0]["id"], "claude-yolo");
        assert_eq!(custom_agents::by_id("claude-yolo").map(|a| a.like), Some(inyon_protocol::AgentKind::Claude));
        // a bad list is refused whole and the saved one stays
        let err = settings_put(&json!({ "customAgents": [{ "name": "x", "command": "x", "like": "vim" }] }), load_settings()).unwrap_err();
        assert_eq!((err.status, err.message.as_str()), (Some(400), "x: pick the agent it works like"));
        assert_eq!(load_settings().0["customAgents"][0]["name"], "Claude YOLO");
        // other saves leave the list alone; an empty list clears it
        settings_put(&json!({ "autoResume": true }), load_settings()).unwrap();
        assert_eq!(custom_agents::list().len(), 1);
        settings_put(&json!({ "customAgents": [] }), load_settings()).unwrap();
        assert_eq!(custom_agents::list(), vec![]);
    }

    // node-update.test.ts and uploads.test.ts: routes run on a machine put in the registry

    use std::cell::RefCell;
    use std::rc::Rc;

    use inyon_core::hosts::host::Host;
    use inyon_core::hosts::registry::HostRuntime;

    /// A machine in the registry as `state` says, never polled.
    fn put(state: Value) -> inyon_core::hosts::registry::Rt {
        let state: inyon_protocol::HostState = serde_json::from_value(state).unwrap();
        let rt = Rc::new(RefCell::new(HostRuntime::new(Host::new(state.config.clone()), state.clone())));
        rt.borrow_mut().hold_polls();
        registry().insert_runtime(&state.config.id, rt.clone());
        rt
    }

    fn host_state(id: &str, kind: &str, more: Value) -> Value {
        let mut s =
            json!({ "config": { "id": id, "name": id, "kind": kind }, "status": "online", "integration": "unknown", "tmux": "ok", "agents": [], "sessions": [], "updatedAt": 0 });
        for (k, v) in more.as_object().unwrap() {
            s[k] = v.clone();
        }
        s
    }

    async fn answered(res: HubResult<Resp>) -> (u16, Value) {
        let res = crate::http::answer(res);
        let status = res.status().as_u16();
        let body = res.into_body().bytes().await.unwrap();
        (status, serde_json::from_slice(&body).unwrap_or(Value::Null))
    }

    struct Sock;
    impl node_link::NodeSocket for Sock {
        fn send(&self, _: &str) {}
        fn close(&self, _: u16, _: &str) {}
    }

    #[test]
    fn the_api_refuses_machines_it_cannot_update_with_a_clear_409() {
        crate::test_util::block(async {
            put(host_state("box", "ssh", json!({})));
            put(host_state("away", "node", json!({ "status": "error", "nodeVersion": "0.3.0" })));
            put(host_state("fine", "node", json!({ "nodeVersion": inyon_protocol::VERSION })));
            node_link::attach("fine", Rc::new(Sock));
            node_link::receive("fine", &json!({ "type": "hello", "version": inyon_protocol::VERSION, "os": "linux x64" }));
            let ask = |id: &str| {
                let mut r = crate::http::test_req("POST", &format!("/api/hosts/{id}/node/update"), &[], "");
                r.params = vec![("id".into(), id.into())];
                async move {
                    let (status, body) = answered(node_update(r).await).await;
                    (status, body["error"].as_str().unwrap_or_default().to_owned())
                }
            };
            assert_eq!(ask("nope").await, (404, "Unknown machine".into()));
            assert_eq!(ask("box").await, (409, "Only machines running inyon can be updated from here.".into()));
            assert_eq!(ask("away").await, (409, "away isn't connected. Update it once it's back online.".into()));
            assert_eq!(ask("fine").await, (409, "inyon on fine is already up to date.".into()));
        });
    }

    /// A multipart upload sent as a stream, so there is no Content-Length to go by.
    fn chunked_upload(fields: &[(&str, &str)], file: &str, size: usize) -> Req {
        const B: &str = "----inyon-test";
        let mut head = String::new();
        for (k, v) in fields {
            head.push_str(&format!("--{B}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n"));
        }
        head.push_str(&format!("--{B}\r\nContent-Disposition: form-data; name=\"files\"; filename=\"{file}\"\r\nContent-Type: application/octet-stream\r\n\r\n"));
        const MB: usize = 1024 * 1024;
        let data = (0..size).step_by(MB).map(move |at| (at..(at + MB).min(size)).map(|i| ((i * 7) & 255) as u8).collect::<Vec<u8>>());
        let chunks = std::iter::once(head.into_bytes()).chain(data).chain(std::iter::once(format!("\r\n--{B}--\r\n").into_bytes()));
        let stream = futures_util::stream::iter(chunks.map(|c| Ok::<_, std::io::Error>(bytes::Bytes::from(c))));
        let mut r = crate::http::test_req("POST", "/api/hosts/up/uploads", &[("content-type", &format!("multipart/form-data; boundary={B}"))], "");
        r.body = Some(crate::body::Body::from_stream(stream));
        r.params = vec![("id".into(), "up".into())];
        r
    }

    /// This machine, as a local machine "up" in the registry whose home is `home`, its commands run
    /// for real.
    fn local_machine(home: &std::path::Path) {
        let backend: Rc<dyn inyon_core::hosts::host::HostBackend> = inyon_backends::process::ProcessBackend::new(home.join("cm"), vec![]);
        inyon_core::hosts::host::set_host_backends(move |_| backend.clone());
        let rt = put(host_state("up", "local", json!({})));
        rt.borrow_mut().probe = Some(Rc::new(RefCell::new(inyon_core::tmux::tmux::ProbeResult::at_home(&home.to_string_lossy()))));
    }

    #[test]
    fn the_upload_route_takes_a_chunked_piece_and_counts_what_it_reads() {
        let home = std::env::temp_dir().join(format!("inyon-up-{}", crate::http::random_hex(6)));
        std::fs::create_dir_all(&home).unwrap();
        let (ok, refused) = crate::test_util::block(async {
            local_machine(&home);
            let ok = answered(upload(chunked_upload(&[("total", "1000"), ("offset", "0")], "small.bin", 1000)).await).await;
            // Ghostex #253: uploads over 1 MiB failed there
            let three = answered(upload(chunked_upload(&[], "photo.jpg", 3 * 1024 * 1024)).await).await;
            assert_eq!(three.0, 200, "{}", three.1);
            assert_eq!(three.1["files"][0]["size"], json!(3 * 1024 * 1024));
            // over the 100 MB setting (plus a megabyte for the form itself)
            let refused = answered(upload(chunked_upload(&[], "big.bin", 102 * 1024 * 1024)).await).await;
            (ok, refused)
        });
        let _ = std::fs::remove_dir_all(&home);
        assert_eq!(ok.0, 200, "{}", ok.1);
        assert_eq!(ok.1["files"][0]["size"], json!(1000));
        assert_eq!(refused, (413, json!({ "error": "Uploads are limited to 100 MB" })));
    }

    /// A request to the chunked upload routes for machine "up".
    fn up_req(method: &str, path: &str, params: &[(&str, &str)], body: Vec<u8>) -> Req {
        let mut r = crate::http::test_req(method, &format!("/api/hosts/up/uploads{path}"), &[("content-type", "application/json")], "");
        r.body = Some(crate::body::Body::from(body));
        r.params = std::iter::once(("id".to_owned(), "up".to_owned())).chain(params.iter().map(|(k, v)| (k.to_string(), v.to_string()))).collect();
        r
    }

    fn sha(b: &[u8]) -> String {
        use sha2::Digest;
        crate::http::hex(&sha2::Sha256::digest(b))
    }

    #[test]
    fn a_file_goes_up_a_chunk_at_a_time_through_the_routes_and_resumes_where_it_stopped() {
        let home = std::env::temp_dir().join(format!("inyon-up-{}", crate::http::random_hex(6)));
        std::fs::create_dir_all(&home).unwrap();
        crate::test_util::block(async {
            local_machine(&home);
            for size in [5 * 1024 * 1024 + 17, 50 * 1024 * 1024] {
                let data: Vec<u8> = (0..size).map(|i: usize| ((i * 31) ^ (i >> 9)) as u8).collect();
                let start = json!({ "name": "screen recording.mp4", "size": size, "type": "video/MP4", "pane": "4" });
                let (status, s) = answered(upload_start(up_req("POST", "/start", &[], start.to_string().into_bytes())).await).await;
                assert_eq!(status, 200, "{s}");
                let (up, chunk) = (s["id"].as_str().unwrap().to_owned(), s["chunk"].as_u64().unwrap() as usize);
                assert_eq!((s["received"].clone(), chunk), (json!(0), 1024 * 1024));
                let put_at = |at: usize| {
                    let r = up_req("PUT", &format!("/{up}/{at}"), &[("upload", &up), ("offset", &at.to_string())], data[at..(at + chunk).min(size)].to_vec());
                    async move { answered(upload_chunk(r).await).await }
                };
                let mut at = 0;
                while at < size {
                    if at == 3 * chunk {
                        // the connection drops: the app asks how much arrived and carries on from there
                        let (status, st) = answered(upload_status_route(&up).await).await;
                        assert_eq!((status, st["received"].as_u64()), (200, Some(at as u64)));
                        // a chunk sent too far ahead is refused with where to carry on from
                        let (status, ahead) = put_at(at + chunk).await;
                        assert_eq!((status, ahead["received"].as_u64()), (409, Some(at as u64)), "{ahead}");
                    }
                    let (status, st) = put_at(at).await;
                    assert_eq!(status, 200, "{st}");
                    at = st["received"].as_u64().unwrap() as usize;
                }
                let finish = json!({ "sha256": sha(&data) }).to_string().into_bytes();
                let (status, u) = answered(upload_finish(up_req("POST", &format!("/{up}/finish"), &[("upload", &up)], finish)).await).await;
                assert_eq!(status, 200, "{u}");
                assert_eq!((u["name"].clone(), u["size"].clone(), u["type"].clone()), (json!("screen recording.mp4"), json!(size), json!("video/mp4")));
                let path = u["path"].as_str().unwrap();
                assert!(path.contains("/.inyon/uploads/") && path.contains("/pane-4/") && path.ends_with("-screen_recording.mp4"), "{path}");
                assert!(std::fs::read(path).unwrap() == data);
            }
            // refusals: too large, a bad hash, a chunk over the limit, a bad offset, an unknown upload
            let (status, e) = answered(upload_start(up_req("POST", "/start", &[], json!({ "name": "a", "size": 200 * 1024 * 1024 }).to_string().into_bytes())).await).await;
            assert_eq!((status, e["error"].clone()), (413, json!("a is larger than 100 MB")));
            let (status, _) = answered(upload_start(up_req("POST", "/start", &[], json!({ "name": "a", "size": 1, "sha256": "nope" }).to_string().into_bytes())).await).await;
            assert_eq!(status, 400);
            let (_, s) = answered(upload_start(up_req("POST", "/start", &[], json!({ "name": "a", "size": 10 }).to_string().into_bytes())).await).await;
            let up = s["id"].as_str().unwrap().to_owned();
            let (status, e) = answered(upload_chunk(up_req("PUT", "", &[("upload", &up), ("offset", "0")], vec![0; 4 * 1024 * 1024 + 1])).await).await;
            assert_eq!((status, e["error"].clone()), (413, json!("Upload chunks are limited to 4 MB")));
            let (status, _) = answered(upload_chunk(up_req("PUT", "", &[("upload", &up), ("offset", "-1")], vec![0; 3])).await).await;
            assert_eq!(status, 400);
            let (status, _) = answered(upload_chunk(up_req("PUT", "", &[("upload", "0123abcd"), ("offset", "0")], vec![0; 3])).await).await;
            assert_eq!(status, 404);
        });
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn the_upload_routes_dont_shadow_each_other() {
        let mut routes = Routes::default();
        add(&mut routes);
        let at = |m: &str, path: &str| routes.handler(path, &m.parse().unwrap()).map(|(_, p)| p);
        let p = |kv: &[(&str, &str)]| Some(kv.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect::<Vec<_>>());
        assert_eq!(at("POST", "/api/hosts/box/uploads"), p(&[("id", "box")]));
        assert_eq!(at("POST", "/api/hosts/box/uploads/clean"), p(&[("id", "box")]));
        assert_eq!(at("POST", "/api/hosts/box/uploads/start"), p(&[("id", "box")]));
        assert_eq!(at("GET", "/api/hosts/box/uploads/ab12"), p(&[("id", "box"), ("upload", "ab12")]));
        assert_eq!(at("DELETE", "/api/hosts/box/uploads/ab12"), p(&[("id", "box"), ("upload", "ab12")]));
        assert_eq!(at("PUT", "/api/hosts/box/uploads/ab12/1048576"), p(&[("id", "box"), ("upload", "ab12"), ("offset", "1048576")]));
        assert_eq!(at("POST", "/api/hosts/box/uploads/ab12/finish"), p(&[("id", "box"), ("upload", "ab12")]));
    }

    async fn upload_status_route(up: &str) -> HubResult<Resp> {
        Ok(json_of(&chunked::upload_status("up", up)?))
    }

    // features/temp.test.ts: the routes hand the machine's `inyon temp` answer on

    /// A machine whose commands are answered by `answer`, each noted with what it got on stdin.
    struct Scripted {
        calls: RefCell<Vec<(String, Option<String>)>>,
        answer: RefCell<inyon_core::hosts::host::ExecResult>,
    }

    impl inyon_core::hosts::host::HostBackend for Scripted {
        fn exec(
            &self,
            _: &HostConfig,
            cmd: &str,
            opts: inyon_core::hosts::host::ExecOptions,
        ) -> futures_util::future::LocalBoxFuture<'static, HubResult<inyon_core::hosts::host::ExecResult>> {
            let cmd = cmd.strip_prefix(inyon_core::hosts::host::UTF8_TMUX).unwrap_or(cmd).to_owned();
            self.calls.borrow_mut().push((cmd, opts.input.map(|i| String::from_utf8_lossy(&i).into_owned())));
            let r = self.answer.borrow().clone();
            Box::pin(async move { Ok(r) })
        }
        fn stream(&self, _: &HostConfig, _: &str, _: inyon_core::hosts::host::OnLine, _: bool) -> inyon_core::hosts::host::StreamHandle {
            inyon_core::hosts::host::StreamHandle::new(|| {}, inyon_core::platform::rt::Deferred::new())
        }
        fn pty(&self, _: &HostConfig, _: &str, _: u16, _: u16, _: inyon_core::hosts::host::OnData) -> HubResult<inyon_core::hosts::host::PtyHandle> {
            Err(HubError::plain("no terminals here"))
        }
    }

    fn probe(home: &str) -> Rc<RefCell<inyon_core::tmux::tmux::ProbeResult>> {
        Rc::new(RefCell::new(inyon_core::tmux::tmux::ProbeResult {
            hostname: "box".into(),
            home: home.into(),
            os: "Linux".into(),
            now: 0.0,
            latency_ms: 0.0,
            tmux: inyon_core::tmux::tmux::TmuxProbe::Ok,
            integration: inyon_protocol::IntegrationState::Missing,
            sessions: vec![],
            agent_procs: Default::default(),
            hook_panes: Default::default(),
            status_panes: Default::default(),
            account: None,
            compact_panes: Default::default(),
            installed: vec![],
            signed_in: vec![],
            agy_conversations: Default::default(),
            agy_status: Default::default(),
            agy_titles: Default::default(),
            agy_integration: None,
            hooked: vec![],
            machine_kind: None,
            keys_at: Default::default(),
            unwatched: Default::default(),
            claude_parked: Default::default(),
        }))
    }

    #[test]
    fn the_temp_routes_hand_the_machines_answer_on_with_the_right_status() {
        crate::test_util::block(async {
            let fake = Rc::new(Scripted { calls: RefCell::default(), answer: RefCell::default() });
            let backend: Rc<dyn inyon_core::hosts::host::HostBackend> = fake.clone();
            inyon_core::hosts::host::set_host_backends(move |_| backend.clone());
            put(host_state("studio", "node", json!({}))).borrow_mut().probe = Some(probe("/home/me"));
            put(host_state("away", "node", json!({})));
            let say =
                |code: i32, stdout: &str, stderr: &str| *fake.answer.borrow_mut() = inyon_core::hosts::host::ExecResult { code, stdout: stdout.into(), stderr: stderr.into() };
            let req = |method: &str, url: &str, body: &str| {
                let mut r = crate::http::test_req(method, url, &[("content-type", "application/json")], body);
                r.params = vec![("id".into(), url.split('/').nth(3).unwrap().into())];
                r
            };

            say(0, r#"{"v":1,"ok":true,"host":"studio","roots":[],"items":[]}"#, "");
            assert_eq!(answered(temp_list(req("GET", "/api/hosts/studio/temp", "")).await).await, (200, json!({ "v": 1, "ok": true, "host": "studio", "roots": [], "items": [] })));
            assert_eq!(fake.calls.borrow().last().unwrap().0, format!("{} ls --json", temp::INYON));
            say(2, "", "error: unrecognized subcommand 'temp'\n");
            assert_eq!(answered(temp_list(req("GET", "/api/hosts/studio/temp", "")).await).await, (409, json!({ "error": "Update Inyon on studio to see its temp files" })));
            assert_eq!(answered(temp_list(req("GET", "/api/hosts/nope/temp", "")).await).await, (404, json!({ "error": "Unknown machine" })));
            assert_eq!(answered(temp_list(req("GET", "/api/hosts/away/temp", "")).await).await, (503, json!({ "error": "Machine is not connected yet" })));

            say(1, r#"{"v":1,"ok":false,"error":"That isn't inside one of the temp folders Inyon lists"}"#, "");
            let refused = answered(temp_preview(req("GET", "/api/hosts/studio/temp/preview?path=%2Fetc%2Fpasswd&max=1500000", "")).await).await;
            assert_eq!(refused, (400, json!({ "error": "That isn't inside one of the temp folders Inyon lists" })));
            assert_eq!(fake.calls.borrow().last().unwrap().0, format!("{} show --json --max 1500000 -- '/etc/passwd'", temp::INYON));
            say(0, r#"{"v":1,"ok":true,"path":"/tmp/x","files":[],"truncated":false}"#, "");
            assert_eq!(answered(temp_files(req("GET", "/api/hosts/studio/temp/files?path=%2Ftmp%2Fx", "")).await).await.0, 200);
            assert_eq!(fake.calls.borrow().last().unwrap().0, format!("{} files --json -- '/tmp/x'", temp::INYON));
            answered(temp_preview(req("GET", "/api/hosts/studio/temp/preview?path=%2Ftmp%2Fx&max=nope", "")).await).await;
            assert!(fake.calls.borrow().last().unwrap().0.contains("--max 8388608 "));

            say(0, r#"{"v":1,"ok":true,"removed":["/tmp/x"],"freed":5,"failed":[]}"#, "");
            let done = answered(temp_delete(req("POST", "/api/hosts/studio/temp/delete", r#"{"paths":["/tmp/x"]}"#)).await).await;
            assert_eq!(done, (200, json!({ "v": 1, "ok": true, "removed": ["/tmp/x"], "freed": 5, "failed": [] })));
            assert_eq!(fake.calls.borrow().last().unwrap().1.as_deref(), Some(r#"["/tmp/x"]"#));
            let bad = answered(temp_delete(req("POST", "/api/hosts/studio/temp/delete", r#"{"paths":"/tmp/x"}"#)).await).await;
            assert_eq!(bad, (400, json!({ "error": "Send the paths to delete" })));

            // each path finds its handler, for the method the app uses
            let mut routes = Routes::default();
            add(&mut routes);
            for (path, method) in [("/api/hosts/a/temp", "GET"), ("/api/hosts/a/temp/files", "GET"), ("/api/hosts/a/temp/preview", "GET"), ("/api/hosts/a/temp/delete", "POST")] {
                match routes.find(path) {
                    Some((crate::routes::Route::Methods(list), params)) => {
                        assert!(list.iter().any(|(m, _)| m.as_str() == method), "{path}");
                        assert_eq!(params, [("id".to_owned(), "a".to_owned())]);
                    }
                    _ => panic!("no route for {path}"),
                }
            }
        });
    }
}
