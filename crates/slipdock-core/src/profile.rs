//! Profiles: one MySQL "defaults file" per environment.
//!
//! A profile is a normal `.cnf` file. mydumper, myloader and the mysql client
//! read `[client]`, `[mydumper]` and `[myloader]` from it directly; Slipdock
//! adds a `[slipdock]` section that those tools ignore.
//!
//! The file belongs to the user. They write comments in it and may use keys
//! Slipdock has never heard of. So [`Document`] does not parse the file into
//! a map and write the map back; it keeps every line as it is and changes
//! only the one line it is asked to change.

use std::fs;
use std::path::{Path, PathBuf};

use crate::platform;
use crate::{Error, Result};

// --- Document: the file as lines --------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    lines: Vec<String>,
}

impl Document {
    pub fn parse(text: &str) -> Self {
        Document {
            lines: text.lines().map(str::to_string).collect(),
        }
    }

    /// The file text, every line ended by a newline.
    pub fn to_text(&self) -> String {
        let mut text = self.lines.join("\n");
        if !text.is_empty() {
            text.push('\n');
        }
        text
    }

    /// The value of `key` in `[section]`, or `None`. The first match wins.
    pub fn get(&self, section: &str, key: &str) -> Option<&str> {
        let index = self.find_key(section, key)?;
        key_value(&self.lines[index]).map(|(_, value)| value)
    }

    /// Sets `key = value` in `[section]`. An existing line is changed in
    /// place; otherwise the line is added at the end of the section, and the
    /// section itself is added at the end of the file if it is missing.
    pub fn set(&mut self, section: &str, key: &str, value: &str) -> Result<()> {
        check_value(key, value)?;
        if let Some(index) = self.find_key(section, key) {
            // Keep the text up to "=" so hand-aligned files stay aligned.
            let line = &self.lines[index];
            let equals = line.find('=').expect("a key line contains '='");
            self.lines[index] = format!("{} {value}", &line[..=equals]);
            return Ok(());
        }
        let new_line = format!("{key} = {value}");
        match self.section_range(section) {
            Some((start, end)) => {
                // After the last line of the section that is not blank.
                let last = (start..end)
                    .rev()
                    .find(|&i| !self.lines[i].trim().is_empty())
                    .unwrap_or(start);
                self.lines.insert(last + 1, new_line);
            }
            None => {
                if self.lines.last().is_some_and(|l| !l.trim().is_empty()) {
                    self.lines.push(String::new());
                }
                self.lines.push(format!("[{section}]"));
                self.lines.push(new_line);
            }
        }
        Ok(())
    }

    /// Removes `key` from `[section]`. Returns whether a line was removed.
    pub fn remove(&mut self, section: &str, key: &str) -> bool {
        match self.find_key(section, key) {
            Some(index) => {
                self.lines.remove(index);
                true
            }
            None => false,
        }
    }

    /// Renames every `[from]` header to `[to]`. Returns whether one was found.
    pub fn rename_section(&mut self, from: &str, to: &str) -> bool {
        let mut found = false;
        for line in &mut self.lines {
            if section_name(line) == Some(from) {
                *line = format!("[{to}]");
                found = true;
            }
        }
        found
    }

    /// Replaces a whole line that equals `from` exactly.
    fn replace_line(&mut self, from: &str, to: &str) {
        for line in &mut self.lines {
            if line == from {
                *line = to.to_string();
            }
        }
    }

    /// Line numbers of a section: (header line, one past its last line).
    fn section_range(&self, section: &str) -> Option<(usize, usize)> {
        let start = self
            .lines
            .iter()
            .position(|line| section_name(line) == Some(section))?;
        let end = (start + 1..self.lines.len())
            .find(|&i| section_name(&self.lines[i]).is_some())
            .unwrap_or(self.lines.len());
        Some((start, end))
    }

    fn find_key(&self, section: &str, key: &str) -> Option<usize> {
        let mut inside = false;
        for (index, line) in self.lines.iter().enumerate() {
            if let Some(name) = section_name(line) {
                inside = name == section;
            } else if inside && key_value(line).is_some_and(|(k, _)| k == key) {
                return Some(index);
            }
        }
        None
    }
}

/// `[client]` -> `client`. Anything else -> `None`.
fn section_name(line: &str) -> Option<&str> {
    line.trim().strip_prefix('[')?.strip_suffix(']')
}

/// `host = 127.0.0.1` -> `("host", "127.0.0.1")`. Comments and lines without
/// `=` -> `None`. The value is everything after the first `=`, so a password
/// may contain `=`.
fn key_value(line: &str) -> Option<(&str, &str)> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') || trimmed.starts_with(';') {
        return None;
    }
    let (key, value) = line.split_once('=')?;
    Some((key.trim(), value.trim()))
}

/// mydumper reads `value  # note` as the value "value  # note". A comment at
/// the end of a line is therefore always a mistake, and we refuse to write one.
fn check_value(key: &str, value: &str) -> Result<()> {
    if value.contains('\n') || value.contains('\r') {
        return Err(Error::msg(format!(
            "{key}: the value must be a single line"
        )));
    }
    let has_comment = value
        .char_indices()
        .any(|(i, c)| c == '#' && value[..i].ends_with([' ', '\t']));
    if has_comment {
        // The value is not repeated in the message: it may be a password.
        return Err(Error::msg(format!(
            "{key}: no comments at the end of a line (a space followed by #); mydumper would \
             read the comment as part of the value. Put the comment on its own line."
        )));
    }
    Ok(())
}

// --- Profile: a named document ------------------------------------------------

/// The section only Slipdock reads.
pub const SECTION: &str = "slipdock";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PasswordStore {
    /// `password = ...` under `[client]` in the profile file (the default).
    File,
    /// The system keychain; the file holds no password.
    Keychain,
}

#[derive(Debug, Clone)]
pub struct Profile {
    pub name: String,
    pub path: PathBuf,
    pub doc: Document,
}

impl Profile {
    pub fn load(config_dir: &Path, name: &str) -> Result<Self> {
        if name.is_empty() {
            return Err(Error::msg("profile name required (see: slipdock profiles)"));
        }
        let path = path_for(config_dir, name);
        if !path.is_file() {
            return Err(Error::msg(format!(
                "no profile '{name}' ({}). Create it with: slipdock init {name}",
                path.display()
            )));
        }
        let text = fs::read_to_string(&path).map_err(Error::io_at("cannot read", &path))?;
        Ok(Profile {
            name: name.to_string(),
            path,
            doc: Document::parse(&text),
        })
    }

    pub fn save(&self) -> Result<()> {
        platform::write_private_file(&self.path, self.doc.to_text().as_bytes())
            .map_err(Error::io_at("cannot write", &self.path))
    }

    fn client(&self, key: &str) -> &str {
        self.doc.get("client", key).unwrap_or("")
    }

    /// `user@host:port`, for headers and lists.
    pub fn target(&self) -> String {
        format!(
            "{}@{}:{}",
            self.client("user"),
            self.client("host"),
            self.client("port")
        )
    }

    pub fn host(&self) -> &str {
        self.client("host")
    }

    /// True when `tls-verify` is `false`, `0`, `no` or `off` (any case):
    /// the connection is still encrypted, but the server's certificate is
    /// not checked.
    pub fn tls_verify_off(&self) -> bool {
        let value = self.doc.get(SECTION, "tls-verify").unwrap_or("");
        ["false", "0", "no", "off"]
            .iter()
            .any(|off| value.eq_ignore_ascii_case(off))
    }

    /// Where this profile's dumps go, with `~` expanded. `None` when unset.
    pub fn dump_dir(&self, home: Option<&Path>) -> Option<PathBuf> {
        let value = self.doc.get(SECTION, "dump-dir")?;
        (!value.is_empty()).then(|| platform::expand_tilde(value, home))
    }

    /// Like [`Profile::dump_dir`], but explains the fix when it is unset.
    pub fn need_dump_dir(&self, home: Option<&Path>) -> Result<PathBuf> {
        self.dump_dir(home).ok_or_else(|| {
            Error::msg(format!(
                "profile {} has no dump-dir. Add under [{SECTION}] in {}:\n  \
                 dump-dir = /path/to/folder\n(or pass --out <folder> to slipdock dump)",
                self.name,
                self.path.display()
            ))
        })
    }

    /// The `verbose` level of `[mydumper]` or `[myloader]`, if it is 0-9.
    pub fn verbose(&self, tool_section: &str) -> Option<u8> {
        self.doc.get(tool_section, "verbose")?.parse().ok()
    }

    pub fn password_store(&self) -> PasswordStore {
        match self.doc.get(SECTION, "password-store") {
            Some(value) if value.eq_ignore_ascii_case("keychain") => PasswordStore::Keychain,
            _ => PasswordStore::File,
        }
    }
}

pub fn profiles_dir(config_dir: &Path) -> PathBuf {
    config_dir.join("profiles")
}

pub fn path_for(config_dir: &Path, name: &str) -> PathBuf {
    profiles_dir(config_dir).join(format!("{name}.cnf"))
}

/// A profile name becomes a file name and part of a dump folder name, so it
/// must not contain a path separator or look like an option.
pub fn check_name(name: &str) -> Result<()> {
    let bad = name.is_empty()
        || name.starts_with(['.', '-'])
        || name
            .chars()
            .any(|c| c == '/' || c == '\\' || c.is_whitespace() || c.is_control());
    if bad {
        return Err(Error::msg(format!(
            "'{name}' is not a usable profile name: no spaces or slashes, \
             and it cannot start with '.' or '-'"
        )));
    }
    Ok(())
}

/// The names of all profiles, sorted.
pub fn names(config_dir: &Path) -> Vec<String> {
    let mut names = cnf_stems(&profiles_dir(config_dir));
    names.sort();
    names
}

/// All profiles that can be read, sorted by name.
pub fn list(config_dir: &Path) -> Vec<Profile> {
    names(config_dir)
        .iter()
        .filter_map(|name| Profile::load(config_dir, name).ok())
        .collect()
}

/// The names of the `*.cnf` files in a folder (`a.cnf` -> `a`). `a.cnf.bak`
/// does not match. A missing folder gives an empty list.
fn cnf_stems(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_file())
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            name.strip_suffix(".cnf").map(str::to_string)
        })
        .filter(|stem| !stem.is_empty())
        .collect()
}

/// Creates a new profile from [`TEMPLATE`]. Never overwrites.
pub fn init(config_dir: &Path, name: &str) -> Result<PathBuf> {
    check_name(name)?;
    let path = path_for(config_dir, name);
    if path.exists() {
        return Err(Error::msg(format!("{} already exists", path.display())));
    }
    write_new_profile(config_dir, &path, TEMPLATE)?;
    Ok(path)
}

fn write_new_profile(config_dir: &Path, path: &Path, text: &str) -> Result<()> {
    let dir = profiles_dir(config_dir);
    // Both levels are made private: the config folder and profiles/ in it.
    platform::create_private_dir(config_dir).map_err(Error::io_at("cannot create", config_dir))?;
    platform::create_private_dir(&dir).map_err(Error::io_at("cannot create", &dir))?;
    platform::create_private_file(path, text.as_bytes())
        .map_err(Error::io_at("cannot create", path))
}

// --- Import from mydb -------------------------------------------------------------

/// Where the `mydb` shell script keeps its profiles.
pub fn mydb_dir(home: &Path) -> PathBuf {
    home.join(".config/mydb")
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub imported: Vec<String>,
    /// Profiles that already exist in Slipdock and were left alone.
    pub skipped: Vec<String>,
}

/// Copies `*.cnf` from a mydb folder into Slipdock, renaming `[mydb]` to
/// `[slipdock]`. Existing Slipdock profiles are never overwritten, and the
/// files in `from` are only read.
pub fn import(from: &Path, config_dir: &Path) -> Result<ImportReport> {
    if !from.is_dir() {
        return Err(Error::msg(format!(
            "nothing to import: {} is not a folder",
            from.display()
        )));
    }
    let mut stems = cnf_stems(from);
    stems.sort();
    let mut report = ImportReport::default();
    for name in stems {
        let target = path_for(config_dir, &name);
        if check_name(&name).is_err() || target.exists() {
            report.skipped.push(name);
            continue;
        }
        let source = from.join(format!("{name}.cnf"));
        let text = fs::read_to_string(&source).map_err(Error::io_at("cannot read", &source))?;
        let mut doc = Document::parse(&text);
        doc.rename_section("mydb", SECTION);
        doc.replace_line(
            "# Read only by mydb, ignored by mydumper/myloader/mysql.",
            "# Read only by Slipdock, ignored by mydumper/myloader/mysql.",
        );
        write_new_profile(config_dir, &target, &doc.to_text())?;
        report.imported.push(name);
    }
    Ok(report)
}

/// The starting point for a new profile.
pub const TEMPLATE: &str = "\
# No comments at the end of a line: mydumper reads \"VALUE  # note\" as the value.
# Host is a hostname or IP only; the port goes on its own line.
[client]
host     = 127.0.0.1
port     = 3306
user     = root
password = changeme
# ssl-mode = REQUIRED

[mydumper]
# Log level: 0 silent, 1 errors, 2 warnings, 3 info
verbose  = 3
threads  = 4
rows     = 50000
# Turn on for remote / VPN connections:
# compress-protocol = TRUE
# If FTWRL is not permitted (managed DBs such as Cloud SQL or RDS):
# sync-thread-lock-mode = NO_LOCK
# Off by default. Turn on only if the db uses them AND the user has the
# privilege (EVENT for events, TRIGGER for triggers, SHOW_ROUTINE for routines):
# routines = TRUE
# events   = TRUE
# triggers = TRUE
# Don't count these MySQL error numbers as failures (mydumper otherwise exits 1).
# 1227 = no SUPER/REPLICATION CLIENT, so \"Couldn't get master position\":
# harmless for a read-only user on a managed DB.
# ignore-errors = 1227
# Views are restored via a stand-in table, MEMORY by default. Managed DBs
# (Cloud SQL) often disable MEMORY, so use InnoDB:
table-engine-for-view-dependency = InnoDB

[myloader]
verbose  = 3
# Create/drop tables one at a time: parallel DROP/CREATE on tables linked by
# foreign keys deadlocks. Data still loads with all threads.
max-threads-for-schema-creation = 1
# myloader runs SET SQL_LOG_BIN = 0 on each connection, which needs SUPER or
# SESSION_VARIABLES_ADMIN. Without it (managed DBs such as Cloud SQL) the query
# is refused and myloader exits 1. To skip it and keep binary logging on:
# enable-binlog = TRUE
threads  = 8
queries-per-transaction = 1000
# compress-protocol = TRUE

# Read only by Slipdock, ignored by mydumper/myloader/mysql.
[slipdock]
# FALSE = don't check the server's TLS certificate (like TablePlus \"PREFERRED\").
# Needed for Cloud SQL unless you set \"ca = <server-ca.pem>\" under [client].
# tls-verify = FALSE
# Required: where this profile's dumps go. ~ is allowed, no quotes.
dump-dir =
# Where the password is kept: \"file\" (the password line under [client]) or
# \"keychain\". Switch with: slipdock password set <profile>
# password-store = file
";

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
# top comment
[client]
host     = db.example.test
port     = 3306
user     = example_user
password = a=b#c
# password = old

[mydumper]
verbose = 1
future-key = kept

[slipdock]
tls-verify = False
dump-dir = ~/dumps
";

    #[test]
    fn get_reads_the_right_section_and_skips_comments() {
        let doc = Document::parse(SAMPLE);
        assert_eq!(doc.get("client", "host"), Some("db.example.test"));
        assert_eq!(doc.get("client", "password"), Some("a=b#c"));
        assert_eq!(doc.get("mydumper", "verbose"), Some("1"));
        assert_eq!(doc.get("myloader", "verbose"), None);
        assert_eq!(doc.get("client", "verbose"), None);
    }

    #[test]
    fn parse_and_write_back_changes_nothing() {
        assert_eq!(Document::parse(SAMPLE).to_text(), SAMPLE);
    }

    #[test]
    fn set_changes_one_line_and_keeps_alignment_comments_and_unknown_keys() {
        let mut doc = Document::parse(SAMPLE);
        doc.set("client", "host", "other.example.test").unwrap();
        let expected = SAMPLE.replace(
            "host     = db.example.test",
            "host     = other.example.test",
        );
        assert_eq!(doc.to_text(), expected);
    }

    #[test]
    fn set_adds_a_missing_key_at_the_end_of_its_section() {
        let mut doc = Document::parse(SAMPLE);
        doc.set("mydumper", "threads", "4").unwrap();
        assert!(
            doc.to_text()
                .contains("future-key = kept\nthreads = 4\n\n[slipdock]")
        );
    }

    #[test]
    fn set_adds_a_missing_section_at_the_end() {
        let mut doc = Document::parse("[client]\nhost = h\n");
        doc.set("slipdock", "dump-dir", "/d").unwrap();
        assert_eq!(
            doc.to_text(),
            "[client]\nhost = h\n\n[slipdock]\ndump-dir = /d\n"
        );
    }

    #[test]
    fn set_refuses_a_comment_at_the_end_of_the_line() {
        let mut doc = Document::parse(SAMPLE);
        let error = doc.set("client", "port", "3306  # default").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("no comments at the end of a line")
        );
        // A '#' inside a value (a password, say) is fine.
        doc.set("client", "password", "p#ss").unwrap();
        assert_eq!(
            doc.to_text().matches("3306").count(),
            1,
            "nothing was written"
        );
    }

    #[test]
    fn remove_and_rename_section() {
        let mut doc = Document::parse("[mydb]\ndump-dir = /d\ntls-verify = FALSE\n");
        assert!(doc.rename_section("mydb", "slipdock"));
        assert!(doc.remove("slipdock", "tls-verify"));
        assert!(!doc.remove("slipdock", "tls-verify"));
        assert_eq!(doc.to_text(), "[slipdock]\ndump-dir = /d\n");
    }

    fn profile_from(text: &str) -> Profile {
        Profile {
            name: "example".into(),
            path: PathBuf::from("/config/profiles/example.cnf"),
            doc: Document::parse(text),
        }
    }

    #[test]
    fn profile_reads_target_tls_dump_dir_and_level() {
        let profile = profile_from(SAMPLE);
        assert_eq!(profile.target(), "example_user@db.example.test:3306");
        assert!(profile.tls_verify_off());
        assert_eq!(
            profile.dump_dir(Some(Path::new("/home/example"))),
            Some(PathBuf::from("/home/example/dumps"))
        );
        assert_eq!(profile.verbose("mydumper"), Some(1));
        assert_eq!(profile.verbose("myloader"), None);
        assert_eq!(profile.password_store(), PasswordStore::File);
    }

    #[test]
    fn tls_check_is_on_unless_switched_off() {
        for (value, off) in [
            ("FALSE", true),
            ("0", true),
            ("No", true),
            ("off", true),
            ("TRUE", false),
            ("", false),
        ] {
            let profile = profile_from(&format!("[slipdock]\ntls-verify = {value}\n"));
            assert_eq!(profile.tls_verify_off(), off, "tls-verify = {value}");
        }
        assert!(!profile_from("[client]\n").tls_verify_off());
    }

    #[test]
    fn the_template_needs_a_dump_dir_and_the_error_shows_the_fix() {
        let profile = profile_from(TEMPLATE);
        assert_eq!(profile.target(), "root@127.0.0.1:3306");
        assert_eq!(profile.dump_dir(None), None);
        let error = profile.need_dump_dir(None).unwrap_err().to_string();
        assert!(error.contains("dump-dir = /path/to/folder"));
        assert!(error.contains("--out"));
        assert!(!TEMPLATE.contains("mydb"));
    }

    #[test]
    fn init_creates_a_private_template_once() {
        let config = tempfile::tempdir().unwrap();
        let path = init(config.path(), "example").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), TEMPLATE);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        assert!(
            init(config.path(), "example")
                .unwrap_err()
                .to_string()
                .contains("already exists")
        );
        assert!(init(config.path(), "../escape").is_err());
        assert!(init(config.path(), "-flag").is_err());
        assert_eq!(names(config.path()), ["example"]);
    }

    #[test]
    fn load_explains_how_to_create_a_missing_profile() {
        let config = tempfile::tempdir().unwrap();
        let error = Profile::load(config.path(), "nope")
            .unwrap_err()
            .to_string();
        assert!(error.contains("no profile 'nope'"));
        assert!(error.contains("slipdock init nope"));
    }

    #[test]
    fn import_copies_cnf_files_renames_the_section_and_never_overwrites() {
        let mydb = tempfile::tempdir().unwrap();
        let config = tempfile::tempdir().unwrap();
        let original = "[client]\nhost = h\n\n# Read only by mydb, ignored by mydumper/myloader/mysql.\n[mydb]\ndump-dir = /d\n";
        fs::write(mydb.path().join("alpha.cnf"), original).unwrap();
        fs::write(mydb.path().join("beta.cnf"), original).unwrap();
        fs::write(mydb.path().join("alpha.cnf.bak"), "backup").unwrap();
        fs::write(mydb.path().join("notes.txt"), "notes").unwrap();
        init(config.path(), "beta").unwrap();

        let report = import(mydb.path(), config.path()).unwrap();

        assert_eq!(report.imported, ["alpha"]);
        assert_eq!(report.skipped, ["beta"]);
        assert_eq!(names(config.path()), ["alpha", "beta"]);
        let imported = fs::read_to_string(path_for(config.path(), "alpha")).unwrap();
        assert_eq!(
            imported,
            "[client]\nhost = h\n\n# Read only by Slipdock, ignored by mydumper/myloader/mysql.\n[slipdock]\ndump-dir = /d\n"
        );
        // The existing profile and the originals are untouched.
        assert_eq!(
            fs::read_to_string(path_for(config.path(), "beta")).unwrap(),
            TEMPLATE
        );
        assert_eq!(
            fs::read_to_string(mydb.path().join("alpha.cnf")).unwrap(),
            original
        );
    }

    #[test]
    fn import_from_a_missing_folder_is_an_error() {
        let config = tempfile::tempdir().unwrap();
        assert!(import(Path::new("/does/not/exist"), config.path()).is_err());
    }
}
