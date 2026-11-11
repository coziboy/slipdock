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

use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
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

    pub(crate) fn label(self) -> &'static str {
        match self {
            Screen::Dump => "Dump",
            Screen::Load => "Load",
            Screen::Dumps => "Dumps",
            Screen::Profiles => "Profiles",
            Screen::Tools => "Tools",
        }
    }

    fn icon(self) -> IconName {
        match self {
            Screen::Dump => IconName::ArrowDown,
            Screen::Load => IconName::ArrowUp,
            Screen::Dumps => IconName::Folder,
            Screen::Profiles => IconName::Globe,
            Screen::Tools => IconName::SquareTerminal,
        }
    }
}

pub struct Shell {
    screen: Screen,
    pub(crate) dump: Entity<DumpView>,
    pub(crate) load: Entity<LoadView>,
    pub(crate) dumps: Entity<DumpsView>,
    pub(crate) profiles: Entity<ProfilesView>,
    pub(crate) tools: Entity<ToolsView>,
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
            dumps: cx.new(|cx| DumpsView::new(window, cx)),
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

/// Below this window width the navigation moves from the side to the top.
const COMPACT_BELOW: Pixels = px(760.);

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        // GPUI concept: `render` runs again when the window is resized, so
        // the layout can depend on the window's size. Most of the app adapts
        // by wrapping (see `widgets.rs`); only this frame switches layout.
        let compact = window.viewport_size().width < COMPACT_BELOW;

        let brand = v_flex()
            .px_2()
            .flex_shrink_0()
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::BOLD)
                    .child("Slipdock"),
            )
            .when(!compact, |this| {
                this.pb_3().child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("MySQL dump and load"),
                )
            });

        let mut items = if compact {
            h_flex().flex_wrap().gap_1()
        } else {
            v_flex().gap_1()
        };
        for (index, screen) in Screen::ALL.into_iter().enumerate() {
            let active = self.screen == screen;
            let hover = theme.sidebar_accent;
            items = items.child(
                h_flex()
                    .id(("nav", index))
                    .role(Role::Button)
                    .aria_label(screen.label())
                    .gap_2()
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .text_sm()
                    .cursor_pointer()
                    .hover(move |style| style.bg(hover))
                    .when(active, |this| {
                        this.bg(theme.sidebar_accent)
                            .text_color(theme.sidebar_accent_foreground)
                            .font_weight(FontWeight::SEMIBOLD)
                    })
                    .child(Icon::new(screen.icon()).small())
                    .child(div().flex_1().child(screen.label()))
                    .when(!compact, |this| {
                        // The key that `main.rs` binds to this screen.
                        this.child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::NORMAL)
                                .text_color(theme.muted_foreground)
                                .child(format!("⌘{}", index + 1)),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.show(screen, cx))),
            );
        }

        let navigation = if compact {
            h_flex()
                .flex_wrap()
                .flex_shrink_0()
                .w_full()
                .gap_x_3()
                .gap_y_1()
                .px_3()
                .py_2()
                .border_b_1()
        } else {
            v_flex()
                .w(px(210.))
                .flex_shrink_0()
                .h_full()
                .gap_1()
                .p_3()
                .border_r_1()
        }
        .bg(theme.sidebar)
        .text_color(theme.sidebar_foreground)
        .border_color(theme.sidebar_border)
        .child(brand)
        .child(items);

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
            .when(compact, |this| this.flex_col())
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
                    // Without these a flex child never gets smaller than its
                    // content: long lines would push past the window, and a
                    // long page would not scroll.
                    .min_w_0()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        // On a wide window the page keeps a readable width
                        // and sits in the middle.
                        div()
                            .w_full()
                            .max_w(px(980.))
                            .mx_auto()
                            .when(compact, |this| this.p_4())
                            .when(!compact, |this| this.p_6())
                            .child(content),
                    )
                    // Lets the tests find this element by its id. Without
                    // the `test-support` feature this does nothing.
                    .test_support(),
            )
    }
}
