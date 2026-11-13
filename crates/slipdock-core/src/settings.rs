//! `settings.toml`: the few choices that are not part of a profile.
//!
//! The command line tool and the app read and write the same file, so a tool
//! path chosen in one is used by the other.
//!
//! ```toml
//! [tools.mysql]
//! source = "path"
//! path = "/opt/homebrew/opt/mysql-client/bin/mysql"
//!
//! [left-out.production]
//! sales = ["activity_log", "jobs"]
//!
//! [options.mydumper]
//! arguments = "--threads 4"
//!
//! [options.myloader]
//! arguments = "--threads 8"
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::platform;
use crate::{Error, Result};

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Settings {
    /// Keyed by tool name (`mydumper`, `myloader`, `mysql`). A tool that is
    /// not listed is searched for automatically.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tools: BTreeMap<String, ToolSetting>,
    /// The tables and views the app last left out of a dump, by profile and
    /// then database. The app ticks the others when it lists the tables
    /// again. The command line tool does not read this: it leaves out what
    /// `--exclude` names.
    #[serde(
        default,
        rename = "left-out",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub left_out: BTreeMap<String, BTreeMap<String, Vec<String>>>,
    /// The colour band the app shows a profile with, as a number from 0.
    /// It is kept here so that a profile has the same band on every start,
    /// and no two profiles share one while there are bands left. The command
    /// line tool does not read this.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub bands: BTreeMap<String, u8>,
    /// Choices that hold for every dump and load, whatever the profile.
    #[serde(default, skip_serializing_if = "Options::is_empty")]
    pub options: Options,
}

/// Choices for every run, per tool. Each one is optional: without it, the
/// profile's file (or the tool's own default) decides.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Options {
    #[serde(default, skip_serializing_if = "ToolOptions::is_empty")]
    pub mydumper: ToolOptions,
    #[serde(default, skip_serializing_if = "ToolOptions::is_empty")]
    pub myloader: ToolOptions,
}

/// The choices for one tool.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolOptions {
    /// Options for every run of the tool, written as on its command line:
    /// `--threads 8 --rows 100000`. See [`parse_arguments`] for what is
    /// taken and how it reaches the tool.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub arguments: String,
}

impl Options {
    fn is_empty(&self) -> bool {
        *self == Options::default()
    }
}

impl ToolOptions {
    pub fn is_empty(&self) -> bool {
        *self == ToolOptions::default()
    }
}

/// Options Slipdock gives the tools itself. One of them among the
/// arguments of the settings would fight with the run.
const OWN_OPTIONS: [&str; 8] = [
    "defaults-file",
    "defaults-extra-file",
    "database",
    "outputdir",
    "directory",
    "logfile",
    "help",
    "version",
];

/// Splits arguments as a shell does: at spaces, with `'...'` and `"..."`
/// keeping spaces together.
pub fn split_arguments(text: &str) -> Result<Vec<String>> {
    let mut words = Vec::new();
    let mut word: Option<String> = None;
    let mut quote: Option<char> = None;
    for c in text.chars() {
        match quote {
            Some(open) if c == open => quote = None,
            Some(_) => word.get_or_insert_with(String::new).push(c),
            None if c == '\'' || c == '"' => {
                quote = Some(c);
                word.get_or_insert_with(String::new);
            }
            None if c.is_whitespace() => words.extend(word.take()),
            None => word.get_or_insert_with(String::new).push(c),
        }
    }
    if let Some(open) = quote {
        return Err(Error::msg(format!(
            "a {open} in the arguments is not closed"
        )));
    }
    words.extend(word);
    Ok(words)
}

/// Reads the arguments of the settings as `(option, value)` pairs:
/// `--threads 8 --rows=100000 --routines` gives `threads = 8`,
/// `rows = 100000` and `routines = TRUE`.
///
/// The pairs go into the tool's section of the defaults file of a run, not
/// on its command line. mydumper and myloader let a value in the defaults
/// file win over the same option given as an argument, so only in the file
/// does an option of the settings take the place of the profile's.
///
/// That is why an option must be in its long form (`--threads`, not `-t`):
/// the file knows options by their long names only.
pub fn parse_arguments(text: &str) -> Result<Vec<(String, String)>> {
    let mut pairs: Vec<(String, String)> = Vec::new();
    let mut words = split_arguments(text)?.into_iter().peekable();
    while let Some(word) = words.next() {
        let Some(option) = word.strip_prefix("--").filter(|name| !name.is_empty()) else {
            return Err(Error::msg(format!(
                "'{word}' is not an option in its long form, like --threads 8"
            )));
        };
        let (name, value) = match option.split_once('=') {
            Some((name, value)) => (name.to_string(), value.to_string()),
            // The next word is the value, unless it is an option itself.
            None => match words.next_if(|next| !next.starts_with("--")) {
                Some(value) => (option.to_string(), value),
                None => (option.to_string(), "TRUE".to_string()),
            },
        };
        if OWN_OPTIONS.contains(&name.as_str()) {
            return Err(Error::msg(format!(
                "--{name} is set by Slipdock itself and cannot be among the arguments"
            )));
        }
        pairs.push((name, value));
    }
    Ok(pairs)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolSetting {
    pub source: Source,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// Search PATH and the usual install locations.
    Auto,
    /// Use `path` and nothing else.
    Path,
}

impl Settings {
    pub fn file(config_dir: &Path) -> PathBuf {
        config_dir.join("settings.toml")
    }

    /// Reads the settings. A missing file gives the defaults.
    pub fn load(config_dir: &Path) -> Result<Self> {
        let file = Self::file(config_dir);
        let text = match fs::read_to_string(&file) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Settings::default());
            }
            Err(error) => return Err(Error::io_at("cannot read", &file)(error)),
        };
        toml::from_str(&text)
            .map_err(|error| Error::msg(format!("{} is not valid: {error}", file.display())))
    }

    pub fn save(&self, config_dir: &Path) -> Result<()> {
        let file = Self::file(config_dir);
        let text = toml::to_string(self).map_err(|error| Error::msg(error.to_string()))?;
        platform::create_private_dir(config_dir)
            .map_err(Error::io_at("cannot create", config_dir))?;
        platform::write_private_file(&file, text.as_bytes())
            .map_err(Error::io_at("cannot write", &file))
    }

    /// The path the user fixed for this tool, if any.
    pub fn tool_path(&self, tool: &str) -> Option<&Path> {
        let setting = self.tools.get(tool)?;
        match setting.source {
            Source::Path => setting.path.as_deref(),
            Source::Auto => None,
        }
    }

    pub fn set_tool_path(&mut self, tool: &str, path: PathBuf) {
        let setting = ToolSetting {
            source: Source::Path,
            path: Some(path),
        };
        self.tools.insert(tool.to_string(), setting);
    }

    /// Goes back to searching automatically.
    pub fn unset_tool_path(&mut self, tool: &str) {
        self.tools.remove(tool);
    }

    /// The tables last left out of a dump of this database with this profile.
    pub fn left_out(&self, profile: &str, database: &str) -> &[String] {
        self.left_out
            .get(profile)
            .and_then(|databases| databases.get(database))
            .map_or(&[], Vec::as_slice)
    }

    /// Remembers the tables left out; an empty list forgets the database.
    pub fn set_left_out(&mut self, profile: &str, database: &str, tables: Vec<String>) {
        if !tables.is_empty() {
            self.left_out
                .entry(profile.to_string())
                .or_default()
                .insert(database.to_string(), tables);
        } else if let Some(databases) = self.left_out.get_mut(profile) {
            databases.remove(database);
            if databases.is_empty() {
                self.left_out.remove(profile);
            }
        }
    }

    /// The band of a profile, if it has one.
    pub fn band(&self, profile: &str) -> Option<u8> {
        self.bands.get(profile).copied()
    }

    /// Gives every profile in `profiles` a band out of `count`, and forgets
    /// the bands of profiles that are gone. A profile that has a band keeps
    /// it. A new one gets the lowest band no profile has; with more profiles
    /// than bands, the band the fewest profiles have. Returns whether
    /// anything changed, so the caller knows to save.
    pub fn assign_bands(&mut self, profiles: &[String], count: u8) -> bool {
        let before = self.bands.len();
        self.bands
            .retain(|name, band| *band < count && profiles.contains(name));
        let mut changed = self.bands.len() != before;
        for name in profiles {
            if self.bands.contains_key(name) {
                continue;
            }
            let uses = |band: u8| self.bands.values().filter(|used| **used == band).count();
            let Some(band) = (0..count).min_by_key(|band| uses(*band)) else {
                break;
            };
            self.bands.insert(name.clone(), band);
            changed = true;
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_file_gives_the_defaults() {
        let config = tempfile::tempdir().unwrap();
        assert_eq!(Settings::load(config.path()).unwrap(), Settings::default());
    }

    #[test]
    fn a_tool_path_survives_save_and_load() {
        let config = tempfile::tempdir().unwrap();
        let dir = config.path().join("not-yet-created");
        let mut settings = Settings::default();
        settings.set_tool_path("mysql", PathBuf::from("/tools/mysql"));
        settings.save(&dir).unwrap();

        let text = fs::read_to_string(Settings::file(&dir)).unwrap();
        assert_eq!(
            text,
            "[tools.mysql]\nsource = \"path\"\npath = \"/tools/mysql\"\n"
        );

        let mut loaded = Settings::load(&dir).unwrap();
        assert_eq!(loaded.tool_path("mysql"), Some(Path::new("/tools/mysql")));
        assert_eq!(loaded.tool_path("mydumper"), None);

        loaded.unset_tool_path("mysql");
        assert_eq!(loaded.tool_path("mysql"), None);
    }

    #[test]
    fn left_out_tables_survive_save_and_load() {
        let config = tempfile::tempdir().unwrap();
        let mut settings = Settings::default();
        settings.set_left_out("production", "sales", vec!["activity_log".into()]);
        settings.save(config.path()).unwrap();

        let text = fs::read_to_string(Settings::file(config.path())).unwrap();
        assert_eq!(text, "[left-out.production]\nsales = [\"activity_log\"]\n");

        let mut loaded = Settings::load(config.path()).unwrap();
        assert_eq!(loaded.left_out("production", "sales"), ["activity_log"]);
        assert!(loaded.left_out("production", "other").is_empty());
        assert!(loaded.left_out("staging", "sales").is_empty());

        // Nothing left out: the entry goes, and the file is empty again.
        loaded.set_left_out("production", "sales", Vec::new());
        assert_eq!(loaded, Settings::default());
    }

    #[test]
    fn options_survive_save_and_load() {
        let config = tempfile::tempdir().unwrap();
        let mut settings = Settings::default();
        settings.options.mydumper.arguments = "--threads 4".into();
        settings.options.myloader.arguments = "--threads 8 --enable-binlog".into();
        settings.save(config.path()).unwrap();
        let text = fs::read_to_string(Settings::file(config.path())).unwrap();
        assert_eq!(
            text,
            "[options.mydumper]\narguments = \"--threads 4\"\n\n\
             [options.myloader]\narguments = \"--threads 8 --enable-binlog\"\n"
        );
        assert_eq!(Settings::load(config.path()).unwrap(), settings);

        // Nothing set: no `[options]` in the file.
        settings.options = Options::default();
        settings.save(config.path()).unwrap();
        assert_eq!(
            fs::read_to_string(Settings::file(config.path())).unwrap(),
            ""
        );
    }

    #[test]
    fn arguments_become_options_and_values() {
        let pair = |name: &str, value: &str| (name.to_string(), value.to_string());
        assert!(parse_arguments("  ").unwrap().is_empty());
        assert_eq!(
            parse_arguments("--threads 8 --rows=100000 --routines --where 'id > 5' --events")
                .unwrap(),
            [
                pair("threads", "8"),
                pair("rows", "100000"),
                pair("routines", "TRUE"),
                pair("where", "id > 5"),
                pair("events", "TRUE"),
            ]
        );
        for (bad, reason) in [
            ("-t 8", "'-t' is not an option in its long form"),
            ("8", "'8' is not an option in its long form"),
            ("--", "'--' is not an option in its long form"),
            ("--where 'id > 5", "a ' in the arguments is not closed"),
            (
                "--threads 8 --logfile /tmp/x",
                "--logfile is set by Slipdock itself",
            ),
            ("--database=other", "--database is set by Slipdock itself"),
        ] {
            let error = parse_arguments(bad).unwrap_err().to_string();
            assert!(error.contains(reason), "{bad}: {error}");
        }
    }

    #[test]
    fn source_auto_ignores_a_leftover_path() {
        let text = "[tools.mysql]\nsource = \"auto\"\npath = \"/old\"\n";
        let settings: Settings = toml::from_str(text).unwrap();
        assert_eq!(settings.tool_path("mysql"), None);
    }

    #[test]
    fn a_broken_file_names_the_file() {
        let config = tempfile::tempdir().unwrap();
        fs::write(Settings::file(config.path()), "tools = 3").unwrap();
        let error = Settings::load(config.path()).unwrap_err().to_string();
        assert!(error.contains("settings.toml is not valid"), "{error}");
    }

    #[test]
    fn a_profile_keeps_its_band() {
        let names = |list: &[&str]| list.iter().map(|name| name.to_string()).collect::<Vec<_>>();
        let mut settings = Settings::default();
        assert!(settings.assign_bands(&names(&["local", "staging"]), 3));
        assert_eq!(settings.band("local"), Some(0));
        assert_eq!(settings.band("staging"), Some(1));

        // A new profile takes a free band; the others keep theirs.
        assert!(settings.assign_bands(&names(&["alpha", "local", "staging"]), 3));
        assert_eq!(settings.band("alpha"), Some(2));
        assert_eq!(settings.band("local"), Some(0));
        assert!(!settings.assign_bands(&names(&["alpha", "local", "staging"]), 3));

        // A deleted profile gives its band back, and the next one takes it.
        assert!(settings.assign_bands(&names(&["alpha", "staging", "zeta"]), 3));
        assert_eq!(settings.band("local"), None);
        assert_eq!(settings.band("zeta"), Some(0));
        assert_eq!(settings.band("staging"), Some(1));

        // More profiles than bands: the band used least is shared.
        assert!(settings.assign_bands(&names(&["alpha", "more", "staging", "zeta"]), 3));
        assert_eq!(settings.band("more"), Some(0));
    }
}
