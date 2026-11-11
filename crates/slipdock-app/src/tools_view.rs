//! Setup / Tools: where mydumper, myloader, mysql and zstd are, and what to do
//! when one is missing or broken.

use std::path::PathBuf;
use std::process::Command;
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
use slipdock_core::tools::{self, InstallPlan, Status, Tool};

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
    /// `None` while the tools are being checked.
    statuses: Option<Vec<Status>>,
    /// The install instructions on show, if "Install…" was pressed.
    install: Option<(Tool, InstallPlan)>,
    installing: bool,
    message: Option<String>,
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
            install: None,
            installing: false,
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

    /// Checks all tools again. Each check starts the tool once, so it runs
    /// on a background thread and the result comes back with `this.update`.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let ctx = core(cx);
        cx.spawn(async move |this, cx| {
            let statuses = cx
                .background_executor()
                .spawn(async move {
                    Tool::ALL
                        .map(|tool| tools::status(tool, &ctx.settings, &ctx.dirs))
                        .to_vec()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.statuses = Some(statuses);
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
        self.message = result.err().map(|error| error.to_string());
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

    pub(crate) fn show_install(&mut self, tool: Tool, cx: &mut Context<Self>) {
        let ctx = core(cx);
        let mut dirs = ctx.dirs.path.clone();
        dirs.extend(ctx.dirs.usual.iter().cloned());
        let managers = platform::package_managers(&dirs);
        let plan = tools::install_plan(tool, platform::current_os(), &managers);
        self.install = Some((tool, plan));
        self.message = None;
        cx.notify();
    }

    /// Runs the install command that is on show. Only reachable through the
    /// button under the command, so the user has seen exactly what runs.
    fn run_install(&mut self, cx: &mut Context<Self>) {
        let Some((_, InstallPlan::Command { program, args })) = self.install.clone() else {
            return;
        };
        self.installing = true;
        self.message = Some(format!("Running {program} {} …", args.join(" ")));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let mut command = Command::new(&program);
                    command.args(&args);
                    exec::capture(&mut command, INSTALL_LIMIT)
                })
                .await;
            let message = match result {
                Ok(output) if output.success() => "Installed.".to_string(),
                Ok(output) => {
                    let text = output.stderr.trim();
                    let tail: Vec<&str> = text.lines().rev().take(6).collect();
                    let tail: Vec<&str> = tail.into_iter().rev().collect();
                    format!("The install did not finish:\n{}", tail.join("\n"))
                }
                Err(exec::CaptureError::TimedOut) => {
                    "The install took too long and was stopped.".to_string()
                }
                Err(exec::CaptureError::NotStarted(error)) => {
                    format!("Could not start the install: {error}")
                }
            };
            let _ = this.update(cx, |this, cx| {
                this.installing = false;
                this.install = None;
                this.message = Some(message);
                this.reload(cx);
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
                .child(if chosen.is_some() {
                    widgets::tag("ready", Tone::Success)
                } else {
                    widgets::tag("not found", Tone::Danger)
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
            None => {
                card = card.child(widgets::error_text(
                    status.problem().unwrap_or_default(),
                    cx,
                ));
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

        let mut buttons = widgets::wrap_row().pt_1();
        if chosen.is_none() {
            buttons = buttons.child(
                Button::new((tool.name(), 102usize))
                    .small()
                    .primary()
                    .label("Install…")
                    .on_click(cx.listener(move |this, _, _, cx| this.show_install(tool, cx))),
            );
        }
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

    fn render_install(&self, tool: Tool, plan: &InstallPlan, cx: &mut Context<Self>) -> Div {
        let card = widgets::section(format!("Install {tool}"), cx);
        match plan {
            InstallPlan::Command { program, args } => card
                .child(widgets::hint(
                    "This command installs it. Nothing runs until you press the button.",
                    cx,
                ))
                .child(widgets::mono(&format!("{program} {}", args.join(" ")), cx))
                .child(
                    widgets::wrap_row()
                        .child(
                            Button::new("run-install")
                                .primary()
                                .label("Run this command")
                                .loading(self.installing)
                                .disabled(self.installing)
                                .on_click(cx.listener(|this, _, _, cx| this.run_install(cx))),
                        )
                        .child(
                            Button::new("close-install")
                                .outline()
                                .label("Close")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.install = None;
                                    cx.notify();
                                })),
                        ),
                ),
            InstallPlan::Steps(steps) => card
                .child(widgets::hint(
                    "These steps need administrator rights, so Slipdock does not run them:",
                    cx,
                ))
                .child(widgets::mono(&steps.join("\n"), cx)),
            InstallPlan::Unsupported(why) => card.child(why.clone()),
        }
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
                        "Slipdock runs mydumper and myloader for you. It never downloads them: \
                         it finds what is installed, or shows how to install it with your package manager.",
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
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.statuses = None;
                            this.reload(cx);
                            cx.notify();
                        })),
                ),
        );

        match &self.statuses {
            None => page = page.child(widgets::loading("Checking the tools…", cx)),
            Some(statuses) => {
                for status in statuses {
                    page = page.child(self.render_tool(status, cx));
                }
            }
        }
        if let Some((tool, plan)) = self.install.clone() {
            page = page.child(self.render_install(tool, &plan, cx));
        }
        if let Some(message) = &self.message {
            page = page.child(widgets::notice(Tone::Info, message, cx));
        }
        page
    }
}
