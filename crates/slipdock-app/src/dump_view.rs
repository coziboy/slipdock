//! Dump: profile -> database -> tables to dump -> folder
//! -> command preview -> run.
//!
//! The lists come from the server. When it does not answer, the screen says
//! so instead of showing the rest of the form.

use gpui_kit::component::Disableable as _;
use gpui_kit::component::button::*;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme, IconName, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use slipdock_core::job::{self, DumpOptions};
use slipdock_core::report::Line;
use slipdock_core::server::{self, Table};
use slipdock_core::tools::Tool;
use slipdock_core::{Error, platform, profile};

use crate::run_view::RunView;
use crate::state::{Core, core, log_options, name_the_profile, server_of};
use crate::theme;
use crate::widgets::{self, Tone};

/// The rows the table list shows at its full height (220 px) before it
/// starts to scroll.
const ROWS_WITHOUT_SCROLL: usize = 7;

pub struct DumpView {
    profiles: Vec<String>,
    profile: Option<String>,
    /// The server of the chosen profile, for the label that heads the form.
    server: Option<String>,
    pub(crate) database: Entity<InputState>,
    /// Names to leave out, typed by hand. Used only when the server could
    /// not list the tables.
    exclude: Entity<InputState>,
    /// Narrows the table list to the names that contain this text.
    pub(crate) filter: Entity<InputState>,
    pub(crate) out: Entity<InputState>,
    databases: Vec<String>,
    /// Tables and views of the chosen database; `true` = dumped.
    pub(crate) tables: Vec<(Table, bool)>,
    /// The database `tables` was listed for. The selection is remembered
    /// under this name, also when the name in the form is edited later.
    listed: Option<String>,
    /// The server could not list the tables: the names to leave out are
    /// typed instead.
    pub(crate) tables_failed: bool,
    /// Why the server of the profile did not answer. While this is set the
    /// rest of the form is not shown.
    pub(crate) connect_error: Option<String>,
    /// The server is being asked for its databases.
    listing_databases: bool,
    /// The server is being asked for the tables of the database.
    listing_tables: bool,
    /// What the last attempt to list databases or tables said.
    list_note: Option<String>,
    /// The result of "Show command": summary and command, or an error.
    pub(crate) preview: Option<Result<(Vec<Line>, String), String>>,
    /// A command is being prepared. Nothing is drawn for it: preparing takes
    /// a moment too short to read a loader in, so one would only flash. It
    /// keeps a second click from starting the same thing twice.
    busy: bool,
    /// Present while a dump runs and until its report is closed.
    pub(crate) run: Option<Entity<RunView>>,
    /// A subscription ends when it is dropped, so the view keeps it.
    _subscription: Subscription,
    _database_subscription: Subscription,
}

impl DumpView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter tables"));
        // The list is drawn by this view, so it draws again when the filter
        // text changes.
        let subscription = cx.subscribe(&filter, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        let database =
            cx.new(|cx| InputState::new(window, cx).placeholder("Type to search the databases"));
        // The list of databases follows what is typed.
        let database_subscription = cx.subscribe(&database, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        let mut view = DumpView {
            profiles: Vec::new(),
            profile: None,
            server: None,
            database,
            exclude: cx.new(|cx| InputState::new(window, cx).placeholder("table1, table2")),
            filter,
            out: cx.new(|cx| InputState::new(window, cx).placeholder("folder for the dump")),
            databases: Vec::new(),
            tables: Vec::new(),
            listed: None,
            tables_failed: false,
            connect_error: None,
            listing_databases: false,
            listing_tables: false,
            list_note: None,
            preview: None,
            busy: false,
            run: None,
            _subscription: subscription,
            _database_subscription: database_subscription,
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
            self.server = None;
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
        // Show the profile's dump folder as the default, written the way
        // the profile form writes it; it can be changed for this dump.
        let dump_dir = ctx
            .profile(&name)
            .ok()
            .and_then(|profile| profile.dump_dir(ctx.home.as_deref()))
            .map(|dir| widgets::tilde_path(&dir, ctx.home.as_deref()))
            .unwrap_or_default();
        self.out
            .update(cx, |input, cx| input.set_value(dump_dir, window, cx));
        self.server = server_of(&name, cx);
        self.profile = Some(name);
        self.databases.clear();
        self.tables.clear();
        self.listed = None;
        self.tables_failed = false;
        self.list_note = None;
        self.preview = None;
        self.list_databases(cx);
    }

    /// Asks the server for its databases, in the background.
    pub(crate) fn list_databases(&mut self, cx: &mut Context<Self>) {
        let Some(name) = self.profile.clone() else {
            return;
        };
        let ctx = core(cx);
        self.list_note = None;
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

    fn pick_database(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.database
            .update(cx, |input, cx| input.set_value(name, window, cx));
        self.list_tables(window, cx);
    }

    fn list_tables(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.profile.clone() else {
            return;
        };
        let database = self.database.read(cx).value().trim().to_string();
        if database.is_empty() {
            return;
        }
        let ctx = core(cx);
        self.tables.clear();
        self.listed = None;
        self.tables_failed = false;
        self.listing_tables = true;
        self.list_note = None;
        cx.notify();
        // A filter for the old list would hide tables of the new one.
        self.filter
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn({
                    let database = database.clone();
                    async move { server::tables(&ctx, &ctx.profile(&name)?, &database) }
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.listing_tables = false;
                match result {
                    Ok(tables) => {
                        this.show_tables(database, tables, cx);
                        this.list_note = None;
                    }
                    Err(error) => {
                        this.tables_failed = true;
                        this.list_note =
                            Some(format!("{error}. Type the tables to leave out below."));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Opens the system's folder dialog and puts the chosen folder in the
    /// form, for this dump only: the profile keeps its own dump folder.
    pub(crate) fn choose_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Dump into this folder".into()),
        });
        cx.spawn_in(window, async move |this, cx| {
            // Ok(Ok(Some(paths))) = the user picked something.
            if let Ok(Ok(Some(paths))) = chosen.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update_in(cx, |this, window, cx| {
                    let text = widgets::tilde_path(&path, core(cx).home.as_deref());
                    this.out
                        .update(cx, |input, cx| input.set_value(text, window, cx));
                });
            }
        })
        .detach();
    }

    /// Puts the tables of a database in the form. Every table is ticked,
    /// except the ones left out the last time with this profile and
    /// database; a table that is new since then is ticked.
    pub(crate) fn show_tables(&mut self, database: String, tables: Vec<Table>, cx: &App) {
        let profile = self.profile.clone().unwrap_or_default();
        let settings = core(cx).settings;
        let left_out = settings.left_out(&profile, &database);
        self.tables = tables
            .into_iter()
            .map(|table| {
                let selected = !left_out.contains(&table.name);
                (table, selected)
            })
            .collect();
        self.listed = Some(database);
    }

    /// Writes the unticked tables to the settings, so the same selection
    /// comes back the next time this database is listed.
    fn remember(&mut self, cx: &mut Context<Self>) {
        let (Some(profile), Some(database)) = (self.profile.clone(), self.listed.clone()) else {
            return;
        };
        let left_out = self.left_out();
        let result = cx.update_global::<Core, _>(|core, _| {
            core.0.settings.set_left_out(&profile, &database, left_out);
            core.0.settings.save(&core.0.config_dir)
        });
        if let Err(error) = result {
            self.list_note = Some(format!("The selection is not remembered: {error}"));
        }
    }

    /// Whether the table list is of another database than the one in the
    /// field now (the name was changed after the tables were listed).
    pub(crate) fn tables_stale(&self, cx: &App) -> bool {
        let database = self.database.read(cx).value();
        self.listed
            .as_ref()
            .is_some_and(|listed| listed != database.trim())
    }

    /// The names of the unticked tables.
    fn left_out(&self) -> Vec<String> {
        self.tables
            .iter()
            .filter(|(_, selected)| !*selected)
            .map(|(table, _)| table.name.clone())
            .collect()
    }

    /// Ticks or unticks one table.
    pub(crate) fn set_table(&mut self, index: usize, selected: bool, cx: &mut Context<Self>) {
        self.tables[index].1 = selected;
        self.remember(cx);
        cx.notify();
    }

    /// The indexes into `tables` of the rows the filter shows.
    fn shown(&self, cx: &App) -> Vec<usize> {
        let filter = self.filter.read(cx).value().trim().to_lowercase();
        self.tables
            .iter()
            .enumerate()
            .filter(|(_, (table, _))| table.name.to_lowercase().contains(&filter))
            .map(|(index, _)| index)
            .collect()
    }

    /// Ticks or unticks the rows the filter shows; hidden rows keep their
    /// state.
    pub(crate) fn set_shown(&mut self, selected: bool, cx: &mut Context<Self>) {
        for index in self.shown(cx) {
            self.tables[index].1 = selected;
        }
        self.remember(cx);
        cx.notify();
    }

    /// The form as the options the core understands: the same ones the
    /// command line tool builds from its arguments.
    pub(crate) fn options(&self, dry_run: bool, cx: &App) -> DumpOptions {
        // mydumper takes the tables to leave out: the unticked ones, or the
        // typed names when there is no list.
        let exclude: Vec<String> = if self.tables_failed {
            vec![self.exclude.read(cx).value().to_string()]
        } else if self.tables_stale(cx) {
            // The list is of another database: nothing of it applies.
            Vec::new()
        } else {
            self.left_out()
        };
        let out = self.out.read(cx).value().trim().to_string();
        let home = core(cx).home;
        let (log_level, extra_args) = log_options(dry_run);
        DumpOptions {
            profile: self.profile.clone().unwrap_or_default(),
            database: self.database.read(cx).value().trim().to_string(),
            exclude,
            out: (!out.is_empty()).then(|| platform::expand_tilde(&out, home.as_deref())),
            log_level,
            extra_args,
            dry_run,
        }
    }

    /// Plans and prepares the dump in the background. A dry run then shows
    /// the command; a real run hands over to a `RunView`.
    pub(crate) fn prepare(&mut self, dry_run: bool, cx: &mut Context<Self>) {
        if !self.tables.is_empty()
            && !self.tables_stale(cx)
            && self.tables.iter().all(|(_, selected)| !*selected)
        {
            self.preview = Some(Err("Select at least one table to dump.".into()));
            cx.notify();
            return;
        }
        if self.busy {
            return;
        }
        let mut ctx = core(cx);
        let mut options = self.options(dry_run, cx);
        if dry_run {
            name_the_profile(&mut ctx, Tool::Mydumper, &mut options.extra_args);
        }
        self.busy = true;
        // "Show command" keeps the command that is on show until the new
        // one takes its place, so the page does not jump.
        if !dry_run {
            self.preview = None;
            cx.notify();
        }
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
        let mut page = widgets::page().child(widgets::header(
            "Dump",
            "Copy a database from a server into a folder with mydumper.",
            cx,
        ));
        if self.profiles.is_empty() {
            return page.child(widgets::notice(
                Tone::Info,
                "No profiles yet. Create one on the Profiles screen.",
                cx,
            ));
        }

        // 1. Profile
        let mut profiles = widgets::wrap_row();
        for (index, name) in self.profiles.iter().enumerate() {
            let picked = name.clone();
            let chosen = self.profile.as_ref() == Some(name);
            profiles = profiles.child(
                widgets::profile_choice(("dump-profile", index), name, chosen, cx).on_click(
                    cx.listener(move |this, _, window, cx| {
                        this.pick_profile(picked.clone(), window, cx)
                    }),
                ),
            );
        }
        let mut source =
            widgets::section("Source", cx).child(widgets::field("Profile", profiles, cx));
        // The form is about one profile: its label heads it.
        page = page.child(widgets::profile_strip(
            self.profile.as_deref(),
            self.server.as_deref(),
            cx,
        ));
        let Some(profile) = self.profile.clone() else {
            return page.child(source);
        };
        // The rest of the form waits for the server's answer, and is not
        // shown when there is none.
        if self.listing_databases {
            return page
                .child(source.child(widgets::loading(format!("Connecting with {profile}…"), cx)));
        }
        if let Some(error) = self.connect_error.clone() {
            return page.child(source).child(widgets::connection_failed(
                &profile,
                &error,
                Button::new("dump-retry")
                    .on_click(cx.listener(|this, _, _, cx| this.list_databases(cx))),
                cx,
            ));
        }

        // 2. Database
        // The name field narrows the list under it; a click on a row
        // takes that database and lists its tables.
        let typed = self.database.read(cx).value().to_string();
        let databases = widgets::database_list(
            "dump-databases",
            &self.databases,
            &typed,
            |name| {
                Box::new(cx.listener(move |this, _, window, cx| {
                    this.pick_database(name.clone(), window, cx)
                }))
            },
            cx,
        );
        source = source.child(widgets::field(
            "Database",
            v_flex()
                .gap_2()
                .child(
                    Input::new(&self.database)
                        .cleanable(true)
                        .font_family(cx.theme().mono_font_family.clone()),
                )
                .children(databases)
                .when_some(self.list_note.clone(), |this, note| {
                    this.child(widgets::hint(note, cx))
                }),
            cx,
        ));
        page = page.child(source);

        // 3. Tables and views to dump
        let mut tables = widgets::section("Tables", cx);
        if self.listing_tables {
            tables = tables.child(widgets::loading("Asking the server for the tables…", cx));
        } else if self.tables_failed {
            // No list from the server: the names to leave out can be typed.
            tables = tables.child(widgets::field(
                "Leave out",
                v_flex()
                    .gap_2()
                    .child(Input::new(&self.exclude))
                    .child(widgets::hint(
                        "Every table and view is dumped, except the names typed here, \
                         separated by commas.",
                        cx,
                    ))
                    .child(
                        h_flex().child(
                            Button::new("list-tables")
                                .outline()
                                .icon(IconName::RefreshCw)
                                .label("List tables again")
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.list_tables(window, cx)),
                                ),
                        ),
                    ),
                cx,
            ));
        } else if self.tables.is_empty() || self.tables_stale(cx) {
            // Not listed yet, or listed for another database than the one
            // in the field now: the button that lists them is here.
            let database = self.database.read(cx).value().trim().to_string();
            let about = if database.is_empty() {
                "Choose a database first. Its tables can then be listed here to select \
                 which are dumped."
                    .to_string()
            } else {
                format!(
                    "List the tables of {database} to select which are dumped. Without \
                     a selection, every table and view is dumped."
                )
            };
            tables = tables.child(widgets::hint(about, cx)).child(
                h_flex().child(
                    Button::new("list-tables")
                        .outline()
                        .label("List tables")
                        .disabled(database.is_empty())
                        .on_click(cx.listener(|this, _, window, cx| this.list_tables(window, cx))),
                ),
            );
        } else {
            let shown = self.shown(cx);
            let selected = self.tables.iter().filter(|(_, selected)| *selected).count();
            // Table names are set like the other names of the server.
            let mut checklist = v_flex()
                .gap_1p5()
                .font_family(cx.theme().mono_font_family.clone());
            for &index in &shown {
                let (table, selected) = &self.tables[index];
                let label = if table.is_view {
                    format!("{}  (view)", table.name)
                } else {
                    table.name.clone()
                };
                checklist = checklist.child(
                    // The id is the place in `tables`, not in the shown rows,
                    // so a tick lands on the right table under a filter.
                    Checkbox::new(("dump-table", index))
                        // Bounded by its parent, so a long label wraps.
                        .w_full()
                        .label(label)
                        .checked(*selected)
                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            this.set_table(index, *checked, cx);
                        })),
                );
            }
            if shown.is_empty() {
                checklist = checklist.child(widgets::hint("No table matches the filter.", cx));
            }
            let filtered = shown.len() < self.tables.len();
            tables = tables.child(widgets::field(
                "Include",
                v_flex()
                    .gap_2()
                    .child(
                        widgets::wrap_row()
                            .items_center()
                            .child(
                                div()
                                    .flex_grow(1.)
                                    .flex_basis(px(160.))
                                    .min_w_0()
                                    .child(Input::new(&self.filter)),
                            )
                            .child(
                                Button::new("tables-all")
                                    .outline()
                                    .label(if filtered {
                                        "Select shown"
                                    } else {
                                        "Select all"
                                    })
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.set_shown(true, cx)),
                                    ),
                            )
                            .child(
                                Button::new("tables-none")
                                    .outline()
                                    .label(if filtered {
                                        "Deselect shown"
                                    } else {
                                        "Select none"
                                    })
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.set_shown(false, cx)),
                                    ),
                            ),
                    )
                    .child(
                        // A database can have hundreds of tables: the list
                        // scrolls on its own, so the rest of the form stays
                        // in reach.
                        div()
                            .id("table-list")
                            .max_h(px(220.))
                            .overflow_y_scroll()
                            // GPUI gives a scroll to every scrolling
                            // element under the pointer. `occlude` hides
                            // the page behind the list from the pointer, so
                            // only the list moves. A list short enough to
                            // show all its rows does not scroll: there the
                            // page keeps the scroll.
                            .when(shown.len() > ROWS_WITHOUT_SCROLL, |this| this.occlude())
                            .p_2()
                            .rounded(theme::RADIUS)
                            .border_1()
                            .border_color(cx.theme().border)
                            .child(checklist),
                    )
                    .child(widgets::hint(
                        format!(
                            "{selected} of {} selected. Tables and views without a tick \
                             are left out of the dump. The selection is remembered for this \
                             profile and database.",
                            self.tables.len()
                        ),
                        cx,
                    )),
                cx,
            ));
        }
        page = page.child(tables);

        // 4. Folder
        page = page.child(
            widgets::section("Output", cx)
                .child(widgets::field(
                    "Folder",
                    h_flex()
                        .gap_2()
                        .child(div().flex_1().min_w_0().child(
                            Input::new(&self.out).font_family(cx.theme().mono_font_family.clone()),
                        ))
                        .child(
                            Button::new("choose-dump-out")
                                .outline()
                                .icon(IconName::FolderOpen)
                                .tooltip("Choose a folder")
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.choose_out(window, cx)),
                                ),
                        ),
                    cx,
                ))
                .child(widgets::hint(
                    "A full log is written beside the dump. The Dumps screen shows it.",
                    cx,
                )),
        );

        // 5. Preview and run
        page = page.child(
            widgets::wrap_row()
                .child(
                    Button::new("dump-run")
                        .primary()
                        .label("Run dump")
                        .on_click(cx.listener(|this, _, _, cx| this.prepare(false, cx))),
                )
                .child(
                    Button::new("dump-dry")
                        .outline()
                        .label("Show command")
                        .on_click(cx.listener(|this, _, _, cx| this.prepare(true, cx))),
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

impl Render for DumpView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // While a run exists, it takes the place of the form.
        if let Some(run) = self.run.clone() {
            let finished = run.read(cx).is_finished();
            return widgets::page()
                .child(widgets::header(
                    "Dump",
                    "Copy a database from a server into a folder with mydumper.",
                    cx,
                ))
                // The run is of this profile: its label stays in place.
                .child(widgets::profile_strip(
                    self.profile.as_deref(),
                    self.server.as_deref(),
                    cx,
                ))
                .child(run)
                .when(finished, |this| {
                    this.child(
                        h_flex().child(
                            Button::new("dump-back")
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
