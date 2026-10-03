# ClankShift

[![CI](https://github.com/xDaveN/clankshift/actions/workflows/ci.yml/badge.svg)](https://github.com/xDaveN/clankshift/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue)](#license)

**Start your Codex and Claude 5h limits early, so they reset when you actually need them.**

A tiny Windows tray app. It sits there doing nothing, and at a time you pick (or when you log in)
it sends one tiny message to kick off a fresh 5h limit.

## Why

With most providers, your 5h limit starts with your first request. Start at 09:00, burn through it by 11:30, and
you're stuck until 14:00.

If it had started at 07:00 instead, it would reset at 12:00, right when you run dry. That's
all ClankShift does. It starts the 5h limit at a time you pick and repeats it as often as you want.

> [!NOTE]
> This moves *when* resets happen. It doesn't give you more quota, and it does nothing for
> weekly limits.

## Install

Windows only for now. Linux and macOS aren't supported yet, but the code is kept portable so
they can follow.

You need x64 Windows 10/11 and the [Codex CLI](https://github.com/openai/codex) and/or
[Claude Code](https://github.com/anthropics/claude-code), signed in with your subscription.

> [!IMPORTANT]
> Using Claude? Turn off **Usage credits / extra usage** in Claude's **Settings > Usage**.
> ClankShift can't see that setting, so with it on, a start could spend paid credits once your
> included quota is gone. I don't know how broke you are.

Download `clankshift.exe` from [Releases](https://github.com/xDaveN/clankshift/releases), put it
somewhere permanent and run it. To update, choose **Quit** in the tray, replace the file, then
run it again.

It isn't code-signed, so SmartScreen may complain the first time: *More info → Run anyway*.

## Use

Everything lives in the tray menu:

- when each provider's 5h limit resets (also on hover)
- **Start Codex / Claude 5h limit** right now
- **Automatic starts** on/off
- **Settings…** for providers, start on launch or daily at a set time, repeat, start with
  Windows, and CLI paths
- **Open logs** if something went wrong

Want a reset around 12:00? Start at 07:00: turn on *Every day at 07:00*, or *On launch* plus
*Start with Windows* if you usually log in around then.

Want another one around 17:00 too? Turn on *Repeat*. After an automatic start, it starts the
next 5h limit right at each reset, a set number of times or until stopped. The tray shows the
progress.

## How it works

ClankShift skips a provider whose reported 5h limit is still running. Between scheduled actions
it stays quiet: no provider polling, background provider processes, or network traffic.

- **Codex:** reads the 5h limit from `codex app-server` (no quota used). If none is running
  and Codex says your plan has usage left, it sends one tiny request with a cheap model.
  If that's unclear, nothing is sent.
- **Claude:** there's no free way to check, so the start request *is* the check: a one-line
  Haiku prompt with tools, MCP and hooks disabled (~450 tokens).

The tray only shows what the provider reported, never guesses. No live usage percentages,
since that would need constant polling.

Settings are in `%APPDATA%\ClankShift\config.toml`; state and log in `%LOCALAPPDATA%\ClankShift\`.

## Good to know

- None of this is an official provider feature. If Codex or Claude change how limits work,
  ClankShift may break.
- Each start can cost a tiny bit of quota. Claude may occasionally get one while a 5h limit is
  already running (say, you used it elsewhere), since it can't check first.
- A failed start is retried every 10 minutes for up to an hour, unless it may already have
  gone through. If a Claude start might have gone through without a clear answer, automatic
  Claude starts pause for about 5¼ hours to be safe.
- Claude rounds its reset times down, so *Repeat* waits about 11 minutes past the shown reset.
- After switching CLI accounts, the old account's reset time may show until it passes,
  but if you're using this tool you're probably not stacking subs anyways.

## License

[MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), your choice. Third-party notices are built into
the exe under **Settings > Licenses**. Found a security issue? Please
[report it privately](https://github.com/xDaveN/clankshift/security/advisories/new).

Not affiliated with OpenAI or Anthropic.
