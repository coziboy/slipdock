//! Walks through the app without a screen: a real window tree in GPUI's
//! headless test platform, a temp config folder, fake tools, fixture dumps.
//!
//! The screens are driven through the same methods their buttons call.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::component::WindowExt as _;
use gpui_kit::test::{TestQueryExt as _, TestWindowExt as _};
use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext, WindowOptions};
use slipdock_core::job::Context;
use slipdock_core::secrets::MemoryStore;
use slipdock_core::settings::Settings;
use slipdock_core::tools::SearchDirs;

use crate::dump_view::DumpView;
use crate::load_view::LoadView;
use crate::shell::{Screen, Shell};
use crate::state::Core;

const PROFILE: &str = "\
[client]
host = db.example.test
port = 3306
user = example_user
password = example-password

[slipdock]
dump-dir = DUMPS
";

fn fake_tool(bin: &Path, name: &str, body: &str) {
    fs::create_dir_all(bin).unwrap();
    let path = bin.join(name);
    let script = format!(
        "#!/bin/sh\nif [ \"$1\" = --version ]; then echo \"{name} v1.0.19-1, built against MariaDB 10.8.8 with SSL support\"; exit 0; fi\n{body}\n"
    );
    fs::write(&path, script).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// A sandbox with one profile, fake mydumper/myloader (no mysql) and one
/// complete dump of `example_db`.
fn sandbox(root: &Path) -> Context {
    let dumps = root.join("dumps");
    let dump = dumps.join("example_db__example__20260101-000000");
    fs::create_dir_all(&dump).unwrap();
    for file in [
        "metadata",
        "example_db-schema-create.sql.zst",
        "example_db.users-schema.sql.zst",
    ] {
        fs::write(dump.join(file), "x").unwrap();
    }
    let profiles = root.join("config/profiles");
    fs::create_dir_all(&profiles).unwrap();
    fs::write(
        profiles.join("example.cnf"),
        PROFILE.replace("DUMPS", dumps.to_str().unwrap()),
    )
    .unwrap();
    fake_tool(&root.join("bin"), "mydumper", "exit 1");
    fake_tool(
        &root.join("bin"),
        "myloader",
        &format!("touch {}/loaded", root.display()),
    );
    Context {
        config_dir: root.join("config"),
        home: Some(root.to_path_buf()),
        settings: Settings::default(),
        dirs: SearchDirs {
            path: vec![root.join("bin")],
            usual: Vec::new(),
        },
        secrets: Arc::new(MemoryStore::default()),
    }
}

/// Opens the real window (root view included) in the headless platform.
fn open(cx: &mut TestAppContext, context: Context) -> (Entity<Shell>, VisualTestContext) {
    let (window, shell) = cx.update(|cx| {
        gpui_kit::init(cx);
        cx.set_global(Core(context));
        gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
            cx.new(|cx| Shell::new(window, cx))
        })
        .unwrap()
    });
    let visual = VisualTestContext::from_window(window, cx);
    visual.run_until_parked();
    (shell, visual)
}

fn temp_root() -> (tempfile::TempDir, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    (temp, root)
}

#[gpui_kit::test]
fn dry_run_dump_shows_the_command(cx: &mut TestAppContext) {
    let (_temp, root) = temp_root();
    let (shell, mut cx) = open(cx, sandbox(&root));
    let dump: Entity<DumpView> = shell.read_with(&cx, |shell, _| shell.dump.clone());

    cx.update(|window, cx| {
        dump.update(cx, |view, cx| {
            view.pick_profile("example".into(), window, cx);
            view.database
                .update(cx, |input, cx| input.set_value("example_db", window, cx));
            view.prepare(true, cx);
        });
    });
    cx.run_until_parked();

    let (summary, command) = dump
        .read_with(&cx, |view, _| view.preview.clone())
        .unwrap()
        .unwrap();
    let summary = slipdock_core::report::plain(&summary);
    assert!(summary.contains("  Database  example_db"), "{summary}");
    assert!(
        command.contains(&format!("{}/bin/mydumper", root.display())),
        "{command}"
    );
    assert!(command.contains("--database example_db"), "{command}");
    assert!(
        dump.read_with(&cx, |view, _| view.run.is_none()),
        "a dry run starts nothing"
    );
}

#[gpui_kit::test]
fn dry_run_load_then_a_confirmed_load(cx: &mut TestAppContext) {
    let (_temp, root) = temp_root();
    let (shell, mut cx) = open(cx, sandbox(&root));
    let load: Entity<LoadView> = shell.read_with(&cx, |shell, _| shell.load.clone());
    cx.update(|_, cx| shell.update(cx, |shell, cx| shell.show(Screen::Load, cx)));

    cx.update(|_, cx| load.update(cx, |view, cx| view.pick_profile("example".into(), cx)));
    cx.run_until_parked();
    assert_eq!(load.read_with(&cx, |view, _| view.dumps.len()), 1);

    // Dry run: the command is shown, nothing is asked.
    cx.update(|window, cx| {
        load.update(cx, |view, cx| {
            view.pick_dump(0, window, cx);
            view.dry_run(cx);
        })
    });
    cx.run_until_parked();
    let (_, command) = load
        .read_with(&cx, |view, _| view.preview.clone())
        .unwrap()
        .unwrap();
    assert!(command.contains("--drop-table DROP"), "{command}");
    assert!(command.contains("--database example_db"), "{command}");

    // Real load: the dialog opens and waits for the name.
    cx.update(|window, cx| load.update(cx, |view, cx| view.request_load(window, cx)));
    cx.run_until_parked();
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
    assert!(load.read_with(&cx, |view, _| view.pending.is_some()));

    // "Start load" is off until the typed name is the target database.
    let start_disabled = |cx: &mut VisualTestContext| {
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.find_by_label("Start load").disabled()
        })
    };
    assert_eq!(start_disabled(&mut cx), Some(true));
    cx.update(|window, cx| {
        load.update(cx, |view, cx| {
            view.confirm
                .update(cx, |input, cx| input.set_value("example_db", window, cx))
        })
    });
    cx.run_until_parked();
    assert_ne!(
        start_disabled(&mut cx),
        Some(true),
        "the right name enables Start"
    );

    // A wrong name starts nothing.
    cx.update(|window, cx| {
        load.update(cx, |view, cx| {
            view.confirm
                .update(cx, |input, cx| input.set_value("wrong", window, cx));
            view.start_confirmed(cx);
        })
    });
    cx.run_until_parked();
    assert!(load.read_with(&cx, |view, _| view.run.is_none()));
    assert!(!root.join("loaded").exists());

    // The right name runs the (fake) myloader and shows the core's report.
    cx.update(|window, cx| load.update(cx, |view, cx| view.request_load(window, cx)));
    cx.run_until_parked();
    cx.update(|window, cx| {
        load.update(cx, |view, cx| {
            view.confirm
                .update(cx, |input, cx| input.set_value("example_db", window, cx));
            view.start_confirmed(cx);
        })
    });
    cx.run_until_parked();
    let run = load
        .read_with(&cx, |view, _| view.run.clone())
        .expect("the run view is shown");
    let report = run
        .read_with(&cx, |view, _| view.report.clone())
        .expect("the run has finished");
    assert!(report.success, "{}", report.plain());
    assert!(report.plain().contains("Load complete"));
    assert!(root.join("loaded").exists());
}

#[gpui_kit::test]
fn every_screen_draws(cx: &mut TestAppContext) {
    let (_temp, root) = temp_root();
    let (shell, mut cx) = open(cx, sandbox(&root));
    for screen in Screen::ALL {
        cx.update(|_, cx| shell.update(cx, |shell, cx| shell.show(screen, cx)));
        // Background loads (tool checks, dump sizes) finish, then a frame
        // is drawn with their results.
        cx.run_until_parked();
        cx.update(|window, cx| window.render_frame(cx));
    }
    // The Profiles form: select the profile and draw again.
    cx.update(|window, cx| {
        let profiles = shell.read(cx).profiles.clone();
        profiles.update(cx, |view, cx| view.select("example", window, cx));
        shell.update(cx, |shell, cx| shell.show(Screen::Profiles, cx));
        window.render_frame(cx);
    });
}

/// Fills in every screen, with a dry run on Dump and Load.
fn fill_every_screen(shell: &Entity<Shell>, cx: &mut VisualTestContext) {
    let (dump, load, profiles) = shell.read_with(cx, |shell, _| {
        (
            shell.dump.clone(),
            shell.load.clone(),
            shell.profiles.clone(),
        )
    });
    cx.update(|window, cx| {
        dump.update(cx, |view, cx| {
            view.pick_profile("example".into(), window, cx);
            view.database
                .update(cx, |input, cx| input.set_value("example_db", window, cx));
            view.prepare(true, cx);
        });
        load.update(cx, |view, cx| view.pick_profile("example".into(), cx));
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        load.update(cx, |view, cx| {
            view.pick_dump(0, window, cx);
            view.dry_run(cx);
        });
        profiles.update(cx, |view, cx| view.select("example", window, cx));
    });
    cx.run_until_parked();
}

/// The cropping guard: at every window size, from the smallest the window
/// allows to a wide one, nothing on any screen reaches past the right edge
/// of the window. (Past the bottom is fine: the page scrolls.)
#[gpui_kit::test]
fn nothing_runs_out_of_the_window(cx: &mut TestAppContext) {
    let (_temp, root) = temp_root();
    // Long names are what used to be cut off.
    let long = "customer_reporting_warehouse__production-replica-eu-west__20260101-000000";
    let context = sandbox(&root);
    let dump = root.join("dumps").join(long);
    fs::create_dir_all(&dump).unwrap();
    fs::write(dump.join("metadata"), "x").unwrap();
    let (shell, mut cx) = open(cx, context);
    fill_every_screen(&shell, &mut cx);

    for width in [520., 640., 759., 760., 1040., 1800.] {
        cx.simulate_resize(gpui_kit::size(gpui_kit::px(width), gpui_kit::px(420.)));
        for screen in Screen::ALL {
            cx.update(|_, cx| shell.update(cx, |shell, cx| shell.show(screen, cx)));
            cx.run_until_parked();
            cx.update(|window, cx| {
                window.render_frame(cx);
                assert_inside(window, width, screen.label());
            });
        }
    }
}

/// Every control drawn in the last frame is between the left and right edge
/// of a window `width` wide.
fn assert_inside(window: &gpui_kit::Window, width: f32, what: &str) {
    let elements = window.elements();
    assert!(elements.len() > 1, "{what}: the screen was observed");
    for element in elements {
        let bounds = element.bounds();
        assert!(
            bounds.left() >= gpui_kit::px(0.) && bounds.right() <= gpui_kit::px(width),
            "{what} at {width} wide: {:?} ({:?}) spans {:?}..{:?}",
            element.label(),
            element.path(),
            bounds.left(),
            bounds.right(),
        );
    }
}

/// The states that replace or cover a form also fit the smallest window:
/// a finished run with its report, the install instructions, and the
/// confirmation dialog of a load.
#[gpui_kit::test]
fn report_install_and_dialog_fit_the_smallest_window(cx: &mut TestAppContext) {
    let (_temp, root) = temp_root();
    let (shell, mut cx) = open(cx, sandbox(&root));
    let (dump, load, tools) = shell.read_with(&cx, |shell, _| {
        (shell.dump.clone(), shell.load.clone(), shell.tools.clone())
    });
    let width = 520.;
    cx.simulate_resize(gpui_kit::size(gpui_kit::px(width), gpui_kit::px(420.)));

    // A real run: the sandbox's mydumper fails at once, so the report shows.
    cx.update(|window, cx| {
        dump.update(cx, |view, cx| {
            view.pick_profile("example".into(), window, cx);
            view.database
                .update(cx, |input, cx| input.set_value("example_db", window, cx));
            view.prepare(false, cx);
        });
    });
    cx.run_until_parked();
    let run = dump
        .read_with(&cx, |view, _| view.run.clone())
        .expect("the run view is shown");
    assert!(run.read_with(&cx, |view, _| view.report.is_some()));
    cx.update(|window, cx| {
        shell.update(cx, |shell, cx| shell.show(Screen::Dump, cx));
        window.render_frame(cx);
        assert!(window.try_find_by_label("Back to the form").is_some());
        assert_inside(window, width, "run report");
    });

    // The sandbox has no mysql, so its install instructions can be shown.
    cx.update(|_, cx| {
        shell.update(cx, |shell, cx| shell.show(Screen::Tools, cx));
        tools.update(cx, |view, cx| {
            view.show_install(slipdock_core::tools::Tool::Mysql, cx)
        });
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.render_frame(cx);
        assert_inside(window, width, "install instructions");
    });

    cx.update(|_, cx| {
        shell.update(cx, |shell, cx| shell.show(Screen::Load, cx));
        load.update(cx, |view, cx| view.pick_profile("example".into(), cx));
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        load.update(cx, |view, cx| {
            view.pick_dump(0, window, cx);
            view.request_load(window, cx);
        })
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        assert!(window.has_active_dialog(cx));
        window.render_frame(cx);
        assert!(window.try_find_by_label("Start load").is_some());
        assert_inside(window, width, "load confirmation");
    });
}

/// On a wide window the page keeps a readable width and sits in the middle
/// of the space beside the navigation.
#[gpui_kit::test]
fn a_wide_window_centres_the_page(cx: &mut TestAppContext) {
    let (_temp, root) = temp_root();
    let (shell, mut cx) = open(cx, sandbox(&root));
    fill_every_screen(&shell, &mut cx);
    cx.update(|_, cx| shell.update(cx, |shell, cx| shell.show(Screen::Dump, cx)));
    cx.simulate_resize(gpui_kit::size(gpui_kit::px(1800.), gpui_kit::px(700.)));
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.render_frame(cx);
        let button = window.find_by_label("Run dump").bounds();
        let input = window.find_by_label("List tables").bounds();
        // Navigation 210, then (1590 - 980) / 2 of free space on each side.
        assert!(button.left() > gpui_kit::px(500.), "{:?}", button.left());
        assert!(input.right() < gpui_kit::px(1500.), "{:?}", input.right());
    });
}

/// A page longer than the window scrolls, in both layouts.
#[gpui_kit::test]
fn a_long_page_scrolls(cx: &mut TestAppContext) {
    let (_temp, root) = temp_root();
    let (shell, mut cx) = open(cx, sandbox(&root));
    fill_every_screen(&shell, &mut cx);
    cx.update(|_, cx| shell.update(cx, |shell, cx| shell.show(Screen::Dump, cx)));

    for width in [520., 1040.] {
        cx.simulate_resize(gpui_kit::size(gpui_kit::px(width), gpui_kit::px(420.)));
        cx.run_until_parked();
        let top = |cx: &mut VisualTestContext| {
            cx.update(|window, cx| {
                window.render_frame(cx);
                window.find_by_label("Run dump").bounds().top()
            })
        };
        let before = top(&mut cx);
        assert!(
            before > gpui_kit::px(420.),
            "the button starts below the window"
        );
        cx.update(|window, cx| {
            window.scroll(
                "content",
                gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
                    gpui_kit::px(0.),
                    gpui_kit::px(-200.),
                )),
                cx,
            )
        });
        assert_eq!(top(&mut cx), before - gpui_kit::px(200.), "at {width} wide");
    }
}

/// The folder button of the profile form asks for one folder and writes it
/// the way a profile stores it: `~` for home, a space left as a space.
#[gpui_kit::test]
fn choosing_a_dump_folder_fills_the_form(cx: &mut TestAppContext) {
    let (_temp, root) = temp_root();
    let (shell, mut cx) = open(cx, sandbox(&root));
    let profiles = shell.read_with(&cx, |shell, _| shell.profiles.clone());
    let text = |cx: &mut VisualTestContext| {
        profiles.read_with(cx, |view, cx| view.dump_dir.read(cx).value().to_string())
    };
    cx.update(|window, cx| {
        profiles.update(cx, |view, cx| {
            view.select("example", window, cx);
            view.choose_dump_dir(window, cx);
        })
    });
    assert!(cx.did_prompt_for_paths());
    cx.simulate_path_prompt_response(|options| {
        assert!(options.directories && !options.files && !options.multiple);
        Some(vec![root.join("DB Dump/mydumper")])
    });
    cx.run_until_parked();
    assert_eq!(text(&mut cx), "~/DB Dump/mydumper");

    // Closing the dialog without a choice leaves the text as it is.
    cx.update(|window, cx| profiles.update(cx, |view, cx| view.choose_dump_dir(window, cx)));
    cx.simulate_path_prompt_response(|_| None);
    cx.run_until_parked();
    assert_eq!(text(&mut cx), "~/DB Dump/mydumper");
}
