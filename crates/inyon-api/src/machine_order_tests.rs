use std::{cell::RefCell, rc::Rc};
use inyon_core::hosts::{host::Host, registry::{registry, HostRuntime}};
use inyon_protocol::HostState;
use serde_json::{json, Value};
use crate::{http::test_req, routes::{dispatch, Routes}};

#[test]
fn machine_order_route_validates_and_publishes_the_saved_order() {
    crate::test_util::block(async {
        for id in ["a", "b", "c"] {
            let state: HostState = serde_json::from_value(json!({"config":{"id":id,"name":id,"kind":"node"},"status":"online","integration":"unknown","tmux":"ok","agents":[],"sessions":[],"updatedAt":0})).unwrap();
            let rt = Rc::new(RefCell::new(HostRuntime::new(Host::new(state.config.clone()), state)));
            rt.borrow_mut().hold_polls();
            registry().insert_runtime(id, rt);
        }
        let mut routes = Routes::default();
        crate::core_routes::add(&mut routes);
        for (body, expected) in [(json!({"before":12}),400), (json!({}),400), (json!({"before":"missing"}),400), (json!({"before":"a"}),200)] {
            let response = dispatch(&routes, test_req("POST", "/api/hosts/c/order", &[("content-type","application/json")], &body.to_string())).await.unwrap();
            assert_eq!(response.status().as_u16(), expected);
        }
        let response = dispatch(&routes, test_req("GET", "/api/overview", &[], "")).await.unwrap();
        let value: Value = serde_json::from_slice(&response.into_body().bytes().await.unwrap()).unwrap();
        assert_eq!(value["hosts"].as_array().unwrap().iter().map(|h| h["config"]["id"].as_str().unwrap()).collect::<Vec<_>>(), ["c", "a", "b"]);
    });
}
