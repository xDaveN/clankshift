# ClankShift

A tiny Windows tray utility that starts ("anchors") your AI subscription usage windows at a time
you choose, so the resets land when you actually need capacity.

Supported: **OpenAI Codex** and **Anthropic Claude** subscriptions.

## Why

Codex and Claude subscriptions have a 5-hour usage window that starts with your first request.
Start working at 09:00, hit the limit at 11:30, and you wait until 14:00. If the window had
started at 07:00, it would have reset at 12:00, with a fresh window right when you need it.

ClankShift sends the smallest possible request at the moment you choose, but only when no
window is already running. **It shifts when resets happen; it does not give you more quota**, and
it does nothing for weekly limits.

## How it works

| | Codex | Claude |
|---|---|---|
| Window status | Read from `codex app-server` (uses no quota) | Only available by sending a request |
| Anchor | If no window is running: one tiny request with the cheapest model at low effort | A one-line Haiku prompt with tools, MCP and hooks disabled (~450 tokens); it also reports the window |
| While a window is known to be running | Nothing is sent | Nothing is sent |

Status shown in the tray always comes from the provider. When a reported window has ended,
ClankShift says the state is unknown rather than guessing. Provider calls run as short-lived
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

- current window per provider (reset time and usage, when known)
- **Anchor Codex/Claude now**, disabled while a window is known to be running
- **Automatic anchoring** on/off
- **Settings…**: providers, anchor when ClankShift starts, anchor every day at a set time,
  start at login, and CLI paths

To have resets at 12:00 and 17:00, anchor at 07:00: turn on *Start ClankShift when I log in*
and/or *Every day at 07:00*.

Files:

- settings: `%APPDATA%\ClankShift\config.toml`
- observed state and log: `%LOCALAPPDATA%\ClankShift\`

## Limitations

- Provider behavior is not a documented contract and can change at any time; ClankShift may
  need updates when it does.
- Each anchor uses a very small amount of your quota.
- A daily anchor missed by more than an hour (computer asleep or off) is skipped.
- Claude may occasionally send a request when a window was already running (e.g. you used
  Claude elsewhere after ClankShift's last check). That costs a negligible amount and changes nothing.

## Build from source

See [CONTRIBUTING.md](CONTRIBUTING.md). Design notes: [docs/architecture.md](docs/architecture.md).

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT license](LICENSE-MIT)
at your option. Unless you explicitly state otherwise, any contribution intentionally submitted for
inclusion in this project shall be dual licensed as above, without any additional terms or conditions.

ClankShift is an independent project, not affiliated with OpenAI or Anthropic.
