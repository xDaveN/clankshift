//! Claude adapter.
//!
//! There is no documented way to read Claude's usage window without sending a request, so the
//! anchor request doubles as the status check: `claude -p` with stream-json output emits a
//! `rate_limit_event` carrying the authoritative 5-hour `resetsAt`. The request is a one-line
//! Haiku prompt with tools, MCP servers, hooks, slash commands and session saving disabled
//! (~450 input tokens). ClankShift skips it entirely while a known window is still running.
//!
//! An API key, auth token, apiKeyHelper or cloud provider takes precedence over the
//! subscription and would make the request paid usage, so nothing is sent unless
//! `claude auth status` reports claude.ai sign-in, and user/project settings are not loaded.
//! `ANTHROPIC_CUSTOM_HEADERS` can carry an API key or bearer token that `auth status` does not
//! see, so it must be unset in the environment and in Claude Code's global config.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use super::{Outcome, Session, now, resolve};
use crate::config::ProviderConfig;
use crate::schedule::started_now;

/// Alias for the current cheapest Claude model; Claude Code resolves it, so it survives model
/// releases. `claude.model` in config.toml overrides it.
const DEFAULT_MODEL: &str = "haiku";

/// Settings sources for both Claude calls, so `auth status` reports the configuration the
/// request uses (user settings could relocate `CLAUDE_CONFIG_DIR` for one and not the other).
/// A root-level flag: it must come before `auth`.
const SETTING_SOURCES: [&str; 2] = ["--setting-sources", ""];

pub fn anchor(cfg: &ProviderConfig) -> Result<(Outcome, Option<i64>), String> {
    check_custom_headers()?;
    let program = resolve(&cfg.command, "claude")?;
    check_subscription(&program)?;
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
        SETTING_SOURCES[0],
        SETTING_SOURCES[1],
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

const HEADERS: &str = "ANTHROPIC_CUSTOM_HEADERS";

/// Claude Code also loads environment variables from the `env` object of its global config,
/// even with `--setting-sources ""`; `auth status` reflects keys and tokens set there, but not
/// custom headers. It uses the legacy `<config dir>/.config.json` if that exists, else
/// `<CLAUDE_CONFIG_DIR or home>/.claude.json` (Claude Code 2.1.286/287), so both are checked.
/// The legacy file is checked again at the directory `auth status` reports, see
/// `check_status`. OAuth overrides change the file name, so they are refused outright.
fn check_custom_headers() -> Result<(), String> {
    if std::env::var_os(HEADERS).is_some_and(|h| !h.is_empty()) {
        return Err(format!(
            "Claude: {HEADERS} is set in the environment and may replace the subscription sign-in; nothing sent"
        ));
    }
    for var in [
        "CLAUDE_CODE_CUSTOM_OAUTH_URL",
        "USE_LOCAL_OAUTH",
        "USE_STAGING_OAUTH",
    ] {
        if std::env::var_os(var).is_some() {
            return Err(format!(
                "Claude: {var} is set (unsupported sign-in); nothing sent"
            ));
        }
    }
    // Claude resolves a relative or empty value against its own working directory (and an
    // empty one differently per file), so only an absolute directory can be checked here.
    let config_dir = match std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from) {
        Some(d) if !d.is_absolute() => {
            return Err(format!(
                "Claude: CLAUDE_CONFIG_DIR must be an absolute path (is {:?}); nothing sent",
                d.display().to_string()
            ));
        }
        d => d,
    };
    let home = dirs::home_dir().ok_or("Claude: home folder not found")?;
    let files = [
        config_dir
            .clone()
            .unwrap_or_else(|| home.clone())
            .join(".claude.json"),
        config_dir
            .unwrap_or_else(|| home.join(".claude"))
            .join(".config.json"),
    ];
    files.iter().try_for_each(|f| check_config_file(f))
}

fn check_config_file(file: &Path) -> Result<(), String> {
    match std::fs::read_to_string(file) {
        Ok(text) if sets_custom_headers(&text) => Err(format!(
            "Claude: {HEADERS} is set in {} and may replace the subscription sign-in; nothing sent",
            file.display()
        )),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("Claude: could not read {}: {e}", file.display())),
    }
}

/// True if a global config sets custom headers, or cannot be read as JSON (unknown).
fn sets_custom_headers(config: &str) -> bool {
    serde_json::from_str::<Value>(config).map_or(true, |v| {
        let h = &v["env"][HEADERS];
        !(h.is_null() || h == "")
    })
}

fn check_subscription(program: &PathBuf) -> Result<(), String> {
    let mut s = Session::spawn(
        program,
        &[
            SETTING_SOURCES[0],
            SETTING_SOURCES[1],
            "auth",
            "status",
            "--json",
        ],
        Duration::from_secs(30),
    )?;
    s.close_stdin();
    let mut out = String::new();
    while let Some(line) = s.next_line()? {
        out.push_str(&line);
    }
    match serde_json::from_str(&out) {
        Ok(status) => check_status(&status),
        Err(_) => Err(format!(
            "Claude: could not read sign-in status {}",
            s.stderr_tail()
        )),
    }
}

/// Subscription sign-in, and no custom headers in the legacy config. Claude picks that file
/// from its config dir after Unicode NFC normalization, which `auth status` reports as
/// `configDirectory`; using it avoids re-deriving the normalization (a decomposed and a
/// composed spelling can be different directories).
fn check_status(status: &Value) -> Result<(), String> {
    subscription_auth(status)?;
    let dir = status["configDirectory"]
        .as_str()
        .map(Path::new)
        .filter(|d| d.is_absolute())
        .ok_or("Claude: sign-in status has no usable config directory; nothing sent")?;
    check_config_file(&dir.join(".config.json"))
}

/// Ok only for a claude.ai subscription sign-in used directly (not via Bedrock/Vertex/etc.).
fn subscription_auth(status: &Value) -> Result<(), String> {
    if status["loggedIn"] != true {
        return Err("Claude: not signed in (run claude auth login)".into());
    }
    if status["authMethod"] == "claude.ai" && status["apiProvider"] == "firstParty" {
        return Ok(());
    }
    Err(format!(
        "Claude: not using a Claude subscription (sign-in: {}, provider: {}); nothing sent",
        status["authMethod"].as_str().unwrap_or("unknown"),
        status["apiProvider"].as_str().unwrap_or("unknown"),
    ))
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

    /// Not a real test: runs `anchor` in a child of the test binary with the environment set up
    /// by `anchor_in_child`, and a CLI path that cannot start (unless CLANKSHIFT_TEST_CLI
    /// names a fake). Any spawn (auth status or the request) fails as "not found", so a
    /// refusal means nothing was started.
    #[test]
    #[ignore]
    fn anchor_child() {
        use std::io::IsTerminal;
        if std::io::stdin().is_terminal() {
            return;
        }
        let cfg = ProviderConfig {
            enabled: true,
            command: std::env::var("CLANKSHIFT_TEST_CLI").unwrap_or(r"Z:\nope\claude.exe".into()),
            model: String::new(),
        };
        println!("RESULT {:?}", anchor(&cfg));
    }

    /// `anchor` result in a child with a clean environment apart from `header`, inside a
    /// throwaway root holding `files` (path, content). The child runs in `root/guard` while
    /// the CLI would run in its temp dir `root/tmp`. `config_dir`: `None` = absolute
    /// `root/cfg`, else the literal value.
    fn anchor_in_child(
        header: Option<&str>,
        config_dir: Option<&str>,
        files: &[(&str, &str)],
    ) -> String {
        static RUN: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let run = RUN.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("clankshift-test-{}-{run}", std::process::id()));
        for dir in ["guard", "tmp", "cfg"] {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        for (path, content) in files {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, content).unwrap();
        }
        let config_dir = config_dir.map_or(root.join("cfg").into_os_string(), Into::into);
        let mut cmd = std::process::Command::new(std::env::current_exe().unwrap());
        cmd.args([
            "providers::claude::tests::anchor_child",
            "--exact",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .current_dir(root.join("guard"))
        .env("TMP", root.join("tmp"))
        .env("TEMP", root.join("tmp"))
        .env("CLAUDE_CONFIG_DIR", config_dir)
        .env_remove(HEADERS);
        if let Some(h) = header {
            cmd.env(HEADERS, h);
        }
        let out = cmd.output().unwrap();
        let _ = std::fs::remove_dir_all(&root);
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    #[test]
    fn custom_credential_headers_send_nothing() {
        let refused = "RESULT Err(\"Claude: ANTHROPIC_CUSTOM_HEADERS is set in";
        for header in ["x-api-key: sk-ant-fake", "Authorization: Bearer fake-token"] {
            let out = anchor_in_child(Some(header), None, &[("cfg/.claude.json", "{}")]);
            assert!(out.contains(refused), "{out}");
            // Clean environment; Claude would load the header from its global config.
            let global = format!(r#"{{"env":{{"{HEADERS}":"{header}"}}}}"#);
            for file in ["cfg/.claude.json", "cfg/.config.json"] {
                let out = anchor_in_child(None, None, &[(file, &global)]);
                assert!(out.contains(refused), "{out}");
            }
        }
        // Control: an ordinary config gets past the guard and tries to start the CLI.
        let ok = r#"{"env":{"DISABLE_TELEMETRY":"1"}}"#;
        let out = anchor_in_child(None, None, &[("cfg/.claude.json", ok)]);
        assert!(out.contains("not found"), "{out}");
    }

    #[test]
    fn config_dir_the_guard_cannot_see_sends_nothing() {
        // The CLI would load the header relative to its working directory (root/tmp); the
        // same relative path from the guard's directory (root/guard) holds a clean config.
        let bad = r#"{"env":{"ANTHROPIC_CUSTOM_HEADERS":"Authorization: Bearer fake-token"}}"#;
        for file in [".claude.json", ".config.json"] {
            let files = [
                (format!("guard/rel/{file}"), "{}"),
                (format!("tmp/rel/{file}"), bad),
            ];
            let files: Vec<_> = files.iter().map(|(p, c)| (p.as_str(), *c)).collect();
            let out = anchor_in_child(None, Some("rel"), &files);
            assert!(
                out.contains("CLAUDE_CONFIG_DIR must be an absolute path"),
                "{out}"
            );
        }
        // Empty value: the CLI reads `.config.json` in its working directory.
        let out = anchor_in_child(None, Some(""), &[("tmp/.config.json", bad)]);
        assert!(
            out.contains("CLAUDE_CONFIG_DIR must be an absolute path"),
            "{out}"
        );
    }

    /// User settings in the normalized legacy dir relocate CLAUDE_CONFIG_DIR to a clean dir.
    /// The fake CLI answers like Claude 2.1.286/287: `auth status` reports that clean dir
    /// unless `--setting-sources ""` comes first, as it does for the request, which then loads
    /// the header from the normalized legacy config. Nothing may reach the request.
    #[cfg(windows)]
    #[test]
    fn auth_status_uses_the_request_settings_sources() {
        let root = std::env::temp_dir().join(format!("clankshift-src-{}", std::process::id()));
        let (decomposed, composed, clean) = (
            root.join("cafe\u{301}"),
            root.join("caf\u{e9}"),
            root.join("clean"),
        );
        for d in [&decomposed, &composed, &clean] {
            std::fs::create_dir_all(d).unwrap();
            std::fs::write(d.join(".claude.json"), "{}").unwrap();
        }
        let bad = r#"{"env":{"ANTHROPIC_CUSTOM_HEADERS":"Authorization: Bearer fake-token"}}"#;
        std::fs::write(composed.join(".config.json"), bad).unwrap();
        let relocate = serde_json::json!({"env":{"CLAUDE_CONFIG_DIR":clean}});
        std::fs::write(composed.join("settings.json"), relocate.to_string()).unwrap();
        let status = |dir: &Path| {
            serde_json::json!({"loggedIn":true,"authMethod":"claude.ai",
                "apiProvider":"firstParty","configDirectory":dir})
            .to_string()
        };
        std::fs::write(root.join("same.json"), status(&composed)).unwrap();
        std::fs::write(root.join("relocated.json"), status(&clean)).unwrap();
        let fake = root.join("claude.cmd");
        std::fs::write(
            &fake,
            "@echo off\r\n\
             if \"%~3\"==\"auth\" (type \"%~dp0same.json\" & exit /b 0)\r\n\
             if \"%~1\"==\"auth\" (type \"%~dp0relocated.json\" & exit /b 0)\r\n\
             echo request>>\"%~dp0requests.txt\"\r\n",
        )
        .unwrap();
        let out = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "providers::claude::tests::anchor_child",
                "--exact",
                "--ignored",
                "--nocapture",
            ])
            .env("CLAUDE_CONFIG_DIR", &decomposed)
            .env("CLANKSHIFT_TEST_CLI", &fake)
            .env_remove(HEADERS)
            .output()
            .unwrap();
        let out = String::from_utf8_lossy(&out.stdout).into_owned();
        let requests = root.join("requests.txt").exists();
        let _ = std::fs::remove_dir_all(&root);
        assert!(!requests, "request started: {out}");
        assert!(out.contains("ANTHROPIC_CUSTOM_HEADERS is set in"), "{out}");
        assert!(out.contains(".config.json"), "{out}");
    }

    /// Windows only: macOS treats both spellings as one directory.
    #[cfg(windows)]
    #[test]
    fn legacy_config_is_checked_where_claude_loads_it() {
        // CLAUDE_CONFIG_DIR = decomposed "cafe\u{301}" (clean); Claude loads the legacy file from
        // the composed "caf\u{e9}" and reports that as configDirectory (2.1.286/287).
        let root = std::env::temp_dir().join(format!("clankshift-nfc-{}", std::process::id()));
        let (decomposed, composed) = (root.join("cafe\u{301}"), root.join("caf\u{e9}"));
        std::fs::create_dir_all(&decomposed).unwrap();
        std::fs::create_dir_all(&composed).unwrap();
        std::fs::write(decomposed.join(".claude.json"), "{}").unwrap();
        let bad = r#"{"env":{"ANTHROPIC_CUSTOM_HEADERS":"Authorization: Bearer fake-token"}}"#;
        std::fs::write(composed.join(".config.json"), bad).unwrap();
        let status = |dir: &Path| {
            serde_json::json!({"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty",
                "configDirectory":dir.to_str().unwrap()})
        };
        let distinct = !decomposed.join(".config.json").exists();
        let result = check_status(&status(&composed));
        let clean = check_status(&status(&decomposed));
        let missing = check_status(&serde_json::json!({"loggedIn":true,
            "authMethod":"claude.ai","apiProvider":"firstParty"}));
        let _ = std::fs::remove_dir_all(&root);
        assert!(distinct, "file system merged the two spellings");
        assert!(
            result
                .unwrap_err()
                .contains("ANTHROPIC_CUSTOM_HEADERS is set in")
        );
        assert!(clean.is_ok());
        assert!(missing.unwrap_err().contains("no usable config directory"));
    }

    #[test]
    fn only_subscription_sign_in_is_accepted() {
        // Shapes from Claude Code 2.1.286 `auth status --json` (trimmed).
        let ok = |s: &str| subscription_auth(&serde_json::from_str(s).unwrap());
        assert!(ok(r#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"firstParty","subscriptionType":"pro"}"#).is_ok());
        for paid in [
            r#"{"loggedIn":true,"authMethod":"api_key","apiProvider":"firstParty","apiKeySource":"ANTHROPIC_API_KEY"}"#,
            r#"{"loggedIn":true,"authMethod":"api_key_helper","apiProvider":"firstParty","apiKeySource":"apiKeyHelper"}"#,
            r#"{"loggedIn":true,"authMethod":"oauth_token","apiProvider":"firstParty"}"#,
            r#"{"loggedIn":true,"authMethod":"third_party","apiProvider":"bedrock"}"#,
            r#"{"loggedIn":true,"authMethod":"claude.ai","apiProvider":"vertex"}"#,
            r#"{"loggedIn":false,"authMethod":"none","apiProvider":"firstParty"}"#,
            r#"{}"#,
        ] {
            assert!(ok(paid).is_err(), "{paid}");
        }
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
