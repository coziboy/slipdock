//! Small building blocks shared by the screens.
//!
//! In GPUI a piece of UI is a value you build with method chains, much like
//! HTML with utility classes: `div().flex().gap_2().child(...)`. The
//! functions here return such values, so screens can say `heading("Dump")`
//! instead of repeating the same chain.

use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use slipdock_core::report::Line;

pub fn heading(text: impl Into<SharedString>) -> Div {
    div()
        .text_xl()
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into())
}

pub fn subheading(text: impl Into<SharedString>) -> Div {
    div()
        .mt_3()
        .font_weight(FontWeight::SEMIBOLD)
        .child(text.into())
}

/// Small grey text for explanations.
pub fn hint(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

pub fn error_text(text: impl Into<SharedString>, cx: &App) -> Div {
    div()
        .text_sm()
        .text_color(cx.theme().danger)
        .child(text.into())
}

/// A form row: a fixed-width label, then the control.
pub fn field(label: &'static str, control: impl IntoElement, cx: &App) -> Div {
    h_flex()
        .gap_3()
        .items_center()
        .child(
            div()
                .w(px(110.))
                .flex_shrink_0()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(div().flex_1().min_w_0().child(control))
}

/// A bordered box that groups related things.
pub fn card(cx: &App) -> Div {
    v_flex()
        .gap_2()
        .p_4()
        .rounded_lg()
        .border_1()
        .border_color(cx.theme().border)
}

/// Text in a fixed-width font, one element per line so line breaks and
/// leading spaces are kept exactly. Used for command previews and logs.
pub fn mono(text: &str, cx: &App) -> Div {
    let theme = cx.theme();
    let mut block = v_flex()
        .p_3()
        .rounded_md()
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

/// Draws a summary or report from the core. The command line tool prints
/// the same `Line` values with terminal colours; here they become elements.
pub fn lines(lines: &[Line], cx: &App) -> Div {
    let theme = cx.theme();
    let muted = theme.muted_foreground;
    let note =
        move |note: &Option<String>| note.clone().map(|note| div().text_color(muted).child(note));
    let mut column = v_flex().gap_1();
    for line in lines {
        let row = match line {
            Line::Title(text) => div().font_weight(FontWeight::SEMIBOLD).child(text.clone()),
            Line::Row {
                label,
                value,
                note: row_note,
                strong,
            } => h_flex()
                .gap_3()
                .items_start()
                .child(
                    div()
                        .w(px(90.))
                        .flex_shrink_0()
                        .text_color(muted)
                        .child(label.clone()),
                )
                .child(
                    div()
                        // Lets a long value (a path) wrap instead of running
                        // out of the window.
                        .min_w_0()
                        .when(*strong, |this| this.font_weight(FontWeight::BOLD))
                        .child(value.clone()),
                )
                .children(note(row_note)),
            Line::Ok {
                text,
                note: ok_note,
            } => h_flex()
                .gap_2()
                .child(div().text_color(theme.success).child("✓"))
                .child(div().font_weight(FontWeight::SEMIBOLD).child(text.clone()))
                .children(note(ok_note)),
            Line::Fail(text) => h_flex()
                .gap_2()
                .child(div().text_color(theme.danger).child("✗"))
                .child(div().font_weight(FontWeight::SEMIBOLD).child(text.clone())),
            Line::Warn(text) => h_flex()
                .gap_2()
                .child(div().text_color(theme.warning).child("!"))
                .child(div().child(text.clone())),
            Line::Text(text) => div().pl_4().child(text.trim_start().to_string()),
            Line::Dim(text) => div().pl_4().text_color(muted).child(text.clone()),
            Line::Blank => div().h_2(),
        };
        column = column.child(row);
    }
    column
}
