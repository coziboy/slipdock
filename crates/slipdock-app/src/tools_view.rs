//! Setup / Tools: where mydumper, myloader, mysql and zstd are, and what to do
//! when one is missing or broken.

use std::path::PathBuf;
use std::time::Duration;

use gpui_kit::component::Disableable as _;
use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use slipdock_core::exec;
use slipdock_core::platform;
use slipdock_core::settings::{self, Settings};
use slipdock_core::tools::{self, Install, InstallPlan, Status, Tool};

use crate::state::{Core, core};
use crate::widgets::{self, Tone};

/// A package install can take a while (downloads, compiling).
const INSTALL_LIMIT: Duration = Duration::from_secs(30 * 60);

/// One choice among the settings of a tool: a field, and how its text gets
/// from and to the settings file.
///
/// The card of each tool draws the choices that are for it from a list of
/// these, so a choice of another kind is one more entry in `settings_list`:
/// the form, "Save" and the unsaved mark need no change.
pub(crate) struct Setting {
    /// The tool the choice is for; it is shown in that tool's card.
    pub(crate) tool: Tool,
    label: &'static str,
    /// What the choice does, and what an empty field means.
    about: String,
    pub(crate) input: Entity<InputState>,
    /// The value in the settings, as the text of the field.
    read: fn(&Settings) -> String,
    /// Puts the typed text into the settings, or says why it cannot.
    write: fn(&mut Settings, &str) -> slipdock_core::Result<()>,
}

/// Every choice, in the order they are shown within their tool.
///
/// Each tool has one open field for arguments rather than a field per
/// option: whatever option the tool knows can be given, and no new option
/// needs a change here.
fn settings_list(window: &mut Window, cx: &mut Context<ToolsView>) -> Vec<Setting> {
    let about = |tool: &str, used_for: &str, example: &str| {
        format!(
            "Options for every {used_for}, written as on the command line of {tool}, for \
             example {example}. Use the long form of an option (--threads, not -t)."
        )
    };
    // Checks the text and keeps it as it was typed.
    fn checked(text: &str) -> slipdock_core::Result<String> {
        settings::parse_arguments(text)?;
        Ok(text.trim().to_string())
    }
    let mut field =
        |example: &'static str| cx.new(|cx| InputState::new(window, cx).placeholder(example));
    vec![
        Setting {
            tool: Tool::Mydumper,
            label: "Arguments",
            about: about("mydumper", "dump", "--threads 8 --rows 100000"),
            input: field("--threads 8"),
            read: |settings| settings.options.mydumper.arguments.clone(),
            write: |settings, text| {
                settings.options.mydumper.arguments = checked(text)?;
                Ok(())
            },
        },
        Setting {
            tool: Tool::Myloader,
            label: "Arguments",
            about: about("myloader", "load", "--threads 8 --enable-binlog"),
            input: field("--threads 8"),
            read: |settings| settings.options.myloader.arguments.clone(),
            write: |settings, text| {
                settings.options.myloader.arguments = checked(text)?;
                Ok(())
            },
        },
    ]
}

pub struct ToolsView {
    /// The choices of the tools; each card shows the ones of its tool.
    pub(crate) settings: Vec<Setting>,
    /// What "Save" of a tool's settings said: (the tool, it worked, text).
    pub(crate) settings_message: Option<(Tool, bool, String)>,
    /// Subscriptions end when they are dropped, so the view keeps them.
    _subscriptions: Vec<Subscription>,
    /// `None` until the tools were checked for the first time. A later
    /// check keeps the last result on show, so the page does not go blank.
    pub(crate) statuses: Option<Vec<Status>>,
    /// A check is running.
    checking: bool,
    /// What to install to get the tools that are missing, one entry for
    /// each package. Found together with `statuses`.
    pub(crate) installs: Vec<Install>,
    /// The plan whose command is running.
    installing: Option<InstallPlan>,
    /// What an install or a change of path said. It is drawn above the
    /// tools, so it is seen wherever the page was scrolled from.
    pub(crate) message: Option<(Tone, String)>,
}

impl ToolsView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = settings_list(window, cx);
        // "Save" is on while a field differs from the file, so the section
        // is drawn again when one changes.
        let subscriptions = settings
            .iter()
            .map(|setting| {
                cx.subscribe(&setting.input, |_, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        cx.notify();
                    }
                })
            })
            .collect();
        let mut view = ToolsView {
            settings,
            settings_message: None,
            _subscriptions: subscriptions,
            statuses: None,
            checking: false,
            installs: Vec::new(),
            installing: None,
            message: None,
        };
        view.show_settings(window, cx);
        view.reload(cx);
        view
    }

    /// Puts the values of the settings file into the fields.
    fn show_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let saved = core(cx).settings;
        for setting in &self.settings {
            let text = (setting.read)(&saved);
            setting
                .input
                .update(cx, |input, cx| input.set_value(text, window, cx));
        }
    }

    /// The choices that are for one tool.
    fn settings_of(&self, tool: Tool) -> impl Iterator<Item = &Setting> {
        self.settings
            .iter()
            .filter(move |setting| setting.tool == tool)
    }

    /// Whether a field of this tool differs from the settings file.
    pub(crate) fn settings_changed(&self, tool: Tool, cx: &App) -> bool {
        let saved = core(cx).settings;
        self.settings_of(tool)
            .any(|setting| setting.input.read(cx).value().trim() != (setting.read)(&saved))
    }

    /// Writes the fields of one tool to the settings file. One value that
    /// cannot be used stops the save, so the file never holds half of a form.
    pub(crate) fn save_settings(
        &mut self,
        tool: Tool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut next = core(cx).settings;
        let checked = self.settings_of(tool).try_for_each(|setting| {
            let text = setting.input.read(cx).value().to_string();
            (setting.write)(&mut next, &text)
        });
        let result = checked.and_then(|()| {
            cx.update_global::<Core, _>(|core, _| {
                next.save(&core.0.config_dir)?;
                core.0.settings = next;
                Ok(())
            })
        });
        self.settings_message = Some(match result {
            Ok(()) => {
                // The fields show the values as the file holds them. The
                // fields of the other tools keep what is typed in them.
                let saved = core(cx).settings;
                for setting in self.settings_of(tool) {
                    let text = (setting.read)(&saved);
                    setting
                        .input
                        .update(cx, |input, cx| input.set_value(text, window, cx));
                }
                let what = match tool {
                    Tool::Myloader => "load",
                    _ => "dump",
                };
                (tool, true, format!("Saved. It holds for the next {what}."))
            }
            Err(error) => (tool, false, error.to_string()),
        });
        cx.notify();
    }

    /// The settings of a tool, for the end of its card. `None` for a tool
    /// without settings.
    fn render_settings(&self, tool: Tool, cx: &mut Context<Self>) -> Option<Div> {
        self.settings_of(tool).next()?;
        let changed = self.settings_changed(tool, cx);
        let mut group = v_flex()
            .gap_3()
            .pt_4()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(widgets::subheading("Settings", cx));
        for setting in self.settings_of(tool) {
            group = group.child(widgets::field(
                setting.label,
                v_flex()
                    .gap_1()
                    .child(Input::new(&setting.input))
                    .child(widgets::hint(setting.about.clone(), cx)),
                cx,
            ));
        }
        let message = self
            .settings_message
            .clone()
            // "Saved" goes when a field is changed again; a reason for not
            // saving stays until the next try.
            .filter(|(of, ok, _)| *of == tool && (!*ok || !changed));
        Some(
            group
                .child(
                    widgets::wrap_row()
                        .items_center()
                        .child(
                            Button::new((tool.name(), 200usize))
                                .primary()
                                .small()
                                .label("Save")
                                .disabled(!changed)
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.save_settings(tool, window, cx)
                                })),
                        )
                        .when(changed, |this| {
                            this.child(widgets::hint("Unsaved changes", cx))
                        }),
                )
                .when_some(message, |this, (_, ok, text)| {
                    let tone = if ok { Tone::Success } else { Tone::Danger };
                    this.child(widgets::notice(tone, &text, cx))
                }),
        )
    }

    /// Checks all tools again, and finds what to install for the missing
    /// ones. Each check starts the tool once, so it runs on a background
    /// thread and the result comes back with `this.update`.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let ctx = core(cx);
        self.checking = true;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let (statuses, installs) = cx
                .background_executor()
                .spawn(async move {
                    let statuses = Tool::ALL
                        .map(|tool| tools::status(tool, &ctx.settings, &ctx.dirs))
                        .to_vec();
                    let missing: Vec<Tool> = statuses
                        .iter()
                        .filter(|status| status.chosen().is_none())
                        .map(|status| status.tool)
                        .collect();
                    let mut dirs = ctx.dirs.path.clone();
                    dirs.extend(ctx.dirs.usual.iter().cloned());
                    let managers = platform::package_managers(&dirs);
                    let installs = tools::installs(&missing, platform::current_os(), &managers);
                    (statuses, installs)
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.statuses = Some(statuses);
                this.installs = installs;
                this.checking = false;
                cx.notify();
            });
        })
        // A task stops when its handle is dropped; `detach` lets it finish
        // on its own.
        .detach();
    }

    /// Fixes the path of a tool (`Some`) or goes back to searching (`None`),
    /// saves the settings and checks again.
    fn set_path(&mut self, tool: Tool, path: Option<PathBuf>, cx: &mut Context<Self>) {
        let result = cx.update_global::<Core, _>(|core, _| {
            match path {
                Some(path) => core.0.settings.set_tool_path(tool.name(), path),
                None => core.0.settings.unset_tool_path(tool.name()),
            }
            core.0.settings.save(&core.0.config_dir)
        });
        self.message = result.err().map(|error| (Tone::Danger, error.to_string()));
        self.reload(cx);
    }

    /// Opens the system's file dialog and uses the chosen file.
    fn choose_file(&mut self, tool: Tool, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(format!("Use as {tool}").into()),
        });
        cx.spawn(async move |this, cx| {
            // Ok(Ok(Some(paths))) = the user picked something.
            if let Ok(Ok(Some(paths))) = chosen.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update(cx, |this, cx| this.set_path(tool, Some(path), cx));
            }
        })
        .detach();
    }

    /// Runs the command of one entry of `installs`, then checks the tools
    /// again. The command is on show beside the button that leads here, so
    /// the user has seen what runs. An entry without a command (steps for
    /// the user) starts nothing.
    pub(crate) fn run_install(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(install) = self.installs.get(index) else {
            return;
        };
        let InstallPlan::Command { program, args } = install.plan.clone() else {
            return;
        };
        if self.installing.is_some() {
            return;
        }
        let names = install.names();
        let shown = format!("{program} {}", args.join(" "));
        let Some(mut command) = tools::install_command(&program, &args, &core(cx).dirs) else {
            self.message = Some((
                Tone::Danger,
                format!("Could not start the install: {program} was not found."),
            ));
            cx.notify();
            return;
        };
        self.installing = Some(install.plan.clone());
        self.message = Some((
            Tone::Info,
            format!("Running {shown}. This can take a few minutes."),
        ));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { exec::capture(&mut command, INSTALL_LIMIT) })
                .await;
            let message = match result {
                Ok(output) if output.success() => (Tone::Success, format!("Installed {names}.")),
                Ok(output) => {
                    // The reason is at the end of what the command printed.
                    let text = match output.stderr.trim() {
                        "" => output.stdout.trim(),
                        errors => errors,
                    };
                    let tail: Vec<&str> = text.lines().rev().take(6).collect();
                    let tail: Vec<&str> = tail.into_iter().rev().collect();
                    (
                        Tone::Danger,
                        format!("{shown} did not finish:\n{}", tail.join("\n")),
                    )
                }
                Err(exec::CaptureError::TimedOut) => (
                    Tone::Danger,
                    format!("{shown} took too long and was stopped."),
                ),
                Err(exec::CaptureError::NotStarted(error)) => (
                    Tone::Danger,
                    format!("Could not start the install: {error}"),
                ),
            };
            let _ = this.update(cx, |this, cx| {
                this.installing = None;
                this.message = Some(message);
                this.reload(cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn render_tool(&self, status: &Status, cx: &mut Context<Self>) -> Div {
        let tool = status.tool;
        let ctx = core(cx);
        let chosen = status.chosen();
        let mut card = widgets::card(cx).child(
            h_flex()
                .flex_wrap()
                .items_center()
                .gap_x_3()
                .gap_y_1()
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(tool.name()),
                )
                .child(widgets::hint(tool.purpose(), cx).flex_1())
                .child(match (chosen.is_some(), status.absent()) {
                    (true, _) => widgets::tag("ready", Tone::Success),
                    // Slipdock works without an optional tool, so one that
                    // is not there has not failed and is not red.
                    (false, true) if tool.optional() => widgets::tag("not found", Tone::Info),
                    (false, true) => widgets::tag("not found", Tone::Danger),
                    // Something is there and cannot be used.
                    (false, false) => widgets::tag("not working", Tone::Danger),
                }),
        );

        match chosen {
            Some(candidate) => {
                let working = candidate
                    .result
                    .as_ref()
                    .expect("the chosen candidate works");
                let source = if status.configured.is_some() {
                    " · set by you"
                } else {
                    ""
                };
                card = card.child(
                    h_flex()
                        .items_start()
                        .gap_2()
                        .child(
                            h_flex().h(px(24.)).flex_shrink_0().child(
                                Icon::new(IconName::CircleCheck)
                                    .small()
                                    .text_color(cx.theme().success),
                            ),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .child(
                                    div()
                                        .font_family(cx.theme().mono_font_family.clone())
                                        .text_sm()
                                        .line_height(px(24.))
                                        .child(ctx.show_path(&candidate.path)),
                                )
                                .child(widgets::hint(
                                    format!("version {}{source}", working.version),
                                    cx,
                                )),
                        ),
                );
                if let Some(warning) = working.version_warning(tool) {
                    card = card.child(widgets::notice(Tone::Warning, &warning, cx));
                }
            }
            // Nothing to show but where to go next: the tag says the rest.
            None if status.absent() => {
                let can_install = self.installs.iter().any(|install| {
                    install.tools.contains(&tool)
                        && !matches!(install.plan, InstallPlan::Unsupported(_))
                });
                let mut text = String::from("Not on PATH or in the usual folders.");
                if tool.optional() {
                    text.push_str(" Slipdock works without it.");
                }
                text.push_str(if can_install {
                    " Install it above, or choose the file if it is in another folder."
                } else {
                    " Choose the file if it is in another folder."
                });
                card = card.child(widgets::hint(text, cx));
            }
            // One copy was found and it is broken: its row below says why,
            // so the reason is not said twice.
            None if status.configured.is_none() && status.candidates.len() == 1 => {}
            // The commands of the command line tool are left out: the
            // buttons of this card do the same.
            None => {
                card = card.child(widgets::error_text(status.reason().unwrap_or_default(), cx));
            }
        }

        // Everything else that was found, each with its own verdict.
        for (index, candidate) in status.candidates.iter().enumerate() {
            if chosen.is_some_and(|c| c.path == candidate.path) {
                continue;
            }
            let verdict = match &candidate.result {
                Ok(working) => format!("version {}, works", working.version),
                Err(problem) => problem.to_string(),
            };
            let path = candidate.path.clone();
            card = card.child(
                h_flex()
                    .flex_wrap()
                    .items_center()
                    .gap_x_3()
                    .gap_y_1()
                    .pt_2()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .child(
                        v_flex()
                            .flex_grow(1.)
                            .flex_basis(px(240.))
                            .min_w_0()
                            .child(
                                div()
                                    .font_family(cx.theme().mono_font_family.clone())
                                    .text_sm()
                                    .child(ctx.show_path(&candidate.path)),
                            )
                            .child(widgets::hint(verdict, cx)),
                    )
                    .when(candidate.result.is_ok(), |this| {
                        this.child(
                            Button::new((tool.name(), index))
                                .small()
                                .outline()
                                .label("Use this one")
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.set_path(tool, Some(path.clone()), cx)
                                })),
                        )
                    }),
            );
        }

        // A missing tool is installed from the top of the page (see
        // `render_installs`), once for each package, not here.
        let mut buttons = widgets::wrap_row().pt_1();
        buttons = buttons.child(
            Button::new((tool.name(), 100usize))
                .small()
                .outline()
                .label("Choose file…")
                .on_click(cx.listener(move |this, _, _, cx| this.choose_file(tool, cx))),
        );
        if status.configured.is_some() {
            buttons = buttons.child(
                Button::new((tool.name(), 101usize))
                    .small()
                    .outline()
                    .label("Search automatically")
                    .on_click(cx.listener(move |this, _, _, cx| this.set_path(tool, None, cx))),
            );
        }
        card.child(buttons).children(self.render_settings(tool, cx))
    }

    /// What is missing and how to get it, one group for each package.
    /// `None` when every tool is there.
    ///
    /// It is drawn above the tools: below them it would be off the window,
    /// and pressing a button would seem to do nothing.
    fn render_installs(&self, cx: &mut Context<Self>) -> Option<Div> {
        if self.installs.is_empty() {
            return None;
        }
        let mut card = widgets::section("Install", cx);
        for (index, install) in self.installs.iter().enumerate() {
            let mut group = v_flex().gap_2().min_w_0();
            if index > 0 {
                group = group.pt_3().border_t_1().border_color(cx.theme().border);
            }
            group = group.child(
                h_flex()
                    .flex_wrap()
                    .items_center()
                    .gap_x_3()
                    .gap_y_1()
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(install.names()),
                    )
                    .when(install.tools.iter().all(|tool| tool.optional()), |this| {
                        this.child(widgets::tag("optional", Tone::Info))
                    }),
            );
            group = match &install.plan {
                InstallPlan::Command { program, args } => {
                    let running = self.installing.as_ref() == Some(&install.plan);
                    group
                        .child(widgets::mono(&format!("{program} {}", args.join(" ")), cx))
                        .child(
                            widgets::wrap_row()
                                .items_center()
                                .child(
                                    Button::new(("install", index))
                                        .small()
                                        .primary()
                                        .label("Install")
                                        .loading(running)
                                        .disabled(self.installing.is_some())
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.run_install(index, cx)
                                        })),
                                )
                                .child(widgets::hint("Runs the command above.", cx)),
                        )
                }
                InstallPlan::Steps(steps) => group
                    .child(widgets::hint(
                        "Slipdock does not run these steps. Do them in a terminal, then press \
                         \u{201c}Check again\u{201d}.",
                        cx,
                    ))
                    .child(widgets::mono(&steps.join("\n"), cx)),
                InstallPlan::Unsupported(why) => group.child(widgets::hint(why.clone(), cx)),
            };
            card = card.child(group);
        }
        Some(card)
    }
}

impl Render for ToolsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut page = widgets::page().child(
            h_flex()
                .flex_wrap()
                .items_start()
                .justify_between()
                .gap_3()
                .child(
                    widgets::header(
                        "Tools",
                        "Slipdock runs mydumper and myloader for you. It finds the ones that are \
                         installed, and installs a missing one with your package manager where \
                         it can.",
                        cx,
                    )
                    .flex_grow(1.)
                    .flex_basis(px(280.)),
                )
                .child(
                    Button::new("recheck")
                        .outline()
                        .icon(IconName::RefreshCw)
                        .label("Check again")
                        .loading(self.checking)
                        .on_click(cx.listener(|this, _, _, cx| {
                            // What the last install or change said is old now.
                            this.message = None;
                            this.reload(cx);
                        })),
                ),
        );

        if let Some((tone, message)) = &self.message {
            page = page.child(widgets::notice(*tone, message, cx));
        }
        match &self.statuses {
            None => page = page.child(widgets::loading("Checking the tools…", cx)),
            Some(statuses) => {
                page = page.children(self.render_installs(cx));
                for status in statuses {
                    page = page.child(self.render_tool(status, cx));
                }
            }
        }
        page
    }
}
