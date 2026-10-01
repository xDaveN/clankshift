# ClankShift

A tiny Windows tray utility that starts your AI subscriptions' 5h limits at times you choose,
so the resets land when you actually need capacity.

Supported: **OpenAI Codex** and **Anthropic Claude** subscriptions.

## Why

Codex and Claude subscriptions have a 5h limit: a usage allowance that starts with your first request and resets 5 hours later.
Start working at 09:00, hit the limit at 11:30, and you wait until 14:00. If the 5h limit had
started at 07:00, it would have reset at 12:00, with a fresh one right when you need it.

ClankShift sends the smallest possible message at the moment you choose, but only when no
5h limit is already running. (In the code and docs this is called *anchoring*.) **It shifts when resets happen; it does not give you more quota**, and
it does nothing for weekly limits.

## How it works

| | Codex | Claude |
|---|---|---|
| 5h limit status | Read from `codex app-server` (uses no quota) | Only available by sending a request |
| Starting a 5h limit | If none is running: one tiny request with the cheapest model at low effort | A one-line Haiku prompt with tools, MCP and hooks disabled (~450 tokens); it also reports the 5h limit |
| While a 5h limit is known to be running | Nothing is sent | Nothing is sent |

The tray only shows what a provider actually reported: when the current 5h limit resets and whether
ClankShift started it. It does not show live usage percentages; that would need constant polling
(and, for Claude, spending quota). Use `/status` in Codex or `/usage` in Claude Code for that. Provider calls run as short-lived
hidden processes; nothing runs while ClankShift is idle.

## Requirements

- Windows 10 or 11 (Linux and macOS are not supported yet)
- The official [Codex CLI](https://github.com/openai/codex) and/or
  [Claude Code](https://github.com/anthropics/claude-code), installed and logged in with
  your subscription

## Install

1. Download `clankshift-vX.Y.Z-windows-x64.zip` from [Releases](https://github.com/xDaveN/clankshift/releases).
2. Extract it anywhere and run `clankshift.exe`.

The executable is not code-signed yet, so Windows SmartScreen may warn on first run
(*More info → Run anyway*).

## Use

Everything is in the tray icon menu:

- each provider's 5h limit: when it resets, or when the last known one ended (hover the icon for the same summary)
- **Start Codex/Claude 5h limit**, disabled while one is known to be running
- **Automatic starts** on/off (same as *Master switch* in Settings)
- **Settings…**: providers, start 5h limits on launch and/or every day at a set time,
  start with Windows, and program paths
- **Open logs**: error details are here

Example: to have resets at 12:00 and 17:00, start them at 07:00: turn on *Every day at 07:00*,
or *On launch* with *Start with Windows* if you log in around then.

Files:

- settings: `%APPDATA%\ClankShift\config.toml`
- observed state and log: `%LOCALAPPDATA%\ClankShift\`

## Limitations

- Provider behavior is not a documented contract and can change at any time; ClankShift may
  need updates when it does.
- Each started 5h limit uses a very small amount of your quota.
- A daily start missed by more than an hour (computer asleep or off) is skipped.
- If an automatic start fails (e.g. Codex is broken or offline), ClankShift tries again every
  10 minutes for up to an hour, then gives up until the next automatic start.
- Claude may occasionally send a request when a 5h limit was already running (e.g. you used
  Claude elsewhere after ClankShift's last check). That costs a negligible amount and changes nothing.

## Build from source

See [CONTRIBUTING.md](CONTRIBUTING.md). Design notes: [docs/architecture.md](docs/architecture.md).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option. Unless you explicitly state otherwise, any contribution intentionally submitted for
inclusion in this project shall be dual licensed as above, without any additional terms or conditions.

ClankShift is an independent project, not affiliated with OpenAI or Anthropic.
