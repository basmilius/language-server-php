# basmilius/language-server-php

A PHP language server in Rust that speaks LSP over stdio. `README.md` and `docs/` are for people who use it, `NATIVE.md` says how the implementation works and why, `MEASUREMENTS.md` holds what it was measured to do (dated, per project), `docs/clients.md` lists what a client has to send and announce, and this file is for agents who work on it.

It moved here from `basmilius/adecore` (`packages/php-language-server`), which took it from Ruimte at revision `9729144f0df3f25628f20cc283dee54f8d9e8162` (`sourceRevision` in `native-source.json`). It is not on npm: an app downloads a release archive pinned by its descriptor, or builds a checkout with cargo.

## Who uses it

Ruimte (`/Users/bas/Development/Projects/ruimte`, `apps/server/src/language`). Its daemon installs the release pinned in `php-native-release.json`, or builds a checkout when it runs from source. It reads the version from `Cargo.toml` and the stubs pin from `crates/index/src/stubs.rs` by pattern, so keep both lines in their current shape.

## The core

What every language server does the same way comes from `basmilius/language-server-core` (usually checked out beside this one as `../core`): the line index and position encodings (`lsc-text`), the token cursor and tree builder the parser is written on (`lsc-syntax`), and documents, URIs, encoding negotiation, dispatch, progress, the main loop and `main` (`lsc-server`). It is a Git dependency pinned to a tag in `[workspace.dependencies]` of `Cargo.toml`. What PHP means stays here; something every server needs goes to the core first, gets a new tag there, and this workspace moves its pin.

To build against a local checkout of the core, put a `[patch]` in `.cargo/config.toml`, which `.gitignore` keeps out of Git:

```toml
[patch."https://github.com/basmilius/language-server-core"]
lsc-server = { path = "../core/crates/server" }
lsc-syntax = { path = "../core/crates/syntax" }
lsc-text = { path = "../core/crates/text" }
```

While it is there, Cargo points the core's entries in `Cargo.lock` at the local paths: run the checks without `--locked` and do not commit `Cargo.lock`. Remove the file and the next Cargo command without `--locked` puts the pinned tag back in the lock; `git diff Cargo.lock` is empty again. For one command, the patch fits on the command line: `cargo --config 'patch."https://github.com/basmilius/language-server-core".lsc-text.path="../core/crates/text"' test`.

## Checks

All of these pass before a commit; CI (`.github/workflows/ci.yml`) runs them on every push to main and every PR, the handshake on every platform.

```sh
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
python3 scripts/test-native-release.py
cargo build --release --locked && python3 scripts/handshake.py target/release/php-language-server
```

## Releases

- The version lives in `Cargo.toml` and `native-source.json`; `test-native-release.py` fails when they differ. The tag is `v<version>`.
- A release starts as a draft: `gh release create v<version> --draft --notes-file <notes>`, then `gh workflow run release.yml -f version=<version>`. The workflow tags the commit, builds every platform, attaches the archives, checksums and descriptor, and publishes the release last.
- Push and release only when Bas asks.

## Documentation

- A change in behavior updates `NATIVE.md` (how and why, no numbers), `docs/` where a user sees it, and `docs/clients.md` when a client has to do something new.
- A measurement goes into `MEASUREMENTS.md` with its date and project, replacing the one it supersedes.

## Conventions

- Rust 2024, `rustfmt.toml`, `unsafe` forbidden. `.editorconfig` is the rule for the rest.
- American English everywhere. Never an em dash or an en dash.
- Comments say why, never what the code already says.
- Conventional commits in English. No attribution lines.
