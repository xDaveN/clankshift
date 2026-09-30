//! Codex adapter.
//!
//! Status: `codex app-server` (stdio JSON-RPC) `account/rateLimits/read` returns the account's
//! windows with `resetsAt` and costs no quota. Evidence so far suggests the read itself may start
//! the 5-hour window, so ClankShift only reads at anchor time, never casually.
//!
//! Anchor: if the 5-hour window reported by the read did not just start, a window was already
//! running and nothing is sent. Otherwise one ephemeral `codex exec` with the cheapest model at
//! low effort makes sure the window is anchored, then the state is re-read.

use std::time::Duration;

use serde_json::Value;

use super::{Outcome, Session, now, resolve};
use crate::config::ProviderConfig;
use crate::schedule::started_now;

/// Cheapest-first. Any request anchors the window, so the cheapest model available wins.
/// When OpenAI renames models, update this list; `codex.model` in config.toml overrides it,
/// and if none are listed by `model/list` the account's default model is used at low effort.
const CHEAP_MODELS: &[&str] = &["gpt-6-luna", "gpt-5.6-luna"];

pub fn anchor(cfg: &ProviderConfig) -> Result<(Outcome, Option<i64>), String> {
    let program = resolve(&cfg.command, "codex")?;
    let started = now();
    let (resets_at, models) = read(&program)?;
    if resets_at.is_some_and(|r| !started_now(r, started)) {
        return Ok((Outcome::AlreadyActive, resets_at));
    }
    let model = if cfg.model.trim().is_empty() {
        pick_model(&models)?
    } else {
        cfg.model.trim().to_string()
    };
    exec(&program, &model)?;
    let (resets_at, _) = read(&program)?;
    if resets_at.is_none() {
        return Err("Codex did not report a window after the anchor request".into());
    }
    Ok((Outcome::Anchored, resets_at))
}

/// One short app-server session: rate limits + model list.
fn read(program: &std::path::PathBuf) -> Result<(Option<i64>, Value), String> {
    let mut s = Session::spawn(program, &["app-server"], Duration::from_secs(45))?;
    s.send(r#"{"id":1,"method":"initialize","params":{"clientInfo":{"name":"clankshift","title":"ClankShift","version":"0.1.0"}}}"#)?;
    s.send(r#"{"method":"initialized"}"#)?;
    s.send(r#"{"id":2,"method":"account/rateLimits/read"}"#)?;
    s.send(r#"{"id":3,"method":"model/list","params":{}}"#)?;
    let (mut limits, mut models) = (None, None);
    while limits.is_none() || models.is_none() {
        let Some(line) = s.next_line()? else {
            return Err(format!(
                "codex app-server exited early: {}",
                s.stderr_tail()
            ));
        };
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let slot = match msg["id"].as_i64() {
            Some(2) => &mut limits,
            Some(3) => &mut models,
            _ => continue,
        };
        if let Some(err) = msg.get("error") {
            return Err(format!(
                "codex app-server: {}",
                err["message"].as_str().unwrap_or("request failed")
            ));
        }
        *slot = Some(msg["result"].clone());
    }
    Ok((parse_rate_limits(&limits.unwrap())?, models.unwrap()))
}

fn exec(program: &std::path::PathBuf, model: &str) -> Result<(), String> {
    let args = [
        "exec",
        "--ephemeral",
        "--skip-git-repo-check",
        "--ignore-user-config",
        "--ignore-rules",
        "--sandbox",
        "read-only",
        "--json",
        "-m",
        model,
        "-c",
        "model_reasoning_effort=low",
        "Reply with OK.",
    ];
    let mut s = Session::spawn(program, &args, Duration::from_secs(180))?;
    s.close_stdin();
    let mut failure = None;
    while let Some(line) = s.next_line()? {
        let Ok(ev) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match ev["type"].as_str() {
            Some("turn.completed") => return Ok(()),
            Some("turn.failed") => failure = ev["error"]["message"].as_str().map(String::from),
            Some("error") => failure = ev["message"].as_str().map(String::from),
            _ => {}
        }
    }
    Err(format!(
        "codex exec failed: {}",
        failure.unwrap_or_else(|| s.stderr_tail())
    ))
}

/// Reset time of the 5-hour window: whichever slot has a short duration (weekly-only accounts have none).
/// Ok(None) = the window exists but reports no reset time.
fn parse_rate_limits(result: &Value) -> Result<Option<i64>, String> {
    let rl = &result["rateLimits"];
    ["primary", "secondary"]
        .iter()
        .map(|slot| &rl[slot])
        .find(|w| w["windowDurationMins"].as_i64().is_some_and(|m| m <= 360))
        .map(|w| w["resetsAt"].as_i64())
        .ok_or_else(|| "Codex reports no 5-hour window for this account".into())
}

fn pick_model(list: &Value) -> Result<String, String> {
    let models = list["data"]
        .as_array()
        .ok_or("unexpected model/list response")?;
    let listed = |id: &str| models.iter().any(|m| m["id"] == id);
    CHEAP_MODELS
        .iter()
        .find(|id| listed(id))
        .map(|id| id.to_string())
        .or_else(|| {
            models
                .iter()
                .find(|m| m["isDefault"] == true)
                .and_then(|m| m["id"].as_str())
                .map(String::from)
        })
        .ok_or_else(|| "no usable Codex model found; set codex.model in config.toml".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_real_rate_limit_response() {
        // Captured from codex-cli 0.159.1 (trimmed).
        let r: Value = serde_json::from_str(r#"{"ordinaryUsageAllowed":true,"rateLimits":{"limitId":"codex","primary":{"usedPercent":0,"windowDurationMins":300,"resetsAt":1790801242},"secondary":{"usedPercent":20,"windowDurationMins":10080,"resetsAt":1791197709},"planType":"plus"}}"#).unwrap();
        assert_eq!(parse_rate_limits(&r).unwrap(), Some(1790801242));
    }

    #[test]
    fn finds_short_window_by_duration_not_slot() {
        let weekly_only = json!({"rateLimits":{"primary":{"usedPercent":5,"windowDurationMins":10080,"resetsAt":9},"secondary":null}});
        assert!(parse_rate_limits(&weekly_only).is_err());
        let swapped = json!({"rateLimits":{"primary":{"windowDurationMins":10080,"resetsAt":9},"secondary":{"windowDurationMins":300,"resetsAt":7}}});
        assert_eq!(parse_rate_limits(&swapped).unwrap(), Some(7));
        let no_reset = json!({"rateLimits":{"primary":{"usedPercent":0,"windowDurationMins":300}}});
        assert_eq!(parse_rate_limits(&no_reset).unwrap(), None);
    }

    #[test]
    fn picks_cheapest_listed_model() {
        let list = json!({"data":[{"id":"gpt-6.1-sol","isDefault":true},{"id":"gpt-5.6-luna"},{"id":"gpt-6-luna"}]});
        assert_eq!(pick_model(&list).unwrap(), "gpt-6-luna");
        let unknown = json!({"data":[{"id":"gpt-9-x"},{"id":"gpt-9-y","isDefault":true}]});
        assert_eq!(pick_model(&unknown).unwrap(), "gpt-9-y");
    }
}
