//! Semantic tokens: what each name and variable of a file is, so an editor can color it by meaning
//! and not only by shape. Keywords, strings, numbers and comments are left to the editor's grammar;
//! the tokens here are names, variables, and the tags and types inside doc comments.

use php_index::{ClassKind, Index, Origin, Type};
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, SyntaxToken, TextRange, WalkEvent};

use crate::ast::{self, last_segment_of_token, range_of};
use crate::context::FileContext;
use crate::doc_refs::{DocItemKind, MemberKind, doc_items};
use crate::infer::Analyzer;
use crate::refs::{Symbol, is_name_token, symbols_of_token};

/// The token types of the legend, in the order a token's `ty` indexes them.
pub const TOKEN_TYPES: &[&str] = &[
    "namespace",
    "class",
    "interface",
    "enum",
    "struct",
    "typeParameter",
    "parameter",
    "variable",
    "property",
    "enumMember",
    "function",
    "method",
    "keyword",
    "decorator",
];

/// The modifiers of the legend: bit `n` of a token's `modifiers` is entry `n` here.
pub const TOKEN_MODIFIERS: &[&str] = &[
    "declaration",
    "readonly",
    "static",
    "deprecated",
    "abstract",
    "defaultLibrary",
    "documentation",
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
enum Kind {
    Namespace = 0,
    Class,
    Interface,
    Enum,
    Struct,
    TypeParameter,
    Parameter,
    Variable,
    Property,
    EnumMember,
    Function,
    Method,
    Keyword,
    Decorator,
}

const DECLARATION: u32 = 1;
const READONLY: u32 = 1 << 1;
const STATIC: u32 = 1 << 2;
const DEPRECATED: u32 = 1 << 3;
const ABSTRACT: u32 = 1 << 4;
const DEFAULT_LIBRARY: u32 = 1 << 5;
const DOCUMENTATION: u32 = 1 << 6;

/// A token of a file: a range on one line, an index into [`TOKEN_TYPES`] and a set of bits of
/// [`TOKEN_MODIFIERS`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SemanticToken {
    pub start: u32,
    pub end: u32,
    pub ty: u32,
    pub modifiers: u32,
}

/// The names that are not names: types the language has and keywords that stand where one would be.
const BUILTIN: &[&str] = &[
    "int", "float", "bool", "string", "array", "callable", "iterable", "object", "mixed", "void", "never", "null",
    "false", "true", "self", "static", "parent",
];

/// The tokens of a file, in order, or the ones that touch a range of it.
pub fn semantic_tokens(index: &Index, root: &SyntaxNode, range: Option<TextRange>) -> Vec<SemanticToken> {
    let ctx = FileContext::new(index, root);
    let mut out: Vec<SemanticToken> = Vec::new();
    let mut preorder = root.preorder_with_tokens();
    while let Some(event) = preorder.next() {
        let WalkEvent::Enter(element) = event else {
            continue;
        };
        match element {
            php_syntax::SyntaxElement::Node(node) => {
                if range.is_some_and(|range| range.intersect(node.text_range()).is_none()) {
                    preorder.skip_subtree();
                }
            }
            php_syntax::SyntaxElement::Token(token) => {
                if range.is_some_and(|range| range.intersect(token.text_range()).is_none()) {
                    continue;
                }
                match token.kind() {
                    DOC_COMMENT => doc_tokens(&ctx, &token, &mut out),
                    VARIABLE => variable_token(&ctx, &token, &mut out),
                    STRING_LITERAL if token.text().contains('$') => route_path_tokens(&ctx, &token, &mut out),
                    kind if is_name_token(&token, kind) => {
                        name_token(&ctx, &token, &mut out);
                    }
                    _ => {}
                }
            }
        }
    }
    out.sort_by_key(|token| token.start);
    out
}

/// The `$name` segments of a Raxos route path, as the parameters they are.
fn route_path_tokens(ctx: &FileContext, token: &SyntaxToken, out: &mut Vec<SemanticToken>) {
    let Some(literal) = token.parent().filter(|node| node.kind() == LITERAL) else {
        return;
    };
    let analyzer = ctx.analyzer(&literal);
    for segment in crate::frameworks::raxos::route_segments(&analyzer, &literal) {
        push(out, segment.range, Kind::Parameter, 0);
    }
}

fn push(out: &mut Vec<SemanticToken>, range: TextRange, kind: Kind, modifiers: u32) {
    if range.is_empty() {
        return;
    }
    out.push(SemanticToken {
        start: u32::from(range.start()),
        end: u32::from(range.end()),
        ty: kind as u32,
        modifiers,
    });
}

fn class_kind(kind: ClassKind) -> Kind {
    match kind {
        ClassKind::Class => Kind::Class,
        ClassKind::Interface => Kind::Interface,
        ClassKind::Enum => Kind::Enum,
        ClassKind::Trait => Kind::Struct,
    }
}

fn origin_bit(origin: Origin) -> u32 {
    if origin == Origin::Stub { DEFAULT_LIBRARY } else { 0 }
}

fn deprecated_bit(doc: Option<&php_index::Doc>) -> u32 {
    if doc.is_some_and(|doc| doc.deprecated.is_some()) {
        DEPRECATED
    } else {
        0
    }
}

/// What a symbol is, as a token type and the modifiers its declaration gives it.
fn describe(index: &Index, symbol: &Symbol) -> Option<(Kind, u32)> {
    match symbol {
        Symbol::Class(name) => match index.class(name) {
            Some(class) => {
                let mut modifiers = origin_bit(class.file.origin) | deprecated_bit(class.decl.doc.as_deref());
                if class.decl.is_abstract {
                    modifiers |= ABSTRACT;
                }
                Some((class_kind(class.decl.kind), modifiers))
            }
            None => Some((Kind::Class, 0)),
        },
        Symbol::Function(name) => {
            let function = index.function(name)?;
            Some((
                Kind::Function,
                origin_bit(function.file.origin) | deprecated_bit(function.decl.doc.as_deref()),
            ))
        }
        Symbol::Constant(name) => {
            let constant = index.constant(name)?;
            Some((
                Kind::Variable,
                READONLY | origin_bit(constant.file.origin) | deprecated_bit(constant.decl.doc.as_deref()),
            ))
        }
        Symbol::Method { class, name } => {
            let class = index.class(class)?;
            let method = class.decl.method(name);
            let mut modifiers = origin_bit(class.file.origin);
            if let Some(method) = method {
                if method.is_static {
                    modifiers |= STATIC;
                }
                if method.is_abstract {
                    modifiers |= ABSTRACT;
                }
                modifiers |= deprecated_bit(method.doc.as_deref());
            }
            Some((Kind::Method, modifiers))
        }
        Symbol::Property { class, name } => {
            let class = index.class(class)?;
            let mut modifiers = origin_bit(class.file.origin);
            if let Some(property) = class.decl.property(name) {
                if property.is_static {
                    modifiers |= STATIC;
                }
                if property.is_readonly || class.decl.is_readonly {
                    modifiers |= READONLY;
                }
                modifiers |= deprecated_bit(property.doc.as_deref());
            }
            Some((Kind::Property, modifiers))
        }
        Symbol::ClassConst { class, name } => {
            let class = index.class(class)?;
            let constant = class.decl.constant(name)?;
            let modifiers = origin_bit(class.file.origin) | deprecated_bit(constant.doc.as_deref());
            if constant.is_case {
                Some((Kind::EnumMember, modifiers | READONLY | STATIC))
            } else {
                Some((Kind::Property, modifiers | READONLY | STATIC))
            }
        }
        Symbol::Parameter { .. } => Some((Kind::Parameter, 0)),
        Symbol::Variable { .. } => Some((Kind::Variable, 0)),
        Symbol::Dataset(_) | Symbol::Key { .. } => None,
    }
}

fn name_token(ctx: &FileContext, token: &SyntaxToken, out: &mut Vec<SemanticToken>) {
    let Some(parent) = token.parent() else {
        return;
    };
    if parent.kind() == ARGUMENT {
        if ast::has_token(&parent, COLON) {
            push(out, token.text_range(), Kind::Parameter, 0);
        }
        return;
    }
    if parent.kind() != NAME || is_alias_declaration(&parent) {
        return;
    }
    let owner_kind = parent.parent().map(|owner| owner.kind());
    if matches!(owner_kind, Some(NAMESPACE_DECLARATION | USE_GROUP)) {
        push(out, token.text_range(), Kind::Namespace, 0);
        return;
    }
    if owner_kind == Some(ATTRIBUTE) {
        attribute_name_token(ctx, token, out);
        return;
    }
    let written = token.text();
    if BUILTIN.contains(&written.to_ascii_lowercase().as_str()) {
        return;
    }
    let symbols = symbols_of_token(ctx, token);
    let Some(symbol) = symbols.first() else {
        return;
    };
    let Some((kind, mut modifiers)) = describe(ctx.index, symbol) else {
        return;
    };
    if is_declaration_name(&parent) {
        modifiers |= DECLARATION;
    }
    let name_range = last_segment_of_token(token);
    let token_start = u32::from(token.text_range().start());
    let name_start = u32::from(name_range.start());
    if name_start > token_start {
        let lead = (written.len() - written.trim_start_matches('\\').len()) as u32;
        push(out, range_of(token_start + lead, name_start - 1), Kind::Namespace, 0);
    }
    push(out, name_range, kind, modifiers);
}

/// The name of an attribute is one token of a single type from its first character to its last,
/// whatever namespace it is written in, so an editor draws all of it as the attribute it names.
fn attribute_name_token(ctx: &FileContext, token: &SyntaxToken, out: &mut Vec<SemanticToken>) {
    let modifiers = symbols_of_token(ctx, token)
        .first()
        .and_then(|symbol| describe(ctx.index, symbol))
        .map_or(0, |(_, modifiers)| modifiers & (DEPRECATED | DEFAULT_LIBRARY));
    push(out, token.text_range(), Kind::Decorator, modifiers);
}

fn is_alias_declaration(name: &SyntaxNode) -> bool {
    name.parent()
        .is_some_and(|owner| matches!(owner.kind(), USE_CLAUSE | TRAIT_ALIAS))
        && name
            .siblings_with_tokens(php_syntax::Direction::Prev)
            .any(|element| element.kind() == AS_KW)
}

fn is_declaration_name(name: &SyntaxNode) -> bool {
    let Some(owner) = name.parent() else {
        return false;
    };
    match owner.kind() {
        CLASS_DECLARATION
        | INTERFACE_DECLARATION
        | TRAIT_DECLARATION
        | ENUM_DECLARATION
        | FUNCTION_DECLARATION
        | METHOD_DECLARATION
        | ENUM_CASE => owner.children().find(|child| child.kind() == NAME).as_ref() == Some(name),
        CONST_ELEMENT => owner.children().next().as_ref() == Some(name),
        _ => false,
    }
}

fn variable_token(ctx: &FileContext, token: &SyntaxToken, out: &mut Vec<SemanticToken>) {
    let Some(parent) = token.parent() else {
        return;
    };
    let name = token.text().trim_start_matches('$');
    if name == "this" {
        return;
    }
    let symbols = symbols_of_token(ctx, token);
    let promoted = parent.kind() == PARAMETER && ast::child_of(&parent, MODIFIER_LIST).is_some();
    let variable = symbols.iter().find_map(|symbol| match symbol {
        Symbol::Variable { name, scope } => Some((name, *scope)),
        _ => None,
    });
    let (kind, mut modifiers) = match variable {
        Some((name, scope)) if !promoted => {
            let is_parameter = scope_has_parameter(&ctx.root, scope, name);
            (if is_parameter { Kind::Parameter } else { Kind::Variable }, 0)
        }
        _ => {
            let Some(property) = symbols.iter().find(|symbol| matches!(symbol, Symbol::Property { .. })) else {
                return;
            };
            describe(ctx.index, property).unwrap_or((Kind::Property, 0))
        }
    };
    if matches!(parent.kind(), PARAMETER | PROPERTY_ELEMENT) {
        modifiers |= DECLARATION;
    }
    push(out, token.text_range(), kind, modifiers);
}

fn scope_has_parameter(root: &SyntaxNode, scope: (u32, u32), name: &str) -> bool {
    let range = range_of(scope.0, scope.1);
    if range == root.text_range() {
        return false;
    }
    let node = match root.covering_element(range) {
        php_syntax::SyntaxElement::Node(node) => node,
        php_syntax::SyntaxElement::Token(token) => match token.parent() {
            Some(parent) => parent,
            None => return false,
        },
    };
    let Some(function) = node.ancestors().find(|ancestor| ast::is_function_like(ancestor.kind())) else {
        return false;
    };
    let wanted = format!("${name}");
    ast::child_of(&function, PARAMETER_LIST).is_some_and(|list| {
        list.children()
            .filter(|child| child.kind() == PARAMETER)
            .any(|parameter| ast::first_token(&parameter, VARIABLE).is_some_and(|token| token.text() == wanted))
    })
}

// Doc comments ----------------------------------------------------------------------------------

fn doc_tokens(ctx: &FileContext, token: &SyntaxToken, out: &mut Vec<SemanticToken>) {
    let Some(parent) = token.parent() else {
        return;
    };
    let text = token.text();
    let base = u32::from(token.text_range().start());
    let analyzer = ctx.analyzer(&parent);
    for item in doc_items(text, base) {
        let range = range_of(item.start, item.end);
        match &item.kind {
            DocItemKind::Tag(_) => push(out, range, Kind::Keyword, DOCUMENTATION),
            DocItemKind::Template(_) => push(out, range, Kind::TypeParameter, DOCUMENTATION),
            DocItemKind::Class(raw) => {
                let lower = raw.to_ascii_lowercase();
                if matches!(lower.as_str(), "self" | "static" | "parent" | "this") {
                    continue;
                }
                let resolved = analyzer.resolver.resolve_class(raw);
                let local = &text[(item.start - base) as usize..(item.end - base) as usize];
                let last = local.rfind('\\').map_or(0, |at| at + 1) as u32;
                let name_range = range_of(item.start + last, item.end);
                let (kind, modifiers) = if ctx.index.class(&resolved).is_some() {
                    match describe(ctx.index, &Symbol::Class(resolved)) {
                        Some(found) => found,
                        None => continue,
                    }
                } else if names_a_template(&analyzer, raw) {
                    (Kind::TypeParameter, 0)
                } else {
                    continue;
                };
                push(out, name_range, kind, modifiers | DOCUMENTATION);
            }
            DocItemKind::Variable(name) => {
                let is_parameter = parent
                    .ancestors()
                    .find(|node| ast::is_function_like(node.kind()))
                    .is_some_and(|function| {
                        let wanted = format!("${name}");
                        ast::child_of(&function, PARAMETER_LIST).is_some_and(|list| {
                            list.children().any(|parameter| {
                                ast::first_token(&parameter, VARIABLE).is_some_and(|token| token.text() == wanted)
                            })
                        })
                    });
                push(
                    out,
                    range,
                    if is_parameter { Kind::Parameter } else { Kind::Variable },
                    DOCUMENTATION,
                );
            }
            DocItemKind::PropertyDecl(_) => push(out, range, Kind::Property, DECLARATION | DOCUMENTATION),
            DocItemKind::MethodDecl(_) => push(out, range, Kind::Method, DECLARATION | DOCUMENTATION),
            DocItemKind::Member { class, name, kind, .. } => {
                let class_type = match class.to_ascii_lowercase().as_str() {
                    "self" | "static" => analyzer
                        .class
                        .as_ref()
                        .map_or(Type::Unknown, |class| Type::class(class.name.clone())),
                    _ => Type::class(analyzer.resolver.resolve_class(class)),
                };
                let symbol = match kind {
                    MemberKind::Method => ctx.index.find_method(&class_type, name).map(|found| Symbol::Method {
                        class: found.class.decl.name.clone(),
                        name: found.member.name.clone(),
                    }),
                    MemberKind::Property => ctx
                        .index
                        .find_property(&class_type, name)
                        .map(|found| Symbol::Property {
                            class: found.class.decl.name.clone(),
                            name: found.member.name.clone(),
                        }),
                    MemberKind::Constant => {
                        ctx.index
                            .find_constant(&class_type, name)
                            .map(|found| Symbol::ClassConst {
                                class: found.class.decl.name.clone(),
                                name: found.member.name.clone(),
                            })
                    }
                };
                if let Some((kind, modifiers)) = symbol.and_then(|symbol| describe(ctx.index, &symbol)) {
                    push(out, range, kind, modifiers | DOCUMENTATION);
                }
            }
            DocItemKind::Function(name) => {
                let candidates = analyzer.resolver.function_candidates(name);
                if let Some(function) = ctx.index.first_function(&candidates) {
                    push(
                        out,
                        range,
                        Kind::Function,
                        DOCUMENTATION | origin_bit(function.file.origin),
                    );
                }
            }
        }
    }
}

/// Whether a name in a doc comment is a template of the class around it.
fn names_a_template(analyzer: &Analyzer<'_>, raw: &str) -> bool {
    analyzer
        .class
        .as_ref()
        .and_then(|class| analyzer.index.class(&class.name))
        .and_then(|class| {
            class
                .decl
                .doc
                .as_ref()
                .map(|doc| doc.templates.iter().any(|template| template.name == raw))
        })
        .unwrap_or(false)
}
