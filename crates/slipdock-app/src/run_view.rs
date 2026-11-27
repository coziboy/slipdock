//! The run view: a dump or load in progress, then its report.
//!
//! GPUI concept: **background tasks**. The window is drawn on the main
//! thread, so nothing slow may happen there. `cx.spawn(async ...)` starts a
//! task on the main thread that can *await* without blocking it;
//! `cx.background_executor().spawn(...)` runs a piece of work on a thread
//! pool. The pattern below combines them: wait for the next event on a
//! background thread, then come back to the main thread to update the view.
//!
//! The core is not async at all. It sends events over a plain
//! `std::sync::mpsc` channel; the only GPUI-specific part is this file.

use std::path::PathBuf;

use gpui_kit::TestSupportExt as _;
use gpui_kit::component::Disableable as _;
use gpui_kit::component::button::*;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use slipdock_core::job::Prepared;
use slipdock_core::report::{self, Line, Report};
use slipdock_core::run::{Cancel, Event, LogReader, Progress};
use slipdock_core::tool_event::{self, DateTime, Level, RunState, Stage, TableStatus};

use crate::log_view::LogView;
use crate::state::core;
use crate::theme;
use crate::widgets;

/// The most of the log that is read at once. A run adds far less than
/// this between two reads, so the log box gets every line.
const READ_BYTES: u64 = 1024 * 1024;

pub struct RunView {
    label: SharedString,
    summary: Vec<Line>,
    log: PathBuf,
    progress: Option<Progress>,
    /// The stages of this kind of run, for the stepper.
    stages: Vec<Stage>,
    /// Reads what the run added to the log file since the last read.
    reader: LogReader,
    /// The log box, with the lines read so far.
    pub(crate) log_view: Entity<LogView>,
    /// The lines of the log are on show. They start hidden: the line with
    /// the spinner says how far the run is, and the log is for looking
    /// closer.
    pub(crate) log_open: bool,
    /// Present while the tool runs.
    cancel: Option<Cancel>,
    cancelling: bool,
    /// Present when the run has ended.
    pub(crate) report: Option<Report>,
    /// After a failed run: what the tool printed, whole, in a box of the
    /// report whose text can be selected and copied.
    pub(crate) tool_output: Option<Entity<LogView>>,
    /// Kept until the run ends: it owns the temporary defaults file.
    prepared: Option<Prepared>,
    /// A task is cancelled when its handle is dropped, so the view keeps it.
    _events: Option<Task<()>>,
}

impl RunView {
    pub fn start(prepared: Prepared, summary: Vec<Line>, cx: &mut Context<Self>) -> Self {
        let mut view = RunView {
            label: prepared.label.clone().into(),
            summary,
            log: prepared.log.clone(),
            progress: None,
            stages: prepared.stages(),
            reader: LogReader::new(&prepared.log, READ_BYTES),
            log_view: cx.new(|_| LogView::new(true)),
            log_open: false,
            cancel: None,
            cancelling: false,
            report: None,
            tool_output: None,
            prepared: None,
            _events: None,
        };
        match prepared.start() {
            Ok(running) => {
                view.cancel = Some(running.cancel.clone());
                view.prepared = Some(prepared);
                let mut events = running.events;
                // `this` is a weak handle to the view: if the view is gone,
                // `this.update` fails and the loop ends.
                view._events = Some(cx.spawn(async move |this, cx| {
                    loop {
                        // Waiting on the channel blocks, so it happens on a
                        // background thread. The receiver travels there and back.
                        let (event, receiver) = cx
                            .background_executor()
                            .spawn(async move {
                                let event = events.recv();
                                (event, events)
                            })
                            .await;
                        events = receiver;
                        let Ok(event) = event else { break };
                        let finished = matches!(event, Event::Finished(_));
                        let updated = this.update(cx, |this, cx| this.on_event(event, cx));
                        if finished || updated.is_err() {
                            break;
                        }
                    }
                }));
            }
            Err(error) => {
                view.report = Some(Report {
                    lines: vec![Line::Fail(error.to_string())],
                    success: false,
                });
            }
        }
        view
    }

    pub fn is_finished(&self) -> bool {
        self.report.is_some()
    }

    /// Hands new lines of the log to the log box. The box follows the
    /// newest line while it is scrolled to the end; scrolled back to read,
    /// it stays where it is.
    fn add_to_log(&mut self, lines: String, cx: &mut Context<Self>) {
        self.log_view.update(cx, |view, cx| view.push(&lines, cx));
    }

    fn on_event(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::Progress(progress) => {
                // The log is re-read about once per second, not on every event.
                let second_changed = self
                    .progress
                    .as_ref()
                    .is_none_or(|old| old.elapsed.as_secs() != progress.elapsed.as_secs());
                if second_changed {
                    let lines = self.reader.read_new();
                    self.add_to_log(lines, cx);
                }
                self.progress = Some(*progress);
            }
            Event::Finished(outcome) => {
                // After a failed run the report shows what the tool printed,
                // so the log box leaves that part out, unless the report
                // did not get all of it.
                let failed = outcome.exit_code() != 0;
                let quoted_in_full = failed && outcome.output_lines <= outcome.output.len();
                if failed && !outcome.output.is_empty() {
                    let mut text = outcome.output.join("\n");
                    let more = outcome.output_lines - outcome.output.len();
                    if more > 0 {
                        text.push_str(&format!("\n… {more} more lines in the log"));
                    }
                    self.tool_output =
                        Some(cx.new(|_| LogView::quote(&text, "Copy the tool output").rows(20)));
                }
                let lines = self.reader.finish(!quoted_in_full);
                self.add_to_log(lines, cx);
                self.log_view.update(cx, |view, cx| view.set_finished(cx));
                // The same report the command line tool prints.
                if let Some(prepared) = self.prepared.take() {
                    let report = prepared.report(&outcome);
                    crate::state::notify(&prepared.notice(&report), cx);
                    self.report = Some(report);
                }
                self.cancel = None;
            }
        }
        // Tells GPUI this view changed and must be drawn again.
        cx.notify();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(cancel) = &self.cancel {
            cancel.cancel();
            self.cancelling = true;
            cx.notify();
        }
    }
}

impl Render for RunView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let running = self.cancel.is_some();
        let status = self.progress.as_ref().map(|progress| {
            format!(
                "{} {}{}",
                self.label,
                report::duration(progress.elapsed),
                report::progress_detail(progress)
            )
        });
        let state = self.progress.as_ref().map(|progress| &progress.state);
        // One time of day for the whole frame, so the bars and the line
        // above them agree.
        let now = Some(tool_event::now());
        let percent = state
            .and_then(|state| state.share(now))
            .map(|share| share.fraction * 100.);
        let stepper = state
            .filter(|state| state.json)
            .map(|state| stage_stepper(&self.stages, state.stage, cx));
        let tables = state
            .filter(|state| !state.table_rows.is_empty())
            .map(|state| table_list(state, now, cx));
        let problems = state
            .filter(|state| !state.problems.is_empty())
            .map(|state| problem_list(state, cx));

        v_flex()
            .gap_5()
            .min_w_0()
            .child(widgets::card(cx).child(widgets::lines(&self.summary, cx)))
            .when(running, |this| {
                this.child(
                    h_flex()
                        .flex_wrap()
                        .items_center()
                        .justify_between()
                        .gap_3()
                        .child(
                            h_flex()
                                .gap_2()
                                .min_w_0()
                                .child(Spinner::new().small())
                                .child(
                                    div()
                                        .min_w_0()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(status.unwrap_or_else(|| self.label.to_string())),
                                ),
                        )
                        .child(
                            Button::new("cancel-run")
                                .danger()
                                .label(if self.cancelling {
                                    "Stopping…"
                                } else {
                                    "Cancel"
                                })
                                .disabled(self.cancelling)
                                // `cx.listener` turns a method on this view
                                // into a click handler.
                                .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                        ),
                )
                .children(stepper)
                .when_some(percent, |this, percent| {
                    this.child(
                        gpui_kit::component::progress::Progress::new("run-progress")
                            .value(percent)
                            .accessibility_label("Progress"),
                    )
                })
                .children(tables)
                .children(problems)
            })
            .when(!self.log_view.read(cx).is_empty(), |this| {
                let open = self.log_open;
                this.child(
                    v_flex()
                        .gap_2()
                        .min_w_0()
                        .child(
                            h_flex().child(
                                Button::new("toggle-log")
                                    .small()
                                    .ghost()
                                    .icon(if open {
                                        IconName::ChevronDown
                                    } else {
                                        IconName::ChevronRight
                                    })
                                    .label(if open { "Hide log" } else { "Show log" })
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.log_open = !this.log_open;
                                        // Opened, the box starts at the newest line.
                                        this.log_view.update(cx, |view, cx| view.go_to_end(cx));
                                        cx.notify();
                                    })),
                            ),
                        )
                        .when(open, |this| {
                            this.child(
                                widgets::hint(
                                    format!(
                                        "Log: {}",
                                        widgets::tilde_path(&self.log, core(cx).home.as_deref())
                                    ),
                                    cx,
                                )
                                .font_family(cx.theme().mono_font_family.clone()),
                            )
                            .child(self.log_view.clone())
                        }),
                )
            })
            .when_some(self.report.as_ref(), |this, report| {
                let color = if report.success {
                    cx.theme().success
                } else {
                    cx.theme().danger
                };
                // The command line tool quotes the first lines the tool
                // printed above its verdict. Here the verdict comes first,
                // and all the tool printed is in a box under it.
                let lines: Vec<Line> = report
                    .lines
                    .iter()
                    .filter(|line| self.tool_output.is_none() || !matches!(line, Line::Quote(_)))
                    .cloned()
                    .collect();
                this.child(
                    // The verdict is said by the edge of the card and by the
                    // mark on its first line, not by a tint.
                    widgets::card(cx)
                        .border_color(color)
                        .child(widgets::lines(&lines, cx))
                        .when_some(self.tool_output.clone(), |card, output| {
                            card.child(
                                v_flex()
                                    .min_w_0()
                                    .child(widgets::subheading("Tool output", cx))
                                    .child(output),
                            )
                        }),
                )
            })
    }
}

/// The stages of the run in a row: those done dimmed with a check, the
/// current one in bold, those to come dimmed.
fn stage_stepper(stages: &[Stage], current: Option<Stage>, cx: &App) -> Div {
    let theme = cx.theme();
    let mut row = h_flex().flex_wrap().gap_x_3().gap_y_1().text_sm();
    for (i, stage) in stages.iter().enumerate() {
        if i > 0 {
            row = row.child(div().text_color(theme.muted_foreground).child("›"));
        }
        let item = div().child(stage.name());
        row = row.child(match current {
            Some(now) if *stage == now => item
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.foreground),
            Some(now) if *stage < now => div()
                .text_color(theme.muted_foreground)
                .child(format!("✓ {}", stage.name())),
            _ => item.text_color(theme.muted_foreground),
        });
    }
    row
}

/// Rows a table list shows before it scrolls in its box.
const TABLE_ROWS_IN_VIEW: usize = 7;

/// Every table of the run in name order, so a row never moves: a mark, the
/// name, a bar and how far it is. The heading names the tables in work, so
/// they can be found in a long list.
pub(crate) fn table_list(state: &RunState, now: Option<DateTime>, cx: &App) -> Div {
    let theme = cx.theme();
    let mut rows = v_flex();
    for (index, row) in state.table_rows.iter().enumerate() {
        let detail = row.detail(state, now);
        let mismatch = row.checksum_ok == Some(false);
        let active = matches!(
            row.status,
            TableStatus::Creating | TableStatus::Rows | TableStatus::Indexing
        );
        let fill = if mismatch {
            theme.warning
        } else if row.status == TableStatus::Done {
            theme.success
        } else {
            theme.foreground
        };
        let mark = div()
            .w(px(16.))
            .flex_shrink_0()
            .flex()
            .justify_center()
            .map(|mark| match row.status {
                _ if mismatch => mark.child(
                    Icon::new(IconName::TriangleAlert)
                        .xsmall()
                        .text_color(theme.warning),
                ),
                TableStatus::Done => mark.child(
                    Icon::new(IconName::Check)
                        .xsmall()
                        .text_color(theme.success),
                ),
                _ if active => mark.child(Spinner::new().xsmall()),
                _ => mark,
            });
        rows = rows.child(
            h_flex()
                .id(("run-table", index))
                .role(Role::ListItem)
                .aria_label(format!("{}, {detail}", row.name))
                .gap_3()
                .px_2()
                .py_1()
                .min_w_0()
                .text_sm()
                .child(mark)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .font_family(theme.mono_font_family.clone())
                        .when(row.status == TableStatus::Waiting, |name| {
                            name.text_color(theme.muted_foreground)
                        })
                        .when(active, |name| name.font_weight(FontWeight::SEMIBOLD))
                        .child(row.name.clone()),
                )
                .child(
                    // A square bar, like the size bars: a track in Rule
                    // filled in ink while in work, in Success when done.
                    div()
                        .w(px(96.))
                        .h(px(4.))
                        .flex_shrink_0()
                        .bg(theme.border)
                        .child(
                            div()
                                .h_full()
                                .w(relative(row.share(state, now).fraction))
                                .bg(fill),
                        ),
                )
                .child(
                    div()
                        .w(px(104.))
                        .flex_shrink_0()
                        .text_right()
                        .text_xs()
                        .text_color(if mismatch {
                            theme.warning
                        } else {
                            theme.muted_foreground
                        })
                        .child(detail),
                )
                // Seen by the layout tests; nothing outside them.
                .test_support(),
        );
    }
    widgets::section("Tables", cx)
        .child(widgets::hint(report::tables_heading(state), cx))
        .child(
            div()
                .id("run-tables")
                .max_h(px(220.))
                .overflow_y_scroll()
                // Only a list long enough to scroll keeps the scroll from
                // the page behind it.
                .when(state.table_rows.len() > TABLE_ROWS_IN_VIEW, |this| {
                    this.occlude()
                })
                .p_1()
                .rounded(theme::RADIUS)
                .border_1()
                .border_color(theme.border)
                .child(rows),
        )
}

/// The problems logged so far, while the run goes on. At most a few; the
/// report lists them all.
const SHOWN_PROBLEMS: usize = 5;

fn problem_list(state: &RunState, cx: &App) -> Div {
    let mut list = v_flex().gap_2().min_w_0();
    for problem in state.problems.iter().take(SHOWN_PROBLEMS) {
        // By level: myloader marks some warnings fatal and goes on.
        let tone = if problem.level >= Level::Error {
            widgets::Tone::Danger
        } else {
            widgets::Tone::Warning
        };
        let text = match problem.count {
            1 => problem.text.clone(),
            n => format!("{} (×{n})", problem.text),
        };
        list = list.child(widgets::notice(tone, &text, cx));
    }
    let more = state.problems.len().saturating_sub(SHOWN_PROBLEMS);
    if more > 0 {
        list = list.child(widgets::hint(format!("{more} more in the log"), cx));
    }
    list
}
