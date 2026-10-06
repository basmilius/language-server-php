use super::Analyzer;
use crate::testing::{Fixture, split_cursor};

/// The type a variable has at the cursor, as a short string.
fn var(fixture: &Fixture, code: &str, name: &str) -> String {
    let fixture_text = code.to_string();
    let (_, root, offset) = split_cursor(&fixture_text);
    let analyzer = Analyzer::new(&fixture.index, &root, offset);
    let env = analyzer.env_at(offset);
    env.get(name)
        .map_or_else(|| "<unset>".to_string(), |ty| ty.display(true))
}

fn models() -> Fixture {
    Fixture::new(&[
        (
            "Models.php",
            r#"<?php
namespace App;

/**
 * @template TKey
 * @template TValue
 * @implements \IteratorAggregate<TKey, TValue>
 */
class Collection implements \IteratorAggregate {
    /** @return TValue|null */
    public function first() {}
    /** @return static */
    public function filter() {}
    public function getIterator(): \Traversable {}
}

class User {
    public string $name;
    public ?Post $latest = null;
    public static function find(int $id): ?static {}
    public function posts(): Collection {}
    /** @return Collection<int, Post> */
    public function recent(): Collection {}
    public function self(): static {}
}

class Post { public function title(): string {} }
class Admin extends User { public function level(): int {} }

/**
 * @template T
 * @param class-string<T> $class
 * @return T
 */
function make(string $class) {}

/**
 * @template T
 * @param T $value
 * @return T
 */
function identity($value) {}

/**
 * @param ($flag is true ? int : string) $x
 * @return ($flag is true ? int : string)
 */
function pick(bool $flag) {}

enum Status: string { case Active = 'a'; case Gone = 'g'; }
"#,
        ),
        (
            "Iterators.php",
            "<?php\n/**\n * @template TKey\n * @template TValue\n */\ninterface Traversable {}\n/**\n * @template TKey\n * @template TValue\n * @extends Traversable<TKey, TValue>\n */\ninterface IteratorAggregate extends Traversable {}",
        ),
    ])
}

#[test]
fn assignments_new_and_declared_returns() {
    let fixture = models();
    let code = r#"<?php
namespace App;
function f(User $u, ?Post $p = null, int ...$rest) {
    $a = new User();
    $b = User::find(1);
    $c = $u->posts();
    $d = $u->name;
    $e = $u->self();
    $f = [1, 2];
    $g = ['a' => 1, 'b' => 'x'];
    $h = (string) $p;
    $0
}
"#;
    assert_eq!(var(&fixture, code, "u"), "User");
    assert_eq!(var(&fixture, code, "p"), "?Post");
    assert_eq!(var(&fixture, code, "rest"), "list<int>");
    assert_eq!(var(&fixture, code, "a"), "User");
    assert_eq!(var(&fixture, code, "b"), "?User");
    assert_eq!(var(&fixture, code, "c"), "Collection");
    assert_eq!(var(&fixture, code, "d"), "string");
    assert_eq!(var(&fixture, code, "e"), "User");
    assert_eq!(var(&fixture, code, "f"), "list<int>");
    assert_eq!(var(&fixture, code, "g"), "array{a: int, b: string}");
    assert_eq!(var(&fixture, code, "h"), "string");
}

#[test]
fn templates_are_carried_from_receivers_and_arguments() {
    let fixture = models();
    let code = r#"<?php
namespace App;
function f(User $u, Admin $admin) {
    $recent = $u->recent();
    $first = $recent->first();
    $made = make(Post::class);
    $same = identity($admin);
    $filtered = $recent->filter();
    $picked = pick(true);
    $other = pick(false);
    $0
}
"#;
    assert_eq!(var(&fixture, code, "recent"), "Collection<int, Post>");
    assert_eq!(var(&fixture, code, "first"), "?Post");
    assert_eq!(var(&fixture, code, "made"), "Post");
    assert_eq!(var(&fixture, code, "same"), "Admin");
    assert_eq!(var(&fixture, code, "filtered"), "Collection<int, Post>");
    assert_eq!(var(&fixture, code, "picked"), "int");
    assert_eq!(var(&fixture, code, "other"), "string");
}

#[test]
fn foreach_destructuring_and_array_elements() {
    let fixture = models();
    let code = r#"<?php
namespace App;
/**
 * @param list<User> $users
 * @param array<string, Post> $byKey
 */
function f(array $users, array $byKey, User $u) {
    foreach ($users as $index => $user) { }
    foreach ($byKey as $key => $post) { }
    foreach ($u->recent() as $item) { }
    [$first, $second] = $users;
    ['x' => $x] = ['x' => new Post()];
    $element = $users[0];
    $0
}
"#;
    assert_eq!(var(&fixture, code, "user"), "User");
    assert_eq!(var(&fixture, code, "index"), "int");
    assert_eq!(var(&fixture, code, "key"), "string");
    assert_eq!(var(&fixture, code, "post"), "Post");
    assert_eq!(var(&fixture, code, "item"), "Post");
    assert_eq!(var(&fixture, code, "first"), "User");
    assert_eq!(var(&fixture, code, "x"), "Post");
    assert_eq!(var(&fixture, code, "element"), "User");
}

#[test]
fn instanceof_narrows_inside_the_branch_only() {
    let fixture = models();
    let inside = r#"<?php
namespace App;
function f(User $u, $any) {
    if ($u instanceof Admin) {
        $0
    }
}
"#;
    assert_eq!(var(&fixture, inside, "u"), "Admin");
    let anything = r#"<?php
namespace App;
function f($any) {
    if ($any instanceof Post) { $0 }
}
"#;
    assert_eq!(var(&fixture, anything, "any"), "Post");
    let after = r#"<?php
namespace App;
function f(User $u) {
    if ($u instanceof Admin) { }
    $0
}
"#;
    assert_eq!(var(&fixture, after, "u"), "User");
    let early_return = r#"<?php
namespace App;
function f($any) {
    if (!$any instanceof Post) { return; }
    $0
}
"#;
    assert_eq!(var(&fixture, early_return, "any"), "Post");
    let negated = r#"<?php
namespace App;
function f(User|Post $x) {
    if (!($x instanceof User)) { $0 }
}
"#;
    assert_eq!(var(&fixture, negated, "x"), "Post");
}

#[test]
fn null_checks_narrow() {
    let fixture = models();
    let code = r#"<?php
namespace App;
function f(?User $a, ?User $b, ?User $c) {
    if ($a !== null) { $0 }
}
"#;
    assert_eq!(var(&fixture, code, "a"), "User");
    let guard = r#"<?php
namespace App;
function f(?User $a) {
    if ($a === null) { throw new \Exception(); }
    $0
}
"#;
    assert_eq!(var(&fixture, guard, "a"), "User");
    let truthy = r#"<?php
namespace App;
function f(?User $a) {
    if ($a && $a->name) { $0 }
}
"#;
    assert_eq!(var(&fixture, truthy, "a"), "User");
    let is_null = r#"<?php
namespace App;
function f(?User $a) {
    if (is_null($a)) { return; }
    $0
}
"#;
    assert_eq!(var(&fixture, is_null, "a"), "User");
    let still = r#"<?php
namespace App;
function f(?User $a) {
    if ($a !== null) { }
    $0
}
"#;
    assert_eq!(var(&fixture, still, "a"), "?User");
}

#[test]
fn branches_merge_into_unions() {
    let fixture = models();
    let code = r#"<?php
namespace App;
function f(bool $flag) {
    if ($flag) { $x = new User(); } else { $x = new Post(); }
    $0
}
"#;
    assert_eq!(var(&fixture, code, "x"), "User|Post");
    let maybe = r#"<?php
namespace App;
function f(bool $flag) {
    $x = 1;
    if ($flag) { $x = 'a'; }
    $0
}
"#;
    assert_eq!(var(&fixture, maybe, "x"), "string|int");
    let exits = r#"<?php
namespace App;
function f(bool $flag) {
    $x = new User();
    if ($flag) { $x = new Post(); return; }
    $0
}
"#;
    assert_eq!(var(&fixture, exits, "x"), "User");
}

#[test]
fn closures_see_what_they_use_and_arrow_functions_see_everything() {
    let fixture = models();
    let code = r#"<?php
namespace App;
function f(User $u, Post $p) {
    $fn = function (Admin $a) use ($u) { $0 };
}
"#;
    assert_eq!(var(&fixture, code, "u"), "User");
    assert_eq!(var(&fixture, code, "a"), "Admin");
    assert_eq!(var(&fixture, code, "p"), "<unset>");
    let arrow = r#"<?php
namespace App;
function f(User $u, Post $p) {
    $fn = fn(Admin $a) => $0;
}
"#;
    assert_eq!(var(&fixture, arrow, "p"), "Post");
    assert_eq!(var(&fixture, arrow, "a"), "Admin");
}

#[test]
fn docblock_var_overrides_and_enums_resolve() {
    let fixture = models();
    let code = r#"<?php
namespace App;
function f() {
    /** @var Post $thing */
    $thing = something();
    /** @var Admin */
    $other = something();
    $status = Status::Active;
    $value = $status->value;
    $cases = Status::cases();
    $from = Status::from('a');
    $0
}
"#;
    assert_eq!(var(&fixture, code, "thing"), "Post");
    assert_eq!(var(&fixture, code, "other"), "Admin");
    assert_eq!(var(&fixture, code, "status"), "Status");
    assert_eq!(var(&fixture, code, "value"), "string");
    assert_eq!(var(&fixture, code, "cases"), "list<Status>");
    assert_eq!(var(&fixture, code, "from"), "Status");
}

#[test]
fn this_and_static_follow_the_class() {
    let code = r#"<?php
namespace App;
class Repo extends User {
    public function go(): void {
        $me = $this;
        $self = $this->self();
        $static = static::find(1);
        $parent = parent::find(2);
        $0
    }
}
"#;
    let fixture = models().with_current(code);
    assert_eq!(var(&fixture, code, "me"), "static");
    assert_eq!(var(&fixture, code, "self"), "Repo");
    assert_eq!(var(&fixture, code, "static"), "?Repo");
    assert_eq!(var(&fixture, code, "parent"), "?User");
}

#[test]
fn array_building_and_unknowns() {
    let fixture = models();
    let code = r#"<?php
namespace App;
function f() {
    $list = [];
    $list[] = new User();
    $list[] = new Post();
    $map = [];
    $map['a'] = new Post();
    $unknown = nothing();
    $0
}
"#;
    assert_eq!(var(&fixture, code, "list"), "list<User|Post>");
    assert_eq!(var(&fixture, code, "map"), "array<string, Post>");
    assert_eq!(var(&fixture, code, "unknown"), "mixed");
}

#[test]
fn nested_array_writes_build_the_element_type() {
    let fixture = models();
    let code = r#"<?php
namespace App;
/** @param list<Post> $posts */
function f(array $posts) {
    $byUser = [];
    foreach ($posts as $post) {
        $byUser[1][] = $post;
    }
    $grid = [];
    $grid['a']['b'] = new User();
    $0
}
"#;
    assert_eq!(var(&fixture, code, "byUser"), "array<int, list<Post>>");
    assert_eq!(var(&fixture, code, "grid"), "array<string, array<string, User>>");
}

#[test]
fn nested_array_writes_survive_a_later_loop() {
    let fixture = models();
    let code = r#"<?php
namespace App;
/** @param list<Post> $posts @param list<string> $ids */
function f(array $posts, array $ids, User $user) {
    $byUser = [];
    foreach ($posts as $post) {
        $byUser[$user->name][] = $post;
    }
    foreach ($ids as $id) {
        if (!isset($ids[$id])) {
            throw new \Exception();
        }
        foreach ($byUser[$id] ?? [] as $item) {
            $0
        }
    }
}
"#;
    assert_eq!(var(&fixture, code, "byUser"), "array<string, list<Post>>");
    assert_eq!(var(&fixture, code, "item"), "Post");
}

#[test]
fn a_long_chain_of_unknown_calls_is_typed_in_one_pass() {
    let fixture = models();
    let chain = "->step()".repeat(40);
    let code = format!("<?php\nnamespace App;\nfunction f() {{\n    $x = unknown(){chain};\n    $0\n}}\n");
    assert_eq!(var(&fixture, &code, "x"), "mixed");
}

#[test]
fn a_template_of_an_omitted_argument_is_what_its_default_is() {
    let fixture = Fixture::new(&[(
        "pick.php",
        "<?php\n/**\n * @template T\n * @template D\n * @param array<int, T> $items\n * @param D $default\n * @return T|D\n */\nfunction pick(array $items, $default = null) {}\n",
    )]);
    let code = "<?php\n$items = [1, 2];\n$a = pick($items);\n$b = pick($items, 'x');\n$0";
    assert_eq!(var(&fixture, code, "a"), "?int");
    assert_eq!(var(&fixture, code, "b"), "int|string");
}

#[test]
fn type_aliases_are_read_where_they_are_declared_and_imported() {
    let fixture = Fixture::new(&[
        (
            "Shapes.php",
            "<?php\nnamespace App;\n/**\n * @psalm-type Point = array{x: int, y: int}\n * @phpstan-type Line array{from: Point, to: Point}\n */\nclass Shapes {\n    /** @return Line */\n    public function line() {}\n}\n",
        ),
        (
            "Canvas.php",
            "<?php\nnamespace App;\n/**\n * @psalm-import-type Point from Shapes\n * @phpstan-import-type Line from Shapes as Segment\n */\nclass Canvas {\n    /** @return Point */\n    public function origin() {}\n    /** @param Segment $segment */\n    public function draw($segment) {\n        /** @var Point $end */\n        $end = $segment['to'];\n        $0\n    }\n}\n",
        ),
    ]);
    let canvas = fixture
        .sources
        .get(&std::path::PathBuf::from("/project/Canvas.php"))
        .cloned()
        .expect("canvas");
    assert_eq!(
        var(&fixture, &canvas, "segment"),
        "array{from: array{x: int, y: int}, to: array{x: int, y: int}}"
    );
    assert_eq!(var(&fixture, &canvas, "end"), "array{x: int, y: int}");
    let code = "<?php\nnamespace App;\nfunction f(Shapes $shapes, Canvas $canvas) {\n    $line = $shapes->line();\n    $origin = $canvas->origin();\n    $0\n}\n";
    assert_eq!(
        var(&fixture, code, "line"),
        "array{from: array{x: int, y: int}, to: array{x: int, y: int}}"
    );
    assert_eq!(var(&fixture, code, "origin"), "array{x: int, y: int}");
}

#[test]
fn a_partial_application_is_a_closure() {
    let fixture = Fixture::new(&[(
        "f.php",
        "<?php\nfunction pad(string $text, int $width, string $with = ' '): string {}\n",
    )]);
    let code = "<?php\n$left = pad(?, 10);\n$rest = pad('a', ...);\n$named = pad(text: 'a', width: ?);\n$0";
    assert_eq!(var(&fixture, code, "left"), "Closure");
    assert_eq!(var(&fixture, code, "rest"), "Closure");
    assert_eq!(var(&fixture, code, "named"), "Closure");
}

#[test]
fn a_trait_method_inherits_the_documented_return_of_the_interface() {
    let fixture = Fixture::new(&[(
        "Raxos.php",
        r#"<?php
namespace Raxos;

/**
 * @template TKey of array-key
 * @template TValue
 */
interface ArrayListInterface {
    /**
     * @param TValue|null $default
     * @return TValue|null
     */
    public function first(?callable $predicate = null, mixed $default = null): mixed;
}

trait ArrayListable {
    /**
     * {@inheritdoc}
     */
    public function first(?callable $predicate = null, mixed $default = null): mixed {}
}

/**
 * @template TKey of array-key
 * @template TValue
 * @implements ArrayListInterface<TKey, TValue>
 */
class ArrayList implements ArrayListInterface {
    use ArrayListable;
}

/**
 * @template TKey of array-key
 * @template TValue of Model
 * @implements ArrayListInterface<TKey, TValue>
 */
class ModelArrayList extends ArrayList {}

class Model {}
class Product extends Model { public string $id; }

class AppTeam extends Model {
    /** @var ModelArrayList<int, Product> */
    public ModelArrayList $products;
}
"#,
    )]);
    let code = r#"<?php
namespace Raxos;
function f(AppTeam $team) {
    $list = $team->products;
    $first = $team->products->first();
    $0
}
"#;
    assert_eq!(var(&fixture, code, "list"), "ModelArrayList<int, Product>");
    assert_eq!(var(&fixture, code, "first"), "?Product");
}

#[test]
fn list_destructuring_takes_each_element_by_position() {
    let fixture = Fixture::new(&[(
        "Range.php",
        "<?php\nnamespace App;\nclass Range {\n    /** @return array{0: \\DateTime, 1: int}|null */\n    public static function keyed(): ?array {}\n    /** @return array{\\DateTime, int} */\n    public static function listed(): array {}\n}\n",
    )]);
    let keyed = "<?php\nnamespace App;\nfunction f() {\n    $range = Range::keyed();\n    if ($range === null) {\n        return;\n    }\n    [$from, $count] = $range;\n    $0\n}\n";
    assert_eq!(var(&fixture, keyed, "from"), "DateTime");
    assert_eq!(var(&fixture, keyed, "count"), "int");
    let listed = "<?php\nnamespace App;\nfunction f() {\n    [$from, $count] = Range::listed();\n    $0\n}\n";
    assert_eq!(var(&fixture, listed, "from"), "DateTime");
    assert_eq!(var(&fixture, listed, "count"), "int");
    let literal = "<?php\nfunction f() {\n    [$number, $text] = [1, 'x'];\n    $0\n}\n";
    assert_eq!(var(&fixture, literal, "number"), "int");
    assert_eq!(var(&fixture, literal, "text"), "string");
}

#[test]
fn a_generator_with_one_argument_gives_its_values() {
    let fixture = Fixture::new(&[
        (
            "Generator.php",
            "<?php\n/**\n * @template-covariant TKey\n * @template-covariant TValue\n * @template TSend\n * @template-covariant TReturn\n */\nfinal class Generator implements Iterator {}\ninterface Iterator extends Traversable {}\ninterface Traversable {}\n",
        ),
        (
            "Query.php",
            "<?php\nnamespace App;\nclass Order {}\nclass Query {\n    /** @return \\Generator<Order> */\n    public function cursor(): \\Generator {}\n    /** @return \\Generator<int, Order> */\n    public function keyed(): \\Generator {}\n}\n",
        ),
    ]);
    let single = "<?php\nnamespace App;\nfunction f(Query $query) {\n    foreach ($query->cursor() as $order) {\n        $0\n    }\n}\n";
    assert_eq!(var(&fixture, single, "order"), "Order");
    let keyed = "<?php\nnamespace App;\nfunction f(Query $query) {\n    foreach ($query->keyed() as $key => $order) {\n        $0\n    }\n}\n";
    assert_eq!(var(&fixture, keyed, "order"), "Order");
    assert_eq!(var(&fixture, keyed, "key"), "int");
}

#[test]
fn max_and_min_give_what_they_compare() {
    let fixture = Fixture::new(&[(
        "Math.php",
        "<?php\nfunction max(mixed $value, mixed ...$values): mixed {}\nfunction min(mixed $value, mixed ...$values): mixed {}\n",
    )]);
    let code = "<?php\n/** @param list<float> $prices */\nfunction f(int $limit, array $prices, ?int $maybe) {\n    $bounded = max(1, min(100, $limit));\n    $highest = max($prices);\n    $mixed = max($limit, 1.5);\n    $unknown = max($limit, $nothing);\n    $0\n}\n";
    assert_eq!(var(&fixture, code, "bounded"), "int");
    assert_eq!(var(&fixture, code, "highest"), "float");
    assert_eq!(var(&fixture, code, "mixed"), "int|float");
    assert_eq!(var(&fixture, code, "unknown"), "mixed");
}

#[test]
fn a_param_tag_that_repeats_the_native_type_keeps_the_inherited_one() {
    let bus = "<?php\nnamespace App;\ninterface MessageInterface {}\nclass Ping implements MessageInterface {}\n/** @template TMessage of MessageInterface */\ninterface HandlerInterface {\n    /** @param TMessage $message */\n    public function handle(MessageInterface $message): void;\n}\n";
    let message_in = |tag: &str| {
        let handler = format!(
            "<?php\nnamespace App;\n/** @implements HandlerInterface<Ping> */\nclass PingHandler implements HandlerInterface {{\n    /** {tag} */\n    public function handle(MessageInterface $message): void {{\n        $0\n    }}\n}}\n"
        );
        let fixture = Fixture::new(&[("Bus.php", bus), ("PingHandler.php", &handler.replace("$0", ""))]);
        var(&fixture, &handler, "message")
    };
    assert_eq!(message_in("@return void"), "Ping");
    assert_eq!(message_in("@param MessageInterface $message"), "Ping");
    assert_eq!(message_in("@param MessageInterface|null $message"), "?MessageInterface");
}

#[test]
fn a_callback_binds_the_templates_of_its_signature() {
    let list = "<?php\nnamespace App;\nclass Product { public string $name; }\n/**\n * @template TKey of array-key\n * @template TValue\n */\ninterface ListOf {\n    /**\n     * @template TMappedValue\n     * @param callable(TValue):TMappedValue $fn\n     * @return ListOf<TKey, TMappedValue>\n     */\n    public function map(callable $fn): ListOf;\n    /**\n     * @template TResult of mixed\n     * @param callable(TResult, TValue, TKey):TResult $fn\n     * @param TResult $initial\n     * @return TResult\n     */\n    public function reduce(callable $fn, mixed $initial = null): mixed;\n}\n";
    let fixture = Fixture::new(&[("ListOf.php", list)]);
    let code = "<?php\nnamespace App;\n/** @param ListOf<int, Product> $products */\nfunction f(ListOf $products) {\n    $typed = $products->map(fn(Product $product): string => $product->name);\n    $inferred = $products->map(fn(Product $product) => $product->name);\n    $total = $products->reduce(fn(int $sum, Product $product) => $sum + 1, 0);\n    $0\n}\n";
    let untyped = "<?php\nnamespace App;\n/** @param ListOf<int, Product> $products */\nfunction f(ListOf $products) {\n    $products->map(function ($product) {\n        $0\n    });\n}\n";
    assert_eq!(var(&fixture, code, "typed"), "ListOf<int, string>");
    assert_eq!(var(&fixture, code, "inferred"), "ListOf<int, string>");
    assert_eq!(var(&fixture, code, "total"), "int");
    assert_eq!(var(&fixture, untyped, "product"), "Product");
}

#[test]
fn a_new_variable_passed_by_reference_takes_the_parameter_type() {
    let fixture = Fixture::new(&[(
        "Pcre.php",
        "<?php\n/** @param string[] $matches */\nfunction preg_match(string $pattern, string $subject, ?array &$matches = null): int|false {}\nfunction sort(array &$array): bool {}\n",
    )]);
    let code = "<?php\n/** @param list<int> $numbers */\nfunction f(string $line, array $numbers) {\n    if (preg_match('/x/', $line, $matches)) {}\n    sort($numbers);\n    $0\n}\n";
    assert_eq!(var(&fixture, code, "matches"), "list<string>");
    assert_eq!(var(&fixture, code, "numbers"), "list<int>");
}

#[test]
fn an_inherited_closure_signature_types_the_call() {
    let middleware = "<?php\nnamespace App;\nclass Response {}\ninterface Middleware {\n    /** @param \\Closure(string):Response $next */\n    public function handle(string $request, \\Closure $next): Response;\n}\n";
    let cors = "<?php\nnamespace App;\nclass Cors implements Middleware {\n    public function handle(string $request, \\Closure $next): Response {\n        $response = $next($request);\n        $0\n    }\n}\n";
    let fixture = Fixture::new(&[("Middleware.php", middleware), ("Cors.php", &cors.replace("$0", ""))]);
    assert_eq!(var(&fixture, cors, "next"), "Closure(string): Response");
    assert_eq!(var(&fixture, cors, "response"), "Response");
}
