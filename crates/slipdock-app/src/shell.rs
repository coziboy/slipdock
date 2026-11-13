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
use slipdock_core::{platform, profile};

use crate::dump_view::DumpView;
use crate::dumps_view::DumpsView;
use crate::load_view::LoadView;
use crate::profiles_view::ProfilesView;
use crate::state::core;
use crate::theme;
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
    /// The window is drawn up to its top edge (macOS): see `TITLE_STRIP`.
    pub(crate) under_title_bar: bool,
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
        theme::assign_bands(cx);
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        Shell {
            screen: first,
            dump: cx.new(|cx| DumpView::new(window, cx)),
            load: cx.new(|cx| LoadView::new(window, cx)),
            dumps: cx.new(|cx| DumpsView::new(window, cx)),
            profiles: cx.new(|cx| ProfilesView::new(window, cx)),
            tools: cx.new(|cx| ToolsView::new(window, cx)),
            focus,
            under_title_bar: platform::draws_under_title_bar(platform::current_os()),
        }
    }

    pub(crate) fn show(&mut self, screen: Screen, cx: &mut Context<Self>) {
        self.screen = screen;
        // A profile made on the Profiles screen gets its band here.
        theme::assign_bands(cx);
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

/// Where the three window buttons of macOS stand on the rail: in line with
/// the icons of the navigation.
pub(crate) const WINDOW_BUTTONS: Point<Pixels> = point(px(20.), px(13.));

/// The strip at the top of a window that is drawn up to its top edge.
///
/// The system still owns this strip: a drag there moves the window, a double
/// click zooms it, and a click on something in it arrives late. So nothing
/// that can be clicked is drawn in it. The rail and the ground run up through
/// it, and the page scrolls below it.
///
/// Its height is the system's: the height of a window button (14px) plus
/// twice the distance of `WINDOW_BUTTONS` from the top.
const TITLE_STRIP: Pixels = px(40.);

/// In the compact layout the name stands in the strip, after the window
/// buttons: three of 14px with 9px between them, from 20px, then 12px.
const AFTER_WINDOW_BUTTONS: Pixels = px(92.);

/// Whether the strip is drawn. A full-screen window has no window buttons
/// and no title bar, so it has no strip either.
fn title_strip(under_title_bar: bool, fullscreen: bool) -> bool {
    under_title_bar && !fullscreen
}

impl Render for Shell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        // GPUI concept: `render` runs again when the window is resized, so
        // the layout can depend on the window's size. Most of the app adapts
        // by wrapping (see `widgets.rs`); only this frame switches layout.
        let compact = window.viewport_size().width < COMPACT_BELOW;
        let strip = title_strip(self.under_title_bar, window.is_fullscreen());

        let brand = v_flex()
            .px_2()
            .flex_shrink_0()
            .child(
                div()
                    .text_xl()
                    .font_weight(FontWeight::BOLD)
                    .child("Slipdock"),
            )
            .when(!compact, |this| {
                this.pb_3().child(
                    div()
                        .text_xs()
                        // The grey of the page cannot be read on the rail.
                        .text_color(theme::color(theme::RAIL_DIM))
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
            let hover = theme::color(theme::RAIL_HOVER);
            items = items.child(
                h_flex()
                    .id(("nav", index))
                    .role(Role::Button)
                    .aria_label(screen.label())
                    .gap_2()
                    .px_2()
                    .py_1p5()
                    .rounded(theme::RADIUS)
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
                                .text_color(theme::color(theme::RAIL_DIM))
                                .child(format!("⌘{}", index + 1)),
                        )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.show(screen, cx))),
            );
        }

        let navigation = if compact && strip {
            // The name stands beside the window buttons, in the strip, and
            // the screens get the row below it.
            v_flex()
                .flex_shrink_0()
                .w_full()
                .border_b_1()
                .child(
                    h_flex()
                        .h(TITLE_STRIP)
                        .flex_shrink_0()
                        // The name has 8px of padding of its own.
                        .pl(AFTER_WINDOW_BUTTONS - px(8.))
                        .child(brand),
                )
                .child(div().px_3().pb_2().child(items))
        } else if compact {
            h_flex()
                .flex_wrap()
                .flex_shrink_0()
                .w_full()
                .gap_x_3()
                .gap_y_1()
                .px_3()
                .py_2()
                .border_b_1()
                .child(brand)
                .child(items)
        } else {
            v_flex()
                .w(px(210.))
                .flex_shrink_0()
                .h_full()
                .gap_1()
                .p_3()
                .border_r_1()
                // The window buttons stand here.
                .when(strip, |this| this.pt(TITLE_STRIP))
                .child(brand)
                .child(items)
        }
        .bg(theme.sidebar)
        .text_color(theme.sidebar_foreground)
        .border_color(theme.sidebar_border);

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
            .bg(theme::color(theme::GROUND))
            .text_color(theme.foreground)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &ShowDump, _, cx| this.show(Screen::Dump, cx)))
            .on_action(cx.listener(|this, _: &ShowLoad, _, cx| this.show(Screen::Load, cx)))
            .on_action(cx.listener(|this, _: &ShowDumps, _, cx| this.show(Screen::Dumps, cx)))
            .on_action(cx.listener(|this, _: &ShowProfiles, _, cx| this.show(Screen::Profiles, cx)))
            .on_action(cx.listener(|this, _: &ShowTools, _, cx| this.show(Screen::Tools, cx)))
            .child(navigation)
            .child(
                v_flex()
                    .flex_1()
                    // Without these a flex child never gets smaller than its
                    // content: long lines would push past the window, and a
                    // long page would not scroll.
                    .min_w_0()
                    .min_h_0()
                    // Beside the rail the strip is ground: the page starts
                    // below it, so nothing scrolls into it.
                    .when(strip && !compact, |this| {
                        this.child(div().h(TITLE_STRIP).flex_shrink_0())
                    })
                    .child(
                        div()
                            .id("content")
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .child(
                                // On a wide window the page keeps a readable
                                // width and sits in the middle.
                                div()
                                    .w_full()
                                    .max_w(px(980.))
                                    .mx_auto()
                                    .when(compact, |this| this.p_4())
                                    .when(!compact, |this| this.p_6())
                                    // The strip is already the space above
                                    // the page.
                                    .when(strip && !compact, |this| this.pt_2())
                                    .child(content),
                            )
                            // Lets the tests find this element by its id.
                            // Without the `test-support` feature this does
                            // nothing.
                            .test_support(),
                    ),
            )
    }
}
