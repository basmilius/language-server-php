//! Where a declaration is used. A name in a file resolves to a [`Symbol`], a [`Query`] says which
//! symbols count as the same thing (a method and the methods that override it), and
//! [`hits_in_file`] finds every place of a file that names one.

use std::collections::HashSet;

use php_index::framework::keys::KeyKind;
use php_index::{ClassDecl, Index, Name, Type};
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxKind, SyntaxNode, SyntaxToken, TextRange};

use crate::ast::{self, child_of, enclosing_function, last_segment, range_of, start};
use crate::context::FileContext;
use crate::doc_refs::{DocItemKind, MemberKind, doc_items};
use crate::infer::Analyzer;
use crate::target::{Callee, Target};

/// What a name stands for, with members named by the class that declares them.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Symbol {
    Class(Name),
    Function(Name),
    Constant(Name),
    Method {
        class: Name,
        name: String,
    },
    Property {
        class: Name,
        name: String,
    },
    ClassConst {
        class: Name,
        name: String,
    },
    Parameter {
        callee: Callee,
        name: String,
    },
    /// A local variable, by the range of the function or file it lives in.
    Variable {
        name: String,
        scope: (u32, u32),
    },
    /// A Pest dataset, by the name it is declared and used under.
    Dataset(String),
    /// A string that names a route, a config key, a view and the like. The scope is the class a
    /// form request field or an entity field belongs to.
    Key {
        kind: KeyKind,
        name: String,
        scope: Option<String>,
    },
}

impl Symbol {
    /// The lowercase name to look for in the text of a file.
    pub fn word(&self) -> String {
        let name = match self {
            Symbol::Class(name) | Symbol::Function(name) | Symbol::Constant(name) => crate::short(name),
            Symbol::Method { name, .. }
            | Symbol::Property { name, .. }
            | Symbol::ClassConst { name, .. }
            | Symbol::Parameter { name, .. }
            | Symbol::Variable { name, .. }
            | Symbol::Dataset(name)
            | Symbol::Key { name, .. } => name,
        };
        match self {
            Symbol::Dataset(name) => name
                .split(|c: char| !c.is_alphanumeric() && c != '_')
                .find(|word| !word.is_empty())
                .unwrap_or(name)
                .to_lowercase(),
            _ => name.to_ascii_lowercase(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitKind {
    /// The name in the declaration itself.
    Declaration,
    Reference,
    /// A name in a `use` statement.
    Import,
    /// A name in a doc comment.
    Doc,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Read,
    Write,
}

/// A place that names a symbol.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hit {
    /// The name alone: the last segment of a qualified name, the `$` of a variable included.
    pub range: TextRange,
    pub kind: HitKind,
    pub access: Access,
    /// The name is written with a `$`.
    pub dollar: bool,
    /// A class written under an alias, which a rename of the class leaves alone.
    pub via_alias: bool,
    pub symbol: Symbol,
}

/// A symbol and every declaration that is one thing with it.
pub struct Query {
    pub symbol: Symbol,
    /// Lowercase names of the classes whose member is the symbol, for a member.
    family: HashSet<String>,
    /// Words besides the symbol's own that a file naming it may hold. A constructor is also called
    /// by `new` and the name of the class (or of a class below it, that has none of its own).
    extra_words: Vec<String>,
}

impl Query {
    pub fn new(index: &Index, symbol: Symbol) -> Query {
        let family = match &symbol {
            Symbol::Method { class, name } => {
                if name.eq_ignore_ascii_case("__construct") || is_private(index, &symbol) {
                    single(class)
                } else {
                    member_family(index, class, |decl| declares_method(decl, name))
                }
            }
            Symbol::Property { class, name } => {
                if is_private(index, &symbol) {
                    single(class)
                } else {
                    member_family(index, class, |decl| declares_property(decl, name))
                }
            }
            Symbol::ClassConst { class, name } => {
                if is_private(index, &symbol) {
                    single(class)
                } else {
                    member_family(index, class, |decl| decl.constant(name).is_some())
                }
            }
            _ => HashSet::new(),
        };
        let extra_words = match &symbol {
            Symbol::Method { class, name } if name.eq_ignore_ascii_case("__construct") => {
                let mut words = vec![crate::short(class).to_ascii_lowercase()];
                words.extend(
                    index
                        .all_subtypes(class)
                        .iter()
                        .map(|subtype| crate::short(&subtype.decl.name).to_ascii_lowercase()),
                );
                words
            }
            // Twig reads `getTitle()` as `post.title`.
            Symbol::Method { name, .. } if index.frameworks().symfony || index.frameworks().twig => {
                let twig = crate::twig::types::twig_name(name).to_ascii_lowercase();
                if twig == name.to_ascii_lowercase() {
                    Vec::new()
                } else {
                    vec![twig]
                }
            }
            _ => Vec::new(),
        };
        Query {
            symbol,
            family,
            extra_words,
        }
    }

    /// The lowercase words a file must hold to name the symbol.
    pub fn words(&self) -> Vec<String> {
        let mut words = vec![self.symbol.word()];
        words.extend(self.extra_words.iter().cloned());
        words
    }

    fn is_constructor(&self) -> bool {
        matches!(&self.symbol, Symbol::Method { name, .. } if name.eq_ignore_ascii_case("__construct"))
    }

    /// The classes a member is one thing across, lowercase.
    pub fn family(&self) -> &HashSet<String> {
        &self.family
    }

    pub fn matches(&self, other: &Symbol) -> bool {
        match (&self.symbol, other) {
            (Symbol::Class(left), Symbol::Class(right)) | (Symbol::Function(left), Symbol::Function(right)) => left
                .trim_start_matches('\\')
                .eq_ignore_ascii_case(right.trim_start_matches('\\')),
            (Symbol::Constant(left), Symbol::Constant(right)) => same_constant(left, right),
            (Symbol::Method { name: wanted, .. }, Symbol::Method { class, name }) => {
                wanted.eq_ignore_ascii_case(name) && self.family.contains(&class.to_ascii_lowercase())
            }
            (Symbol::Property { name: wanted, .. }, Symbol::Property { class, name })
            | (Symbol::ClassConst { name: wanted, .. }, Symbol::ClassConst { class, name }) => {
                wanted == name && self.family.contains(&class.to_ascii_lowercase())
            }
            (
                Symbol::Parameter {
                    callee: wanted,
                    name: wanted_name,
                },
                Symbol::Parameter { callee, name },
            ) => wanted_name == name && same_callee(wanted, callee),
            (Symbol::Dataset(wanted), Symbol::Dataset(name)) => wanted == name,
            (
                Symbol::Key {
                    kind: wanted_kind,
                    name: wanted_name,
                    scope: wanted_scope,
                },
                Symbol::Key { kind, name, scope },
            ) => {
                let same_name = match kind {
                    KeyKind::Attribute => {
                        php_index::framework::layouts::attribute_key(wanted_name)
                            == php_index::framework::layouts::attribute_key(name)
                    }
                    _ => wanted_name == name,
                };
                wanted_kind == kind
                    && same_name
                    && (!matches!(
                        kind,
                        KeyKind::Field | KeyKind::EntityField | KeyKind::Slot | KeyKind::Attribute
                    ) || wanted_scope.as_deref().map(str::to_ascii_lowercase)
                        == scope.as_deref().map(str::to_ascii_lowercase))
            }
            _ => false,
        }
    }
}

fn single(class: &str) -> HashSet<String> {
    HashSet::from([class.to_ascii_lowercase()])
}

fn same_constant(left: &str, right: &str) -> bool {
    let (left, right) = (left.trim_start_matches('\\'), right.trim_start_matches('\\'));
    match (left.rsplit_once('\\'), right.rsplit_once('\\')) {
        (Some((left_ns, left_name)), Some((right_ns, right_name))) => {
            left_ns.eq_ignore_ascii_case(right_ns) && left_name == right_name
        }
        (None, None) => left == right,
        _ => false,
    }
}

fn same_callee(left: &Callee, right: &Callee) -> bool {
    match (left, right) {
        (Callee::Function(left), Callee::Function(right)) => left.eq_ignore_ascii_case(right),
        (
            Callee::Method { class, name },
            Callee::Method {
                class: other_class,
                name: other_name,
            },
        ) => class.eq_ignore_ascii_case(other_class) && name.eq_ignore_ascii_case(other_name),
        _ => false,
    }
}

pub(crate) fn declares_method(decl: &ClassDecl, name: &str) -> bool {
    decl.method(name).is_some()
        || decl
            .doc
            .as_ref()
            .is_some_and(|doc| doc.methods.iter().any(|method| method.name.eq_ignore_ascii_case(name)))
}

pub(crate) fn declares_property(decl: &ClassDecl, name: &str) -> bool {
    decl.property(name).is_some()
        || decl
            .doc
            .as_ref()
            .is_some_and(|doc| doc.properties.iter().any(|property| property.name == name))
}

fn is_private(index: &Index, symbol: &Symbol) -> bool {
    use php_index::Visibility::Private;
    match symbol {
        Symbol::Method { class, name } => index
            .class(class)
            .and_then(|found| found.decl.method(name).map(|method| method.visibility == Private))
            .unwrap_or(false),
        Symbol::Property { class, name } => index
            .class(class)
            .and_then(|found| found.decl.property(name).map(|property| property.visibility == Private))
            .unwrap_or(false),
        Symbol::ClassConst { class, name } => index
            .class(class)
            .and_then(|found| found.decl.constant(name).map(|constant| constant.visibility == Private))
            .unwrap_or(false),
        _ => false,
    }
}

/// The classes that declare a member and are linked to the one that does by `extends`, `implements`
/// and trait use, in either direction.
fn member_family(index: &Index, class: &str, declares: impl Fn(&ClassDecl) -> bool) -> HashSet<String> {
    let mut family = single(class);
    let mut pending = vec![class.to_string()];
    let mut seen = HashSet::new();
    while let Some(current) = pending.pop() {
        if !seen.insert(current.to_ascii_lowercase()) {
            continue;
        }
        let up = index.ancestors(&Type::class(current.clone()));
        let down = index.all_subtypes(&current);
        let related = up
            .iter()
            .map(|ancestor| ancestor.class.decl)
            .chain(down.iter().map(|subtype| subtype.decl));
        for decl in related {
            if declares(decl) && family.insert(decl.name.to_ascii_lowercase()) {
                pending.push(decl.name.clone());
            }
        }
    }
    family
}

/// What the tokens of a name or variable under an offset stand for.
pub fn symbols_of_token(ctx: &FileContext, token: &SyntaxToken) -> Vec<Symbol> {
    let Some(parent) = token.parent() else {
        return Vec::new();
    };
    let analyzer = ctx.analyzer(&parent);
    if token.kind() == VARIABLE {
        return variable_symbols(&analyzer, token, &parent);
    }
    from_targets(&analyzer, analyzer.targets_of_token(token))
}

fn variable_symbols(analyzer: &Analyzer<'_>, token: &SyntaxToken, parent: &SyntaxNode) -> Vec<Symbol> {
    let name = token.text().trim_start_matches('$').to_string();
    if name == "this" {
        return Vec::new();
    }
    let static_property = parent.kind() == VARIABLE_EXPR
        && parent.parent().is_some_and(|grand| {
            grand.kind() == STATIC_PROPERTY_EXPR && grand.children().next().as_ref() != Some(parent)
        });
    if static_property || parent.kind() == PROPERTY_ELEMENT {
        return from_targets(analyzer, analyzer.targets_of_token(token));
    }
    let mut out = Vec::new();
    match parent.kind() {
        VARIABLE_EXPR | CLOSURE_USE_VARIABLE | STATIC_VARIABLE | CATCH_CLAUSE | PARAMETER => {
            let scope = binding_scope(&analyzer.root, token, &name);
            out.push(Symbol::Variable {
                name: name.clone(),
                scope: (start(&scope), ast::end(&scope)),
            });
        }
        _ => {}
    }
    if parent.kind() == PARAMETER {
        if let Some(function) = parent.ancestors().find(|node| ast::is_function_like(node.kind())) {
            if let Some(callee) = callee_of_declaration(analyzer, &function) {
                out.push(Symbol::Parameter {
                    callee,
                    name: name.clone(),
                });
            }
        }
        if child_of(parent, MODIFIER_LIST).is_some() {
            if let Some(class) = &analyzer.class {
                out.push(Symbol::Property {
                    class: class.name.clone(),
                    name,
                });
            }
        }
    }
    out
}

/// The function or method a declaration node is, as a call names it.
pub fn callee_of_declaration(analyzer: &Analyzer<'_>, function: &SyntaxNode) -> Option<Callee> {
    let name = child_of(function, NAME).map(|name| ast::text_of(&name))?;
    match function.kind() {
        FUNCTION_DECLARATION => Some(Callee::Function(analyzer.resolver.qualify(&name))),
        METHOD_DECLARATION => analyzer.class.as_ref().map(|class| Callee::Method {
            class: class.name.clone(),
            name,
        }),
        _ => None,
    }
}

pub fn from_targets(analyzer: &Analyzer<'_>, targets: Vec<Target>) -> Vec<Symbol> {
    let index = analyzer.index;
    let mut out: Vec<Symbol> = Vec::new();
    let mut push = |symbol: Symbol| {
        if !out.contains(&symbol) {
            out.push(symbol);
        }
    };
    for target in targets {
        match target {
            Target::Class(name) => push(Symbol::Class(name)),
            Target::Function(name) => push(Symbol::Function(name)),
            Target::Constant(name) => push(Symbol::Constant(name)),
            Target::Method { receiver, name } => {
                for member in receiver.members() {
                    if let Some(found) = index.find_method(member, &name) {
                        push(Symbol::Method {
                            class: found.class.decl.name.clone(),
                            name: found.member.name.clone(),
                        });
                    }
                }
            }
            Target::Property { receiver, name } => {
                for member in receiver.members() {
                    if let Some(found) = index.find_property(member, &name) {
                        push(Symbol::Property {
                            class: found.class.decl.name.clone(),
                            name: found.member.name.clone(),
                        });
                    }
                }
            }
            Target::ClassConst { receiver, name } => {
                for member in receiver.members() {
                    if let Some(found) = index.find_constant(member, &name) {
                        push(Symbol::ClassConst {
                            class: found.class.decl.name.clone(),
                            name: found.member.name.clone(),
                        });
                    }
                }
            }
            Target::Parameter { callee, name } => {
                if let Callee::Method { class, name: method } = &callee {
                    let promoted = index
                        .class(class)
                        .and_then(|found| found.decl.method(method).map(|method| method.callable.params.clone()))
                        .is_some_and(|params| {
                            params
                                .iter()
                                .any(|param| param.name == name && param.promoted.is_some())
                        });
                    if promoted && method.eq_ignore_ascii_case("__construct") {
                        push(Symbol::Property {
                            class: class.clone(),
                            name: name.clone(),
                        });
                    }
                }
                push(Symbol::Parameter { callee, name });
            }
            Target::Dataset(name) => push(Symbol::Dataset(name)),
            Target::Key { kind, name, scope } => push(Symbol::Key { kind, name, scope }),
            Target::Variable { .. } => {}
        }
    }
    out
}

// Variables -------------------------------------------------------------------------------------

fn has_parameter(function: &SyntaxNode, name: &str) -> bool {
    let wanted = format!("${name}");
    child_of(function, PARAMETER_LIST).is_some_and(|list| {
        list.children()
            .filter(|child| child.kind() == PARAMETER)
            .any(|parameter| ast::first_token(&parameter, VARIABLE).is_some_and(|token| token.text() == wanted))
    })
}

fn uses_variable(closure: &SyntaxNode, name: &str) -> bool {
    let wanted = format!("${name}");
    child_of(closure, CLOSURE_USE).is_some_and(|uses| {
        uses.children()
            .filter(|child| child.kind() == CLOSURE_USE_VARIABLE)
            .any(|variable| ast::first_token(&variable, VARIABLE).is_some_and(|token| token.text() == wanted))
    })
}

/// The function, closure or file whose variable a `$name` at a token is: an arrow function sees
/// the variables around it, and a closure sees the ones it lists after `use`.
pub fn binding_scope(root: &SyntaxNode, token: &SyntaxToken, name: &str) -> SyntaxNode {
    let mut from = token.parent();
    if let Some(parent) = from.clone().filter(|parent| parent.kind() == CLOSURE_USE_VARIABLE) {
        // A name after `use` is the variable of the scope around the closure.
        from = parent
            .ancestors()
            .find(|node| node.kind() == CLOSURE_EXPR)
            .and_then(|closure| closure.parent());
    }
    loop {
        let Some(scope) = from.as_ref().and_then(enclosing_function) else {
            return root.clone();
        };
        let outward = match scope.kind() {
            CLOSURE_EXPR => !has_parameter(&scope, name) && uses_variable(&scope, name),
            ARROW_FUNCTION_EXPR => !has_parameter(&scope, name),
            _ => false,
        };
        if !outward {
            return scope;
        }
        from = scope.parent();
    }
}

/// Every place the variable `name` of a scope appears, in the order of the file.
pub fn variable_hits(ctx: &FileContext, scope: (u32, u32), name: &str) -> Vec<Hit> {
    let scope_node = scope_node(&ctx.root, scope);
    let wanted = format!("${name}");
    let mut hits = Vec::new();
    let symbol = Symbol::Variable {
        name: name.to_string(),
        scope,
    };
    let mut preorder = scope_node.preorder_with_tokens();
    while let Some(event) = preorder.next() {
        let php_syntax::WalkEvent::Enter(element) = event else {
            continue;
        };
        match element {
            php_syntax::SyntaxElement::Node(node) => {
                if node != scope_node && is_separate_scope(&node, name) {
                    preorder.skip_subtree();
                }
            }
            php_syntax::SyntaxElement::Token(token) => {
                if token.kind() == DOC_COMMENT {
                    doc_variable_hits(&token, name, &symbol, &mut hits);
                    continue;
                }
                if token.kind() != VARIABLE || token.text() != wanted {
                    continue;
                }
                let Some(parent) = token.parent() else {
                    continue;
                };
                if is_property_name(&parent) {
                    continue;
                }
                let declaration = parent.kind() == PARAMETER;
                let access = if declaration
                    || is_write_target(&parent)
                    || matches!(parent.kind(), CATCH_CLAUSE | STATIC_VARIABLE)
                {
                    Access::Write
                } else {
                    Access::Read
                };
                hits.push(Hit {
                    range: token.text_range(),
                    kind: if declaration {
                        HitKind::Declaration
                    } else {
                        HitKind::Reference
                    },
                    access,
                    dollar: true,
                    via_alias: false,
                    symbol: symbol.clone(),
                });
            }
        }
    }
    hits
}

fn scope_node(root: &SyntaxNode, scope: (u32, u32)) -> SyntaxNode {
    let range = range_of(scope.0, scope.1);
    if range == root.text_range() {
        return root.clone();
    }
    match root.covering_element(range) {
        php_syntax::SyntaxElement::Node(node) => node
            .ancestors()
            .find(|ancestor| ancestor.text_range() == range)
            .unwrap_or(node),
        php_syntax::SyntaxElement::Token(token) => token.parent().unwrap_or_else(|| root.clone()),
    }
}

/// A node inside a scope that has variables of its own for `name`.
fn is_separate_scope(node: &SyntaxNode, name: &str) -> bool {
    match node.kind() {
        FUNCTION_DECLARATION | METHOD_DECLARATION | PROPERTY_HOOK => true,
        kind if ast::is_class_like(kind) => true,
        CLOSURE_EXPR => !uses_variable(node, name) || has_parameter(node, name),
        ARROW_FUNCTION_EXPR => has_parameter(node, name),
        _ => false,
    }
}

/// A variable token that is not a variable: the name of a property.
fn is_property_name(parent: &SyntaxNode) -> bool {
    match parent.kind() {
        PROPERTY_ELEMENT => true,
        VARIABLE_EXPR => parent.parent().is_some_and(|grand| {
            grand.kind() == STATIC_PROPERTY_EXPR && grand.children().next().as_ref() != Some(parent)
        }),
        _ => false,
    }
}

fn doc_variable_hits(token: &SyntaxToken, name: &str, symbol: &Symbol, hits: &mut Vec<Hit>) {
    let text = token.text();
    if !text.contains(name) {
        return;
    }
    let base = u32::from(token.text_range().start());
    for item in doc_items(text, base) {
        if let DocItemKind::Variable(found) = &item.kind {
            if found == name {
                hits.push(Hit {
                    range: range_of(item.start, item.end),
                    kind: HitKind::Doc,
                    access: Access::Read,
                    dollar: true,
                    via_alias: false,
                    symbol: symbol.clone(),
                });
            }
        }
    }
}

/// Whether a variable, property or element expression is assigned to, incremented, unset or the
/// target of a `foreach` or a destructuring.
pub fn is_write_target(node: &SyntaxNode) -> bool {
    let mut current = node.clone();
    loop {
        let Some(parent) = current.parent() else {
            return false;
        };
        let first = parent.children().next().as_ref() == Some(&current);
        match parent.kind() {
            INDEX_EXPR if first => current = parent,
            ASSIGN_EXPR => return first,
            PREFIX_EXPR | POSTFIX_EXPR => {
                return ast::has_token(&parent, INC) || ast::has_token(&parent, DEC);
            }
            ARRAY_ITEM | ARRAY_EXPR | LIST_EXPR => current = parent,
            FOREACH_STATEMENT => return !first && !matches!(current.kind(), BLOCK | STATEMENT_LIST),
            UNSET_STATEMENT | GLOBAL_STATEMENT => return true,
            _ => return false,
        }
    }
}

// Names -----------------------------------------------------------------------------------------

fn is_declaration_name(token: &SyntaxToken, name: &SyntaxNode) -> bool {
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
        _ => {
            let _ = token;
            false
        }
    }
}

fn node_is_alias_declaration(name: &SyntaxNode) -> bool {
    let Some(owner) = name.parent() else {
        return false;
    };
    match owner.kind() {
        USE_CLAUSE => ast::tokens(&owner)
            .take_while(|token| token.text_range().start() < name.text_range().start())
            .any(|token| token.kind() == AS_KW),
        TRAIT_ALIAS => ast::tokens(&owner)
            .take_while(|token| token.text_range().start() < name.text_range().start())
            .any(|token| token.kind() == AS_KW),
        _ => false,
    }
}

/// A token that spells a name. A member may be called like a keyword (`Lang::DEFAULT`, `->list()`),
/// which the lexer reads as the keyword and the parser puts in a name node all the same.
pub(crate) fn is_name_token(token: &SyntaxToken, kind: SyntaxKind) -> bool {
    match kind {
        IDENT | QUALIFIED_NAME | FULLY_QUALIFIED_NAME | RELATIVE_NAME => true,
        _ => kind.is_keyword() && token.parent().is_some_and(|parent| parent.kind() == NAME),
    }
}

/// Every place of a file that names the symbol of the query.
pub fn hits_in_file(ctx: &FileContext, text: &str, query: &Query) -> Vec<Hit> {
    if let Symbol::Variable { name, scope } = &query.symbol {
        return variable_hits(ctx, *scope, name);
    }
    if let Symbol::Key { name, .. } = &query.symbol {
        return key_hits(ctx, name, query);
    }
    let word = query.symbol.word();
    let aliases = class_aliases(ctx, query);
    let mut hits: Vec<Hit> = Vec::new();
    for element in ctx.root.descendants_with_tokens() {
        let Some(token) = element.into_token() else {
            continue;
        };
        match token.kind() {
            DOC_COMMENT => doc_hits(ctx, &token, &word, query, &mut hits),
            VARIABLE => {
                if !matches!(query.symbol, Symbol::Property { .. })
                    || token.text().trim_start_matches('$').to_ascii_lowercase() != word
                {
                    continue;
                }
                push_token_hit(ctx, text, &token, query, &mut hits, false);
            }
            kind if is_name_token(&token, kind) => {
                let Some(parent) = token.parent() else {
                    continue;
                };
                if parent.kind() == ARGUMENT {
                    if matches!(query.symbol, Symbol::Parameter { .. } | Symbol::Property { .. })
                        && token.text().eq_ignore_ascii_case(&word)
                    {
                        push_token_hit(ctx, text, &token, query, &mut hits, false);
                    }
                    continue;
                }
                if parent.kind() != NAME || node_is_alias_declaration(&parent) {
                    continue;
                }
                let written = token.text();
                let last = written.rsplit('\\').next().unwrap_or(written).to_ascii_lowercase();
                let first = written.trim_start_matches('\\').split('\\').next().unwrap_or(written);
                let aliased = aliases.contains(&first.to_ascii_lowercase());
                if last != word && !aliased {
                    continue;
                }
                push_token_hit(ctx, text, &token, query, &mut hits, aliased);
            }
            _ => {}
        }
    }
    if query.is_constructor() {
        constructor_hits(ctx, text, query, &mut hits);
    }
    if matches!(
        query.symbol,
        Symbol::Method { .. } | Symbol::Function(_) | Symbol::Dataset(_)
    ) {
        test_string_hits(ctx, query, &mut hits);
    }
    if let Symbol::Method { name, .. } = &query.symbol {
        if ctx.index.frameworks().eloquent {
            relation_string_hits(ctx, name, query, &mut hits);
        }
    }
    hits.sort_by_key(|hit| (hit.range.start(), hit.range.end()));
    hits.dedup_by_key(|hit| (hit.range.start(), hit.range.end()));
    hits
}

/// The strings that name a relation: each segment of `with('posts.comments')` is a usage of the
/// relation method it names.
fn relation_string_hits(ctx: &FileContext, name: &str, query: &Query, hits: &mut Vec<Hit>) {
    for node in ctx.root.descendants().filter(|node| node.kind() == LITERAL) {
        if php_index::test_facts::string_value(&node).is_none_or(|(value, _)| !value.contains(name)) {
            continue;
        }
        let analyzer = ctx.analyzer(&node);
        let Some(key) = crate::frameworks::keys::key_of_literal(&analyzer, &node, None) else {
            continue;
        };
        for segment in crate::frameworks::relations::segments(ctx.index, &key) {
            let Some(relation) = segment.relation else {
                continue;
            };
            let symbol = Symbol::Method {
                class: relation.owner,
                name: relation.name,
            };
            if query.matches(&symbol) {
                hits.push(Hit {
                    range: segment.range,
                    kind: HitKind::Reference,
                    access: Access::Read,
                    dollar: false,
                    via_alias: false,
                    symbol,
                });
            }
        }
    }
}

/// The strings that name a route, a config key or the like. Only a literal that holds the name is
/// asked what it is, which is what keeps a search cheap.
fn key_hits(ctx: &FileContext, name: &str, query: &Query) -> Vec<Hit> {
    let mut hits = Vec::new();
    for node in ctx.root.descendants().filter(|node| node.kind() == LITERAL) {
        if php_index::test_facts::string_value(&node).is_none_or(|(value, _)| value != name) {
            continue;
        }
        let analyzer = ctx.analyzer(&node);
        let Some(key) = crate::frameworks::keys::key_of_literal(&analyzer, &node, None) else {
            continue;
        };
        let symbol = Symbol::Key {
            kind: key.kind,
            name: key.value,
            scope: key.scope,
        };
        if query.matches(&symbol) {
            hits.push(Hit {
                range: key.range,
                kind: HitKind::Reference,
                access: Access::Read,
                dollar: false,
                via_alias: false,
                symbol,
            });
        }
    }
    hits
}

/// The strings of a test that name the symbol: a data provider, a dependency, a dataset.
fn test_string_hits(ctx: &FileContext, query: &Query, hits: &mut Vec<Hit>) {
    use crate::phpunit::strings::{strings_in, symbol_of};
    for string in strings_in(ctx) {
        let node = ast::node_at(&ctx.root, u32::from(string.range.start()));
        let analyzer = ctx.analyzer(&node);
        let Some(symbol) = symbol_of(ctx.index, &string, &analyzer) else {
            continue;
        };
        if !query.matches(&symbol) {
            continue;
        }
        hits.push(Hit {
            range: string.range,
            kind: if string.declaration {
                HitKind::Declaration
            } else {
                HitKind::Reference
            },
            access: Access::Read,
            dollar: false,
            via_alias: false,
            symbol,
        });
    }
}

/// A `new Foo` is a call of the constructor Foo has or inherits.
fn constructor_hits(ctx: &FileContext, text: &str, query: &Query, hits: &mut Vec<Hit>) {
    for node in ctx.root.descendants().filter(|node| node.kind() == NEW_EXPR) {
        let Some(name) = node.children().find(|child| child.kind() == NAME) else {
            continue;
        };
        let written = ast::text_of(&name);
        let last = written.rsplit('\\').next().unwrap_or(&written).to_ascii_lowercase();
        let relative = matches!(last.as_str(), "self" | "static" | "parent");
        if !relative && !query.extra_words.contains(&last) {
            continue;
        }
        let analyzer = ctx.analyzer(&name);
        let class = analyzer.class_type(&written);
        let Some(found) = ctx.index.find_method(&class, "__construct") else {
            continue;
        };
        let symbol = Symbol::Method {
            class: found.class.decl.name.clone(),
            name: found.member.name.clone(),
        };
        if !query.matches(&symbol) {
            continue;
        }
        let Some(token) = ast::tokens(&name).find(|token| !token.kind().is_trivia()) else {
            continue;
        };
        hits.push(Hit {
            range: last_segment(token.text_range(), text),
            kind: HitKind::Reference,
            access: Access::Read,
            dollar: false,
            via_alias: false,
            symbol,
        });
    }
}

fn push_token_hit(
    ctx: &FileContext,
    text: &str,
    token: &SyntaxToken,
    query: &Query,
    hits: &mut Vec<Hit>,
    _aliased: bool,
) {
    let symbols = symbols_of_token(ctx, token);
    let Some(symbol) = symbols.into_iter().find(|symbol| query.matches(symbol)) else {
        return;
    };
    let Some(parent) = token.parent() else {
        return;
    };
    let (range, kind, dollar) = if token.kind() == VARIABLE {
        let declaration = matches!(parent.kind(), PROPERTY_ELEMENT | PARAMETER);
        (
            token.text_range(),
            if declaration {
                HitKind::Declaration
            } else {
                HitKind::Reference
            },
            true,
        )
    } else if parent.kind() == ARGUMENT {
        (token.text_range(), HitKind::Reference, false)
    } else {
        let declaration = is_declaration_name(token, &parent);
        let import = parent.parent().is_some_and(|owner| owner.kind() == USE_CLAUSE);
        (
            last_segment(token.text_range(), text),
            if declaration {
                HitKind::Declaration
            } else if import {
                HitKind::Import
            } else {
                HitKind::Reference
            },
            false,
        )
    };
    let via_alias = match &symbol {
        Symbol::Class(name) => {
            let written = &text[usize::from(range.start())..usize::from(range.end())];
            kind == HitKind::Reference && !written.eq_ignore_ascii_case(crate::short(name))
        }
        _ => false,
    };
    let access_node = if token.kind() == VARIABLE {
        parent
            .parent()
            .filter(|grand| grand.kind() == STATIC_PROPERTY_EXPR)
            .unwrap_or_else(|| parent.clone())
    } else {
        match parent.parent() {
            Some(owner) if matches!(owner.kind(), PROPERTY_FETCH_EXPR | SCOPED_ACCESS_EXPR) => owner,
            _ => parent.clone(),
        }
    };
    let access = if kind != HitKind::Import
        && matches!(symbol, Symbol::Property { .. } | Symbol::ClassConst { .. })
        && (is_write_target(&access_node)
            || kind == HitKind::Declaration && token.kind() == VARIABLE && parent.kind() == PARAMETER)
    {
        Access::Write
    } else {
        Access::Read
    };
    hits.push(Hit {
        range,
        kind,
        access,
        dollar,
        via_alias,
        symbol,
    });
}

/// The names a file imports a class under that differ from the class's own name.
fn class_aliases(ctx: &FileContext, query: &Query) -> HashSet<String> {
    let Symbol::Class(name) = &query.symbol else {
        return HashSet::new();
    };
    let end = ast::end(&ctx.root);
    let resolver = php_index::extract::resolver_at(&ctx.root, end);
    resolver
        .imports(php_index::UseKind::Class)
        .filter(|(_, target)| target.eq_ignore_ascii_case(name.trim_start_matches('\\')))
        .map(|(alias, _)| alias.to_ascii_lowercase())
        .collect()
}

fn doc_hits(ctx: &FileContext, token: &SyntaxToken, word: &str, query: &Query, hits: &mut Vec<Hit>) {
    let text = token.text();
    if !text.to_ascii_lowercase().contains(word) {
        return;
    }
    let Some(parent) = token.parent() else {
        return;
    };
    let analyzer = ctx.analyzer(&parent);
    let base = u32::from(token.text_range().start());
    let index = ctx.index;
    for item in doc_items(text, base) {
        let range = range_of(item.start, item.end);
        let (symbol, kind, dollar, range) = match &item.kind {
            DocItemKind::Class(raw) => {
                let lower = raw.to_ascii_lowercase();
                if matches!(lower.as_str(), "self" | "static" | "parent" | "this") {
                    continue;
                }
                let resolved = analyzer.resolver.resolve_class(raw);
                let local = &text[(item.start - base) as usize..(item.end - base) as usize];
                let last = local.rfind('\\').map_or(0, |at| at + 1) as u32;
                (
                    Symbol::Class(resolved),
                    HitKind::Doc,
                    false,
                    range_of(item.start + last, item.end),
                )
            }
            DocItemKind::PropertyDecl(name) => {
                let Some(class) = enclosing_class_name(&analyzer, &parent) else {
                    continue;
                };
                (
                    Symbol::Property {
                        class,
                        name: name.clone(),
                    },
                    HitKind::Declaration,
                    true,
                    range,
                )
            }
            DocItemKind::MethodDecl(name) => {
                let Some(class) = enclosing_class_name(&analyzer, &parent) else {
                    continue;
                };
                (
                    Symbol::Method {
                        class,
                        name: name.clone(),
                    },
                    HitKind::Declaration,
                    false,
                    range,
                )
            }
            DocItemKind::Member { class, name, kind, .. } => {
                let class_type = match class.to_ascii_lowercase().as_str() {
                    "self" | "static" => analyzer
                        .class
                        .as_ref()
                        .map_or(Type::Unknown, |class| Type::class(class.name.clone())),
                    _ => Type::class(analyzer.resolver.resolve_class(class)),
                };
                let symbol = match kind {
                    MemberKind::Method => index.find_method(&class_type, name).map(|found| Symbol::Method {
                        class: found.class.decl.name.clone(),
                        name: found.member.name.clone(),
                    }),
                    MemberKind::Property => index.find_property(&class_type, name).map(|found| Symbol::Property {
                        class: found.class.decl.name.clone(),
                        name: found.member.name.clone(),
                    }),
                    MemberKind::Constant => index.find_constant(&class_type, name).map(|found| Symbol::ClassConst {
                        class: found.class.decl.name.clone(),
                        name: found.member.name.clone(),
                    }),
                };
                let Some(symbol) = symbol else {
                    continue;
                };
                (symbol, HitKind::Doc, false, range)
            }
            DocItemKind::Function(name) => {
                let candidates = analyzer.resolver.function_candidates(name);
                let Some(function) = index.first_function(&candidates) else {
                    continue;
                };
                (Symbol::Function(function.decl.name.clone()), HitKind::Doc, false, range)
            }
            DocItemKind::Tag(_) | DocItemKind::Variable(_) | DocItemKind::Template(_) => continue,
        };
        if !query.matches(&symbol) {
            continue;
        }
        let via_alias = match &symbol {
            Symbol::Class(name) => {
                let written = &ctx_text_slice(token, base, range);
                !written.eq_ignore_ascii_case(crate::short(name))
            }
            _ => false,
        };
        hits.push(Hit {
            range,
            kind,
            access: Access::Read,
            dollar,
            via_alias,
            symbol,
        });
    }
}

fn ctx_text_slice(token: &SyntaxToken, base: u32, range: TextRange) -> String {
    let text = token.text();
    let from = (u32::from(range.start()) - base) as usize;
    let to = (u32::from(range.end()) - base) as usize;
    text[from..to].to_string()
}

fn enclosing_class_name(analyzer: &Analyzer<'_>, node: &SyntaxNode) -> Option<Name> {
    if node
        .ancestors()
        .any(|ancestor| ast::is_class_like(ancestor.kind()) && ancestor.kind() != ANONYMOUS_CLASS)
    {
        analyzer.class.as_ref().map(|class| class.name.clone())
    } else {
        None
    }
}
