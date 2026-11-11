//! Dump: profile -> database -> tables to leave out -> folder and log level
//! -> command preview -> run.
//!
//! Every step works without a connection: lists come from the server when
//! the mysql client can reach it, and the names can always be typed.

use gpui_kit::component::Disableable as _;
use gpui_kit::component::button::*;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{Selectable, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use slipdock_core::job::{self, DumpOptions};
use slipdock_core::report::Line;
use slipdock_core::server::{self, Table};
use slipdock_core::{Error, platform, profile};

use crate::run_view::RunView;
use crate::state::core;
use crate::widgets;

pub struct DumpView {
    profiles: Vec<String>,
    profile: Option<String>,
    pub(crate) database: Entity<InputState>,
    /// Names typed by hand, in addition to the ticked tables.
    exclude: Entity<InputState>,
    out: Entity<InputState>,
    databases: Vec<String>,
    /// Tables and views of the chosen database; `true` = leave out.
    tables: Vec<(Table, bool)>,
    /// What the last attempt to list databases or tables said.
    list_note: Option<String>,
    /// `None` = the profile's own log level.
    level: Option<u8>,
    /// The result of "Show command": summary and command, or an error.
    pub(crate) preview: Option<Result<(Vec<Line>, String), String>>,
    busy: bool,
    /// Present while a dump runs and until its report is closed.
    pub(crate) run: Option<Entity<RunView>>,
}

impl DumpView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let mut view = DumpView {
            profiles: Vec::new(),
            profile: None,
            database: cx.new(|cx| InputState::new(window, cx).placeholder("database name")),
            exclude: cx.new(|cx| InputState::new(window, cx).placeholder("table1, table2")),
            out: cx.new(|cx| InputState::new(window, cx).placeholder("folder for the dump")),
            databases: Vec::new(),
            tables: Vec::new(),
            list_note: None,
            level: None,
            preview: None,
            busy: false,
            run: None,
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
        }
        cx.notify();
    }

    pub(crate) fn pick_profile(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let ctx = core(cx);
        // Show the profile's dump folder as the default; it can be changed
        // for this dump.
        let dump_dir = ctx
            .profile(&name)
            .ok()
            .and_then(|profile| profile.dump_dir(ctx.home.as_deref()))
            .map(|dir| dir.to_string_lossy().into_owned())
            .unwrap_or_default();
        self.out
            .update(cx, |input, cx| input.set_value(dump_dir, window, cx));
        self.profile = Some(name);
        self.databases.clear();
        self.tables.clear();
        self.list_note = None;
        self.preview = None;
        self.list_databases(cx);
    }

    /// Asks the server for its databases, in the background.
    fn list_databases(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.profile.clone() else {
            return;
        };
        let ctx = core(cx);
        self.list_note = Some("Asking the server for its databases…".into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { server::databases(&ctx, &ctx.profile(&name)?) })
                .await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(databases) => {
                        this.databases = databases;
                        this.list_note = None;
                    }
                    Err(error) => {
                        this.list_note = Some(format!("{error}. Type the database name instead."));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn pick_database(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.database
            .update(cx, |input, cx| input.set_value(name, window, cx));
        self.list_tables(cx);
    }

    fn list_tables(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.profile.clone() else {
            return;
        };
        let database = self.database.read(cx).value().trim().to_string();
        if database.is_empty() {
            return;
        }
        let ctx = core(cx);
        self.tables.clear();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { server::tables(&ctx, &ctx.profile(&name)?, &database) })
                .await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(tables) => {
                        this.tables = tables.into_iter().map(|table| (table, false)).collect();
                        this.list_note = None;
                    }
                    Err(error) => {
                        this.list_note =
                            Some(format!("{error}. Type the tables to leave out instead."));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The form as the options the core understands: the same ones the
    /// command line tool builds from its arguments.
    fn options(&self, dry_run: bool, cx: &App) -> DumpOptions {
        let mut exclude: Vec<String> = self
            .tables
            .iter()
            .filter(|(_, excluded)| *excluded)
            .map(|(table, _)| table.name.clone())
            .collect();
        exclude.push(self.exclude.read(cx).value().to_string());
        let out = self.out.read(cx).value().trim().to_string();
        let home = core(cx).home;
        DumpOptions {
            profile: self.profile.clone().unwrap_or_default(),
            database: self.database.read(cx).value().trim().to_string(),
            exclude,
            out: (!out.is_empty()).then(|| platform::expand_tilde(&out, home.as_deref())),
            log_level: self.level,
            extra_args: Vec::new(),
            dry_run,
        }
    }

    /// Plans and prepares the dump in the background. A dry run then shows
    /// the command; a real run hands over to a `RunView`.
    pub(crate) fn prepare(&mut self, dry_run: bool, cx: &mut Context<Self>) {
        let ctx = core(cx);
        let options = self.options(dry_run, cx);
        self.busy = true;
        self.preview = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let plan = job::plan_dump(&ctx, options, &job::timestamp())?;
                    let summary = plan.summary.clone();
                    let prepared = plan.prepare(&ctx)?;
                    Ok::<_, Error>((summary, prepared))
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok((summary, prepared)) if dry_run => {
                        this.preview = Some(Ok((summary, prepared.invocation.preview())));
                    }
                    Ok((summary, prepared)) => {
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
        let mut page = v_flex().gap_4().child(widgets::heading("Dump"));
        if self.profiles.is_empty() {
            return page.child(widgets::hint(
                "No profiles yet. Create one on the Profiles screen.",
                cx,
            ));
        }

        // 1. Profile
        let mut profiles = h_flex().gap_2().flex_wrap();
        for (index, name) in self.profiles.iter().enumerate() {
            let picked = name.clone();
            profiles = profiles.child(
                Button::new(("dump-profile", index))
                    .label(name.clone())
                    .selected(self.profile.as_ref() == Some(name))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.pick_profile(picked.clone(), window, cx)
                    })),
            );
        }
        page = page.child(widgets::field("Profile", profiles, cx));
        if self.profile.is_none() {
            return page;
        }

        // 2. Database
        let mut databases = h_flex().gap_2().flex_wrap();
        for (index, name) in self.databases.iter().enumerate() {
            let picked = name.clone();
            databases = databases.child(
                Button::new(("dump-db", index))
                    .small()
                    .label(name.clone())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.pick_database(picked.clone(), window, cx)
                    })),
            );
        }
        page = page
            .child(widgets::field(
                "Database",
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(Input::new(&self.database)))
                    .child(
                        Button::new("list-tables")
                            .label("List tables")
                            .on_click(cx.listener(|this, _, _, cx| this.list_tables(cx))),
                    ),
                cx,
            ))
            .when(!self.databases.is_empty(), |this| {
                this.child(widgets::field("", databases, cx))
            })
            .when_some(self.list_note.clone(), |this, note| {
                this.child(widgets::field("", widgets::hint(note, cx), cx))
            });

        // 3. Tables and views to leave out
        if !self.tables.is_empty() {
            let mut checklist = v_flex().gap_1();
            for (index, (table, excluded)) in self.tables.iter().enumerate() {
                let label = if table.is_view {
                    format!("{}  (view)", table.name)
                } else {
                    table.name.clone()
                };
                checklist = checklist.child(
                    Checkbox::new(("exclude", index))
                        .label(label)
                        .checked(*excluded)
                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            this.tables[index].1 = *checked;
                            cx.notify();
                        })),
                );
            }
            page = page.child(widgets::field("Leave out", checklist, cx));
        }
        page = page.child(widgets::field(
            if self.tables.is_empty() {
                "Leave out"
            } else {
                "Also leave out"
            },
            Input::new(&self.exclude),
            cx,
        ));

        // 4. Folder and log level
        let mut levels = h_flex().gap_2();
        let choices: [(Option<u8>, &str); 5] = [
            (None, "From profile"),
            (Some(0), "0 silent"),
            (Some(1), "1 errors"),
            (Some(2), "2 warnings"),
            (Some(3), "3 info"),
        ];
        for (index, (level, label)) in choices.into_iter().enumerate() {
            levels = levels.child(
                Button::new(("dump-level", index))
                    .small()
                    .label(label)
                    .selected(self.level == level)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.level = level;
                        cx.notify();
                    })),
            );
        }
        page = page
            .child(widgets::field("Folder", Input::new(&self.out), cx))
            .child(widgets::field("Log level", levels, cx));

        // 5. Preview and run
        page = page.child(
            h_flex()
                .gap_2()
                .child(
                    Button::new("dump-dry")
                        .label("Dry run (show command)")
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, _, cx| this.prepare(true, cx))),
                )
                .child(
                    Button::new("dump-run")
                        .primary()
                        .label("Run dump")
                        .disabled(self.busy)
                        .on_click(cx.listener(|this, _, _, cx| this.prepare(false, cx))),
                ),
        );
        match &self.preview {
            Some(Ok((summary, command))) => {
                page = page
                    .child(widgets::lines(summary, cx))
                    .child(widgets::mono(command, cx))
                    .child(widgets::hint("(dry run, nothing executed)", cx));
            }
            Some(Err(error)) => page = page.child(widgets::error_text(error.clone(), cx)),
            None => {}
        }
        page
    }
}

impl Render for DumpView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // While a run exists, it takes the place of the form.
        if let Some(run) = self.run.clone() {
            let finished = run.read(cx).is_finished();
            return v_flex()
                .gap_4()
                .child(widgets::heading("Dump"))
                .child(run)
                .when(finished, |this| {
                    this.child(
                        h_flex().child(Button::new("dump-back").label("Back").on_click(
                            cx.listener(|this, _, _, cx| {
                                this.run = None;
                                cx.notify();
                            }),
                        )),
                    )
                });
        }
        self.render_form(cx)
    }
}
