# What a client provides

The server knows nothing of the editor that starts it. Everything it can do depends on what the client sends and announces over standard LSP, plus the custom request below. This page lists what a client has to do for each feature to work. A client that leaves something out still gets a working server, only without that feature.

## Documents

- Send `textDocument/didOpen`, `didChange` and `didClose` for PHP files.
- Send the same for Blade templates (language id `blade` or a path ending in `.blade.php`), Twig templates (`twig` or `.twig`) and YAML files (`yaml`, `.yaml` or `.yml`). The server only reads a YAML file under a Symfony project's `config/` or `translations/`, but it needs the open text to answer in it.
- Changes may be incremental. Positions are UTF-16 unless the client offers `utf-8` in `general.positionEncodings`.

## Files on disk

- Support dynamic registration of `workspace/didChangeWatchedFiles` and send the changes the server registers for: PHP files, `composer.json`, `vendor/composer/installed.json`, `vendor/composer/autoload_classmap.php` (`**/vendor/composer/autoload_classmap.php`, so a client that only lets `vendor` through for a glob naming it still sends it), `.env` files, `lang`, `translations`, `templates`, YAML and XML under `config`, `.sql` files (`**/*.sql`: the DDL the SQL in strings is read against, and Laravel's schema dumps), and the Inertia pages under `resources/js` or `resources/ts` (`Pages` or `pages`).
- A schema snapshot the `sql` setting names is registered on its own (registration id `php-sql-snapshots`, a glob per snapshot path, registered again when the set changes) once it was read. A client that cannot watch still gets a changed snapshot read again: the server compares its modification time whenever it has answered what was queued.
- `.inc` files that Composer's class map names are read at startup and when the project is read again. Their changes are not watched.

## Workspace edits

| Capability                                                     | Needed for                                                                                         |
| -------------------------------------------------------------- | -------------------------------------------------------------------------------------------------- |
| `workspace.workspaceEdit.documentChanges`                      | Every refactor that moves or makes a file, and versioned edits                                     |
| `workspace.workspaceEdit.resourceOperations` with `"rename"`   | Rename of a class that moves its file, and move class                                              |
| `workspace.workspaceEdit.resourceOperations` with `"create"`   | Extract interface, which writes the interface into a new file                                      |
| `workspace.workspaceEdit.snippetEditSupport`                   | The name a refactor writes as a snippet placeholder, instead of the `php.rename` command           |
| `textDocument.codeAction.resolveSupport` with `"edit"`         | Refactors whose edits are worked out on `codeAction/resolve`                                       |
| `workspace.fileOperations.willRename`                          | A moved or renamed PHP file or folder brings its namespace and references along                    |

Without `snippetEditSupport`, a code action that writes a name carries the command `php.rename` with the document and the position of the name. The client runs it after applying the edit by starting a rename there. The server answers `workspace/executeCommand` for it with `null`.

## Settings

Settings come from `initializationOptions`, `workspace/configuration` (section `phpLanguageServer`, with the document as `scopeUri`) and `workspace/didChangeConfiguration`. See [configuration](./configuration.md). A client that keeps a cache between runs passes `storagePath`. Without it the standard library stubs are not downloaded and the index of words is not kept on disk.

## SQL in strings

What the server answers for the SQL in strings comes through the same requests as the rest ([SQL in strings](./sql.md)); a client does nothing new for it to work. What it sees:

- **Diagnostics** of SQL have the source `sql` (PHP's have `php`), the SQL inspection's id as `code`, `data.feature` for a row of the SQL feature table, and `relatedInformation` in the same document.
- **Completion** inside SQL answers with SQL's items when there are any. `.` and a space are trigger characters as well; outside SQL they answer nothing, so they never open PHP's list. With `textDocument.completion.completionItem.snippetSupport` a function comes with its parentheses as a snippet; without it, plain text.
- **Semantic tokens**: the legend is PHP's types, then the ones only SQL has, in this order: `comment`, `string`, `number`, `operator`, `type`; the modifiers are PHP's. Inside a string of SQL the SQL's own tokens (keywords, names, strings, numbers, comments, operators and placeholders) stand where the editor's grammar would color one string; a PHP token over the same text gives way, tokens never overlap, and the interpolations keep PHP's tokens. A client that maps the legend by name needs nothing more; one with a fixed list of types should add these five.
- **Code actions** of SQL are `quickfix` and `refactor.rewrite`; `source.fixAll.sql` applies every safe fix of the document's SQL and comes when a client asks for `source.fixAll` (or that kind) in `only`.
- **Rename** inside SQL changes only names the string declares and answers an error for a table or column.

## Folding

A client that announces `textDocument.foldingRange.lineFoldingOnly: false` gets a run of `use` statements with `startCharacter` after the first `use` and `endCharacter` at the last `;`, so it can draw the folded run as `use …;`. Other folds and other clients fold whole lines.

## Progress and refreshes

- `window.workDoneProgress` shows indexing as `$/progress`.
- `workspace.semanticTokens.refreshSupport`, `workspace.inlayHint.refreshSupport` and `workspace.diagnostics.refreshSupport` let the server ask for new tokens, hints and pulled diagnostics once indexing is done, and when what the SQL in strings is read against changed (the `sql` setting, a snapshot, a `.sql` file).
- Diagnostics are pushed, or pulled with `textDocument/diagnostic` by a client that announces `textDocument.diagnostic`.

## Custom request

`php/runnables` with `{ "uri": "file:///project/tests/FooTest.php" }` lists what an open test file can run. See [features](./features.md#tests). Run markers also come as code lenses, so a client can use either.
