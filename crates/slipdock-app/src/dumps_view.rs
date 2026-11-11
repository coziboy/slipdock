//! Dumps: every dump folder of every profile, with size and state.

use std::path::PathBuf;

use gpui_kit::component::button::*;
use gpui_kit::component::{ActiveTheme, Sizable, h_flex, v_flex};
use gpui_kit::*;
use slipdock_core::dumps::{self, DumpInfo};
use slipdock_core::profile;

use crate::state::core;
use crate::widgets;

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
        let mut page = v_flex().gap_4().child(
            h_flex()
                .gap_4()
                .items_center()
                .child(widgets::heading("Dumps"))
                .child(
                    Button::new("refresh-dumps")
                        .small()
                        .label("Refresh")
                        .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                ),
        );
        let Some(roots) = &self.roots else {
            return page.child(widgets::hint("Reading…", cx));
        };
        if roots.is_empty() {
            return page.child(widgets::hint(
                "No profile has a dump-dir yet. Set one on the Profiles screen.",
                cx,
            ));
        }
        for (root_index, (root, found)) in roots.iter().enumerate() {
            let mut card = widgets::card(cx).child(
                div()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(ctx.show_path(root)),
            );
            if found.is_empty() {
                card = card.child(widgets::hint("No dumps here.", cx));
            }
            for (index, dump) in found.iter().enumerate() {
                let path = dump.path.clone();
                let state = if dump.complete {
                    div()
                        .text_sm()
                        .text_color(cx.theme().success)
                        .child("complete")
                } else {
                    div()
                        .text_sm()
                        .text_color(cx.theme().warning)
                        .child("partial")
                };
                card = card.child(
                    h_flex()
                        .gap_3()
                        .items_center()
                        .child(
                            div()
                                .w(px(60.))
                                .text_sm()
                                .child(dumps::human_size(dump.size)),
                        )
                        .child(div().flex_1().child(dump.name.clone()))
                        .child(widgets::hint(format!("{} tables", dump.tables), cx))
                        .child(div().w(px(70.)).child(state))
                        .child(
                            Button::new(("reveal", root_index * 10_000 + index))
                                .xsmall()
                                .ghost()
                                .label("Show")
                                // Opens the file manager with the folder selected.
                                .on_click(move |_, _, cx| cx.reveal_path(&path)),
                        ),
                );
            }
            page = page.child(card);
        }
        page
    }
}
