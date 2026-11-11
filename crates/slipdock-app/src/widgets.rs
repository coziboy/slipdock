//! Small building blocks shared by the screens.
//!
//! In GPUI a piece of UI is a value you build with method chains, much like
//! HTML with utility classes: `div().flex().gap_2().child(...)`. The
//! functions here return such values, so screens can say `header("Dump", ..)`
//! instead of repeating the same chain.
//!
//! GPUI concept: **layout that follows the window**. GPUI lays elements out
//! with flexbox, and three rules keep content inside a window of any size:
//!
//! - A row that may hold more than fits gets `flex_wrap()`, so the rest
//!   continues on the next line instead of running out of the window.
//! - A flex child never gets narrower than its content unless it has
//!   `min_w_0()`. With it, long text (a path, a host name) wraps.
//! - `flex_basis(width)` with `flex_grow(1.)` says "I want this much, and
//!   take what is left". In a wrapping row, an item whose basis no longer
//!   fits moves to the next line and fills it. `field` uses this to put the
//!   control under its label in a narrow window.

use std::path::Path;

use gpui_kit::component::button::*;
use gpui_kit::component::clipboard::Clipboard;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{ActiveTheme, Icon, IconName, Selectable, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use slipdock_core::dumps::{self, DumpInfo};
use slipdock_core::job;
use slipdock_core::report::Line;

/// The top of a screen: its name and one line about what it is for.
pub fn header(title: &'static str, about: &'static str, cx: &App) -> Div {
    v_flex()
        .gap_1()
        .min_w_0()
        .child(
            div()
                .text_2xl()
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(hint(about, cx))
}

/// A screen: sections under each other.
pub fn page() -> Div {
    v_flex().gap_5().w_full().min_w_0()
}

/// Small grey text for explanations.
pub fn hint(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .min_w_0()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

/// A folder as the forms show it and as it is written in a profile: the home folder shortened to `~`,
/// nothing escaped (the core expands `~` again and reads the rest as it is).
pub fn tilde_path(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|home| path.strip_prefix(home).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.to_string_lossy()),
        None => path.to_string_lossy().into_owned(),
    }
}

/// A spinner with a line about what is being fetched or prepared. Shown
/// while work runs in the background.
pub fn loading(text: impl Into<SharedString>, cx: &App) -> Div {
    h_flex()
        .gap_2()
        .min_w_0()
        .child(Spinner::new().small())
        .child(hint(text, cx).flex_1())
}

/// A form row: a fixed-width label, then the control. When the window is
/// too narrow for both, the control moves under the label.
pub fn field(label: &'static str, control: impl IntoElement, cx: &App) -> Div {
    h_flex()
        .flex_wrap()
        .items_start()
        .gap_x_3()
        .gap_y_1()
        .child(
            // As high as an input, so the label sits beside its first line.
            h_flex()
                .w(px(110.))
                .h(px(32.))
                .flex_shrink_0()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(
            div()
                .flex_grow(1.)
                .flex_basis(px(220.))
                .min_w_0()
                .child(control),
        )
}

/// A row of things that continues on the next line when it is full: choice
/// buttons, action buttons.
pub fn wrap_row() -> Div {
    h_flex().flex_wrap().gap_2().min_w_0()
}

/// One of several choices (a profile, a log level). The chosen one is
/// filled.
pub fn choice(id: impl Into<ElementId>, label: impl Into<SharedString>, chosen: bool) -> Button {
    let button = Button::new(id).label(label).max_w_full().selected(chosen);
    if chosen {
        button.primary()
    } else {
        button.outline()
    }
}

/// A bordered box that groups related things.
pub fn card(cx: &App) -> Div {
    let theme = cx.theme();
    v_flex()
        .gap_3()
        .p_4()
        .min_w_0()
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .bg(theme.background)
}

/// A card with a small title.
pub fn section(title: impl Into<SharedString>, cx: &App) -> Div {
    card(cx).child(subheading(title, cx))
}

/// The small title of a group of fields.
pub fn subheading(title: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(cx.theme().muted_foreground)
        .child(title.into().to_uppercase())
}

/// A row in a list that can be clicked as a whole (a dump, a profile). Its
/// text wraps, which the label of a button cannot do. The caller adds the
/// content and, unless it is off, `on_click`.
pub fn option_row(
    id: impl Into<ElementId>,
    name: impl Into<SharedString>,
    chosen: bool,
    off: bool,
    cx: &App,
) -> Stateful<Div> {
    let theme = cx.theme();
    h_flex()
        .id(id)
        // The row is drawn by us, so we say what it is for screen readers.
        .role(Role::Button)
        .aria_label(name)
        .gap_3()
        .px_3()
        .py_2()
        .min_w_0()
        .rounded_md()
        .border_1()
        .border_color(if chosen { theme.primary } else { theme.border })
        .when(chosen, |this| this.bg(theme.accent))
        .when(off, |this| this.opacity(0.55))
        .when(!off, |this| {
            let hover = theme.accent;
            this.cursor_pointer().hover(move |style| style.bg(hover))
        })
}

/// The mark at the start of an `option_row`: filled when chosen.
pub fn option_mark(chosen: bool, cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .size(px(14.))
        .flex_shrink_0()
        .rounded_full()
        .border_1()
        .border_color(if chosen {
            theme.primary
        } else {
            theme.muted_foreground
        })
        .when(chosen, |this| this.border_4())
}

/// The rows `database_list` draws at most. A server can have thousands of
/// databases; the name field narrows the list down.
pub const DATABASE_ROWS: usize = 50;

/// The databases of `databases` that a typed name lets through, in order:
/// those that contain it, without regard to case. An empty name lets all
/// through.
pub fn matching_databases<'a>(databases: &'a [String], typed: &str) -> Vec<&'a String> {
    let typed = typed.trim().to_lowercase();
    databases
        .iter()
        .filter(|name| typed.is_empty() || name.to_lowercase().contains(&typed))
        .collect()
}

/// The list under a database name field: the databases of the server that
/// match what is typed, one per row, in a box that scrolls on its own. A
/// click on a row calls what `on_pick` made for its name.
///
/// Nothing is drawn when the server listed no databases. Once the typed
/// name is one of the databases, the list gives way to one line that says
/// so: the choice is made, and clearing the field brings the list back.
pub fn database_list(
    id: &'static str,
    databases: &[String],
    typed: &str,
    on_pick: impl Fn(String) -> Box<dyn Fn(&ClickEvent, &mut Window, &mut App)>,
    cx: &App,
) -> Option<Div> {
    if databases.is_empty() {
        return None;
    }
    let theme = cx.theme();
    let typed = typed.trim();
    if databases.iter().any(|name| name == typed) {
        return Some(
            h_flex()
                .gap_1p5()
                .min_w_0()
                .child(
                    Icon::new(IconName::CircleCheck)
                        .small()
                        .text_color(theme.success),
                )
                .child(hint("A database on this server.", cx).flex_1()),
        );
    }
    let matching = matching_databases(databases, typed);
    let count = if typed.is_empty() {
        format!("{} databases on this server", databases.len())
    } else {
        format!("{} of {} databases match", matching.len(), databases.len())
    };
    if matching.is_empty() {
        return Some(hint(count, cx));
    }
    let hover = theme.accent;
    let mut rows = v_flex();
    for (index, name) in matching.iter().take(DATABASE_ROWS).enumerate() {
        rows = rows.child(
            div()
                .id((id, index))
                .role(Role::Button)
                .aria_label((*name).clone())
                .px_2()
                .py_1()
                .min_w_0()
                .rounded_sm()
                .text_sm()
                .cursor_pointer()
                .hover(move |style| style.bg(hover))
                .child((*name).clone())
                .on_click(on_pick((*name).clone())),
        );
    }
    if matching.len() > DATABASE_ROWS {
        rows = rows.child(
            hint(
                format!(
                    "and {} more: type a part of the name to narrow the list",
                    matching.len() - DATABASE_ROWS
                ),
                cx,
            )
            .px_2()
            .py_1(),
        );
    }
    Some(
        v_flex().gap_1().min_w_0().child(hint(count, cx)).child(
            div()
                .id(id)
                .max_h(px(180.))
                .overflow_y_scroll()
                // Only a list long enough to scroll keeps the scroll from
                // the page behind it.
                .when(matching.len() > 6, |this| this.occlude())
                .p_1()
                .rounded_md()
                .border_1()
                .border_color(theme.border)
                .child(rows),
        ),
    )
}

/// A dump as every list shows it: an icon, when it was taken, and its
/// folder name. The caller puts its own controls around it (a checkbox and
/// buttons on the Dumps screen, the mark of a choice on the Load screen).
pub fn dump_summary(dump: &DumpInfo, cx: &App) -> Div {
    let theme = cx.theme();
    // The date in the name is what tells dumps apart, so it is the title
    // and the folder name goes under it. A folder named by hand has no
    // date: its name is the title.
    let (title, folder) = match dumps::taken(&dump.name) {
        Some(taken) => (taken, Some(dump.name.clone())),
        None => (dump.name.clone(), None),
    };
    let tile_color = if dump.complete {
        theme.muted_foreground
    } else {
        theme.warning
    };
    h_flex()
        .gap_3()
        .min_w_0()
        .child(
            h_flex()
                .size(px(36.))
                .flex_shrink_0()
                .justify_center()
                .rounded_md()
                .bg(tile_color.opacity(0.12))
                .child(Icon::new(gpui_kit::assets::IconName::Database).text_color(tile_color)),
        )
        .child(
            v_flex()
                .flex_1()
                .min_w_0()
                .child(
                    wrap_row()
                        .items_center()
                        .child(div().min_w_0().font_weight(FontWeight::MEDIUM).child(title))
                        .when(!dump.complete, |this| {
                            this.child(tag("partial", Tone::Warning))
                        }),
                )
                .when_some(folder, |this, folder| {
                    this.child(hint(folder, cx).text_xs())
                }),
        )
}

/// What a dump holds: `65 tables · 15M`.
pub fn dump_stats(dump: &DumpInfo, cx: &App) -> Div {
    hint(
        format!("{} tables · {}", dump.tables, dumps::human_size(dump.size)),
        cx,
    )
    .flex_shrink_0()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Success,
    Danger,
    Warning,
    Info,
}

/// A small coloured label for a state: "complete", "partial", "missing".
pub fn tag(text: impl Into<SharedString>, tone: Tone) -> Tag {
    let tag = match tone {
        Tone::Success => Tag::success(),
        Tone::Danger => Tag::danger(),
        Tone::Warning => Tag::warning(),
        Tone::Info => Tag::secondary(),
    };
    tag.outline().small().child(text.into())
}

/// Shown in place of the rest of a form when the server of the chosen
/// profile did not answer: the reason and "Try again". The caller gives the
/// button its action.
pub fn connection_failed(profile: &str, error: &str, retry: Button, cx: &App) -> Div {
    section("Connection failed", cx)
        .child(error_text(
            format!("Could not connect with {profile}.\n{error}"),
            cx,
        ))
        .child(hint(
            "Check the host, the user and the password on the Profiles screen \
             (\"Test connection\"), and that the server can be reached from here, \
             for example through a VPN.",
            cx,
        ))
        .child(wrap_row().child(retry.icon(IconName::RefreshCw).label("Try again")))
}

/// A message in a tinted box, with an icon for its tone.
pub fn notice(tone: Tone, text: &str, cx: &App) -> Div {
    let theme = cx.theme();
    let (color, icon) = match tone {
        Tone::Success => (theme.success, IconName::CircleCheck),
        Tone::Danger => (theme.danger, IconName::CircleX),
        Tone::Warning => (theme.warning, IconName::TriangleAlert),
        Tone::Info => (theme.muted_foreground, IconName::Info),
    };
    let mut body = v_flex().flex_1().min_w_0().text_sm();
    for line in text.lines() {
        body = body.child(div().child(line.to_string()));
    }
    h_flex()
        .items_start()
        .gap_2()
        .p_3()
        .min_w_0()
        .rounded_md()
        .border_1()
        .border_color(color.opacity(0.35))
        .bg(color.opacity(0.08))
        .child(
            // As high as a line of the text, so the icon sits beside the
            // first line.
            h_flex()
                .h(px(20.))
                .flex_shrink_0()
                .child(Icon::new(icon).small().text_color(color)),
        )
        .child(body)
}

pub fn error_text(text: impl Into<SharedString>, cx: &App) -> Div {
    notice(Tone::Danger, &text.into(), cx)
}

/// Text in a fixed-width font, one element per line so line breaks and
/// leading spaces are kept exactly. Used for command previews and logs.
pub fn mono(text: &str, cx: &App) -> Div {
    let theme = cx.theme();
    let mut block = v_flex()
        .p_3()
        .min_w_0()
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .bg(theme.muted)
        .font_family(theme.mono_font_family.clone())
        .text_sm();
    for line in text.lines() {
        // An empty div has no height, so an empty line gets a space.
        let line = if line.is_empty() { " " } else { line };
        block = block.child(div().child(line.to_string()));
    }
    block
}

/// What "Show command" shows: the summary, the command with a button that
/// copies it, and that nothing ran.
pub fn preview(summary: &[Line], command: &str, cx: &App) -> Div {
    card(cx)
        .child(
            // The title, with the copy button at the end of its line.
            h_flex()
                .items_center()
                .justify_between()
                .gap_2()
                .child(subheading("Command", cx))
                .child(
                    Clipboard::new("copy-command")
                        .value(command.trim().to_string())
                        .tooltip("Copy the command"),
                ),
        )
        .child(lines(summary, cx))
        .child(mono(command, cx))
        .when(command.contains("slipdock-"), |this| {
            this.child(hint(job::TEMPORARY_DEFAULTS_NOTE, cx))
        })
        .child(hint("Nothing was executed.", cx))
}

/// An icon with a line of text beside it; the text wraps.
fn marked(icon: IconName, color: Hsla, text: Div) -> Div {
    h_flex()
        .items_start()
        .gap_2()
        .min_w_0()
        .child(
            h_flex()
                .h(px(24.))
                .flex_shrink_0()
                .child(Icon::new(icon).small().text_color(color)),
        )
        .child(text.flex_1().min_w_0())
}

/// Draws a summary or report from the core. The command line tool prints
/// the same `Line` values with terminal colours; here they become elements.
pub fn lines(lines: &[Line], cx: &App) -> Div {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let note = move |note: &Option<String>| {
        note.clone()
            .map(|note| div().text_sm().text_color(muted).child(note))
    };
    let mut column = v_flex().gap_1().min_w_0();
    for line in lines {
        // The core says which log level a run was given. The app gives
        // every run the same one (`state::LOG_LEVEL`) and offers no choice,
        // so the row would only be noise here.
        if matches!(line, Line::Row { label, .. } if label == "Log level") {
            continue;
        }
        let row = match line {
            Line::Title(text) => div().font_weight(FontWeight::SEMIBOLD).child(text.clone()),
            Line::Row {
                label,
                value,
                note: row_note,
                strong,
            } => h_flex()
                .flex_wrap()
                .items_start()
                .gap_x_3()
                .child(
                    div()
                        .w(px(90.))
                        .flex_shrink_0()
                        .text_color(muted)
                        .child(label.clone()),
                )
                .child(
                    // The note goes under the value, so a long value (a
                    // path) and a long note (a server) each get the full
                    // width to wrap in.
                    v_flex()
                        .flex_grow(1.)
                        .flex_basis(px(200.))
                        .min_w_0()
                        .child(
                            div()
                                .when(*strong, |this| this.font_weight(FontWeight::BOLD))
                                .child(value.clone()),
                        )
                        .children(note(row_note)),
                ),
            Line::Ok {
                text,
                note: ok_note,
            } => marked(
                IconName::CircleCheck,
                theme.success,
                v_flex()
                    .child(div().font_weight(FontWeight::SEMIBOLD).child(text.clone()))
                    .children(note(ok_note)),
            ),
            Line::Fail(text) => marked(
                IconName::CircleX,
                theme.danger,
                div().font_weight(FontWeight::SEMIBOLD).child(text.clone()),
            ),
            Line::Warn(text) => marked(
                IconName::TriangleAlert,
                theme.warning,
                div().child(text.clone()),
            ),
            Line::Text(text) => div().pl_4().child(text.trim_start().to_string()),
            Line::Dim(text) => div().pl_4().text_color(muted).child(text.clone()),
            Line::Blank => div().h_2(),
        };
        column = column.child(row);
    }
    column
}
