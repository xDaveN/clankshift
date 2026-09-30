//! Claude adapter.
//!
//! There is no documented way to read Claude's usage window without sending a request, so the
//! anchor request doubles as the status check: `claude -p` with stream-json output emits a
//! `rate_limit_event` carrying the authoritative 5-hour `resetsAt`. The request is a one-line
//! Haiku prompt with tools, MCP servers, hooks, slash commands and session saving disabled
//! (~450 input tokens). ClankShift skips it entirely while a known window is still running.

use std::time::Duration;

use serde_json::Value;

use super::{Outcome, Session, now, resolve};
use crate::config::ProviderConfig;
use crate::schedule::started_now;

/// Alias for the current cheapest Claude model; Claude Code resolves it, so it survives model
/// releases. `claude.model` in config.toml overrides it.
const DEFAULT_MODEL: &str = "haiku";

pub fn anchor(cfg: &ProviderConfig) -> Result<(Outcome, Option<i64>), String> {
    let program = resolve(&cfg.command, "claude")?;
    let model = if cfg.model.trim().is_empty() {
        DEFAULT_MODEL
    } else {
        cfg.model.trim()
    };
    let args = [
        "-p",
        "hi",
        "--model",
        model,
        "--system-prompt",
        "Reply with OK.",
        "--tools",
        "",
        "--strict-mcp-config",
        "--disable-slash-commands",
        "--no-session-persistence",
        "--settings",
        r#"{"disableAllHooks":true}"#,
        "--output-format",
        "stream-json",
        "--verbose",
    ];
    let started = now();
    let mut s = Session::spawn(&program, &args, Duration::from_secs(120))?;
    s.close_stdin();
    let mut lines = Vec::new();
    while let Some(line) = s.next_line()? {
        lines.push(line);
    }
    let resets_at =
        parse_stream(&lines).map_err(|e| if e.is_empty() { s.stderr_tail() } else { e })?;
    let outcome = match resets_at {
        Some(r) if started_now(r, started) => Outcome::Anchored,
        _ => Outcome::AlreadyActive,
    };
    Ok((outcome, resets_at))
}

/// Extract the 5-hour window reset time from stream-json output. Empty error = no usable output at all.
fn parse_stream(lines: &[String]) -> Result<Option<i64>, String> {
    let events: Vec<Value> = lines
        .iter()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let result = events.iter().find(|e| e["type"] == "result");
    if let Some(r) = result.filter(|r| r["is_error"] == true) {
        let msg = r["result"]
            .as_str()
            .or(r["api_error_status"].as_str())
            .unwrap_or("request failed");
        return Err(format!("Claude: {msg}"));
    }
    for e in events.iter().filter(|e| e["type"] == "rate_limit_event") {
        let info = &e["rate_limit_info"];
        let five = &info["unifiedWindows"]["five_hour"];
        if five.is_object() {
            return Ok(five["resetsAt"].as_i64());
        }
        if info["rateLimitType"] == "five_hour" {
            return Ok(info["resetsAt"].as_i64());
        }
    }
    match result {
        Some(_) => Err("Claude answered but did not report its 5h limit".into()),
        None => Err(String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(s: &[&str]) -> Vec<String> {
        s.iter().map(|l| l.to_string()).collect()
    }

    #[test]
    fn parses_real_rate_limit_event() {
        // Captured from Claude Code 2.1.285 (trimmed).
        let out = lines(&[
            r#"{"type":"system","subtype":"init"}"#,
            r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed","resetsAt":1790793600,"rateLimitType":"five_hour","unifiedWindows":{"five_hour":{"utilization":0.11,"resetsAt":1790793600},"seven_day":{"utilization":0.08,"resetsAt":1791010800}}}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"result":"OK"}"#,
        ]);
        assert_eq!(parse_stream(&out).unwrap(), Some(1790793600));
    }

    #[test]
    fn falls_back_to_top_level_five_hour_fields() {
        let out = lines(&[
            r#"{"type":"rate_limit_event","rate_limit_info":{"resetsAt":42,"rateLimitType":"five_hour"}}"#,
        ]);
        assert_eq!(parse_stream(&out).unwrap(), Some(42));
    }

    #[test]
    fn reports_errors() {
        let out = lines(&[r#"{"type":"result","is_error":true,"result":"Not logged in"}"#]);
        assert_eq!(parse_stream(&out).unwrap_err(), "Claude: Not logged in");
        assert_eq!(parse_stream(&lines(&["garbage"])).unwrap_err(), "");
        let no_event = lines(&[r#"{"type":"result","is_error":false,"result":"OK"}"#]);
        assert!(
            parse_stream(&no_event)
                .unwrap_err()
                .contains("did not report")
        );
    }
}
