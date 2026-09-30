//! Provider adapters. Each exposes one blocking operation: `anchor`, which starts a 5-hour
//! window only if none is running, and reports the provider's authoritative window state.

pub mod claude;
pub mod codex;

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::config::{Config, ProviderConfig};
use crate::platform;
use crate::state::{ProviderState, State};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Codex,
    Claude,
}

/// Window state as reported by the provider (never guessed).
#[derive(Debug, Clone, PartialEq)]
pub struct Observation {
    pub resets_at: Option<i64>,
    pub used_percent: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// We started a new window.
    Anchored,
    /// A window was already running; nothing changed (Codex: no request sent).
    AlreadyActive,
}

impl Provider {
    pub const ALL: [Provider; 2] = [Provider::Codex, Provider::Claude];

    pub fn name(self) -> &'static str {
        match self {
            Provider::Codex => "Codex",
            Provider::Claude => "Claude",
        }
    }

    pub fn config(self, c: &Config) -> &ProviderConfig {
        match self {
            Provider::Codex => &c.codex,
            Provider::Claude => &c.claude,
        }
    }

    pub fn state(self, s: &State) -> &ProviderState {
        match self {
            Provider::Codex => &s.codex,
            Provider::Claude => &s.claude,
        }
    }

    pub fn state_mut(self, s: &mut State) -> &mut ProviderState {
        match self {
            Provider::Codex => &mut s.codex,
            Provider::Claude => &mut s.claude,
        }
    }

    /// Blocking (seconds to a minute); run on a worker thread.
    pub fn anchor(self, cfg: &ProviderConfig) -> Result<(Outcome, Observation), String> {
        match self {
            Provider::Codex => codex::anchor(cfg),
            Provider::Claude => claude::anchor(cfg),
        }
    }
}

pub fn now() -> i64 {
    jiff::Timestamp::now().as_second()
}

/// Configured path, else the first `name{.exe,.cmd}` on PATH.
fn resolve(command: &str, name: &str) -> Result<PathBuf, String> {
    if !command.trim().is_empty() {
        return Ok(PathBuf::from(command.trim()));
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path)
        .flat_map(|dir| {
            platform::EXE_SUFFIXES
                .iter()
                .map(move |s| dir.join(format!("{name}{s}")))
        })
        .find(|p| p.is_file())
        .ok_or_else(|| format!("{name} CLI not found on PATH (set its path in Settings)"))
}

/// A short-lived, hidden provider process read line by line with a hard deadline.
/// Dropping it kills the process and anything it spawned.
struct Session {
    child: Child,
    _guard: platform::ProcessGuard,
    stdin: Option<ChildStdin>,
    lines: Receiver<String>,
    stderr: Arc<Mutex<String>>,
    deadline: Instant,
}

impl Session {
    fn spawn(program: &PathBuf, args: &[&str], timeout: Duration) -> Result<Self, String> {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(std::env::temp_dir())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        platform::hide_window(&mut cmd);
        let mut child = cmd
            .spawn()
            .map_err(|e| format!("failed to start {}: {e}", program.display()))?;
        let guard = platform::contain(&child);

        let (tx, lines) = channel();
        let stdout = child.stdout.take().unwrap();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let stderr = Arc::new(Mutex::new(String::new()));
        let (mut err_pipe, sink) = (child.stderr.take().unwrap(), stderr.clone());
        std::thread::spawn(move || {
            let mut buf = String::new();
            let _ = err_pipe.read_to_string(&mut buf);
            *sink.lock().unwrap() = buf;
        });

        Ok(Self {
            stdin: child.stdin.take(),
            child,
            _guard: guard,
            lines,
            stderr,
            deadline: Instant::now() + timeout,
        })
    }

    fn send(&mut self, line: &str) -> Result<(), String> {
        let stdin = self.stdin.as_mut().ok_or("stdin closed")?;
        writeln!(stdin, "{line}")
            .and_then(|_| stdin.flush())
            .map_err(|e| format!("write failed: {e}"))
    }

    fn close_stdin(&mut self) {
        self.stdin = None;
    }

    /// Next stdout line; None at end of output.
    fn next_line(&mut self) -> Result<Option<String>, String> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        match self.lines.recv_timeout(left) {
            Ok(l) => Ok(Some(l)),
            Err(RecvTimeoutError::Disconnected) => Ok(None),
            Err(RecvTimeoutError::Timeout) => Err("timed out".into()),
        }
    }

    /// Last bit of stderr, for error messages once output has ended.
    fn stderr_tail(&self) -> String {
        std::thread::sleep(Duration::from_millis(100)); // let the stderr reader finish
        let s = self.stderr.lock().unwrap();
        let s = s.trim();
        let start = s.char_indices().rev().nth(300).map_or(0, |(i, _)| i);
        s[start..].to_string()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
