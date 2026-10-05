use super::check;
use expect_test::expect;

#[test]
fn echo_and_simple_statements() {
    check(
        "echo 'a', $b; ;",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (ECHO_STATEMENT "echo" (LITERAL "'a'") "," (VARIABLE_EXPR "$b") ";")
          (EMPTY_STATEMENT ";"))
    "#]],
    );
    check(
        "return; return 1; break; break 2; continue 2;",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (RETURN_STATEMENT "return" ";")
          (RETURN_STATEMENT "return" (LITERAL "1") ";")
          (BREAK_STATEMENT "break" ";")
          (BREAK_STATEMENT "break" (LITERAL "2") ";")
          (CONTINUE_STATEMENT "continue" (LITERAL "2") ";"))
    "#]],
    );
    check(
        "global $a, $$b; static $c = 1, $d; unset($a, $b['x']);",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (GLOBAL_STATEMENT "global" (VARIABLE_EXPR "$a") "," (VARIABLE_VARIABLE "$" (VARIABLE_EXPR "$b")) ";")
          (STATIC_VARIABLE_STATEMENT "static" (STATIC_VARIABLE "$c" "=" (LITERAL "1")) "," (STATIC_VARIABLE "$d") ";")
          (UNSET_STATEMENT "unset" "(" (VARIABLE_EXPR "$a") "," (INDEX_EXPR (VARIABLE_EXPR "$b") "[" (LITERAL "'x'") "]") ")" ";"))
    "#]],
    );
    check(
        "goto end; end: echo 1;",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (GOTO_STATEMENT "goto" (NAME "end") ";")
          (LABEL_STATEMENT (NAME "end") ":")
          (ECHO_STATEMENT "echo" (LITERAL "1") ";"))
    "#]],
    );
    check(
        "declare(strict_types=1);",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (DECLARE_STATEMENT "declare" "(" (DECLARE_DIRECTIVE (NAME "strict_types") "=" (LITERAL "1")) ")" ";"))
    "#]],
    );
    check(
        "declare(ticks=1) { echo 1; }",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (DECLARE_STATEMENT "declare" "(" (DECLARE_DIRECTIVE (NAME "ticks") "=" (LITERAL "1")) ")" (BLOCK
              "{"
              (ECHO_STATEMENT "echo" (LITERAL "1") ";")
              "}")))
    "#]],
    );
    check(
        "declare(ticks=1): echo 1; enddeclare;",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (DECLARE_STATEMENT "declare" "(" (DECLARE_DIRECTIVE (NAME "ticks") "=" (LITERAL "1")) ")" ":" (STATEMENT_LIST
              (ECHO_STATEMENT "echo" (LITERAL "1") ";")) "enddeclare" ";"))
    "#]],
    );
}

#[test]
fn control_flow() {
    check(
        "if ($a) { b(); } elseif ($c) d(); else if ($e) f(); else g();",
        expect![[r#"
            (SOURCE_FILE
              "<?php\n"
              (IF_STATEMENT "if" "(" (VARIABLE_EXPR "$a") ")" (BLOCK
                  "{"
                  (EXPR_STATEMENT (CALL_EXPR (NAME "b") (ARGUMENT_LIST "(" ")")) ";")
                  "}") (ELSEIF_CLAUSE "elseif" "(" (VARIABLE_EXPR "$c") ")" (EXPR_STATEMENT (CALL_EXPR (NAME "d") (ARGUMENT_LIST "(" ")")) ";")) (ELSE_CLAUSE "else" (IF_STATEMENT "if" "(" (VARIABLE_EXPR "$e") ")" (EXPR_STATEMENT (CALL_EXPR (NAME "f") (ARGUMENT_LIST "(" ")")) ";") (ELSE_CLAUSE "else" (EXPR_STATEMENT (CALL_EXPR (NAME "g") (ARGUMENT_LIST "(" ")")) ";"))))))
        "#]],
    );
    check(
        "while ($a) $b++;",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (WHILE_STATEMENT "while" "(" (VARIABLE_EXPR "$a") ")" (EXPR_STATEMENT (POSTFIX_EXPR (VARIABLE_EXPR "$b") "++") ";")))
    "#]],
    );
    check(
        "do { $a++; } while ($a < 3);",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (DO_WHILE_STATEMENT "do" (BLOCK
              "{"
              (EXPR_STATEMENT (POSTFIX_EXPR (VARIABLE_EXPR "$a") "++") ";")
              "}") "while" "(" (BINARY_EXPR (VARIABLE_EXPR "$a") "<" (LITERAL "3")) ")" ";"))
    "#]],
    );
    check(
        "for ($i = 0, $j = 1; $i < 3; $i++, $j--) {}",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (FOR_STATEMENT "for" "(" (ASSIGN_EXPR (VARIABLE_EXPR "$i") "=" (LITERAL "0")) "," (ASSIGN_EXPR (VARIABLE_EXPR "$j") "=" (LITERAL "1")) ";" (BINARY_EXPR (VARIABLE_EXPR "$i") "<" (LITERAL "3")) ";" (POSTFIX_EXPR (VARIABLE_EXPR "$i") "++") "," (POSTFIX_EXPR (VARIABLE_EXPR "$j") "--") ")" (BLOCK
              "{"
              "}")))
    "#]],
    );
    check(
        "for (;;) {}",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (FOR_STATEMENT "for" "(" ";" ";" ")" (BLOCK
              "{"
              "}")))
    "#]],
    );
    check(
        "foreach ($a as $k => &$v) {}",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (FOREACH_STATEMENT "foreach" "(" (VARIABLE_EXPR "$a") "as" (VARIABLE_EXPR "$k") "=>" "&" (VARIABLE_EXPR "$v") ")" (BLOCK
              "{"
              "}")))
    "#]],
    );
    check(
        "foreach ($a as [$x, $y]) {}",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (FOREACH_STATEMENT "foreach" "(" (VARIABLE_EXPR "$a") "as" (ARRAY_EXPR "[" (ARRAY_ITEM (VARIABLE_EXPR "$x")) "," (ARRAY_ITEM (VARIABLE_EXPR "$y")) "]") ")" (BLOCK
              "{"
              "}")))
    "#]],
    );
    check(
        "foreach ($a as ['k' => $x]) {}",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (FOREACH_STATEMENT "foreach" "(" (VARIABLE_EXPR "$a") "as" (ARRAY_EXPR "[" (ARRAY_ITEM (LITERAL "'k'") "=>" (VARIABLE_EXPR "$x")) "]") ")" (BLOCK
              "{"
              "}")))
    "#]],
    );
    check(
        "switch ($a) { case 1: case 2; echo 1; break; default: echo 2; }",
        expect![[r#"
            (SOURCE_FILE
              "<?php\n"
              (SWITCH_STATEMENT "switch" "(" (VARIABLE_EXPR "$a") ")" "{" (CASE_CLAUSE "case" (LITERAL "1") ":") (CASE_CLAUSE "case" (LITERAL "2") ";" (ECHO_STATEMENT "echo" (LITERAL "1") ";") (BREAK_STATEMENT "break" ";")) (DEFAULT_CLAUSE "default" ":" (ECHO_STATEMENT "echo" (LITERAL "2") ";")) "}"))
        "#]],
    );
    check(
        "try { a(); } catch (A | B $e) { b(); } catch (C) {} finally { c(); }",
        expect![[r#"
            (SOURCE_FILE
              "<?php\n"
              (TRY_STATEMENT "try" (BLOCK
                  "{"
                  (EXPR_STATEMENT (CALL_EXPR (NAME "a") (ARGUMENT_LIST "(" ")")) ";")
                  "}") (CATCH_CLAUSE "catch" "(" (UNION_TYPE (NAMED_TYPE (NAME "A")) "|" (NAMED_TYPE (NAME "B"))) "$e" ")" (BLOCK
                    "{"
                    (EXPR_STATEMENT (CALL_EXPR (NAME "b") (ARGUMENT_LIST "(" ")")) ";")
                    "}")) (CATCH_CLAUSE "catch" "(" (NAMED_TYPE (NAME "C")) ")" (BLOCK
                    "{"
                    "}")) (FINALLY_CLAUSE "finally" (BLOCK
                    "{"
                    (EXPR_STATEMENT (CALL_EXPR (NAME "c") (ARGUMENT_LIST "(" ")")) ";")
                    "}"))))
        "#]],
    );
}

#[test]
fn alternative_syntax() {
    check(
        "if ($a): echo 1; elseif ($b): echo 2; else: echo 3; endif;",
        expect![[r#"
            (SOURCE_FILE
              "<?php\n"
              (IF_STATEMENT "if" "(" (VARIABLE_EXPR "$a") ")" ":" (STATEMENT_LIST
                  (ECHO_STATEMENT "echo" (LITERAL "1") ";")) (ELSEIF_CLAUSE "elseif" "(" (VARIABLE_EXPR "$b") ")" ":" (STATEMENT_LIST
                    (ECHO_STATEMENT "echo" (LITERAL "2") ";"))) (ELSE_CLAUSE "else" ":" (STATEMENT_LIST
                    (ECHO_STATEMENT "echo" (LITERAL "3") ";"))) "endif" ";"))
        "#]],
    );
    check(
        "while ($a): $b++; endwhile;",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (WHILE_STATEMENT "while" "(" (VARIABLE_EXPR "$a") ")" ":" (STATEMENT_LIST
              (EXPR_STATEMENT (POSTFIX_EXPR (VARIABLE_EXPR "$b") "++") ";")) "endwhile" ";"))
    "#]],
    );
    check(
        "for ($i = 0; $i < 3; $i++): echo $i; endfor;",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (FOR_STATEMENT "for" "(" (ASSIGN_EXPR (VARIABLE_EXPR "$i") "=" (LITERAL "0")) ";" (BINARY_EXPR (VARIABLE_EXPR "$i") "<" (LITERAL "3")) ";" (POSTFIX_EXPR (VARIABLE_EXPR "$i") "++") ")" ":" (STATEMENT_LIST
              (ECHO_STATEMENT "echo" (VARIABLE_EXPR "$i") ";")) "endfor" ";"))
    "#]],
    );
    check(
        "foreach ($a as $b): echo $b; endforeach;",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (FOREACH_STATEMENT "foreach" "(" (VARIABLE_EXPR "$a") "as" (VARIABLE_EXPR "$b") ")" ":" (STATEMENT_LIST
              (ECHO_STATEMENT "echo" (VARIABLE_EXPR "$b") ";")) "endforeach" ";"))
    "#]],
    );
    check(
        "switch ($a): case 1: echo 1; endswitch;",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (SWITCH_STATEMENT "switch" "(" (VARIABLE_EXPR "$a") ")" ":" (CASE_CLAUSE "case" (LITERAL "1") ":" (ECHO_STATEMENT "echo" (LITERAL "1") ";")) "endswitch" ";"))
    "#]],
    );
}

#[test]
fn inline_html_and_tags() {
    let text = "?>\n<ul><?php foreach ($a as $b): ?>\n<li><?= $b ?></li>\n<?php endforeach; ?>\n</ul>\n";
    check(
        text,
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          "?>\n"
          "<ul>"
          "<?php "
          (FOREACH_STATEMENT "foreach" "(" (VARIABLE_EXPR "$a") "as" (VARIABLE_EXPR "$b") ")" ":" (STATEMENT_LIST
              "?>\n"
              "<li>"
              (ECHO_STATEMENT "<?=" (VARIABLE_EXPR "$b") "?>")
              "</li>\n"
              "<?php ") "endforeach" ";")
          "?>\n"
          "</ul>\n")
    "#]],
    );
}

#[test]
fn namespaces_and_use() {
    check(
        "namespace A\\B; use C\\D; echo 1;",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (NAMESPACE_DECLARATION "namespace" (NAME "A\\B") ";" (STATEMENT_LIST
              (USE_STATEMENT "use" (USE_CLAUSE (NAME "C\\D")) ";")
              (ECHO_STATEMENT "echo" (LITERAL "1") ";"))))
    "#]],
    );
    check(
        "namespace A { class B {} } namespace { class C {} }",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (NAMESPACE_DECLARATION "namespace" (NAME "A") (BLOCK
              "{"
              (CLASS_DECLARATION "class" (NAME "B") (CLASS_BODY
                  "{"
                  "}"))
              "}"))
          (NAMESPACE_DECLARATION "namespace" (BLOCK
              "{"
              (CLASS_DECLARATION "class" (NAME "C") (CLASS_BODY
                  "{"
                  "}"))
              "}")))
    "#]],
    );
    check(
        "use A\\{B, C as D, function e, const F};",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (USE_STATEMENT "use" (USE_GROUP (NAME "A") "\\" "{" (USE_CLAUSE (NAME "B")) "," (USE_CLAUSE (NAME "C") "as" (NAME "D")) "," (USE_CLAUSE "function" (NAME "e")) "," (USE_CLAUSE "const" (NAME "F")) "}") ";"))
    "#]],
    );
    check(
        "use function A\\f, A\\g; use const A\\C as K;",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (USE_STATEMENT "use" "function" (USE_CLAUSE (NAME "A\\f")) "," (USE_CLAUSE (NAME "A\\g")) ";")
          (USE_STATEMENT "use" "const" (USE_CLAUSE (NAME "A\\C") "as" (NAME "K")) ";"))
    "#]],
    );
}

#[test]
fn functions() {
    check(
        "function f(int|string $a, ?A $b = null, A&B $c, (A&B)|null $d, &$e, ...$rest): static|false {}",
        expect![[r#"
            (SOURCE_FILE
              "<?php\n"
              (FUNCTION_DECLARATION "function" (NAME "f") (PARAMETER_LIST "(" (PARAMETER (UNION_TYPE (NAMED_TYPE (NAME "int")) "|" (NAMED_TYPE (NAME "string"))) "$a") "," (PARAMETER (NULLABLE_TYPE "?" (NAMED_TYPE (NAME "A"))) "$b" "=" (NAME "null")) "," (PARAMETER (INTERSECTION_TYPE (NAMED_TYPE (NAME "A")) "&" (NAMED_TYPE (NAME "B"))) "$c") "," (PARAMETER (UNION_TYPE (PAREN_TYPE "(" (NAMED_TYPE (NAME "A")) "&" (NAMED_TYPE (NAME "B")) ")") "|" (NAMED_TYPE (NAME "null"))) "$d") "," (PARAMETER "&" "$e") "," (PARAMETER "..." "$rest") ")") (RETURN_TYPE ":" (UNION_TYPE (NAMED_TYPE (NAME "static")) "|" (NAMED_TYPE (NAME "false")))) (BLOCK
                  "{"
                  "}")))
        "#]],
    );
    check(
        "function &ref(): iterable {}",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (FUNCTION_DECLARATION "function" "&" (NAME "ref") (PARAMETER_LIST "(" ")") (RETURN_TYPE ":" (NAMED_TYPE (NAME "iterable"))) (BLOCK
              "{"
              "}")))
    "#]],
    );
    check(
        "#[A, B(1)] #[C] function g() {}",
        expect![[r##"
        (SOURCE_FILE
          "<?php\n"
          (FUNCTION_DECLARATION (ATTRIBUTE_LIST "#[" (ATTRIBUTE (NAME "A")) "," (ATTRIBUTE (NAME "B") (ARGUMENT_LIST "(" (ARGUMENT (LITERAL "1")) ")")) "]") (ATTRIBUTE_LIST "#[" (ATTRIBUTE (NAME "C")) "]") "function" (NAME "g") (PARAMETER_LIST "(" ")") (BLOCK
              "{"
              "}")))
    "##]],
    );
    check(
        "function f(#[SensitiveParameter] $secret, $a = new Foo(1)) {}",
        expect![[r##"
            (SOURCE_FILE
              "<?php\n"
              (FUNCTION_DECLARATION "function" (NAME "f") (PARAMETER_LIST "(" (PARAMETER (ATTRIBUTE_LIST "#[" (ATTRIBUTE (NAME "SensitiveParameter")) "]") "$secret") "," (PARAMETER "$a" "=" (NEW_EXPR "new" (NAME "Foo") (ARGUMENT_LIST "(" (ARGUMENT (LITERAL "1")) ")"))) ")") (BLOCK
                  "{"
                  "}")))
        "##]],
    );
    check(
        "const A = 1, B = 'x';",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (CONST_STATEMENT "const" (CONST_ELEMENT (NAME "A") "=" (LITERAL "1")) "," (CONST_ELEMENT (NAME "B") "=" (LITERAL "'x'")) ";"))
    "#]],
    );
    check(
        "#[Attr] const C = 1;",
        expect![[r##"
        (SOURCE_FILE
          "<?php\n"
          (CONST_STATEMENT (ATTRIBUTE_LIST "#[" (ATTRIBUTE (NAME "Attr")) "]") "const" (CONST_ELEMENT (NAME "C") "=" (LITERAL "1")) ";"))
    "##]],
    );
}

#[test]
fn classes() {
    check(
        "abstract class A extends B implements C, \\D\\E { abstract protected function f(): void; }",
        expect![[r#"
            (SOURCE_FILE
              "<?php\n"
              (CLASS_DECLARATION (MODIFIER_LIST "abstract") "class" (NAME "A") (EXTENDS_CLAUSE "extends" (NAMED_TYPE (NAME "B"))) (IMPLEMENTS_CLAUSE "implements" (NAMED_TYPE (NAME "C")) "," (NAMED_TYPE (NAME "\\D\\E"))) (CLASS_BODY
                  "{"
                  (METHOD_DECLARATION (MODIFIER_LIST "abstract" "protected") "function" (NAME "f") (PARAMETER_LIST "(" ")") (RETURN_TYPE ":" (NAMED_TYPE (NAME "void"))) ";")
                  "}")))
        "#]],
    );
    check(
        "final readonly class P { public function __construct(public int $x, private readonly string $y = 'a') {} }",
        expect![[r#"
            (SOURCE_FILE
              "<?php\n"
              (CLASS_DECLARATION (MODIFIER_LIST "final" "readonly") "class" (NAME "P") (CLASS_BODY
                  "{"
                  (METHOD_DECLARATION (MODIFIER_LIST "public") "function" (NAME "__construct") (PARAMETER_LIST "(" (PARAMETER (MODIFIER_LIST "public") (NAMED_TYPE (NAME "int")) "$x") "," (PARAMETER (MODIFIER_LIST "private" "readonly") (NAMED_TYPE (NAME "string")) "$y" "=" (LITERAL "'a'")) ")") (BLOCK
                      "{"
                      "}"))
                  "}")))
        "#]],
    );
    check(
        "class K { const A = 1, B = 2; public const int C = 3; final protected const D = 4; }",
        expect![[r#"
            (SOURCE_FILE
              "<?php\n"
              (CLASS_DECLARATION "class" (NAME "K") (CLASS_BODY
                  "{"
                  (CLASS_CONST_DECLARATION "const" (CONST_ELEMENT (NAME "A") "=" (LITERAL "1")) "," (CONST_ELEMENT (NAME "B") "=" (LITERAL "2")) ";")
                  (CLASS_CONST_DECLARATION (MODIFIER_LIST "public") "const" (NAMED_TYPE (NAME "int")) (CONST_ELEMENT (NAME "C") "=" (LITERAL "3")) ";")
                  (CLASS_CONST_DECLARATION (MODIFIER_LIST "final" "protected") "const" (CONST_ELEMENT (NAME "D") "=" (LITERAL "4")) ";")
                  "}")))
        "#]],
    );
    check(
        "class V { var $a; public ?int $b = null; static public array $c = []; public int|string $d, $e; }",
        expect![[r#"
            (SOURCE_FILE
              "<?php\n"
              (CLASS_DECLARATION "class" (NAME "V") (CLASS_BODY
                  "{"
                  (PROPERTY_DECLARATION (MODIFIER_LIST "var") (PROPERTY_ELEMENT "$a") ";")
                  (PROPERTY_DECLARATION (MODIFIER_LIST "public") (NULLABLE_TYPE "?" (NAMED_TYPE (NAME "int"))) (PROPERTY_ELEMENT "$b" "=" (NAME "null")) ";")
                  (PROPERTY_DECLARATION (MODIFIER_LIST "static" "public") (NAMED_TYPE (NAME "array")) (PROPERTY_ELEMENT "$c" "=" (ARRAY_EXPR "[" "]")) ";")
                  (PROPERTY_DECLARATION (MODIFIER_LIST "public") (UNION_TYPE (NAMED_TYPE (NAME "int")) "|" (NAMED_TYPE (NAME "string"))) (PROPERTY_ELEMENT "$d") "," (PROPERTY_ELEMENT "$e") ";")
                  "}")))
        "#]],
    );
    check(
        "class T { use A, B { A::f insteadof B; B::f as protected g; h as private; } }",
        expect![[r#"
            (SOURCE_FILE
              "<?php\n"
              (CLASS_DECLARATION "class" (NAME "T") (CLASS_BODY
                  "{"
                  (TRAIT_USE "use" (NAME "A") "," (NAME "B") (TRAIT_ADAPTATIONS
                      "{"
                      (TRAIT_PRECEDENCE (NAME "A") "::" (NAME "f") "insteadof" (NAME "B") ";")
                      (TRAIT_ALIAS (NAME "B") "::" (NAME "f") "as" (MODIFIER_LIST "protected") (NAME "g") ";")
                      (TRAIT_ALIAS (NAME "h") "as" (MODIFIER_LIST "private") ";")
                      "}"))
                  "}")))
        "#]],
    );
    check(
        "class M { public static function make(): static { return new static; } function list() {} function class() {} }",
        expect![[r#"
            (SOURCE_FILE
              "<?php\n"
              (CLASS_DECLARATION "class" (NAME "M") (CLASS_BODY
                  "{"
                  (METHOD_DECLARATION (MODIFIER_LIST "public" "static") "function" (NAME "make") (PARAMETER_LIST "(" ")") (RETURN_TYPE ":" (NAMED_TYPE (NAME "static"))) (BLOCK
                      "{"
                      (RETURN_STATEMENT "return" (NEW_EXPR "new" (NAME "static")) ";")
                      "}"))
                  (METHOD_DECLARATION "function" (NAME "list") (PARAMETER_LIST "(" ")") (BLOCK
                      "{"
                      "}"))
                  (METHOD_DECLARATION "function" (NAME "class") (PARAMETER_LIST "(" ")") (BLOCK
                      "{"
                      "}"))
                  "}")))
        "#]],
    );
    check(
        "/** Doc */ #[A] class D {}",
        expect![[r##"
        (SOURCE_FILE
          "<?php\n"
          (CLASS_DECLARATION (ATTRIBUTE_LIST "#[" (ATTRIBUTE (NAME "A")) "]") "class" (NAME "D") (CLASS_BODY
              "{"
              "}")))
    "##]],
    );
}

#[test]
fn interfaces_traits_and_enums() {
    check(
        "interface I extends J, K { const X = 1; public function f(int $a): string; }",
        expect![[r#"
            (SOURCE_FILE
              "<?php\n"
              (INTERFACE_DECLARATION "interface" (NAME "I") (EXTENDS_CLAUSE "extends" (NAMED_TYPE (NAME "J")) "," (NAMED_TYPE (NAME "K"))) (CLASS_BODY
                  "{"
                  (CLASS_CONST_DECLARATION "const" (CONST_ELEMENT (NAME "X") "=" (LITERAL "1")) ";")
                  (METHOD_DECLARATION (MODIFIER_LIST "public") "function" (NAME "f") (PARAMETER_LIST "(" (PARAMETER (NAMED_TYPE (NAME "int")) "$a") ")") (RETURN_TYPE ":" (NAMED_TYPE (NAME "string"))) ";")
                  "}")))
        "#]],
    );
    check(
        "trait T { public $a; abstract public function f(); public static function g() {} }",
        expect![[r#"
            (SOURCE_FILE
              "<?php\n"
              (TRAIT_DECLARATION "trait" (NAME "T") (CLASS_BODY
                  "{"
                  (PROPERTY_DECLARATION (MODIFIER_LIST "public") (PROPERTY_ELEMENT "$a") ";")
                  (METHOD_DECLARATION (MODIFIER_LIST "abstract" "public") "function" (NAME "f") (PARAMETER_LIST "(" ")") ";")
                  (METHOD_DECLARATION (MODIFIER_LIST "public" "static") "function" (NAME "g") (PARAMETER_LIST "(" ")") (BLOCK
                      "{"
                      "}"))
                  "}")))
        "#]],
    );
    check(
        "enum Suit { case Hearts; case Spades; public function label(): string { return 'x'; } const D = self::Hearts; }",
        expect![[r#"
            (SOURCE_FILE
              "<?php\n"
              (ENUM_DECLARATION "enum" (NAME "Suit") (CLASS_BODY
                  "{"
                  (ENUM_CASE "case" (NAME "Hearts") ";")
                  (ENUM_CASE "case" (NAME "Spades") ";")
                  (METHOD_DECLARATION (MODIFIER_LIST "public") "function" (NAME "label") (PARAMETER_LIST "(" ")") (RETURN_TYPE ":" (NAMED_TYPE (NAME "string"))) (BLOCK
                      "{"
                      (RETURN_STATEMENT "return" (LITERAL "'x'") ";")
                      "}"))
                  (CLASS_CONST_DECLARATION "const" (CONST_ELEMENT (NAME "D") "=" (SCOPED_ACCESS_EXPR (NAME "self") "::" (NAME "Hearts"))) ";")
                  "}")))
        "#]],
    );
    check(
        "enum Status: string implements HasLabel { use Labels; case Active = 'a'; case Off = 'o'; }",
        expect![[r#"
            (SOURCE_FILE
              "<?php\n"
              (ENUM_DECLARATION "enum" (NAME "Status") (ENUM_BACKING_TYPE ":" (NAMED_TYPE (NAME "string"))) (IMPLEMENTS_CLAUSE "implements" (NAMED_TYPE (NAME "HasLabel"))) (CLASS_BODY
                  "{"
                  (TRAIT_USE "use" (NAME "Labels") ";")
                  (ENUM_CASE "case" (NAME "Active") "=" (LITERAL "'a'") ";")
                  (ENUM_CASE "case" (NAME "Off") "=" (LITERAL "'o'") ";")
                  "}")))
        "#]],
    );
    check(
        "enum E: int { case A = 1 << 0; }",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (ENUM_DECLARATION "enum" (NAME "E") (ENUM_BACKING_TYPE ":" (NAMED_TYPE (NAME "int"))) (CLASS_BODY
              "{"
              (ENUM_CASE "case" (NAME "A") "=" (BINARY_EXPR (LITERAL "1") "<<" (LITERAL "0")) ";")
              "}")))
    "#]],
    );
}

#[test]
fn property_hooks_and_asymmetric_visibility() {
    check(
        "class P { public string $name { get => $this->name; set(string $v) { $this->name = $v; } } }",
        expect![[r#"
            (SOURCE_FILE
              "<?php\n"
              (CLASS_DECLARATION "class" (NAME "P") (CLASS_BODY
                  "{"
                  (PROPERTY_DECLARATION (MODIFIER_LIST "public") (NAMED_TYPE (NAME "string")) (PROPERTY_ELEMENT "$name") (PROPERTY_HOOK_LIST
                      "{"
                      (PROPERTY_HOOK (NAME "get") "=>" (PROPERTY_FETCH_EXPR (VARIABLE_EXPR "$this") "->" (NAME "name")) ";")
                      (PROPERTY_HOOK (NAME "set") (PARAMETER_LIST "(" (PARAMETER (NAMED_TYPE (NAME "string")) "$v") ")") (BLOCK
                          "{"
                          (EXPR_STATEMENT (ASSIGN_EXPR (PROPERTY_FETCH_EXPR (VARIABLE_EXPR "$this") "->" (NAME "name")) "=" (VARIABLE_EXPR "$v")) ";")
                          "}"))
                      "}"))
                  "}")))
        "#]],
    );
    check(
        "class P { public int $x { get; set; } public ?int $y = 1 { final get => 1; &get { return $this->y; } } }",
        expect![[r#"
            (SOURCE_FILE
              "<?php\n"
              (CLASS_DECLARATION "class" (NAME "P") (CLASS_BODY
                  "{"
                  (PROPERTY_DECLARATION (MODIFIER_LIST "public") (NAMED_TYPE (NAME "int")) (PROPERTY_ELEMENT "$x") (PROPERTY_HOOK_LIST
                      "{"
                      (PROPERTY_HOOK (NAME "get") ";")
                      (PROPERTY_HOOK (NAME "set") ";")
                      "}"))
                  (PROPERTY_DECLARATION (MODIFIER_LIST "public") (NULLABLE_TYPE "?" (NAMED_TYPE (NAME "int"))) (PROPERTY_ELEMENT "$y" "=" (LITERAL "1")) (PROPERTY_HOOK_LIST
                      "{"
                      (PROPERTY_HOOK (MODIFIER_LIST "final") (NAME "get") "=>" (LITERAL "1") ";")
                      (PROPERTY_HOOK "&" (NAME "get") (BLOCK
                          "{"
                          (RETURN_STATEMENT "return" (PROPERTY_FETCH_EXPR (VARIABLE_EXPR "$this") "->" (NAME "y")) ";")
                          "}"))
                      "}"))
                  "}")))
        "#]],
    );
    check(
        "class P { public private(set) int $a = 0; protected(set) readonly int $b; public public(set) static int $c = 0; }",
        expect![[r#"
            (SOURCE_FILE
              "<?php\n"
              (CLASS_DECLARATION "class" (NAME "P") (CLASS_BODY
                  "{"
                  (PROPERTY_DECLARATION (MODIFIER_LIST "public" "private" "(" "set" ")") (NAMED_TYPE (NAME "int")) (PROPERTY_ELEMENT "$a" "=" (LITERAL "0")) ";")
                  (PROPERTY_DECLARATION (MODIFIER_LIST "protected" "(" "set" ")" "readonly") (NAMED_TYPE (NAME "int")) (PROPERTY_ELEMENT "$b") ";")
                  (PROPERTY_DECLARATION (MODIFIER_LIST "public" "public" "(" "set" ")" "static") (NAMED_TYPE (NAME "int")) (PROPERTY_ELEMENT "$c" "=" (LITERAL "0")) ";")
                  "}")))
        "#]],
    );
    check(
        "class P { public function __construct(public protected(set) string $a, final public int $b { get => 1; }) {} }",
        expect![[r#"
            (SOURCE_FILE
              "<?php\n"
              (CLASS_DECLARATION "class" (NAME "P") (CLASS_BODY
                  "{"
                  (METHOD_DECLARATION (MODIFIER_LIST "public") "function" (NAME "__construct") (PARAMETER_LIST "(" (PARAMETER (MODIFIER_LIST "public" "protected" "(" "set" ")") (NAMED_TYPE (NAME "string")) "$a") "," (PARAMETER (MODIFIER_LIST "final" "public") (NAMED_TYPE (NAME "int")) "$b" (PROPERTY_HOOK_LIST
                          "{"
                          (PROPERTY_HOOK (NAME "get") "=>" (LITERAL "1") ";")
                          "}")) ")") (BLOCK
                      "{"
                      "}"))
                  "}")))
        "#]],
    );
    check(
        "class P { public int $a { #[A] set(int $v) => $this->a = $v; } }",
        expect![[r##"
            (SOURCE_FILE
              "<?php\n"
              (CLASS_DECLARATION "class" (NAME "P") (CLASS_BODY
                  "{"
                  (PROPERTY_DECLARATION (MODIFIER_LIST "public") (NAMED_TYPE (NAME "int")) (PROPERTY_ELEMENT "$a") (PROPERTY_HOOK_LIST
                      "{"
                      (PROPERTY_HOOK (ATTRIBUTE_LIST "#[" (ATTRIBUTE (NAME "A")) "]") (NAME "set") (PARAMETER_LIST "(" (PARAMETER (NAMED_TYPE (NAME "int")) "$v") ")") "=>" (ASSIGN_EXPR (PROPERTY_FETCH_EXPR (VARIABLE_EXPR "$this") "->" (NAME "a")) "=" (VARIABLE_EXPR "$v")) ";")
                      "}"))
                  "}")))
        "##]],
    );
}

#[test]
fn halt_compiler() {
    check(
        "echo 1; __halt_compiler(); raw data here",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (ECHO_STATEMENT "echo" (LITERAL "1") ";")
          (HALT_COMPILER_STATEMENT "__halt_compiler" "(" ")" ";" " raw data here"))
    "#]],
    );
}

#[test]
fn doc_comments_belong_to_declarations() {
    use crate::{DumpOptions, dump, parse};
    let parsed =
        parse("<?php\n/** A */\nclass A {\n    /** m */\n    public function m() {}\n}\n\n/** x */\n$x = 1;\n");
    let tree = dump(
        &parsed.syntax(),
        DumpOptions {
            trivia: true,
            ranges: true,
        },
    );
    expect![[r#"
        SOURCE_FILE@0..85
          OPEN_TAG@0..6 "<?php\n"
          CLASS_DECLARATION@6..66
            DOC_COMMENT@6..14 "/** A */"
            WHITESPACE@14..15 "\n"
            CLASS_KW@15..20 "class"
            WHITESPACE@20..21 " "
            NAME@21..22
              IDENT@21..22 "A"
            WHITESPACE@22..23 " "
            CLASS_BODY@23..66
              LBRACE@23..24 "{"
              WHITESPACE@24..29 "\n    "
              METHOD_DECLARATION@29..64
                DOC_COMMENT@29..37 "/** m */"
                WHITESPACE@37..42 "\n    "
                MODIFIER_LIST@42..48
                  PUBLIC_KW@42..48 "public"
                WHITESPACE@48..49 " "
                FUNCTION_KW@49..57 "function"
                WHITESPACE@57..58 " "
                NAME@58..59
                  IDENT@58..59 "m"
                PARAMETER_LIST@59..61
                  LPAREN@59..60 "("
                  RPAREN@60..61 ")"
                WHITESPACE@61..62 " "
                BLOCK@62..64
                  LBRACE@62..63 "{"
                  RBRACE@63..64 "}"
              WHITESPACE@64..65 "\n"
              RBRACE@65..66 "}"
          WHITESPACE@66..68 "\n\n"
          DOC_COMMENT@68..76 "/** x */"
          WHITESPACE@76..77 "\n"
          EXPR_STATEMENT@77..84
            ASSIGN_EXPR@77..83
              VARIABLE_EXPR@77..79
                VARIABLE@77..79 "$x"
              WHITESPACE@79..80 " "
              ASSIGN@80..81 "="
              WHITESPACE@81..82 " "
              LITERAL@82..83
                INT_LITERAL@82..83 "1"
            SEMICOLON@83..84 ";"
          WHITESPACE@84..85 "\n"
    "#]]
    .assert_eq(&tree);
}

/// The first declaration after `namespace X;`, or inside a braced one or an alternative syntax
/// body, keeps its doc comment inside itself like any other declaration.
#[test]
fn the_first_declaration_of_a_namespace_owns_its_doc_comment() {
    use crate::{SyntaxKind, parse};
    for text in [
        "<?php\nnamespace A;\n\n/** doc */\nclass C {}\n",
        "<?php\nnamespace A {\n    /** doc */\n    class C {}\n}\n",
        "<?php\nif ($a):\n    /** doc */\n    function f() {}\nendif;\n",
    ] {
        let parsed = parse(text);
        let declaration = parsed
            .syntax()
            .descendants()
            .find(|node| {
                matches!(
                    node.kind(),
                    SyntaxKind::CLASS_DECLARATION | SyntaxKind::FUNCTION_DECLARATION
                )
            })
            .expect("a declaration");
        let first = declaration.first_token().expect("a token");
        assert_eq!(first.kind(), SyntaxKind::DOC_COMMENT, "{text}");
        assert_eq!(parsed.syntax().text().to_string(), text);
    }
}
