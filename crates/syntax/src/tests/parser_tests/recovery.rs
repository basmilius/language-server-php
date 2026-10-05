use super::check;
use expect_test::expect;

#[test]
fn missing_semicolon_keeps_the_next_statement() {
    check(
        "$a = 1\n$b = 2;",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (EXPR_STATEMENT (ASSIGN_EXPR (VARIABLE_EXPR "$a") "=" (LITERAL "1")))
          (EXPR_STATEMENT (ASSIGN_EXPR (VARIABLE_EXPR "$b") "=" (LITERAL "2")) ";"))
        error 6..6: ';' expected
    "#]],
    );
    check(
        "echo 1\necho 2;",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (ECHO_STATEMENT "echo" (LITERAL "1"))
          (ECHO_STATEMENT "echo" (LITERAL "2") ";"))
        error 6..6: ';' expected
    "#]],
    );
}

#[test]
fn unclosed_brackets_stop_at_the_statement_end() {
    check(
        "foo(1, 2;\n$b = 2;",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (EXPR_STATEMENT (CALL_EXPR (NAME "foo") (ARGUMENT_LIST "(" (ARGUMENT (LITERAL "1")) "," (ARGUMENT (LITERAL "2")))) ";")
          (EXPR_STATEMENT (ASSIGN_EXPR (VARIABLE_EXPR "$b") "=" (LITERAL "2")) ";"))
        error 8..8: ')' expected
    "#]],
    );
    check(
        "$a = [1, 2;\n$b = 2;",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (EXPR_STATEMENT (ASSIGN_EXPR (VARIABLE_EXPR "$a") "=" (ARRAY_EXPR "[" (ARRAY_ITEM (LITERAL "1")) "," (ARRAY_ITEM (LITERAL "2")))) ";")
          (EXPR_STATEMENT (ASSIGN_EXPR (VARIABLE_EXPR "$b") "=" (LITERAL "2")) ";"))
        error 10..10: ']' expected
    "#]],
    );
    check(
        "$a = (1 + ;\n$b = 2;",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (EXPR_STATEMENT (ASSIGN_EXPR (VARIABLE_EXPR "$a") "=" (PAREN_EXPR "(" (BINARY_EXPR (LITERAL "1") "+"))) ";")
          (EXPR_STATEMENT (ASSIGN_EXPR (VARIABLE_EXPR "$b") "=" (LITERAL "2")) ";"))
        error 9..9: Expression expected
        error 9..9: ')' expected
    "#]],
    );
}

#[test]
fn missing_expression() {
    check(
        "$a = ;\n$b = 2;",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (EXPR_STATEMENT (ASSIGN_EXPR (VARIABLE_EXPR "$a") "=") ";")
          (EXPR_STATEMENT (ASSIGN_EXPR (VARIABLE_EXPR "$b") "=" (LITERAL "2")) ";"))
        error 4..4: Expression expected
    "#]],
    );
    check(
        "$a = 1 + ;\n$b = 2;",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (EXPR_STATEMENT (ASSIGN_EXPR (VARIABLE_EXPR "$a") "=" (BINARY_EXPR (LITERAL "1") "+")) ";")
          (EXPR_STATEMENT (ASSIGN_EXPR (VARIABLE_EXPR "$b") "=" (LITERAL "2")) ";"))
        error 8..8: Expression expected
    "#]],
    );
    check(
        "return $a +;",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (RETURN_STATEMENT "return" (BINARY_EXPR (VARIABLE_EXPR "$a") "+") ";"))
        error 11..11: Expression expected
    "#]],
    );
}

#[test]
fn stray_tokens() {
    check(
        ") $a = 1; ] $b = 2; }\n$c = 3;",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (ERROR ")")
          (EXPR_STATEMENT (ASSIGN_EXPR (VARIABLE_EXPR "$a") "=" (LITERAL "1")) ";")
          (ERROR "]")
          (EXPR_STATEMENT (ASSIGN_EXPR (VARIABLE_EXPR "$b") "=" (LITERAL "2")) ";")
          (ERROR "}")
          (EXPR_STATEMENT (ASSIGN_EXPR (VARIABLE_EXPR "$c") "=" (LITERAL "3")) ";"))
        error 0..1: Unexpected ')'
        error 10..11: Unexpected ']'
        error 20..21: Unexpected '}'
    "#]],
    );
    check(
        "$a = 1; endif; $b = 2;",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (EXPR_STATEMENT (ASSIGN_EXPR (VARIABLE_EXPR "$a") "=" (LITERAL "1")) ";")
          (ERROR "endif")
          (EMPTY_STATEMENT ";")
          (EXPR_STATEMENT (ASSIGN_EXPR (VARIABLE_EXPR "$b") "=" (LITERAL "2")) ";"))
        error 8..13: Unexpected 'endif'
    "#]],
    );
}

#[test]
fn unclosed_function_body_does_not_swallow_the_next_method() {
    check(
        "class A { function a() { $x = 1;\n public function b() {} }",
        expect![[r#"
            (SOURCE_FILE
              "<?php\n"
              (CLASS_DECLARATION "class" (NAME "A") (CLASS_BODY
                  "{"
                  (METHOD_DECLARATION "function" (NAME "a") (PARAMETER_LIST "(" ")") (BLOCK
                      "{"
                      (EXPR_STATEMENT (ASSIGN_EXPR (VARIABLE_EXPR "$x") "=" (LITERAL "1")) ";")))
                  (METHOD_DECLARATION (MODIFIER_LIST "public") "function" (NAME "b") (PARAMETER_LIST "(" ")") (BLOCK
                      "{"
                      "}"))
                  "}")))
            error 32..32: '}' expected
        "#]],
    );
    check(
        "class A { function a() { if ($x) { $y = 1;\n  }\n public function b() {} }",
        expect![[r#"
            (SOURCE_FILE
              "<?php\n"
              (CLASS_DECLARATION "class" (NAME "A") (CLASS_BODY
                  "{"
                  (METHOD_DECLARATION "function" (NAME "a") (PARAMETER_LIST "(" ")") (BLOCK
                      "{"
                      (IF_STATEMENT "if" "(" (VARIABLE_EXPR "$x") ")" (BLOCK
                          "{"
                          (EXPR_STATEMENT (ASSIGN_EXPR (VARIABLE_EXPR "$y") "=" (LITERAL "1")) ";")
                          "}"))))
                  (METHOD_DECLARATION (MODIFIER_LIST "public") "function" (NAME "b") (PARAMETER_LIST "(" ")") (BLOCK
                      "{"
                      "}"))
                  "}")))
            error 46..46: '}' expected
        "#]],
    );
}

#[test]
fn broken_class_members() {
    check(
        "class A { public function () {} public $b; }",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (CLASS_DECLARATION "class" (NAME "A") (CLASS_BODY
              "{"
              (METHOD_DECLARATION (MODIFIER_LIST "public") "function" (PARAMETER_LIST "(" ")") (BLOCK
                  "{"
                  "}"))
              (PROPERTY_DECLARATION (MODIFIER_LIST "public") (PROPERTY_ELEMENT "$b") ";")
              "}")))
        error 25..25: Identifier expected
    "#]],
    );
    check(
        "class A { public }",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (CLASS_DECLARATION "class" (NAME "A") (CLASS_BODY
              "{"
              (ERROR (MODIFIER_LIST "public"))
              "}")))
        error 16..16: Declaration expected
    "#]],
    );
    check(
        "class A { $x; function f() {} }",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (CLASS_DECLARATION "class" (NAME "A") (CLASS_BODY
              "{"
              (PROPERTY_DECLARATION (PROPERTY_ELEMENT "$x") ";")
              (METHOD_DECLARATION "function" (NAME "f") (PARAMETER_LIST "(" ")") (BLOCK
                  "{"
                  "}"))
              "}")))
    "#]],
    );
    check(
        "class A { function f(int $a $b) {} }",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (CLASS_DECLARATION "class" (NAME "A") (CLASS_BODY
              "{"
              (METHOD_DECLARATION "function" (NAME "f") (PARAMETER_LIST "(" (PARAMETER (NAMED_TYPE (NAME "int")) "$a") (PARAMETER "$b") ")") (BLOCK
                  "{"
                  "}"))
              "}")))
        error 27..27: ',' expected
    "#]],
    );
    check(
        "class A { garbage ; const X = 1; }",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (CLASS_DECLARATION "class" (NAME "A") (CLASS_BODY
              "{"
              (PROPERTY_DECLARATION (NAMED_TYPE (NAME "garbage")) (PROPERTY_ELEMENT) ";")
              (CLASS_CONST_DECLARATION "const" (CONST_ELEMENT (NAME "X") "=" (LITERAL "1")) ";")
              "}")))
        error 17..17: Variable expected
    "#]],
    );
}

#[test]
fn unclosed_class_ends_at_the_next_declaration() {
    check(
        "class A { function f() {}\nclass B {}",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (CLASS_DECLARATION "class" (NAME "A") (CLASS_BODY
              "{"
              (METHOD_DECLARATION "function" (NAME "f") (PARAMETER_LIST "(" ")") (BLOCK
                  "{"
                  "}"))))
          (CLASS_DECLARATION "class" (NAME "B") (CLASS_BODY
              "{"
              "}")))
        error 25..25: '}' expected
    "#]],
    );
}

#[test]
fn incomplete_constructs_at_the_end_of_the_file() {
    check(
        "class A { function f(",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (CLASS_DECLARATION "class" (NAME "A") (CLASS_BODY
              "{"
              (METHOD_DECLARATION "function" (NAME "f") (PARAMETER_LIST "(")))))
        error 21..21: ')' expected
        error 21..21: '{' or ';' expected
        error 21..21: '}' expected
    "#]],
    );
    check(
        "if ($a",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (IF_STATEMENT "if" "(" (VARIABLE_EXPR "$a")))
        error 6..6: ')' expected
        error 6..6: Statement expected
    "#]],
    );
    check(
        "$a = [1, 2",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (EXPR_STATEMENT (ASSIGN_EXPR (VARIABLE_EXPR "$a") "=" (ARRAY_EXPR "[" (ARRAY_ITEM (LITERAL "1")) "," (ARRAY_ITEM (LITERAL "2"))))))
        error 10..10: ']' expected
        error 10..10: ';' expected
    "#]],
    );
    check(
        "function f() { return",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (FUNCTION_DECLARATION "function" (NAME "f") (PARAMETER_LIST "(" ")") (BLOCK
              "{"
              (RETURN_STATEMENT "return"))))
        error 21..21: ';' expected
        error 21..21: '}' expected
    "#]],
    );
    check(
        "$a->",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (EXPR_STATEMENT (PROPERTY_FETCH_EXPR (VARIABLE_EXPR "$a") "->")))
        error 4..4: Property name expected
        error 4..4: ';' expected
    "#]],
    );
    check(
        "Foo::",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (EXPR_STATEMENT (SCOPED_ACCESS_EXPR (NAME "Foo") "::")))
        error 5..5: Member name expected
        error 5..5: ';' expected
    "#]],
    );
    check(
        "new",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (EXPR_STATEMENT (NEW_EXPR "new")))
        error 3..3: Class name expected
        error 3..3: ';' expected
    "#]],
    );
    check(
        "match ($a) { 1 =>",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (EXPR_STATEMENT (MATCH_EXPR "match" "(" (VARIABLE_EXPR "$a") ")" "{" (MATCH_ARM (LITERAL "1") "=>"))))
        error 17..17: Expression expected
        error 17..17: '}' expected
        error 17..17: ';' expected
    "#]],
    );
}

#[test]
fn reserved_word_names() {
    check(
        "function exit() {} class list {} const return = 1;",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (FUNCTION_DECLARATION "function" (NAME "exit") (PARAMETER_LIST "(" ")") (BLOCK
              "{"
              "}"))
          (CLASS_DECLARATION "class" (NAME "list") (CLASS_BODY
              "{"
              "}"))
          (CONST_STATEMENT "const" (CONST_ELEMENT (NAME "return") "=" (LITERAL "1")) ";"))
        error 9..13: Reserved word 'exit' cannot be used as a name
        error 25..29: Reserved word 'list' cannot be used as a name
        error 39..45: Reserved word 'return' cannot be used as a name
    "#]],
    );
    check(
        "trait T extends A {} interface I implements J {} enum E extends F {} class C extends A, B {}",
        expect![[r#"
            (SOURCE_FILE
              "<?php\n"
              (TRAIT_DECLARATION "trait" (NAME "T") (EXTENDS_CLAUSE "extends" (NAMED_TYPE (NAME "A"))) (CLASS_BODY
                  "{"
                  "}"))
              (INTERFACE_DECLARATION "interface" (NAME "I") (IMPLEMENTS_CLAUSE "implements" (NAMED_TYPE (NAME "J"))) (CLASS_BODY
                  "{"
                  "}"))
              (ENUM_DECLARATION "enum" (NAME "E") (EXTENDS_CLAUSE "extends" (NAMED_TYPE (NAME "F"))) (CLASS_BODY
                  "{"
                  "}"))
              (CLASS_DECLARATION "class" (NAME "C") (EXTENDS_CLAUSE "extends" (NAMED_TYPE (NAME "A")) "," (NAMED_TYPE (NAME "B"))) (CLASS_BODY
                  "{"
                  "}")))
            error 8..15: A trait cannot extend
            error 33..43: An interface cannot implement
            error 56..63: An enum cannot extend
            error 86..87: A class can only extend one class
        "#]],
    );
}

#[test]
fn syntax_that_php_rejects() {
    check(
        "$a = &new Foo; $b = &foo(); $c = &1;",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (EXPR_STATEMENT (ASSIGN_EXPR (VARIABLE_EXPR "$a") "=" "&" (NEW_EXPR "new" (NAME "Foo"))) ";")
          (EXPR_STATEMENT (ASSIGN_EXPR (VARIABLE_EXPR "$b") "=" "&" (CALL_EXPR (NAME "foo") (ARGUMENT_LIST "(" ")"))) ";")
          (EXPR_STATEMENT (ASSIGN_EXPR (VARIABLE_EXPR "$c") "=" "&" (LITERAL "1")) ";"))
        error 13..14: Only variables and calls can be assigned by reference
        error 35..36: Only variables and calls can be assigned by reference
    "#]],
    );
    check(
        "echo (void) 1; (void) foo();",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (ECHO_STATEMENT "echo" (CAST_EXPR "(void)" (LITERAL "1")) ";")
          (EXPR_STATEMENT (CAST_EXPR "(void)" (CALL_EXPR (NAME "foo") (ARGUMENT_LIST "(" ")"))) ";"))
        error 5..11: '(void)' cast is only allowed as a statement
    "#]],
    );
    check(
        "unset(foo(), $a);",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (UNSET_STATEMENT "unset" "(" (CALL_EXPR (NAME "foo") (ARGUMENT_LIST "(" ")")) "," (VARIABLE_EXPR "$a") ")" ";"))
        error 6..11: Variable expected
    "#]],
    );
    check(
        "use A\\{\\B, function C}; use function A\\{function B};",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (USE_STATEMENT "use" (USE_GROUP (NAME "A") "\\" "{" (USE_CLAUSE (NAME "\\B")) "," (USE_CLAUSE "function" (NAME "C")) "}") ";")
          (USE_STATEMENT "use" "function" (USE_GROUP (NAME "A") "\\" "{" (USE_CLAUSE "function" (NAME "B")) "}") ";"))
        error 7..9: A name in a group use cannot start with a backslash
        error 40..48: A group use cannot mix `function` or `const` with its own kind
    "#]],
    );
    check(
        "namespace \\A;",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (NAMESPACE_DECLARATION "namespace" (NAME "\\A") ";" (STATEMENT_LIST)))
        error 10..12: A namespace name cannot start with a backslash or 'namespace\'
    "#]],
    );
}

#[test]
fn string_and_literal_errors() {
    check(
        "$a = 'abc",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (EXPR_STATEMENT (ASSIGN_EXPR (VARIABLE_EXPR "$a") "=" (LITERAL "'abc"))))
        error 5..9: Unterminated string
        error 9..9: ';' expected
    "#]],
    );
    check(
        "$a = \"\\u{110000} \\u{} \\u{zz} \\u{41}\";",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (EXPR_STATEMENT (ASSIGN_EXPR (VARIABLE_EXPR "$a") "=" (LITERAL "\"\\u{110000} \\u{} \\u{zz} \\u{41}\"")) ";"))
        error 6..16: Invalid UTF-8 codepoint escape sequence: Codepoint too large
        error 17..21: Invalid UTF-8 codepoint escape sequence
        error 22..26: Invalid UTF-8 codepoint escape sequence
    "#]],
    );
    check(
        "$a = 089;",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (EXPR_STATEMENT (ASSIGN_EXPR (VARIABLE_EXPR "$a") "=" (LITERAL "089")) ";"))
        error 5..8: Invalid numeric literal
    "#]],
    );
    check(
        "$a = <<<EOT\n  ok\n bad\n  EOT;\n",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n"
          (EXPR_STATEMENT (ASSIGN_EXPR (VARIABLE_EXPR "$a") "=" (HEREDOC "<<<EOT\n" "  ok\n bad\n" "  EOT")) ";"))
        error 17..18: Invalid body indentation level (expecting an indentation level of at least 2)
    "#]],
    );
    check(
        "/* never closed",
        expect![[r#"
        (SOURCE_FILE
          "<?php\n")
        error 0..15: Unterminated comment
    "#]],
    );
}
