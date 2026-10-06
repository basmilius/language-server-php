//! The PHP lexer: a mode stack that mirrors how PHP itself tokenizes, so inline HTML, interpolated
//! strings, heredocs and `${` land on the same boundaries. Every byte of the input ends up in exactly
//! one token, trivia included.

use crate::SyntaxKind::{self, *};

/// One token: its kind and its length in bytes. Offsets follow from the tokens before it.
pub type Token = lsc_syntax::Token<SyntaxKind>;

/// A problem the lexer saw, such as a string that never ends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LexError {
    pub start: u32,
    pub end: u32,
    pub message: &'static str,
}

/// A mode of the lexer. The stack mirrors PHP's own state stack, where `{` pushes scripting and `}` pops it.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Mode {
    Initial,
    Scripting,
    DoubleQuote,
    Backtick,
    Heredoc {
        label: String,
    },
    Nowdoc {
        label: String,
    },
    /// After `->` and `?->`: the next label is a property name, whatever it spells.
    LookingForProperty,
    /// After `${`: a label before `[` or `}` is a variable name and anything else is an expression.
    LookingForVarName,
    /// Inside the `[...]` of `"$a[...]"`.
    VarOffset,
    /// After `__halt_compiler`: how far through `( ) ;` the lexer is. Everything after is data.
    HaltCompiler(u8),
}

/// What a lexer needs to carry on from the middle of a text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LexState {
    modes: Vec<Mode>,
}

impl Default for LexState {
    fn default() -> Self {
        LexState {
            modes: vec![Mode::Initial],
        }
    }
}

/// A place the lexer can restart from: the first token of a line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Checkpoint {
    pub offset: u32,
    pub state: LexState,
}

/// The result of lexing a whole text.
#[derive(Debug, Default)]
pub struct Lexed {
    pub tokens: Vec<Token>,
    pub errors: Vec<LexError>,
    /// Empty unless asked for with [`lex_with_checkpoints`].
    pub checkpoints: Vec<Checkpoint>,
}

/// Lexes a whole text.
pub fn lex(text: &str) -> Lexed {
    run(Lexer::new(text), false)
}

/// Lexes a whole text and records where a lexer can restart, for incremental relexing.
pub fn lex_with_checkpoints(text: &str) -> Lexed {
    run(Lexer::new(text), true)
}

/// Lexes from `offset` on with the state a checkpoint recorded.
pub fn lex_from(text: &str, offset: u32, state: LexState) -> Lexed {
    run(Lexer::resume(text, offset, state), false)
}

fn run(mut lexer: Lexer, checkpoints: bool) -> Lexed {
    let mut lexed = Lexed::default();
    let mut at_line_start = lexer.pos == 0;
    while lexer.pos < lexer.bytes.len() {
        if checkpoints && at_line_start {
            lexed.checkpoints.push(Checkpoint {
                offset: lexer.pos as u32,
                state: lexer.state.clone(),
            });
        }
        let start = lexer.pos;
        match lexer.next_token() {
            Some(token) => {
                at_line_start = lexer.bytes[lexer.pos - 1] == b'\n';
                debug_assert!(lexer.pos > start);
                lexed.tokens.push(token);
            }
            None => break,
        }
    }
    // At the end of the text this only reports a string or heredoc that never closed.
    lexer.next_token();
    lexed.errors = lexer.errors;
    lexed
}

/// The lexer. Drive it with [`Lexer::next_token`] or use [`lex`].
pub struct Lexer<'a> {
    bytes: &'a [u8],
    src: &'a str,
    pos: usize,
    state: LexState,
    errors: Vec<LexError>,
}

fn is_label_start(byte: u8) -> bool {
    byte.is_ascii_alphabetic() || byte == b'_' || byte >= 0x80
}

fn is_label_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 0x80
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Lexer<'a> {
        Lexer {
            bytes: src.as_bytes(),
            src,
            pos: 0,
            state: LexState::default(),
            errors: Vec::new(),
        }
    }

    pub fn resume(src: &'a str, offset: u32, state: LexState) -> Lexer<'a> {
        Lexer {
            bytes: src.as_bytes(),
            src,
            pos: offset as usize,
            state,
            errors: Vec::new(),
        }
    }

    /// The state to resume from at the current position.
    pub fn state(&self) -> &LexState {
        &self.state
    }

    pub fn offset(&self) -> u32 {
        self.pos as u32
    }

    pub fn errors(&self) -> &[LexError] {
        &self.errors
    }

    fn peek(&self, ahead: usize) -> u8 {
        self.bytes.get(self.pos + ahead).copied().unwrap_or(0)
    }

    fn mode(&self) -> &Mode {
        self.state.modes.last().unwrap_or(&Mode::Initial)
    }

    fn set_mode(&mut self, mode: Mode) {
        match self.state.modes.last_mut() {
            Some(last) => *last = mode,
            None => self.state.modes.push(mode),
        }
    }

    fn push(&mut self, mode: Mode) {
        self.state.modes.push(mode);
    }

    fn pop(&mut self) {
        if self.state.modes.len() > 1 {
            self.state.modes.pop();
        }
    }

    fn error(&mut self, start: usize, end: usize, message: &'static str) {
        self.errors.push(LexError {
            start: start as u32,
            end: end as u32,
            message,
        });
    }

    fn starts_with(&self, at: usize, needle: &str) -> bool {
        self.bytes
            .get(at..at + needle.len())
            .is_some_and(|slice| slice == needle.as_bytes())
    }

    fn starts_with_ignore_case(&self, at: usize, needle: &str) -> bool {
        self.bytes
            .get(at..at + needle.len())
            .is_some_and(|slice| slice.eq_ignore_ascii_case(needle.as_bytes()))
    }

    fn token(&mut self, kind: SyntaxKind, len: usize) -> Option<Token> {
        self.pos += len;
        Some(Token { kind, len: len as u32 })
    }

    /// The next token, or `None` at the end of the text.
    pub fn next_token(&mut self) -> Option<Token> {
        loop {
            if self.pos >= self.bytes.len() {
                if self.state.modes.len() > 1 || !matches!(self.mode(), Mode::Initial | Mode::Scripting) {
                    self.finish_unterminated();
                }
                return None;
            }
            match self.mode().clone() {
                Mode::Initial => return self.lex_initial(),
                Mode::Scripting => return self.lex_scripting(),
                Mode::DoubleQuote | Mode::Backtick | Mode::Heredoc { .. } => {
                    return self.lex_string_part();
                }
                Mode::Nowdoc { label } => return self.lex_nowdoc(&label),
                Mode::LookingForProperty => {
                    if let Some(token) = self.lex_looking_for_property() {
                        return Some(token);
                    }
                }
                Mode::LookingForVarName => {
                    if let Some(token) = self.lex_looking_for_var_name() {
                        return Some(token);
                    }
                }
                Mode::VarOffset => {
                    if let Some(token) = self.lex_var_offset() {
                        return Some(token);
                    }
                }
                Mode::HaltCompiler(stage) => return self.lex_halt_compiler(stage),
            }
        }
    }

    fn finish_unterminated(&mut self) {
        let end = self.bytes.len();
        let message = match self.state.modes.iter().rev().find(|mode| {
            matches!(
                mode,
                Mode::DoubleQuote | Mode::Backtick | Mode::Heredoc { .. } | Mode::Nowdoc { .. }
            )
        }) {
            Some(Mode::Heredoc { .. } | Mode::Nowdoc { .. }) => "Unterminated heredoc",
            Some(_) => "Unterminated string",
            None => return,
        };
        self.error(end, end, message);
        self.state.modes.truncate(1);
    }

    fn lex_initial(&mut self) -> Option<Token> {
        if let Some(len) = self.open_tag_len() {
            let kind = if self.peek(2) == b'=' { OPEN_TAG_ECHO } else { OPEN_TAG };
            self.set_mode(Mode::Scripting);
            return self.token(kind, len);
        }
        let mut end = self.pos + 1;
        while end < self.bytes.len() {
            if self.bytes[end] == b'<' && self.open_tag_len_at(end).is_some() {
                break;
            }
            end += 1;
        }
        let len = end - self.pos;
        self.token(INLINE_HTML, len)
    }

    fn open_tag_len(&self) -> Option<usize> {
        self.open_tag_len_at(self.pos)
    }

    /// `<?php` with the one whitespace character after it (or the end of the text), or `<?=`.
    fn open_tag_len_at(&self, at: usize) -> Option<usize> {
        if self.starts_with(at, "<?=") {
            return Some(3);
        }
        if !self.starts_with_ignore_case(at, "<?php") {
            return None;
        }
        match self.bytes.get(at + 5) {
            None => Some(5),
            Some(b' ' | b'\t' | b'\n') => Some(6),
            Some(b'\r') => Some(if self.bytes.get(at + 6) == Some(&b'\n') { 7 } else { 6 }),
            Some(_) => None,
        }
    }

    fn lex_scripting(&mut self) -> Option<Token> {
        let byte = self.bytes[self.pos];
        match byte {
            b' ' | b'\t' | b'\n' | b'\r' => {
                let mut end = self.pos + 1;
                while end < self.bytes.len() && matches!(self.bytes[end], b' ' | b'\t' | b'\n' | b'\r') {
                    end += 1;
                }
                self.token(WHITESPACE, end - self.pos)
            }
            b'#' => {
                if self.peek(1) == b'[' {
                    return self.token(HASH_BRACKET, 2);
                }
                self.line_comment()
            }
            b'/' => match self.peek(1) {
                b'/' => self.line_comment(),
                b'*' => self.block_comment(),
                b'=' => self.token(SLASH_ASSIGN, 2),
                _ => self.token(SLASH, 1),
            },
            b'0'..=b'9' => self.number(),
            b'.' => {
                if self.peek(1).is_ascii_digit() {
                    self.number()
                } else if self.peek(1) == b'.' && self.peek(2) == b'.' {
                    self.token(ELLIPSIS, 3)
                } else if self.peek(1) == b'=' {
                    self.token(DOT_ASSIGN, 2)
                } else {
                    self.token(DOT, 1)
                }
            }
            b'\\' => {
                if is_label_start(self.peek(1)) {
                    let end = self.name_end(self.pos + 1);
                    self.token(FULLY_QUALIFIED_NAME, end - self.pos)
                } else {
                    self.token(BACKSLASH, 1)
                }
            }
            b'$' => {
                if is_label_start(self.peek(1)) {
                    let end = self.label_end(self.pos + 1);
                    self.token(VARIABLE, end - self.pos)
                } else {
                    self.token(DOLLAR, 1)
                }
            }
            b'\'' => self.single_quoted(self.pos),
            b'"' => self.double_quoted(self.pos),
            b'`' => {
                self.push(Mode::Backtick);
                self.token(BACKTICK, 1)
            }
            b'(' => {
                if let Some(len) = self.cast_len() {
                    return self.token(CAST, len);
                }
                self.token(LPAREN, 1)
            }
            b')' => self.token(RPAREN, 1),
            b'[' => self.token(LBRACKET, 1),
            b']' => self.token(RBRACKET, 1),
            b'{' => {
                self.push(Mode::Scripting);
                self.token(LBRACE, 1)
            }
            b'}' => {
                self.pop();
                self.token(RBRACE, 1)
            }
            b';' => self.token(SEMICOLON, 1),
            b',' => self.token(COMMA, 1),
            b'@' => self.token(AT, 1),
            b'~' => self.token(TILDE, 1),
            b':' => {
                if self.peek(1) == b':' {
                    self.token(DOUBLE_COLON, 2)
                } else {
                    self.token(COLON, 1)
                }
            }
            b'?' => match (self.peek(1), self.peek(2)) {
                (b'>', _) => {
                    let len = self.close_tag_len();
                    self.state.modes.truncate(1);
                    self.set_mode(Mode::Initial);
                    self.token(CLOSE_TAG, len)
                }
                (b'-', b'>') => {
                    self.push(Mode::LookingForProperty);
                    self.token(NULLSAFE_ARROW, 3)
                }
                (b'?', b'=') => self.token(COALESCE_ASSIGN, 3),
                (b'?', _) => self.token(COALESCE, 2),
                _ => self.token(QUESTION, 1),
            },
            b'+' => match self.peek(1) {
                b'+' => self.token(INC, 2),
                b'=' => self.token(PLUS_ASSIGN, 2),
                _ => self.token(PLUS, 1),
            },
            b'-' => match self.peek(1) {
                b'-' => self.token(DEC, 2),
                b'=' => self.token(MINUS_ASSIGN, 2),
                b'>' => {
                    self.push(Mode::LookingForProperty);
                    self.token(ARROW, 2)
                }
                _ => self.token(MINUS, 1),
            },
            b'*' => match (self.peek(1), self.peek(2)) {
                (b'*', b'=') => self.token(POW_ASSIGN, 3),
                (b'*', _) => self.token(POW, 2),
                (b'=', _) => self.token(STAR_ASSIGN, 2),
                _ => self.token(STAR, 1),
            },
            b'%' => {
                if self.peek(1) == b'=' {
                    self.token(PERCENT_ASSIGN, 2)
                } else {
                    self.token(PERCENT, 1)
                }
            }
            b'=' => match (self.peek(1), self.peek(2)) {
                (b'=', b'=') => self.token(IDENTICAL, 3),
                (b'=', _) => self.token(EQ, 2),
                (b'>', _) => self.token(FAT_ARROW, 2),
                _ => self.token(ASSIGN, 1),
            },
            b'!' => match (self.peek(1), self.peek(2)) {
                (b'=', b'=') => self.token(NOT_IDENTICAL, 3),
                (b'=', _) => self.token(NEQ, 2),
                _ => self.token(BANG, 1),
            },
            b'<' => self.less_than(),
            b'>' => match (self.peek(1), self.peek(2)) {
                (b'>', b'=') => self.token(SHR_ASSIGN, 3),
                (b'>', _) => self.token(SHR, 2),
                (b'=', _) => self.token(GE, 2),
                _ => self.token(GT, 1),
            },
            b'&' => match self.peek(1) {
                b'&' => self.token(AND_AND, 2),
                b'=' => self.token(AMP_ASSIGN, 2),
                _ => self.token(AMP, 1),
            },
            b'|' => match self.peek(1) {
                b'|' => self.token(OR_OR, 2),
                b'=' => self.token(PIPE_ASSIGN, 2),
                b'>' => self.token(PIPE_GT, 2),
                _ => self.token(PIPE, 1),
            },
            b'^' => {
                if self.peek(1) == b'=' {
                    self.token(CARET_ASSIGN, 2)
                } else {
                    self.token(CARET, 1)
                }
            }
            byte if is_label_start(byte) => self.word(),
            _ => {
                let len = self.src[self.pos..].chars().next().map_or(1, char::len_utf8);
                let start = self.pos;
                self.error(start, start + len, "Unexpected character");
                self.token(UNKNOWN, len)
            }
        }
    }

    fn less_than(&mut self) -> Option<Token> {
        if self.peek(1) == b'<' && self.peek(2) == b'<' {
            if let Some(token) = self.heredoc_start(self.pos, self.pos) {
                return Some(token);
            }
        }
        match (self.peek(1), self.peek(2)) {
            (b'<', b'=') => self.token(SHL_ASSIGN, 3),
            (b'<', _) => self.token(SHL, 2),
            (b'=', b'>') => self.token(SPACESHIP, 3),
            (b'=', _) => self.token(LE, 2),
            (b'>', _) => self.token(NEQ, 2),
            _ => self.token(LT, 1),
        }
    }

    fn line_comment(&mut self) -> Option<Token> {
        let mut end = self.pos + 1;
        while end < self.bytes.len() {
            match self.bytes[end] {
                b'\n' | b'\r' => break,
                b'?' if self.bytes.get(end + 1) == Some(&b'>') => break,
                _ => end += 1,
            }
        }
        self.token(COMMENT, end - self.pos)
    }

    fn block_comment(&mut self) -> Option<Token> {
        let start = self.pos;
        let is_doc = self.peek(2) == b'*' && matches!(self.peek(3), b' ' | b'\t' | b'\n' | b'\r');
        let end = match self.bytes[start + 2..].windows(2).position(|pair| pair == b"*/") {
            Some(offset) => start + 2 + offset + 2,
            None => {
                self.error(start, self.bytes.len(), "Unterminated comment");
                self.bytes.len()
            }
        };
        self.token(if is_doc { DOC_COMMENT } else { BLOCK_COMMENT }, end - start)
    }

    fn label_end(&self, from: usize) -> usize {
        let mut end = from;
        while end < self.bytes.len() && is_label_char(self.bytes[end]) {
            end += 1;
        }
        end
    }

    /// The end of a name that starts at a label: further segments follow as long as a backslash
    /// is followed by a label.
    fn name_end(&self, from: usize) -> usize {
        let mut end = self.label_end(from);
        while self.bytes.get(end) == Some(&b'\\') && self.bytes.get(end + 1).is_some_and(|byte| is_label_start(*byte)) {
            end = self.label_end(end + 1);
        }
        end
    }

    fn word(&mut self) -> Option<Token> {
        let start = self.pos;
        let first_end = self.label_end(start);
        if first_end - start == 1 && matches!(self.bytes[start], b'b' | b'B') {
            match self.bytes.get(first_end) {
                Some(b'\'') => return self.single_quoted(start),
                Some(b'"') => return self.double_quoted(start),
                Some(b'<') if self.starts_with(first_end, "<<<") => {
                    if let Some(token) = self.heredoc_start(first_end, start) {
                        return Some(token);
                    }
                }
                _ => {}
            }
        }
        let end = self.name_end(start);
        if end != first_end {
            let kind = if self.starts_with_ignore_case(start, "namespace\\") {
                RELATIVE_NAME
            } else {
                QUALIFIED_NAME
            };
            return self.token(kind, end - start);
        }
        let word = &self.src[start..end];
        let kind = keyword_kind(word).unwrap_or(IDENT);
        match kind {
            HALT_COMPILER_KW => {
                self.set_mode(Mode::HaltCompiler(0));
                self.token(kind, end - start)
            }
            IDENT if is_magic_constant(word) => self.token(MAGIC_CONSTANT, end - start),
            _ => self.token(kind, end - start),
        }
    }

    /// `?>` and the single newline that belongs to it.
    fn close_tag_len(&self) -> usize {
        match (self.peek(2), self.peek(3)) {
            (b'\n', _) => 3,
            (b'\r', b'\n') => 4,
            (b'\r', _) => 3,
            _ => 2,
        }
    }

    fn lex_halt_compiler(&mut self, stage: u8) -> Option<Token> {
        let byte = self.bytes[self.pos];
        let expected = *b"();";
        if stage == 3 {
            let len = self.bytes.len() - self.pos;
            self.set_mode(Mode::Scripting);
            return self.token(HALT_DATA, len);
        }
        match byte {
            b' ' | b'\t' | b'\n' | b'\r' => {
                let mut end = self.pos + 1;
                while end < self.bytes.len() && matches!(self.bytes[end], b' ' | b'\t' | b'\n' | b'\r') {
                    end += 1;
                }
                self.token(WHITESPACE, end - self.pos)
            }
            b'#' => self.line_comment(),
            b'/' if self.peek(1) == b'/' => self.line_comment(),
            b'/' if self.peek(1) == b'*' => self.block_comment(),
            byte if byte == expected[stage as usize] => {
                self.set_mode(Mode::HaltCompiler(stage + 1));
                let kind = match byte {
                    b'(' => LPAREN,
                    b')' => RPAREN,
                    _ => SEMICOLON,
                };
                self.token(kind, 1)
            }
            b'?' if stage == 2 && self.peek(1) == b'>' => {
                let len = self.close_tag_len();
                self.set_mode(Mode::HaltCompiler(3));
                self.token(CLOSE_TAG, len)
            }
            _ => {
                self.set_mode(Mode::Scripting);
                self.lex_scripting()
            }
        }
    }

    fn cast_len(&self) -> Option<usize> {
        let mut at = self.pos + 1;
        while matches!(self.bytes.get(at), Some(b' ' | b'\t')) {
            at += 1;
        }
        let word_start = at;
        while self.bytes.get(at).is_some_and(u8::is_ascii_alphabetic) {
            at += 1;
        }
        let word = &self.src[word_start..at];
        while matches!(self.bytes.get(at), Some(b' ' | b'\t')) {
            at += 1;
        }
        if self.bytes.get(at) != Some(&b')') {
            return None;
        }
        const CASTS: &[&str] = &[
            "int", "integer", "bool", "boolean", "float", "double", "real", "string", "binary", "array", "object",
            "unset", "void",
        ];
        CASTS
            .iter()
            .any(|cast| word.eq_ignore_ascii_case(cast))
            .then_some(at + 1 - self.pos)
    }

    fn number(&mut self) -> Option<Token> {
        let start = self.pos;
        let mut end = start;
        if self.bytes[start] == b'0' {
            let radix: Option<fn(u8) -> bool> = match self.peek(1) {
                b'x' | b'X' => Some(|byte| byte.is_ascii_hexdigit()),
                b'b' | b'B' => Some(|byte| matches!(byte, b'0' | b'1')),
                b'o' | b'O' => Some(|byte| matches!(byte, b'0'..=b'7')),
                _ => None,
            };
            if let Some(is_digit) = radix {
                if self.bytes.get(start + 2).is_some_and(|byte| is_digit(*byte)) {
                    end = self.digits(start + 2, is_digit);
                    return self.token(INT_LITERAL, end - start);
                }
            }
        }
        let mut is_float = false;
        if self.bytes[start] != b'.' {
            end = self.digits(start, |byte| byte.is_ascii_digit());
        }
        if self.bytes.get(end) == Some(&b'.') && !(self.bytes.get(end + 1) == Some(&b'.')) {
            let after = end + 1;
            if self.bytes.get(after).is_some_and(u8::is_ascii_digit) {
                end = self.digits(after, |byte| byte.is_ascii_digit());
                is_float = true;
            } else if self.bytes[start] != b'.' {
                end = after;
                is_float = true;
            }
        }
        if matches!(self.bytes.get(end), Some(b'e' | b'E')) {
            let mut at = end + 1;
            if matches!(self.bytes.get(at), Some(b'+' | b'-')) {
                at += 1;
            }
            if self.bytes.get(at).is_some_and(u8::is_ascii_digit) {
                end = self.digits(at, |byte| byte.is_ascii_digit());
                is_float = true;
            }
        }
        if !is_float && end - start > 1 && self.bytes[start] == b'0' && self.src[start..end].contains(['8', '9']) {
            self.error(start, end, "Invalid numeric literal");
        }
        self.token(if is_float { FLOAT_LITERAL } else { INT_LITERAL }, end - start)
    }

    /// Digits with `_` separators, which only count between two digits.
    fn digits(&self, from: usize, is_digit: fn(u8) -> bool) -> usize {
        let mut end = from;
        while end < self.bytes.len() {
            let byte = self.bytes[end];
            let separator = byte == b'_' && end > from && self.bytes.get(end + 1).is_some_and(|next| is_digit(*next));
            if !is_digit(byte) && !separator {
                break;
            }
            end += 1;
        }
        end
    }

    fn single_quoted(&mut self, start: usize) -> Option<Token> {
        let quote = self.bytes[start..]
            .iter()
            .position(|byte| *byte == b'\'')
            .map_or(start, |offset| start + offset);
        let mut at = quote + 1;
        while at < self.bytes.len() {
            match self.bytes[at] {
                b'\\' => at += 2,
                b'\'' => return self.token(STRING_LITERAL, at + 1 - start),
                _ => at += 1,
            }
        }
        let end = self.bytes.len();
        self.error(start, end, "Unterminated string");
        self.token(STRING_LITERAL, end - start)
    }

    fn double_quoted(&mut self, start: usize) -> Option<Token> {
        let quote = self.bytes[start..]
            .iter()
            .position(|byte| *byte == b'"')
            .map_or(start, |q| start + q);
        let mut at = quote + 1;
        while at < self.bytes.len() {
            match self.bytes[at] {
                b'\\' => at += 2,
                b'"' => return self.token(STRING_LITERAL, at + 1 - start),
                b'$' | b'{' if self.interpolation_at(at).is_some() => {
                    self.push(Mode::DoubleQuote);
                    return self.token(DOUBLE_QUOTE, quote + 1 - start);
                }
                _ => at += 1,
            }
        }
        let end = self.bytes.len();
        self.error(start, end, "Unterminated string");
        self.token(STRING_LITERAL, end - start)
    }

    fn interpolation_at(&self, at: usize) -> Option<Interpolation> {
        let next = self.bytes.get(at + 1).copied().unwrap_or(0);
        match self.bytes.get(at) {
            Some(b'$') if is_label_start(next) => Some(Interpolation::Variable),
            Some(b'$') if next == b'{' => Some(Interpolation::DollarBrace),
            Some(b'{') if next == b'$' => Some(Interpolation::Curly),
            _ => None,
        }
    }

    /// `<<<LABEL`, `<<<"LABEL"` or `<<<'LABEL'` and the newline after it.
    fn heredoc_start(&mut self, marker: usize, start: usize) -> Option<Token> {
        let mut at = marker + 3;
        while matches!(self.bytes.get(at), Some(b' ' | b'\t')) {
            at += 1;
        }
        let quote = match self.bytes.get(at) {
            Some(q @ (b'"' | b'\'')) => {
                at += 1;
                Some(*q)
            }
            _ => None,
        };
        if !self.bytes.get(at).is_some_and(|byte| is_label_start(*byte)) {
            return None;
        }
        let label_start = at;
        let label_end = self.label_end(at);
        at = label_end;
        if let Some(q) = quote {
            if self.bytes.get(at) != Some(&q) {
                return None;
            }
            at += 1;
        }
        match (self.bytes.get(at), self.bytes.get(at + 1)) {
            (Some(b'\n'), _) => at += 1,
            (Some(b'\r'), Some(b'\n')) => at += 2,
            (Some(b'\r'), _) => at += 1,
            _ => return None,
        }
        let label = self.src[label_start..label_end].to_string();
        self.push(if quote == Some(b'\'') {
            Mode::Nowdoc { label }
        } else {
            Mode::Heredoc { label }
        });
        self.token(HEREDOC_START, at - start)
    }

    fn at_line_start(&self) -> bool {
        self.pos == 0 || matches!(self.bytes[self.pos - 1], b'\n' | b'\r')
    }

    /// Whether the line that starts at `line_start` holds the closing label of a heredoc, after
    /// any indentation.
    fn closes_heredoc(&self, line_start: usize, label: &str) -> bool {
        let mut at = line_start;
        while matches!(self.bytes.get(at), Some(b' ' | b'\t')) {
            at += 1;
        }
        self.starts_with(at, label)
            && !self
                .bytes
                .get(at + label.len())
                .is_some_and(|byte| is_label_char(*byte))
    }

    /// The end of the text part that starts at the cursor, which runs up to and including the
    /// newline before the closing line. A closing label is only looked for in text, never
    /// inside a `{$` block, which is how a label may appear in an interpolated expression.
    fn heredoc_text_end(&self, label: &str, interpolates: bool) -> usize {
        let mut end = self.pos;
        while end < self.bytes.len() {
            match self.bytes[end] {
                b'\\' if interpolates && !matches!(self.bytes.get(end + 1), Some(b'\n' | b'\r')) => end += 2,
                b'\\' if interpolates => end += 1,
                b'\n' | b'\r' => {
                    end += if self.bytes[end] == b'\r' && self.bytes.get(end + 1) == Some(&b'\n') {
                        2
                    } else {
                        1
                    };
                    if self.closes_heredoc(end, label) {
                        return end;
                    }
                }
                b'$' | b'{' if interpolates && end > self.pos && self.interpolation_at(end).is_some() => break,
                _ => end += 1,
            }
        }
        end.min(self.bytes.len())
    }

    fn lex_nowdoc(&mut self, label: &str) -> Option<Token> {
        if self.at_line_start() && self.closes_heredoc(self.pos, label) {
            return self.heredoc_end(label.len());
        }
        let end = self.heredoc_text_end(label, false);
        self.token(STRING_CONTENT, end - self.pos)
    }

    fn heredoc_end(&mut self, label_len: usize) -> Option<Token> {
        let mut at = self.pos;
        while matches!(self.bytes.get(at), Some(b' ' | b'\t')) {
            at += 1;
        }
        self.pop();
        let len = at + label_len - self.pos;
        self.token(HEREDOC_END, len)
    }

    /// A stretch of an interpolated string, a backtick string or a heredoc: text, a variable
    /// or the start of a `{$` or `${` block, or the end of the string.
    fn lex_string_part(&mut self) -> Option<Token> {
        let mode = self.mode().clone();
        if let Mode::Heredoc { label } = &mode {
            if self.at_line_start() && self.closes_heredoc(self.pos, label) {
                return self.heredoc_end(label.len());
            }
        }
        let byte = self.bytes[self.pos];
        let closing = match mode {
            Mode::DoubleQuote => Some((b'"', DOUBLE_QUOTE)),
            Mode::Backtick => Some((b'`', BACKTICK)),
            _ => None,
        };
        if let Some((quote, kind)) = closing {
            if byte == quote {
                self.pop();
                return self.token(kind, 1);
            }
        }
        if let Some(interpolation) = self.interpolation_at(self.pos) {
            return match interpolation {
                Interpolation::Variable => {
                    let end = self.label_end(self.pos + 1);
                    let len = end - self.pos;
                    match (self.bytes.get(end), self.bytes.get(end + 1), self.bytes.get(end + 2)) {
                        (Some(b'['), _, _) => self.push(Mode::VarOffset),
                        (Some(b'-'), Some(b'>'), Some(next)) if is_label_start(*next) => {
                            self.push(Mode::LookingForProperty)
                        }
                        (Some(b'?'), Some(b'-'), Some(b'>'))
                            if self.bytes.get(end + 3).is_some_and(|next| is_label_start(*next)) =>
                        {
                            self.push(Mode::LookingForProperty)
                        }
                        _ => {}
                    }
                    self.token(VARIABLE, len)
                }
                Interpolation::DollarBrace => {
                    self.push(Mode::LookingForVarName);
                    self.token(DOLLAR_OPEN_CURLY, 2)
                }
                Interpolation::Curly => {
                    self.push(Mode::Scripting);
                    self.token(CURLY_OPEN, 1)
                }
            };
        }
        let end = if let Mode::Heredoc { label } = &mode {
            self.heredoc_text_end(label, true)
        } else {
            let stop_byte = if mode == Mode::DoubleQuote { b'"' } else { b'`' };
            let mut end = self.pos;
            while end < self.bytes.len() {
                let current = self.bytes[end];
                if current == b'\\' {
                    end += 2;
                } else if current == stop_byte
                    || ((current == b'$' || current == b'{') && end > self.pos && self.interpolation_at(end).is_some())
                {
                    break;
                } else {
                    end += 1;
                }
            }
            end.min(self.bytes.len())
        };
        self.token(STRING_CONTENT, end - self.pos)
    }

    fn lex_looking_for_property(&mut self) -> Option<Token> {
        let byte = self.bytes[self.pos];
        match byte {
            b' ' | b'\t' | b'\n' | b'\r' => {
                let mut end = self.pos + 1;
                while end < self.bytes.len() && matches!(self.bytes[end], b' ' | b'\t' | b'\n' | b'\r') {
                    end += 1;
                }
                self.token(WHITESPACE, end - self.pos)
            }
            b'-' if self.peek(1) == b'>' => self.token(ARROW, 2),
            b'?' if self.peek(1) == b'-' && self.peek(2) == b'>' => self.token(NULLSAFE_ARROW, 3),
            byte if is_label_start(byte) => {
                let end = self.label_end(self.pos);
                self.pop();
                self.token(IDENT, end - self.pos)
            }
            _ => {
                self.pop();
                None
            }
        }
    }

    fn lex_looking_for_var_name(&mut self) -> Option<Token> {
        if is_label_start(self.bytes[self.pos]) {
            let end = self.label_end(self.pos);
            if matches!(self.bytes.get(end), Some(b'[' | b'}')) {
                self.set_mode(Mode::Scripting);
                return self.token(IDENT, end - self.pos);
            }
        }
        self.set_mode(Mode::Scripting);
        None
    }

    fn lex_var_offset(&mut self) -> Option<Token> {
        let byte = self.bytes[self.pos];
        match byte {
            b'[' => self.token(LBRACKET, 1),
            b']' => {
                self.pop();
                self.token(RBRACKET, 1)
            }
            b'-' => self.token(MINUS, 1),
            b'0'..=b'9' => {
                let end = self.digits(self.pos, |byte| byte.is_ascii_alphanumeric());
                self.token(INT_LITERAL, end - self.pos)
            }
            b'$' if is_label_start(self.peek(1)) => {
                let end = self.label_end(self.pos + 1);
                self.token(VARIABLE, end - self.pos)
            }
            byte if is_label_start(byte) => {
                let end = self.label_end(self.pos);
                self.token(IDENT, end - self.pos)
            }
            _ => {
                self.pop();
                None
            }
        }
    }
}

enum Interpolation {
    Variable,
    DollarBrace,
    Curly,
}

fn is_magic_constant(word: &str) -> bool {
    const NAMES: &[&str] = &[
        "__class__",
        "__dir__",
        "__file__",
        "__function__",
        "__line__",
        "__method__",
        "__namespace__",
        "__trait__",
        "__property__",
    ];
    word.len() >= 7 && NAMES.iter().any(|name| word.eq_ignore_ascii_case(name))
}

/// The keyword a word spells, in any case.
pub fn keyword_kind(word: &str) -> Option<SyntaxKind> {
    if word.len() > 15 || !word.is_ascii() {
        return None;
    }
    let mut buffer = [0u8; 15];
    for (slot, byte) in buffer.iter_mut().zip(word.bytes()) {
        *slot = byte.to_ascii_lowercase();
    }
    let lower = std::str::from_utf8(&buffer[..word.len()]).ok()?;
    Some(match lower {
        "abstract" => ABSTRACT_KW,
        "and" => AND_KW,
        "array" => ARRAY_KW,
        "as" => AS_KW,
        "break" => BREAK_KW,
        "callable" => CALLABLE_KW,
        "case" => CASE_KW,
        "catch" => CATCH_KW,
        "class" => CLASS_KW,
        "clone" => CLONE_KW,
        "const" => CONST_KW,
        "continue" => CONTINUE_KW,
        "declare" => DECLARE_KW,
        "default" => DEFAULT_KW,
        "do" => DO_KW,
        "echo" => ECHO_KW,
        "else" => ELSE_KW,
        "elseif" => ELSEIF_KW,
        "empty" => EMPTY_KW,
        "enddeclare" => ENDDECLARE_KW,
        "endfor" => ENDFOR_KW,
        "endforeach" => ENDFOREACH_KW,
        "endif" => ENDIF_KW,
        "endswitch" => ENDSWITCH_KW,
        "endwhile" => ENDWHILE_KW,
        "eval" => EVAL_KW,
        "exit" | "die" => EXIT_KW,
        "extends" => EXTENDS_KW,
        "final" => FINAL_KW,
        "finally" => FINALLY_KW,
        "fn" => FN_KW,
        "for" => FOR_KW,
        "foreach" => FOREACH_KW,
        "function" => FUNCTION_KW,
        "global" => GLOBAL_KW,
        "goto" => GOTO_KW,
        "__halt_compiler" => HALT_COMPILER_KW,
        "if" => IF_KW,
        "implements" => IMPLEMENTS_KW,
        "include" => INCLUDE_KW,
        "include_once" => INCLUDE_ONCE_KW,
        "instanceof" => INSTANCEOF_KW,
        "insteadof" => INSTEADOF_KW,
        "interface" => INTERFACE_KW,
        "isset" => ISSET_KW,
        "list" => LIST_KW,
        "match" => MATCH_KW,
        "namespace" => NAMESPACE_KW,
        "new" => NEW_KW,
        "or" => OR_KW,
        "print" => PRINT_KW,
        "private" => PRIVATE_KW,
        "protected" => PROTECTED_KW,
        "public" => PUBLIC_KW,
        "readonly" => READONLY_KW,
        "require" => REQUIRE_KW,
        "require_once" => REQUIRE_ONCE_KW,
        "return" => RETURN_KW,
        "static" => STATIC_KW,
        "switch" => SWITCH_KW,
        "throw" => THROW_KW,
        "trait" => TRAIT_KW,
        "try" => TRY_KW,
        "unset" => UNSET_KW,
        "use" => USE_KW,
        "var" => VAR_KW,
        "while" => WHILE_KW,
        "xor" => XOR_KW,
        "yield" => YIELD_KW,
        _ => return None,
    })
}
