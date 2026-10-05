use super::{applied, offered, refused};

fn body(code: &str) -> String {
    format!("<?php\nfunction f(int $a, string $s, $x)\n{{\n{code}}}\n")
}

fn rewritten(code: &str, title: &str) -> String {
    applied(&[], &body(code), title)
}

#[test]
fn array_long_and_short_syntax() {
    assert_eq!(
        rewritten(
            "    $v = arr$0ay(1, [2, 3]);\n    return $v;\n",
            "Convert array() to []"
        ),
        body("    $v = [1, [2, 3]];\n    return $v;\n")
    );
    assert_eq!(
        rewritten("    $v = [1, [2$0, 3]];\n    return $v;\n", "Convert [] to array()"),
        body("    $v = [1, array(2, 3)];\n    return $v;\n")
    );
    assert!(
        !offered(&[], &body("    [$q$0, $r] = $x;\n    return 1;\n")).contains(&"Convert [] to array()".to_string())
    );
}

#[test]
fn if_else_and_ternary() {
    assert_eq!(
        rewritten(
            "    i$0f ($a > 1) {\n        return 'big';\n    } else {\n        return 'small';\n    }\n",
            "Replace if/else with a ternary"
        ),
        body("    return $a > 1 ? 'big' : 'small';\n")
    );
    assert_eq!(
        rewritten(
            "    i$0f ($a) $r = 1; else $r = 2;\n    return $r;\n",
            "Replace if/else with a ternary"
        ),
        body("    $r = $a ? 1 : 2;\n    return $r;\n")
    );
    assert!(
        !offered(
            &[],
            &body("    i$0f ($a) {\n        return 1;\n    } else {\n        $z = 2;\n    }\n    return 3;\n")
        )
        .contains(&"Replace if/else with a ternary".to_string())
    );
    assert_eq!(
        rewritten("    return $a ?$0 'x' : 'y';\n", "Replace the ternary with if/else"),
        body("    if ($a) {\n        return 'x';\n    } else {\n        return 'y';\n    }\n")
    );
    assert_eq!(
        rewritten(
            "    $r = $a ? 1$0 : 2;\n    return $r;\n",
            "Replace the ternary with if/else"
        ),
        body("    if ($a) {\n        $r = 1;\n    } else {\n        $r = 2;\n    }\n    return $r;\n")
    );
    assert!(!offered(&[], &body("    return $a ?$0: 2;\n")).contains(&"Replace the ternary with if/else".to_string()));
}

#[test]
fn a_ternary_that_is_nested_gets_parentheses() {
    assert_eq!(
        rewritten(
            "    i$0f ($a) {\n        return $x ? 1 : 2;\n    } else {\n        return 3;\n    }\n",
            "Replace if/else with a ternary"
        ),
        body("    return $a ? ($x ? 1 : 2) : 3;\n")
    );
}

#[test]
fn switch_becomes_match_when_nothing_changes() {
    let code = "    swit$0ch ($a) {\n        case 1:\n        case 2:\n            return 'low';\n        case 3:\n            return 'mid';\n        default:\n            return 'high';\n    }\n";
    assert_eq!(
        rewritten(code, "Convert switch to match"),
        body(
            "    return match ($a) {\n        1, 2 => 'low',\n        3 => 'mid',\n        default => 'high',\n    };\n"
        )
    );
    let assigns = "    swit$0ch ($s) {\n        case 'a':\n            $r = 1;\n            break;\n        default:\n            $r = 2;\n            break;\n    }\n    return $r;\n";
    assert_eq!(
        rewritten(assigns, "Convert switch to match"),
        body("    $r = match ($s) {\n        'a' => 1,\n        default => 2,\n    };\n    return $r;\n")
    );
}

#[test]
fn switch_stays_when_a_match_would_behave_differently() {
    let no_default = "    swit$0ch ($a) {\n        case 1:\n            return 'x';\n    }\n    return 'y';\n";
    assert!(!offered(&[], &body(no_default)).contains(&"Convert switch to match".to_string()));
    let loose = "    swit$0ch ($x) {\n        case 1:\n            return 'x';\n        default:\n            return 'y';\n    }\n";
    assert!(!offered(&[], &body(loose)).contains(&"Convert switch to match".to_string()));
    let mixed = "    swit$0ch ($a) {\n        case 'one':\n            return 'x';\n        default:\n            return 'y';\n    }\n";
    assert!(!offered(&[], &body(mixed)).contains(&"Convert switch to match".to_string()));
    let two = "    swit$0ch ($a) {\n        case 1:\n            $z = 1;\n            return 'x';\n        default:\n            return 'y';\n    }\n";
    assert!(!offered(&[], &body(two)).contains(&"Convert switch to match".to_string()));
}

#[test]
fn concatenation_interpolation_and_sprintf() {
    assert_eq!(
        rewritten(
            "    return 'hello ' . $s$0 . ', you are ' . $a . '!';\n",
            "Convert concatenation to an interpolated string"
        ),
        body("    return \"hello {$s}, you are {$a}!\";\n")
            .replace("{$s}", "$s")
            .replace("{$a}!", "$a!")
    );
    assert_eq!(
        rewritten(
            "    return 'x' . $s$0 . 'y';\n",
            "Convert concatenation to an interpolated string"
        ),
        body("    return \"x{$s}y\";\n")
    );
    assert_eq!(
        rewritten(
            "    return 'a %d' . $s$0 . \"it's\";\n",
            "Convert concatenation to sprintf"
        ),
        body("    return sprintf('a %%d%s' . \"it's\", $s);\n")
            .replace("sprintf('a %%d%s' . \"it's\", $s)", "sprintf('a %%d%sit\\'s', $s)")
    );
    assert!(
        !offered(&[], &body("    return 'a' . strlen($s$0);\n"))
            .contains(&"Convert concatenation to an interpolated string".to_string())
    );
}

#[test]
fn an_interpolated_string_and_sprintf_become_concatenations() {
    assert_eq!(
        rewritten(
            "    return \"hello $s$0, you are {$a}!\";\n",
            "Convert the string to a concatenation"
        ),
        body("    return 'hello ' . $s . ', you are ' . $a . '!';\n")
    );
    assert_eq!(
        rewritten(
            "    return sprintf$0('%s is %s%%', $s, $a);\n",
            "Convert sprintf to a concatenation"
        ),
        body("    return $s . ' is ' . $a . '%';\n")
    );
    assert_eq!(
        rewritten(
            "    return sprintf$0('%s is %s', $s, $a);\n",
            "Convert sprintf to an interpolated string"
        ),
        body("    return \"$s is $a\";\n")
    );
    assert!(
        !offered(&[], &body("    return sprintf$0('%d', $a);\n"))
            .iter()
            .any(|title| title.starts_with("Convert sprintf"))
    );
}

#[test]
fn braces_are_added_and_removed() {
    assert_eq!(
        rewritten("    i$0f ($a) return 1;\n    else return 2;\n", "Add braces"),
        body("    if ($a) {\n        return 1;\n    } else {\n        return 2;\n    }\n")
    );
    assert_eq!(
        rewritten("    foreach ([1] as $y$0) echo $y;\n    return 1;\n", "Add braces"),
        body("    foreach ([1] as $y) {\n        echo $y;\n    }\n    return 1;\n")
    );
    assert_eq!(
        rewritten(
            "    i$0f ($a) {\n        return 1;\n    }\n    return 2;\n",
            "Remove braces"
        ),
        body("    if ($a) return 1;\n    return 2;\n")
    );
    assert!(
        !offered(
            &[],
            &body("    i$0f ($a) {\n        // note\n        return 1;\n    }\n    return 2;\n")
        )
        .contains(&"Remove braces".to_string())
    );
    let dangling = rewritten(
        "    i$0f ($a) {\n        if ($x) {\n            return 1;\n        }\n    } else {\n        return 2;\n    }\n    return 3;\n",
        "Remove braces",
    );
    assert_eq!(
        dangling,
        body(
            "    if ($a) {\n        if ($x) {\n            return 1;\n        }\n    } else return 2;\n    return 3;\n"
        )
    );
}

#[test]
fn declarations_are_split_and_joined() {
    let class = "<?php\nclass A\n{\n    private int $a$0, $b = 2;\n    const X = 1, Y = 2;\n}\n";
    let found = applied(&[], class, "Split into separate declarations");
    assert_eq!(
        found,
        "<?php\nclass A\n{\n    private int $a;\n    private int $b = 2;\n    const X = 1, Y = 2;\n}\n"
    );
    let joined = applied(
        &[],
        "<?php\nclass A\n{\n    private int $a;\n    private int $b$0 = 2;\n    public int $c;\n}\n",
        "Join adjacent declarations",
    );
    assert_eq!(
        joined,
        "<?php\nclass A\n{\n    private int $a, $b = 2;\n    public int $c;\n}\n"
    );
    let constants = applied(
        &[],
        "<?php\nclass A\n{\n    const X = 1, Y$0 = 2;\n}\n",
        "Split into separate declarations",
    );
    assert_eq!(constants, "<?php\nclass A\n{\n    const X = 1;\n    const Y = 2;\n}\n");
}

#[test]
fn comparisons_flip() {
    assert_eq!(
        rewritten("    return $a <$0 5;\n", "Flip the comparison"),
        body("    return 5 > $a;\n")
    );
    assert_eq!(
        rewritten("    return $a !==$0 5;\n", "Flip the comparison"),
        body("    return 5 !== $a;\n")
    );
    assert!(!offered(&[], &body("    return foo($a)$0 === foo($s);\n")).contains(&"Flip the comparison".to_string()));
}

#[test]
fn an_if_is_inverted() {
    assert_eq!(
        rewritten(
            "    i$0f ($a === 1) {\n        return 'one';\n    } else {\n        return 'other';\n    }\n",
            "Invert the if"
        ),
        body("    if ($a !== 1) {\n        return 'other';\n    } else {\n        return 'one';\n    }\n")
    );
    assert_eq!(
        rewritten(
            "    i$0f (!$a) {\n        return 1;\n    } else {\n        return 2;\n    }\n",
            "Invert the if"
        ),
        body("    if ($a) {\n        return 2;\n    } else {\n        return 1;\n    }\n")
    );
    assert_eq!(
        rewritten(
            "    i$0f ($a > 1 && $s) {\n        return 1;\n    } else {\n        return 2;\n    }\n",
            "Invert the if"
        ),
        body("    if (!($a > 1 && $s)) {\n        return 2;\n    } else {\n        return 1;\n    }\n")
    );
    assert!(
        !offered(&[], &body("    i$0f ($a) {\n        return 1;\n    }\n    return 2;\n"))
            .contains(&"Invert the if".to_string())
    );
}

#[test]
fn arguments_get_names_and_lose_them() {
    let functions = "<?php\nfunction f(int $first, string $second = '', int ...$rest) {}\nfunction g(int $first, string $second = '') {}\n";
    let source = format!("{functions}g(1, 'x');\n");
    let with_cursor = source.replace("g(1, 'x')", "g(1$0, 'x')");
    assert_eq!(
        applied(&[], &with_cursor, "Add argument names"),
        format!("{functions}\ng(first: 1, second: 'x');\n")
    );
    let named = format!("{functions}g(first: 1$0, second: 'x');\n");
    assert_eq!(
        applied(&[], &named, "Remove the argument names"),
        format!("{functions}\ng(1, 'x');\n")
    );
    let out_of_order = format!("{functions}g(second: 'x'$0, first: 1);\n");
    assert!(!offered(&[], &out_of_order).contains(&"Remove the argument names".to_string()));
    let variadic = format!("{functions}f(1, 'x', 3$0);\n");
    assert!(!offered(&[], &variadic).contains(&"Add argument names".to_string()));
    let _ = refused;
}
