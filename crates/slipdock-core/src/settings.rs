//! `settings.toml`: the few choices that are not part of a profile.
//!
//! The command line tool and the app read and write the same file, so a tool
//! path chosen in one is used by the other.
//!
//! ```toml
//! [tools.mysql]
//! source = "path"
//! path = "/opt/homebrew/opt/mysql-client/bin/mysql"
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
}
