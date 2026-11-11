//! Runs a short command and collects what it prints, with a time limit.
//!
//! Used for quick questions to a tool ("what version are you?", "which
//! databases exist?"). Long runs with progress are in [`crate::run`].

use std::io::Read;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct Captured {
    /// The exit code; `None` when the process was ended by a signal.
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Captured {
    pub fn success(&self) -> bool {
        self.code == Some(0)
    }
}

#[derive(Debug)]
pub enum CaptureError {
    /// The program could not be started at all.
    NotStarted(std::io::Error),
    /// Still running after the time limit; it was killed.
    TimedOut,
}

pub fn capture(command: &mut Command, limit: Duration) -> Result<Captured, CaptureError> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(CaptureError::NotStarted)?;

    // Each pipe is read on its own thread. Reading them one after the other
    // here could block forever: a child that fills the stderr pipe waits for
    // us while we wait for its stdout to end.
    let stdout = read_in_background(child.stdout.take());
    let stderr = read_in_background(child.stderr.take());

    let deadline = Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(10)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                // The reader threads are not joined: a grandchild may still
                // hold the pipes open. They end on their own when it exits.
                return Err(CaptureError::TimedOut);
            }
            Err(error) => return Err(CaptureError::NotStarted(error)),
        }
    };
    Ok(Captured {
        code: status.code(),
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

fn read_in_background<R: Read + Send + 'static>(pipe: Option<R>) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn sh(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        command
    }

    #[test]
    fn collects_both_streams_and_the_exit_code() {
        let out = capture(
            &mut sh("echo out; echo err >&2; exit 3"),
            Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(out.code, Some(3));
        assert_eq!(out.stdout, "out\n");
        assert_eq!(out.stderr, "err\n");
        assert!(!out.success());
    }

    #[test]
    fn a_slow_command_is_stopped_at_the_limit() {
        let started = Instant::now();
        let result = capture(&mut sh("sleep 30"), Duration::from_millis(100));
        assert!(matches!(result, Err(CaptureError::TimedOut)));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_missing_program_is_reported() {
        let result = capture(
            &mut Command::new("/no/such/program"),
            Duration::from_secs(1),
        );
        assert!(matches!(result, Err(CaptureError::NotStarted(_))));
    }
}
