use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxElement, SyntaxNode, SyntaxToken, TextRange, TextSize};

use crate::LineIndex;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FoldKind {
    Comment,
    Imports,
    Region,
}

/// A stretch of lines that can fold: from the line that stays visible to the last line that hides.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fold {
    pub start_line: u32,
    pub end_line: u32,
    pub kind: Option<FoldKind>,
    /// Where the visible text of the first line ends and that of the last line starts, for a
    /// client that folds within a line; `None` folds whole lines.
    pub characters: Option<(TextSize, TextSize)>,
}

/// What folds in a file: the bodies of classes, functions and control structures, arrays, `match`
/// and `switch` bodies, property hooks, heredocs, attribute lists, multi-line comments and runs of
/// line comments, runs of `use` statements, `// region` markers and PHP tags between markup.
///
/// A fold ends on the line before a closing bracket that starts its line, so the bracket stays
/// visible when the fold is closed.
pub fn folding_ranges(root: &SyntaxNode, text: &str, index: &LineIndex) -> Vec<Fold> {
    let mut folds = Vec::new();
    let mut walker = Walker {
        text,
        index,
        folds: &mut folds,
    };
    walker.walk(root);
    folds.retain(|fold| fold.end_line > fold.start_line);
    folds.sort_by_key(|fold| (fold.start_line, std::cmp::Reverse(fold.end_line)));
    folds.dedup_by_key(|fold| (fold.start_line, fold.end_line, fold.kind));
    folds
}

struct Walker<'a> {
    text: &'a str,
    index: &'a LineIndex,
    folds: &'a mut Vec<Fold>,
}

impl Walker<'_> {
    fn line(&self, offset: u32) -> u32 {
        self.index.line_of(offset)
    }

    fn push(&mut self, start_line: u32, end_line: u32, kind: Option<FoldKind>) {
        self.folds.push(Fold {
            start_line,
            end_line,
            kind,
            characters: None,
        });
    }

    /// Whether only whitespace precedes the offset on its line.
    fn starts_its_line(&self, offset: u32) -> bool {
        let line_start = self.index.line_start(self.line(offset)) as usize;
        self.text[line_start..offset as usize].chars().all(char::is_whitespace)
    }

    /// A bracketed range, folded up to the line before a closer that starts its own line.
    fn bracketed(&mut self, node: &SyntaxNode, opener: Option<SyntaxToken>, closer: Option<SyntaxToken>) {
        let (Some(opener), Some(closer)) = (opener, closer) else {
            return;
        };
        let start = self.line(opener.text_range().start().into());
        let closer_start: u32 = closer.text_range().start().into();
        let mut end = self.line(closer_start);
        if self.starts_its_line(closer_start) {
            end = end.saturating_sub(1);
        }
        let _ = node;
        self.push(start, end, None);
    }

    fn token_child(node: &SyntaxNode, kind: php_syntax::SyntaxKind) -> Option<SyntaxToken> {
        node.children_with_tokens()
            .filter_map(SyntaxElement::into_token)
            .find(|token| token.kind() == kind)
    }

    fn last_token_child(node: &SyntaxNode, kind: php_syntax::SyntaxKind) -> Option<SyntaxToken> {
        node.children_with_tokens()
            .filter_map(SyntaxElement::into_token)
            .filter(|token| token.kind() == kind)
            .last()
    }

    fn walk(&mut self, root: &SyntaxNode) {
        let mut line_comments: Vec<SyntaxToken> = Vec::new();
        let mut regions: Vec<u32> = Vec::new();
        let mut open_tag: Option<SyntaxToken> = None;
        for element in root.descendants_with_tokens() {
            match element {
                SyntaxElement::Token(token) => {
                    self.token(&token, &mut line_comments, &mut regions, &mut open_tag);
                }
                SyntaxElement::Node(node) => self.node(&node),
            }
        }
        self.flush_comment_run(&mut line_comments);
    }

    fn token(
        &mut self,
        token: &SyntaxToken,
        run: &mut Vec<SyntaxToken>,
        regions: &mut Vec<u32>,
        open_tag: &mut Option<SyntaxToken>,
    ) {
        let kind = token.kind();
        if kind != COMMENT && kind != WHITESPACE {
            self.flush_comment_run(run);
        }
        match kind {
            DOC_COMMENT | BLOCK_COMMENT => {
                let range = token.text_range();
                let (start, end) = (self.line(range.start().into()), self.line(range.end().into()));
                self.push(start, end, Some(FoldKind::Comment));
                self.region_marker(token, regions);
            }
            COMMENT => {
                if let Some(last) = run.last() {
                    let between =
                        &self.text[usize::from(last.text_range().end())..usize::from(token.text_range().start())];
                    if between.matches('\n').count() != 1 || !between.trim().is_empty() {
                        self.flush_comment_run(run);
                    }
                }
                run.push(token.clone());
                self.region_marker(token, regions);
            }
            OPEN_TAG | OPEN_TAG_ECHO => *open_tag = Some(token.clone()),
            CLOSE_TAG => {
                if let Some(open) = open_tag.take() {
                    let start = self.line(open.text_range().start().into());
                    let end = self.line(token.text_range().start().into());
                    self.push(start, end, None);
                }
            }
            _ => {}
        }
    }

    /// `// region Name` and `// endregion`, in any comment style.
    fn region_marker(&mut self, token: &SyntaxToken, regions: &mut Vec<u32>) {
        let text = token.text().trim_start_matches(['/', '#', '*', ' ', '\t']);
        let lower = text.to_ascii_lowercase();
        let line = self.line(token.text_range().start().into());
        if lower.starts_with("endregion") {
            if let Some(start) = regions.pop() {
                self.push(start, line, Some(FoldKind::Region));
            }
        } else if lower.starts_with("region") && lower[6..].chars().next().is_none_or(|next| !next.is_alphanumeric()) {
            regions.push(line);
        }
    }

    fn flush_comment_run(&mut self, run: &mut Vec<SyntaxToken>) {
        if let (Some(first), Some(last)) = (run.first(), run.last()) {
            let start = self.line(first.text_range().start().into());
            let end = self.line(last.text_range().start().into());
            self.push(start, end, Some(FoldKind::Comment));
        }
        run.clear();
    }

    fn node(&mut self, node: &SyntaxNode) {
        match node.kind() {
            CLASS_BODY | BLOCK | PROPERTY_HOOK_LIST | TRAIT_ADAPTATIONS => {
                let (open, close) = (Self::token_child(node, LBRACE), Self::last_token_child(node, RBRACE));
                self.bracketed(node, open, close);
            }
            ARRAY_EXPR => {
                let open = Self::token_child(node, LBRACKET).or_else(|| Self::token_child(node, LPAREN));
                let close = Self::last_token_child(node, RBRACKET).or_else(|| Self::last_token_child(node, RPAREN));
                self.bracketed(node, open, close);
            }
            MATCH_EXPR | SWITCH_STATEMENT => {
                let (open, close) = (Self::token_child(node, LBRACE), Self::last_token_child(node, RBRACE));
                self.bracketed(node, open, close);
            }
            ATTRIBUTE_LIST => {
                let (open, close) = (
                    Self::token_child(node, HASH_BRACKET),
                    Self::last_token_child(node, RBRACKET),
                );
                self.bracketed(node, open, close);
            }
            HEREDOC => {
                let (open, close) = (
                    Self::token_child(node, HEREDOC_START),
                    Self::last_token_child(node, HEREDOC_END),
                );
                self.bracketed(node, open, close);
            }
            STATEMENT_LIST => self.alternative_body(node),
            USE_STATEMENT => self.imports(node),
            _ => {}
        }
    }

    /// The statements of `if (...):` up to `endif`: from the colon to the line before the next keyword.
    fn alternative_body(&mut self, list: &SyntaxNode) {
        let Some(colon) = list.prev_sibling_or_token().and_then(|mut element| {
            while element.kind().is_trivia() {
                element = element.prev_sibling_or_token()?;
            }
            element.into_token().filter(|token| token.kind() == COLON)
        }) else {
            return;
        };
        let start = self.line(colon.text_range().start().into());
        let end = self.line(list.text_range().end().into());
        self.push(start, end, None);
    }

    /// A run of `use` statements folds as one, started by its first statement.
    fn imports(&mut self, statement: &SyntaxNode) {
        if statement
            .prev_sibling()
            .is_some_and(|previous| previous.kind() == USE_STATEMENT)
        {
            return;
        }
        let mut last = statement.clone();
        while let Some(next) = last.next_sibling() {
            if next.kind() != USE_STATEMENT {
                break;
            }
            last = next;
        }
        let range = TextRange::new(statement.text_range().start(), last.text_range().end());
        let (start, end) = (self.line(range.start().into()), self.line(range.end().into()));
        self.push(start, end, Some(FoldKind::Imports));
        let keyword = Self::token_child(statement, USE_KW);
        let semicolon = Self::last_token_child(&last, SEMICOLON);
        if let (Some(fold), Some(keyword), Some(semicolon)) = (self.folds.last_mut(), keyword, semicolon) {
            fold.characters = Some((keyword.text_range().end(), semicolon.text_range().start()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::{Expect, expect};
    use php_syntax::parse;

    fn check(text: &str, expect: Expect) {
        let parsed = parse(text);
        let index = LineIndex::new(text);
        let mut out = String::new();
        for fold in folding_ranges(&parsed.syntax(), text, &index) {
            let kind = fold.kind.map_or("", |kind| match kind {
                FoldKind::Comment => " comment",
                FoldKind::Imports => " imports",
                FoldKind::Region => " region",
            });
            out.push_str(&format!("{}-{}{kind}\n", fold.start_line, fold.end_line));
        }
        expect.assert_eq(&out);
    }

    #[test]
    fn folds_bodies_and_leaves_the_closing_bracket_visible() {
        check(
            "<?php\n/**\n * Doc\n */\nclass A\n{\n    public function f(): void\n    {\n        $x = [\n            1,\n            2,\n        ];\n    }\n}\n",
            expect![[r#"
                1-3 comment
                5-12
                7-11
                8-10
            "#]],
        );
    }

    #[test]
    fn folds_imports_comments_regions_heredocs_and_tags() {
        check(
            "<?php\n// one\n// two\nuse A\\B;\nuse A\\C;\n\n// region Helpers\nfunction f() { return <<<EOT\n  text\n  EOT; }\n// endregion\n#[A(\n  1\n)]\nclass X {}\n?>\n<p>\n<?php if ($a): ?>\nx\n<?php endif; ?>\n",
            expect![[r#"
                0-15
                1-2 comment
                3-4 imports
                6-10 region
                7-9
                7-8
                11-13
                17-19
            "#]],
        );
    }

    #[test]
    fn folds_alternative_syntax_and_match() {
        check(
            "<?php\nif ($a):\n    echo 1;\n    echo 2;\nendif;\n$x = match ($a) {\n    1 => 'a',\n    default => 'b',\n};\nswitch ($a) {\n    case 1:\n        break;\n}\n",
            expect![[r#"
                1-3
                5-7
                9-11
            "#]],
        );
    }
}
