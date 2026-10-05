# @adecore/php-language-server

A PHP language server over stdio, with its complete five-crate Cargo workspace and a small Node entry point. It reads PHP 8.1 through 8.5 and supports navigation, completion, rename, inspections, formatting, refactors, PHPUnit, Pest, Laravel and Symfony. The [native workspace guide](./NATIVE.md) preserves the implementation details, measurements and remaining work.

The package is private at `0.0.0`. Cargo's workspace and binary remain `0.1.0`. Building the JavaScript entry point does not compile Rust, download stubs or install a server.

```ts
import { PHP_LANGUAGE_SERVER_METADATA, phpLanguageServerSourcePath, phpLanguageServerBinaryPath } from '@adecore/php-language-server';

const sourcePath = phpLanguageServerSourcePath();
const executable = phpLanguageServerBinaryPath({ sourcePath });
```

`phpLanguageServerSourcePath()` locates the bundled Cargo workspace from both `source` and compiled exports. `phpLanguageServerBinaryPath()` returns an existing release build or `null`. Set `targetDirectory` when using `CARGO_TARGET_DIR`; set `platform` when inspecting a Windows build from another host. Electron hosts must unpack native executable paths from `app.asar` into `app.asar.unpacked`.

`PHP_LANGUAGE_SERVER_METADATA` exposes `version`, `stubsCommit` and the original `sourceRevision`. `PhpLanguageServerRelease` and `PhpLanguageServerAsset` describe the installer's pinned binary version, stubs commit, platform URLs, SHA-256 checksums, archive formats and executable paths. `PhpLanguageServerPlatform` lists supported release assets. `native-source.json` holds the Rust targets for macOS arm64/x64, Linux arm64/x64 and Windows x64. There are no downloaded binaries or stub files in this npm package.

Read the [PHP handbook](https://adecore.dev/php-language-server/handbook/getting-started) for installation boundaries, distribution, lifecycle, configuration, language features and native maintenance.

## Build and validate

From this folder:

```sh
bun run build
bun run typecheck
bun run test
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
python3 scripts/test-native-release.py
cargo build --release --locked
python3 scripts/handshake.py target/release/php-language-server
```

On Windows, pass `target/release/php-language-server.exe` to the handshake. It checks the real process's version, initialize response, UTF-8 negotiation, document symbols, shutdown and exit without PHP, network access or fixed delays. Real-corpus tests report a skip when `corpus/` is absent. The original corpus and measurement scripts are retained; `scripts/fetch-corpus.sh` downloads the pinned corpora only when explicitly run.

The MIT license matches the native workspace's declaration. [Third-party notes](./THIRD-PARTY.md) describe the external stubs and corpora, which are fetched separately and retain their licenses.

## Native release assets

The dedicated `php-language-server.yml` workflow checks Cargo and builds native targets separately from the TypeScript graph. A manual run or a shared workflow call with `release_tag` set to an existing Adecore tag generates archives, checksum sidecars and `php-language-server-release.json` as workflow artifacts. It does not attach them to a GitHub release or publish npm packages. The shared release workflow calls the native workflow with the exact release tag, verifies the descriptor version and attaches the files before npm publication. Consumers must pin a published descriptor; extraction itself publishes no assets.

Native archives include the runtime Rust dependencies' license files and a dependency manifest, collected from the locked Cargo registry cache. Cargo must be on the path when generating assets. Cargo metadata may fetch registry manifests missing from its cache; no external stubs or corpora are included. For a locally built binary, generate one asset with:

```sh
python3 scripts/native-release.py asset \
    --binary target/release/php-language-server \
    --platform darwin-arm64 --tag v0.0.0-local --output artifacts
```

The descriptor's `version` is Cargo's `0.1.0`, which the installer checks against `--version`. `adecoreVersion` records the shared release series, and `sourceRevision` pins the exact Adecore source commit. Archive metadata also retains the original source handoff. Cargo versions and internal crate requirements stay intact in this migration; a later native version change must update Cargo.lock and `native-source.json` together. The JavaScript build rejects stale metadata.

## Host integration

Use an explicit source path or `phpLanguageServerSourcePath()` in development, replacing assumptions about sibling checkout layout. An installed daemon should consume a release descriptor only after its assets exist, and pass it into its current native install policy. The descriptor fits a `version`, `stubsCommit`, `assets` shape without changing the LSP or an application's wire protocol.

The host keeps user-triggered installation, custom-server permission checks, checksum verification before extraction, its existing cache location, stub installation markers and one server process per project. Launch the installed executable with `--stdio`, supplying `storagePath` and the pinned `stubsPath` through initialization options. Importing this package performs no installation or process management.

Keep the original implementation in the consumer until the complete cutover has passed package, daemon, editor and installed-artifact validation. Further language and framework work remains as listed in `NATIVE.md`.
