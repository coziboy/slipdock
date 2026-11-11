//! Looking at dump folders on disk.
//!
//! A dump is a folder written by mydumper. Slipdock keeps no index of them:
//! everything shown (complete or not, tables, size, source database) is read
//! from the folder each time.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::profile::Profile;

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
