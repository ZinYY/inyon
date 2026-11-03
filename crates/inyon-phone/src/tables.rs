//! The phone's route tables, looked up as the TypeScript phone core's `makeRouter` did: the
//! phone's own routes first, then the core's. A table that has the path but not the method passes
//! the request on (the phone overrides only some methods of a path), and a path no table answers
//! for that method is 405.

use http::Method;
use inyon_api::routes::{Handler, Route, Routes};

pub enum Lookup {
    Handler(Handler, Vec<(String, String)>),
    /// some table has the path, none the method
    NotAllowed,
    NotFound,
}

pub fn find(tables: &[Routes], path: &str, method: &Method) -> Lookup {
    let mut allowed = false;
    for t in tables {
        let Some((route, params)) = t.find(path) else { continue };
        let handler = match route {
            Route::Any(h) => Some(h.clone()),
            // HEAD takes GET's handler, as the hub's table does
            Route::Methods(list) => list.iter().find(|(m, _)| m == method || (*method == Method::HEAD && *m == Method::GET)).map(|(_, h)| h.clone()),
        };
        match handler {
            Some(h) => return Lookup::Handler(h, params),
            None => allowed = true,
        }
    }
    if allowed {
        Lookup::NotAllowed
    } else {
        Lookup::NotFound
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use inyon_api::http::text;
    use inyon_api::routes::raw;

    fn table(pattern: &str, methods: &[(Method, &'static str)]) -> Routes {
        let mut r = Routes::default();
        r.methods(pattern, methods.iter().cloned().map(|(m, said)| (m, raw(move |_| async move { text(200, said) }))).collect());
        r
    }

    fn said(f: Lookup) -> String {
        let Lookup::Handler(h, _) = f else { return "none".into() };
        let res = futures_util::FutureExt::now_or_never(h(inyon_api::http::test_req("GET", "/", &[], ""))).unwrap();
        String::from_utf8(futures_util::FutureExt::now_or_never(res.into_body().bytes()).unwrap().unwrap().to_vec()).unwrap()
    }

    #[test]
    fn the_phones_own_route_wins_and_a_method_it_lacks_falls_through_to_the_core() {
        let tables = [table("/api/hosts/:id", &[(Method::PATCH, "phone")]), table("/api/hosts/:id", &[(Method::PATCH, "core"), (Method::DELETE, "core")])];
        assert_eq!(said(find(&tables, "/api/hosts/a", &Method::PATCH)), "phone");
        assert_eq!(said(find(&tables, "/api/hosts/a", &Method::DELETE)), "core");
        assert!(matches!(find(&tables, "/api/hosts/a", &Method::GET), Lookup::NotAllowed));
        assert!(matches!(find(&tables, "/api/nothing", &Method::GET), Lookup::NotFound));
    }

    #[test]
    fn parameters_come_from_the_table_that_answers_and_head_takes_get() {
        let tables = [table("/api/x", &[(Method::POST, "phone")]), table("/api/:thing", &[(Method::GET, "core")])];
        let Lookup::Handler(_, params) = find(&tables, "/api/x", &Method::HEAD) else { panic!("a handler") };
        assert_eq!(params, [("thing".to_owned(), "x".to_owned())]);
    }
}
