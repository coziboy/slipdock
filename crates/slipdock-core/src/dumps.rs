//! Looking at dump folders on disk.
//!
//! A dump is a folder written by mydumper. Slipdock keeps no index of them:
//! everything shown (complete or not, tables, size, source database) is read
//! from the folder each time.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::profile::Profile;
use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DumpInfo {
    pub path: PathBuf,
    /// The folder name, `<db>__<profile>__<timestamp>` for Slipdock's dumps.
    pub name: String,
    /// False when mydumper did not reach the end (no `metadata` file).
    pub complete: bool,
    pub tables: usize,
    pub size: u64,
    /// The database the dump was taken from, if it can be told.
    pub source_db: Option<String>,
    pub modified: SystemTime,
}

pub fn inspect(dir: &Path) -> DumpInfo {
    let names = file_names(dir);
    DumpInfo {
        path: dir.to_path_buf(),
        name: dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default(),
        complete: is_complete(dir),
        tables: count_tables(&names),
        size: dir_size(dir),
        source_db: find_source_database(&names),
        modified: fs::metadata(dir)
            .and_then(|meta| meta.modified())
            .unwrap_or(SystemTime::UNIX_EPOCH),
    }
}

/// The folders inside `root`, newest first. A missing root gives no dumps.
pub fn list(root: &Path) -> Vec<DumpInfo> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut dumps: Vec<DumpInfo> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .map(|path| inspect(&path))
        .collect();
    // Newest first; the name breaks ties so the order is stable.
    dumps.sort_by(|a, b| b.modified.cmp(&a.modified).then(b.name.cmp(&a.name)));
    dumps
}

/// When a dump was taken, read from the `__<YYYYmmdd-HHMMSS>` at the end of
/// its name: `10 Nov 2026, 17:39`. `None` for a folder named differently.
pub fn taken(name: &str) -> Option<String> {
    let (_, stamp) = name.rsplit_once("__")?;
    let time = jiff::civil::DateTime::strptime("%Y%m%d-%H%M%S", stamp).ok()?;
    Some(time.strftime("%-d %b %Y, %H:%M").to_string())
}

/// Deletes a dump for good: the folder and the files Slipdock wrote beside
/// it (`<dump>.log`, `<dump>.omit`, `<dump>__load_*.log`).
pub fn delete(dump: &Path) -> Result<()> {
    fs::remove_dir_all(dump).map_err(Error::io_at("cannot delete", dump))?;
    let (Some(parent), Some(name)) = (dump.parent(), dump.file_name()) else {
        return Ok(());
    };
    let name = name.to_string_lossy();
    let load_logs = format!("{name}__load_");
    for file in file_names(parent) {
        let beside = file == format!("{name}.log")
            || file == format!("{name}.omit")
            || (file.starts_with(&load_logs) && file.ends_with(".log"));
        let path = parent.join(&file);
        if beside && path.is_file() {
            fs::remove_file(&path).map_err(Error::io_at("cannot delete", &path))?;
        }
    }
    Ok(())
}

/// One of the log files Slipdock wrote beside a dump.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DumpLog {
    pub path: PathBuf,
    /// What the log is of: `Dump`, or `Load into staging, 3 Jan 2026, 00:00`.
    pub title: String,
    /// The end of the log, at most [`crate::run::TAIL_BYTES`] of it.
    pub text: String,
    /// Whether the log has earlier lines that are not in `text`.
    pub cut: bool,
    /// The size of the whole log file, in bytes.
    pub size: u64,
}

/// The logs beside a dump: the one of the dump itself first, then one per
/// load, newest first. A dump without logs gives an empty list.
pub fn logs(dump: &Path) -> Vec<DumpLog> {
    let (Some(parent), Some(name)) = (dump.parent(), dump.file_name()) else {
        return Vec::new();
    };
    let name = name.to_string_lossy();
    let load_logs = format!("{name}__load_");
    let mut found = Vec::new();
    let dump_log = parent.join(format!("{name}.log"));
    if dump_log.is_file() {
        found.push(read_log(dump_log, "Dump".to_string()));
    }
    let mut loads: Vec<String> = file_names(parent)
        .into_iter()
        .filter(|file| file.starts_with(&load_logs) && file.ends_with(".log"))
        .collect();
    // The timestamp ends the name, so the newest load comes first.
    loads.sort_by(|a, b| load_stamp(b).cmp(load_stamp(a)));
    for file in loads {
        // `<profile>_<timestamp>` is what is left between the two marks.
        let middle = &file[load_logs.len()..file.len() - ".log".len()];
        let title = match middle.rsplit_once('_') {
            Some((profile, stamp)) => match taken(&format!("__{stamp}")) {
                Some(when) => format!("Load into {profile}, {when}"),
                None => format!("Load into {middle}"),
            },
            None => format!("Load into {middle}"),
        };
        found.push(read_log(parent.join(&file), title));
    }
    found
}

/// The `<timestamp>.log` at the end of a load log's name.
fn load_stamp(file: &str) -> &str {
    file.rsplit_once('_').map_or(file, |(_, stamp)| stamp)
}

fn read_log(path: PathBuf, title: String) -> DumpLog {
    let size = fs::metadata(&path).map_or(0, |meta| meta.len());
    // Only the end of the file is read: a log can be many megabytes, and
    // what went wrong is at its end.
    let (text, cut) = match crate::run::read_end(&path, crate::run::TAIL_BYTES) {
        // A log may hold bytes that are not UTF-8 (a table name, a value).
        Ok((bytes, cut)) => (String::from_utf8_lossy(&bytes).into_owned(), cut),
        Err(error) => (format!("cannot read the log: {error}"), false),
    };
    DumpLog {
        path,
        title,
        text: text.trim_end().to_string(),
        cut,
        size,
    }
}

/// The distinct dump folders of the given profiles, in profile order.
pub fn roots(profiles: &[Profile], home: Option<&Path>) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for dir in profiles.iter().filter_map(|profile| profile.dump_dir(home)) {
        if !roots.contains(&dir) {
            roots.push(dir);
        }
    }
    roots
}

/// mydumper writes the `metadata` file last, so it marks a finished dump.
pub fn is_complete(dir: &Path) -> bool {
    dir.join("metadata").is_file()
}

/// True when the dump has files compressed with zstd. myloader needs the
/// `zstd` program to read those.
pub fn is_zstd(dir: &Path) -> bool {
    file_names(dir).iter().any(|name| name.ends_with(".zst"))
}

/// Tables (and views) in a dump: one `<db>.<table>-schema.sql[.zst]` each.
/// `<db>-schema-create.sql` is the database itself and does not count.
pub fn table_count(dir: &Path) -> usize {
    count_tables(&file_names(dir))
}

fn count_tables(names: &[String]) -> usize {
    names
        .iter()
        .filter(|name| name.contains("-schema.sql") && !name.contains("-schema-create.sql"))
        .count()
}

/// The database a dump was taken from, read from the name of its
/// `<db>-schema-create.sql[.zst]` file.
pub fn source_database(dir: &Path) -> Option<String> {
    find_source_database(&file_names(dir))
}

fn find_source_database(names: &[String]) -> Option<String> {
    const MARK: &str = "-schema-create.sql";
    names.iter().find_map(|name| {
        let at = name.rfind(MARK)?;
        let after = &name[at + MARK.len()..];
        let is_match = at > 0 && (after.is_empty() || after.starts_with('.'));
        is_match.then(|| name[..at].to_string())
    })
}

/// Files mydumper has written so far (`*.sql*`), as a sign of progress.
pub fn sql_file_count(dir: &Path) -> usize {
    file_names(dir)
        .iter()
        .filter(|name| name.contains(".sql"))
        .count()
}

/// The file names in a folder, sorted. Unreadable folder: empty.
fn file_names(dir: &Path) -> Vec<String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// The total size of the files in a folder and its subfolders, in bytes.
pub fn dir_size(dir: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    entries
        .filter_map(|entry| entry.ok())
        .map(|entry| match entry.metadata() {
            Ok(meta) if meta.is_dir() => dir_size(&entry.path()),
            Ok(meta) => meta.len(),
            Err(_) => 0,
        })
        .sum()
}

/// A size the way `du -h` writes it: `0B`, `812B`, `1.5K`, `23M`, `4.2G`.
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "K", "M", "G", "T"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes}B")
    } else if value < 9.95 {
        format!("{value:.1}{}", UNITS[unit])
    } else {
        format!("{value:.0}{}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::Document;

    /// Builds a folder that looks like a mydumper dump of `example_db`.
    fn fixture(root: &Path, name: &str, complete: bool) -> PathBuf {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        for file in [
            "example_db-schema-create.sql.zst",
            "example_db.users-schema.sql.zst",
            "example_db.users.00000.sql.zst",
            "example_db.orders-schema.sql.zst",
            "example_db.orders.00000.sql.zst",
            "example_db.report-schema-view.sql.zst",
        ] {
            fs::write(dir.join(file), b"0123456789").unwrap();
        }
        if complete {
            fs::write(dir.join("metadata"), b"done").unwrap();
        }
        dir
    }

    #[test]
    fn inspect_reads_everything_from_the_folder() {
        let root = tempfile::tempdir().unwrap();
        let dir = fixture(root.path(), "example_db__example__20260102-030405", true);
        let info = inspect(&dir);
        assert_eq!(info.name, "example_db__example__20260102-030405");
        assert!(info.complete);
        assert_eq!(info.tables, 2, "schema-create and schema-view do not count");
        assert_eq!(info.size, 64);
        assert_eq!(info.source_db.as_deref(), Some("example_db"));
        assert_eq!(sql_file_count(&dir), 6);
    }

    #[test]
    fn the_date_comes_from_the_name() {
        assert_eq!(
            taken("example_db__example__20261110-173915").as_deref(),
            Some("10 Nov 2026, 17:39")
        );
        assert_eq!(
            taken("odd__name__example__20260102-030405").as_deref(),
            Some("2 Jan 2026, 03:04")
        );
        assert_eq!(taken("a_folder_named_by_hand"), None);
        assert_eq!(taken("example_db__example__20261340-000000"), None);
    }

    #[test]
    fn logs_are_the_dump_log_then_the_loads_newest_first() {
        let root = tempfile::tempdir().unwrap();
        let dir = fixture(root.path(), "example_db__example__20260102-030405", true);
        assert!(logs(&dir).is_empty());

        // Longer than what is read of a log: 16 bytes a line.
        let lines = crate::run::TAIL_BYTES / 16 + 5;
        let long: String = (0..lines)
            .map(|line| format!("line {line:010}\n"))
            .collect();
        for (file, text) in [
            ("example_db__example__20260102-030405.log", long.as_str()),
            (
                "example_db__example__20260102-030405__load_local_20260103-000000.log",
                "old",
            ),
            (
                "example_db__example__20260102-030405__load_a_b_20260104-101500.log",
                "new",
            ),
            ("example_db__example__20260102-030406.log", "another dump"),
        ] {
            fs::write(root.path().join(file), text).unwrap();
        }

        let found = logs(&dir);
        let titles: Vec<&str> = found.iter().map(|log| log.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "Dump",
                "Load into a_b, 4 Jan 2026, 10:15",
                "Load into local, 3 Jan 2026, 00:00"
            ]
        );
        // A long log keeps its end.
        assert!(found[0].cut);
        assert_eq!(found[0].size, long.len() as u64);
        assert!(!found[1].cut);
        // The read starts at the end of line 5, which goes with the cut.
        assert!(
            found[0].text.starts_with("line 0000000006\n"),
            "{}",
            &found[0].text[..20]
        );
        assert!(found[0].text.ends_with(&format!("line {:010}", lines - 1)));
        assert_eq!(found[1].text, "new");
    }

    #[test]
    fn delete_removes_the_dump_and_the_files_beside_it() {
        let root = tempfile::tempdir().unwrap();
        let dir = fixture(root.path(), "example_db__example__20260102-030405", true);
        let other = fixture(root.path(), "example_db__example__20260102-030406", true);
        let beside = [
            "example_db__example__20260102-030405.log",
            "example_db__example__20260102-030405.omit",
            "example_db__example__20260102-030405__load_example_20260103-000000.log",
        ];
        let kept = ["example_db__example__20260102-030406.log", "notes.txt"];
        for file in beside.iter().chain(&kept) {
            fs::write(root.path().join(file), b"x").unwrap();
        }

        delete(&dir).unwrap();

        assert!(!dir.exists());
        for file in beside {
            assert!(!root.path().join(file).exists(), "{file}");
        }
        assert!(other.is_dir(), "another dump is not touched");
        for file in kept {
            assert!(root.path().join(file).exists(), "{file}");
        }
        let error = delete(&dir).unwrap_err().to_string();
        assert!(error.contains("cannot delete"), "{error}");
    }

    #[test]
    fn a_dump_without_metadata_is_partial() {
        let root = tempfile::tempdir().unwrap();
        let dir = fixture(root.path(), "partial", false);
        assert!(!inspect(&dir).complete);
    }

    #[test]
    fn source_database_handles_plain_compressed_and_odd_names() {
        let find = |names: &[&str]| {
            let names: Vec<String> = names.iter().map(|n| n.to_string()).collect();
            find_source_database(&names)
        };
        assert_eq!(find(&["a-schema-create.sql"]).as_deref(), Some("a"));
        assert_eq!(find(&["a-schema-create.sql.gz"]).as_deref(), Some("a"));
        assert_eq!(
            find(&["my-db-schema-create.sql.zst"]).as_deref(),
            Some("my-db")
        );
        assert_eq!(find(&["a-schema-create.sqlx", "metadata"]), None);
        assert_eq!(find(&["-schema-create.sql"]), None);
        assert_eq!(
            find(&["b-schema-create.sql", "a-schema-create.sql"]).as_deref(),
            Some("b")
        );
    }

    #[test]
    fn list_shows_folders_newest_first_and_ignores_files() {
        let root = tempfile::tempdir().unwrap();
        let old = fixture(root.path(), "old", true);
        let new = fixture(root.path(), "new", false);
        fs::write(root.path().join("old.log"), "log").unwrap();
        let earlier = SystemTime::now() - std::time::Duration::from_secs(3600);
        fs::File::open(&old).unwrap().set_modified(earlier).unwrap();

        let names: Vec<String> = list(root.path()).into_iter().map(|d| d.name).collect();
        assert_eq!(names, ["new", "old"]);
        let _ = new;
        assert!(list(&root.path().join("missing")).is_empty());
    }

    #[test]
    fn roots_are_distinct_and_skip_profiles_without_dump_dir() {
        let profile = |name: &str, dump_dir: &str| Profile {
            name: name.into(),
            path: PathBuf::new(),
            doc: Document::parse(&format!("[slipdock]\ndump-dir = {dump_dir}\n")),
        };
        let profiles = [
            profile("a", "~/dumps"),
            profile("b", ""),
            profile("c", "/srv/dumps"),
            profile("d", "~/dumps"),
        ];
        assert_eq!(
            roots(&profiles, Some(Path::new("/home/example"))),
            [
                PathBuf::from("/home/example/dumps"),
                PathBuf::from("/srv/dumps")
            ]
        );
    }

    #[test]
    fn sizes_read_like_du() {
        assert_eq!(human_size(0), "0B");
        assert_eq!(human_size(812), "812B");
        assert_eq!(human_size(1536), "1.5K");
        assert_eq!(human_size(23 * 1024 * 1024), "23M");
        assert_eq!(human_size(4_509_715_660), "4.2G");
        assert_eq!(human_size(1024 * 1024 * 1024 * 1024 * 5), "5.0T");
    }
}
