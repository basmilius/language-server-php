use super::{applied, done, offered, refused};

fn class(members: &str) -> String {
    format!("<?php\nnamespace App;\n\nclass A\n{{\n{members}}}\n")
}

#[test]
fn a_constant_goes_after_the_last_one_and_replaces_the_expression() {
    let source = class(
        "    private const FIRST = 1;\n\n    public function run(): int\n    {\n        return strlen(«'hello world'») + 1;\n    }\n",
    );
    assert_eq!(
        applied(&[], &source, "Extract constant"),
        class(
            "    private const FIRST = 1;\n    private const HELLO_WORLD = 'hello world';\n\n    public function run(): int\n    {\n        return strlen(self::HELLO_WORLD) + 1;\n    }\n"
        )
    );
}

#[test]
fn the_first_constant_goes_above_the_properties_and_methods() {
    let source =
        class("    private int $x = 0;\n\n    public function run(): int\n    {\n        return «60 * 60»;\n    }\n");
    let found = applied(&[], &source, "Extract constant");
    assert!(
        found.contains("{\n    private const VALUE = 60 * 60;\n\n    private int $x = 0;"),
        "{found}"
    );
    assert!(found.contains("return self::VALUE;"), "{found}");
}

#[test]
fn replaces_every_equal_value_in_the_class() {
    let source = class(
        "    public function run(): array\n    {\n        return [«'a-b'», 'a-b', 'c'];\n    }\n\n    public function other(): string\n    {\n        return 'a-b';\n    }\n",
    );
    let titles = offered(&[], &source);
    assert!(
        titles.contains(&"Extract constant, replacing all 3 occurrences".to_string()),
        "{titles:?}"
    );
    let found = applied(&[], &source, "Extract constant, replacing all 3 occurrences");
    assert_eq!(found.matches("self::A_B").count(), 3, "{found}");
}

#[test]
fn only_constants_become_constants() {
    let source = class("    public function run(int $a): int\n    {\n        return «$a + 1»;\n    }\n");
    assert!(!offered(&[], &source).contains(&"Extract constant".to_string()));
    let source = class("    public function run(): int\n    {\n        return «strlen('a')»;\n    }\n");
    assert!(!offered(&[], &source).contains(&"Extract constant".to_string()));
    let source = class("    public function run(): int\n    {\n        return «static::X + 1»;\n    }\n");
    assert!(!offered(&[], &source).contains(&"Extract constant".to_string()));
}

#[test]
fn takes_the_next_name_when_one_is_taken() {
    let source = class(
        "    private const VALUE = 1;\n\n    public function run(): int\n    {\n        return «60 * 60»;\n    }\n",
    );
    let found = applied(&[], &source, "Extract constant");
    assert!(found.contains("const VALUE2 = 60 * 60;"), "{found}");
}

#[test]
fn a_field_can_start_as_a_constant() {
    let source = class("    public function run(): int\n    {\n        return strlen(«'hello'»);\n    }\n");
    let found = applied(&[], &source, "Extract field, initialized inline");
    assert!(found.contains("    private string $hello = 'hello';\n"), "{found}");
    assert!(found.contains("return strlen($this->hello);"), "{found}");
}

#[test]
fn a_field_can_be_set_in_a_new_constructor() {
    let source = class("    public function run(): int\n    {\n        return strlen(«new \\DateTime()»);\n    }\n");
    let found = applied(&[], &source, "Extract field, initialized in the constructor");
    assert!(found.contains("{\n    private DateTime $dateTime;\n\n    public function __construct()\n    {\n        $this->dateTime = new \\DateTime();\n    }\n\n    public function run()"), "{found}");
    assert!(found.contains("use DateTime;"), "{found}");
    assert!(found.contains("return strlen($this->dateTime);"), "{found}");
}

#[test]
fn a_field_is_added_to_the_end_of_the_constructor_it_has() {
    let source = class(
        "    private int $total = 0;\n\n    public function __construct(int $a)\n    {\n        $this->total = $a;\n    }\n\n    public function run(): int\n    {\n        return strlen(«'x' . PHP_EOL»);\n    }\n",
    );
    let found = applied(&[], &source, "Extract field, initialized in the constructor");
    assert!(
        found.contains("        $this->total = $a;\n        $this->string = 'x' . PHP_EOL;\n    }"),
        "{found}"
    );
    assert!(
        found.contains("private int $total = 0;\n    private string $string;"),
        "{found}"
    );
}

#[test]
fn a_static_method_gets_a_static_field() {
    let source = class("    public static function run(): int\n    {\n        return strlen(«'hello'»);\n    }\n");
    let found = applied(&[], &source, "Extract field, initialized inline");
    assert!(found.contains("private static string $hello = 'hello';"), "{found}");
    assert!(found.contains("strlen(self::$hello)"), "{found}");
    let titles = offered(&[], &source);
    assert!(!titles.contains(&"Extract field, initialized in the constructor".to_string()));
}

#[test]
fn a_field_cannot_use_what_the_method_has() {
    let source = class("    public function run(int $a): int\n    {\n        return strlen(«(string) $a»);\n    }\n");
    assert_eq!(
        refused(&[], &source, "Extract field, initialized in the constructor"),
        "The expression uses $a, which the constructor does not have"
    );
}

#[test]
fn a_parameter_is_added_everywhere_and_given_at_every_call() {
    let current = "<?php\nnamespace App;\n\nclass Impl extends Base\n{\n    public function r$0un(int $a): int\n    {\n        return $a * «60»;\n    }\n}\n";
    let base = "<?php\nnamespace App;\n\nabstract class Base\n{\n    abstract public function run(int $a): int;\n}\n";
    let user = "<?php\nnamespace App;\n\nclass User\n{\n    public function go(Impl $impl): void\n    {\n        $impl->run(1);\n        $impl->run(a: 2);\n    }\n}\n";
    let current = current.replace("r$0un", "run");
    let result = done(&[("Base.php", base), ("User.php", user)], &current, "Extract parameter");
    assert!(
        result.files["current.php"].contains("public function run(int $a, int $int): int"),
        "{}",
        result.files["current.php"]
    );
    assert!(result.files["current.php"].contains("return $a * $int;"));
    assert!(result.files["Base.php"].contains("abstract public function run(int $a, int $int): int;"));
    let user = &result.files["User.php"];
    assert!(user.contains("$impl->run(1, 60);"), "{user}");
    assert!(user.contains("$impl->run(a: 2, int: 60);"), "{user}");
}

#[test]
fn a_parameter_after_optional_ones_takes_the_expression_as_its_default() {
    let current = "<?php\nfunction f(int $a = 1)\n{\n    return $a * «60»;\n}\nf(2);\n";
    let found = applied(&[], current, "Extract parameter");
    assert!(found.contains("function f(int $a = 1, int $int = 60)"), "{found}");
    assert!(found.contains("f(2);"), "{found}");
}

#[test]
fn names_in_the_value_are_written_in_full_for_other_files() {
    let current =
        "<?php\nnamespace App;\n\nuse Lib\\Limits;\n\nfunction f(int $a)\n{\n    return $a * «Limits::MAX»;\n}\n";
    let lib = "<?php\nnamespace Lib;\n\nclass Limits\n{\n    public const MAX = 5;\n}\n";
    let user = "<?php\nnamespace Other;\n\nuse function App\\f;\n\nf(1);\n";
    let result = done(&[("Limits.php", lib), ("User.php", user)], current, "Extract parameter");
    assert!(
        result.files["User.php"].contains("f(1, \\Lib\\Limits::MAX);"),
        "{}",
        result.files["User.php"]
    );
    assert!(result.files["current.php"].contains("return $a * $max;"));
}

#[test]
fn a_parameter_cannot_take_what_only_the_function_has() {
    let current = "<?php\nfunction f(int $a)\n{\n    return «$a * 60»;\n}\n";
    assert!(!offered(&[], current).contains(&"Extract parameter".to_string()));
}
