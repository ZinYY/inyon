// JSON the way JavaScript writes it, so a message the Rust side sends is byte for byte what the Bun
// hub would have sent, and the other way round.

use std::fmt;
use std::ops::Deref;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use ts_rs::TS;

/// A TypeScript `number` that comes from an agent's own output (a percentage, a cost, a duration),
/// so it may be fractional. JavaScript writes `42`, not `42.0`, for a whole value; this does the
/// same, where a bare `f64` would not.
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Default)]
pub struct Num(pub f64);

/// The largest integer a JavaScript number holds exactly (2^53).
const SAFE: f64 = 9_007_199_254_740_992.0;

impl Serialize for Num {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let v = self.0;
        if v.is_finite() && v.fract() == 0.0 && v.abs() < SAFE {
            s.serialize_i64(v as i64)
        } else {
            // NaN and the infinities become null, as JSON.stringify writes them
            s.serialize_f64(v)
        }
    }
}

// null back as NaN, which is what JSON.stringify wrote it for (an Option<Num> reads null as None)
impl<'de> Deserialize<'de> for Num {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        Option::<f64>::deserialize(d).map(|v| Num(v.unwrap_or(f64::NAN)))
    }
}

impl TS for Num {
    type WithoutGenerics = Self;
    type OptionInnerType = Self;
    fn name(_: &ts_rs::Config) -> String {
        "number".into()
    }
    fn inline(_: &ts_rs::Config) -> String {
        "number".into()
    }
}

impl From<f64> for Num {
    fn from(v: f64) -> Self {
        Num(v)
    }
}

impl From<Num> for f64 {
    fn from(v: Num) -> Self {
        v.0
    }
}

impl Deref for Num {
    type Target = f64;
    fn deref(&self) -> &f64 {
        &self.0
    }
}

impl fmt::Display for Num {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// The TypeScript literal type `true`: a flag that is either there and true, or left out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct True;

impl Serialize for True {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bool(true)
    }
}

impl<'de> Deserialize<'de> for True {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match bool::deserialize(d)? {
            true => Ok(True),
            false => Err(serde::de::Error::custom("expected true")),
        }
    }
}

impl TS for True {
    type WithoutGenerics = Self;
    type OptionInnerType = Self;
    fn name(_: &ts_rs::Config) -> String {
        "true".into()
    }
    fn inline(_: &ts_rs::Config) -> String {
        "true".into()
    }
}

/// For `field?: T | null`: left out is `None`, `null` is `Some(None)`. Serde alone reads both as
/// `None`, which would lose the difference (for `ScreenState.remote`, "not known" and "turned off").
pub(crate) fn double_option<'de, T: Deserialize<'de>, D: Deserializer<'de>>(d: D) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}
