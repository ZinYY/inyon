//! The route table: Bun's routes, matched segment by segment, with the check every API route goes
//! through (`guarded`). Which check that is belongs to whoever builds the table: the hub's
//! (its own names, same origin, sign-in, companions, CORS for the phone app), or none in the phone
//! app, whose routes only the app itself can reach.

use std::rc::Rc;

use futures_util::future::LocalBoxFuture;
use http::Method;
use inyon_core::hosts::http_error::HubResult;

use crate::http::{answer, Req, Resp};

pub type Handler = Rc<dyn Fn(Req) -> LocalBoxFuture<'static, Resp>>;
pub type ApiHandler = Rc<dyn Fn(Req) -> LocalBoxFuture<'static, HubResult<Resp>>>;
/// What puts an API handler behind the table's checks: the pattern and method it serves, the
/// handler; the handler as it is served.
pub type Guard = Rc<dyn Fn(&str, &Method, ApiHandler) -> Handler>;

/// An async closure as an API handler.
pub fn h<F, Fut>(f: F) -> ApiHandler
where
    F: Fn(Req) -> Fut + 'static,
    Fut: std::future::Future<Output = HubResult<Resp>> + 'static,
{
    Rc::new(move |r| Box::pin(f(r)))
}

/// An async closure as a plain handler (no checks, no error mapping).
pub fn raw<F, Fut>(f: F) -> Handler
where
    F: Fn(Req) -> Fut + 'static,
    Fut: std::future::Future<Output = Resp> + 'static,
{
    Rc::new(move |r| Box::pin(f(r)))
}

/// No check, errors as JSON answers: the phone app's table.
pub fn plain_guard() -> Guard {
    Rc::new(|_, _, f| {
        Rc::new(move |r| {
            let f = f.clone();
            Box::pin(async move { answer(f(r).await) })
        })
    })
}

/// What one path answers: every method with one handler, or some methods each with its own.
pub enum Route {
    Any(Handler),
    Methods(Vec<(Method, Handler)>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Seg {
    Static(String),
    Param(String),
}

fn segments(pattern: &str) -> Vec<Seg> {
    pattern.split('/').skip(1).map(|s| s.strip_prefix(':').map_or_else(|| Seg::Static(s.to_owned()), |p| Seg::Param(p.to_owned()))).collect()
}

/// Bun's routes: a path matches a pattern segment for segment, a `:name` takes any one segment,
/// and where two patterns match, the one with a fixed segment earlier wins.
pub struct Routes {
    list: Vec<(Vec<Seg>, Route)>,
    guard: Guard,
}

impl Default for Routes {
    fn default() -> Self {
        Routes::with_guard(plain_guard())
    }
}

impl Routes {
    /// A table whose `guarded` routes go through `guard`.
    pub fn with_guard(guard: Guard) -> Self {
        Routes { list: Vec::new(), guard }
    }

    pub fn add(&mut self, pattern: &str, r: Route) {
        self.list.push((segments(pattern), r));
    }

    pub fn methods(&mut self, pattern: &str, list: Vec<(Method, Handler)>) {
        self.add(pattern, Route::Methods(list));
    }

    pub fn find(&self, path: &str) -> Option<(&Route, Vec<(String, String)>)> {
        let parts: Vec<&str> = path.split('/').skip(1).collect();
        let mut best: Option<(&Vec<Seg>, &Route)> = None;
        for (segs, route) in &self.list {
            if segs.len() != parts.len() || !segs.iter().zip(&parts).all(|(s, p)| matches!(s, Seg::Param(_)) && !p.is_empty() || matches!(s, Seg::Static(x) if x == p)) {
                continue;
            }
            let better = match best {
                None => true,
                Some((b, _)) => {
                    let first_diff = segs.iter().zip(b.iter()).find(|(x, y)| matches!(x, Seg::Static(_)) != matches!(y, Seg::Static(_)));
                    matches!(first_diff, Some((Seg::Static(_), _)))
                }
            };
            if better {
                best = Some((segs, route));
            }
        }
        let (segs, route) = best?;
        let params = segs
            .iter()
            .zip(&parts)
            .filter_map(|(s, p)| match s {
                Seg::Param(name) => Some((name.clone(), crate::http::decode_uri_component(p).unwrap_or_else(|| (*p).to_owned()))),
                Seg::Static(_) => None,
            })
            .collect();
        Some((route, params))
    }

    /// The handler for a request's path and method (HEAD takes GET's), with the path's parameters.
    pub fn handler(&self, path: &str, method: &Method) -> Option<(Handler, Vec<(String, String)>)> {
        match self.find(path)? {
            (Route::Any(h), params) => Some((h.clone(), params)),
            (Route::Methods(list), params) => list.iter().find(|(m, _)| m == method || (*method == Method::HEAD && *m == Method::GET)).map(|(_, h)| (h.clone(), params)),
        }
    }
}

/// The shared routes, each method behind the table's check.
pub fn guarded(routes: &mut Routes, pattern: &str, list: Vec<(Method, ApiHandler)>) {
    let list = list.into_iter().map(|(m, f)| (m.clone(), (routes.guard)(pattern, &m, f))).collect();
    routes.methods(pattern, list);
}

/// The same for handlers that make their own answers.
pub fn guarded_raw(routes: &mut Routes, pattern: &str, list: Vec<(Method, Handler)>) {
    let wrap = |f: Handler| -> ApiHandler {
        Rc::new(move |r| {
            let f = f.clone();
            Box::pin(async move { Ok(f(r).await) })
        })
    };
    let list = list.into_iter().map(|(m, f)| (m.clone(), (routes.guard)(pattern, &m, wrap(f)))).collect();
    routes.methods(pattern, list);
}

/// Answers a request from the table: its route if one matches, else None.
pub async fn dispatch(routes: &Routes, mut req: Req) -> Option<Resp> {
    let (h, params) = routes.handler(&req.path, &req.method)?;
    req.params = params;
    Some(h(req).await)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::text;

    fn table() -> Routes {
        let mut r = Routes::default();
        let none: Handler = raw(|_| async { text(200, "") });
        r.methods("/api/hosts/pair", vec![(Method::POST, none.clone())]);
        r.methods("/api/hosts/:id", vec![(Method::PATCH, none.clone())]);
        r.methods("/api/hosts/:id/panes/:pane/send", vec![(Method::POST, none.clone())]);
        r.methods("/api/phones/claim", vec![(Method::POST, none.clone())]);
        r.methods("/api/phones/:id", vec![(Method::DELETE, none)]);
        r
    }

    #[test]
    fn a_fixed_segment_beats_a_parameter_and_parameters_are_decoded() {
        let t = table();
        let (_, p) = t.find("/api/hosts/pair").unwrap();
        assert!(p.is_empty());
        let (_, p) = t.find("/api/hosts/my%20box/panes/12/send").unwrap();
        assert_eq!(p, vec![("id".into(), "my box".into()), ("pane".into(), "12".into())]);
        let (_, p) = t.find("/api/phones/claim").unwrap();
        assert!(p.is_empty());
        assert!(t.find("/api/hosts/").is_none());
        assert!(t.find("/api/hosts/a/b").is_none());
    }

    #[test]
    fn a_guarded_route_goes_through_the_tables_check_and_head_takes_get() {
        crate::test_util::block(async {
            let seen = Rc::new(std::cell::RefCell::new(Vec::new()));
            let s = seen.clone();
            let mut r = Routes::with_guard(Rc::new(move |pattern, m, f| {
                s.borrow_mut().push(format!("{m} {pattern}"));
                plain_guard()(pattern, m, f)
            }));
            guarded(&mut r, "/api/x/:id", vec![(Method::GET, h(|r| async move { Ok(text(200, r.param("id"))) }))]);
            assert_eq!(*seen.borrow(), ["GET /api/x/:id"]);
            let res = dispatch(&r, crate::http::test_req("HEAD", "/api/x/7", &[], "")).await.unwrap();
            assert_eq!(res.into_body().bytes().await.unwrap(), "7");
            let e = dispatch(&r, crate::http::test_req("GET", "/api/y", &[], "")).await;
            assert!(e.is_none());
        });
    }
}
