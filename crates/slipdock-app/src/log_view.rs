//! The log viewer: the text of a log in a box that scrolls on its own.
//!
//! GPUI concept: **drawing only what is on show**. A log can be tens of
//! thousands of lines. An element per line, or one text element for all of
//! them, would be laid out in full on every frame. The component library's
//! code editor lays out only the rows in view (a "virtual" scroll), and its
//! text can be selected and copied, so the log is shown in one that is
//! read-only.
//!
//! The viewer is fed with [`LogView::push`]: text is added at the end and
//! the lines already there do not move.
//!
//! ```text
//!   at the end       new lines come into view (the log is followed)
//!   scrolled back    the view stays where it is; "Follow" goes back to
//!                    the end
//! ```
//!
//! The editor needs a window to change its text, and a run that reports
//! progress has none at hand. So `push` only keeps the text, and the next
//! draw (which has the window) hands it to the editor.

use gpui_kit::component::button::*;
use gpui_kit::component::clipboard::Clipboard;
use gpui_kit::component::input::{Editor, EditorState};
use gpui_kit::component::{IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;

use crate::widgets;

/// The most text the viewer holds. Over this, the older half is dropped
/// (the file has all of it): the editor is made for some 50 000 lines.
const MAX_BYTES: usize = 2 * 1024 * 1024;

/// The rows of the box: it is as high as its text, from this many to
/// [`LogView::rows`]. A box that starts at its first line (a quote) can be
/// as low as one row.
const MIN_ROWS: usize = 3;

/// What the editor adds to the height of its rows: padding and border.
const FRAME: Pixels = px(18.);

/// A scroll offset below any text. The editor brings it back to its end.
const END: Point<Pixels> = point(px(0.), px(-1.0e9));

pub struct LogView {
    /// Made at the first draw, which has the window it needs.
    editor: Option<Entity<EditorState>>,
    /// Pushed, and not yet in the editor.
    pending: String,
    /// The bytes of text in the editor.
    shown: usize,
    /// The view stays at the newest line.
    pub(crate) follow: bool,
    /// Where the editor was scrolled to when last looked at. Somewhere
    /// else the next time, it was scrolled in between.
    last_offset: Pixels,
    /// More lines may come: the button says "Follow", not "Go to end".
    live: bool,
    /// The most rows the box shows at once.
    max_rows: usize,
    /// The box holds a text that is whole from the start (what a tool
    /// printed, quoted in a report): it opens at its first line, where the
    /// cause is, and has no button to its end.
    from_start: bool,
    /// What the copy button says it copies.
    copy_tooltip: SharedString,
    /// Older lines were dropped to stay under [`MAX_BYTES`].
    trimmed: bool,
    /// `pending` lost its start, so it does not go on from the editor's text.
    gap: bool,
    _observe_editor: Option<Subscription>,
}

impl LogView {
    pub fn new(live: bool) -> Self {
        LogView {
            editor: None,
            pending: String::new(),
            shown: 0,
            follow: true,
            last_offset: px(0.),
            live,
            max_rows: 16,
            from_start: false,
            copy_tooltip: "Copy the log".into(),
            trimmed: false,
            gap: false,
            _observe_editor: None,
        }
    }

    /// Sets the most rows the box shows at once (16 when not set).
    pub fn rows(mut self, rows: usize) -> Self {
        self.max_rows = rows;
        self
    }

    /// A box for a text that is whole from the start: it opens at its
    /// first line and stays there. `copy_tooltip` names what is copied.
    pub fn quote(text: &str, copy_tooltip: impl Into<SharedString>) -> Self {
        let mut view = LogView::new(false);
        view.follow = false;
        view.from_start = true;
        view.copy_tooltip = copy_tooltip.into();
        // A text over the limit would wait to be followed, which a quote
        // never is: it keeps its start, where the cause is.
        let mut end = text.len().min(MAX_BYTES / 2);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        view.pending = text[..end].to_string();
        view
    }

    /// Adds lines at the end.
    pub fn push(&mut self, text: &str, cx: &mut Context<Self>) {
        if text.is_empty() {
            return;
        }
        self.pending.push_str(text);
        if self.pending.len() > MAX_BYTES {
            self.pending = newest_half(&self.pending).to_string();
            self.gap = true;
        }
        cx.notify();
    }

    /// No more lines will come.
    pub fn set_finished(&mut self, cx: &mut Context<Self>) {
        self.live = false;
        cx.notify();
    }

    pub fn is_empty(&self) -> bool {
        self.shown == 0 && self.pending.trim().is_empty()
    }

    /// All the text: what the copy button copies.
    pub fn text(&self, cx: &App) -> SharedString {
        let shown = self
            .editor
            .as_ref()
            .map(|editor| editor.read(cx).value())
            .unwrap_or_default();
        if self.pending.is_empty() {
            return shown;
        }
        if shown.is_empty() || self.gap {
            return self.pending.trim_end().to_string().into();
        }
        format!("{shown}\n{}", self.pending.trim_end()).into()
    }

    /// Goes to the newest line and stays there.
    pub fn go_to_end(&mut self, cx: &mut Context<Self>) {
        self.follow = true;
        if let Some(editor) = &self.editor {
            editor.update(cx, |state, cx| state.set_scroll_offset(END, cx));
        }
        cx.notify();
    }

    /// Where the editor is scrolled to, and whether that is its end.
    #[cfg(test)]
    pub(crate) fn scroll(&self, cx: &App) -> (Pixels, bool) {
        let state = self.editor.as_ref().expect("drawn once").read(cx);
        (state.scroll_offset().y, at_end(state))
    }

    /// Puts the cursor at the end of the text, as a click there would.
    #[cfg(test)]
    pub(crate) fn cursor_to_end(&self, cx: &mut App) {
        self.editor
            .as_ref()
            .expect("drawn once")
            .update(cx, |state, cx| {
                let end = state.text().len();
                state.set_selected_range(end..end, cx);
            });
    }

    /// Where the text is drawn in the window.
    #[cfg(test)]
    pub(crate) fn bounds(&self, cx: &App) -> Bounds<Pixels> {
        self.editor
            .as_ref()
            .expect("drawn once")
            .read(cx)
            .input_bounds()
    }

    fn editor(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<EditorState> {
        if let Some(editor) = &self.editor {
            return editor.clone();
        }
        let editor = cx.new(|cx| {
            EditorState::new(window, cx)
                .line_number(false)
                .folding(false)
                .soft_wrap(true)
                // The last line is at the bottom of the box, with no empty
                // rows under it, and the box does not bounce there.
                .scroll_beyond_last_line(Some(0))
                .scroll_bounce(false)
        });
        self.editor = Some(editor.clone());
        if self.from_start {
            // Nothing is added to a quote, so there is no end to follow.
            return editor;
        }
        // Runs whenever the editor changed: among other things, when it
        // was scrolled.
        self._observe_editor = Some(cx.observe(&editor, |this, editor, cx| {
            let state = editor.read(cx);
            let offset = state.scroll_offset().y;
            let at_end = at_end(state);
            let moved = offset != this.last_offset;
            this.last_offset = offset;
            // Followed: lines that were added move the end away, which is
            // not the reader leaving it; a scroll to anywhere else is.
            let follow = if this.follow {
                !moved || at_end
            } else {
                at_end
            };
            if follow != this.follow {
                this.follow = follow;
                cx.notify();
            }
        }));
        editor
    }

    /// Hands the pushed text to the editor.
    fn flush(&mut self, editor: &Entity<EditorState>, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending.is_empty() {
            return;
        }
        let over = self.gap || self.shown + self.pending.len() > MAX_BYTES;
        if over && !self.follow {
            // Dropping old lines would move the text under the reader, so
            // the new lines wait until the log is followed again.
            return;
        }
        let pending = std::mem::take(&mut self.pending);
        let added = pending.trim_end_matches('\n');
        // With too much text, the newest half of all of it replaces it.
        let replacement = over.then(|| {
            let all = if self.gap || self.shown == 0 {
                added.to_string()
            } else {
                format!("{}\n{added}", editor.read(cx).value())
            };
            newest_half(&all).to_string()
        });
        self.trimmed |= over;
        self.gap = false;
        let follow = self.follow;
        let first = self.shown == 0;
        (self.last_offset, self.shown) = editor.update(cx, |state, cx| {
            let offset = state.scroll_offset();
            match replacement {
                Some(text) => state.set_value(text, window, cx),
                None => {
                    // The editor adds text where its cursor is: the cursor
                    // goes to the end for that, and then back, so a
                    // selection of the reader is kept.
                    let selection = state.selected_range();
                    let end = state.text().len();
                    state.set_selected_range(end..end, cx);
                    let added = if first {
                        added.to_string()
                    } else {
                        format!("\n{added}")
                    };
                    state.insert(added, window, cx);
                    state.set_selected_range(selection, cx);
                }
            }
            // Moving the cursor scrolled the box; this puts it where it
            // has to be, at the layout that follows in this same draw. So
            // the observer above never sees the box in between.
            state.set_scroll_offset(if follow { END } else { offset }, cx);
            (state.scroll_offset().y, state.text().len())
        });
    }
}

/// Whether the last row of the editor is in view.
fn at_end(state: &EditorState) -> bool {
    // Before the first layout nothing was scrolled.
    let Some(line_height) = state.line_height() else {
        return true;
    };
    let text = line_height * state.display_map().wrap_row_count() as f32;
    let bottom = state.input_bounds().size.height - state.scroll_offset().y;
    bottom >= text - line_height
}

/// The newest half of [`MAX_BYTES`] of `text`, from the start of a line.
fn newest_half(text: &str) -> &str {
    let Some(from) = text.len().checked_sub(MAX_BYTES / 2) else {
        return text;
    };
    // A line break is one byte, so the byte after it starts a character.
    match text.as_bytes()[from..]
        .iter()
        .position(|byte| *byte == b'\n')
    {
        Some(line_break) => &text[from + line_break + 1..],
        None => "",
    }
}

impl Render for LogView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let editor = self.editor(window, cx);
        self.flush(&editor, window, cx);

        let state = editor.read(cx);
        let rows = match state.display_map().wrap_row_count() {
            // Not laid out yet: no line is known to wrap.
            0 => state.display_map().buffer_line_count(),
            rows => rows,
        };
        let line_height = state.line_height().unwrap_or(px(20.));
        let min_rows = if self.from_start { 1 } else { MIN_ROWS };
        let height = line_height * rows.clamp(min_rows, self.max_rows) as f32 + FRAME;
        let this = cx.entity().downgrade();

        v_flex()
            .gap_1()
            .min_w_0()
            .child(
                h_flex()
                    .flex_wrap()
                    .items_center()
                    .justify_end()
                    .gap_2()
                    // As high as the button that comes and goes, so the
                    // box under the row does not move with it.
                    .min_h(px(28.))
                    .when(self.trimmed, |row| {
                        row.child(
                            widgets::hint("The first lines are only in the log file.", cx).flex_1(),
                        )
                    })
                    .when(!self.follow && !self.from_start, |row| {
                        row.child(
                            Button::new("log-to-end")
                                .small()
                                .outline()
                                .icon(IconName::ArrowDown)
                                .label(if self.live { "Follow" } else { "Go to end" })
                                .on_click(cx.listener(|this, _, _, cx| this.go_to_end(cx))),
                        )
                    })
                    .child(
                        Clipboard::new("copy-log")
                            // Asked for at the click, not at every draw:
                            // the text can be long.
                            .value_fn(move |_, cx| {
                                this.upgrade()
                                    .map(|view| view.read(cx).text(cx))
                                    .unwrap_or_default()
                            })
                            .tooltip(self.copy_tooltip.clone()),
                    ),
            )
            .child(Editor::new(&editor).h(height).readonly(true))
    }
}
