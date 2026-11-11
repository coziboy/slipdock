//! Runs mydumper or myloader and reports progress while it works.
//!
//! [`start`] returns at once. The tool runs under a watcher thread, and the
//! caller receives [`Event`]s over a channel: a `Progress` a few times per
//! second, then one `Finished`. The command line tool reads the channel on
//! its main thread; the app reads it on a background task. Neither needs to
//! know how the other does it.
//!
//! ```text
//!   caller                       watcher thread               tool
//!     | start() ------------------> spawn --------------------> |
//!     | <------- Event::Progress -- every 200 ms                |
//!     | cancel.cancel() ----------> sees the flag -- SIGTERM -> |
//!     | <------- Event::Finished -- child exited <------------- |
//! ```

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use crate::command::Invocation;
use crate::{Error, Result, dumps, platform};

const TICK: Duration = Duration::from_millis(200);
/// The dump folder is measured every this many ticks (once per second).
const MEASURE_EVERY: u32 = 5;
/// How much of the tool's own output is kept for the report.
const HEAD_LINES: usize = 4;
const HEAD_WIDTH: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub elapsed: Duration,
    /// For dumps: files written so far and their total size.
    pub written: Option<(usize, u64)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    /// The tool's exit code; `None` when a signal ended it.
    pub code: Option<i32>,
    /// True when the run was cancelled (Ctrl-C or the Cancel button).
    pub interrupted: bool,
    pub elapsed: Duration,
    /// The first lines the tool printed itself (not its log file), without
    /// empty lines and tidied with [`tidy_line`].
    pub output_head: Vec<String>,
    /// How many lines it printed in total, not counting empty ones.
    pub output_lines: usize,
}

impl Outcome {
    /// One number for "how did it end", following shell custom: 130 for an
    /// interrupted run, 1 when a signal ended the tool.
    pub fn exit_code(&self) -> i32 {
        if self.interrupted {
            130
        } else {
            self.code.unwrap_or(1)
        }
    }
}

#[derive(Debug)]
pub enum Event {
    Progress(Progress),
    Finished(Outcome),
}

/// A switch that asks the running tool to stop. It can be cloned and used
/// from any thread; the watcher thread sees it on its next tick.
#[derive(Debug, Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    /// The flag itself, for a signal handler to set.
    pub fn flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.0)
    }
}

pub struct Running {
    pub events: Receiver<Event>,
    pub cancel: Cancel,
}

/// Starts the tool.
///
/// - `watch`: the dump folder to measure for progress, if any.
/// - `log`: whatever the tool prints to its terminal is appended to this
///   file at the end, under a `--- tool output ---` line. (The tool writes
///   its normal log there itself, through `--logfile`.)
pub fn start(invocation: &Invocation, watch: Option<&Path>, log: &Path) -> Result<Running> {
    // stdout and stderr go to one unnamed temp file, in the order written.
    let mut output = tempfile::tempfile().map_err(|source| Error::Io {
        context: "cannot create a temporary file".into(),
        source,
    })?;
    let clone = |file: &File| {
        file.try_clone().map_err(|source| Error::Io {
            context: "cannot prepare the tool's output".into(),
            source,
        })
    };
    let mut child = invocation
        .to_command()
        .stdin(Stdio::null())
        .stdout(clone(&output)?)
        .stderr(clone(&output)?)
        .spawn()
        .map_err(Error::io_at("cannot start", &invocation.program))?;

    let (sender, events) = mpsc::channel();
    let cancel = Cancel::default();
    let watcher_cancel = cancel.clone();
    let watch: Option<PathBuf> = watch.map(Path::to_path_buf);
    let log = log.to_path_buf();

    thread::spawn(move || {
        let started = Instant::now();
        let mut signalled = false;
        let mut written = None;
        let mut tick = 0;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {}
                Err(_) => break None,
            }
            if watcher_cancel.is_cancelled() && !signalled {
                // Ask once, then keep waiting: the tool finishes its current
                // step and exits on its own.
                let _ = platform::terminate(child.id());
                signalled = true;
            }
            if let Some(dir) = watch.as_deref()
                && tick % MEASURE_EVERY == 0
                && dir.is_dir()
            {
                written = Some((dumps::sql_file_count(dir), dumps::dir_size(dir)));
            }
            // A send fails only when the receiver is gone; the run goes on.
            let _ = sender.send(Event::Progress(Progress {
                elapsed: started.elapsed(),
                written,
            }));
            tick += 1;
            thread::sleep(TICK);
        };

        let text = read_all(&mut output);
        append_tool_output(&log, &text);
        let lines: Vec<String> = text
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(tidy_line)
            .collect();
        let _ = sender.send(Event::Finished(Outcome {
            code: status.and_then(|status| status.code()),
            interrupted: watcher_cancel.is_cancelled(),
            elapsed: started.elapsed(),
            output_head: lines
                .iter()
                .take(HEAD_LINES)
                .map(|line| line.chars().take(HEAD_WIDTH).collect())
                .collect(),
            output_lines: lines.len(),
        }));
    });

    Ok(Running { events, cancel })
}

fn read_all(file: &mut File) -> String {
    let mut bytes = Vec::new();
    if file.seek(SeekFrom::Start(0)).is_ok() {
        let _ = file.read_to_end(&mut bytes);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The line in the log file above what the tool printed to its terminal.
pub const TOOL_OUTPUT_MARKER: &str = "--- tool output ---";

fn append_tool_output(log: &Path, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Ok(mut file) = OpenOptions::new().append(true).create(true).open(log) {
        let _ = write!(file, "{TOOL_OUTPUT_MARKER}\n{text}");
    }
}

/// Makes a line the tool printed easier to read. mydumper and myloader
/// print through GLib, which wraps each message:
///
/// ```text
/// ** (mydumper:42720): CRITICAL **: 12:55:04.027: zstd was not found
/// ```
///
/// becomes `CRITICAL: zstd was not found`. Other lines are returned as they
/// are. The log file keeps the full text.
pub fn tidy_line(line: &str) -> String {
    let parsed = || {
        let rest = line.strip_prefix("** ")?;
        // Plain messages have a shorter form: `** Message: 12:55:04.027: text`.
        let (head, message) = rest.split_once(" **: ").or_else(|| {
            rest.split_once(": ")
                .filter(|(word, _)| word.chars().all(|c| c.is_ascii_alphabetic()))
        })?;
        let level = head.rsplit(' ').next()?;
        // The time of day, `12:55:04.027: `, when GLib adds one.
        let message = match message.split_once(": ") {
            Some((time, rest))
                if !time.is_empty()
                    && time
                        .chars()
                        .all(|c| c.is_ascii_digit() || c == ':' || c == '.') =>
            {
                rest
            }
            _ => message,
        };
        Some(format!("{level}: {message}"))
    };
    parsed().unwrap_or_else(|| line.to_string())
}

/// The distinct problems in a mydumper/myloader log, sorted: the text after
/// `[ERROR] - `, `[WARNING] - ` or `[CRITICAL] - ` of each such line.
pub fn log_problems(log: &Path) -> Vec<String> {
    let text = fs::read(log).unwrap_or_default();
    let text = String::from_utf8_lossy(&text);
    let mut problems: Vec<String> = text
        .lines()
        .filter_map(|line| {
            ["[ERROR]", "[WARNING]", "[CRITICAL]"]
                .iter()
                .find_map(|level| {
                    let at = line.find(level)?;
                    let rest = &line[at + level.len()..];
                    Some(rest.strip_prefix(" - ").unwrap_or(rest).to_string())
                })
        })
        .collect();
    problems.sort();
    problems.dedup();
    problems
}

/// How much of the end of a log is read to show its last lines. A log line
/// is some 100 to 300 bytes, so this holds well over a thousand of them.
pub const TAIL_BYTES: u64 = 512 * 1024;

/// The end of a file: at most `limit` bytes, and whether that left a part
/// of the file out. When it did, the bytes start at the first whole line.
pub fn read_end(path: &Path, limit: u64) -> std::io::Result<(Vec<u8>, bool)> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = fs::File::open(path)?;
    let size = file.metadata()?.len();
    let cut = size > limit;
    if cut {
        file.seek(SeekFrom::Start(size - limit))?;
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    if cut {
        // The read starts somewhere in a line: drop up to its end.
        let first_line_end = bytes.iter().position(|byte| *byte == b'\n');
        bytes.drain(..first_line_end.map_or(0, |at| at + 1));
    }
    Ok((bytes, cut))
}

/// The last `count` lines of a log file, for a live view: without empty
/// lines, and tidied with [`tidy_line`].
///
/// `with_tool_output: false` leaves out what was added under
/// [`TOOL_OUTPUT_MARKER`], for a view that shows that text elsewhere.
///
/// Only the end of the file is read ([`TAIL_BYTES`]), so a view that asks
/// every second stays quick however long the log has become.
pub fn log_tail(log: &Path, count: usize, with_tool_output: bool) -> Vec<String> {
    let (text, _) = read_end(log, TAIL_BYTES).unwrap_or_default();
    let text = String::from_utf8_lossy(&text);
    let mut lines: Vec<&str> = text.lines().collect();
    if !with_tool_output
        && let Some(marker) = lines.iter().rposition(|line| *line == TOOL_OUTPUT_MARKER)
    {
        lines.truncate(marker);
    }
    lines.retain(|line| !line.trim().is_empty());
    let start = lines.len().saturating_sub(count);
    lines[start..].iter().map(|line| tidy_line(line)).collect()
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn sh(script: &str) -> Invocation {
        Invocation {
            program: PathBuf::from("/bin/sh"),
            args: vec!["-c".into(), script.into()],
            env: vec![("SLIPDOCK_TEST_VALUE".into(), "from-env".into())],
        }
    }

    /// Reads events until the run ends. Returns the outcome and how many
    /// progress events came first.
    fn wait(running: &Running) -> (Outcome, Vec<Progress>) {
        let mut progress = Vec::new();
        for event in &running.events {
            match event {
                Event::Progress(p) => progress.push(p),
                Event::Finished(outcome) => return (outcome, progress),
            }
        }
        panic!("the channel closed without Event::Finished");
    }

    #[test]
    fn a_successful_run_reports_progress_and_appends_output_to_the_log() {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("run.log");
        fs::write(&log, "tool log line\n").unwrap();
        let out = temp.path().join("out");
        let script = format!(
            "mkdir {0} && echo x > {0}/a.sql.zst && echo $SLIPDOCK_TEST_VALUE && sleep 0.5",
            out.display()
        );

        let running = start(&sh(&script), Some(&out), &log).unwrap();
        let (outcome, progress) = wait(&running);

        assert_eq!(outcome.code, Some(0));
        assert_eq!(outcome.exit_code(), 0);
        assert!(!outcome.interrupted);
        assert_eq!(outcome.output_head, ["from-env"]);
        assert!(!progress.is_empty());
        assert_eq!(
            fs::read_to_string(&log).unwrap(),
            "tool log line\n--- tool output ---\nfrom-env\n"
        );
    }

    #[test]
    fn a_failing_run_keeps_the_first_lines_of_output() {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("run.log");
        let running = start(
            &sh("for i in 1 2 3 4 5 6; do echo line $i >&2; done; exit 3"),
            None,
            &log,
        )
        .unwrap();
        let (outcome, _) = wait(&running);
        assert_eq!(outcome.exit_code(), 3);
        assert_eq!(
            outcome.output_head,
            ["line 1", "line 2", "line 3", "line 4"]
        );
        assert_eq!(outcome.output_lines, 6);
    }

    #[test]
    fn a_silent_run_does_not_touch_the_log() {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("run.log");
        let running = start(&sh("true"), None, &log).unwrap();
        wait(&running);
        assert!(!log.exists());
    }

    #[test]
    fn cancel_stops_the_tool_and_marks_the_run_interrupted() {
        let temp = tempfile::tempdir().unwrap();
        let running = start(&sh("exec sleep 30"), None, &temp.path().join("run.log")).unwrap();
        running.cancel.cancel();
        let (outcome, _) = wait(&running);
        assert!(outcome.interrupted);
        assert_eq!(outcome.exit_code(), 130);
        assert!(outcome.elapsed < Duration::from_secs(10));
    }

    #[test]
    fn a_program_that_does_not_exist_is_an_error() {
        let invocation = Invocation {
            program: PathBuf::from("/no/such/tool"),
            args: Vec::new(),
            env: Vec::new(),
        };
        let error = start(&invocation, None, Path::new("/tmp/unused.log"))
            .err()
            .unwrap();
        assert!(error.to_string().starts_with("cannot start /no/such/tool"));
    }

    #[test]
    fn log_problems_are_distinct_and_sorted() {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("dump.log");
        fs::write(
            &log,
            "2026-01-02 03:04:05 [INFO] - Connected\n\
             2026-01-02 03:04:06 [WARNING] - Couldn't get master position\n\
             2026-01-02 03:04:07 [ERROR] - Broken thing\n\
             2026-01-02 03:04:08 [WARNING] - Couldn't get master position\n\
             ** (mydumper:1): CRITICAL **: not this format\n\
             2026-01-02 03:04:09 [CRITICAL] - Another\n",
        )
        .unwrap();
        assert_eq!(
            log_problems(&log),
            ["Another", "Broken thing", "Couldn't get master position"]
        );
        assert!(log_problems(&temp.path().join("missing.log")).is_empty());
        assert_eq!(log_tail(&log, 2, true).len(), 2);
    }

    #[test]
    fn glib_lines_are_tidied_and_other_lines_kept() {
        assert_eq!(
            tidy_line(
                "** (mydumper:42720): CRITICAL **: 12:55:04.027: zstd was not found in PATH, use --x"
            ),
            "CRITICAL: zstd was not found in PATH, use --x"
        );
        assert_eq!(
            tidy_line("** (myloader:7): WARNING **: no time: here"),
            "WARNING: no time: here"
        );
        assert_eq!(
            tidy_line("** Message: 01:02:03.004: hello"),
            "Message: hello"
        );
        for plain in [
            "could not connect",
            "** odd",
            "2026-01-02 03:04:05 [INFO] - Connected",
        ] {
            assert_eq!(tidy_line(plain), plain);
        }
    }

    #[test]
    fn only_the_end_of_a_long_file_is_read() {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("long.log");
        let text: String = (0..1000).map(|line| format!("line {line:04}\n")).collect();
        fs::write(&log, &text).unwrap();

        // The whole file fits: nothing is left out.
        let (bytes, cut) = read_end(&log, 1_000_000).unwrap();
        assert!(!cut);
        assert_eq!(bytes.len(), text.len());

        // 25 bytes from the end is in the middle of a line: the bytes start
        // at the next whole one.
        let (bytes, cut) = read_end(&log, 25).unwrap();
        assert!(cut);
        assert_eq!(String::from_utf8(bytes).unwrap(), "line 0998\nline 0999\n");

        assert!(read_end(&temp.path().join("missing.log"), 25).is_err());
        assert_eq!(log_tail(&log, 2, true), ["line 0998", "line 0999"]);
    }

    #[test]
    fn the_head_and_the_tail_skip_empty_lines() {
        let temp = tempfile::tempdir().unwrap();
        let log = temp.path().join("run.log");
        fs::write(&log, "first\n\nsecond\n").unwrap();
        let script = r#"printf '\n** (mydumper:1): CRITICAL **: 01:02:03.004: no zstd\n'; exit 1"#;
        let running = start(&sh(script), None, &log).unwrap();
        let (outcome, _) = wait(&running);
        assert_eq!(outcome.output_head, ["CRITICAL: no zstd"]);
        assert_eq!(outcome.output_lines, 1);
        // The file keeps what the tool printed, unchanged.
        assert!(fs::read_to_string(&log).unwrap().ends_with(
            "--- tool output ---\n\n** (mydumper:1): CRITICAL **: 01:02:03.004: no zstd\n"
        ));
        assert_eq!(
            log_tail(&log, 9, true),
            ["first", "second", TOOL_OUTPUT_MARKER, "CRITICAL: no zstd"]
        );
        assert_eq!(log_tail(&log, 9, false), ["first", "second"]);
    }
}
