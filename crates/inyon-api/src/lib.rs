//! The hub's API on inyon-core, with no server in it: what a route handler gets and gives back
//! (`http`, `body`), the route table and its checks (`routes`), the routes every hub has
//! (`core_routes`, `organize_routes`), and the app's WebSocket clients (`clients`, and `streams` for `/ws2`). The hub carries these over axum;
//! the phone app runs them in the browser, as the TS hub's `coreRoutes` ran in both.

pub mod body;
pub mod clients;
pub mod core_routes;
pub mod http;
pub mod multipart;
pub mod organize_routes;
pub mod push;
pub mod routes;
pub mod streams;
#[cfg(test)]
pub mod test_util;
