use super::{applied, offered, refused};

const HEAD: &str = "<?php\nnamespace App;\n\nclass Repo\n{\n    public function name(): string {}\n    public int $count = 0;\n}\n\nclass A\n{\n    private Repo $repo;\n\n";

fn method(body: &str) -> String {
    format!("{HEAD}    public function run(int $a, int $b, array $items = []): int\n    {{\n{body}    }}\n}}\n")
}

fn inline(body: &str) -> String {
    applied(&[], &method(body), "Inline variable")
}

#[test]
fn replaces_every_read_with_the_value_and_drops_the_assignment() {
    assert_eq!(
        inline("        $sum$0 = $a + $b;\n        $x = $sum * 2;\n        return $sum - $x;\n"),
        method("        $x = ($a + $b) * 2;\n        return $a + $b - $x;\n")
    );
}

#[test]
fn works_from_a_read_as_well() {
    assert_eq!(
        inline("        $sum = $a + $b;\n        return $su$0m;\n"),
        method("        return $a + $b;\n")
    );
}

#[test]
fn leaves_off_parentheses_where_the_place_closes_the_value_in() {
    let found = inline("        $sum$0 = $a + $b;\n        foo($sum, [$sum]);\n        return $items[$sum];\n");
    assert!(
        found.contains("foo($a + $b, [$a + $b]);\n        return $items[$a + $b];"),
        "{found}"
    );
}

#[test]
fn wraps_a_value_that_is_the_base_of_a_member() {
    let found = inline("        $r$0 = new Repo();\n        return $r->count;\n");
    assert!(found.contains("return (new Repo())->count;"), "{found}");
}

#[test]
fn a_call_moves_to_the_one_read_that_follows_it() {
    assert_eq!(
        inline("        $name$0 = $this->repo->name();\n        $b = 2;\n        return strlen($name);\n"),
        method("        $b = 2;\n        return strlen($this->repo->name());\n")
    );
}

#[test]
fn writes_a_value_into_a_string() {
    let found = inline("        $n$0 = $this->repo->count;\n        echo \"hi {$n} and $n\";\n        return 1;\n");
    assert!(
        found.contains("echo \"hi {$this->repo->count} and {$this->repo->count}\";"),
        "{found}"
    );
}

#[test]
fn offers_nothing_for_what_is_assigned_twice_or_not_at_all() {
    let titles = offered(&[], &method("        $x$0 = 1;\n        $x = 2;\n        return $x;\n"));
    assert!(!titles.contains(&"Inline variable".to_string()));
    let titles = offered(&[], &method("        return $a$0;\n"));
    assert!(!titles.contains(&"Inline variable".to_string()));
    let titles = offered(&[], &method("        $x$0 = 1;\n        return 2;\n"));
    assert!(!titles.contains(&"Inline variable".to_string()));
    let titles = offered(&[], &method("        $x$0 = 1;\n        $x++;\n        return $x;\n"));
    assert!(!titles.contains(&"Inline variable".to_string()));
    let titles = offered(
        &[],
        &method("        $x$0 = [];\n        $x[] = 1;\n        return count($x);\n"),
    );
    assert!(!titles.contains(&"Inline variable".to_string()));
}

#[test]
fn refuses_to_copy_a_value_that_does_something() {
    assert_eq!(
        refused(
            &[],
            &method("        $name$0 = $this->repo->name();\n        return strlen($name) + strlen($name);\n"),
            "Inline variable"
        ),
        "The value does something, so it cannot be copied to every place"
    );
}

#[test]
fn refuses_to_move_a_call_past_other_effects() {
    let reason = refused(
        &[],
        &method(
            "        $name$0 = $this->repo->name();\n        $this->repo->count = 3;\n        return strlen($name);\n",
        ),
        "Inline variable",
    );
    assert_eq!(
        reason,
        "What the value reads or does may change before the variable is read"
    );
    let reason = refused(
        &[],
        &method(
            "        $name$0 = $this->repo->name();\n        if ($a) {\n            return strlen($name);\n        }\n        return 1;\n",
        ),
        "Inline variable",
    );
    assert_eq!(reason, "The value only runs sometimes where the variable is read");
    let reason = refused(
        &[],
        &method(
            "        $name$0 = $this->repo->name();\n        if ($a) {\n            return 3;\n        }\n        return strlen($name);\n",
        ),
        "Inline variable",
    );
    assert_eq!(reason, "Code between the assignment and the read may leave first");
}

#[test]
fn refuses_a_value_whose_inputs_change_before_the_read() {
    let reason = refused(
        &[],
        &method("        $sum$0 = $a + $b;\n        $a = 5;\n        return $sum;\n"),
        "Inline variable",
    );
    assert_eq!(
        reason,
        "What the value reads or does may change before the variable is read"
    );
}

#[test]
fn refuses_a_read_in_a_loop_whose_body_changes_the_inputs() {
    let reason = refused(
        &[],
        &method(
            "        $sum$0 = $a + $b;\n        while ($b < 10) {\n            $x = $sum;\n            $b++;\n        }\n        return 1;\n",
        ),
        "Inline variable",
    );
    assert_eq!(
        reason,
        "What the value reads or does may change before the variable is read"
    );
}

#[test]
fn a_value_that_cannot_change_may_move_across_calls() {
    let found = inline("        $x$0 = 5;\n        foo($this->repo->name());\n        return $x + 1;\n");
    assert!(found.contains("return 5 + 1;"), "{found}");
}
