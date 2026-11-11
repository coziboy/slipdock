//! Runs the real `slipdock` binary against a temp config folder, fake tools
//! and fixture dump folders. Nothing here talks to a database.
#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use assert_cmd::Command;
use predicates::prelude::*;
use predicates::str::contains;

const PROFILE: &str = "\
[client]
host = db.example.test
port = 3306
user = example_user
password = example-password

[mydumper]
verbose = 1

[slipdock]
tls-verify = FALSE
dump-dir = DUMPS
";

/// A temp folder with a config dir, one profile, fake tools and a dump root.
struct Sandbox {
    root: PathBuf,
    _temp: tempfile::TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let sandbox = Sandbox { root, _temp: temp };
        let profiles = sandbox.config().join("profiles");
        fs::create_dir_all(&profiles).unwrap();
        let dumps = sandbox.dumps();
        fs::write(
            profiles.join("example.cnf"),
            PROFILE.replace("DUMPS", dumps.to_str().unwrap()),
        )
        .unwrap();
        sandbox.fake_tool("mydumper", "");
        sandbox.fake_tool("myloader", "");
        sandbox.fake_tool("zstd", "");
        // Tool paths are fixed in the settings, so the tests never find the
        // tools installed on this machine.
        fs::write(
            sandbox.config().join("settings.toml"),
            format!(
                "[tools.mydumper]\nsource = \"path\"\npath = \"{0}/bin/mydumper\"\n\n\
                 [tools.myloader]\nsource = \"path\"\npath = \"{0}/bin/myloader\"\n\n\
                 [tools.zstd]\nsource = \"path\"\npath = \"{0}/bin/zstd\"\n",
                sandbox.root.display()
            ),
        )
        .unwrap();
        sandbox
    }

    fn config(&self) -> PathBuf {
        self.root.join("config")
    }

    fn dumps(&self) -> PathBuf {
        self.root.join("dumps")
    }

    /// A stand-in tool: answers --version like the real one, otherwise runs `body`.
    fn fake_tool(&self, name: &str, body: &str) {
        let bin = self.root.join("bin");
        fs::create_dir_all(&bin).unwrap();
        let path = bin.join(name);
        let script = format!(
            "#!/bin/sh\nif [ \"$1\" = --version ]; then echo \"{name} v1.0.19-1, built against MariaDB 10.8.8 with SSL support\"; exit 0; fi\n{body}\n"
        );
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// A finished dump of `example_db` with two tables.
    fn dump_fixture(&self, name: &str, complete: bool) -> PathBuf {
        let dir = self.dumps().join(name);
        fs::create_dir_all(&dir).unwrap();
        for file in [
            "example_db-schema-create.sql.zst",
            "example_db.users-schema.sql.zst",
            "example_db.orders-schema.sql.zst",
        ] {
            fs::write(dir.join(file), "x").unwrap();
        }
        if complete {
            fs::write(dir.join("metadata"), "x").unwrap();
        }
        dir
    }

    fn slipdock(&self) -> Command {
        let mut command = Command::cargo_bin("slipdock").unwrap();
        command
            .env_clear()
            .env("SLIPDOCK_CONFIG_DIR", self.config())
            .env("HOME", &self.root)
            // The fake zstd is on PATH, as a real one is in a shell, so
            // the tools get no PATH of their own.
            .env("PATH", self.path());
        command
    }

    fn path(&self) -> String {
        format!("{}/bin:/usr/bin:/bin", self.root.display())
    }
}

fn stdout(command: &mut Command) -> String {
    let output = command.assert().success().get_output().stdout.clone();
    String::from_utf8(output).unwrap()
}

/// Dump names contain the time; replace it so output can be compared whole.
fn fix_time(text: &str) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(at) = rest.find("__20") {
        let stamp = &rest[at + 2..];
        let is_stamp = stamp.len() >= 15
            && stamp.as_bytes()[8] == b'-'
            && stamp[..8].bytes().all(|b| b.is_ascii_digit())
            && stamp[9..15].bytes().all(|b| b.is_ascii_digit());
        if is_stamp {
            out.push_str(&rest[..at + 2]);
            out.push_str("TIME");
            rest = &stamp[15..];
        } else {
            out.push_str(&rest[..at + 2]);
            rest = stamp;
        }
    }
    out.push_str(rest);
    out
}

#[test]
fn without_a_terminal_and_without_a_command_it_prints_help() {
    let sandbox = Sandbox::new();
    sandbox
        .slipdock()
        .assert()
        .success()
        .stdout(contains("Usage: slipdock").and(contains("guided mode")));
}

#[test]
fn init_then_profiles() {
    let sandbox = Sandbox::new();
    sandbox
        .slipdock()
        .args(["init", "staging"])
        .assert()
        .success()
        .stdout(contains(
            "(mode 600). Edit it, then: slipdock dump staging <db>",
        ));
    let file = sandbox.config().join("profiles/staging.cnf");
    assert_eq!(
        fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o600
    );

    sandbox
        .slipdock()
        .args(["init", "staging"])
        .assert()
        .failure()
        .stderr(contains("already exists"));

    let listed = stdout(sandbox.slipdock().arg("profiles"));
    assert_eq!(
        listed,
        "example                example_user@db.example.test:3306  (TLS cert check off)\n\
         staging                root@127.0.0.1:3306\n"
    );
}

#[test]
fn config_dir_option_wins_over_the_environment() {
    let sandbox = Sandbox::new();
    let other = sandbox.root.join("other-config");
    sandbox
        .slipdock()
        .args(["--config-dir", other.to_str().unwrap(), "init", "elsewhere"])
        .assert()
        .success();
    assert!(other.join("profiles/elsewhere.cnf").is_file());
    assert_eq!(
        fs::metadata(&other).unwrap().permissions().mode() & 0o777,
        0o700
    );
}

#[test]
fn dump_dry_run_shows_summary_and_command_and_runs_nothing() {
    let sandbox = Sandbox::new();
    // If the fake were started for the dump, it would leave this file.
    sandbox.fake_tool(
        "mydumper",
        &format!("touch {}/started", sandbox.root.display()),
    );

    let out = stdout(sandbox.slipdock().args([
        "dump",
        "example",
        "example_db",
        "-x",
        "logs, audit",
        "--exclude",
        "other_db.jobs",
        "--dry-run",
        "--",
        "--threads",
        "2",
    ]));
    let root = sandbox.root.display();
    let expected = format!(
        "
slipdock dump
  Profile   example  example_user@db.example.test:3306
  Database  example_db
  Exclude   logs, audit, other_db.jobs
  Into      ~/dumps/
            example_db__example__TIME
  Note      TLS certificate check is off (encrypted, server not verified)

  env MARIADB_TLS_DISABLE_PEER_VERIFICATION=1 {root}/bin/mydumper \\
      --defaults-file {root}/config/profiles/example.cnf \\
      --omit-from-file {root}/dumps/example_db__example__TIME.omit \\
      --database example_db \\
      --outputdir {root}/dumps/example_db__example__TIME \\
      --compress ZSTD \\
      --logfile {root}/dumps/example_db__example__TIME.log \\
      --threads 2
(dry run, nothing executed)
"
    );
    assert_eq!(fix_time(&out), expected);
    assert!(!sandbox.root.join("started").exists());

    // The omit file is written also on a dry run, so the command can be pasted.
    let omit: Vec<_> = fs::read_dir(sandbox.dumps())
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    assert_eq!(omit.len(), 1);
    assert_eq!(
        fs::read_to_string(&omit[0]).unwrap(),
        "example_db.logs\nexample_db.audit\nother_db.jobs\n"
    );
}

#[test]
fn dump_log_level_uses_a_temporary_copy_and_validates_the_range() {
    let sandbox = Sandbox::new();
    let out = stdout(sandbox.slipdock().args([
        "dump",
        "example",
        "example_db",
        "--log-level",
        "3",
        "--dry-run",
    ]));
    assert!(out.contains("  Log level 3 (this run only)\n"), "{out}");
    assert!(
        !out.contains("profiles/example.cnf"),
        "a copy is used: {out}"
    );
    assert!(out.contains("--defaults-file "));

    sandbox
        .slipdock()
        .args([
            "dump",
            "example",
            "example_db",
            "--log-level",
            "4",
            "--dry-run",
        ])
        .assert()
        .failure()
        .stderr(contains("--log-level"));
}

#[test]
fn dump_errors_name_the_fix() {
    let sandbox = Sandbox::new();
    sandbox
        .slipdock()
        .args(["dump", "nope", "example_db", "--dry-run"])
        .assert()
        .code(1)
        .stderr(contains("slipdock: no profile 'nope'").and(contains("slipdock init nope")));

    let profile = sandbox.config().join("profiles/example.cnf");
    let text = fs::read_to_string(&profile).unwrap();
    let without: String = text
        .lines()
        .filter(|l| !l.starts_with("dump-dir"))
        .map(|l| format!("{l}\n"))
        .collect();
    fs::write(&profile, without).unwrap();
    sandbox
        .slipdock()
        .args(["dump", "example", "example_db", "--dry-run"])
        .assert()
        .code(1)
        .stderr(contains("has no dump-dir").and(contains("--out <folder>")));
    // --out makes up for it.
    let out = sandbox.root.join("elsewhere");
    sandbox
        .slipdock()
        .args([
            "dump",
            "example",
            "example_db",
            "--dry-run",
            "--out",
            out.to_str().unwrap(),
        ])
        .assert()
        .success();
}

#[test]
fn a_real_dump_with_a_fake_mydumper_reports_by_the_metadata_file() {
    let sandbox = Sandbox::new();
    let body = r#"
while [ $# -gt 0 ]; do
  case "$1" in --outputdir) out="$2"; shift ;; esac
  shift
done
mkdir -p "$out"
echo x > "$out/example_db-schema-create.sql.zst"
echo x > "$out/example_db.users-schema.sql.zst"
echo x > "$out/metadata"
[ "$MARIADB_TLS_DISABLE_PEER_VERIFICATION" = 1 ] || exit 9
exit 1"#;
    sandbox.fake_tool("mydumper", body);

    // Exit 1 with a metadata file: complete, with a warning. Exit code 0.
    let assert = sandbox
        .slipdock()
        .args(["dump", "example", "example_db", "--show-command"])
        .assert()
        .success();
    let output = assert.get_output();
    let out = String::from_utf8_lossy(&output.stdout);
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        out.contains("/bin/mydumper \\\n"),
        "--show-command prints the command: {out}"
    );
    assert!(out.contains("✓ Dump complete  0:00 · 1 tables · "), "{out}");
    assert!(
        out.contains("! mydumper exited with 1, but the dump is complete. It reported:"),
        "{out}"
    );
    assert!(
        out.contains("Nothing recorded: this profile logs errors only (verbose = 1)."),
        "{out}"
    );
    assert!(
        !out.contains('\x1b'),
        "no colour codes when not on a terminal"
    );
    assert!(err.is_empty(), "{err}");
}

#[test]
fn an_unfinished_dump_fails_and_keeps_the_partial_folder() {
    let sandbox = Sandbox::new();
    sandbox.fake_tool("mydumper", "echo 'could not connect' >&2; exit 1");
    sandbox
        .slipdock()
        .args(["dump", "example", "example_db"])
        .assert()
        .code(1)
        .stderr(
            contains("│ could not connect")
                .and(contains("✗ Dump did not finish (exit 1) after 0:00"))
                .and(contains("Partial   ~/dumps/example_db__example__")),
        );
}

#[test]
fn load_dry_run_shows_summary_and_command_without_asking() {
    let sandbox = Sandbox::new();
    let dir = sandbox.dump_fixture("example_db__example__20260101-000000", true);
    let out =
        stdout(
            sandbox
                .slipdock()
                .args(["load", "example", dir.to_str().unwrap(), "--dry-run"]),
        );
    let root = sandbox.root.display();
    let expected = format!(
        "
slipdock load
  Profile   example  example_user@db.example.test:3306
  Dump      example_db__example__TIME  2 tables · 4B
  In        ~/dumps/
  Target    example_db  tables in the dump are dropped and recreated
  Note      TLS certificate check is off (encrypted, server not verified)

  env MARIADB_TLS_DISABLE_PEER_VERIFICATION=1 {root}/bin/myloader \\
      --defaults-file {root}/config/profiles/example.cnf \\
      --directory {root}/dumps/example_db__example__TIME \\
      --database example_db \\
      --drop-table DROP \\
      --logfile {root}/dumps/example_db__example__TIME__load_example_TIME2.log
(dry run, nothing executed)
"
    );
    // The log name carries a second timestamp, after "_example_".
    let out = fix_time(&out);
    let at = out.find("__load_example_").unwrap() + "__load_example_".len();
    let out = format!("{}TIME2{}", &out[..at], &out[at + 15..]);
    assert_eq!(out, expected);
}

#[test]
fn load_needs_confirmation_and_there_is_no_way_around_it() {
    let sandbox = Sandbox::new();
    let dir = sandbox.dump_fixture("d", true);
    let dir = dir.to_str().unwrap();
    sandbox.fake_tool(
        "myloader",
        &format!("touch {}/loaded", sandbox.root.display()),
    );
    let loaded = sandbox.root.join("loaded");

    // No terminal, no --confirm: refuse.
    sandbox
        .slipdock()
        .args(["load", "example", dir])
        .assert()
        .code(1)
        .stderr(
            contains("refusing to load without confirmation").and(contains("--confirm example_db")),
        );
    // --confirm must be the *target*, also when the target is given.
    sandbox
        .slipdock()
        .args(["load", "example", dir, "copy_db", "--confirm", "example_db"])
        .assert()
        .code(1)
        .stderr(contains(
            "--confirm example_db does not match the target database copy_db",
        ));
    sandbox
        .slipdock()
        .args(["load", "example", dir, "--yes"])
        .assert()
        .code(2);
    assert!(!loaded.exists(), "myloader was never started");

    sandbox
        .slipdock()
        .args(["load", "example", dir, "copy_db", "--confirm", "copy_db"])
        .assert()
        .success()
        .stdout(contains("✓ Load complete  0:00 · 2 tables into copy_db"));
    assert!(loaded.exists());
}

#[test]
fn a_failed_load_shows_the_tool_output_and_the_log() {
    let sandbox = Sandbox::new();
    let dir = sandbox.dump_fixture("d", true);
    sandbox.fake_tool("myloader", "echo 'Access denied' >&2; exit 1");
    sandbox
        .slipdock()
        .args([
            "load",
            "example",
            dir.to_str().unwrap(),
            "--confirm",
            "example_db",
        ])
        .assert()
        .code(1)
        .stderr(
            contains("│ Access denied")
                .and(contains("✗ Load failed (exit 1) after 0:00"))
                .and(contains("Log       ~/dumps/d__load_example_")),
        );
}

#[test]
fn load_rejects_folders_that_are_not_finished_dumps() {
    let sandbox = Sandbox::new();
    let partial = sandbox.dump_fixture("partial", false);
    sandbox
        .slipdock()
        .args(["load", "example", partial.to_str().unwrap(), "--dry-run"])
        .assert()
        .code(1)
        .stderr(contains("is not a mydumper dump (no metadata file)"));
    sandbox
        .slipdock()
        .args(["load", "example", "/no/such/folder", "--dry-run"])
        .assert()
        .code(1)
        .stderr(contains("dump folder not found: /no/such/folder"));
}

#[test]
fn dumps_lists_folders_and_marks_partial_ones() {
    let sandbox = Sandbox::new();
    sandbox.dump_fixture("example_db__example__20260101-000000", true);
    sandbox.dump_fixture("example_db__example__20260102-000000", false);
    let out = stdout(sandbox.slipdock().args(["dumps", "example"]));
    assert!(
        out.starts_with(&format!("{}\n", sandbox.dumps().display())),
        "{out}"
    );
    assert!(
        out.contains("        4B  example_db__example__20260101-000000\n"),
        "{out}"
    );
    assert!(
        out.contains("        3B  example_db__example__20260102-000000  (partial)\n"),
        "{out}"
    );
    // Without a profile: every distinct dump folder.
    assert_eq!(stdout(sandbox.slipdock().arg("dumps")), out);
}

#[test]
fn tools_shows_status_and_set_checks_before_saving() {
    let sandbox = Sandbox::new();
    let out = stdout(sandbox.slipdock().arg("tools"));
    assert!(
        out.contains("mydumper   ~/bin/mydumper  1.0.19-1  (set by you)"),
        "{out}"
    );
    assert!(
        out.contains("myloader   ~/bin/myloader  1.0.19-1  (set by you)"),
        "{out}"
    );

    let settings = sandbox.config().join("settings.toml");
    let before = fs::read_to_string(&settings).unwrap();
    sandbox
        .slipdock()
        .args(["tools", "set", "mydumper", "/no/such/mydumper"])
        .assert()
        .code(1)
        .stderr(contains("/no/such/mydumper cannot be run"));
    assert_eq!(fs::read_to_string(&settings).unwrap(), before);

    sandbox
        .slipdock()
        .args(["tools", "unset", "myloader"])
        .assert()
        .success();
    assert!(!fs::read_to_string(&settings).unwrap().contains("myloader"));
}

#[test]
fn a_missing_required_tool_stops_a_dump_with_the_install_hint() {
    let sandbox = Sandbox::new();
    fs::remove_file(sandbox.config().join("settings.toml")).unwrap();
    // PATH has only an empty folder, HOME is the sandbox: nothing to find
    // unless this machine has the tools in a usual location.
    let mut command = sandbox.slipdock();
    command.env("PATH", sandbox.root.join("empty"));
    let output = command
        .args(["dump", "example", "example_db", "--dry-run"])
        .output()
        .unwrap();
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        assert!(err.contains("slipdock tools install mydumper"), "{err}");
    }
}

#[test]
fn completions_print_a_script_and_complete_profile_names() {
    let sandbox = Sandbox::new();
    for shell in ["zsh", "bash", "fish"] {
        sandbox
            .slipdock()
            .args(["completions", shell])
            .assert()
            .success()
            .stdout(contains("slipdock"));
    }
    // What the fish script runs when Tab is pressed after "slipdock dump ".
    let out = stdout(
        sandbox
            .slipdock()
            .env("COMPLETE", "fish")
            .args(["--", "slipdock", "dump", ""]),
    );
    assert!(out.lines().any(|line| line.starts_with("example")), "{out}");
}

#[test]
fn color_always_adds_codes_and_no_color_is_the_default_off_a_terminal() {
    let sandbox = Sandbox::new();
    let coloured = stdout(sandbox.slipdock().args([
        "--color",
        "always",
        "dump",
        "example",
        "example_db",
        "--dry-run",
    ]));
    assert!(coloured.contains("\x1b[1m"), "{coloured:?}");
    let plain = stdout(
        sandbox
            .slipdock()
            .args(["dump", "example", "example_db", "--dry-run"]),
    );
    assert!(!plain.contains('\x1b'));
}

#[test]
fn app_without_the_app_installed_says_so() {
    let sandbox = Sandbox::new();
    let beside = Path::new(env!("CARGO_BIN_EXE_slipdock")).with_file_name("slipdock-app");
    if !beside.exists() {
        sandbox
            .slipdock()
            .arg("app")
            .assert()
            .code(1)
            .stderr(contains("the desktop app is not installed"));
    }
}

#[test]
fn ctrl_c_stops_the_tool_reports_it_and_removes_the_temporary_file() {
    use std::os::unix::process::CommandExt;
    use std::process::{Command as Process, Stdio};

    let sandbox = Sandbox::new();
    sandbox.fake_tool("mydumper", "exec sleep 30");
    let tmp = sandbox.root.join("tmp");
    fs::create_dir_all(&tmp).unwrap();

    // Its own process group, like a command started from a shell: Ctrl-C
    // goes to slipdock and to the tool at the same moment.
    let child = Process::new(env!("CARGO_BIN_EXE_slipdock"))
        .args(["dump", "example", "example_db", "--log-level", "2"])
        .env_clear()
        .env("SLIPDOCK_CONFIG_DIR", sandbox.config())
        .env("HOME", &sandbox.root)
        .env("PATH", "/usr/bin:/bin")
        .env("TMPDIR", &tmp)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn()
        .unwrap();

    // Wait until the temporary defaults file exists: the run has started.
    let temp_files = || {
        fs::read_dir(&tmp)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .file_name()
                    .to_string_lossy()
                    .starts_with("slipdock-")
            })
            .count()
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while temp_files() == 0 && std::time::Instant::now() < deadline {
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    assert_eq!(
        temp_files(),
        1,
        "the temporary defaults file exists during the run"
    );
    std::thread::sleep(std::time::Duration::from_millis(300));

    let group = format!("-{}", child.id());
    assert!(
        Process::new("kill")
            .args(["-INT", "--", &group])
            .status()
            .unwrap()
            .success()
    );
    let output = child.wait_with_output().unwrap();

    let err = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(130), "{err}");
    assert!(err.contains("! Interrupted."), "{err}");
    assert!(err.contains("✗ Dump did not finish (exit 130)"), "{err}");
    assert_eq!(temp_files(), 0, "the temporary defaults file is removed");
}
