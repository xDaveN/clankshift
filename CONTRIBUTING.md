# Contributing

Thanks for your interest! ClankShift is small on purpose; please open an issue before large changes.

## Build and test

Requires Rust (via [rustup](https://rustup.rs)) and, on Windows, the MSVC C++ build tools.

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run
```

`cargo run` starts the tray app; `cargo run -- --settings` opens only the settings window.
Logs go to `%LOCALAPPDATA%\ClankShift\clankshift.log`.

## Guidelines

- Keep idle cost at zero: no polling, no resident provider processes, no timers for cosmetics.
- Never guess provider state. Only values reported by the provider are shown as known.
- Keep OS-specific code in `src/platform/`.
- Tests focus on timing decisions and provider-output parsing.
- PR titles use [Conventional Commits](https://www.conventionalcommits.org) (`feat:`, `fix:`, `docs:` …);
  PRs are squash-merged and the title becomes the changelog entry.

## Releases (maintainers)

[release-plz](https://release-plz.dev) keeps a release PR open on `main`. Merging it tags the
version, creates the GitHub Release and attaches the Windows zip.

The release PR is opened by a bot, so its CI waits for approval: open the PR's *Checks* tab
and click *Approve workflows to run*, then merge once the checks pass. To skip that step, add a
repository secret `RELEASE_PLZ_TOKEN` holding a fine-grained personal access token for this
repository with *Contents* and *Pull requests* read/write permission.
