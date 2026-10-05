# php-language-server

A language server for PHP, written in Rust. It reads PHP 8.1 through 8.5 with a parser of its own that keeps every byte of a file, comments and whitespace included, and that goes on past a syntax error instead of stopping at it. The aim is the insight a full PHP IDE gives (navigation, completion, rename, inspections, refactors) as a server any editor can talk to over LSP.

Phase 1a is the parser, the syntax tree and a small server on top of it. Phase 1b adds an index of the project, its Composer packages and the standard library, a type layer, and hover, navigation, workspace symbols and completion with auto-import on top of that. Phase 2 adds find usages, rename, highlights, signature help, call and type hierarchies, semantic tokens, inlay hints and completion of overridable methods, and keeps the declarations of packages in the cache file until something needs them. Phase 3 adds inspections, quick fixes and code actions, and a formatter. Phase 4 adds refactors, the wrapping of long lines in the formatter and `.editorconfig`. Phase 5 adds PHPUnit and Pest, and the type work phase 1b left: closure arguments typed through templates, return types read from bodies and `@psalm-assert` narrowing. Phase 6 adds Laravel and Symfony: the members, strings and containers that frameworks make up at run time.

## Where the code comes from

Everything here is written from scratch. The sources it is allowed to learn from are:

- the PHP language reference and grammar (php.net, the grammar in php-src) and php-src's own tests for the cases the grammar leaves open;
- the open source IntelliJ Platform (Apache 2.0) for general ideas such as lossless trees and error recovery;
- JetBrains/phpstorm-stubs (Apache 2.0), as a corpus the parser must read without errors now and as the description of the standard library later;
- PHPStan and Psalm (MIT) for type rules, later;
- watching what an installed IDE does with a piece of code.

Nothing was taken from the bytecode or the decompiled classes of any IDE plugin, and no code was copied from another product. Crates under MIT, Apache or BSD licenses are used freely. The corpora are fetched by a script into a folder git ignores, so none of that material is part of this repository and there is no NOTICE to carry.

## Layout

A Cargo workspace with five crates.

| Crate | Holds |
| --- | --- |
| `crates/syntax` (`php-syntax`) | The lexer, the parser, the tree (on `rowan`), the language level table and the pass that checks a tree against a level. |
| `crates/format` (`php-format`) | The formatter: it reads a tree and decides the whitespace between tokens, and nothing else, and reads `.editorconfig`. Knows nothing of LSP. |
| `crates/index` (`php-index`) | The declarations of a file with their PHPDoc, name resolution, PHPDoc types, Composer metadata, the stubs of the standard library, the persistent cache and the stores that read declarations back from it, the parallel indexer, the class hierarchy, the word index that narrows a search to the files that may hold a name, and the test facts (groups, Pest datasets and bindings of a test case to a folder), and the framework layer (`framework/`: Laravel and Symfony). Knows nothing of LSP. |
| `crates/analysis` (`php-analysis`) | Questions about a tree and an index: document symbols, folding, selection ranges, diagnostics, the type layer, hover, definitions, implementations, workspace symbols, completion, usages, highlights, rename, signature help, hierarchies, semantic tokens, inlay hints, the refactors, the test support (PHPUnit, Pest, runnables), the strings of the frameworks (`frameworks/`, `blade.rs`) and the line index that maps offsets to positions. Knows nothing of LSP. |
| `crates/server` (`php-language-server`) | The LSP front end over stdio: documents, incremental sync, workspace folders, background indexing with progress, the requests below, configuration. Library and binary. |

### The syntax crate

- `lexer.rs` is a mode stack that mirrors how PHP tokenizes: inline HTML, `<?php` and `<?=`, interpolated strings with `$a[0]`, `$a->b`, `{$...}` and `${...}`, heredocs and nowdocs with flexible closing markers, casts, numbers with separators and every base, names as single tokens (`Foo\Bar`, `\Foo`, `namespace\Foo`), `__halt_compiler`. Every byte lands in exactly one token. The state is a small value that can be cloned: `lex_with_checkpoints` records it at the first token of every line, and `lex_from` resumes from one, which is what incremental relexing needs.
- `parser/` is recursive descent with a Pratt parser for expressions, over the union of PHP 8.1 to 8.5 syntax: property hooks, asymmetric visibility, `new` without parentheses in a chain, the pipe operator, `clone` with arguments, the `(void)` cast, closures and first-class callables in constant expressions, attributes everywhere they are allowed, alternative syntax, enums and so on. It never looks at a language level.
- `kind.rs` holds the one enum of token and node kinds. `dump.rs` prints a tree for tests and for looking at what the parser made.
- `language_level.rs` is described below.

Notes on the shape of the tree:

- A node starts at its first token and ends at its last, so trivia sit between nodes. The exception is a declaration, which also owns the doc comment right before it (and its attributes), so `/** ... */ final class A {}` is one node.
- Names are single tokens wrapped in a `NAME` node, for declarations and references alike. `A::$b` is a `STATIC_PROPERTY_EXPR`, `A::B` and `A::b()` are a `SCOPED_ACCESS_EXPR`, which is what tells an assignable target from one that is not.
- A `?>` ends a statement the way a `;` does, and a file may swap between PHP and inline HTML anywhere, including in the middle of `if (...): ?> ... <?php endif; ?>`.

### Error recovery

The parser never fails and never panics: every input gives a tree whose text equals the input, with `ERROR` nodes around what it could not read and a list of errors with ranges. A missing token is reported without being consumed, a list stops at a token that cannot belong to it (`;`, `{`, `}`), a statement list stops at a token that closes something outside it, and a class body picks up again at the next token that starts a member. A `public` or `private` found inside a method body ends that body, because it means a `}` went missing above it. Nesting is limited to 200 levels, past which the parser reports it and goes on, so a hostile file cannot overflow the stack.

Messages say what is missing in the form of an IDE (`';' expected`, `Expression expected`) and an empty range, which is where something is missing, is widened by the server to the character before it so a client has something to draw.

## Language level

The lexer and the parser accept all supported syntax and never reject it by version. What a file may use is a separate question, answered by `PhpVersion` and one table.

- `PhpVersion { major, minor }` is ordered and parses `8.4`, `8.4.2` and constraints such as `^8.1`.
- `FEATURES` in `language_level.rs` has one row per piece of syntax: its id, the name it has in a message, the version that introduced it, the version that deprecated it and the version that removed it, the node or token kinds to look at, a function that finds the construct, and an example. About forty rows cover 8.0 to 8.5.
- `check_language_level(tree, version)` walks the tree once and reports syntax newer than the level as an error (`Property hooks are only available since PHP 8.4`), syntax removed by then as an error (`The (real) cast was removed in PHP 8.0`) and deprecated syntax as a warning with the deprecated tag (`The backtick operator is deprecated since PHP 8.5`), each ranged on the construct.

Supporting PHP 8.6 means adding rows to the table, plus the syntax to the parser when there is any. No version check sits anywhere else. A test parses the example of every row and holds the row against it: reported just below its `since`, accepted at it, and the same for deprecations and removals.

## The server

`php-language-server --stdio` (the flag is the default) speaks LSP over stdin and stdout. It answers:

- `initialize`, with incremental document sync and a position encoding it negotiates (UTF-8 when the client offers it, else UTF-16);
- diagnostics: syntax errors and language level findings, pushed with `textDocument/publishDiagnostics` after a burst of changes has settled, or pulled with `textDocument/diagnostic` when the client announces that it pulls (then nothing is pushed). Zero-width ranges are widened;
- `textDocument/documentSymbol`: namespaces, classes, interfaces, traits, enums and their members (methods, constructors, properties and promoted properties, constants, enum cases), functions and constants, with signatures as detail and the deprecated tag from `@deprecated`. A client that cannot nest gets the flat form. Declarations behind `if (!function_exists(...))` count;
- `textDocument/foldingRange`: class, function and control structure bodies, arrays, `match` and `switch` bodies, property hooks, attribute lists, heredocs, alternative syntax bodies, multi-line and consecutive line comments (`comment`), runs of `use` statements (`imports`), `// region` and `// endregion` (`region`) and PHP tags between markup. A fold ends on the line before a closing bracket that starts its line, so the bracket stays visible;
- `textDocument/selectionRange`, growing from the token through every enclosing node to the file.

- `textDocument/hover`, `textDocument/definition`, `textDocument/typeDefinition`, `textDocument/implementation`, `workspace/symbol`, `textDocument/completion` and `completionItem/resolve`, described under "Phase 1b" below;
- `textDocument/references`, `textDocument/documentHighlight`, `textDocument/prepareRename`, `textDocument/rename`, `textDocument/signatureHelp`, `textDocument/prepareCallHierarchy` with `callHierarchy/incomingCalls` and `callHierarchy/outgoingCalls`, `textDocument/prepareTypeHierarchy` with `typeHierarchy/supertypes` and `typeHierarchy/subtypes`, `textDocument/semanticTokens/full` and `/range`, and `textDocument/inlayHint`, described under "Phase 2" below.
- `php/runnables` and `textDocument/codeLens`, described under "Phase 5" below.

### Configuration

Only standard LSP channels are used:

- `initializationOptions`: `{ "phpVersion": "8.4", "storagePath": "/some/folder", "stubsPath": "/some/stubs", "inlayHints": { "parameterNames": true, "closureTypes": true } }`. `phpVersion` is the language level for a project whose `composer.json` names none. `storagePath` is where the server keeps its cache and the standard library stubs; without it nothing is kept between runs and the stubs are not fetched, so the server then knows the project and nothing of the standard library. `stubsPath` reads a folder of phpstorm-stubs instead of fetching them; `inlayHints` switches the two kinds of inlay hint, both on by default;
- `workspace/configuration`, when the client supports it: the server asks for the section `phpLanguageServer` with the document as `scopeUri` and reads `{ "phpVersion": "8.4", "inlayHints": { ... }, "inspections": { ... }, "format": { ... } }`, so a client can answer differently per project or folder;
- `workspace/didChangeConfiguration` with the same object, bare or under `phpLanguageServer`.

The level of a document is the answer for its scope, else the default, else the newest version (8.5).

### Incremental reparse

A change is applied to the text and the whole file is parsed again. That is measured to be faster than anything a patch could save: a typical 54 KB file parses in about 0.5 ms and a 1.4 MB file in about 13 ms, while typing arrives at a fraction of that rate. The server also answers every message already queued before it publishes diagnostics, so a burst of keystrokes costs one parse. Reusing unchanged subtrees of the old green tree is possible later if a profile ever asks for it.

## Phase 1b: the index, types and the features on top

### What the index holds

`php-index` reads every file into a `FileSymbols`: classes, interfaces, traits and enums with their cases, constants, properties (promoted ones and ones with hooks and asymmetric visibility included) and methods, plus functions and constants (`const` and `define()`). A declaration carries its signature, modifiers, attributes, location, the PHPDoc (summary and description, `@param`, `@return`, `@var`, `@throws`, `@deprecated`, `@template` with bounds and defaults, `@extends`, `@implements`, `@use`, `@mixin`, `@property*`, `@method`, `@see`) and its types with every class name already resolved, so a cached entry needs nothing of the file it came from.

- **Sources.** The project's own files (everything under the folder except `vendor/`, `node_modules`, dot folders and generated caches), the installed packages (only the files Composer would load, from the `autoload` of `vendor/composer/installed.json`), and the standard library stubs. A project file wins over a package, and a package over the stubs.
- **Cache.** One file per project under `<storagePath>/cache`, keyed by the path of each source file with its size, modification time and a content hash. A file whose stamp matches is not read; one that was only touched is read and hashed but not parsed. The cache is dropped when the sources that decide what an entry means change (a hash of them is in the header). Its layout is described under "Memory" in the phase 2 section.
- **Parallel and incremental.** Files are extracted on a rayon pool and reach the server in batches, which reports them as `$/progress` (work done progress, when the client supports it). Open documents are indexed from their text as it changes and win over the disk; `workspace/didChangeWatchedFiles` (registered dynamically for `**/*.php`, `composer.json` and `installed.json`, and, for what the framework layer reads, `.env` files, `lang`, `translations`, `templates`, YAML and XML under `config` and `database/schema` dumps) updates single files, and a change to Composer's files reads the project again. Workspace folders can come and go.
- **Missing classes.** A class the index has not seen is looked up in the PSR-4 and PSR-0 maps and read on demand, for hover and navigation.

- **Composer projects below the workspace folder.** A workspace folder that has no `composer.json` of its own (a repository with `backend/`, `frontend/` and `shop/` next to each other) is searched up to four folders deep for `composer.json` files, skipping `vendor`, `node_modules`, dot folders and folders starting with `~` (backups). Each one found is a project of its own, with the packages in its own `vendor/`, its own autoload maps, language level, stub extensions and cache file. A file belongs to the project with the deepest root above it, so hover, navigation and completion in `backend/` only see what `backend/` installs, even when `shop/` installs another version of the same package. The folder's own project reads the files that are in no composer project and skips the ones that are. A workspace folder that has a `composer.json` is one project, as before, whatever lies below it, which keeps a monorepo of packages visible to itself. `workspace/symbol` asks every project and lists a package or standard library symbol once, whichever project reported it first; symbols of the project's own files are listed for every project that has them. A `composer.json` that appears or disappears below the folder adds or drops a project, and one that changes reads that project again.

Measured on a project with 9,288 PHP files (its own and the packages Composer loads, 9,272 classes) on an Apple M-series laptop with 16 cores, release build:

| | Cold (no cache) | Warm (cache) |
| --- | --- | --- |
| Standard library stubs (547 files, 9 MB) | 140 ms | 8 ms |
| The project | discover 100 ms, index 340 ms | discover 90 ms, index 11 ms |
| Resident memory of the whole server after indexing | 125 MB | 35 MB |
| ... after a find usages, a workspace symbol search, completion and semantic tokens | 130 MB | 45 MB |

Before phase 2 the whole process held 440 MB cold and 310 MB warm, since every declaration of every package sat in memory. The numbers above are `scripts/measure-memory.py` on a real server, the ones of the bench example are in the next paragraph.

`cargo run --release -p php-index --example bench -- <project> <stubs> [cache]` measures the indexing on any project and prints the resident size. Single-threaded extraction of all stubs takes about 100 ms (`cargo bench -p php-index`), and adding them to an index 2 ms.

### The standard library stubs

The stubs are JetBrains/phpstorm-stubs (Apache 2.0). They are not in this repository and not in the binary. On first run the server downloads the tarball of one pinned commit (`STUBS_COMMIT` in `crates/index/src/stubs.rs`) into `<storagePath>/stubs/<commit>/`, unpacks only the `.php` files and the `LICENSE`, and marks the folder complete once it is whole, so an interrupted download starts over. A later run reads that folder, and a new pin fetches a new folder. The download runs in the background; until it is done the standard library is simply not offered, and a failure (no network) is logged with `window/logMessage` and leaves the rest working. `stubsPath` points at a folder that already exists, which is what the tests and `scripts/fetch-corpus.sh` use.

The stubs of one extension live in one folder. A project sees the folders of a default set (core, standard, SPL, date, json, pcre, mbstring, curl, PDO, intl and the like) plus every `ext-*` its `composer.json` requires, so `ext-redis` brings in the Redis classes and a project without it does not get them.

### Language level

The level of a project is `config.platform.php` of its `composer.json`, else the lower bound of `require.php` (`^8.1 || ^8.2`, `>=8.1 <8.4`, `~8.1.0`, `8.3.*` and hyphen ranges are read), else the `phpVersion` the client configured, else the newest version. The index filters the stubs by it: `@since` and `@removed` of a declaration, `#[PhpStormStubsElementAvailable(from:, to:)]` on functions, methods, classes and parameters, and `#[LanguageLevelTypeAware([...], default:)]` on return types, properties and parameters, which pick the type of the project's level. An 8.1 project is not offered `array_find` or `Random\Randomizer`. The `@since` of a project's own doc comments means nothing and is ignored there. A level that comes from `composer.json` also decides the syntax diagnostics of that project's files.

### Names

`NameResolver` holds the namespace and the `use` statements (classes, functions and constants, grouped, aliased) at a point of a file. Class names resolve through the imports and the namespace, relative `namespace\Name` and qualified names included; functions and constants try the namespace and then the global namespace. `self`, `static` and `parent` follow the class around the cursor. `ClassDecl`s keep traits with their `insteadof` and `as` rules, and the hierarchy (`Index::ancestors`) walks the class, its traits, its parents, its interfaces and its `@mixin`s in the order PHP looks members up, carrying the template arguments of `@extends Base<Foo>` down the chain.

### The type layer

`php-analysis` infers what a variable or an expression is at a cursor. It follows:

- declared types (unions, intersections, nullable, `static`, `self`) and PHPDoc types: `array<K, V>`, `list<T>`, `T[]`, array shapes, `class-string<T>`, `callable(A): R` and `Closure(A): R`, literals, `int<..>` and the `non-empty-string` family, and conditional types such as `($flag is true ? int : string)`;
- generics: `@template` on classes, methods and functions, bound from the receiver (`Collection<int, User>`) and from the arguments of a call (`make(User::class)` is a `User`, `identity($x)` is the type of `$x`), also for `new`;
- variables: parameters (with `@param` types, defaults of `null`, variadics), assignments, `new`, calls of functions, methods and static methods, property reads, constants and enum cases, array literals and `$a[] = x`, indexing, destructuring and `foreach` over arrays, iterables, `Iterator` and `IteratorAggregate` classes, `catch`, `static` and `global` variables, closures with `use`, arrow functions and inline `/** @var T $x */`;
- flow: branches merge into unions, a branch that ends in `return` or `throw` does not, `instanceof`, `=== null`, `isset`, `is_*` functions, truthiness and `assert` narrow in the branch they hold for and survive an early exit.

- calls with closures: the parameter a closure takes is typed from the `callable(T): U` the callee asks for, with `T` bound from the other arguments and the receiver, and the closure's return type binds `U`. The standard library stubs have no generics, so `array_map`, `array_filter`, `array_reduce`, `usort` and 27 more array functions get theirs from `crates/index/src/stub_overlay.php`;
- return types read from bodies: a function, method or closure with no declared or documented return type returns what its `return` statements return (a generator is a `Generator<K, V, mixed, R>` from its `yield`s), when all of them are known. A body in another file is read from disk, three bodies deep at most;
- `@psalm-assert`, `@phpstan-assert` and their `-if-true` and `-if-false` forms, with `!Type` and `=Type`, narrow the argument a call names (`assertInstanceOf`, `assertNotNull`, `Assert::assertIsString`).

It stops at: loops that need a fixed point, references, `list` with references, variable variables and dynamic member names, `__get` and `__call`, PHPDoc type aliases (`@psalm-type`), `key-of`, `value-of` and the other type-level functions, which come out as `mixed`, and a return type read from a body where one of the returns is not known. Where it gives up the type is `mixed` and nothing is offered or reported on it.

### Hover, navigation and symbols

- **Hover** is a title line with the qualified name, a fenced `php` signature (parameters with their types and defaults, modifiers, the return type), the PHPDoc as markdown (the summary, the description, one `_@tag_` line per tag, the HTML some doc comments use turned into markdown, a method without a doc inheriting the one above it) and the file it is defined in (relative to the project, or `PHP standard library (ext)`). It works on classes, functions, constants, members, enum cases and variables (with their inferred type).
- **Definition** goes to the name of a class, function, constant, method, property or class constant, to the use of a variable, and into the stubs as files in the storage folder. **Type definition** goes to the class of a variable, a property or what a method returns. **Implementation** lists the subclasses and implementors of a class or interface, and the overrides of a method.
- **Workspace symbols** match classes, interfaces, traits, enums, functions and constants everywhere, and members of the project's own classes, by prefix, camel humps (`uc` finds `UserController`) and substring, project first.

### Completion

The word being typed is replaced by a placeholder before the text is parsed again, so an unfinished `$user->` is a whole expression to the parser and its tree says which kind of name belongs there. Completion handles:

- members after `->` and `?->` (instance members, by visibility from the class around the cursor), and after `::` (static methods, constants, enum cases, static properties as `$name`, `class`; and, after `parent::`, `self::` and `static::`, the instance methods too);
- classes in expressions, `new` (instantiable ones only, with the constructor's parameters as `detail`), `extends` (not final classes), `implements` (interfaces), trait `use`, `catch` (throwables), `instanceof`, type positions (with the built-in types) and `#[` (classes marked `#[Attribute]`);
- functions and constants, variables in scope with their types (and superglobals), named arguments of the callee, enum cases, and keywords by place (statement, expression, class body, type);
- `use` statements: namespace segments, classes, `use function` and `use const`.

A class that needs an import carries an `additionalTextEdits` entry that inserts the `use` line in sorted order in the right block: the block of the file that shares the root namespace, else the one it sorts into, after an existing group at its end, and for a file without imports after the `namespace` or `declare` line with a blank line around it. A class of the current namespace or one already imported needs nothing, and one whose short name is taken is written in full. Items have a `kind`, `labelDetails.description` with the namespace (the class for a member), `detail` with the signature, a deprecated tag from `@deprecated`, and `completionItem/resolve` adds the same documentation hover shows. No call snippets are inserted: the client adds the parentheses. With nothing typed only variables, keywords and members are offered; at most 300 items come back and `isIncomplete` says when there were more. Comments, strings and inline HTML get nothing.

## Phase 2: usages, rename, hierarchies, signatures, tokens and hints

### Names and their usages

A name in a file resolves to a `Symbol` through the same resolution and type layer hover uses: a class, function or constant by its qualified name, a method, property or class constant by the class that declares it, a parameter by the function that owns it, and a local variable by the function or file it lives in. Several declarations are one symbol when they are one thing: a method is one with the methods it overrides and implements, in both directions (an interface method, its implementations, and the classes below them), and a property or class constant works the same way. A private member and a constructor are their own. A promoted constructor parameter is a variable, a parameter and a property at once, so its usages are the variable inside the constructor, the named arguments of `new` and every `$object->name`.

What counts as a usage: a class in every type and expression position (types, `new`, `extends`, `implements`, `instanceof`, `catch`, `Foo::class`, static access, attributes, trait `use` and its `insteadof` and `as` rules), `use` statements and group uses, functions and constants, methods (also through `static::`, `self::`, `parent::`, first-class callables and `?->`), properties (also written as `Foo::$name`), class constants and enum cases, variables with their closures and arrow functions, parameters through their named arguments, and PHPDoc: the classes in the types of every tag, `@param` and `@var` variables, `@property` and `@method` declarations, `@template` names, and the targets of `@see` and `{@see}`. A constructor also counts the `new` that calls it, also for a class below it that has no constructor of its own. A class imported under an alias counts where the alias is written.

How it is found: `php-index` keeps, per file of the project, the sorted hashes of the words in it (`words.rs`), built the first time something asks and kept current from then on (open documents, watched files). A search reads only the files whose words contain the name, in parallel, and resolves the names in them against the index, so a result is never stale when a file elsewhere changed. Open documents are searched as they are in the editor. The resolved references are not stored. In the 798 own files of the project above, finding the 321 usages of a class in 68 files takes 60 ms in the probe (the words took 80 ms the first time) and 25 ms through the server, and a method used in five places takes 2 ms.

Counting conventions, as an IDE counts the "N usages" above a declaration: the declaration itself is not a usage (a client asks with `includeDeclaration: false`, and a promoted parameter's own line is left out as well), a `use` import is one, a name in a doc comment is one (types of every tag, also inside array shapes that span lines, and `@see` targets), and so is a call through an interface or a parent for each method of the family, `parent::` included.Folders whose name starts with `~` are not read, so a copy of the project in a `~backup` folder does not double every count.

`scripts/survey-usages.py` runs the same questions at a spread of declarations on two servers and prints every difference, which is how these conventions were checked against a real project of 564 source files: of 453 declarations, 376 give the same set of places. The rest are the installed packages (calls from `vendor/` that this server does not read), the declaration line of a property that the other server reports as a usage, and receivers the type layer cannot name.

Limits: only the project's own files are searched, not the installed packages. A usage whose receiver the type layer cannot name (`mixed`, a dynamic member name, `__get` and `__call`) is not found, and neither is a name inside a string (`'App\Foo'`, `[$this, 'run']`, `compact('x')`). `$$name` and `${name}` are not variables here. A usage in a file that is not on disk and not open is not seen.

`textDocument/documentHighlight` marks the same places inside the open file. Variables and properties and class constants say read or write (an assignment, a compound assignment, `++`, `unset`, a `foreach` target, a destructuring and a by-reference `use` count as writes), the rest says text.

### Rename

`prepareRename` answers the range and the name of what is under the cursor, or says why not: a keyword (`self`, `static`, `parent`, `$this`), a name declared in an installed package or the standard library, a magic method, a name the index does not know. `rename` returns a workspace edit, as `documentChanges` when the client sends them and as `changes` otherwise. It renames:

- classes, interfaces, traits and enums, with their `use` imports (the imported name only, so `use A\B as C` becomes `use A\X as C` and `C` stays), the names in doc comments and the attributes. A class written under an alias keeps the alias. When the file carries the class's name, is the only class in it, and Composer's autoload map agrees that the class lives there, the file is renamed too, as a `RenameFile` after the text edits and only when the client announces resource operations;
- methods with their overrides, implementations and callers, properties with their `@property` lines, promoted parameters and named arguments, class constants and enum cases, functions and constants with their `use function` and `use const` imports;
- variables and parameters inside their scope (a `use ($x)` of a closure and the `$x` of an arrow function follow), with the `@param` and inline `@var` of the doc comments, and the named arguments at the call sites;
- a namespace, from its `namespace` statement: every file with that exact namespace, every `use` of a name in it, group use prefixes and fully qualified names. Names written relative to another namespace are left, and so are sub-namespaces and Composer's `autoload` section.

A new name is checked: it has to be an identifier (a namespace may have backslashes), a class, function or constant may not be a reserved word, a type name or a keyword, a method or property may be named after a keyword, a class constant may not be `class`, a variable may not be `this` or a superglobal. Names that are taken are refused with the place: a method, property or constant of the same name anywhere in the hierarchy, a class or function of that name, a variable in the same scope, a class another class of the same file already imports under that name. The error goes back as a failed request with the message.

Strings and ordinary comments are not changed, and the server says so in a `window/logMessage` with every rename.

### Signature help

`textDocument/signatureHelp` finds the argument list around the cursor (also one that is not closed yet), resolves the callee (functions, methods, static methods, constructors with `new`, attributes, closures with a documented signature) and answers each way the callee can be called: a function or method that is declared more than once, as the stubs do for functions that can be called in different ways, has one signature per declaration that exists at the project's language level. Variants the stubs spell with `#[PhpStormStubsElementAvailable]` are one signature at the level of the project, as before. The active signature is the first that fits the arguments typed so far, the active parameter follows the commas, a variadic parameter takes every extra argument, and a named argument selects the parameter of that name. A signature has the native types of the declaration, the doc comment's summary and description, and each parameter's `@param` text.

### Call hierarchy and type hierarchy

`prepareCallHierarchy` takes a function or method at a declaration or a usage. Incoming calls are the usages of the symbol (so the callers of an interface method include the calls through its implementations, and the `new` of a constructor), grouped by the function or method they sit in; code outside any function is one item for the file. Outgoing calls walk the body, resolve every call and `new` with the type layer and group them by callee, with the called names as ranges. A closure's calls belong to the function around it.

`prepareTypeHierarchy` takes a class at a declaration or a usage. Supertypes are the parent class, the interfaces and the traits it uses, subtypes are the classes that extend, implement or use it directly. `lsp-types` has no field for the capability yet, so `typeHierarchyProvider` is added to the `initialize` answer by hand.

### Semantic tokens

`textDocument/semanticTokens/full` and `/range`. Keywords, strings, numbers and comments are left to the editor's grammar. The legend:

| Types | `namespace`, `class`, `interface`, `enum`, `struct` (traits), `typeParameter` (`@template` names), `parameter`, `variable`, `property`, `enumMember`, `function`, `method`, `keyword` (doc tags), `decorator` (the name of an attribute) |
| --- | --- |
| Modifiers | `declaration`, `readonly`, `static`, `deprecated`, `abstract`, `defaultLibrary` (the standard library), `documentation` (inside a doc comment) |

Classes and the like come from the index (a name it does not know is a plain `class`), constants are `variable` with `readonly`, class constants are `property` with `readonly` and `static`, and the declaration of a name carries `declaration`. A variable is a `parameter` when the function it lives in declares it as one. A qualified name is a `namespace` token for its prefix and the type for its last segment, except the name of an attribute, which is one `decorator` token from its first character to its last so an editor draws all of it as the attribute; its arguments keep their own tokens. In doc comments the tags, the classes of the types, the variables, the `@property` and `@method` names, `@see` targets and templates get tokens with `documentation`. A 5,000 line file takes about 60 ms and a typical one 5 to 20 ms. The server asks the client to refresh the tokens when the index changed (`workspace/semanticTokens/refresh`).

### Inlay hints

`textDocument/inlayHint` for a range, with two kinds:

- **Parameter names** in front of positional arguments, as `name:`. They are left out when the argument already says it: a variable, property, constant or call named like the parameter (`$user_id` for `$userId`, `$this->name`, `getName()`), a parameter with a one letter name or one that starts with an underscore, a function that takes one argument unless the argument is a bare `true`, `false` or `null`, an argument after a named or spread one, and the arguments of a variadic parameter. Accepting a hint inserts `name: `. These rules follow the common ones of editors built on the IntelliJ Platform, from what the platform itself documents; the PHP plugin's own rules were not available to compare, so expect small differences.
- **Closure parameter types** for a closure or arrow function whose parameter has no type, from the `callable(User): bool` or `Closure(...)` that the function being called declares for it, with the templates of the call bound. A type that is only built-in types can be accepted and inserts `type `.

### Completion of overridable methods

After `function ` in a class, enum or trait body, completion offers the methods of the parents, interfaces and abstract traits that the class does not declare yet and that are neither private nor final, with what must be implemented first. The inserted text is the whole method: the name, parameters with native types, defaults, by-reference and variadic markers, the return type, and a body in the indentation of the line (`return parent::name($a);` for a method that has a body above, an empty body for an abstract or interface method, a semicolon after `abstract`). Classes in the signature come with their `use` imports, a missing visibility (and `static`) is added in front of the declaration, and when the parentheses are already typed only the name is completed.

### Memory

The cache file (`<storagePath>/cache/project-<key>.bin`) has a header, an index and the declarations of every file one after the other. The index has, per file, its path, its stamp and hash, the place of its declarations and a summary: the names of its classes (with kind, abstract and deprecated flags, the level they exist at and the classes they extend, implement or use), functions and constants. A warm start reads only the index, which is what the server needs to find a class by name, to list names for completion and workspace symbols, and to know what stands below a class. The declarations of a file are read from the cache file the first time something asks for them (a lookup of one of its classes, a hover, a rename) and stay in memory while they are in use. The server lets go of the ones used longest ago when more than 1,500 are loaded, checked every three seconds, and reads them again when asked. Files that are open, files that changed since the last run, and projects without a storage folder keep their declarations in memory as before. A file whose declarations cannot be read from the cache any more is read from its source, and a class found at a place that no longer holds it (the file changed since it was indexed) is not returned.

On the first run the declarations of a file are not kept at all: the file is read again when something asks, until the cache file is written, and then every file is read from there. The old cache file is released when the new one replaces it.

Open documents keep their syntax tree, and no other tree is kept. Strings are not interned: after this change the declarations that stay resident are a few names per class, which is small next to what was dropped (the signatures, types and doc comments of 9,000 files).

`cargo run --release -p php-index --example bench` prints the resident size next to the indexing times, and `scripts/measure-memory.py` starts the real server on a project and prints it after indexing and after a few requests.

### Trying it on a real project

```sh
cargo run --release -p php-analysis --example probe -- <project> <stubs> <file> <line> <column> [complete|hover|definition|references|rename:<name>|tokens]
cargo run --release -p php-analysis --example stress -- <project> <stubs> [limit]   # every analysis at every name, looking for panics
cargo run --release -p php-analysis --example typecov -- <project> <stubs> [--under <folder>] [--top <n>]   # how many expressions the type layer cannot type
cargo bench -p php-index                  # extraction and index building over the stubs
cargo bench -p php-analysis               # completion and hover over the stubs
cargo bench -p php-analysis --bench features   # tokens, hints, signature help, usages and rename
```

## Phase 3: inspections, fixes and formatting

### Inspections

Diagnostics from the index and the type layer, in the spirit of PHPStan and Psalm, are published next to the syntax errors and language level findings. Each has a code, a default severity and a default state, and every one can be switched off or given another severity with the `inspections` key of the configuration:

```json
{ "inspections": { "unused-import": "off", "undefined-class": "warning", "deprecated": { "severity": "hint" }, "missing-strict-types": true } }
```

A value is `true` or `false`, a severity (`error`, `warning`, `information`, `hint`, or `off`), or an object with `enabled` and `severity`.

| Group | Codes |
| --- | --- |
| Names | `undefined-class`, `undefined-function`, `undefined-constant`, `undefined-class-constant`, `undefined-method`, `undefined-property`, `undefined-variable`, `undefined-named-argument`, `this-in-static-context` |
| Unused code | `unused-import`, `unused-private-method`, `unused-private-property`, `unused-private-constant`, `unused-variable`, `unused-parameter`, `unreachable-code` |
| Calls and types | `wrong-argument-count`, `argument-type-mismatch`, `return-type-mismatch`, `missing-return`, `incompatible-comparison`, `assignment-in-condition`, `deprecated`, `static-call-of-instance-method`, `instance-call-of-static-method` |
| Classes | `abstract-method-not-implemented`, `interface-method-not-implemented`, `incompatible-override`, `readonly-reassigned`, `enum-misuse` |
| PHPDoc and style | `phpdoc-unknown-parameter`, `phpdoc-type-mismatch`, `missing-strict-types` (off by default) |
| Tests | `missing-data-provider`, `missing-test-dependency`, `data-provider-arity`, `missing-double-method`, `double-return-type-mismatch` |
| Frameworks | `unknown-config-key`, `unknown-route`, `unknown-view`, `unknown-template`, `unknown-translation` |

The inspections report only what is certain. A name is undefined only after the project, its packages and the standard library of the extensions it requires have all been read, and a call is not checked when it reaches the callee through `__call`, which is how a method lent by a `@mixin` is called. `cargo run --release -p php-analysis --example survey -- <project> <stubs> [--all]` runs every inspection over a project and counts what they report, to find the ones that are wrong on code that is right. Over the 798 PHP files of a real project it takes about 3.7 s and reports 66 findings with the default set, none of them wrong when read by hand. The first run reported 68: a call through a `@mixin` and the classes Composer writes into `vendor/composer` were wrong, and both are fixed.

### Quick fixes and code actions

`textDocument/codeAction` offers, with the edits sent along or resolved on demand (`codeAction/resolve`):

- quick fixes: import a class, function or constant, or write its name in full; remove an unused import, declaration or variable (keeping the call when the assignment has a side effect); remove unreachable code; replace `=` with `===` in a condition; remove a `@param` of a parameter that does not exist; implement the missing methods of an abstract class or interface; create a missing method or property; add `declare(strict_types=1)`; make a member public, protected or private where an override needs it;
- intentions: add or update a PHPDoc, add a return type from what the body returns, change the visibility of a member, make a property readonly, convert a constructor and its properties to constructor property promotion;
- `source.organizeImports`.

`textDocument/onTypeFormatting` also writes the doc block after `/**` and Enter, with a `@param` per parameter and the `@return`.

### Formatting

`textDocument/formatting`, `textDocument/rangeFormatting` and `textDocument/onTypeFormatting` (on `}`, `;` and a new line) are answered by `php-format`. The defaults are PER Coding Style 2.0, and the indent follows the `FormattingOptions` the client sends.

The formatter moves whitespace and nothing else. It decides the text between two tokens with a rule per pair of neighbors, writes the indentation from the brackets and unfinished statements around a line, and keeps the line breaks the code has where PER has no opinion (arguments, chains, array items), with at most one blank line in a row. Four things follow from that:

- **Lossless.** Comments, strings and heredocs are never touched, apart from the indentation of the later lines of a block comment, which moves with its first line. Before an edit is sent the result is parsed again and compared with the original token by token, and a text whose tokens would differ is left as it is.
- **Idempotent.** The second pass over a formatted text changes nothing. This holds because every decision is made from the structure of the tree and the line breaks that are there, never from the columns of the old layout.
- **Safe on what it cannot read.** A text with syntax errors or with markup around its PHP is not formatted, and `textDocument/formatting` answers `null`. `onTypeFormatting` does lay out unfinished code (a new line gets the indentation of the open brackets above it), since that is when it is asked.
- **Range formatting follows the whole file.** The layout is worked out for the whole text and only the edits in the lines asked for are sent, so a line comes out as it would in a full format.

What PER decides is applied: one blank line between header blocks (`declare`, `namespace`, the `use` block) and between top-level declarations, methods separated by a blank line, the brace of a class or method on its own line, `{}` for an empty body, and `) {` together on one line after a parameter list that runs over several lines. A blank line at the start or end of a class body is kept; one at the start or end of a function or control structure body is removed. The spacing of casts (`(int)$x` or `(int) $x`) is left as written, since the specification does not say.

The options come from the `format` key of the configuration, besides the indent that the client sends:

| Key | Values | Default |
| --- | --- | --- |
| `classBrace` | `nextLine`, `sameLine` | `nextLine` |
| `functionBrace` | `nextLine`, `sameLine` | `nextLine` |
| `blankLinesBetweenMembers` | a number, the blank lines around a method | `1` |
| `alignAssignments` | pad the `=` of assignment statements on consecutive lines to one column | `false` |
| `alignArrayArrows` | pad the `=>` of array items on consecutive lines to one column | `false` |
| `lineLength` | a line longer than this is wrapped (see below); `0` never wraps | `120` |
| `editorconfig` | read `indent_style`, `indent_size`, `tab_width` and `max_line_length` from the project's `.editorconfig` | `true` |

A line that runs past `lineLength` is wrapped at the outermost construct that sits on that line and crosses the limit, and the line is looked at again until nothing more can be broken. The constructs, the one that starts furthest left first: the operands of a long binary chain (`.`, `&&`, `||`, `and`, `or`, `??`, `+`, `-`), which break before each operator; the two sides of a ternary, which break before `?` and `:`; a method chain of at least two calls (or three links), which breaks before every `->` but the first when it starts at a variable; the items of an array literal; and the arguments or parameters of a call or a function. The condition of an `if`, `elseif` or `while` that is broken goes on lines of its own between its parentheses, with `) {` together on the last line, as PER asks. Nothing is added to the tokens: a trailing comma is not written, and a construct that already runs over several lines is left as its author broke it.

The indent and the line length can also come from the project. The `.editorconfig` files from the folder of a file up to the first one that says `root = true` are read with the globs of the specification (`*`, `**`, `?`, `[...]`, `{a,b}`, `{1..3}`), the nearer file winning, and `indent_style`, `indent_size` (a number or `tab`), `tab_width` and `max_line_length` (a number or `off`) are laid over the options. The client always sends a tab size and `insertSpaces`, which says nothing of what a project wants, so `.editorconfig` goes before them; the `format` keys of the configuration, such as an explicit `lineLength`, go before `.editorconfig`; `"editorconfig": false` switches it off. Code a refactor writes follows the same options, with the indent the file already uses ahead of the client's.

Held to the corpora (`cargo run --release -p php-format --example check -- <folder>... [--tabs --wrap --align --width <n>]`): every PHP file of phpstorm-stubs, the files of a real project and the `--FILE--` sections of 6,180 php-src tests keep their tokens and format the same twice, with the defaults and with tabs, line lengths of 40, 60, 80 and 120 and alignment all on. A test in the default run does the same over the stubs when they are fetched.

## Phase 4: refactors

### How they are offered

Every refactor is a code action of kind `refactor.extract`, `refactor.inline`, `refactor.rewrite` or `refactor.move`, offered only where it applies, so a cursor on something it cannot change shows nothing. The edits are worked out over the lossless tree and then formatted: the text of each file is written with the refactor applied, `php-format` lays out the lines the edits touched (and only those), and what reaches the client is the few edits that lead from the old text to the new one. Comments and the layout of everything else stay. `use` statements that the change leaves with nothing to import are removed, and only those. The expensive ones (everything but the rewrites) are sent without their edit and resolved by `codeAction/resolve`, where a refusal comes back as an error with its reason; a client that cannot resolve gets the edits at once and the refactors that refuse are left out. A refactor that works across files returns a workspace edit with `documentChanges` (and `rename` operations for a file that moves) or, for a client without them, `changes`.

A refactor that writes a name the person is expected to change (extract variable, method, constant, field and parameter, add parameter) inserts a name from the expression and its type (`getUser()` is `$user`, `new DateTime()` is `$dateTime`, a string is `$string`; a taken name gets a number). The client can use it in two ways. With `workspace.workspaceEdit.snippetEditSupport` the edit that writes the name is a snippet text edit with the name as the first placeholder (`${1:user}`, `$` escaped). Otherwise the action carries a `command` named `php.rename`, with the document and the position of the name in the text after the edits, which the client runs after applying them (the server answers `workspace/executeCommand` for it with `null`). A client that does neither still gets a program that works.

### The refactors

**Extract variable.** An expression, the selection or what is around the cursor, becomes a variable assigned right before its statement. Without a selection the outermost expression that is worth a name is taken, with the inner ones offered next to it, and with two or more equal expressions in reach a second action replaces them all. Refused when the expression runs only sometimes (right of `&&`, `||`, `??`, `and`, `or`, one side of a ternary or a `match`, `elseif` and loop conditions, `case` labels), sits in an arrow function, inside `isset`, `empty` or `unset`, after `@`, on the left of `??`, in a string's simple interpolation, in a place that only takes constants (default values, property and constant initializers, attributes), under a body without braces, is written to or called, or when something with an effect runs earlier in the statement and the expression is not constant or made of local variables nothing earlier can touch. The occurrences a second action replaces must be pure, in the same block after the first, and not follow a write to what the expression reads, a loop's whole body counting when the occurrence is inside it.

**Inline variable.** A local assigned once, in a statement of its own, is replaced by its value in every read, and the assignment goes. Parentheses are added by precedence. Refused (or not offered) for parameters, variables assigned or updated elsewhere (`$x[] = 1`, `$x->p = 1`, `++`, passing by reference), variables used by name (`compact`, `extract`, `$$x`), reads before or outside the block of the assignment, a read inside a closure's `use`, a value that does something when it would be copied, moved past other effects, past a `return` or `break`, into a condition that runs sometimes or into a closure, a value whose inputs change before a read (a loop counts whole), and a value that does not fit a place such as `isset($x)` or the inside of a string.

**Extract method.** One expression or a run of whole statements becomes a private method after the current one (a function when cut from a function), named `get<Thing>` or `extracted`. What the code reads and has before it becomes the parameters, typed from the type layer in the file's own style (a parameter of the function keeps its declared type); what it sets and is read afterwards is returned, a list `[$a, $b]` when it is more than one; `$this` and static context follow the original; a run that always returns is returned from; a run that only throws is `never` from PHP 8.1; `yield` becomes `yield from` and the method a generator; by-reference parameters stay by reference. A doc block is written when the method it was cut from has one or most methods of its class do. Refused when the code is in a closure or an interface, binds references (`=&`, `use (&$x)`, `foreach ... as &$v`), declares things, uses `global`, `static` or `goto`, reads variables by name, leaves a loop it does not contain, returns on some paths only or with a value and without, yields and hands values back, or when an expression changes variables read afterwards.

**Extract constant, field and parameter.** A constant expression becomes a class constant after the last one (`private`, `public` in an interface), with a second action for every equal expression of the class. A field is `private` and typed, initialized inline when the expression is constant, or in the constructor, which is added after the properties when there is none; it is static in a static method (inline only). A parameter is added to the function and every override, the expression is replaced and every call gets the argument (named when a call leaves out arguments before it; names in the argument are written in full for other files); after optional parameters the new one takes the expression as its default instead. Refused: a field or parameter that needs a variable the constructor or the call does not have, `$this` or `self` in a parameter, an expression that does something and sits in a loop or runs sometimes, a variadic function, a function declared in `vendor/`, calls that spread their arguments, and uses that are not calls.

**Inline method.** One call, or every call and the method with them. A body of one `return` goes anywhere the call stands, with the arguments where the parameters were (an argument that does something must be read once and first); a few statements (up to ten, ending in at most one `return`) go where the call is a statement of its own, the value of a last `return` going to `$x = call;` or `return call;`, parameters that cannot be used as they are become local variables and locals that clash with the caller are renamed. A body that uses its object only goes to calls on `$this`, `self` or `static` from its own class; the names in a body copied to another file are written in full. Refused for generators, recursion, by-reference or variadic parameters, `static` and `global`, closures that use a parameter, early returns, a method that is overridden or implemented elsewhere, references that are not calls, calls inside the arguments of another, and declarations in `vendor/`.

**Change signature.** Intentions on a function or method, applied to every override, implementation and call that references find (named arguments included), or not at all. Add a parameter with a default (`$parameter = null`, with a `@param` line when the doc block has them). Remove an unused parameter (the `@param` line goes with it; refused when an override uses it, a call passes something that does something, or a call spreads). Move a parameter left or right (arguments swap; a call that leaves out the argument the new order needs gets the default of the other parameter when it is a literal, else a name; refused when a required parameter would follow an optional one). References that are not calls (callable strings and arrays), declarations in `vendor/` and calls whose receiver the type layer does not know are the limits: a call that is not found is not changed.

**Move class.** A code action on a class name moves the class to another namespace of its Composer root (the namespaces that classes of the project already have, nearest first), by moving the file to the PSR-4 path and updating the `namespace` statement, every `use` (a name in a group gives the class up to a statement of its own), every qualified or fully qualified reference and every unqualified one that relied on the old namespace (an import is added, or the name is written in full where the short name is taken), and, inside the moved class, the names that relied on the old namespace. Where a file and its namespace disagree, the file moves to the namespace's path or the namespace follows the file. Strings that hold a class name are not changed. `workspace/willRenameFiles` does the same when a client moves or renames a PHP file or a folder: the namespace and the class name follow the path when the file followed its PSR-4 map before, and the edits are returned for the client to apply before it moves the file (a folder moves every class below it together). Refused: a target that is taken, a file with more than one class or namespace, a block namespace moved to the global one, a path no PSR-4 map reaches.

**Pull members up and push them down.** On a method or property of a class. Pull up moves it into the parent (private becomes protected, names are imported in the parent's file, refused when the member needs something the parent lacks, calls `parent::`, or the parent already has it); declare in an interface or abstract parent keeps the method and adds its signature (refused when another implementer would be left without it). Push down moves a member into every direct subclass (refused when it is private or abstract, calls `parent::`, or is used anywhere but in the member or through `$this` below). A target in `vendor/` is always refused.

**Rewrites** (`refactor.rewrite`, edits sent at once): `array()` and `[]`; `if`/`else` as a ternary (both branches one `return`, assignment to the same variable, or `echo`) and back; `switch` as `match` (every case one `return`, `echo` or assignment to one variable, a `default`, and every label of the very type of the subject, which has to be known to be an int, a string or an enum, since `match` compares strictly); concatenation as an interpolated string or `sprintf` and back (`%s` and `%%` only); braces added to or removed from a body of one statement (not when a comment or a dangling `else` is in the way); a declaration of several properties, constants, `static` or `global` variables or `use` clauses split, and adjacent property or constant declarations joined; a comparison flipped (refused when both sides have effects); an `if` with an `else` inverted; the arguments of a call named, from the cursor on, and the names given up when they stand where their parameter is (PHP 8.0 and up).

### Trying them on a real project

`cargo run --release -p php-analysis --example refactor_smoke -- <project> <stubs> [--every <n>] [--per-file <n>] [--title <prefix>] [--show <n>] [--dump]` offers every refactor at a sample of positions of the project's files (a caret and the range of each statement, expression, declaration and parameter, and runs of two or three statements), applies each, and checks the result: every file it changed parses with no more errors than before, and the inspections report nothing they did not report before (the files that moved are inspected at their new place, and the index is updated with the new texts first). What it found while this phase was written is fixed and has a test. Adding a parameter is the one refactor that is expected to bring a finding (an unused parameter, a hint for a private or final method), so the run leaves that one out.

Run over a real application (798 PHP files, vendor trees left out; a sample of positions in each file, the refactors in groups), it made 68,508 offers, applied 55,237 of them and refused 13,271 with a reason. None of the applied ones left a file that parses worse than before or a finding the inspections did not already have.

| Refactors | Files | Offered | Applied | Refused |
| --- | --- | --- | --- | --- |
| Extract variable, constant, field and parameter, inline variable | 798 | 32,508 | 24,084 | 8,424 |
| Extract method | 798 | 9,915 | 5,552 | 4,363 |
| Rewrites | 798 | 25,067 | 25,067 | 0 |
| Inline method, change signature, pull up, push down | 399 | 778 | 294 | 484 |
| Move class | 200 | 240 | 240 | 0 |

Most refusals are what the rules above say: extract method is mostly offered on a selection inside a closure or a top-level script, which it declines, and the rest is code that returns on some paths only. Two kinds of result that the run made visible are written down as limits. A dead assignment to a parameter in the cut code (one nothing reads afterwards) is reported as an unused variable in the new method, where the parameter hid it before, and the types of an extraction are only the ones PHP guarantees (scalars and arrays, literals, `new`, declared types and the parameters of the function itself), since a doc block or the way objects flow is not checked by PHP and a wrong one would become a `TypeError`.

## Phase 5: tests, and what was left of phase 1b

### Closure arguments, returns and assert tags

The type layer part of this phase is described under "The type layer" above. A function with a `callable(T): U` parameter gives the closure handed to it the types its other arguments bound, so `$user` is a `User` in `array_map(fn ($user) => $user->name, $users)` and the call is an `array<int, string>`. Nested array writes (`$byTicket[$id][] = $pivot`) build the element type, and `A|(A&B)` is `A`.

### PHPUnit

A class is a test case when it extends `PHPUnit\Framework\TestCase`, directly or through other classes. Without PHPUnit in the index a parent named `TestCase` counts. A method is a test when it is public, not static, and is named `test...`, carries `#[Test]` or has `@test`.

The strings and tags that name something are followed like names:

| Written | Names |
| --- | --- |
| `#[DataProvider('name')]`, `@dataProvider name` | a method of the class |
| `#[DataProviderExternal(Foo::class, 'name')]`, `@dataProvider Foo::name` | a method of `Foo` |
| `#[Depends('name')]`, `#[DependsUsingDeepClone]`, `#[DependsUsingShallowClone]`, `@depends name` | a test of the class |
| `#[DependsExternal(Foo::class, 'name')]` and its clone forms | a test of `Foo` |
| `#[CoversMethod(Foo::class, 'name')]`, `#[UsesMethod(...)]` | a method of `Foo` |
| `#[CoversFunction('name')]`, `#[UsesFunction('name')]` | a function |
| `#[Group('name')]`, `@group name` | a group, kept per file in the index |

Definition, hover, find usages, highlights and rename (the string is renamed with the method) work on the text inside the quotes, and completion offers the data providers (public methods that are not tests, static ones first), the tests (for a dependency), the methods, the functions or the groups of the project there. `#[CoversClass(Foo::class)]`, `#[UsesClass]`, `#[CoversTrait]`, `#[UsesTrait]` and `#[DependsOnClass]` name a class by `::class`, which is an ordinary class reference that navigates like one, and completing a name in their argument inserts `Foo::class`. A group has no declaration, so it completes and is not followed. `@covers` and `@uses` tags in a doc comment are not read.

Inspections: `missing-data-provider` (error), `missing-test-dependency` (warning) and `data-provider-arity` (warning), which compares the values of each row of a provider with the parameters of its test. It reads only a provider whose rows are written out as arrays of positional values in one `return` or in `yield`s, in this file or another, and stays silent for a test that also has `#[Depends]`, whose results arrive after the data set. A row with fewer values than the required parameters and one with more values than the test takes both count.

Test doubles take their types from what PHPUnit declares: `createMock(Foo::class)` is `MockObject&Foo` through the `@return MockObject&RealInstanceType` of the package, `createStub` is `Foo&Stub` and `getMockBuilder(Foo::class)->...->getMock()` is `MockObject&Foo` through `MockBuilder<Foo>`. The class a double stands for is also found through a call on it (`$mock->expects($this->once())->method('send')`), so the strings of `->method('name')`, `->onlyMethods(['name'])` and `createPartialMock(Foo::class, ['name'])` are followed, completed and renamed like the strings above. `missing-double-method` (error) reports a name the class does not have, and `double-return-type-mismatch` (warning) a value `->method('name')->willReturn(value)` gives that the method's declared return type never takes, only when the type of the value is certain (a literal, `new`, a typed variable).

In a test, completing `$this->` puts the `assert...` methods first, static ones included, as they are called through `$this` all the time.

### Pest

A Pest file is made of calls: `test()`, `it()`, `describe()`, `beforeEach()`, `afterEach()`, `beforeAll()`, `afterAll()`, `dataset()`, `uses()`, `arch()` and `todo()`. They are recognized by name, whatever their namespace.

**`$this` is the test case.** In the closure of a test or a `beforeEach` or `afterEach`, `$this` is the class that `uses(...)->in('Feature')` or `pest()->extend(...)->in('Feature')` binds to the folder of the file, found in `tests/Pest.php` or any other file (the index keeps those calls with the file), and the traits named there (`->use()`, or any trait in `uses()`) are part of its type. `uses(...)` without `->in()` in the file itself binds to that file, and when nothing binds a class it is `PHPUnit\Framework\TestCase`. The deepest folder wins for the class. `__DIR__` and `__DIR__ . '/Feature'` work as folders. The folder of the file has to be known, so a client names the file of a request through the `textDocument` it sends (the server does) and a library user wraps its calls in `php_analysis::document::enter(Some(path))`. Without a path only the bindings of the file itself and the default apply.

**Properties of `beforeEach`.** What `$this->name = value` assigns in a `beforeEach` of the file, or of the `describe` blocks around the test, is a property of `$this` in the tests it runs for, with the union of the types it is assigned (`??=` too). They complete after `$this->`, and `$this->event->startsOn` resolves. A property that is declared on the case class wins, and one that is not assigned in a hook (in the test itself, or by `pest()->beforeEach` in another file) is not known.

**Datasets.** The name in `->with('name')` and the one `dataset('name', ...)` declares are the same thing: definition, find usages, rename and completion (the names of the project and of the file) work on them. A test closure takes the types of an inline dataset (`->with([[1, 'a'], [2, 'b']])`, `->with(['label' => [1, 'a']])`, `->with([1, 2])`) or of a named one the project declares as an array or as a closure returning one, for the parameters that declare no type of their own, when every row is written out. Two `->with()` on one test, or a row that is not written out, give nothing.

**Expectations.** `expect($value)` and its chain follow what Pest declares in its own source (`Expectation<TValue>`, the `@property` of `not` and `each`, the `@mixin` of the expectation methods), so nothing about them is written into the server. An expectation a project adds with `expect()->extend('name', fn)` completes after `expect(...)->`, hovers and leads to its string, and a call of it gives the expectation back; inside that closure `$this` is the expectation. Usages and rename of a custom expectation, and checks on its arguments, are not offered.

Inside a Pest closure `$this->name` and `$this->method()` are not checked for existence: the generated case class forwards what it does not have through `__call`, and a property can come from a hook the server does not read.

### What a test file can run

The server runs nothing. It says what is runnable, and the client runs it.

`php/runnables` is a custom request with `{ "uri": "file:///project/tests/FooTest.php" }` (`{ "textDocument": { "uri": ... } }` also works), which needs the document to be open. It answers a list in file order:

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

- `kind` is `phpunit`, `pest`, `artisan` or `console`. `scope` is `class`, `method`, `test`, `describe`, `arch` or `command`. A command (a class that extends Laravel's or Symfony's `Command` and names itself in `#[AsCommand]`, `$signature`, `$name` or `$defaultName`) has the name of the command as its `label` and `filter`, which the client runs as `php artisan <filter>` or `bin/console <filter>`, and `Run command` as the title of its lens.
- `range` is the name of the class or method, or the Pest function that declares the test, where a run marker goes.
- `filter` is a delimited regular expression for `--filter`, which PHPUnit and Pest both take. PHPUnit matches it against `Class::method with data set "x"`, so a class is `/^Ns\\Class::/` and a method adds `( with data set .*)?$`. Pest turns the description of a test into the name of a method (`it does x` is `__pest_evaluable_it_does_x`, an underscore is doubled, any other character that is not a letter or a digit becomes an underscore, and the describe blocks around a test are prefixed as `` `outer` → ``), which the filter matches, so it does not hold the class and is meant to be run on `file`. A `describe` is the prefix of its tests. An `arch()` without a description is named after the calls chained to it, so its filter is the prefix of the first of them and reaches every test of the file that starts so. A test whose description is not a string is not listed.
- `file` is the path of the file and `configFile` the closest `phpunit.xml`, `phpunit.xml.dist` or `phpunit.dist.xml` from its folder up to the workspace folder, or `null`.

`textDocument/codeLens` answers the same list for a client that only knows lenses: one lens per runnable on its `range`, with the command `php.runTest` and the runnable as its only argument, titled `Run tests in class`, `Run group` or `Run test`. The server does not list `php.runTest` in `executeCommandProvider`, since it cannot run it. The client registers the command, runs `vendor/bin/phpunit` or `vendor/bin/pest` with `--configuration configFile --filter filter file`, and shows the result.

### Measured on a real project

`cargo run --release -p php-analysis --example typecov -- <project> <stubs>` counts the expressions of the project's own files (variables, calls, property reads, index reads and static reads) whose type is unknown or `mixed`. On the Pest backend this was developed against (798 files, 71,901 expressions, 22,178 of them the object of a member read or an index read):

| | Before phase 5 | After |
| --- | --- | --- |
| Unresolved, all files | 15,890 (22.1%) | 4,823 (6.7%) |
| Unresolved, `tests/` | 13,963 (32.0%) | 2,919 (6.7%) |
| Unresolved, objects of a member read | 5,002 (22.6%) | 1,572 (7.1%) |

Nearly all of the gain is `$this` in Pest closures, `$this->event->startsOn` and the like (about 11,000 expressions); closures typed through templates, return types and nested array writes add a few dozen on this code, which declares most of its types. `survey` (the inspections over the project) reports the same 66 findings before and after, all of which are real (unused imports and variables), so nothing new is reported on the tests. Running the inspections over the project takes about 1.8 times as long as before, since far more of what they read now resolves.

## Phase 6: Laravel and Symfony

Nothing here activates in a project that does not install the framework. `php-index` reads `composer.json` and the installed packages (`Frameworks::detect`): `laravel/framework` or `illuminate/*` turn on the facades and Eloquent (and, with the framework itself, the conventions of an application), `symfony/framework-bundle` turns on Symfony, `doctrine/orm` the repositories. A project with none of them reaches none of this code, and its numbers did not move (below).

What the frameworks declare themselves is read like any other code: docblocks, `@template` and `@extends`, attributes, the arrays of `config/` and `vendor/`. The type layer already follows generics, so `Builder<TModel>`, `HasMany<TRelatedModel, TDeclaringModel>` and `Collection<TKey, TValue>` need nothing. What no declaration says is in two overlays, PHP files that are read and never run: `crates/index/src/framework/laravel_overlay.php` and `symfony_overlay.php`. The tags on a function or method are the data: which argument names a config key, a route, a view, a service or a template (`@key route`, `@key service $service`), which call hands out the container's services (`@container`), the logged in user (`@user`), a repository (`@repository`), which class passes what it lacks on to another (`@forwards`), and the Blueprint methods a migration is read with (`@column`, `@columns`, `@morphs`). A few base classes are named in code (`Model`, `Builder`, `Relation`, `Facade`, `EntityRepository`), since the magic hangs on them.

### Members that are made up

Members that exist only at run time are made up by the index when a type is asked for its methods and properties (`framework/mod.rs`, called from `hierarchy.rs`), so hover, completion, navigation, signature help and the type layer all see them. A name asked for is looked for alone (`find_method` no longer builds the list of every method first), which also made every other member lookup cheaper.

**Laravel facades.** A class that extends `Facade` forwards to the class its `getFacadeAccessor()` returns: a class name, or a string that `Application::registerCoreContainerAliases()` and the project's providers map to one (both read from the code), else the class the facade's `@see` names. The methods come back as static calls, never reported as undefined, with `static` bound to the real class. A facade that documents its methods with `@method static` keeps those.

**Eloquent models.**

- Attributes. The columns of the table come from `database/migrations` (`Schema::create` and `Schema::table`, in file order, with `dropColumn`, `renameColumn`, `change()`, `nullable()`, `morphs`, `timestamps`, `softDeletes`, `foreignIdFor` and the rest of the Blueprint) and from a `database/schema/*.sql` dump that stands for the migrations before it. The table is `$table`, `#[Table(name:)]`, else the snake case plural of the class (`Str::pluralStudly`'s rules). `$casts` and `casts()` give the type (`datetime` is `Carbon`, an enum cast is the enum, a class that implements `CastsAttributes<TGet, TSet>` is `TGet`), a nullable column is `?T`, `created_at` and `updated_at` are `Carbon` unless `$timestamps` is false, and `deleted_at` with `SoftDeletes`. Accessors (`getFooAttribute()`, and `foo(): Attribute` with a typed `get:` closure), `$appends` and `#[Appends]` are properties as well. `@property` docs keep working.
- Relations. A method that returns a `Relation` is a property, typed from `Relation::getResults()`'s template (`HasMany<Post, $this>` gives `Collection<int, Post>`, `BelongsTo<User, $this>` gives `?User`). A relation method that declares only `HasMany` is read from its body (`return $this->hasMany(Post::class)`, bound through the `class-string<TRelatedModel>` of `hasMany`), also for the call `$user->posts()->create(...)`, which is a `Post`.
- Scopes. `scopeActive()` and `#[Scope]` methods are methods of `Builder<User>` and static methods of `User`, without their builder parameter, and the untyped first parameter of a scope is a `Builder<Model>` in its own body.
- `where{Column}` is a method of the builder for every known column, as a completion and as a type.
- A model forwards what it does not declare to `Builder<static>` (what `newQuery()` returns), as an instance call and as a static one, so `User::query()->where(...)->first()` is `?User` and `User::where(...)->get()->each(fn ($user) => ...)` types `$user`.
- Factories. `User::factory()` is the factory `@use HasFactory<UserFactory>` names, else the one `#[UseFactory]` names, else `Database\Factories\UserFactory` by Laravel's own rule. A factory with no `@extends Factory<Model>` is bound to its `$model` or to the model its name stands for.
- `Auth::user()`, `auth()->user()` and `$request->user()` are the model `config/auth.php` names (`?User`), and `auth()` forwards what its contract lacks to the guard.

**The container.** `app(Foo::class)`, `resolve()`, `app()->make()` and `$this->app->make()` are `Foo` through the conditional types of the framework (a conditional whose argument was left out now uses the default of its parameter, and `class-string` is understood). A string gives the class of the framework's own aliases (`app('cache')`) and of a binding a service provider of the project makes with literal names (`$this->app->singleton('billing', fn () => new Billing)`, `bind(Foo::class, Bar::class)`, `alias`). In Symfony `$container->get('app.mailer')` follows `config/services.yaml`, `services.php`, aliases and the `resource:` entries that make every class of a folder a service.

### Strings that name things

The argument of a function or method that the overlay marks is followed like a name: completion of the names the project declares, go to definition (to the line of the key, the route, the template, the `.env` line), hover, and for some an inspection. Each inspection stays silent unless it is certain, and each says what makes it uncertain.

| Kind | Where the names come from | Reported as unknown when |
| --- | --- | --- |
| Laravel config key (`config()`, `Config::get`, `Repository`) | the arrays of `config/**/*.php`, nested keys with dots | the file exists, every array on the way is written out, no package merges config into that file (`mergeConfigFrom` in a vendor provider) and the project does not write it while it runs (`config([...])`, `Config::set`); never with a default argument or `has()` |
| Laravel route (`route()`, `to_route()`, `URL::route`, `redirect()->route`) | `routes/**/*.php`: `->name()`, the prefixes of `name()->group()` and `['as' => ...]`, resources with `only`, `except` and `names`, files that are `require`d inside a group | every route file was followed (no computed names, no loops) and no vendor provider registers routes |
| Laravel view (`view()`, `View::make`, `Route::view`, `response()->view`) | files below `resources/views`, `.` or `/` | the name has no `::` and no provider adds view locations |
| Laravel translation (`__()`, `trans()`, `Lang`) | `lang/<locale>/*.php` (folders included) and `lang/<locale>.json` | the key starts with a group file that exists, has dots and no spaces, and no locale has it |
| `env()` | the names of `.env*` files, never their values | never |
| Abilities (`Gate::allows`, `can`, `authorize`, `@can`) | `Gate::define` in the project's providers and the public methods of `*Policy` classes | never |
| Fields of a form request (`$request->input('x')`, `validated('x')`) | the keys of the array `rules()` returns, for the receiver's class | never |
| Symfony route (`generateUrl`, `redirectToRoute`, `UrlGenerator::generate`) | `#[Route]` (with the `name:` prefix of the class, and the name Symfony makes up for a route without one), `config/routes*.yaml` and `.php`, and the files a bundle's `@Bundle/...` import points at | every import was followed (`type: service` loaders other than the logout route are not) |
| Symfony template (`render`, `renderView`, `Environment::render`) | `templates/**/*.twig` | the name does not start with `@` and `twig.yaml` adds no path |
| Symfony translation, parameter, service, event | `translations/<domain>.<locale>.yaml\|xlf\|php`, `parameters:` of the YAML and PHP configuration and the kernel's own, the ids of `services.yaml`, the `@Event` constants of the `*Events` classes and the classes of events | never |
| Doctrine entity field (`findBy(['x' => ...])`, `findOneBy`, `count`) | the properties with a mapping attribute of the entity the repository serves | never |

`%name%` and `%env(NAME)%` inside the strings of `#[Autowire]` (`'%kernel.project_dir%/x'`, `env: 'APP_SECRET'`, `param:`, `service:`) lead to the parameter, the `.env` line and the service. `#[AsEventListener(event: '...')]`, `dispatch($event, 'name')`, `addListener('name')` and the keys of `getSubscribedEvents()` complete the events. In `$builder->add('name', |)` and `createForm(|)` the class completion offers the form types as `TextType::class`.

### Doctrine

`#[ORM\Entity(repositoryClass: UserRepository::class)]` makes `$em->getRepository(User::class)` a `UserRepository`. The finders of a repository (`find`, `findAll`, `findBy`, `findOneBy`) give the entity (`?User`, `list<User>`) from the `ObjectRepository<T>` the repository implements, with `T` read from `@extends ServiceEntityRepository<User>` or from the entity that names the repository, and `findByEmail`, `findOneByFirstName` and `countByEmail` exist for the fields of the entity. A `Collection` property of a `OneToMany` or `ManyToMany` with a `targetEntity` is a `Collection<int, Target>`. DQL strings are not read.

### Blade

A document with the language id `blade` (or a path ending in `.blade.php`) is not a PHP file with errors: the server reads the names and the PHP in it, and nothing else. `@extends`, `@include`, `@includeIf`, `@includeWhen`, `@includeUnless`, `@component`, `@each`, `@lang`, `@choice`, `@can` and `@cannot` complete and lead to the view, the translation or the ability (which directive takes which argument is the overlay's `BladeCompiler` entry), `<x-alert>` and `</x-forms.input>` lead to the template under `resources/views/components` or to the class under `App\View\Components`, and the PHP in `{{ }}`, `{!! !!}`, `@php ... @endphp` and the arguments of a directive is read as a small document of its own, so classes, static calls, facades and the strings above work in it (definition, hover, completion). It publishes no diagnostics, and it does not know the variables a controller passes or what a directive does to the scope: `$user` in a template is unknown unless it is declared in a `@php` block. Layouts and slots, `@props`, Livewire tags and a model of the whole template are left.

### Measured

`php-analysis`'s `typecov` example counts the expressions of the project's own files that the type layer cannot type (unknown or `mixed`), and `survey` runs every inspection. Before phase 6 is the binary of the commit before it, on the same project and stubs.

| Project | Files | Unresolved before | After | Receivers before | After |
| --- | --- | --- | --- | --- | --- |
| Laravel 13 skeleton plus models, relations, scopes, a policy, a form request, routes, views and translations | 45 | 42.9% | 31.4% | 14.4% | 0.5% |
| `laravelio/laravel.io` (Laravel 11, Eloquent, Pest, Livewire) | 452 | 35.6% | 14.9% | 34.7% | 5.8% |
| Symfony 8.1 skeleton plus entities, repositories, a form, services, a subscriber | 17 | 8.9% | 8.1% | 0.0% | 0.0% |
| `symfony/symfony-demo` | 51 | 3.2% | 3.2% | 0.4% | 0.4% |

What is left in the Laravel numbers is `env()` and `config()` (mixed by nature), the Pest functions and `$this` of a test (`test()`, `it()`, `$this->get()`), and `Cache::remember` and the like, which the framework documents as `mixed`. `survey` reports nothing on any of them that is not a real finding: on the skeleton and the Symfony projects only unused variables, an undeclared `authorize()` in a controller without the trait, and names that do not exist (a view, a config key, a template). On `laravel.io` it reports three static methods called through an object (`Factory::times()` and a model's own `mostSolutions()`), which are real and which it could not see before.

Speed and memory, on Passly's backend (798 own files, 9,288 with packages), which uses neither framework:

| | Before | After |
| --- | --- | --- |
| Indexing, no cache | 389 to 513 ms | 385 to 404 ms |
| Resident after indexing | 341 to 347 MB | 350 to 352 MB |
| `survey`, every inspection over the project | 4.6 s | 2.9 s |
| `stress`, every analysis at every name (175,084 positions) | 144 s | 100 s, 0 failures |

The speed comes from one change that is not about frameworks: looking a method or property up by name no longer builds the whole list of members first. It undoes most of what phase 5 added to the inspections. Indexing a Laravel (8,086 files) or Symfony (7,628 files) project takes as long as before, since the framework layer reads its files when a name asks for them (a section of the index, built once and dropped when a file it read changes): the first config key, route, model or container name of a Laravel project takes about 2 ms, and completion of a model's members about 5 ms.

### Limits

- Nothing is run. A config key set by code that is not a literal, a route made in a loop, a binding made with a computed name, a view path added by a package, and a translation key that is words, are never reported.
- Eloquent knows the table by migrations and schema dumps only: a column added by raw SQL, a table another connection owns, a `$table` computed at run time and attributes a trait adds in its `initialize...()` (except `deleted_at` with `SoftDeletes`) are not known. A cast class that is `Castable` and not `CastsAttributes` gives `mixed`. An accessor with no declared type on its `get:` closure gives `mixed`. A relation built from a string (`hasMany('App\\Models\\Post')`) or by a macro gives the relation without its model. `Factory::create()` is `Collection<int, Model>|Model`, as the framework documents it, since `count()` is not followed. `$user->posts()->...` through `__call` to the query builder is typed by the builder's `@mixin` and nothing else.
- Facades: a facade whose accessor is not a literal or a class constant has no target. `Auth::guard('api')->user()` is the model of the first provider of `config/auth.php`, not the one of that guard.
- The container: `$app['config']`, `bound()`, contextual bindings and a binding made outside a provider are not read. `#[AsService]` does not exist in Symfony; `#[AsAlias]` and `#[Autowire]` are read, `#[Autoconfigure]`, `#[AsTaggedItem]` and tags are not.
- Symfony routes without a literal name, `routing.yaml` loaders of other bundles and XML other than `<route id=>` are not followed, and the route is then not judged. Names in Twig templates and in YAML files (`%name%`, `@service`, translation keys) are not completed, since the server reads PHP documents and Blade.
- Twig itself is not read: `{% include %}`, filters and variables have no completion.
- Blade has no model of a template: the scope of `@foreach`, `@props`, slots, `@livewire` and component attributes are not followed, and a PHP document inside a directive cannot import a class.

### Trying it

`cargo run --release -p php-analysis --example typecov -- <project> <stubs>` and `--example survey` open the project with its Composer metadata, so the framework layer is on when the project installs it. The default test run holds unit tests of every catalog (`crates/index/src/framework/*`), of the overlay, of the types the layer gives (`crates/analysis/src/frameworks/tests.rs`), of the Blade reading (`blade.rs`), and end-to-end tests over the in-memory connection (`crates/server/tests/frameworks.rs`) for Laravel and Symfony projects written to a folder with the pieces of the framework they need (`php-index`'s `testing` feature has them).

## Build, test and run

```sh
cargo build --release --locked            # target/release/php-language-server
cargo fmt --all --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
```

The default test run needs no network and no PHP. It holds, per construct, snapshot tests of the tree in a compact text form (`expect-test`: `UPDATE_EXPECT=1 cargo test` rewrites them), error recovery tests, a round trip test (the text of the tree equals the input for every prefix of a sample file and after every single edit), tests of the language level table, and, for phase 1b, tests of the PHPDoc and type grammar, name resolution, the extractor, Composer metadata, the cache and the parallel indexer (on temporary folders), the hierarchy, type inference cases, completion at about thirty cursor positions over fixture projects (with a fake package and stubs), the placement of `use` lines, hover snapshots, navigation, and end-to-end tests of the server over an in-memory connection against a project on disk with Composer metadata, a vendor package, stubs and a cache (progress, the cache on a second run, completion with an import, level filtering of stubs, hover and definition across files, watched files, open documents over the disk). For phase 2 it adds the doc comment scanner, usages over fixture projects (a class with its imports and aliases, a method across its hierarchy, a promoted property with named arguments, variables with closures, strings, enums and traits, reads and writes), rename (a class with its file, a method across a hierarchy, a promoted property, parameters with docs, namespaces, name validation and conflicts), signature help with overloads, named arguments and variadics, call and type hierarchies, snapshots of semantic tokens and inlay hints, overridable method completion, the cache file and the lazy loading and trimming of declarations, and end-to-end tests of every new request over the in-memory connection. For phase 4 it adds, for each refactor, snapshots of the text before and after and the cases it must refuse or not offer, each result being parsed and inspected again so that it brings no syntax error and no new finding (`refactor/tests` in `php-analysis`), the diff that turns a whole rewritten text into small edits, the names a refactor suggests, the wrapping of long lines and the `.editorconfig` globs in `php-format`, and end-to-end tests over the in-memory connection of a code action with and without snippet edits, `codeAction/resolve` and its refusals, a move of a class with its file, `workspace/willRenameFiles` for a file and a folder, and the `.editorconfig` of a project in `textDocument/formatting`. Tests that read the real stubs use the corpus below and report that they skipped when it is not there.

### Corpus

```sh
./scripts/fetch-corpus.sh                 # into ./corpus, ignored by git
cargo test --locked --test corpus         # without a corpus these tests report that and pass
cargo run --release -p php-syntax --example corpus -- stubs
cargo run --release -p php-syntax --example corpus -- phpt --oracle --verbose
cargo run --release -p php-syntax --example corpus -- dir=/some/project --oracle
```

The script pins phpstorm-stubs to a commit and php-src to the `php-8.5.11` tag (its `Zend/tests` and `tests` folders). The example reports files that fail to parse; with `--oracle` it asks `php -l` of the PHP on the path whether PHP itself agrees.

Results:

| Corpus | Files | Result |
| --- | --- | --- |
| phpstorm-stubs | 1052, 13 MB | no syntax errors, parsed in about 80 ms |
| php-src tests (the `--FILE--` of 6170 `.phpt`) | 6170 | no errors on any file PHP accepts; the 148 files PHP rejects are rejected here too, except those listed below |
| Real projects (vendor trees and applications on the author's machine) | 152,511, 1.1 GB | no disagreement with `php -l` on any file |

The stubs declare `exit()` and `die()` as functions, which the parser reports in a user file as a reserved word used as a name, so that one message is ignored for the stubs.

Valid files rejected by the parser: none, except `Zend/tests/stack_limit/*.phpt`, which nest tens of thousands of operators to test PHP's own stack limit and hit the nesting limit of 200 here.

Invalid files accepted by the parser, the syntax errors PHP reports that this phase does not (all are semantic rather than lexical, and a later inspection can report them):

- `(real)` cast removed in 8.0: the language level pass reports it (`real_cast.phpt`);
- `list(...)` used as a value (`list_011.phpt`), `const` inside a function (`oss_fuzz_416302790.phpt`), `?static` as a property type (`static_type_property.phpt`);
- `(void)` as the only expression of a `for` condition (`gh18301_cast_to_void_statement_for_condition.phpt`).

The one construct of older PHP the parser does not read is the removed `$a{0}` offset syntax.

### Benchmarks

```sh
cargo bench -p php-syntax                 # criterion; not part of cargo test
```

Measured on an Apple Silicon laptop, release build: lexing about 345 MiB/s, parsing about 100 MiB/s (1.4 MB synthetic file in 13.5 ms, a 54 KB file in 0.54 ms, all of phpstorm-stubs in about 80 ms).

## Roadmap

1. **Phase 1a (done):** parser, lossless tree, error recovery, language level table and gate, a server with diagnostics, symbols, folding and selection ranges.
2. **Phase 1b, index and completion (done):** the index, the cache, Composer, the language level per project, name resolution, the type layer, hover, definition, type definition, implementation, workspace symbols and completion with auto-import, as described above.
3. **Phase 2, usages, rename and editing aids (done):** find usages, document highlights, rename, signature help, call and type hierarchies, semantic tokens, inlay hints, completion of overridable methods and a cache that keeps declarations on disk, as described above.
4. **Phase 3, inspections, fixes and formatting (done):** the inspections, quick fixes, code actions and the formatter described above.
5. **Phase 4, refactors (done):** extract variable, constant, field, method and parameter, inline variable and method, move a class (and follow renamed files), change signature, pull up and push down, the rewrite intentions, and the formatter's wrapping and `.editorconfig`, as described above. Left for later: introducing a type or an interface from a class, a preview of a rename that moves namespaces, and strings that hold class names.
6. **Phase 5, PHPUnit and Pest, and what was left of phase 1b (done):** test and dataset navigation, `$this` and the properties of `beforeEach` in Pest closures, `expect()` chains and custom expectations, test doubles, the run markers (`php/runnables` and code lenses), closure arguments typed through templates, return types read from bodies and `@psalm-assert` narrowing, as described above. Left for later: the doc and attribute formats of Laravel and Symfony macros and `@psalm-type` aliases, usages in the installed packages, usages found through a reference index that is kept on disk, and interned strings if the memory of the summaries ever matters.
7. **Phase 6, Laravel and Symfony (done):** facades, Eloquent (attributes from migrations and casts, relations, accessors, scopes, builder and collection generics, factories), the container, the strings that name config keys, routes, views, translations, environment variables, abilities, services, parameters, templates and events, form request fields, Doctrine repositories and entity fields, a minimal Blade reading, and the commands of `artisan` and `bin/console` as runnables, as described above.

What is left after the planned phases:

- Twig and Blade as languages of their own: a Blade model of the template (scope, slots, `@props`, layouts), Twig variables and tags, and YAML and Twig documents for the strings that are completed in PHP now. Livewire and Inertia.
- DQL and the Doctrine query builder (aliases, fields in `->andWhere('u.email')`), Eloquent query strings (`with('author')`, `where('author.name')`, `whereHas`), validation rule strings and `$casts` strings.
- A reference index kept on disk, so that usages of a route name, a config key or a service id across the project (and of the installed packages) are found; today only definitions and completion are.
- Other frameworks and packages in the same overlay format (Livewire, Filament, Pennant, API Platform, Symfony Messenger and Workflow), and the container of Laravel's `bootstrap/app.php` and `bind` calls outside providers.
- Composer autoload maps beyond PSR-4 and PSR-0 (classmap authoritative, `files` reading by name), usages in installed packages, and the `@psalm-type` aliases of phase 5's list.
- A PHP 8.6 level, when its syntax exists.
