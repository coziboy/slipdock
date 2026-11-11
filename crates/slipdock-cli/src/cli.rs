//! The command line: which commands and options exist.
//!
//! This file only describes the interface. `clap` turns the types below into
//! a parser, the `--help` text and the shell completions; the work itself is
//! in `commands.rs`.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};
use clap_complete::engine::{ArgValueCandidates, CompletionCandidate};

#[derive(Debug, Parser)]
#[command(
    name = "slipdock",
    version,
    about = "Dump and load MySQL/MariaDB databases with mydumper and myloader",
    long_about = "Dump and load MySQL/MariaDB databases with mydumper and myloader.\n\n\
        Connection settings live in one file per environment (a \"profile\").\n\
        Run without a command on a terminal for guided mode.",
    after_help = "Log level: \"verbose = N\" under [mydumper] / [myloader] in the profile \
        (0 silent, 1 errors, 2 warnings, 3 info). --log-level N overrides it for one run."
)]
pub struct Cli {
    /// Where profiles and settings are kept [default: $SLIPDOCK_CONFIG_DIR, else the system's config folder]
    #[arg(long, global = true, value_name = "DIR")]
    pub config_dir: Option<PathBuf>,

    /// When to use colours
    #[arg(
        long,
        global = true,
        value_enum,
        default_value = "auto",
        value_name = "WHEN"
    )]
    pub color: Color,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Color {
    /// On a terminal, unless NO_COLOR is set
    Auto,
    Always,
    Never,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Create a profile from the template
    Init { profile: String },

    /// List profiles
    Profiles,

    /// Dump a database into a new folder <db>__<profile>__<time>
    Dump {
        #[arg(add = profile_names())]
        profile: String,
        /// The database to dump
        db: String,
        /// Tables or views to leave out, comma separated; may be repeated
        #[arg(short = 'x', long, value_name = "TABLES")]
        exclude: Vec<String>,
        /// Where to put the dump [default: dump-dir of the profile]
        #[arg(long, value_name = "DIR")]
        out: Option<PathBuf>,
        #[command(flatten)]
        run: RunOptions,
        /// Passed to mydumper unchanged
        #[arg(last = true, value_name = "MYDUMPER ARGS")]
        extra: Vec<String>,
    },

    /// Load a dump into a database (tables in the dump are dropped and recreated)
    Load {
        #[arg(add = profile_names())]
        profile: String,
        /// The dump folder
        dump_dir: PathBuf,
        /// The database to load into [default: the one the dump was taken from]
        target_db: Option<String>,
        /// Confirm without a prompt: must be the name of the target database
        #[arg(long, value_name = "NAME")]
        confirm: Option<String>,
        #[command(flatten)]
        run: RunOptions,
        /// Passed to myloader unchanged
        #[arg(last = true, value_name = "MYLOADER ARGS")]
        extra: Vec<String>,
    },

    /// List dump folders with their sizes
    Dumps {
        #[arg(add = profile_names())]
        profile: Option<String>,
    },

    /// Show path, version and status of mydumper, myloader and mysql
    Tools {
        #[command(subcommand)]
        command: Option<ToolsCommand>,
    },

    /// Keep a profile's password in the system keychain instead of its file
    Password {
        #[command(subcommand)]
        command: PasswordCommand,
    },

    /// Print the shell setup for tab completion (profile names included)
    #[command(
        long_about = "Print the shell setup for tab completion (profile names included).\n\n\
        zsh:   echo 'source <(slipdock completions zsh)' >> ~/.zshrc\n\
        bash:  echo 'source <(slipdock completions bash)' >> ~/.bashrc\n\
        fish:  echo 'slipdock completions fish | source' >> ~/.config/fish/completions/slipdock.fish"
    )]
    Completions { shell: Shell },

    /// Open the desktop app, if it is installed
    App,
}

#[derive(Debug, Args)]
pub struct RunOptions {
    /// Log level for this run only: 0 silent, 1 errors, 2 warnings, 3 info
    #[arg(long, value_name = "0-3", value_parser = clap::value_parser!(u8).range(0..=3))]
    pub log_level: Option<u8>,
    /// Show the command and stop; nothing is executed
    #[arg(long)]
    pub dry_run: bool,
    /// Show the full command before running it
    #[arg(long)]
    pub show_command: bool,
}

#[derive(Debug, Subcommand)]
pub enum ToolsCommand {
    /// Show how to install a missing tool, and offer to run the command
    Install { tool: Option<ToolName> },
    /// Always use this file for a tool
    Set { tool: ToolName, path: PathBuf },
    /// Go back to searching for a tool
    Unset { tool: ToolName },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum ToolName {
    Mydumper,
    Myloader,
    Mysql,
}

#[derive(Debug, Subcommand)]
pub enum PasswordCommand {
    /// Ask for the password, store it in the keychain and remove it from the file
    Set {
        #[arg(add = profile_names())]
        profile: String,
    },
    /// Remove the password from the keychain; the profile goes back to its file
    Clear {
        #[arg(add = profile_names())]
        profile: String,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Shell {
    Zsh,
    Bash,
    Fish,
}

/// Lets the shell complete profile names. Completion runs outside a normal
/// command, so this looks in the default config folder (it cannot see a
/// `--config-dir` typed earlier on the line).
fn profile_names() -> ArgValueCandidates {
    ArgValueCandidates::new(|| {
        let dir = slipdock_core::platform::config_dir(
            None,
            &|name| std::env::var_os(name),
            slipdock_core::platform::current_os(),
            std::env::home_dir().as_deref(),
        );
        match dir {
            Ok(dir) => slipdock_core::profile::names(&dir)
                .into_iter()
                .map(CompletionCandidate::new)
                .collect(),
            Err(_) => Vec::new(),
        }
    })
}
