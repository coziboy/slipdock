//! The one piece of state every screen shares.
//!
//! GPUI concept: **Global**. A global is a single value of a type, stored in
//! the application itself. Any code that has an `App` (or a `Context`, which
//! derefs to `App`) can read it with `cx.global::<Core>()`. We keep the
//! core's `Context` there: config folder, settings, where to look for tools,
//! and the keychain.

use gpui_kit::{App, Global};
use slipdock_core::tools::Tool;
use slipdock_core::{job, settings};

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

/// Makes "Show command" name the profile itself.
///
/// The arguments the settings give a tool reach a real run in a temporary
/// copy of the profile. A command shown for pasting cannot name that copy,
/// so the arguments are taken out of `ctx` and those of `tool` are put on
/// the command line instead.
///
/// The pasted command is then close to the run, not the same: mydumper and
/// myloader let a value in the profile win over the same option on the
/// command line, so an argument counts only where the profile sets nothing.
pub fn name_the_profile(ctx: &mut job::Context, tool: Tool, extra_args: &mut Vec<String>) {
    let options = std::mem::take(&mut ctx.settings.options);
    let arguments = match tool {
        Tool::Mydumper => options.mydumper.arguments,
        Tool::Myloader => options.myloader.arguments,
        Tool::Mysql | Tool::Zstd => String::new(),
    };
    // Arguments that cannot be read are left out here; a real run says why.
    extra_args.extend(settings::split_arguments(&arguments).unwrap_or_default());
}

pub struct Core(pub job::Context);

impl Global for Core {}

/// A copy of the core context. Cheap, and a copy can be moved to a
/// background thread, which a reference into the app cannot.
pub fn core(cx: &App) -> job::Context {
    cx.global::<Core>().0.clone()
}
