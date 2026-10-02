//! Provider adapters. Each exposes one blocking operation: `anchor`, which starts a 5-hour
//! window only if none is running, and reports the provider's authoritative window state.

pub mod claude;
pub mod codex;

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, sync_channel};
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

    /// True if the request is sent without a status check first (Claude), so only its reply
    /// tells whether it started a 5h limit.
    pub fn sends_unchecked(self) -> bool {
        self == Provider::Claude
    }

    /// How long after a reported reset the 5h limit has surely ended: Claude rounds resets down.
    pub fn reset_slack(self) -> i64 {
        match self {
            Provider::Codex => codex::CLOCK_SKEW_SECS,
            Provider::Claude => claude::MAX_RESET_EXTRA_SECS,
        }
    }

    /// Blocking (seconds to a minute); run on a worker thread.
    pub fn anchor(self, cfg: &ProviderConfig) -> Result<(Outcome, i64), String> {
        match self {
            Provider::Codex => codex::anchor(cfg),
            Provider::Claude => claude::anchor(cfg),
        }
    }
}

pub fn now() -> i64 {
    jiff::Timestamp::now().as_second()
}

/// End of every "program not found" error; the tray uses it to point the user at Settings.
const NOT_FOUND: &str = "not found (set its path in Settings)";

pub fn is_not_found(error: &str) -> bool {
    error.ends_with(NOT_FOUND)
}

/// End of every error after a request may have reached the provider (and started a 5h limit).
/// Retrying could spend quota on a limit that is already running, so the tray does not.
const MAYBE_SENT: &str = "(may have been sent; not retried)";

pub fn maybe_sent(error: &str) -> bool {
    error.ends_with(MAYBE_SENT)
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
        .ok_or_else(|| format!("{name} CLI {NOT_FOUND}"))
}

/// Why `Session::spawn` failed. `launched`: the process did start, with its arguments (which
/// may carry a request), before it was stopped.
#[derive(Debug)]
struct SpawnError {
    message: String,
    launched: bool,
}

impl From<SpawnError> for String {
    fn from(e: SpawnError) -> String {
        e.message
    }
}

/// Stdout lines queued ahead of the reader; a noisier process then waits on its pipe.
const QUEUED_LINES: usize = 64;
/// Stdout input bytes one operation may produce. Queued lines and parsed values can use more
/// memory than this. Real replies are KBs (a Codex status read with models: 12 KB); more is an error.
const STDOUT_LIMIT: u64 = 4 << 20;
/// Stderr bytes kept: enough for `stderr_tail`, however much the process writes.
const STDERR_KEPT: usize = 4096;

/// A short-lived, hidden provider process read line by line with a hard deadline.
/// Dropping it kills the process and anything it spawned.
struct Session {
    child: Child,
    _guard: platform::ProcessGuard,
    stdin: Option<ChildStdin>,
    lines: Receiver<Result<String, String>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    deadline: Instant,
}

impl Session {
    fn spawn(program: &PathBuf, args: &[&str], timeout: Duration) -> Result<Self, SpawnError> {
        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(std::env::temp_dir())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        platform::hide_window(&mut cmd);
        let mut child = cmd.spawn().map_err(|e| SpawnError {
            message: match e.kind() {
                std::io::ErrorKind::NotFound => format!("{} {NOT_FOUND}", program.display()),
                _ => format!("failed to start {}: {e}", program.display()),
            },
            launched: false,
        })?;
        let guard = platform::contain(&child);
        #[cfg(test)]
        let guard = match std::env::var("CLANKSHIFT_TEST_UNCONTAINED") {
            Ok(arg) if args.contains(&arg.as_str()) => Err("injected failure".into()),
            _ => guard,
        };
        // Without containment a timeout could leave e.g. node/codex running, so stop it. It has
        // already been running, though, and may have acted on its arguments.
        let guard = guard.map_err(|e| {
            let _ = child.kill();
            let _ = child.wait();
            SpawnError {
                message: format!("could not contain {}: {e}", program.display()),
                launched: true,
            }
        })?;

        let (tx, lines) = sync_channel(QUEUED_LINES);
        let stdout = child.stdout.take().unwrap();
        std::thread::spawn(move || {
            // The byte limit applies while a line is read, so not even one line outgrows it.
            let mut out = BufReader::new(stdout).take(STDOUT_LIMIT + 1);
            loop {
                let mut buf = Vec::new();
                let line = match out.read_until(b'\n', &mut buf) {
                    Ok(0) => break,
                    Ok(_) if out.limit() == 0 => {
                        Err(format!("output exceeded {} MiB", STDOUT_LIMIT >> 20))
                    }
                    Ok(_) => {
                        // Line ends as `lines()` strips them: "\n" or "\r\n".
                        if buf.pop_if(|b| *b == b'\n').is_some() {
                            buf.pop_if(|b| *b == b'\r');
                        }
                        String::from_utf8(buf).map_err(|_| "output is not UTF-8".to_string())
                    }
                    Err(e) => Err(format!("output read failed: {e}")),
                };
                let failed = line.is_err();
                if tx.send(line).is_err() || failed {
                    break;
                }
            }
        });
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let (mut err_pipe, sink) = (child.stderr.take().unwrap(), stderr.clone());
        std::thread::spawn(move || {
            let mut chunk = [0; 4096];
            while let Ok(n @ 1..) = err_pipe.read(&mut chunk) {
                let mut kept = sink.lock().unwrap();
                kept.extend_from_slice(&chunk[..n]);
                let excess = kept.len().saturating_sub(STDERR_KEPT);
                kept.drain(..excess);
            }
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
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{line}")
            .and_then(|_| stdin.flush())
            .map_err(|e| format!("write failed: {e}"))
    }

    fn close_stdin(&mut self) {
        self.stdin = None;
    }

    /// Next stdout line; None at end of output. Past the deadline even queued lines are refused.
    fn next_line(&mut self) -> Result<Option<String>, String> {
        let left = self.deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err("timed out".into());
        }
        match self.lines.recv_timeout(left) {
            Ok(l) => l.map(Some),
            Err(RecvTimeoutError::Disconnected) => Ok(None),
            Err(RecvTimeoutError::Timeout) => Err("timed out".into()),
        }
    }

    /// Last bit of stderr, for error messages once output has ended.
    fn stderr_tail(&self) -> String {
        std::thread::sleep(Duration::from_millis(100)); // let the stderr reader finish
        let s = String::from_utf8_lossy(&self.stderr.lock().unwrap()).into_owned();
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_program_is_not_found() {
        let e = resolve("", "clankshift-no-such-program").unwrap_err();
        assert!(is_not_found(&e), "{e}");
        let e = Session::spawn(
            &PathBuf::from(r"Z:\nope\codex.exe"),
            &[],
            Duration::from_secs(1),
        )
        .err()
        .unwrap();
        assert!(!e.launched);
        assert!(is_not_found(&e.message), "{}", e.message);
        assert!(!is_not_found("model not found"));
    }

    /// Not a real test: a provider stand-in that writes stdout and stderr nonstop.
    #[test]
    #[ignore]
    fn fake_flood() {
        std::thread::spawn(|| {
            loop {
                eprintln!("err {}", "x".repeat(100));
            }
        });
        loop {
            println!("out");
        }
    }

    #[test]
    fn deadline_holds_against_queued_output() {
        let args = [
            "providers::tests::fake_flood",
            "--exact",
            "--ignored",
            "--nocapture",
        ];
        let exe = std::env::current_exe().unwrap();
        let mut s = Session::spawn(&exe, &args, Duration::from_millis(500)).unwrap();
        assert!(matches!(s.next_line(), Ok(Some(_))));
        std::thread::sleep(Duration::from_millis(700)); // output keeps queuing meanwhile
        assert_eq!(s.next_line(), Err("timed out".into()));
        assert!(s.stderr.lock().unwrap().len() <= STDERR_KEPT);
        assert!(s.stderr_tail().contains("err xxx"));
    }
    /// Not a real test: writes more stdout than `STDOUT_LIMIT`, as one unterminated line
    /// ("long") or as ordinary lines, then waits.
    #[test]
    #[ignore]
    fn fake_output() {
        use std::io::Write;
        let mut out = std::io::stdout().lock();
        let line = if std::env::args().any(|a| a == "long") {
            "x"
        } else {
            "x\n"
        };
        let block = line.repeat(1 << 10);
        for _ in 0..(STDOUT_LIMIT >> 10) + 64 {
            if out.write_all(block.as_bytes()).is_err() {
                return;
            }
        }
        std::thread::sleep(Duration::from_secs(60));
    }

    #[test]
    fn stdout_over_budget_is_an_error() {
        let exe = std::env::current_exe().unwrap();
        for mode in ["long", "lines"] {
            let args = [
                "providers::tests::fake_output",
                "--exact",
                "--ignored",
                "--nocapture",
                mode,
            ];
            let mut s = Session::spawn(&exe, &args, Duration::from_secs(20)).unwrap();
            let (mut lines, mut bytes) = (0, 0);
            let end = loop {
                match s.next_line() {
                    Ok(Some(l)) => (lines, bytes) = (lines + 1, bytes + l.len() as u64 + 1),
                    end => break end,
                }
            };
            // An error at the budget: not the deadline, not a clean end, nothing truncated.
            assert_eq!(end, Err("output exceeded 4 MiB".into()), "{mode}");
            assert!(bytes <= STDOUT_LIMIT, "{mode}: {bytes}");
            assert!(mode == "long" || lines > 1 << 20, "{mode}: {lines}");
        }
    }
}
