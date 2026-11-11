//! Terminal output and the small prompts of guided mode.
//!
//! All printing goes through `anstream`. It removes colour codes on its own
//! when the output is not a terminal, when `NO_COLOR` is set, or when
//! `--color never` was given, so the code here always writes colours and
//! never asks whether it should.

use std::io::{self, BufRead, IsTerminal, Write};

use anstyle::{AnsiColor, Style};
use slipdock_core::report::Line;

const BOLD: Style = Style::new().bold();
const DIM: Style = Style::new().dimmed();
const GREEN: Style = AnsiColor::Green.on_default();
const YELLOW: Style = AnsiColor::Yellow.on_default();
const RED: Style = AnsiColor::Red.on_default();
const CYAN: Style = AnsiColor::Cyan.on_default();

/// `{style}` switches a style on and `{style:#}` switches it off again.
fn paint(style: Style, text: &str) -> String {
    format!("{style}{text}{style:#}")
}

pub fn bold(text: &str) -> String {
    paint(BOLD, text)
}

pub fn dim(text: &str) -> String {
    paint(DIM, text)
}

fn styled(line: &Line) -> String {
    let note = |note: &Option<String>| match note {
        Some(note) => format!("  {}", dim(note)),
        None => String::new(),
    };
    match line {
        Line::Title(text) => format!("\n{}", bold(text)),
        Line::Row {
            label,
            value,
            note: row_note,
            strong,
        } => {
            let value = if *strong { bold(value) } else { value.clone() };
            format!(
                "  {} {value}{}",
                dim(&format!("{label:<9}")),
                note(row_note)
            )
        }
        Line::Ok {
            text,
            note: ok_note,
        } => {
            format!("{} {}{}", paint(GREEN, "✓"), bold(text), note(ok_note))
        }
        Line::Fail(text) => format!("{} {}", paint(RED, "✗"), bold(text)),
        Line::Warn(text) => format!("{} {text}", paint(YELLOW, "!")),
        Line::Text(text) => format!("  {text}"),
        Line::Dim(text) => format!("  {}", dim(text)),
        Line::Blank => String::new(),
    }
}

pub fn print_lines(lines: &[Line]) {
    for line in lines {
        anstream::println!("{}", styled(line));
    }
}

/// For a failed run: the whole report goes to stderr.
pub fn print_lines_err(lines: &[Line]) {
    for line in lines {
        anstream::eprintln!("{}", styled(line));
    }
}

pub fn warn(text: &str) {
    anstream::eprintln!("{} {text}", paint(YELLOW, "!"));
}

pub fn is_interactive() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

// --- Progress ---------------------------------------------------------------------

const FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// One line that is redrawn in place while a tool runs. Draws nothing when
/// the output is not a terminal (a log file should not fill with spinners).
pub struct ProgressLine {
    enabled: bool,
    frame: usize,
}

impl ProgressLine {
    pub fn new() -> Self {
        ProgressLine {
            enabled: io::stdout().is_terminal(),
            frame: 0,
        }
    }

    pub fn draw(&mut self, label: &str, elapsed: &str, detail: &str) {
        if !self.enabled {
            return;
        }
        // "\r" goes back to the start of the line, "\x1b[K" clears it.
        anstream::print!(
            "\r\x1b[K  {} {label} {elapsed}{}",
            paint(CYAN, FRAMES[self.frame % FRAMES.len()]),
            dim(detail)
        );
        let _ = io::stdout().flush();
        self.frame += 1;
    }

    pub fn clear(&self) {
        if self.enabled {
            anstream::print!("\r\x1b[K");
            let _ = io::stdout().flush();
        }
    }
}

// --- Prompts ----------------------------------------------------------------------

/// Shows `question` and reads one line. `None` when the input has ended
/// (Ctrl-D) or cannot be read.
pub fn ask(question: &str) -> Option<String> {
    anstream::print!("{question}");
    let _ = io::stdout().flush();
    let mut answer = String::new();
    match io::stdin().lock().read_line(&mut answer) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(answer.trim_end_matches(['\n', '\r']).to_string()),
    }
}

pub fn ask_yes_no(question: &str) -> bool {
    matches!(
        ask(&format!("{question} [y/N] ")).as_deref().map(str::trim),
        Some("y" | "Y" | "yes" | "Yes")
    )
}

/// A numbered menu. Returns the index of the chosen item, or `None` when the
/// user gives an empty answer or ends the input.
pub fn pick(title: &str, items: &[String]) -> Option<usize> {
    anstream::println!("\n{}", bold(title));
    for (index, item) in items.iter().enumerate() {
        anstream::println!("  {} {item}", dim(&format!("{:>2})", index + 1)));
    }
    loop {
        let answer = ask(&format!("Number (1-{}, empty to cancel): ", items.len()))?;
        let answer = answer.trim();
        if answer.is_empty() {
            return None;
        }
        match answer.parse::<usize>() {
            Ok(number) if (1..=items.len()).contains(&number) => return Some(number - 1),
            _ => anstream::println!("Please type a number from 1 to {}.", items.len()),
        }
    }
}

/// Like [`pick`], but a name that is not in the list may be typed instead.
/// With an empty list this is a plain question.
pub fn pick_or_type(title: &str, items: &[String]) -> Option<String> {
    if items.is_empty() {
        let answer = ask(&format!("{title}: "))?;
        let answer = answer.trim().to_string();
        return (!answer.is_empty()).then_some(answer);
    }
    anstream::println!("\n{}", bold(title));
    for (index, item) in items.iter().enumerate() {
        anstream::println!("  {} {item}", dim(&format!("{:>2})", index + 1)));
    }
    let answer = ask("Number, or type a name (empty to cancel): ")?;
    let answer = answer.trim();
    if answer.is_empty() {
        return None;
    }
    match answer.parse::<usize>() {
        Ok(number) if (1..=items.len()).contains(&number) => Some(items[number - 1].clone()),
        _ => Some(answer.to_string()),
    }
}
