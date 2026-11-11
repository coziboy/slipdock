# Slipdock

Dump and load MySQL/MariaDB databases with [mydumper](https://github.com/mydumper/mydumper)
and myloader, from a command line tool or a desktop app.

Slipdock does not replace mydumper. It runs it for you: it keeps one
connection file per environment, builds the command, shows progress, and
judges the result. Before a load it makes you name the database that is about
to be overwritten.

- `slipdock`: the command line tool
- `slipdock-app`: the desktop app (built with GPUI)

Both are thin layers over one library, `slipdock-core`, so they behave the same.

## Requirements

| Tool | Needed for | Version |
| --- | --- | --- |
| mydumper, myloader | dumping and loading | 0.20.1 or newer (tested with 1.0.19) |
| zstd | compressing dumps; mydumper and myloader start it | any |
| mysql or mariadb client | optional: lists databases and tables to pick from | any |

Slipdock never downloads these. It finds what is installed and otherwise shows
the install command for your package manager:

```sh
slipdock tools            # path, version and status of each tool
slipdock tools install    # shows the command; asks before running it
```

- **macOS:** `brew install mydumper mysql-client`
- **Linux:** take the `.deb` or `.rpm` from mydumper's
  [releases](https://github.com/mydumper/mydumper/releases). Distribution
  repositories ship 0.10 or nothing. Installing needs root, so Slipdock shows
  the steps and does not run them.
- **Windows:** mydumper has no Windows builds. Use Slipdock inside WSL.

## Build

```sh
cargo build --release
# target/release/slipdock and target/release/slipdock-app
```

`rust-toolchain.toml` pins the compiler version; rustup fetches it on the
first build. To build only the command line tool (no GPUI, much faster):

```sh
cargo build --release -p slipdock-cli
```

### macOS app bundle

`cargo build` cannot make a `.app`; [cargo-bundle](https://github.com/burtonageo/cargo-bundle)
does. Install it once, then run it **from the repository root**:

```sh
cargo install cargo-bundle --locked     # tested with 0.12.0
cargo bundle --release -p slipdock-app --format osx
# target/release/bundle/osx/Slipdock.app
codesign --force --sign - target/release/bundle/osx/Slipdock.app
cp -R target/release/bundle/osx/Slipdock.app /Applications/
```

The settings are in `crates/slipdock-app/Cargo.toml` under
`[package.metadata.bundle]`. Their paths are relative to the repository root.
Started from another folder, cargo-bundle does not find the icon or
`Info.ext.plist` and leaves them out without an error.

cargo-bundle does not sign the bundle, so the `codesign` line seals it with an
ad hoc signature. That is enough for the Mac that built it. To give the app
to someone else it needs a Developer ID signature and notarization.

The icon is `crates/slipdock-app/macos/icon.svg`; cargo-bundle converts it.
Without `--format osx` it also builds `Slipdock.dmg`.

## Quick start

```sh
slipdock init staging            # creates a profile from the template
# edit the file it printed: host, user, password, dump-dir
slipdock dump staging shop --dry-run
slipdock dump staging shop
slipdock dumps staging
slipdock load local ~/dumps/shop__staging__20260102-030405
```

Or run `slipdock` alone on a terminal for guided mode: it asks for the
profile, database and tables, shows the command it is about to run, and offers
a dry run.

## Commands

```text
slipdock                                   guided mode on a terminal, help otherwise
slipdock init <profile>
slipdock profiles
slipdock dump <profile> <db> [-x, --exclude t1,t2]... [--out DIR] [--log-level 0-3] [-- mydumper args]
slipdock load <profile> <dump-dir> [target-db] [--log-level 0-3] [--confirm NAME] [-- myloader args]
slipdock dumps [profile]
slipdock tools
slipdock tools install [tool]
slipdock tools set <tool> <path> | unset <tool>    # tool: mydumper, myloader, mysql, zstd
slipdock password set|clear <profile>
slipdock completions zsh|bash|fish
slipdock app
```

On `dump` and `load`: `--dry-run` shows the command and stops,
`--show-command` shows it and runs. Everywhere: `--config-dir DIR` and
`--color auto|always|never` (`auto` honours `NO_COLOR`).

### The load guard

A load drops and recreates the tables that are in the dump. Before it starts,
you type the name of the target database. Without a terminal, pass
`--confirm <target database>` instead. There is no `--yes`.

### How a dump is judged

By the `metadata` file, not by the exit code. mydumper writes that file last,
and it can exit with 1 over a warning on a dump that is complete. So:

- no `metadata`: "Dump did not finish". The partial folder and the log are
  shown; nothing is deleted.
- `metadata` and a non-zero exit: success, followed by the distinct errors and
  warnings from the log.

## Files

Config folder: `$SLIPDOCK_CONFIG_DIR`, else `$XDG_CONFIG_HOME/slipdock`, else
`~/.config/slipdock` (Linux) or `~/Library/Application Support/slipdock`
(macOS). The folder is mode 0700 and the files in it 0600.

- `profiles/<name>.cnf`: a normal MySQL defaults file. mydumper, myloader and
  mysql read `[client]`, `[mydumper]` and `[myloader]`; Slipdock reads
  `[slipdock]`:

  | Key | Meaning |
  | --- | --- |
  | `dump-dir` | where this profile's dumps go (`~` allowed). Required unless you pass `--out`. |
  | `tls-verify = FALSE` | do not check the server's certificate (still encrypted) |
  | `password-store = keychain` | the password is in the system keychain, not in the file |

  Slipdock changes single lines and keeps your comments and any keys it does
  not know. Do not put a comment at the end of a line: mydumper reads
  `value  # note` as the value.
- `settings.toml`: a fixed path per tool, when you set one.

A dump is a folder `<db>__<profile>__<YYYYmmdd-HHMMSS>` in the dump folder.
Beside it: `<name>.log`, and `<name>.omit` when tables were excluded. Each
load writes `<name>__load_<profile>_<time>.log`.

### Passwords

By default the password is the `password` line in the profile file.
`slipdock password set <profile>` moves it to the system keychain;
`slipdock password clear <profile>` removes it from there.

### One-run settings and the temporary file

`--log-level` and a keychain password are not written to your profile. For
that run, Slipdock makes a private copy of the profile (mode 0600, in the
system temp folder) with those values filled in, passes it as
`--defaults-file`, and deletes it afterwards.

It does not use `--defaults-extra-file` for this: mydumper 1.0.19 takes its
connection settings from one file only, and as soon as an extra file is given,
the `[client]` section of the main file is ignored.

## The app

```sh
slipdock app        # or start slipdock-app directly
```

Screens: **Dump**, **Load**, **Dumps**, **Profiles**, **Tools** (Cmd-1 to
Cmd-5). A run shows elapsed time, files, size, the end of the log, and a
Cancel button; the report at the end is the same text the command line prints.

Known limit: closing the app while a tool runs does not stop the tool. Use
Cancel first.

On macOS, `slipdock app` looks for `slipdock-app` beside `slipdock` and on
`PATH`. It does not look in `/Applications`, so start an installed
`Slipdock.app` from Finder, Spotlight or `open -a Slipdock`. An app started
that way does not see variables set only in your shell
(`SLIPDOCK_CONFIG_DIR`, `XDG_CONFIG_HOME`), and its `PATH` is the one in the
bundle's `Info.plist`.

## Development

### Set up

You need [rustup](https://rustup.rs) and nothing else. `rust-toolchain.toml`
pins the compiler (with rustfmt and clippy), and rustup fetches it the first
time you run `cargo` in this folder. Your default toolchain is not changed.

```sh
git clone <repository url> && cd Slipdock
cargo test
```

You do not need mydumper, a database or a keychain to build or to run the
tests. You need them only to try a real dump or load.

### Run from source

```sh
cargo run -p slipdock-cli -- tools       # arguments for slipdock go after `--`
cargo run -p slipdock-app                # opens the window
```

`cargo run -p slipdock-cli -- app` builds only `slipdock`. It starts the
`slipdock-app` that is beside it in `target/debug`, so run `cargo build` first,
or it reports that the app is not installed (or starts an old build).

A debug build reads the same config folder as an installed Slipdock, which
means your real profiles. To work on scratch data, point it somewhere else:

```sh
export SLIPDOCK_CONFIG_DIR="$PWD/target/dev-config"   # target/ is not in git
cargo run -p slipdock-cli -- init staging
cargo run -p slipdock-app                             # sees the same folder
```

`--config-dir DIR` does the same for one command line run.

### Code layout

```text
crates/slipdock-core   the library: no terminal, no window, no async runtime
crates/slipdock-cli    the `slipdock` binary
crates/slipdock-app    the `slipdock-app` binary
```

Core modules, each small and with its own tests:

| Module | Does |
| --- | --- |
| `platform` | the only place that checks the operating system |
| `profile` | reads and writes profile files line by line |
| `settings` | `settings.toml` |
| `tools` | finds and probes mydumper, myloader, mysql, zstd; install plans |
| `command` | builds the exact argument lists; the preview text |
| `dumps` | reads dump folders: size, tables, complete or partial |
| `run` | runs a tool on a thread, sends progress over a channel, cancel |
| `job` | a dump or load in steps: plan, prepare, start, report |
| `report` | summaries and reports as data, drawn by each front end |
| `server` | lists databases and tables through the mysql client |
| `secrets` | the keychain, behind a trait |
| `exec` | runs a short command with a time limit |

If you are reading the app to learn GPUI, start with the table at the top of
`crates/slipdock-app/src/main.rs`.

### Where a change goes

- **A rule** (what a profile means, which arguments a tool gets, when a dump
  counts as complete) goes in `slipdock-core`, so both front ends get it. The
  core never prints, never opens a window and is not async: long work runs on
  a thread and reports over a `std::sync::mpsc` channel.
- **Text for the user** is built in `report` as data. The command line tool
  and the app each draw it, so do not format a report in a front end.
- **An operating system check** goes in `platform` and nowhere else.
- **A new command or flag** goes in `crates/slipdock-cli/src/cli.rs` (the
  clap definitions) and `commands.rs` (what it does). Also add it to
  [Commands](#commands) above.
- **A new screen or control** goes in the `*_view.rs` file of that screen;
  parts used by several screens are in `widgets.rs`.

### Tests

```sh
cargo test                       # everything
cargo test -p slipdock-core      # one crate
cargo test -p slipdock-core profile::    # one module
cargo test -p slipdock-app every_screen_draws    # one test by name
```

No test talks to a database, installs anything, or touches the real keychain.
Tools are replaced by small shell scripts in temp folders, and the keychain by
`secrets::MemoryStore`.

| Crate | Where the tests are | How they work |
| --- | --- | --- |
| `slipdock-core` | a `tests` module at the end of each file | call the functions directly, with fake tools and fixture dump folders |
| `slipdock-cli` | `crates/slipdock-cli/tests/cli.rs` | run the real `slipdock` binary (assert_cmd) in a `Sandbox`: temp config folder, one profile, fake tools |
| `slipdock-app` | `crates/slipdock-app/src/tests.rs` | open the real window in GPUI's headless test platform; no screen needed |

To add one:

- **Core or command line:** write a fake tool as a shell script body with the
  helper the tests already use (`fake_tool` in `job.rs` and `tests/cli.rs`),
  then assert on the report or the output.
- **App:** mark the test `#[gpui_kit::test]`, take
  `cx: &mut TestAppContext`, and start with `open(cx, sandbox(&root))`. Drive
  a screen through the methods its buttons call. When you add a control, the
  layout tests (`nothing_runs_out_of_the_window` and the ones after it) check
  that it stays inside the smallest window.

### Before you commit

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

There is no CI, so these three are the whole check. All of them must pass.

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org)
with the area as scope: `feat(app): ...`, `fix(cli): ...`, `test(core): ...`.

### Dependencies

Every dependency is declared once, in `[workspace.dependencies]` of the root
`Cargo.toml`, with an exact version (`"=1.2.3"`). A crate then takes it with
`name.workspace = true`. `Cargo.lock` is committed. Together these make a
build next year use the same code as a build today.

To update one, change the pinned version, run `cargo update -p <name>`, and
run the checks above. For `gpui-kit`, also look at the `taffy` and
`unicode-properties` lines in `crates/slipdock-app/Cargo.toml`: they must
match what the new `gpui-kit` pins, and the comment there says why. A newer
`gpui-kit` may also need a newer compiler in `rust-toolchain.toml`.

## License

MIT or Apache-2.0, at your option. See `LICENSE-MIT` and `LICENSE-APACHE`.
