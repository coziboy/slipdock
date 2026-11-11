//! Shared logic of Slipdock.
//!
//! The command line tool and the desktop app are thin layers over this
//! library. Nothing in here knows about a terminal or a window, and nothing
//! is asynchronous: long work runs on a plain thread and reports back over a
//! `std::sync::mpsc` channel.

pub mod command;
pub mod dumps;
pub mod error;
pub mod exec;
pub mod job;
pub mod platform;
pub mod profile;
pub mod report;
pub mod run;
pub mod secrets;
pub mod server;
pub mod settings;
pub mod tools;

pub use error::{Error, Result};
