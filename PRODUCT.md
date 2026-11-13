# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

## Users

Any developer or database administrator who works with MySQL or MariaDB and
uses, or wants to use, mydumper and myloader. Slipdock is a public open-source
tool (MIT or Apache-2.0), not an internal one.

They come to Slipdock for three jobs. The jobs are not ranked:

- **Refresh a development database.** Dump staging or production, then load it
  into a local or test database.
- **Backups.** Take dumps to keep, and restore one when something goes wrong.
- **Move between servers.** Copy or migrate a database from one environment or
  server to another.

Not decided: how much the user is assumed to know about mydumper.

## Product Purpose

Slipdock dumps and loads MySQL and MariaDB databases with mydumper and
myloader, from a desktop app or a command line tool. It does not replace
mydumper. It runs it: it keeps one connection file per environment, builds the
command, shows progress and judges the result.

A run succeeds when the user knows, without reading the tool's log, whether
the dump or load is complete, and when no database was overwritten that the
user did not name.

## Positioning

- **It runs the real tools and shows the command.** The user can see the exact
  mydumper or myloader command before it runs, and can do a dry run.
- **The load guard.** A load drops and recreates the tables in the dump.
  Before it starts, the user types the name of the target database. There is
  no `--yes`.
- **A dump is judged by its `metadata` file, not by the exit code.** mydumper
  can exit with 1 on a complete dump, and Slipdock reports that as a success
  with the warnings listed.
- **Two front ends, one library.** The app and the command line tool are thin
  layers over `slipdock-core`, so they apply the same rules and show the same
  report text.

## Operating Context

- **The desktop app is the main product.** Design work serves it first. The
  command line tool is the scriptable companion.
- The app has five screens: Dump, Load, Dumps, Profiles and Tools. A run shows
  elapsed time, files, size, the log as it grows and a Cancel button. The
  report at the end is the same text the command line tool prints.
- A profile is one environment (for example `staging` or `local`): a normal
  MySQL defaults file in the config folder. A dump is a folder named
  `<db>__<profile>__<time>` with its log beside it.
- The user must already have mydumper, myloader and zstd installed. The mysql
  or mariadb client is optional and lets the user pick databases and tables
  from a list.
- The work is high-stakes and often slow: a dump or load can run for a long
  time, and a load destroys data in the target database.

## Capabilities and Constraints

**About the Platform value.** `web` is the closest value Impeccable offers; it
has none for desktop. The facts are:

- The app is drawn with GPUI through the `gpui-kit` crate, in Rust. There is
  no browser, HTML or CSS.
- It runs in desktop windows only. There are no mobile sizes.
- Impeccable's HTML and CSS detector, live mode and mobile viewport checks do
  not apply to it.
- The window cannot be checked from a screenshot in an agent session. The app
  tests (`cargo test -p slipdock-app`) check layout in a headless window, and
  the user checks the look by hand.

**macOS and Linux have equal standing.** The app must feel right on both, with
no conventions that exist on macOS only. Current state, which does not yet
meet this:

- The documented screen shortcuts are Cmd-1 to Cmd-5.
- Only macOS has a packaged app (`Slipdock.app`, through cargo-bundle). Linux
  has no package.
- Windows is not supported, because mydumper has no Windows builds. Windows
  users run Slipdock inside WSL.

**Rules that design work must keep:**

- Slipdock never downloads a tool itself. It finds what is installed, and
  otherwise shows the install command of the package manager. The command
  runs only when the user starts it, with the command in view.
- A partial dump is never deleted. The folder and the log are shown.
- A rule goes in `slipdock-core`, and text for the user is built there as
  data in the `report` module. A front end draws a report; it does not write
  one.
- Closing the app while a tool runs does not stop the tool (a known limit).

**Terms:** profile, dump, load, dump folder, load guard, tools, dry run.

## Brand Commitments

- The name is Slipdock. The binaries are `slipdock` and `slipdock-app`, and
  the bundle identifier is `app.slipdock.Slipdock`.
- The app icon is `crates/slipdock-app/macos/icon.svg`.

Not decided: voice, personality and any further identity rule.

## Evidence on Hand

- `README.md` and `docs/reference.md`: the full description of behaviour, commands and files.
- `crates/slipdock-app/macos/icon.svg`: the icon.
- The app itself, in `crates/slipdock-app/src/`.

There are none of the following, and future work must not invent them:
screenshots or visual goldens, a website, testimonials, named users, usage
numbers, benchmarks, or a signed and notarized release.

## Product Principles

1. **Never overwrite what the user did not name.** A destructive step needs
   the user to state the target, not only to agree.
2. **Show what will run and what happened.** The command before the run, and a
   judged result after it. The user should not have to read a tool's log to
   know the outcome.
3. **Run the real tools; do not hide them.** Slipdock adds safety and
   convenience around mydumper and myloader. It does not replace them or
   pretend they are not there.
4. **One behaviour, two front ends.** What the app does and says, the command
   line tool does and says too.
5. **Leave the user's machine and files alone.** Do not install, download or
   delete without asking, and keep the comments and unknown keys in a profile
   file.
