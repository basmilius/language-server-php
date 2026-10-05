use crate::lexer::{lex, lex_from, lex_with_checkpoints};
use expect_test::{Expect, expect};

fn tokens(text: &str) -> String {
    let lexed = lex(text);
    let mut out = String::new();
    let mut offset = 0usize;
    let mut total = 0usize;
    for token in &lexed.tokens {
        let slice = &text[offset..offset + token.len as usize];
        out.push_str(&format!("{} {:?}\n", token.kind.name(), slice));
        offset += token.len as usize;
        total += token.len as usize;
    }
    assert_eq!(total, text.len(), "tokens must cover every byte");
    for error in &lexed.errors {
        out.push_str(&format!("error {}..{} {}\n", error.start, error.end, error.message));
    }
    out
}

fn check(text: &str, expect: Expect) {
    expect.assert_eq(&tokens(text));
}

#[test]
fn inline_html_and_tags() {
    check(
        "<b>hi</b>\n<?php echo 1 ?>\nx<?= $a ?>",
        expect![[r#"
        INLINE_HTML "<b>hi</b>\n"
        OPEN_TAG "<?php "
        ECHO_KW "echo"
        WHITESPACE " "
        INT_LITERAL "1"
        WHITESPACE " "
        CLOSE_TAG "?>\n"
        INLINE_HTML "x"
        OPEN_TAG_ECHO "<?="
        WHITESPACE " "
        VARIABLE "$a"
        WHITESPACE " "
        CLOSE_TAG "?>"
    "#]],
    );
}

#[test]
fn numbers() {
    check(
        "<?php 1 0x1F 0b101 0o17 017 1_000 1.5 .5 1e3 1.5E-3 1. 0x",
        expect![[r#"
        OPEN_TAG "<?php "
        INT_LITERAL "1"
        WHITESPACE " "
        INT_LITERAL "0x1F"
        WHITESPACE " "
        INT_LITERAL "0b101"
        WHITESPACE " "
        INT_LITERAL "0o17"
        WHITESPACE " "
        INT_LITERAL "017"
        WHITESPACE " "
        INT_LITERAL "1_000"
        WHITESPACE " "
        FLOAT_LITERAL "1.5"
        WHITESPACE " "
        FLOAT_LITERAL ".5"
        WHITESPACE " "
        FLOAT_LITERAL "1e3"
        WHITESPACE " "
        FLOAT_LITERAL "1.5E-3"
        WHITESPACE " "
        FLOAT_LITERAL "1."
        WHITESPACE " "
        INT_LITERAL "0"
        IDENT "x"
    "#]],
    );
}

#[test]
fn strings_and_interpolation() {
    check(
        r#"<?php 'a\'b' "plain" "a $b c" "x {$y->z} ${w} $a[1] $a[k] $a->b $a?->b" b'x'"#,
        expect![[r#"
        OPEN_TAG "<?php "
        STRING_LITERAL "'a\\'b'"
        WHITESPACE " "
        STRING_LITERAL "\"plain\""
        WHITESPACE " "
        DOUBLE_QUOTE "\""
        STRING_CONTENT "a "
        VARIABLE "$b"
        STRING_CONTENT " c"
        DOUBLE_QUOTE "\""
        WHITESPACE " "
        DOUBLE_QUOTE "\""
        STRING_CONTENT "x "
        CURLY_OPEN "{"
        VARIABLE "$y"
        ARROW "->"
        IDENT "z"
        RBRACE "}"
        STRING_CONTENT " "
        DOLLAR_OPEN_CURLY "${"
        IDENT "w"
        RBRACE "}"
        STRING_CONTENT " "
        VARIABLE "$a"
        LBRACKET "["
        INT_LITERAL "1"
        RBRACKET "]"
        STRING_CONTENT " "
        VARIABLE "$a"
        LBRACKET "["
        IDENT "k"
        RBRACKET "]"
        STRING_CONTENT " "
        VARIABLE "$a"
        ARROW "->"
        IDENT "b"
        STRING_CONTENT " "
        VARIABLE "$a"
        NULLSAFE_ARROW "?->"
        IDENT "b"
        DOUBLE_QUOTE "\""
        WHITESPACE " "
        STRING_LITERAL "b'x'"
    "#]],
    );
}

#[test]
fn heredoc_and_nowdoc() {
    check(
        "<?php $a = <<<EOT\n  hi $x\n  EOT;\n$b = <<<'RAW'\n$no\nRAW;\n$c = <<<\"Q\"\nq\nQ",
        expect![[r#"
        OPEN_TAG "<?php "
        VARIABLE "$a"
        WHITESPACE " "
        ASSIGN "="
        WHITESPACE " "
        HEREDOC_START "<<<EOT\n"
        STRING_CONTENT "  hi "
        VARIABLE "$x"
        STRING_CONTENT "\n"
        HEREDOC_END "  EOT"
        SEMICOLON ";"
        WHITESPACE "\n"
        VARIABLE "$b"
        WHITESPACE " "
        ASSIGN "="
        WHITESPACE " "
        HEREDOC_START "<<<'RAW'\n"
        STRING_CONTENT "$no\n"
        HEREDOC_END "RAW"
        SEMICOLON ";"
        WHITESPACE "\n"
        VARIABLE "$c"
        WHITESPACE " "
        ASSIGN "="
        WHITESPACE " "
        HEREDOC_START "<<<\"Q\"\n"
        STRING_CONTENT "q\n"
        HEREDOC_END "Q"
    "#]],
    );
}

#[test]
fn casts_and_operators() {
    check(
        "<?php (int)$a (string ) $b (foo) $a ?? $b ??= 1 |> f(...) <=> <> **= ?->",
        expect![[r#"
        OPEN_TAG "<?php "
        CAST "(int)"
        VARIABLE "$a"
        WHITESPACE " "
        CAST "(string )"
        WHITESPACE " "
        VARIABLE "$b"
        WHITESPACE " "
        LPAREN "("
        IDENT "foo"
        RPAREN ")"
        WHITESPACE " "
        VARIABLE "$a"
        WHITESPACE " "
        COALESCE "??"
        WHITESPACE " "
        VARIABLE "$b"
        WHITESPACE " "
        COALESCE_ASSIGN "??="
        WHITESPACE " "
        INT_LITERAL "1"
        WHITESPACE " "
        PIPE_GT "|>"
        WHITESPACE " "
        IDENT "f"
        LPAREN "("
        ELLIPSIS "..."
        RPAREN ")"
        WHITESPACE " "
        SPACESHIP "<=>"
        WHITESPACE " "
        NEQ "<>"
        WHITESPACE " "
        POW_ASSIGN "**="
        WHITESPACE " "
        NULLSAFE_ARROW "?->"
    "#]],
    );
}

#[test]
fn comments_and_attributes() {
    check(
        "<?php // a ?> b\n",
        expect![[r#"
        OPEN_TAG "<?php "
        COMMENT "// a "
        CLOSE_TAG "?>"
        INLINE_HTML " b\n"
    "#]],
    );
    check(
        "<?php # c\n/* d */ /** e */ /**/ #[A] ?>",
        expect![[r##"
        OPEN_TAG "<?php "
        COMMENT "# c"
        WHITESPACE "\n"
        BLOCK_COMMENT "/* d */"
        WHITESPACE " "
        DOC_COMMENT "/** e */"
        WHITESPACE " "
        BLOCK_COMMENT "/**/"
        WHITESPACE " "
        HASH_BRACKET "#["
        IDENT "A"
        RBRACKET "]"
        WHITESPACE " "
        CLOSE_TAG "?>"
    "##]],
    );
}

#[test]
fn names() {
    check(
        "<?php Foo\\Bar \\Foo namespace\\x \\ Foo\\ list $this->class",
        expect![[r#"
        OPEN_TAG "<?php "
        QUALIFIED_NAME "Foo\\Bar"
        WHITESPACE " "
        FULLY_QUALIFIED_NAME "\\Foo"
        WHITESPACE " "
        RELATIVE_NAME "namespace\\x"
        WHITESPACE " "
        BACKSLASH "\\"
        WHITESPACE " "
        IDENT "Foo"
        BACKSLASH "\\"
        WHITESPACE " "
        LIST_KW "list"
        WHITESPACE " "
        VARIABLE "$this"
        ARROW "->"
        IDENT "class"
    "#]],
    );
}

#[test]
fn halt_compiler() {
    check(
        "<?php a(); __halt_compiler(); raw <?php data",
        expect![[r#"
        OPEN_TAG "<?php "
        IDENT "a"
        LPAREN "("
        RPAREN ")"
        SEMICOLON ";"
        WHITESPACE " "
        HALT_COMPILER_KW "__halt_compiler"
        LPAREN "("
        RPAREN ")"
        SEMICOLON ";"
        HALT_DATA " raw <?php data"
    "#]],
    );
}

#[test]
fn unterminated() {
    check(
        "<?php 'abc",
        expect![[r#"
        OPEN_TAG "<?php "
        STRING_LITERAL "'abc"
        error 6..10 Unterminated string
    "#]],
    );
    check(
        "<?php /* x",
        expect![[r#"
        OPEN_TAG "<?php "
        BLOCK_COMMENT "/* x"
        error 6..10 Unterminated comment
    "#]],
    );
    check(
        "<?php \"a $b",
        expect![[r#"
        OPEN_TAG "<?php "
        DOUBLE_QUOTE "\""
        STRING_CONTENT "a "
        VARIABLE "$b"
        error 11..11 Unterminated string
    "#]],
    );
    check(
        "<?php <<<EOT\nabc",
        expect![[r#"
        OPEN_TAG "<?php "
        HEREDOC_START "<<<EOT\n"
        STRING_CONTENT "abc"
        error 16..16 Unterminated heredoc
    "#]],
    );
}

#[test]
fn checkpoints_resume_to_the_same_tokens() {
    let text = "<?php\n$a = <<<EOT\nx {$b}\nEOT;\n$c = \"a\n$d\";\nfoo();\n?>\nhtml\n<?php\n$e;\n";
    let full = lex_with_checkpoints(text);
    assert!(full.checkpoints.len() > 4);
    for checkpoint in &full.checkpoints {
        let resumed = lex_from(text, checkpoint.offset, checkpoint.state.clone());
        let mut offset = 0u32;
        let mut index = 0;
        while offset < checkpoint.offset {
            offset += full.tokens[index].len;
            index += 1;
        }
        assert_eq!(offset, checkpoint.offset);
        assert_eq!(resumed.tokens, full.tokens[index..], "resume at {}", checkpoint.offset);
    }
}
