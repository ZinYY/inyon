//! `inyon`, the command people type: setup, the status screen, pairing, services and updates.
//! The engine (`inyon-engine`, next to it) runs the link, the hub connection and the hub.

pub mod app;
pub mod cli;
pub mod commands;
pub mod desktop;
pub mod docs;
pub mod engine;
pub mod http;
pub mod hub_api;
pub mod hub_cert;
pub mod networks;
pub mod pages;
pub mod paths;
/// the pairing QR code, drawn in the terminal
pub use inyon_engine::qr;
pub mod service;
pub mod shellpath;
pub mod signing;
pub mod skills;
pub mod state;
pub mod sys;
pub mod temp;
pub mod tray;
pub mod ui;
pub mod unhook;
pub mod update;
