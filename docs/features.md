# Features

## What it answers

| Area          | Requests                                                                                                    |
| ------------- | ----------------------------------------------------------------------------------------------------------- |
| Documents     | Incremental sync, push diagnostics, or pull diagnostics for a client that supports them                     |
| Reading       | Hover, signature help, document symbols, folding ranges, selection ranges, semantic tokens, inlay hints     |
| Navigation    | Definition, type definition, implementation, references, document highlights, workspace symbols, call and type hierarchies |
| Writing       | Completion with resolve, rename with prepare, code actions with resolve, formatting, range formatting, formatting on type at newline, `}` and `;` |
| Files         | `workspace/willRenameFiles` for PHP files and folders                                                        |
| Tests         | Code lenses and the request `php/runnables`                                                                 |

Positions are UTF-8 when the client offers it and UTF-16 otherwise. A document is parsed whole on every change; a burst of changes is parsed once.

## Completion

Completion offers variables, members by the visibility of the place, classes, functions, constants, enum cases, named arguments, keywords, attributes and methods to override. A class from elsewhere comes with its `use` line as an additional edit. At most 300 items come back, with `isIncomplete` set when there were more. The trigger characters are `$`, `>`, `:`, `\`, `#` and `[`.

Types follow declarations, PHPDoc with generics and array shapes, narrowing by conditions, closure arguments, return types read from bodies, `@psalm-assert`, and `@psalm-type` and `@phpstan-type` aliases with their imports. What cannot be known, such as variable variables, is `mixed`, which offers nothing rather than a wrong guess.

## Rename and refactors

Rename checks the new name and conflicts, follows methods and properties through the class hierarchy, updates promoted parameters and named arguments, and changes PHPDoc references. A class that is alone in its file, named after it and found through PSR-4 also renames its file, for a client that announces `documentChanges` and the rename resource operation. Names in ordinary strings and comments stay as they are, and so do the usages in installed packages.

Find usages also works on the strings a framework reads as names: from `route('home')`, `@include('partials.nav')` or `__('auth.failed')`, or from the place that declares the name, such as `->name('home')` or a key of a config file, it lists every call and Blade directive that names the same thing. With `usages.packages` on, find usages and incoming calls also search the installed packages. Rename does not change these strings.

Code actions offer quick fixes of inspections and imports, and refactors: extract variable, constant, field, method, parameter and interface, inline variable and method, move a class, change signature, pull up, push down and rewrite intentions. Expensive refactors are worked out on `codeAction/resolve`, where an unsafe one returns an error with the reason. Extract interface writes a new file, which needs a client that can create files in a workspace edit. A preview of a rename that moves a namespace is not done.

## Formatting

The formatter follows PER Coding Style 2.0, with the [settings](./configuration.md#formatter), keeps every token, and gives the same result when run twice. It does not add trailing commas or touch comments and strings. A file with a syntax error, or with markup around its PHP, is not formatted: the answer is `null`.

## Frameworks

Support turns on by the packages Composer installed: `laravel/framework` or `illuminate/*` for Laravel, `symfony/framework-bundle` for Symfony and `doctrine/orm` for Doctrine repositories. The server boots no framework.

- Laravel: facades, Eloquent attributes from migrations and schema dumps, casts, relations, accessors, scopes, builders and factories, the container, and the strings that name config keys, routes, views, translations, environment variables, abilities and form request fields, relation and column strings of queries, validation rules and the keys and values of `$casts`, with completion and navigation. Livewire components (`<livewire:...>`, `@livewire()`) lead to their class, and `wire:model` and `wire:click` in a component's view to its properties and methods.
- Symfony: services from YAML and PHP configuration, routes, templates, translations, parameters and events, and the fields DQL and the Doctrine query builder name (`->andWhere('p.title = :title')`).
- Doctrine: repositories and the fields of entities.
- Symfony configuration in YAML: parameters, environment variables, service references, the classes of services and the controllers of routes hover, complete and lead to their declarations, and are found as usages from PHP. A class that does not exist is reported.

Blade templates are read as a whole. The PHP in them knows its variables: the ones `@foreach`, `@php`, `@props` and `@inject` make, and the ones the controllers, mailables, components, `@include` and component tags that render the template pass it. Hover, definition, completion, usages, highlights and rename work in that PHP, sections and stacks lead to the layout, attributes and slots to the component, and finding the usages of a class or a method also finds the templates. A template gets diagnostics for PHP that does not parse, block directives that do not close, and views, routes, config keys and translations the project does not have.

Twig templates (language id `twig`) complete and lead to the templates they extend, include and import, the blocks of their parents, the functions, filters and tests of the project's and the packages' Twig extensions, and the routes and translations named in `path()`, `url()` and `|trans`. Their variables have the types the controllers that render them pass, so `post.title` hovers, completes and leads to `getTitle()`, and finding the usages of a route, a translation or a getter lists the templates too. A Twig template gets diagnostics for what does not parse, tags that do not close, templates and routes the project does not have, and functions, filters and tests no extension declares.

A missing name is only reported where the code is literal: a route made in a loop or a computed config key is never called missing. Environment support reads the names of variables, never their values.

## Tests

PHPUnit and Pest get navigation to providers, dependencies and datasets, test double types, `$this` in Pest closures and `expect()` chains with custom expectations.

`php/runnables` lists what a test file can run, for an open document: `{ "uri": "file:///project/tests/FooTest.php" }`. Each runnable has a `kind` (`phpunit`, `pest`, `artisan` or `console`), a `scope` (`class`, `method`, `test`, `describe`, `arch` or `command`), a `label`, the `range` for a run marker, a `filter`, the `file` and the nearest PHPUnit `configFile` or `null`.

```json
{
    "kind": "phpunit",
    "scope": "method",
    "label": "FooTest::testIt",
    "range": { "start": { "line": 9, "character": 20 }, "end": { "line": 9, "character": 26 } },
    "filter": "/^Tests\\\\Unit\\\\FooTest::testIt( with data set .*)?$/",
    "file": "/project/tests/Unit/FooTest.php",
    "configFile": "/project/phpunit.xml"
}
```

For a test, run `vendor/bin/phpunit` or `vendor/bin/pest` with `--configuration`, `--filter` and the file. For a command, the filter is the command's name for `php artisan` or `bin/console`. `textDocument/codeLens` gives the same list as lenses with the command `php.runTest`, which the client registers and runs; the server runs nothing.
