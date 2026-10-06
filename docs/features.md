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

Rename checks the new name and conflicts, follows methods and properties through the class hierarchy, updates promoted parameters and named arguments, and changes PHPDoc references and the strings that name the symbol: a qualified class name (`'App\Models\User'`, `'App\Http\Home@show'`), a relation segment, a DQL field, a `wire:` attribute. Strings in a `migrations` folder are left alone, since they record what a database holds. A class that is alone in its file, named after it and found through Composer also renames its file, for a client that announces `documentChanges` and the rename resource operation. Ordinary strings and comments stay as they are, and so do the usages in installed packages.

Find usages also works on the strings a framework reads as names: from `route('home')`, `@include('partials.nav')` or `__('auth.failed')`, or from the place that declares the name, such as `->name('home')`, a key of a config file or a column in a migration, it lists every call, template and configuration line that names the same thing. These keys themselves are not renamed. With `usages.packages` on, find usages and incoming calls also search the installed packages.

Code actions offer quick fixes of inspections and imports, and refactors: extract variable, constant, field, method, parameter and interface, inline variable and method, move a class, change signature, pull up, push down and rewrite intentions. Expensive refactors are worked out on `codeAction/resolve`, where an unsafe one returns an error with the reason. Extract interface writes a new file, which needs a client that can create files in a workspace edit (see [clients](./clients.md)). A preview of a rename that moves a namespace is not done.

## Formatting

The formatter follows PER Coding Style 2.0, with the [settings](./configuration.md#formatter), keeps every token, and gives the same result when run twice. It does not add trailing commas or touch comments and strings. A file with a syntax error, or with markup around its PHP, is not formatted: the answer is `null`.

## Frameworks

Support turns on by the packages Composer installed: `laravel/framework` or `illuminate/*` for Laravel, `symfony/framework-bundle` for Symfony, `doctrine/orm` for Doctrine and `twig/twig` for Twig and `raxos/database` or `raxos/router` for Raxos; Livewire, Inertia, Pennant, Filament, Messenger and the rest when their classes are installed. The server boots no framework. Names complete, hover, lead to their declaration and have usages; a missing one is only reported where the code is literal, so a route made in a loop or a computed config key is never called missing.

### Laravel

- Facades, the container (bindings in providers, `bootstrap/app.php` and elsewhere in `app/`), and the logged in user.
- Eloquent: attributes from migrations and schema dumps, casts, relations, accessors, scopes, builders and factories.
- Strings: config keys, routes, views, translations, environment variable names (never their values), abilities and form request fields.
- Queries: relation strings (`with('posts.comments')`), column strings (`where('email')`, `orderBy('users.name')`), validation rules (`'required|email|unique:users,email'`) and the keys and values of `$casts`.
- Livewire: `<livewire:...>` and `@livewire()` lead to the component, its attributes to the properties they fill, and `wire:model` and `wire:click` in a component's view to its properties and methods.
- Inertia: page names lead to the page component, and a missing one is reported.
- Pennant: features lead to where they are defined, in PHP and in `@feature`.
- Filament: the names of columns, fields and entries lead to the column of the resource's model.

### Symfony

- Services, parameters, routes, templates, translations and events, from PHP, YAML and attributes.
- Doctrine: repositories, the fields of entities, and DQL and the query builder (`->andWhere('p.title = :title')`), with their aliases and joins.
- Serialization groups in contexts and `#[ApiResource]` lead to the `#[Groups]` that declare them.
- Go to implementation on a Messenger message lists its handlers.
- Workflow transitions, places and names lead to the workflow configuration.
- The YAML of `config/`: parameters, environment variables, service references, the classes of services and the controllers of routes hover, complete and lead to their declarations, and are found as usages from PHP. A class that does not exist is reported.

### Raxos

- A `ModelArrayList` property with `#[HasMany]`, `#[HasManyThrough]` or `#[BelongsToMany]` holds the model the attribute names, so `$team->scans->first()->` completes without a `@var`. An inlay hint shows the type after the declaration.
- Every relation is also a method that gives its query, so `$order->buyer()->` completes without an `@method` line.
- `$order->lines->column('quantity')` completes the properties of the model and gives a list of what the property holds, here `ArrayList<int, int>`; `column('buyer', 'email')` follows the relation first. An `@method` that says the same is marked as redundant and can be removed; one for a property that is no relation, or with another model, is reported.
- Column keys in `Model::col('created_on')`, property names in `only()`, `makeVisible()`, `makeHidden()` and `#[Visible]`, and relation names in `eagerLoad()` and `#[MapModelRelation]` complete, lead to their property and are reported when the model lacks them. A nested array below a relation key is read against the related model.
- A `#[Macro]` callable that does not take the model or gives a class the property cannot hold, a `#[Caster]` that is no caster and a `#[Handler]` that does not handle its message are reported.
- Hover on `#[Get]`, `#[Post]` or `#[Controller]` shows the whole path, with the prefixes of the parent controllers. A `$name` in a path is the parameter it fills: it is colored as one, hover shows its type and doc, definition leads to it, and renaming the parameter renames the segment too. A `$name` in a path that no parameter fills, a `#[MapModelRelation]` parent no controller provides and a route without a return type are reported.

### Templates

Blade templates are read as a whole. The PHP in them knows its variables: the ones `@foreach`, `@php`, `@props` and `@inject` make, and the ones the controllers, mailables, components, Livewire components, `@include` and component tags that render the template pass it. Hover, definition, completion, usages, highlights and rename work in that PHP, sections and stacks lead to the layout, attributes and slots to the component, and finding the usages of a class or a method also finds the templates. A template gets diagnostics for PHP that does not parse, block directives that do not close, and views, routes, config keys and translations the project does not have.

Twig templates complete and lead to the templates they extend, include and import, the blocks of their parents, the functions, filters and tests of the project's and the packages' extensions, and the routes, translations and workflow transitions their functions and filters name. Their variables have the types the controllers that render them pass, so `post.title` hovers, completes and leads to `getTitle()`, and finding the usages of a route, a translation or a getter lists the templates too. A Twig template gets diagnostics for what does not parse, tags that do not close, templates and routes the project does not have, and functions, filters and tests no extension declares.

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
