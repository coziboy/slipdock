//! Builds the exact command lines for mydumper, myloader and mysql.
//!
//! Nothing here touches the disk or starts a process. Each function takes
//! plain values and returns an [`Invocation`]: program, arguments and extra
//! environment. That makes this the best-tested module: a test states the
//! inputs and compares the whole argument list.
//!
//! The same `Invocation` is used to show the command (`preview`) and to run
//! it (`to_command`), so what is shown is what runs.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Set for mydumper/myloader when the profile has `tls-verify = FALSE`.
/// MariaDB Connector/C, which they are built on, then skips the certificate
/// check. The connection is still encrypted.
pub const TLS_OFF_ENV: (&str, &str) = ("MARIADB_TLS_DISABLE_PEER_VERIFICATION", "1");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invocation {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    /// Variables added to the environment of the tool.
    pub env: Vec<(String, String)>,
}

impl Invocation {
    fn new(program: &Path, tls_off: bool) -> Self {
        let env = if tls_off {
            vec![(TLS_OFF_ENV.0.to_string(), TLS_OFF_ENV.1.to_string())]
        } else {
            Vec::new()
        };
        Invocation {
            program: program.to_path_buf(),
            args: Vec::new(),
            env,
        }
    }

    fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    fn args<I: IntoIterator<Item = S>, S: Into<OsString>>(mut self, args: I) -> Self {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn to_command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.args);
        command.envs(self.env.iter().map(|(key, value)| (key, value)));
        command
    }

    /// The command as text that can be pasted into a shell. Each option
    /// starts a new line, so long paths stay readable:
    ///
    /// ```text
    ///   env VAR=1 mydumper \
    ///       --defaults-file /config/profiles/example.cnf \
    ///       --database example_db
    /// ```
    pub fn preview(&self) -> String {
        let mut words = Vec::new();
        if !self.env.is_empty() {
            words.push("env".to_string());
            words.extend(self.env.iter().map(|(k, v)| format!("{k}={v}")));
        }
        words.push(self.program.to_string_lossy().into_owned());
        words.extend(self.args.iter().map(|a| a.to_string_lossy().into_owned()));

        let mut text = String::from(" ");
        for (index, word) in words.iter().enumerate() {
            if index > 0 && word.starts_with('-') {
                text.push_str(" \\\n     ");
            }
            text.push(' ');
            text.push_str(&quote(word));
        }
        text
    }
}

// --- The three tools --------------------------------------------------------

pub struct Dump<'a> {
    pub program: &'a Path,
    /// The file given as `--defaults-file`: the profile, or a temporary copy
    /// of it (see `job::Defaults`).
    pub defaults_file: &'a Path,
    /// A file listing `db.table` names to leave out, if any.
    pub omit_file: Option<&'a Path>,
    pub database: &'a str,
    /// The new dump folder.
    pub out: &'a Path,
    pub log: &'a Path,
    /// Arguments after `--`, passed on unchanged.
    pub extra_args: &'a [String],
    pub tls_off: bool,
}

pub fn mydumper(dump: &Dump) -> Invocation {
    let mut invocation = Invocation::new(dump.program, dump.tls_off)
        .args(["--defaults-file".as_ref(), dump.defaults_file.as_os_str()]);
    if let Some(omit) = dump.omit_file {
        invocation = invocation.args(["--omit-from-file".as_ref(), omit.as_os_str()]);
    }
    invocation
        .args(["--database", dump.database])
        .args(["--outputdir".as_ref(), dump.out.as_os_str()])
        .args(["--compress", "ZSTD"])
        .args(["--logfile".as_ref(), dump.log.as_os_str()])
        .args(dump.extra_args)
}

pub struct Load<'a> {
    pub program: &'a Path,
    pub defaults_file: &'a Path,
    /// The dump folder to load.
    pub dir: &'a Path,
    /// The database to load into.
    pub database: &'a str,
    pub log: &'a Path,
    pub extra_args: &'a [String],
    pub tls_off: bool,
}

pub fn myloader(load: &Load) -> Invocation {
    Invocation::new(load.program, load.tls_off)
        .args(["--defaults-file".as_ref(), load.defaults_file.as_os_str()])
        .args(["--directory".as_ref(), load.dir.as_os_str()])
        .args(["--database", load.database])
        .args(["--drop-table", "DROP"])
        .args(["--logfile".as_ref(), load.log.as_os_str()])
        .args(load.extra_args)
}

/// One query with the mysql client: no column names (`-N`), tab separated
/// (`-B`). The client wants `--defaults-file=...` as one word, and first.
pub fn mysql_query(program: &Path, defaults_file: &Path, sql: &str, tls_off: bool) -> Invocation {
    let mut defaults = OsString::from("--defaults-file=");
    defaults.push(defaults_file);
    // Recent MariaDB clients verify the certificate by default, so the
    // variable is set for them too. The MySQL client ignores it.
    Invocation::new(program, tls_off)
        .arg(defaults)
        .args(["-N", "-B", "-e", sql])
}

// --- Names -------------------------------------------------------------------

/// `<db>__<profile>__<YYYYmmdd-HHMMSS>`: the folder name of a new dump.
pub fn dump_name(database: &str, profile: &str, timestamp: &str) -> String {
    format!("{database}__{profile}__{timestamp}")
}

/// Files that sit beside a dump folder share its name: `<dump>.log`,
/// `<dump>.omit`.
pub fn beside(dump: &Path, extension: &str) -> PathBuf {
    let mut name = dump.as_os_str().to_os_string();
    name.push(".");
    name.push(extension);
    PathBuf::from(name)
}

/// `<dump>__load_<profile>_<timestamp>.log`: each load writes its own log.
pub fn load_log(dump: &Path, profile: &str, timestamp: &str) -> PathBuf {
    let mut name = dump.as_os_str().to_os_string();
    name.push(format!("__load_{profile}_{timestamp}.log"));
    PathBuf::from(name)
}

/// Turns what the user typed after `--exclude` into the lines of the omit
/// file. Values may hold several names separated by commas. A bare table
/// name gets the database in front; a dotted name is kept as it is.
pub fn omit_lines(database: &str, exclude: &[String]) -> Vec<String> {
    exclude
        .iter()
        .flat_map(|value| value.split(','))
        .map(|name| name.replace(' ', ""))
        .filter(|name| !name.is_empty())
        .map(|name| {
            if name.contains('.') {
                name
            } else {
                format!("{database}.{name}")
            }
        })
        .collect()
}

/// The omit lines as shown in the summary: without the `db.` in front.
pub fn omit_shown(database: &str, lines: &[String]) -> Vec<String> {
    let prefix = format!("{database}.");
    lines
        .iter()
        .map(|line| line.strip_prefix(&prefix).unwrap_or(line).to_string())
        .collect()
}

// --- Quoting -------------------------------------------------------------------

/// Characters that never need quoting in a shell word.
fn is_plain(c: char) -> bool {
    c.is_alphanumeric() || "_-./:@%+,=~".contains(c)
}

/// Quotes a word for a POSIX shell, only when needed: `plain` stays as it
/// is, `two words` becomes `'two words'`.
pub fn quote(word: &str) -> String {
    let needs_quotes =
        word.is_empty() || word.starts_with(['=', '~']) || !word.chars().all(is_plain);
    if needs_quotes {
        // Inside single quotes everything is literal, except the quote
        // itself: end the quotes, add an escaped quote, start again.
        format!("'{}'", word.replace('\'', r"'\''"))
    } else {
        word.to_string()
    }
}

/// A path for display: the home folder is shortened to `~` and special
/// characters get a backslash, so the text can be pasted into a command
/// unquoted (inside quotes `~` would not be expanded).
pub fn display_path(path: &Path, home: Option<&Path>) -> String {
    let escape = |text: &str| -> String {
        let mut out = String::new();
        for c in text.chars() {
            if !is_plain(c) {
                out.push('\\');
            }
            out.push(c);
        }
        out
    };
    if let Some(rest) = home.and_then(|home| path.strip_prefix(home).ok()) {
        let rest = rest.to_string_lossy();
        if rest.is_empty() {
            return "~".to_string();
        }
        return format!("~/{}", escape(&rest));
    }
    escape(&path.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compares an argument list with plain strings.
    fn strings(invocation: &Invocation) -> Vec<String> {
        invocation
            .args
            .iter()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    fn dump<'a>() -> Dump<'a> {
        Dump {
            program: Path::new("/tools/mydumper"),
            defaults_file: Path::new("/config/profiles/example.cnf"),
            omit_file: None,
            database: "example_db",
            out: Path::new("/dumps/example_db__example__20260102-030405"),
            log: Path::new("/dumps/example_db__example__20260102-030405.log"),
            extra_args: &[],
            tls_off: false,
        }
    }

    #[test]
    fn mydumper_plain() {
        let invocation = mydumper(&dump());
        assert_eq!(invocation.program, Path::new("/tools/mydumper"));
        assert!(invocation.env.is_empty());
        assert_eq!(
            strings(&invocation),
            [
                "--defaults-file",
                "/config/profiles/example.cnf",
                "--database",
                "example_db",
                "--outputdir",
                "/dumps/example_db__example__20260102-030405",
                "--compress",
                "ZSTD",
                "--logfile",
                "/dumps/example_db__example__20260102-030405.log",
            ]
        );
    }

    #[test]
    fn mydumper_with_omit_file_extra_args_and_tls_off() {
        let extra = ["--threads".to_string(), "2".to_string()];
        let invocation = mydumper(&Dump {
            omit_file: Some(Path::new("/dumps/x.omit")),
            extra_args: &extra,
            tls_off: true,
            ..dump()
        });
        assert_eq!(
            invocation.env,
            [(
                "MARIADB_TLS_DISABLE_PEER_VERIFICATION".to_string(),
                "1".to_string()
            )]
        );
        assert_eq!(
            strings(&invocation),
            [
                "--defaults-file",
                "/config/profiles/example.cnf",
                "--omit-from-file",
                "/dumps/x.omit",
                "--database",
                "example_db",
                "--outputdir",
                "/dumps/example_db__example__20260102-030405",
                "--compress",
                "ZSTD",
                "--logfile",
                "/dumps/example_db__example__20260102-030405.log",
                "--threads",
                "2",
            ]
        );
    }

    fn load<'a>() -> Load<'a> {
        Load {
            program: Path::new("/tools/myloader"),
            defaults_file: Path::new("/config/profiles/example.cnf"),
            dir: Path::new("/dumps/example_db__example__20260102-030405"),
            database: "target_db",
            log: Path::new(
                "/dumps/example_db__example__20260102-030405__load_example_20260103-000000.log",
            ),
            extra_args: &[],
            tls_off: false,
        }
    }

    #[test]
    fn myloader_plain_and_with_extras() {
        let expected = [
            "--defaults-file",
            "/config/profiles/example.cnf",
            "--directory",
            "/dumps/example_db__example__20260102-030405",
            "--database",
            "target_db",
            "--drop-table",
            "DROP",
            "--logfile",
            "/dumps/example_db__example__20260102-030405__load_example_20260103-000000.log",
        ];
        let plain = myloader(&load());
        assert_eq!(strings(&plain), expected);
        assert!(plain.env.is_empty());

        let extra = ["--enable-binlog".to_string()];
        let with_extras = myloader(&Load {
            extra_args: &extra,
            tls_off: true,
            ..load()
        });
        let mut expected: Vec<&str> = expected.to_vec();
        expected.push("--enable-binlog");
        assert_eq!(strings(&with_extras), expected);
        assert_eq!(with_extras.env.len(), 1);
    }

    #[test]
    fn mysql_query_puts_the_defaults_file_first_as_one_word() {
        let invocation = mysql_query(
            Path::new("/tools/mysql"),
            Path::new("/config/profiles/example.cnf"),
            "SHOW DATABASES",
            true,
        );
        assert_eq!(invocation.env.len(), 1);
        assert_eq!(
            strings(&invocation),
            [
                "--defaults-file=/config/profiles/example.cnf",
                "-N",
                "-B",
                "-e",
                "SHOW DATABASES"
            ]
        );
    }

    #[test]
    fn preview_starts_a_new_line_at_each_option() {
        let extra = ["--where".to_string(), "id > 5".to_string()];
        let invocation = mydumper(&Dump {
            extra_args: &extra,
            tls_off: true,
            ..dump()
        });
        assert_eq!(
            invocation.preview(),
            "  env MARIADB_TLS_DISABLE_PEER_VERIFICATION=1 /tools/mydumper \\
      --defaults-file /config/profiles/example.cnf \\
      --database example_db \\
      --outputdir /dumps/example_db__example__20260102-030405 \\
      --compress ZSTD \\
      --logfile /dumps/example_db__example__20260102-030405.log \\
      --where 'id > 5'"
        );
    }

    #[test]
    fn names() {
        assert_eq!(
            dump_name("example_db", "example", "20260102-030405"),
            "example_db__example__20260102-030405"
        );
        let dump = Path::new("/dumps/example_db__example__20260102-030405");
        assert_eq!(
            beside(dump, "omit"),
            Path::new("/dumps/example_db__example__20260102-030405.omit")
        );
        assert_eq!(
            load_log(dump, "other", "20260103-000000"),
            Path::new(
                "/dumps/example_db__example__20260102-030405__load_other_20260103-000000.log"
            )
        );
    }

    #[test]
    fn excludes_become_omit_lines() {
        let typed = [
            "logs, audit ,,other_db.jobs".to_string(),
            "".to_string(),
            "cache".to_string(),
        ];
        let lines = omit_lines("example_db", &typed);
        assert_eq!(
            lines,
            [
                "example_db.logs",
                "example_db.audit",
                "other_db.jobs",
                "example_db.cache"
            ]
        );
        assert_eq!(
            omit_shown("example_db", &lines),
            ["logs", "audit", "other_db.jobs", "cache"]
        );
        assert!(omit_lines("example_db", &[]).is_empty());
    }

    #[test]
    fn quote_only_when_needed() {
        for plain in [
            "plain", "a=b", "a,b", "a:b", "a@b", "a%b", "a+b", "-x", "a~b", "/p/q.r", "é",
        ] {
            assert_eq!(quote(plain), plain);
        }
        assert_eq!(quote(""), "''");
        assert_eq!(quote("a b"), "'a b'");
        assert_eq!(quote("=b"), "'=b'");
        assert_eq!(quote("~x"), "'~x'");
        assert_eq!(quote("a#b"), "'a#b'");
        assert_eq!(quote("a*b"), "'a*b'");
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(quote("$HOME"), "'$HOME'");
    }

    #[test]
    fn display_path_shortens_home_and_escapes_spaces() {
        let home = Some(Path::new("/home/example"));
        assert_eq!(
            display_path(Path::new("/home/example/dumps"), home),
            "~/dumps"
        );
        assert_eq!(
            display_path(Path::new("/home/example/My Dumps/a"), home),
            r"~/My\ Dumps/a"
        );
        assert_eq!(display_path(Path::new("/home/example"), home), "~");
        assert_eq!(display_path(Path::new("/srv/dumps"), home), "/srv/dumps");
        assert_eq!(
            display_path(Path::new("/home/example2/x"), home),
            "/home/example2/x"
        );
        assert_eq!(display_path(Path::new("/srv/a b"), None), r"/srv/a\ b");
    }
}
