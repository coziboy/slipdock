//! A dump or a load from start to finish, as steps both front ends share.
//!
//! ```text
//!   plan_dump / plan_load      check everything, build the summary
//!        |                     (the load guard is asked here, by the caller)
//!   plan.prepare()             write the temporary defaults file, build the command
//!        |                     (a dry run shows prepared.invocation and stops)
//!   prepared.start()           run the tool, events over a channel (see `run`)
//!        |
//!   prepared.report(outcome)   judge the result, build the report
//! ```
//!
//! Planning and preparing are separate so that nothing secret exists while
//! the user is still reading the summary or typing the confirmation.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::command::{self, Invocation};
use crate::profile::{PasswordStore, Profile};
use crate::report::{self, Line, Report};
use crate::run::{self, Outcome, Running};
use crate::secrets::SecretStore;
use crate::settings::{self, Settings};
use crate::tool_event::{self, RunState};
use crate::tools::{self, SearchDirs, Tool};
use crate::{Error, Result, dumps, platform};

/// Everything a job needs to know about its surroundings. The programs build
/// one at start; tests build one that points at temp folders and fakes.
#[derive(Clone)]
pub struct Context {
    pub config_dir: PathBuf,
    pub home: Option<PathBuf>,
    pub settings: Settings,
    pub dirs: SearchDirs,
    pub secrets: Arc<dyn SecretStore>,
}

impl Context {
    /// The context of the real system. `config_dir` is the `--config-dir`
    /// option, if given.
    pub fn system(config_dir: Option<&Path>) -> Result<Context> {
        let home = std::env::home_dir();
        let config_dir = platform::config_dir(
            config_dir,
            &|name| std::env::var_os(name),
            platform::current_os(),
            home.as_deref(),
        )?;
        Ok(Context {
            settings: Settings::load(&config_dir)?,
            config_dir,
            home,
            dirs: SearchDirs::system(),
            secrets: Arc::new(crate::secrets::Keychain),
        })
    }

    pub fn profile(&self, name: &str) -> Result<Profile> {
        Profile::load(&self.config_dir, name)
    }

    pub fn show_path(&self, path: &Path) -> String {
        command::display_path(path, self.home.as_deref())
    }
}

/// The time now as `YYYYmmdd-HHMMSS`, local time. Used in dump and log names.
pub fn timestamp() -> String {
    jiff::Zoned::now().strftime("%Y%m%d-%H%M%S").to_string()
}

pub const LOG_LEVEL_HELP: &str = "0 silent, 1 errors, 2 warnings, 3 info";

fn check_level(level: Option<u8>) -> Result<()> {
    match level {
        Some(level) if level > 3 => Err(Error::msg(format!(
            "--log-level must be 0-3 ({LOG_LEVEL_HELP})"
        ))),
        _ => Ok(()),
    }
}

// --- The defaults file ----------------------------------------------------------

/// The file handed to a tool as `--defaults-file`.
///
/// Usually that is the profile itself. For a one-run log level or a keychain
/// password it is a private temporary copy of the profile with those values
/// filled in. Why a copy and not `--defaults-extra-file`: mydumper 1.0.19
/// takes its connection settings from one file only, and with an extra file
/// that is the extra file, so host, user and password of the profile would
/// be ignored.
///
/// The copy is deleted when this value is dropped.
pub struct Defaults {
    path: PathBuf,
    _temp: Option<tempfile::TempPath>,
}

impl Defaults {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_temporary(&self) -> bool {
        self._temp.is_some()
    }
}

/// Shown under a dry-run command that names a temporary defaults file.
pub const TEMPORARY_DEFAULTS_NOTE: &str = "(--defaults-file is a temporary copy of the profile \
     with this run's settings; it exists only while a run lasts)";

/// The defaults file for a query with the mysql client.
///
/// - `level`: a `verbose` value for `[mydumper]` and `[myloader]`, this run only.
/// - `with_secret`: fetch a keychain password. A dry run passes `false`: it
///   must not touch the keychain.
pub fn defaults_for(
    ctx: &Context,
    profile: &Profile,
    level: Option<u8>,
    with_secret: bool,
) -> Result<Defaults> {
    defaults_with(ctx, profile, level, &[], with_secret)
}

/// The defaults file for a dump or a load: as [`defaults_for`], with the
/// choices of the settings that hold for every run (`[options.<tool>]`).
///
/// They go into the file, not on the command line: mydumper and myloader
/// let a value in the defaults file win over the same option given as an
/// argument, so `--threads` would do nothing for a profile that sets
/// `threads`. See [`settings::parse_arguments`].
pub fn run_defaults(
    ctx: &Context,
    profile: &Profile,
    level: Option<u8>,
    with_secret: bool,
) -> Result<Defaults> {
    let options = &ctx.settings.options;
    let arguments = [
        (
            "mydumper",
            settings::parse_arguments(&options.mydumper.arguments)?,
        ),
        (
            "myloader",
            settings::parse_arguments(&options.myloader.arguments)?,
        ),
    ];
    defaults_with(ctx, profile, level, &arguments, with_secret)
}

fn defaults_with(
    ctx: &Context,
    profile: &Profile,
    level: Option<u8>,
    // The options of the settings, per section of the file.
    arguments: &[(&str, Vec<(String, String)>)],
    with_secret: bool,
) -> Result<Defaults> {
    let keychain = profile.password_store() == PasswordStore::Keychain;
    let no_arguments = arguments.iter().all(|(_, pairs)| pairs.is_empty());
    if level.is_none() && no_arguments && !keychain {
        return Ok(Defaults {
            path: profile.path.clone(),
            _temp: None,
        });
    }
    let mut doc = profile.doc.clone();
    if let Some(level) = level {
        doc.set("mydumper", "verbose", &level.to_string())?;
        doc.set("myloader", "verbose", &level.to_string())?;
    }
    // After the level, so `--verbose` among the arguments has the last word.
    for (section, pairs) in arguments {
        for (name, value) in pairs {
            doc.set(section, name, value)?;
        }
    }
    if keychain && with_secret {
        let password = ctx.secrets.get(&profile.name)?.ok_or_else(|| {
            Error::msg(format!(
                "profile {0} keeps its password in the keychain, but none is stored. \
                 Store it with: slipdock password set {0}",
                profile.name
            ))
        })?;
        doc.set("client", "password", &password)?;
    }
    temporary_defaults(&doc)
}

/// Writes a defaults file that exists only until the value is dropped.
pub fn temporary_defaults(doc: &crate::profile::Document) -> Result<Defaults> {
    let temp_error = |source| Error::Io {
        context: "cannot write the temporary defaults file".into(),
        source,
    };
    // tempfile creates the file with mode 0600 and a random name.
    let file = tempfile::Builder::new()
        .prefix("slipdock-")
        .suffix(".cnf")
        .tempfile()
        .map_err(temp_error)?;
    fs::write(file.path(), doc.to_text()).map_err(temp_error)?;
    let temp = file.into_temp_path();
    Ok(Defaults {
        path: temp.to_path_buf(),
        _temp: Some(temp),
    })
}

/// Summary rows that explain a temporary defaults file.
fn defaults_rows(
    profile: &Profile,
    level: Option<u8>,
    // The arguments the settings give the tool of this run.
    arguments: &str,
) -> Vec<Line> {
    let mut rows = Vec::new();
    if let Some(level) = level {
        rows.push(Line::row("Log level", format!("{level} (this run only)")));
    }
    if !arguments.trim().is_empty() {
        rows.push(Line::row(
            "Arguments",
            format!("{} (from the settings)", arguments.trim()),
        ));
    }
    if profile.password_store() == PasswordStore::Keychain {
        rows.push(Line::row("Password", "from the system keychain"));
    }
    rows
}

// --- Dump --------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct DumpOptions {
    pub profile: String,
    pub database: String,
    /// Tables or views to leave out; each value may hold several, comma separated.
    pub exclude: Vec<String>,
    /// Overrides the profile's `dump-dir`.
    pub out: Option<PathBuf>,
    pub log_level: Option<u8>,
    /// Passed to mydumper unchanged.
    pub extra_args: Vec<String>,
    pub dry_run: bool,
}

pub struct DumpPlan {
    pub summary: Vec<Line>,
    /// The folder the dump will be written to.
    pub out: PathBuf,
    profile: Profile,
    program: PathBuf,
    /// The `PATH` for mydumper, when it must be told where zstd is.
    tool_path: Option<String>,
    omit_file: Option<PathBuf>,
    options: DumpOptions,
}

/// Checks a dump request and gets the disk ready: creates the dump root and
/// writes the `.omit` file (a dry run does this too, so the command it shows
/// can be pasted and run).
pub fn plan_dump(ctx: &Context, options: DumpOptions, timestamp: &str) -> Result<DumpPlan> {
    check_level(options.log_level)?;
    let profile = ctx.profile(&options.profile)?;
    let database = options.database.as_str();
    if database.is_empty() || database.contains(['/', '\\']) {
        return Err(Error::msg(format!(
            "'{database}' is not a usable database name"
        )));
    }
    let root = match &options.out {
        Some(out) => out.clone(),
        None => profile.need_dump_dir(ctx.home.as_deref())?,
    };
    let program = tools::resolve(Tool::Mydumper, &ctx.settings, &ctx.dirs)?;
    // mydumper starts zstd to compress, and stops at once if it cannot find
    // it. Checked here, before anything is written.
    let zstd = tools::resolve(Tool::Zstd, &ctx.settings, &ctx.dirs)?;
    let tool_path = tools::path_with(&zstd, &ctx.dirs);

    fs::create_dir_all(&root).map_err(Error::io_at("cannot create", &root))?;
    let root = root
        .canonicalize()
        .map_err(Error::io_at("cannot open", &root))?;
    let out = root.join(command::dump_name(database, &profile.name, timestamp));

    let omit_lines = command::omit_lines(database, &options.exclude);
    let omit_file = if omit_lines.is_empty() {
        None
    } else {
        // Kept beside the dump as a record of what was left out.
        let file = command::beside(&out, "omit");
        fs::write(&file, omit_lines.join("\n") + "\n")
            .map_err(Error::io_at("cannot write", &file))?;
        Some(file)
    };

    let shown = command::omit_shown(database, &omit_lines);
    let mut summary = vec![
        Line::Title("slipdock dump".into()),
        Line::row_note("Profile", &profile.name, profile.target()),
        Line::row("Database", database),
        Line::row(
            "Exclude",
            if shown.is_empty() {
                "none".to_string()
            } else {
                shown.join(", ")
            },
        ),
        Line::row("Into", format!("{}/", ctx.show_path(&root))),
        Line::row("", out.file_name().unwrap_or_default().to_string_lossy()),
    ];
    if profile.tls_verify_off() {
        summary.push(report::tls_note());
    }
    summary.extend(defaults_rows(
        &profile,
        options.log_level,
        &ctx.settings.options.mydumper.arguments,
    ));
    summary.push(Line::Blank);

    Ok(DumpPlan {
        summary,
        out,
        profile,
        program,
        tool_path,
        omit_file,
        options,
    })
}

impl DumpPlan {
    pub fn prepare(self, ctx: &Context) -> Result<Prepared> {
        let options = &self.options;
        let defaults = run_defaults(ctx, &self.profile, options.log_level, !options.dry_run)?;
        let log = command::beside(&self.out, "log");
        let invocation = command::mydumper(&command::Dump {
            program: &self.program,
            defaults_file: defaults.path(),
            omit_file: self.omit_file.as_deref(),
            database: &options.database,
            out: &self.out,
            log: &log,
            extra_args: &options.extra_args,
            tls_off: self.profile.tls_verify_off(),
            path: self.tool_path.as_deref(),
            json_log: tool_event::supported(Tool::Mydumper, &self.program),
        });
        Ok(Prepared {
            invocation,
            label: format!("Dumping {}", options.database),
            database: options.database.clone(),
            log,
            kind: Kind::Dump { out: self.out },
            level: options
                .log_level
                .or(self.profile.verbose("mydumper"))
                .unwrap_or(2),
            home: ctx.home.clone(),
            ignored_errors: ignored_errors(ctx, &self.profile, "mydumper", &options.extra_args),
            skip_checksums: false,
            _defaults: defaults,
        })
    }
}

// --- Load --------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
pub struct LoadOptions {
    pub profile: String,
    /// The dump folder.
    pub dir: PathBuf,
    /// The database to load into. Default: the one the dump was taken from.
    pub target: Option<String>,
    pub log_level: Option<u8>,
    /// Passed to myloader unchanged.
    pub extra_args: Vec<String>,
    pub dry_run: bool,
}

pub struct LoadPlan {
    pub summary: Vec<Line>,
    /// The database that will be written to. The user must type this name
    /// before a real load starts.
    pub target: String,
    /// `user@host:port` of the profile, for the confirmation.
    pub server: String,
    profile: Profile,
    program: PathBuf,
    /// The `PATH` for myloader, when it must be told where zstd is.
    tool_path: Option<String>,
    dir: PathBuf,
    tables: usize,
    options: LoadOptions,
}

pub fn plan_load(ctx: &Context, options: LoadOptions) -> Result<LoadPlan> {
    check_level(options.log_level)?;
    let profile = ctx.profile(&options.profile)?;
    let dir = &options.dir;
    if !dir.is_dir() {
        let mut message = format!("dump folder not found: {}", dir.display());
        // A path copied without its first "/" is a common slip.
        let with_slash = Path::new("/").join(dir);
        if dir.is_relative() && with_slash.is_dir() {
            message.push_str(&format!(
                "\n      did you mean {} ? (missing leading /)",
                with_slash.display()
            ));
        }
        return Err(Error::msg(message));
    }
    if !dumps::is_complete(dir) {
        return Err(Error::msg(format!(
            "{} is not a mydumper dump (no metadata file)",
            dir.display()
        )));
    }
    let target = match options.target.clone().filter(|name| !name.is_empty()) {
        Some(target) => target,
        None => dumps::source_database(dir).ok_or_else(|| {
            Error::msg("could not detect the database name; pass it after the dump folder")
        })?,
    };
    let program = tools::resolve(Tool::Myloader, &ctx.settings, &ctx.dirs)?;
    // myloader starts zstd to read compressed files. A dump without any
    // (made elsewhere) loads without it.
    let tool_path = if dumps::is_zstd(dir) {
        let zstd = tools::resolve(Tool::Zstd, &ctx.settings, &ctx.dirs)?;
        tools::path_with(&zstd, &ctx.dirs)
    } else {
        None
    };
    let dir = dir
        .canonicalize()
        .map_err(Error::io_at("cannot open", dir))?;
    let tables = dumps::table_count(&dir);

    let name = dir.file_name().unwrap_or_default().to_string_lossy();
    let parent = dir.parent().unwrap_or(Path::new("/"));
    let mut summary = vec![
        Line::Title("slipdock load".into()),
        Line::row_note("Profile", &profile.name, profile.target()),
        Line::row_note(
            "Dump",
            name,
            format!(
                "{tables} tables · {}",
                dumps::human_size(dumps::dir_size(&dir))
            ),
        ),
        Line::row("In", format!("{}/", ctx.show_path(parent))),
        Line::Row {
            label: "Target".into(),
            value: target.clone(),
            note: Some("tables in the dump are dropped and recreated".into()),
            strong: true,
        },
    ];
    if profile.tls_verify_off() {
        summary.push(report::tls_note());
    }
    summary.extend(defaults_rows(
        &profile,
        options.log_level,
        &ctx.settings.options.myloader.arguments,
    ));
    summary.push(Line::Blank);

    Ok(LoadPlan {
        summary,
        target,
        server: profile.target(),
        profile,
        program,
        tool_path,
        dir,
        tables,
        options,
    })
}

impl LoadPlan {
    /// The load guard: the answer must be exactly the target database name.
    pub fn confirms(&self, answer: &str) -> bool {
        answer == self.target
    }

    pub fn prepare(self, ctx: &Context, timestamp: &str) -> Result<Prepared> {
        let options = &self.options;
        let defaults = run_defaults(ctx, &self.profile, options.log_level, !options.dry_run)?;
        let log = command::load_log(&self.dir, &self.profile.name, timestamp);
        let invocation = command::myloader(&command::Load {
            program: &self.program,
            defaults_file: defaults.path(),
            dir: &self.dir,
            database: &self.target,
            log: &log,
            extra_args: &options.extra_args,
            tls_off: self.profile.tls_verify_off(),
            path: self.tool_path.as_deref(),
            json_log: tool_event::supported(Tool::Myloader, &self.program),
        });
        Ok(Prepared {
            invocation,
            label: format!("Loading into {}", self.target),
            database: self.target.clone(),
            log,
            level: options
                .log_level
                .or(self.profile.verbose("myloader"))
                .unwrap_or(2),
            kind: Kind::Load {
                target: self.target,
                tables: self.tables,
                dir: self.dir,
            },
            home: ctx.home.clone(),
            ignored_errors: ignored_errors(ctx, &self.profile, "myloader", &options.extra_args),
            skip_checksums: skips_checksums(ctx, &self.profile, &options.extra_args),
            _defaults: defaults,
        })
    }
}

// --- Prepared: ready to run -----------------------------------------------------------

enum Kind {
    Dump {
        out: PathBuf,
    },
    Load {
        target: String,
        tables: usize,
        /// The dump folder, to list its tables.
        dir: PathBuf,
    },
}

/// A job that is ready to run. Holds the temporary defaults file, if any;
/// dropping this value deletes it.
pub struct Prepared {
    pub invocation: Invocation,
    /// What is happening, for a progress line: `Dumping example_db`.
    pub label: String,
    /// The database the run is about, named in its notification.
    database: String,
    pub log: PathBuf,
    kind: Kind,
    /// The log level in effect (run option, else profile, else 2).
    level: u8,
    home: Option<PathBuf>,
    /// MySQL error numbers the tool was told to ignore.
    ignored_errors: Vec<String>,
    /// A load told to skip its checksums.
    skip_checksums: bool,
    _defaults: Defaults,
}

impl Prepared {
    /// True when `--defaults-file` is a temporary copy, not the profile.
    pub fn uses_temporary_defaults(&self) -> bool {
        self._defaults.is_temporary()
    }

    pub fn start(&self) -> Result<Running> {
        match &self.kind {
            Kind::Dump { out } => run::start_with(
                &self.invocation,
                Some(out.as_path()),
                &self.log,
                RunState::for_dump().ignoring(&self.ignored_errors),
            ),
            Kind::Load { dir, .. } => run::start_with(
                &self.invocation,
                None,
                &self.log,
                RunState::for_load(dumps::data_files_per_table(dir)).ignoring(&self.ignored_errors),
            ),
        }
    }

    /// The stages this kind of run goes through, for a stepper.
    /// The stages this run goes through, for a stepper. A load that skips
    /// its checksums has no Checksums stage.
    pub fn stages(&self) -> Vec<tool_event::Stage> {
        match self.kind {
            Kind::Dump { .. } => tool_event::Stage::DUMP.to_vec(),
            Kind::Load { .. } => tool_event::Stage::LOAD
                .iter()
                .copied()
                .filter(|stage| !(self.skip_checksums && *stage == tool_event::Stage::Checksums))
                .collect(),
        }
    }

    /// What the log of the finished run tells, without ignored errors.
    fn log_state(&self) -> RunState {
        let mut state = RunState::default().ignoring(&self.ignored_errors);
        state.read_log(&self.log);
        state
    }

    fn show(&self, path: &Path) -> String {
        command::display_path(path, self.home.as_deref())
    }

    /// The desktop notification for a finished run and its report.
    pub fn notice(&self, report: &Report) -> report::Notice {
        report::notice(&self.database, report)
    }

    /// Judges a finished run.
    pub fn report(&self, outcome: &Outcome) -> Report {
        let mut lines = Vec::new();
        let code = outcome.exit_code();
        if code != 0 {
            // What the tool printed itself (not its log) often names the cause.
            lines.extend(outcome.output_head.iter().map(|l| Line::Quote(l.clone())));
            let more = outcome
                .output_lines
                .saturating_sub(outcome.output_head.len());
            if more > 0 {
                lines.push(Line::Quote(format!("… {more} more lines in the log")));
            }
        }
        if outcome.interrupted {
            lines.push(Line::Warn("Interrupted.".into()));
        }
        let took = report::duration(outcome.elapsed);
        let success = match &self.kind {
            Kind::Dump { out } => self.report_dump(&mut lines, out, code, &took),
            Kind::Load { target, tables, .. } => {
                self.report_load(&mut lines, target, *tables, code, &took)
            }
        };
        Report { lines, success }
    }

    /// A dump is judged by its `metadata` file, not by the exit code:
    /// mydumper writes that file last, and it can exit non-zero over warnings
    /// on a dump that is complete. Nothing is ever deleted.
    fn report_dump(&self, lines: &mut Vec<Line>, out: &Path, code: i32, took: &str) -> bool {
        if !dumps::is_complete(out) {
            lines.push(Line::Fail(format!(
                "Dump did not finish (exit {code}) after {took}"
            )));
            lines.push(Line::row("Partial", self.show(out)));
            lines.push(Line::row("Log", self.show(&self.log)));
            return false;
        }
        let size = dumps::human_size(dumps::dir_size(out));
        let state = self.log_state();
        let files = state
            .summary
            .as_ref()
            .map(|summary| format!(" · {} files", summary.files))
            .unwrap_or_default();
        lines.push(Line::Ok {
            text: "Dump complete".into(),
            note: Some(format!(
                "{took} · {} tables{files} · {size}",
                dumps::table_count(out)
            )),
        });
        lines.push(Line::row("Folder", self.show(out)));
        lines.push(Line::row("Log", self.show(&self.log)));
        if code != 0 {
            // mydumper counts every failed query, also ones it logs as
            // warnings, and exits 1 if any were counted. Known-harmless
            // error numbers can be left uncounted with
            // "ignore-errors = <n,...>" under [mydumper].
            lines.push(Line::Blank);
            lines.push(Line::Warn(format!(
                "mydumper exited with {code}, but the dump is complete. It reported:"
            )));
            let problems = run::log_problems(&self.log, &self.ignored_errors);
            if !problems.is_empty() {
                lines.extend(problems.into_iter().map(|p| Line::Text(format!("  {p}"))));
            } else if self.level < 2 {
                // Below level 2 mydumper does not log warnings at all.
                lines.push(Line::Text(format!(
                    "  Nothing recorded: this profile logs errors only (verbose = {}).",
                    self.level
                )));
                lines.push(Line::Text(
                    "  Run again with --log-level 2 to see which query failed.".into(),
                ));
            } else {
                lines.push(Line::Text(
                    "  Nothing recorded in the log; see the tool output above, if any.".into(),
                ));
            }
        }
        true
    }

    fn report_load(
        &self,
        lines: &mut Vec<Line>,
        target: &str,
        tables: usize,
        code: i32,
        took: &str,
    ) -> bool {
        let state = self.log_state();
        if code == 0 {
            let mut note = format!("{took} · {tables} tables into {target}");
            if let Some(summary) = &state.summary {
                note = format!(
                    "{took} · {} tables · {} files into {target}",
                    summary.tables, summary.files
                );
            }
            lines.push(Line::Ok {
                text: "Load complete".into(),
                note: Some(note),
            });
            lines.push(Line::row("Log", self.show(&self.log)));
            tool_findings(lines, &state, "myloader");
            return true;
        }
        lines.push(Line::Fail(format!(
            "Load failed (exit {code}) after {took}"
        )));
        lines.push(Line::row("Log", self.show(&self.log)));
        tool_findings(lines, &state, "myloader");
        // myloader, like mydumper, can exit 1 over problems it only logs as
        // warnings; below level 2 those are not recorded anywhere.
        let log_is_empty = fs::metadata(&self.log).map_or(true, |meta| meta.len() == 0);
        if log_is_empty && self.level < 2 {
            for text in [
                format!(
                    "The log is empty: this profile logs errors only (verbose = {}),",
                    self.level
                ),
                "and myloader also exits 1 on problems it logs as warnings.".to_string(),
                "Run again with --log-level 2 to see what it was.".to_string(),
            ] {
                lines.push(Line::Dim(text));
            }
        }
        false
    }
}

/// Every value a run gives `tool`'s option `key`, from wherever it can be
/// set: the arguments in the settings, the profile's `[mydumper]` or
/// `[myloader]` section, and the extra arguments of the run (`--key value`
/// or `--key=value`). In that order, so the last one is the one in effect.
fn option_values(
    ctx: &Context,
    profile: &Profile,
    tool: &str,
    extra_args: &[String],
    key: &str,
) -> Vec<String> {
    let mut values: Vec<String> = Vec::new();
    let settings = match tool {
        "mydumper" => &ctx.settings.options.mydumper.arguments,
        _ => &ctx.settings.options.myloader.arguments,
    };
    if let Ok(pairs) = settings::parse_arguments(settings) {
        values.extend(
            pairs
                .into_iter()
                .filter(|(name, _)| name == key)
                .map(|(_, value)| value),
        );
    }
    values.extend(profile.doc.get(tool, key).map(str::to_string));
    let flag = format!("--{key}");
    let with_value = format!("--{key}=");
    let mut args = extra_args.iter();
    while let Some(arg) = args.next() {
        if let Some(value) = arg.strip_prefix(&with_value) {
            values.push(value.to_string());
        } else if *arg == flag {
            values.extend(args.next().cloned());
        }
    }
    values
}

/// The MySQL error numbers a run tells `tool` to ignore (`ignore-errors`,
/// a comma-separated list such as `1227,1146`, wherever it is set).
fn ignored_errors(
    ctx: &Context,
    profile: &Profile,
    tool: &str,
    extra_args: &[String],
) -> Vec<String> {
    let mut codes: Vec<String> = option_values(ctx, profile, tool, extra_args, "ignore-errors")
        .iter()
        .flat_map(|list| list.split(','))
        .map(|code| code.trim().to_string())
        .filter(|code| !code.is_empty())
        .collect();
    codes.sort();
    codes.dedup();
    codes
}

/// Whether a load tells myloader to skip its checksums (`checksum = skip`).
fn skips_checksums(ctx: &Context, profile: &Profile, extra_args: &[String]) -> bool {
    option_values(ctx, profile, "myloader", extra_args, "checksum")
        .last()
        .is_some_and(|mode| mode.trim().eq_ignore_ascii_case("skip"))
}

/// What a JSON log adds to a load report: the problems the tool logged
/// (stopping ones first) and tables whose checksum did not match.
fn tool_findings(lines: &mut Vec<Line>, state: &RunState, tool: &str) {
    if !state.checksum_mismatches.is_empty() {
        lines.push(Line::Blank);
        lines.push(Line::Warn(format!(
            "Checksum differs for {}",
            state.checksum_mismatches.join(", ")
        )));
    }
    if state.problems.is_empty() {
        return;
    }
    lines.push(Line::Blank);
    lines.push(Line::Text(format!("{tool} reported:")));
    let mut problems: Vec<_> = state.problems.iter().collect();
    problems.sort_by_key(|p| std::cmp::Reverse(p.level));
    for problem in problems {
        let text = match problem.count {
            1 => problem.text.clone(),
            n => format!("{} (×{n})", problem.text),
        };
        // By level: myloader marks some warnings fatal and goes on.
        lines.push(if problem.level >= tool_event::Level::Error {
            Line::Fail(text)
        } else {
            Line::Warn(text)
        });
    }
}

#[cfg(all(test, unix))]
pub(crate) mod testing {
    //! Shared by the tests of this module and of `server`.
    use super::*;
    use crate::secrets::MemoryStore;
    use std::os::unix::fs::PermissionsExt;

    pub fn fake_tool(dir: &Path, name: &str, body: &str) -> PathBuf {
        fs::create_dir_all(dir).unwrap();
        let path = dir.join(name);
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// A context inside `temp`: config in `config/`, tools in `bin/`, home
    /// at `home/`, and an in-memory keychain.
    pub fn context(temp: &Path) -> (Context, Arc<MemoryStore>) {
        let secrets = Arc::new(MemoryStore::default());
        let ctx = Context {
            config_dir: temp.join("config"),
            home: Some(temp.join("home")),
            settings: Settings::default(),
            dirs: SearchDirs {
                path: vec![temp.join("bin")],
                usual: Vec::new(),
            },
            secrets: secrets.clone(),
        };
        (ctx, secrets)
    }

    pub fn write_profile(ctx: &Context, name: &str, text: &str) -> PathBuf {
        let path = crate::profile::path_for(&ctx.config_dir, name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::testing::*;
    use super::*;
    use crate::run::Event;
    use std::time::Duration;

    const TS: &str = "20260102-030405";
    const PROFILE: &str = "\
[client]
host = db.example.test
port = 3306
user = example_user
password = file-password

[mydumper]
verbose = 1

[slipdock]
dump-dir = ~/dumps
";

    const ZSTD_VERSION: &str = r#"echo "*** Zstandard CLI (64-bit) v1.5.7, by Yann Collet ***""#;

    fn version_only(name: &str) -> String {
        format!(r#"echo "{name} v1.0.19-1, built against MariaDB 10.8.8 with SSL support""#)
    }

    struct Setup {
        temp: tempfile::TempDir,
        ctx: Context,
        secrets: Arc<crate::secrets::MemoryStore>,
        cnf: PathBuf,
    }

    fn setup(profile_text: &str) -> Setup {
        let temp = tempfile::tempdir().unwrap();
        // macOS temp folders are reached through a symlink; resolve it so
        // expected paths match what the code canonicalizes.
        let root = temp.path().canonicalize().unwrap();
        let (ctx, secrets) = context(&root);
        fake_tool(&root.join("bin"), "mydumper", &version_only("mydumper"));
        fake_tool(&root.join("bin"), "myloader", &version_only("myloader"));
        fake_tool(&root.join("bin"), "zstd", ZSTD_VERSION);
        let cnf = write_profile(&ctx, "example", profile_text);
        Setup {
            temp,
            ctx,
            secrets,
            cnf,
        }
    }

    fn dump_options() -> DumpOptions {
        DumpOptions {
            profile: "example".into(),
            database: "example_db".into(),
            dry_run: true,
            ..Default::default()
        }
    }

    fn args(prepared: &Prepared) -> Vec<String> {
        prepared
            .invocation
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn dump_plan_uses_the_profile_dump_dir_and_the_profile_file() {
        let s = setup(PROFILE);
        let plan = plan_dump(&s.ctx, dump_options(), TS).unwrap();
        let root = s.ctx.home.as_ref().unwrap().join("dumps");
        let out = root.join("example_db__example__20260102-030405");
        assert_eq!(plan.out, out);
        assert!(root.is_dir(), "the dump root is created");
        assert_eq!(
            report::plain(&plan.summary),
            "slipdock dump\n  Profile   example  example_user@db.example.test:3306\n  Database  example_db\n  \
             Exclude   none\n  Into      ~/dumps/\n            example_db__example__20260102-030405\n"
        );

        let prepared = plan.prepare(&s.ctx).unwrap();
        assert_eq!(
            prepared.invocation.program,
            s.ctx.dirs.path[0].join("mydumper")
        );
        assert_eq!(
            args(&prepared),
            [
                "--defaults-file",
                s.cnf.to_str().unwrap(),
                "--database",
                "example_db",
                "--outputdir",
                out.to_str().unwrap(),
                "--compress",
                "ZSTD",
                "--logfile",
                &format!("{}.log", out.display()),
                // The fake tool answers --version with 1.0.19.
                "--machine-log-json",
            ]
        );
        assert_eq!(prepared.label, "Dumping example_db");
    }

    #[test]
    fn dump_plan_writes_the_omit_file_and_honours_out() {
        let s = setup(PROFILE);
        let out_root = s.temp.path().canonicalize().unwrap().join("elsewhere");
        let plan = plan_dump(
            &s.ctx,
            DumpOptions {
                exclude: vec!["logs, other_db.jobs".into()],
                out: Some(out_root.clone()),
                extra_args: vec!["--threads".into(), "2".into()],
                ..dump_options()
            },
            TS,
        )
        .unwrap();
        let omit = out_root.join("example_db__example__20260102-030405.omit");
        assert_eq!(
            fs::read_to_string(&omit).unwrap(),
            "example_db.logs\nother_db.jobs\n"
        );
        assert!(report::plain(&plan.summary).contains("  Exclude   logs, other_db.jobs\n"));

        let prepared = plan.prepare(&s.ctx).unwrap();
        let args = args(&prepared);
        assert_eq!(&args[2..4], ["--omit-from-file", omit.to_str().unwrap()]);
        assert_eq!(&args[args.len() - 2..], ["--threads", "2"]);
    }

    #[test]
    fn arguments_from_the_settings_go_into_a_private_copy_of_the_profile() {
        let mut s = setup(&PROFILE.replace("[slipdock]", "[mydumper]\nthreads = 4\n\n[slipdock]"));
        // Nothing set: the profile itself is the defaults file.
        let prepared = plan_dump(&s.ctx, dump_options(), TS)
            .unwrap()
            .prepare(&s.ctx)
            .unwrap();
        assert_eq!(PathBuf::from(&args(&prepared)[1]), s.cnf);

        s.ctx.settings.options.mydumper.arguments = "--threads 12 --routines".into();
        s.ctx.settings.options.myloader.arguments = "--threads=6".into();
        let plan = plan_dump(&s.ctx, dump_options(), TS).unwrap();
        let summary = report::plain(&plan.summary);
        assert!(
            summary.contains("  Arguments --threads 12 --routines (from the settings)\n"),
            "{summary}"
        );
        let prepared = plan.prepare(&s.ctx).unwrap();
        let copy = PathBuf::from(&args(&prepared)[1]);
        assert_ne!(copy, s.cnf);
        let text = fs::read_to_string(&copy).unwrap();
        // The profile's own value is replaced, and myloader gets its own.
        assert!(text.contains("threads = 12"), "{text}");
        assert!(!text.contains("threads = 4"), "{text}");
        assert!(text.contains("routines = TRUE"), "{text}");
        assert!(text.contains("[myloader]\nthreads = 6\n"), "{text}");
        assert!(
            fs::read_to_string(&s.cnf).unwrap().contains("threads = 4"),
            "the profile is untouched"
        );
    }

    #[test]
    fn a_log_level_goes_into_a_private_copy_of_the_profile() {
        let s = setup(PROFILE);
        let plan = plan_dump(
            &s.ctx,
            DumpOptions {
                log_level: Some(3),
                ..dump_options()
            },
            TS,
        )
        .unwrap();
        assert!(report::plain(&plan.summary).contains("  Log level 3 (this run only)\n"));
        let prepared = plan.prepare(&s.ctx).unwrap();
        let copy = PathBuf::from(&args(&prepared)[1]);
        assert_ne!(copy, s.cnf);
        let text = fs::read_to_string(&copy).unwrap();
        assert!(text.contains("[mydumper]\nverbose = 3\n"));
        assert!(text.contains("[myloader]\nverbose = 3\n"));
        assert!(
            text.contains("host = db.example.test"),
            "connection settings are kept"
        );
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&copy).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert_eq!(
            fs::read_to_string(&s.cnf).unwrap(),
            PROFILE,
            "the profile is untouched"
        );
        drop(prepared);
        assert!(!copy.exists(), "the copy is removed after the run");
    }

    #[test]
    fn a_keychain_password_is_added_for_real_runs_only() {
        let text =
            PROFILE.replace("password = file-password\n", "") + "password-store = keychain\n";
        let s = setup(&text);
        s.secrets.set("example", "keychain-password").unwrap();

        // Dry run: a copy is used (so the command has the same shape), but
        // the keychain is not read.
        let dry = plan_dump(&s.ctx, dump_options(), TS).unwrap();
        assert!(report::plain(&dry.summary).contains("  Password  from the system keychain\n"));
        let dry = dry.prepare(&s.ctx).unwrap();
        assert!(
            !fs::read_to_string(&args(&dry)[1])
                .unwrap()
                .contains("keychain-password")
        );

        let real = plan_dump(
            &s.ctx,
            DumpOptions {
                dry_run: false,
                ..dump_options()
            },
            TS,
        )
        .unwrap();
        let real = real.prepare(&s.ctx).unwrap();
        let copy = fs::read_to_string(&args(&real)[1]).unwrap();
        assert!(copy.contains("password = keychain-password\n"));

        s.secrets.clear("example").unwrap();
        let missing = plan_dump(
            &s.ctx,
            DumpOptions {
                dry_run: false,
                ..dump_options()
            },
            TS,
        )
        .unwrap();
        let error = missing.prepare(&s.ctx).err().unwrap().to_string();
        assert!(error.contains("slipdock password set example"), "{error}");
    }

    #[test]
    fn dump_plan_errors_name_the_fix() {
        let s = setup(&PROFILE.replace("dump-dir = ~/dumps", "dump-dir ="));
        let error = |options| plan_dump(&s.ctx, options, TS).err().unwrap().to_string();
        assert!(error(dump_options()).contains("has no dump-dir"));
        assert!(
            error(DumpOptions {
                profile: "nope".into(),
                ..dump_options()
            })
            .contains("slipdock init nope")
        );
        assert!(
            error(DumpOptions {
                log_level: Some(4),
                ..dump_options()
            })
            .contains("must be 0-3")
        );
        assert!(
            error(DumpOptions {
                database: "a/b".into(),
                ..dump_options()
            })
            .contains("not a usable database name")
        );
    }

    #[test]
    fn tls_off_adds_the_note_and_the_variable() {
        let s = setup(&(PROFILE.to_string() + "tls-verify = FALSE\n"));
        let plan = plan_dump(&s.ctx, dump_options(), TS).unwrap();
        assert!(report::plain(&plan.summary).contains("TLS certificate check is off"));
        let prepared = plan.prepare(&s.ctx).unwrap();
        assert_eq!(
            prepared.invocation.env[0].0,
            "MARIADB_TLS_DISABLE_PEER_VERIFICATION"
        );
    }

    /// A finished dump folder with two tables.
    fn dump_fixture(dir: &Path, complete: bool) {
        fs::create_dir_all(dir).unwrap();
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
    }

    fn load_options(dir: &Path) -> LoadOptions {
        LoadOptions {
            profile: "example".into(),
            dir: dir.to_path_buf(),
            dry_run: true,
            ..Default::default()
        }
    }

    #[test]
    fn a_load_needs_zstd_only_for_a_compressed_dump() {
        let mut s = setup(PROFILE);
        let home = s.ctx.home.clone().unwrap();
        let compressed = home.join("dumps/example_db__example__20260101-000000");
        dump_fixture(&compressed, true);
        let plain = home.join("dumps/example_db__example__20260102-000000");
        fs::create_dir_all(&plain).unwrap();
        for file in ["metadata", "example_db-schema-create.sql"] {
            fs::write(plain.join(file), "x").unwrap();
        }
        let env = |ctx: &Context, dir: &Path| {
            let plan = plan_load(ctx, load_options(dir))?;
            Ok::<_, Error>(plan.prepare(ctx, TS)?.invocation.env)
        };

        // zstd is off PATH, in a folder Slipdock also looks in.
        let bin = s.ctx.dirs.path[0].clone();
        let brew = bin.with_file_name("brew-bin");
        fs::create_dir_all(&brew).unwrap();
        fs::rename(bin.join("zstd"), brew.join("zstd")).unwrap();
        s.ctx.dirs.usual = vec![brew.clone()];
        assert_eq!(
            env(&s.ctx, &compressed).unwrap(),
            [(
                "PATH".to_string(),
                format!("{}:{}", brew.display(), bin.display())
            )]
        );
        assert!(env(&s.ctx, &plain).unwrap().is_empty());

        // No zstd anywhere: only the compressed dump is refused.
        fs::remove_file(brew.join("zstd")).unwrap();
        let error = env(&s.ctx, &compressed).err().unwrap().to_string();
        assert_eq!(
            error,
            "zstd was not found. See: slipdock tools install zstd"
        );
        assert!(env(&s.ctx, &plain).unwrap().is_empty());
    }

    #[test]
    fn load_plan_takes_the_target_from_the_dump_and_builds_the_command() {
        let s = setup(PROFILE);
        let dir = s
            .ctx
            .home
            .as_ref()
            .unwrap()
            .join("dumps/example_db__example__20260101-000000");
        dump_fixture(&dir, true);

        let plan = plan_load(&s.ctx, load_options(&dir)).unwrap();
        assert_eq!(plan.target, "example_db");
        assert_eq!(plan.server, "example_user@db.example.test:3306");
        assert!(plan.confirms("example_db"));
        assert!(!plan.confirms("example_db "));
        assert!(!plan.confirms(""));
        assert_eq!(
            report::plain(&plan.summary),
            "slipdock load\n  Profile   example  example_user@db.example.test:3306\n  \
             Dump      example_db__example__20260101-000000  2 tables · 4B\n  In        ~/dumps/\n  \
             Target    example_db  tables in the dump are dropped and recreated\n"
        );

        let prepared = plan.prepare(&s.ctx, TS).unwrap();
        let log = format!("{}__load_example_{TS}.log", dir.display());
        assert_eq!(
            args(&prepared),
            [
                "--defaults-file",
                s.cnf.to_str().unwrap(),
                "--directory",
                dir.to_str().unwrap(),
                "--database",
                "example_db",
                "--drop-table",
                "DROP",
                "--logfile",
                &log,
                "--machine-log-json",
            ]
        );
        assert_eq!(prepared.label, "Loading into example_db");
    }

    #[test]
    fn load_plan_accepts_another_target_and_rejects_bad_folders() {
        let s = setup(PROFILE);
        let dumps = s.ctx.home.as_ref().unwrap().join("dumps");
        let good = dumps.join("good");
        let partial = dumps.join("partial");
        dump_fixture(&good, true);
        dump_fixture(&partial, false);

        let plan = plan_load(
            &s.ctx,
            LoadOptions {
                target: Some("copy_db".into()),
                ..load_options(&good)
            },
        )
        .unwrap();
        assert_eq!(plan.target, "copy_db");

        let error = |dir: &Path| {
            plan_load(&s.ctx, load_options(dir))
                .err()
                .unwrap()
                .to_string()
        };
        assert!(error(&partial).contains("is not a mydumper dump (no metadata file)"));
        assert!(error(&dumps.join("missing")).starts_with("dump folder not found"));
        // A dump whose source database cannot be told needs an explicit target.
        fs::remove_file(good.join("example_db-schema-create.sql.zst")).unwrap();
        assert!(error(&good).contains("could not detect the database name"));
    }

    #[test]
    fn a_relative_path_that_exists_under_the_root_gets_the_slash_hint() {
        let s = setup(PROFILE);
        let dir = s.temp.path().canonicalize().unwrap().join("dump");
        dump_fixture(&dir, true);
        let relative = dir.strip_prefix("/").unwrap();
        let error = plan_load(&s.ctx, load_options(relative))
            .err()
            .unwrap()
            .to_string();
        assert!(error.contains("(missing leading /)"), "{error}");
    }

    // --- Runs with fake tools --------------------------------------------------

    fn run_to_end(prepared: &Prepared) -> Outcome {
        let running = prepared.start().unwrap();
        for event in &running.events {
            if let Event::Finished(outcome) = event {
                return outcome;
            }
        }
        panic!("no Finished event");
    }

    /// A mydumper that writes a small dump into --outputdir, logs to
    /// --logfile and exits with `$FAKE_EXIT`-like fixed code.
    fn fake_mydumper(s: &Setup, write_metadata: bool, log_line: &str, exit: i32) {
        let metadata = if write_metadata {
            r#"echo done > "$out/metadata""#
        } else {
            ""
        };
        let body = format!(
            r#"if [ "$1" = --version ]; then {version}; exit 0; fi
while [ $# -gt 0 ]; do
  case "$1" in
    --outputdir) out="$2"; shift ;;
    --logfile) log="$2"; shift ;;
  esac
  shift
done
mkdir -p "$out"
echo x > "$out/example_db-schema-create.sql.zst"
echo x > "$out/example_db.users-schema.sql.zst"
{metadata}
printf '%s' '{log_line}' > "$log"
echo "printed by the tool" >&2
exit {exit}"#,
            version = version_only("mydumper"),
        );
        fake_tool(&s.ctx.dirs.path[0], "mydumper", &body);
    }

    fn real_dump(s: &Setup, level: Option<u8>) -> (Prepared, Outcome) {
        let options = DumpOptions {
            dry_run: false,
            log_level: level,
            ..dump_options()
        };
        let prepared = plan_dump(&s.ctx, options, TS)
            .unwrap()
            .prepare(&s.ctx)
            .unwrap();
        let outcome = run_to_end(&prepared);
        (prepared, outcome)
    }

    /// An app started from the Dock has a short PATH. mydumper is found in a
    /// usual folder all the same, but it looks for zstd on its own PATH.
    #[test]
    fn mydumper_gets_a_path_that_leads_to_zstd() {
        let mut s = setup(PROFILE);
        let bin = s.ctx.dirs.path[0].clone();
        let brew = bin.with_file_name("brew-bin");
        fs::rename(bin.join("zstd"), {
            fs::create_dir_all(&brew).unwrap();
            brew.join("zstd")
        })
        .unwrap();
        // What a short PATH still has, and the folder Slipdock also looks in.
        s.ctx.dirs.path = vec![PathBuf::from("/usr/bin"), PathBuf::from("/bin")];
        s.ctx.dirs.usual = vec![bin.clone(), brew.clone()];
        let body = format!(
            r#"if [ "$1" = --version ]; then {version}; exit 0; fi
while [ $# -gt 0 ]; do
  case "$1" in --outputdir) out="$2"; shift ;; esac
  shift
done
if ! command -v zstd >/dev/null; then echo "zstd was not found in PATH" >&2; exit 1; fi
mkdir -p "$out" && echo done > "$out/metadata""#,
            version = version_only("mydumper"),
        );
        fake_tool(&bin, "mydumper", &body);

        let (prepared, outcome) = real_dump(&s, None);
        assert_eq!(
            prepared.invocation.env,
            [(
                "PATH".to_string(),
                format!("{}:/usr/bin:/bin", brew.display())
            )]
        );
        assert_eq!(outcome.output_head, [] as [&str; 0]);
        assert!(prepared.report(&outcome).success);
    }

    #[test]
    fn zstd_on_the_path_changes_nothing_and_a_missing_one_stops_the_dump() {
        let s = setup(PROFILE);
        let plan = plan_dump(&s.ctx, dump_options(), TS).unwrap();
        assert!(plan.prepare(&s.ctx).unwrap().invocation.env.is_empty());

        fs::remove_file(s.ctx.dirs.path[0].join("zstd")).unwrap();
        let error = plan_dump(&s.ctx, dump_options(), TS).err().unwrap();
        assert_eq!(
            error.to_string(),
            "zstd was not found. See: slipdock tools install zstd"
        );
    }

    #[test]
    fn a_complete_dump_is_a_success() {
        let s = setup(PROFILE);
        fake_mydumper(&s, true, "", 0);
        let (prepared, outcome) = real_dump(&s, None);
        let report = prepared.report(&outcome);
        assert!(report.success);
        let text = report.plain();
        assert!(
            text.starts_with("✓ Dump complete  0:00 · 1 tables · "),
            "{text}"
        );
        assert!(text.contains("\n  Folder    ~/dumps/example_db__example__20260102-030405\n"));
        assert!(text.ends_with("  Log       ~/dumps/example_db__example__20260102-030405.log"));
    }

    #[test]
    fn a_dump_without_metadata_did_not_finish_and_nothing_is_deleted() {
        let s = setup(PROFILE);
        fake_mydumper(&s, false, "", 1);
        let (prepared, outcome) = real_dump(&s, None);
        let report = prepared.report(&outcome);
        assert!(!report.success);
        assert_eq!(
            report.plain(),
            "  │ printed by the tool\n✗ Dump did not finish (exit 1) after 0:00\n  \
             Partial   ~/dumps/example_db__example__20260102-030405\n  \
             Log       ~/dumps/example_db__example__20260102-030405.log"
        );
        let out = s
            .ctx
            .home
            .as_ref()
            .unwrap()
            .join("dumps/example_db__example__20260102-030405");
        assert!(out.join("example_db.users-schema.sql.zst").exists());
    }

    #[test]
    fn a_complete_dump_with_a_non_zero_exit_lists_what_the_log_says() {
        let s = setup(PROFILE);
        let log = "2026-01-02 03:04:05 [WARNING] - Couldn\\047t get master position\\n";
        fake_mydumper(&s, true, "", 1);
        // Write the log line with printf so the quote survives the shell.
        let script = s.ctx.dirs.path[0].join("mydumper");
        let body = fs::read_to_string(&script)
            .unwrap()
            .replace("printf '%s' ''", &format!("printf '{log}'"));
        fs::write(&script, body).unwrap();

        let (prepared, outcome) = real_dump(&s, None);
        let report = prepared.report(&outcome);
        assert!(report.success, "the dump is complete");
        let text = report.plain();
        assert!(text.contains("! mydumper exited with 1, but the dump is complete. It reported:\n    Couldn't get master position"), "{text}");
    }

    #[test]
    fn a_silent_non_zero_exit_explains_the_log_level() {
        let s = setup(PROFILE); // verbose = 1 in the profile
        fake_mydumper(&s, true, "", 1);
        let (prepared, outcome) = real_dump(&s, None);
        let text = prepared.report(&outcome).plain();
        assert!(
            text.contains("Nothing recorded: this profile logs errors only (verbose = 1)."),
            "{text}"
        );
        assert!(text.contains("Run again with --log-level 2"));

        // With --log-level 2 the hint changes: the level is not the reason.
        let (prepared, outcome) = real_dump(&s, Some(2));
        let text = prepared.report(&outcome).plain();
        assert!(
            text.contains("Nothing recorded in the log; see the tool output above, if any."),
            "{text}"
        );
    }

    #[test]
    fn load_reports_follow_the_exit_code() {
        let s = setup(&PROFILE.replace("[slipdock]", "[myloader]\nverbose = 1\n\n[slipdock]"));
        let dir = s.ctx.home.as_ref().unwrap().join("dumps/d");
        dump_fixture(&dir, true);
        let run = |exit: i32| {
            let body = format!(
                "if [ \"$1\" = --version ]; then {}; exit 0; fi\nexit {exit}",
                version_only("myloader")
            );
            fake_tool(&s.ctx.dirs.path[0], "myloader", &body);
            let options = LoadOptions {
                dry_run: false,
                ..load_options(&dir)
            };
            let prepared = plan_load(&s.ctx, options)
                .unwrap()
                .prepare(&s.ctx, TS)
                .unwrap();
            let outcome = run_to_end(&prepared);
            prepared.report(&outcome)
        };

        let ok = run(0);
        assert!(ok.success);
        assert_eq!(
            ok.plain(),
            format!(
                "✓ Load complete  0:00 · 2 tables into example_db\n  Log       ~/dumps/d__load_example_{TS}.log"
            )
        );

        let failed = run(1);
        assert!(!failed.success);
        let text = failed.plain();
        assert!(
            text.starts_with(
                "✗ Load failed (exit 1) after 0:00\n  Log       ~/dumps/d__load_example_"
            ),
            "{text}"
        );
        assert!(text.contains("The log is empty: this profile logs errors only (verbose = 1),"));
    }

    #[test]
    fn a_load_report_shows_what_the_json_log_says() {
        let s = setup(PROFILE);
        let dir = s.ctx.home.as_ref().unwrap().join("dumps/d");
        dump_fixture(&dir, true);
        // The fake myloader writes `log_lines` to its --logfile.
        let run = |log_lines: &str, exit: i32| {
            let body = format!(
                "if [ \"$1\" = --version ]; then {}; exit 0; fi\n\
                 while [ $# -gt 0 ]; do [ \"$1\" = --logfile ] && log=\"$2\"; shift; done\n\
                 cat > \"$log\" <<'EOF'\n{log_lines}EOF\nexit {exit}",
                version_only("myloader")
            );
            fake_tool(&s.ctx.dirs.path[0], "myloader", &body);
            let options = LoadOptions {
                dry_run: false,
                ..load_options(&dir)
            };
            let prepared = plan_load(&s.ctx, options)
                .unwrap()
                .prepare(&s.ctx, TS)
                .unwrap();
            let outcome = run_to_end(&prepared);
            prepared.report(&outcome)
        };

        let ok = run(include_str!("../tests/data/load.jsonl"), 0);
        assert!(ok.success);
        let text = ok.plain();
        assert!(
            text.starts_with("✓ Load complete  0:00 · 3 tables · 18 files into example_db"),
            "{text}"
        );
        // The GLib assertion in the log is not reported.
        assert!(!text.contains("reported"), "{text}");

        let failed = run(
            "{\"ts\":\"2026-11-27 16:51:50.1\",\"level\":\"ERROR\",\"event\":\"mysql_connect\",\"status\":\"failed\",\"message\":\"Error connection to database\",\"fatal\":true,\"mysql_errno\":\"2002\"}\n",
            1,
        );
        assert!(!failed.success);
        let text = failed.plain();
        assert!(
            text.contains(
                "myloader reported:\n✗ ERROR: Error connection to database (MySQL error 2002)"
            ),
            "{text}"
        );
    }

    #[test]
    fn ignore_errors_is_read_from_the_settings_the_profile_and_the_extra_arguments() {
        let mut s = setup(&PROFILE.replace(
            "[slipdock]",
            "[myloader]\nignore-errors = 1146\n\n[slipdock]",
        ));
        s.ctx.settings.options.myloader.arguments = "--threads 4 --ignore-errors 1227,1062".into();
        let profile = s.ctx.profile("example").unwrap();
        let extra = ["--ignore-errors=1050".to_string()];
        assert_eq!(
            ignored_errors(&s.ctx, &profile, "myloader", &extra),
            ["1050", "1062", "1146", "1227"]
        );
        assert!(ignored_errors(&s.ctx, &profile, "mydumper", &[]).is_empty());
    }

    #[test]
    fn checksum_skip_is_read_from_wherever_it_is_set_and_the_last_wins() {
        let mut s = setup(PROFILE);
        let profile = s.ctx.profile("example").unwrap();
        assert!(!skips_checksums(&s.ctx, &profile, &[]));
        s.ctx.settings.options.myloader.arguments = "--threads 4 --checksum skip".into();
        assert!(skips_checksums(&s.ctx, &profile, &[]));
        // A run's own argument comes last and wins.
        assert!(!skips_checksums(
            &s.ctx,
            &profile,
            &["--checksum=warn".to_string()]
        ));
        assert!(skips_checksums(
            &s.ctx,
            &profile,
            &["--checksum".to_string(), "SKIP".to_string()]
        ));
    }

    #[test]
    fn an_interrupted_dump_says_so_and_exits_130() {
        let s = setup(PROFILE);
        let body = format!(
            "if [ \"$1\" = --version ]; then {}; exit 0; fi\nexec sleep 30",
            version_only("mydumper")
        );
        fake_tool(&s.ctx.dirs.path[0], "mydumper", &body);
        let options = DumpOptions {
            dry_run: false,
            ..dump_options()
        };
        let prepared = plan_dump(&s.ctx, options, TS)
            .unwrap()
            .prepare(&s.ctx)
            .unwrap();
        let running = prepared.start().unwrap();
        running.cancel.cancel();
        let outcome = running
            .events
            .iter()
            .find_map(|event| match event {
                Event::Finished(outcome) => Some(outcome),
                Event::Progress(_) => None,
            })
            .unwrap();
        assert!(outcome.elapsed < Duration::from_secs(10));
        let report = prepared.report(&outcome);
        assert!(!report.success);
        assert!(
            report
                .plain()
                .starts_with("! Interrupted.\n✗ Dump did not finish (exit 130)")
        );
    }
}
