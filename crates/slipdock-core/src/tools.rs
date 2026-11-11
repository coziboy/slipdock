//! Finding and checking the external tools: mydumper, myloader, mysql.
//!
//! Slipdock never downloads a tool. It looks for what is installed, checks
//! that each candidate really works, and otherwise shows how to install it
//! with the system's package manager.
//!
//! Three steps, each a function here:
//! 1. [`candidates`]: every place the tool could be (all of them, not only
//!    the first, so a broken one can be skipped).
//! 2. [`probe`]: run the candidate and check that it is the real tool.
//! 3. [`status`]: put it together with the user's setting.

use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use crate::exec::{CaptureError, capture};
use crate::platform::{self, Os, PackageManager};
use crate::settings::Settings;
use crate::{Error, Result};

const PROBE_LIMIT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Tool {
    Mydumper,
    Myloader,
    Mysql,
}

impl Tool {
    pub const ALL: [Tool; 3] = [Tool::Mydumper, Tool::Myloader, Tool::Mysql];

    /// The name used in settings and on the command line.
    pub fn name(self) -> &'static str {
        match self {
            Tool::Mydumper => "mydumper",
            Tool::Myloader => "myloader",
            Tool::Mysql => "mysql",
        }
    }

    pub fn from_name(name: &str) -> Option<Tool> {
        Tool::ALL.into_iter().find(|tool| tool.name() == name)
    }

    /// File names to look for. MariaDB's client is called `mariadb`; newer
    /// MariaDB packages no longer install a `mysql` alias.
    fn file_names(self) -> &'static [&'static str] {
        match self {
            Tool::Mydumper => &["mydumper"],
            Tool::Myloader => &["myloader"],
            Tool::Mysql => &["mysql", "mariadb"],
        }
    }

    /// What the tool is needed for, for status lists.
    pub fn purpose(self) -> &'static str {
        match self {
            Tool::Mydumper => "dumps a database",
            Tool::Myloader => "loads a dump",
            Tool::Mysql => "optional: lists databases and tables to pick from",
        }
    }
}

impl fmt::Display for Tool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

// --- 1. Find -----------------------------------------------------------------

/// The folders to search. Tests fill this with temp folders; the programs
/// use [`SearchDirs::system`].
#[derive(Debug, Clone, Default)]
pub struct SearchDirs {
    /// The folders of `$PATH`, in order.
    pub path: Vec<PathBuf>,
    /// Usual install locations that may not be on PATH.
    pub usual: Vec<PathBuf>,
}

impl SearchDirs {
    pub fn system() -> Self {
        SearchDirs {
            path: platform::path_dirs(&|name| std::env::var_os(name)),
            usual: platform::usual_tool_dirs(platform::current_os()),
        }
    }

    fn all(&self) -> impl Iterator<Item = &PathBuf> {
        self.path.iter().chain(&self.usual)
    }
}

/// Every executable that could be `tool`: PATH first, then the usual
/// locations. The same file reached by two routes is listed once.
pub fn candidates(tool: Tool, dirs: &SearchDirs) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut seen = Vec::new();
    for dir in dirs.all() {
        for name in tool.file_names() {
            let path = dir.join(name);
            if !platform::is_executable(&path) {
                continue;
            }
            // Symlinks are resolved only to spot duplicates; the path shown
            // and used stays the one the user would recognise.
            let real = path.canonicalize().unwrap_or_else(|_| path.clone());
            if !seen.contains(&real) {
                seen.push(real);
                found.push(path);
            }
        }
    }
    found
}

// --- 2. Probe ------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    /// The mydumper version Slipdock's options were checked against.
    pub const VERIFIED: Version = Version {
        major: 1,
        minor: 0,
        patch: 19,
    };

    /// Reads the leading `x.y.z` of a text such as `1.0.19-1`.
    pub fn parse(text: &str) -> Option<Version> {
        let mut numbers = text
            .split(|c: char| !c.is_ascii_digit())
            .take(3)
            .map(|part| part.parse::<u32>());
        Some(Version {
            major: numbers.next()?.ok()?,
            minor: numbers.next()?.ok()?,
            patch: numbers.next()?.ok()?,
        })
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// A candidate that passed the probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Working {
    /// The version as the tool prints it, for example `1.0.19-1`.
    pub version: String,
}

impl Working {
    /// For mydumper/myloader: a note when the version is older than the one
    /// Slipdock was checked against. Older versions are allowed, not blocked.
    pub fn version_warning(&self, tool: Tool) -> Option<String> {
        if tool == Tool::Mysql {
            return None;
        }
        let version = Version::parse(&self.version)?;
        (version < Version::VERIFIED).then(|| {
            format!(
                "{tool} {} is older than {}, the version Slipdock was checked against; \
                 some options may be missing",
                self.version,
                Version::VERIFIED
            )
        })
    }
}

/// Why a candidate cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Problem {
    CannotRun(String),
    TimedOut,
    /// It ran, but did not answer like the tool. Holds its first output line.
    NotTheTool(String),
    /// A mysql that ignores `--defaults-file`: Slipdock could not hand it
    /// the profile.
    RejectsDefaultsFile,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Problem::CannotRun(why) => write!(f, "cannot be run ({why})"),
            Problem::TimedOut => write!(f, "did not answer within {} s", PROBE_LIMIT.as_secs()),
            Problem::NotTheTool(line) if line.is_empty() => {
                write!(f, "printed no version")
            }
            Problem::NotTheTool(line) => write!(f, "unexpected answer to --version: {line}"),
            Problem::RejectsDefaultsFile => {
                write!(
                    f,
                    "does not accept --defaults-file (looks like a wrapper script)"
                )
            }
        }
    }
}

/// Runs the candidate and checks that it is a working `tool`.
pub fn probe(tool: Tool, path: &Path) -> std::result::Result<Working, Problem> {
    let banner = ask(Command::new(path).arg("--version"))?;
    let first_line = banner.lines().next().unwrap_or("").trim().to_string();
    let version = match tool {
        Tool::Mydumper | Tool::Myloader => mydumper_version(&first_line),
        Tool::Mysql => mysql_version(&first_line),
    };
    let Some(version) = version else {
        return Err(Problem::NotTheTool(first_line));
    };
    if tool == Tool::Mysql {
        check_reads_defaults_file(path)?;
    }
    Ok(Working { version })
}

/// Runs a command and returns stdout followed by stderr.
fn ask(command: &mut Command) -> std::result::Result<String, Problem> {
    match capture(command, PROBE_LIMIT) {
        Ok(output) => Ok(output.stdout + &output.stderr),
        Err(CaptureError::TimedOut) => Err(Problem::TimedOut),
        Err(CaptureError::NotStarted(error)) => Err(Problem::CannotRun(error.to_string())),
    }
}

/// `mydumper v1.0.19-1, built against ...` -> `1.0.19-1`.
/// myloader prints the same line with its own name.
fn mydumper_version(line: &str) -> Option<String> {
    let rest = line
        .strip_prefix("mydumper v")
        .or_else(|| line.strip_prefix("myloader v"))?;
    let version = rest.split([',', ' ']).next()?;
    Version::parse(version).map(|_| version.to_string())
}

/// The client version from a banner such as
/// `mysql  Ver 9.7.1 for macos26.4 on arm64 (Homebrew)`,
/// `mysql  Ver 15.1 Distrib 10.11.6-MariaDB, for Linux` or
/// `mariadb from 11.4.2-MariaDB, client 15.2 for Linux`.
fn mysql_version(line: &str) -> Option<String> {
    let lower = line.to_ascii_lowercase();
    if !lower.contains("mysql") && !lower.contains("mariadb") {
        return None;
    }
    let words: Vec<&str> = line.split_whitespace().collect();
    let after = |marker: &str| {
        let index = words.iter().position(|word| *word == marker)?;
        words.get(index + 1).map(|word| word.trim_end_matches(','))
    };
    // "Distrib" (older MariaDB) names the release; "Ver" alone is MySQL's.
    after("Distrib")
        .or_else(|| after("from"))
        .or_else(|| after("Ver"))
        .filter(|version| version.starts_with(|c: char| c.is_ascii_digit()))
        .map(str::to_string)
}

/// Slipdock passes the profile to mysql as `--defaults-file=<file>`. Some
/// "mysql" commands are wrapper scripts (for example ones that run the client
/// inside a container) and cannot do that. They still answer `--version`
/// like the real client, so the check is functional: give it a defaults file
/// with a made-up user and ask it to print the options it read. Nothing
/// connects anywhere.
fn check_reads_defaults_file(path: &Path) -> std::result::Result<(), Problem> {
    const MARKER: &str = "slipdock_probe";
    let cannot = |error: std::io::Error| Problem::CannotRun(error.to_string());
    let mut file = tempfile::Builder::new()
        .prefix("slipdock-probe.")
        .suffix(".cnf")
        .tempfile()
        .map_err(cannot)?;
    writeln!(file, "[client]\nuser={MARKER}").map_err(cannot)?;

    let mut defaults = std::ffi::OsString::from("--defaults-file=");
    defaults.push(file.path());
    let answer = ask(Command::new(path).arg(defaults).arg("--print-defaults"))?;
    if answer.contains(MARKER) {
        Ok(())
    } else {
        Err(Problem::RejectsDefaultsFile)
    }
}

// --- 3. Status -------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Candidate {
    pub path: PathBuf,
    pub result: std::result::Result<Working, Problem>,
}

#[derive(Debug, Clone)]
pub struct Status {
    pub tool: Tool,
    /// The path fixed in the settings, if any. Then it is the only one used.
    pub configured: Option<PathBuf>,
    /// The configured path first (if set), then everything found.
    pub candidates: Vec<Candidate>,
}

impl Status {
    /// The candidate Slipdock uses: the configured one, or else the first
    /// one that works.
    pub fn chosen(&self) -> Option<&Candidate> {
        match &self.configured {
            Some(path) => self
                .candidates
                .iter()
                .find(|c| &c.path == path && c.result.is_ok()),
            None => self.candidates.iter().find(|c| c.result.is_ok()),
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.chosen().map(|candidate| candidate.path.as_path())
    }

    /// One sentence on why no candidate is used. `None` when one is.
    pub fn problem(&self) -> Option<String> {
        if self.chosen().is_some() {
            return None;
        }
        let tool = self.tool;
        if let Some(path) = &self.configured {
            let why = match self.candidates.iter().find(|c| &c.path == path) {
                Some(Candidate {
                    result: Err(problem),
                    ..
                }) => problem.to_string(),
                _ => "not found".to_string(),
            };
            return Some(format!(
                "the path set for {tool}, {}, {why}. Change it with: slipdock tools set {tool} <path>, \
                 or go back to searching with: slipdock tools unset {tool}",
                path.display()
            ));
        }
        Some(match self.candidates.as_slice() {
            [] => format!("{tool} was not found. See: slipdock tools install {tool}"),
            [only] => format!(
                "{} {}",
                only.path.display(),
                only.result.as_ref().unwrap_err()
            ),
            many => format!("none of the {} copies of {tool} found works", many.len()),
        })
    }
}

/// Finds and probes `tool`. This starts each candidate once, so it takes a
/// moment; callers do it once per run, not per use.
pub fn status(tool: Tool, settings: &Settings, dirs: &SearchDirs) -> Status {
    let configured = settings.tool_path(tool.name()).map(Path::to_path_buf);
    let mut paths: Vec<PathBuf> = configured.iter().cloned().collect();
    for path in candidates(tool, dirs) {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    let candidates = paths
        .into_iter()
        .map(|path| {
            let result = probe(tool, &path);
            Candidate { path, result }
        })
        .collect();
    Status {
        tool,
        configured,
        candidates,
    }
}

/// The path to use for `tool`, or an error that says what to do.
pub fn resolve(tool: Tool, settings: &Settings, dirs: &SearchDirs) -> Result<PathBuf> {
    let status = status(tool, settings, dirs);
    match status.path() {
        Some(path) => Ok(path.to_path_buf()),
        None => Err(Error::msg(status.problem().unwrap_or_default())),
    }
}

// --- Install plan ------------------------------------------------------------------

/// How to get a missing tool. Slipdock runs a command only when it needs no
/// root rights, and only after the user has seen it and said yes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallPlan {
    /// A command Slipdock can run as the current user.
    Command {
        program: String,
        args: Vec<String>,
    },
    /// Steps for the user to do themselves.
    Steps(Vec<String>),
    Unsupported(String),
}

pub const MYDUMPER_RELEASES: &str = "https://github.com/mydumper/mydumper/releases";

pub fn install_plan(tool: Tool, os: Os, managers: &[PackageManager]) -> InstallPlan {
    let has = |manager| managers.contains(&manager);
    // mydumper and myloader come in one package.
    let brew_package = match tool {
        Tool::Mydumper | Tool::Myloader => "mydumper",
        Tool::Mysql => "mysql-client",
    };
    match os {
        Os::Windows => InstallPlan::Unsupported(
            "mydumper publishes no Windows builds. Use Slipdock inside WSL, where it works as on Linux."
                .to_string(),
        ),
        Os::MacOs if has(PackageManager::Homebrew) => InstallPlan::Command {
            program: "brew".to_string(),
            args: vec!["install".to_string(), brew_package.to_string()],
        },
        Os::MacOs => InstallPlan::Steps(vec![
            "Install Homebrew: https://brew.sh".to_string(),
            format!("brew install {brew_package}"),
        ]),
        Os::Linux | Os::Other => InstallPlan::Steps(linux_steps(tool, managers)),
    }
}

/// On Linux every route needs root, so these are shown and never run.
fn linux_steps(tool: Tool, managers: &[PackageManager]) -> Vec<String> {
    let has = |manager| managers.contains(&manager);
    let mut steps = Vec::new();
    match tool {
        Tool::Mydumper | Tool::Myloader => {
            steps.push(format!(
                "Download the package for your distribution from {MYDUMPER_RELEASES}"
            ));
            steps.push(
                "(the distribution's own mydumper package is usually much older)".to_string(),
            );
            if has(PackageManager::Apt) {
                steps.push(
                    "sudo apt-get install ./mydumper_<version>.<distro>_<arch>.deb".to_string(),
                );
            }
            if has(PackageManager::Dnf) {
                steps.push("sudo dnf install ./mydumper-<version>.<distro>.<arch>.rpm".to_string());
            }
            if has(PackageManager::Pacman) {
                steps.push(
                    "Arch: mydumper is in the AUR, not in the official repositories".to_string(),
                );
            }
        }
        Tool::Mysql => {
            if has(PackageManager::Apt) {
                steps.push("sudo apt-get install default-mysql-client".to_string());
            }
            if has(PackageManager::Dnf) {
                steps.push("sudo dnf install mariadb".to_string());
            }
            if has(PackageManager::Pacman) {
                steps.push("sudo pacman -S mariadb-clients".to_string());
            }
            if steps.is_empty() {
                steps.push(
                    "Install the MySQL or MariaDB command line client with your package manager"
                        .to_string(),
                );
            }
        }
    }
    if has(PackageManager::Homebrew) {
        let package = if tool == Tool::Mysql {
            "mysql-client"
        } else {
            "mydumper"
        };
        steps.push(format!(
            "or, with Homebrew on Linux: brew install {package}"
        ));
    }
    steps
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    /// Writes an executable shell script: a stand-in for a real tool.
    fn fake(dir: &Path, name: &str, body: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    const MYDUMPER: &str =
        r#"echo "mydumper v1.0.19-1, built against MariaDB 10.8.8 with SSL support""#;

    /// A real client: prints its banner, and with --print-defaults the
    /// options it read from the defaults file.
    const REAL_MYSQL: &str = r#"
case "$1" in
  --defaults-file=*)
    file="${1#--defaults-file=}"
    echo "mysql would have been started with the following arguments:"
    sed -n 's/^user=/--user=/p' "$file" ;;
  *) echo "mysql  Ver 9.7.1 for macos26.4 on arm64 (Homebrew)" ;;
esac"#;

    /// A wrapper that runs the client somewhere else. Observed behaviour of a
    /// real one: --version works, --defaults-file is refused with exit 7.
    const WRAPPER_MYSQL: &str = r#"
case "$1" in
  --defaults-file=*) echo "mysql: [ERROR] unknown variable '${1#--}'." >&2; exit 7 ;;
  *) echo "/usr/bin/mysql  Ver 8.4.13 for Linux on aarch64 (MySQL Community Server - GPL)" ;;
esac"#;

    fn dirs(path: &[&Path], usual: &[&Path]) -> SearchDirs {
        SearchDirs {
            path: path.iter().map(|p| p.to_path_buf()).collect(),
            usual: usual.iter().map(|p| p.to_path_buf()).collect(),
        }
    }

    #[test]
    fn versions_are_read_from_banners() {
        let line = "mydumper v1.0.19-1, built against MariaDB 10.8.8 with SSL support";
        assert_eq!(mydumper_version(line).as_deref(), Some("1.0.19-1"));
        assert_eq!(
            mydumper_version("myloader v0.9.5, built against MySQL 5.7").as_deref(),
            Some("0.9.5")
        );
        assert_eq!(mydumper_version("something else"), None);

        let banner = |line| mysql_version(line);
        assert_eq!(
            banner("mysql  Ver 9.7.1 for macos26.4 on arm64 (Homebrew)").as_deref(),
            Some("9.7.1")
        );
        assert_eq!(
            banner("mysql  Ver 15.1 Distrib 10.11.6-MariaDB, for debian-linux-gnu").as_deref(),
            Some("10.11.6-MariaDB")
        );
        assert_eq!(
            banner("mariadb from 11.4.2-MariaDB, client 15.2 for Linux (x86_64)").as_deref(),
            Some("11.4.2-MariaDB")
        );
        assert_eq!(banner("usage: something"), None);

        assert!(Version::parse("1.0.19-1").unwrap() == Version::VERIFIED);
        assert!(Version::parse("0.16.7-5").unwrap() < Version::VERIFIED);
        assert_eq!(Version::parse("abc"), None);
    }

    #[test]
    fn candidates_keep_path_order_and_drop_duplicates() {
        let temp = tempfile::tempdir().unwrap();
        let (a, b, c) = (
            temp.path().join("a"),
            temp.path().join("b"),
            temp.path().join("c"),
        );
        let first = fake(&a, "mysql", "");
        let second = fake(&b, "mariadb", "");
        // c/mysql is a symlink to a/mysql: the same file, listed once.
        fs::create_dir_all(&c).unwrap();
        std::os::unix::fs::symlink(&first, c.join("mysql")).unwrap();
        // Not executable: not a candidate.
        fs::write(b.join("mysql"), "").unwrap();

        let found = candidates(Tool::Mysql, &dirs(&[&a, &c], &[&b, &a]));
        assert_eq!(found, [first, second]);
        assert!(candidates(Tool::Mydumper, &dirs(&[&a], &[])).is_empty());
    }

    #[test]
    fn probe_accepts_real_tools_and_reads_the_version() {
        let temp = tempfile::tempdir().unwrap();
        let mydumper = fake(temp.path(), "mydumper", MYDUMPER);
        assert_eq!(
            probe(Tool::Mydumper, &mydumper).unwrap().version,
            "1.0.19-1"
        );
        let mysql = fake(temp.path(), "mysql", REAL_MYSQL);
        assert_eq!(probe(Tool::Mysql, &mysql).unwrap().version, "9.7.1");
    }

    #[test]
    fn probe_names_each_kind_of_failure() {
        let temp = tempfile::tempdir().unwrap();
        let wrapper = fake(temp.path(), "mysql", WRAPPER_MYSQL);
        let problem = probe(Tool::Mysql, &wrapper).unwrap_err();
        assert_eq!(problem, Problem::RejectsDefaultsFile);
        assert_eq!(
            problem.to_string(),
            "does not accept --defaults-file (looks like a wrapper script)"
        );

        let other = fake(temp.path(), "mydumper", "echo hello");
        assert_eq!(
            probe(Tool::Mydumper, &other).unwrap_err(),
            Problem::NotTheTool("hello".into())
        );

        let missing = probe(Tool::Mydumper, &temp.path().join("nope")).unwrap_err();
        assert!(matches!(missing, Problem::CannotRun(_)));
    }

    #[test]
    fn a_wrapper_first_on_path_is_skipped_for_the_next_working_candidate() {
        let temp = tempfile::tempdir().unwrap();
        let wrapper = fake(&temp.path().join("shims"), "mysql", WRAPPER_MYSQL);
        let real = fake(&temp.path().join("keg"), "mysql", REAL_MYSQL);
        let search = dirs(&[wrapper.parent().unwrap()], &[real.parent().unwrap()]);

        let status = status(Tool::Mysql, &Settings::default(), &search);

        assert_eq!(status.candidates.len(), 2, "both are kept for display");
        assert_eq!(status.path(), Some(real.as_path()));
        assert_eq!(status.problem(), None);
        assert_eq!(
            resolve(Tool::Mysql, &Settings::default(), &search).unwrap(),
            real
        );
    }

    #[test]
    fn a_configured_path_is_used_alone_and_its_failure_is_explained() {
        let temp = tempfile::tempdir().unwrap();
        let found = fake(&temp.path().join("bin"), "mydumper", MYDUMPER);
        let custom = fake(&temp.path().join("custom"), "mydumper", MYDUMPER);
        let search = dirs(&[found.parent().unwrap()], &[]);

        let mut settings = Settings::default();
        settings.set_tool_path("mydumper", custom.clone());
        assert_eq!(
            status(Tool::Mydumper, &settings, &search).path(),
            Some(custom.as_path())
        );

        // A broken configured path is not silently replaced by a found one.
        settings.set_tool_path("mydumper", temp.path().join("gone"));
        let status = status(Tool::Mydumper, &settings, &search);
        assert_eq!(status.path(), None);
        let problem = status.problem().unwrap();
        assert!(
            problem.contains("slipdock tools unset mydumper"),
            "{problem}"
        );
    }

    #[test]
    fn a_missing_tool_points_to_install() {
        let error =
            resolve(Tool::Myloader, &Settings::default(), &SearchDirs::default()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "myloader was not found. See: slipdock tools install myloader"
        );
    }

    #[test]
    fn old_versions_warn_but_work() {
        let old = Working {
            version: "0.16.7-5".into(),
        };
        assert!(
            old.version_warning(Tool::Mydumper)
                .unwrap()
                .contains("older than 1.0.19")
        );
        let current = Working {
            version: "1.0.19-1".into(),
        };
        assert_eq!(current.version_warning(Tool::Mydumper), None);
        assert_eq!(old.version_warning(Tool::Mysql), None);
    }

    #[test]
    fn install_plans() {
        let brew = [PackageManager::Homebrew];
        assert_eq!(
            install_plan(Tool::Myloader, Os::MacOs, &brew),
            InstallPlan::Command {
                program: "brew".into(),
                args: vec!["install".into(), "mydumper".into()]
            }
        );
        assert_eq!(
            install_plan(Tool::Mysql, Os::MacOs, &brew),
            InstallPlan::Command {
                program: "brew".into(),
                args: vec!["install".into(), "mysql-client".into()]
            }
        );
        assert!(matches!(
            install_plan(Tool::Mysql, Os::MacOs, &[]),
            InstallPlan::Steps(_)
        ));
        // Linux never gets a command to run, also not with apt present.
        let InstallPlan::Steps(steps) =
            install_plan(Tool::Mysql, Os::Linux, &[PackageManager::Apt])
        else {
            panic!("expected steps");
        };
        assert_eq!(steps, ["sudo apt-get install default-mysql-client"]);
        assert!(matches!(
            install_plan(Tool::Mydumper, Os::Linux, &[PackageManager::Dnf]),
            InstallPlan::Steps(_)
        ));
        assert!(matches!(
            install_plan(Tool::Mydumper, Os::Windows, &[]),
            InstallPlan::Unsupported(_)
        ));
    }
}
