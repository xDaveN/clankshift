# Contributing

ClankShift is small on purpose. For anything bigger than a fix, open an issue first.

## Build

You need Rust ([rustup](https://rustup.rs)) and, on Windows, the MSVC C++ build tools.

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo run                 # tray app
cargo run -- --settings   # settings window only
```

## Ground rules

- Zero idle cost: no polling, no resident provider processes.
- Never guess provider state; only show what the provider reported.
- OS-specific code goes in `src/platform/`.
- PR titles use [Conventional Commits](https://www.conventionalcommits.org) (`feat:`, `fix:`, `docs:` …).
  PRs are squash-merged and the title becomes the changelog entry.

Contributions are dual licensed under MIT and Apache-2.0, like the rest of the project.

## Releasing

[release-plz](https://release-plz.dev) keeps a release PR open on `main`. Merging it tags the
version and creates a draft GitHub Release; the *Windows exe* job attaches `clankshift.exe` plus its
license notices, then publishes it. If that job fails, open the Release run and use *Re-run failed
jobs* (re-running all jobs builds nothing, because the tag already exists). The notices are written
by `scripts/license-notices.sh`: `THIRD-PARTY-LICENSES.txt` (crates and fonts, via
[cargo-about](https://github.com/EmbarkStudios/cargo-about) and `about.toml`) and `RUST-LICENSES.html`
(Rust standard library, from the building toolchain). CI runs the same script, so a missing notice
fails the PR.

That PR is opened by a bot, so its CI waits for approval: *Checks* tab → *Approve workflows to run*,
then merge once green. (A `RELEASE_PLZ_TOKEN` secret with a fine-grained PAT, *Contents* and
*Pull requests* read/write, skips that step.)
