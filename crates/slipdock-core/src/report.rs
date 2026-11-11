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
    fn durations() {
        assert_eq!(duration(Duration::from_secs(0)), "0:00");
        assert_eq!(duration(Duration::from_secs(83)), "1:23");
        assert_eq!(duration(Duration::from_secs(3723)), "62:03");
    }
}
