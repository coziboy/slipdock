//! The one error type of the core.
//!
//! Slipdock's errors are meant for a person, not for a program: almost all of
//! them end up printed as they are. So there are only two kinds. `Message` is
//! a finished text (it may span several lines, for example to show the fix).
//! `Io` keeps the operating system's error and adds what we were doing.

use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Message(String),

    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
}

/// Short form used across the core: `Result<T>` instead of `Result<T, Error>`.
pub type Result<T> = std::result::Result<T, Error>;

impl Error {
    pub fn msg(text: impl Into<String>) -> Self {
        Error::Message(text.into())
    }

    /// Builds a closure for `map_err`, so call sites stay on one line:
    /// `fs::read(path).map_err(Error::io_at("cannot read", path))?`.
    pub fn io_at(action: &str, path: &Path) -> impl FnOnce(std::io::Error) -> Error {
        let context = format!("{action} {}", path.display());
        move |source| Error::Io { context, source }
    }
}
