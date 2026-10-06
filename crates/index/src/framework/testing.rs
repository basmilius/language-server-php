//! Fixtures for the framework tests: an index of files held in memory.

use std::path::PathBuf;
use std::sync::Arc;

use php_syntax::{PhpVersion, parse};

use super::Frameworks;
use crate::extract::{ExtractOptions, extract};
use crate::index::{Index, Origin};

pub const ROOT: &str = "/project";

/// An index of files given as paths below the project and their text. A path starting with `vendor/`
/// is a package's. Every framework is on.
pub fn project(files: &[(&str, &str)]) -> Index {
    let mut index = Index::new(PhpVersion::V8_4);
    index.set_frameworks(std::path::Path::new(ROOT), Frameworks::all());
    for (relative, text) in files {
        add(&mut index, relative, text);
    }
    index
}

pub fn add(index: &mut Index, relative: &str, text: &str) {
    let path = PathBuf::from(ROOT).join(relative);
    let origin = if relative.starts_with("vendor/") {
        Origin::Vendor
    } else {
        Origin::Project
    };
    index.set_open_text(&path, Some(Arc::from(text)));
    if relative.ends_with(".php") {
        let symbols = extract(&parse(text).syntax(), ExtractOptions::default());
        index.set_file(path, origin, Arc::new(symbols));
    }
}

/// Enough of Eloquent, with the docblocks the framework writes, for models to be typed.
pub const ELOQUENT: &[(&str, &str)] = &[
    (
        "vendor/laravel/Model.php",
        r#"<?php
namespace Illuminate\Database\Eloquent;
use Illuminate\Database\Eloquent\Concerns\HasRelationships;
abstract class Model {
    use HasRelationships;
    /** @return \Illuminate\Database\Eloquent\Builder<static> */
    public static function query() {}
    /** @return \Illuminate\Database\Eloquent\Builder<static> */
    public function newQuery() {}
    public function save(array $options = []): bool {}
    /** @return \Illuminate\Database\Eloquent\Builder<static> */
    public static function with($relations) {}
    /** @return $this */
    public function load($relations) {}
    public function __call($method, $parameters) {}
    public static function __callStatic($method, $parameters) {}
    public function __get($key) {}
}
"#,
    ),
    (
        "vendor/laravel/Builder.php",
        r#"<?php
namespace Illuminate\Database\Eloquent;
/**
 * @template TModel of \Illuminate\Database\Eloquent\Model
 * @mixin \Illuminate\Database\Query\Builder
 */
class Builder {
    /** @return $this */
    public function where($column, $operator = null, $value = null, $boolean = 'and') {}
    /** @return $this */
    public function with($relations, $callback = null) {}
    /** @return $this */
    public function whereHas($relation, $callback = null, $operator = '>=', $count = 1) {}
    /** @return $this */
    public function withCount($relations) {}
    /** @return TModel|null */
    public function first($columns = ['*']) {}
    /** @return \Illuminate\Database\Eloquent\Collection<int, TModel> */
    public function get($columns = ['*']) {}
    /** @return TModel */
    public function create(array $attributes = []) {}
}
"#,
    ),
    (
        "vendor/laravel/QueryBuilder.php",
        "<?php namespace Illuminate\\Database\\Query; class Builder { /** @return $this */ public function orderBy($column) {} public function count(): int {} }",
    ),
    (
        "vendor/laravel/Collection.php",
        r#"<?php
namespace Illuminate\Database\Eloquent;
/**
 * @template TKey of array-key
 * @template TModel of \Illuminate\Database\Eloquent\Model
 * @extends \Illuminate\Support\Collection<TKey, TModel>
 */
class Collection extends \Illuminate\Support\Collection {}
"#,
    ),
    (
        "vendor/laravel/SupportCollection.php",
        r#"<?php
namespace Illuminate\Support;
/**
 * @template TKey of array-key
 * @template-covariant TValue
 */
class Collection {
    /** @return TValue|null */
    public function first() {}
    /**
     * @param callable(TValue, TKey): mixed $callback
     * @return $this
     */
    public function each(callable $callback) {}
}
"#,
    ),
    (
        "vendor/laravel/Carbon.php",
        "<?php namespace Illuminate\\Support; class Carbon { public function format(string $format): string {} }",
    ),
    (
        "vendor/laravel/HasRelationships.php",
        r#"<?php
namespace Illuminate\Database\Eloquent\Concerns;
trait HasRelationships {
    /**
     * @template TRelatedModel of \Illuminate\Database\Eloquent\Model
     * @param  class-string<TRelatedModel>  $related
     * @return \Illuminate\Database\Eloquent\Relations\HasMany<TRelatedModel, $this>
     */
    public function hasMany($related, $foreignKey = null, $localKey = null) {}
    /**
     * @template TRelatedModel of \Illuminate\Database\Eloquent\Model
     * @param  class-string<TRelatedModel>  $related
     * @return \Illuminate\Database\Eloquent\Relations\BelongsTo<TRelatedModel, $this>
     */
    public function belongsTo($related, $foreignKey = null, $ownerKey = null, $relation = null) {}
}
"#,
    ),
    (
        "vendor/laravel/Relation.php",
        r#"<?php
namespace Illuminate\Database\Eloquent\Relations;
/**
 * @template TRelatedModel of \Illuminate\Database\Eloquent\Model
 * @template TDeclaringModel of \Illuminate\Database\Eloquent\Model
 * @template TResult
 * @mixin \Illuminate\Database\Eloquent\Builder<TRelatedModel>
 */
abstract class Relation {
    /** @return TResult */
    abstract public function getResults();
}
"#,
    ),
    (
        "vendor/laravel/HasMany.php",
        r#"<?php
namespace Illuminate\Database\Eloquent\Relations;
/**
 * @template TRelatedModel of \Illuminate\Database\Eloquent\Model
 * @template TDeclaringModel of \Illuminate\Database\Eloquent\Model
 * @extends Relation<TRelatedModel, TDeclaringModel, \Illuminate\Database\Eloquent\Collection<int, TRelatedModel>>
 */
class HasMany extends Relation { /** @inheritDoc */ public function getResults() {} }
"#,
    ),
    (
        "vendor/laravel/BelongsTo.php",
        r#"<?php
namespace Illuminate\Database\Eloquent\Relations;
/**
 * @template TRelatedModel of \Illuminate\Database\Eloquent\Model
 * @template TDeclaringModel of \Illuminate\Database\Eloquent\Model
 * @extends Relation<TRelatedModel, TDeclaringModel, ?TRelatedModel>
 */
class BelongsTo extends Relation { /** @inheritDoc */ public function getResults() {} }
"#,
    ),
    (
        "vendor/laravel/Attribute.php",
        "<?php namespace Illuminate\\Database\\Eloquent\\Casts; class Attribute { public static function make(?callable $get = null, ?callable $set = null): static {} }",
    ),
    (
        "vendor/laravel/CastsAttributes.php",
        "<?php namespace Illuminate\\Contracts\\Database\\Eloquent; /** @template TGet @template TSet */ interface CastsAttributes { /** @return TGet|null */ public function get($model, string $key, mixed $value, array $attributes); }",
    ),
    (
        "vendor/laravel/SoftDeletes.php",
        "<?php namespace Illuminate\\Database\\Eloquent;\n/** @method static \\Illuminate\\Database\\Eloquent\\Builder<static> withTrashed(bool $withTrashed = true) */\ntrait SoftDeletes {}",
    ),
    (
        "vendor/laravel/Factory.php",
        "<?php namespace Illuminate\\Database\\Eloquent\\Factories;\n/** @template TModel of \\Illuminate\\Database\\Eloquent\\Model */\nabstract class Factory {\n    /** @return \\Illuminate\\Database\\Eloquent\\Collection<int, TModel>|TModel */\n    public function create($attributes = [], $parent = null) {}\n    /** @return TModel */\n    public function makeOne($attributes = []) {}\n}",
    ),
    (
        "vendor/laravel/HasFactory.php",
        "<?php namespace Illuminate\\Database\\Eloquent\\Factories; /** @template TFactory of Factory */ trait HasFactory { /** @return TFactory */ public static function factory($count = null, $state = []) {} }",
    ),
    (
        "vendor/laravel/HasAttributes.php",
        "<?php namespace Illuminate\\Database\\Eloquent\\Concerns;\ntrait HasAttributes {\n    protected static $primitiveCastTypes = ['array', 'bool', 'boolean', 'date', 'datetime', 'decimal', 'encrypted', 'encrypted:array', 'hashed', 'int', 'integer', 'string'];\n}",
    ),
    (
        "vendor/laravel/ScopeAttribute.php",
        "<?php namespace Illuminate\\Database\\Eloquent\\Attributes; #[\\Attribute] class Scope {}",
    ),
];

/// The container, the application with its core aliases, and the helpers that resolve from it.
pub const CONTAINER: &[(&str, &str)] = &[
    (
        "vendor/laravel/Container.php",
        r#"<?php
namespace Illuminate\Contracts\Container;
interface Container {
    /**
     * @template TClass of object
     * @param string|class-string<TClass> $abstract
     * @return ($abstract is class-string<TClass> ? TClass : mixed)
     */
    public function make($abstract, array $parameters = []);
}
"#,
    ),
    (
        "vendor/laravel/Application.php",
        r#"<?php
namespace Illuminate\Foundation;
class Application implements \Illuminate\Contracts\Container\Container {
    public function make($abstract, array $parameters = []) {}
    public function registerCoreContainerAliases() {
        foreach ([
            'app' => [self::class, \Illuminate\Contracts\Container\Container::class],
            'cache' => [\Illuminate\Cache\CacheManager::class, \Illuminate\Contracts\Cache\Factory::class],
        ] as $key => $aliases) {}
    }
}
"#,
    ),
    (
        "vendor/laravel/CacheManager.php",
        "<?php namespace Illuminate\\Cache; class CacheManager { public function store(): int {} }",
    ),
    (
        "vendor/laravel/ServiceProvider.php",
        "<?php namespace Illuminate\\Support; abstract class ServiceProvider { public function __construct(protected $app) {} }",
    ),
    (
        "vendor/laravel/helpers.php",
        r#"<?php
/**
 * @template TClass of object
 * @param string|class-string<TClass>|null $abstract
 * @return ($abstract is class-string<TClass> ? TClass : ($abstract is null ? \Illuminate\Foundation\Application : mixed))
 */
function app($abstract = null, array $parameters = []) {}
"#,
    ),
];

/// The global helpers that take names a project declares, and the facades with the same methods.
pub const HELPERS: &[(&str, &str)] = &[
    (
        "vendor/laravel/support-helpers.php",
        r#"<?php
function config($key = null, $default = null) {}
function route($name, $parameters = [], $absolute = true) {}
function view($view = null, $data = [], $mergeData = []) {}
function __($key = null, $replace = [], $locale = null) {}
function env($key, $default = null) {}
"#,
    ),
    (
        "vendor/laravel/Facades.php",
        r#"<?php
namespace Illuminate\Support\Facades;
/**
 * @method static mixed get(string $key, mixed $default = null)
 * @method static bool has(string $key)
 * @method static void set(string $key, mixed $value = null)
 */
class Config {}
/** @method static bool has(string $name) */
class Route {}
/** @method static \Illuminate\Contracts\View\View make(string $view) */
class View {}
"#,
    ),
];

/// The parts of Symfony that take names, and the container.
pub const SYMFONY: &[(&str, &str)] = &[
    (
        "vendor/symfony/AbstractController.php",
        "<?php namespace Symfony\\Bundle\\FrameworkBundle\\Controller; abstract class AbstractController { protected function generateUrl(string $route, array $parameters = [], int $referenceType = 1): string {} protected function redirectToRoute(string $route, array $parameters = [], int $status = 302) {} protected function render(string $view, array $parameters = [], $response = null) {} protected function getParameter(string $name) {} }",
    ),
    (
        "vendor/symfony/ContainerInterface.php",
        "<?php namespace Symfony\\Component\\DependencyInjection;\ninterface ContainerInterface {\n    /**\n     * @template C of object\n     * @param string|class-string<C> $id\n     * @return ($id is class-string<C> ? C|object : object)\n     */\n    public function get(string $id, int $invalidBehavior = 1): ?object;\n}",
    ),
    (
        "vendor/symfony/Autowire.php",
        "<?php namespace Symfony\\Component\\DependencyInjection\\Attribute; #[\\Attribute] class Autowire { public function __construct($value = null, $service = null, $expression = null, $env = null, $param = null, $lazy = false) {} }",
    ),
    (
        "vendor/symfony/Route.php",
        "<?php namespace Symfony\\Component\\Routing\\Attribute; #[\\Attribute] class Route { public function __construct($path = null, $name = null) {} }",
    ),
];

/// The entity manager and the repository classes of Doctrine.
pub const DOCTRINE: &[(&str, &str)] = &[
    (
        "vendor/orm/Entity.php",
        "<?php namespace Doctrine\\ORM\\Mapping; #[\\Attribute] class Entity { public function __construct(public ?string $repositoryClass = null) {} } #[\\Attribute] class Column {}",
    ),
    (
        "vendor/orm/EntityManagerInterface.php",
        "<?php namespace Doctrine\\ORM;\ninterface EntityManagerInterface {\n    /**\n     * @template T of object\n     * @param class-string<T> $className\n     * @return EntityRepository<T>\n     */\n    public function getRepository(string $className): EntityRepository;\n}",
    ),
    (
        "vendor/orm/ObjectRepository.php",
        "<?php namespace Doctrine\\Persistence;\n/** @template-covariant T of object */\ninterface ObjectRepository {\n    /** @return T|null */\n    public function find($id);\n    /** @return T[] */\n    public function findAll();\n    /** @return T[] */\n    public function findBy(array $criteria, ?array $orderBy = null, $limit = null, $offset = null);\n    /** @return T|null */\n    public function findOneBy(array $criteria);\n}",
    ),
    (
        "vendor/orm/EntityRepository.php",
        "<?php namespace Doctrine\\ORM;\n/**\n * @template T of object\n * @template-implements \\Doctrine\\Persistence\\ObjectRepository<T>\n */\nclass EntityRepository implements \\Doctrine\\Persistence\\ObjectRepository {\n    /** @return object|null The entity */\n    public function find(mixed $id, $lockMode = null, $lockVersion = null): object|null {}\n    public function findAll(): array {}\n    public function findBy(array $criteria, ?array $orderBy = null, $limit = null, $offset = null): array {}\n    public function findOneBy(array $criteria, ?array $orderBy = null): object|null {}\n    public function __call(string $method, array $arguments): mixed {}\n}",
    ),
    (
        "vendor/orm/ServiceEntityRepository.php",
        "<?php namespace Doctrine\\Bundle\\DoctrineBundle\\Repository;\n/**\n * @template T of object\n * @template-extends \\Doctrine\\ORM\\EntityRepository<T>\n */\nclass ServiceEntityRepository extends \\Doctrine\\ORM\\EntityRepository {}",
    ),
];

/// Twig's extension classes and the ones of Symfony's bridge that take names.
pub const TWIG: &[(&str, &str)] = &[
    (
        "vendor/symfony/twig/Extension.php",
        "<?php namespace Twig\\Extension; interface ExtensionInterface {} abstract class AbstractExtension implements ExtensionInterface {}",
    ),
    (
        "vendor/symfony/twig/Callables.php",
        "<?php namespace Twig; class TwigFunction {} class TwigFilter {} class TwigTest {}",
    ),
    (
        "vendor/symfony/twig/CoreExtension.php",
        "<?php\nnamespace Twig\\Extension;\nuse Twig\\{TwigFilter, TwigFunction, TwigTest};\nfinal class CoreExtension extends AbstractExtension {\n    public function getFilters(): array { return [new TwigFilter('upper', 'strtoupper'), new TwigFilter('default', [self::class, 'default']), new TwigFilter('length', [self::class, 'length'], ['needs_charset' => true])]; }\n    public function getFunctions(): array { return [new TwigFunction('include', [self::class, 'include'], ['needs_environment' => true, 'needs_context' => true])]; }\n    public function getTests(): array { return [new TwigTest('even', null), new TwigTest('same as', null)]; }\n    /** Counts. */\n    public static function length(string $charset, $thing): int {}\n    public static function include($env, $context, $template, $variables = []) {}\n}\n",
    ),
    (
        "vendor/symfony/twig-bridge/RoutingExtension.php",
        "<?php\nnamespace Symfony\\Bridge\\Twig\\Extension;\nuse Twig\\Extension\\AbstractExtension;\nuse Twig\\TwigFunction;\nfinal class RoutingExtension extends AbstractExtension {\n    public function getFunctions(): array { return [new TwigFunction('path', $this->getPath(...))]; }\n    public function getPath(string $name, array $parameters = [], bool $relative = false): string {}\n}\n",
    ),
    (
        "vendor/symfony/twig-bridge/TranslationExtension.php",
        "<?php\nnamespace Symfony\\Bridge\\Twig\\Extension;\nuse Twig\\Extension\\AbstractExtension;\nuse Twig\\TwigFilter;\nfinal class TranslationExtension extends AbstractExtension {\n    public function getFilters(): array { return [new TwigFilter('trans', [$this, 'trans'])]; }\n    public function trans($message, $arguments = [], ?string $domain = null): string {}\n}\n",
    ),
];

/// Enough of Laravel's validator for rule strings: the rules, a form request and the helper.
pub const VALIDATION: &[(&str, &str)] = &[
    (
        "vendor/laravel/Validator.php",
        "<?php\nnamespace Illuminate\\Validation;\nuse Illuminate\\Validation\\Concerns\\ValidatesAttributes;\nclass Validator { use ValidatesAttributes; }\n",
    ),
    (
        "vendor/laravel/ValidatesAttributes.php",
        "<?php\nnamespace Illuminate\\Validation\\Concerns;\ntrait ValidatesAttributes {\n    /** Validate that a required attribute exists. */\n    public function validateRequired($attribute, $value) {}\n    public function validateRequiredIf($attribute, $value, $parameters) {}\n    public function validateEmail($attribute, $value, $parameters) {}\n    public function validateMax($attribute, $value, $parameters) {}\n    public function validateInteger($attribute, $value) {}\n    public function validateUnique($attribute, $value, $parameters) {}\n    public function validateExists($attribute, $value, $parameters) {}\n    public function validateNullable() {}\n}\n",
    ),
    (
        "vendor/laravel/Request.php",
        "<?php\nnamespace Illuminate\\Http;\n/**\n * @method array validate(array $rules, ...$params)\n */\nclass Request {}\n",
    ),
    (
        "vendor/laravel/FormRequest.php",
        "<?php\nnamespace Illuminate\\Foundation\\Http;\nuse Illuminate\\Http\\Request;\nclass FormRequest extends Request {}\n",
    ),
    (
        "vendor/laravel/validator-helper.php",
        "<?php\nfunction validator(array $data = [], array $rules = [], array $messages = [], array $attributes = []) {}\n",
    ),
    (
        "database/migrations/2020_01_01_000000_create_users_table.php",
        "<?php\nreturn new class {\n    public function up() {\n        Schema::create('users', function (Blueprint $table) {\n            $table->id();\n            $table->string('email');\n        });\n    }\n};\n",
    ),
];
