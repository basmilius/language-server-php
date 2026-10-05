use super::Analyzer;
use crate::testing::{Fixture, split_cursor};

fn var(fixture: &Fixture, code: &str, name: &str) -> String {
    let (_, root, offset) = split_cursor(code);
    let analyzer = Analyzer::new(&fixture.index, &root, offset);
    let env = analyzer.env_at(offset);
    env.get(name)
        .map_or_else(|| "<unset>".to_string(), |ty| ty.display(true))
}

fn library() -> Fixture {
    Fixture::new(&[(
        "Library.php",
        r#"<?php
namespace Lib;

class Foo { public function foo(): int {} }
class Bar extends Foo {}

final class Check {
    /**
     * @template E of object
     * @param class-string<E> $expected
     * @phpstan-assert =E $actual
     */
    public static function instance(string $expected, mixed $actual): void {}

    /** @phpstan-assert !null $actual */
    public static function notNull(mixed $actual): void {}

    /** @phpstan-assert string $actual */
    public static function string(mixed $actual): void {}

    /** @phpstan-assert list<mixed> $actual */
    public static function list(mixed $actual): void {}

    /** @phpstan-assert true $condition */
    public static function true(mixed $condition): void {}

    /** @psalm-assert-if-true Foo $value */
    public static function isFoo(mixed $value): bool {}

    /** @psalm-assert-if-false Foo $value */
    public static function isNotFoo(mixed $value): bool {}

    /** @psalm-assert-if-true int $this->value */
    public function isInt(): bool {}
}

/** @psalm-assert Bar $value */
function assertBar(mixed $value): void {}
"#,
    )])
}

#[test]
fn an_assert_tag_narrows_the_variable_after_the_call() {
    let fixture = library();
    let code = r#"<?php
namespace App;
use Lib\{Check, Foo};
function f(mixed $a, ?Foo $b, int|string $c, mixed $d, mixed $e) {
    Check::instance(Foo::class, $a);
    Check::notNull($b);
    Check::string($c);
    Check::list($d);
    Check::true($e);
    $0
}
"#;
    assert_eq!(var(&fixture, code, "a"), "Foo");
    assert_eq!(var(&fixture, code, "b"), "Foo");
    assert_eq!(var(&fixture, code, "c"), "string");
    assert_eq!(var(&fixture, code, "d"), "list<mixed>");
    assert_eq!(var(&fixture, code, "e"), "true");
}

#[test]
fn an_assert_function_narrows_to_the_type_of_its_tag() {
    let fixture = library();
    let code = r#"<?php
namespace App;
use Lib\Foo;
use function Lib\assertBar;
function f(?Foo $a) {
    assertBar($a);
    $0
}
"#;
    assert_eq!(var(&fixture, code, "a"), "Bar");
}

#[test]
fn if_true_and_if_false_narrow_in_the_branch_they_hold_for() {
    let fixture = library();
    let inside = r#"<?php
namespace App;
use Lib\{Check, Foo};
function f(mixed $a, mixed $b) {
    if (Check::isFoo($a)) {
        $0
    }
}
"#;
    assert_eq!(var(&fixture, inside, "a"), "Foo");
    let negated = r#"<?php
namespace App;
use Lib\{Check, Foo};
function f(mixed $a) {
    if (!Check::isFoo($a)) {
        return;
    }
    $0
}
"#;
    assert_eq!(var(&fixture, negated, "a"), "Foo");
    let if_false = r#"<?php
namespace App;
use Lib\{Check, Foo};
function f(Foo|string $a) {
    if (Check::isNotFoo($a)) {
        return;
    }
    $0
}
"#;
    assert_eq!(var(&fixture, if_false, "a"), "Foo");
    let untouched = r#"<?php
namespace App;
use Lib\{Check, Foo};
function f(mixed $a) {
    if (Check::isFoo($a)) {
    }
    $0
}
"#;
    assert_eq!(var(&fixture, untouched, "a"), "mixed");
}

#[test]
fn assertions_through_this_and_a_receiver() {
    let fixture = library();
    let code = r#"<?php
namespace App;
use Lib\Foo;
class Case1 extends \Lib\Check {
    public function run(mixed $a) {
        $this->notNull($a);
        $0
    }
}
"#;
    assert_eq!(var(&fixture, code, "a"), "mixed");
}
