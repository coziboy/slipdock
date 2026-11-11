//! The one piece of state every screen shares.
//!
//! GPUI concept: **Global**. A global is a single value of a type, stored in
//! the application itself. Any code that has an `App` (or a `Context`, which
//! derefs to `App`) can read it with `cx.global::<Core>()`. We keep the
//! core's `Context` there: config folder, settings, where to look for tools,
//! and the keychain.

use gpui_kit::{App, Global};
use slipdock_core::job;

pub struct Core(pub job::Context);

impl Global for Core {}

/// A copy of the core context. Cheap, and a copy can be moved to a
/// background thread, which a reference into the app cannot.
pub fn core(cx: &App) -> job::Context {
    cx.global::<Core>().0.clone()
}
