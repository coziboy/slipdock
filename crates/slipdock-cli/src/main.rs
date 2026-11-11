//! `slipdock`, the command line tool.
//!
//! - `cli.rs`: the commands and options (what can be typed)
//! - `commands.rs`: what each command does
//! - `guided.rs`: the question-and-answer mode
//! - `ui.rs`: colours, the progress line, prompts
//!
//! The real work is in the `slipdock-core` library, shared with the app.

mod cli;
mod commands;
mod guided;
mod ui;

use clap::{CommandFactory, Parser};
use slipdock_core::Result;
use slipdock_core::job::Context;

use cli::{Cli, Color, Command};

fn main() {
    // When the shell asks for completions (COMPLETE=zsh slipdock ...), this
    // answers and exits. Otherwise it does nothing.
    clap_complete::CompleteEnv::with_factory(Cli::command).complete();

    let cli = Cli::parse();
    match cli.color {
        Color::Auto => {}
        Color::Always => anstream::ColorChoice::Always.write_global(),
        Color::Never => anstream::ColorChoice::Never.write_global(),
    }

    let code = match run(cli) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("slipdock: {error}");
            1
        }
    };
    // `run` has returned, so everything it owned (temporary files included)
    // has been cleaned up before the process ends.
    std::process::exit(code);
}

fn run(cli: Cli) -> Result<i32> {
    let ctx = Context::system(cli.config_dir.as_deref())?;
    match cli.command {
        None if ui::is_interactive() => guided::run(&ctx),
        None => {
            Cli::command().print_long_help().ok();
            Ok(0)
        }
        Some(Command::Init { profile }) => commands::init(&ctx, &profile),
        Some(Command::Profiles) => commands::profiles(&ctx),
        Some(Command::Dump {
            profile,
            db,
            exclude,
            out,
            run,
            extra,
        }) => {
            let args = commands::DumpArgs {
                profile,
                db,
                exclude,
                out,
                extra,
            };
            commands::dump(&ctx, args, &run)
        }
        Some(Command::Load {
            profile,
            dump_dir,
            target_db,
            confirm,
            run,
            extra,
        }) => {
            let args = commands::LoadArgs {
                profile,
                dir: dump_dir,
                target: target_db,
                confirm,
                extra,
            };
            commands::load(&ctx, args, &run)
        }
        Some(Command::Dumps { profile }) => commands::dumps(&ctx, profile.as_deref()),
        Some(Command::Tools { command }) => commands::tools(&ctx, command),
        Some(Command::Password { command }) => commands::password(&ctx, command),
        Some(Command::Completions { shell }) => commands::completions(shell),
        Some(Command::App) => commands::app(&ctx),
    }
}
