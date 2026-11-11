//! The window's frame: navigation on the left, the current screen beside it.
//!
//! GPUI concepts that first appear here:
//!
//! - **Entity**: a piece of state owned by the application, reached through
//!   a handle (`Entity<T>`). `cx.new(|cx| ...)` creates one. Each screen is
//!   an entity; `Shell` holds their handles.
//! - **Context**: `Context<Shell>` is "the app, seen from this entity". It
//!   can do everything `App` can, plus things about this entity: `notify()`
//!   (redraw me), `listener(...)` (call my method on an event), `spawn(...)`.
//! - **Render**: an entity that implements `Render` is a *view*. `render`
//!   is called whenever the entity was notified, and returns the elements to
//!   draw. It describes the whole view from the current state each time;
//!   there is no "update this label" step.
//! - **Actions**: named commands (`ShowDump`, `Quit`) that keys are bound
//!   to. The view says which actions it handles with `.on_action(...)`; the
//!   key bindings are set once in `main.rs`.

use gpui_kit::component::button::*;
use gpui_kit::component::{ActiveTheme, Selectable, v_flex};
use gpui_kit::*;
use slipdock_core::profile;

use crate::dump_view::DumpView;
use crate::dumps_view::DumpsView;
use crate::load_view::LoadView;
use crate::profiles_view::ProfilesView;
use crate::state::core;
use crate::tools_view::ToolsView;
use crate::{ShowDump, ShowDumps, ShowLoad, ShowProfiles, ShowTools};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Screen {
    Dump,
    Load,
    Dumps,
    Profiles,
    Tools,
}

impl Screen {
    pub(crate) const ALL: [Screen; 5] = [
        Screen::Dump,
        Screen::Load,
        Screen::Dumps,
        Screen::Profiles,
        Screen::Tools,
    ];

    fn label(self) -> &'static str {
        match self {
            Screen::Dump => "Dump",
            Screen::Load => "Load",
            Screen::Dumps => "Dumps",
            Screen::Profiles => "Profiles",
            Screen::Tools => "Tools",
        }
    }
}

pub struct Shell {
    screen: Screen,
    pub(crate) dump: Entity<DumpView>,
    pub(crate) load: Entity<LoadView>,
    dumps: Entity<DumpsView>,
    pub(crate) profiles: Entity<ProfilesView>,
    tools: Entity<ToolsView>,
    /// Lets the window deliver key presses (and so actions) to this view.
    focus: FocusHandle,
}

impl Shell {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // First run: without profiles there is nothing to dump yet, so start
        // where the tools are checked.
        let first = if profile::names(&core(cx).config_dir).is_empty() {
            Screen::Tools
        } else {
            Screen::Dump
        };
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Shell {
            screen: first,
            dump: cx.new(|cx| DumpView::new(window, cx)),
            load: cx.new(|cx| LoadView::new(window, cx)),
            dumps: cx.new(DumpsView::new),
            profiles: cx.new(|cx| ProfilesView::new(window, cx)),
            tools: cx.new(ToolsView::new),
            focus,
        }
    }

    pub(crate) fn show(&mut self, screen: Screen, cx: &mut Context<Self>) {
        self.screen = screen;
        // Profiles or dumps may have changed on another screen: reload what
        // the new screen shows.
        match screen {
            Screen::Dump => self.dump.update(cx, |view, cx| view.reload(cx)),
            Screen::Load => self.load.update(cx, |view, cx| view.reload(cx)),
            Screen::Dumps => self.dumps.update(cx, |view, cx| view.reload(cx)),
            Screen::Profiles => self.profiles.update(cx, |view, cx| view.reload(cx)),
            Screen::Tools => self.tools.update(cx, |view, cx| view.reload(cx)),
        }
        cx.notify();
    }
}

impl Render for Shell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        let mut navigation = v_flex()
            .w(px(170.))
            .flex_shrink_0()
            .h_full()
            .gap_1()
            .p_3()
            .bg(theme.sidebar)
            .border_r_1()
            .border_color(theme.sidebar_border)
            .child(
                div()
                    .px_2()
                    .pb_3()
                    .text_lg()
                    .font_weight(FontWeight::BOLD)
                    .child("Slipdock"),
            );
        for (index, screen) in Screen::ALL.into_iter().enumerate() {
            navigation = navigation.child(
                Button::new(("nav", index))
                    .ghost()
                    .label(screen.label())
                    .selected(self.screen == screen)
                    .on_click(cx.listener(move |this, _, _, cx| this.show(screen, cx))),
            );
        }

        // An `Entity<V>` where V implements Render can be used as a child.
        let content: AnyView = match self.screen {
            Screen::Dump => self.dump.clone().into(),
            Screen::Load => self.load.clone().into(),
            Screen::Dumps => self.dumps.clone().into(),
            Screen::Profiles => self.profiles.clone().into(),
            Screen::Tools => self.tools.clone().into(),
        };

        div()
            .flex()
            .size_full()
            .bg(theme.background)
            .text_color(theme.foreground)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &ShowDump, _, cx| this.show(Screen::Dump, cx)))
            .on_action(cx.listener(|this, _: &ShowLoad, _, cx| this.show(Screen::Load, cx)))
            .on_action(cx.listener(|this, _: &ShowDumps, _, cx| this.show(Screen::Dumps, cx)))
            .on_action(cx.listener(|this, _: &ShowProfiles, _, cx| this.show(Screen::Profiles, cx)))
            .on_action(cx.listener(|this, _: &ShowTools, _, cx| this.show(Screen::Tools, cx)))
            .child(navigation)
            .child(
                div()
                    .id("content")
                    .flex_1()
                    // Without this a flex child never gets narrower than its
                    // content, and long lines would push past the window.
                    .min_w_0()
                    .h_full()
                    .overflow_y_scroll()
                    .p_6()
                    .child(content),
            )
    }
}
