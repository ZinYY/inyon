//! How sessions are organised (from 0.24; see inyon_core::features::organize): one read of all of
//! it, and a route for each thing that changes. Every change goes out to the apps at once, in the
//! overview on `/ws` and on the organize stream of `/ws2`. Messages sent later are under the
//! machine they go to (`/api/hosts/:id/…`), so a phone reaches a paired hub's the same way.

use http::Method;
use inyon_core::clock::now_ms;
use inyon_core::features::organize::{self, auto, SessionPatch};
use serde_json::{json, Value};

use crate::http::{json_of, ok, ok_json, pane_param, Req, Resp};
use crate::routes::{guarded, h, Routes};
use inyon_core::hosts::http_error::HubResult;

/// `{ session }`: the mark after the change, or null when nothing is kept about it any more.
fn session(m: Option<inyon_protocol::SessionMark>) -> Resp {
    ok_json(&json!({ "session": m }))
}

async fn body(r: &mut Req) -> HubResult<Value> {
    r.body().await
}

/// The message waiting to go, checked to be on the machine the route names.
fn send_on(r: &Req) -> HubResult<String> {
    let (host, id) = (r.param("id"), r.param("did"));
    if !organize::get().sends.unwrap_or_default().iter().any(|s| s.id == id && s.host == host) {
        return Err(inyon_core::hosts::http_error::http(404, "That message isn't waiting any more"));
    }
    Ok(id)
}

pub fn add(routes: &mut Routes) {
    use Method as M;
    guarded(routes, "/api/organize", vec![(M::GET, h(|_r| async { Ok(json_of(&organize::current())) }))]);
    guarded(
        routes,
        "/api/hosts/:id/panes/:pane/organize",
        vec![(
            M::PATCH,
            h(|mut r| async move {
                let p = SessionPatch::read(&body(&mut r).await?, now_ms())?;
                Ok(session(organize::mark_pane(&r.param("id"), &pane_param(&r.param("pane")), &p)?))
            }),
        )],
    );
    guarded(
        routes,
        "/api/hosts/:id/panes/:pane/sends",
        vec![(M::POST, h(|mut r| async move { Ok(json_of(&auto::add_send(&r.param("id"), &pane_param(&r.param("pane")), &body(&mut r).await?, now_ms())?)) }))],
    );
    guarded(
        routes,
        "/api/hosts/:id/sends",
        vec![(
            M::GET,
            h(|r| async move {
                let host = r.param("id");
                Ok(json_of(&organize::get().sends.unwrap_or_default().into_iter().filter(|s| s.host == host).collect::<Vec<_>>()))
            }),
        )],
    );
    guarded(
        routes,
        "/api/hosts/:id/sends/:did",
        vec![
            (
                M::PATCH,
                h(|mut r| async move {
                    let id = send_on(&r)?;
                    Ok(json_of(&auto::update_send(&id, &body(&mut r).await?, now_ms())?))
                }),
            ),
            (
                M::DELETE,
                h(|r| async move {
                    auto::remove_send(&send_on(&r)?)?;
                    Ok(ok())
                }),
            ),
        ],
    );
    guarded(
        routes,
        "/api/hosts/:id/sends/:did/now",
        vec![(
            M::POST,
            h(|r| async move {
                let pane = auto::send_now(&send_on(&r)?)?;
                Ok(ok_json(&json!({ "pane": pane })))
            }),
        )],
    );
    guarded(routes, "/api/organize/sleep", vec![(M::PUT, h(|mut r| async move { Ok(json_of(&auto::set_sleep(&body(&mut r).await?)?)) }))]);
    guarded(routes, "/api/organize/prompts", vec![(M::POST, h(|mut r| async move { Ok(json_of(&auto::add_prompt(&body(&mut r).await?)?)) }))]);
    guarded(
        routes,
        "/api/organize/prompts/:qid",
        vec![
            (M::PATCH, h(|mut r| async move { Ok(json_of(&auto::update_prompt(&r.param("qid"), &body(&mut r).await?)?)) })),
            (
                M::DELETE,
                h(|r| async move {
                    auto::remove_prompt(&r.param("qid"))?;
                    Ok(ok())
                }),
            ),
        ],
    );
    guarded(
        routes,
        "/api/organize/sessions/:sid",
        vec![
            (
                M::PATCH,
                h(|mut r| async move {
                    let p = SessionPatch::read(&body(&mut r).await?, now_ms())?;
                    Ok(session(organize::mark_by_id(&r.param("sid"), &p)?))
                }),
            ),
            (
                M::DELETE,
                h(|r| async move {
                    organize::forget(&r.param("sid"))?;
                    Ok(ok())
                }),
            ),
        ],
    );
    guarded(
        routes,
        "/api/organize/order",
        vec![(
            M::PUT,
            h(|mut r| async move {
                organize::set_order(&body(&mut r).await?)?;
                Ok(ok())
            }),
        )],
    );
    guarded(routes, "/api/organize/projects", vec![(M::POST, h(|mut r| async move { Ok(json_of(&organize::add_project(&body(&mut r).await?)?)) }))]);
    guarded(
        routes,
        "/api/organize/projects/:pid",
        vec![
            (M::PATCH, h(|mut r| async move { Ok(json_of(&organize::update_project(&r.param("pid"), &body(&mut r).await?)?)) })),
            (
                M::DELETE,
                h(|r| async move {
                    organize::remove_project(&r.param("pid"))?;
                    Ok(ok())
                }),
            ),
        ],
    );
    guarded(routes, "/api/organize/groups", vec![(M::POST, h(|mut r| async move { Ok(json_of(&organize::add_group(&body(&mut r).await?)?)) }))]);
    guarded(
        routes,
        "/api/organize/groups/:gid",
        vec![
            (M::PATCH, h(|mut r| async move { Ok(json_of(&organize::update_group(&r.param("gid"), &body(&mut r).await?)?)) })),
            (
                M::DELETE,
                h(|r| async move {
                    organize::remove_group(&r.param("gid"))?;
                    Ok(ok())
                }),
            ),
        ],
    );
    guarded(routes, "/api/organize/spaces", vec![(M::POST, h(|mut r| async move { Ok(json_of(&organize::add_space(&body(&mut r).await?)?)) }))]);
    guarded(
        routes,
        "/api/organize/spaces/:wid",
        vec![
            (M::PATCH, h(|mut r| async move { Ok(json_of(&organize::update_space(&r.param("wid"), &body(&mut r).await?)?)) })),
            (
                M::DELETE,
                h(|r| async move {
                    organize::remove_space(&r.param("wid"))?;
                    Ok(ok())
                }),
            ),
        ],
    );
}

#[cfg(test)]
#[path = "organize_tests.rs"]
mod tests;
