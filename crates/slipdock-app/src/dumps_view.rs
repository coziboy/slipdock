//! Dumps: every dump folder of every profile, with size and state.
//!
//! A dump folder can hold hundreds of dumps. The screen stays usable and
//! quick to draw with that many: a filter narrows the lists, a folder can be
//! closed, and each folder draws one page of rows at a time.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use gpui_kit::component::button::*;
use gpui_kit::component::checkbox::Checkbox;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::select::{Select, SelectEvent, SelectItem, SelectState};
use gpui_kit::component::{
    ActiveTheme, Disableable as _, Icon, IconName, IndexPath, Sizable, WindowExt as _, h_flex,
    v_flex,
};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use slipdock_core::dumps::{self, DumpInfo};
use slipdock_core::profile;

use crate::log_view::LogView;
use crate::state::core;
use crate::theme;
use crate::widgets::{self, Tone};

pub struct DumpsView {
    /// Each dump root with its dumps, newest first. `None` while loading.
    pub(crate) roots: Option<Vec<(PathBuf, Vec<DumpInfo>)>>,
    /// The dumps ticked for "Delete selected".
    selected: HashSet<PathBuf>,
    /// The dumps that are being deleted.
    deleting: Vec<PathBuf>,
    /// Why the last delete did not work.
    pub(crate) error: Option<String>,
    /// The dump whose logs are being read for showing.
    reading_logs: Option<PathBuf>,
    /// What the logs dialog shows, while it is open.
    pub(crate) logs_shown: Option<WeakEntity<DumpLogs>>,
    /// The folders are being read again ("Refresh").
    reading: bool,
    /// Narrows the lists to the dumps whose name or date contains this text.
    pub(crate) filter: Entity<InputState>,
    /// The folders whose rows are hidden.
    collapsed: HashSet<PathBuf>,
    /// How many rows a folder draws, once "Show more" was pressed there.
    limits: HashMap<PathBuf, usize>,
    /// The filter text `limits` was made under. Under another text every
    /// folder starts at its first page again.
    limits_filter: String,
    /// A subscription ends when it is dropped, so the view keeps it.
    _subscription: Subscription,
}

/// The rows a folder draws at first, and how many "Show more" adds.
pub(crate) const PAGE: usize = 25;

impl DumpsView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter by name or date"));
        // The lists are drawn by this view, so it draws again when the
        // filter text changes.
        let subscription = cx.subscribe(&filter, |_, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        });
        let mut view = DumpsView {
            roots: None,
            selected: HashSet::new(),
            deleting: Vec::new(),
            error: None,
            reading_logs: None,
            logs_shown: None,
            reading: false,
            filter,
            collapsed: HashSet::new(),
            limits: HashMap::new(),
            limits_filter: String::new(),
            _subscription: subscription,
        };
        view.reload(cx);
        view
    }

    /// Measuring folders reads the disk, so it happens in the background.
    pub fn reload(&mut self, cx: &mut Context<Self>) {
        let ctx = core(cx);
        self.reading = true;
        cx.notify();
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
                // A dump that is gone cannot stay ticked.
                let present: HashSet<&PathBuf> = roots
                    .iter()
                    .flat_map(|(_, found)| found)
                    .map(|dump| &dump.path)
                    .collect();
                this.selected.retain(|path| present.contains(path));
                this.reading = false;
                this.roots = Some(roots);
                cx.notify();
            });
        })
        .detach();
    }

    /// The dumps of a folder that the filter lets through, with their place
    /// in the folder's list. The filter looks at the folder name and at the
    /// date as it is shown, without regard to case.
    pub(crate) fn matching<'a>(
        &self,
        found: &'a [DumpInfo],
        cx: &App,
    ) -> Vec<(usize, &'a DumpInfo)> {
        let filter = self.filter.read(cx).value().trim().to_lowercase();
        found
            .iter()
            .enumerate()
            .filter(|(_, dump)| {
                filter.is_empty()
                    || dump.name.to_lowercase().contains(&filter)
                    || dumps::taken(&dump.name)
                        .is_some_and(|taken| taken.to_lowercase().contains(&filter))
            })
            .collect()
    }

    /// How many rows of a folder are drawn.
    pub(crate) fn limit(&self, root: &PathBuf, cx: &App) -> usize {
        let same_filter = self.filter.read(cx).value().as_ref() == self.limits_filter;
        self.limits
            .get(root)
            .copied()
            .filter(|_| same_filter)
            .unwrap_or(PAGE)
    }

    /// Draws one more page of a folder.
    pub(crate) fn show_more(&mut self, root: &PathBuf, cx: &mut Context<Self>) {
        let limit = self.limit(root, cx) + PAGE;
        let filter = self.filter.read(cx).value().to_string();
        if filter != self.limits_filter {
            self.limits.clear();
            self.limits_filter = filter;
        }
        self.limits.insert(root.clone(), limit);
        cx.notify();
    }

    /// Hides or shows the rows of a folder.
    fn toggle(&mut self, root: &PathBuf, cx: &mut Context<Self>) {
        if !self.collapsed.remove(root) {
            self.collapsed.insert(root.clone());
        }
        cx.notify();
    }

    /// Ticks or unticks dumps for "Delete selected".
    pub(crate) fn select(&mut self, paths: &[PathBuf], selected: bool, cx: &mut Context<Self>) {
        for path in paths {
            if selected {
                self.selected.insert(path.clone());
            } else {
                self.selected.remove(path);
            }
        }
        cx.notify();
    }

    /// The ticked dumps, in the order of the list.
    pub(crate) fn selected_dumps(&self) -> Vec<DumpInfo> {
        self.roots
            .iter()
            .flatten()
            .flat_map(|(_, found)| found)
            .filter(|dump| self.selected.contains(&dump.path))
            .cloned()
            .collect()
    }

    /// Asks before dumps are deleted: they cannot be brought back.
    pub(crate) fn request_delete(
        &mut self,
        dumps: Vec<DumpInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if dumps.is_empty() {
            return;
        }
        /// More names than this are summed up in one line.
        const SHOWN: usize = 6;
        let view = cx.entity();
        let paths: Vec<PathBuf> = dumps.iter().map(|dump| dump.path.clone()).collect();
        let count = dumps.len();
        let shown: Vec<DumpInfo> = dumps.iter().take(SHOWN).cloned().collect();
        let size = dumps::human_size(dumps.iter().map(|dump| dump.size).sum());
        let (title, action, details) = match dumps.as_slice() {
            [dump] => (
                "Delete this dump?".to_string(),
                "Delete dump".to_string(),
                format!("{} tables · {size}", dump.tables),
            ),
            _ => (
                format!("Delete {} dumps?", dumps.len()),
                format!("Delete {} dumps", dumps.len()),
                format!("{size} in total"),
            ),
        };
        window.open_dialog(cx, move |dialog, _, cx| {
            let view = view.clone();
            let paths = paths.clone();
            let mut list = v_flex()
                .p_3()
                .gap_2()
                .min_w_0()
                .rounded(theme::RADIUS)
                .border_1()
                .border_color(cx.theme().border);
            // Each as the list shows it: the band of its profile, broken
            // for a partial dump, the date and the folder name.
            for dump in &shown {
                list = list.child(widgets::dump_summary(dump, cx));
            }
            if count > SHOWN {
                list = list.child(widgets::hint(format!("and {} more", count - SHOWN), cx));
            }
            dialog
                .title(title.clone())
                .child(
                    v_flex()
                        .gap_3()
                        .child(list.child(widgets::hint(details.clone(), cx)))
                        .child(widgets::notice(
                            Tone::Danger,
                            "The folders and their log files are deleted from the disk. \
                             This cannot be undone.",
                            cx,
                        )),
                )
                .footer(
                    h_flex()
                        .gap_2()
                        .justify_end()
                        .child(
                            Button::new("delete-cancel")
                                .outline()
                                .label("Cancel")
                                .on_click(|_, window, cx| window.close_dialog(cx)),
                        )
                        .child(
                            Button::new("delete-confirm")
                                .danger()
                                .label(action.clone())
                                .on_click(move |_, window, cx| {
                                    window.close_dialog(cx);
                                    view.update(cx, |this, cx| this.delete(paths.clone(), cx));
                                }),
                        ),
                )
        });
    }

    /// Reads the logs beside a dump in the background and shows them in a
    /// dialog: the log of the dump, then one per load.
    pub(crate) fn show_logs(
        &mut self,
        dump: &DumpInfo,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let path = dump.path.clone();
        let name = dump.name.clone();
        self.reading_logs = Some(path.clone());
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let logs = cx
                .background_executor()
                .spawn(async move { dumps::logs(&path) })
                .await;
            let _ = this.update_in(cx, |this, window, cx| {
                this.reading_logs = None;
                cx.notify();
                this.logs_shown = Some(Self::open_logs(name, logs, window, cx).downgrade());
            });
        })
        .detach();
    }

    fn open_logs(
        name: String,
        logs: Vec<dumps::DumpLog>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<DumpLogs> {
        // The builder below may run many times, so what the dialog shows
        // is a view that is made once, here.
        let logs = cx.new(|cx| DumpLogs::new(logs, window, cx));
        let body = logs.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            let body = body.clone();
            dialog
                .title(format!("Logs of {name}"))
                .w(px(760.))
                // No box that scrolls around the body: the dialog is no
                // higher than the window and scrolls what does not fit, and
                // the body has one height however many logs there are (see
                // `DumpLogs`). A box with a height of its own here cut the
                // log box off at its bottom.
                .child(body)
                .footer(
                    h_flex().justify_end().child(
                        Button::new("logs-close")
                            .outline()
                            .label("Close")
                            .on_click(|_, window, cx| window.close_dialog(cx)),
                    ),
                )
        });
        logs
    }

    /// Deletes dumps in the background, then reads the folders again. One
    /// that cannot be deleted does not stop the others.
    pub(crate) fn delete(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        self.deleting = paths.clone();
        self.error = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let errors = cx
                .background_executor()
                .spawn(async move {
                    paths
                        .iter()
                        .filter_map(|path| dumps::delete(path).err())
                        .map(|error| error.to_string())
                        .collect::<Vec<_>>()
                })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.deleting.clear();
                this.error = (!errors.is_empty()).then(|| errors.join("\n"));
                this.reload(cx);
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
        let busy = !self.deleting.is_empty();
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
                    widgets::wrap_row()
                        // Appears once a dump is ticked.
                        .when(!self.selected.is_empty(), |this| {
                            this.child(
                                Button::new("delete-selected")
                                    .danger()
                                    .label(format!("Delete {} selected", self.selected.len()))
                                    .loading(busy)
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.request_delete(this.selected_dumps(), window, cx)
                                    })),
                            )
                        })
                        .child(
                            Button::new("refresh-dumps")
                                .outline()
                                .icon(IconName::RefreshCw)
                                .label("Refresh")
                                .loading(self.reading)
                                .disabled(self.reading)
                                .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                        ),
                ),
        );
        if let Some(error) = &self.error {
            page = page.child(widgets::error_text(error.clone(), cx));
        }
        let Some(roots) = &self.roots else {
            return page.child(widgets::loading("Reading the dump folders…", cx));
        };
        if roots.is_empty() {
            return page.child(widgets::notice(
                Tone::Info,
                "No profile has a dump-dir yet. Set one on the Profiles screen.",
                cx,
            ));
        }
        // With more than one page of dumps, a filter and the totals.
        let everything = roots.iter().flat_map(|(_, found)| found);
        let dump_count = everything.clone().count();
        if dump_count > PAGE || !self.filter.read(cx).value().is_empty() {
            let size: u64 = everything.map(|dump| dump.size).sum();
            page = page.child(
                widgets::wrap_row()
                    .items_center()
                    .gap_x_3()
                    .child(
                        div()
                            .flex_grow(1.)
                            .flex_basis(px(220.))
                            .max_w(px(360.))
                            .min_w_0()
                            .child(Input::new(&self.filter).cleanable(true)),
                    )
                    .child(widgets::hint(
                        format!(
                            "{dump_count} dumps · {} in {} folders",
                            dumps::human_size(size),
                            roots.len()
                        ),
                        cx,
                    )),
            );
        }
        let filtering = !self.filter.read(cx).value().trim().is_empty();
        let theme = cx.theme();
        let muted = theme.muted;
        let muted_foreground = theme.muted_foreground;
        let danger = theme.danger;
        for (root_index, (root, found)) in roots.iter().enumerate() {
            // The folder, with what it holds in total at the end of the line.
            let total: u64 = found.iter().map(|dump| dump.size).sum();
            let matching = self.matching(found, cx);
            let count = match (found.len(), filtering) {
                (count, true) => format!("{} of {count} dumps", matching.len()),
                (1, false) => "1 dump".to_string(),
                (count, false) => format!("{count} dumps"),
            };
            // The folder's checkbox ticks what the filter lets through,
            // also the rows that are not drawn yet.
            let all: Vec<PathBuf> = matching.iter().map(|(_, dump)| dump.path.clone()).collect();
            let all_selected =
                !all.is_empty() && all.iter().all(|path| self.selected.contains(path));
            let collapsed = self.collapsed.contains(root);
            let limit = self.limit(root, cx);
            let toggled = root.clone();
            let mut card = widgets::card(cx).gap_0().p_0().overflow_hidden().child(
                h_flex()
                    .flex_wrap()
                    .items_center()
                    .gap_x_3()
                    .gap_y_1()
                    .px_4()
                    .py_3()
                    .bg(muted)
                    .child(
                        h_flex()
                            .flex_grow(1.)
                            .flex_basis(px(260.))
                            .min_w_0()
                            .items_start()
                            .gap_2()
                            .when(!found.is_empty(), |this| {
                                this.child(
                                    Button::new(("toggle-root", root_index))
                                        .xsmall()
                                        .ghost()
                                        .icon(if collapsed {
                                            IconName::ChevronRight
                                        } else {
                                            IconName::ChevronDown
                                        })
                                        .tooltip(if collapsed {
                                            "Show the dumps"
                                        } else {
                                            "Hide the dumps"
                                        })
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.toggle(&toggled, cx)
                                        })),
                                )
                            })
                            .when(!all.is_empty(), |this| {
                                // Ticks every dump of the folder at once.
                                this.child(
                                    h_flex().h(px(20.)).mr_1().flex_shrink_0().child(
                                        Checkbox::new(("select-root", root_index))
                                            .checked(all_selected)
                                            .on_click(cx.listener(
                                                move |this, checked: &bool, _, cx| {
                                                    this.select(&all, *checked, cx)
                                                },
                                            )),
                                    ),
                                )
                            })
                            .child(
                                h_flex().h(px(20.)).flex_shrink_0().child(
                                    Icon::new(IconName::Folder)
                                        .small()
                                        .text_color(muted_foreground),
                                ),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_sm()
                                    .font_family(theme.mono_font_family.clone())
                                    .child(ctx.show_path(root)),
                            ),
                    )
                    .when(!found.is_empty(), |this| {
                        this.child(widgets::hint(
                            format!("{count} · {}", dumps::human_size(total)),
                            cx,
                        ))
                    }),
            );
            if found.is_empty() {
                card = card.child(
                    div()
                        .px_4()
                        .py_3()
                        .border_t_1()
                        .border_color(border)
                        .child(widgets::hint("No dumps here.", cx)),
                );
            } else if matching.is_empty() && !collapsed {
                card = card.child(
                    div()
                        .px_4()
                        .py_3()
                        .border_t_1()
                        .border_color(border)
                        .child(widgets::hint("No dump here matches the filter.", cx)),
                );
            }
            let drawn = if collapsed { 0 } else { limit };
            for &(index, dump) in matching.iter().take(drawn) {
                let path = dump.path.clone();
                let id = root_index * 10_000 + index;
                let deleting = self.deleting.contains(&dump.path);
                let selected = self.selected.contains(&dump.path);
                let ticked = vec![dump.path.clone()];
                let asked = dump.clone();
                let logged = dump.clone();
                let reading_logs = self.reading_logs.as_ref() == Some(&dump.path);
                card = card.child(
                    // The name takes the room that is left. In a narrow
                    // window the details move under it.
                    h_flex()
                        .flex_wrap()
                        .items_center()
                        .gap_x_4()
                        .gap_y_2()
                        .px_4()
                        .py_3()
                        .border_t_1()
                        .border_color(border)
                        .child(
                            h_flex()
                                .flex_grow(1.)
                                .flex_basis(px(300.))
                                .min_w_0()
                                .gap_3()
                                .child(Checkbox::new(("select", id)).checked(selected).on_click(
                                    cx.listener(move |this, checked: &bool, _, cx| {
                                        this.select(&ticked, *checked, cx)
                                    }),
                                ))
                                .child(widgets::dump_summary(dump, cx).flex_1()),
                        )
                        .child(
                            // A wrapping row: in a narrow window the
                            // buttons go under the details.
                            widgets::wrap_row()
                                .items_center()
                                .gap_x_3()
                                .child(widgets::dump_stats(dump, cx))
                                .child(
                                    h_flex()
                                        .gap_1()
                                        .child(
                                            Button::new(("logs", id))
                                                .small()
                                                .ghost()
                                                .icon(Icon::new(
                                                    gpui_kit::assets::IconName::FileText,
                                                ))
                                                .tooltip("Show the logs of the dump and its loads")
                                                .loading(reading_logs)
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.show_logs(&logged, window, cx)
                                                    },
                                                )),
                                        )
                                        .child(
                                            Button::new(("reveal", id))
                                                .small()
                                                .ghost()
                                                .icon(IconName::FolderOpen)
                                                .tooltip("Show in the file manager")
                                                .on_click(move |_, _, cx| cx.reveal_path(&path)),
                                        )
                                        .child(
                                            Button::new(("delete", id))
                                                .small()
                                                .ghost()
                                                .icon(
                                                    Icon::new(gpui_kit::assets::IconName::Trash)
                                                        .text_color(danger),
                                                )
                                                .tooltip("Delete this dump")
                                                .loading(deleting)
                                                .disabled(busy)
                                                .on_click(cx.listener(
                                                    move |this, _, window, cx| {
                                                        this.request_delete(
                                                            vec![asked.clone()],
                                                            window,
                                                            cx,
                                                        )
                                                    },
                                                )),
                                        ),
                                ),
                        ),
                );
            }
            // The rest of a long folder is drawn a page at a time.
            if !collapsed && matching.len() > limit {
                let more = root.clone();
                card = card.child(
                    widgets::wrap_row()
                        .items_center()
                        .justify_between()
                        .px_4()
                        .py_2()
                        .border_t_1()
                        .border_color(border)
                        .child(widgets::hint(
                            format!("Showing {limit} of {}", matching.len()),
                            cx,
                        ))
                        .child(
                            Button::new(("more", root_index))
                                .small()
                                .outline()
                                .label(format!("Show {} more", PAGE.min(matching.len() - limit)))
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.show_more(&more, cx)),
                                ),
                        ),
                );
            }
            page = page.child(card);
        }
        page
    }
}

/// The logs beside a dump, as the logs dialog shows them: one at a time,
/// with a list to pick from when there are several.
///
/// One at a time, because a log box takes the wheel while the pointer is
/// on it. Several boxes under each other, in a dialog that scrolls too,
/// would leave little room to scroll the dialog from. The list is one that
/// drops down, because a dump can be loaded any number of times: the
/// dialog is as high with thirty logs as with two, and all of the log box
/// is in it.
pub(crate) struct DumpLogs {
    logs: Vec<dumps::DumpLog>,
    /// The log box of each log, made once: it keeps where it is scrolled to.
    pub(crate) views: Vec<Entity<LogView>>,
    /// Which log is on show.
    pub(crate) shown: usize,
    /// The list the log is picked from.
    pub(crate) picker: Entity<SelectState<Vec<LogChoice>>>,
    _observe_picker: Subscription,
}

/// A log in the list of the logs dialog: its title, and its place in
/// [`DumpLogs::logs`]. The place is what tells two logs apart, since two
/// loads within one minute have one title.
#[derive(Clone)]
pub(crate) struct LogChoice {
    index: usize,
    title: SharedString,
}

impl SelectItem for LogChoice {
    type Value = usize;

    fn title(&self) -> SharedString {
        self.title.clone()
    }

    fn value(&self) -> &usize {
        &self.index
    }
}

/// The rows of a log box in the dialog: few enough that the dialog shows
/// all of it in a window of the size the app opens with.
const DIALOG_LOG_ROWS: usize = 12;

impl DumpLogs {
    fn new(logs: Vec<dumps::DumpLog>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let views = logs
            .iter()
            .map(|log| {
                cx.new(|cx| {
                    let mut view = LogView::new(false).rows(DIALOG_LOG_ROWS);
                    view.push(&log.text, cx);
                    view
                })
            })
            .collect();
        let choices: Vec<LogChoice> = logs
            .iter()
            .enumerate()
            .map(|(index, log)| LogChoice {
                index,
                title: log.title.clone().into(),
            })
            .collect();
        let first = (!choices.is_empty()).then(|| IndexPath::new(0));
        let picker = cx.new(|cx| SelectState::new(choices, first, window, cx));
        let _observe_picker = cx.subscribe(&picker, |this, _, event, cx| {
            let SelectEvent::Confirm(picked) = event;
            if let Some(index) = picked {
                this.shown = *index;
                cx.notify();
            }
        });
        DumpLogs {
            logs,
            views,
            shown: 0,
            picker,
            _observe_picker,
        }
    }
}

impl Render for DumpLogs {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(log) = self.logs.get(self.shown) else {
            return widgets::hint(
                "No log beside this dump. A dump or load started by Slipdock writes one.",
                cx,
            );
        };
        let file = log.path.clone();
        v_flex()
            .gap_2()
            .min_w_0()
            .child(
                h_flex()
                    .flex_wrap()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    // The name of the log on show. With several logs it is
                    // the list they are picked from.
                    .child(if self.logs.len() > 1 {
                        div()
                            .flex_1()
                            .min_w(px(240.))
                            .max_w(px(420.))
                            .child(Select::new(&self.picker).accessibility_label("Log"))
                    } else {
                        div()
                            .min_w_0()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(log.title.clone())
                    })
                    .child(
                        Button::new("open-log")
                            .small()
                            .outline()
                            .icon(IconName::ExternalLink)
                            .label("Open file")
                            .on_click(move |_, _, cx| cx.open_with_system(&file)),
                    ),
            )
            .when(log.cut, |this| {
                this.child(widgets::hint(
                    format!(
                        "The end of the log. All of it ({}) is in the file.",
                        dumps::human_size(log.size)
                    ),
                    cx,
                ))
            })
            .child(if log.text.trim().is_empty() {
                widgets::hint("The log is empty.", cx)
            } else {
                div().min_w_0().child(self.views[self.shown].clone())
            })
    }
}
