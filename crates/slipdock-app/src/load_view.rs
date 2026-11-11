//! Load: profile to load into -> dump (of that profile or of another one)
//! -> target database -> confirmation -> run.
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
use slipdock_core::{Error, profile, server};

use crate::run_view::RunView;
use crate::state::{core, log_options};
use crate::widgets::{self, Tone};

pub struct LoadView {
    profiles: Vec<String>,
    /// The profile whose server is loaded into.
    profile: Option<String>,
    /// The profile whose dump folder is listed, when it is not `profile`:
    /// a dump taken from one server can be loaded into another.
    source: Option<String>,
    /// The dumps in the dump folder of `dumps_from`, newest first.
    pub(crate) dumps: Vec<DumpInfo>,
    /// The dump folder is being read.
    listing: bool,
    dump: Option<PathBuf>,
    pub(crate) target: Entity<InputState>,
    /// The databases on the server of `profile`, to pick the target from.
    pub(crate) databases: Vec<String>,
    /// The server is being asked for its databases.
    listing_databases: bool,
    /// Why the server of `profile` did not answer. While this is set the
    /// rest of the form is not shown.
    pub(crate) connect_error: Option<String>,
    pub(crate) preview: Option<Result<(Vec<Line>, String), String>>,
    /// A command is being prepared. Nothing is drawn for it: preparing takes
    /// a moment too short to read a loader in, so one would only flash. It
    /// keeps a second click from starting the same thing twice.
    busy: bool,
    /// The typed database name in the confirmation dialog.
    pub(crate) confirm: Entity<InputState>,
    /// The checked plan that waits for the confirmation.
    pub(crate) pending: Option<LoadPlan>,
    pub(crate) run: Option<Entity<RunView>>,
    /// A subscription ends when it is dropped, so the view keeps it.
    _subscription: Subscription,
    _target_subscription: Subscription,
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
        let target = cx.new(|cx| InputState::new(window, cx).placeholder("target database"));
        // The database button that matches the typed name is filled, so the
        // form is drawn again when the name changes.
        let target_subscription = cx.subscribe(&target, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        let mut view = LoadView {
            profiles: Vec::new(),
            profile: None,
            source: None,
            databases: Vec::new(),
            listing_databases: false,
            connect_error: None,
            dumps: Vec::new(),
            listing: false,
            dump: None,
            target,
            preview: None,
            busy: false,
            confirm,
            pending: None,
            run: None,
            _subscription: subscription,
            _target_subscription: target_subscription,
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
            self.databases.clear();
            self.connect_error = None;
            self.dumps.clear();
            self.dump = None;
        }
        if let Some(name) = &self.source
            && !self.profiles.contains(name)
        {
            self.source = None;
            self.dumps.clear();
            self.dump = None;
        }
        self.list_dumps(cx);
        cx.notify();
    }

    /// The profile whose dump folder is listed.
    pub(crate) fn dumps_from(&self) -> Option<&String> {
        self.source.as_ref().or(self.profile.as_ref())
    }

    /// Chooses the profile to load into. The dumps follow it, unless
    /// another profile was chosen for them: then they stay as they are.
    pub(crate) fn pick_profile(&mut self, name: String, cx: &mut Context<Self>) {
        let from_before = self.dumps_from().cloned();
        // Loading into the profile the dumps are of is the plain case.
        if self.source.as_ref() == Some(&name) {
            self.source = None;
        }
        if self.profile.as_ref() != Some(&name) {
            self.profile = Some(name);
            self.list_databases(cx);
        }
        self.preview = None;
        if self.dumps_from().cloned() != from_before {
            self.dump = None;
            self.dumps.clear();
            self.list_dumps(cx);
        }
        cx.notify();
    }

    /// Chooses the profile whose dumps are listed.
    pub(crate) fn pick_source(&mut self, name: String, cx: &mut Context<Self>) {
        if self.dumps_from() == Some(&name) {
            return;
        }
        // The profile that is loaded into needs no separate choice.
        self.source = (self.profile.as_ref() != Some(&name)).then_some(name);
        self.dump = None;
        self.dumps.clear();
        self.preview = None;
        self.list_dumps(cx);
        cx.notify();
    }

    /// Asks the server that is loaded into for its databases, in the
    /// background. Without an answer the name is typed.
    pub(crate) fn list_databases(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.profile.clone() else {
            return;
        };
        let ctx = core(cx);
        self.databases.clear();
        self.connect_error = None;
        self.listing_databases = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn({
                    let name = name.clone();
                    async move { server::databases(&ctx, &ctx.profile(&name)?) }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                // An answer for a profile that is no longer chosen is dropped.
                if this.profile.as_ref() != Some(&name) {
                    return;
                }
                this.listing_databases = false;
                match result {
                    Ok(databases) => this.databases = databases,
                    Err(error) => this.connect_error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn list_dumps(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.dumps_from().cloned() else {
            return;
        };
        let ctx = core(cx);
        self.listing = true;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let root = ctx.profile(&name)?.need_dump_dir(ctx.home.as_deref())?;
                    Ok::<_, Error>(dumps::list(&root))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.listing = false;
                match result {
                    Ok(dumps) => {
                        // The chosen dump may have been deleted meanwhile.
                        if let Some(chosen) = &this.dump
                            && !dumps.iter().any(|dump| &dump.path == chosen)
                        {
                            this.dump = None;
                            this.preview = None;
                        }
                        this.dumps = dumps;
                    }
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
        let (log_level, extra_args) = log_options(dry_run);
        LoadOptions {
            profile: self.profile.clone().unwrap_or_default(),
            dir: self.dump.clone().unwrap_or_default(),
            target: (!target.is_empty()).then_some(target),
            log_level,
            extra_args,
            dry_run,
        }
    }

    /// Dry run: plan, prepare and show the command. Nothing is asked,
    /// because nothing is changed.
    pub(crate) fn dry_run(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let ctx = core(cx);
        let options = self.options(true, cx);
        // The command that is on show stays until the new one takes its
        // place, so the page does not jump.
        self.busy = true;
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
        if self.busy {
            return;
        }
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
                        // Room for the ring of the focused field, which the
                        // dialog would cut off at its edge.
                        .p_1()
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
        let Some(into) = self.profile.clone() else {
            return page;
        };
        // The rest of the form waits for the server's answer, and is not
        // shown when there is none: a load into a server that cannot be
        // reached would fail anyway.
        if self.listing_databases {
            return page.child(
                widgets::card(cx).child(widgets::loading(format!("Connecting with {into}…"), cx)),
            );
        }
        if let Some(error) = self.connect_error.clone() {
            return page.child(widgets::connection_failed(
                &into,
                &error,
                Button::new("load-retry")
                    .on_click(cx.listener(|this, _, _, cx| this.list_databases(cx))),
                cx,
            ));
        }

        // Dumps, newest first. A partial dump cannot be loaded.
        let mut list = widgets::section("Dump", cx);
        // With one profile there is nothing to choose from.
        if self.profiles.len() > 1 {
            let mut sources = widgets::wrap_row();
            for (index, name) in self.profiles.iter().enumerate() {
                let picked = name.clone();
                let chosen = self.dumps_from() == Some(name);
                sources = sources.child(
                    widgets::choice(("load-source", index), name.clone(), chosen).on_click(
                        cx.listener(move |this, _, _, cx| this.pick_source(picked.clone(), cx)),
                    ),
                );
            }
            let crossing = self.source.as_ref().zip(self.profile.as_ref());
            list = list.child(widgets::field(
                "Dumps of",
                v_flex()
                    .gap_2()
                    .child(sources)
                    .when_some(crossing, |this, (from, into)| {
                        this.child(widgets::notice(
                            Tone::Warning,
                            &format!("A dump of {from} will be loaded into {into}."),
                            cx,
                        ))
                    }),
                cx,
            ));
        }
        if self.listing {
            list = list.child(widgets::loading("Reading the dump folder…", cx));
        } else if self.dumps.is_empty() {
            list = list.child(widgets::hint("No dumps in this profile's dump folder.", cx));
        }
        for (index, dump) in self.dumps.iter().enumerate() {
            let chosen = self.dump.as_ref() == Some(&dump.path);
            list = list.child(
                // The same row as on the Dumps screen, without its actions.
                widgets::option_row(
                    ("load-dump", index),
                    dump.name.clone(),
                    chosen,
                    !dump.complete,
                    cx,
                )
                .flex_wrap()
                .gap_y_1()
                .child(
                    h_flex()
                        .flex_grow(1.)
                        .flex_basis(px(300.))
                        .min_w_0()
                        .gap_3()
                        .child(widgets::option_mark(chosen, cx))
                        .child(widgets::dump_summary(dump, cx).flex_1()),
                )
                .child(if dump.complete {
                    widgets::dump_stats(dump, cx)
                } else {
                    widgets::hint("cannot be loaded", cx)
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

        // The databases of the server that is loaded into. A name that is
        // not among them can still be typed.
        let typed = self.target.read(cx).value().to_string();
        let databases = widgets::database_list(
            "load-databases",
            &self.databases,
            &typed,
            |name| {
                Box::new(cx.listener(move |this, _, window, cx| {
                    this.target
                        .update(cx, |input, cx| input.set_value(name.clone(), window, cx));
                    this.preview = None;
                    cx.notify();
                }))
            },
            cx,
        );
        let database = v_flex()
            .gap_2()
            .child(Input::new(&self.target).cleanable(true))
            .children(databases);
        page = page
            .child(
                widgets::section("Target", cx)
                    .child(widgets::field("Database", database, cx))
                    .child(widgets::hint(
                        "A full log of the load is written beside the dump. The Dumps \
                         screen shows it.",
                        cx,
                    )),
            )
            .child(
                widgets::wrap_row()
                    .child(
                        Button::new("load-run").danger().label("Load…").on_click(
                            cx.listener(|this, _, window, cx| this.request_load(window, cx)),
                        ),
                    )
                    .child(
                        Button::new("load-dry")
                            .outline()
                            .label("Show command")
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
