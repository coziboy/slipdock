//! The one piece of state every screen shares.
//!
//! GPUI concept: **Global**. A global is a single value of a type, stored in
//! the application itself. Any code that has an `App` (or a `Context`, which
//! derefs to `App`) can read it with `cx.global::<Core>()`. We keep the
//! core's `Context` there: config folder, settings, where to look for tools,
//! and the keychain.

use gpui_kit::{App, Global};
use slipdock_core::job;

/// The log level of every dump and load the app starts: mydumper's and
/// myloader's most detailed one ("info"). The app has no choice of level:
/// a full log is always written beside the dump, and the Dumps screen
/// shows it. (The command line tool still takes `--log-level`.)
pub const LOG_LEVEL: Option<u8> = Some(3);

/// How a run gets its log level, as `(log_level, extra_args)` for the core's
/// options.
///
/// A real run hands the level to the core, which writes it into a temporary
/// copy of the profile. "Show command" must give a command that still works
/// when it is pasted into a terminal later, and that copy is gone by then:
/// there the profile itself is named and the level is an option of the
/// command.
pub fn log_options(show_command: bool) -> (Option<u8>, Vec<String>) {
    match (show_command, LOG_LEVEL) {
        (true, Some(level)) => (None, vec!["--verbose".into(), level.to_string()]),
        (_, level) => (level, Vec::new()),
    }
}

pub struct Core(pub job::Context);

impl Global for Core {}

/// A copy of the core context. Cheap, and a copy can be moved to a
/// background thread, which a reference into the app cannot.
pub fn core(cx: &App) -> job::Context {
    cx.global::<Core>().0.clone()
}
