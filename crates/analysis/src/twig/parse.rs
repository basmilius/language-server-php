//! Twig expressions and tags, read from the tokens of a template. The reading never fails: what it
//! cannot read becomes `Expr::Missing` and the rest goes on, which completion needs while a person
//! types.

use super::lex::{Kind, Token};

/// A Twig expression, with the offsets in the template of what a person can point at.
#[derive(Clone, Debug, PartialEq)]
pub enum Expr {
    Name {
        name: String,
        start: u32,
        end: u32,
    },
    /// A string, with the offsets of the text inside its quotes.
    Str {
        value: String,
        start: u32,
        end: u32,
    },
    Number,
    Literal(String),
    Array(Vec<Expr>),
    Hash(Vec<(HashKey, Expr)>),
    /// `object.name`, `object.name(args)`.
    Attribute {
        object: Box<Expr>,
        name: String,
        start: u32,
        end: u32,
        args: Option<Vec<Arg>>,
    },
    /// `object[index]`.
    Index {
        object: Box<Expr>,
        index: Box<Expr>,
    },
    /// `name(args)`.
    Call {
        name: String,
        start: u32,
        end: u32,
        args: Vec<Arg>,
    },
    /// `subject|name(args)`.
    Filter {
        subject: Box<Expr>,
        name: String,
        start: u32,
        end: u32,
        args: Vec<Arg>,
    },
    /// `subject is name(args)`, `subject is not name`.
    Test {
        subject: Box<Expr>,
        name: String,
        start: u32,
        end: u32,
        args: Vec<Arg>,
    },
    Unary {
        op: String,
        operand: Box<Expr>,
    },
    Binary {
        op: String,
        left: Box<Expr>,
        right: Box<Expr>,
    },
    Conditional {
        condition: Box<Expr>,
        then: Option<Box<Expr>>,
        otherwise: Option<Box<Expr>>,
    },
    /// `(a, b) => a ~ b`.
    Arrow {
        params: Vec<(String, u32, u32)>,
        body: Box<Expr>,
    },
    /// Where an expression should be and is not.
    Missing(u32),
}

#[derive(Clone, Debug, PartialEq)]
pub enum HashKey {
    Name(String, u32, u32),
    Str(String, u32, u32),
    Expr(Box<Expr>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Arg {
    /// `name: value` or `name = value`.
    pub name: Option<(String, u32, u32)>,
    pub value: Expr,
}

impl Expr {
    /// Calls `visit` with this expression and every one inside it.
    pub fn walk<'a>(&'a self, visit: &mut dyn FnMut(&'a Expr)) {
        visit(self);
        let args = |args: &'a [Arg], visit: &mut dyn FnMut(&'a Expr)| {
            for arg in args {
                arg.value.walk(visit);
            }
        };
        match self {
            Expr::Array(items) => items.iter().for_each(|item| item.walk(visit)),
            Expr::Hash(pairs) => {
                for (key, value) in pairs {
                    if let HashKey::Expr(key) = key {
                        key.walk(visit);
                    }
                    value.walk(visit);
                }
            }
            Expr::Attribute { object, args: list, .. } => {
                object.walk(visit);
                if let Some(list) = list {
                    args(list, visit);
                }
            }
            Expr::Index { object, index } => {
                object.walk(visit);
                index.walk(visit);
            }
            Expr::Call { args: list, .. } => args(list, visit),
            Expr::Filter {
                subject, args: list, ..
            }
            | Expr::Test {
                subject, args: list, ..
            } => {
                subject.walk(visit);
                args(list, visit);
            }
            Expr::Unary { operand, .. } => operand.walk(visit),
            Expr::Binary { left, right, .. } => {
                left.walk(visit);
                right.walk(visit);
            }
            Expr::Conditional {
                condition,
                then,
                otherwise,
            } => {
                condition.walk(visit);
                if let Some(then) = then {
                    then.walk(visit);
                }
                if let Some(otherwise) = otherwise {
                    otherwise.walk(visit);
                }
            }
            Expr::Arrow { body, .. } => body.walk(visit),
            _ => {}
        }
    }
}

/// Reads expressions from a run of tokens.
pub struct Parser<'a> {
    source: &'a str,
    tokens: &'a [Token],
    at: usize,
    /// Where the run ends, for a missing expression at its end.
    end: u32,
}

/// The binary operators by how tightly they bind, and whether they bind to the right.
fn binary(op: &str) -> Option<(u16, bool)> {
    Some(match op {
        "or" => (10, false),
        "xor" => (12, false),
        "and" => (15, false),
        "b-or" => (16, false),
        "b-xor" => (17, false),
        "b-and" => (18, false),
        "==" | "!=" | "<=>" | "<" | ">" | ">=" | "<=" | "in" | "not in" | "matches" | "starts with" | "ends with"
        | "has some" | "has every" => (20, false),
        ".." => (25, false),
        "+" | "-" => (30, false),
        "~" => (40, false),
        "*" | "/" | "//" | "%" => (60, false),
        "**" => (200, true),
        "??" => (300, true),
        _ => return None,
    })
}

impl<'a> Parser<'a> {
    pub fn new(source: &'a str, tokens: &'a [Token], end: u32) -> Parser<'a> {
        Parser {
            source,
            tokens,
            at: 0,
            end,
        }
    }

    pub fn peek(&self) -> Option<&'a Token> {
        self.tokens.get(self.at)
    }

    pub fn peek_text(&self) -> Option<&'a str> {
        self.peek().map(|token| token.text(self.source))
    }

    fn peek_at(&self, ahead: usize) -> Option<&'a str> {
        self.tokens.get(self.at + ahead).map(|token| token.text(self.source))
    }

    pub fn advance(&mut self) -> Option<&'a Token> {
        let token = self.tokens.get(self.at);
        if token.is_some() {
            self.at += 1;
        }
        token
    }

    pub fn eat(&mut self, text: &str) -> bool {
        if self.peek_text() == Some(text) {
            self.at += 1;
            true
        } else {
            false
        }
    }

    pub fn at_end(&self) -> bool {
        self.at >= self.tokens.len()
    }

    fn here(&self) -> u32 {
        self.peek().map_or(self.end, |token| token.start)
    }

    /// A name, with where it is.
    pub fn name(&mut self) -> Option<(String, u32, u32)> {
        let token = self.peek().filter(|token| token.kind == Kind::Name)?;
        self.at += 1;
        Some((token.text(self.source).to_string(), token.start, token.end))
    }

    /// The operator at the cursor, words and pairs of words included.
    fn operator(&self) -> Option<(String, usize)> {
        let first = self.peek()?;
        let text = first.text(self.source);
        if first.kind == Kind::Name {
            let second = self.peek_at(1);
            let pair = match (text, second) {
                ("not", Some("in")) => Some("not in"),
                ("starts", Some("with")) => Some("starts with"),
                ("ends", Some("with")) => Some("ends with"),
                ("has", Some("some")) => Some("has some"),
                ("has", Some("every")) => Some("has every"),
                _ => None,
            };
            if let Some(pair) = pair {
                return Some((pair.to_string(), 2));
            }
            return matches!(
                text,
                "or" | "and" | "xor" | "b-or" | "b-xor" | "b-and" | "in" | "matches" | "is"
            )
            .then(|| (text.to_string(), 1));
        }
        (first.kind == Kind::Punct).then(|| (text.to_string(), 1))
    }

    /// The filters of an `apply` tag, `upper|escape('html')`, on a subject that is not written.
    pub fn filter_chain(&mut self, mut subject: Expr) -> Expr {
        while let Some((name, start, end)) = self.name() {
            let args = if self.peek_text() == Some("(") {
                self.arguments()
            } else {
                Vec::new()
            };
            subject = Expr::Filter {
                subject: Box::new(subject),
                name,
                start,
                end,
                args,
            };
            if !self.eat("|") {
                break;
            }
        }
        subject
    }

    pub fn expression(&mut self) -> Expr {
        self.expression_bp(0)
    }

    fn expression_bp(&mut self, min: u16) -> Expr {
        let mut left = self.unary();
        while let Some((op, width)) = self.operator() {
            if op == "is" {
                if 100 < min {
                    break;
                }
                self.at += width;
                let negated = self.peek_text() == Some("not");
                if negated {
                    self.at += 1;
                }
                left = self.test(left, negated);
                continue;
            }
            if op == "?" || op == "?:" {
                if min > 0 {
                    break;
                }
                self.at += 1;
                left = if op == "?:" {
                    let otherwise = self.expression_bp(0);
                    Expr::Conditional {
                        condition: Box::new(left),
                        then: None,
                        otherwise: Some(Box::new(otherwise)),
                    }
                } else {
                    let then = self.expression_bp(0);
                    let otherwise = self.eat(":").then(|| Box::new(self.expression_bp(0)));
                    Expr::Conditional {
                        condition: Box::new(left),
                        then: Some(Box::new(then)),
                        otherwise,
                    }
                };
                continue;
            }
            let Some((precedence, right)) = binary(&op) else {
                break;
            };
            if precedence < min {
                break;
            }
            self.at += width;
            let next = if right { precedence } else { precedence + 1 };
            let rhs = self.expression_bp(next);
            left = Expr::Binary {
                op,
                left: Box::new(left),
                right: Box::new(rhs),
            };
        }
        left
    }

    fn test(&mut self, subject: Expr, negated: bool) -> Expr {
        let Some((mut name, start, mut end)) = self.name() else {
            return Expr::Test {
                subject: Box::new(subject),
                name: String::new(),
                start: self.here(),
                end: self.here(),
                args: Vec::new(),
            };
        };
        // Tests of two words: `same as`, `divisible by`.
        if let Some(second) = self.peek().filter(|token| token.kind == Kind::Name) {
            let word = second.text(self.source);
            if matches!((name.as_str(), word), ("same", "as") | ("divisible", "by")) {
                name = format!("{name} {word}");
                end = second.end;
                self.at += 1;
            }
        }
        let args = if self.peek_text() == Some("(") {
            self.arguments()
        } else if self.starts_lone_argument() {
            // A test of one argument may take it without parentheses: `is same as null`.
            vec![Arg {
                name: None,
                value: self.unary(),
            }]
        } else {
            Vec::new()
        };
        let test = Expr::Test {
            subject: Box::new(subject),
            name,
            start,
            end,
            args,
        };
        if negated {
            Expr::Unary {
                op: "not".to_string(),
                operand: Box::new(test),
            }
        } else {
            test
        }
    }

    fn starts_lone_argument(&self) -> bool {
        let Some(token) = self.peek() else {
            return false;
        };
        match token.kind {
            Kind::Number | Kind::Str => true,
            Kind::Name => !matches!(
                token.text(self.source),
                "and"
                    | "or"
                    | "xor"
                    | "not"
                    | "in"
                    | "is"
                    | "matches"
                    | "starts"
                    | "ends"
                    | "has"
                    | "if"
                    | "else"
                    | "with"
                    | "only"
                    | "as"
                    | "b-and"
                    | "b-or"
                    | "b-xor"
            ),
            Kind::Punct => matches!(token.text(self.source), "[" | "{"),
            _ => false,
        }
    }

    fn unary(&mut self) -> Expr {
        match self.peek_text() {
            Some("not") => {
                self.at += 1;
                let operand = self.expression_bp(50);
                return Expr::Unary {
                    op: "not".to_string(),
                    operand: Box::new(operand),
                };
            }
            Some(op @ ("-" | "+")) => {
                self.at += 1;
                let operand = self.expression_bp(500);
                return Expr::Unary {
                    op: op.to_string(),
                    operand: Box::new(operand),
                };
            }
            _ => {}
        }
        let primary = self.primary();
        self.postfix(primary)
    }

    fn primary(&mut self) -> Expr {
        let Some(token) = self.peek() else {
            return Expr::Missing(self.end);
        };
        let text = token.text(self.source);
        match token.kind {
            Kind::Name => {
                self.at += 1;
                if self.peek_text() == Some("=>") {
                    self.at += 1;
                    let body = self.expression_bp(0);
                    return Expr::Arrow {
                        params: vec![(text.to_string(), token.start, token.end)],
                        body: Box::new(body),
                    };
                }
                match text {
                    "true" | "false" | "null" | "none" | "TRUE" | "FALSE" | "NULL" | "NONE" => {
                        Expr::Literal(text.to_ascii_lowercase())
                    }
                    _ if self.peek_text() == Some("(") => {
                        let args = self.arguments();
                        Expr::Call {
                            name: text.to_string(),
                            start: token.start,
                            end: token.end,
                            args,
                        }
                    }
                    _ => Expr::Name {
                        name: text.to_string(),
                        start: token.start,
                        end: token.end,
                    },
                }
            }
            Kind::Number => {
                self.at += 1;
                Expr::Number
            }
            Kind::Str => {
                self.at += 1;
                let inner_start = token.start + 1;
                let inner_end = token.end.saturating_sub(1).max(inner_start);
                let value = self.source[inner_start as usize..inner_end as usize].to_string();
                Expr::Str {
                    value,
                    start: inner_start,
                    end: inner_end,
                }
            }
            Kind::Punct if text == "(" => {
                if let Some(arrow) = self.arrow() {
                    return arrow;
                }
                self.at += 1;
                let inner = self.expression_bp(0);
                self.eat(")");
                inner
            }
            Kind::Punct if text == "[" => {
                self.at += 1;
                let mut items = Vec::new();
                while !self.at_end() && self.peek_text() != Some("]") {
                    let before = self.at;
                    self.eat("...");
                    items.push(self.expression_bp(0));
                    if !self.eat(",") && self.at == before {
                        break;
                    }
                    if self.at == before {
                        self.at += 1;
                    }
                }
                self.eat("]");
                Expr::Array(items)
            }
            Kind::Punct if text == "{" => self.hash(),
            _ => Expr::Missing(token.start),
        }
    }

    /// `(a, b) => ...`, when the parentheses hold names and an arrow follows.
    fn arrow(&mut self) -> Option<Expr> {
        let start = self.at;
        self.at += 1;
        let mut params = Vec::new();
        while let Some(param) = self.name() {
            params.push(param);
            if !self.eat(",") {
                break;
            }
        }
        if self.eat(")") && self.eat("=>") {
            let body = self.expression_bp(0);
            return Some(Expr::Arrow {
                params,
                body: Box::new(body),
            });
        }
        self.at = start;
        None
    }

    fn hash(&mut self) -> Expr {
        self.at += 1;
        let mut pairs = Vec::new();
        while !self.at_end() && self.peek_text() != Some("}") {
            let before = self.at;
            let key = match self.peek() {
                Some(token) if token.kind == Kind::Name => {
                    self.at += 1;
                    HashKey::Name(token.text(self.source).to_string(), token.start, token.end)
                }
                Some(token) if token.kind == Kind::Str => {
                    self.at += 1;
                    let inner_start = token.start + 1;
                    let inner_end = token.end.saturating_sub(1).max(inner_start);
                    HashKey::Str(
                        self.source[inner_start as usize..inner_end as usize].to_string(),
                        inner_start,
                        inner_end,
                    )
                }
                Some(token) if token.kind == Kind::Number => {
                    self.at += 1;
                    HashKey::Expr(Box::new(Expr::Number))
                }
                _ => HashKey::Expr(Box::new(self.expression_bp(0))),
            };
            let value = if self.eat(":") {
                self.expression_bp(0)
            } else {
                match &key {
                    HashKey::Name(name, start, end) => Expr::Name {
                        name: name.clone(),
                        start: *start,
                        end: *end,
                    },
                    _ => Expr::Missing(self.here()),
                }
            };
            pairs.push((key, value));
            if !self.eat(",") {
                if self.at == before {
                    self.at += 1;
                }
                if self.peek_text() != Some("}") {
                    break;
                }
            }
        }
        self.eat("}");
        Expr::Hash(pairs)
    }

    fn arguments(&mut self) -> Vec<Arg> {
        self.at += 1;
        let mut args = Vec::new();
        while !self.at_end() && self.peek_text() != Some(")") {
            let before = self.at;
            let named = match (self.peek(), self.peek_at(1)) {
                (Some(token), Some(":" | "=")) if token.kind == Kind::Name => {
                    self.at += 2;
                    Some((token.text(self.source).to_string(), token.start, token.end))
                }
                _ => None,
            };
            self.eat("...");
            let value = self.expression_bp(0);
            args.push(Arg { name: named, value });
            if !self.eat(",") {
                if self.at == before {
                    self.at += 1;
                }
                if self.peek_text() != Some(")") {
                    break;
                }
            }
        }
        self.eat(")");
        args
    }

    fn postfix(&mut self, mut expr: Expr) -> Expr {
        loop {
            match self.peek_text() {
                Some("." | "?.") => {
                    let dot_end = self.peek().map_or(self.end, |token| token.end);
                    self.at += 1;
                    let (name, start, end) = match self.peek() {
                        Some(token) if matches!(token.kind, Kind::Name | Kind::Number) && token.start == dot_end => {
                            self.at += 1;
                            (token.text(self.source).to_string(), token.start, token.end)
                        }
                        _ => (String::new(), dot_end, dot_end),
                    };
                    let args = (self.peek_text() == Some("(")).then(|| self.arguments());
                    expr = Expr::Attribute {
                        object: Box::new(expr),
                        name,
                        start,
                        end,
                        args,
                    };
                }
                Some("[") => {
                    self.at += 1;
                    let index = if self.peek_text() == Some(":") {
                        Expr::Number
                    } else {
                        self.expression_bp(0)
                    };
                    while !self.at_end() && self.peek_text() != Some("]") {
                        self.at += 1;
                    }
                    self.eat("]");
                    expr = Expr::Index {
                        object: Box::new(expr),
                        index: Box::new(index),
                    };
                }
                Some("|") => {
                    let bar_end = self.peek().map_or(self.end, |token| token.end);
                    self.at += 1;
                    let (name, start, end) = self.name().unwrap_or((String::new(), bar_end, bar_end));
                    let args = if self.peek_text() == Some("(") {
                        self.arguments()
                    } else {
                        Vec::new()
                    };
                    expr = Expr::Filter {
                        subject: Box::new(expr),
                        name,
                        start,
                        end,
                        args,
                    };
                }
                _ => return expr,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::lex::lex;
    use super::*;

    fn parse_one(text: &str) -> Expr {
        let source = format!("{{{{ {text} }}}}");
        let tokens = lex(&source);
        let inner = &tokens[1..tokens.len() - 1];
        Parser::new(&source, inner, source.len() as u32).expression()
    }

    fn shape(expr: &Expr) -> String {
        match expr {
            Expr::Name { name, .. } => name.clone(),
            Expr::Str { value, .. } => format!("'{value}'"),
            Expr::Number => "1".to_string(),
            Expr::Literal(text) => text.clone(),
            Expr::Array(items) => format!("[{}]", items.iter().map(shape).collect::<Vec<_>>().join(", ")),
            Expr::Hash(pairs) => format!(
                "{{{}}}",
                pairs
                    .iter()
                    .map(|(key, value)| {
                        let key = match key {
                            HashKey::Name(name, ..) | HashKey::Str(name, ..) => name.clone(),
                            HashKey::Expr(expr) => shape(expr),
                        };
                        format!("{key}: {}", shape(value))
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Expr::Attribute { object, name, args, .. } => match args {
                Some(args) => format!("{}.{name}({})", shape(object), args_shape(args)),
                None => format!("{}.{name}", shape(object)),
            },
            Expr::Index { object, index } => format!("{}[{}]", shape(object), shape(index)),
            Expr::Call { name, args, .. } => format!("{name}({})", args_shape(args)),
            Expr::Filter {
                subject, name, args, ..
            } => format!("({}|{name}({}))", shape(subject), args_shape(args)),
            Expr::Test { subject, name, .. } => format!("({} is {name})", shape(subject)),
            Expr::Unary { op, operand } => format!("({op} {})", shape(operand)),
            Expr::Binary { op, left, right } => format!("({} {op} {})", shape(left), shape(right)),
            Expr::Conditional {
                condition,
                then,
                otherwise,
            } => format!(
                "({} ? {} : {})",
                shape(condition),
                then.as_deref().map(shape).unwrap_or_default(),
                otherwise.as_deref().map(shape).unwrap_or_default()
            ),
            Expr::Arrow { params, body } => format!(
                "({}) => {}",
                params
                    .iter()
                    .map(|(name, ..)| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                shape(body)
            ),
            Expr::Missing(_) => "?".to_string(),
        }
    }

    fn args_shape(args: &[Arg]) -> String {
        args.iter()
            .map(|arg| match &arg.name {
                Some((name, ..)) => format!("{name}: {}", shape(&arg.value)),
                None => shape(&arg.value),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    #[test]
    fn reads_expressions_by_precedence() {
        assert_eq!(shape(&parse_one("a + b * c ~ d")), "(a + ((b * c) ~ d))");
        assert_eq!(shape(&parse_one("not a and b or c")), "(((not a) and b) or c)");
        assert_eq!(
            shape(&parse_one("post.author.name|upper|slice(0, 3)")),
            "((post.author.name|upper())|slice(1, 1))"
        );
        assert_eq!(
            shape(&parse_one("path('blog', {page: 1, tag})")),
            "path('blog', {page: 1, tag: tag})"
        );
        assert_eq!(
            shape(&parse_one("x is not defined ? y : z")),
            "((not (x is defined)) ? y : z)"
        );
        assert_eq!(
            shape(&parse_one("items|filter(i => i.ok)|map((k, v) => v)")),
            "((items|filter((i) => i.ok))|map((k, v) => v))"
        );
        assert_eq!(shape(&parse_one("a['b'].c(d: 1) ?? 'e'")), "(a['b'].c(d: 1) ?? 'e')");
        assert_eq!(shape(&parse_one("1..5")), "(1 .. 1)");
        assert_eq!(shape(&parse_one("x is divisible by(3)")), "(x is divisible by)");
        assert_eq!(
            shape(&parse_one("x is not same as null and y")),
            "((not (x is same as)) and y)"
        );
    }

    #[test]
    fn reads_what_is_half_written() {
        assert_eq!(shape(&parse_one("post.")), "post.");
        assert_eq!(shape(&parse_one("post|")), "(post|())");
        assert_eq!(shape(&parse_one("path(")), "path()");
        assert_eq!(shape(&parse_one("a +")), "(a + ?)");
    }
}
