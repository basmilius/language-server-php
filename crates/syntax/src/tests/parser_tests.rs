use crate::{SyntaxError, SyntaxKind, SyntaxNode, dump_compact, parse};
use expect_test::Expect;

fn render(node: &SyntaxNode, errors: &[SyntaxError], shift: u32) -> String {
    let mut out = dump_compact(node);
    for SyntaxError { range, message } in errors {
        out.push_str(&format!(
            "error {}..{}: {message}\n",
            u32::from(range.start()) - shift,
            u32::from(range.end()) - shift
        ));
    }
    out
}

/// The compact tree of a snippet in PHP mode, with its errors after it. Also checks the tree keeps every byte.
fn tree_of(text: &str) -> String {
    let source = format!("<?php\n{text}");
    let parsed = parse(&source);
    assert_eq!(
        parsed.syntax().text().to_string(),
        source,
        "the tree must hold every byte"
    );
    render(&parsed.syntax(), parsed.errors(), 6)
}

pub(super) fn check(text: &str, expect: Expect) {
    expect.assert_eq(&tree_of(text));
}

/// The expression of the first expression statement, which is what precedence tests are about.
pub(super) fn expr(text: &str, expect: Expect) {
    let source = format!("<?php\n{text};");
    let parsed = parse(&source);
    assert_eq!(parsed.syntax().text().to_string(), source);
    let statement = parsed
        .syntax()
        .descendants()
        .find(|node| node.kind() == SyntaxKind::EXPR_STATEMENT)
        .expect("an expression statement");
    let expression = statement.children().next().expect("an expression");
    expect.assert_eq(&render(&expression, parsed.errors(), 6));
}

mod expressions;
mod recovery;
mod robustness;
mod statements;
