//! Chunked uploads from the app's side (hubs from 0.24), as `apps/web/src/lib/upload.ts` sends
//! them: the routes, which bytes go next, and what to do with each answer. The app brings the
//! network, the file and the SHA-256 (worked out over the chunks as they're read, then sent with
//! the finish).
//!
//! 1. `POST {start}` with `{ name, size, type, pane }` gives an [`UploadStatus`].
//! 2. `PUT {chunk}` with the bytes of [`next_chunk`], until `received == size`.
//! 3. `POST {finish}` with `{ sha256 }` gives the saved file (`{ name, path, size, type }`).
//!
//! After a dropped connection, wait ([`RETRY_WAITS_MS`]), `GET {status}`, carry on from `received`.

use std::ops::Range;

use serde::Deserialize;
use serde_json::Value;

use crate::js::encode_uri_component;

/// How long to wait before each try while the hub can't be reached: about ten minutes in all.
pub const RETRY_WAITS_MS: [u64; 23] = [
    1000, 2000, 4000, 8000, 15_000, 30_000, 30_000, 30_000, 30_000, 30_000, 30_000, 30_000, 30_000, 30_000, 30_000, 30_000, 30_000, 30_000, 30_000, 30_000, 30_000, 30_000, 30_000,
];

/// An upload as the hub tells it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct UploadStatus {
    pub id: String,
    pub size: u64,
    pub received: u64,
    pub chunk: u64,
}

fn base(host: &str) -> String {
    format!("/api/hosts/{}/uploads", encode_uri_component(host))
}

pub fn start_path(host: &str) -> String {
    format!("{}/start", base(host))
}

pub fn status_path(host: &str, id: &str) -> String {
    format!("{}/{}", base(host), encode_uri_component(id))
}

pub fn chunk_path(host: &str, id: &str, offset: u64) -> String {
    format!("{}/{offset}", status_path(host, id))
}

pub fn finish_path(host: &str, id: &str) -> String {
    format!("{}/finish", status_path(host, id))
}

/// The bytes to send next, or None once the hub has them all.
pub fn next_chunk(s: &UploadStatus) -> Option<Range<u64>> {
    (s.received < s.size).then(|| s.received..(s.received + s.chunk.max(1)).min(s.size))
}

/// The share of the file the machine has, 0 to 1.
pub fn fraction(received: u64, size: u64) -> f64 {
    if size == 0 {
        1.0
    } else {
        (received as f64 / size as f64).min(1.0)
    }
}

/// What to do after an answer from one of the upload routes (`status` 0: no answer at all).
#[derive(Debug, Clone, PartialEq)]
pub enum Next {
    /// carry on from this byte (a chunk taken, or one refused for not following on: 409)
    From(u64),
    /// the upload is done; this is the saved file
    Done(Value),
    /// the connection dropped or the hub is busy: wait, ask for the status, try again
    Wait,
    /// the hub forgot the upload (it restarted, or an hour went by): start the file over
    StartOver,
    /// the hub has no chunked uploads (before 0.24): send the file the old way
    OldHub,
    /// refused, with the hub's words
    Fail(String),
}

/// Reads an answer: `finish` says whether it answered the finish, whose body is the saved file.
pub fn next(status: u16, body: &Value, finish: bool) -> Next {
    let error = body.get("error").and_then(Value::as_str);
    match status {
        200..=299 if finish => Next::Done(body.clone()),
        200..=299 | 409 if body.get("received").and_then(Value::as_u64).is_some() => Next::From(body["received"].as_u64().unwrap_or(0)),
        0 | 408 | 429 | 500..=599 => Next::Wait,
        404 if error == Some("Not found") => Next::OldHub,
        404 | 410 => Next::StartOver,
        _ => Next::Fail(error.map_or_else(|| format!("Upload failed ({status})"), str::to_owned)),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn routes_and_chunks() {
        assert_eq!(start_path("my box"), "/api/hosts/my%20box/uploads/start");
        assert_eq!(chunk_path("hub:h1/local", "ab", 1048576), "/api/hosts/hub%3Ah1%2Flocal/uploads/ab/1048576");
        assert_eq!(finish_path("box", "ab"), "/api/hosts/box/uploads/ab/finish");
        let mut s = UploadStatus { id: "ab".into(), size: 10, received: 0, chunk: 4 };
        assert_eq!(next_chunk(&s), Some(0..4));
        s.received = 8;
        assert_eq!(next_chunk(&s), Some(8..10));
        s.received = 10;
        assert_eq!(next_chunk(&s), None);
        assert_eq!((fraction(5, 10), fraction(0, 0)), (0.5, 1.0));
        assert!(RETRY_WAITS_MS.iter().sum::<u64>() > 9 * 60_000);
    }

    #[test]
    fn answers() {
        let st = json!({ "id": "ab", "size": 10, "received": 4, "chunk": 4 });
        assert_eq!(next(200, &st, false), Next::From(4));
        assert_eq!(next(409, &st, false), Next::From(4));
        assert_eq!(next(200, &json!({ "path": "/p" }), true), Next::Done(json!({ "path": "/p" })));
        for s in [0, 408, 429, 502, 503] {
            assert_eq!(next(s, &json!({}), false), Next::Wait, "{s}");
        }
        assert_eq!(next(404, &json!({ "error": "Not found" }), false), Next::OldHub);
        assert_eq!(next(404, &json!({ "error": "This upload is no longer on the hub. Send the file again." }), false), Next::StartOver);
        assert_eq!(next(410, &json!({ "error": "x" }), false), Next::StartOver);
        assert_eq!(next(413, &json!({ "error": "a is larger than 100 MB" }), false), Next::Fail("a is larger than 100 MB".into()));
        assert_eq!(next(422, &json!({}), true), Next::Fail("Upload failed (422)".into()));
    }
}
