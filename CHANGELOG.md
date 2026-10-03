# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/xDaveN/clankshift/releases/tag/v0.1.0) - 2026-10-03

Initial release for x64 Windows 10/11.

- Start Codex and Claude subscription 5h limits from the tray, on launch, or at a daily time.
- Repeat automatic starts a set number of times or until stopped.
- Show provider-reported reset times and Repeat progress in the tray.
- Configure providers, schedules, CLI paths, and startup in Settings.
- Portable `clankshift.exe` with embedded license notices; no installer required.

Requires the Codex CLI and/or Claude Code, signed in with a subscription. For Claude, turn off Usage credits / extra usage before enabling starts. The executable is unsigned; Windows SmartScreen may show a warning.
