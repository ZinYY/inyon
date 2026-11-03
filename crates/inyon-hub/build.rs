//! The web app goes into a release build from apps/web/dist (static_files.rs). Cargo only watches
//! the files it embedded last time, so a release built before `bun run build`, or files added to
//! dist later (Vite gives each build new names), would never get in: a release watches the folder
//! instead, or while there's none yet, apps/web, where it will appear. A debug build reads the
//! folder at run time and embeds nothing, so it watches only this file.
use std::path::Path;

fn main() {
    if std::env::var_os("CARGO_CFG_DEBUG_ASSERTIONS").is_some() {
        println!("cargo:rerun-if-changed=build.rs");
    } else if Path::new("../../apps/web/dist").is_dir() {
        println!("cargo:rerun-if-changed=../../apps/web/dist");
    } else {
        println!("cargo:rerun-if-changed=../../apps/web");
    }
}
