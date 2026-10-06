# php-language-server

A language server for PHP, written in Rust. It reads PHP 8.1 through 8.6 with a parser of its own that keeps every byte of a file, comments and whitespace included, and that goes on past a syntax error instead of stopping at it. The aim is the insight a full PHP IDE gives (navigation, completion, rename, inspections, refactors, frameworks and templates) as a server any editor can talk to over LSP.

This file says how the server works and why it works that way. What it was measured to do is in [MEASUREMENTS.md](./MEASUREMENTS.md), what a client has to send and announce is in [docs/clients.md](./docs/clients.md), and how the work came about is in the history of the repository.

## Where the code comes from

Everything here is written from scratch. The sources it is allowed to learn from are:

- the PHP language reference and grammar (php.net, the grammar in php-src) and php-src's own tests for the cases the grammar leaves open;
- the open source IntelliJ Platform (Apache 2.0) for general ideas such as lossless trees and error recovery;
- JetBrains/phpstorm-stubs (Apache 2.0), as a corpus the parser must read without errors and as the description of the standard library;
- PHPStan and Psalm (MIT) for type rules;
- watching what an installed IDE does with a piece of code.

Nothing was taken from the bytecode or the decompiled classes of any IDE plugin, and no code was copied from another product. Crates under MIT, Apache or BSD licenses are used freely. The corpora are fetched by a script into a folder git ignores, so none of that material is part of this repository and there is no NOTICE to carry.

## Layout

A Cargo workspace with five crates. Only the server knows LSP.

| Crate | Holds |
| --- | --- |
| `crates/syntax` (`php-syntax`) | The lexer, the parser, the tree (on `rowan`), the language level table and the pass that checks a tree against a level. |
| `crates/format` (`php-format`) | The formatter: it reads a tree and decides the whitespace between tokens, and nothing else, and reads `.editorconfig`. |
| `crates/index` (`php-index`) | The declarations of a file with their PHPDoc, name resolution, PHPDoc types, Composer metadata, the standard library stubs, the persistent cache, the parallel indexer, the class hierarchy, the word index, the test facts (groups, Pest datasets, bindings of a test case to a folder) and the framework layer (`framework/`). |
| `crates/analysis` (`php-analysis`) | Questions about a tree and an index: symbols, folding, selection, diagnostics, the type layer, hover, navigation, completion, usages, rename, signature help, hierarchies, semantic tokens, inlay hints, inspections, fixes, refactors, tests and runnables, the strings of the frameworks (`frameworks/`), Blade (`blade/`), Twig (`twig/`), YAML (`yaml.rs`) and the class names in strings (`class_strings.rs`). |
| `crates/server` (`php-language-server`) | The LSP front end over stdio: documents, incremental sync, workspace folders, background indexing with progress, configuration, and the conversion of everything above to LSP. Library and binary. |

## Syntax

### Lexer and parser

- `lexer.rs` is a mode stack that mirrors how PHP tokenizes: inline HTML, `<?php` and `<?=`, interpolated strings with `$a[0]`, `$a->b`, `{$...}` and `${...}`, heredocs and nowdocs with flexible closing markers, casts, numbers with separators and every base, names as single tokens (`Foo\Bar`, `\Foo`, `namespace\Foo`), `__halt_compiler`. Every byte lands in exactly one token. The state is a small value that can be cloned: `lex_with_checkpoints` records it at the first token of every line and `lex_from` resumes from one.
- `parser/` is recursive descent with a Pratt parser for expressions, over the union of PHP 8.1 to 8.6 syntax: property hooks, asymmetric visibility, `new` without parentheses in a chain, the pipe operator, `clone` with arguments, the `(void)` cast, closures and first-class callables in constant expressions, attributes everywhere they are allowed, alternative syntax, enums, and partial function application (`str_replace('a', ?, ...)`: `?`, `name: ?` and a `...` that spreads nothing, in a call, a `new` and `clone(?)`). It never looks at a language level.
- `kind.rs` holds the one enum of token and node kinds. `dump.rs` prints a tree for tests and for looking at what the parser made.

The shape of the tree:

- A node starts at its first token and ends at its last, so trivia sit between nodes. The exception is a declaration, which also owns the doc comment right before it and its attributes, so `/** ... */ final class A {}` is one node.
- Names are single tokens wrapped in a `NAME` node, for declarations and references alike. `A::$b` is a `STATIC_PROPERTY_EXPR`, `A::B` and `A::b()` are a `SCOPED_ACCESS_EXPR`, which is what tells an assignable target from one that is not.
- A `?>` ends a statement the way a `;` does, and a file may swap between PHP and inline HTML anywhere, including in the middle of `if (...): ?> ... <?php endif; ?>`.

The one construct of older PHP the parser does not read is the removed `$a{0}` offset syntax.

### Error recovery

The parser never fails and never panics: every input gives a tree whose text equals the input, with `ERROR` nodes around what it could not read and a list of errors with ranges. A missing token is reported without being consumed, a list stops at a token that cannot belong to it (`;`, `{`, `}`), a statement list stops at a token that closes something outside it, and a class body picks up again at the next token that starts a member. A `public` or `private` found inside a method body ends that body, because it means a `}` went missing above it. Nesting is limited to 200 levels, past which the parser reports it and goes on, so a hostile file cannot overflow the stack.

Messages say what is missing in the form of an IDE (`';' expected`, `Expression expected`). An empty range, which is where something is missing, is widened by the server to the character before it so a client has something to draw.

Against php-src's tests the parser rejects no file PHP accepts, except `Zend/tests/stack_limit/*.phpt`, which nest tens of thousands of operators and hit the nesting limit. It accepts a few files PHP rejects, all for reasons that are semantic rather than lexical: the removed `(real)` cast (which the language level pass reports), `list(...)` used as a value, `const` inside a function, `?static` as a property type, and `(void)` as the only expression of a `for` condition.

### Language level

The lexer and the parser accept all supported syntax and never reject it by version. What a file may use is a separate question, answered by `PhpVersion` and one table.

- `PhpVersion { major, minor }` is ordered and parses `8.4`, `8.4.2` and constraints such as `^8.1`. `LATEST` is the newest version the parser knows (8.6); `DEFAULT` is the level of a file nothing configures (8.5), the newest release, until 8.6 is out in November 2026 and the two meet again. A project on 8.5 that writes 8.6 syntax by mistake then sees an error instead of silence.
- `FEATURES` in `language_level.rs` has one row per piece of syntax: its id, the name it has in a message, the version that introduced it, deprecated it and removed it, the node or token kinds to look at, a function that finds the construct, and an example. The rows cover 8.0 to 8.6; the 8.6 rows are partial function application, a default value of a readonly property (or of any property of a readonly class), a write to a property of an object in a constant (`FOO->bar = 1`), `#[\Override]` on a class constant or an enum case, and `__debugInfo()` on an enum.
- `check_language_level(tree, version)` walks the tree once and reports syntax newer than the level as an error (`Property hooks are only available since PHP 8.4`), syntax removed by then as an error (`The (real) cast was removed in PHP 8.0`) and deprecated syntax as a warning with the deprecated tag (`The backtick operator is deprecated since PHP 8.5`), each ranged on the construct.

Supporting a new version means adding rows to the table, plus the syntax to the parser when there is any. No version check sits anywhere else. A test parses the example of every row and holds the row against it: reported just below its `since`, accepted at it, and the same for deprecations and removals.

## The server

`php-language-server --stdio` (the flag is the default) speaks LSP over stdin and stdout. It negotiates the position encoding (UTF-8 when the client offers it, else UTF-16) and syncs documents incrementally. A change is applied to the text and the whole file is parsed again, which is cheaper than anything a patch could save; the server also answers every message already queued before it publishes diagnostics, so a burst of keystrokes costs one parse.

Besides the requests the sections below describe, it answers:

- diagnostics: syntax errors, language level findings and inspections, pushed with `textDocument/publishDiagnostics` after a burst of changes has settled, or pulled with `textDocument/diagnostic` by a client that announces it (then nothing is pushed);
- `textDocument/documentSymbol`: namespaces, classes, interfaces, traits, enums and their members, functions and constants, with signatures as detail and the deprecated tag from `@deprecated`. A client that cannot nest gets the flat form. Declarations behind `if (!function_exists(...))` count;
- `textDocument/foldingRange`: bodies, arrays, `match` and `switch`, property hooks, attribute lists, heredocs, alternative syntax, multi-line and consecutive line comments (`comment`), runs of `use` statements (`imports`), `// region` and `// endregion` (`region`) and PHP tags between markup. A fold ends on the line before a closing bracket that starts its line, so the bracket stays visible;
- `textDocument/selectionRange`, growing from the token through every enclosing node to the file.

### Configuration

Only standard LSP channels are used: `initializationOptions`, `workspace/configuration` for the section `phpLanguageServer` with the document as `scopeUri` (so a client can answer per project or folder), and `workspace/didChangeConfiguration`, each with the settings bare or under `phpLanguageServer`. The settings are `phpVersion`, `storagePath`, `stubsPath`, `inlayHints`, `inspections`, `format` and `usages.packages`, described in [docs/configuration.md](./docs/configuration.md). The level of a document is the answer for its scope, else the project's, else the default.

### Documents

A document is read by what it is: PHP, a Blade template (language id `blade` or a path ending in `.blade.php`), a Twig template (`twig` or `.twig`) or YAML (`yaml`, `.yaml`, `.yml`; only the configuration of a Symfony project means anything). Open documents win over the disk everywhere: in the index, in the words a search reads, and in what the framework layer reads.

## The index

### What it holds

`php-index` reads every file into a `FileSymbols`: classes, interfaces, traits and enums with their cases, constants, properties (promoted ones and ones with hooks and asymmetric visibility included) and methods, plus functions and constants (`const` and `define()`). A declaration carries its signature, modifiers, attributes, location, its PHPDoc (summary and description, `@param`, `@return`, `@var`, `@throws`, `@deprecated`, `@template` with bounds and defaults, `@extends`, `@implements`, `@use`, `@mixin`, `@property*`, `@method`, `@see`, the asserts, and the type aliases and their imports) and its types with every class name already resolved, so a cached entry needs nothing of the file it came from.

### Sources

- The project's own files: everything under the folder except `vendor/`, `node_modules`, dot folders, folders starting with `~` (backups) and generated caches.
- The installed packages: only the files Composer would load, from the `autoload` of `vendor/composer/installed.json`.
- The files Composer loads that a walk leaves out: the project's own `files` and `classmap` entries in a skipped folder, and every file `vendor/composer/autoload_classmap.php` names whatever its extension, so a class in a `.inc` file is known. Those are read at startup and when the project is read again; their changes are not watched.
- The standard library stubs.

A project file wins over a package, and a package over the stubs. A class the index has not seen is looked up through Composer and read on demand: the generated class map first, then the PSR-4 and PSR-0 maps. With `config.classmap-authoritative` the lookup stops at the class map, as the autoloader does, so `undefined-class` does not lean on a path Composer would not try.

Files are extracted on a rayon pool and reach the server in batches, which it reports as `$/progress`. `workspace/didChangeWatchedFiles` (registered dynamically for PHP files, the Composer files and what the framework layer reads) updates single files, and a change to `composer.json`, `installed.json` or `autoload_classmap.php` reads the project again. Workspace folders can come and go.

A workspace folder that has no `composer.json` of its own (a repository with `backend/`, `frontend/` and `shop/` next to each other) is searched up to four folders deep for `composer.json` files, skipping `vendor`, `node_modules`, dot folders and `~` folders. Each one found is a project of its own, with its own packages, autoload maps, language level, stub extensions and cache file, and a file belongs to the project with the deepest root above it, so `backend/` only sees what `backend/` installs. The folder's own project reads the files that are in no Composer project. A folder that has a `composer.json` is one project, whatever lies below it, which keeps a monorepo of packages visible to itself. `workspace/symbol` asks every project and lists a package or standard library symbol once.

### The cache and memory

One file per project under `<storagePath>/cache/project-<key>.bin`, with a header, an index and the declarations of every file one after the other. The index has, per file, its path, its stamp (size, modification time) and content hash, the place of its declarations and a summary: the names of its classes (with kind, abstract and deprecated flags, the level they exist at and the classes they extend, implement or use), functions and constants. A file whose stamp matches is not read; one that was only touched is read and hashed but not parsed. The cache is dropped when the sources that decide what an entry means change, since a hash of them is in the header.

A warm start reads only the index, which is what the server needs to find a class by name, list names for completion and workspace symbols, and know what stands below a class. The declarations of a file are read from the cache the first time something asks and stay in memory while they are in use; the server lets go of the ones used longest ago when more than 1,500 are loaded (checked every three seconds) and reads them again when asked. Open files, files that changed since the last run, and projects without a storage folder keep their declarations in memory. On the first run declarations are not kept at all: a file is read again when asked until the cache file is written. A class found at a place that no longer holds it is not returned.

Open documents keep their syntax tree and no other tree is kept. Strings are not interned: the declarations that stay resident are a few names per class.

### The standard library stubs

The stubs are JetBrains/phpstorm-stubs, not in this repository and not in the binary. On first run the server downloads the tarball of one pinned commit (`STUBS_COMMIT` in `crates/index/src/stubs.rs`) into `<storagePath>/stubs/<commit>/`, unpacks only the `.php` files and the `LICENSE`, and marks the folder complete once it is whole, so an interrupted download starts over. The download runs in the background; until it is done the standard library is not offered, and a failure is logged and leaves the rest working. `stubsPath` points at a folder that already exists, which is what the tests and `scripts/fetch-corpus.sh` use.

A project sees the stub folders of a default set of extensions (core, standard, SPL, date, json, pcre, mbstring, curl, PDO, intl and the like) plus every `ext-*` its `composer.json` requires.

### The language level of a project

`config.platform.php` of its `composer.json`, else the lower bound of `require.php` (`^8.1 || ^8.2`, `>=8.1 <8.4`, `~8.1.0`, `8.3.*` and hyphen ranges are read), else the `phpVersion` the client configured, else the default. The index filters the stubs by it: `@since` and `@removed`, `#[PhpStormStubsElementAvailable(from:, to:)]` on functions, methods, classes and parameters, and `#[LanguageLevelTypeAware([...], default:)]` on types. The `@since` of a project's own doc comments means nothing and is ignored. A level from `composer.json` also decides the syntax diagnostics of that project's files.

### Words

A search reads only the files that may hold a name. `words.rs` keeps, per file, the sorted hashes of the words in it, and also of every run of identifier characters joined by `.`, `-`, `:` or `/` (`admin.users.index`, `mail::welcome`) with each tail of it that starts at a word (the `articles.summary` of `<x-articles.summary>`), so a search for a route name reads only the files that write the whole string; a name with spaces needs all its words. PHP files, Blade and Twig templates and the YAML of the configuration are all in it.

The words are built the first time something asks, kept current from then on, and kept in `<storagePath>/cache/words-<project>.bin` with the stamp of each file; the first search after a start reads that file and only the files whose stamp changed. With `usages.packages` the files Composer loads from `vendor/` get a second word index, built on the first search that asks and kept in `package-words-<project>.bin`. Nothing resolved is stored: a search resolves what it reads against the index at that moment, so a result is never stale.

## Types

### Names and the hierarchy

`NameResolver` holds the namespace and the `use` statements (classes, functions and constants, grouped, aliased) at a point of a file. Class names resolve through the imports and the namespace, relative `namespace\Name` and qualified names included; functions and constants try the namespace and then the global namespace. `self`, `static` and `parent` follow the class around the cursor; in an anonymous class they are that class, whose own members the index does not hold.

`Index::ancestors` walks a class, its traits (with their `insteadof` and `as` rules), its parents, its interfaces and its `@mixin`s in the order PHP looks members up, carrying the template arguments of `@extends Base<Foo>` down the chain. A member is looked up by name without building the list of every member first. Members that exist only at run time come from the framework layer at the same point, so every feature sees them.

### The type layer

`php-analysis` infers what a variable or an expression is at a cursor. It follows:

- declared types (unions, intersections, nullable, `static`, `self`) and PHPDoc types: `array<K, V>`, `list<T>`, `T[]`, array shapes, `class-string<T>`, `callable(A): R` and `Closure(A): R`, literals, `int<..>` and the `non-empty-string` family, and conditional types such as `($flag is true ? int : string)`;
- type aliases: `@psalm-type Name = Type` and `@phpstan-type Name Type` on a class, read in order so an alias may use one above it, and read in place by the class's own docs; `@psalm-import-type Name from Class as Alias` and `@phpstan-import-type` make the name a template of the class that the hierarchy binds to the alias of the class it comes from (`Index::type_aliases`, following imports four deep), and the parameters and inline `@var` of a method body read both kinds directly. A plain `@type` means other things in other conventions and is left alone;
- generics: `@template` on classes, methods and functions, bound from the receiver (`Collection<int, User>`) and from the arguments of a call (`make(User::class)` is a `User`, `identity($x)` the type of `$x`), also for `new`. A method without a `@return` of its own (a trait method with only `{@inheritdoc}`, say) takes the one of the method it overrides or implements, read with the template arguments the receiver gives that ancestor, when it says no less than the native return type;
- variables: parameters (with `@param` types, defaults of `null`, variadics), assignments, `new`, calls, property reads, constants and enum cases, array literals and `$a[] = x`, nested array writes, indexing, destructuring and `foreach` over arrays, iterables, `Iterator` and `IteratorAggregate`, `catch`, `static` and `global` variables, closures with `use`, arrow functions and inline `/** @var T $x */`;
- flow: branches merge into unions, a branch that ends in `return` or `throw` does not, `instanceof`, `=== null`, `isset`, `is_*` functions, truthiness and `assert` narrow in the branch they hold for and survive an early exit, and `@psalm-assert`, `@phpstan-assert` and their `-if-true` and `-if-false` forms (with `!Type` and `=Type`) narrow the argument a call names. A call of a function declared `never` ends the flow; one only documented as `never` does not;
- closures: the parameter a closure takes is typed from the `callable(T): U` the callee asks for, with `T` bound from the other arguments and the receiver, and the closure's return type binds `U`. The standard library stubs have no generics, so `array_map`, `array_filter`, `array_reduce`, `usort` and 27 more get theirs from `crates/index/src/stub_overlay.php`;
- return types read from bodies: a function, method or closure with no declared or documented return type returns what its `return` statements return (a generator is a `Generator<K, V, mixed, R>` from its `yield`s), when all of them are known; a body in another file is read from disk, three bodies deep at most;
- first-class callables and partial applications are a `Closure`, not a call.

It stops at loops that need a fixed point, references, variable variables and dynamic member names, `__get` and `__call`, and `key-of`, `value-of` and the other type-level functions. Where it gives up the type is `mixed` and nothing is offered or reported on it.

## Navigation and editing

### Hover, definition and symbols

- **Hover** is a title line with the qualified name, a fenced `php` signature (parameters with their types and defaults, modifiers, the return type), the PHPDoc as markdown (summary, description, one `_@tag_` line per tag, the HTML some doc comments use turned into markdown, a method without a doc inheriting the one above it) and the file it is defined in. It works on classes, functions, constants, members, enum cases, variables (with their inferred type) and type aliases.
- **Definition** goes to the name of a class, function, constant, method, property or class constant, to the first place of a variable, and into the stubs as files in the storage folder. **Type definition** goes to the class of a variable, a property or what a method returns. **Implementation** lists the subclasses and implementors of a class, the overrides of a method, and the handlers of a Messenger message.
- Names in doc comments lead somewhere too: a class in a type, `Class::member()` and `function()` after `@see`, the names `@property` and `@method` declare, and a type alias, which goes to its `@psalm-type` line.
- A string literal whose text is a qualified name of a class the index knows (`'App\Models\User'`, `'\\App\\Models\\User'`) names that class, and `'App\Http\Home::show'` and `'App\Http\Home@show'` name its method (`class_strings.rs`). A short name without a namespace stays a word, an unknown name is left alone, and a double-quoted string with a variable in it is not read.
- **Workspace symbols** match classes, interfaces, traits, enums, functions and constants everywhere, and members of the project's own classes, by prefix, camel humps (`uc` finds `UserController`) and substring, project first.

### Completion

The word being typed is replaced by a placeholder before the text is parsed again, so an unfinished `$user->` is a whole expression to the parser and its tree says which kind of name belongs there. Completion handles:

- members after `->` and `?->` (by visibility from the class around the cursor) and after `::` (static methods, constants, enum cases, static properties as `$name`, `class`; after `parent::`, `self::` and `static::` the instance methods too);
- classes in expressions, `new` (instantiable ones, with the constructor's parameters as detail), `extends` (not final classes), `implements` (interfaces), trait `use`, `catch` (throwables), `instanceof`, type positions (with the built-in types) and `#[` (classes marked `#[Attribute]`);
- functions and constants, variables in scope with their types (and superglobals), named arguments of the callee, enum cases, and keywords by place;
- `use` statements: namespace segments, classes, `use function` and `use const`;
- after `function ` in a class body, the methods of the parents, interfaces and abstract traits that the class does not declare yet and that are neither private nor final, with what must be implemented first. The inserted text is the whole method: signature, return type and a body in the indentation of the line (`return parent::name($a);` or empty), with the `use` lines the signature needs;
- inside a string that starts a qualified name (`'App\Mo|'`), the classes below it, written with doubled backslashes when the string doubles them;
- the strings frameworks and tests give meaning to, described in their sections.

A class that needs an import carries an `additionalTextEdits` entry that inserts the `use` line in sorted order in the right block: the block that shares the root namespace, else the one it sorts into, after an existing group at its end, and for a file without imports after the `namespace` or `declare` line with a blank line around it. A class of the current namespace or one already imported needs nothing, and one whose short name is taken is written in full. Items have a `kind`, the namespace (or the class of a member) as description, the signature as detail, a deprecated tag, and `completionItem/resolve` adds the documentation hover shows. No call snippets are inserted. With nothing typed only variables, keywords and members are offered; at most 300 items come back and `isIncomplete` says when there were more. Comments, plain strings and inline HTML get nothing.

### Usages and highlights

A name in a file resolves to a `Symbol` through the same resolution and type layer hover uses: a class, function or constant by its qualified name, a method, property or class constant by the class that declares it, a parameter by the function that owns it, a local variable by the function or file it lives in, and a key (a route, a config key, a column and the like) by its kind, name and scope. Several declarations are one symbol when they are one thing: a method is one with the methods it overrides and implements, in both directions, and a property or class constant works the same way. A private member and a constructor are their own. A promoted constructor parameter is a variable, a parameter and a property at once.

What counts as a usage: a class in every type and expression position, `use` statements and group uses, functions and constants, methods (also through `static::`, `self::`, `parent::`, first-class callables and `?->`), properties (also as `Foo::$name`), class constants and enum cases, variables with their closures and arrow functions, parameters through their named arguments, PHPDoc (the classes of every tag, `@param` and `@var` variables, `@property` and `@method`, `@template` names, `@see` targets), the strings that hold a class name, the strings frameworks give meaning to (a relation segment counts for its method, a DQL field for its property, a `wire:` attribute for the member of its component), and the same names in Blade, Twig and YAML. A constructor also counts the `new` that calls it, also for a class below it without a constructor of its own. A class imported under an alias counts where the alias is written. A hit says what it is: a declaration, a reference, an import, a name in a doc comment, or a name in a string.

Counting follows an IDE's "N usages": the declaration itself is not a usage (a client asks with `includeDeclaration: false`), an import is one, a name in a doc comment is one, and a call through an interface or a parent counts for each method of the family. A search covers the project's own files, and the installed packages with `usages.packages`; rename and the refactors never look at packages. A usage whose receiver the type layer cannot name, `$$name`, `[$this, 'run']` and `compact('x')` are not found, and neither is a usage in a file that is not on disk and not open.

`textDocument/documentHighlight` marks the same places inside the open file, with read and write for variables, properties and class constants (an assignment, a compound assignment, `++`, `unset`, a `foreach` target, a destructuring and a by-reference `use` are writes).

### Rename

`prepareRename` answers the range and the name under the cursor, or says why not: a keyword, a name declared in a package or the standard library, a magic method, a name the index does not know, or a key (a route, a config key; their declarations are elsewhere and of another kind). `rename` returns a workspace edit, as `documentChanges` when the client takes them. It renames:

- classes, interfaces, traits and enums, with their imports (the imported name only, so an alias stays), the names in doc comments, attributes and strings that hold the class name. When the file carries the class's name, holds only that class, and Composer's map agrees that the class lives there, the file is renamed too, as a `RenameFile` after the text edits, when the client announces resource operations;
- methods with their overrides, implementations, callers and the strings that name them (`'App\Home::show'`, relation strings, `wire:` attributes), properties with their `@property` lines and the DQL that names them, promoted parameters and named arguments, class constants and enum cases, functions and constants with their imports;
- variables and parameters inside their scope, with `@param` and inline `@var` and the named arguments at the call sites, in PHP and in a template;
- a namespace, from its `namespace` statement: every file with that exact namespace, every `use` of a name in it, group use prefixes and fully qualified names. Names written relative to another namespace, sub-namespaces and Composer's `autoload` section are left.

A string in a file under a `migrations` folder is not renamed: it records what a database holds, which a rename of the class does not change. Ordinary comments and other strings are not changed either.

A new name is checked: an identifier (a namespace may have backslashes), not a reserved word, type name or keyword for a class, function or constant, not `class` for a class constant, not `this` or a superglobal for a variable. A name that is taken is refused with the place: a member of the same name anywhere in the hierarchy, a class or function of that name, a variable in the same scope, a name another class of the same file already imports.

### Signature help

`textDocument/signatureHelp` finds the argument list around the cursor (also one that is not closed yet), resolves the callee (functions, methods, constructors with `new`, attributes, closures with a documented signature) and answers each way it can be called: a function the stubs declare more than once has one signature per declaration at the project's level, and variants spelled with `#[PhpStormStubsElementAvailable]` are one signature at that level. The active signature is the first that fits the arguments typed so far, the active parameter follows the commas, a variadic parameter takes every extra argument, and a named argument selects its parameter.

### Call and type hierarchy

Incoming calls are the usages of a function or method grouped by the function they sit in, code outside any function being one item for the file. Outgoing calls walk the body, resolve every call and `new`, and group them by callee. A closure's calls belong to the function around it. The type hierarchy gives a class's parent, interfaces and traits as supertypes and the classes that extend, implement or use it as subtypes. `lsp-types` has no field for the capability, so `typeHierarchyProvider` is added to the `initialize` answer by hand.

### Semantic tokens

`textDocument/semanticTokens/full` and `/range`. Keywords, strings, numbers and comments are left to the editor's grammar.

| Types | `namespace`, `class`, `interface`, `enum`, `struct` (traits), `typeParameter` (`@template` names), `parameter`, `variable`, `property`, `enumMember`, `function`, `method`, `keyword` (doc tags), `decorator` (the name of an attribute) |
| --- | --- |
| Modifiers | `declaration`, `readonly`, `static`, `deprecated`, `abstract`, `defaultLibrary` (the standard library), `documentation` (inside a doc comment) |

Constants are `variable` with `readonly`, class constants `property` with `readonly` and `static`. A qualified name is a `namespace` token for its prefix and the type for its last segment, except the name of an attribute, which is one `decorator` token. The server asks the client to refresh the tokens when the index changed.

### Inlay hints

- **Parameter names** in front of positional arguments, left out when the argument already says it: a variable, property, constant or call named like the parameter (`$user_id` for `$userId`, `getName()`), a one-letter or underscore parameter, a function of one argument unless the argument is a bare `true`, `false` or `null`, an argument after a named or spread one, and the arguments of a variadic parameter. Accepting a hint inserts `name: `.
- **Closure parameter types** for a closure or arrow function whose parameter has no type, from the `callable(User): bool` the callee declares for it, with the templates of the call bound. A type made of built-in types only can be accepted.
- **Property types**: the type arguments a framework gives a property that has no `@var` (the target of a Raxos to-many relation or a Doctrine collection), after its declared type. They cannot be accepted, since PHP has no generic types.

## Inspections and fixes

### Inspections

Every inspection has a code, a default severity and a default state, and can be switched off or given another severity with the `inspections` setting.

| Group | Codes |
| --- | --- |
| Names | `undefined-class`, `undefined-function`, `undefined-constant`, `undefined-class-constant`, `undefined-method`, `undefined-property`, `undefined-variable`, `undefined-named-argument`, `this-in-static-context` |
| Unused code | `unused-import`, `unused-private-method`, `unused-private-property`, `unused-private-constant`, `unused-variable`, `unused-parameter`, `unreachable-code` |
| Calls and types | `wrong-argument-count`, `argument-type-mismatch`, `return-type-mismatch`, `missing-return`, `incompatible-comparison`, `assignment-in-condition`, `deprecated`, `static-call-of-instance-method`, `instance-call-of-static-method` |
| Classes | `abstract-method-not-implemented`, `interface-method-not-implemented`, `incompatible-override`, `readonly-reassigned`, `enum-misuse` |
| PHPDoc and style | `phpdoc-unknown-parameter`, `phpdoc-type-mismatch`, `missing-strict-types` (off by default) |
| Tests | `missing-data-provider`, `missing-test-dependency`, `data-provider-arity`, `missing-double-method`, `double-return-type-mismatch` |
| Frameworks | `unknown-config-key`, `unknown-route`, `unknown-view`, `unknown-template`, `unknown-translation`, `unknown-relation`, `unknown-validation-rule`, `unknown-cast`, `unknown-entity-field`, `unknown-inertia-page`, `unknown-feature`, `unknown-serializer-group`, `unknown-workflow-name`, `unknown-model-key`, `model-method-mismatch`, `redundant-model-method`, `invalid-model-attribute`, `unknown-route-parameter`, `route-without-return-type`, `message-handler-mismatch` |
| Templates | `unbalanced-directive`, `unknown-twig-function`, `unknown-twig-filter`, `unknown-twig-test` |

An inspection reports only what is certain, and stays silent where it cannot be. A name is undefined only after the project, its packages and the standard library of the extensions it requires have all been read. A call is not checked when it reaches its callee through `__call` (how a method lent by a `@mixin`, a scope or an `@method` tag is called), neither for being static nor for its arguments; a partial application is not checked for its arguments. A function only documented as `never` (Laravel's `abort()`) keeps `missing-return` silent and does not make the code after it unreachable. `survey` runs every inspection over a project to find the ones that are wrong on code that is right; [MEASUREMENTS.md](./MEASUREMENTS.md) has what it found.

### Quick fixes and code actions

`textDocument/codeAction` offers, with the edits sent along or resolved on demand:

- quick fixes: import a class, function or constant, or write its name in full; remove an unused import, declaration or variable (keeping the call when the assignment has an effect); remove unreachable code; `=` to `===` in a condition; remove a `@param` of a parameter that does not exist; implement the missing methods of an abstract class or interface; create a missing method or property; add `declare(strict_types=1)`; change the visibility a member needs for an override;
- intentions: add or update a PHPDoc, add a return type from what the body returns, change the visibility of a member, make a property readonly, convert a constructor to property promotion;
- `source.organizeImports`.

`textDocument/onTypeFormatting` also writes the doc block after `/**` and Enter, with a `@param` per parameter and the `@return`.

## Formatting

`textDocument/formatting`, `rangeFormatting` and `onTypeFormatting` (on `}`, `;` and a new line) are answered by `php-format`, with PER Coding Style 2.0 as the default.

The formatter moves whitespace and nothing else. It decides the text between two tokens with a rule per pair of neighbors, writes the indentation from the brackets and unfinished statements around a line, and keeps the line breaks the code has where PER has no opinion (arguments, chains, array items), with at most one blank line in a row. From that follows:

- **Lossless.** Comments, strings and heredocs are never touched, apart from the indentation of the later lines of a block comment. Before an edit is sent the result is parsed again and compared with the original token by token, and a text whose tokens would differ is left as it is.
- **Idempotent.** The second pass changes nothing, because every decision comes from the tree and the line breaks that are there, never from the columns of the old layout.
- **Safe on what it cannot read.** A text with syntax errors or with markup around its PHP is not formatted. `onTypeFormatting` does lay out unfinished code, since that is when it is asked.
- **Range formatting follows the whole file.** The layout is worked out for the whole text and only the edits in the lines asked for are sent.

PER's decisions are applied: one blank line between header blocks and between top-level declarations, methods separated by a blank line, the brace of a class or method on its own line, `{}` for an empty body, and `) {` together after a parameter list that runs over several lines. A blank line at the start or end of a class body is kept; one at the start or end of a function or control structure body is removed. The spacing of casts is left as written, since PER does not say.

The `format` setting has `classBrace`, `functionBrace`, `blankLinesBetweenMembers`, `alignAssignments`, `alignArrayArrows`, `lineLength` (default 120, `0` never wraps) and `editorconfig`. A line past `lineLength` is wrapped at the outermost construct on it that crosses the limit, until nothing more can be broken: the operands of a long binary chain (before each operator), the two sides of a ternary, a method chain of at least two calls (before every `->` but the first when it starts at a variable), the items of an array literal, and the arguments or parameters of a call or a function. A broken `if`, `elseif` or `while` condition goes on lines of its own between its parentheses. Nothing is added to the tokens, and a construct that already runs over several lines is left as its author broke it.

The `.editorconfig` files from the folder of a file up to the first with `root = true` are read with the globs of the specification, the nearer file winning, and `indent_style`, `indent_size`, `tab_width` and `max_line_length` are laid over the options. The client always sends a tab size and `insertSpaces`, which says nothing of what a project wants, so `.editorconfig` goes before them; explicit `format` settings go before `.editorconfig`. Code a refactor writes follows the same options, with the indent the file already uses ahead of the client's.

## Refactors

### How they are offered

Every refactor is a code action of kind `refactor.extract`, `refactor.inline`, `refactor.rewrite` or `refactor.move`, offered only where it applies. The edits are worked out over the lossless tree and then formatted: the text of each file is written with the refactor applied, `php-format` lays out the lines the edits touched (and only those), and what reaches the client is the few edits that lead from the old text to the new one. Comments and the layout of everything else stay. `use` statements the change leaves with nothing to import are removed, and only those. The expensive ones (everything but the rewrites) are resolved by `codeAction/resolve`, where a refusal comes back as an error with its reason; a client that cannot resolve gets the edits at once and the refactors that refuse are left out. A refactor across files returns `documentChanges`, with `rename` operations for a file that moves and `create` operations for a file it makes.

A refactor that writes a name the person is expected to change inserts a name from the expression and its type (`getUser()` is `$user`, `new DateTime()` is `$dateTime`; a taken name gets a number). With `snippetEditSupport` the name is a snippet placeholder; otherwise the action carries a `php.rename` command with the position of the name after the edits, which the client runs (the server answers `workspace/executeCommand` for it with `null`).

### The refactors

**Extract variable.** An expression becomes a variable assigned right before its statement. Without a selection the outermost expression worth a name is taken, with the inner ones offered next to it, and with two or more equal expressions in reach a second action replaces them all. Refused when the expression runs only sometimes (right of `&&`, `||`, `??`, `and`, `or`, one side of a ternary or a `match`, `elseif` and loop conditions, `case` labels), sits in an arrow function, inside `isset`, `empty` or `unset`, after `@`, on the left of `??`, in a string's simple interpolation, in a place that only takes constants, under a body without braces, is written to or called, or when something with an effect runs earlier in the statement and the expression is not constant or made of local variables nothing earlier touches. The occurrences a second action replaces must be pure, in the same block after the first, and not follow a write to what the expression reads.

**Inline variable.** A local assigned once, in a statement of its own, is replaced by its value in every read, and the assignment goes; parentheses are added by precedence. Refused for parameters, variables written elsewhere (`$x[] = 1`, `$x->p = 1`, `++`, by reference), variables used by name (`compact`, `extract`, `$$x`), reads before or outside the block of the assignment, a read in a closure's `use`, a value that does something when it would be copied, moved past other effects, past a `return` or `break`, into a condition that runs sometimes or into a closure, a value whose inputs change before a read, and a value that does not fit its place (`isset($x)`, the inside of a string).

**Extract method.** One expression or a run of whole statements becomes a private method after the current one (a function when cut from a function). What the code reads and has before it becomes the parameters, typed in the file's own style with the types PHP guarantees; what it sets and is read afterwards is returned, as a list when it is more than one; `$this` and static context follow the original; a run that always returns is returned from; a run that only throws is `never`; `yield` becomes `yield from`; by-reference parameters stay by reference. A doc block is written when the method it was cut from has one or most methods of its class do. Refused in a closure or an interface, for code that binds references, declares things, uses `global`, `static` or `goto`, reads variables by name, leaves a loop it does not contain, returns on some paths only or with a value and without, yields and hands values back, or changes variables read afterwards in an expression.

**Extract constant, field and parameter.** A constant expression becomes a class constant (`private`, `public` in an interface), with a second action for every equal expression of the class. A field is `private` and typed, initialized inline when the expression is constant, else in the constructor (added when there is none); static in a static method. A parameter is added to the function and every override, the expression is replaced and every call gets the argument (named when a call leaves out arguments before it); after optional parameters it takes the expression as its default. Refused: a field or parameter that needs a variable the constructor or the call does not have, `$this` or `self` in a parameter, an expression with an effect in a loop or that runs sometimes, a variadic function, a declaration in `vendor/`, calls that spread their arguments, and uses that are not calls.

**Inline method.** One call, or every call and the method with them. A body of one `return` goes anywhere the call stands; a few statements (up to ten, ending in at most one `return`) go where the call is a statement of its own, parameters that cannot be used as they are become locals and clashing locals are renamed. A body that uses its object only goes to calls on `$this`, `self` or `static` from its own class; names copied to another file are written in full. Refused for generators, recursion, by-reference or variadic parameters, `static` and `global`, closures that use a parameter, early returns, a method overridden or implemented elsewhere, references that are not calls, calls inside the arguments of another, and declarations in `vendor/`.

**Change signature.** On a function or method, applied to every override, implementation and call that find usages sees, or not at all: add a parameter with a default, remove an unused parameter, move a parameter left or right. Refused when an override uses a removed parameter, a call passes something with an effect or spreads, a required parameter would follow an optional one, or a partial application names the function, since it binds arguments by position. Callable strings and arrays, declarations in `vendor/` and calls whose receiver the type layer does not know are not changed.

**Extract interface.** On the name of a class of the project that has public methods: `{Class}Interface` in a file of its own next to it, with the same namespace and `declare(strict_types=1)`, the `use` lines of the class's file that the signatures name, and every public method but the constructor and the other magic methods, with its doc comment, written as the class writes it up to the end of the return type. The class gets `implements` and the name is the focus, so the client starts a rename right away. The file is a `CreateFile`, which needs `create` among the client's resource operations. Refused: a name the index or the folder already has, a name the file imports for something else, and a method with a parameter typed `self` or `static`, which an interface reads as itself and the class could not narrow.

**Move class.** On a class name: the class moves to another namespace of its Composer root (the namespaces classes of the project already have, nearest first), by moving the file to the PSR-4 path and updating the `namespace` statement, every `use`, every qualified or fully qualified reference and every unqualified one that relied on the old namespace (an import is added, or the name is written in full where the short name is taken), the names inside the class that relied on it, and the strings that hold the class name, in their own spelling (outside `migrations` folders). Where a file and its namespace disagree, the file moves to the namespace's path or the namespace follows the file. `workspace/willRenameFiles` does the same when a client moves a PHP file or a folder. Refused: a taken target, a file with more than one class or namespace, a block namespace moved to the global one, a path no PSR-4 map reaches. YAML that names the class is left as it is.

**Pull up and push down.** Pull up moves a method or property into the parent (private becomes protected, names are imported in the parent's file); declare in an interface or abstract parent keeps the method and adds its signature. Push down moves a member into every direct subclass. Refused when the member needs something the target lacks, calls `parent::`, already exists there, is used anywhere but in the member or through `$this` below, is named by a template, the configuration or a string, or when a target is in `vendor/`.

**Rewrites** (`refactor.rewrite`, edits sent at once): `array()` and `[]`; `if`/`else` as a ternary and back; `switch` as `match` when every case is one `return`, `echo` or assignment, there is a `default`, and every label has the very type of a subject known to be an int, a string or an enum; concatenation as an interpolated string or `sprintf` and back; braces added to or removed from a body of one statement; split and joined declarations; a flipped comparison; an inverted `if`; named arguments given and given up.

`refactor_smoke` offers every refactor at a sample of positions of a real project and checks each result: every changed or created file parses no worse than before, and the inspections report nothing new (with moved and created files in the index first). Adding a parameter is expected to bring an unused parameter and is left out. Two results it made visible are limits: a dead assignment to a parameter in cut code shows up as an unused variable in the new method, and the parameter types of an extraction are only the ones PHP guarantees, since a wrong doc type would become a `TypeError`.

## Tests

### PHPUnit

A class is a test case when it extends `PHPUnit\Framework\TestCase` (without PHPUnit in the index, a parent named `TestCase`). A method is a test when it is public, not static, and named `test...`, marked `#[Test]` or `@test`.

The strings and tags that name something are followed like names:

| Written | Names |
| --- | --- |
| `#[DataProvider('name')]`, `@dataProvider name` | a method of the class |
| `#[DataProviderExternal(Foo::class, 'name')]`, `@dataProvider Foo::name` | a method of `Foo` |
| `#[Depends('name')]` and its clone forms, `@depends name` | a test of the class |
| `#[DependsExternal(Foo::class, 'name')]` and its clone forms | a test of `Foo` |
| `#[CoversMethod(Foo::class, 'name')]`, `#[UsesMethod(...)]` | a method of `Foo` |
| `#[CoversFunction('name')]`, `#[UsesFunction('name')]` | a function |
| `#[Group('name')]`, `@group name` | a group, kept per file in the index |

Definition, hover, usages, highlights and rename work on the text inside the quotes, and completion offers what fits there. A group has no declaration, so it completes and is not followed. `@covers` and `@uses` are not read. The inspections `missing-data-provider`, `missing-test-dependency` and `data-provider-arity` check them; arity reads only rows written out as arrays of positional values and stays silent for a test with `#[Depends]`.

Test doubles take their types from PHPUnit's own declarations (`createMock(Foo::class)` is `MockObject&Foo`, `getMockBuilder(Foo::class)->...->getMock()` through `MockBuilder<Foo>`). The method names of `->method('name')`, `->onlyMethods([...])` and `createPartialMock(Foo::class, [...])` are followed, completed and renamed; `missing-double-method` reports a name the class lacks, and `double-return-type-mismatch` a `willReturn()` value the method's return type never takes, when the value's type is certain. In a test, `$this->` completion puts the `assert...` methods first.

### Pest

A Pest file is calls: `test()`, `it()`, `describe()`, `beforeEach()`, `afterEach()`, `beforeAll()`, `afterAll()`, `dataset()`, `uses()`, `arch()` and `todo()`, recognized by name.

- **`$this` is the test case.** In the closure of a test or hook, `$this` is the class `uses(...)->in('Feature')` or `pest()->extend(...)->in('Feature')` binds to the file's folder (found in any file; the deepest folder wins), with the traits named there; `uses(...)` without `->in()` binds the file itself; else `PHPUnit\Framework\TestCase`. The folder of the file has to be known, so a library user wraps its calls in `php_analysis::document::enter(Some(path))`.
- **Properties of `beforeEach`.** What `$this->name = value` assigns in a `beforeEach` of the file or of the `describe` blocks around a test is a property of `$this` in the tests it runs for, with the union of its types. A property declared on the case class wins.
- **Datasets.** The name in `->with('name')` and the one `dataset('name', ...)` declares are one thing, with definition, usages, rename and completion. A test closure's untyped parameters take the types of an inline dataset or of a named one written out as an array or a closure returning one.
- **Expectations.** `expect($value)` and its chain follow Pest's own declarations. An expectation added with `expect()->extend('name', fn)` completes, hovers and leads to its string, and inside that closure `$this` is the expectation.

Inside a Pest closure `$this->name` and `$this->method()` are not checked for existence: the case class forwards through `__call`, and a property can come from a hook the server does not read.

### What a test file can run

The server runs nothing; it says what is runnable. `php/runnables` with `{ "uri": "..." }` (or `{ "textDocument": { "uri": ... } }`), for an open document, answers a list in file order:

```json
[
  {
    "kind": "phpunit",
    "scope": "method",
    "label": "FooTest::testIt",
    "range": { "start": { "line": 9, "character": 20 }, "end": { "line": 9, "character": 26 } },
    "filter": "/^Tests\\\\Unit\\\\FooTest::testIt( with data set .*)?$/",
    "file": "/project/tests/Unit/FooTest.php",
    "configFile": "/project/phpunit.xml"
  }
]
```

- `kind` is `phpunit`, `pest`, `artisan` or `console`; `scope` is `class`, `method`, `test`, `describe`, `arch` or `command`. A command (a class that extends Laravel's or Symfony's `Command` and names itself in `#[AsCommand]`, `$signature`, `$name` or `$defaultName`) has its name as `label` and `filter`, run as `php artisan <filter>` or `bin/console <filter>`.
- `range` is where a run marker goes.
- `filter` is a delimited regular expression for `--filter`. PHPUnit matches `Class::method with data set "x"`. Pest turns a description into a method name (`it does x` is `__pest_evaluable_it_does_x`, an underscore doubled, any other character that is no letter or digit an underscore, describe blocks prefixed as `` `outer` → ``), so its filter does not hold the class and is meant to run on `file`. An `arch()` without a description is named after its chained calls. A test whose description is not a string is not listed.
- `configFile` is the closest `phpunit.xml`, `phpunit.xml.dist` or `phpunit.dist.xml` up to the workspace folder, or `null`.

`textDocument/codeLens` answers the same list as lenses with the command `php.runTest` and the runnable as its argument, which the client registers and runs; the server does not list it in `executeCommandProvider`, since it cannot run it.

## Frameworks

### How the layer works

Nothing here activates in a project that does not install the framework. `Frameworks::detect` reads `composer.json` and the installed packages: `laravel/framework` or `illuminate/*` turn on the facades and Eloquent (and, with the framework, the conventions of an application), `symfony/framework-bundle` turns on Symfony, `doctrine/orm` the repositories, `twig/twig` Twig, `raxos/database` the Raxos ORM. A package such as Livewire, Inertia, Pennant or Filament is on when its base class is in the index.

What the frameworks declare themselves is read like any other code: docblocks, `@template` and `@extends`, attributes, the arrays of `config/`. What no declaration says is in two overlays, PHP files that are read and never run: `crates/index/src/framework/laravel_overlay.php` and `symfony_overlay.php`. The tags on a function or method are the data:

| Tag | Says |
| --- | --- |
| `@key kind [position\|$name\|*]` | the argument names something of that kind: `config`, `route`, `view`, `translation`, `env`, `ability`, `field`, `service`, `parameter`, `template`, `event`, `entity-field`, `section`, `stack`, `relation`, `column`, `livewire`, `inertia-page`, `feature`, `serializer-group`, `workflow`, `workflow-transition`, `workflow-place`, `filament-field`; `*` is every argument |
| `@container`, `@user`, `@repository` | the call hands out a container service, the logged in user, a repository |
| `@forwards A B` | the class passes what it lacks on to these |
| `@rules` | the argument is an array of validation rules |
| `@dql`, `@dql-part`, `@dql-from`, `@dql-join`, `@dql-alias` | DQL statements, parts of them, and the calls that give query aliases |
| `@class-argument`, `@return-keys` | an argument is a class of a base; the keys of a returned array name something |
| `@column`, `@columns`, `@morphs`, `@drops` | how a migration's Blueprint call changes a table |

`is_marked`, the cheap test before a call is resolved, only counts the markers of a framework the project has. A few base classes are named in code (`Model`, `Builder`, `Relation`, `Facade`, `EntityRepository`, `Livewire\Component`, Filament's `Resource`), since the magic hangs on them.

What the layer works out from the files of a project is kept in sections of the index: each is built the first time a question needs it and dropped when a file it read changes.

### Members that are made up

They are made up when a type is asked for its members (`framework/mod.rs`, called from `hierarchy.rs`), so every feature sees them.

**Laravel facades.** A class that extends `Facade` forwards to the class its `getFacadeAccessor()` returns: a class name, or a string the framework's core aliases and the project's providers map to one, else the class its `@see` names. Methods come back static, with `static` bound to the real class. Laravel's global class aliases (`Facade::defaultAliases()` and the `aliases` of `config/app.php`) are known classes.

**Eloquent models.**

- Attributes: the columns of the table from `database/migrations` (`Schema::create` and `Schema::table` in file order, with drops, renames, `change()`, `nullable()` and the rest of the Blueprint) and from a `database/schema/*.sql` dump that stands for the migrations before it. The table is `$table`, `#[Table(name:)]`, else the snake case plural of the class. `$casts` and `casts()` give the type (`datetime` is `Carbon`, an enum cast the enum, a `CastsAttributes<TGet, TSet>` class `TGet`), a nullable column is `?T`, `created_at`, `updated_at` and `deleted_at` follow `$timestamps` and `SoftDeletes`. Accessors, `$appends` and `#[Appends]` are properties too, and `@property` docs keep working.
- Relations: a method that returns a `Relation` is a property typed from `getResults()`'s template; one that declares only `HasMany` is read from its body (`return $this->hasMany(Post::class)`).
- Scopes: `scopeActive()` and `#[Scope]` methods are methods of `Builder<User>` and static methods of `User`.
- `where{Column}` is a builder method for every known column, and a model forwards what it does not declare to `Builder<static>`, as an instance call and a static one.
- Factories: `User::factory()` is the factory `@use HasFactory<UserFactory>`, `#[UseFactory]` or Laravel's naming rule gives.
- `Auth::user()`, `auth()->user()` and `$request->user()` are the model `config/auth.php` names, and `auth()` forwards what its contract lacks to the guard.

**The container.** `app(Foo::class)`, `resolve()` and `make()` are `Foo` through the framework's conditional types. A string gives the class of the framework's own aliases (`app('cache')`) and of what the project binds under a name written out: `bind`, `singleton`, `scoped`, `instance` and `alias` in a service provider, the provider's `$bindings` and `$singletons`, `withBindings()` and `withSingletons()` in `bootstrap/app.php`, and the same calls outside a provider in `app/` and `bootstrap/app.php` through `App::`, `app()`, `$app`, `$container` or `Container::getInstance()`. A name bound to two different classes stands for neither; tests are not read. In Symfony `$container->get('app.mailer')` follows `config/services.yaml`, `services.php`, aliases and `resource:` entries.

**Doctrine.** `#[ORM\Entity(repositoryClass: ...)]` makes `$em->getRepository(User::class)` that repository. The finders give the entity from the `ObjectRepository<T>` the repository implements, and `findByEmail`, `findOneByFirstName` and `countByEmail` exist for its fields. A `Collection` of a `OneToMany` or `ManyToMany` is a `Collection<int, Target>`.

**Raxos.** `framework/raxos/orm.rs` reads every model of the project the way the ORM's structure generator does: a property with `#[Column]`, `#[PrimaryKey]` or `#[ForeignKey]` is a column under its key (the attribute's argument, else its name) and its alias (`#[Alias('x')]`, or the key for a bare `#[Alias]`); one with a relation attribute is a relation to the model a to-many attribute names first, or to the type of the property; `#[Macro]` and `#[Embedded]` are their own kinds. A model is read once per change of a PHP file, since the class name in an attribute needs the imports of its file, and a model below another has the properties of both. What follows from it:

- a `ModelArrayList` property of a to-many relation without a `@var` is a `ModelArrayList<int, Target>`;
- every relation is a method of its name that gives `QueryInterface<Target>`, which the model answers through `__call`; an `@method` the class writes wins;
- the strings of `Model::col()` and `column()` name a column, those of `only()`, `makeVisible()`, `makeHidden()`, `ModelArrayList::column()` and `#[Visible]` on a relation any property, and those of `eagerLoad()` a relation. A nested array below a relation key (`only(['buyer' => ['id']])`) is read against that relation's model, `#[Visible]` against the model of the relation it sits on, and the second argument of `#[MapModelRelation]` against the model of the constructor parameter its first names. Each completes, leads to its property and is reported by `unknown-model-key` when the model has no property of that name, alias or key, which the ORM throws for;
- an `@method` of a model whose property is no relation, or that names another model than its relation, is `model-method-mismatch`; one that says what the relation does is `redundant-model-method`, with a fix that removes the line;
- `invalid-model-attribute` reports a `#[Macro]` callable whose first parameter cannot take the model or whose return type is a class the property cannot hold, and a `#[Caster]` class that does not implement `CasterInterface`.

`framework/raxos/router.rs` reads the controller tree: `#[Controller(prefix:)]`, the `#[Child]` controllers below it and the routes of its methods (`#[Get]` to `#[Any]`). A controller nobody has as a child is mounted at its own prefix, so a route has one whole path per chain of parents, which hover on its attribute shows. The router puts a parameter into a path where `$name` matches the name of one, so `unknown-route-parameter` reports a `$name` of a prefix that no constructor parameter has, of a route path that no parameter of the method has, and a first argument of `#[MapModelRelation]` that no constructor of the controller or one above has. A route method without a return type is `route-without-return-type`, since the mapper refuses it. `message-handler-mismatch` reports a `#[Handler]` on a message whose class does not implement `HandlerInterface` for that message.

### Strings that name things

The argument of a function or method the overlay marks is followed like a name: completion of the names the project declares, definition (to the key, the route, the template, the `.env` line), hover, usages in both directions (from the declaration too: the `'home'` of `->name('home')`, a key of `config/app.php`, `#[Route(name:)]`), and for some an inspection. A string the overlay does not mark is not a usage, which keeps a route name out of `Route::prefix('home')`.

| Kind | Where the names come from | Reported as unknown when |
| --- | --- | --- |
| Laravel config key | the arrays of `config/**/*.php` over the framework's own `config/*.php` (unless `dontMergeFrameworkConfiguration`) | the file exists, every array on the way is written out, no package merges into it and the project does not write it at run time; never with a default or `has()` |
| Laravel route | `routes/**/*.php`: `->name()`, group prefixes, resources, required files | every route file was followed and no vendor provider registers routes |
| Laravel view | `resources/views` | the name has no `::` and no provider adds locations |
| Laravel translation | `lang/<locale>/*.php` and `lang/<locale>.json` | the key starts with a group file that exists, has dots and no spaces, and no locale has it |
| `env()` | the names in `.env*` files | never |
| Abilities | `Gate::define` and the public methods of `*Policy` classes | never |
| Form request fields | the keys `rules()` returns | never |
| Symfony route | `#[Route]` with class prefixes and generated names, `config/routes*`, bundle imports | every import was followed and the kernel does not load routes itself |
| Symfony template | `templates/**/*.twig` | the name has no `@` and `twig.yaml` adds no path |
| Symfony translation, parameter, service, event | the translation files, `parameters:`, the service ids, the `*Events` constants and event classes | never |
| Doctrine entity field (`findBy(['x' => ...])`) | the mapped properties of the repository's entity | never |
| Inertia page | files below `resources/js/Pages` (`pages`, `resources/ts/Pages`), `vue`, `tsx`, `jsx`, `svelte`, `ts`, `js` | a pages folder holds a page at all |
| Pennant feature | `Feature::define('name')` in `app/` and the classes of `app/Features` | the project defines features and none by a computed name |
| Serialization group | `#[Groups]` on properties, methods and classes | no group mapping in `config/serializer` or `config/api_platform`; never `Default` |
| Symfony workflow, transition, place | `framework.workflows` in the YAML of `config/` | no PHP file of `config/` configures workflows |
| Livewire component | subclasses of `Livewire\Component` below the class namespace, and `Livewire::component()` | never |
| Raxos model column, property, relation | the ORM properties of the model the call is made on or holds, and of the models above it | the project declares the model |

`%name%` and `%env(NAME)%` in `#[Autowire]` lead to the parameter, the `.env` line and the service. `#[AsEventListener(event:)]`, `dispatch($event, 'name')`, `addListener()` and the keys of `getSubscribedEvents()` complete the events. In `$builder->add('name', |)` and `createForm(|)` the form types complete as `TextType::class`.

### Query strings

**Eloquent relations.** The relation of `with()`, `without()`, `has()`, `whereHas()`, `withCount()`, the aggregates, `load()` and their kin, in the argument, a list or a key (`with(['posts' => fn ...])`), on a model, a builder, a relation or a collection (`frameworks/relations.rs`). Each segment of `posts.comments.author` is a relation of the model the segment before leads to; `posts:id,title` and `posts as total` name `posts`. A segment is the relation method: hover and definition go to it, its usages list the strings and a rename renames them. `unknown-relation` reports a segment its model has no method of at all, not when a model below it has one, and not in a project or package that makes relations at run time (`resolveRelationUsing`). `setRelation()` and its kin take any name and are not read.

**Validation rules.** The array a form request's `rules()` returns and an argument marked `@rules` (`validate()`, `Validator::make()`, `validator()`) are rules by field, split on `|` as Laravel splits them (`frameworks/rules.rs`). A rule is the method the validator checks it with (`required_if` is `validateRequiredIf()`), so it hovers and leads there; completion offers the validator's rules and the ones `Validator::extend()` and its kin add. `exists:` and `unique:` name a table and its column; the rules that compare fields name other keys of the array. `unknown-validation-rule` reports a rule nothing has or adds, and nothing when a rule is added under a computed name.

**Casts.** The keys of a model's `$casts` and `casts()` are columns of its table; the values written as strings are casts (`frameworks/casts.rs`), completed from `HasAttributes::$primitiveCastTypes` of the installed framework. `unknown-cast` reports a cast that is none of those, has no `date:`, `datetime:`, `immutable_date:`, `immutable_datetime:` or `decimal:` prefix and whose part before the colon is no class; a string with a backslash is left to the class inspections, and casts compare without case, as the model lowercases them.

**Columns of a query.** A string marked `@key column` on the query builder (`where('email')`, the keys of `where([...])`, `orderBy()`, `latest()`, `pluck()`, `select()` and `groupBy()` with every argument, the `where` forms, the aggregates) is a column of the table of the query's model (`column_key` in `frameworks/keys.rs`). `posts.title` names the column of `posts`, `options->theme` the JSON column, `email as address` the column `email`; a raw expression or `*` names nothing, and a query without a model stays out. Definition goes to the migration, completion offers the columns, and the usages of a migration's column list the strings and the cast keys. Nothing is reported: an accessor, a select alias or a joined table makes a name right that is no column of the table.

**Filament.** The name of a column, field or entry (`TextColumn::make('title')`, `TextInput::make()`, `TextEntry::make()`, marked `@key filament-field`) is an attribute of the model the class works on (`framework/filament.rs`): a resource's `$model` or the model named after it, the classes a resource calls statically (`ArticlesTable::configure()`) and the pages whose `$resource` names it. `author.name` follows the relation, and the last segment is a column of that model's table. A class two resources call stands for neither. Nothing is reported.

**DQL.** A string marked `@dql` is a whole statement (`createQuery()`, `setDQL()`), one marked `@dql-part` a part of one: the query builder's select, where, having, group and order arguments, the join path and condition, the arguments of `Expr` (`frameworks/dql.rs`). Pieces joined with `.` are read as one, with `User::class` standing for its class. A statement's aliases come from its own `FROM`, `JOIN`, `UPDATE` and `DELETE`; a part's from the builders of the same function (`from()`, `update()`, `delete()`, the joins, a repository's `createQueryBuilder('p')`) and from a subquery in the part. An alias given to two entities, or to an unknown class, stands for nothing. `p.title` is the property `title` of the alias's entity: hover, definition, completion after the dot, usages and rename. The class after `FROM`, `JOIN`, `NEW` and `INSTANCE OF` leads to it. `unknown-entity-field` reports a field an attribute-mapped entity, and every class below it, lacks as a property; an alias a builder in another function makes is not followed.

### Laravel packages

**Livewire.** Components are named in kebab case after their class below the class namespace (`livewire.class_namespace`, else `App\Livewire` or `App\Http\Livewire`), folders joined with dots, or as `Livewire::component()` registers them (`framework/livewire.rs`). Each renders the view its `render()` returns, or `livewire.{name}`. In Blade, `<livewire:forum.edit-reply>` and `@livewire('forum.edit-reply')` lead to the class and complete, an attribute leads to the public property or `mount()` parameter it fills, and a bound attribute's PHP is read. In the view exactly one component renders, `wire:model` names a public property (`form.title` the property of a form object) and `wire:click`, `wire:submit`, `wire:keydown`, `wire:poll` and the other actions a public method, with hover, definition, completion (lifecycle methods left out) and usages. A view rendered by convention is given the component's public properties. Nothing is reported.

**Inertia.** `Inertia::render('Users/Index')`, `inertia()`, `Route::inertia()` and the `->component()` of the test assertions name a page (`framework/inertia.rs`), which definition opens. The server watches the pages folders, so a new page counts at once. Props are not followed into the page, which is in another language.

**Pennant.** `Feature::active('new-api')` and its kin (on the facade, `Feature::for($user)` and a store), and `@feature` and `@featureany`, name a feature (`framework/pennant.rs`). A class named by `::class` is an ordinary class reference.

### Symfony packages

**Serialization groups.** A group is declared by a `#[Groups]` (`framework/symfony/serializer.rs`) and named by a context, under `AbstractNormalizer::GROUPS` or `'groups'`, in an argument named `normalizationContext`, `denormalizationContext`, `serializationContext` or `context` (API Platform's resources and operations, `#[MapRequestPayload]`, `json()`) or in an argument of `serialize()`, `deserialize()`, `normalize()`, `denormalize()` and `json()`. A validation context uses the same key for validation groups, which is why the argument decides. Definition lists every `#[Groups]` that names the group.

**Messenger.** The handlers of a message are the classes marked `#[AsMessageHandler]` (their `__invoke` or the `method:` named), the methods marked so, and the classes that implement `MessageHandlerInterface`; the message is `handles:`, else the class of the method's first parameter (`framework/symfony/messenger.rs`). Go to implementation on a message lists them. Handlers configured in YAML are not read, and a message without one is not reported, since another application may handle it.

**Workflow.** The workflows and state machines of `framework.workflows` (also under `when@dev`), with their places and transitions, are read from the YAML of `config/` (`framework/symfony/workflow.rs`). The transitions of `apply()`, `can()` and their kin on a `WorkflowInterface`, the workflow of `Registry::get()` and `has()`, the arguments of the listener attributes and of Twig's `workflow_can()` and kin lead to the configuration and complete. A transition is looked up in every workflow, since which one an injected `WorkflowInterface` is depends on its argument's name.

### Limits

- Nothing is run. A config key set by code that is not a literal, a route made in a loop, a binding with a computed name, a view path a package adds and a translation key that is a sentence are never reported.
- Eloquent knows the table by migrations and schema dumps only: raw SQL, another connection's table, a computed `$table` and attributes a trait adds at run time are not known. A `Castable` cast class and an accessor without a typed `get:` give `mixed`. A relation built from a string or a macro gives the relation without its model. `Factory::create()` is `Collection<int, Model>|Model`, as the framework documents it.
- A facade whose accessor is not a literal or class constant has no target. `Auth::guard('api')->user()` is the model of the first provider.
- `$app['config']`, `bound()` and contextual bindings are not read. In Symfony `#[AsAlias]` and `#[Autowire]` are read; `#[Autoconfigure]`, `#[AsTaggedItem]` and tags are not.
- Symfony routes without a literal name, other bundles' loaders and XML other than `<route id=>` are not followed, and the routes are then not judged.

## Templates and configuration

### Blade

A template is read the way Blade's compiler reads it (`blade/scan.rs`): `{{-- --}}` comments, `@verbatim` and `@php ... @endphp` blocks, `<x-...>` and `<livewire:...>` tags with their attributes, directives (an `@` after a character that is not part of a word, `@@` escaping one, arguments in balanced parentheses), echoes with `@{{` escaping one, and `<?php ?>`. All of its PHP is written out as one document (`compile.rs`), which the type layer reads:

- echoes become `echo (...)`, `@php` and `<?php ?>` are copied, bound attributes of tags become expressions, and the arguments of every other directive the compiler knows become an array expression;
- `@if`, `@unless`, `@isset`, `@empty`, `@foreach`, `@forelse`, `@for`, `@while` and `@switch` become the control structures they compile to, and the other blocks (`@auth`, `@can`, `@section`, `@push` and the rest) become `if` blocks, so narrowing and loop scope hold inside them;
- `@foreach` gives `$loop` (a `stdClass`), `@error` gives `$message`, `@session` gives `$value`, `@use` is a `use` statement, `@inject` a variable of its class, and each key of `@props` and `@aware` a variable with the type of its default;
- every template of a Laravel project has `$errors`, `$__env` and `$app`, a component also `$attributes` and `$slot`;
- what the places that render the template give it is declared at the top (`blade/data.rs`). Those places are the usages of its view name or component tag: the array after the name in `view()`, `View::make`, `Route::view`, `response()->view`, `new Content(view:, with:)` and a mailable's or mail message's `view()` and `markdown()`, with `compact()` and shapes read; the `->with()` calls after it; the public properties of the component, mailable or Livewire component that renders it; an `@include`'s scope and array, `@each`'s item and `@extends`' child; and the attributes of `<x-...>` for an anonymous component. A variable is the union of what every place gives, followed two includes deep. The server keeps what a template is given until another file changes.

Layouts, stacks, slots and component attributes are keys of their own (`framework/layouts.rs`): a section is declared by `@yield` or `@section ... @show` and named by `@section`, `@hasSection` and `@sectionMissing`; a stack is declared by `@stack` and named by `@push` and its kin; an attribute of `<x-card show-count>` is a prop of the card (an entry of `@props` or a constructor parameter, matched in camel case, offered in kebab case); a slot belongs to the component around it and completes from the variables its template uses but does not make.

A directive the project registers with `Blade::directive('name')` replaces the compiler's of that name, so its arguments are left alone and it opens no block; one registered with `Blade::if('name')` opens a block with `@else<name>`, `@unless<name>` and `@end<name>`, and so does any directive a template closes with `@end<name>`. Both are found in the project's PHP outside views, tests, config and translations.

Diagnostics, all of them certain: syntax errors of the PHP where the template writes it (an error in what the document adds is dropped); `unbalanced-directive` for a block never closed, an `@end...` that closes nothing, or an `@else`, `@case` or `@empty` outside its block; and `unknown-view`, `unknown-route`, `unknown-config-key` and `unknown-translation` under the rules of PHP files, for the project's own templates.

Hover, definition, completion, usages, highlights and rename are asked of the document and mapped back; only code copied from the template maps back. Finding the usages of a class, method or property also finds them in templates and renaming renames them there. Move class, change signature and pull up and push down do not write templates and refuse when one uses what they would change.

Limits: `$loop` has no shape. A component tag with no template or class is not reported, since packages register components. A template more than two includes from its controller gets nothing from it. View composers and `View::share` are not known. The PHP inside a directive the project registers is not read.

### Twig

A Twig template is read as Twig (`twig/`). The lexer follows Twig's: markup, `{{ }}` and `{% %}` with `-` and `~`, comments, `verbatim` and `raw` left as markup. Expressions are read by precedence as Twig does (`~` binds tighter than `+`, `is` tests, `??`, `?:`, arrow functions, hashes with shorthand keys, named arguments) and never fail: what is missing is a hole, which completion needs while a person types. `extends`, `include`, `embed`, `use`, `import`, `from`, `block`, `for`, `set`, `macro` and `apply` are read by their shape; another tag as the expressions in it.

What a template names:

- templates, in the strings of the tags above and `form_theme`, which complete, lead to the file and have usages;
- blocks, `{% block name %}` and `block('name')`, which complete from the blocks of the template's parents and lead to them (`framework/twig.rs`);
- functions, filters and tests, as the extensions of the project and its packages declare them: `new TwigFunction('name', callable, options)` and its kin in every class that implements `ExtensionInterface` (Twig's own `CoreExtension` among them), `#[AsTwigFunction]`, `#[AsTwigFilter]` and `#[AsTwigTest]`, and patterns such as `render_*`. They complete, hover with the PHP that runs for them and lead to it;
- the names the PHP behind a function or filter takes: Twig's arguments are laid onto the method's parameters (after the environment, charset and context its options pass, a filter's subject first), and the overlay's markers say what the parameter names, so `path('blog_index')` is a route, `'post.title'|trans` a translation, `include()` a template and `workflow_can(post, 'publish')` a transition, with usages in both directions.

Variables have types (`twig/types.rs`, `twig/data.rs`): what the places that render the template give it (`render()`, `renderView()`, `Environment::render()`, `#[Template]`, includes, embeds and `extends`), `app`, the variables of `for` with `loop`, `set`, macro parameters and `with`. A form a controller passes is its `FormView`. `post.title` is what Twig's `getAttribute` finds: a key, a public property, then `title()`, `getTitle()`, `isTitle()` or `hasTitle()`, and for an `ArrayAccess` what `offsetGet()` returns. Filters type by their PHP, except the ones that hand back what they get. Hover, definition and completion work on variables and attributes, and finding the usages of a getter or property from PHP lists the attributes that read it.

Diagnostics, all of them certain: an unclosed `{{` or `{%`, a token nothing takes, a missing expression, attribute, filter or test name, `unbalanced-directive` for block tags, `unknown-template` and `unknown-route` under the rules of PHP files, and `unknown-twig-function`, `unknown-twig-filter` and `unknown-twig-test` for a name no extension declares, judged only once Twig's own extension was read, and never for a macro the template has or for `parent`, `block` and `attribute`.

### YAML

The YAML of a Symfony project's `config/` and `translations/` is read for the names in it (`yaml.rs`, on the tree `framework/yaml.rs` reads): `%parameter%` and `%env(PROCESSOR:NAME)%` in any string, `@service` and `@?service`, the classes services are declared by (a key under `services:` that is a class, and `class:`), and `controller: App\Controller\Blog::show`. A scalar whose text differs from its value is left alone, so a range never lands beside the name. They hover, lead to the declaration and complete as they are typed, from the text, so a document that does not parse yet still completes; the place a file declares a parameter, service, route or translation key has usages. Finding the usages of a class or method from PHP lists the configuration that names it, and renaming a class renames it there. `undefined-class` reports a class a service is declared by that neither the index nor Composer's maps know.

## Build, test and check

```sh
cargo build --release --locked            # target/release/php-language-server
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
python3 scripts/test-native-release.py
python3 scripts/handshake.py target/release/php-language-server
```

The default test run needs no network and no PHP. It holds snapshot tests of the tree per construct (`UPDATE_EXPECT=1 cargo test` rewrites them), error recovery tests, a round trip test (the text of the tree equals the input for every prefix of a sample file and after every single edit), the language level table, the PHPDoc and type grammar, name resolution, the extractor, Composer metadata, the cache and the indexer on temporary folders, the hierarchy, type inference, completion, imports, hover, navigation, usages, rename, signature help, hierarchies, semantic tokens and inlay hints, every inspection, every refactor (each result parsed and inspected again, so it brings no syntax error and no new finding), the formatter, the test support, every catalog of the framework layer and every kind of string (`crates/index/src/framework/*`, `crates/analysis/src/frameworks/tests.rs`), Blade and Twig, and end-to-end tests of the server over an in-memory connection against projects written to a folder (`crates/server/tests/`). The pieces of Laravel, Symfony and their packages these tests need are in `php-index`'s `testing` feature.

The corpus of real PHP is fetched on demand:

```sh
./scripts/fetch-corpus.sh                 # phpstorm-stubs and php-src's test folders, into ./corpus
cargo test --locked --test corpus         # without a corpus these tests say so and pass
cargo run --release -p php-syntax --example corpus -- phpt --oracle --verbose
```

`--oracle` asks `php -l` of the PHP on the path whether PHP agrees with a verdict. The stubs declare `exit()` and `die()` as functions, which the parser reports as a reserved word used as a name, so that one message is ignored for the stubs. The tools that measure a project are listed in [MEASUREMENTS.md](./MEASUREMENTS.md).

## What is left

- A preview of a rename that moves namespaces.
- The doc and attribute formats of Laravel and Symfony macros.
- Interned strings, if the memory of the summaries ever matters.
- `#[ApiFilter(properties: [...])]` as entity fields, Messenger handlers configured in YAML, and an injected `WorkflowInterface` tied to its workflow by the argument's name.
- No diagnostics for query columns, Filament names and `wire:` attributes, where accessors and magic make "unknown" uncertain; a reason to add one would be a way to know the attributes a model has at run time.
