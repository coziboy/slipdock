//! What each command does. Every function returns the process exit code.
//!
//! These functions are thin: they call the core, then print. Anything worth
//! testing in isolation lives in `slipdock-core`.

use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use slipdock_core::job::{self, Context, DumpOptions, LoadOptions, Prepared};
use slipdock_core::platform::{self, PackageManager};
use slipdock_core::profile::{self, PasswordStore, SECTION};
use slipdock_core::report;
use slipdock_core::run::{Event, Outcome};
use slipdock_core::settings::Settings;
use slipdock_core::tools::{self, InstallPlan, Tool};
use slipdock_core::{Error, Result, dumps};

use crate::cli::{PasswordCommand, RunOptions, Shell, ToolName, ToolsCommand};
use crate::ui;

const OK: i32 = 0;
const FAILED: i32 = 1;

// --- Profiles ----------------------------------------------------------------------

pub fn init(ctx: &Context, name: &str) -> Result<i32> {
    let path = profile::init(&ctx.config_dir, name)?;
    println!(
        "Created {} (mode 600). Edit it, then: slipdock dump {name} <db>",
        path.display()
    );
    Ok(OK)
}

pub fn profiles(ctx: &Context) -> Result<i32> {
    for profile in profile::list(&ctx.config_dir) {
        let mut notes = String::new();
        if profile.tls_verify_off() {
            notes.push_str("  (TLS cert check off)");
        }
        if profile.password_store() == PasswordStore::Keychain {
            notes.push_str("  (password in keychain)");
        }
        println!("{:<22} {}{notes}", profile.name, profile.target());
    }
    Ok(OK)
}

pub fn import(ctx: &Context, from: Option<PathBuf>) -> Result<i32> {
    let from = match (from, &ctx.home) {
        (Some(from), _) => from,
        (None, Some(home)) => profile::mydb_dir(home),
        (None, None) => {
            return Err(Error::msg(
                "cannot find your home folder; pass --from <dir>",
            ));
        }
    };
    let report = profile::import(&from, &ctx.config_dir)?;
    for name in &report.imported {
        println!("imported  {name}");
    }
    for name in &report.skipped {
        println!("skipped   {name}  (already exists in Slipdock)");
    }
    if report.imported.is_empty() && report.skipped.is_empty() {
        println!("No *.cnf files in {}", from.display());
    } else {
        println!(
            "Profiles are in {}. The files in {} were not changed.",
            profile::profiles_dir(&ctx.config_dir).display(),
            from.display()
        );
    }
    Ok(OK)
}

// --- Dump and load -------------------------------------------------------------------

pub struct DumpArgs {
    pub profile: String,
    pub db: String,
    pub exclude: Vec<String>,
    pub out: Option<PathBuf>,
    pub extra: Vec<String>,
}

pub fn dump(ctx: &Context, args: DumpArgs, run: &RunOptions) -> Result<i32> {
    let options = DumpOptions {
        profile: args.profile,
        database: args.db,
        exclude: args.exclude,
        out: args.out,
        log_level: run.log_level,
        extra_args: args.extra,
        dry_run: run.dry_run,
    };
    let interrupt = watch_for_interrupt()?;
    let plan = job::plan_dump(ctx, options, &job::timestamp())?;
    ui::print_lines(&plan.summary);
    let prepared = plan.prepare(ctx)?;
    execute(&prepared, run, &interrupt)
}

pub struct LoadArgs {
    pub profile: String,
    pub dir: PathBuf,
    pub target: Option<String>,
    pub confirm: Option<String>,
    pub extra: Vec<String>,
}

pub fn load(ctx: &Context, args: LoadArgs, run: &RunOptions) -> Result<i32> {
    let options = LoadOptions {
        profile: args.profile,
        dir: args.dir,
        target: args.target,
        log_level: run.log_level,
        extra_args: args.extra,
        dry_run: run.dry_run,
    };
    let plan = job::plan_load(ctx, options)?;
    ui::print_lines(&plan.summary);

    // The guard. A load drops tables, so the user names the database once
    // more: at the prompt, or with --confirm when no one is there to ask.
    // A dry run changes nothing and needs no guard.
    if !run.dry_run {
        match &args.confirm {
            Some(name) if plan.confirms(name) => {}
            Some(name) => {
                return Err(Error::msg(format!(
                    "--confirm {name} does not match the target database {}",
                    plan.target
                )));
            }
            None if ui::is_interactive() => {
                let question = format!("Type {} to continue: ", ui::bold(&plan.target));
                let answer = ui::ask(&question).unwrap_or_default();
                if !plan.confirms(&answer) {
                    println!("Aborted.");
                    return Ok(FAILED);
                }
                println!();
            }
            None => {
                return Err(Error::msg(format!(
                    "refusing to load without confirmation. On a terminal you are asked; \
                     otherwise pass --confirm {}",
                    plan.target
                )));
            }
        }
    }

    // From here on a temporary file with a password may exist, so Ctrl-C is
    // caught and handled in `execute` instead of ending the process at once.
    let interrupt = watch_for_interrupt()?;
    let prepared = plan.prepare(ctx, &job::timestamp())?;
    execute(&prepared, run, &interrupt)
}

/// Makes Ctrl-C (and a polite `kill`) set a flag instead of ending the
/// process. Without this the process would die on the spot and leave the
/// temporary defaults file behind.
fn watch_for_interrupt() -> Result<Arc<AtomicBool>> {
    let flag = Arc::new(AtomicBool::new(false));
    for signal in [signal_hook::consts::SIGINT, signal_hook::consts::SIGTERM] {
        signal_hook::flag::register(signal, Arc::clone(&flag)).map_err(|source| Error::Io {
            context: "cannot watch for Ctrl-C".into(),
            source,
        })?;
    }
    Ok(flag)
}

/// Shows the command if asked, runs it, shows progress and the report.
fn execute(prepared: &Prepared, run: &RunOptions, interrupt: &AtomicBool) -> Result<i32> {
    if run.dry_run || run.show_command {
        println!("{}", prepared.invocation.preview());
    }
    if run.dry_run {
        anstream::println!("{}", ui::dim("(dry run, nothing executed)"));
        return Ok(OK);
    }
    if interrupt.load(Ordering::SeqCst) {
        ui::warn("Interrupted.");
        return Ok(130);
    }

    let running = prepared.start()?;
    let mut line = ui::ProgressLine::new();
    let mut outcome: Option<Outcome> = None;
    // The loop ends when the watcher thread has sent Finished and dropped
    // its end of the channel.
    for event in &running.events {
        if interrupt.load(Ordering::SeqCst) {
            running.cancel.cancel();
        }
        match event {
            Event::Progress(progress) => {
                let detail = match progress.written {
                    Some((files, size)) => {
                        format!(" · {files} files · {}", dumps::human_size(size))
                    }
                    None => String::new(),
                };
                line.draw(
                    &prepared.label,
                    &report::duration(progress.elapsed),
                    &detail,
                );
            }
            Event::Finished(finished) => outcome = Some(finished),
        }
    }
    line.clear();
    let outcome = outcome.ok_or_else(|| Error::msg("the run ended without a result"))?;

    let report = prepared.report(&outcome);
    if report.success {
        ui::print_lines(&report.lines);
        Ok(OK)
    } else {
        ui::print_lines_err(&report.lines);
        Ok(if outcome.interrupted { 130 } else { FAILED })
    }
}

// --- Dumps ---------------------------------------------------------------------------

pub fn dumps(ctx: &Context, profile: Option<&str>) -> Result<i32> {
    let home = ctx.home.as_deref();
    // One profile's folder, or every distinct profile folder.
    let roots = match profile {
        Some(name) => vec![ctx.profile(name)?.need_dump_dir(home)?],
        None => dumps::roots(&profile::list(&ctx.config_dir), home),
    };
    for root in roots.iter().filter(|root| root.is_dir()) {
        println!("{}", root.display());
        for dump in dumps::list(root) {
            let partial = if dump.complete { "" } else { "  (partial)" };
            anstream::println!(
                "  {:>8}  {}{}",
                dumps::human_size(dump.size),
                dump.name,
                ui::dim(partial)
            );
        }
    }
    Ok(OK)
}

// --- Tools ---------------------------------------------------------------------------

impl From<ToolName> for Tool {
    fn from(name: ToolName) -> Tool {
        match name {
            ToolName::Mydumper => Tool::Mydumper,
            ToolName::Myloader => Tool::Myloader,
            ToolName::Mysql => Tool::Mysql,
        }
    }
}

pub fn tools(ctx: &Context, command: Option<ToolsCommand>) -> Result<i32> {
    match command {
        None => tools_status(ctx),
        Some(ToolsCommand::Install { tool }) => tools_install(ctx, tool.map(Tool::from)),
        Some(ToolsCommand::Set { tool, path }) => tools_set(ctx, tool.into(), &path),
        Some(ToolsCommand::Unset { tool }) => {
            let tool = Tool::from(tool);
            let mut settings = ctx.settings.clone();
            settings.unset_tool_path(tool.name());
            settings.save(&ctx.config_dir)?;
            println!("{tool}: searching PATH and the usual locations again");
            Ok(OK)
        }
    }
}

fn tools_status(ctx: &Context) -> Result<i32> {
    let mut all_required_found = true;
    for tool in Tool::ALL {
        let status = tools::status(tool, &ctx.settings, &ctx.dirs);
        let chosen = status.chosen();
        match chosen {
            Some(candidate) => {
                let working = candidate
                    .result
                    .as_ref()
                    .expect("the chosen candidate works");
                let source = if status.configured.is_some() {
                    "  (set by you)"
                } else {
                    ""
                };
                anstream::println!(
                    "{}  {}  {}{}",
                    ui::bold(&format!("{:<9}", tool.name())),
                    ctx.show_path(&candidate.path),
                    working.version,
                    ui::dim(source)
                );
                if let Some(warning) = working.version_warning(tool) {
                    ui::warn(&warning);
                }
            }
            None => {
                anstream::println!(
                    "{}  {}",
                    ui::bold(&format!("{:<9}", tool.name())),
                    status.problem().unwrap_or_default()
                );
                if tool != Tool::Mysql {
                    all_required_found = false;
                }
            }
        }
        // Everything else that was found, so a broken copy is no mystery.
        for candidate in &status.candidates {
            if chosen.is_some_and(|c| c.path == candidate.path) {
                continue;
            }
            let note = match &candidate.result {
                Ok(working) => format!("{}  also works", working.version),
                Err(problem) => problem.to_string(),
            };
            anstream::println!(
                "           {}",
                ui::dim(&format!("{}  {note}", ctx.show_path(&candidate.path)))
            );
        }
    }
    anstream::println!(
        "{}",
        ui::dim("mysql is optional: it lists databases and tables to pick from.")
    );
    Ok(if all_required_found { OK } else { FAILED })
}

fn tools_set(ctx: &Context, tool: Tool, path: &Path) -> Result<i32> {
    let path = std::path::absolute(path).map_err(Error::io_at("cannot use", path))?;
    // Check before saving: a path that does not work helps no one.
    let working = tools::probe(tool, &path)
        .map_err(|problem| Error::msg(format!("{} {problem}", path.display())))?;
    let mut settings: Settings = ctx.settings.clone();
    settings.set_tool_path(tool.name(), path.clone());
    settings.save(&ctx.config_dir)?;
    println!("{tool}: using {} ({})", path.display(), working.version);
    Ok(OK)
}

fn tools_install(ctx: &Context, only: Option<Tool>) -> Result<i32> {
    let wanted: Vec<Tool> = match only {
        Some(tool) => vec![tool],
        None => Tool::ALL
            .into_iter()
            .filter(|&tool| {
                tools::status(tool, &ctx.settings, &ctx.dirs)
                    .chosen()
                    .is_none()
            })
            .collect(),
    };
    if wanted.is_empty() {
        println!("Nothing to install: mydumper, myloader and mysql all work.");
        return Ok(OK);
    }
    let mut search = ctx.dirs.path.clone();
    search.extend(ctx.dirs.usual.iter().cloned());
    let managers: Vec<PackageManager> = platform::package_managers(&search);

    let mut done: Vec<InstallPlan> = Vec::new();
    for tool in wanted {
        let plan = tools::install_plan(tool, platform::current_os(), &managers);
        // mydumper and myloader share one package: do not offer it twice.
        if done.contains(&plan) {
            continue;
        }
        anstream::println!("\n{}", ui::bold(tool.name()));
        match &plan {
            InstallPlan::Command { program, args } => {
                println!("  {program} {}", args.join(" "));
                if ui::is_interactive() && ui::ask_yes_no("Run this now?") {
                    let status = Process::new(program)
                        .args(args)
                        .status()
                        .map_err(|source| Error::Io {
                            context: format!("cannot run {program}"),
                            source,
                        })?;
                    if !status.success() {
                        return Err(Error::msg(format!("{program} did not finish successfully")));
                    }
                } else {
                    println!("  (not run)");
                }
            }
            InstallPlan::Steps(steps) => {
                for step in steps {
                    println!("  {step}");
                }
            }
            InstallPlan::Unsupported(why) => println!("  {why}"),
        }
        done.push(plan);
    }
    println!("\nAfterwards, check with: slipdock tools");
    Ok(OK)
}

// --- Passwords -----------------------------------------------------------------------

pub fn password(ctx: &Context, command: PasswordCommand) -> Result<i32> {
    match command {
        PasswordCommand::Set { profile: name } => {
            let mut profile = ctx.profile(&name)?;
            let question = format!("Password for {} ({}): ", name, profile.target());
            let password = rpassword::prompt_password(question).map_err(|source| Error::Io {
                context: "cannot read the password".into(),
                source,
            })?;
            if password.is_empty() {
                return Err(Error::msg("no password given; nothing changed"));
            }
            // Keychain first: if it refuses, the file still has the password.
            ctx.secrets.set(&name, &password)?;
            profile.doc.set(SECTION, "password-store", "keychain")?;
            profile.doc.remove("client", "password");
            profile.save()?;
            println!(
                "Stored in the system keychain. The password line was removed from {}.",
                profile.path.display()
            );
        }
        PasswordCommand::Clear { profile: name } => {
            let mut profile = ctx.profile(&name)?;
            ctx.secrets.clear(&name)?;
            profile.doc.remove(SECTION, "password-store");
            profile.save()?;
            println!(
                "Removed from the system keychain. Add the password to {} again:\n  \
                 password = <password>   (under [client])",
                profile.path.display()
            );
        }
    }
    Ok(OK)
}

// --- Completions and the app ------------------------------------------------------------

pub fn completions(shell: Shell) -> Result<i32> {
    let name = match shell {
        Shell::Zsh => "zsh",
        Shell::Bash => "bash",
        Shell::Fish => "fish",
    };
    // The script is small: it makes the shell call `slipdock` itself for
    // suggestions, which is how profile names can be completed.
    let shells = clap_complete::env::Shells::builtins();
    let completer = shells
        .completer(name)
        .ok_or_else(|| Error::msg(format!("no completions for {name}")))?;
    completer
        .write_registration(
            "COMPLETE",
            "slipdock",
            "slipdock",
            "slipdock",
            &mut std::io::stdout(),
        )
        .map_err(|source| Error::Io {
            context: "cannot write the completion script".into(),
            source,
        })?;
    Ok(OK)
}

/// Starts `slipdock-app`: the one beside this program, else one on PATH.
pub fn app(ctx: &Context) -> Result<i32> {
    const NAME: &str = "slipdock-app";
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(NAME)));
    let found = beside
        .into_iter()
        .chain(ctx.dirs.path.iter().map(|dir| dir.join(NAME)))
        .find(|path| platform::is_executable(path));
    let Some(path) = found else {
        return Err(Error::msg(
            "the desktop app is not installed (no slipdock-app beside slipdock or on PATH)",
        ));
    };
    let mut process = Process::new(&path);
    process.env("SLIPDOCK_CONFIG_DIR", &ctx.config_dir);
    process
        .spawn()
        .map_err(Error::io_at("cannot start", &path))?;
    Ok(OK)
}
