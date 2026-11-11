//! Dumps: every dump folder of every profile, with size and state.

use std::path::PathBuf;

use gpui_kit::component::button::*;
use gpui_kit::component::{ActiveTheme, Icon, IconName, Sizable, h_flex};
use gpui_kit::*;
use slipdock_core::dumps::{self, DumpInfo};
use slipdock_core::profile;

use crate::state::core;
use crate::widgets::{self, Tone};

pub struct DumpsView {
    /// Each dump root with its dumps, newest first. `None` while loading.
    roots: Option<Vec<(PathBuf, Vec<DumpInfo>)>>,
}

impl DumpsView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut view = DumpsView { roots: None };
        view.reload(cx);
        view
    }

    /// Measuring folders reads the disk, so it happens in the background.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let ctx = core(cx);
        cx.spawn(async move |this, cx| {
            let roots = cx
                .background_executor()
                .spawn(async move {
                    let profiles = profile::list(&ctx.config_dir);
                    dumps::roots(&profiles, ctx.home.as_deref())
                        .into_iter()
                        .map(|root| {
                            let found = dumps::list(&root);
                            (root, found)
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.roots = Some(roots);
                cx.notify();
            });
        })
        .detach();
    }
}

impl Render for DumpsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ctx = core(cx);
        let border = cx.theme().border;
        let mut page = widgets::page().child(
            h_flex()
                .flex_wrap()
                .items_start()
                .justify_between()
                .gap_3()
                .child(
                    widgets::header(
                        "Dumps",
                        "Every dump in the dump folders of your profiles, newest first.",
                        cx,
                    )
                    .flex_grow(1.)
                    .flex_basis(px(280.)),
                )
                .child(
                    Button::new("refresh-dumps")
                        .outline()
                        .icon(IconName::RefreshCw)
                        .label("Refresh")
                        .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                ),
        );
        let Some(roots) = &self.roots else {
            return page.child(widgets::hint("Reading…", cx));
        };
        if roots.is_empty() {
            return page.child(widgets::notice(
                Tone::Info,
                "No profile has a dump-dir yet. Set one on the Profiles screen.",
                cx,
            ));
        }
        for (root_index, (root, found)) in roots.iter().enumerate() {
            let mut card = widgets::card(cx).gap_0().child(
                h_flex()
                    .items_start()
                    .gap_2()
                    .pb_2()
                    .child(
                        h_flex()
                            .h(px(24.))
                            .flex_shrink_0()
                            .child(Icon::new(IconName::Folder).small()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(ctx.show_path(root)),
                    ),
            );
            if found.is_empty() {
                card = card.child(widgets::hint("No dumps here.", cx));
            }
            for (index, dump) in found.iter().enumerate() {
                let path = dump.path.clone();
                let state = if dump.complete {
                    widgets::tag("complete", Tone::Success)
                } else {
                    widgets::tag("partial", Tone::Warning)
                };
                card = card.child(
                    // The name takes the room that is left. In a narrow
                    // window the details move under it.
                    h_flex()
                        .flex_wrap()
                        .items_center()
                        .gap_x_4()
                        .gap_y_1()
                        .py_2()
                        .border_t_1()
                        .border_color(border)
                        .child(
                            div()
                                .flex_grow(1.)
                                .flex_basis(px(260.))
                                .min_w_0()
                                .child(dump.name.clone()),
                        )
                        .child(
                            h_flex()
                                .gap_3()
                                .flex_shrink_0()
                                .child(widgets::hint(
                                    format!(
                                        "{} tables · {}",
                                        dump.tables,
                                        dumps::human_size(dump.size)
                                    ),
                                    cx,
                                ))
                                .child(state)
                                .child(
                                    Button::new(("reveal", root_index * 10_000 + index))
                                        .small()
                                        .ghost()
                                        .icon(IconName::FolderOpen)
                                        .label("Show")
                                        // Opens the file manager with the folder selected.
                                        .on_click(move |_, _, cx| cx.reveal_path(&path)),
                                ),
                        ),
                );
            }
            page = page.child(card);
        }
        page
    }
}
