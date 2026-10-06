//! Twig templates as a language of their own. A template is read into its tags and the expressions
//! in them (`lex`, `parse`); the names they give are templates (`extends`, `include`, `embed`,
//! `use`, `import`, `from`), blocks, and the strings the PHP behind a function or a filter takes
//! (`path('blog_index')`, `'post.title'|trans`), followed through the overlay as in PHP. Functions,
//! filters and tests are the ones the Twig extensions of the project and its packages declare.

pub mod data;
pub mod lex;
pub mod parse;
pub mod types;

use std::path::Path;

use php_index::framework::keys::{KeyKind, definitions};
use php_index::framework::overlay::{Marker, markers_for};
use php_index::framework::symfony::templates::Templates;
use php_index::framework::twig::{TwigCallable, TwigExtensions, TwigKind};
use php_index::{Index, Type};
use php_syntax::TextRange;

use crate::ast::range_of;
use crate::completion::{CompletionItem, CompletionList, CompletionOptions, ItemKind, TextEdit, match_score};
use crate::frameworks::complete::key_items;
use crate::infer::Analyzer;
use crate::nav::{HoverResult, Place, hover_markdown};
use crate::refs::{Access, Hit, HitKind, Query, Symbol};
use crate::target::Target;
use lex::{Kind, Token, lex};
use parse::{Arg, Expr, HashKey, Parser};

/// Whether a file is a Twig template, by its name.
pub fn is_template(path: &Path) -> bool {
    path.extension().is_some_and(|extension| extension == "twig")
}

/// A name a tag or a string gives: a template, a block, a route, a translation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NameRef {
    pub kind: KeyKind,
    pub value: String,
    pub start: u32,
    pub end: u32,
    pub scope: Option<String>,
}

/// A function, filter or test named in the template.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallRef {
    pub kind: TwigKind,
    pub name: String,
    pub start: u32,
    pub end: u32,
}

/// A tag, read by what its name says it holds.
#[derive(Clone, Debug, PartialEq)]
pub enum TagBody {
    /// `extends`, `include`, `embed`, `use`, `import`, `from` and the like: the template they name,
    /// and what they pass.
    Templates {
        template: Expr,
        with: Option<Expr>,
        /// `only`: the template gets nothing of the context.
        only: bool,
    },
    Block {
        name: (String, u32, u32),
        short: Option<Expr>,
    },
    For {
        targets: Vec<(String, u32, u32)>,
        iterable: Expr,
        condition: Option<Expr>,
    },
    Set {
        targets: Vec<(String, u32, u32)>,
        values: Vec<Expr>,
    },
    Macro {
        name: (String, u32, u32),
        params: Vec<((String, u32, u32), Option<Expr>)>,
    },
    /// `import 'forms.html.twig' as forms` and `from ... import a as b`: the names the macros get.
    Import {
        template: Expr,
        names: Vec<(String, u32, u32)>,
    },
    /// Anything else: the expressions in it, in order.
    Other(Vec<Expr>),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Tag {
    pub name: String,
    pub name_start: u32,
    pub name_end: u32,
    pub start: u32,
    pub end: u32,
    pub body: TagBody,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Item {
    Output { expr: Expr, start: u32, end: u32 },
    Tag(Tag),
}

/// A template, read.
pub struct Template {
    pub items: Vec<Item>,
    pub names: Vec<NameRef>,
    pub calls: Vec<CallRef>,
    /// The names written alone, a variable or a function being typed, and the places an expression
    /// is missing.
    pub bare: Vec<(u32, u32)>,
    /// The name of the template itself, when it is one of the project's.
    pub name: Option<String>,
}

/// The tags whose only argument is a template, perhaps with `with` and `only` after it.
const TEMPLATE_TAGS: &[&str] = &["extends", "include", "embed", "use", "form_theme", "sandbox"];

impl Template {
    pub fn read(index: &Index, path: Option<&Path>, text: &str) -> Template {
        let tokens = lex(text);
        let items = items(text, &tokens);
        let name = path.and_then(|path| {
            index
                .section::<Templates>()
                .templates
                .iter()
                .find(|template| template.path == path)
                .map(|template| template.name.clone())
        });
        let extensions = index.section::<TwigExtensions>();
        let mut template = Template {
            items,
            names: Vec::new(),
            calls: Vec::new(),
            bare: Vec::new(),
            name,
        };
        template.collect(index, &extensions);
        template
    }

    fn collect(&mut self, index: &Index, extensions: &TwigExtensions) {
        let mut names = Vec::new();
        let mut calls = Vec::new();
        let mut tag_names = Vec::new();
        let mut bare = Vec::new();
        let scope = self.name.clone();
        let block_scope = scope.clone();
        let mut visit = |expr: &Expr| match expr {
            Expr::Call { name, start, end, args } => {
                calls.push(CallRef {
                    kind: TwigKind::Function,
                    name: name.clone(),
                    start: *start,
                    end: *end,
                });
                if name == "block" {
                    if let Some(Expr::Str { value, start, end }) = args.first().map(|arg| &arg.value) {
                        names.push(NameRef {
                            kind: KeyKind::Block,
                            value: value.clone(),
                            start: *start,
                            end: *end,
                            scope: scope.clone(),
                        });
                    }
                }
                if let Some(callable) = extensions.find(TwigKind::Function, name) {
                    let args: Vec<&Arg> = args.iter().collect();
                    names.extend(callable_names(index, callable, None, &args));
                }
            }
            Expr::Filter {
                subject,
                name,
                start,
                end,
                args,
            } => {
                calls.push(CallRef {
                    kind: TwigKind::Filter,
                    name: name.clone(),
                    start: *start,
                    end: *end,
                });
                if let Some(callable) = extensions.find(TwigKind::Filter, name) {
                    let args: Vec<&Arg> = args.iter().collect();
                    names.extend(callable_names(index, callable, Some(subject), &args));
                }
            }
            Expr::Test { name, start, end, .. } => calls.push(CallRef {
                kind: TwigKind::Test,
                name: name.clone(),
                start: *start,
                end: *end,
            }),
            Expr::Name { start, end, .. } => bare.push((*start, *end)),
            Expr::Missing(at) => bare.push((*at, *at)),
            _ => {}
        };
        for item in &self.items {
            match item {
                Item::Output { expr, .. } => expr.walk(&mut visit),
                Item::Tag(tag) => {
                    if tag.name == "apply" {
                        if let TagBody::Other(exprs) = &tag.body {
                            for expr in exprs {
                                expr.walk(&mut visit);
                            }
                        }
                        continue;
                    }
                    for expr in tag_exprs(&tag.body) {
                        expr.walk(&mut visit);
                    }
                    match &tag.body {
                        TagBody::Templates { template, .. } | TagBody::Import { template, .. } => {
                            template_names(template, &mut tag_names)
                        }
                        TagBody::Block { name, .. } => tag_names.push(NameRef {
                            kind: KeyKind::Block,
                            value: name.0.clone(),
                            start: name.1,
                            end: name.2,
                            scope: block_scope.clone(),
                        }),
                        _ => {}
                    }
                }
            }
        }
        names.extend(tag_names);
        names.sort_by_key(|name| name.start);
        self.names = names;
        self.calls = calls;
        self.bare = bare;
    }

    fn name_at(&self, offset: u32) -> Option<&NameRef> {
        self.names
            .iter()
            .find(|name| name.start <= offset && offset <= name.end)
    }

    fn call_at(&self, offset: u32) -> Option<&CallRef> {
        self.calls
            .iter()
            .find(|call| call.start <= offset && offset <= call.end)
    }
}

/// The expressions a tag holds.
pub fn tag_exprs(body: &TagBody) -> Vec<&Expr> {
    match body {
        TagBody::Templates { template, with, .. } => std::iter::once(template).chain(with).collect(),
        TagBody::Block { short, .. } => short.iter().collect(),
        TagBody::For {
            iterable, condition, ..
        } => std::iter::once(iterable).chain(condition).collect(),
        TagBody::Set { values, .. } => values.iter().collect(),
        TagBody::Macro { params, .. } => params.iter().filter_map(|(_, default)| default.as_ref()).collect(),
        TagBody::Import { template, .. } => vec![template],
        TagBody::Other(exprs) => exprs.iter().collect(),
    }
}

/// The names an expression gives: a string, the strings of a list, the two sides of a condition or
/// of `??`.
fn template_names(expr: &Expr, names: &mut Vec<NameRef>) {
    match expr {
        Expr::Str { value, start, end } => names.push(NameRef {
            kind: KeyKind::Template,
            value: value.clone(),
            start: *start,
            end: *end,
            scope: None,
        }),
        Expr::Array(items) => items.iter().for_each(|item| template_names(item, names)),
        Expr::Conditional { then, otherwise, .. } => {
            then.iter()
                .chain(otherwise)
                .for_each(|side| template_names(side, names));
        }
        Expr::Binary { op, left, right } if op == "??" => {
            template_names(left, names);
            template_names(right, names);
        }
        _ => {}
    }
}

/// The strings the PHP behind a function or a filter reads as a name, by what the overlay says of
/// its parameters. A filter's subject is the first of them.
fn callable_names(index: &Index, callable: &TwigCallable, subject: Option<&Expr>, args: &[&Arg]) -> Vec<NameRef> {
    let (Some(class), Some(method)) = (&callable.class, &callable.target) else {
        return Vec::new();
    };
    let params: Vec<String> = index
        .class(class)
        .and_then(|found| {
            found
                .decl
                .method(method)
                .map(|method| method.callable.params.iter().map(|param| param.name.clone()).collect())
        })
        .unwrap_or_default();
    let mut values: Vec<(usize, &Expr)> = Vec::new();
    let first = callable.skipped + usize::from(subject.is_some());
    if let Some(subject) = subject {
        values.push((callable.skipped, subject));
    }
    for (position, arg) in args.iter().enumerate() {
        let at = match &arg.name {
            Some((name, ..)) => params.iter().position(|param| param == name),
            None => Some(first + position),
        };
        if let Some(at) = at {
            values.push((at, &arg.value));
        }
    }
    let mut out = Vec::new();
    for marker in markers_for(index, Some(class), Some(class), method) {
        let Marker::Key { kind, position, name } = marker else {
            continue;
        };
        let Some(kind) = KeyKind::parse(&kind) else {
            continue;
        };
        let wanted = match &name {
            Some(name) => params.iter().position(|param| param == name),
            None => Some(position),
        };
        let Some(wanted) = wanted else {
            continue;
        };
        for (at, value) in &values {
            if *at != wanted {
                continue;
            }
            let mut strings = Vec::new();
            template_names(value, &mut strings);
            out.extend(strings.into_iter().map(|string| NameRef {
                kind,
                scope: None,
                ..string
            }));
        }
    }
    out
}

/// The outputs and tags of a template, with the expressions of each read.
fn items(source: &str, tokens: &[Token]) -> Vec<Item> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < tokens.len() {
        let token = tokens[at];
        match token.kind {
            Kind::VarStart | Kind::BlockStart => {
                let end_kind = if token.kind == Kind::VarStart {
                    Kind::VarEnd
                } else {
                    Kind::BlockEnd
                };
                let close = tokens[at + 1..]
                    .iter()
                    .position(|candidate| {
                        candidate.kind == end_kind
                            || matches!(candidate.kind, Kind::VarStart | Kind::BlockStart | Kind::Text)
                    })
                    .map_or(tokens.len(), |found| at + 1 + found);
                let inner = &tokens[at + 1..close];
                let closed = tokens.get(close).is_some_and(|candidate| candidate.kind == end_kind);
                let end = if closed {
                    tokens[close].end
                } else {
                    inner.last().map_or(token.end, |last| last.end)
                };
                if token.kind == Kind::VarStart {
                    let expr = Parser::new(source, inner, end).expression();
                    out.push(Item::Output {
                        expr,
                        start: token.start,
                        end,
                    });
                } else if let Some(tag) = tag(source, inner, token.start, end) {
                    out.push(Item::Tag(tag));
                }
                at = if closed { close + 1 } else { close };
            }
            _ => at += 1,
        }
    }
    out
}

fn tag(source: &str, tokens: &[Token], start: u32, end: u32) -> Option<Tag> {
    let first = tokens.first().filter(|token| token.kind == Kind::Name)?;
    let name = first.text(source).to_string();
    let mut parser = Parser::new(source, &tokens[1..], end);
    let body = match name.as_str() {
        name if TEMPLATE_TAGS.contains(&name) => {
            let template = parser.expression();
            let (mut with, mut only) = (None, false);
            loop {
                if parser.eat("ignore") {
                    parser.eat("missing");
                } else if parser.eat("with") {
                    with = Some(parser.expression());
                } else if parser.eat("only") {
                    only = true;
                } else {
                    break;
                }
            }
            TagBody::Templates { template, with, only }
        }
        "block" => {
            let block = parser.name().unwrap_or((String::new(), first.end, first.end));
            let short = (!parser.at_end()).then(|| parser.expression());
            TagBody::Block { name: block, short }
        }
        "for" => {
            let mut targets = Vec::new();
            while let Some(target) = parser.name() {
                if target.0 == "in" {
                    break;
                }
                targets.push(target);
                if !parser.eat(",") {
                    parser.eat("in");
                    break;
                }
            }
            let iterable = parser.expression();
            let condition = parser.eat("if").then(|| parser.expression());
            TagBody::For {
                targets,
                iterable,
                condition,
            }
        }
        "set" => {
            let mut targets = Vec::new();
            while let Some(target) = parser.name() {
                targets.push(target);
                if !parser.eat(",") {
                    break;
                }
            }
            let mut values = Vec::new();
            if parser.eat("=") {
                loop {
                    values.push(parser.expression());
                    if !parser.eat(",") {
                        break;
                    }
                }
            }
            TagBody::Set { targets, values }
        }
        "macro" => {
            let macro_name = parser.name().unwrap_or((String::new(), first.end, first.end));
            let mut params = Vec::new();
            if parser.eat("(") {
                while let Some(param) = parser.name() {
                    let default = parser.eat("=").then(|| parser.expression());
                    params.push((param, default));
                    if !parser.eat(",") {
                        break;
                    }
                }
                parser.eat(")");
            }
            TagBody::Macro {
                name: macro_name,
                params,
            }
        }
        "import" => {
            let template = parser.expression();
            parser.eat("as");
            let names = parser.name().into_iter().collect();
            TagBody::Import { template, names }
        }
        "from" => {
            let template = parser.expression();
            parser.eat("import");
            let mut names = Vec::new();
            while let Some(imported) = parser.name() {
                let alias = if parser.eat("as") { parser.name() } else { None };
                names.push(alias.unwrap_or(imported));
                if !parser.eat(",") {
                    break;
                }
            }
            TagBody::Import { template, names }
        }
        "apply" => TagBody::Other(vec![parser.filter_chain(Expr::Missing(first.end))]),
        _ => {
            let mut exprs = Vec::new();
            while !parser.at_end() {
                let before = parser.peek().map(|token| token.start);
                let expr = parser.expression();
                if !matches!(expr, Expr::Missing(_)) {
                    exprs.push(expr);
                }
                if parser.peek().map(|token| token.start) == before {
                    parser.advance();
                }
            }
            TagBody::Other(exprs)
        }
    };
    Some(Tag {
        name,
        name_start: first.start,
        name_end: first.end,
        start,
        end,
        body,
    })
}

/// The function, filter or test a call names, as the PHP behind it, else as the place the
/// extension declares it.
fn describe_call(index: &Index, call: &CallRef) -> Vec<crate::nav::Description> {
    let extensions = index.section::<TwigExtensions>();
    let Some(callable) = extensions.find(call.kind, &call.name) else {
        return Vec::new();
    };
    let root = php_syntax::parse("<?php ").syntax();
    let analyzer = Analyzer::new(index, &root, 0);
    let target = match (&callable.class, &callable.target) {
        (Some(class), Some(method)) => Some(Target::Method {
            receiver: Type::class(class.clone()),
            name: method.clone(),
        }),
        (None, Some(function)) => Some(Target::Function(function.clone())),
        _ => None,
    };
    let described: Vec<crate::nav::Description> = target
        .map(|target| analyzer.describe(&target))
        .unwrap_or_default()
        .into_iter()
        .map(|mut description| {
            description.title = format!("{} '{}' ({})", kind_label(call.kind), call.name, description.title);
            description
        })
        .collect();
    if !described.is_empty() {
        return described;
    }
    vec![crate::nav::Description {
        title: format!("{} '{}'", kind_label(call.kind), call.name),
        signature: call.name.clone(),
        doc: None,
        place: Some(Place {
            path: Some(callable.path.clone()),
            span: callable.span,
        }),
    }]
}

fn kind_label(kind: TwigKind) -> &'static str {
    match kind {
        TwigKind::Function => "Twig function",
        TwigKind::Filter => "Twig filter",
        TwigKind::Test => "Twig test",
    }
}

/// Where the name under an offset of a template is declared.
pub fn definitions_at(
    index: &Index,
    path: Option<&Path>,
    text: &str,
    given: &[(String, Type)],
    offset: u32,
) -> Vec<Place> {
    let template = Template::read(index, path, text);
    if let Some(name) = template.name_at(offset) {
        return definitions(index, name.kind, &name.value, name.scope.as_deref())
            .into_iter()
            .map(|definition| Place {
                path: Some(definition.path),
                span: definition.span,
            })
            .collect();
    }
    if let Some(call) = template.call_at(offset) {
        return describe_call(index, call)
            .into_iter()
            .filter_map(|description| description.place)
            .collect();
    }
    match spot_at(index, &template, given, offset) {
        Some(Spot::Attribute {
            object,
            member: Some(member),
            ..
        }) => describe_member(index, &object, &member)
            .into_iter()
            .filter_map(|description| description.place)
            .collect(),
        _ => Vec::new(),
    }
}

/// What the name under an offset of a template is.
pub fn hover_at(
    index: &Index,
    path: Option<&Path>,
    text: &str,
    given: &[(String, Type)],
    offset: u32,
) -> Option<HoverResult> {
    let template = Template::read(index, path, text);
    if let Some(name) = template.name_at(offset) {
        let sections: Vec<String> =
            crate::frameworks::keys::describe(index, name.kind, &name.value, name.scope.as_deref())
                .iter()
                .map(hover_markdown)
                .collect();
        return Some(HoverResult {
            markdown: sections.join("\n\n---\n\n"),
            range: range_of(name.start, name.end),
        });
    }
    if let Some(call) = template.call_at(offset) {
        let sections: Vec<String> = describe_call(index, call).iter().map(hover_markdown).collect();
        return (!sections.is_empty()).then(|| HoverResult {
            markdown: sections.join("\n\n---\n\n"),
            range: range_of(call.start, call.end),
        });
    }
    match spot_at(index, &template, given, offset)? {
        Spot::Variable { name, start, end, ty } => (!matches!(ty, Type::Unknown)).then(|| HoverResult {
            markdown: format!("```php\n{} ${name}\n```", ty.display(true)),
            range: range_of(start, end),
        }),
        Spot::Attribute {
            object,
            member,
            name,
            start,
            end,
            ty,
        } => {
            let sections: Vec<String> = member
                .map(|member| describe_member(index, &object, &member))
                .unwrap_or_default()
                .iter()
                .map(hover_markdown)
                .collect();
            if !sections.is_empty() {
                return Some(HoverResult {
                    markdown: sections.join("\n\n---\n\n"),
                    range: range_of(start, end),
                });
            }
            (!matches!(ty, Type::Unknown)).then(|| HoverResult {
                markdown: format!("```php\n{} {name}\n```", ty.display(true)),
                range: range_of(start, end),
            })
        }
    }
}

/// What the cursor is on, with the types the variables in force give it.
enum Spot {
    Variable {
        name: String,
        start: u32,
        end: u32,
        ty: Type,
    },
    Attribute {
        object: Type,
        member: Option<types::Member>,
        name: String,
        start: u32,
        end: u32,
        ty: Type,
    },
}

fn spot_at(index: &Index, template: &Template, given: &[(String, Type)], offset: u32) -> Option<Spot> {
    let typer = types::Typer::new(index);
    let mut spot = None;
    types::walk(&typer, template, given, Some(offset), &mut |expr, env| {
        expr.walk(&mut |inner| match inner {
            Expr::Name { name, start, end } if *start <= offset && offset <= *end => {
                spot = Some(Spot::Variable {
                    name: name.clone(),
                    start: *start,
                    end: *end,
                    ty: env.get(name).cloned().unwrap_or(Type::Unknown),
                });
            }
            Expr::Attribute {
                object,
                name,
                start,
                end,
                args,
            } if *start <= offset && offset <= *end => {
                let object = typer.type_of(object, env);
                let (ty, member) = typer.attribute(&object, name, args.is_some());
                spot = Some(Spot::Attribute {
                    object,
                    member,
                    name: name.clone(),
                    start: *start,
                    end: *end,
                    ty,
                });
            }
            _ => {}
        });
    });
    spot
}

/// The variables in force at an offset.
fn env_at(index: &Index, template: &Template, given: &[(String, Type)], offset: u32) -> crate::infer::Env {
    let typer = types::Typer::new(index);
    let mut found = None;
    types::walk(&typer, template, given, Some(offset), &mut |_, env| {
        found = Some(env.clone())
    });
    found.unwrap_or_else(|| {
        let mut env = crate::infer::Env::default();
        for (name, ty) in types::globals(index).into_iter().chain(given.iter().cloned()) {
            env.set(name, ty);
        }
        env
    })
}

fn describe_member(index: &Index, object: &Type, member: &types::Member) -> Vec<crate::nav::Description> {
    let root = php_syntax::parse("<?php ").syntax();
    let analyzer = Analyzer::new(index, &root, 0);
    let target = match member {
        types::Member::Property { class, name } => Target::Property {
            receiver: object
                .members()
                .iter()
                .find(|ty| matches!(ty, Type::Class { .. }))
                .cloned()
                .unwrap_or_else(|| Type::class(class.clone())),
            name: name.clone(),
        },
        types::Member::Method { class, name } => Target::Method {
            receiver: object
                .members()
                .iter()
                .find(|ty| matches!(ty, Type::Class { .. }))
                .cloned()
                .unwrap_or_else(|| Type::class(class.clone())),
            name: name.clone(),
        },
    };
    analyzer.describe(&target)
}

/// What can be typed at an offset of a template.
pub fn complete_at(
    index: &Index,
    path: Option<&Path>,
    text: &str,
    given: &[(String, Type)],
    offset: u32,
    options: CompletionOptions,
) -> Option<CompletionList> {
    let template = Template::read(index, path, text);
    if let Some(name) = template.name_at(offset) {
        let typed = text.get(name.start as usize..offset as usize)?;
        return Some(key_items(
            index,
            name.kind,
            name.scope.as_deref(),
            typed,
            (name.start, name.end),
            options,
        ));
    }
    if let Some(call) = template.call_at(offset) {
        let typed = text.get(call.start as usize..offset as usize)?;
        return Some(callable_items(index, call.kind, typed, (call.start, call.end), options));
    }
    if let Some(Spot::Attribute { object, start, end, .. }) = spot_at(index, &template, given, offset) {
        let typed = text.get(start as usize..offset as usize)?;
        let typer = types::Typer::new(index);
        let items = typer
            .attributes_of(&object)
            .into_iter()
            .filter_map(|(name, detail)| {
                let score = match_score(&name, typed)?;
                Some((
                    score,
                    CompletionItem {
                        label: name.clone(),
                        kind: ItemKind::Property,
                        detail,
                        description: None,
                        edit: TextEdit {
                            start,
                            end,
                            new_text: name.clone(),
                        },
                        additional_edits: Vec::new(),
                        sort_text: name.clone(),
                        filter_text: Some(name),
                        deprecated: false,
                        data: None,
                    },
                ))
            })
            .collect();
        return Some(finish(items, options));
    }
    let (start, end) = template
        .bare
        .iter()
        .copied()
        .find(|(start, end)| *start <= offset && offset <= *end)?;
    let typed = text.get(start as usize..offset as usize)?;
    let mut list = callable_items(index, TwigKind::Function, typed, (start, end), options);
    let env = env_at(index, &template, given, offset);
    let mut variables: Vec<(u8, CompletionItem)> = env
        .vars
        .iter()
        .filter_map(|(name, ty)| {
            let score = match_score(name, typed)?;
            Some((
                score,
                CompletionItem {
                    label: name.clone(),
                    kind: ItemKind::Variable,
                    detail: (!matches!(ty, Type::Unknown)).then(|| ty.display(true)),
                    description: None,
                    edit: TextEdit {
                        start,
                        end,
                        new_text: name.clone(),
                    },
                    additional_edits: Vec::new(),
                    sort_text: format!("0{name}"),
                    filter_text: Some(name.clone()),
                    deprecated: false,
                    data: None,
                },
            ))
        })
        .collect();
    variables.extend(list.items.drain(..).map(|item| (1, item)));
    list = finish(variables, options);
    Some(list)
}

fn finish(mut items: Vec<(u8, CompletionItem)>, options: CompletionOptions) -> CompletionList {
    items.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            .then_with(|| left.1.sort_text.cmp(&right.1.sort_text))
    });
    let incomplete = items.len() > options.limit;
    CompletionList {
        items: items.into_iter().take(options.limit).map(|(_, item)| item).collect(),
        incomplete,
    }
}

/// The functions, filters or tests that fit what was typed.
fn callable_items(
    index: &Index,
    kind: TwigKind,
    typed: &str,
    (start, end): (u32, u32),
    options: CompletionOptions,
) -> CompletionList {
    let extensions = index.section::<TwigExtensions>();
    let mut seen = std::collections::HashSet::new();
    let mut items: Vec<(u8, CompletionItem)> = extensions
        .of_kind(kind)
        .filter(|entry| seen.insert(entry.name.clone()))
        .filter_map(|entry| {
            let score = match_score(&entry.name, typed)?;
            let detail = match (&entry.class, &entry.target) {
                (Some(class), Some(method)) => Some(format!("{}::{method}", crate::short(class))),
                (None, Some(function)) => Some(function.clone()),
                _ => None,
            };
            Some((
                score,
                CompletionItem {
                    label: entry.name.clone(),
                    kind: if kind == TwigKind::Function {
                        ItemKind::Function
                    } else {
                        ItemKind::Method
                    },
                    detail,
                    description: Some(kind_label(kind).to_string()),
                    edit: TextEdit {
                        start,
                        end,
                        new_text: entry.name.clone(),
                    },
                    additional_edits: Vec::new(),
                    sort_text: entry.name.clone(),
                    filter_text: Some(entry.name.clone()),
                    deprecated: false,
                    data: None,
                },
            ))
        })
        .collect();
    items.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.label.cmp(&right.1.label)));
    let incomplete = items.len() > options.limit;
    CompletionList {
        items: items.into_iter().take(options.limit).map(|(_, item)| item).collect(),
        incomplete,
    }
}

/// What is under an offset of a template, for usages.
pub fn symbols_at(index: &Index, path: Option<&Path>, text: &str, offset: u32) -> Option<(TextRange, Vec<Symbol>)> {
    let template = Template::read(index, path, text);
    let name = template.name_at(offset)?;
    Some((
        range_of(name.start, name.end),
        vec![Symbol::Key {
            kind: name.kind,
            name: name.value.clone(),
            scope: name.scope.clone(),
        }],
    ))
}

/// The places of a template that name what a query asks for.
pub fn hits(index: &Index, path: Option<&Path>, text: &str, given: &[(String, Type)], query: &Query) -> Vec<Hit> {
    if matches!(query.symbol, Symbol::Method { .. } | Symbol::Property { .. }) {
        return member_hits(index, path, text, given, query);
    }
    if !matches!(query.symbol, Symbol::Key { .. }) {
        return Vec::new();
    }
    let template = Template::read(index, path, text);
    template
        .names
        .iter()
        .filter_map(|name| {
            let symbol = Symbol::Key {
                kind: name.kind,
                name: name.value.clone(),
                scope: name.scope.clone(),
            };
            query.matches(&symbol).then(|| Hit {
                range: range_of(name.start, name.end),
                kind: if name.kind == KeyKind::Block && is_block_tag(&template, name.start) {
                    HitKind::Declaration
                } else {
                    HitKind::Reference
                },
                access: Access::Read,
                dollar: false,
                via_alias: false,
                symbol,
            })
        })
        .collect()
}

/// The attributes of a template that read a property or call a method, through the types of the
/// variables the template is given.
fn member_hits(index: &Index, path: Option<&Path>, text: &str, given: &[(String, Type)], query: &Query) -> Vec<Hit> {
    let template = Template::read(index, path, text);
    let typer = types::Typer::new(index);
    let mut out = Vec::new();
    types::walk(&typer, &template, given, None, &mut |expr, env| {
        expr.walk(&mut |inner| {
            let Expr::Attribute {
                object,
                name,
                start,
                end,
                args,
            } = inner
            else {
                return;
            };
            let object = typer.type_of(object, env);
            let (_, Some(member)) = typer.attribute(&object, name, args.is_some()) else {
                return;
            };
            let symbol = match member {
                types::Member::Property { class, name } => Symbol::Property { class, name },
                types::Member::Method { class, name } => Symbol::Method { class, name },
            };
            if query.matches(&symbol) {
                out.push(Hit {
                    range: range_of(*start, *end),
                    kind: HitKind::Reference,
                    access: Access::Read,
                    dollar: false,
                    via_alias: false,
                    symbol,
                });
            }
        });
    });
    out
}

/// Whether a block name is the one a `{% block %}` tag gives.
fn is_block_tag(template: &Template, start: u32) -> bool {
    template
        .items
        .iter()
        .any(|item| matches!(item, Item::Tag(Tag { body: TagBody::Block { name, .. }, .. }) if name.1 == start))
}

/// The keys of a hash literal, for the variables `include ... with {...}` passes.
pub fn hash_keys(expr: &Expr) -> Vec<(String, &Expr)> {
    let Expr::Hash(pairs) = expr else {
        return Vec::new();
    };
    pairs
        .iter()
        .filter_map(|(key, value)| match key {
            HashKey::Name(name, ..) | HashKey::Str(name, ..) => Some((name.clone(), value)),
            HashKey::Expr(_) => None,
        })
        .collect()
}

#[cfg(test)]
mod tests;
