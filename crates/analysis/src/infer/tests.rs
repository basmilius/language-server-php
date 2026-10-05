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
