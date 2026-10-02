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
Both pre-start reads must also report `ordinaryUsageAllowed: true`, the backend's permission for
included plan usage. False, missing, null or malformed permission stops the start. A running
window is still recorded without sending, and the post-start observation is not gated on this
permission because the request has already completed.

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

Subscription sign-in does not establish that included quota remains: Claude can use paid usage
credits when the account has them enabled. There is no verified included-only request control in
the supported CLI (`--max-budget-usd` caps estimated API spend, not overage), and `auth status`
does not report the setting. Claude support therefore requires Usage credits / extra usage to be
disabled in Claude Settings > Usage; the README states this requirement. The app cannot verify or
enforce that account setting, so an account with credits enabled is unsupported and carries a
billing risk. This is a documented requirement, not a programmatic no-overage guarantee.

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
- **Daily:** at a local time (DST-aware). After sleep, only the latest missed time counts, and only
  within an hour; older ones are skipped. Times that passed while ClankShift was closed, or before a
  settings change, are never made up: only *Start* runs at launch.

All triggers do nothing for a provider whose window is known to be running.

`state.json` and `config.toml` are replaced atomically: each write creates its own new temp file
(unique name), flushes it, then renames it over the original. A crash or a concurrent writer leaves
one complete file, never a truncated or mixed one. A missing `state.json` is a first run. One that
exists but cannot be read may have held a window whose reset is as late as the parsers accept for an
observation made by its last modification: 5h + 11 min (Claude's 10 min rounding and 1 min clock
skew; Codex's bound is smaller). For Claude it may also have held a pending request (below), which
may have gone out until this start, so Claude counts the 5h + 11 min from now. Until then, the
providers are marked unknown in the state (saved like any other state), and automatic starts are
skipped. Manual starts still work, and a provider's next report clears its mark, so a manually
observed window survives a restart while the other provider stays unknown.

A failed automatic (start or daily) anchor is retried every 10 minutes, but only within an hour
of the trigger; later than that the reset would land somewhere the user did not ask for, so it
gives up. Retries are in memory only. Changing when automatic starts run (the automatic switch,
on-launch, daily time, or a provider's switch) cancels pending retries and any retry of a start
still running; CLI path/model edits do not. Failed manual starts are not retried. A Claude failure after
the request was launched (timeout, no 5h limit reported) is not retried either: the request may
have started the 5h limit, and repeating it would only spend quota. Codex retries are safe because
every Codex start is preceded by fresh status reads. A usable 5h reset Claude reports is kept even
if the request then fails or its output breaks off (timeout); a missing or malformed reset never
hides such a failure.

Only the reply tells whether a Claude request started a window, and nothing bounds when it goes out
while its process runs: the PC can sleep during the sign-in check or the request, and process
timeouts may not count wall-clock time across sleep. So before launching, the tray saves the request
as pending (nothing is sent if that save fails), and automatic Claude starts wait while it is
pending. When the operation ends, its processes have ended too, so a request went out, if at all,
before that moment: a report clears the mark, a failure known to precede the request just drops it,
and a possibly-sent failure marks Claude unknown until 5h + 11 min after it. A pending mark left by
a crash, Quit or failed save is resolved the same way at the next start: only one ClankShift runs,
and its request processes die with it (job object), so the previous request went out before then.

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
