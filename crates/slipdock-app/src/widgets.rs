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
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::tag::Tag;
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use slipdock_core::dumps::{self, DumpInfo};
use slipdock_core::job;
use slipdock_core::report::Line;

use crate::theme;

/// The top of a screen: its name and one line about what it is for.
pub fn header(title: &'static str, about: &'static str, cx: &App) -> Div {
    v_flex()
        .gap_1()
        .min_w_0()
        .child(div().text_3xl().font_weight(FontWeight::BOLD).child(title))
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

/// A block of a profile's band colour. Without a colour (a dump of a profile
/// that is gone, a folder named by hand) the block is an outline.
fn band_block(color: Option<Hsla>, width: f32, height: f32, cx: &App) -> Div {
    let block = div()
        .w(px(width))
        .h(px(height))
        .flex_shrink_0()
        .rounded(px(1.));
    match color {
        Some(color) => block.bg(color),
        None => block.border_1().border_color(cx.theme().muted_foreground),
    }
}

/// The band at the start of a row: one block, as high as two lines of text.
/// A band that is not `whole` is broken in three: a partial dump. The shape
/// says it as well as the colour of its tag does.
pub fn band_mark(color: Option<Hsla>, whole: bool, cx: &App) -> Div {
    if whole {
        return band_block(color, 10., 36., cx);
    }
    v_flex()
        .flex_shrink_0()
        .gap(px(3.))
        .children((0..3).map(|_| band_block(color, 10., 10., cx)))
}

/// One of the profiles to choose from, as a label: its band, then its name.
/// The band of the chosen one is wider and its name is bold.
pub fn profile_choice(id: impl Into<ElementId>, name: &str, chosen: bool, cx: &App) -> Button {
    let width = if chosen { 28. } else { 10. };
    let theme = cx.theme();
    Button::new(id)
        .outline()
        .max_w_full()
        // Chosen as a row is chosen: an edge in ink.
        .when(chosen, |this| {
            this.border_color(theme.primary).bg(theme.background)
        })
        // The button holds drawn parts, so it is given its name.
        .accessibility_label(name.to_string())
        .child(
            h_flex()
                .gap_2()
                .min_w_0()
                .child(band_block(theme::band(name, cx), width, 16., cx))
                .child(
                    div()
                        .min_w_0()
                        .font_family(cx.theme().mono_font_family.clone())
                        .text_sm()
                        .when(chosen, |this| this.font_weight(FontWeight::BOLD))
                        .child(name.to_string()),
                ),
        )
}

/// The label that heads a form: the band of the chosen profile across the
/// start, its name, and its server at the end of the line. What follows
/// under it is about this profile. The label is there before a profile is
/// chosen too, empty, so that choosing one does not move the form.
pub fn profile_strip(name: Option<&str>, server: Option<&str>, cx: &App) -> Div {
    let theme = cx.theme();
    let band = name.and_then(|name| theme::band(name, cx));
    h_flex()
        // The band is as high as the text beside it, also when that wraps.
        .items_stretch()
        .min_w_0()
        .overflow_hidden()
        .rounded(theme::RADIUS)
        .border_1()
        .border_color(theme.border)
        .bg(theme.background)
        .child(
            div()
                .w(px(44.))
                .min_h(px(40.))
                .flex_shrink_0()
                .map(|this| match band {
                    Some(color) => this.bg(color),
                    // No band: the place of one, marked off by a line.
                    None => this.border_r_1().border_color(theme.border),
                }),
        )
        .child(
            h_flex()
                .flex_1()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_x_4()
                .min_w_0()
                .px_4()
                .py_2()
                .font_family(theme.mono_font_family.clone())
                .text_sm()
                .child(match name {
                    Some(name) => div()
                        .min_w_0()
                        .font_weight(FontWeight::BOLD)
                        .child(name.to_string()),
                    None => div()
                        .min_w_0()
                        .text_color(theme.muted_foreground)
                        .child("No profile chosen"),
                })
                .when_some(server, |this, server| {
                    this.child(
                        div()
                            .min_w_0()
                            .text_color(theme.muted_foreground)
                            .child(server.to_string()),
                    )
                }),
        )
}

/// A bordered box that groups related things.
pub fn card(cx: &App) -> Div {
    let theme = cx.theme();
    v_flex()
        .gap_3()
        .p_4()
        .min_w_0()
        .rounded(theme::RADIUS)
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
        .rounded(theme::RADIUS)
        .border_1()
        .border_color(if chosen { theme.primary } else { theme.border })
        .bg(if chosen {
            theme.accent
        } else {
            theme.background
        })
        .when(!off, |this| {
            let hover = theme.accent;
            this.cursor_pointer().hover(move |style| style.bg(hover))
        })
}

/// The mark at the start of an `option_row`: filled when chosen.
/// A row that cannot be chosen (`off`) has a faint one.
pub fn option_mark(chosen: bool, off: bool, cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .size(px(14.))
        .flex_shrink_0()
        .rounded_full()
        .border_1()
        .border_color(if chosen {
            theme.primary
        } else if off {
            theme.border
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
                .rounded(theme::RADIUS)
                .text_sm()
                .font_family(theme.mono_font_family.clone())
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
                .rounded(theme::RADIUS)
                .border_1()
                .border_color(theme.border)
                .child(rows),
        ),
    )
}

/// A dump as every list shows it: the band of its profile, when it was
/// taken, and its folder name. The caller puts its own controls around it (a checkbox and
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
    // The band is of the profile in the dump's name, not of the folder the
    // dump lies in: a dump keeps saying where it came from.
    let band = dumps::profile_of(&dump.name).and_then(|profile| theme::band(profile, cx));
    h_flex()
        .gap_3()
        .min_w_0()
        .child(band_mark(band, dump.complete, cx))
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
                    this.child(
                        hint(folder, cx)
                            .text_xs()
                            .font_family(theme.mono_font_family.clone()),
                    )
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

/// A message in a box with an edge and an icon in the colour of its tone.
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
        .rounded(theme::RADIUS)
        .border_1()
        .border_color(color)
        .bg(theme.background)
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
        .rounded(theme::RADIUS)
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

/// A command as `Invocation::preview` writes it, one option per line, on
/// the fill of a `mono` block.
///
/// GPUI concept: **a hanging indent**. Text that wraps starts again at the
/// left edge of its own element. So the spaces that lead a line become
/// padding of the row, and the value of an option is an element of its own:
/// a long path then continues under where it started, not under the `env`
/// of the first line.
///
/// What is drawn is what `copy_button` copies: only the place of the line
/// breaks differs.
fn command_block(command: &str, cx: &App) -> Div {
    let theme = cx.theme();
    let mut block = v_flex()
        .p_3()
        .gap_0p5()
        .min_w_0()
        .rounded(theme::RADIUS)
        .border_1()
        .border_color(theme.border)
        .bg(theme.muted)
        .font_family(theme.mono_font_family.clone())
        .text_sm();
    for line in command.lines() {
        let text = line.trim();
        // An option with a value: `--outputdir /dumps/example \`.
        let (option, value) = match text.split_once(' ') {
            // The mark that the command goes on (`--routines \`) is no value.
            Some((option, value)) if option.starts_with('-') && value != "\\" => {
                (option, Some(value))
            }
            _ => (text, None),
        };
        // Every line after the first belongs to the tool named in the first.
        let continued = line.starts_with("   ");
        block = block.child(
            h_flex()
                .flex_wrap()
                .items_start()
                .gap_x_2()
                .min_w_0()
                .when(continued, |row| row.pl_6())
                .child(match value {
                    // May get narrower than its text: an option can be one
                    // long word (`--tables-list=...`), and that has to wrap.
                    Some(_) => div().min_w_0().child(option.to_string()),
                    None => div().flex_1().min_w_0().child(option.to_string()),
                })
                .when_some(value, |row, value| {
                    // In a narrow window the value moves under its option
                    // and wraps there.
                    row.child(
                        div()
                            .flex_grow(1.)
                            .flex_basis(px(160.))
                            .min_w_0()
                            .child(value.to_string()),
                    )
                }),
        );
    }
    block
}

/// A button that says what it copies, and says "Copied" for two seconds
/// after it did. The library's `Clipboard` is the same thing as an icon
/// only; this one is for where the copy is the point of the card.
///
/// GPUI concept: **state of an element**. A function that builds elements
/// keeps nothing between two draws. `Window::use_keyed_state` gives an
/// element a value that lives as long as an element with this id is drawn,
/// which is what "was just clicked" needs. Only a component (`RenderOnce`)
/// gets the window while it is built, so this is one.
#[derive(IntoElement)]
struct CopyButton {
    id: ElementId,
    label: SharedString,
    value: SharedString,
}

fn copy_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    value: impl Into<SharedString>,
) -> CopyButton {
    CopyButton {
        id: id.into(),
        label: label.into(),
        value: value.into(),
    }
}

impl RenderOnce for CopyButton {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let copied = window.use_keyed_state(self.id.clone(), cx, |_, _| false);
        let done = *copied.read(cx);
        let value = self.value;
        Button::new(self.id)
            .outline()
            .small()
            .flex_shrink_0()
            .icon(if done {
                IconName::Check
            } else {
                IconName::Copy
            })
            .label(if done { "Copied".into() } else { self.label })
            // Off while it says "Copied", so one timer runs at a time.
            .when(!done, |this| {
                this.on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(value.to_string()));
                    copied.update(cx, |copied, cx| {
                        *copied = true;
                        cx.notify();
                    });
                    let copied = copied.clone();
                    cx.spawn(async move |cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_secs(2))
                            .await;
                        let _ = copied.update(cx, |copied, cx| {
                            *copied = false;
                            cx.notify();
                        });
                    })
                    .detach();
                })
            })
    }
}

/// What "Show command" shows. From the top: that nothing ran, the summary,
/// then the command under a button that copies it.
pub fn preview(summary: &[Line], command: &str, cx: &App) -> Div {
    let muted = cx.theme().muted_foreground;
    card(cx)
        // First, because it is what the reader has to know before the rest:
        // on the Load screen the summary names a database that would be
        // overwritten.
        .child(marked(
            IconName::Info,
            muted,
            div().child("Nothing was executed."),
        ))
        .child(lines(summary, cx))
        .child(
            v_flex()
                .gap_2()
                .min_w_0()
                .child(
                    // The title of the block, with the button that copies
                    // the block at the end of its line.
                    h_flex()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .min_w_0()
                        .child(subheading("Command", cx))
                        .child(copy_button(
                            "copy-command",
                            "Copy command",
                            command.trim().to_string(),
                        )),
                )
                .child(command_block(command, cx)),
        )
        .when(command.contains("slipdock-"), |this| {
            this.child(hint(job::TEMPORARY_DEFAULTS_NOTE, cx))
        })
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
            Line::Quote(text) => div().pl_4().text_color(muted).child(format!("│ {text}")),
            Line::Blank => div().h_2(),
        };
        column = column.child(row);
    }
    column
}
