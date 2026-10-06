# language-server-php

A language server for PHP 8.1 through 8.6, written in Rust, that speaks LSP over stdio. It indexes a project with its Composer packages and the standard library, and answers completion, hover, navigation, usages, rename, inspections with fixes, refactors, formatting, semantic tokens, inlay hints and the tests a file can run, with support for PHPUnit, Pest, Laravel and Symfony. It reads code and never runs it.

## Install

Every [release](https://github.com/basmilius/language-server-php/releases) attaches an archive for macOS on Apple silicon, Linux arm64 and x64, and Windows x64, with a checksum each and a descriptor an installer pins. Or build it yourself with Rust 1.85 or newer:

```sh
cargo build --release --locked
```

The binary lands in `target/release/php-language-server`. PHP itself is not needed, to build or to run.

```sh
php-language-server --stdio
```

## Documentation

| Page                                          | What it covers                                                  |
| --------------------------------------------- | --------------------------------------------------------------- |
| [Getting started](./docs/getting-started.md)  | Building, starting and connecting                               |
| [Configuration](./docs/configuration.md)      | Settings, language level, Composer, stubs, cache and formatter  |
| [Features](./docs/features.md)                | What it answers, frameworks and runnable tests                  |
| [Clients](./docs/clients.md)                  | What an editor sends and announces for each feature             |
| [Distribution](./docs/distribution.md)        | Release archives and the descriptor an installer pins           |
| [Maintaining](./docs/maintaining.md)          | The crates, checks, corpus and releases                         |

[NATIVE.md](./NATIVE.md) describes how the implementation works and what is left to do, and [MEASUREMENTS.md](./MEASUREMENTS.md) what it was measured to do.

## Limits

- PHP 8.6 syntax is known; a project that names no level is read at 8.5 until 8.6 is released.
- Frameworks and packages are modeled where they give strings and members meaning: Laravel, Eloquent, Livewire, Inertia, Pennant, Filament, Symfony, Doctrine, API Platform's serialization groups, Messenger, Workflow and the Raxos ORM. Other packages get what their declarations and PHPDoc say.
- Usages are found in the project, and in installed packages when `usages.packages` asks for them, across PHP, Blade, Twig and the YAML of a Symfony project. Strings count when a framework gives them meaning or when they hold a qualified class name; other strings and ordinary comments do not.
- Nothing is run: dynamic code, such as variable variables and members made at run time, gives `mixed`, and a name built at run time is never reported as missing.

## License

[Functional Source License, Version 1.1, MIT Future License](./LICENSE). The phpstorm-stubs and php-src corpora are downloaded separately and keep their own licenses; see [THIRD-PARTY.md](./THIRD-PARTY.md).
