# @adecore/php-language-server

[![npm](https://img.shields.io/npm/v/@adecore/php-language-server)](https://www.npmjs.com/package/@adecore/php-language-server)
[![Docs](https://img.shields.io/badge/docs-adecore.dev-blue)](https://adecore.dev/php-language-server/)

A language server for PHP 8.1 through 8.5, written in Rust, that speaks LSP over stdio: completion, hover, navigation, usages, rename, inspections with fixes, refactors, formatting, semantic tokens, inlay hints and runnable tests, with support for Composer, PHPUnit, Pest, Laravel and Symfony. The package holds the Cargo workspace and a small Node entry point that finds a built binary. It downloads, builds, installs and starts nothing.

## Install and build

```sh
bun add @adecore/php-language-server
cd node_modules/@adecore/php-language-server && cargo build --release --locked
```

Or install a release archive: every release attaches one for macOS on Apple silicon, Linux arm64 and x64, and Windows x64, with a checksum each and a descriptor to pin.

```ts
import { PHP_LANGUAGE_SERVER_METADATA, phpLanguageServerBinaryPath } from '@adecore/php-language-server';

const executable = phpLanguageServerBinaryPath(); // the release build, or null
```

Start it with `--stdio`. `--version` prints the server's own version, `PHP_LANGUAGE_SERVER_METADATA.version`, which is separate from the npm version.

## Documentation

| Page | What it covers |
|---|---|
| [Getting started](https://adecore.dev/php-language-server/getting-started) | Building, finding, starting and connecting |
| [Configuration](https://adecore.dev/php-language-server/configuration) | Settings, language level, Composer, stubs, cache and formatter |
| [Features](https://adecore.dev/php-language-server/features) | What it answers, frameworks and runnable tests |
| [Distribution](https://adecore.dev/php-language-server/distribution) | Release archives and the descriptor an installer pins |
| [Maintaining](https://adecore.dev/php-language-server/maintaining) | The crates, checks and corpus |

[NATIVE.md](./NATIVE.md) describes the implementation, measurements and what is left to do.

## License

MIT. The phpstorm-stubs and php-src corpora are downloaded separately and keep their own licenses; see [THIRD-PARTY.md](./THIRD-PARTY.md).
