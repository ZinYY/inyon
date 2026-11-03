//! Port of hosts/http-error.ts. The core's errors carry the HTTP status the hub answers with, or
//! none for a plain `Error` (the hub answers 500); the message is shown in the app as is.

use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HubError {
    /// `HttpError.status`; None for a plain `Error`
    pub status: Option<u16>,
    pub message: String,
}

pub type HubResult<T> = Result<T, HubError>;

impl HubError {
    /// `new HttpError(status, message)`
    pub fn http(status: u16, message: impl Into<String>) -> Self {
        HubError { status: Some(status), message: message.into() }
    }

    /// `new Error(message)`
    pub fn plain(message: impl Into<String>) -> Self {
        HubError { status: None, message: message.into() }
    }
}

/// `new HttpError(status, message)`
pub fn http(status: u16, message: impl Into<String>) -> HubError {
    HubError::http(status, message)
}

impl fmt::Display for HubError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for HubError {}

impl From<String> for HubError {
    fn from(message: String) -> Self {
        HubError::plain(message)
    }
}

impl From<&str> for HubError {
    fn from(message: &str) -> Self {
        HubError::plain(message)
    }
}
