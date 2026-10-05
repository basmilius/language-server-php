use super::{applied, offered, refused};

const HEAD: &str = "<?php\nnamespace App;\n\nclass Repo\n{\n    public function name(): string {}\n}\n\nclass A\n{\n    private Repo $repo;\n    private int $total = 0;\n\n";

fn class(members: &str) -> String {
    format!("{HEAD}{members}}}\n")
}

fn extract(members: &str) -> String {
    applied(&[], &class(members), "Extract method")
}

#[test]
fn an_expression_becomes_a_method_that_returns_it() {
    assert_eq!(
        extract("    public function run(int $a, int $b): int\n    {\n        return strlen(«$a . $b»);\n    }\n"),
        class(
            "    public function run(int $a, int $b): int\n    {\n        return strlen($this->getString($a, $b));\n    }\n\n    private function getString(int $a, int $b): string\n    {\n        return $a . $b;\n    }\n"
        )
    );
}

#[test]
fn what_is_read_afterwards_is_returned() {
    assert_eq!(
        extract(
            "    public function run(int $a, int $b): int\n    {\n        «$sum = $a + $b;\n        $sum *= 2;»\n        return $sum;\n    }\n"
        ),
        class(
            "    public function run(int $a, int $b): int\n    {\n        $sum = $this->getSum($a, $b);\n        return $sum;\n    }\n\n    private function getSum(int $a, int $b): int\n    {\n        $sum = $a + $b;\n        $sum *= 2;\n        return $sum;\n    }\n"
        )
    );
}

#[test]
fn a_variable_that_is_set_and_read_again_is_passed_in_and_back() {
    let found = extract(
        "    public function run(int $a, int $b): int\n    {\n        «$a = $a + $b;»\n        return $a;\n    }\n",
    );
    assert!(found.contains("$a = $this->getA($a, $b);"), "{found}");
    assert!(
        found.contains(
            "private function getA(int $a, int $b): int\n    {\n        $a = $a + $b;\n        return $a;\n    }"
        ),
        "{found}"
    );
}

#[test]
fn several_results_come_back_as_a_list() {
    let found = extract(
        "    public function run(array $items): int\n    {\n        «$min = min($items);\n        $max = max($items);»\n        return $max - $min;\n    }\n",
    );
    assert!(found.contains("[$min, $max] = $this->extracted($items);"), "{found}");
    assert!(
        found.contains("        $max = max($items);\n        return [$min, $max];\n    }"),
        "{found}"
    );
    assert!(
        found.contains("private function extracted(array $items): array"),
        "{found}"
    );
}

#[test]
fn code_that_hands_nothing_back_is_void() {
    let found = extract(
        "    public function run(int $a): void\n    {\n        «$this->total += $a;\n        echo $a;»\n    }\n",
    );
    assert!(found.contains("        $this->extracted($a);\n    }"), "{found}");
    assert!(
        found.contains(
            "private function extracted(int $a): void\n    {\n        $this->total += $a;\n        echo $a;\n    }"
        ),
        "{found}"
    );
}

#[test]
fn a_run_that_always_returns_is_returned_from() {
    let found = extract(
        "    public function run(int $a): int\n    {\n        if ($a > 1) {\n            return 1;\n        }\n        «if ($a > 0) {\n            return 2;\n        }\n        return 3;»\n    }\n",
    );
    assert!(found.contains("        return $this->extracted($a);\n    }"), "{found}");
    assert!(found.contains("private function extracted(int $a): int\n    {\n        if ($a > 0) {\n            return 2;\n        }\n        return 3;\n    }"), "{found}");
}

#[test]
fn a_bare_return_stays_a_return() {
    let found = extract(
        "    public function run(int $a): void\n    {\n        «if ($a > 0) {\n            echo $a;\n        }\n        return;»\n    }\n",
    );
    assert!(
        found.contains("        $this->extracted($a);\n        return;\n    }"),
        "{found}"
    );
    assert!(found.contains("private function extracted(int $a): void"), "{found}");
}

#[test]
fn a_static_method_gets_a_static_method() {
    let found = extract("    public static function run(int $a): int\n    {\n        return «$a * 2» + 1;\n    }\n");
    assert!(found.contains("return self::getInt($a) + 1;"), "{found}");
    assert!(found.contains("private static function getInt(int $a)"), "{found}");
}

#[test]
fn a_function_gets_a_function() {
    let source = "<?php\nfunction run(int $a): int\n{\n    return strlen(«(string) $a»);\n}\n";
    let found = applied(&[], source, "Extract method");
    assert!(found.contains("return strlen(getString($a));"), "{found}");
    assert!(
        found.contains("function getString(int $a): string\n{\n    return (string) $a;\n}"),
        "{found}"
    );
}

#[test]
fn a_generator_part_is_delegated_to() {
    let found = applied(
        &[("Generator.php", "<?php\nfinal class Generator {}\n")],
        &class(
            "    public function run(int $a): \\Generator\n    {\n        «yield $a;\n        yield $a + 1;»\n        yield 3;\n    }\n",
        ),
        "Extract method",
    );
    assert!(
        found.contains("        yield from $this->extracted($a);\n        yield 3;"),
        "{found}"
    );
    assert!(
        found.contains("private function extracted(int $a): \\Generator"),
        "{found}"
    );
}

#[test]
fn a_parameter_taken_by_reference_stays_one() {
    let found = extract("    public function run(array &$items): void\n    {\n        «$items[] = 1;»\n    }\n");
    assert!(found.contains("$this->extracted($items);"), "{found}");
    assert!(
        found.contains("private function extracted(array &$items): void"),
        "{found}"
    );
}

#[test]
fn loops_inside_the_selection_may_break() {
    let found = extract(
        "    public function run(array $items): int\n    {\n        $sum = 0;\n        «foreach ($items as $item) {\n            if ($item < 0) {\n                break;\n            }\n            $sum += $item;\n        }»\n        return $sum;\n    }\n",
    );
    assert!(found.contains("$sum = $this->getSum($items, $sum);"), "{found}");
    assert!(
        found.contains("private function getSum(array $items, int $sum): int"),
        "{found}"
    );
}

#[test]
fn keeps_the_comments_and_the_strings_inside() {
    let found = extract(
        "    public function run(int $a): string\n    {\n        «// say it\n        $text = <<<EOT\n  hello $a\nEOT;\n        $text .= 'x'; // done»\n        return $text;\n    }\n",
    );
    assert!(found.contains("        // say it\n        $text = <<<EOT\n  hello $a\nEOT;\n        $text .= 'x'; // done\n        return $text;"), "{found}");
}

#[test]
fn writes_a_doc_block_where_the_class_writes_them() {
    let found = extract(
        "    /**\n     * @param int[] $items\n     */\n    public function run(array $items): int\n    {\n        return count(«array_filter($items)»);\n    }\n",
    );
    assert!(
        found.contains(
            "    /**\n     * @param list<int> $items\n     */\n    private function getArrayFilter(array $items)"
        ),
        "{found}"
    );
}

#[test]
fn refuses_what_cannot_be_cut_out() {
    let cases = [
        (
            "    public function run(int $a): int\n    {\n        foreach ([1] as $x) {\n            «if ($a) {\n                break;\n            }»\n        }\n        return 1;\n    }\n",
            "The selection leaves a loop that it does not contain",
        ),
        (
            "    public function run(int $a): int\n    {\n        «if ($a) {\n            return 1;\n        }»\n        return 2;\n    }\n",
            "The selection returns on some paths only",
        ),
        (
            "    public function run(int $a): int\n    {\n        «static $n = 0;»\n        return 2;\n    }\n",
            "The selection binds variables that outlive a call",
        ),
        (
            "    public function run(int $a): int\n    {\n        «$b = &$a;»\n        return $b;\n    }\n",
            "The selection binds a reference",
        ),
    ];
    for (members, reason) in cases {
        assert_eq!(refused(&[], &class(members), "Extract method"), reason, "{members}");
    }
}

#[test]
fn is_not_offered_without_a_selection_or_for_half_a_statement() {
    let titles = offered(
        &[],
        &class("    public function run(int $a): int\n    {\n        return $a$0 + 1;\n    }\n"),
    );
    assert!(!titles.contains(&"Extract method".to_string()));
    let titles = offered(
        &[],
        &class("    public function run(int $a): int\n    {\n        «return $a» + 1;\n    }\n"),
    );
    assert!(!titles.contains(&"Extract method".to_string()), "{titles:?}");
}

#[test]
fn takes_the_next_free_name() {
    let found = extract(
        "    public function extracted() {}\n    public function run(int $a): void\n    {\n        «echo $a;»\n    }\n",
    );
    assert!(found.contains("$this->extracted2($a);"), "{found}");
}

/// The write is dead in the function too, but its parameter hid that from the inspection; as a
/// variable of the new method it is reported, which is the one new finding that is accepted.
#[test]
fn a_dead_write_to_a_parameter_goes_along() {
    let source = class(
        "    public function run(int $a, int $n): int\n    {\n        echo $n;\n        «$n = $a + 1;»\n        return $a;\n    }\n",
    );
    let outcome = super::run_with(
        super::Setup::PLAIN,
        &[],
        &source,
        "Extract method",
        &php_format::FormatOptions::default(),
    );
    let found = outcome.result.expect("offered").expect("done").text;
    assert!(found.contains("$this->extracted($a);"), "{found}");
    assert!(
        found.contains("private function extracted(int $a): void\n    {\n        $n = $a + 1;\n    }"),
        "{found}"
    );
}
