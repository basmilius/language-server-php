# basmilius/language-server-php

A PHP language server in Rust that speaks LSP over stdio. `README.md` and `docs/` are for people who use it, `NATIVE.md` holds the implementation notes and measurements, and this file is for agents who work on it.

It moved here from `basmilius/adecore` (`packages/php-language-server`), which took it from Ruimte at revision `9729144f0df3f25628f20cc283dee54f8d9e8162` (`sourceRevision` in `native-source.json`). It is not on npm: an app downloads a release archive pinned by its descriptor, or builds a checkout with cargo.

## Who uses it

Ruimte (`/Users/bas/Development/Projects/ruimte`, `apps/server/src/language`). Its daemon installs the release pinned in `php-native-release.json`, or builds a checkout when it runs from source. It reads the version from `Cargo.toml` and the stubs pin from `crates/index/src/stubs.rs` by pattern, so keep both lines in their current shape.

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

## Conventions

- Rust 2024, `rustfmt.toml`, `unsafe` forbidden. `.editorconfig` is the rule for the rest.
- American English everywhere. Never an em dash or an en dash.
- Comments say why, never what the code already says.
- Conventional commits in English. No attribution lines.
