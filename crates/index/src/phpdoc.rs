//! Doc comments: the summary, the prose and the tags, and the grammar of PHPDoc types (generics,
//! array shapes, `callable(...)`, `class-string<T>`, conditional types).

use php_syntax::PhpVersion;

use crate::model::{
    AssertWhen, Doc, DocAssert, DocMethod, DocParam, DocProperty, RawTag, Span, Template, TypeAlias, TypeImport,
    encode_version,
};
use crate::resolve::NameResolver;
use crate::types::{CallableParam, CallableType, Conditional, ShapeField, Type};

/// What a type in a doc comment is read against.
pub struct TypeContext<'a> {
    pub resolver: &'a NameResolver,
    /// The class the comment is in, which `self` names.
    pub class_name: Option<&'a str>,
    pub parent_name: Option<&'a str>,
    pub in_trait: bool,
    /// Names of `@template` in scope.
    pub templates: Vec<String>,
    /// The type aliases of the class, by name.
    pub aliases: Vec<(String, Type)>,
}

impl<'a> TypeContext<'a> {
    pub fn new(resolver: &'a NameResolver) -> TypeContext<'a> {
        TypeContext {
            resolver,
            class_name: None,
            parent_name: None,
            in_trait: false,
            templates: Vec::new(),
            aliases: Vec::new(),
        }
    }

    /// The type a written class name stands for.
    pub fn class_type(&self, raw: &str) -> Type {
        if self.templates.iter().any(|name| name == raw) {
            return Type::Template(raw.to_string());
        }
        if let Some((_, ty)) = self.aliases.iter().find(|(name, _)| name == raw) {
            return ty.clone();
        }
        match raw.to_ascii_lowercase().as_str() {
            "self" => match self.class_name {
                Some(name) if !self.in_trait => Type::class(name),
                _ => Type::SelfType,
            },
            "static" => Type::Static,
            "parent" => match self.parent_name {
                Some(name) => Type::class(name),
                None => Type::Parent,
            },
            _ => Type::class(self.resolver.resolve_class(raw)),
        }
    }
}

/// Parses a type written as a string, such as the argument of `#[LanguageLevelTypeAware]`. Returns
/// `None` when nothing of it reads as a type.
pub fn parse_type(text: &str, cx: &TypeContext) -> Option<Type> {
    let mut parser = TypeParser::new(text, cx);
    parser.top()
}

/// Parses a type at the start of a text and says how many bytes it took.
pub fn parse_type_prefix(text: &str, cx: &TypeContext) -> Option<(Type, usize)> {
    let mut parser = TypeParser::new(text, cx);
    let ty = parser.top()?;
    Some((ty, parser.pos))
}

struct TypeParser<'a> {
    src: &'a str,
    pos: usize,
    cx: &'a TypeContext<'a>,
    depth: u32,
}

const MAX_DEPTH: u32 = 32;

impl<'a> TypeParser<'a> {
    fn new(src: &'a str, cx: &'a TypeContext<'a>) -> TypeParser<'a> {
        TypeParser {
            src,
            pos: 0,
            cx,
            depth: 0,
        }
    }

    fn rest(&self) -> &'a str {
        &self.src[self.pos..]
    }

    fn peek(&self) -> Option<char> {
        self.rest().chars().next()
    }

    fn eat(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.pos += expected.len_utf8();
            true
        } else {
            false
        }
    }

    fn skip_ws(&mut self) {
        let rest = self.rest();
        self.pos += rest.len() - rest.trim_start().len();
    }

    fn top(&mut self) -> Option<Type> {
        self.skip_ws();
        self.conditional_or_union()
    }

    fn conditional_or_union(&mut self) -> Option<Type> {
        if self.depth > MAX_DEPTH {
            return None;
        }
        self.depth += 1;
        let result = self.conditional_or_union_inner();
        self.depth -= 1;
        result
    }

    fn conditional_or_union_inner(&mut self) -> Option<Type> {
        let start = self.pos;
        let mut subject = None;
        let first = if self.peek() == Some('$') && !self.rest().starts_with("$this") {
            let name = self.variable_name()?;
            subject = Some(name);
            None
        } else {
            let ty = self.union()?;
            Some(ty)
        };
        let after = self.pos;
        self.skip_ws();
        let is_keyword = self.rest().starts_with("is ") || self.rest().starts_with("is\t");
        if !is_keyword {
            self.pos = after;
            if let Some(ty) = first {
                return Some(ty);
            }
            self.pos = start;
            return self.union();
        }
        let subject = match (subject, &first) {
            (Some(name), _) => name,
            (None, Some(Type::Template(name))) => name.clone(),
            (None, Some(other)) => other.display(false),
            _ => return None,
        };
        self.pos += 2;
        self.skip_ws();
        let negated = if self.rest().starts_with("not ") {
            self.pos += 4;
            self.skip_ws();
            true
        } else {
            false
        };
        let is = self.union()?;
        self.skip_ws();
        if !self.eat('?') {
            return None;
        }
        self.skip_ws();
        let then = self.conditional_or_union()?;
        self.skip_ws();
        if !self.eat(':') {
            return None;
        }
        self.skip_ws();
        let otherwise = self.conditional_or_union()?;
        Some(Type::Conditional(Box::new(Conditional {
            subject,
            is,
            negated,
            then,
            otherwise,
        })))
    }

    fn variable_name(&mut self) -> Option<String> {
        let rest = self.rest();
        let bytes = rest.as_bytes();
        if bytes.first() != Some(&b'$') {
            return None;
        }
        let mut end = 1;
        while end < bytes.len() && (bytes[end].is_ascii_alphanumeric() || bytes[end] == b'_' || bytes[end] >= 0x80) {
            end += 1;
        }
        if end == 1 {
            return None;
        }
        self.pos += end;
        Some(rest[..end].to_string())
    }

    fn union(&mut self) -> Option<Type> {
        let mut members = vec![self.intersection()?];
        while self.peek() == Some('|') {
            self.pos += 1;
            members.push(self.intersection()?);
        }
        Some(Type::union(members))
    }

    fn intersection(&mut self) -> Option<Type> {
        let mut members = vec![self.postfix()?];
        loop {
            if self.peek() != Some('&') {
                break;
            }
            let next = self.rest()[1..].chars().next();
            if next.is_none_or(|c| c.is_whitespace() || c == '$' || c == '.' || c == '&') {
                break;
            }
            self.pos += 1;
            members.push(self.postfix()?);
        }
        if members.len() == 1 {
            return members.pop();
        }
        Some(Type::Intersection(members))
    }

    fn postfix(&mut self) -> Option<Type> {
        let mut ty = self.atom()?;
        while self.rest().starts_with("[]") {
            self.pos += 2;
            ty = Type::List(Box::new(ty));
        }
        Some(ty)
    }

    fn atom(&mut self) -> Option<Type> {
        match self.peek()? {
            '?' => {
                self.pos += 1;
                let inner = self.atom()?;
                Some(inner.nullable())
            }
            '(' => {
                self.pos += 1;
                self.skip_ws();
                let inner = self.conditional_or_union()?;
                self.skip_ws();
                if !self.eat(')') {
                    return None;
                }
                Some(inner)
            }
            '\'' | '"' => self.string_literal(),
            '$' => {
                if self.rest().starts_with("$this") {
                    self.pos += 5;
                    Some(Type::Static)
                } else {
                    None
                }
            }
            c if c.is_ascii_digit() || c == '-' => self.number(),
            c if is_name_start(c) => self.name(),
            _ => None,
        }
    }

    fn string_literal(&mut self) -> Option<Type> {
        let quote = self.peek()?;
        let rest = &self.rest()[1..];
        let end = rest.find(quote)?;
        let value = rest[..end].to_string();
        self.pos += end + 2;
        Some(Type::StringLiteral(value))
    }

    fn number(&mut self) -> Option<Type> {
        let rest = self.rest();
        let mut end = 0;
        let bytes = rest.as_bytes();
        if bytes[0] == b'-' {
            end = 1;
        }
        let digits_start = end;
        while end < bytes.len() && (bytes[end].is_ascii_digit() || bytes[end] == b'_') {
            end += 1;
        }
        if end == digits_start {
            return None;
        }
        let mut is_float = false;
        if end + 1 < bytes.len() && bytes[end] == b'.' && bytes[end + 1].is_ascii_digit() {
            is_float = true;
            end += 1;
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
        }
        let text = &rest[..end];
        self.pos += end;
        if is_float {
            return Some(Type::Float);
        }
        Some(text.replace('_', "").parse().map(Type::IntLiteral).unwrap_or(Type::Int))
    }

    fn name(&mut self) -> Option<Type> {
        let rest = self.rest();
        let mut end = 0;
        for (index, c) in rest.char_indices() {
            if is_name_char(c) {
                end = index + c.len_utf8();
            } else {
                break;
            }
        }
        // A trailing hyphen belongs to prose ("string- ..."), not to the name.
        let raw = rest[..end].trim_end_matches('-');
        if raw.is_empty() {
            return None;
        }
        self.pos += raw.len();
        let lower = raw.to_ascii_lowercase();

        if self.rest().starts_with("::") {
            self.pos += 2;
            let rest = self.rest();
            let consumed = rest
                .char_indices()
                .find(|(_, c)| !(c.is_alphanumeric() || *c == '_' || *c == '*'))
                .map_or(rest.len(), |(index, _)| index);
            self.pos += consumed;
            return Some(Type::Mixed);
        }

        match lower.as_str() {
            "array" | "non-empty-array" | "associative-array" => return Some(self.array_like(false)),
            "list" | "non-empty-list" => return Some(self.array_like(true)),
            "iterable" => return Some(self.iterable()),
            "class-string" | "trait-string" | "interface-string" | "enum-string" => {
                if self.rest().starts_with('<') {
                    let args = self.generic_args()?;
                    return Some(Type::ClassString(args.into_iter().next().map(Box::new)));
                }
                return Some(Type::ClassString(None));
            }
            "callable" | "pure-callable" | "closure" | "\\closure" => {
                let closure = lower.trim_start_matches('\\') == "closure";
                if self.rest().starts_with('(') {
                    return self.callable_signature(closure);
                }
                if closure {
                    return Some(Type::class("Closure"));
                }
                return Some(Type::Callable(None));
            }
            "object" => {
                if self.rest().starts_with('{') {
                    self.skip_braces();
                }
                return Some(Type::Object);
            }
            "int" | "integer" => {
                if self.rest().starts_with('<') {
                    self.generic_args()?;
                }
                return Some(Type::Int);
            }
            "int-mask-of" | "int-mask" => {
                if self.rest().starts_with('<') {
                    self.generic_args()?;
                }
                return Some(Type::Int);
            }
            "key-of" | "value-of" | "properties-of" | "new" | "template-type" => {
                if self.rest().starts_with('<') {
                    self.generic_args()?;
                }
                return Some(Type::Mixed);
            }
            _ => {}
        }
        if let Some(simple) = simple_keyword(&lower) {
            return Some(simple);
        }
        let ty = self.cx.class_type(raw);
        if self.rest().starts_with('<') {
            let args = self.generic_args()?;
            if let Type::Class { name, .. } = ty {
                return Some(Type::Class { name, args });
            }
        }
        Some(ty)
    }

    fn skip_braces(&mut self) {
        let mut depth = 0;
        for (index, c) in self.rest().char_indices() {
            match c {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        self.pos += index + 1;
                        return;
                    }
                }
                _ => {}
            }
        }
        self.pos = self.src.len();
    }

    fn generic_args(&mut self) -> Option<Vec<Type>> {
        if !self.eat('<') {
            return None;
        }
        let mut args = Vec::new();
        loop {
            self.skip_ws();
            if self.eat('>') {
                break;
            }
            if self.eat('*') {
                args.push(Type::Mixed);
            } else if self.rest().starts_with("max") || self.rest().starts_with("min") {
                self.pos += 3;
                args.push(Type::Int);
            } else {
                args.push(self.conditional_or_union()?);
            }
            self.skip_ws();
            if self.eat(',') {
                continue;
            }
            self.skip_ws();
            if !self.eat('>') {
                return None;
            }
            break;
        }
        Some(args)
    }

    fn array_like(&mut self, list: bool) -> Type {
        if self.rest().starts_with('{') {
            return self.shape(list);
        }
        if self.rest().starts_with('<') {
            let saved = self.pos;
            match self.generic_args() {
                Some(mut args) => {
                    return match (list, args.len()) {
                        (true, _) => Type::List(Box::new(args.remove(0))),
                        (false, 1) => Type::array_of(args.remove(0)),
                        (false, _) => {
                            let key = args.remove(0);
                            Type::Array(Box::new(key), Box::new(args.remove(0)))
                        }
                    };
                }
                None => self.pos = saved,
            }
        }
        if list {
            Type::List(Box::new(Type::Mixed))
        } else {
            Type::plain_array()
        }
    }

    fn iterable(&mut self) -> Type {
        if self.rest().starts_with('<') {
            let saved = self.pos;
            match self.generic_args() {
                Some(mut args) if args.len() == 1 => {
                    return Type::Iterable(Box::new(Type::ArrayKey), Box::new(args.remove(0)));
                }
                Some(mut args) if args.len() >= 2 => {
                    let key = args.remove(0);
                    return Type::Iterable(Box::new(key), Box::new(args.remove(0)));
                }
                _ => self.pos = saved,
            }
        }
        Type::Iterable(Box::new(Type::Mixed), Box::new(Type::Mixed))
    }

    fn shape(&mut self, list: bool) -> Type {
        let start = self.pos;
        self.pos += 1;
        let mut fields = Vec::new();
        let mut index = 0u32;
        loop {
            self.skip_ws();
            if self.eat('}') {
                break;
            }
            if self.rest().starts_with("...") {
                self.pos += 3;
                self.skip_ws();
                if self.peek() != Some('}') && self.peek() != Some(',') {
                    let _ = self.conditional_or_union();
                }
                self.skip_ws();
                self.eat(',');
                continue;
            }
            let Some(field) = self.shape_field(&mut index, list) else {
                self.pos = start;
                self.skip_braces();
                return if list {
                    Type::List(Box::new(Type::Mixed))
                } else {
                    Type::plain_array()
                };
            };
            fields.push(field);
            self.skip_ws();
            if self.eat(',') {
                continue;
            }
            self.skip_ws();
            if !self.eat('}') {
                self.pos = start;
                self.skip_braces();
                return Type::plain_array();
            }
            break;
        }
        Type::Shape(fields)
    }

    fn shape_field(&mut self, index: &mut u32, list: bool) -> Option<ShapeField> {
        let saved = self.pos;
        if let Some((key, optional)) = self.shape_key() {
            self.skip_ws();
            let ty = self.conditional_or_union()?;
            return Some(ShapeField {
                key: Some(key),
                optional,
                ty,
            });
        }
        self.pos = saved;
        let ty = self.conditional_or_union()?;
        let key = list.then(|| {
            let key = index.to_string();
            *index += 1;
            key
        });
        Some(ShapeField {
            key,
            optional: false,
            ty,
        })
    }

    /// A key followed by `:` or `?:`, which tells a shape entry from a bare type.
    fn shape_key(&mut self) -> Option<(String, bool)> {
        let rest = self.rest();
        let (key, consumed) = match rest.chars().next()? {
            quote @ ('\'' | '"') => {
                let inner = &rest[1..];
                let end = inner.find(quote)?;
                (inner[..end].to_string(), end + 2)
            }
            c if c.is_alphanumeric() || c == '_' || c == '-' => {
                let end = rest
                    .char_indices()
                    .find(|(_, c)| !(c.is_alphanumeric() || *c == '_'))
                    .map_or(rest.len(), |(index, _)| index);
                (rest[..end].to_string(), end)
            }
            _ => return None,
        };
        let after = &rest[consumed..];
        if let Some(rest) = after.strip_prefix("?:") {
            self.pos += rest.as_ptr() as usize - self.rest().as_ptr() as usize;
            return Some((key, true));
        }
        if let Some(rest) = after.strip_prefix(':') {
            if rest.starts_with(':') {
                return None;
            }
            self.pos += rest.as_ptr() as usize - self.rest().as_ptr() as usize;
            return Some((key, false));
        }
        None
    }

    fn callable_signature(&mut self, closure: bool) -> Option<Type> {
        if !self.eat('(') {
            return None;
        }
        let mut params = Vec::new();
        loop {
            self.skip_ws();
            if self.eat(')') {
                break;
            }
            let ty = self.conditional_or_union()?;
            self.skip_ws();
            let mut param = CallableParam {
                ty,
                optional: false,
                variadic: false,
                by_ref: false,
                name: None,
            };
            if self.eat('&') {
                param.by_ref = true;
                self.skip_ws();
            }
            if self.rest().starts_with("...") {
                self.pos += 3;
                param.variadic = true;
                self.skip_ws();
            }
            if self.peek() == Some('$') {
                let name = self.variable_name()?;
                param.name = Some(name);
            }
            if self.eat('=') {
                param.optional = true;
            }
            params.push(param);
            self.skip_ws();
            if self.eat(',') {
                continue;
            }
            if !self.eat(')') {
                return None;
            }
            break;
        }
        let mut ret = None;
        let before = self.pos;
        self.skip_ws();
        if self.eat(':') {
            self.skip_ws();
            ret = Some(self.postfix_union_for_return()?);
        } else {
            self.pos = before;
        }
        Some(Type::Callable(Some(Box::new(CallableType { closure, params, ret }))))
    }

    /// A callable's return type binds tighter than the union around the callable, so
    /// `callable(): int|string` is one callable returning a union, as PHPStan reads it.
    fn postfix_union_for_return(&mut self) -> Option<Type> {
        self.union()
    }
}

fn is_name_start(c: char) -> bool {
    c.is_alphabetic() || c == '_' || c == '\\' || c as u32 >= 0x80
}

fn is_name_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '\\' || c == '-' || c as u32 >= 0x80
}

fn simple_keyword(lower: &str) -> Option<Type> {
    Some(match lower {
        "mixed" => Type::Mixed,
        "void" => Type::Void,
        "never" | "never-return" | "never-returns" | "no-return" | "noreturn" => Type::Never,
        "null" => Type::Null,
        "bool" | "boolean" => Type::Bool,
        "true" => Type::True,
        "false" => Type::False,
        "positive-int" | "negative-int" | "non-positive-int" | "non-negative-int" | "non-zero-int" => Type::Int,
        "float" | "double" => Type::Float,
        "string"
        | "numeric-string"
        | "non-empty-string"
        | "literal-string"
        | "lowercase-string"
        | "uppercase-string"
        | "callable-string"
        | "truthy-string"
        | "non-falsy-string"
        | "non-empty-lowercase-string"
        | "non-empty-uppercase-string"
        | "html-escaped-string"
        | "non-empty-literal-string" => Type::String,
        "resource" | "closed-resource" | "open-resource" => Type::Resource,
        "callable-array" => Type::plain_array(),
        "scalar" => Type::Scalar,
        "numeric" => Type::Numeric,
        "array-key" => Type::ArrayKey,
        "empty" => Type::Null,
        "self" | "static" | "parent" => return None,
        _ => return None,
    })
}

/// The text of a doc comment with its `/**`, `*/` and the star at the start of each line removed.
pub fn clean_comment(raw: &str) -> Vec<String> {
    let body = raw.strip_prefix("/**").unwrap_or(raw);
    let body = body.strip_suffix("*/").unwrap_or(body);
    body.lines()
        .map(|line| {
            let line = line.trim_end();
            let trimmed = line.trim_start();
            match trimmed.strip_prefix('*') {
                Some(rest) => rest.strip_prefix(' ').unwrap_or(rest).to_string(),
                None => trimmed.to_string(),
            }
        })
        .collect()
}

/// Reads a doc comment into its parts. `stub` marks a file whose `@since` and `@removed` are PHP
/// versions; in a project they are the project's own and mean nothing here.
pub fn parse_doc(raw: &str, cx: &TypeContext, stub: bool) -> Doc {
    let lines = clean_comment(raw);
    let mut prose: Vec<String> = Vec::new();
    let mut tags: Vec<(String, String)> = Vec::new();
    let mut in_fence = false;
    for line in lines {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
        }
        if !in_fence && let Some(rest) = trimmed.strip_prefix('@') {
            let name_end = rest.find(|c: char| c.is_whitespace() || c == '(').unwrap_or(rest.len());
            let name = rest[..name_end].to_string();
            if !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == ':')
            {
                tags.push((name, rest[name_end..].trim_start().to_string()));
                continue;
            }
        }
        match tags.last_mut() {
            Some((_, text)) => {
                if !line.trim().is_empty() || !text.is_empty() {
                    text.push('\n');
                    text.push_str(line.trim_end());
                }
            }
            None => prose.push(line),
        }
    }

    let mut doc = Doc::default();
    let (summary, description) = split_prose(&prose);
    doc.summary = summary;
    doc.description = description;

    // Templates first: the other tags may name them.
    let mut scoped = TypeContext {
        resolver: cx.resolver,
        class_name: cx.class_name,
        parent_name: cx.parent_name,
        in_trait: cx.in_trait,
        templates: cx.templates.clone(),
        aliases: cx.aliases.clone(),
    };
    for (name, text) in &tags {
        if is_tag(name, "template")
            || name == "template-covariant"
            || name == "template-contravariant"
            || name.ends_with("-template")
            || name.ends_with("-template-covariant")
            || name.ends_with("-template-contravariant")
        {
            let template = parse_template(text, &scoped);
            if let Some(template) = template {
                scoped.templates.push(template.name.clone());
                doc.templates.push(template);
            }
        }
    }

    // Then the type aliases, in order, so that one may use an alias above it.
    for (name, text) in &tags {
        match name.as_str() {
            "psalm-import-type" | "phpstan-import-type" => {
                if let Some(import) = parse_type_import(text, &scoped) {
                    scoped.templates.push(import.alias.clone());
                    doc.type_imports.push(import);
                }
            }
            "psalm-type" | "phpstan-type" => {
                if let Some((alias, ty)) = parse_type_alias(text, &scoped) {
                    scoped.aliases.push((alias.clone(), ty.clone()));
                    let name_span = alias_span(raw, &alias);
                    doc.type_aliases.push(TypeAlias {
                        name: alias,
                        ty,
                        name_span,
                    });
                }
            }
            _ => {}
        }
    }

    for (name, text) in &tags {
        let tag = strip_prefix_vendor(name);
        let vendor = tag.len() != name.len();
        match tag {
            "param" => {
                if let Some(param) = parse_param_tag(text, &scoped) {
                    match doc.params.iter_mut().find(|existing| existing.name == param.name) {
                        Some(existing) if vendor || existing.ty.is_none() => *existing = param,
                        Some(_) => {}
                        None => doc.params.push(param),
                    }
                }
            }
            "return" => {
                if let Some((ty, rest)) = parse_type_prefix(text, &scoped) {
                    if doc.ret.is_none() || vendor {
                        doc.ret = Some((ty, rest_text(text, rest)));
                    }
                }
            }
            "var" => {
                if let Some((ty, used)) = parse_type_prefix(text, &scoped) {
                    let rest = text[used..].trim_start();
                    let (variable, description) = match rest.strip_prefix('$') {
                        Some(after) => {
                            let end = after.find(|c: char| c.is_whitespace()).unwrap_or(after.len());
                            (Some(after[..end].to_string()), after[end..].trim().to_string())
                        }
                        None => (None, rest.trim().to_string()),
                    };
                    if doc.var.is_none() || vendor {
                        doc.var = Some((ty, variable, description));
                    }
                }
            }
            "throws" => {
                if let Some((ty, used)) = parse_type_prefix(text, &scoped) {
                    doc.throws.push((ty, text[used..].trim().to_string()));
                }
            }
            "extends" => {
                if let Some(ty) = parse_type(text, &scoped) {
                    doc.extends.push(ty);
                }
            }
            "implements" => {
                if let Some(ty) = parse_type(text, &scoped) {
                    doc.implements.push(ty);
                }
            }
            "use" => {
                if let Some(ty) = parse_type(text, &scoped) {
                    doc.uses.push(ty);
                }
            }
            "mixin" => {
                if let Some(ty) = parse_type(text, &scoped) {
                    doc.mixins.push(ty);
                }
            }
            "property" | "property-read" | "property-write" => {
                if let Some(property) = parse_property_tag(text, tag, &scoped) {
                    doc.properties.push(property);
                }
            }
            "method" => {
                if let Some(method) = parse_method_tag(text, &scoped) {
                    doc.methods.push(method);
                }
            }
            "deprecated" => doc.deprecated = Some(text.trim().to_string()),
            "internal" => doc.internal = true,
            "since" if stub => doc.since = first_version(text),
            "removed" if stub => doc.removed = first_version(text),
            "see" | "link" => doc.see.push(text.trim().to_string()),
            "assert" | "assert-if-true" | "assert-if-false" => {
                let when = match tag {
                    "assert-if-true" => AssertWhen::IfTrue,
                    "assert-if-false" => AssertWhen::IfFalse,
                    _ => AssertWhen::Always,
                };
                if let Some(assert) = parse_assert_tag(text, when, &scoped) {
                    doc.asserts.push(assert);
                }
            }
            "template" | "template-covariant" | "template-contravariant" => {}
            "type" | "import-type" if vendor => {}
            _ => doc.tags.push(RawTag {
                name: name.clone(),
                text: text.trim().to_string(),
            }),
        }
    }
    doc
}

/// `Name = Type`, or `Name Type` as PHPStan also takes it.
fn parse_type_alias(text: &str, cx: &TypeContext) -> Option<(String, Type)> {
    let text = text.trim_start();
    let end = text
        .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .unwrap_or(text.len());
    let name = &text[..end];
    if name.is_empty() || name.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let rest = text[end..].trim_start();
    let rest = rest.strip_prefix('=').unwrap_or(rest).trim_start();
    let (ty, _) = parse_type_prefix(rest, cx)?;
    Some((name.to_string(), ty))
}

/// Where the name of an alias is written in the comment, counted from its start.
fn alias_span(raw: &str, alias: &str) -> Span {
    for (at, _) in raw.match_indices("-type") {
        let before = &raw[..at];
        if !(before.ends_with("@psalm") || before.ends_with("@phpstan")) {
            continue;
        }
        let rest = &raw[at + "-type".len()..];
        let trimmed = rest.trim_start();
        let follows = trimmed
            .strip_prefix(alias)
            .is_some_and(|after| !after.starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_'));
        if follows && trimmed.len() < rest.len() {
            let start = (raw.len() - trimmed.len()) as u32;
            return Span {
                start,
                end: start + alias.len() as u32,
            };
        }
    }
    Span::default()
}

/// `Name from Class`, with `as Alias` to use it under another name.
fn parse_type_import(text: &str, cx: &TypeContext) -> Option<TypeImport> {
    let mut words = text.split_whitespace();
    let name = words.next()?.to_string();
    if !words.next()?.eq_ignore_ascii_case("from") {
        return None;
    }
    let class = cx.resolver.resolve_class(words.next()?);
    let alias = match words.next() {
        Some(word) if word.eq_ignore_ascii_case("as") => words.next()?.to_string(),
        _ => name.clone(),
    };
    Some(TypeImport { alias, name, class })
}

fn rest_text(text: &str, used: usize) -> String {
    text[used..].trim().to_string()
}

fn is_tag(name: &str, base: &str) -> bool {
    name == base || strip_prefix_vendor(name) == base
}

/// `phpstan-return` and `psalm-return` are `return`, read in preference to it.
fn strip_prefix_vendor(name: &str) -> &str {
    name.strip_prefix("phpstan-")
        .or_else(|| name.strip_prefix("psalm-"))
        .unwrap_or(name)
}

fn first_version(text: &str) -> Option<u16> {
    let word = text.split_whitespace().next()?;
    if !word.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    PhpVersion::parse(word).map(encode_version)
}

/// The summary is the first paragraph, up to a period at the end of a line when there is one.
fn split_prose(lines: &[String]) -> (String, String) {
    let mut start = 0;
    while start < lines.len() && lines[start].trim().is_empty() {
        start += 1;
    }
    let mut end = lines.len();
    while end > start && lines[end - 1].trim().is_empty() {
        end -= 1;
    }
    let lines = &lines[start..end];
    let mut summary_end = lines.len();
    for (index, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            summary_end = index;
            break;
        }
        if line.trim_end().ends_with('.') && index + 1 < lines.len() && !lines[index + 1].trim().is_empty() {
            summary_end = index + 1;
            break;
        }
    }
    let summary = lines[..summary_end].join("\n");
    let description = lines[summary_end..].join("\n").trim().to_string();
    (summary, description)
}

fn parse_template(text: &str, cx: &TypeContext) -> Option<Template> {
    let text = text.trim();
    let end = text.find(|c: char| c.is_whitespace()).unwrap_or(text.len());
    let name = &text[..end];
    if name.is_empty() {
        return None;
    }
    let mut rest = text[end..].trim_start();
    let mut bound = None;
    let mut default = None;
    if let Some(after) = rest.strip_prefix("of ").or_else(|| rest.strip_prefix("as ")) {
        if let Some((ty, used)) = parse_type_prefix(after, cx) {
            bound = Some(ty);
            rest = after[used..].trim_start();
        }
    }
    if let Some(after) = rest.strip_prefix('=') {
        if let Some((ty, used)) = parse_type_prefix(after.trim_start(), cx) {
            default = Some(ty);
            rest = after.trim_start()[used..].trim_start();
        }
    }
    Some(Template {
        name: name.to_string(),
        bound,
        default,
        description: rest.trim().to_string(),
    })
}

fn parse_param_tag(text: &str, cx: &TypeContext) -> Option<DocParam> {
    let text = text.trim_start();
    let (ty, rest) =
        if text.starts_with('$') || text.starts_with("&$") || text.starts_with("...$") || text.starts_with("&...$") {
            (None, text)
        } else {
            let (ty, used) = parse_type_prefix(text, cx)?;
            (Some(ty), text[used..].trim_start())
        };
    let mut rest = rest;
    let mut by_ref = false;
    let mut variadic = false;
    if let Some(after) = rest.strip_prefix('&') {
        by_ref = true;
        rest = after.trim_start();
    }
    if let Some(after) = rest.strip_prefix("...") {
        variadic = true;
        rest = after;
    }
    let after = rest.strip_prefix('$')?;
    let end = after.find(|c: char| c.is_whitespace()).unwrap_or(after.len());
    Some(DocParam {
        name: after[..end].to_string(),
        ty,
        description: after[end..].trim().to_string(),
        variadic,
        by_ref,
    })
}

fn parse_assert_tag(text: &str, when: AssertWhen, cx: &TypeContext) -> Option<DocAssert> {
    let mut text = text.trim_start();
    let negated = text.starts_with('!');
    let equality = text.starts_with('=') || text.starts_with("!=");
    text = text.trim_start_matches(['!', '=']);
    let (ty, used) = parse_type_prefix(text, cx)?;
    let rest = text[used..].trim_start();
    if !rest.starts_with('$') {
        return None;
    }
    let end = rest.find(|c: char| c.is_whitespace()).unwrap_or(rest.len());
    Some(DocAssert {
        when,
        subject: rest[..end].to_string(),
        ty,
        negated,
        equality,
    })
}

fn parse_property_tag(text: &str, tag: &str, cx: &TypeContext) -> Option<DocProperty> {
    let text = text.trim_start();
    let (ty, rest) = if text.starts_with('$') {
        (None, text)
    } else {
        let (ty, used) = parse_type_prefix(text, cx)?;
        (Some(ty), text[used..].trim_start())
    };
    let after = rest.strip_prefix('$')?;
    let end = after.find(|c: char| c.is_whitespace()).unwrap_or(after.len());
    Some(DocProperty {
        name: after[..end].to_string(),
        ty,
        read_only: tag == "property-read",
        write_only: tag == "property-write",
        description: after[end..].trim().to_string(),
    })
}

fn parse_method_tag(text: &str, cx: &TypeContext) -> Option<DocMethod> {
    let mut text = text.trim_start();
    let mut is_static = false;
    if let Some(rest) = text.strip_prefix("static ") {
        is_static = true;
        text = rest.trim_start();
    }
    let open = text.find('(')?;
    let head = text[..open].trim_end();
    let (ret, name) = match head.rsplit_once(char::is_whitespace) {
        Some((ret, name)) => (parse_type(ret.trim(), cx), name),
        None => (None, head),
    };
    if name.is_empty() || !name.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return None;
    }
    let mut depth = 0;
    let mut close = None;
    for (index, c) in text[open..].char_indices() {
        match c {
            '(' | '<' | '{' | '[' => depth += 1,
            ')' | '>' | '}' | ']' => {
                depth -= 1;
                if depth == 0 {
                    close = Some(open + index);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close?;
    let params = split_top_level(&text[open + 1..close])
        .into_iter()
        .filter_map(|piece| parse_param_tag(&piece, cx))
        .collect();
    Some(DocMethod {
        name: name.to_string(),
        is_static,
        ret,
        params,
        description: text[close + 1..].trim().to_string(),
    })
}

fn split_top_level(text: &str) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (index, c) in text.char_indices() {
        match c {
            '(' | '<' | '{' | '[' => depth += 1,
            ')' | '>' | '}' | ']' => depth -= 1,
            ',' if depth == 0 => {
                pieces.push(text[start..index].to_string());
                start = index + 1;
            }
            _ => {}
        }
    }
    if !text[start..].trim().is_empty() {
        pieces.push(text[start..].to_string());
    }
    pieces
}

/// The type in an inline `/** @var Type $name */` comment.
pub fn parse_var_comment(raw: &str, cx: &TypeContext) -> Option<(Type, Option<String>)> {
    let doc = parse_doc(raw, cx, false);
    doc.var.map(|(ty, name, _)| (ty, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn resolver() -> NameResolver {
        let mut resolver = NameResolver::new("App");
        resolver.add_use(crate::resolve::UseKind::Class, "Illuminate\\Support\\Collection", None);
        resolver
    }

    fn ty(text: &str) -> String {
        let resolver = resolver();
        let mut cx = TypeContext::new(&resolver);
        cx.templates = vec!["T".to_string(), "K".to_string()];
        cx.class_name = Some("App\\Box");
        parse_type(text, &cx).map_or_else(|| "<none>".to_string(), |ty| ty.display(false))
    }

    #[test]
    fn reads_scalar_and_union_types() {
        assert_eq!(ty("int|string|null"), "int|string|null");
        assert_eq!(ty("?Foo"), "?App\\Foo");
        assert_eq!(ty("\\Foo\\Bar"), "Foo\\Bar");
        assert_eq!(ty("positive-int"), "int");
        assert_eq!(ty("non-empty-string"), "string");
        assert_eq!(ty("self"), "App\\Box");
        assert_eq!(ty("static|null"), "?static");
        assert_eq!(ty("$this"), "static");
    }

    #[test]
    fn reads_generics_and_arrays() {
        assert_eq!(ty("array<string, int>"), "array<string, int>");
        assert_eq!(ty("array<int>"), "array<int>");
        assert_eq!(ty("list<Foo>"), "list<App\\Foo>");
        assert_eq!(ty("Foo[]"), "list<App\\Foo>");
        assert_eq!(
            ty("Collection<int, User>"),
            "Illuminate\\Support\\Collection<int, App\\User>"
        );
        assert_eq!(ty("iterable<K, T>"), "iterable<K, T>");
        assert_eq!(ty("class-string<T>"), "class-string<T>");
        assert_eq!(ty("int<0, max>"), "int");
    }

    #[test]
    fn reads_shapes_and_callables() {
        assert_eq!(ty("array{a: int, b?: string}"), "array{a: int, b?: string}");
        assert_eq!(ty("array{'x-y': int}"), "array{x-y: int}");
        assert_eq!(ty("list{int, string}"), "array{0: int, 1: string}");
        assert_eq!(ty("callable(int, string=): bool"), "callable(int, string=): bool");
        assert_eq!(ty("Closure(T): T"), "Closure(T): T");
        assert_eq!(ty("callable"), "callable");
        assert_eq!(ty("\\Closure(int): string"), "Closure(int): string");
    }

    #[test]
    fn reads_conditional_types() {
        assert_eq!(ty("($x is int ? string : bool)"), "($x is int ? string : bool)");
        assert_eq!(ty("(T is null ? int : T)"), "(T is null ? int : T)");
    }

    #[test]
    fn keeps_the_rest_of_the_line() {
        let resolver = resolver();
        let cx = TypeContext::new(&resolver);
        let (ty, used) = parse_type_prefix("array<int, string> The list of things", &cx).unwrap();
        assert_eq!(ty.display(false), "array<int, string>");
        assert_eq!(&"array<int, string> The list of things"[used..], " The list of things");
    }

    fn doc(text: &str) -> Doc {
        let resolver = resolver();
        let cx = TypeContext::new(&resolver);
        parse_doc(text, &cx, true)
    }

    #[test]
    fn splits_summary_description_and_tags() {
        let doc = doc(
            "/**\n * Finds a user.\n *\n * Longer text\n * on two lines.\n *\n * @param int $id The id\n *   continued\n * @return User|null\n * @throws \\RuntimeException when gone\n * @deprecated use find()\n * @since 8.1\n * @see Foo::bar()\n */",
        );
        assert_eq!(doc.summary, "Finds a user.");
        assert_eq!(doc.description, "Longer text\non two lines.");
        assert_eq!(doc.params.len(), 1);
        assert_eq!(doc.params[0].name, "id");
        assert_eq!(doc.params[0].description, "The id\n  continued");
        assert_eq!(doc.ret.as_ref().unwrap().0.display(true), "?User");
        assert_eq!(doc.throws[0].0.display(false), "RuntimeException");
        assert_eq!(doc.deprecated.as_deref(), Some("use find()"));
        assert_eq!(doc.since, Some(801));
        assert_eq!(doc.see, vec!["Foo::bar()"]);
    }

    #[test]
    fn reads_assert_tags_in_every_form() {
        let doc = doc(
            "/**\n * @template E of object\n * @param class-string<E> $expected\n * @phpstan-assert =E $actual\n * @psalm-assert !null $value\n * @phpstan-assert-if-true list<mixed> $this->items\n * @psalm-assert-if-false int|string $key\n */",
        );
        let shown: Vec<String> = doc
            .asserts
            .iter()
            .map(|assert| {
                format!(
                    "{:?} {}{}{} {}",
                    assert.when,
                    if assert.negated { "!" } else { "" },
                    if assert.equality { "=" } else { "" },
                    assert.ty.display(false),
                    assert.subject
                )
            })
            .collect();
        assert_eq!(
            shown,
            vec![
                "Always =E $actual",
                "Always !null $value",
                "IfTrue list<mixed> $this->items",
                "IfFalse int|string $key",
            ]
        );
    }

    #[test]
    fn a_summary_ends_at_a_period() {
        let doc = doc("/** Returns a thing.\n * It also does more.\n * @return int */");
        assert_eq!(doc.summary, "Returns a thing.");
        assert_eq!(doc.description, "It also does more.");
    }

    #[test]
    fn reads_templates_and_inheritance_tags() {
        let doc = doc(
            "/**\n * @template T of \\Countable\n * @template-covariant V = int\n * @extends Base<T>\n * @implements \\IteratorAggregate<int, V>\n * @mixin Helper\n * @property-read string $name The name\n * @method static self make(int $id, string ...$rest) Makes one\n */",
        );
        assert_eq!(doc.templates.len(), 2);
        assert_eq!(doc.templates[0].bound.as_ref().unwrap().display(false), "Countable");
        assert_eq!(doc.templates[1].default.as_ref().unwrap().display(false), "int");
        assert_eq!(doc.extends[0].display(false), "App\\Base<T>");
        assert_eq!(doc.implements[0].display(false), "IteratorAggregate<int, V>");
        assert_eq!(doc.mixins[0].display(false), "App\\Helper");
        assert!(doc.properties[0].read_only);
        assert_eq!(doc.methods[0].name, "make");
        assert!(doc.methods[0].is_static);
        assert_eq!(doc.methods[0].params.len(), 2);
        assert!(doc.methods[0].params[1].variadic);
    }

    #[test]
    fn vendor_tags_win_over_plain_ones() {
        let doc = doc(
            "/**\n * @return array\n * @phpstan-return list<int>\n * @param array $a\n * @psalm-param non-empty-list<string> $a\n */",
        );
        assert_eq!(doc.ret.unwrap().0.display(false), "list<int>");
        assert_eq!(doc.params[0].ty.as_ref().unwrap().display(false), "list<string>");
    }

    #[test]
    fn since_means_nothing_outside_the_stubs() {
        let resolver = resolver();
        let cx = TypeContext::new(&resolver);
        let doc = parse_doc("/** @since 12.0 */", &cx, false);
        assert_eq!(doc.since, None);
    }

    #[test]
    fn reads_an_inline_var_comment() {
        let resolver = resolver();
        let cx = TypeContext::new(&resolver);
        let (ty, name) = parse_var_comment("/** @var Collection<int, User> $items */", &cx).unwrap();
        assert_eq!(name.as_deref(), Some("items"));
        assert_eq!(ty.display(true), "Collection<int, User>");
    }
}
