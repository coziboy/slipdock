# Development

How to build, test and change Slipdock. For what it does, see the [README](../README.md) and the [reference](reference.md).

## Set up

You need [rustup](https://rustup.rs) and nothing else. `rust-toolchain.toml`
pins the compiler (with rustfmt and clippy), and rustup fetches it the first
time you run `cargo` in this folder. Your default toolchain is not changed.

```sh
git clone https://github.com/coziboy/slipdock.git && cd slipdock
cargo test
```

You do not need mydumper, a database or a keychain to build or to run the
tests. You need them only to try a real dump or load.

## Run from source

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
ad hoc signature. That is enough for the Mac that built it. A copy that
someone else downloads also runs, but macOS blocks it until they allow it
(see [Install](../README.md#install)); only a Developer ID signature and
notarization remove that step.

`.github/workflows/release.yml` runs these steps on GitHub for every `v*` tag
and attaches the zipped app to a release:

```sh
git tag v0.1.0 && git push origin v0.1.0
```

The workflow keeps cargo-bundle and the compiled dependencies in GitHub's
cache. A tag run can only read a cache that a run on `main` saved, so start
the workflow by hand on `main` before a release, or after the dependencies
changed (Actions > release > Run workflow, or `gh workflow run release.yml`).
That run builds the app without making a release. Without a cache a release
still works; it takes about 15 minutes.

The icon is `crates/slipdock-app/macos/icon.svg`; cargo-bundle converts it.
Without `--format osx` it also builds `Slipdock.dmg`.

## Code layout

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

## Where a change goes

- **A rule** (what a profile means, which arguments a tool gets, when a dump
  counts as complete) goes in `slipdock-core`, so both front ends get it. The
  core never prints, never opens a window and is not async: long work runs on
  a thread and reports over a `std::sync::mpsc` channel.
- **Text for the user** is built in `report` as data. The command line tool
  and the app each draw it, so do not format a report in a front end.
- **An operating system check** goes in `platform` and nowhere else.
- **A new command or flag** goes in `crates/slipdock-cli/src/cli.rs` (the
  clap definitions) and `commands.rs` (what it does). Also add it to
  [Commands](../README.md#commands) in the README.
- **A new screen or control** goes in the `*_view.rs` file of that screen;
  parts used by several screens are in `widgets.rs`.

## Tests

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

## Before you commit

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
```

No workflow runs these for you (the only one builds the release), so these
three are the whole check. All of them must pass.

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org)
with the area as scope: `feat(app): ...`, `fix(cli): ...`, `test(core): ...`.

## Dependencies

Every dependency is declared once, in `[workspace.dependencies]` of the root
`Cargo.toml`, with an exact version (`"=1.2.3"`). A crate then takes it with
`name.workspace = true`. `Cargo.lock` is committed. Together these make a
build next year use the same code as a build today.

To update one, change the pinned version, run `cargo update -p <name>`, and
run the checks above. For `gpui-kit`, also look at the `taffy` and
`unicode-properties` lines in `crates/slipdock-app/Cargo.toml`: they must
match what the new `gpui-kit` pins, and the comment there says why. A newer
`gpui-kit` may also need a newer compiler in `rust-toolchain.toml`.
