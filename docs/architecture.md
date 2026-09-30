# Architecture

ClankShift is one small Rust binary with two modes:

- **tray** (default): resident, event-driven, owns scheduling and provider calls.
- **settings** (`--settings`): a short-lived egui window that edits `config.toml` and exits.

```
src/
  main.rs         mode dispatch
  tray.rs         tray menu + event loop + triggers
  settings.rs     settings window
  config.rs       user config (TOML, %APPDATA%)
  state.rs        observed provider state (JSON, %LOCALAPPDATA%) + action log
  schedule.rs     pure timing decisions (tested)
  providers/      one module per provider; `anchor()` is the only operation
  platform/       everything OS-specific (hidden processes, login startup, single instance)
```

## Idle model

The tray process sleeps in the OS event loop (`winit`, `ControlFlow::WaitUntil`) until a menu
click, a finished provider call, the next daily trigger, or a known window ending (to refresh the
menu). Waits are capped at 10 minutes because OS wait timers pause during system sleep; a
wake-up only compares timestamps. Measured on Windows 11 (release build): 0 ms CPU over
90 s idle, about 20 MB working set, one thread.

## Provider calls

Each call is a hidden child process (`CREATE_NO_WINDOW`) placed in a Windows job object that
kills the whole process tree when the call finishes or times out. No provider process outlives a
call.

**Codex.** `codex app-server` over stdio: `initialize`, then `account/rateLimits/read` (5-hour
and weekly windows with `resetsAt`) and `model/list`. If the 5-hour window did not start just
now, one was already running: done, nothing sent. Otherwise one ephemeral
`codex exec` with the cheapest listed model at low effort ensures the window is anchored,
then limits are re-read. Observations (2026-09) suggest the rate-limit read may itself start the
window, so ClankShift never reads Codex status casually, only when an anchor is due.

**Claude.** There is no documented quota-free status. `claude -p ... --output-format stream-json`
emits a `rate_limit_event` with the authoritative 5-hour `resetsAt`, so the anchor request is also
the status check. It is skipped while a previously reported window is still running.

**Window classification** (`schedule.rs`): a reported reset time within 10 minutes of
`now + 5h` means the window started with this call; anything earlier means it was already running.

## Triggers

- **Manual:** *Anchor … now* in the tray.
- **Start:** when ClankShift starts. With *Start at login* (per-user `Run` registry key, no
  admin rights), this is the login trigger.
- **Daily:** at a local time (DST-aware). If missed by more than an hour (asleep/off), it is skipped.

All triggers do nothing for a provider whose window is known to be running.

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
   changelog; a portable zip is the only artifact; nothing is published to crates.io.
