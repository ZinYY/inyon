// Shared by the integration tests: where the fixtures are, and how to read a protocol sample.
// Each test file uses only some of it.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;

/// The repository's top-level fixtures/ folder.
pub fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures")
}

/// The files in a folder, sorted, so failures come out in the same order every time.
pub fn entries(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())).map(|e| e.unwrap().path()).collect();
    out.sort();
    out
}

/// Reads `value` as a `T` and writes it back; the result must be the same JSON value. Integers and
/// whole numbers count as different (JavaScript writes `42`, never `42.0`), and so do `null` and
/// a field left out.
pub fn round_trip<T: DeserializeOwned + Serialize>(value: &Value) -> Result<(), String> {
    let parsed: T = serde_json::from_value(value.clone()).map_err(|e| format!("doesn't parse: {e}"))?;
    let back = serde_json::to_value(&parsed).map_err(|e| format!("doesn't serialize: {e}"))?;
    if &back == value {
        Ok(())
    } else {
        Err(format!("came back different:\n  in:  {value}\n  out: {back}"))
    }
}

/// Round-trips a sample by the name of the type it is, which is its folder under fixtures/protocol.
pub fn round_trip_as(ty: &str, value: &Value) -> Result<(), String> {
    use inyon_protocol::*;
    macro_rules! by_name {
        ($($t:ident),* $(,)?) => {
            match ty {
                $(stringify!($t) => round_trip::<$t>(value),)*
                _ => Err(format!("fixtures/protocol/{ty} isn't the name of a protocol type")),
            }
        };
    }
    inyon_protocol::with_types!(by_name)
}
