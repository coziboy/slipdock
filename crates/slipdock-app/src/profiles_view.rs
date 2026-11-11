//! Profiles: list, create, import, and a form for the common keys.
//!
//! The form edits only the keys it shows. Everything else in the file
//! (comments, tuning options) is left exactly as it is, because saving goes
//! through the core's line-preserving `Document`. "Open file" is there for
//! the rest.

use gpui_kit::component::button::*;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{IconName, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use slipdock_core::profile::{self, PasswordStore, Profile, SECTION};
use slipdock_core::{Error, Result, secrets};

use crate::state::core;
use crate::widgets::{self, Tone};

pub struct ProfilesView {
    profiles: Vec<Profile>,
    /// The name of the profile in the form.
    selected: Option<String>,
    // GPUI concept: a text field's content lives in its own entity
    // (`InputState`); the `Input` element in `render` only displays it.
    new_name: Entity<InputState>,
    host: Entity<InputState>,
    port: Entity<InputState>,
    user: Entity<InputState>,
    password: Entity<InputState>,
    dump_dir: Entity<InputState>,
    tls_off: bool,
    keychain: bool,
    /// Feedback after an action: (it worked, text).
    message: Option<(bool, String)>,
}

impl ProfilesView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
            cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
        };
        let mut view = ProfilesView {
            profiles: Vec::new(),
            selected: None,
            new_name: input("new profile name", window, cx),
            host: input("127.0.0.1", window, cx),
            port: input("3306", window, cx),
            user: input("root", window, cx),
            password: cx.new(|cx| InputState::new(window, cx).masked(true)),
            dump_dir: input("~/dumps", window, cx),
            tls_off: false,
            keychain: false,
            message: None,
        };
        view.reload(cx);
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
        self.selected = Some(name.to_string());
        self.message = None;
        cx.notify();
    }

    fn create(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.new_name.read(cx).value().trim().to_string();
        match profile::init(&core(cx).config_dir, &name) {
            Ok(_) => {
                self.new_name
                    .update(cx, |input, cx| input.set_value("", window, cx));
                self.reload(cx);
                self.select(&name, window, cx);
                self.message = Some((true, format!("Created {name}. Fill in the form and save.")));
            }
            Err(error) => self.message = Some((false, error.to_string())),
        }
        cx.notify();
    }

    fn import(&mut self, cx: &mut Context<Self>) {
        let ctx = core(cx);
        let result = match &ctx.home {
            Some(home) => profile::import(&profile::mydb_dir(home), &ctx.config_dir),
            None => Err(Error::msg("cannot find your home folder")),
        };
        self.message = Some(match result {
            Ok(report) => (
                true,
                format!(
                    "Imported: {}. Already here, left alone: {}.",
                    list_or_none(&report.imported),
                    list_or_none(&report.skipped)
                ),
            ),
            Err(error) => (false, error.to_string()),
        });
        self.reload(cx);
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let result = self.write_form(cx);
        self.message = Some(match result {
            Ok(()) => (true, "Saved.".to_string()),
            Err(error) => (false, error.to_string()),
        });
        self.reload(cx);
    }

    fn write_form(&mut self, cx: &mut Context<Self>) -> Result<()> {
        let ctx = core(cx);
        let name = self
            .selected
            .clone()
            .ok_or_else(|| Error::msg("no profile selected"))?;
        // Read the file again, so a change made in an editor meanwhile is kept.
        let mut profile = ctx.profile(&name)?;
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

    fn render_form(&self, profile: &Profile, cx: &mut Context<Self>) -> Div {
        let path = profile.path.clone();
        let password_hint = if self.keychain {
            "Kept in the system keychain. Leave empty to keep the stored one."
        } else {
            "Kept in the profile file (readable only by you)."
        };
        widgets::card(cx)
            .child(
                v_flex()
                    .min_w_0()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(profile.name.clone()),
                    )
                    .child(widgets::hint(core(cx).show_path(&profile.path), cx)),
            )
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
            .child(widgets::field("Dump folder", Input::new(&self.dump_dir), cx))
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
            ))
            .child(
                widgets::wrap_row()
                    .pt_1()
                    .child(
                        Button::new("save-profile")
                            .primary()
                            .label("Save")
                            .on_click(cx.listener(|this, _, _, cx| this.save(cx))),
                    )
                    .child(
                        Button::new("open-profile")
                            .outline()
                            .icon(IconName::ExternalLink)
                            .label("Open file")
                            .tooltip("Threads, rows and the other mydumper/myloader options are edited in the file")
                            .on_click(move |_, _, cx| cx.open_with_system(&path)),
                    ),
            )
    }
}

fn list_or_none(names: &[String]) -> String {
    if names.is_empty() {
        "none".to_string()
    } else {
        names.join(", ")
    }
}

impl Render for ProfilesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut list = v_flex().gap_2().min_w_0();
        for (index, profile) in self.profiles.iter().enumerate() {
            let name = profile.name.clone();
            let chosen = self.selected.as_ref() == Some(&profile.name);
            list = list.child(
                widgets::option_row(("profile", index), profile.name.clone(), chosen, false, cx)
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(profile.name.clone()),
                            )
                            .child(widgets::hint(profile.target(), cx)),
                    )
                    .when(profile.tls_verify_off(), |this| {
                        this.child(widgets::tag("TLS check off", Tone::Warning))
                    })
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.select(&name, window, cx)),
                    ),
            );
        }
        if self.profiles.is_empty() {
            list = list.child(widgets::hint("No profiles yet. Create one below.", cx));
        }
        list = list.child(
            h_flex()
                .gap_2()
                .pt_2()
                .child(div().flex_1().min_w_0().child(Input::new(&self.new_name)))
                .child(
                    Button::new("create-profile")
                        .icon(IconName::Plus)
                        .label("New")
                        .on_click(cx.listener(|this, _, window, cx| this.create(window, cx))),
                ),
        )
        .child(
            Button::new("import-mydb")
                .outline()
                .label("Import from mydb")
                .tooltip("Copies ~/.config/mydb/*.cnf. Existing profiles and the originals are not changed.")
                .on_click(cx.listener(|this, _, _, cx| this.import(cx))),
        );

        let selected = self
            .selected
            .as_ref()
            .and_then(|name| self.profiles.iter().find(|profile| &profile.name == name));
        let form = match selected {
            Some(profile) => self.render_form(profile, cx),
            None => widgets::card(cx)
                .items_center()
                .py_8()
                .child(widgets::hint("Select a profile to edit it.", cx)),
        };

        widgets::page()
            .child(widgets::header(
                "Profiles",
                "A profile is one defaults file with the connection settings of an environment.",
                cx,
            ))
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
                    .child(list.flex_grow(1.).flex_basis(px(250.)))
                    .child(form.flex_grow(3.).flex_basis(px(380.))),
            )
    }
}
