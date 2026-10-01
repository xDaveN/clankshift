# Architecture

ClankShift is one small Rust binary with two modes:

- **tray** (default): resident, event-driven, owns scheduling and provider calls.
- **settings** (`--settings`): a short-lived egui window that edits `config.toml` and exits.

```
src/
  main.rs         mode dispatch
  tray.rs         tray menu + event loop + triggers
  settings.rs     settings window
  icon.rs         app icon drawn in code (tray, settings, and the .exe via build.rs)
  config.rs       user config (TOML, %APPDATA%)
  state.rs        observed provider state (JSON, %LOCALAPPDATA%) + action log
  schedule.rs     pure timing decisions (tested)
  providers/      one module per provider; `anchor()` is the only operation
  platform/       everything OS-specific (hidden processes, login startup, single instance)
```

## Idle model

The tray process sleeps in the OS event loop (`winit`, `ControlFlow::WaitUntil`) until a menu
click, a finished provider call, the next daily trigger or retry, or a known window ending (to refresh the
menu). Waits are capped at 10 minutes because OS wait timers pause during system sleep; a
wake-up only compares timestamps. Measured on Windows 11 (release build): 0 ms CPU over
90 s idle, about 20 MB working set, one thread.

## Provider calls

Each call is a hidden child process (`CREATE_NO_WINDOW`) placed in a Windows job object that
kills the whole process tree when the call finishes or times out. No provider process outlives a
call.

**Codex.** `codex app-server` over stdio: `initialize`, then `account/rateLimits/read` (5-hour
and weekly windows with `resetsAt`) and `model/list`. With no window running, Codex reports a
placeholder: 0% used and `resetsAt` = read time + 5h, moving with each read (live-tested
2026-10-01; reading does not start a window), whereas a running window's reset stays fixed. An
earlier reset or any usage means a window is already running: nothing is sent. A reset that
looks like the placeholder (within a minute of clock skew) is read again 10 s later, status
only: the model is chosen from the first read, so nothing slow sits between this read and the
request. Only if the reset moved by the time between the two status replies (measured from
request to reply, within Codex's 1 s rounding) does one ephemeral `codex exec` with the cheapest
listed model at low effort follow, then limits are re-read. A fixed reset means a just-started window: nothing is sent. A
reply slower than 2 s, an expired reset, one more than 5h away, or any other change (such as a
window started during the check) fails without sending.
ClankShift still reads Codex status only when an anchor is due.

**Claude.** There is no documented quota-free status. `claude -p ... --output-format stream-json`
emits a `rate_limit_event` with the authoritative 5-hour `resetsAt`, so the anchor request is also
the status check. It is skipped while a previously reported window is still running.
Nothing is sent unless `claude auth status` reports a claude.ai sign-in used directly: an API key,
auth token, apiKeyHelper or Bedrock/Vertex setting would take precedence and bill paid usage. The
request skips user/project settings (`--setting-sources ""`). `ANTHROPIC_CUSTOM_HEADERS` can carry an
API key or bearer token that `auth status` does not show, so ClankShift refuses when it is set in its
environment or in the `env` object of Claude Code's global config, which Claude still loads: the
legacy `.config.json` in its config dir if present, else `.claude.json` in `CLAUDE_CONFIG_DIR` or the
home folder. Both are checked. Claude normalizes the config dir (Unicode NFC) before looking for the
legacy file, so that file is checked in the `configDirectory` reported by `auth status`, not a
re-derived path; `auth status` runs with the same `--setting-sources ""` as the request (placed
before `auth`), since user settings can relocate `CLAUDE_CONFIG_DIR`. Only that `env` entry is inspected. `CLAUDE_CONFIG_DIR` must be unset or absolute:
Claude resolves a relative or empty value against its own working directory, so those are refused.
Admin-managed policy is trusted.

**Window classification** (`schedule.rs`, descriptive only): after a request was sent, a reported
reset time within 10 minutes of `now + 5h` means the window started with this call; anything
earlier means it was already running. It never decides whether to send.

## Terminology

User-facing text says **5h limit** and "start"; code and these docs say **window** and **anchor**.
Keep UI wording free of "window"/"anchor".

## Triggers

- **Manual:** *Start … 5h limit* in the tray.
- **Start:** when ClankShift starts. With *Start with Windows* (per-user `Run` registry key, no
  admin rights), this is the login trigger.
- **Daily:** at a local time (DST-aware). If missed by more than an hour (asleep/off), it is skipped.

All triggers do nothing for a provider whose window is known to be running.

A failed automatic (start or daily) anchor is retried every 10 minutes, but only within an hour
of the trigger; later than that the reset would land somewhere the user did not ask for, so it
gives up. Retries are in memory only. Failed manual starts are not retried.

## What the tray shows

Only provider-reported facts: the current window's reset time and whether ClankShift started it,
or when the last known window ended. No usage percentages: they go stale immediately without
polling, and polling Claude would spend quota.

## Decisions

These are deliberate; change them only with a good reason.

1. **Native Rust, no webview.** `tray-icon` + `winit` for the tray; `eframe` (glow) only for
   settings. Keeps the idle footprint tiny and the toolchain single-language.
2. **Settings run in a separate process.** The tray never loads UI or GPU code (the settings
   window alone uses ~150 MB while open, mostly the graphics driver).
3. **Short-lived provider processes only.** No daemons, sessions or long-lived connections.
4. **Official CLIs, not private APIs.** ClankShift never reads provider credentials or calls
   undocumented HTTP endpoints; the CLIs handle authentication.
5. **Releases via release-plz + GitHub Releases.** Conventional Commits drive versions and the
   changelog; a single portable `clankshift.exe` is the only artifact; nothing is published to crates.io.
