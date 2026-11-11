//! Load: profile -> dump -> target database -> confirmation -> run.
//!
//! A load drops and recreates tables. So the last step is a dialog that
//! shows where the data is going, and "Start load" stays off until the name
//! of the target database has been typed. The check itself is the core's
//! (`LoadPlan::confirms`), the same one the command line tool uses.

use std::path::PathBuf;

use gpui_kit::component::Disableable as _;
use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme, WindowExt, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use slipdock_core::dumps::{self, DumpInfo};
use slipdock_core::job::{self, LoadOptions, LoadPlan};
use slipdock_core::report::Line;
use slipdock_core::{Error, profile};

use crate::run_view::RunView;
use crate::state::core;
use crate::widgets::{self, Tone};

pub struct LoadView {
    profiles: Vec<String>,
    profile: Option<String>,
    /// The dumps in the profile's dump folder, newest first.
    pub(crate) dumps: Vec<DumpInfo>,
    dump: Option<PathBuf>,
    target: Entity<InputState>,
    level: Option<u8>,
    pub(crate) preview: Option<Result<(Vec<Line>, String), String>>,
    busy: bool,
    /// The typed database name in the confirmation dialog.
    pub(crate) confirm: Entity<InputState>,
    /// The checked plan that waits for the confirmation.
    pub(crate) pending: Option<LoadPlan>,
    pub(crate) run: Option<Entity<RunView>>,
    /// A subscription ends when it is dropped, so the view keeps it.
    _subscription: Subscription,
}

impl LoadView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let confirm = cx.new(|cx| InputState::new(window, cx));
        // GPUI concept: entities can emit events, and others subscribe.
        // The dialog is drawn by the window, not by this view, so when the
        // typed name changes we ask the whole window to draw again; that
        // re-runs the dialog's builder, which enables or disables "Start".
        let subscription =
            cx.subscribe_in(&confirm, window, |_, _, event: &InputEvent, window, _| {
                if matches!(event, InputEvent::Change) {
                    window.refresh();
                }
            });
        let mut view = LoadView {
            profiles: Vec::new(),
            profile: None,
            dumps: Vec::new(),
            dump: None,
            target: cx.new(|cx| InputState::new(window, cx).placeholder("target database")),
            level: None,
            preview: None,
            busy: false,
            confirm,
            pending: None,
            run: None,
            _subscription: subscription,
        };
        view.reload(cx);
        view
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.profiles = profile::names(&core(cx).config_dir);
        if let Some(name) = &self.profile
            && !self.profiles.contains(name)
        {
            self.profile = None;
            self.dumps.clear();
            self.dump = None;
        }
        self.list_dumps(cx);
        cx.notify();
    }

    pub(crate) fn pick_profile(&mut self, name: String, cx: &mut Context<Self>) {
        self.profile = Some(name);
        self.dump = None;
        self.dumps.clear();
        self.preview = None;
        self.list_dumps(cx);
        cx.notify();
    }

    fn list_dumps(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.profile.clone() else {
            return;
        };
        let ctx = core(cx);
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let root = ctx.profile(&name)?.need_dump_dir(ctx.home.as_deref())?;
                    Ok::<_, Error>(dumps::list(&root))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(dumps) => this.dumps = dumps,
                    Err(error) => this.preview = Some(Err(error.to_string())),
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(crate) fn pick_dump(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dump) = self.dumps.get(index) else {
            return;
        };
        self.dump = Some(dump.path.clone());
        // Default target: the database the dump was taken from.
        let source = dump.source_db.clone().unwrap_or_default();
        self.target
            .update(cx, |input, cx| input.set_value(source, window, cx));
        self.preview = None;
        cx.notify();
    }

    fn options(&self, dry_run: bool, cx: &App) -> LoadOptions {
        let target = self.target.read(cx).value().trim().to_string();
        LoadOptions {
            profile: self.profile.clone().unwrap_or_default(),
            dir: self.dump.clone().unwrap_or_default(),
            target: (!target.is_empty()).then_some(target),
            log_level: self.level,
            extra_args: Vec::new(),
            dry_run,
        }
    }

    /// Dry run: plan, prepare and show the command. Nothing is asked,
    /// because nothing is changed.
    pub(crate) fn dry_run(&mut self, cx: &mut Context<Self>) {
        let ctx = core(cx);
        let options = self.options(true, cx);
        self.busy = true;
        self.preview = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let plan = job::plan_load(&ctx, options)?;
                    let summary = plan.summary.clone();
                    let prepared = plan.prepare(&ctx, &job::timestamp())?;
                    Ok::<_, Error>((summary, prepared.invocation.preview()))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                this.preview = Some(result.map_err(|error| error.to_string()));
                cx.notify();
            });
        })
        .detach();
    }

    /// Real load, step 1: check everything, then ask.
    pub(crate) fn request_load(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ctx = core(cx);
        let options = self.options(false, cx);
        self.busy = true;
        self.preview = None;
        cx.notify();
        // `spawn_in` is `spawn` with the window at hand, needed to open the dialog.
        cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { job::plan_load(&ctx, options) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.busy = false;
                match result {
                    Ok(plan) => this.open_confirmation(plan, window, cx),
                    Err(error) => this.preview = Some(Err(error.to_string())),
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Real load, step 2: the dialog.
    fn open_confirmation(&mut self, plan: LoadPlan, window: &mut Window, cx: &mut Context<Self>) {
        let profile = self.profile.clone().unwrap_or_default();
        let server = plan.server.clone();
        let target = plan.target.clone();
        let dump = self
            .dump
            .as_ref()
            .and_then(|path| path.file_name())
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.pending = Some(plan);
        self.confirm
            .update(cx, |input, cx| input.set_value("", window, cx));

        let view = cx.entity();
        let input = self.confirm.clone();
        // The builder may run many times while the dialog is open (also more
        // than once per frame), so it only describes the dialog from the
        // current state and changes nothing, not even the focus.
        window.open_dialog(cx, move |dialog, _, cx| {
            let typed = input.read(cx).value();
            let matches = typed.as_ref() == target.as_str();
            let cancel_view = view.clone();
            let start_view = view.clone();
            dialog
                .title("Load into this database?")
                .overlay_closable(false)
                .child(
                    v_flex()
                        .gap_3()
                        .child(
                            v_flex()
                                .p_3()
                                .min_w_0()
                                .rounded_md()
                                .bg(cx.theme().muted)
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(cx.theme().muted_foreground)
                                        .child("Profile and server"),
                                )
                                .child(
                                    div()
                                        .text_lg()
                                        .font_weight(FontWeight::BOLD)
                                        .child(format!("{profile}   {server}")),
                                ),
                        )
                        .child(widgets::hint(format!("Dump: {dump}"), cx))
                        .child(widgets::notice(
                            Tone::Danger,
                            "Tables in the dump are dropped and recreated in the target database.",
                            cx,
                        ))
                        .child(format!("Click the field and type {target} to continue:"))
                        .child(Input::new(&input)),
                )
                .footer(
                    h_flex()
                        .gap_2()
                        .justify_end()
                        .child(
                            Button::new("load-cancel")
                                .outline()
                                .label("Cancel")
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    cancel_view.update(cx, |this, _| this.pending = None);
                                }),
                        )
                        .child(
                            Button::new("load-start")
                                .danger()
                                .label("Start load")
                                .disabled(!matches)
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    start_view.update(cx, |this, cx| this.start_confirmed(cx));
                                }),
                        ),
                )
        });
    }

    /// Real load, step 3: the name was typed; prepare and run.
    pub(crate) fn start_confirmed(&mut self, cx: &mut Context<Self>) {
        let Some(plan) = self.pending.take() else {
            return;
        };
        // Checked again here, so the rule does not depend on a button state.
        if !plan.confirms(self.confirm.read(cx).value().as_ref()) {
            self.preview = Some(Err(
                "The typed name does not match the target database.".into()
            ));
            cx.notify();
            return;
        }
        let ctx = core(cx);
        let summary = plan.summary.clone();
        self.busy = true;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { plan.prepare(&ctx, &job::timestamp()) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(prepared) => {
                        this.run = Some(cx.new(|cx| RunView::start(prepared, summary, cx)));
                    }
                    Err(error) => this.preview = Some(Err(error.to_string())),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn render_form(&mut self, cx: &mut Context<Self>) -> Div {
        let mut page = widgets::page().child(widgets::header(
            "Load",
            "Put a dump into a database with myloader. Tables in the dump are dropped and recreated.",
            cx,
        ));
        if self.profiles.is_empty() {
            return page.child(widgets::notice(
                Tone::Info,
                "No profiles yet. Create one on the Profiles screen.",
                cx,
            ));
        }

        let mut profiles = widgets::wrap_row();
        for (index, name) in self.profiles.iter().enumerate() {
            let picked = name.clone();
            let chosen = self.profile.as_ref() == Some(name);
            profiles = profiles.child(
                widgets::choice(("load-profile", index), name.clone(), chosen).on_click(
                    cx.listener(move |this, _, _, cx| this.pick_profile(picked.clone(), cx)),
                ),
            );
        }
        page = page.child(widgets::section("Destination", cx).child(widgets::field(
            "Load into",
            profiles,
            cx,
        )));
        if self.profile.is_none() {
            return page;
        }

        // Dumps, newest first. A partial dump cannot be loaded.
        let mut list = widgets::section("Dump", cx);
        if self.dumps.is_empty() {
            list = list.child(widgets::hint("No dumps in this profile's dump folder.", cx));
        }
        for (index, dump) in self.dumps.iter().enumerate() {
            let chosen = self.dump.as_ref() == Some(&dump.path);
            let note = if dump.complete {
                format!("{} tables · {}", dump.tables, dumps::human_size(dump.size))
            } else {
                "cannot be loaded".to_string()
            };
            list = list.child(
                widgets::option_row(
                    ("load-dump", index),
                    dump.name.clone(),
                    chosen,
                    !dump.complete,
                    cx,
                )
                .child(widgets::option_mark(chosen, cx))
                .child(
                    v_flex()
                        .flex_1()
                        .min_w_0()
                        .child(div().child(dump.name.clone()))
                        .child(widgets::hint(note, cx)),
                )
                .when(!dump.complete, |this| {
                    this.child(widgets::tag("partial", Tone::Warning))
                })
                .when(dump.complete, |this| {
                    this.on_click(
                        cx.listener(move |this, _, window, cx| this.pick_dump(index, window, cx)),
                    )
                }),
            );
        }
        page = page.child(list);
        if self.dump.is_none() {
            return page.when_some(self.preview.clone().and_then(|p| p.err()), |this, error| {
                this.child(widgets::error_text(error, cx))
            });
        }

        let mut levels = widgets::wrap_row();
        let choices: [(Option<u8>, &str); 5] = [
            (None, "From profile"),
            (Some(0), "0 silent"),
            (Some(1), "1 errors"),
            (Some(2), "2 warnings"),
            (Some(3), "3 info"),
        ];
        for (index, (level, label)) in choices.into_iter().enumerate() {
            levels = levels.child(
                widgets::choice(("load-level", index), label, self.level == level).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.level = level;
                        cx.notify();
                    }),
                ),
            );
        }
        page = page
            .child(
                widgets::section("Target", cx)
                    .child(widgets::field("Database", Input::new(&self.target), cx))
                    .child(widgets::field("Log level", levels, cx)),
            )
            .child(
                widgets::wrap_row()
                    .child(
                        Button::new("load-run")
                            .danger()
                            .label("Load…")
                            .disabled(self.busy)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.request_load(window, cx)),
                            ),
                    )
                    .child(
                        Button::new("load-dry")
                            .outline()
                            .label("Dry run (show command)")
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.dry_run(cx))),
                    ),
            );
        match &self.preview {
            Some(Ok((summary, command))) => {
                page = page.child(widgets::preview(summary, command, cx));
            }
            Some(Err(error)) => page = page.child(widgets::error_text(error.clone(), cx)),
            None => {}
        }
        page
    }
}

impl Render for LoadView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(run) = self.run.clone() {
            let finished = run.read(cx).is_finished();
            return widgets::page()
                .child(widgets::header(
                    "Load",
                    "Put a dump into a database with myloader.",
                    cx,
                ))
                .child(run)
                .when(finished, |this| {
                    this.child(
                        h_flex().child(
                            Button::new("load-back")
                                .outline()
                                .label("Back to the form")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.run = None;
                                    cx.notify();
                                })),
                        ),
                    )
                });
        }
        self.render_form(cx)
    }
}
