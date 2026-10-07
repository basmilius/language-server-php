# Configuration

The server reads its settings from `initializationOptions`, from `workspace/configuration` for the section `phpLanguageServer` when the client supports it, and from `workspace/didChangeConfiguration`. Each takes the settings bare or nested under `phpLanguageServer`. A `workspace/configuration` request carries the document as `scopeUri`, so a client can answer per project or folder.

```json
{
    "phpVersion": "8.4",
    "storagePath": "/Users/me/Library/Caches/my-app/php",
    "inlayHints": { "parameterNames": true, "closureTypes": false },
    "inspections": { "unused-import": "off", "undefined-class": "warning", "deprecated": { "severity": "hint" } },
    "format": { "lineLength": 100, "alignAssignments": true },
    "usages": { "packages": false },
    "sql": { "dialect": "mysql", "version": "8.4", "schema": "db/schema.json" }
}
```

| Setting                     | Default | Meaning                                                                                  |
| --------------------------- | ------- | ---------------------------------------------------------------------------------------- |
| `phpVersion`                | `8.5`   | The language level of a project whose `composer.json` names none; `8.6` is known too     |
| `storagePath`               | none    | Where the server keeps its cache and downloads the standard library stubs                |
| `stubsPath`                 | none    | A folder of phpstorm-stubs to read instead of downloading them                           |
| `inlayHints.parameterNames` | `true`  | Names of parameters before arguments                                                     |
| `inlayHints.closureTypes`   | `true`  | The types of closure parameters and returns                                              |
| `inlayHints.propertyTypes`  | `true`  | The type arguments a framework gives a property, such as the model of a relation         |
| `inspections`               |         | Per inspection code: `false` or `'off'`, a severity, or `{ enabled, severity }`          |
| `format`                    |         | The [formatter](#formatter)                                                              |
| `usages.packages`           | `false` | Find usages and incoming calls also read the installed packages                          |
| `sql`                       |         | [SQL in strings](#sql-in-strings)                                                        |

A severity is `error`, `warning`, `information` (or `info`) or `hint`. Pass the folders at initialization.

## Projects

The workspace folders are the projects, with `rootUri` as the fallback, and folders may come and go. A folder with a `composer.json` is one project. A folder without one is searched up to four folders deep for `composer.json` files, skipping `vendor`, `node_modules`, dot folders and `~` backups, and each one found is a project with its own packages, language level and cache. A file belongs to the deepest project above it.

The language level of a project is `config.platform.php` of its `composer.json`, else the lower bound of `require.php`, else `phpVersion`, else 8.5. The level decides which syntax is an error and which functions of the standard library exist: an 8.1 project is not offered `array_find`.

Declarations come from the project's files, from the packages Composer installed through their PSR-4 and PSR-0 maps, and from the stubs, in that order of precedence. The server reads `composer.json` and `vendor/composer/installed.json`; it does not run Composer.

The server registers file watchers for PHP files, the Composer files and what the framework support reads, such as `.env` files and configuration. Send `workspace/didChangeWatchedFiles` for them. Indexing runs in the background and reports `$/progress` to a client that supports it; until it is done, answers are less complete.

## Standard library stubs

The standard library comes from JetBrains' phpstorm-stubs at one pinned commit, `PHP_LANGUAGE_SERVER_METADATA.stubsCommit`. They are not in the package or the binary. With a `storagePath` and no `stubsPath`, the server downloads that commit into `<storagePath>/stubs/<commit>/` in the background, keeps only the `.php` files and the license, and marks the folder complete when it is whole. A failed download is logged with `window/logMessage` and the rest keeps working, without the standard library.

For an app that controls downloads, install the pinned stubs itself and pass `stubsPath`. Without either setting the server knows the project and nothing of the standard library.

A project sees a default set of extensions (core, standard, SPL, date, json, pcre, mbstring, curl, PDO, intl and similar) plus every `ext-*` its `composer.json` requires.

## Cache

With a `storagePath`, each project keeps its index in `<storagePath>/cache`, so a second start skips the files that did not change. The words that find usages searches by are kept there too, read on the first search and again only for the files that changed. Without it nothing is kept between runs.

## Formatter

| `format` key               | Values                     | Default    |
| -------------------------- | -------------------------- | ---------- |
| `classBrace`               | `nextLine` or `sameLine`   | `nextLine` |
| `functionBrace`            | `nextLine` or `sameLine`   | `nextLine` |
| `blankLinesBetweenMembers` | A number                   | `1`        |
| `alignAssignments`         | Boolean                    | `false`    |
| `alignArrayArrows`         | Boolean                    | `false`    |
| `lineLength`               | A number; `0` wraps nothing | `120`     |
| `editorconfig`             | Boolean                    | `true`     |

The indentation comes from the client's formatting options. With `editorconfig` on, the nearest `.editorconfig` files up to `root = true` set `indent_style`, `indent_size`, `tab_width` and `max_line_length`, and a `format` setting overrides those.

## SQL in strings

The `sql` setting says whether and how the server reads the [SQL in strings](./sql.md). Without it, SQL is found and read in no dialect and without a schema.

```json
{
    "sql": {
        "enabled": true,
        "dialect": "mariadb",
        "version": "11.4",
        "schema": "db/schema.json",
        "sqlMode": "STRICT_TRANS_TABLES",
        "inspections": { "double-quoted-string": "off", "missing-where": "error" },
        "inlayHints": { "insertColumns": true, "selectColumns": true, "parameterNames": false },
        "questionPlaceholders": true,
        "detection": { "markers": true, "sinks": true, "heuristic": true, "threshold": 0.8 },
        "overrides": [
            { "path": "modules/reports", "dialect": "postgres", "version": "18", "schema": "/srv/reports.json" }
        ]
    }
}
```

| Key                       | Default   | Meaning |
| ------------------------- | --------- | ------- |
| `enabled`                 | `true`    | Whether strings are read as SQL at all |
| `dialect`                 | none      | `sqlite`, `mysql`, `mariadb`, `postgres` (also `postgresql`, `pgsql`) or `generic`. Without one, the dialect of the function a string is passed to or the one the project is configured for, else none |
| `version`                 | the newest | The version of the server, such as `8.4`, `11.4` or `3.47.2`; it counts only for its own dialect |
| `schema`                  | none      | A schema snapshot: absolute, a `file:` URI, or relative to the workspace folder of the document |
| `sqlMode`                 | the snapshot's | MySQL's and MariaDB's `sql_mode` of the connection, which decides some inspections |
| `inspections`             |           | Per SQL inspection id or row of the feature table: `false` or `"off"`, a severity, or `{ enabled, severity }` |
| `inlayHints`              | all on    | `insertColumns`, `selectColumns` and `parameterNames` |
| `questionPlaceholders`    | `true`    | A `?` is a placeholder of the database layer, also in PostgreSQL |
| `detection.markers`       | `true`    | `language=SQL` comments and heredocs labeled `SQL` |
| `detection.sinks`         | `true`    | Strings passed to the functions and methods that take SQL |
| `detection.heuristic`     | `true`    | Strings elsewhere that read as a whole statement |
| `detection.threshold`     | `0.8`     | How sure the heuristic has to be, from 0 to 1; higher finds fewer strings |
| `overrides`               |           | Entries with a `path` (a file or folder: absolute, a `file:` URI, or relative to the workspace folder of the document) and any of `dialect`, `version`, `schema` and `sqlMode`; the most specific path wins, key by key |

The SQL inspections and their ids are listed in the [SQL language server's documentation](https://github.com/basmilius/language-server-sql/blob/main/docs/inspections.md), and so is the [snapshot format](https://github.com/basmilius/language-server-sql/blob/main/docs/snapshot-format.md). Like the rest of the settings, `sql` may be answered per folder through `workspace/configuration`. A setting that cannot be read, such as an unknown dialect, is logged once and left out; a snapshot that cannot be read is shown once and read again when it changes.
