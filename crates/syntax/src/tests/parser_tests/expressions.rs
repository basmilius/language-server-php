use super::{check, expr};
use expect_test::expect;

#[test]
fn arithmetic_precedence() {
    expr(
        "$a + $b * $c",
        expect![[r#"
        (BINARY_EXPR (VARIABLE_EXPR "$a") "+" (BINARY_EXPR (VARIABLE_EXPR "$b") "*" (VARIABLE_EXPR "$c")))
    "#]],
    );
    expr(
        "$a * $b + $c",
        expect![[r#"
        (BINARY_EXPR (BINARY_EXPR (VARIABLE_EXPR "$a") "*" (VARIABLE_EXPR "$b")) "+" (VARIABLE_EXPR "$c"))
    "#]],
    );
    expr(
        "$a - $b - $c",
        expect![[r#"
        (BINARY_EXPR (BINARY_EXPR (VARIABLE_EXPR "$a") "-" (VARIABLE_EXPR "$b")) "-" (VARIABLE_EXPR "$c"))
    "#]],
    );
    expr(
        "$a . $b + $c",
        expect![[r#"
        (BINARY_EXPR (VARIABLE_EXPR "$a") "." (BINARY_EXPR (VARIABLE_EXPR "$b") "+" (VARIABLE_EXPR "$c")))
    "#]],
    );
    expr(
        "$a << 1 + 2",
        expect![[r#"
        (BINARY_EXPR (VARIABLE_EXPR "$a") "<<" (BINARY_EXPR (LITERAL "1") "+" (LITERAL "2")))
    "#]],
    );
    expr(
        "2 ** 3 ** 2",
        expect![[r#"
        (BINARY_EXPR (LITERAL "2") "**" (BINARY_EXPR (LITERAL "3") "**" (LITERAL "2")))
    "#]],
    );
    expr(
        "-2 ** 2",
        expect![[r#"
        (PREFIX_EXPR "-" (BINARY_EXPR (LITERAL "2") "**" (LITERAL "2")))
    "#]],
    );
    expr(
        "2 ** -1",
        expect![[r#"
        (BINARY_EXPR (LITERAL "2") "**" (PREFIX_EXPR "-" (LITERAL "1")))
    "#]],
    );
}

#[test]
fn logical_and_comparison_precedence() {
    expr(
        "$a || $b && $c",
        expect![[r#"
        (BINARY_EXPR (VARIABLE_EXPR "$a") "||" (BINARY_EXPR (VARIABLE_EXPR "$b") "&&" (VARIABLE_EXPR "$c")))
    "#]],
    );
    expr(
        "$a == $b < $c",
        expect![[r#"
        (BINARY_EXPR (VARIABLE_EXPR "$a") "==" (BINARY_EXPR (VARIABLE_EXPR "$b") "<" (VARIABLE_EXPR "$c")))
    "#]],
    );
    expr(
        "$a | $b ^ $c & $d",
        expect![[r#"
        (BINARY_EXPR (VARIABLE_EXPR "$a") "|" (BINARY_EXPR (VARIABLE_EXPR "$b") "^" (BINARY_EXPR (VARIABLE_EXPR "$c") "&" (VARIABLE_EXPR "$d"))))
    "#]],
    );
    expr(
        "$a and $b or $c xor $d",
        expect![[r#"
        (BINARY_EXPR (BINARY_EXPR (VARIABLE_EXPR "$a") "and" (VARIABLE_EXPR "$b")) "or" (BINARY_EXPR (VARIABLE_EXPR "$c") "xor" (VARIABLE_EXPR "$d")))
    "#]],
    );
    expr(
        "!$a instanceof B",
        expect![[r#"
        (PREFIX_EXPR "!" (BINARY_EXPR (VARIABLE_EXPR "$a") "instanceof" (NAME "B")))
    "#]],
    );
    expr(
        "!$a && $b",
        expect![[r#"
        (BINARY_EXPR (PREFIX_EXPR "!" (VARIABLE_EXPR "$a")) "&&" (VARIABLE_EXPR "$b"))
    "#]],
    );
    expr(
        "$a <=> $b",
        expect![[r#"
        (BINARY_EXPR (VARIABLE_EXPR "$a") "<=>" (VARIABLE_EXPR "$b"))
    "#]],
    );
}

#[test]
fn assignment() {
    expr(
        "$a = $b = 1",
        expect![[r#"
        (ASSIGN_EXPR (VARIABLE_EXPR "$a") "=" (ASSIGN_EXPR (VARIABLE_EXPR "$b") "=" (LITERAL "1")))
    "#]],
    );
    expr(
        "$a = $b + 1",
        expect![[r#"
        (ASSIGN_EXPR (VARIABLE_EXPR "$a") "=" (BINARY_EXPR (VARIABLE_EXPR "$b") "+" (LITERAL "1")))
    "#]],
    );
    expr(
        "$a += 1",
        expect![[r#"
        (ASSIGN_EXPR (VARIABLE_EXPR "$a") "+=" (LITERAL "1"))
    "#]],
    );
    expr(
        "$a ??= []",
        expect![[r#"
        (ASSIGN_EXPR (VARIABLE_EXPR "$a") "??=" (ARRAY_EXPR "[" "]"))
    "#]],
    );
    expr(
        "$a = &$b",
        expect![[r#"
        (ASSIGN_EXPR (VARIABLE_EXPR "$a") "=" "&" (VARIABLE_EXPR "$b"))
    "#]],
    );
    expr(
        "!$a = foo()",
        expect![[r#"
        (PREFIX_EXPR "!" (ASSIGN_EXPR (VARIABLE_EXPR "$a") "=" (CALL_EXPR (NAME "foo") (ARGUMENT_LIST "(" ")"))))
    "#]],
    );
    expr(
        "$a + $b = 3",
        expect![[r#"
        (BINARY_EXPR (VARIABLE_EXPR "$a") "+" (ASSIGN_EXPR (VARIABLE_EXPR "$b") "=" (LITERAL "3")))
    "#]],
    );
    expr(
        "[$a, [$b, $c]] = $x",
        expect![[r#"
        (ASSIGN_EXPR (ARRAY_EXPR "[" (ARRAY_ITEM (VARIABLE_EXPR "$a")) "," (ARRAY_ITEM (ARRAY_EXPR "[" (ARRAY_ITEM (VARIABLE_EXPR "$b")) "," (ARRAY_ITEM (VARIABLE_EXPR "$c")) "]")) "]") "=" (VARIABLE_EXPR "$x"))
    "#]],
    );
    expr(
        "['k' => $v, 1 => [$w]] = $x",
        expect![[r#"
        (ASSIGN_EXPR (ARRAY_EXPR "[" (ARRAY_ITEM (LITERAL "'k'") "=>" (VARIABLE_EXPR "$v")) "," (ARRAY_ITEM (LITERAL "1") "=>" (ARRAY_EXPR "[" (ARRAY_ITEM (VARIABLE_EXPR "$w")) "]")) "]") "=" (VARIABLE_EXPR "$x"))
    "#]],
    );
    expr(
        "list($a, , $b) = $x",
        expect![[r#"
        (ASSIGN_EXPR (LIST_EXPR "list" "(" (ARRAY_ITEM (VARIABLE_EXPR "$a")) "," "," (ARRAY_ITEM (VARIABLE_EXPR "$b")) ")") "=" (VARIABLE_EXPR "$x"))
    "#]],
    );
    expr(
        "$a->b['c']::$d = 1",
        expect![[r#"
        (ASSIGN_EXPR (STATIC_PROPERTY_EXPR (INDEX_EXPR (PROPERTY_FETCH_EXPR (VARIABLE_EXPR "$a") "->" (NAME "b")) "[" (LITERAL "'c'") "]") "::" (VARIABLE_EXPR "$d")) "=" (LITERAL "1"))
    "#]],
    );
    expr(
        "$a and $b = 1",
        expect![[r#"
        (BINARY_EXPR (VARIABLE_EXPR "$a") "and" (ASSIGN_EXPR (VARIABLE_EXPR "$b") "=" (LITERAL "1")))
    "#]],
    );
}

#[test]
fn ternary_and_coalesce() {
    expr(
        "$a ? $b : $c",
        expect![[r#"
        (TERNARY_EXPR (VARIABLE_EXPR "$a") "?" (VARIABLE_EXPR "$b") ":" (VARIABLE_EXPR "$c"))
    "#]],
    );
    expr(
        "$a ?: $b",
        expect![[r#"
        (TERNARY_EXPR (VARIABLE_EXPR "$a") "?" ":" (VARIABLE_EXPR "$b"))
    "#]],
    );
    expr(
        "$a ?? $b ?? $c",
        expect![[r#"
        (BINARY_EXPR (VARIABLE_EXPR "$a") "??" (BINARY_EXPR (VARIABLE_EXPR "$b") "??" (VARIABLE_EXPR "$c")))
    "#]],
    );
    expr(
        "$a ? $b : $c ? $d : $e",
        expect![[r#"
        (TERNARY_EXPR (TERNARY_EXPR (VARIABLE_EXPR "$a") "?" (VARIABLE_EXPR "$b") ":" (VARIABLE_EXPR "$c")) "?" (VARIABLE_EXPR "$d") ":" (VARIABLE_EXPR "$e"))
    "#]],
    );
    expr(
        "$a ?: $b ?: $c",
        expect![[r#"
        (TERNARY_EXPR (TERNARY_EXPR (VARIABLE_EXPR "$a") "?" ":" (VARIABLE_EXPR "$b")) "?" ":" (VARIABLE_EXPR "$c"))
    "#]],
    );
    expr(
        "$a ?? $b ? 1 : 2",
        expect![[r#"
        (TERNARY_EXPR (BINARY_EXPR (VARIABLE_EXPR "$a") "??" (VARIABLE_EXPR "$b")) "?" (LITERAL "1") ":" (LITERAL "2"))
    "#]],
    );
    expr(
        "$a ? 1 : $b = 2",
        expect![[r#"
        (TERNARY_EXPR (VARIABLE_EXPR "$a") "?" (LITERAL "1") ":" (ASSIGN_EXPR (VARIABLE_EXPR "$b") "=" (LITERAL "2")))
    "#]],
    );
}

#[test]
fn unary_and_casts() {
    expr(
        "-$a + +$b",
        expect![[r#"
        (BINARY_EXPR (PREFIX_EXPR "-" (VARIABLE_EXPR "$a")) "+" (PREFIX_EXPR "+" (VARIABLE_EXPR "$b")))
    "#]],
    );
    expr(
        "~$a & !$b",
        expect![[r#"
        (BINARY_EXPR (PREFIX_EXPR "~" (VARIABLE_EXPR "$a")) "&" (PREFIX_EXPR "!" (VARIABLE_EXPR "$b")))
    "#]],
    );
    expr(
        "@$a['x']",
        expect![[r#"
        (PREFIX_EXPR "@" (INDEX_EXPR (VARIABLE_EXPR "$a") "[" (LITERAL "'x'") "]"))
    "#]],
    );
    expr(
        "(int) $a + 1",
        expect![[r#"
        (BINARY_EXPR (CAST_EXPR "(int)" (VARIABLE_EXPR "$a")) "+" (LITERAL "1"))
    "#]],
    );
    expr(
        "(string) $a . 'x'",
        expect![[r#"
        (BINARY_EXPR (CAST_EXPR "(string)" (VARIABLE_EXPR "$a")) "." (LITERAL "'x'"))
    "#]],
    );
    expr(
        "++$a + $b--",
        expect![[r#"
        (BINARY_EXPR (PREFIX_EXPR "++" (VARIABLE_EXPR "$a")) "+" (POSTFIX_EXPR (VARIABLE_EXPR "$b") "--"))
    "#]],
    );
    expr(
        "(void) foo()",
        expect![[r#"
        (CAST_EXPR "(void)" (CALL_EXPR (NAME "foo") (ARGUMENT_LIST "(" ")")))
    "#]],
    );
    expr(
        "clone $a->b",
        expect![[r#"
        (CLONE_EXPR "clone" (PROPERTY_FETCH_EXPR (VARIABLE_EXPR "$a") "->" (NAME "b")))
    "#]],
    );
    expr(
        "print $a . 'x'",
        expect![[r#"
        (PRINT_EXPR "print" (BINARY_EXPR (VARIABLE_EXPR "$a") "." (LITERAL "'x'")))
    "#]],
    );
}

#[test]
fn postfix_chains() {
    expr(
        "$a->b->c()",
        expect![[r#"
        (CALL_EXPR (PROPERTY_FETCH_EXPR (PROPERTY_FETCH_EXPR (VARIABLE_EXPR "$a") "->" (NAME "b")) "->" (NAME "c")) (ARGUMENT_LIST "(" ")"))
    "#]],
    );
    expr(
        "$a?->b?->c",
        expect![[r#"
        (PROPERTY_FETCH_EXPR (PROPERTY_FETCH_EXPR (VARIABLE_EXPR "$a") "?->" (NAME "b")) "?->" (NAME "c"))
    "#]],
    );
    expr(
        "$a['x'][] ",
        expect![[r#"
        (INDEX_EXPR (INDEX_EXPR (VARIABLE_EXPR "$a") "[" (LITERAL "'x'") "]") "[" "]")
    "#]],
    );
    expr(
        "foo()()",
        expect![[r#"
        (CALL_EXPR (CALL_EXPR (NAME "foo") (ARGUMENT_LIST "(" ")")) (ARGUMENT_LIST "(" ")"))
    "#]],
    );
    expr(
        "Foo::bar()::baz",
        expect![[r#"
        (SCOPED_ACCESS_EXPR (CALL_EXPR (SCOPED_ACCESS_EXPR (NAME "Foo") "::" (NAME "bar")) (ARGUMENT_LIST "(" ")")) "::" (NAME "baz"))
    "#]],
    );
    expr(
        "Foo::$bar",
        expect![[r#"
        (STATIC_PROPERTY_EXPR (NAME "Foo") "::" (VARIABLE_EXPR "$bar"))
    "#]],
    );
    expr(
        "Foo::BAR",
        expect![[r#"
        (SCOPED_ACCESS_EXPR (NAME "Foo") "::" (NAME "BAR"))
    "#]],
    );
    expr(
        "Foo::class",
        expect![[r#"
        (SCOPED_ACCESS_EXPR (NAME "Foo") "::" (NAME "class"))
    "#]],
    );
    expr(
        "$a::class",
        expect![[r#"
        (SCOPED_ACCESS_EXPR (VARIABLE_EXPR "$a") "::" (NAME "class"))
    "#]],
    );
    expr(
        "static::create()",
        expect![[r#"
        (CALL_EXPR (SCOPED_ACCESS_EXPR (NAME "static") "::" (NAME "create")) (ARGUMENT_LIST "(" ")"))
    "#]],
    );
    expr(
        "$a->{'b' . 'c'}",
        expect![[r#"
        (PROPERTY_FETCH_EXPR (VARIABLE_EXPR "$a") "->" "{" (BINARY_EXPR (LITERAL "'b'") "." (LITERAL "'c'")) "}")
    "#]],
    );
    expr(
        "$a->$b",
        expect![[r#"
        (PROPERTY_FETCH_EXPR (VARIABLE_EXPR "$a") "->" (VARIABLE_EXPR "$b"))
    "#]],
    );
    expr(
        "$$a",
        expect![[r#"
        (VARIABLE_VARIABLE "$" (VARIABLE_EXPR "$a"))
    "#]],
    );
    expr(
        "${'a' . 'b'}",
        expect![[r#"
        (VARIABLE_VARIABLE "$" "{" (BINARY_EXPR (LITERAL "'a'") "." (LITERAL "'b'")) "}")
    "#]],
    );
    expr(
        "$a->class",
        expect![[r#"
        (PROPERTY_FETCH_EXPR (VARIABLE_EXPR "$a") "->" (NAME "class"))
    "#]],
    );
    expr(
        "Foo::list()",
        expect![[r#"
        (CALL_EXPR (SCOPED_ACCESS_EXPR (NAME "Foo") "::" (NAME "list")) (ARGUMENT_LIST "(" ")"))
    "#]],
    );
    expr(
        "Foo::{$name}",
        expect![[r#"
        (SCOPED_ACCESS_EXPR (NAME "Foo") "::" "{" (VARIABLE_EXPR "$name") "}")
    "#]],
    );
    expr(
        "\\Foo\\bar(1)",
        expect![[r#"
        (CALL_EXPR (NAME "\\Foo\\bar") (ARGUMENT_LIST "(" (ARGUMENT (LITERAL "1")) ")"))
    "#]],
    );
    expr(
        "namespace\\foo()",
        expect![[r#"
        (CALL_EXPR (NAME "namespace\\foo") (ARGUMENT_LIST "(" ")"))
    "#]],
    );
    expr(
        "(clone $a)->b",
        expect![[r#"
        (PROPERTY_FETCH_EXPR (PAREN_EXPR "(" (CLONE_EXPR "clone" (VARIABLE_EXPR "$a")) ")") "->" (NAME "b"))
    "#]],
    );
}

#[test]
fn calls_and_arguments() {
    expr(
        "foo(1, ...$rest, name: 2,)",
        expect![[r#"
        (CALL_EXPR (NAME "foo") (ARGUMENT_LIST "(" (ARGUMENT (LITERAL "1")) "," (ARGUMENT "..." (VARIABLE_EXPR "$rest")) "," (ARGUMENT "name" ":" (LITERAL "2")) "," ")"))
    "#]],
    );
    expr(
        "foo(...)",
        expect![[r#"
        (CALL_EXPR (NAME "foo") (ARGUMENT_LIST "(" "..." ")"))
    "#]],
    );
    expr(
        "$a->foo(...)",
        expect![[r#"
        (CALL_EXPR (PROPERTY_FETCH_EXPR (VARIABLE_EXPR "$a") "->" (NAME "foo")) (ARGUMENT_LIST "(" "..." ")"))
    "#]],
    );
    expr(
        "Foo::bar(...)",
        expect![[r#"
        (CALL_EXPR (SCOPED_ACCESS_EXPR (NAME "Foo") "::" (NAME "bar")) (ARGUMENT_LIST "(" "..." ")"))
    "#]],
    );
    expr(
        "foo(default: 1, class: 2)",
        expect![[r#"
        (CALL_EXPR (NAME "foo") (ARGUMENT_LIST "(" (ARGUMENT "default" ":" (LITERAL "1")) "," (ARGUMENT "class" ":" (LITERAL "2")) ")"))
    "#]],
    );
    expr(
        "strlen(...)",
        expect![[r#"
        (CALL_EXPR (NAME "strlen") (ARGUMENT_LIST "(" "..." ")"))
    "#]],
    );
    expr(
        "clone(...)",
        expect![[r#"
        (CLONE_EXPR "clone" (ARGUMENT_LIST "(" "..." ")"))
    "#]],
    );
    expr(
        "clone($a, ['x' => 1])",
        expect![[r#"
        (CLONE_EXPR "clone" (ARGUMENT_LIST "(" (ARGUMENT (VARIABLE_EXPR "$a")) "," (ARGUMENT (ARRAY_EXPR "[" (ARRAY_ITEM (LITERAL "'x'") "=>" (LITERAL "1")) "]")) ")"))
    "#]],
    );
    expr(
        "clone(object: $a)",
        expect![[r#"
        (CLONE_EXPR "clone" (ARGUMENT_LIST "(" (ARGUMENT "object" ":" (VARIABLE_EXPR "$a")) ")"))
    "#]],
    );
}

#[test]
fn new_expressions() {
    expr(
        "new Foo",
        expect![[r#"
        (NEW_EXPR "new" (NAME "Foo"))
    "#]],
    );
    expr(
        "new Foo(1, 2)",
        expect![[r#"
        (NEW_EXPR "new" (NAME "Foo") (ARGUMENT_LIST "(" (ARGUMENT (LITERAL "1")) "," (ARGUMENT (LITERAL "2")) ")"))
    "#]],
    );
    expr(
        "new Foo()->bar()",
        expect![[r#"
        (CALL_EXPR (PROPERTY_FETCH_EXPR (NEW_EXPR "new" (NAME "Foo") (ARGUMENT_LIST "(" ")")) "->" (NAME "bar")) (ARGUMENT_LIST "(" ")"))
    "#]],
    );
    expr(
        "new static",
        expect![[r#"
        (NEW_EXPR "new" (NAME "static"))
    "#]],
    );
    expr(
        "new $class($a)",
        expect![[r#"
        (NEW_EXPR "new" (VARIABLE_EXPR "$class") (ARGUMENT_LIST "(" (ARGUMENT (VARIABLE_EXPR "$a")) ")"))
    "#]],
    );
    expr(
        "new $a->b['c']",
        expect![[r#"
        (NEW_EXPR "new" (INDEX_EXPR (PROPERTY_FETCH_EXPR (VARIABLE_EXPR "$a") "->" (NAME "b")) "[" (LITERAL "'c'") "]"))
    "#]],
    );
    expr(
        "new A::$b",
        expect![[r#"
        (NEW_EXPR "new" (STATIC_PROPERTY_EXPR (NAME "A") "::" (VARIABLE_EXPR "$b")))
    "#]],
    );
    expr(
        "new ($a . 'b')",
        expect![[r#"
        (NEW_EXPR "new" (PAREN_EXPR "(" (BINARY_EXPR (VARIABLE_EXPR "$a") "." (LITERAL "'b'")) ")"))
    "#]],
    );
    expr(
        "new class(1) extends Base implements I { public $x = 1; }",
        expect![[r#"
            (NEW_EXPR "new" (ANONYMOUS_CLASS "class" (ARGUMENT_LIST "(" (ARGUMENT (LITERAL "1")) ")") (EXTENDS_CLAUSE "extends" (NAMED_TYPE (NAME "Base"))) (IMPLEMENTS_CLAUSE "implements" (NAMED_TYPE (NAME "I"))) (CLASS_BODY
                  "{"
                  (PROPERTY_DECLARATION (MODIFIER_LIST "public") (PROPERTY_ELEMENT "$x" "=" (LITERAL "1")) ";")
                  "}")))
        "#]],
    );
    expr(
        "new #[Attr] class {}",
        expect![[r##"
        (NEW_EXPR "new" (ATTRIBUTE_LIST "#[" (ATTRIBUTE (NAME "Attr")) "]") (ANONYMOUS_CLASS "class" (CLASS_BODY
              "{"
              "}")))
    "##]],
    );
    expr(
        "new readonly class {}",
        expect![[r#"
        (NEW_EXPR "new" (ANONYMOUS_CLASS (MODIFIER_LIST "readonly") "class" (CLASS_BODY
              "{"
              "}")))
    "#]],
    );
    expr(
        "(new Foo)->bar",
        expect![[r#"
        (PROPERTY_FETCH_EXPR (PAREN_EXPR "(" (NEW_EXPR "new" (NAME "Foo")) ")") "->" (NAME "bar"))
    "#]],
    );
}

#[test]
fn class_references_with_static_properties() {
    expr(
        "$a instanceof self::$engine",
        expect![[r#"
        (BINARY_EXPR (VARIABLE_EXPR "$a") "instanceof" (STATIC_PROPERTY_EXPR (NAME "self") "::" (VARIABLE_EXPR "$engine")))
    "#]],
    );
    expr(
        "$a instanceof $b->c",
        expect![[r#"
        (BINARY_EXPR (VARIABLE_EXPR "$a") "instanceof" (PROPERTY_FETCH_EXPR (VARIABLE_EXPR "$b") "->" (NAME "c")))
    "#]],
    );
    expr(
        "new static::$class($env)",
        expect![[r#"
        (NEW_EXPR "new" (STATIC_PROPERTY_EXPR (NAME "static") "::" (VARIABLE_EXPR "$class")) (ARGUMENT_LIST "(" (ARGUMENT (VARIABLE_EXPR "$env")) ")"))
    "#]],
    );
}

#[test]
fn closures_and_arrow_functions() {
    expr(
        "function ($a) use ($b, &$c): int { return 1; }",
        expect![[r#"
        (CLOSURE_EXPR "function" (PARAMETER_LIST "(" (PARAMETER "$a") ")") (CLOSURE_USE "use" "(" (CLOSURE_USE_VARIABLE "$b") "," (CLOSURE_USE_VARIABLE "&" "$c") ")") (RETURN_TYPE ":" (NAMED_TYPE (NAME "int"))) (BLOCK
            "{"
            (RETURN_STATEMENT "return" (LITERAL "1") ";")
            "}"))
    "#]],
    );
    expr(
        "static function () {}",
        expect![[r#"
        (CLOSURE_EXPR "static" "function" (PARAMETER_LIST "(" ")") (BLOCK
            "{"
            "}"))
    "#]],
    );
    expr(
        "fn($x) => $x * 2",
        expect![[r#"
        (ARROW_FUNCTION_EXPR "fn" (PARAMETER_LIST "(" (PARAMETER "$x") ")") "=>" (BINARY_EXPR (VARIABLE_EXPR "$x") "*" (LITERAL "2")))
    "#]],
    );
    expr(
        "static fn(int ...$xs): int => array_sum($xs)",
        expect![[r#"
        (ARROW_FUNCTION_EXPR "static" "fn" (PARAMETER_LIST "(" (PARAMETER (NAMED_TYPE (NAME "int")) "..." "$xs") ")") (RETURN_TYPE ":" (NAMED_TYPE (NAME "int"))) "=>" (CALL_EXPR (NAME "array_sum") (ARGUMENT_LIST "(" (ARGUMENT (VARIABLE_EXPR "$xs")) ")")))
    "#]],
    );
    expr(
        "fn&($x) => $x",
        expect![[r#"
        (ARROW_FUNCTION_EXPR "fn" "&" (PARAMETER_LIST "(" (PARAMETER "$x") ")") "=>" (VARIABLE_EXPR "$x"))
    "#]],
    );
    expr(
        "#[Pure] fn() => 1",
        expect![[r##"
        (ARROW_FUNCTION_EXPR (ATTRIBUTE_LIST "#[" (ATTRIBUTE (NAME "Pure")) "]") "fn" (PARAMETER_LIST "(" ")") "=>" (LITERAL "1"))
    "##]],
    );
    expr(
        "fn($x) => fn($y) => $x + $y",
        expect![[r#"
        (ARROW_FUNCTION_EXPR "fn" (PARAMETER_LIST "(" (PARAMETER "$x") ")") "=>" (ARROW_FUNCTION_EXPR "fn" (PARAMETER_LIST "(" (PARAMETER "$y") ")") "=>" (BINARY_EXPR (VARIABLE_EXPR "$x") "+" (VARIABLE_EXPR "$y"))))
    "#]],
    );
    expr(
        "(function () {})()",
        expect![[r#"
        (CALL_EXPR (PAREN_EXPR "(" (CLOSURE_EXPR "function" (PARAMETER_LIST "(" ")") (BLOCK
                "{"
                "}")) ")") (ARGUMENT_LIST "(" ")"))
    "#]],
    );
}

#[test]
fn match_expressions() {
    expr(
        "match ($a) { 1, 2 => 'x', default => 'y' }",
        expect![[r#"
        (MATCH_EXPR "match" "(" (VARIABLE_EXPR "$a") ")" "{" (MATCH_ARM (LITERAL "1") "," (LITERAL "2") "=>" (LITERAL "'x'")) "," (MATCH_ARM "default" "=>" (LITERAL "'y'")) "}")
    "#]],
    );
    expr(
        "match (true) { $a > 1 => throw new E, }",
        expect![[r#"
        (MATCH_EXPR "match" "(" (NAME "true") ")" "{" (MATCH_ARM (BINARY_EXPR (VARIABLE_EXPR "$a") ">" (LITERAL "1")) "=>" (THROW_EXPR "throw" (NEW_EXPR "new" (NAME "E")))) "," "}")
    "#]],
    );
}

#[test]
fn arrays_and_lists() {
    expr(
        "[1, 'a' => 2, ...$rest, &$ref]",
        expect![[r#"
        (ARRAY_EXPR "[" (ARRAY_ITEM (LITERAL "1")) "," (ARRAY_ITEM (LITERAL "'a'") "=>" (LITERAL "2")) "," (ARRAY_ITEM "..." (VARIABLE_EXPR "$rest")) "," (ARRAY_ITEM "&" (VARIABLE_EXPR "$ref")) "]")
    "#]],
    );
    expr(
        "array(1, 2,)",
        expect![[r#"
        (ARRAY_EXPR "array" "(" (ARRAY_ITEM (LITERAL "1")) "," (ARRAY_ITEM (LITERAL "2")) "," ")")
    "#]],
    );
    expr(
        "[]",
        expect![[r#"
        (ARRAY_EXPR "[" "]")
    "#]],
    );
    expr(
        "[1, 2][0]",
        expect![[r#"
        (INDEX_EXPR (ARRAY_EXPR "[" (ARRAY_ITEM (LITERAL "1")) "," (ARRAY_ITEM (LITERAL "2")) "]") "[" (LITERAL "0") "]")
    "#]],
    );
    expr(
        "[,, $a]",
        expect![[r#"
        (ARRAY_EXPR "[" "," "," (ARRAY_ITEM (VARIABLE_EXPR "$a")) "]")
    "#]],
    );
}

#[test]
fn language_constructs() {
    expr(
        "isset($a, $b['c'])",
        expect![[r#"
        (ISSET_EXPR "isset" "(" (VARIABLE_EXPR "$a") "," (INDEX_EXPR (VARIABLE_EXPR "$b") "[" (LITERAL "'c'") "]") ")")
    "#]],
    );
    expr(
        "empty($a)",
        expect![[r#"
        (EMPTY_EXPR "empty" "(" (VARIABLE_EXPR "$a") ")")
    "#]],
    );
    expr(
        "eval('1')",
        expect![[r#"
        (EVAL_EXPR "eval" "(" (LITERAL "'1'") ")")
    "#]],
    );
    expr(
        "exit",
        expect![[r#"
        (EXIT_EXPR "exit")
    "#]],
    );
    expr(
        "die('x')",
        expect![[r#"
        (EXIT_EXPR "die" "(" (LITERAL "'x'") ")")
    "#]],
    );
    expr(
        "exit(...)",
        expect![[r#"
        (EXIT_EXPR "exit" (ARGUMENT_LIST "(" "..." ")"))
    "#]],
    );
    expr(
        "include 'a.php'",
        expect![[r#"
        (INCLUDE_EXPR "include" (LITERAL "'a.php'"))
    "#]],
    );
    expr(
        "require_once __DIR__ . '/b.php'",
        expect![[r#"
        (INCLUDE_EXPR "require_once" (BINARY_EXPR (LITERAL "__DIR__") "." (LITERAL "'/b.php'")))
    "#]],
    );
    expr(
        "throw new Exception('x')",
        expect![[r#"
        (THROW_EXPR "throw" (NEW_EXPR "new" (NAME "Exception") (ARGUMENT_LIST "(" (ARGUMENT (LITERAL "'x'")) ")")))
    "#]],
    );
    expr(
        "$a ?? throw new E",
        expect![[r#"
        (BINARY_EXPR (VARIABLE_EXPR "$a") "??" (THROW_EXPR "throw" (NEW_EXPR "new" (NAME "E"))))
    "#]],
    );
    expr(
        "yield",
        expect![[r#"
        (YIELD_EXPR "yield")
    "#]],
    );
    expr(
        "yield $a",
        expect![[r#"
        (YIELD_EXPR "yield" (VARIABLE_EXPR "$a"))
    "#]],
    );
    expr(
        "yield $k => $v",
        expect![[r#"
        (YIELD_EXPR "yield" (VARIABLE_EXPR "$k") "=>" (VARIABLE_EXPR "$v"))
    "#]],
    );
    expr(
        "yield from $a",
        expect![[r#"
        (YIELD_FROM_EXPR "yield" "from" (VARIABLE_EXPR "$a"))
    "#]],
    );
    expr(
        "$x = yield $y",
        expect![[r#"
        (ASSIGN_EXPR (VARIABLE_EXPR "$x") "=" (YIELD_EXPR "yield" (VARIABLE_EXPR "$y")))
    "#]],
    );
    expr(
        "__LINE__",
        expect![[r#"
        (LITERAL "__LINE__")
    "#]],
    );
}

#[test]
fn pipe_operator() {
    expr(
        "$a |> foo(...) |> bar(...)",
        expect![[r#"
        (BINARY_EXPR (BINARY_EXPR (VARIABLE_EXPR "$a") "|>" (CALL_EXPR (NAME "foo") (ARGUMENT_LIST "(" "..." ")"))) "|>" (CALL_EXPR (NAME "bar") (ARGUMENT_LIST "(" "..." ")")))
    "#]],
    );
    expr(
        "$a . 'x' |> strlen(...)",
        expect![[r#"
        (BINARY_EXPR (BINARY_EXPR (VARIABLE_EXPR "$a") "." (LITERAL "'x'")) "|>" (CALL_EXPR (NAME "strlen") (ARGUMENT_LIST "(" "..." ")")))
    "#]],
    );
    expr(
        "$a |> foo(...) == 3",
        expect![[r#"
        (BINARY_EXPR (BINARY_EXPR (VARIABLE_EXPR "$a") "|>" (CALL_EXPR (NAME "foo") (ARGUMENT_LIST "(" "..." ")"))) "==" (LITERAL "3"))
    "#]],
    );
}

#[test]
fn numbers_and_strings() {
    expr(
        "0x1F + 0b11 + 0o17 + 1_000 + 1.5e3",
        expect![[r#"
        (BINARY_EXPR (BINARY_EXPR (BINARY_EXPR (BINARY_EXPR (LITERAL "0x1F") "+" (LITERAL "0b11")) "+" (LITERAL "0o17")) "+" (LITERAL "1_000")) "+" (LITERAL "1.5e3"))
    "#]],
    );
    expr(
        "'single'",
        expect![[r#"
        (LITERAL "'single'")
    "#]],
    );
    expr(
        "\"double\"",
        expect![[r#"
        (LITERAL "\"double\"")
    "#]],
    );
}

#[test]
fn interpolation() {
    expr(
        r#""a $b c""#,
        expect![[r#"
        (INTERPOLATED_STRING "\"" "a " (VARIABLE_EXPR "$b") " c" "\"")
    "#]],
    );
    expr(
        r#""$a[0] $a[k] $a[-1] $a[$i] $a->b $a?->b""#,
        expect![[r#"
        (INTERPOLATED_STRING "\"" (INDEX_EXPR (VARIABLE_EXPR "$a") "[" (LITERAL "0") "]") " " (INDEX_EXPR (VARIABLE_EXPR "$a") "[" (LITERAL "k") "]") " " (INDEX_EXPR (VARIABLE_EXPR "$a") "[" "-" (LITERAL "1") "]") " " (INDEX_EXPR (VARIABLE_EXPR "$a") "[" (VARIABLE_EXPR "$i") "]") " " (PROPERTY_FETCH_EXPR (VARIABLE_EXPR "$a") "->" (NAME "b")) " " (PROPERTY_FETCH_EXPR (VARIABLE_EXPR "$a") "?->" (NAME "b")) "\"")
    "#]],
    );
    expr(
        r#""{$a->b['c']->d()} ${name} ${name[1]} ${expr()}""#,
        expect![[r#"
        (INTERPOLATED_STRING "\"" (BRACED_INTERPOLATION "{" (CALL_EXPR (PROPERTY_FETCH_EXPR (INDEX_EXPR (PROPERTY_FETCH_EXPR (VARIABLE_EXPR "$a") "->" (NAME "b")) "[" (LITERAL "'c'") "]") "->" (NAME "d")) (ARGUMENT_LIST "(" ")")) "}") " " (DOLLAR_BRACE_INTERPOLATION "${" (NAME "name") "}") " " (DOLLAR_BRACE_INTERPOLATION "${" (NAME "name") "[" (LITERAL "1") "]" "}") " " (DOLLAR_BRACE_INTERPOLATION "${" (CALL_EXPR (NAME "expr") (ARGUMENT_LIST "(" ")")) "}") "\"")
    "#]],
    );
    expr(
        "`ls $dir`",
        expect![[r#"
        (SHELL_EXEC_EXPR "`" "ls " (VARIABLE_EXPR "$dir") "`")
    "#]],
    );
}

#[test]
fn heredoc_and_nowdoc() {
    check(
        "$a = <<<EOT\n  Hello $name {$x['y']}\n  EOT;\n",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (EXPR_STATEMENT (ASSIGN_EXPR (VARIABLE_EXPR "$a") "=" (HEREDOC "<<<EOT\n" "  Hello " (VARIABLE_EXPR "$name") " " (BRACED_INTERPOLATION "{" (INDEX_EXPR (VARIABLE_EXPR "$x") "[" (LITERAL "'y'") "]") "}") "\n" "  EOT")) ";"))
    "#]],
    );
    check(
        "$a = <<<'EOT'\n  raw $name\n  EOT;\n",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (EXPR_STATEMENT (ASSIGN_EXPR (VARIABLE_EXPR "$a") "=" (HEREDOC "<<<'EOT'\n" "  raw $name\n" "  EOT")) ";"))
    "#]],
    );
    check(
        "foo(<<<A\nx\nA, 2);",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (EXPR_STATEMENT (CALL_EXPR (NAME "foo") (ARGUMENT_LIST "(" (ARGUMENT (HEREDOC "<<<A\n" "x\n" "A")) "," (ARGUMENT (LITERAL "2")) ")")) ";"))
    "#]],
    );
}

#[test]
fn reserved_words_as_names() {
    check(
        "$a->list(); $a->new; Foo::class; Foo::function();",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (EXPR_STATEMENT (CALL_EXPR (PROPERTY_FETCH_EXPR (VARIABLE_EXPR "$a") "->" (NAME "list")) (ARGUMENT_LIST "(" ")")) ";")
          (EXPR_STATEMENT (PROPERTY_FETCH_EXPR (VARIABLE_EXPR "$a") "->" (NAME "new")) ";")
          (EXPR_STATEMENT (SCOPED_ACCESS_EXPR (NAME "Foo") "::" (NAME "class")) ";")
          (EXPR_STATEMENT (CALL_EXPR (SCOPED_ACCESS_EXPR (NAME "Foo") "::" (NAME "function")) (ARGUMENT_LIST "(" ")")) ";"))
    "#]],
    );
}
