//! Guided mode: `slipdock` without a command, on a terminal.
//!
//! It asks a few questions, then runs the same `dump` or `load` command you
//! could have typed, and shows that command first so it can be learned.

use slipdock_core::job::Context;
use slipdock_core::{Result, command, dumps, profile, server};

use crate::cli::RunOptions;
use crate::commands::{self, DumpArgs, LoadArgs};
use crate::ui;

const CANCELLED: i32 = 1;

pub fn run(ctx: &Context) -> Result<i32> {
    let actions = [
        "dump      back up a database",
        "load      restore a dump into a database",
        "dumps     list dumps",
        "profiles  list profiles",
        "init      create a profile",
        "tools     check mydumper, myloader, mysql and zstd",
    ]
    .map(String::from);
    let Some(action) = ui::pick("slipdock", &actions) else {
        return Ok(CANCELLED);
    };
    match action {
        0 => dump(ctx),
        1 => load(ctx),
        2 => commands::dumps(ctx, None),
        3 => commands::profiles(ctx),
        4 => {
            let Some(name) = ui::ask("New profile name: ").filter(|name| !name.trim().is_empty())
            else {
                return Ok(CANCELLED);
            };
            commands::init(ctx, name.trim())
        }
        _ => commands::tools(ctx, None),
    }
}

fn pick_profile(ctx: &Context) -> Result<Option<profile::Profile>> {
    let names = profile::names(&ctx.config_dir);
    if names.is_empty() {
        println!("No profiles yet. Run: slipdock init <name>");
        return Ok(None);
    }
    let Some(index) = ui::pick("Profile", &names) else {
        return Ok(None);
    };
    ctx.profile(&names[index]).map(Some)
}

fn dump(ctx: &Context) -> Result<i32> {
    let Some(profile) = pick_profile(ctx)? else {
        return Ok(CANCELLED);
    };
    profile.need_dump_dir(ctx.home.as_deref())?;

    // Lists come from the server when the mysql client can reach it;
    // otherwise the names are typed.
    println!("Connecting to {} to list databases...", profile.host());
    let databases = server::databases(ctx, &profile).unwrap_or_else(|error| {
        println!("(could not list databases: {error})");
        Vec::new()
    });
    let Some(db) = ui::pick_or_type("Database", &databases) else {
        return Ok(CANCELLED);
    };

    let tables = server::tables(ctx, &profile, &db).unwrap_or_default();
    if !tables.is_empty() {
        anstream::println!("\n{}", ui::bold("Tables and views"));
        for (index, table) in tables.iter().enumerate() {
            let view = if table.is_view { "  (view)" } else { "" };
            anstream::println!("  {:>3}) {}{}", index + 1, table.name, ui::dim(view));
        }
    }
    let answer = ui::ask("Exclude (numbers or names, comma separated; empty = dump everything): ")
        .unwrap_or_default();
    // A number picks from the list above; anything else is taken as a name.
    let exclude: Vec<String> = answer
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(|part| match part.parse::<usize>() {
            Ok(number) if (1..=tables.len()).contains(&number) => tables[number - 1].name.clone(),
            _ => part.to_string(),
        })
        .collect();

    let mut words = vec![
        "slipdock".to_string(),
        "dump".into(),
        profile.name.clone(),
        db.clone(),
    ];
    if !exclude.is_empty() {
        words.push("--exclude".into());
        words.push(exclude.join(","));
    }
    let Some(dry_run) = confirm_command(&words, "mydumper") else {
        return Ok(CANCELLED);
    };
    let args = DumpArgs {
        profile: profile.name,
        db,
        exclude,
        out: None,
        extra: Vec::new(),
    };
    commands::dump(ctx, args, &run_options(dry_run))
}

fn load(ctx: &Context) -> Result<i32> {
    let Some(profile) = pick_profile(ctx)? else {
        return Ok(CANCELLED);
    };
    let root = profile.need_dump_dir(ctx.home.as_deref())?;
    // Only complete dumps can be loaded.
    let found: Vec<_> = dumps::list(&root)
        .into_iter()
        .filter(|d| d.complete)
        .collect();
    if found.is_empty() {
        println!("No complete dumps in {}", root.display());
        return Ok(CANCELLED);
    }
    let labels: Vec<String> = found
        .iter()
        .map(|dump| format!("{}  ({})", dump.name, dumps::human_size(dump.size)))
        .collect();
    let Some(index) = ui::pick("Dump to load (newest first)", &labels) else {
        return Ok(CANCELLED);
    };
    let chosen = &found[index];

    let question = match &chosen.source_db {
        Some(source) => format!("Target database on {} [{source}]: ", profile.name),
        None => format!("Target database on {}: ", profile.name),
    };
    let typed = ui::ask(&question).unwrap_or_default();
    let target = match (typed.trim(), &chosen.source_db) {
        ("", Some(source)) => source.clone(),
        ("", None) => return Ok(CANCELLED),
        (name, _) => name.to_string(),
    };

    let words = vec![
        "slipdock".to_string(),
        "load".into(),
        profile.name.clone(),
        chosen.path.to_string_lossy().into_owned(),
        target.clone(),
    ];
    let Some(dry_run) = confirm_command(&words, "myloader") else {
        return Ok(CANCELLED);
    };
    let args = LoadArgs {
        profile: profile.name,
        dir: chosen.path.clone(),
        target: Some(target),
        confirm: None,
        extra: Vec::new(),
    };
    // A real load still asks for the database name: the guard is part of
    // `load`, not of this menu.
    commands::load(ctx, args, &run_options(dry_run))
}

/// Shows the equivalent command and asks how to go on.
/// `Some(false)` = run, `Some(true)` = dry run, `None` = cancel.
fn confirm_command(words: &[String], tool: &str) -> Option<bool> {
    let line: Vec<String> = words.iter().map(|word| command::quote(word)).collect();
    println!("\nCommand: {}", line.join(" "));
    let choices = [
        "run".to_string(),
        format!("dry run (show the {tool} command only)"),
        "cancel".to_string(),
    ];
    match ui::pick("Run it?", &choices) {
        Some(0) => Some(false),
        Some(1) => Some(true),
        _ => {
            println!("Cancelled.");
            None
        }
    }
}

fn run_options(dry_run: bool) -> RunOptions {
    RunOptions {
        log_level: None,
        dry_run,
        show_command: false,
    }
}
