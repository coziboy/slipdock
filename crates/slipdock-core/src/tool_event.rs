//! The JSON log of mydumper and myloader.
//!
//! From 1.0.19 both tools take `--machine-log-json`: every line of the log
//! file is then one JSON object, an *event*:
//!
//! ```text
//! {"seq":36,"ts":"2026-11-27 16:57:36.438191","level":"INFO","thread":"T02",
//!  "event":"restore_data_progress","phase":"restore_data","status":"progress",
//!  "message":"restoring data chunk","fatal":false,"table":"big",
//!  "progress_value":"2","progress_total":"12","tables_done":"0","tables_total":"3"}
//! ```
//!
//! [`ToolEvent::parse`] reads one line; [`RunState`] folds the events of a
//! run into what a front end shows: the stage, the files and tables done,
//! what each thread is on, the problems so far and the closing summary.
//! A log in the older text form gives a `RunState` too, with only the file
//! count (from myloader's `Progress X of Y`).
//!
//! The tools write every value as a string, numbers included.

use std::collections::BTreeMap;
use std::path::Path;

/// A time of day as the tools log it, local and without a zone.
pub use jiff::civil::DateTime;
use serde_json::Value;

/// The first mydumper/myloader release with `--machine-log-json`.
pub const FIRST_VERSION: crate::tools::Version = crate::tools::Version {
    major: 1,
    minor: 0,
    patch: 19,
};

/// The option that turns the JSON log on.
pub const OPTION: &str = "--machine-log-json";

/// Whether the mydumper or myloader at `program` can write a JSON log. Asks
/// the tool its version; one that does not answer gets the text log.
pub fn supported(tool: crate::tools::Tool, program: &Path) -> bool {
    crate::tools::probe(tool, program)
        .ok()
        .and_then(|working| crate::tools::Version::parse(&working.version))
        .is_some_and(|version| version >= FIRST_VERSION)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Debug,
    Info,
    Message,
    Warning,
    Error,
    Critical,
}

impl Level {
    fn parse(text: &str) -> Level {
        match text {
            "DEBUG" => Level::Debug,
            "MESSAGE" => Level::Message,
            "WARNING" => Level::Warning,
            "ERROR" => Level::Error,
            "CRITICAL" => Level::Critical,
            _ => Level::Info,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Level::Debug => "DEBUG",
            Level::Info => "INFO",
            Level::Message => "MESSAGE",
            Level::Warning => "WARNING",
            Level::Error => "ERROR",
            Level::Critical => "CRITICAL",
        }
    }
}

/// One line of the JSON log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolEvent {
    /// `2026-11-27 16:57:36.438191`, as the tool wrote it.
    pub ts: String,
    pub level: Level,
    /// What happened, for example `restore_data_progress`.
    pub event: String,
    pub phase: String,
    /// `started`, `progress`, `finished`, `failed` or `warning`.
    pub status: String,
    /// True when the tool stops because of it.
    pub fatal: bool,
    pub message: String,
    /// Every other field, as text.
    pub fields: BTreeMap<String, String>,
}

impl ToolEvent {
    /// Reads one log line; `None` when it is not a tool event.
    pub fn parse(line: &str) -> Option<ToolEvent> {
        let line = line.trim();
        if !line.starts_with('{') {
            return None;
        }
        let Value::Object(map) = serde_json::from_str::<Value>(line).ok()? else {
            return None;
        };
        let text = |key: &str| match map.get(key) {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Null) | None => String::new(),
            Some(other) => other.to_string(),
        };
        let event = text("event");
        if event.is_empty() {
            return None;
        }
        let fields = map
            .iter()
            .filter(|(key, _)| !COMMON.contains(&key.as_str()))
            .map(|(key, value)| {
                let value = match value {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                (key.clone(), value)
            })
            .collect();
        Some(ToolEvent {
            ts: text("ts"),
            level: Level::parse(&text("level")),
            event,
            phase: text("phase"),
            status: text("status"),
            fatal: map.get("fatal").and_then(Value::as_bool).unwrap_or(false),
            message: text("message"),
            fields,
        })
    }

    pub fn field(&self, key: &str) -> Option<&str> {
        self.fields
            .get(key)
            .map(String::as_str)
            .filter(|s| !s.is_empty())
    }

    pub fn number(&self, key: &str) -> Option<u64> {
        self.field(key)?.parse().ok()
    }

    /// The time of day, `16:57:36`.
    pub fn time(&self) -> &str {
        self.ts
            .split_once(' ')
            .map_or(self.ts.as_str(), |(_, time)| time)
            .split('.')
            .next()
            .unwrap_or("")
    }

    /// The table the event is about, without its database.
    pub fn table(&self) -> Option<&str> {
        self.field("table")
    }

    /// Noise that the tools log as an error but that changes nothing: a
    /// GLib assertion inside myloader on every run (marked fatal, though the
    /// load goes on and succeeds), and mydumper asking for variables of
    /// other servers (`@@tokudb_version not found`).
    fn is_noise(&self) -> bool {
        (self.field("domain") == Some("GLib") && self.message.contains("assertion"))
            || (!self.fatal && self.event == "mysql_operation")
    }

    /// Whether the event is a problem to show the user.
    pub fn is_problem(&self) -> bool {
        (self.fatal || self.level >= Level::Warning) && !self.is_noise()
    }

    /// The event as one readable log line:
    /// `16:57:36 restoring data chunk · big` or
    /// `16:57:36 ERROR: Error connection to database (MySQL error 2002)`.
    pub fn readable(&self) -> String {
        let mut line = self.time().to_string();
        if !line.is_empty() {
            line.push(' ');
        }
        if self.level >= Level::Warning {
            line.push_str(self.level.name());
            line.push_str(": ");
        }
        line.push_str(&self.message);
        if let Some(table) = self.table() {
            line.push_str(" · ");
            line.push_str(table);
            if let Some(file) = self
                .field("filename")
                .filter(|_| self.event.contains("data"))
            {
                let file = file.rsplit('/').next().unwrap_or(file);
                line.push_str(&format!(" ({file})"));
            }
        }
        if let Some(code) = self.field("mysql_errno") {
            line.push_str(&format!(" (MySQL error {code})"));
        }
        line
    }
}

/// The fields every event has, kept out of [`ToolEvent::fields`].
const COMMON: &[&str] = &[
    "schema_version",
    "event_version",
    "seq",
    "run_id",
    "kind",
    "ts",
    "level",
    "tool",
    "program",
    "pid",
    "thread",
    "event",
    "phase",
    "status",
    "parent_seq",
    "message",
    "fatal",
];

/// Where a run is. The order is the order of a run; a stage never goes
/// back, though threads overlap (one builds indexes while another still
/// loads data).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    Starting,
    Locking,
    Schema,
    Data,
    Indexes,
    Objects,
    Checksums,
    Done,
}

impl Stage {
    pub fn name(self) -> &'static str {
        match self {
            Stage::Starting => "Connecting",
            Stage::Locking => "Locking",
            Stage::Schema => "Tables",
            Stage::Data => "Data",
            Stage::Indexes => "Indexes",
            Stage::Objects => "Views and routines",
            Stage::Checksums => "Checksums",
            Stage::Done => "Done",
        }
    }

    /// The stages a dump goes through, for a stepper.
    pub const DUMP: &[Stage] = &[
        Stage::Starting,
        Stage::Locking,
        Stage::Schema,
        Stage::Data,
        Stage::Objects,
        Stage::Done,
    ];

    /// The stages a load goes through.
    pub const LOAD: &[Stage] = &[
        Stage::Starting,
        Stage::Schema,
        Stage::Data,
        Stage::Indexes,
        Stage::Objects,
        Stage::Checksums,
        Stage::Done,
    ];

    fn of(event: &ToolEvent) -> Option<Stage> {
        Some(match event.event.as_str() {
            "lock" | "backup_consistency" => Stage::Locking,
            "dump_schema" | "restore_table_progress" => Stage::Schema,
            "dump_table_progress" | "restore_data_progress" => Stage::Data,
            "index_job" | "restore_object_progress" => Stage::Indexes,
            "dump_view" | "restore_schema_file_progress" | "post_worker" => Stage::Objects,
            "checksum_run" | "checksum_check" => Stage::Checksums,
            "dump_completed" | "restore_completed" => Stage::Done,
            _ => return None,
        })
    }
}

/// What one thread is working on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activity {
    pub table: String,
    /// For dumps: how far into the table, 0 to 100.
    pub percent: Option<u8>,
}

/// A problem the tool logged, with how often it came.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub level: Level,
    pub fatal: bool,
    /// The readable text, without the time.
    pub text: String,
    pub count: usize,
}

/// The tool's own closing numbers (`dump_completed`, `restore_completed`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    pub duration_ms: u64,
    pub tables: u64,
    pub files: u64,
    pub bytes: u64,
    pub errors: u64,
    pub warnings: u64,
    pub retries: u64,
    pub skipped: u64,
}

/// A run as far as its log tells.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RunState {
    /// True once a JSON event was seen.
    pub json: bool,
    pub stage: Option<Stage>,
    /// Files restored and the total (loads).
    pub files: Option<(u64, u64)>,
    /// Tables done and the total.
    pub tables: Option<(u64, u64)>,
    /// By thread number.
    pub threads: BTreeMap<u32, Activity>,
    pub problems: Vec<Problem>,
    /// Tables whose checksum did not match.
    pub checksum_mismatches: Vec<String>,
    pub summary: Option<Summary>,
    /// True for a dump, false for a load.
    pub dump: bool,
    /// Every table of the run, in name order. A load knows them all from
    /// the dump folder at the start; a dump adds each one as it writes its
    /// schema.
    pub table_rows: Vec<TableProgress>,
    /// MySQL error numbers the run was told to ignore (`ignore-errors`).
    /// The tool still logs them, even marked fatal, and goes on; they are
    /// not problems.
    pub ignored_errors: Vec<String>,
    /// Loads: the file each thread works on, by thread number.
    pub running: BTreeMap<u32, RunningFile>,
    /// Loads: bytes of finished files and the time they took, in
    /// milliseconds summed over the threads: the speed of one thread.
    pub speed: (u64, u64),
}

/// Where one table is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TableStatus {
    Waiting,
    /// Its `CREATE TABLE` runs.
    Creating,
    /// Its rows are written (a dump) or loaded (a load).
    Rows,
    /// myloader adds its indexes after the rows.
    Indexing,
    Done,
}

/// One table of a run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableProgress {
    pub name: String,
    pub status: TableStatus,
    /// Loads: data files started and the total, one per chunk.
    pub files: Option<(u64, u64)>,
    /// Dumps: how far into the table, 0 to 100.
    pub percent: Option<u8>,
    /// `Some(false)` when its checksum did not match after a load.
    pub checksum_ok: Option<bool>,
    /// Loads: the size of each data file, by file name.
    pub file_sizes: BTreeMap<String, u64>,
    /// Loads: bytes of the files finished, and of all its files.
    pub bytes: Option<(u64, u64)>,
}

/// A table of a dump folder: its data files, each with its size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableFiles {
    pub name: String,
    pub files: Vec<(String, u64)>,
}

/// How far something is: a share from 0 to 1, and whether a part of it is
/// an estimate for a file still in work.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Share {
    pub fraction: f32,
    pub estimated: bool,
}

impl Share {
    /// `40%`, or `~40%` when estimated.
    pub fn percent(self) -> String {
        let percent = (self.fraction * 100.0).floor() as u32;
        if self.estimated {
            format!("~{percent}%")
        } else {
            format!("{percent}%")
        }
    }
}

/// An estimate never shows a file as finished before the tool moves on.
const ESTIMATE_CAP: f64 = 0.95;

impl TableProgress {
    fn new(name: &str, files: Option<u64>) -> Self {
        TableProgress {
            name: name.to_string(),
            status: TableStatus::Waiting,
            files: files.map(|total| (0, total)),
            percent: None,
            checksum_ok: None,
            file_sizes: BTreeMap::new(),
            bytes: None,
        }
    }

    fn with_files(table: &TableFiles) -> Self {
        let total = table.files.iter().map(|(_, size)| size).sum();
        TableProgress {
            file_sizes: table.files.iter().cloned().collect(),
            bytes: Some((0, total)),
            ..TableProgress::new(&table.name, Some(table.files.len() as u64))
        }
    }

    /// Bytes finished plus the estimate for its files in work, and its
    /// total; `None` when its size is not known.
    fn bytes_at(&self, state: &RunState, now: Option<DateTime>) -> Option<(f64, f64, bool)> {
        let (done, total) = self.bytes.filter(|(_, total)| *total > 0)?;
        let mut estimate = 0.0;
        for file in state
            .running
            .values()
            .filter(|file| file.table == self.name)
        {
            estimate += state.estimate(file, now);
        }
        Some((done as f64 + estimate, total as f64, estimate > 0.0))
    }

    /// How far the table is. In a load, by bytes: the files finished, and
    /// for each file in work an estimate from the speed of the files before
    /// it (`now` is the time of day; `None` leaves the estimate out).
    pub fn share(&self, state: &RunState, now: Option<DateTime>) -> Share {
        let exact = |fraction: f32| Share {
            fraction: fraction.clamp(0.0, 1.0),
            estimated: false,
        };
        if self.status == TableStatus::Done {
            return exact(1.0);
        }
        if let Some((done, total, estimated)) = self.bytes_at(state, now) {
            return Share {
                fraction: (done / total).clamp(0.0, 1.0) as f32,
                estimated,
            };
        }
        match (self.files, self.percent) {
            // myloader names a file when it starts it: the files before
            // the last one started are the ones finished.
            (Some((started, total)), _) if total > 0 => {
                exact(started.saturating_sub(1) as f32 / total as f32)
            }
            (_, Some(percent)) => exact(f32::from(percent) / 100.0),
            _ => exact(0.0),
        }
    }

    /// The short state at the end of its row: `waiting`, `creating`,
    /// `40%` (or `~40%` with an estimate in it), `indexing`, `100%`. A
    /// share, never a number of files: how a tool splits a table is not the
    /// user's concern.
    pub fn detail(&self, state: &RunState, now: Option<DateTime>) -> String {
        match self.status {
            TableStatus::Waiting => "waiting".into(),
            TableStatus::Creating => "creating".into(),
            TableStatus::Indexing => "indexing".into(),
            TableStatus::Done if self.checksum_ok == Some(false) => "checksum differs".into(),
            TableStatus::Done => "100%".into(),
            TableStatus::Rows => self.share(state, now).percent(),
        }
    }
}

/// A data file a thread of myloader works on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunningFile {
    pub table: String,
    pub size: u64,
    pub started: DateTime,
}

/// The time of day now, to compare with the times in the tool's log (the
/// tool runs on this machine, in its local time).
pub fn now() -> DateTime {
    jiff::Zoned::now().datetime()
}

/// The time of an event: `2026-11-27 17:28:42.836585`.
fn event_time(event: &ToolEvent) -> Option<DateTime> {
    event.ts.parse().ok()
}

/// More problems than this are counted, not kept.
const MAX_PROBLEMS: usize = 50;

impl RunState {
    /// The state a dump starts from.
    pub fn for_dump() -> RunState {
        RunState {
            dump: true,
            ..RunState::default()
        }
    }

    /// The state a load starts from: every table of the dump, with its
    /// data files and their sizes (see `dumps::data_files_per_table`).
    pub fn for_load(tables: Vec<TableFiles>) -> RunState {
        RunState {
            table_rows: tables.iter().map(TableProgress::with_files).collect(),
            ..RunState::default()
        }
    }

    /// The bytes a thread has likely loaded of `file` by `now`: its time so
    /// far at the speed of the files finished before, at most
    /// [`ESTIMATE_CAP`] of the file.
    fn estimate(&self, file: &RunningFile, now: Option<DateTime>) -> f64 {
        let (bytes, millis) = self.speed;
        let Some(now) = now.filter(|_| bytes > 0 && millis > 0) else {
            return 0.0;
        };
        let Ok(span) = now.since(file.started) else {
            return 0.0;
        };
        let Ok(elapsed) = span.total(jiff::Unit::Millisecond) else {
            return 0.0;
        };
        let rate = bytes as f64 / millis as f64;
        (elapsed.max(0.0) * rate).min(file.size as f64 * ESTIMATE_CAP)
    }

    /// How far a dump is: each table counts the same, a finished one whole
    /// and one in work by mydumper's own percent for it. mydumper names
    /// every table (its schema) before it writes any rows, so the tables
    /// are all known by then. Views are not counted: they have no rows.
    fn dump_share(&self) -> Option<Share> {
        if self.summary.is_some() {
            return Some(Share {
                fraction: 1.0,
                estimated: false,
            });
        }
        if self.table_rows.is_empty() {
            return None;
        }
        let done: f32 = self
            .table_rows
            .iter()
            .map(|row| match row.status {
                TableStatus::Done => 1.0,
                TableStatus::Rows => row.percent.map_or(0.0, |p| f32::from(p) / 100.0),
                _ => 0.0,
            })
            .sum();
        Some(Share {
            fraction: (done / self.table_rows.len() as f32).clamp(0.0, 1.0),
            estimated: false,
        })
    }

    /// A thread is done with its file: its bytes count as loaded, and its
    /// time teaches the speed.
    fn finish_file(&mut self, thread: u32, at: Option<DateTime>) {
        let Some(file) = self.running.remove(&thread) else {
            return;
        };
        let millis = at
            .and_then(|at| at.since(file.started).ok())
            .and_then(|span| span.total(jiff::Unit::Millisecond).ok())
            .unwrap_or(0.0);
        if millis > 0.0 && file.size > 0 {
            self.speed.0 += file.size;
            self.speed.1 += millis as u64;
        }
        if let Some(row) = self
            .table_rows
            .iter_mut()
            .find(|row| row.name == file.table)
            && let Some((done, total)) = row.bytes.as_mut()
        {
            *done = (*done + file.size).min(*total);
        }
    }

    /// How far the whole load is, by bytes with the estimate for the files
    /// in work; by files when the sizes are not known.
    pub fn share(&self, now: Option<DateTime>) -> Option<Share> {
        if self.dump {
            return self.dump_share();
        }
        let (mut done, mut total, mut estimated) = (0.0, 0.0, false);
        for row in &self.table_rows {
            if row.status == TableStatus::Done {
                let size = row.bytes.map_or(0, |(_, total)| total) as f64;
                done += size;
                total += size;
            } else if let Some((row_done, row_total, row_estimated)) = row.bytes_at(self, now) {
                done += row_done;
                total += row_total;
                estimated |= row_estimated;
            }
        }
        if total > 0.0 {
            return Some(Share {
                fraction: (done / total).clamp(0.0, 1.0) as f32,
                estimated,
            });
        }
        self.fraction().map(|fraction| Share {
            fraction,
            estimated: false,
        })
    }

    /// Tables done and the number of tables, from the rows.
    pub fn rows_done(&self) -> (usize, usize) {
        let done = self
            .table_rows
            .iter()
            .filter(|row| row.status == TableStatus::Done)
            .count();
        (done, self.table_rows.len())
    }

    /// The tables a thread works on now, in name order.
    pub fn active_tables(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .table_rows
            .iter()
            .filter(|row| {
                matches!(
                    row.status,
                    TableStatus::Rows | TableStatus::Indexing | TableStatus::Creating
                )
            })
            .map(|row| row.name.as_str())
            .collect();
        names.dedup();
        names
    }

    fn row(&mut self, name: &str) -> &mut TableProgress {
        let at = match self.table_rows.iter().position(|row| row.name == name) {
            Some(at) => at,
            None => {
                // A table the start did not know: keep the name order.
                let at = self
                    .table_rows
                    .partition_point(|row| row.name.as_str() < name);
                self.table_rows.insert(at, TableProgress::new(name, None));
                at
            }
        };
        &mut self.table_rows[at]
    }

    /// Moves the table an event is about on.
    fn track_table(&mut self, event: &ToolEvent) {
        let done = |status| status == TableStatus::Done;
        match (event.event.as_str(), event.status.as_str()) {
            // A dump: the schema of a table (not of the database) is written.
            ("dump_schema", _) => {
                if let Some(table) = event.table() {
                    self.row(table);
                }
            }
            ("restore_table_progress", status) => {
                let Some(table) = event.table() else { return };
                // A view's stand-in table comes through here too; only
                // tables known from the dump are followed.
                if let Some(row) = self.table_rows.iter_mut().find(|row| row.name == table) {
                    match status {
                        "started" if row.status == TableStatus::Waiting => {
                            row.status = TableStatus::Creating;
                        }
                        // A table without data files is done once created.
                        "finished" if row.files.is_some_and(|(_, total)| total == 0) => {
                            row.status = TableStatus::Done;
                        }
                        _ => {}
                    }
                }
            }
            ("restore_data_progress", _) => {
                let Some(table) = event.table() else { return };
                let at = event_time(event);
                let thread = event.number("thread_id").map(|n| n as u32);
                // Starting a file means the thread finished the one before.
                if let Some(thread) = thread {
                    self.finish_file(thread, at);
                }
                let file = event
                    .field("filename")
                    .map(|f| f.rsplit('/').next().unwrap_or(f));
                let row = self.row(table);
                let size = file.and_then(|f| row.file_sizes.get(f)).copied();
                if let (Some(thread), Some(size), Some(started)) = (thread, size, at) {
                    self.running.insert(
                        thread,
                        RunningFile {
                            table: table.to_string(),
                            size,
                            started,
                        },
                    );
                }
                let row = self.row(table);
                if !done(row.status) {
                    row.status = TableStatus::Rows;
                }
                row.files = Some(match row.files {
                    Some((started, total)) => ((started + 1).min(total), total),
                    None => (1, event.number("part_total").unwrap_or(0).max(1)),
                });
            }
            ("dump_table_progress", _) => {
                let Some(table) = event.table() else { return };
                let percent = event.number("progress_pct").map(|p| p.min(100) as u8);
                let row = self.row(table);
                if !done(row.status) {
                    row.status = TableStatus::Rows;
                    row.percent = percent.max(row.percent);
                }
            }
            ("index_job", _) => {
                if let Some(table) = event.table() {
                    let row = self.row(table);
                    row.status = TableStatus::Indexing;
                }
            }
            ("restore_object_progress", "finished") if event.field("object") == Some("indexes") => {
                if let Some(table) = event.table() {
                    self.row(table).status = TableStatus::Done;
                }
            }
            ("loader_thread", "finished") => {
                if let Some(thread) = event.number("thread_id") {
                    self.finish_file(thread as u32, event_time(event));
                }
            }
            ("checksum_check", "finished") => {
                let Some(table) = event.table() else { return };
                let same = event
                    .field("expected_checksum")
                    .unwrap_or("")
                    .eq_ignore_ascii_case(event.field("actual_checksum").unwrap_or(""));
                if let Some(row) = self.table_rows.iter_mut().find(|row| row.name == table) {
                    row.checksum_ok = Some(row.checksum_ok.unwrap_or(true) && same);
                }
            }
            ("dump_completed" | "restore_completed", _) => {
                self.running.clear();
                for row in &mut self.table_rows {
                    row.status = TableStatus::Done;
                }
            }
            _ => {}
        }
        self.settle();
    }

    /// A table whose rows are all started and that no thread works on any
    /// more is done. (myloader names a file when it starts it, so the last
    /// file is in work until the thread moves on.)
    fn settle(&mut self) {
        let busy: Vec<&str> = self
            .threads
            .values()
            .map(|a| a.table.as_str())
            .chain(self.running.values().map(|file| file.table.as_str()))
            .collect();
        for row in &mut self.table_rows {
            if row.status != TableStatus::Rows || busy.contains(&row.name.as_str()) {
                continue;
            }
            let all_started = match (row.files, row.percent) {
                (Some((started, total)), _) => started >= total,
                // A dump: the thread moved on, so the table is written.
                (None, Some(_)) => self.dump,
                _ => false,
            };
            if all_started {
                row.status = TableStatus::Done;
            }
        }
    }

    /// Takes in one log line, JSON or text.
    pub fn take_line(&mut self, line: &str) {
        match ToolEvent::parse(line) {
            Some(event) => self.apply(&event),
            None => {
                if let Some((done, total)) = crate::run::last_restore_progress(line) {
                    self.files = Some((done as u64, total as u64));
                    self.stage = self.stage.max(Some(Stage::Data));
                }
            }
        }
    }

    /// Takes in every line of `text`.
    pub fn take_text(&mut self, text: &str) {
        for line in text.lines() {
            self.take_line(line);
        }
    }

    /// Reads a whole log file.
    pub fn from_log(log: &Path) -> RunState {
        let mut state = RunState::default();
        state.read_log(log);
        state
    }

    /// Takes in a whole log file.
    pub fn read_log(&mut self, log: &Path) {
        if let Ok(bytes) = std::fs::read(log) {
            self.take_text(&String::from_utf8_lossy(&bytes));
        }
    }

    /// This state, leaving out the problems with these MySQL error numbers.
    pub fn ignoring(mut self, errors: &[String]) -> RunState {
        self.ignored_errors = errors.to_vec();
        self
    }

    pub fn apply(&mut self, event: &ToolEvent) {
        self.json = true;
        self.stage = self.stage.max(Stage::of(event)).or(Some(Stage::Starting));
        let thread = event.number("thread_id").map(|n| n as u32);
        if let (Some(done), Some(total)) =
            (event.number("tables_done"), event.number("tables_total"))
        {
            self.tables = Some((done, total));
        }
        match event.event.as_str() {
            "restore_data_progress" => {
                if let (Some(done), Some(total)) = (
                    event.number("progress_value"),
                    event.number("progress_total"),
                ) {
                    self.files = Some((done, total));
                }
                if let (Some(thread), Some(table)) = (thread, event.table()) {
                    self.threads.insert(
                        thread,
                        Activity {
                            table: table.to_string(),
                            percent: None,
                        },
                    );
                }
            }
            "dump_table_progress" => {
                if let (Some(left), Some(total)) = (
                    event.number("tables_remaining"),
                    event.number("tables_total"),
                ) {
                    self.tables = Some((total.saturating_sub(left), total));
                }
                if let (Some(thread), Some(table)) = (thread, event.table()) {
                    self.threads.insert(
                        thread,
                        Activity {
                            table: table.to_string(),
                            percent: event.number("progress_pct").map(|p| p.min(100) as u8),
                        },
                    );
                }
            }
            "loader_thread" | "dump_phase" if event.status == "finished" => {
                if let Some(thread) = thread {
                    self.threads.remove(&thread);
                }
            }
            "checksum_check" if event.status == "finished" => {
                let expected = event.field("expected_checksum").unwrap_or("");
                let actual = event.field("actual_checksum").unwrap_or("");
                if !expected.eq_ignore_ascii_case(actual) {
                    let table = event.table().unwrap_or("?").to_string();
                    if !self.checksum_mismatches.contains(&table) {
                        self.checksum_mismatches.push(table);
                    }
                }
            }
            "dump_completed" | "restore_completed" => {
                let n = |key| event.number(key).unwrap_or(0);
                self.summary = Some(Summary {
                    duration_ms: n("duration_ms"),
                    tables: n("tables"),
                    files: n("files"),
                    bytes: n("bytes"),
                    errors: n("errors"),
                    warnings: n("warnings"),
                    retries: n("retries"),
                    skipped: n("skipped"),
                });
                self.threads.clear();
                if let (Some(files), Some((_, total))) = (event.number("files"), self.files) {
                    self.files = Some((files.min(total), total));
                }
            }
            _ => {}
        }
        self.track_table(event);
        let ignored = event
            .field("mysql_errno")
            .is_some_and(|code| self.ignored_errors.iter().any(|i| i == code));
        if event.is_problem() && !ignored {
            self.add_problem(event);
        }
    }

    fn add_problem(&mut self, event: &ToolEvent) {
        let mut text = event.readable();
        // Without the time: the same problem at another second is the same.
        if let Some(rest) = text.strip_prefix(event.time()) {
            text = rest.trim_start().to_string();
        }
        if let Some(seen) = self.problems.iter_mut().find(|p| p.text == text) {
            seen.count += 1;
            seen.fatal |= event.fatal;
            return;
        }
        if self.problems.len() < MAX_PROBLEMS {
            self.problems.push(Problem {
                level: event.level,
                fatal: event.fatal,
                text,
                count: 1,
            });
        }
    }

    /// Files done as a share of the total, 0 to 1.
    pub fn fraction(&self) -> Option<f32> {
        let (done, total) = self.files?;
        (total > 0).then(|| (done as f32 / total as f32).min(1.0))
    }
}

/// A log line made readable: an event becomes [`ToolEvent::readable`], any
/// other line comes back as it is.
pub fn readable_line(line: &str) -> Option<String> {
    ToolEvent::parse(line).map(|event| event.readable())
}

#[cfg(test)]
mod tests {
    use super::*;

    const DUMP: &str = include_str!("../tests/data/dump.jsonl");
    const LOAD: &str = include_str!("../tests/data/load.jsonl");

    fn state(text: &str) -> RunState {
        let mut state = RunState::default();
        state.take_text(text);
        state
    }

    #[test]
    fn an_event_line_is_parsed_with_its_fields() {
        let line = r#"{"seq":5,"ts":"2026-11-27 16:51:50.146814","level":"ERROR","event":"mysql_connect","phase":"startup","status":"failed","message":"Error connection to database","fatal":true,"retryable":true,"mysql_errno":"2002","port":"1"}"#;
        let event = ToolEvent::parse(line).unwrap();
        assert_eq!(event.level, Level::Error);
        assert!(event.fatal);
        assert_eq!(event.number("port"), Some(1));
        assert_eq!(event.field("retryable"), Some("true"));
        assert_eq!(event.time(), "16:51:50");
        assert_eq!(
            event.readable(),
            "16:51:50 ERROR: Error connection to database (MySQL error 2002)"
        );
        assert!(event.is_problem());
    }

    #[test]
    fn text_lines_are_not_events() {
        assert_eq!(
            ToolEvent::parse("2026-11-27 16:29:53 [INFO] - Thread 3"),
            None
        );
        assert_eq!(ToolEvent::parse("{not json"), None);
        assert_eq!(ToolEvent::parse(r#"{"no":"event"}"#), None);
    }

    #[test]
    fn a_load_log_gives_files_tables_stage_and_summary() {
        let state = state(LOAD);
        assert!(state.json);
        assert_eq!(state.stage, Some(Stage::Done));
        assert_eq!(state.files, Some((12, 12)));
        assert_eq!(state.fraction(), Some(1.0));
        assert!(state.threads.is_empty());
        assert!(state.checksum_mismatches.is_empty());
        let summary = state.summary.unwrap();
        assert_eq!((summary.tables, summary.files, summary.errors), (3, 18, 0));
        // The GLib assertion myloader logs as an error on every run is noise.
        assert!(state.problems.is_empty(), "{:?}", state.problems);
    }

    #[test]
    fn a_load_in_progress_shows_what_each_thread_loads() {
        let half: String = LOAD
            .lines()
            .take_while(|line| !line.contains("\"index_job\""))
            .map(|line| format!("{line}\n"))
            .collect();
        let state = state(&half);
        assert_eq!(state.stage, Some(Stage::Data));
        let (done, total) = state.files.unwrap();
        assert!(done > 0 && done <= total && total == 12);
        assert!(state.threads.values().any(|a| a.table == "big"));
    }

    #[test]
    fn a_dump_log_gives_tables_and_summary_and_keeps_its_warning() {
        let state = state(DUMP);
        assert_eq!(state.stage, Some(Stage::Done));
        assert_eq!(state.summary.as_ref().unwrap().files, 18);
        assert_eq!(state.problems.len(), 1);
        assert_eq!(state.problems[0].level, Level::Warning);
        assert!(state.problems[0].text.contains("--trx-tables"));
    }

    #[test]
    fn a_fatal_error_and_a_checksum_mismatch_are_problems() {
        let state = state(concat!(
            r#"{"ts":"2026-11-27 16:51:50.1","level":"ERROR","event":"mysql_connect","status":"failed","message":"Error connection to database","fatal":true,"mysql_errno":"2002"}"#,
            "\n",
            r#"{"ts":"2026-11-27 16:51:51.1","level":"INFO","event":"checksum_check","status":"finished","message":"checksum verification","fatal":false,"table":"big","expected_checksum":"aa","actual_checksum":"bb"}"#,
            "\n",
        ));
        assert_eq!(state.problems.len(), 1);
        assert!(state.problems[0].fatal);
        assert_eq!(state.checksum_mismatches, vec!["big".to_string()]);
    }

    #[test]
    fn a_text_log_still_gives_the_file_count() {
        let state = state(
            "2026-11-27 16:30:01 [INFO] - Thread 1: restoring db.a part 192 of 57 from db.a.00295.sql.zst | Progress 622 of 698. Tables 0 of 65 completed\n",
        );
        assert!(!state.json);
        assert_eq!(state.files, Some((622, 698)));
        assert_eq!(state.stage, Some(Stage::Data));
    }

    #[test]
    fn the_same_problem_is_counted_once() {
        let line = r#"{"ts":"2026-11-27 16:51:50.1","level":"WARNING","event":"x","message":"slow","fatal":false}"#;
        let state = state(&format!("{line}\n{line}\n"));
        assert_eq!(state.problems.len(), 1);
        assert_eq!(state.problems[0].count, 2);
    }

    /// The tables of the fixture dump, 1000 bytes a file.
    fn fixture_tables() -> Vec<TableFiles> {
        let files = |name: &str, chunks: std::ops::Range<u32>| TableFiles {
            name: name.into(),
            files: chunks
                .map(|i| (format!("shop.{name}.{i:05}.sql.zst"), 1000))
                .collect(),
        };
        vec![
            files("big", 1..12),
            files("empty", 0..0),
            files("small", 0..1),
        ]
    }

    fn load_state(text: &str) -> RunState {
        let mut state = RunState::for_load(fixture_tables());
        state.take_text(text);
        state
    }

    #[test]
    fn a_load_lists_every_table_from_the_start() {
        let state = load_state("");
        let names: Vec<&str> = state.table_rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["big", "empty", "small"]);
        assert!(
            state
                .table_rows
                .iter()
                .all(|r| r.status == TableStatus::Waiting)
        );
        assert_eq!(state.table_rows[0].detail(&state, None), "waiting");
        assert_eq!(state.table_rows[0].bytes, Some((0, 11_000)));
        assert_eq!(state.rows_done(), (0, 3));
    }

    #[test]
    fn a_load_in_progress_counts_the_bytes_of_finished_files() {
        let half: String = LOAD
            .lines()
            .take_while(|line| !line.contains("\"index_job\""))
            .map(|line| format!("{line}\n"))
            .collect();
        let state = load_state(&half);
        let big = &state.table_rows[0];
        let (done, total) = big.bytes.unwrap();
        assert_eq!(total, 11_000);
        assert!(done > 0 && done < total, "{done}");
        let share = big.share(&state, None);
        assert!(!share.estimated);
        assert_eq!(share.fraction, done as f32 / 11_000.0);
        assert_eq!(big.detail(&state, None), share.percent());
        // The view's stand-in table is not added as a table.
        assert!(state.table_rows.iter().all(|r| r.name != "vw"));
    }

    #[test]
    fn a_finished_load_has_every_table_done_with_its_checksum() {
        let state = load_state(LOAD);
        assert_eq!(state.rows_done(), (3, 3));
        let big = &state.table_rows[0];
        assert_eq!(big.files, Some((11, 11)));
        assert_eq!(big.checksum_ok, Some(true));
        assert_eq!(big.detail(&state, None), "100%");
        assert_eq!(big.share(&state, None).fraction, 1.0);
        assert!(state.running.is_empty());
        assert!(state.active_tables().is_empty());
        assert_eq!(state.share(None).unwrap().fraction, 1.0);
    }

    /// `restoring data chunk` of `file` by `thread` at `time` (seconds).
    fn chunk(thread: u32, file: &str, time: &str) -> String {
        format!(
            r#"{{"ts":"2026-11-27 10:00:{time}","level":"INFO","event":"restore_data_progress","status":"progress","message":"restoring data chunk","fatal":false,"thread_id":"{thread}","table":"t","filename":"{file}"}}"#
        ) + "\n"
    }

    #[test]
    fn the_file_in_work_is_estimated_from_the_speed_of_those_before_it() {
        let mut state = RunState::for_load(vec![TableFiles {
            name: "t".into(),
            files: vec![
                ("db.t.00000.sql".into(), 1000),
                ("db.t.00001.sql".into(), 1000),
            ],
        }]);
        // The first file takes one second: 1000 bytes a second.
        state.take_text(&chunk(1, "db.t.00000.sql", "00.000"));
        state.take_text(&chunk(1, "db.t.00001.sql", "01.000"));
        let at = |time: &str| Some(format!("2026-11-27 10:00:{time}").parse().unwrap());
        let row = &state.table_rows[0];
        assert_eq!(row.bytes, Some((1000, 2000)));

        // Half a second into the second file: about 500 of its bytes.
        let share = row.share(&state, at("01.500"));
        assert!(share.estimated);
        assert_eq!(share.fraction, 0.75);
        assert_eq!(row.detail(&state, at("01.500")), "~75%");
        // Without a time, only what is known.
        assert_eq!(row.detail(&state, None), "50%");
        // Long after: never past 95% of the file before the tool moves on.
        assert_eq!(row.share(&state, at("30.000")).fraction, 0.975);
        assert_eq!(state.share(at("01.500")).unwrap().percent(), "~75%");
    }

    #[test]
    fn a_dump_adds_tables_as_it_writes_them() {
        let mut state = RunState::for_dump();
        assert_eq!(state.share(None), None);
        let schema_and_some: String = DUMP
            .lines()
            .take_while(|line| !line.contains("\"progress_pct\":\"50\""))
            .map(|line| format!("{line}\n"))
            .collect();
        state.take_text(&schema_and_some);
        let names: Vec<&str> = state.table_rows.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["big", "small"]);
        let big = &state.table_rows[0];
        assert_eq!(big.status, TableStatus::Rows);
        assert_eq!(big.detail(&state, None), "40%");
        assert_eq!(state.active_tables(), ["big"]);
        // Two tables: `small` done, `big` 40% in, so (1 + 0.4) / 2. The
        // view is not counted, though mydumper's own total has it.
        assert_eq!(state.table_rows[1].status, TableStatus::Done);
        assert_eq!(state.share(None).unwrap().fraction, 0.7);
        assert_eq!(state.share(None).unwrap().percent(), "70%");

        state.take_text(DUMP);
        assert_eq!(state.rows_done(), (2, 2));
        assert_eq!(state.share(None).unwrap().fraction, 1.0);
    }

    #[test]
    fn an_ignored_error_is_not_a_problem_though_marked_fatal() {
        let line = r#"{"ts":"2026-11-27 17:28:42.8","level":"WARNING","event":"mysql_operation","status":"failed","message":"Set session failed: SET SESSION SQL_LOG_BIN = 0 ","fatal":true,"mysql_errno":"1227"}"#;
        let mut ignoring = RunState::default().ignoring(&["1227".to_string()]);
        ignoring.take_line(line);
        assert!(ignoring.problems.is_empty());
        let mut plain = RunState::default();
        plain.take_line(line);
        assert_eq!(plain.problems.len(), 1);
    }
}
