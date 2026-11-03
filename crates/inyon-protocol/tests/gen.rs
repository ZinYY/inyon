//! Writes the Rust types out as TypeScript (packages/protocol/src/gen/index.ts) and fails when
//! the committed file was stale, so a change on one side can't go unnoticed. The protocol
//! package's typecheck then compares the generated types with the hand-written ones.

use std::path::Path;

use inyon_protocol::*;
use ts_rs::{Config, TS};

const HEADER: &str = "// Generated from crates/inyon-protocol by `cargo test -p inyon-protocol --test gen`. Don't edit it:
// change the Rust types, run that test and commit what it writes. gen-check.ts compares these
// types with the hand-written ones in index.ts.
";

macro_rules! declarations {
    ($($t:ty),* $(,)?) => {{
        // u64 is a plain number in JavaScript; every one the protocol carries fits in 2^53
        let cfg = Config::new().with_large_int("number");
        let mut out = String::from(HEADER);
        $(
            out.push('\n');
            out.push_str(&<$t as TS>::docs().unwrap_or_default());
            out.push_str("export ");
            out.push_str(&<$t as TS>::decl(&cfg));
            out.push('\n');
        )*
        out
    }};
}

#[test]
fn generated_typescript_is_current() {
    let fresh = inyon_protocol::with_types!(declarations);
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packages/protocol/src/gen/index.ts");
    let old = std::fs::read_to_string(&path).unwrap_or_default();
    if old != fresh {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &fresh).unwrap();
        panic!("{} was out of date and has been rewritten; check it and commit it", path.display());
    }
}
