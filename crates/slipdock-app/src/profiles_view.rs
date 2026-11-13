//! Profiles: list, create, delete, test the connection, and a form for the
//! common keys.
//!
//! The form edits only the keys it shows. Everything else in the file
//! (comments, tuning options) is left exactly as it is, because saving goes
//! through the core's line-preserving `Document`. "Open file" is there for
//! the rest.
//!
//! The colour of a profile is not in its file but in the settings, so it is
//! not part of "Save": it is written the moment it is chosen.

use gpui_kit::component::button::*;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::{
    ActiveTheme, Disableable as _, Icon, IconName, Sizable as _, WindowExt as _, h_flex, v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use slipdock_core::profile::{self, PasswordStore, Profile, SECTION};
use slipdock_core::{Error, Result, secrets, server};

use crate::band_picker::{BandPicked, BandPicker};
use crate::state::{Core, core};
use crate::theme;
use crate::widgets::{self, Tone};

pub struct ProfilesView {
    profiles: Vec<Profile>,
    /// The name of the profile in the form.
    selected: Option<String>,
    // GPUI concept: a text field's content lives in its own entity
    // (`InputState`); the `Input` element in `render` only displays it.
    /// The name typed in the "New profile" dialog.
    pub(crate) new_name: Entity<InputState>,
    /// Why the name in the dialog was not accepted.
    pub(crate) new_error: Option<String>,
    pub(crate) host: Entity<InputState>,
    port: Entity<InputState>,
    user: Entity<InputState>,
    password: Entity<InputState>,
    pub(crate) dump_dir: Entity<InputState>,
    /// The band of the profile in the form, as the colour field shows it.
    pub(crate) band: Entity<BandPicker>,
    tls_off: bool,
    keychain: bool,
    /// The connection test is waiting for the server.
    testing: bool,
    /// Feedback after an action: (it worked, text).
    pub(crate) message: Option<(bool, String)>,
    /// Subscriptions end when they are dropped, so the view keeps them.
    _subscriptions: Vec<Subscription>,
}

impl ProfilesView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let new_name = input("Name, for example staging", window, cx);
        let host = input("127.0.0.1", window, cx);
        let port = input("3306", window, cx);
        let user = input("root", window, cx);
        let password = cx.new(|cx| InputState::new(window, cx).masked(true));
        let dump_dir = input("~/dumps", window, cx);
        // "Unsaved changes" follows what is typed, so the form is drawn
        // again when a field changes.
        let mut subscriptions: Vec<Subscription> = [&host, &port, &user, &password, &dump_dir]
            .into_iter()
            .map(|field| {
                cx.subscribe(field, |_, _, event: &InputEvent, cx| {
                    if matches!(event, InputEvent::Change) {
                        cx.notify();
                    }
                })
            })
            .collect();
        // Enter in the name field of the dialog does what "Create" does.
        subscriptions.push(cx.subscribe_in(
            &new_name,
            window,
            |this, _, event: &InputEvent, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.create(window, cx);
                }
            },
        ));
        // A colour chosen in the field is the profile's band from then on.
        let band = cx.new(|cx| BandPicker::new(window, cx));
        subscriptions.push(cx.subscribe(&band, |this, _, event: &BandPicked, cx| {
            this.set_band(event.0, cx);
        }));
        let mut view = ProfilesView {
            profiles: Vec::new(),
            selected: None,
            new_name,
            new_error: None,
            host,
            port,
            user,
            password,
            dump_dir,
            band,
            tls_off: false,
            keychain: false,
            testing: false,
            message: None,
            _subscriptions: subscriptions,
        };
        view.reload(cx);
        view.select_first(window, cx);
        view
    }

    pub fn reload(&mut self, cx: &mut Context<Self>) {
        self.profiles = profile::list(&core(cx).config_dir);
        if let Some(name) = &self.selected
            && !self.profiles.iter().any(|profile| &profile.name == name)
        {
            self.selected = None;
        }
        cx.notify();
    }

    /// Shows the first profile, so the screen does not open on an empty
    /// form.
    fn select_first(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_none()
            && let Some(name) = self.profiles.first().map(|profile| profile.name.clone())
        {
            self.select(&name, window, cx);
        }
    }

    /// Whether the form differs from the profile as it is saved.
    pub(crate) fn has_changes(&self, profile: &Profile, cx: &App) -> bool {
        let differs = |input: &Entity<InputState>, section: &str, key: &str| {
            input.read(cx).value().trim() != profile.doc.get(section, key).unwrap_or("")
        };
        let was_keychain = profile.password_store() == PasswordStore::Keychain;
        let password = self.password.read(cx).value();
        // The field of a keychain profile is empty until a new one is typed.
        let password_differs = if was_keychain {
            !password.is_empty()
        } else {
            password.as_ref() != profile.doc.get("client", "password").unwrap_or("")
        };
        differs(&self.host, "client", "host")
            || differs(&self.port, "client", "port")
            || differs(&self.user, "client", "user")
            || differs(&self.dump_dir, SECTION, "dump-dir")
            || password_differs
            || self.tls_off != profile.tls_verify_off()
            || self.keychain != was_keychain
    }

    /// Puts a profile's values into the form.
    pub(crate) fn select(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(profile) = self.profiles.iter().find(|profile| profile.name == name) else {
            return;
        };
        let get =
            |section: &str, key: &str| profile.doc.get(section, key).unwrap_or("").to_string();
        let values = [
            (&self.host, get("client", "host")),
            (&self.port, get("client", "port")),
            (&self.user, get("client", "user")),
            (&self.password, get("client", "password")),
            (&self.dump_dir, get(SECTION, "dump-dir")),
        ];
        self.tls_off = profile.tls_verify_off();
        self.keychain = profile.password_store() == PasswordStore::Keychain;
        for (input, value) in values {
            input.update(cx, |input, cx| input.set_value(value, window, cx));
        }
        // A profile made a moment ago has no band until it is given one.
        theme::assign_bands(cx);
        let band = core(cx).settings.band(name);
        // Showing a band in the field is not a choice: it changes nothing.
        self.band
            .update(cx, |field, cx| field.set_color(band, window, cx));
        self.selected = Some(name.to_string());
        self.message = None;
        cx.notify();
    }

    /// Makes `band` (`0xRRGGBB`) the band of the profile in the form, at
    /// once: the band is in the settings, not in the profile's file, so
    /// "Save" has no part in it.
    pub(crate) fn set_band(&mut self, band: u32, cx: &mut Context<Self>) {
        let Some(name) = self.selected.clone() else {
            return;
        };
        if let Err(error) = theme::set_band(&name, band, cx) {
            self.message = Some((false, error.to_string()));
        }
        cx.notify();
    }

    /// Opens the dialog that asks for the name of a new profile.
    pub(crate) fn open_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.new_error = None;
        self.new_name.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });
        let view = cx.entity();
        let input = self.new_name.clone();
        // The builder may run many times while the dialog is open, so it
        // only describes the dialog from the current state.
        window.open_dialog(cx, move |dialog, _, cx| {
            let error = view.read(cx).new_error.clone();
            let create_view = view.clone();
            dialog
                .title("New profile")
                .child(
                    v_flex()
                        .gap_2()
                        // The dialog cuts its content off at the edge, and
                        // the ring of a focused field is drawn outside the
                        // field: this is the room for it.
                        .p_1()
                        .child(Input::new(&input))
                        .child(widgets::hint(
                            "The name of the profile, for example staging. It becomes the \
                             name of its file and part of the name of its dumps, so no \
                             spaces or slashes.",
                            cx,
                        ))
                        .when_some(error, |this, error| {
                            this.child(widgets::error_text(error, cx))
                        }),
                )
                .footer(
                    h_flex()
                        .gap_2()
                        .justify_end()
                        .child(
                            Button::new("new-profile-cancel")
                                .outline()
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("new-profile-create")
                                .primary()
                                .label("Create")
                                .on_click(move |_, window, cx| {
                                    create_view.update(cx, |this, cx| this.create(window, cx));
                                }),
                        ),
                )
        });
        // So the name can be typed at once.
        self.new_name
            .update(cx, |input, cx| input.focus(window, cx));
    }

    /// Creates the profile named in the dialog and puts it in the form. A
    /// name that cannot be used keeps the dialog open and says why.
    pub(crate) fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.new_name.read(cx).value().trim().to_string();
        match profile::init(&core(cx).config_dir, &name) {
            Ok(_) => {
                window.close_dialog(cx);
                self.new_error = None;
                self.reload(cx);
                self.select(&name, window, cx);
                self.message = Some((true, format!("Created {name}. Fill in the form and save.")));
            }
            Err(error) => {
                self.new_error = Some(error.to_string());
                // The dialog is drawn by the window, not by this view.
                window.refresh();
            }
        }
        cx.notify();
    }

    pub(crate) fn save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let result = self.write_form(cx);
        let saved = result.is_ok();
        let message = Some(match result {
            Ok(()) => (true, "Saved.".to_string()),
            Err(error) => (false, error.to_string()),
        });
        self.reload(cx);
        // Show the profile as it is in the file now: a password that went
        // to the keychain leaves the field, and nothing counts as unsaved.
        if saved && let Some(name) = self.selected.clone() {
            self.select(&name, window, cx);
        }
        self.message = message;
    }

    fn write_form(&mut self, cx: &mut Context<Self>) -> Result<()> {
        let ctx = core(cx);
        let name = self
            .selected
            .clone()
            .ok_or_else(|| Error::msg("no profile selected"))?;
        // Read the file again, so a change made in an editor meanwhile is kept.
        let mut profile = ctx.profile(&name)?;
        self.apply_form(&mut profile, cx)?;

        // The password is not trimmed: spaces may be part of it.
        let password = self.password.read(cx).value().to_string();
        let was_keychain = profile.password_store() == PasswordStore::Keychain;
        if self.keychain {
            if !password.is_empty() {
                secrets::move_to_store(ctx.secrets.as_ref(), &mut profile, &password)?;
            } else if !was_keychain {
                return Err(Error::msg("type the password to move it to the keychain"));
            }
        } else if was_keychain {
            if password.is_empty() {
                return Err(Error::msg(
                    "type the password to move it back into the file",
                ));
            }
            secrets::move_to_file(ctx.secrets.as_ref(), &mut profile, Some(&password))?;
        } else {
            profile.doc.set("client", "password", &password)?;
        }
        profile.save()
    }

    /// Puts the form's values, except the password, into a profile.
    fn apply_form(&self, profile: &mut Profile, cx: &App) -> Result<()> {
        let value = |input: &Entity<InputState>| input.read(cx).value().trim().to_string();
        profile.doc.set("client", "host", &value(&self.host))?;
        profile.doc.set("client", "port", &value(&self.port))?;
        profile.doc.set("client", "user", &value(&self.user))?;
        profile
            .doc
            .set(SECTION, "dump-dir", &value(&self.dump_dir))?;
        if self.tls_off {
            profile.doc.set(SECTION, "tls-verify", "FALSE")?;
        } else {
            profile.doc.remove(SECTION, "tls-verify");
        }
        Ok(())
    }

    /// Connects with what the form shows, saved or not, and says what the
    /// server answered. Nothing is written.
    pub(crate) fn test_connection(&mut self, cx: &mut Context<Self>) {
        let ctx = core(cx);
        let Some(name) = self.selected.clone() else {
            return;
        };
        let prepared = ctx.profile(&name).and_then(|mut profile| {
            self.apply_form(&mut profile, cx)?;
            Ok(profile)
        });
        let profile = match prepared {
            Ok(profile) => profile,
            Err(error) => {
                self.message = Some((false, error.to_string()));
                cx.notify();
                return;
            }
        };
        // An empty field of a keychain profile means "the stored one".
        let typed = self.password.read(cx).value().to_string();
        let stored = typed.is_empty() && profile.password_store() == PasswordStore::Keychain;
        let password = (!stored).then_some(typed);
        self.testing = true;
        self.message = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let target = profile.target();
            let result = cx
                .background_executor()
                .spawn(async move { server::version(&ctx, &profile, password.as_deref()) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.testing = false;
                this.message = Some(match result {
                    Ok(version) => (
                        true,
                        format!("Connected to {target}. Server version {version}."),
                    ),
                    Err(error) => (false, error.to_string()),
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// Asks before the profile in the form is deleted.
    fn request_delete(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self.selected.clone() else {
            return;
        };
        let view = cx.entity();
        window.open_dialog(cx, move |dialog, _, cx| {
            let view = view.clone();
            let name = name.clone();
            dialog
                .title("Delete this profile?")
                .child(
                    v_flex()
                        .gap_3()
                        .child(
                            div()
                                .p_3()
                                .min_w_0()
                                .rounded(theme::RADIUS)
                                .bg(cx.theme().muted)
                                .font_family(cx.theme().mono_font_family.clone())
                                .font_weight(FontWeight::BOLD)
                                .child(name.clone()),
                        )
                        .child(widgets::notice(
                            Tone::Danger,
                            "The profile file is deleted, with its password in the keychain \
                             if it has one. This cannot be undone.",
                            cx,
                        ))
                        .child(widgets::hint(
                            "Dumps taken with this profile stay on the disk.",
                            cx,
                        )),
                )
                .footer(
                    h_flex()
                        .gap_2()
                        .justify_end()
                        .child(
                            Button::new("delete-profile-cancel")
                                .outline()
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("delete-profile-confirm")
                                .danger()
                                .label("Delete profile")
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    view.update(cx, |this, cx| this.delete(&name, window, cx));
                                }),
                        ),
                )
        });
    }

    /// Deletes a profile: its keychain password, its file, and the table
    /// selections remembered for it.
    pub(crate) fn delete(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let ctx = core(cx);
        let result = ctx.profile(name).and_then(|profile| {
            // The keychain first: if it refuses, the profile is still there
            // and the delete can be tried again.
            if profile.password_store() == PasswordStore::Keychain {
                ctx.secrets.clear(name)?;
            }
            profile::delete(&ctx.config_dir, name)
        });
        self.message = Some(match result {
            Ok(()) => {
                let forgotten = cx.update_global::<Core, _>(|core, _| {
                    match core.0.settings.left_out.remove(name) {
                        Some(_) => core.0.settings.save(&core.0.config_dir),
                        None => Ok(()),
                    }
                });
                match forgotten {
                    Ok(()) => (true, format!("Deleted {name}.")),
                    Err(error) => (false, format!("Deleted {name}, but: {error}")),
                }
            }
            Err(error) => (false, error.to_string()),
        });
        let message = self.message.take();
        self.reload(cx);
        // The next profile takes the place of the deleted one in the form.
        self.select_first(window, cx);
        self.message = message;
    }

    /// Opens the system's folder dialog and puts the chosen folder in the
    /// form. Like typed text, it is written to the file on "Save".
    pub(crate) fn choose_dump_dir(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Use as dump folder".into()),
        });
        // `spawn_in` is `spawn` with the window at hand, needed to set the text.
        cx.spawn_in(window, async move |this, cx| {
            // Ok(Ok(Some(paths))) = the user picked something.
            if let Ok(Ok(Some(paths))) = chosen.await
                && let Some(path) = paths.into_iter().next()
            {
                let _ = this.update_in(cx, |this, window, cx| {
                    let text = widgets::tilde_path(&path, core(cx).home.as_deref());
                    this.dump_dir
                        .update(cx, |input, cx| input.set_value(text, window, cx));
                });
            }
        })
        .detach();
    }

    fn render_form(&self, profile: &Profile, cx: &mut Context<Self>) -> Div {
        let path = profile.path.clone();
        let border = cx.theme().border;
        let changed = self.has_changes(profile, cx);
        let password_hint = if self.keychain {
            "Kept in the system keychain. Leave empty to keep the stored one."
        } else {
            "Kept in the profile file (readable only by you)."
        };
        // A group of fields under a small title, set off by a line.
        let group = |title: &'static str, cx: &App| {
            v_flex()
                .gap_3()
                .pt_4()
                .border_t_1()
                .border_color(border)
                .child(widgets::subheading(title, cx))
        };
        widgets::card(cx)
            .gap_4()
            .child(
                // The name and the file, with "Open file" at the end of the
                // line; in a narrow window the button goes under them.
                h_flex()
                    .flex_wrap()
                    .items_start()
                    .gap_3()
                    .child(widgets::band_mark(
                        theme::band(&profile.name, cx),
                        true,
                        cx,
                    ))
                    .child(
                        v_flex()
                            .flex_grow(1.)
                            .flex_basis(px(200.))
                            .min_w_0()
                            .font_family(cx.theme().mono_font_family.clone())
                            .child(
                                div()
                                    .font_weight(FontWeight::BOLD)
                                    .child(profile.name.clone()),
                            )
                            .child(widgets::hint(core(cx).show_path(&profile.path), cx)),
                    )
                    .child(
                        Button::new("open-profile")
                            .small()
                            .outline()
                            .icon(IconName::ExternalLink)
                            .label("Open file")
                            .tooltip("Threads, rows and the other mydumper/myloader options are edited in the file")
                            .on_click(move |_, _, cx| cx.open_with_system(&path)),
                    ),
            )
            .child(
                group("Colour", cx).child(widgets::field(
                    "Band",
                    v_flex()
                        .gap_2()
                        // The button is as wide as what it shows, not
                        // as wide as the form.
                        .child(h_flex().child(self.band.clone()))
                        .child(widgets::hint(
                            "Shown beside this profile and its dumps. Kept as soon as it is chosen.",
                            cx,
                        )),
                    cx,
                )),
            )
            .child(
                group("Connection", cx)
                    .child(widgets::field("Host", Input::new(&self.host), cx))
                    .child(widgets::field("Port", Input::new(&self.port), cx))
                    .child(widgets::field("User", Input::new(&self.user), cx))
                    .child(widgets::field(
                        "Password",
                        v_flex()
                            .gap_2()
                            .child(Input::new(&self.password).mask_toggle())
                            .child(widgets::hint(password_hint, cx))
                            .child(
                                Checkbox::new("keychain")
                                    // Bounded by its parent, so a long label wraps.
                                    .w_full()
                                    .label("Keep the password in the system keychain")
                                    .checked(self.keychain)
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        this.keychain = *checked;
                                        cx.notify();
                                    })),
                            ),
                        cx,
                    ))
                    .child(widgets::field(
                        "TLS",
                        // As high as an input, so the box lines up with its label.
                        h_flex().min_h(px(32.)).child(
                            Checkbox::new("tls-off")
                                // Bounded by its parent, so a long label wraps.
                                .w_full()
                                .label("Do not check the server's certificate (still encrypted)")
                                .checked(self.tls_off)
                                .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                    this.tls_off = *checked;
                                    cx.notify();
                                })),
                        ),
                        cx,
                    )),
            )
            .child(
                group("Dumps", cx).child(widgets::field(
                    "Folder",
                    v_flex()
                        .gap_2()
                        .child(
                            h_flex()
                                .gap_2()
                                .child(div().flex_1().min_w_0().child(Input::new(&self.dump_dir)))
                                .child(
                                    Button::new("choose-dump-dir")
                                        .outline()
                                        .icon(IconName::FolderOpen)
                                        .tooltip("Choose a folder")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.choose_dump_dir(window, cx)
                                        })),
                                ),
                        )
                        .child(widgets::hint(
                            "Where dumps taken with this profile are written.",
                            cx,
                        )),
                    cx,
                )),
            )
            .child(
                // Save and test at the start of the line, delete at its end.
                h_flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .pt_4()
                    .border_t_1()
                    .border_color(border)
                    .child(
                        widgets::wrap_row()
                            .items_center()
                            .child(
                                Button::new("save-profile")
                                    .primary()
                                    .label("Save")
                                    .disabled(!changed)
                                    .on_click(cx.listener(|this, _, window, cx| this.save(window, cx))),
                            )
                            .child(
                                Button::new("test-profile")
                                    .outline()
                                    .label("Test connection")
                                    .tooltip("Connects with the values in the form, saved or not")
                                    .loading(self.testing)
                                    .disabled(self.testing)
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.test_connection(cx)),
                                    ),
                            )
                            .when(changed, |this| {
                                this.child(widgets::hint("Unsaved changes", cx))
                            }),
                    )
                    .child(
                        Button::new("delete-profile")
                            .danger()
                            .ghost()
                            .icon(Icon::new(gpui_kit::assets::IconName::Trash))
                            .label("Delete")
                            .on_click(
                                cx.listener(|this, _, window, cx| this.request_delete(window, cx)),
                            ),
                    ),
            )
    }
}

impl Render for ProfilesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self
            .selected
            .as_ref()
            .and_then(|name| self.profiles.iter().find(|profile| &profile.name == name));
        let changed = selected.is_some_and(|profile| self.has_changes(profile, cx));

        let mut list = v_flex().gap_2().min_w_0();
        for (index, profile) in self.profiles.iter().enumerate() {
            let name = profile.name.clone();
            let chosen = self.selected.as_ref() == Some(&profile.name);
            let keychain = profile.password_store() == PasswordStore::Keychain;
            list = list.child(
                widgets::option_row(("profile", index), profile.name.clone(), chosen, false, cx)
                    .child(widgets::band_mark(theme::band(&profile.name, cx), true, cx))
                    .child(
                        // Name, then the server on a line of its own, then
                        // the tags: nothing has to share a line, so a long
                        // host is not broken in the middle of its port.
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .font_family(cx.theme().mono_font_family.clone())
                                            .font_weight(FontWeight::BOLD)
                                            .child(profile.name.clone()),
                                    )
                                    .when(chosen && changed, |this| {
                                        this.child(widgets::tag("unsaved", Tone::Info))
                                    }),
                            )
                            .child(widgets::hint(profile.target(), cx))
                            .when(profile.tls_verify_off() || keychain, |this| {
                                this.child(
                                    widgets::wrap_row()
                                        .gap_1()
                                        .pt_1()
                                        .when(keychain, |this| {
                                            this.child(widgets::tag("keychain", Tone::Info))
                                        })
                                        .when(profile.tls_verify_off(), |this| {
                                            this.child(widgets::tag("TLS check off", Tone::Warning))
                                        }),
                                )
                            }),
                    )
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.select(&name, window, cx)),
                    ),
            );
        }
        // Without profiles there is no list: the card that says so, with the
        // one "New profile" button, takes the whole width.
        let empty = self.profiles.is_empty();

        let form = match selected {
            Some(profile) => self.render_form(profile, cx),
            None if self.profiles.is_empty() => widgets::card(cx)
                .items_center()
                .py_8()
                .gap_1()
                .child(
                    div()
                        .font_weight(FontWeight::MEDIUM)
                        .child("No profiles yet"),
                )
                .child(widgets::hint(
                    "A profile holds the connection settings of one server.",
                    cx,
                ))
                .child(
                    div().pt_3().child(
                        Button::new("first-profile")
                            .primary()
                            .icon(IconName::Plus)
                            .label("New profile")
                            .on_click(cx.listener(|this, _, window, cx| this.open_new(window, cx))),
                    ),
                ),
            None => widgets::card(cx)
                .items_center()
                .py_8()
                .child(widgets::hint("Select a profile to edit it.", cx)),
        };

        widgets::page()
            .child(
                // The title, with "New profile" at the end of its line; in
                // a narrow window the button goes under the title.
                h_flex()
                    .flex_wrap()
                    .items_start()
                    .justify_between()
                    .gap_3()
                    .child(
                        widgets::header(
                            "Profiles",
                            "A profile is one defaults file with the connection settings of an environment.",
                            cx,
                        )
                        .flex_grow(1.)
                        .flex_basis(px(280.)),
                    )
                    .when(!empty, |this| {
                        this.child(
                            Button::new("new-profile")
                                .outline()
                                .icon(IconName::Plus)
                                .label("New profile")
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.open_new(window, cx)),
                                ),
                        )
                    }),
            )
            .when_some(self.message.clone(), |this, (ok, text)| {
                let tone = if ok { Tone::Success } else { Tone::Danger };
                this.child(widgets::notice(tone, &text, cx))
            })
            .child(
                // List and form side by side. Each has a width it wants
                // (`flex_basis`); when the window cannot give both, the form
                // moves under the list and both take the full width.
                h_flex()
                    .flex_wrap()
                    .items_start()
                    .gap_5()
                    .when(!empty, |this| {
                        this.child(list.flex_grow(1.).flex_basis(px(250.)))
                    })
                    .child(form.flex_grow(3.).flex_basis(px(380.))),
            )
    }
}
