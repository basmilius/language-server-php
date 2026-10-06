//! Each inspection against code that has the problem and code that looks like it and does not.

use php_index::Index;
use php_index::Origin;
use php_index::extract::{ExtractOptions, extract};
use php_syntax::{PhpVersion, parse};
use std::path::PathBuf;
use std::sync::Arc;

use super::{Externals, InspectionEnv, InspectionSettings, Override, inspect};
use crate::DiagnosticSeverity;

pub(crate) const STUBS: &str = r#"<?php
namespace {
    function strlen(string $string): int {}
    function count(Countable|array $value, int $mode = COUNT_NORMAL): int {}
    function array_map(?callable $callback, array $array, array ...$arrays): array {}
    function preg_match(string $pattern, string $subject, &$matches = null, int $flags = 0, int $offset = 0): int|false {}
    function sprintf(string $format, mixed ...$values): string {}
    function in_array(mixed $needle, array $haystack, bool $strict = false): bool {}
    function compact($var_name, ...$var_names): array {}
    function extract(array &$array, int $flags = 0, string $prefix = ""): int {}
    function class_exists(string $class, bool $autoload = true): bool {}
    function function_exists(string $function): bool {}
    function defined(string $constant_name): bool {}
    #[Deprecated(since: "9.0")]
    function future_thing(): void {}
    #[Deprecated(since: "8.2")]
    function utf8_encode(string $string): string {}
    /** @deprecated use something else */
    function old_thing(): void {}
    const PHP_EOL = "\n";
    const COUNT_NORMAL = 0;
    interface Countable { public function count(): int; }
    interface Traversable {}
    interface IteratorAggregate extends Traversable { public function getIterator(): Traversable; }
    interface Stringable { public function __toString(): string; }
    interface Throwable extends Stringable {}
    class Exception implements Throwable {
        public function __construct(string $message = "", int $code = 0, ?Throwable $previous = null) {}
        public function getMessage(): string {}
        public function __toString(): string {}
    }
    class RuntimeException extends Exception {}
    class stdClass {}
    class ArrayObject {}
    final class Closure { public function __invoke() {} public function call(object $newThis, mixed ...$args): mixed {} }
    class DateTime { public function format(string $format): string {} }
}
"#;

pub(super) struct Found {
    pub code: &'static str,
    pub text: String,
    pub severity: DiagnosticSeverity,
}

pub(crate) fn index_with(files: &[(&str, &str)], current: &str) -> Index {
    let mut index = Index::new(PhpVersion::V8_4);
    let stubs = extract(&parse(STUBS).syntax(), ExtractOptions { stub: true });
    index.set_file(PathBuf::from("/stubs/core.php"), Origin::Stub, Arc::new(stubs));
    for (path, text) in files {
        let symbols = extract(&parse(text).syntax(), ExtractOptions::default());
        index.set_file(
            PathBuf::from(format!("/project/{path}")),
            Origin::Project,
            Arc::new(symbols),
        );
    }
    let symbols = extract(&parse(current).syntax(), ExtractOptions::default());
    index.set_file(
        PathBuf::from("/project/current.php"),
        Origin::Project,
        Arc::new(symbols),
    );
    index
}

pub(super) fn run_with(files: &[(&str, &str)], source: &str, settings: &InspectionSettings) -> Vec<Found> {
    let index = index_with(files, source);
    let tree = parse(source);
    let root = tree.syntax();
    let externals = Externals::none();
    let env = InspectionEnv {
        index: &index,
        text: source,
        root: &root,
        settings,
        ready: true,
        externals: &externals,
    };
    inspect(&env)
        .into_iter()
        .map(|finding| Found {
            code: finding.diagnostic.code,
            text: source[usize::from(finding.diagnostic.range.start())..usize::from(finding.diagnostic.range.end())]
                .to_string(),
            severity: finding.diagnostic.severity,
        })
        .collect()
}

pub(super) fn check(source: &str) -> Vec<(&'static str, String)> {
    check_with(&[], source)
}

pub(super) fn check_with(files: &[(&str, &str)], source: &str) -> Vec<(&'static str, String)> {
    run_with(files, source, &InspectionSettings::default())
        .into_iter()
        .map(|found| (found.code, found.text))
        .collect()
}

/// The findings with one code.
pub(super) fn only(code: &str, source: &str) -> Vec<String> {
    check(source)
        .into_iter()
        .filter(|(found, _)| *found == code)
        .map(|(_, text)| text)
        .collect()
}

fn none(source: &str) {
    let found = check(source);
    assert!(found.is_empty(), "expected nothing, found {found:?} in\n{source}");
}

#[test]
fn reports_undefined_classes_where_php_would_fail() {
    let source = "<?php\nnamespace App;\n$a = new Missing();\nMissing::run();\nfunction f(Missing $x): Missing {}\nclass B extends Missing {}\n";
    let found = only("undefined-class", source);
    assert_eq!(found, ["Missing", "Missing", "Missing", "Missing", "Missing"]);
}

#[test]
fn leaves_classes_alone_that_are_only_named() {
    none(
        "<?php\nnamespace App;\n$name = Missing::class;\ntry { f(); } catch (\\Foo\\Gone $e) {}\nif ($x instanceof Absent) {}\n#[Attr]\nfunction f() {}\n",
    );
}

#[test]
fn finds_classes_through_imports_and_the_namespace() {
    let files = [
        ("A.php", "<?php\nnamespace Lib;\nclass Tool {}\n"),
        ("B.php", "<?php\nnamespace App;\nclass Local {}\n"),
    ];
    let source = "<?php\nnamespace App;\nuse Lib\\Tool;\n$a = new Tool();\n$b = new Local();\n$c = new \\Exception();\n$d = new Missing\\Deeper();\n";
    assert_eq!(
        check_with(&files, source),
        [("undefined-class", "Missing\\Deeper".to_string())]
    );
}

#[test]
fn a_global_class_in_a_namespace_needs_an_import() {
    let found = only("undefined-class", "<?php\nnamespace App;\n$e = new Exception();\n");
    assert_eq!(found, ["Exception"]);
}

#[test]
fn does_not_report_names_behind_an_existence_check() {
    none(
        "<?php\nif (class_exists('Memcached')) { $m = new Memcached(); }\nif (function_exists('gone')) { gone(); }\nif (defined('GONE')) { echo GONE; }\n",
    );
}

#[test]
fn reports_undefined_functions_and_constants() {
    let source = "<?php\nstrlen('a');\nmissing_function(1);\necho PHP_EOL, MISSING_CONSTANT, true, null;\n";
    assert_eq!(
        check(source),
        [
            ("undefined-function", "missing_function".to_string()),
            ("undefined-constant", "MISSING_CONSTANT".to_string()),
        ]
    );
}

#[test]
fn nothing_is_undefined_until_the_index_is_ready() {
    let index = index_with(&[], "<?php\nnew Missing();\n");
    let tree = parse("<?php\nnew Missing();\n");
    let root = tree.syntax();
    let externals = Externals::none();
    let settings = InspectionSettings::default();
    let env = InspectionEnv {
        index: &index,
        text: "<?php\nnew Missing();\n",
        root: &root,
        settings: &settings,
        ready: false,
        externals: &externals,
    };
    assert!(inspect(&env).is_empty());
}

#[test]
fn names_held_back_by_an_extension_are_not_undefined() {
    let index = index_with(&[], "<?php\nnew Redis();\nredis_thing();\n");
    let source = "<?php\nnew Redis();\nredis_thing();\n";
    let tree = parse(source);
    let root = tree.syntax();
    let is_redis = |name: &str| name == "Redis" || name == "redis_thing";
    let externals = Externals {
        class: &is_redis,
        function: &is_redis,
        constant: &is_redis,
    };
    let settings = InspectionSettings::default();
    let env = InspectionEnv {
        index: &index,
        text: source,
        root: &root,
        settings: &settings,
        ready: true,
        externals: &externals,
    };
    assert!(inspect(&env).is_empty());
}

#[test]
fn settings_switch_inspections_and_move_their_severity() {
    let source = "<?php\nnew Missing();\n";
    let mut settings = InspectionSettings::default();
    settings.set(
        "undefined-class",
        Override {
            enabled: Some(false),
            severity: None,
        },
    );
    assert!(run_with(&[], source, &settings).is_empty());
    settings.set(
        "undefined-class",
        Override {
            enabled: None,
            severity: Some(DiagnosticSeverity::Hint),
        },
    );
    let found = run_with(&[], source, &settings);
    assert_eq!(found[0].severity, DiagnosticSeverity::Hint);
}

#[test]
fn flags_deprecated_functions_classes_and_members() {
    let files = [(
        "A.php",
        "<?php\n/** @deprecated gone soon */\nclass Old {}\nclass Fresh {\n    /** @deprecated */\n    public function legacy(): void {}\n    /** @deprecated */\n    public const OLD = 1;\n    /** @deprecated */\n    public int $stale = 0;\n}\n",
    )];
    let source = "<?php\nold_thing();\nutf8_encode('x');\n$o = new Old();\n$f = new Fresh();\n$f->legacy();\n$f->stale;\nFresh::OLD;\n";
    let found: Vec<_> = check_with(&files, source)
        .into_iter()
        .filter(|(code, _)| *code == "deprecated")
        .collect();
    assert_eq!(
        found.iter().map(|(_, text)| text.as_str()).collect::<Vec<_>>(),
        ["old_thing", "utf8_encode", "Old", "legacy", "stale", "OLD"]
    );
}

#[test]
fn a_deprecated_attribute_with_a_later_since_is_not_deprecated_yet() {
    none("<?php\nfuture_thing();\n");
}

#[test]
fn reports_methods_a_class_does_not_have() {
    let files = [("Foo.php", "<?php\nclass Foo { public function real(): void {} }\n")];
    let source = "<?php\nfunction f(Foo $foo) {\n    $foo->real();\n    $foo->fake();\n    Foo::alsoFake();\n}\n";
    let found = check_with(&files, source);
    assert_eq!(
        found,
        [
            ("undefined-method", "fake".to_string()),
            ("undefined-method", "alsoFake".to_string()),
            ("static-call-of-instance-method", "real".to_string()),
        ]
        .into_iter()
        .filter(|(code, _)| *code == "undefined-method")
        .collect::<Vec<_>>()
    );
}

#[test]
fn stays_silent_where_the_class_may_have_more_than_it_declares() {
    let files = [
        (
            "Magic.php",
            "<?php\nclass Magic { public function __call($n, $a) {} public function __get($n) {} }\n",
        ),
        ("Child.php", "<?php\nclass Child extends Gone { }\n"),
        (
            "Doc.php",
            "<?php\n/** @method int dynamic()\n * @property string $virtual */\nclass Doc {}\n",
        ),
        (
            "Tr.php",
            "<?php\ntrait Tr { public function fromTrait() { $this->host(); } }\n",
        ),
    ];
    let source = "<?php\nfunction f(Magic $m, Child $c, Doc $d) {\n    $m->anything();\n    $m->prop;\n    $c->whatever();\n    $d->dynamic();\n    $d->virtual;\n}\n";
    assert_eq!(check_with(&files, source), []);
}

#[test]
fn reports_undefined_properties_on_the_projects_own_classes() {
    let files = [(
        "Foo.php",
        "<?php\nclass Foo { public int $real = 1; public function __construct(public string $promoted = '') {} }\n",
    )];
    let source = "<?php\nfunction f(Foo $foo, \\stdClass $std) {\n    $foo->real;\n    $foo->promoted;\n    $foo->fake;\n    $std->anything;\n    isset($foo->maybe);\n    $foo->maybe ?? 1;\n}\n";
    assert_eq!(check_with(&files, source), [("undefined-property", "fake".to_string())]);
}

#[test]
fn does_not_trust_a_receiver_that_a_loop_changes() {
    let files = [(
        "A.php",
        "<?php\nclass A { public function next(): B {} }\nclass B { public function other(): void {} public function next(): B {} }\n",
    )];
    let source = "<?php\nfunction f(A $a) {\n    $node = $a;\n    while ($node) {\n        $node->other();\n        $node = $node->next();\n    }\n}\n";
    assert_eq!(check_with(&files, source), []);
}

#[test]
fn reports_undefined_class_constants_and_enum_misuse() {
    let files = [(
        "E.php",
        "<?php\nenum Suit { case Hearts; const Wild = self::Hearts; }\nenum Level: int { case Low = 1; }\nclass K { const A = 1; }\n",
    )];
    let source = "<?php\nK::A;\nK::B;\nSuit::Hearts;\nSuit::Spades;\nSuit::from('x');\nLevel::from(1);\nLevel::Low->value;\nSuit::Hearts->value;\nnew Suit();\n";
    assert_eq!(
        check_with(&files, source),
        [
            ("undefined-class-constant", "B".to_string()),
            ("undefined-class-constant", "Spades".to_string()),
            ("enum-misuse", "from".to_string()),
            ("enum-misuse", "value".to_string()),
            ("enum-misuse", "Suit".to_string()),
        ]
    );
}

#[test]
fn reports_instance_methods_called_statically() {
    let files = [(
        "A.php",
        "<?php\nclass A { public function inst(): void {} public static function stat(): void {} }\nclass B extends A { public function ok(): void { A::inst(); parent::inst(); self::inst(); } public static function no(): void { A::inst(); } }\n",
    )];
    let source = "<?php\nA::inst();\nA::stat();\nfunction f(A $a) { $a->stat(); $a->inst(); }\n";
    let found = check_with(&files, source);
    assert_eq!(
        found,
        [
            ("static-call-of-instance-method", "inst".to_string()),
            ("instance-call-of-static-method", "stat".to_string()),
        ]
    );
}

#[test]
fn readonly_properties_cannot_be_modified_or_initialized_from_outside() {
    let files = [(
        "R.php",
        "<?php\nfinal class R { public function __construct(public readonly int $a, public readonly array $list = []) {} public function set(): void { $this->a = 2; } }\n",
    )];
    let source = "<?php\nfunction f(R $r) {\n    $r->a = 1;\n    $r->list[] = 2;\n    $r->a++;\n    echo $r->a;\n}\n";
    assert_eq!(
        check_with(&files, source),
        [
            ("readonly-reassigned", "a".to_string()),
            ("readonly-reassigned", "list".to_string()),
            ("readonly-reassigned", "a".to_string()),
        ]
    );
}

#[test]
fn reports_variables_that_are_never_assigned() {
    let source = "<?php\nfunction f(int $a) {\n    echo $a, $missing;\n    $b = 1;\n    echo $b;\n    foreach ([1] as $k => $v) { echo $k, $v; }\n    preg_match('/x/', 'x', $m);\n    echo $m;\n    $fn = function () use ($b) { return $b + $nope; };\n    $arrow = fn($x) => $x + $a + $absent;\n    echo isset($maybe), $x2 ?? 1;\n}\n";
    assert_eq!(only("undefined-variable", source), ["$missing", "$nope", "$absent"]);
}

#[test]
fn variables_made_in_ways_a_scan_cannot_follow_are_not_reported() {
    none(
        "<?php\nfunction f() {\n    extract(['a' => 1]);\n    echo $a;\n}\nfunction g() {\n    $name = 'x';\n    $$name = 1;\n    echo $x;\n}\nfunction h() {\n    include 'a.php';\n    echo $fromInclude;\n}\nfunction i(callable $unknown) {\n    global $shared;\n    static $cache;\n    echo $shared, $cache;\n    $unknown($byRef);\n    echo $byRef;\n}\nfunction j() {\n    try {} catch (\\Exception $e) { echo $e; }\n    [$a, [$b]] = [1, [2]];\n    list('k' => $c) = ['k' => 3];\n    echo $a, $b, $c;\n}\n",
    );
}

#[test]
fn reports_variables_that_are_assigned_and_never_read() {
    let source = "<?php\nfunction f() {\n    $unused = 1;\n    $used = 2;\n    $kept = strlen('a');\n    $arr = [];\n    $arr[] = 1;\n    $counter = 0;\n    $counter++;\n    echo $used;\n    return compact('arr');\n}\n";
    assert_eq!(only("unused-variable", source), ["$unused", "$kept"]);
}

#[test]
fn does_not_call_variables_unused_that_closures_or_references_use() {
    none(
        "<?php\nfunction f() {\n    $a = 1;\n    $b = 2;\n    $c = [];\n    $fn = fn() => $a;\n    $g = function () use ($b, &$c) { $c[] = $b; };\n    $g();\n    $fn();\n    return $c;\n}\n",
    );
}

#[test]
fn reports_unused_parameters_of_private_and_final_methods() {
    let source = "<?php\nclass A {\n    private function p(int $used, int $spare) { return $used; }\n    final public function f($a, $b) { return $a; }\n    public function open($x) {}\n    private function empty($x) {}\n    private function viaCompact($x) { return compact('x'); }\n}\n";
    assert_eq!(only("unused-parameter", source), ["$spare", "$b"]);
}

#[test]
fn a_private_method_that_overrides_nothing_is_the_only_kind_flagged() {
    let files = [(
        "P.php",
        "<?php\nabstract class P { abstract protected function hook(int $x); }\n",
    )];
    let source = "<?php\nfinal class C extends P { final protected function hook(int $x) { return 1; } }\n";
    assert_eq!(check_with(&files, source), []);
}

#[test]
fn this_does_not_exist_in_static_code() {
    let source = "<?php\nclass A {\n    public static function s() { return $this->x; }\n    public function i() { return $this; }\n    public function c() { return static fn() => $this; }\n}\nfunction g() { return $this; }\n";
    assert_eq!(only("this-in-static-context", source), ["$this", "$this", "$this"]);
}

#[test]
fn reports_unused_imports_of_every_kind() {
    let source = "<?php\nnamespace App;\n\nuse Foo\\Used;\nuse Foo\\Unused;\nuse Foo\\Doc;\nuse Foo\\{A, B};\nuse function Foo\\helper;\nuse function Foo\\spare;\nuse const Foo\\LIMIT;\nuse Foo\\Aliased as Other;\n\n/** @param Doc $d */\nfunction f(Doc $d, A $a) {\n    new Used();\n    helper();\n    echo LIMIT;\n}\n";
    let found = only("unused-import", source);
    assert_eq!(
        found,
        [
            "use Foo\\Unused;",
            "B",
            "use function Foo\\spare;",
            "use Foo\\Aliased as Other;"
        ]
    );
}

#[test]
fn reports_private_members_nothing_uses() {
    let source = "<?php\nclass A {\n    private int $unused = 0;\n    private int $used = 0;\n    private const GONE = 1;\n    private const KEPT = 2;\n    private function dead() {}\n    private function alive() {}\n    private function viaString() {}\n    public function __construct(private int $quiet, private int $loud) {}\n    public function run() {\n        $this->alive();\n        echo $this->used, self::KEPT, $this->loud;\n        array_map([$this, 'viaString'], []);\n    }\n}\n";
    assert_eq!(
        check(source)
            .into_iter()
            .filter(|(code, _)| code.starts_with("unused-private"))
            .collect::<Vec<_>>(),
        [
            ("unused-private-property", "$unused".to_string()),
            ("unused-private-constant", "GONE".to_string()),
            ("unused-private-method", "dead".to_string()),
            ("unused-private-property", "$quiet".to_string()),
        ]
    );
}

#[test]
fn private_members_of_classes_with_traits_or_dynamic_access_are_left_alone() {
    none(
        "<?php\ntrait T {}\nclass A { use T; private function maybe() {} }\nclass B { private int $x = 1; public function get($name) { return $this->$name; } }\n",
    );
}

#[test]
fn counts_arguments_against_the_parameters() {
    let source = "<?php\nfunction f(int $a, int $b = 2, int ...$rest) {}\nfunction g(int $a, int $b) {}\nfunction h() { return func_get_args(); }\nf(1);\nf();\nf(1, 2, 3, 4);\ng(1);\ng(1, 2, 3);\ng(...[1, 2]);\ng(b: 2, a: 1);\ng(a: 1);\nh(1, 2, 3);\nstrlen();\nstrlen('a', 'b');\nsprintf('%s', 1, 2);\n";
    let found: Vec<_> = check(source)
        .into_iter()
        .filter(|(code, _)| *code == "wrong-argument-count")
        .map(|(_, text)| text)
        .collect();
    assert_eq!(found, ["f", "g", "3", "g", "strlen", "'b'"]);
}

#[test]
fn constructors_and_methods_are_counted_too() {
    let files = [(
        "A.php",
        "<?php\nclass A { public function __construct(public int $x) {} public function m(int $a): void {} public static function s(): void {} }\n",
    )];
    let source = "<?php\nnew A();\nnew A(1, 2);\n$a = new A(1);\n$a->m();\n$a->m(1);\nA::s(1);\n";
    let found: Vec<_> = check_with(&files, source)
        .into_iter()
        .filter(|(code, _)| *code == "wrong-argument-count")
        .collect();
    assert_eq!(found.len(), 4, "{found:?}");
}

#[test]
fn named_arguments_need_a_parameter_of_that_name() {
    let source = "<?php\nfunction f(int $a, int ...$more) {}\nfunction g(int $a, int $b) {}\nf(a: 1, other: 2);\ng(a: 1, c: 3);\ng(a: 1, b: 2);\n";
    assert_eq!(only("undefined-named-argument", source), ["c"]);
}

#[test]
fn arguments_that_can_never_fit_are_reported() {
    let files = [(
        "A.php",
        "<?php\nfinal class Box {}\nclass Plain {}\nclass Other {}\nclass Named { public function __toString(): string { return ''; } }\nfunction takesInt(int $x) {}\nfunction takesString(string $x) {}\nfunction takesBox(Box $b) {}\nfunction takesPlain(Plain $p) {}\nfunction takesNullable(?int $x) {}\nfunction takesFloat(float $f) {}\n",
    )];
    let source = "<?php\ntakesInt([]);\ntakesInt('5');\ntakesInt(1.5);\ntakesString(new Box());\ntakesString(new Named());\ntakesBox(1);\ntakesBox(new Plain());\ntakesPlain(new Other());\ntakesPlain(new Plain());\ntakesNullable(null);\ntakesInt(null);\ntakesFloat(3);\nstrlen(null);\nstrlen([]);\n";
    let found = only_in(&files, "argument-type-mismatch", source);
    assert_eq!(
        found,
        ["[]", "new Box()", "1", "new Plain()", "new Other()", "null", "[]"]
    );
}

fn only_in(files: &[(&str, &str)], code: &str, source: &str) -> Vec<String> {
    check_with(files, source)
        .into_iter()
        .filter(|(found, _)| *found == code)
        .map(|(_, text)| text)
        .collect()
}

#[test]
fn a_method_lent_by_a_mixin_is_not_checked_because_the_call_goes_through_call() {
    let files = [(
        "A.php",
        "<?php\nclass Helper { public function say(string $text, int $times = 1) {} }\n/** @mixin Helper */\nclass Facade { public function __call(string $name, array $arguments) {} }\n",
    )];
    let source =
        "<?php\ndeclare(strict_types=1);\n$facade = new Facade();\n$facade->say(5, 'x', 3);\n(new Helper())->say(5);\n";
    assert_eq!(only_in(&files, "argument-type-mismatch", source), ["5"]);
    assert!(only_in(&files, "wrong-argument-count", source).is_empty());
}

#[test]
fn strict_files_do_not_convert_scalars() {
    let files = [(
        "A.php",
        "<?php\nfunction takesString(string $x) {}\nfunction takesFloat(float $x) {}\n",
    )];
    let source = "<?php\ndeclare(strict_types=1);\ntakesString(5);\ntakesFloat(5);\n";
    assert_eq!(only_in(&files, "argument-type-mismatch", source), ["5"]);
}

#[test]
fn a_default_of_null_makes_a_parameter_nullable() {
    let files = [(
        "A.php",
        "<?php\nfunction takesArray(array $x = null) {}\nfunction strict(array $x) {}\n",
    )];
    let source = "<?php\ntakesArray(null);\nstrict(null);\n";
    assert_eq!(only_in(&files, "argument-type-mismatch", source), ["null"]);
}

#[test]
fn reports_code_after_a_statement_that_always_leaves() {
    let source = "<?php\nfunction a() {\n    return 1;\n    echo 'never';\n}\nfunction b($x) {\n    if ($x) { return 1; } else { throw new Exception(); }\n    echo 'never';\n}\nfunction c($x) {\n    if ($x) { return 1; }\n    echo 'fine';\n}\nfunction d($x) {\n    switch ($x) {\n        case 1:\n            return 1;\n            break;\n        default:\n            return 2;\n    }\n}\nfunction e() {\n    while (true) { }\n    echo 'never';\n}\nfunction f($items) {\n    foreach ($items as $item) { continue; echo 'never'; }\n}\nfunction g() {\n    return 1;\n    function hoisted() {}\n}\nfunction h() {\n    exit(1);\n    echo 'never';\n}\n";
    assert_eq!(
        only("unreachable-code", source),
        [
            "echo 'never';",
            "echo 'never';",
            "break;",
            "echo 'never';",
            "echo 'never';",
            "echo 'never';"
        ]
    );
}

#[test]
fn code_around_a_goto_is_left_alone() {
    none("<?php\nfunction a() {\n    goto end;\n    echo 1;\n    end:\n    echo 2;\n}\n");
}

#[test]
fn reports_functions_that_can_end_without_their_value() {
    let source = "<?php\nfunction a(int $x): int {\n    if ($x) { return 1; }\n}\nfunction b(int $x): int {\n    if ($x) { return 1; } else { return 2; }\n}\nfunction c(int $x): int {\n    switch ($x) { case 1: return 1; default: throw new Exception(); }\n}\nfunction d(int $x): int {\n    switch ($x) { case 1: return 1; }\n}\nfunction e(): int {\n    while (true) { if (rand()) { return 1; } }\n}\nfunction f(): ?int {\n    try { return 1; } catch (Exception $e) { }\n}\nfunction g(): void {\n    echo 1;\n}\nfunction h(): iterable {\n    yield 1;\n}\nfunction i(): int {\n    throw new Exception();\n}\nfunction j(): int {\n    try { return 1; } finally { echo 1; }\n}\nabstract class A { abstract public function m(): int; }\nfunction k(): never { exit(); }\nfunction l(): int {\n    for ($i = 0; ; ++$i) { if ($i > 3) { return $i; } }\n}\n";
    assert_eq!(only("missing-return", source), ["a", "d", "f"]);
}

#[test]
fn a_function_documented_as_never_is_no_missing_return() {
    let source = "<?php\n/** @return never */\nfunction abort($code) { throw new Exception(); }\nfunction a(int $x): int {\n    if ($x) { return 1; }\n    abort(500);\n}\nfunction b() {\n    abort(404);\n    echo 'maybe';\n}\n";
    assert_eq!(only("missing-return", source), Vec::<String>::new());
    assert_eq!(only("unreachable-code", source), Vec::<String>::new());
}

#[test]
fn a_never_function_ends_the_flow() {
    let source = "<?php\nfunction fail(): never { throw new Exception(); }\nfunction a(int $x): int {\n    if ($x) { return 1; }\n    fail();\n}\nfunction b() {\n    fail();\n    echo 'never';\n}\n";
    assert_eq!(only("missing-return", source), Vec::<String>::new());
    assert_eq!(only("unreachable-code", source), ["echo 'never';"]);
}

#[test]
fn reports_returns_the_declared_type_cannot_take() {
    let source = "<?php\nfinal class Box {}\nfunction a(): int { return []; }\nfunction b(): void { return 1; }\nfunction c(): int { return; }\nfunction d(): string { return new Box(); }\nfunction e(): ?int { return null; }\nfunction f(): int { return null; }\nfunction g(): float { return 1; }\nfunction h(): Box { return 1; }\nfunction i(): int { $x = 5; return fn() => 1; }\nfunction j(): iterable { yield 1; return 5; }\n";
    assert_eq!(
        only("return-type-mismatch", source),
        ["[]", "1", "return;", "new Box()", "null", "1", "fn() => 1"]
    );
}

#[test]
fn constants_assigned_in_conditions_are_reported() {
    let source = "<?php\nfunction f($a) {\n    if ($b = 5) {}\n    if (($c = 5)) {}\n    while ($row = next_row()) {}\n    if ($d = $a) {}\n    if (!$e = null) {}\n    echo $a ? $f = true : 0;\n}\n";
    assert_eq!(only("assignment-in-condition", source), ["$b = 5", "$e = null"]);
}

#[test]
fn strict_comparisons_of_kinds_that_never_match_are_reported() {
    let files = [("A.php", "<?php\nfinal class Box {}\n")];
    let source = "<?php\nfunction f(int $a, ?string $b, array $c, float $d, Box $e, int|string $g) {\n    $a === 'x';\n    $a === null;\n    $b === null;\n    $a === true;\n    $c !== 'x';\n    $d === 1;\n    $e === 1;\n    $g === 5;\n    $a == 'x';\n    $a === $a;\n    $a === 1;\n}\n";
    let found: Vec<_> = check_with(&files, source)
        .into_iter()
        .filter(|(code, _)| *code == "incompatible-comparison")
        .map(|(_, text)| text)
        .collect();
    assert_eq!(
        found,
        ["$a === 'x'", "$a === true", "$c !== 'x'", "$d === 1", "$e === 1"]
    );
}

#[test]
fn concrete_classes_must_implement_what_they_inherit_as_abstract() {
    let files = [(
        "A.php",
        "<?php\ninterface Shape { public function area(): float; public function name(): string; }\nabstract class Base implements Shape { abstract protected function hook(): void; public function name(): string { return ''; } }\ntrait Needs { abstract public function need(): int; }\n",
    )];
    let source = "<?php\nfinal class Square extends Base {}\nfinal class Circle extends Base { protected function hook(): void {} public function area(): float { return 1.0; } }\nabstract class Half extends Base {}\nclass UsesTrait { use Needs; }\nclass Counted implements Countable { public function count(): int { return 0; } }\nclass Plain implements Countable {}\nenum E: int implements Shape { case A = 1; public function area(): float { return 1.0; } public function name(): string { return ''; } }\n";
    let found: Vec<_> = check_with(&files, source)
        .into_iter()
        .filter(|(code, _)| code.ends_with("not-implemented"))
        .collect();
    assert_eq!(
        found,
        [
            ("abstract-method-not-implemented", "Square".to_string()),
            ("interface-method-not-implemented", "Square".to_string()),
            ("abstract-method-not-implemented", "UsesTrait".to_string()),
            ("interface-method-not-implemented", "Plain".to_string()),
        ]
    );
}

#[test]
fn an_incomplete_hierarchy_hides_missing_methods() {
    let files = [(
        "A.php",
        "<?php\nabstract class Base extends Gone { abstract public function m(): void; }\n",
    )];
    assert_eq!(check_with(&files, "<?php\nclass C extends Base {}\n"), []);
}

#[test]
fn overrides_must_be_able_to_stand_in_for_the_original() {
    let files = [(
        "A.php",
        "<?php\nclass Animal {}\nclass Dog extends Animal {}\nclass P {\n    public function run(int $a, string $b = ''): int { return 1; }\n    final public function locked(): void {}\n    public function stat(): void {}\n    public static function shared(): void {}\n    protected function guarded(): void {}\n    public function fetch(): Animal { return new Animal(); }\n    public function take(Dog $d): void {}\n    private function hidden(int $x): void {}\n    public function untyped($x) {}\n}\n",
    )];
    let source = "<?php\nclass C extends P {\n    public function run(int $a, string $b = '', $c): int { return 1; }\n    public function locked(): void {}\n    public function stat(): void {}\n    public function shared(): void {}\n    private function guarded(): void {}\n    public function fetch(): Dog { return new Dog(); }\n    public function take(Animal $d): void {}\n    public function hidden(string $x, int $more): void {}\n    public function untyped(int $x) {}\n}\nclass D extends P {\n    public function run(int $a): int { return 1; }\n    public function fetch(): int { return 1; }\n    public function take(string $d): void {}\n}\n";
    let found: Vec<_> = check_with(&files, source)
        .into_iter()
        .filter(|(code, _)| *code == "incompatible-override")
        .map(|(_, text)| text)
        .collect();
    assert_eq!(
        found,
        ["run", "locked", "shared", "guarded", "untyped", "run", "fetch", "take"]
    );
}

#[test]
fn strict_types_is_asked_for_only_when_switched_on() {
    let source = "<?php\nnamespace A;\n\nclass B {}\n";
    assert_eq!(only("missing-strict-types", source), Vec::<String>::new());
    let mut settings = InspectionSettings::default();
    settings.set(
        "missing-strict-types",
        Override {
            enabled: Some(true),
            severity: None,
        },
    );
    let found = run_with(&[], source, &settings);
    assert_eq!(
        found.iter().map(|found| found.text.as_str()).collect::<Vec<_>>(),
        ["<?php\n"]
    );
    let strict = "<?php\ndeclare(strict_types=1);\nclass B {}\n";
    assert!(run_with(&[], strict, &settings).is_empty());
}

#[test]
fn doc_comments_must_describe_parameters_the_function_has() {
    let source = "<?php\n/**\n * @param int $a\n * @param string $gone\n * @param $b\n */\nfunction f(int $a, $b) {}\n\n/** @param int $x */\nfunction g() {}\n\n/**\n * @param int $x\n */\nfunction h() { return func_get_args(); }\n";
    assert_eq!(only("phpdoc-unknown-parameter", source), ["$gone", "$x"]);
}

#[test]
fn doc_types_may_not_contradict_the_declared_ones() {
    let files = [(
        "A.php",
        "<?php\nclass Dog {}\nclass Cat {}\nclass Animal {}\nclass Puppy extends Dog {}\n",
    )];
    let source = "<?php\n/**\n * @param string $a\n * @param int $b\n * @param Cat $c\n * @param Puppy $d\n * @param int|string $e\n * @param positive-int $f\n * @param int $g\n * @param array<string> $h\n * @return string\n */\nfunction f(int $a, string $b, Dog $c, Dog $d, int $e, int $f, float $g, array $h): int {}\n";
    let found: Vec<_> = check_with(&files, source)
        .into_iter()
        .filter(|(code, _)| *code == "phpdoc-type-mismatch")
        .map(|(_, text)| text)
        .collect();
    assert_eq!(found, ["$a", "$b", "$c", "@return"]);
}

#[test]
fn a_method_reached_through_call_is_neither_static_nor_counted() {
    let files = [(
        "A.php",
        "<?php\n/**\n * @method static self subHour()\n */\nclass Date {\n    public function __call($name, $arguments) {}\n    public static function __callStatic($name, $arguments) {}\n    public static function make(): static {}\n}\n",
    )];
    let source = "<?php\nfunction f(Date $date) {\n    $date->subHour(1);\n    $date->make();\n}\n";
    assert_eq!(only_in(&files, "wrong-argument-count", source), Vec::<String>::new());
    assert_eq!(only_in(&files, "instance-call-of-static-method", source), ["make"]);
}

#[test]
fn self_and_static_in_an_anonymous_class_are_that_class() {
    let files = [("A.php", "<?php\nclass Migration {}\n")];
    let source = "<?php\nreturn new class extends Migration {\n    protected static array $map = [];\n    const LIMIT = 1;\n    public function up() {\n        return [static::$map, self::$map, self::LIMIT, static::helper(), static::$missing];\n    }\n    private static function helper() {}\n};\n";
    assert_eq!(only_in(&files, "undefined-property", source), Vec::<String>::new());
    assert_eq!(
        only_in(&files, "undefined-class-constant", source),
        Vec::<String>::new()
    );
    assert_eq!(only_in(&files, "undefined-method", source), Vec::<String>::new());
    let named = "<?php\nclass Seeder extends Migration {\n    public function up() {\n        return static::$missing;\n    }\n}\n";
    assert_eq!(only_in(&files, "undefined-property", named), ["$missing"]);
}

#[test]
fn a_partial_application_is_no_call_with_too_few_arguments() {
    let files = [(
        "f.php",
        "<?php\nfunction pad(string $text, int $width, string $with = ' '): string {}\n",
    )];
    let source = "<?php\n$left = pad(?, 10);\n$rest = pad(...);\n$one = pad(?);\n";
    assert_eq!(only_in(&files, "wrong-argument-count", source), Vec::<String>::new());
}
