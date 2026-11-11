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
