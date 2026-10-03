# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/xDaveN/clankshift/releases/tag/v0.1.0) - 2026-10-03

### Added

- show app version in tray and settings ([#43](https://github.com/xDaveN/clankshift/pull/43))
- embed license notices in the exe and show them in Settings ([#41](https://github.com/xDaveN/clankshift/pull/41))
- Repeat counts only the 5h limits after the first ([#37](https://github.com/xDaveN/clankshift/pull/37))
- compact tray status with bars and Repeat progress segments ([#36](https://github.com/xDaveN/clankshift/pull/36))
- repeat automatic 5h limit starts ([#34](https://github.com/xDaveN/clankshift/pull/34))
- settings window follows the Windows accent color ([#6](https://github.com/xDaveN/clankshift/pull/6))
- hard-hat robot app icon ([#5](https://github.com/xDaveN/clankshift/pull/5))
- cleaner settings window and tray text ([#3](https://github.com/xDaveN/clankshift/pull/3))
- retry failed automatic starts for up to an hour ([#2](https://github.com/xDaveN/clankshift/pull/2))
- call it a 5h limit instead of a window in the UI
- plainer tray and settings UX, app icon, log folder
- tray app that anchors Codex and Claude usage windows

### Fixed

- move settings version into window title ([#44](https://github.com/xDaveN/clankshift/pull/44))
- find provider CLIs missing from the tray's inherited PATH ([#42](https://github.com/xDaveN/clankshift/pull/42))
- fit collapsed settings window without scrolling ([#38](https://github.com/xDaveN/clankshift/pull/38))
- refuse unclear Codex 5h limit usage before starting ([#35](https://github.com/xDaveN/clankshift/pull/35))
- drop the settings reset preview and log settings window failures ([#31](https://github.com/xDaveN/clankshift/pull/31))
- keep the tray automatic-starts switch unchanged when saving fails ([#30](https://github.com/xDaveN/clankshift/pull/30))
- confirm a Codex 5h limit actually started ([#29](https://github.com/xDaveN/clankshift/pull/29))
- cancel automatic retries when the schedule changes ([#28](https://github.com/xDaveN/clankshift/pull/28))
- require included usage before starting a 5h limit ([#27](https://github.com/xDaveN/clankshift/pull/27))
- keep automatic Claude starts paused while a request is unresolved ([#26](https://github.com/xDaveN/clankshift/pull/26))
- publish releases only after the exe is attached ([#25](https://github.com/xDaveN/clankshift/pull/25))
- keep tray and settings from overwriting each other's config ([#24](https://github.com/xDaveN/clankshift/pull/24))
- keep automatic starts off while the settings file is unreadable ([#23](https://github.com/xDaveN/clankshift/pull/23))
- write state and config atomically and keep unreadable state uncertain ([#22](https://github.com/xDaveN/clankshift/pull/22))
- bound provider output by deadline and size ([#21](https://github.com/xDaveN/clankshift/pull/21))
- stop provider operations when process containment fails ([#20](https://github.com/xDaveN/clankshift/pull/20))
- use only the latest plausible Claude 5h limit reset ([#19](https://github.com/xDaveN/clankshift/pull/19))
- run today's daily start after a multi-day sleep ([#18](https://github.com/xDaveN/clankshift/pull/18))
- limit daily-start retries to an hour after the scheduled time ([#17](https://github.com/xDaveN/clankshift/pull/17))
- ship third-party and Rust runtime license notices with releases ([#16](https://github.com/xDaveN/clankshift/pull/16))
- link the C runtime statically so the exe runs without the VC++ Redistributable ([#15](https://github.com/xDaveN/clankshift/pull/15))
- don't retry Claude starts that may have been sent ([#14](https://github.com/xDaveN/clankshift/pull/14))
- only start Claude 5h limit with subscription sign-in ([#13](https://github.com/xDaveN/clankshift/pull/13))
- stop on unrecognized Codex 5h limit data ([#12](https://github.com/xDaveN/clankshift/pull/12))
- only start Codex 5h limit when none is running ([#11](https://github.com/xDaveN/clankshift/pull/11))
- show cargo test output in Windows terminals

### Other

- unslop README ([#40](https://github.com/xDaveN/clankshift/pull/40))
- tighten README tone and trim edge-case notes ([#39](https://github.com/xDaveN/clankshift/pull/39))
- remove redundant checks and impossible fallbacks ([#33](https://github.com/xDaveN/clankshift/pull/33))
- drop stale description mention from the settings row comment ([#32](https://github.com/xDaveN/clankshift/pull/32))
- tidy .gitignore and package description ([#10](https://github.com/xDaveN/clankshift/pull/10))
- release a plain clankshift.exe instead of a zip ([#9](https://github.com/xDaveN/clankshift/pull/9))
- say Linux and macOS are not supported yet ([#8](https://github.com/xDaveN/clankshift/pull/8))
- slimmer, friendlier README and fewer boilerplate files ([#7](https://github.com/xDaveN/clankshift/pull/7))
- match new settings and tray wording ([#4](https://github.com/xDaveN/clankshift/pull/4))
- explain approving CI on release PRs
- release only when the release PR is merged
- add docs, licenses, CI, release and dependency automation
