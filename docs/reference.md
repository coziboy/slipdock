# Reference

How Slipdock behaves in detail. For a first look, see the [README](../README.md).

## The load guard

A load drops and recreates the tables that are in the dump. Before it starts,
you type the name of the target database. Without a terminal, pass
`--confirm <target database>` instead. There is no `--yes`.

## How a dump is judged

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
- `settings.toml`: a fixed path per tool, when you set one, and the colour
  band the app shows each profile with.

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
Cmd-5). A run shows elapsed time, files, size, the log as it grows, and a
Cancel button; the report at the end is the same text the command line prints.
The log box follows the newest line. Scrolled back, it stays where you read,
and **Follow** returns to the end.

Known limit: closing the app while a tool runs does not stop the tool. Use
Cancel first.

On macOS, `slipdock app` looks for `slipdock-app` beside `slipdock` and on
`PATH`. It does not look in `/Applications`, so start an installed
`Slipdock.app` from Finder, Spotlight or `open -a Slipdock`. An app started
that way does not see variables set only in your shell
(`SLIPDOCK_CONFIG_DIR`, `XDG_CONFIG_HOME`), and its `PATH` is the one in the
bundle's `Info.plist`.
