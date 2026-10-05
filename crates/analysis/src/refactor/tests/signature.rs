use super::{applied, done, offered, refused};

const BASE: &str = "<?php\nnamespace App;\n\nabstract class Base\n{\n    abstract public function run(int $a, string $b = 'x'): int;\n}\n";
const USER: &str = "<?php\nnamespace App;\n\nclass User\n{\n    public function go(Impl $impl, Base $base): void\n    {\n        $impl->run(1);\n        $impl->run(1, 'y');\n        $impl->run(b: 'z', a: 2);\n        $base->run(3, b: 'w');\n    }\n}\n";

#[test]
fn adds_a_parameter_with_a_default_everywhere_it_is_declared() {
    let current = "<?php\nnamespace App;\n\nclass Impl extends Base\n{\n    public function r$0un(int $a, string $b = 'x'): int\n    {\n        return $a;\n    }\n}\n";
    let result = done(&[("Base.php", BASE), ("User.php", USER)], current, "Add parameter");
    assert_eq!(
        result.files["current.php"],
        "<?php\nnamespace App;\n\nclass Impl extends Base\n{\n    public function run(int $a, string $b = 'x', $parameter = null): int\n    {\n        return $a;\n    }\n}\n"
    );
    assert!(
        result.files["Base.php"]
            .contains("abstract public function run(int $a, string $b = 'x', $parameter = null): int;")
    );
    assert!(!result.files.contains_key("User.php"), "no call needs to change");
    assert!(result.change.focus.is_some());
}

#[test]
fn takes_the_next_name_when_one_is_taken() {
    let current = "<?php\nfunction f(int $parameter) {}\nf$0(1);\n";
    let source = "<?php\nfunction f$0(int $parameter) { $parameter2 = 1; }\n";
    let _ = current;
    let found = applied(&[], source, "Add parameter");
    assert!(
        found.contains("function f(int $parameter, $parameter3 = null)"),
        "{found}"
    );
}

#[test]
fn refuses_to_add_before_a_variadic_parameter() {
    let source = "<?php\nfunction f$0(int $a, ...$rest) {}\n";
    assert_eq!(
        refused(&[], source, "Add parameter"),
        "A parameter cannot be added before a variadic one"
    );
}

#[test]
fn removes_an_unused_parameter_from_every_declaration_and_call() {
    let current = "<?php\nnamespace App;\n\nclass Impl extends Base\n{\n    public function run(int $a, string $b$0 = 'x'): int\n    {\n        return $a;\n    }\n}\n";
    let result = done(
        &[("Base.php", BASE), ("User.php", USER)],
        current,
        "Remove unused parameter '$b'",
    );
    assert!(result.files["current.php"].contains("public function run(int $a): int"));
    assert!(result.files["Base.php"].contains("abstract public function run(int $a): int;"));
    let user = &result.files["User.php"];
    assert!(
        user.contains("$impl->run(1);\n        $impl->run(1);\n        $impl->run(a: 2);\n        $base->run(3);"),
        "{user}"
    );
}

#[test]
fn does_not_offer_to_remove_a_parameter_that_is_used() {
    let source = "<?php\nfunction f(int $a, int $b$0) { return $b; }\n";
    let titles = offered(&[], source);
    assert!(
        !titles.iter().any(|title| title.starts_with("Remove unused")),
        "{titles:?}"
    );
}

#[test]
fn refuses_to_remove_a_parameter_an_override_uses() {
    let override_file = "<?php\nnamespace App;\n\nclass Other extends Base\n{\n    public function run(int $a, string $b = 'x'): int\n    {\n        return strlen($b);\n    }\n}\n";
    let current = "<?php\nnamespace App;\n\nclass Impl extends Base\n{\n    public function run(int $a, string $b$0 = 'x'): int\n    {\n        return $a;\n    }\n}\n";
    let reason = refused(
        &[("Base.php", BASE), ("Other.php", override_file)],
        current,
        "Remove unused parameter '$b'",
    );
    assert!(
        reason.starts_with("An override still uses the parameter in"),
        "{reason}"
    );
}

#[test]
fn refuses_to_remove_a_parameter_when_a_call_does_something_with_it() {
    let current = "<?php\nfunction f(int $a, int $b$0) { return $a; }\nf(1, g());\nfunction g(): int { return 1; }\n";
    let reason = refused(&[], current, "Remove unused parameter '$b'");
    assert!(reason.contains("passes something that does something"), "{reason}");
}

#[test]
fn removes_the_doc_line_with_the_parameter() {
    let source =
        "<?php\n/**\n * @param int $a\n * @param int $b\n */\nfunction f(int $a, int $b$0)\n{\n    return $a;\n}\n";
    let found = applied(&[], source, "Remove unused parameter '$b'");
    assert_eq!(
        found,
        "<?php\n/**\n * @param int $a\n */\nfunction f(int $a)\n{\n    return $a;\n}\n"
    );
}

#[test]
fn moves_a_parameter_and_its_arguments() {
    let current = "<?php\nnamespace App;\n\nclass Impl extends Base\n{\n    public function run(int $a$0, string $b = 'x'): int\n    {\n        return $a;\n    }\n}\n";
    let user = "<?php\nnamespace App;\n\nclass User\n{\n    public function go(Impl $impl): void\n    {\n        $impl->run(1);\n        $impl->run(1, 'y');\n        $impl->run(b: 'z', a: 2);\n    }\n}\n";
    let base = "<?php\nnamespace App;\n\nabstract class Base\n{\n    abstract public function run(int $a, string $b): int;\n}\n";
    let current = current.replace("string $b = 'x'", "string $b");
    let result = done(
        &[("Base.php", base), ("User.php", user)],
        &current,
        "Move parameter '$a' right",
    );
    assert!(result.files["current.php"].contains("public function run(string $b, int $a): int"));
    assert!(result.files["Base.php"].contains("abstract public function run(string $b, int $a): int;"));
    let user = &result.files["User.php"];
    assert!(user.contains("$impl->run('y', 1);"), "{user}");
    assert!(user.contains("$impl->run(b: 'z', a: 2);"), "{user}");
}

#[test]
fn a_call_that_leaves_out_an_argument_gets_a_name_or_a_default() {
    let current = "<?php\nfunction f(int $a$0 = 1, int $b = 2) { return $a + $b; }\nf(5);\n";
    let found = applied(&[], current, "Move parameter '$a' right");
    assert!(found.contains("function f(int $b = 2, int $a = 1)"), "{found}");
    assert!(found.contains("f(2, 5);"), "{found}");
}

#[test]
fn refuses_an_order_that_puts_a_required_parameter_after_an_optional_one() {
    let source = "<?php\nfunction f(int $a$0, int $b = 2) {}\n";
    assert_eq!(
        refused(&[], source, "Move parameter '$a' right"),
        "A parameter without a default cannot come after one that has"
    );
}

#[test]
fn refuses_a_call_that_spreads_its_arguments() {
    let source = "<?php\nfunction f(int $a$0, int $b) {}\nf(...[1, 2]);\n";
    assert!(refused(&[], source, "Move parameter '$a' right").contains("spreads its arguments"));
}
