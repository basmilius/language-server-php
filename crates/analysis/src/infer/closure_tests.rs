use super::Analyzer;
use crate::testing::{Fixture, split_cursor};

fn var(fixture: &Fixture, code: &str, name: &str) -> String {
    let (_, root, offset) = split_cursor(code);
    let analyzer = Analyzer::new(&fixture.index, &root, offset);
    let env = analyzer.env_at(offset);
    env.get(name)
        .map_or_else(|| "<unset>".to_string(), |ty| ty.display(true))
}

fn fixture() -> Fixture {
    Fixture::on_disk(
        &[(
            "Models.php",
            r#"<?php
namespace App;

class User {
    public string $name;
    public int $age;
    public function posts(): Posts {}
}
class Post { public function title(): string {} }

/**
 * @template TKey of array-key
 * @template TValue
 */
class Bag {
    /**
     * @template TMap
     * @param callable(TValue, TKey): TMap $callback
     * @return Bag<TKey, TMap>
     */
    public function map(callable $callback): Bag {}

    /** @param callable(TValue): bool $callback */
    public function each(callable $callback): void {}
}

class Maker {
    public function user() { return new User(); }
    public function self() { return $this; }
    public function number() { if ($this->flag()) { return 1; } return 2.5; }
    public function flag(): bool {}
    public function nothing() { return; }
    public function unknown() { return mystery(); }
    public function chain() { return $this->user()->posts(); }
    public function names() { yield 'a' => new User(); yield 'b' => new User(); }
    public function listed() { yield new Post(); }
}

function make() { return new Post(); }
function recursive() { return recursive(); }
"#,
        )],
        &[(
            "standard.php",
            r#"<?php
function array_map(?callable $callback, array $array): array {}
function array_filter(array $array, ?callable $callback = null, int $mode = 0): array {}
function array_reduce(array $array, callable $callback, mixed $initial = null): mixed {}
function usort(array &$array, callable $callback) {}
function array_values(array $array): array {}
function current(object|array $array): mixed {}
/** @template TKey @template TValue */
interface Traversable {}
/**
 * @template TKey
 * @template TValue
 * @extends Traversable<TKey, TValue>
 */
interface Iterator extends Traversable {}
/**
 * @template TKey
 * @template TValue
 * @template TSend
 * @template TReturn
 * @implements Iterator<TKey, TValue>
 */
final class Generator implements Iterator {}
"#,
        )],
    )
}

#[test]
fn array_map_types_the_closure_parameter_and_the_result() {
    let fixture = fixture();
    let code = r#"<?php
namespace App;
/** @param list<User> $users */
function f(array $users) {
    $names = array_map(fn($user) => $user->name, $users);
    $posts = array_map(function ($user) {
        $0
        return $user->posts();
    }, $users);
}
"#;
    assert_eq!(var(&fixture, code, "user"), "User");
    let after = r#"<?php
namespace App;
/** @param list<User> $users */
function f(array $users) {
    $names = array_map(fn($user) => $user->name, $users);
    $posts = array_map(function ($user) { return $user->posts(); }, $users);
    $0
}
"#;
    assert_eq!(var(&fixture, after, "names"), "array<int, string>");
    assert_eq!(var(&fixture, after, "posts"), "array<int, Posts>");
}

#[test]
fn a_closure_parameter_with_its_own_type_keeps_it() {
    let fixture = fixture();
    let code = r#"<?php
namespace App;
/** @param list<User> $users */
function f(array $users) {
    array_map(fn(object $user) => $0, $users);
}
"#;
    assert_eq!(var(&fixture, code, "user"), "object");
}

#[test]
fn array_filter_usort_and_array_reduce_type_their_callbacks() {
    let fixture = fixture();
    let filter = r#"<?php
namespace App;
/** @param array<string, User> $users */
function f(array $users) {
    $adults = array_filter($users, fn($user, $key) => $user->age > 17);
    array_filter($users, function ($user, $key) { $0 });
}
"#;
    assert_eq!(var(&fixture, filter, "user"), "User");
    assert_eq!(var(&fixture, filter, "key"), "string");
    let kept = r#"<?php
namespace App;
/** @param array<string, User> $users */
function f(array $users) {
    $adults = array_filter($users, fn($user) => $user->age > 17);
    $0
}
"#;
    assert_eq!(var(&fixture, kept, "adults"), "array<string, User>");
    let sorted = r#"<?php
namespace App;
/** @param list<User> $users */
function f(array $users) {
    usort($users, function ($left, $right) { $0 });
}
"#;
    assert_eq!(var(&fixture, sorted, "left"), "User");
    assert_eq!(var(&fixture, sorted, "right"), "User");
    let reduced = r#"<?php
namespace App;
/** @param list<User> $users */
function f(array $users) {
    $total = array_reduce($users, fn($carry, $user) => $carry + $user->age, 0);
    array_reduce($users, function ($carry, $user) { $0 }, 0);
}
"#;
    assert_eq!(var(&fixture, reduced, "user"), "User");
    assert_eq!(var(&fixture, reduced, "carry"), "int");
}

#[test]
fn only_a_use_key_filter_gives_the_callback_the_key() {
    let fixture = fixture();
    let code = r#"<?php
namespace App;
/** @param array<string, User> $users */
function f(array $users) {
    array_filter($users, function ($key) { $0 }, ARRAY_FILTER_USE_KEY);
}
"#;
    assert_eq!(var(&fixture, code, "key"), "string");
}

#[test]
fn methods_with_callable_templates_bind_from_the_receiver() {
    let fixture = fixture();
    let code = r#"<?php
namespace App;
/** @param Bag<int, User> $bag */
function f(Bag $bag) {
    $names = $bag->map(fn($user, $key) => $user->name);
    $bag->each(function ($user) { $0 });
}
"#;
    assert_eq!(var(&fixture, code, "user"), "User");
    let result = r#"<?php
namespace App;
/** @param Bag<int, User> $bag */
function f(Bag $bag) {
    $names = $bag->map(fn($user, $key) => $user->name);
    $0
}
"#;
    assert_eq!(var(&fixture, result, "names"), "Bag<int, string>");
}

#[test]
fn array_functions_keep_the_element_type() {
    let fixture = fixture();
    let code = r#"<?php
namespace App;
/** @param array<string, User> $users */
function f(array $users) {
    $values = array_values($users);
    $first = current($users);
    $0
}
"#;
    assert_eq!(var(&fixture, code, "values"), "list<User>");
    assert_eq!(var(&fixture, code, "first"), "User|false");
}

#[test]
fn return_types_come_from_the_body_when_none_is_declared() {
    let fixture = fixture();
    let code = r#"<?php
namespace App;
function f(Maker $maker) {
    $user = $maker->user();
    $self = $maker->self();
    $number = $maker->number();
    $nothing = $maker->nothing();
    $unknown = $maker->unknown();
    $chain = $maker->chain();
    $post = make();
    $loop = recursive();
    $0
}
"#;
    assert_eq!(var(&fixture, code, "user"), "User");
    assert_eq!(var(&fixture, code, "self"), "Maker");
    assert_eq!(var(&fixture, code, "number"), "int|float");
    assert_eq!(var(&fixture, code, "nothing"), "null");
    assert_eq!(var(&fixture, code, "unknown"), "mixed");
    assert_eq!(var(&fixture, code, "chain"), "Posts");
    assert_eq!(var(&fixture, code, "post"), "Post");
    assert_eq!(var(&fixture, code, "loop"), "mixed");
}

#[test]
fn a_generator_body_gives_its_key_and_value_types() {
    let fixture = fixture();
    let code = r#"<?php
namespace App;
function f(Maker $maker) {
    foreach ($maker->names() as $key => $user) {
        $0
    }
}
"#;
    assert_eq!(var(&fixture, code, "key"), "string");
    assert_eq!(var(&fixture, code, "user"), "User");
    let listed = r#"<?php
namespace App;
function f(Maker $maker) {
    foreach ($maker->listed() as $key => $post) {
        $0
    }
}
"#;
    assert_eq!(var(&fixture, listed, "key"), "int");
    assert_eq!(var(&fixture, listed, "post"), "Post");
}

#[test]
fn a_closure_called_later_returns_what_its_body_returns() {
    let fixture = fixture();
    let code = r#"<?php
namespace App;
function f() {
    $make = fn() => new User();
    $made = $make();
    $block = function () { return new Post(); };
    $built = $block();
    $0
}
"#;
    assert_eq!(var(&fixture, code, "made"), "User");
    assert_eq!(var(&fixture, code, "built"), "Post");
}
