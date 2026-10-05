use super::{applied, done, offered, refused};

fn class(members: &str) -> String {
    format!("<?php\nnamespace App;\n\nclass A\n{{\n{members}}}\n")
}

#[test]
fn a_one_line_method_goes_to_every_call_and_is_removed() {
    let source = class(
        "    private function double(int $x): int\n    {\n        return $x * 2;\n    }\n\n    public function run(int $a, int $b): int\n    {\n        return $this->dou$0ble($a + 1) + $this->double($b);\n    }\n",
    );
    assert_eq!(
        applied(&[], &source, "Inline method"),
        class("    public function run(int $a, int $b): int\n    {\n        return ($a + 1) * 2 + $b * 2;\n    }\n")
    );
}

#[test]
fn works_from_the_declaration_too() {
    let source = class(
        "    private function dou$0ble(int $x): int\n    {\n        return $x * 2;\n    }\n\n    public function run(int $a): int\n    {\n        return $this->double($a);\n    }\n",
    );
    assert_eq!(
        applied(&[], &source, "Inline method"),
        class("    public function run(int $a): int\n    {\n        return $a * 2;\n    }\n")
    );
}

#[test]
fn one_call_can_be_inlined_and_the_method_stays() {
    let source = class(
        "    private function double(int $x): int\n    {\n        return $x * 2;\n    }\n\n    public function run(int $a, int $b): int\n    {\n        return $this->dou$0ble($a) + $this->double($b);\n    }\n",
    );
    let found = applied(&[], &source, "Inline this call");
    assert!(found.contains("return $a * 2 + $this->double($b);"), "{found}");
    assert!(found.contains("private function double(int $x): int"), "{found}");
}

#[test]
fn statements_replace_a_call_that_is_a_statement() {
    let source = class(
        "    private int $total = 0;\n\n    private function add(int $n): void\n    {\n        $copy = $n * 2;\n        $this->total += $copy;\n    }\n\n    public function run(int $a): void\n    {\n        $copy = 1;\n        $this->ad$0d($a + 1);\n        echo $copy;\n    }\n",
    );
    let found = applied(&[], &source, "Inline method");
    assert!(
        found.contains("        $copy = 1;\n        $n = $a + 1;\n        $copy2 = $n * 2;\n        $this->total += $copy2;\n        echo $copy;"),
        "{found}"
    );
    assert!(!found.contains("private function add"), "{found}");
}

#[test]
fn a_trivial_argument_is_used_as_it_is() {
    let source = class(
        "    private int $total = 0;\n\n    private function add(int $n): void\n    {\n        $this->total += $n;\n    }\n\n    public function run(int $a): void\n    {\n        $this->ad$0d($a);\n    }\n",
    );
    let found = applied(&[], &source, "Inline method");
    assert!(found.contains("        $this->total += $a;\n    }"), "{found}");
}

#[test]
fn a_final_return_goes_to_the_assignment_that_took_it() {
    let source = class(
        "    private function make(int $n): int\n    {\n        $x = $n + 1;\n        return $x * 2;\n    }\n\n    public function run(int $a): int\n    {\n        $r = $this->ma$0ke($a);\n        return $r;\n    }\n",
    );
    let found = applied(&[], &source, "Inline method");
    assert!(
        found.contains("        $x = $a + 1;\n        $r = $x * 2;\n        return $r;"),
        "{found}"
    );
}

#[test]
fn a_function_goes_to_another_file_with_its_names_in_full() {
    let helper = "<?php\nnamespace Lib;\n\nuse DateTime;\n\nfunction stamp(string $s): string\n{\n    return (new DateTime($s))->format('Y');\n}\n";
    let current =
        "<?php\nnamespace App;\n\nuse function Lib\\stamp;\n\nfunction f(): string\n{\n    return sta$0mp('now');\n}\n";
    let result = done(&[("helper.php", helper)], current, "Inline method");
    assert!(
        result.files["current.php"].contains("return (new \\DateTime('now'))->format('Y');"),
        "{}",
        result.files["current.php"]
    );
}

#[test]
fn refuses_what_cannot_be_copied() {
    let recursive = class(
        "    private function fact(int $n): int\n    {\n        return $n <= 1 ? 1 : $n * $this->fact($n - 1);\n    }\n\n    public function run(): int\n    {\n        return $this->fa$0ct(3);\n    }\n",
    );
    assert!(!offered(&[], &recursive).contains(&"Inline method".to_string()));
    let early = class(
        "    private function pick(int $n): int\n    {\n        if ($n > 1) {\n            return 1;\n        }\n        return 2;\n    }\n\n    public function run(): int\n    {\n        return $this->pi$0ck(3);\n    }\n",
    );
    assert!(!offered(&[], &early).contains(&"Inline method".to_string()));
    let generator = class(
        "    private function gen(): \\Generator\n    {\n        yield 1;\n    }\n\n    public function run(): void\n    {\n        foreach ($this->ge$0n() as $x) {}\n    }\n",
    );
    assert!(!offered(&[], &generator).contains(&"Inline method".to_string()));
}

#[test]
fn refuses_a_method_another_class_overrides() {
    let child = "<?php\nnamespace App;\n\nclass B extends A\n{\n    public function double(int $x): int\n    {\n        return $x * 3;\n    }\n}\n";
    let current = class(
        "    public function dou$0ble(int $x): int\n    {\n        return $x * 2;\n    }\n\n    public function run(int $a): int\n    {\n        return $this->double($a);\n    }\n",
    );
    assert_eq!(
        refused(&[("B.php", child)], &current, "Inline method"),
        "The method is overridden or implemented elsewhere"
    );
    let on_call = class(
        "    public function double(int $x): int\n    {\n        return $x * 2;\n    }\n\n    public function run(int $a): int\n    {\n        return $this->dou$0ble($a);\n    }\n",
    );
    assert!(!offered(&[("B.php", child)], &on_call).contains(&"Inline method".to_string()));
}

#[test]
fn refuses_to_copy_effects_into_a_place_that_runs_them_in_another_order() {
    let source = class(
        "    private function twice(int $x): int\n    {\n        return $x + $x;\n    }\n\n    public function run(): int\n    {\n        return $this->tw$0ice(count([1]));\n    }\n",
    );
    let reason = refused(&[], &source, "Inline method");
    assert!(reason.contains("does something"), "{reason}");
}

#[test]
fn refuses_a_body_that_uses_its_object_where_it_is_called_from_outside() {
    let user = "<?php\nnamespace App;\n\nclass User\n{\n    public function go(A $a): int\n    {\n        return $a->get();\n    }\n}\n";
    let current =
        class("    private int $v = 1;\n\n    public function ge$0t(): int\n    {\n        return $this->v;\n    }\n");
    let reason = refused(&[("User.php", user)], &current, "Inline method");
    assert!(reason.starts_with("The body uses its object"), "{reason}");
}
