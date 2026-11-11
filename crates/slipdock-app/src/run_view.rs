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

use gpui_kit::component::Disableable as _;
use gpui_kit::component::button::*;
use gpui_kit::component::{ActiveTheme, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use slipdock_core::dumps;
use slipdock_core::job::Prepared;
use slipdock_core::report::{self, Line, Report};
use slipdock_core::run::{self, Cancel, Event, Progress};

use crate::widgets;

/// How many lines of the log are shown while the tool runs.
const TAIL_LINES: usize = 14;

pub struct RunView {
    label: SharedString,
    summary: Vec<Line>,
    log: PathBuf,
    progress: Option<Progress>,
    tail: Vec<String>,
    /// Present while the tool runs.
    cancel: Option<Cancel>,
    cancelling: bool,
    /// Present when the run has ended.
    pub(crate) report: Option<Report>,
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
            tail: Vec::new(),
            cancel: None,
            cancelling: false,
            report: None,
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

    fn on_event(&mut self, event: Event, cx: &mut Context<Self>) {
        match event {
            Event::Progress(progress) => {
                // The log is re-read about once per second, not on every event.
                let second_changed = self
                    .progress
                    .as_ref()
                    .is_none_or(|old| old.elapsed.as_secs() != progress.elapsed.as_secs());
                if second_changed {
                    self.tail = run::log_tail(&self.log, TAIL_LINES);
                }
                self.progress = Some(progress);
            }
            Event::Finished(outcome) => {
                self.tail = run::log_tail(&self.log, TAIL_LINES);
                // The same report the command line tool prints.
                self.report = self
                    .prepared
                    .take()
                    .map(|prepared| prepared.report(&outcome));
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
            let mut text = format!("{} {}", self.label, report::duration(progress.elapsed));
            if let Some((files, size)) = progress.written {
                text.push_str(&format!(" · {files} files · {}", dumps::human_size(size)));
            }
            text
        });

        v_flex()
            .gap_4()
            .child(widgets::lines(&self.summary, cx))
            .when(running, |this| {
                this.child(
                    h_flex()
                        .gap_4()
                        .items_center()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(status.unwrap_or_else(|| self.label.to_string())),
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
            })
            .when(!self.tail.is_empty(), |this| {
                this.child(
                    v_flex()
                        .gap_1()
                        .child(widgets::hint(format!("Log: {}", self.log.display()), cx))
                        .child(widgets::mono(&self.tail.join("\n"), cx)),
                )
            })
            .when_some(self.report.as_ref(), |this, report| {
                this.child(
                    widgets::card(cx)
                        .border_color(if report.success {
                            cx.theme().success
                        } else {
                            cx.theme().danger
                        })
                        .child(widgets::lines(&report.lines, cx)),
                )
            })
    }
}
