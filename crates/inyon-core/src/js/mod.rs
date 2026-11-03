//! JavaScript's rules where the TypeScript readers lean on them, so the Rust port reads text
//! exactly as the Bun hub does: regular expressions (`\s`, `\w`, `\b` and `.` mean what they mean
//! in JS), string lengths and slices in UTF-16 units, `trim`, `Number()`, `Math.round`, dates and
//! `JSON.stringify`.

pub mod date;
pub mod json;
pub mod num;
pub mod re;
pub mod text;

pub use json::{stringify, Js};
pub use num::{fmt_number, js_round, to_number};
pub use text::JsStr;
