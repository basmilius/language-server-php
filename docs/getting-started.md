# Getting started

## Install

Download the archive of your platform from a [release](https://github.com/basmilius/language-server-php/releases) and check it against its `.sha256` file, or let an installer do that from the [descriptor](./distribution.md).

## Build

The workspace needs Rust 1.85 or newer. From a checkout:

```sh
cargo build --release --locked
```

The first build fetches the crates from the registry. The binary is `target/release/php-language-server`, with `.exe` on Windows, or `$CARGO_TARGET_DIR/release/php-language-server` when that is set. Inside an Electron archive, unpack it when packaging, since a binary cannot run from `app.asar`.

## Start it

```sh
php-language-server --stdio
```

Without arguments it starts the same way. `--version` (or `-V`) prints `php-language-server 0.3.0` and `--help` the usage; an unknown argument exits with code 2. Stdout carries only the protocol, so read stderr apart.

## Connect

Spawn the binary in the app's backend and connect an LSP client over its stdio. The server picks UTF-8 positions when a client offers them and UTF-16 otherwise. With [`@adecore/lsp`](https://adecore.dev/lsp/sessions), which offers only UTF-16:

```ts
import { LspSession, createStreamTransport, pathToFileUri } from '@adecore/lsp';

const child = spawn(executable, ['--stdio'], { cwd: projectFolder });
const session = new LspSession(createStreamTransport(streamOf(child)), {
    rootUri: pathToFileUri(projectFolder),
    initializationOptions: { storagePath: cacheFolder, phpVersion: '8.4' }
});

await session.initialize();
```

`streamOf` is the `ByteStream` adapter on [Sessions and transports](https://adecore.dev/lsp/sessions#transports). Pass a `storagePath`, or the server keeps no cache and has no standard library; see [Configuration](./configuration.md).

Run one server per project and stop it with `session.shutdown()` before the process. Which executable may run, where its cache lives and when it is installed are the app's to decide.
