//! Text for people: summaries before a run and reports after it.
//!
//! The core decides *what* is said; the front ends decide *how it looks*.
//! A report is a list of [`Line`]s. The command line tool prints them with
//! colours, the app draws them as rows. Both therefore show the same words.

use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Line {
    /// A heading, such as `slipdock dump`.
    Title(String),
    /// `Label    value  note`. The note is shown dimmed.
    Row {
        label: String,
        value: String,
        note: Option<String>,
        /// Shown in bold: the one value the reader must not miss.
        strong: bool,
    },
    /// `✓ text  note`
    Ok {
        text: String,
        note: Option<String>,
    },
    /// `✗ text`
    Fail(String),
    /// `! text`
    Warn(String),
    /// An indented line of plain text.
    Text(String),
    /// An indented line of less important text.
    Dim(String),
    /// `│ text`: a line the tool printed itself, quoted in the report. A
    /// front end that shows the tool's output in another way leaves these
    /// lines out.
    Quote(String),
    Blank,
}

impl Line {
    pub fn row(label: &str, value: impl Into<String>) -> Line {
        Line::Row {
            label: label.to_string(),
            value: value.into(),
            note: None,
            strong: false,
        }
    }

    pub fn row_note(label: &str, value: impl Into<String>, note: impl Into<String>) -> Line {
        Line::Row {
            label: label.to_string(),
            value: value.into(),
            note: Some(note.into()),
            strong: false,
        }
    }

    /// The line without colours.
    pub fn plain(&self) -> String {
        let with_note = |text: &str, note: &Option<String>| match note {
            Some(note) => format!("{text}  {note}"),
            None => text.to_string(),
        };
        match self {
            Line::Title(text) => text.clone(),
            Line::Row {
                label, value, note, ..
            } => format!("  {label:<9} {}", with_note(value, note)),
            Line::Ok { text, note } => format!("✓ {}", with_note(text, note)),
            Line::Fail(text) => format!("✗ {text}"),
            Line::Warn(text) => format!("! {text}"),
            Line::Text(text) | Line::Dim(text) => format!("  {text}"),
            Line::Quote(text) => format!("  │ {text}"),
            Line::Blank => String::new(),
        }
    }
}

/// What a run came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub lines: Vec<Line>,
    pub success: bool,
}

impl Report {
    pub fn plain(&self) -> String {
        plain(&self.lines)
    }
}

pub fn plain(lines: &[Line]) -> String {
    lines.iter().map(Line::plain).collect::<Vec<_>>().join("\n")
}

/// What a desktop notification says when a run has ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub body: String,
}

/// The notice for a finished run: the report's verdict (its first ✓ or ✗
/// line) as the title, the database and the verdict's note as the body.
/// `Dump complete` / `example_db · 1:54 · 65 tables · 15M`.
pub fn notice(database: &str, report: &Report) -> Notice {
    let verdict = report.lines.iter().find_map(|line| match line {
        Line::Ok { text, note } => Some((text.clone(), note.clone())),
        Line::Fail(text) => Some((text.clone(), None)),
        _ => None,
    });
    let (title, note) = verdict.unwrap_or_else(|| {
        let text = if report.success { "Finished" } else { "Failed" };
        (text.to_string(), None)
    });
    let body = match note {
        Some(note) => format!("{database} · {note}"),
        None => database.to_string(),
    };
    Notice { title, body }
}

/// `83 s` -> `1:23`.
pub fn duration(elapsed: Duration) -> String {
    let seconds = elapsed.as_secs();
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// The row shown when certificate checking is off for a profile.
pub fn tls_note() -> Line {
    Line::row(
        "Note",
        "TLS certificate check is off (encrypted, server not verified)",
    )
}

/// The detail after a run's label and time on its progress line:
/// ` · Data · 89% · about 1:15 left` for a load, ` · Data · 20M` for a
/// dump. No file counts: how mydumper splits a table is not the user's
/// concern.
pub fn progress_detail(progress: &crate::run::Progress) -> String {
    progress_detail_at(progress, Some(crate::tool_event::now()))
}

/// [`progress_detail`] at a given time of day, which the estimate for the
/// files in work is made for.
pub fn progress_detail_at(
    progress: &crate::run::Progress,
    now: Option<jiff::civil::DateTime>,
) -> String {
    let mut text = String::new();
    let state = &progress.state;
    if let Some(stage) = state.stage.filter(|_| state.json) {
        text.push_str(&format!(" · {}", stage.name()));
    }
    if let Some((_, size)) = progress.written {
        text.push_str(&format!(" · {}", crate::dumps::human_size(size)));
    }
    if let Some(share) = state.share(now) {
        text.push_str(&format!(" · {}", share.percent()));
        let done = f64::from(share.fraction);
        if done > 0.0 && done < 1.0 {
            let left = progress.elapsed.mul_f64((1.0 - done) / done);
            text.push_str(&format!(" · about {} left", duration(left)));
        }
    } else if let Some((done, total)) = state.tables {
        text.push_str(&format!(" · {done} of {total} tables"));
    }
    text
}

/// The line over a run's table list: `3 of 65 tables done · now
/// account_targets, targets`.
pub fn tables_heading(state: &crate::tool_event::RunState) -> String {
    let (done, total) = state.rows_done();
    let mut text = format!("{done} of {total} tables done");
    let active = state.active_tables();
    if !active.is_empty() {
        text.push_str(&format!(" · now {}", active.join(", ")));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_as_plain_text() {
        let lines = [
            Line::Title("slipdock dump".into()),
            Line::row_note("Profile", "example", "user@host:3306"),
            Line::row("Database", "example_db"),
            Line::row("", "second line"),
            Line::Ok {
                text: "Dump complete".into(),
                note: Some("0:05".into()),
            },
            Line::Fail("Load failed".into()),
            Line::Warn("Interrupted.".into()),
            Line::Text("  detail".into()),
            Line::Quote("CRITICAL: no zstd".into()),
            Line::Blank,
        ];
        assert_eq!(
            plain(&lines),
            "slipdock dump\n  Profile   example  user@host:3306\n  Database  example_db\n            second line\n\
             ✓ Dump complete  0:05\n✗ Load failed\n! Interrupted.\n    detail\n  │ CRITICAL: no zstd\n"
        );
    }

    #[test]
    fn notice_takes_the_verdict() {
        let report = Report {
            lines: vec![
                Line::Quote("tool output".into()),
                Line::Ok {
                    text: "Dump complete".into(),
                    note: Some("1:54 · 65 tables · 15M".into()),
                },
                Line::row("Folder", "~/dumps/x"),
            ],
            success: true,
        };
        assert_eq!(
            notice("example_db", &report),
            Notice {
                title: "Dump complete".into(),
                body: "example_db · 1:54 · 65 tables · 15M".into(),
            }
        );
        let failed = Report {
            lines: vec![Line::Fail("Load failed (exit 1) after 0:02".into())],
            success: false,
        };
        assert_eq!(
            notice("example_db", &failed),
            Notice {
                title: "Load failed (exit 1) after 0:02".into(),
                body: "example_db".into(),
            }
        );
        let empty = Report {
            lines: vec![],
            success: false,
        };
        assert_eq!(notice("example_db", &empty).title, "Failed");
    }

    #[test]
    fn progress_detail_names_the_stage_share_and_time_left() {
        use crate::run::Progress;
        use crate::tool_event::{RunState, Stage};
        let progress = Progress {
            elapsed: Duration::from_secs(60),
            written: None,
            state: RunState {
                json: true,
                stage: Some(Stage::Data),
                files: Some((25, 100)),
                ..RunState::default()
            },
        };
        assert_eq!(
            progress_detail(&progress),
            format!(
                " · Data · 25% · about {} left",
                duration(Duration::from_secs(180))
            )
        );
        let dump = Progress {
            elapsed: Duration::from_secs(5),
            written: Some((3, 2048)),
            state: RunState::default(),
        };
        assert_eq!(progress_detail(&dump), " · 2.0K");
    }

    #[test]
    fn the_tables_heading_counts_done_and_names_the_active_ones() {
        use crate::tool_event::RunState;
        use crate::tool_event::TableFiles;
        let table = |name: &str| TableFiles {
            name: name.into(),
            files: vec![(format!("db.{name}.00000.sql.zst"), 10)],
        };
        let state = RunState::for_load(vec![table("a"), table("b")]);
        assert_eq!(tables_heading(&state), "0 of 2 tables done");
    }

    #[test]
    fn durations() {
        assert_eq!(duration(Duration::from_secs(0)), "0:00");
        assert_eq!(duration(Duration::from_secs(83)), "1:23");
        assert_eq!(duration(Duration::from_secs(3723)), "62:03");
    }
}
