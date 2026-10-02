//! Codex adapter.
//!
//! Status: `codex app-server` (stdio JSON-RPC) `account/rateLimits/read` returns the account's
//! windows with `resetsAt`, costs no quota and does not start a window (see `looks_inactive`).
//! ClankShift still only reads at anchor time, never casually.
//!
//! Anchor: unless two spaced reads show the moving "not running" placeholder, a window was
//! already running (or the state is unclear) and nothing is sent. Nor is anything sent unless both
//! reads report included usage allowed: otherwise the request could use paid credits. Otherwise
//! one ephemeral `codex exec` with the cheapest model at low effort anchors the window, then the
//! state is re-read (twice if needed) to confirm it started; if not, it is reported uncertain.

use std::time::Duration;

use serde_json::Value;

use super::{MAYBE_SENT, Outcome, Session, resolve};
use crate::config::ProviderConfig;
use crate::schedule::WINDOW_SECS;

/// Allowed difference between our clock and Codex's, plus its 1 s rounding.
pub const CLOCK_SKEW_SECS: i64 = 60;

/// Pause between the two reads that tell the moving placeholder from a fixed reset.
const RECHECK_SECS: u64 = 10;

/// Slowest rate-limit reply that still tells a moving placeholder from a window started during
/// the check (live replies: 0.5-1.2 s). Slower ones are unclear: nothing is sent.
const MAX_REPLY_MS: i64 = 2000;

/// The 5-hour window as read: (resetsAt, usedPercent).
type Limit = (i64, Option<i64>);

/// One rate-limit read: the 5-hour window, and when the request was sent and answered (Unix ms).
struct Snapshot {
    resets_at: i64,
    used: Option<i64>,
    /// `ordinaryUsageAllowed`: Codex says a request would use the plan's included usage.
    included: bool,
    sent_ms: i64,
    replied_ms: i64,
}

impl Snapshot {
    /// A rate-limit reply sent and answered at these times (Unix ms). Every read, including the
    /// one after a start, must report a reset Codex could really report, or it is not used.
    fn new(limits: &Value, sent_ms: i64, replied_ms: i64) -> Result<Snapshot, String> {
        let (resets_at, used) = parse_rate_limits(limits)?;
        let snapshot = Snapshot {
            resets_at,
            used,
            included: limits["ordinaryUsageAllowed"] == true,
            sent_ms,
            replied_ms,
        };
        check_reset(resets_at, snapshot.seconds().1)?;
        Ok(snapshot)
    }

    /// When the read was sent and answered, rounded outwards to whole seconds.
    fn seconds(&self) -> (i64, i64) {
        (self.sent_ms / 1000, (self.replied_ms + 999) / 1000)
    }

    /// Could this read be the "not running" placeholder? Live-tested 2026-10-01 (codex-cli
    /// 0.159.3, no Codex use for the window's lifetime): two reads 20 min apart both reported
    /// usedPercent 0 and resetsAt = read time + 5h, i.e. the placeholder moves with the clock and
    /// reading does not start a window. A running window resets earlier than that; anything else
    /// fails `new`. A window started within the clock-skew allowance also passes; `moved` settles it.
    fn looks_inactive(&self) -> bool {
        let (before, _) = self.seconds();
        self.used == Some(0) && self.resets_at >= before + WINDOW_SECS - CLOCK_SKEW_SECS
    }

    /// Does this read alone show a running 5h limit: usage, or a reset earlier than the
    /// placeholder's? Not simply `!looks_inactive`, which also refuses unknown or malformed usage.
    fn running(&self) -> bool {
        let (before, _) = self.seconds();
        let earlier = self.resets_at < before + WINDOW_SECS - CLOCK_SKEW_SECS;
        earlier || self.used.is_some_and(|u| u > 0)
    }

    /// Only `true` counts: false (e.g. weekly limit reached) or unknown could mean paid credits.
    fn check_included(&self) -> Result<(), String> {
        if self.included {
            return Ok(());
        }
        Err("Codex does not report included plan usage available; nothing sent".into())
    }
}

/// Cheapest-first. Any request anchors the window, so the cheapest model available wins.
/// When OpenAI renames models, update this list; `codex.model` in config.toml overrides it,
/// and if none are listed by `model/list` the account's default model is used at low effort.
const CHEAP_MODELS: &[&str] = &["gpt-6-luna", "gpt-5.6-luna"];

pub fn anchor(cfg: &ProviderConfig) -> Result<(Outcome, i64), String> {
    let program = resolve(&cfg.command, "codex")?;
    let pick = cfg.model.trim().is_empty();
    let (first, models) = read(&program, pick)?;
    if !first.looks_inactive() {
        return Ok((Outcome::AlreadyActive, first.resets_at));
    }
    first.check_included()?;
    // Chosen before the confirming read: nothing slow may sit between it and the request.
    let model = if pick {
        pick_model(&models)?
    } else {
        cfg.model.trim().to_string()
    };
    // The placeholder, or a window started within the last minute or so: only the placeholder moves.
    std::thread::sleep(Duration::from_secs(RECHECK_SECS));
    let (second, _) = read(&program, false)?;
    if !second.looks_inactive() || !moved(&first, &second)? {
        return Ok((Outcome::AlreadyActive, second.resets_at));
    }
    second.check_included()?;
    exec(&program, &model)?;
    let resets_at = read(&program, false).and_then(|(after, _)| {
        confirm(&after, || {
            std::thread::sleep(Duration::from_secs(RECHECK_SECS));
            read(&program, false).map(|(again, _)| again)
        })
    });
    // The request was sent: retrying could start another 5h limit or spend more.
    let resets_at = resets_at.map_err(|e| format!("{e} {MAYBE_SENT}"))?;
    Ok((Outcome::Anchored, resets_at))
}

/// Reset of the 5h limit a completed start began, from the read `after` it. A just-started
/// limit looks like the placeholder in one read, so unless it is `running` it is read `again`:
/// only a fixed reset confirms the start. Still moving (or unclear): no reset is reported.
fn confirm(
    after: &Snapshot,
    again: impl FnOnce() -> Result<Snapshot, String>,
) -> Result<i64, String> {
    if after.running() {
        return Ok(after.resets_at);
    }
    let again = again()?;
    if again.running() || moved(after, &again) == Ok(false) {
        return Ok(again.resets_at);
    }
    Err("Codex did not confirm that the 5h limit started".into())
}

/// One short app-server session: rate limits, then the model list if `with_models` (else `Null`).
/// The rate-limit request is sent only after initialization and alone, so its timing covers just
/// Codex's answer, and without models the read returns the moment that answer arrives.
fn read(program: &std::path::PathBuf, with_models: bool) -> Result<(Snapshot, Value), String> {
    let mut s = Session::spawn(program, &["app-server"], Duration::from_secs(45))?;
    read_from(&mut s, with_models)
}

fn read_from(s: &mut Session, with_models: bool) -> Result<(Snapshot, Value), String> {
    s.send(r#"{"id":1,"method":"initialize","params":{"clientInfo":{"name":"clankshift","title":"ClankShift","version":"0.1.0"}}}"#)?;
    let (mut limits, mut models) = (None, (!with_models).then_some(Value::Null));
    let (mut sent_ms, mut replied_ms) = (0, 0);
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
        let id = msg["id"].as_i64();
        if let Some(err) = msg.get("error").filter(|_| id.is_some()) {
            return Err(format!(
                "codex app-server: {}",
                err["message"].as_str().unwrap_or("request failed")
            ));
        }
        match id {
            Some(1) => {
                s.send(r#"{"method":"initialized"}"#)?;
                s.send(r#"{"id":2,"method":"account/rateLimits/read"}"#)?;
                sent_ms = now_ms();
            }
            Some(2) => {
                replied_ms = now_ms();
                limits = Some(msg["result"].clone());
                if with_models {
                    s.send(r#"{"id":3,"method":"model/list","params":{}}"#)?;
                }
            }
            Some(3) => models = Some(msg["result"].clone()),
            _ => {}
        }
    }
    let snapshot = Snapshot::new(&limits.unwrap(), sent_ms, replied_ms)?;
    Ok((snapshot, models.unwrap()))
}

fn now_ms() -> i64 {
    jiff::Timestamp::now().as_millisecond()
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

/// Reset time and used percent of the 5-hour window: whichever slot lasts exactly 5 hours
/// (weekly-only accounts have none). Even the "not running" placeholder has a reset time, so a
/// missing or malformed one is a reply we don't understand: stop rather than send.
fn parse_rate_limits(result: &Value) -> Result<Limit, String> {
    let rl = &result["rateLimits"];
    let w = ["primary", "secondary"]
        .iter()
        .map(|slot| &rl[slot])
        .find(|w| w["windowDurationMins"].as_i64() == Some(WINDOW_SECS / 60))
        .ok_or("Codex reports no 5h limit for this account")?;
    let reset = w["resetsAt"].as_i64().ok_or_else(|| {
        format!(
            "Codex reported an unexpected 5h limit reset ({})",
            w["resetsAt"]
        )
    })?;
    Ok((reset, w["usedPercent"].as_i64()))
}

/// A reset already past or beyond 5h away (allowing for clock skew) after a read answered at
/// `after` is not something Codex reports: stop.
fn check_reset(resets_at: i64, after: i64) -> Result<(), String> {
    if resets_at <= after || resets_at > after + WINDOW_SECS + CLOCK_SKEW_SECS {
        return Err(format!(
            "Codex reported an unexpected 5h limit reset ({resets_at})"
        ));
    }
    Ok(())
}

/// Did the reset move between two reads like the placeholder, which is reply time + 5h rounded to
/// whole seconds, rather than stay fixed like a running window (live: 1790880307 at 15:45 and
/// again at 15:48)? The move must fit the measured request-to-reply times within Codex's 1 s
/// rounding, so a window started during the check is accepted only if it started within the
/// first reply time + 2 s before the second request: about the same as the unavoidable gap
/// between that request and sending the start prompt. Anything else is unclear: nothing is sent.
fn moved(first: &Snapshot, second: &Snapshot) -> Result<bool, String> {
    let (r1, r2) = (first.resets_at, second.resets_at);
    if (r2 - r1).abs() <= 1 {
        return Ok(false);
    }
    let slow = |s: &Snapshot| s.replied_ms - s.sent_ms > MAX_REPLY_MS;
    if slow(first) || slow(second) {
        return Err("Codex answered too slowly to check its 5h limit; nothing sent".into());
    }
    let earliest = second.sent_ms - first.replied_ms - 1000;
    let latest = second.replied_ms - first.sent_ms + 1000;
    if (earliest..=latest).contains(&((r2 - r1) * 1000)) {
        Ok(true)
    } else {
        Err(format!(
            "Codex 5h limit reset changed during the check ({r1} -> {r2}); nothing sent"
        ))
    }
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
        assert_eq!(parse_rate_limits(&r).unwrap(), (1790801242, Some(0)));
    }

    #[test]
    fn only_a_placeholder_candidate_looks_inactive() {
        let looks_inactive = |reset, used: Option<i64>, before, after| {
            let limits = json!({"rateLimits":{"primary":{"usedPercent":used,"windowDurationMins":300,"resetsAt":reset}}});
            Snapshot::new(&limits, before * 1000, after * 1000).map(|s| s.looks_inactive())
        };
        // Live placeholder (2026-10-01): read sent at 1790860348 reported resetsAt 1790878349, 0% used.
        let (before, after) = (1_790_860_346, 1_790_860_350);
        assert_eq!(
            looks_inactive(1_790_878_349, Some(0), before, after),
            Ok(true)
        );
        // Started 5 minutes ago: running, even with 0% used.
        assert_eq!(
            looks_inactive(before + WINDOW_SECS - 300, Some(0), before, after),
            Ok(false)
        );
        // Fresh-looking but already used: running.
        assert_eq!(
            looks_inactive(before + WINDOW_SECS, Some(1), before, after),
            Ok(false)
        );
        assert_eq!(
            looks_inactive(before + WINDOW_SECS, None, before, after),
            Ok(false)
        );
        // Expired or too far away: not a state Codex reports; stop rather than send.
        assert!(looks_inactive(after - 60, Some(0), before, after).is_err());
        assert!(looks_inactive(after + 2 * WINDOW_SECS, Some(0), before, after).is_err());
    }

    /// A read sent at `sent` (Unix s) answered `reply_ms` later, reporting `reset` at 0% used.
    fn snap(reset: i64, sent: i64, reply_ms: i64) -> Snapshot {
        Snapshot {
            resets_at: reset,
            used: Some(0),
            included: true,
            sent_ms: sent * 1000,
            replied_ms: sent * 1000 + reply_ms,
        }
    }

    /// Codex's placeholder for a reply at `at_ms`: reply time + 5h, rounded either way.
    fn placeholder(at_ms: i64, round_up: bool) -> i64 {
        (at_ms + if round_up { 999 } else { 0 }) / 1000 + WINDOW_SECS
    }

    const T: i64 = 1_000_000;

    #[test]
    fn recently_started_limit_is_not_started_again() {
        // Started 30 s before the check with 0% used: passes the first look...
        let reset = T + WINDOW_SECS - 30;
        let (first, second) = (snap(reset, T, 600), snap(reset, T + 11, 600));
        assert!(first.looks_inactive());
        // ...but its reset is fixed across the recheck, so it is running.
        assert_eq!(moved(&first, &second), Ok(false));
        assert_eq!(moved(&first, &snap(reset + 1, T + 11, 600)), Ok(false)); // rounding
        // Fixed stays "running" even when replies are slow.
        assert_eq!(
            moved(&snap(reset, T, 40_000), &snap(reset, T + 50, 40_000)),
            Ok(false)
        );
    }

    #[test]
    fn usage_starting_between_reads_is_not_mistaken_for_the_placeholder() {
        // Fast replies (0.6 s): idle at the first read, another client starts a window 5 s (or
        // 3 s) before the second request; its reset is then fixed at that start + 5h.
        let first = snap(placeholder(T * 1000 + 600, true), T, 600);
        for before_second in [5, 3] {
            let start = T + 11 - before_second;
            let second = snap(start + WINDOW_SECS, T + 11, 600);
            assert!(moved(&first, &second).is_err(), "{before_second}");
        }
        // Slow replies (40 s, the review's case): unclear, nothing sent.
        let first = snap(T + WINDOW_SECS + 1, T, 40_000);
        let second = snap(T + 20 + WINDOW_SECS, T + 50, 40_000);
        assert!(moved(&first, &second).is_err());
        // Just over the reply limit, even with a perfectly moving reset: unclear.
        let first = snap(placeholder(T * 1000, false), T, MAX_REPLY_MS + 1);
        let second = snap(placeholder((T + 12) * 1000, false), T + 12, 600);
        assert!(moved(&first, &second).is_err());
    }

    #[test]
    fn moving_placeholder_is_inactive() {
        // Whenever Codex answered within each reply time and however it rounded: moved.
        for reply_ms in [300, 1200, MAX_REPLY_MS] {
            let second_sent = T + 10 + reply_ms / 1000 + 1;
            for (a, b) in [(0, 0), (reply_ms, reply_ms), (0, reply_ms), (reply_ms, 0)] {
                for round_up in [false, true] {
                    let first = snap(placeholder(T * 1000 + a, round_up), T, reply_ms);
                    let r2 = placeholder(second_sent * 1000 + b, round_up);
                    let second = snap(r2, second_sent, reply_ms);
                    assert!(second.looks_inactive());
                    assert_eq!(moved(&first, &second), Ok(true), "{reply_ms} {a} {b}");
                }
            }
        }
        // Live placeholder reads 2026-10-01, sent at 1790860348 and 1790861549.
        let live = moved(
            &snap(1_790_878_349, 1_790_860_348, 700),
            &snap(1_790_879_550, 1_790_861_549, 700),
        );
        assert_eq!(live, Ok(true));
        // Backwards or too far: unclear, nothing sent.
        assert!(moved(&snap(T, T, 600), &snap(T - 10, T + 11, 600)).is_err());
        assert!(moved(&snap(T, T, 600), &snap(T + 100, T + 11, 600)).is_err());
    }

    /// Seconds the fake app-server holds back its `model/list` reply.
    const MODEL_DELAY_MS: i64 = 3000;

    /// Not a real test: a fake `codex app-server` that `read_from` talks to, run as a child of the
    /// test binary. Status (a moving placeholder) replies at once; `model/list` only after a delay.
    #[test]
    #[ignore]
    fn fake_app_server() {
        use std::io::IsTerminal;
        if std::io::stdin().is_terminal() {
            return;
        }
        println!(); // end libtest's "test ... " line so replies start on their own lines
        for line in std::io::stdin().lines().map_while(Result::ok) {
            let msg: Value = serde_json::from_str(&line).unwrap();
            let reply = match msg["id"].as_i64() {
                Some(1) => json!({"id":1,"result":{}}),
                Some(2) => {
                    let reset = now_ms() / 1000 + WINDOW_SECS + 1;
                    json!({"id":2,"result":{"rateLimits":{"primary":{"usedPercent":0,"windowDurationMins":300,"resetsAt":reset}}}})
                }
                Some(3) => {
                    std::thread::sleep(Duration::from_millis(MODEL_DELAY_MS as u64));
                    json!({"id":3,"result":{"data":[{"id":"gpt-6-luna"}]}})
                }
                _ => continue,
            };
            println!("{reply}");
        }
    }

    fn fake_session() -> Session {
        let args = [
            "providers::codex::tests::fake_app_server",
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ];
        Session::spawn(
            &std::env::current_exe().unwrap(),
            &args,
            Duration::from_secs(30),
        )
        .unwrap()
    }

    #[test]
    fn confirming_read_does_not_wait_for_models() {
        // A slow model list after the status reply leaves time for another client to start a
        // window, so a read that waits for it returns a stale status.
        let (snap, models) = read_from(&mut fake_session(), true).unwrap();
        assert_eq!(pick_model(&models).unwrap(), "gpt-6-luna");
        assert!(now_ms() - snap.replied_ms >= MODEL_DELAY_MS);
        // The confirming read skips the model list: its status is fresh the moment it returns.
        let (snap, models) = read_from(&mut fake_session(), false).unwrap();
        assert!(snap.looks_inactive());
        assert_eq!(models, Value::Null);
        assert!(now_ms() - snap.replied_ms < 1000);
    }

    #[test]
    fn finds_short_window_by_duration_not_slot() {
        let weekly_only = json!({"rateLimits":{"primary":{"usedPercent":5,"windowDurationMins":10080,"resetsAt":9},"secondary":null}});
        assert!(parse_rate_limits(&weekly_only).is_err());
        let swapped = json!({"rateLimits":{"primary":{"windowDurationMins":10080,"resetsAt":9},"secondary":{"windowDurationMins":300,"resetsAt":7}}});
        assert_eq!(parse_rate_limits(&swapped).unwrap().0, 7);
    }

    #[test]
    fn unrecognized_5h_limit_stops_the_start() {
        // Other buckets are not the 5h limit, however short.
        for mins in [15, 0, -300, 360] {
            let other = json!({"rateLimits":{"primary":{"usedPercent":0,"windowDurationMins":mins,"resetsAt":9}}});
            assert!(parse_rate_limits(&other).is_err(), "{mins}");
        }
        // Missing, null or mistyped reset: not the placeholder (which has one), so nothing is sent.
        for reset in [json!(null), json!("1790880307"), json!(1790880307.5)] {
            let r = json!({"rateLimits":{"primary":{"usedPercent":0,"windowDurationMins":300,"resetsAt":reset}}});
            assert!(parse_rate_limits(&r).is_err(), "{reset}");
        }
        let no_reset = json!({"rateLimits":{"primary":{"usedPercent":0,"windowDurationMins":300}}});
        assert!(parse_rate_limits(&no_reset).is_err());
    }

    #[test]
    fn invalid_reset_after_a_start_is_not_reported_as_started() {
        let reply = |reset: i64| json!({"rateLimits":{"primary":{"usedPercent":0,"windowDurationMins":300,"resetsAt":reset}}});
        // Two valid moving placeholder reads allow the start...
        let first = Snapshot::new(&reply(T + WINDOW_SECS + 1), T * 1000, T * 1000 + 600).unwrap();
        let s2 = T + 11;
        let second =
            Snapshot::new(&reply(s2 + WINDOW_SECS + 1), s2 * 1000, s2 * 1000 + 600).unwrap();
        assert_eq!(moved(&first, &second), Ok(true));
        // ...then the read after it must still report a possible reset.
        let s3 = T + 20;
        let read_after = |reset| Snapshot::new(&reply(reset), s3 * 1000, s3 * 1000 + 600);
        for bad in [-1, 0, s3 - 60, s3 + 2 * WINDOW_SECS, i64::MIN, i64::MAX] {
            assert!(read_after(bad).is_err(), "{bad}");
        }
        // A real just-started 5h limit, and one started a while ago, still read fine.
        assert_eq!(
            read_after(s3 + WINDOW_SECS).unwrap().resets_at,
            s3 + WINDOW_SECS
        );
        assert!(read_after(s3 + 60).is_ok());
    }

    #[test]
    fn start_is_confirmed_only_by_a_running_limit() {
        // Read after the start (sent at T + 30), and again 11 s later.
        let (s3, s4) = (T + 30, T + 41);
        let unused = || -> Result<Snapshot, String> { panic!("no second read needed") };
        // Already used, or reset fixed earlier: running, no second read.
        let used = Snapshot {
            used: Some(1),
            ..snap(s3 + WINDOW_SECS, s3, 600)
        };
        assert_eq!(confirm(&used, unused), Ok(s3 + WINDOW_SECS));
        assert_eq!(confirm(&snap(s3 + 60, s3, 600), unused), Ok(s3 + 60));
        // Just started (fresh-looking, 0%): its reset stays fixed across the second read.
        let started = s3 - 5 + WINDOW_SECS;
        let after = snap(started, s3, 600);
        assert!(after.looks_inactive());
        assert_eq!(confirm(&after, || Ok(snap(started, s4, 600))), Ok(started));
        // Still the moving placeholder: not reported as started.
        let after = snap(placeholder(s3 * 1000 + 600, true), s3, 600);
        let again = || Ok(snap(placeholder(s4 * 1000 + 600, true), s4, 600));
        assert!(confirm(&after, again).is_err());
        // Unclear or failed second read: not started either.
        assert!(confirm(&after, || Ok(snap(s4 + WINDOW_SECS + 30, s4, 600))).is_err());
        assert!(confirm(&after, || Err("exited early".into())).is_err());
        // Unknown or malformed usage on a moving reset proves nothing, on either read.
        for used in [None, Some(-1)] {
            let odd = |s: Snapshot| Snapshot { used, ..s };
            let odd_after = odd(snap(placeholder(s3 * 1000 + 600, true), s3, 600));
            assert!(confirm(&odd_after, again).is_err(), "{used:?}");
            let odd_again = || Ok(odd(snap(placeholder(s4 * 1000 + 600, true), s4, 600)));
            assert!(confirm(&after, odd_again).is_err(), "{used:?}");
        }
    }

    #[test]
    fn only_explicitly_included_usage_allows_a_start() {
        // Included usage unavailable (e.g. weekly limit reached) reads `false`: a start could use credits.
        let read = |allowed: Option<Value>| {
            let mut r = json!({"rateLimits":{"primary":{"usedPercent":0,"windowDurationMins":300,"resetsAt":T + WINDOW_SECS}}});
            if let Some(a) = allowed {
                r["ordinaryUsageAllowed"] = a;
            }
            Snapshot::new(&r, T * 1000, T * 1000 + 600).unwrap()
        };
        assert_eq!(read(Some(json!(true))).check_included(), Ok(()));
        for allowed in [
            Some(json!(false)),
            Some(json!(null)),
            Some(json!("true")),
            None,
        ] {
            assert!(
                read(allowed.clone()).check_included().is_err(),
                "{allowed:?}"
            );
        }
    }

    #[test]
    fn picks_cheapest_listed_model() {
        let list = json!({"data":[{"id":"gpt-6.1-sol","isDefault":true},{"id":"gpt-5.6-luna"},{"id":"gpt-6-luna"}]});
        assert_eq!(pick_model(&list).unwrap(), "gpt-6-luna");
        let unknown = json!({"data":[{"id":"gpt-9-x"},{"id":"gpt-9-y","isDefault":true}]});
        assert_eq!(pick_model(&unknown).unwrap(), "gpt-9-y");
    }
}
