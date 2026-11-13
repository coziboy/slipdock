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
use slipdock_core::server::Table;
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

/// A sandbox with one profile, fake mydumper/myloader/mysql and one
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
    fake_tool(&root.join("bin"), "zstd", "");
    // A mysql client whose server answers with one database and no tables.
    let mysql = root.join("bin/mysql");
    let script = r#"#!/bin/sh
case "$1" in
  --defaults-file=*) file="${1#--defaults-file=}" ;;
  *) echo "mysql  Ver 9.7.1 for example on arm64"; exit 0 ;;
esac
if [ "$2" = --print-defaults ]; then sed -n 's/^user=/--user=/p' "$file"; exit 0; fi
case "$5" in "SHOW DATABASES") echo example_db ;; esac
"#;
    fs::write(&mysql, script).unwrap();
    fs::set_permissions(&mysql, fs::Permissions::from_mode(0o755)).unwrap();
    // A myloader that writes one line to the file after `--logfile`.
    fake_tool(
        &root.join("bin"),
        "myloader",
        &format!(
            "before=\"\"\nfor arg in \"$@\"; do\n  if [ \"$before\" = --logfile ]; then echo \"2026-01-01 00:00:00 [INFO] - a line of the fake myloader\" >> \"$arg\"; fi\n  before=\"$arg\"\ndone\ntouch {}/loaded",
            root.display()
        ),
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
        crate::theme::apply(cx);
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
    // The shown command can be pasted and run later: it names the profile
    // itself, not a temporary copy, and carries the log level as an option.
    let profile = root.join("config/profiles/example.cnf");
    assert!(
        command.contains(&format!("--defaults-file {}", profile.display())),
        "{command}"
    );
    assert!(command.contains("--verbose 3"), "{command}");
    assert!(
        dump.read_with(&cx, |view, _| view.run.is_none()),
        "a dry run starts nothing"
    );
}

fn tables(names: &[&str]) -> Vec<(Table, bool)> {
    names
        .iter()
        .map(|name| {
            let table = Table {
                name: name.to_string(),
                is_view: false,
            };
            (table, true)
        })
        .collect()
}

/// The selected tables are dumped: the unticked ones are what mydumper is
/// told to leave out. "Select none" and "Select all" act on the rows the
/// filter shows.
#[gpui_kit::test]
fn unselected_tables_are_left_out(cx: &mut TestAppContext) {
    let (_temp, root) = temp_root();
    let (shell, mut cx) = open(cx, sandbox(&root));
    let dump: Entity<DumpView> = shell.read_with(&cx, |shell, _| shell.dump.clone());
    let excluded =
        |cx: &VisualTestContext| dump.read_with(cx, |view, cx| view.options(true, cx).exclude);

    cx.update(|window, cx| {
        dump.update(cx, |view, cx| {
            view.pick_profile("example".into(), window, cx);
            view.database
                .update(cx, |input, cx| input.set_value("example_db", window, cx));
            view.tables = tables(&["account_plans", "accounts", "users"]);
        });
    });
    assert!(excluded(&cx).is_empty(), "everything is selected at first");

    // With a filter, "none" unticks only the rows that match it.
    cx.update(|window, cx| {
        dump.update(cx, |view, cx| {
            view.filter
                .update(cx, |input, cx| input.set_value("ACCOUNT", window, cx));
            view.set_shown(false, cx);
        });
    });
    assert_eq!(excluded(&cx), ["account_plans", "accounts"]);

    // Without a filter, it unticks all, and a dump of nothing is refused.
    cx.update(|window, cx| {
        dump.update(cx, |view, cx| {
            view.filter
                .update(cx, |input, cx| input.set_value("", window, cx));
            view.set_shown(false, cx);
            view.prepare(true, cx);
        });
    });
    cx.run_until_parked();
    let refused = dump.read_with(&cx, |view, _| view.preview.clone()).unwrap();
    assert_eq!(refused.unwrap_err(), "Select at least one table to dump.");

    // One ticked table: the others reach the command as the omit file.
    cx.update(|_, cx| {
        dump.update(cx, |view, cx| {
            view.tables[2].1 = true;
            view.prepare(true, cx);
        });
    });
    cx.run_until_parked();
    assert_eq!(excluded(&cx), ["account_plans", "accounts"]);
    let (_, command) = dump
        .read_with(&cx, |view, _| view.preview.clone())
        .unwrap()
        .unwrap();
    assert!(command.contains("--omit-from-file"), "{command}");
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
    // The shown command can be pasted and run later: it names the profile
    // itself, not a temporary copy, and carries the log level as an option.
    let profile = root.join("config/profiles/example.cnf");
    assert!(
        command.contains(&format!("--defaults-file {}", profile.display())),
        "{command}"
    );
    assert!(command.contains("--verbose 3"), "{command}");

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
            // A long name, so the table selector is checked too.
            view.tables = tables(&[
                "customer_reporting_warehouse_monthly_account_segment_histories_archive",
                "users",
            ]);
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

/// "Copy command" is drawn on the Dump and on the Load screen, puts the
/// command that is on show on the clipboard, as it can be pasted, and then
/// says that it did.
#[gpui_kit::test]
fn copy_command_copies_the_command_on_show(cx: &mut TestAppContext) {
    let (_temp, root) = temp_root();
    let (shell, mut cx) = open(cx, sandbox(&root));
    fill_every_screen(&shell, &mut cx);
    // High enough that the card under the form is in the window.
    cx.simulate_resize(gpui_kit::size(gpui_kit::px(1040.), gpui_kit::px(3000.)));

    let (dump, load) = shell.read_with(&cx, |shell, _| (shell.dump.clone(), shell.load.clone()));
    let commands = [
        (
            Screen::Dump,
            dump.read_with(&cx, |view, _| view.preview.clone()),
        ),
        (
            Screen::Load,
            load.read_with(&cx, |view, _| view.preview.clone()),
        ),
    ];
    for (screen, preview) in commands {
        let (_, command) = preview.unwrap().unwrap();
        cx.update(|_, cx| shell.update(cx, |shell, cx| shell.show(screen, cx)));
        cx.run_until_parked();
        click_label("Copy command", &mut cx);
        let copied = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .unwrap_or_default();
        assert_eq!(copied, command.trim(), "{}", screen.label());
        assert!(!copied.is_empty());
        cx.update(|window, cx| {
            window.render_frame(cx);
            assert!(
                window.try_find_by_label("Copied").is_some(),
                "{}: the button says that it copied",
                screen.label()
            );
        });
    }
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

    // A tool that is missing brings the install instructions to the Tools
    // screen. With a package manager there (one that does nothing here), a
    // system that can run the install draws its button too.
    fs::remove_file(root.join("bin/mysql")).unwrap();
    fake_tool(&root.join("bin"), "brew", "");
    cx.update(|_, cx| shell.update(cx, |shell, cx| shell.show(Screen::Tools, cx)));
    cx.run_until_parked();
    assert_eq!(tools.read_with(&cx, |view, _| view.installs.len()), 1);
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
        let input = window.find_by_label("Select none").bounds();
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

/// In a window that is drawn up to its top edge (macOS), the strip the system
/// owns takes its place above the page. A full-screen window
/// has no such strip, and neither has a window with a title bar of its own.
#[gpui_kit::test]
fn the_page_starts_below_the_title_strip(cx: &mut TestAppContext) {
    let (_temp, root) = temp_root();
    let (shell, mut cx) = open(cx, sandbox(&root));
    fill_every_screen(&shell, &mut cx);
    let top = |under: bool, cx: &mut VisualTestContext| {
        cx.update(|_, cx| {
            shell.update(cx, |shell, cx| {
                shell.under_title_bar = under;
                shell.show(Screen::Dump, cx);
            })
        });
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.render_frame(cx);
            window.find_by_label("Run dump").bounds().top()
        })
    };

    for width in [520., 1040.] {
        cx.simulate_resize(gpui_kit::size(gpui_kit::px(width), gpui_kit::px(760.)));
        let plain = top(false, &mut cx);
        let under = top(true, &mut cx);
        if width == 1040. {
            // The strip is 40px and the page keeps 8px of its 24px above it.
            assert_eq!(under, plain + gpui_kit::px(24.));
        } else {
            // The compact navigation is laid out anew: the name stands in
            // the strip and the screens in one row below it.
            assert_ne!(under, plain);
        }

        cx.update(|window, _| window.toggle_fullscreen());
        assert_eq!(top(true, &mut cx), plain, "full screen, at {width} wide");
        cx.update(|window, _| window.toggle_fullscreen());
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

/// The folder button of the dump form asks for one folder and uses it for
/// this dump.
#[gpui_kit::test]
fn choosing_an_output_folder_fills_the_dump_form(cx: &mut TestAppContext) {
    let (_temp, root) = temp_root();
    let (shell, mut cx) = open(cx, sandbox(&root));
    let dump: Entity<DumpView> = shell.read_with(&cx, |shell, _| shell.dump.clone());
    let out =
        |cx: &mut VisualTestContext| dump.read_with(cx, |view, cx| view.options(true, cx).out);
    let folder = root.join("DB Dump/other");
    cx.update(|window, cx| {
        dump.update(cx, |view, cx| {
            view.pick_profile("example".into(), window, cx);
            view.choose_out(window, cx);
        })
    });
    assert!(cx.did_prompt_for_paths());
    cx.simulate_path_prompt_response(|options| {
        assert!(options.directories && !options.files && !options.multiple);
        Some(vec![folder.clone()])
    });
    cx.run_until_parked();
    assert_eq!(out(&mut cx), Some(folder.clone()));

    // Closing the dialog without a choice leaves the folder as it is.
    cx.update(|window, cx| dump.update(cx, |view, cx| view.choose_out(window, cx)));
    cx.simulate_path_prompt_response(|_| None);
    cx.run_until_parked();
    assert_eq!(out(&mut cx), Some(folder));
}

/// The unticked tables are written to the settings as they change, and come
/// back unticked when the same database is listed again. A table that is
/// new since then is ticked.
#[gpui_kit::test]
fn the_table_selection_is_remembered(cx: &mut TestAppContext) {
    let (_temp, root) = temp_root();
    let (shell, mut cx) = open(cx, sandbox(&root));
    let dump: Entity<DumpView> = shell.read_with(&cx, |shell, _| shell.dump.clone());
    let table = |name: &str| Table {
        name: name.to_string(),
        is_view: false,
    };
    let list = |names: &[&str]| names.iter().map(|name| table(name)).collect::<Vec<_>>();
    let saved = || {
        let settings = Settings::load(&root.join("config")).unwrap();
        settings.left_out("example", "example_db").to_vec()
    };

    cx.update(|window, cx| {
        dump.update(cx, |view, cx| {
            view.pick_profile("example".into(), window, cx);
            view.show_tables(
                "example_db".into(),
                list(&["accounts", "jobs", "users"]),
                cx,
            );
            view.set_table(1, false, cx);
        });
    });
    assert_eq!(saved(), ["jobs"]);

    // Another database of the same profile starts with everything ticked.
    cx.update(|window, cx| {
        dump.update(cx, |view, cx| {
            view.database
                .update(cx, |input, cx| input.set_value("other_db", window, cx));
            view.show_tables("other_db".into(), list(&["jobs"]), cx);
            assert!(view.options(true, cx).exclude.is_empty());
        });
    });

    // Back on the first one, as after a restart of the app.
    cx.update(|window, cx| {
        dump.update(cx, |view, cx| {
            view.pick_profile("example".into(), window, cx);
            view.database
                .update(cx, |input, cx| input.set_value("example_db", window, cx));
            view.show_tables(
                "example_db".into(),
                list(&["accounts", "jobs", "new_table", "users"]),
                cx,
            );
            assert_eq!(view.options(true, cx).exclude, ["jobs"]);
            // With another name in the field the list is of another
            // database: nothing of it is left out.
            view.database
                .update(cx, |input, cx| input.set_value("third_db", window, cx));
            assert!(view.tables_stale(cx));
            assert!(view.options(true, cx).exclude.is_empty());
            view.database
                .update(cx, |input, cx| input.set_value("example_db", window, cx));
            assert!(!view.tables_stale(cx));
            // Everything ticked again: nothing is kept in the settings.
            view.set_shown(true, cx);
        });
    });
    assert!(saved().is_empty());
}

/// Deleting asks first, then removes the ticked dumps and their list
/// entries; a dump that is not ticked stays.
#[gpui_kit::test]
fn ticked_dumps_are_deleted_after_a_confirmation(cx: &mut TestAppContext) {
    let (_temp, root) = temp_root();
    let context = sandbox(&root);
    let folder = |stamp: &str| root.join(format!("dumps/example_db__example__{stamp}"));
    for stamp in ["20260102-000000", "20260103-000000"] {
        fs::create_dir_all(folder(stamp)).unwrap();
        fs::write(folder(stamp).join("metadata"), "x").unwrap();
    }
    let (shell, mut cx) = open(cx, context);
    let dumps = shell.read_with(&cx, |shell, _| shell.dumps.clone());
    cx.run_until_parked();
    let listed = |cx: &mut VisualTestContext| {
        dumps.read_with(cx, |view, _| view.roots.clone().unwrap()[0].1.clone())
    };
    assert_eq!(listed(&mut cx).len(), 3);
    let ticked = vec![folder("20260101-000000"), folder("20260103-000000")];

    // "Delete selected" only opens the dialog: nothing is deleted yet.
    cx.update(|window, cx| {
        dumps.update(cx, |view, cx| {
            view.select(&ticked, true, cx);
            let selected = view.selected_dumps();
            assert_eq!(selected.len(), 2);
            view.request_delete(selected, window, cx);
        });
    });
    cx.run_until_parked();
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
    assert!(ticked.iter().all(|path| path.is_dir()));

    // The button in the dialog calls this.
    cx.update(|window, cx| {
        window.close_dialog(cx);
        dumps.update(cx, |view, cx| view.delete(ticked.clone(), cx));
    });
    cx.run_until_parked();
    assert!(ticked.iter().all(|path| !path.exists()));
    let left = listed(&mut cx);
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].path, folder("20260102-000000"));
    assert_eq!(dumps.read_with(&cx, |view, _| view.error.clone()), None);
    assert!(
        dumps.read_with(&cx, |view, _| view.selected_dumps().is_empty()),
        "a deleted dump is no longer ticked"
    );
}

/// An icon whose file is not in the app is drawn as an empty space. Every
/// icon the screens take from the full Lucide set must be in `AppAssets`.
#[test]
fn the_extra_icons_are_in_the_app() {
    use gpui_kit::AssetSource as _;
    use gpui_kit::assets::IconName;
    for icon in [IconName::Database, IconName::FileText, IconName::Trash] {
        let path = icon.path();
        let found = crate::AppAssets.load(&path).unwrap();
        assert!(found.is_some(), "{path}");
    }
    // The icons of the components still come through.
    assert!(crate::AppAssets.load("icons/check.svg").unwrap().is_some());
}

/// "Test connection" asks the server with what the form shows and says what
/// came back; "Delete" removes the profile and what was remembered for it.
#[gpui_kit::test]
fn a_profile_is_tested_and_deleted(cx: &mut TestAppContext) {
    let (_temp, root) = temp_root();
    let mut context = sandbox(&root);
    context
        .settings
        .set_left_out("example", "example_db", vec!["jobs".into()]);
    // A mysql that knows one query, and fails when the form's host is not
    // in the defaults file it is given.
    // Written by hand: `fake_tool` answers `--version` like mydumper.
    let mysql = root.join("bin/mysql");
    let script = r#"#!/bin/sh
case "$1" in
  --defaults-file=*) file="${1#--defaults-file=}" ;;
  *) echo "mysql  Ver 9.7.1 for example on arm64"; exit 0 ;;
esac
if [ "$2" = --print-defaults ]; then sed -n 's/^user=/--user=/p' "$file"; exit 0; fi
grep -q typed.example.test "$file" || { echo "ERROR 2005: Unknown host" >&2; exit 1; }
echo 8.0.36
"#;
    fs::write(&mysql, script).unwrap();
    fs::set_permissions(&mysql, fs::Permissions::from_mode(0o755)).unwrap();
    let (shell, mut cx) = open(cx, context);
    let profiles = shell.read_with(&cx, |shell, _| shell.profiles.clone());
    let message =
        |cx: &mut VisualTestContext| profiles.read_with(cx, |view, _| view.message.clone());

    // The saved host is refused; the server's reason is shown.
    cx.update(|window, cx| {
        profiles.update(cx, |view, cx| {
            view.select("example", window, cx);
            view.test_connection(cx);
        })
    });
    cx.run_until_parked();
    let (ok, text) = message(&mut cx).unwrap();
    assert!(!ok && text.contains("Unknown host"), "{text}");

    // A host typed in the form is tested without a save.
    cx.update(|window, cx| {
        profiles.update(cx, |view, cx| {
            view.host.update(cx, |input, cx| {
                input.set_value("typed.example.test", window, cx)
            });
            view.test_connection(cx);
        })
    });
    cx.run_until_parked();
    let (ok, text) = message(&mut cx).unwrap();
    assert!(ok, "{text}");
    assert_eq!(
        text,
        "Connected to example_user@typed.example.test:3306. Server version 8.0.36."
    );
    let file = root.join("config/profiles/example.cnf");
    assert!(
        fs::read_to_string(&file)
            .unwrap()
            .contains("db.example.test")
    );

    // The button in the confirmation dialog calls this.
    cx.update(|window, cx| profiles.update(cx, |view, cx| view.delete("example", window, cx)));
    assert!(!file.exists());
    assert_eq!(message(&mut cx), Some((true, "Deleted example.".into())));
    let settings = Settings::load(&root.join("config")).unwrap();
    assert!(settings.left_out("example", "example_db").is_empty());
}

/// A dump of one profile can be loaded into another: the dumps are listed
/// from the profile chosen for them, the load goes to the one loaded into.
#[gpui_kit::test]
fn a_dump_of_another_profile_can_be_loaded(cx: &mut TestAppContext) {
    let (_temp, root) = temp_root();
    let context = sandbox(&root);
    // A second profile with its own, empty dump folder.
    fs::create_dir_all(root.join("local-dumps")).unwrap();
    fs::write(
        root.join("config/profiles/local.cnf"),
        PROFILE
            .replace("db.example.test", "127.0.0.1")
            .replace("DUMPS", root.join("local-dumps").to_str().unwrap()),
    )
    .unwrap();
    let (shell, mut cx) = open(cx, context);
    let load: Entity<LoadView> = shell.read_with(&cx, |shell, _| shell.load.clone());
    let from =
        |cx: &mut VisualTestContext| load.read_with(cx, |view, _| view.dumps_from().cloned());

    // The dumps follow the profile that is loaded into.
    cx.update(|_, cx| load.update(cx, |view, cx| view.pick_profile("local".into(), cx)));
    cx.run_until_parked();
    assert_eq!(from(&mut cx).as_deref(), Some("local"));
    assert_eq!(load.read_with(&cx, |view, _| view.dumps.len()), 0);

    // Until another profile is chosen for them.
    cx.update(|_, cx| load.update(cx, |view, cx| view.pick_source("example".into(), cx)));
    cx.run_until_parked();
    assert_eq!(load.read_with(&cx, |view, _| view.dumps.len()), 1);
    cx.update(|window, cx| {
        load.update(cx, |view, cx| {
            view.pick_dump(0, window, cx);
            view.dry_run(cx);
        });
    });
    cx.run_until_parked();
    let (summary, _) = load
        .read_with(&cx, |view, _| view.preview.clone())
        .unwrap()
        .unwrap();
    let summary = slipdock_core::report::plain(&summary);
    assert!(summary.contains("  Profile   local"), "{summary}");
    assert!(
        summary.contains("example_db__example__20260101-000000"),
        "{summary}"
    );

    // Loading into the profile the dumps are of is the plain case again.
    cx.update(|_, cx| load.update(cx, |view, cx| view.pick_profile("example".into(), cx)));
    cx.run_until_parked();
    assert_eq!(from(&mut cx).as_deref(), Some("example"));
    assert_eq!(load.read_with(&cx, |view, _| view.dumps.len()), 1);
}

/// The target database of a load is picked from the databases of the server
/// that is loaded into; the list follows that profile.
#[gpui_kit::test]
fn the_target_databases_come_from_the_profile_loaded_into(cx: &mut TestAppContext) {
    let (_temp, root) = temp_root();
    let context = sandbox(&root);
    fs::write(
        root.join("config/profiles/local.cnf"),
        PROFILE
            .replace("db.example.test", "127.0.0.1")
            .replace("DUMPS", root.join("dumps").to_str().unwrap()),
    )
    .unwrap();
    // A mysql whose databases depend on the host in its defaults file.
    let mysql = root.join("bin/mysql");
    let script = r#"#!/bin/sh
case "$1" in
  --defaults-file=*) file="${1#--defaults-file=}" ;;
  *) echo "mysql  Ver 9.7.1 for example on arm64"; exit 0 ;;
esac
if [ "$2" = --print-defaults ]; then sed -n 's/^user=/--user=/p' "$file"; exit 0; fi
if grep -q 127.0.0.1 "$file"; then printf 'local_a\nlocal_b\nsys\n'; else printf 'example_db\n'; fi
"#;
    fs::write(&mysql, script).unwrap();
    fs::set_permissions(&mysql, fs::Permissions::from_mode(0o755)).unwrap();
    let (shell, mut cx) = open(cx, context);
    let load: Entity<LoadView> = shell.read_with(&cx, |shell, _| shell.load.clone());
    let databases =
        |cx: &mut VisualTestContext| load.read_with(cx, |view, _| view.databases.clone());

    cx.update(|_, cx| load.update(cx, |view, cx| view.pick_profile("example".into(), cx)));
    cx.run_until_parked();
    assert_eq!(databases(&mut cx), ["example_db"]);

    // Another profile to load into: its own databases, without the
    // server's system ones.
    cx.update(|_, cx| load.update(cx, |view, cx| view.pick_profile("local".into(), cx)));
    cx.run_until_parked();
    assert_eq!(databases(&mut cx), ["local_a", "local_b"]);

    // The profile the dumps are of does not change the list.
    cx.update(|_, cx| load.update(cx, |view, cx| view.pick_source("example".into(), cx)));
    cx.run_until_parked();
    assert_eq!(databases(&mut cx), ["local_a", "local_b"]);
}

/// A folder with many dumps is drawn a page at a time, and the filter
/// narrows it by name or by date.
#[gpui_kit::test]
fn a_long_dump_folder_is_paged_and_filtered(cx: &mut TestAppContext) {
    use crate::dumps_view::PAGE;
    let (_temp, root) = temp_root();
    let context = sandbox(&root);
    // 60 more dumps, taken a minute apart on 2 March 2026.
    for minute in 0..60 {
        let dump = root.join(format!(
            "dumps/example_db__example__20260302-10{minute:02}00"
        ));
        fs::create_dir_all(&dump).unwrap();
        fs::write(dump.join("metadata"), "x").unwrap();
    }
    let (shell, mut cx) = open(cx, context);
    let dumps = shell.read_with(&cx, |shell, _| shell.dumps.clone());
    cx.update(|_, cx| shell.update(cx, |shell, cx| shell.show(Screen::Dumps, cx)));
    cx.run_until_parked();
    let folder = root.join("dumps");
    let matching = |cx: &mut VisualTestContext| {
        dumps.read_with(cx, |view, cx| {
            let found = &view.roots.as_ref().unwrap()[0].1;
            view.matching(found, cx).len()
        })
    };
    assert_eq!(matching(&mut cx), 61);

    // One page at first, one more with each "Show more"; at every width the
    // rows and the row under them stay inside the window.
    assert_eq!(
        dumps.read_with(&cx, |view, cx| view.limit(&folder, cx)),
        PAGE
    );
    cx.update(|_, cx| dumps.update(cx, |view, cx| view.show_more(&folder, cx)));
    assert_eq!(
        dumps.read_with(&cx, |view, cx| view.limit(&folder, cx)),
        2 * PAGE
    );
    for width in [520., 1040.] {
        cx.simulate_resize(gpui_kit::size(gpui_kit::px(width), gpui_kit::px(420.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.render_frame(cx);
            assert_inside(window, width, "Dumps with many dumps");
        });
    }

    // The filter looks at the name and at the date as it is shown, and
    // starts at the first page again.
    let filter = |text: &str, cx: &mut VisualTestContext| {
        cx.update(|window, cx| {
            dumps.update(cx, |view, cx| {
                view.filter
                    .update(cx, |input, cx| input.set_value(text, window, cx));
            })
        });
        cx.run_until_parked();
    };
    filter("20260302-1005", &mut cx);
    assert_eq!(matching(&mut cx), 1);
    assert_eq!(
        dumps.read_with(&cx, |view, cx| view.limit(&folder, cx)),
        PAGE
    );
    filter("2 mar 2026", &mut cx);
    assert_eq!(matching(&mut cx), 60);
    filter("1 Jan 2026", &mut cx);
    assert_eq!(matching(&mut cx), 1);
    filter("nothing like this", &mut cx);
    assert_eq!(matching(&mut cx), 0);
}

/// The screen opens on the first profile, and "Save" knows whether the form
/// differs from the file.
#[gpui_kit::test]
fn the_profile_form_knows_its_unsaved_changes(cx: &mut TestAppContext) {
    let (_temp, root) = temp_root();
    let (shell, mut cx) = open(cx, sandbox(&root));
    let profiles = shell.read_with(&cx, |shell, _| shell.profiles.clone());
    let changed = |cx: &mut VisualTestContext| {
        let ctx = cx.read(crate::state::core);
        let profile = ctx.profile("example").unwrap();
        profiles.read_with(cx, |view, cx| view.has_changes(&profile, cx))
    };
    // No click was needed: the first profile is in the form.
    assert_eq!(
        profiles.read_with(&cx, |view, cx| view.host.read(cx).value().to_string()),
        "db.example.test"
    );
    assert!(!changed(&mut cx));

    cx.update(|window, cx| {
        profiles.update(cx, |view, cx| {
            view.host.update(cx, |input, cx| {
                input.set_value("other.example.test", window, cx)
            });
        })
    });
    assert!(changed(&mut cx));

    cx.update(|window, cx| profiles.update(cx, |view, cx| view.save(window, cx)));
    assert!(
        !changed(&mut cx),
        "saved: the form and the file agree again"
    );
    let file = fs::read_to_string(root.join("config/profiles/example.cnf")).unwrap();
    assert!(file.contains("other.example.test"), "{file}");
}

/// "New profile" asks for the name in a dialog. A name that cannot be used
/// keeps the dialog open; a good one closes it and opens the new profile.
#[gpui_kit::test]
fn a_new_profile_is_named_in_a_dialog(cx: &mut TestAppContext) {
    let (_temp, root) = temp_root();
    let (shell, mut cx) = open(cx, sandbox(&root));
    let profiles = shell.read_with(&cx, |shell, _| shell.profiles.clone());
    let create = |name: &str, cx: &mut VisualTestContext| {
        cx.update(|window, cx| {
            profiles.update(cx, |view, cx| {
                view.new_name
                    .update(cx, |input, cx| input.set_value(name, window, cx));
                view.create(window, cx);
            })
        });
        cx.run_until_parked();
    };

    cx.update(|window, cx| profiles.update(cx, |view, cx| view.open_new(window, cx)));
    cx.run_until_parked();
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));

    create("not a name", &mut cx);
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
    let error = profiles
        .read_with(&cx, |view, _| view.new_error.clone())
        .unwrap();
    assert!(error.contains("not a usable profile name"), "{error}");
    // The existing one is not overwritten either.
    create("example", &mut cx);
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));

    create("staging", &mut cx);
    assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
    assert!(root.join("config/profiles/staging.cnf").is_file());
    assert_eq!(
        profiles.read_with(&cx, |view, _| view.message.clone()),
        Some((true, "Created staging. Fill in the form and save.".into()))
    );
}

/// When the server of the chosen profile does not answer, Dump and Load
/// show why instead of the rest of the form, and fit the window doing so.
#[gpui_kit::test]
fn a_failed_connection_takes_the_place_of_the_form(cx: &mut TestAppContext) {
    let (_temp, root) = temp_root();
    // Without a mysql client, asking the server fails.
    let context = sandbox(&root);
    fs::remove_file(root.join("bin/mysql")).unwrap();
    let (shell, mut cx) = open(cx, context);
    let (dump, load) = shell.read_with(&cx, |shell, _| (shell.dump.clone(), shell.load.clone()));
    cx.update(|window, cx| {
        dump.update(cx, |view, cx| {
            view.pick_profile("example".into(), window, cx)
        });
        load.update(cx, |view, cx| view.pick_profile("example".into(), cx));
    });
    cx.run_until_parked();
    let dump_error = dump
        .read_with(&cx, |view, _| view.connect_error.clone())
        .unwrap();
    assert!(dump_error.contains("mysql"), "{dump_error}");
    assert!(load.read_with(&cx, |view, _| view.connect_error.is_some()));

    for screen in [Screen::Dump, Screen::Load] {
        cx.update(|_, cx| shell.update(cx, |shell, cx| shell.show(screen, cx)));
        for width in [520., 1040.] {
            cx.simulate_resize(gpui_kit::size(gpui_kit::px(width), gpui_kit::px(420.)));
            cx.run_until_parked();
            cx.update(|window, cx| {
                window.render_frame(cx);
                assert_inside(window, width, "a failed connection");
            });
        }
    }

    // "Try again" asks the server once more; the error is there again.
    cx.update(|_, cx| dump.update(cx, |view, cx| view.list_databases(cx)));
    assert!(dump.read_with(&cx, |view, _| view.connect_error.is_none()));
    cx.run_until_parked();
    assert!(dump.read_with(&cx, |view, _| view.connect_error.is_some()));
}

/// Every profile has a band once the window is open, a dump shows the band
/// of the profile in its name, and the band is in the settings file.
#[gpui_kit::test]
fn a_profile_has_a_band(cx: &mut TestAppContext) {
    let (_temp, root) = temp_root();
    let (_shell, mut cx) = open(cx, sandbox(&root));
    cx.update(|_, cx| {
        let band = crate::theme::band("example", cx);
        assert_eq!(band, Some(crate::theme::color(crate::theme::BANDS[0])));
        assert_eq!(crate::theme::band("gone", cx), None);
        // The components draw from Slipdock's theme, not the library's.
        use crate::theme::{INK, PANEL, RAIL, color};
        use gpui_kit::component::ActiveTheme as _;
        let theme = cx.theme();
        assert_eq!(theme.background, color(PANEL));
        assert_eq!(theme.primary, color(INK));
        assert_eq!(theme.button_primary, color(INK));
        assert_eq!(theme.sidebar, color(RAIL));
        assert_eq!(theme.radius, crate::theme::RADIUS);
        assert!(!theme.shadow);
    });
    let saved = fs::read_to_string(root.join("config/settings.toml")).unwrap();
    assert!(saved.contains("example = 0"), "{saved}");
}

/// The database list follows the typed name, without regard to case or to
/// spaces around it.
#[test]
fn the_database_list_is_narrowed_by_the_typed_name() {
    use crate::widgets::matching_databases;
    let databases: Vec<String> = (0..2000)
        .map(|number| format!("customer_{number:04}"))
        .chain(["Sales_Goal".to_string(), "sales_goal_testing".to_string()])
        .collect();
    assert_eq!(matching_databases(&databases, "").len(), 2002);
    assert_eq!(
        matching_databases(&databases, " sales_goal "),
        ["Sales_Goal", "sales_goal_testing"]
    );
    assert_eq!(matching_databases(&databases, "_19").len(), 100);
    assert!(matching_databases(&databases, "nothing like this").is_empty());
}

/// The app always asks for a full log, and the Dumps screen shows the logs
/// beside a dump in a dialog that fits the window.
#[gpui_kit::test]
fn the_logs_of_a_dump_are_shown(cx: &mut TestAppContext) {
    let (_temp, root) = temp_root();
    let context = sandbox(&root);
    let long_line = "** Message: a line of a log that is long enough to need wrapping ".repeat(6);
    fs::write(
        root.join("dumps/example_db__example__20260101-000000.log"),
        format!("{long_line}\nFinished dump\n"),
    )
    .unwrap();
    let (shell, mut cx) = open(cx, context);
    let (dump, dumps) = shell.read_with(&cx, |shell, _| (shell.dump.clone(), shell.dumps.clone()));

    // Every dump the app starts logs at the most detailed level.
    cx.update(|window, cx| {
        dump.update(cx, |view, cx| {
            view.pick_profile("example".into(), window, cx)
        })
    });
    cx.run_until_parked();
    assert_eq!(
        dump.read_with(&cx, |view, cx| view.options(false, cx).log_level),
        Some(3)
    );

    cx.update(|_, cx| shell.update(cx, |shell, cx| shell.show(Screen::Dumps, cx)));
    cx.run_until_parked();
    let listed = dumps.read_with(&cx, |view, _| view.roots.clone().unwrap()[0].1[0].clone());
    cx.update(|window, cx| dumps.update(cx, |view, cx| view.show_logs(&listed, window, cx)));
    cx.run_until_parked();
    assert!(cx.update(|window, cx| window.has_active_dialog(cx)));
    for width in [520., 1040.] {
        cx.simulate_resize(gpui_kit::size(gpui_kit::px(width), gpui_kit::px(420.)));
        cx.run_until_parked();
        cx.update(|window, cx| {
            window.render_frame(cx);
            assert_inside(window, width, "the logs dialog");
        });
    }
}

/// The arguments of a tool are set in its card, go to the settings file
/// for that tool only, and text that cannot be used is refused.
#[gpui_kit::test]
fn the_arguments_of_a_tool_are_saved_and_used(cx: &mut TestAppContext) {
    use slipdock_core::tools::Tool;
    let (_temp, root) = temp_root();
    let (shell, mut cx) = open(cx, sandbox(&root));
    let (tools, dump) = shell.read_with(&cx, |shell, _| (shell.tools.clone(), shell.dump.clone()));
    let save = |tool: Tool, text: &str, cx: &mut VisualTestContext| {
        cx.update(|window, cx| {
            tools.update(cx, |view, cx| {
                let setting = view.settings.iter().find(|s| s.tool == tool).unwrap();
                setting
                    .input
                    .clone()
                    .update(cx, |input, cx| input.set_value(text, window, cx));
                view.save_settings(tool, window, cx);
            })
        });
        let (of, ok, text) = tools.read_with(cx, |view, _| view.settings_message.clone().unwrap());
        assert_eq!(of, tool);
        (ok, text)
    };
    let saved = || {
        let options = Settings::load(&root.join("config")).unwrap().options;
        (options.mydumper.arguments, options.myloader.arguments)
    };
    assert!(!tools.read_with(&cx, |view, cx| view.settings_changed(Tool::Mydumper, cx)));

    let (ok, text) = save(Tool::Mydumper, "-t 12", &mut cx);
    assert!(!ok && text.contains("long form"), "{text}");
    assert_eq!(
        saved(),
        (String::new(), String::new()),
        "nothing is written"
    );

    let (ok, _) = save(Tool::Mydumper, " --threads 12 --routines ", &mut cx);
    assert!(ok);
    assert_eq!(saved().0, "--threads 12 --routines");
    assert!(!tools.read_with(&cx, |view, cx| view.settings_changed(Tool::Mydumper, cx)));
    let (ok, _) = save(Tool::Myloader, "--threads 6", &mut cx);
    assert!(ok);
    assert_eq!(saved().1, "--threads 6");

    // "Show command" of a dump names the profile and carries mydumper's
    // arguments, not myloader's.
    cx.update(|window, cx| {
        dump.update(cx, |view, cx| {
            view.pick_profile("example".into(), window, cx);
            view.database
                .update(cx, |input, cx| input.set_value("example_db", window, cx));
            view.prepare(true, cx);
        });
    });
    cx.run_until_parked();
    let (_, command) = dump
        .read_with(&cx, |view, _| view.preview.clone())
        .unwrap()
        .unwrap();
    assert!(command.contains("--threads 12"), "{command}");
    assert!(command.contains("--routines"), "{command}");
    assert!(command.contains("profiles/example.cnf"), "{command}");

    // Empty again: nothing is added. The other tool keeps its own.
    let (ok, _) = save(Tool::Mydumper, "", &mut cx);
    assert!(ok);
    assert_eq!(saved(), (String::new(), "--threads 6".to_string()));
}

/// Sends a wheel event to what is at `position`. A negative `delta`
/// scrolls down, as in GPUI.
fn wheel(position: gpui_kit::Point<gpui_kit::Pixels>, delta: f32, cx: &mut VisualTestContext) {
    cx.simulate_mouse_move(position, None, gpui_kit::Modifiers::default());
    cx.simulate_event(gpui_kit::ScrollEvent {
        position,
        delta: gpui_kit::ScrollDelta::Pixels(gpui_kit::point(
            gpui_kit::px(0.),
            gpui_kit::px(delta),
        )),
        ..Default::default()
    });
    cx.run_until_parked();
}

/// Clicks the middle of the one thing with this label.
fn click_label(label: &'static str, cx: &mut VisualTestContext) {
    let position = bounds_of(label, cx).center();
    cx.simulate_click(position, gpui_kit::Modifiers::default());
    cx.run_until_parked();
}

/// Opens the logs dialog of the one dump of the sandbox, which has a long
/// dump log and `loads` load logs beside it.
fn open_logs_dialog(
    loads: usize,
    cx: &mut TestAppContext,
) -> (
    tempfile::TempDir,
    Entity<crate::dumps_view::DumpLogs>,
    VisualTestContext,
) {
    let (temp, root) = temp_root();
    let context = sandbox(&root);
    let long: String = (0..2000).map(|line| format!("line {line}\n")).collect();
    let dump = "dumps/example_db__example__20260101-000000";
    fs::write(root.join(format!("{dump}.log")), &long).unwrap();
    for load in 0..loads {
        fs::write(
            root.join(format!(
                "{dump}__load_example_202601{:02}-000000.log",
                load + 2
            )),
            format!("load {load}\n"),
        )
        .unwrap();
    }
    let (shell, mut cx) = open(cx, context);
    let dumps = shell.read_with(&cx, |shell, _| shell.dumps.clone());
    cx.update(|_, cx| shell.update(cx, |shell, cx| shell.show(Screen::Dumps, cx)));
    cx.run_until_parked();
    let listed = dumps.read_with(&cx, |view, _| view.roots.clone().unwrap()[0].1[0].clone());
    cx.update(|window, cx| dumps.update(cx, |view, cx| view.show_logs(&listed, window, cx)));
    cx.run_until_parked();
    cx.update(|window, cx| window.render_frame(cx));
    let logs = dumps
        .read_with(&cx, |view, _| view.logs_shown.clone())
        .and_then(|logs| logs.upgrade())
        .expect("the dialog is open");
    (temp, logs, cx)
}

/// The logs dialog shows one log at a time, all of its box at once, and
/// the box scrolls through all that was read of the log.
#[gpui_kit::test]
fn the_logs_dialog_shows_a_long_log_in_a_box_that_scrolls(cx: &mut TestAppContext) {
    let (_temp, logs, mut cx) = open_logs_dialog(1, cx);
    let view = logs.read_with(&cx, |logs, _| logs.views[0].clone());
    cx.run_until_parked();
    cx.update(|window, cx| window.render_frame(cx));

    // The box is above the Close button: nothing of it is cut off.
    let bounds = view.read_with(&cx, |view, cx| view.bounds(cx));
    // (The dialog has two of them: the cross at its top is the other.)
    let close = cx.update(|window, _| {
        let tops = window.find_all_by_label("Close");
        tops.iter()
            .map(|close| close.bounds().top())
            .fold(gpui_kit::px(0.), |a, b| a.max(b))
    });
    assert!(bounds.bottom() < close, "{bounds:?} {close:?}");

    // All that was read is there, not only the last lines, and the box
    // starts at the end.
    let text = view.read_with(&cx, |view, cx| view.text(cx));
    assert!(text.starts_with("line 0\n") && text.ends_with("line 1999"));
    let (end, at_end) = view.read_with(&cx, |view, cx| view.scroll(cx));
    assert!(at_end);

    // The wheel on the text scrolls the box, and a button goes back.
    wheel(bounds.center(), 300., &mut cx);
    let (offset, at_end) = view.read_with(&cx, |view, cx| view.scroll(cx));
    assert_eq!(offset, end + gpui_kit::px(300.));
    assert!(!at_end);
    cx.update(|window, cx| {
        window.render_frame(cx);
        assert!(window.find_by_label("Go to end").visible());
    });

    // The other log is picked from the list that drops down.
    click_label("Log", &mut cx);
    cx.simulate_keystrokes("down enter");
    cx.run_until_parked();
    assert_eq!(logs.read_with(&cx, |logs, _| logs.shown), 1);
    cx.update(|window, cx| window.render_frame(cx));
    let other = logs.read_with(&cx, |logs, _| logs.views[1].clone());
    assert_eq!(other.read_with(&cx, |view, cx| view.text(cx)), "load 0");
}

/// Where the one thing with this label is drawn, after a fresh frame.
fn bounds_of(
    label: &'static str,
    cx: &mut VisualTestContext,
) -> gpui_kit::Bounds<gpui_kit::Pixels> {
    cx.update(|window, cx| {
        window.render_frame(cx);
        window.find_by_label(label).bounds()
    })
}

/// In a window of the size the app opens with, the logs dialog of a dump
/// with this many load logs shows all of the log box: the box ends above
/// the Close button, the dialog ends in the window, and the dialog has
/// nothing to scroll (so nothing of it is cut off).
fn the_log_box_is_whole(loads: usize, cx: &mut TestAppContext) {
    let (_temp, logs, mut cx) = open_logs_dialog(loads, cx);
    cx.simulate_resize(gpui_kit::size(gpui_kit::px(1040.), gpui_kit::px(760.)));
    cx.run_until_parked();
    cx.update(|window, cx| window.render_frame(cx));
    let view = logs.read_with(&cx, |logs, _| logs.views[0].clone());
    let bounds = view.read_with(&cx, |view, cx| view.bounds(cx));
    let (close_top, close_bottom, window_height) = cx.update(|window, _| {
        let close = window
            .find_all_by_label("Close")
            .iter()
            .map(|close| close.bounds())
            .fold(None, |low: Option<gpui_kit::Bounds<_>>, next| match low {
                Some(low) if low.top() > next.top() => Some(low),
                _ => Some(next),
            })
            .unwrap();
        (close.top(), close.bottom(), window.viewport_size().height)
    });
    assert!(bounds.bottom() < close_top, "{bounds:?} {close_top:?}");
    assert!(close_bottom < window_height);

    // The wheel off the log box moves nothing.
    let before = bounds_of("Open file", &mut cx);
    wheel(before.center(), -60., &mut cx);
    assert_eq!(bounds_of("Open file", &mut cx), before);
}

#[gpui_kit::test]
fn the_logs_dialog_shows_all_of_the_box_with_a_few_loads(cx: &mut TestAppContext) {
    the_log_box_is_whole(3, cx);
}

#[gpui_kit::test]
fn the_logs_dialog_shows_all_of_the_box_with_some_loads(cx: &mut TestAppContext) {
    the_log_box_is_whole(7, cx);
}

#[gpui_kit::test]
fn the_logs_dialog_shows_all_of_the_box_with_many_loads(cx: &mut TestAppContext) {
    the_log_box_is_whole(28, cx);
}

/// In a window too low for the logs dialog, the dialog scrolls with the
/// wheel, and the page behind it does not.
#[gpui_kit::test]
fn the_logs_dialog_scrolls_in_a_low_window(cx: &mut TestAppContext) {
    let (_temp, _logs, mut cx) = open_logs_dialog(1, cx);
    // Lower than the app lets its window get: low enough that the page
    // behind the dialog has something to scroll too.
    cx.simulate_resize(gpui_kit::size(gpui_kit::px(1040.), gpui_kit::px(260.)));
    cx.run_until_parked();
    let before = bounds_of("Open file", &mut cx);
    let behind = bounds_of("Refresh", &mut cx);
    wheel(before.center(), -20., &mut cx);
    assert_eq!(
        bounds_of("Open file", &mut cx).top(),
        before.top() - gpui_kit::px(20.)
    );
    assert_eq!(bounds_of("Refresh", &mut cx), behind);

    // At its end the dialog has nothing more to scroll, and the wheel
    // still does not go through to the page.
    wheel(before.center(), -5000., &mut cx);
    let at_end = bounds_of("Open file", &mut cx);
    wheel(before.center(), -20., &mut cx);
    assert_eq!(bounds_of("Open file", &mut cx), at_end);
    assert_eq!(bounds_of("Refresh", &mut cx), behind);

    // Without the dialog, the same wheel scrolls the page.
    cx.update(|window, cx| window.close_dialog(cx));
    cx.run_until_parked();
    wheel(before.center(), -20., &mut cx);
    assert_eq!(
        bounds_of("Refresh", &mut cx).top(),
        behind.top() - gpui_kit::px(20.)
    );
}

/// The log box of a run holds every line that was read, keeps its place
/// while lines are added when it is scrolled back, and follows the newest
/// line again after "Follow".
#[gpui_kit::test]
fn the_log_of_a_run_stays_where_it_is_read(cx: &mut TestAppContext) {
    let (_temp, root) = temp_root();
    let (shell, mut cx) = open(cx, sandbox(&root));
    let load: Entity<LoadView> = shell.read_with(&cx, |shell, _| shell.load.clone());
    cx.update(|_, cx| shell.update(cx, |shell, cx| shell.show(Screen::Load, cx)));
    cx.update(|_, cx| load.update(cx, |view, cx| view.pick_profile("example".into(), cx)));
    cx.run_until_parked();
    cx.update(|window, cx| {
        load.update(cx, |view, cx| {
            view.pick_dump(0, window, cx);
            view.request_load(window, cx);
        })
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        load.update(cx, |view, cx| {
            view.confirm
                .update(cx, |input, cx| input.set_value("example_db", window, cx));
            view.start_confirmed(cx);
        });
        // The button that confirms closes the dialog; here nothing was
        // clicked, and an open dialog would take the wheel.
        window.close_dialog(cx);
    });
    cx.run_until_parked();
    assert!(!cx.update(|window, cx| window.has_active_dialog(cx)));
    let run = load
        .read_with(&cx, |view, _| view.run.clone())
        .expect("the run view is shown");
    let log = run.read_with(&cx, |view, _| view.log_view.clone());
    let lines = |range: std::ops::Range<usize>| -> String {
        range.map(|line| format!("line {line}\n")).collect()
    };
    let draw = |cx: &mut VisualTestContext| {
        cx.run_until_parked();
        cx.update(|window, cx| window.render_frame(cx));
        cx.run_until_parked();
    };

    // What the tool wrote to its log is in the box.
    assert!(
        log.read_with(&cx, |view, cx| view.text(cx))
            .contains("a line of the fake myloader")
    );

    // Far more lines than the box shows: all of them are kept.
    cx.update(|_, cx| {
        run.update(cx, |view, cx| {
            view.log_open = true;
            cx.notify();
        });
        log.update(cx, |view, cx| view.push(&lines(0..1000), cx));
    });
    draw(&mut cx);
    let text = log.read_with(&cx, |view, cx| view.text(cx));
    assert!(text.contains("\nline 0\n") && text.ends_with("line 999"));
    let (_, at_end) = log.read_with(&cx, |view, cx| view.scroll(cx));
    assert!(at_end, "the box starts at the newest line");

    // Followed, the box moves with the lines that are added.
    cx.update(|_, cx| log.update(cx, |view, cx| view.push(&lines(1000..1010), cx)));
    draw(&mut cx);
    let (_, at_end) = log.read_with(&cx, |view, cx| view.scroll(cx));
    assert!(at_end, "the box follows the newest line");

    // The page is scrolled to the box, then the box is scrolled back.
    let window_height = cx.update(|window, _| window.viewport_size().height);
    let bounds = log.read_with(&cx, |view, cx| view.bounds(cx));
    if bounds.bottom() > window_height {
        let by = bounds.bottom() - window_height + gpui_kit::px(20.);
        cx.update(|window, cx| {
            window.scroll(
                "content",
                gpui_kit::ScrollDelta::Pixels(gpui_kit::point(gpui_kit::px(0.), -by)),
                cx,
            )
        });
        draw(&mut cx);
    }
    // The cursor is at the end of the text, as after a click there. Adding
    // text moves it, which must not take the box along.
    cx.update(|_, cx| log.update(cx, |view, cx| view.cursor_to_end(cx)));
    draw(&mut cx);
    let bounds = log.read_with(&cx, |view, cx| view.bounds(cx));
    wheel(bounds.center(), 300., &mut cx);
    draw(&mut cx);
    assert_eq!(
        log.read_with(&cx, |view, cx| view.bounds(cx)),
        bounds,
        "the button that appears does not move the box"
    );
    let (scrolled_back, at_end) = log.read_with(&cx, |view, cx| view.scroll(cx));
    assert!(!at_end);
    assert!(!log.read_with(&cx, |view, _| view.follow));

    // Lines that are added do not move it.
    for (start, count) in [(1010, 10), (1020, 1), (1021, 9)] {
        cx.update(|_, cx| log.update(cx, |view, cx| view.push(&lines(start..start + count), cx)));
        draw(&mut cx);
        let (offset, at_end) = log.read_with(&cx, |view, cx| view.scroll(cx));
        assert_eq!(offset, scrolled_back);
        assert!(!at_end);
        assert!(!log.read_with(&cx, |view, _| view.follow));
    }
    assert!(
        log.read_with(&cx, |view, cx| view.text(cx))
            .ends_with("line 1029")
    );

    // The button goes back to the newest line, and the box follows again.
    cx.update(|window, cx| {
        window.render_frame(cx);
        let button = window.find_by_label("Go to end");
        assert!(button.visible());
    });
    cx.update(|_, cx| log.update(cx, |view, cx| view.go_to_end(cx)));
    draw(&mut cx);
    cx.update(|_, cx| log.update(cx, |view, cx| view.push(&lines(1030..1040), cx)));
    draw(&mut cx);
    let (_, at_end) = log.read_with(&cx, |view, cx| view.scroll(cx));
    assert!(at_end);
    assert!(log.read_with(&cx, |view, _| view.follow));
}

/// After a failed load, all the tool printed is in a box of the report:
/// whole lines that can be copied, from the first one, and not a second
/// time in the log box.
#[gpui_kit::test]
fn a_failed_load_shows_all_the_tool_printed(cx: &mut TestAppContext) {
    let (_temp, root) = temp_root();
    let ctx = sandbox(&root);
    // Longer than the 200 characters the command line report keeps.
    let cause = format!("ERROR 1822: {} in the referenced table", "x".repeat(300));
    let body = format!(
        "before=\"\"\nfor arg in \"$@\"; do\n  if [ \"$before\" = --logfile ]; then echo \"2026-01-01 00:00:00 [INFO] - a line of the fake myloader\" >> \"$arg\"; fi\n  before=\"$arg\"\ndone\n\
         echo '** (myloader:1): CRITICAL **: 15:01:33.381: {cause}' >&2\n\
         for i in $(seq 1 40); do echo \"  column $i\" >&2; done\nexit 1"
    );
    fake_tool(&root.join("bin"), "myloader", &body);
    let (shell, mut cx) = open(cx, ctx);
    let load: Entity<LoadView> = shell.read_with(&cx, |shell, _| shell.load.clone());
    cx.update(|_, cx| shell.update(cx, |shell, cx| shell.show(Screen::Load, cx)));
    cx.update(|_, cx| load.update(cx, |view, cx| view.pick_profile("example".into(), cx)));
    cx.run_until_parked();
    cx.update(|window, cx| {
        load.update(cx, |view, cx| {
            view.pick_dump(0, window, cx);
            view.request_load(window, cx);
        })
    });
    cx.run_until_parked();
    cx.update(|window, cx| {
        load.update(cx, |view, cx| {
            view.confirm
                .update(cx, |input, cx| input.set_value("example_db", window, cx));
            view.start_confirmed(cx);
        });
        window.close_dialog(cx);
    });
    cx.run_until_parked();
    let run = load
        .read_with(&cx, |view, _| view.run.clone())
        .expect("the run view is shown");
    let report = run
        .read_with(&cx, |view, _| view.report.clone())
        .expect("the run has finished");
    assert!(!report.success);
    cx.update(|_, cx| {
        run.update(cx, |view, cx| {
            view.log_open = true;
            cx.notify();
        })
    });
    cx.update(|window, cx| window.render_frame(cx));
    cx.run_until_parked();
    cx.update(|window, cx| window.render_frame(cx));

    let output = run
        .read_with(&cx, |view, _| view.tool_output.clone())
        .expect("the report has a box with the tool output");
    let text = output.read_with(&cx, |view, cx| view.text(cx));
    assert!(
        text.starts_with(&format!("CRITICAL: {cause}\n  column 1\n")),
        "{text}"
    );
    assert!(text.ends_with("  column 40"), "{text}");
    // More lines than the box is high: it shows the first, where the cause is.
    let (offset, at_end) = output.read_with(&cx, |view, cx| view.scroll(cx));
    assert_eq!(offset, gpui_kit::px(0.));
    assert!(!at_end);

    // The log box has the log of the tool, and not its output again.
    let log = run.read_with(&cx, |view, _| view.log_view.clone());
    let log_text = log.read_with(&cx, |view, cx| view.text(cx));
    assert!(
        log_text.contains("a line of the fake myloader"),
        "{log_text}"
    );
    assert!(!log_text.contains("column 1"), "{log_text}");

    // A box that starts at its first line has no button to its end.
    cx.update(|window, _| assert!(window.try_find_by_label("Go to end").is_none()));

    // In a narrow window the report and its box stay inside it.
    let width = 520.;
    cx.simulate_resize(gpui_kit::size(gpui_kit::px(width), gpui_kit::px(420.)));
    cx.update(|window, cx| {
        window.render_frame(cx);
        assert_inside(window, width, "failed load report");
    });
}

/// mydumper and myloader come in one package, so they are one install. It is
/// offered at the top of the Tools screen, inside the window, and its button
/// runs the package manager of the search folders, never one from `PATH`.
#[gpui_kit::test]
fn missing_tools_are_installed_from_the_top_of_the_tools_screen(cx: &mut TestAppContext) {
    use slipdock_core::tools::{InstallPlan, Tool};
    let (_temp, root) = temp_root();
    let context = sandbox(&root);
    // A package manager that notes how it was called and "installs" by
    // putting the two tools back.
    let (bin, stash, ran) = (root.join("bin"), root.join("stash"), root.join("brew-ran"));
    fs::create_dir_all(&stash).unwrap();
    for tool in ["mydumper", "myloader"] {
        fs::rename(bin.join(tool), stash.join(tool)).unwrap();
    }
    fake_tool(
        &bin,
        "brew",
        &format!(
            "echo \"$@\" > {}\nmv {}/* {}/",
            ran.display(),
            stash.display(),
            bin.display()
        ),
    );
    let (shell, mut cx) = open(cx, context);
    let tools = shell.read_with(&cx, |shell, _| shell.tools.clone());
    let height = 760.;
    cx.simulate_resize(gpui_kit::size(gpui_kit::px(1040.), gpui_kit::px(height)));
    cx.update(|_, cx| shell.update(cx, |shell, cx| shell.show(Screen::Tools, cx)));
    cx.run_until_parked();

    let installs = tools.read_with(&cx, |view, _| view.installs.clone());
    assert_eq!(installs.len(), 1, "one install for both tools");
    assert_eq!(installs[0].tools, [Tool::Mydumper, Tool::Myloader]);

    // Which plan there is depends on the system the test runs on.
    if !matches!(installs[0].plan, InstallPlan::Command { .. }) {
        // Steps for the user: there is nothing to press and nothing runs.
        cx.update(|window, cx| {
            window.render_frame(cx);
            assert!(window.try_find_by_label("Install").is_none());
        });
        tools.update(&mut cx, |view, cx| view.run_install(0, cx));
        cx.run_until_parked();
        assert!(!ran.exists());
        return;
    }
    cx.update(|window, cx| {
        window.render_frame(cx);
        let button = window
            .try_find_by_label("Install")
            .expect("the install button is drawn");
        assert!(
            button.bounds().bottom() <= gpui_kit::px(height),
            "the install button is inside the window: {:?}",
            button.bounds()
        );
    });
    tools.update(&mut cx, |view, cx| view.run_install(0, cx));
    cx.run_until_parked();
    assert_eq!(fs::read_to_string(&ran).unwrap().trim(), "install mydumper");
    let (installs, message) =
        tools.read_with(&cx, |view, _| (view.installs.clone(), view.message.clone()));
    assert!(installs.is_empty(), "both tools are found again");
    let (tone, text) = message.expect("the result is shown");
    assert!(tone == crate::widgets::Tone::Success, "{text}");
    assert_eq!(text, "Installed mydumper and myloader.");

    // Another check keeps the cards on show while it runs.
    tools.update(&mut cx, |view, cx| {
        view.reload(cx);
        assert!(view.statuses.is_some());
    });
    cx.run_until_parked();
}
