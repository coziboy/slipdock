//! `slipdock-app`, the desktop app.
//!
//! Built with GPUI (through the `gpui-kit` crate). The app is a thin layer:
//! every rule about profiles, tools, commands and results is in
//! `slipdock-core`, shared with the command line tool.
//!
//! Where to read about each GPUI idea, in the order they build on each other:
//!
//! | Idea                      | File            |
//! | ------------------------- | --------------- |
//! | application, window       | `main.rs`       |
//! | global state              | `state.rs`      |
//! | the theme                 | `theme.rs`      |
//! | entity, context, render, actions | `shell.rs` |
//! | elements as values, layout that follows the window | `widgets.rs` |
//! | background tasks          | `run_view.rs`, `tools_view.rs` |
//! | drawing only what is on show | `log_view.rs` |
//! | text inputs               | `profiles_view.rs` |
//! | events, dialogs           | `load_view.rs`  |

mod dump_view;
mod dumps_view;
mod load_view;
mod log_view;
mod profiles_view;
mod run_view;
mod shell;
mod state;
mod theme;
mod tools_view;
mod widgets;

#[cfg(test)]
mod tests;

use gpui_kit::*;
use slipdock_core::{job, platform};

use shell::Shell;
use state::Core;

// Actions are plain types. This macro declares one per name; keys are bound
// to them below, and views handle them with `.on_action(...)`.
actions!(
    slipdock,
    [Quit, ShowDump, ShowLoad, ShowDumps, ShowProfiles, ShowTools]
);

// gpui-kit's own `Assets` holds only the icons its components draw. An icon
// from the full Lucide set (`gpui_kit::assets::IconName`) is drawn as an
// empty space unless its file is in the app, so each one used is named here.
gpui_kit::assets::icon_assets!(ExtraIcons, [Database, FileText, Trash]);

/// The icons of the components, plus the extra ones above.
pub(crate) struct AppAssets;

impl AssetSource for AppAssets {
    fn load(&self, path: &str) -> Result<Option<std::borrow::Cow<'static, [u8]>>> {
        match ExtraIcons.load(path)? {
            Some(bytes) => Ok(Some(bytes)),
            None => gpui_kit::assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = gpui_kit::assets::Assets.list(path)?;
        paths.extend(ExtraIcons.list(path)?);
        Ok(paths)
    }
}

fn main() {
    // The same context the command line tool builds: config folder (honours
    // SLIPDOCK_CONFIG_DIR), settings, tool search folders, keychain.
    let context = match job::Context::system(None) {
        Ok(context) => context,
        Err(error) => {
            eprintln!("slipdock-app: {error}");
            std::process::exit(1);
        }
    };

    // `run` starts the event loop and calls the closure once it is ready.
    // `cx` is the `App`: the owner of all state, windows and entities.
    // The assets are the icons that are drawn (check marks, spinners).
    let application = gpui_kit::application().with_assets(AppAssets);
    application.run(move |cx| {
        gpui_kit::init(cx);
        theme::apply(cx);
        // Scroll bars stay on show, not only while scrolling: a log box
        // should say at a glance that there is more above or below.
        gpui_kit::component::Theme::global_mut(cx).scrollbar_mode =
            gpui_kit::component::scroll::ScrollbarMode::Always;
        cx.set_global(Core(context));

        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.bind_keys([
            KeyBinding::new("cmd-q", Quit, None),
            KeyBinding::new("cmd-1", ShowDump, None),
            KeyBinding::new("cmd-2", ShowLoad, None),
            KeyBinding::new("cmd-3", ShowDumps, None),
            KeyBinding::new("cmd-4", ShowProfiles, None),
            KeyBinding::new("cmd-5", ShowTools, None),
        ]);
        // One window: closing it ends the app.
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let under_title_bar = platform::draws_under_title_bar(platform::current_os());
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                None,
                size(px(1040.), px(760.)),
                cx,
            ))),
            // Every screen fits in this; below it the window stops shrinking.
            window_min_size: Some(size(px(520.), px(420.))),
            // On macOS the window is drawn up to its top edge and the three
            // window buttons sit on the rail (see `shell::TITLE_STRIP`). The
            // title is not drawn then, but Mission Control and the Window
            // menu still show it. Dragging the top of the window stays the
            // system's job.
            titlebar: Some(TitlebarOptions {
                title: Some("Slipdock".into()),
                appears_transparent: under_title_bar,
                traffic_light_position: under_title_bar.then_some(shell::WINDOW_BUTTONS),
            }),
            ..Default::default()
        };
        // The closure builds the window's root view: an entity that
        // implements `Render`.
        gpui_kit::open_window(options, cx, |window, cx| {
            cx.new(|cx| Shell::new(window, cx))
        })
        .expect("failed to open the window");
        cx.activate(true);
    });
}
