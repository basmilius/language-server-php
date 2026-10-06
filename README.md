# language-server-php

A language server for PHP 8.1 through 8.5, written in Rust, that speaks LSP over stdio. It indexes a project with its Composer packages and the standard library, and answers completion, hover, navigation, usages, rename, inspections with fixes, refactors, formatting, semantic tokens, inlay hints and the tests a file can run, with support for PHPUnit, Pest, Laravel and Symfony. It reads code and never runs it.

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

[NATIVE.md](./NATIVE.md) describes the implementation, measurements and what is left to do.

## Limits

- Twig templates follow their templates, blocks, functions, filters, the routes and translations they name and the variables controllers pass, with diagnostics. Blade templates are read as a whole: scope, `@foreach`, `@props`, the variables controllers, includes and components pass, sections, stacks, slots, usages, rename and diagnostics.
- Framework strings and templates are read where a framework gives them meaning: Inertia page names, Pennant features, Filament columns, Livewire components and their `wire:` attributes, Eloquent relation and column strings, validation rules, casts, DQL and the Doctrine query builder, and serialization groups. Other packages are not modeled.
- Usages are found in the project, and in installed packages when `usages.packages` asks for them. Strings count when they name a route, a config key, a view, a translation or another name a framework declares, in PHP and Blade; ordinary strings and comments, Twig and YAML do not.
- Dynamic code, such as variable variables and members made at run time, gives `mixed`.

## License

MIT. The phpstorm-stubs and php-src corpora are downloaded separately and keep their own licenses; see [THIRD-PARTY.md](./THIRD-PARTY.md).
