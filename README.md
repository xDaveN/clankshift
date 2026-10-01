# ClankShift

[![CI](https://github.com/xDaveN/clankshift/actions/workflows/ci.yml/badge.svg)](https://github.com/xDaveN/clankshift/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)

**Start your Codex and Claude 5h limits early, so they reset when you actually need them.**

A tiny Windows tray app. It sits there doing nothing, and once a day (or when you log in)
it sends one tiny message to kick off a fresh 5h limit.

## Why

Your 5h limit starts with your first request. Start at 09:00, burn through it by 11:30, and
you're stuck until 14:00.

If it had started at 07:00 instead, it would reset at 12:00, right when you run dry. That's
all ClankShift does: it starts the 5h limit at a time you pick, while you're still having coffee.

> [!NOTE]
> This moves *when* resets happen. It doesn't give you more quota, and it does nothing for
> weekly limits.

## Install

Windows only for now. Linux and macOS aren't supported yet, but the code is kept portable so
they can follow.

You need Windows 10/11 and the [Codex CLI](https://github.com/openai/codex) and/or
[Claude Code](https://github.com/anthropics/claude-code), signed in with your subscription.

Download `clankshift.exe` from [Releases](https://github.com/xDaveN/clankshift/releases), put it
somewhere permanent and run it. To update, replace the file.

It isn't code-signed, so SmartScreen may complain the first time: *More info → Run anyway*.

## Use

Everything lives in the tray menu:

- when each provider's 5h limit resets (also on hover)
- **Start Codex / Claude 5h limit** right now
- **Automatic starts** on/off
- **Settings…** for providers, start on launch or daily at a set time, start with Windows,
  and CLI paths
- **Open logs** if something went wrong

For resets at 12:00 and 17:00, start at 07:00: turn on *Every day at 07:00*, or *On launch*
plus *Start with Windows* if you usually log in around then.

## How it works

ClankShift only acts when no 5h limit is already running. Otherwise it stays quiet: no polling,
no background processes, no network traffic.

- **Codex:** reads the 5h limit from `codex app-server` (no quota used). If none is running,
  sends one tiny request with the cheapest model.
- **Claude:** there's no free way to check, so the start request *is* the check: a one-line
  Haiku prompt with tools, MCP and hooks off (~450 tokens).

The tray only shows what the provider reported, never guesses. No live usage percentages,
since that would need constant polling. Use `/status` in Codex or `/usage` in Claude Code for that.

Settings are in `%APPDATA%\ClankShift\config.toml`; state and log in `%LOCALAPPDATA%\ClankShift\`.

## Good to know

- None of this is a documented provider contract. If Codex or Claude change how limits work,
  ClankShift may need an update.
- Each start costs a very small bit of quota.
- A daily start missed by more than an hour (PC asleep or off) is skipped.
- A failed automatic start is retried every 10 minutes for up to an hour.
- Claude may occasionally get a request while a 5h limit is already running (if you used it
  elsewhere in the meantime). Costs next to nothing.

## Building

See [CONTRIBUTING.md](CONTRIBUTING.md). Design notes are in [docs/architecture.md](docs/architecture.md).
Found a security issue? Please [report it privately](https://github.com/xDaveN/clankshift/security/advisories/new).

## License

[MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), your choice.

Not affiliated with OpenAI or Anthropic.
