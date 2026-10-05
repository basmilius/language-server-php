use super::{applied, offered, refused};

const HEAD: &str = "<?php\nnamespace App;\n\nclass Repo\n{\n    public function findUser(int $id): ?Repo {}\n    public function name(): string {}\n}\n\nclass A\n{\n    private Repo $repo;\n\n";

fn method(body: &str) -> String {
    format!("{HEAD}    public function run(int $a, int $b, array $items = []): int\n    {{\n{body}    }}\n}}\n")
}

fn extract(body: &str) -> String {
    applied(&[], &method(body), "Extract variable")
}

#[test]
fn extracts_the_selected_expression_before_its_statement() {
    assert_eq!(
        extract("        return («$a + $b») * 2;\n"),
        method("        $int = $a + $b;\n        return ($int) * 2;\n")
    );
}

#[test]
fn the_cursor_picks_the_outermost_expression_that_is_worth_a_name() {
    assert_eq!(
        extract("        $x = strlen($a$0 + $b) * 2;\n        return $x;\n"),
        method("        $strlen = strlen($a + $b);\n        $x = $strlen * 2;\n        return $x;\n")
    );
}

#[test]
fn the_names_come_from_the_expression() {
    let found = extract("        $name = $this->repo->find$0User($a)->name();\n        return 1;\n");
    assert!(
        found.contains("$user = $this->repo->findUser($a);\n        $name = $user->name();"),
        "{found}"
    );
    let found = extract("        return count(«$this->repo->findUser($a)->name()»);\n");
    assert!(found.contains("$name = $this->repo->findUser($a)->name();"), "{found}");
    let found = extract("        foo(«new \\DateTime()»);\n        return 1;\n");
    assert!(found.contains("$dateTime = new \\DateTime();"), "{found}");
    let found = extract("        foo(«$items['first-name']»);\n        return 1;\n");
    assert!(found.contains("$firstName = $items['first-name'];"), "{found}");
    let found = extract("        foo(«$a . 'x'»);\n        return 1;\n");
    assert!(found.contains("$string = $a . 'x';"), "{found}");
}

#[test]
fn offers_the_inner_expressions_as_well() {
    let titles = offered(&[], &method("        return strlen($this->repo->name$0()) + 1;\n"));
    assert!(titles.contains(&"Extract variable".to_string()), "{titles:?}");
    assert!(
        titles.iter().any(|title| title.starts_with("Extract variable from '")),
        "{titles:?}"
    );
}

#[test]
fn replaces_every_equal_expression_in_reach() {
    let source = method("        $x = «$a * $b» + 1;\n        $y = $a * $b + 2;\n        return $x + $y;\n");
    let titles = offered(&[], &source);
    assert!(
        titles.contains(&"Extract variable, replacing all 2 occurrences".to_string()),
        "{titles:?}"
    );
    assert_eq!(
        applied(&[], &source, "Extract variable, replacing all 2 occurrences"),
        method("        $int = $a * $b;\n        $x = $int + 1;\n        $y = $int + 2;\n        return $x + $y;\n")
    );
}

#[test]
fn an_occurrence_after_the_inputs_changed_is_left_alone() {
    let source =
        method("        $x = «$a * $b» + 1;\n        $a = 5;\n        $y = $a * $b + 2;\n        return $x + $y;\n");
    assert!(
        !offered(&[], &source)
            .iter()
            .any(|title| title.contains("replacing all"))
    );
}

#[test]
fn an_expression_with_effects_is_not_run_once_for_two_places() {
    let source = method(
        "        $x = «$this->repo->findUser($a)» ?? null;\n        $y = $this->repo->findUser($a);\n        return 1;\n",
    );
    assert!(
        !offered(&[], &source)
            .iter()
            .any(|title| title.contains("replacing all"))
    );
}

#[test]
fn goes_in_front_of_the_condition_it_was_taken_from() {
    assert_eq!(
        extract("        if («$a + $b» > 3) {\n            return 1;\n        }\n        return 2;\n"),
        method(
            "        $int = $a + $b;\n        if ($int > 3) {\n            return 1;\n        }\n        return 2;\n"
        )
    );
}

#[test]
fn works_inside_a_closure_body() {
    let found =
        extract("        $f = function () use ($a) {\n            return «$a * 3»;\n        };\n        return 1;\n");
    assert!(
        found.contains("            $int = $a * 3;\n            return $int;"),
        "{found}"
    );
}

#[test]
fn refuses_what_only_runs_sometimes() {
    let cases = [
        (
            "        return $a && «strlen($this->repo->name()) > 1»;\n",
            "The expression only runs when the left side allows it",
        ),
        (
            "        return $a ? «$a + 1» : 0;\n",
            "The expression only runs for one outcome of the condition",
        ),
        (
            "        return $a ?: «$b + 1»;\n",
            "The expression only runs for one outcome of the condition",
        ),
        (
            "        return match ($a) { 1 => «$b + 1», default => 0 };\n",
            "The expression only runs for one arm of the match",
        ),
        (
            "        while («$a + 1» < 3) {\n            $a++;\n        }\n        return 1;\n",
            "A loop condition runs on every pass",
        ),
        (
            "        $f = fn ($z) => «$z + 1»;\n        return 1;\n",
            "An arrow function has no statement to put a variable in",
        ),
        (
            "        return isset($items[«$a + 1»]) ? 1 : 0;\n",
            "An expression inside isset, empty or unset cannot be moved",
        ),
        (
            "        return @«$items[$a]» ?? 1;\n",
            "The expression is silenced with @",
        ),
        (
            "        if ($a) {\n            return 1;\n        } elseif («$a + $b» > 3) {\n            return 2;\n        }\n        return 3;\n",
            "An elseif condition only runs when the ones above it were false",
        ),
        (
            "        if ($a) return «$a + 1»;\n        return 3;\n",
            "Put braces around the body of the statement first",
        ),
        (
            "        return $items[«$a»] ?? 1;\n",
            "The left side of ?? may be undefined, which a variable would report",
        ),
    ];
    for (body, reason) in cases {
        assert_eq!(refused(&[], &method(body), "Extract variable"), reason, "{body}");
    }
}

#[test]
fn refuses_to_reorder_effects() {
    let reason = refused(
        &[],
        &method(
            "        return $this->repo->findUser(1) !== null ? 1 : strlen(«$this->repo->name()») + strlen($this->repo->name());\n",
        ),
        "Extract variable",
    );
    assert!(reason.starts_with("The expression only runs"), "{reason}");
    let reason = refused(
        &[],
        &method("        return foo($this->repo->name(), «$this->repo->name()»);\n"),
        "Extract variable",
    );
    assert_eq!(reason, "Moving the expression out would change the order things run in");
}

#[test]
fn constants_may_move_past_effects() {
    let found = extract("        return foo($this->repo->name(), «$a . 'x'»);\n");
    assert!(found.contains("$string = $a . 'x';"), "{found}");
}

#[test]
fn keeps_comments_and_neighbors_as_they_are() {
    let found = extract(
        "        // first\n        $x = 1;   // trailing\n\n        /* about this */\n        return («$a + $b») * $x; // done\n",
    );
    assert!(
        found.contains("        // first\n        $x = 1;   // trailing\n"),
        "{found}"
    );
    assert!(
        found.contains("/* about this */\n        $int = $a + $b;\n        return ($int) * $x; // done"),
        "{found}"
    );
}

#[test]
fn keeps_the_line_endings_the_file_has() {
    let source = method("        return («$a + $b») * 2;\n").replace('\n', "\r\n");
    let found = applied(&[], &source, "Extract variable");
    assert!(
        found.contains("$int = $a + $b;\r\n        return ($int) * 2;"),
        "{found:?}"
    );
    assert!(
        !found.replace("\r\n", "").contains('\n'),
        "a bare line feed came in: {found:?}"
    );
}

#[test]
fn writes_with_the_indent_of_the_options_and_leaves_multibyte_text_alone() {
    let source = "<?php\nfunction f($a)\n{\n\treturn strlen(«'héllo ' . $a») + 1;\n}\n";
    let tabs = php_format::FormatOptions {
        indent: php_format::Indent::Tab,
        ..php_format::FormatOptions::default()
    };
    let outcome = super::run_with(super::Setup::PLAIN, &[], source, "Extract variable", &tabs);
    let done = outcome.result.expect("offered").expect("done");
    assert_eq!(
        done.text,
        "<?php\nfunction f($a)\n{\n\t$string = 'héllo ' . $a;\n\treturn strlen($string) + 1;\n}\n"
    );
}

#[test]
fn refuses_what_a_call_takes_by_reference() {
    let reason = refused(
        &[],
        &method("        preg_match('/a/', 'abc', «$items»);\n        return 1;\n"),
        "Extract variable",
    );
    assert_eq!(reason, "The expression is taken by reference");
}
