//! A recursive descent parser with a Pratt expression parser. It always produces a tree that holds
//! every byte of the input: what it cannot make sense of ends up in `ERROR` nodes.

mod expr;
mod item;
mod stmt;
mod string;
mod ty;

use std::cell::Cell;

use rowan::{Checkpoint, GreenNode, GreenNodeBuilder, TextRange, TextSize};

use crate::kind::{PhpLanguage, SyntaxKind, SyntaxNode};
use crate::lexer::{self, Token};
use SyntaxKind::*;

/// A syntax error with the range it belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyntaxError {
    pub range: TextRange,
    pub message: String,
}

/// The tree of a text and the syntax errors found while building it.
#[derive(Clone, Debug)]
pub struct Parse {
    green: GreenNode,
    errors: Vec<SyntaxError>,
}

impl Parse {
    pub fn syntax(&self) -> SyntaxNode {
        SyntaxNode::new_root(self.green.clone())
    }

    pub fn green(&self) -> &GreenNode {
        &self.green
    }

    pub fn errors(&self) -> &[SyntaxError] {
        &self.errors
    }
}

/// Parses a text. Never fails: a broken file gives a tree with `ERROR` nodes and a list of errors.
pub fn parse(text: &str) -> Parse {
    let lexed = lexer::lex(text);
    let mut parser = Parser::new(text, lexed.tokens);
    for error in &lexed.errors {
        parser.errors.push(SyntaxError {
            range: TextRange::new(TextSize::from(error.start), TextSize::from(error.end)),
            message: error.message.to_string(),
        });
    }
    stmt::source_file(&mut parser);
    parser.finish()
}

/// How many lookups without consuming a token the parser allows before it gives up on the rest of
/// the file. Every loop consumes a token or leaves, so this is a guard against a bug and not a limit
/// a real file reaches.
const FUEL: u32 = 200_000;

/// How deeply expressions and statements may nest.
const MAX_DEPTH: u32 = 200;

pub(crate) struct Parser<'a> {
    text: &'a str,
    tokens: Vec<Token>,
    starts: Vec<u32>,
    /// Indexes into `tokens` of everything that is not trivia.
    significant: Vec<u32>,
    cursor: usize,
    /// The next token that goes into the tree; trivia between it and the cursor waits.
    emitted: usize,
    builder: GreenNodeBuilder<'static>,
    errors: Vec<SyntaxError>,
    fuel: Cell<u32>,
    /// Set while the statement being parsed is allowed to start with a `(void)` cast.
    pub(crate) void_cast_allowed: bool,
    depth: u32,
}

impl<'a> Parser<'a> {
    fn new(text: &'a str, tokens: Vec<Token>) -> Parser<'a> {
        let mut starts = Vec::with_capacity(tokens.len());
        let mut significant = Vec::with_capacity(tokens.len());
        let mut offset = 0u32;
        for (index, token) in tokens.iter().enumerate() {
            starts.push(offset);
            offset += token.len;
            if !token.kind.is_trivia() {
                significant.push(index as u32);
            }
        }
        Parser {
            text,
            tokens,
            starts,
            significant,
            cursor: 0,
            emitted: 0,
            builder: GreenNodeBuilder::new(),
            errors: Vec::new(),
            fuel: Cell::new(FUEL),
            void_cast_allowed: false,
            depth: 0,
        }
    }

    fn finish(self) -> Parse {
        Parse {
            green: self.builder.finish(),
            errors: self.errors,
        }
    }

    // Nesting

    /// Counts one more level of nesting. Says no past `MAX_DEPTH`, so deeply nested input ends in
    /// an error and not in a stack overflow. Every `true` is matched by a `leave`.
    pub(crate) fn enter(&mut self) -> bool {
        if self.depth >= MAX_DEPTH {
            if self.depth == MAX_DEPTH {
                self.depth += 1;
                self.error_here("Nesting is too deep");
            }
            return false;
        }
        self.depth += 1;
        true
    }

    pub(crate) fn leave(&mut self) {
        self.depth = self.depth.saturating_sub(1);
    }

    // Lookahead

    pub(crate) fn nth(&self, n: usize) -> SyntaxKind {
        let fuel = self.fuel.get();
        if fuel == 0 {
            return EOF;
        }
        self.fuel.set(fuel - 1);
        match self.significant.get(self.cursor + n) {
            Some(index) => self.tokens[*index as usize].kind,
            None => EOF,
        }
    }

    pub(crate) fn current(&self) -> SyntaxKind {
        self.nth(0)
    }

    pub(crate) fn at(&self, kind: SyntaxKind) -> bool {
        self.current() == kind
    }

    pub(crate) fn at_any(&self, kinds: &[SyntaxKind]) -> bool {
        kinds.contains(&self.current())
    }

    /// The index of the cursor, to tell whether a parse step consumed anything.
    pub(crate) fn position(&self) -> usize {
        self.cursor
    }

    pub(crate) fn eof(&self) -> bool {
        self.current() == EOF
    }

    /// The text of the `n`th upcoming token.
    pub(crate) fn nth_text(&self, n: usize) -> &'a str {
        match self.significant.get(self.cursor + n) {
            Some(index) => {
                let index = *index as usize;
                let start = self.starts[index] as usize;
                &self.text[start..start + self.tokens[index].len as usize]
            }
            None => "",
        }
    }

    pub(crate) fn current_text(&self) -> &'a str {
        self.nth_text(0)
    }

    /// Whether the `n`th upcoming token is an identifier that spells `word`, in any case.
    pub(crate) fn nth_is_word(&self, n: usize, word: &str) -> bool {
        self.nth(n) == IDENT && self.nth_text(n).eq_ignore_ascii_case(word)
    }

    /// Whether the `n`th upcoming token is followed by the next one with nothing in between.
    #[allow(dead_code)]
    pub(crate) fn nth_touches_next(&self, n: usize) -> bool {
        match (
            self.significant.get(self.cursor + n),
            self.significant.get(self.cursor + n + 1),
        ) {
            (Some(first), Some(second)) => *first + 1 == *second,
            _ => false,
        }
    }

    /// Whether the current token follows the previous one with nothing in between.
    pub(crate) fn nth_touches_prev(&self) -> bool {
        match self
            .cursor
            .checked_sub(1)
            .and_then(|previous| self.significant.get(previous))
        {
            Some(previous) => self
                .significant
                .get(self.cursor)
                .is_some_and(|current| *previous + 1 == *current),
            None => false,
        }
    }

    /// The byte of the text before `offset`.
    pub(crate) fn byte_before(&self, offset: u32) -> Option<u8> {
        offset
            .checked_sub(1)
            .and_then(|at| self.text.as_bytes().get(at as usize))
            .copied()
    }

    /// The offset where the current token starts.
    pub(crate) fn current_offset(&self) -> u32 {
        self.significant
            .get(self.cursor)
            .map_or(self.text.len() as u32, |index| self.starts[*index as usize])
    }

    pub(crate) fn error_at(&mut self, range: TextRange, message: impl Into<String>) {
        self.errors.push(SyntaxError {
            range,
            message: message.into(),
        });
    }

    fn current_range(&self) -> TextRange {
        match self.significant.get(self.cursor) {
            Some(index) => {
                let index = *index as usize;
                let start = self.starts[index];
                TextRange::new(TextSize::from(start), TextSize::from(start + self.tokens[index].len))
            }
            None => {
                let end = TextSize::of(self.text);
                TextRange::new(end, end)
            }
        }
    }

    /// An empty range right after the previous token, where something that is missing belongs.
    fn after_previous_range(&self) -> TextRange {
        let end = match self
            .cursor
            .checked_sub(1)
            .and_then(|previous| self.significant.get(previous))
        {
            Some(index) => {
                let index = *index as usize;
                self.starts[index] + self.tokens[index].len
            }
            None => self
                .significant
                .get(self.cursor)
                .map_or(self.text.len() as u32, |index| self.starts[*index as usize]),
        };
        TextRange::empty(TextSize::from(end))
    }

    // Building the tree

    fn flush_trivia(&mut self) {
        let until = self
            .significant
            .get(self.cursor)
            .map_or(self.tokens.len(), |index| *index as usize);
        while self.emitted < until {
            self.emit(self.emitted);
            self.emitted += 1;
        }
    }

    fn emit(&mut self, index: usize) {
        let token = self.tokens[index];
        let start = self.starts[index] as usize;
        self.builder.token(
            PhpLanguage::kind_to_raw_kind(token.kind),
            &self.text[start..start + token.len as usize],
        );
    }

    /// Like `flush_trivia`, but a doc comment right before the next token stays out of the flush
    /// so the declaration that starts there takes it along.
    fn flush_trivia_before_declaration(&mut self) {
        let until = self
            .significant
            .get(self.cursor)
            .map_or(self.tokens.len(), |index| *index as usize);
        let mut doc = None;
        for index in (self.emitted..until).rev() {
            match self.tokens[index].kind {
                WHITESPACE => continue,
                DOC_COMMENT => doc = Some(index),
                _ => {}
            }
            break;
        }
        let stop = doc.unwrap_or(until);
        while self.emitted < stop {
            self.emit(self.emitted);
            self.emitted += 1;
        }
    }

    /// Starts a node at the next token. Trivia before it stays outside.
    pub(crate) fn start(&mut self, kind: SyntaxKind) {
        self.flush_trivia();
        self.builder.start_node(PhpLanguage::kind_to_raw_kind(kind));
    }

    /// Starts a node whose first child may be a declaration, which takes the doc comment before it
    /// along instead of leaving it outside the node.
    pub(crate) fn start_before_declaration(&mut self, kind: SyntaxKind) {
        self.flush_trivia_before_declaration();
        self.builder.start_node(PhpLanguage::kind_to_raw_kind(kind));
    }

    /// Starts the root, which also owns the trivia before the first token.
    pub(crate) fn start_root(&mut self, kind: SyntaxKind) {
        self.builder.start_node(PhpLanguage::kind_to_raw_kind(kind));
    }

    pub(crate) fn finish_node(&mut self) {
        self.builder.finish_node();
    }

    pub(crate) fn checkpoint(&mut self) -> Checkpoint {
        self.flush_trivia();
        self.builder.checkpoint()
    }

    pub(crate) fn declaration_checkpoint(&mut self) -> Checkpoint {
        self.flush_trivia_before_declaration();
        self.builder.checkpoint()
    }

    pub(crate) fn start_at(&mut self, checkpoint: Checkpoint, kind: SyntaxKind) {
        self.builder
            .start_node_at(checkpoint, PhpLanguage::kind_to_raw_kind(kind));
    }

    /// Consumes the current token into the tree.
    pub(crate) fn bump(&mut self) {
        if self.cursor >= self.significant.len() {
            return;
        }
        self.flush_trivia();
        self.emit(self.emitted);
        self.emitted += 1;
        self.cursor += 1;
        self.fuel.set(FUEL);
    }

    pub(crate) fn eat(&mut self, kind: SyntaxKind) -> bool {
        if self.at(kind) {
            self.bump();
            true
        } else {
            false
        }
    }

    /// Consumes `kind` or reports it missing, in which case nothing is consumed.
    pub(crate) fn expect(&mut self, kind: SyntaxKind, what: &str) -> bool {
        if self.eat(kind) {
            return true;
        }
        self.error_expected(what);
        false
    }

    // Errors

    pub(crate) fn error_expected(&mut self, what: &str) {
        let range = self.after_previous_range();
        self.errors.push(SyntaxError {
            range,
            message: format!("{what} expected"),
        });
    }

    pub(crate) fn error_here(&mut self, message: impl Into<String>) {
        let range = self.current_range();
        self.errors.push(SyntaxError {
            range,
            message: message.into(),
        });
    }

    /// Wraps the current token in an `ERROR` node with a message about it.
    pub(crate) fn error_bump(&mut self) {
        let message = match self.current() {
            EOF => return,
            _ => format!("Unexpected '{}'", self.current_text().escape_debug()),
        };
        self.error_here(message);
        self.start(ERROR);
        self.bump();
        self.finish_node();
    }

    /// Wraps tokens in one `ERROR` node until one of `stop` or the end of the file, taking at least one.
    pub(crate) fn error_recover(&mut self, stop: &[SyntaxKind]) {
        if self.eof() {
            return;
        }
        self.error_here(format!("Unexpected '{}'", self.current_text().escape_debug()));
        self.start(ERROR);
        self.bump();
        while !self.eof() && !stop.contains(&self.current()) {
            self.bump();
        }
        self.finish_node();
    }

    /// Everything that is left of the file, trivia included, goes in before the root closes.
    pub(crate) fn flush_rest(&mut self) {
        while self.emitted < self.tokens.len() {
            self.emit(self.emitted);
            self.emitted += 1;
        }
    }
}

impl PhpLanguage {
    fn kind_to_raw_kind(kind: SyntaxKind) -> rowan::SyntaxKind {
        <PhpLanguage as rowan::Language>::kind_to_raw(kind)
    }
}

pub(crate) fn is_name_token(kind: SyntaxKind) -> bool {
    matches!(kind, IDENT | QUALIFIED_NAME | FULLY_QUALIFIED_NAME | RELATIVE_NAME)
}

/// Tokens that can be the name of a member, a constant or a label: PHP allows reserved words there.
pub(crate) fn is_identifier_like(kind: SyntaxKind) -> bool {
    kind == IDENT || kind == MAGIC_CONSTANT || kind.is_keyword()
}
