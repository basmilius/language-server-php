# Measurements

What the server was measured to do, on which project and when. `NATIVE.md` says how the server works and why; the numbers live here, so that a new measurement replaces an old one without touching the design notes. Each section names its date. "Before 2026-10-05" means the measurement was made in Ruimte, where the server was developed first, and came along when the code moved here; it holds for the build of that time.

All measurements are release builds on an Apple Silicon laptop with 16 cores, unless a section says otherwise.

## The projects

| Project | What it is | Files |
| --- | --- | --- |
| Passly | A private PHP application with Pest tests, no Laravel or Symfony; the workspace root is the folder above `backend/` | 798 own, 9,288 with packages |
| `laravelio/laravel.io` | Laravel 11, Eloquent, Pest, Livewire, Filament | 452 (319 without templates) |
| `BookStackApp/BookStack` | Laravel, Blade | 1,835 (1,513 without templates) |
| `symfony/symfony-demo` | Symfony, Doctrine, Twig | 51 |
| `kimai/kimai` | Symfony, Doctrine, Twig, plugins | 1,899 (11,779 in `vendor/`) |
| `inertiajs/pingcrm` | Laravel with Inertia and Vue | 44 |
| `api-platform/demo` (`api/`) | Symfony with API Platform | 89 |
| Laravel 13 and Symfony 8.1 skeletons | Fresh projects with a few models, entities, routes and templates added | 45 and 17 |

The public projects are cloned into a folder outside the repository, installed with `composer install --no-scripts`, and deleted afterwards.

## The tools

```sh
cargo bench -p php-syntax                       # lexing and parsing
cargo bench -p php-index                        # extraction and index building over the stubs
cargo bench -p php-analysis                     # completion and hover over the stubs
cargo bench -p php-analysis --bench features    # tokens, hints, signature help, usages and rename
cargo run --release -p php-index --example bench -- <project> <stubs> [cache]
cargo run --release -p php-syntax --example corpus -- stubs|phpt|dir=<path> [--oracle]
cargo run --release -p php-format --example check -- <folder>... [--tabs --wrap --align --width <n> --typing]
cargo run --release -p php-analysis --example probe -- <project> <stubs> <file> <line> <column> <question>
cargo run --release -p php-analysis --example stress -- <project> <stubs> [limit]
cargo run --release -p php-analysis --example typecov -- <project> <stubs>
cargo run --release -p php-analysis --example survey -- <project> <stubs> [--code <code>] [--vendor]
cargo run --release -p php-analysis --example refactor_smoke -- <project> <stubs> [--title <prefix>]
cargo run --release -p php-analysis --example key_usages -- <project> <stubs>
cargo run --release -p php-analysis --example blade_survey -- <project> <stubs>
cargo run --release -p php-analysis --example twig_survey -- <project> <stubs>
python3 scripts/measure-memory.py <binary> <project> <stubs> <storage> <file> <class>
python3 scripts/measure-usages.py ...
python3 scripts/survey-usages.py ...
```

`survey` with `STRING_PARTS=1` also counts the DQL fields and the strings that hold a class name, and `blade_survey` counts the Livewire tags and `wire:` names that resolve.

## Parsing

Before 2026-10-05.

| | |
| --- | --- |
| Lexing | about 345 MiB/s |
| Parsing | about 100 MiB/s: a 1.4 MB synthetic file in 13.5 ms, a 54 KB file in 0.54 ms |
| All of phpstorm-stubs | about 80 ms |

A change to a document parses the whole file again; at these speeds that is cheaper than anything a patch would save, and a burst of keystrokes costs one parse.

2026-10-07, after the token cursor and tree builder of the parser moved to `lsc-syntax` of `basmilius/language-server-core`, measured against a checkout of the commit before the move in alternating runs:

| | Before | After |
| --- | --- | --- |
| Lexing and parsing all of phpstorm-stubs | 67.0 to 67.6 ms | 66.4 to 68.5 ms |
| Extracting the declarations of every stub file | 103.9 to 105.3 ms | 103.8 to 104.8 ms |
| A typical file | 492 µs | 498 µs |
| Lexing the 1.4 MB synthetic file | 3.65 ms | 3.92 ms |
| Lexing it with one codegen unit | 3.90 ms | 3.89 ms |
| Parsing it with one codegen unit | 12.6 ms | 12.2 ms |

The lexer did not change; with the release profile's four codegen units it lands in a different place, and with one the difference is gone.

### Corpus

Before 2026-10-05, with the PHP on the path as the oracle.

| Corpus | Files | Result |
| --- | --- | --- |
| phpstorm-stubs | 1,052, 13 MB | no syntax errors, about 80 ms |
| php-src tests (the `--FILE--` of each `.phpt`, `php-8.5.11`) | 6,170 | no error on a file PHP accepts; the files PHP rejects are rejected too, except the ones `NATIVE.md` lists |
| Vendor trees and applications on the author's machine | 152,511, 1.1 GB | no disagreement with `php -l` |

2026-10-06, after partial function application was added: the php-src 8.5 corpus gives exactly the result of the parser before it. Of the 184 tests of php-src's `PHP-8.6` branch about partial application, readonly property defaults, `#[\Override]` on constants and `__debugInfo()` on enums, 183 parse; the one that does not (`g(foo: ...)`) is one PHP itself rejects.

## Formatting

Before 2026-10-05: every PHP file of phpstorm-stubs, the files of Passly and the `--FILE--` sections of 6,180 php-src tests keep their tokens and format the same twice, with the defaults and with tabs, line lengths of 40, 60, 80 and 120 and alignment all on.

2026-10-06: over the 184 PHP 8.6 test files, 180 are formatted, none broken (the other four are markup or a syntax error, which the formatter refuses), and 7,422 typed probes cause no panic.

## Indexing and memory

### Passly, before 2026-10-05

9,288 PHP files, 9,272 classes.

| | Cold (no cache) | Warm (cache) |
| --- | --- | --- |
| Standard library stubs (547 files, 9 MB) | 140 ms | 8 ms |
| The project | discover 100 ms, index 340 ms | discover 90 ms, index 11 ms |
| Resident memory of the server after indexing | 125 MB | 35 MB |
| ... after a find usages, a workspace symbol search, completion and semantic tokens | 130 MB | 45 MB |

Before the declarations of packages were kept in the cache file until something asked for them, the process held 440 MB cold and 310 MB warm. Single-threaded extraction of all stubs takes about 100 ms, and adding them to an index 2 ms.

When the framework layer was added (same project, which uses neither framework):

| | Before | After |
| --- | --- | --- |
| Indexing, no cache | 389 to 513 ms | 385 to 404 ms |
| Resident after indexing | 341 to 347 MB | 350 to 352 MB |
| `survey` | 4.6 s | 2.9 s |
| `stress` (175,084 positions) | 144 s | 100 s, 0 failures |

The speed came from looking a member up by name instead of building the list of every member first. Indexing a Laravel project (8,086 files) or a Symfony one (7,628 files) took as long as before; the first config key, route, model or container name of a Laravel project costs about 2 ms, completion of a model's members about 5 ms.

### Passly, 2026-10-06

With Composer's generated class map read (`measure-memory.py`, warm start): 1.2 s and 46 MB after indexing, against 47 MB the build before. The cold start was 148 MB against 145 MB.

## Usages

### Search speed, before 2026-10-05

In Passly's 798 own files, finding the 321 usages of a class in 68 files takes 60 ms in the probe (the words took 80 ms the first time) and 25 ms through the server; a method used in five places takes 2 ms.

`survey-usages.py` held the counting conventions against another server on a project of 564 source files: of 453 declarations, 376 give the same set of places. The rest are calls from `vendor/` (which this server did not read then), the declaration line of a property that the other server counts, and receivers the type layer cannot name.

### The reference index on disk, 2026-10-06

Passly's workspace; the usages of an interface used in 58 files, 216 places; against the build before.

| | Before | After |
| --- | --- | --- |
| Startup (index from the cache) | 0.8 s, 47 MB | 0.8 s, 46 MB |
| First search, no words on disk | 16 ms | 16 to 24 ms, and the words are written (0.5 MB) |
| First search after a restart (four runs of each, in turns) | 17 to 25 ms | 10 to 18 ms |
| Second search | 5 ms | 5 ms |
| First search with packages, no words on disk | | 136 ms, 357 places in 90 files |
| First search with packages after a restart | | 26 ms (7.6 MB on disk) |
| Resident after a search with packages | | 70 to 73 MB, 17 MB more than without |

`key_usages` on laravel.io runs 132 searches in 74 ms (the slowest 3.4 ms); of the quoted strings it does not count, read by hand, each is something else (an array key or a URI spelled like a route, a `@props` name, a doc comment, the declaration). It misses Livewire's `redirectRoute()`, the `:href="route(...)"` attributes of components and helpers of the project that take a route name. On symfony-demo it runs 27 searches in 7 ms.

## Editor features

Before 2026-10-05: semantic tokens take about 60 ms for a 5,000-line file and 5 to 20 ms for a typical one.

## Inspections

### Passly

Before 2026-10-05: `survey` over the 798 files took about 3.7 s and reported 66 findings with the default set, all of them real (unused imports and variables). The first run reported 68; a call through a `@mixin` and the classes Composer writes into `vendor/composer` were wrong and were fixed. When Pest support came the 66 stayed the same and the run took about 1.8 times as long, since far more of what the inspections read now resolves. After the framework layer it took 2.9 s.

2026-10-06: 798 files in 2.6 to 2.9 s; 1 `deprecated`, 2 `phpdoc-unknown-parameter`, 4 `undefined-class`, 43 `unused-import`, 16 `unused-variable`.

### The framework projects, 2026-10-06

| Project | Files | Time | Findings |
| --- | --- | --- | --- |
| laravel.io | 319 | 1.1 to 1.3 s | 1 `instance-call-of-static-method` (`Factory::times()`), 1 `missing-return`, 6 `phpdoc-type-mismatch`, 1 `unused-variable`, 5 `wrong-argument-count` (`RegisterUser` built with 7 arguments, 5 declared) |
| BookStack | 1,513 | 5.5 to 6.2 s | 1 `deprecated`, 3 `instance-call-of-static-method` (`Entity::getType()`), 1 `undefined-method` (`softDestroyBookshelf`), 1 `unknown-translation`, 3 `unreachable-code` (after `markTestSkipped()`), 19 `unused-import`, 30 `unused-variable`, 2 `wrong-argument-count` (`assertSessionHas` with 2 arguments, 1 declared) |
| Kimai | 1,899 | 5.8 to 6.2 s | 84 `deprecated`, 10 `undefined-class`, 1 `undefined-method`, 9 `unused-parameter`, 13 `unused-variable` |
| Kimai `vendor/` | 11,779 | 44 s | unchanged by type aliases (the same counts before and after) |
| symfony-demo | 51 | 60 ms | nothing |
| pingcrm | 44 | 76 ms | 17 `undefined-property` (`$this->user` set in tests without a declaration) |
| API Platform demo | 89 | 170 ms | 9 `undefined-class` (php-cs-fixer and Rector configuration, tools that are not installed), 1 `missing-return` |

The false positives found and fixed on the way: a scope or `@method` called through an object taken for a static call, and its arguments counted against the `@method` (laravel.io's `mostSolutions()`, Carbon's `subDay(2)`); `self::`/`static::` of an anonymous class looked up in its parent (BookStack's migration); a Blade template surveyed as plain PHP by the tool itself; validation groups taken for serialization groups (API Platform demo).

## Refactors

### Passly, before 2026-10-05

`refactor_smoke` over the 798 files: 68,508 offers, 55,237 applied, 13,271 refused with a reason. No applied refactor left a file that parses worse or a finding the inspections did not already have.

| Refactors | Files | Offered | Applied | Refused |
| --- | --- | --- | --- | --- |
| Extract variable, constant, field and parameter, inline variable | 798 | 32,508 | 24,084 | 8,424 |
| Extract method | 798 | 9,915 | 5,552 | 4,363 |
| Rewrites | 798 | 25,067 | 25,067 | 0 |
| Inline method, change signature, pull up, push down | 399 | 778 | 294 | 484 |
| Move class | 200 | 240 | 240 | 0 |

Most refusals are extract method on a selection inside a closure or a script, and code that returns on some paths only.

### 2026-10-06

| Refactors | Project | Offered and applied | Parse errors | New findings |
| --- | --- | --- | --- | --- |
| Move class and rename, with strings that hold class names | BookStack (120 files) | 160 | 0 | 0 |
| Move class and rename, with strings that hold class names | Passly (120 files) | 232 | 0 | 0 |
| Extract interface | BookStack | 15 | 0 | 0 |
| Extract interface | Passly | 16 | 0 | 0 |

## Type coverage

`typecov` counts the expressions of the project's own files whose type is unknown or `mixed`.

### Pest support, Passly, before 2026-10-05

71,901 expressions, 22,178 of them the object of a member or index read.

| | Before | After |
| --- | --- | --- |
| Unresolved, all files | 15,890 (22.1%) | 4,823 (6.7%) |
| Unresolved, `tests/` | 13,963 (32.0%) | 2,919 (6.7%) |
| Unresolved, objects of a member read | 5,002 (22.6%) | 1,572 (7.1%) |

Nearly all of it is `$this` in Pest closures (about 11,000 expressions).

### The framework layer, before 2026-10-05

| Project | Files | Unresolved before | After | Receivers before | After |
| --- | --- | --- | --- | --- | --- |
| Laravel 13 skeleton plus models, relations, scopes, a policy, a form request, routes, views and translations | 45 | 42.9% | 31.4% | 14.4% | 0.5% |
| laravel.io | 452 | 35.6% | 14.9% | 34.7% | 5.8% |
| Symfony 8.1 skeleton plus entities, repositories, a form, services, a subscriber | 17 | 8.9% | 8.1% | 0.0% | 0.0% |
| symfony-demo | 51 | 3.2% | 3.2% | 0.4% | 0.4% |

What is left in the Laravel numbers is `env()` and `config()` (mixed by nature), the Pest functions and `$this` of a test, and `Cache::remember` and the like, which the framework documents as `mixed`.

### 2026-10-06

After the fixes for scopes and `@method` calls: BookStack 9.3% to 9.0%, laravel.io 14.9% to 14.8%, Passly unchanged. Variables: laravel.io 1,871 of 2,040 typed (8.3% unresolved), BookStack 21,278 of 22,993 (7.5%).

## Strings of the frameworks, 2026-10-06

| What | Project | Found | Reported |
| --- | --- | --- | --- |
| Relation strings | laravel.io | 10 names in 16 places | nothing |
| Validation rules | laravel.io, BookStack | | nothing |
| Casts | laravel.io (10 in 3 arrays), BookStack (14 in 8), Passly | | nothing; inspections 5.7 to 6.0 s on BookStack, as before |
| Columns of queries | laravel.io | 27 columns in 53 places | |
| Columns of queries | BookStack | 81 columns in 605 places | |
| Filament columns, fields and entries | laravel.io | column strings grow to 33 in 78 places | |
| DQL fields | symfony-demo | 7 | nothing |
| DQL fields | Kimai | 474 | nothing; inspections 6.2 s, as before |
| Strings that hold a class name | BookStack, Kimai | 7, 2 | |
| Inertia pages | pingcrm | 12 pages in 20 places, all found | nothing |
| Serialization groups | API Platform demo | 11 groups in 58 places | nothing |
| Type aliases | Kimai `vendor/` | 285 files with aliases | survey unchanged |

A plain search finds more column names than the queries name: the keys of attribute arrays, `$request->validated()` and the `groupBy()` of a collection, which are no columns of a query.

Pennant, Messenger and Workflow have no public project at hand that uses them; they rest on their tests, and the projects without them show no change.

## Templates and configuration, 2026-10-06

### Blade

| | laravel.io (199 templates) | BookStack (399 templates) |
| --- | --- | --- |
| Diagnostics | nothing | nothing |
| Reading all templates | 3 to 5 ms; the longest (392 lines) 0.1 ms | about 0.1 ms each, the first about 100 ms for the route and translation catalogs |
| Requests at a spread of offsets | 155,130, 0.16 to 0.46 ms on average, no panic | |
| Variables without a type, read alone | 1,126 of 1,387 | |
| Variables without a type, with what the renderers give | 682 of 1,387 | |
| Finding what renders a template | 3 ms on average, 46 ms for the slowest | |
| Livewire | 22 of 22 component tags and 5 of 5 `wire:` names resolve | |

Finding the 100 usages of a model method across 38 files, templates included, takes 29 ms. The slowest requests, about 100 ms, are completions where every class is offered, as in a PHP file. The bound attributes of `<livewire:...>` tags added 25 variables to the count, 15 of them typed.

### Twig

On symfony-demo: the 32 templates are read in 2 ms; 202 functions, filters and tests are declared and all 277 calls of the templates are found among them; finding the usages of its 8 route names goes from 12 places to 35; the renderers give 49 variables in 21 ms; 81 of the 122 attributes have a type; 20,583 requests take 0.02 ms each on average, no panic. Over the 32 templates of symfony-demo and the 177 of Kimai the diagnostics report nothing.

### YAML

Over the 27 YAML files of symfony-demo and the 26 of Kimai, `undefined-class` reports two classes, both real (in Kimai's test configuration: a route to a removed `LayoutController` and a service `App\Importer\ImporterService` that does not exist).
