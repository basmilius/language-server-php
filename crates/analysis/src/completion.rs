//! Completion: what can be typed at a cursor. The text around the cursor gets a placeholder name in
//! place of the word being typed, so the parser sees a whole expression and the syntax tree says
//! what kind of thing belongs there.

use std::collections::HashSet;

use php_index::{Class, ClassKind, Index, Origin, Type, UseKind};
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, SyntaxToken, parse};

use crate::ast::{has_token, text_of};
use crate::imports::{ImportPlan, import_edit, plan_import};
use crate::infer::{Analyzer, Env};
use crate::render;

mod overrides;

const PLACEHOLDER: &str = "__ph__";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    Class,
    Interface,
    Trait,
    Enum,
    EnumMember,
    Function,
    Method,
    Property,
    Constant,
    Variable,
    Keyword,
    Module,
    Parameter,
}

/// A replacement of a byte range of the text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextEdit {
    pub start: u32,
    pub end: u32,
    pub new_text: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CompletionItem {
    pub label: String,
    pub kind: ItemKind,
    /// The signature.
    pub detail: Option<String>,
    /// The namespace, or the class a member is from.
    pub description: Option<String>,
    pub edit: TextEdit,
    pub additional_edits: Vec<TextEdit>,
    pub sort_text: String,
    pub filter_text: Option<String>,
    pub deprecated: bool,
    /// What `completionItem/resolve` needs to find the documentation again.
    pub data: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct CompletionList {
    pub items: Vec<CompletionItem>,
    /// More items match than were returned; ask again as the word grows.
    pub incomplete: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct CompletionOptions {
    pub limit: usize,
}

impl Default for CompletionOptions {
    fn default() -> CompletionOptions {
        CompletionOptions { limit: 300 }
    }
}

/// Which names are wanted where the cursor is.
#[derive(Clone, Debug)]
enum Context {
    Nothing,
    Member {
        object: SyntaxNode,
    },
    Static {
        qualifier: SyntaxNode,
    },
    Variable,
    New,
    Attribute,
    Use(UseKind),
    Extends {
        interface: bool,
    },
    Implements,
    TraitUse,
    Instanceof,
    Catch,
    Type {
        is_return: bool,
    },
    ClassBody,
    /// The name of a method being declared: `function ` and a word.
    MethodName {
        declaration: SyntaxNode,
    },
    Expression {
        statement_start: bool,
    },
}

struct Word {
    start: usize,
    text: String,
}

fn word_before(text: &str, offset: usize) -> Word {
    let bytes = text.as_bytes();
    let mut start = offset;
    while start > 0 {
        let byte = bytes[start - 1];
        if byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'\\' || byte >= 0x80 {
            start -= 1;
        } else {
            break;
        }
    }
    while start < offset && !text.is_char_boundary(start) {
        start += 1;
    }
    if start > 0 && bytes[start - 1] == b'$' && !text[start..offset].contains('\\') {
        start -= 1;
    }
    Word {
        start,
        text: text[start..offset].to_string(),
    }
}

/// Whether the cursor is somewhere that takes no completion: a comment, a string or markup.
fn in_dead_zone(root: &SyntaxNode, offset: u32) -> bool {
    use php_syntax::TextSize;
    let token = match root.token_at_offset(TextSize::from(offset)) {
        php_syntax::TokenAtOffset::None => return false,
        php_syntax::TokenAtOffset::Single(token) => token,
        php_syntax::TokenAtOffset::Between(left, right) => {
            if matches!(left.kind(), COMMENT | INLINE_HTML) && u32::from(left.text_range().end()) == offset {
                left
            } else {
                right
            }
        }
    };
    let range = token.text_range();
    let inside = u32::from(range.start()) < offset && offset <= u32::from(range.end());
    match token.kind() {
        COMMENT => inside || offset == u32::from(range.end()) && !token.text().ends_with('\n'),
        BLOCK_COMMENT | DOC_COMMENT => u32::from(range.start()) < offset && offset < u32::from(range.end()),
        STRING_LITERAL | STRING_CONTENT | HEREDOC_START | HEREDOC_END | INLINE_HTML | HALT_DATA => {
            u32::from(range.start()) < offset && offset < u32::from(range.end())
        }
        _ => false,
    }
}

/// What can be typed at an offset of a text.
pub fn complete(index: &Index, text: &str, offset: u32, options: CompletionOptions) -> CompletionList {
    let offset = (offset as usize).min(text.len());
    let real = parse(text).syntax();
    if let Some(list) = crate::phpunit::complete::complete_string(index, &real, text, offset as u32, options) {
        return list;
    }
    if let Some(list) = crate::frameworks::complete_key(index, &real, text, offset as u32, options) {
        return list;
    }
    if in_dead_zone(&real, offset as u32) {
        return CompletionList::default();
    }
    let word = word_before(text, offset);
    let is_variable = word.text.starts_with('$');
    let placeholder = if is_variable {
        format!("${PLACEHOLDER}")
    } else {
        PLACEHOLDER.to_string()
    };
    let mut probe = String::with_capacity(text.len() + placeholder.len());
    probe.push_str(&text[..word.start]);
    probe.push_str(&placeholder);
    probe.push_str(&text[offset..]);
    let root = parse(&probe).syntax();
    let word_start = word.start as u32;
    let Some(token) = placeholder_token(&root, word_start, is_variable) else {
        return CompletionList::default();
    };
    let analyzer = Analyzer::new(index, &root, word_start);
    let context = classify(&token, is_variable);
    let env = analyzer.env_at(word_start);
    let mut builder = Builder {
        analyzer: &analyzer,
        index,
        text,
        real: &real,
        word,
        offset: offset as u32,
        env: &env,
        items: Vec::new(),
        options,
    };
    builder.run(&context, &token);
    let mut list = builder.finish();
    if let Some(base) = crate::frameworks::keys::class_argument_base(&analyzer, &token) {
        list.items.retain(|item| {
            matches!(item.kind, ItemKind::Class)
                && item
                    .description
                    .as_deref()
                    .map(|namespace| {
                        if namespace.is_empty() {
                            item.label.clone()
                        } else {
                            format!("{namespace}\\{}", item.label)
                        }
                    })
                    .is_some_and(|name| index.is_subclass_of(&name, &base) && !name.eq_ignore_ascii_case(&base))
        });
        for item in &mut list.items {
            item.edit.new_text.push_str("::class");
        }
    }
    if crate::phpunit::complete::in_class_argument(&analyzer, &token) {
        list.items.retain(|item| {
            matches!(
                item.kind,
                ItemKind::Class | ItemKind::Interface | ItemKind::Trait | ItemKind::Enum
            )
        });
        for item in &mut list.items {
            item.edit.new_text.push_str("::class");
        }
    }
    list
}

fn placeholder_token(root: &SyntaxNode, start: u32, is_variable: bool) -> Option<SyntaxToken> {
    let probe_offset = start + if is_variable { 2 } else { 1 };
    let token = match root.token_at_offset(php_syntax::TextSize::from(probe_offset)) {
        php_syntax::TokenAtOffset::None => return None,
        php_syntax::TokenAtOffset::Single(token) => token,
        php_syntax::TokenAtOffset::Between(left, right) => {
            if left.text().contains(PLACEHOLDER) {
                left
            } else {
                right
            }
        }
    };
    token.text().contains(PLACEHOLDER).then_some(token)
}

fn classify(token: &SyntaxToken, is_variable: bool) -> Context {
    let Some(parent) = token.parent() else {
        return Context::Nothing;
    };
    if is_variable || token.kind() == VARIABLE {
        return match parent.kind() {
            VARIABLE_EXPR => match parent.parent() {
                Some(grand)
                    if grand.kind() == STATIC_PROPERTY_EXPR && grand.children().next().as_ref() != Some(&parent) =>
                {
                    match grand.children().next() {
                        Some(qualifier) => Context::Static { qualifier },
                        None => Context::Nothing,
                    }
                }
                _ => Context::Variable,
            },
            PARAMETER | PROPERTY_ELEMENT | CLOSURE_USE_VARIABLE | STATIC_VARIABLE | CATCH_CLAUSE => Context::Nothing,
            _ => Context::Variable,
        };
    }
    let name_node = if parent.kind() == NAME {
        parent.clone()
    } else {
        token.parent().unwrap_or(parent.clone())
    };
    let owner = if parent.kind() == NAME {
        parent.parent()
    } else {
        Some(parent.clone())
    };
    let Some(owner) = owner else {
        return Context::Nothing;
    };
    let is_first = owner.children().next().as_ref() == Some(&name_node);
    match owner.kind() {
        PROPERTY_FETCH_EXPR if !is_first => match owner.children().next() {
            Some(object) => Context::Member { object },
            None => Context::Nothing,
        },
        SCOPED_ACCESS_EXPR | STATIC_PROPERTY_EXPR if !is_first => match owner.children().next() {
            Some(qualifier) => Context::Static { qualifier },
            None => Context::Nothing,
        },
        SCOPED_ACCESS_EXPR | STATIC_PROPERTY_EXPR => Context::Expression { statement_start: false },
        NEW_EXPR => Context::New,
        ATTRIBUTE => Context::Attribute,
        USE_CLAUSE | USE_GROUP => {
            let statement = owner.ancestors().find(|node| node.kind() == USE_STATEMENT);
            let kind_of = |node: &SyntaxNode| {
                if has_token(node, FUNCTION_KW) {
                    Some(UseKind::Function)
                } else if has_token(node, CONST_KW) {
                    Some(UseKind::Constant)
                } else {
                    None
                }
            };
            Context::Use(
                kind_of(&owner)
                    .or_else(|| statement.as_ref().and_then(kind_of))
                    .unwrap_or(UseKind::Class),
            )
        }
        NAMED_TYPE => classify_type(&owner),
        TRAIT_USE => Context::TraitUse,
        BINARY_EXPR => Context::Instanceof,
        CLASS_DECLARATION
        | INTERFACE_DECLARATION
        | TRAIT_DECLARATION
        | ENUM_DECLARATION
        | FUNCTION_DECLARATION
        | ENUM_CASE
        | CONST_ELEMENT
        | NAMESPACE_DECLARATION
        | CLASS_CONST_DECLARATION => Context::Nothing,
        METHOD_DECLARATION if owner.parent().is_some_and(|body| body.kind() == CLASS_BODY) => {
            Context::MethodName { declaration: owner }
        }
        METHOD_DECLARATION => Context::Nothing,
        ERROR | CLASS_BODY => in_error(&owner),
        PARAMETER_LIST | PARAMETER => Context::Type { is_return: false },
        _ => Context::Expression {
            statement_start: owner.kind() == EXPR_STATEMENT,
        },
    }
}

/// A name the parser could not place: the surrounding node says what was being started.
fn in_error(node: &SyntaxNode) -> Context {
    for ancestor in node.ancestors() {
        match ancestor.kind() {
            CLASS_BODY => return Context::ClassBody,
            PARAMETER_LIST => return Context::Type { is_return: false },
            RETURN_TYPE => return Context::Type { is_return: true },
            EXTENDS_CLAUSE | IMPLEMENTS_CLAUSE => return classify_type(&ancestor),
            _ => {}
        }
    }
    Context::Expression { statement_start: true }
}

fn classify_type(named: &SyntaxNode) -> Context {
    for ancestor in named.ancestors() {
        match ancestor.kind() {
            EXTENDS_CLAUSE => {
                let interface = ancestor
                    .parent()
                    .is_some_and(|parent| parent.kind() == INTERFACE_DECLARATION);
                return Context::Extends { interface };
            }
            IMPLEMENTS_CLAUSE => return Context::Implements,
            CATCH_CLAUSE => return Context::Catch,
            RETURN_TYPE => return Context::Type { is_return: true },
            PROPERTY_DECLARATION => {
                // A word in a class body with no variable after it is a member being started.
                let has_variable = ancestor
                    .children()
                    .filter(|child| child.kind() == PROPERTY_ELEMENT)
                    .any(|element| crate::ast::has_token(&element, VARIABLE));
                return if has_variable {
                    Context::Type { is_return: false }
                } else {
                    Context::ClassBody
                };
            }
            PARAMETER | CLASS_CONST_DECLARATION | ENUM_BACKING_TYPE => return Context::Type { is_return: false },
            FUNCTION_DECLARATION | METHOD_DECLARATION | CLOSURE_EXPR | ARROW_FUNCTION_EXPR | CLASS_BODY
            | SOURCE_FILE => break,
            _ => {}
        }
    }
    Context::Type { is_return: false }
}

struct Builder<'a> {
    analyzer: &'a Analyzer<'a>,
    index: &'a Index,
    text: &'a str,
    real: &'a SyntaxNode,
    word: Word,
    offset: u32,
    env: &'a Env,
    items: Vec<(u8, CompletionItem)>,
    options: CompletionOptions,
}

/// How well a candidate matches what was typed: lower is better, `None` is no match.
pub(crate) fn match_score(candidate: &str, typed: &str) -> Option<u8> {
    if typed.is_empty() {
        return Some(0);
    }
    if candidate.starts_with(typed) {
        return Some(0);
    }
    let candidate_lower = candidate.to_ascii_lowercase();
    let typed_lower = typed.to_ascii_lowercase();
    if candidate_lower.starts_with(&typed_lower) {
        return Some(1);
    }
    if typed.len() >= 2 {
        let mut initials = String::new();
        let mut previous_lower = false;
        for (position, c) in candidate.chars().enumerate() {
            if position == 0 || (c.is_ascii_uppercase() && previous_lower) || c == '_' {
                if c != '_' {
                    initials.push(c.to_ascii_lowercase());
                }
            } else if previous_underscore(candidate, position) {
                initials.push(c.to_ascii_lowercase());
            }
            previous_lower = c.is_ascii_lowercase() || c.is_ascii_digit();
        }
        if initials.starts_with(&typed_lower) {
            return Some(2);
        }
    }
    if typed.len() >= 3 && candidate_lower.contains(&typed_lower) {
        return Some(3);
    }
    None
}

fn previous_underscore(candidate: &str, position: usize) -> bool {
    candidate.as_bytes().get(position.wrapping_sub(1)) == Some(&b'_')
}

fn class_item_kind(kind: ClassKind) -> ItemKind {
    match kind {
        ClassKind::Class => ItemKind::Class,
        ClassKind::Interface => ItemKind::Interface,
        ClassKind::Trait => ItemKind::Trait,
        ClassKind::Enum => ItemKind::Enum,
    }
}

fn origin_rank(origin: Origin) -> u8 {
    match origin {
        Origin::Project => 0,
        Origin::Vendor => 1,
        Origin::Stub => 2,
    }
}

impl Builder<'_> {
    fn range_edit(&self, new_text: String) -> TextEdit {
        TextEdit {
            start: self.word.start as u32,
            end: self.offset,
            new_text,
        }
    }

    fn typed(&self) -> &str {
        &self.word.text
    }

    fn push(&mut self, score: u8, item: CompletionItem) {
        self.items.push((score, item));
    }

    fn finish(mut self) -> CompletionList {
        self.items
            .sort_by(|a, b| (a.0, &a.1.sort_text, &a.1.label).cmp(&(b.0, &b.1.sort_text, &b.1.label)));
        let incomplete = self.items.len() > self.options.limit;
        self.items.truncate(self.options.limit);
        CompletionList {
            items: self.items.into_iter().map(|(_, item)| item).collect(),
            incomplete,
        }
    }

    fn run(&mut self, context: &Context, token: &SyntaxToken) {
        match context {
            Context::Nothing => {}
            Context::Member { object } => {
                self.members(object);
                self.pest_members(object);
                self.custom_expectations(object);
            }
            Context::Static { qualifier } => self.static_members(qualifier),
            Context::Variable => self.variables(),
            Context::New => self.classes(ClassFilter::Instantiable, true),
            Context::Attribute => self.classes(ClassFilter::Attribute, false),
            Context::Use(kind) => self.use_names(*kind),
            Context::Extends { interface } => self.classes(
                if *interface {
                    ClassFilter::Interface
                } else {
                    ClassFilter::Extendable
                },
                false,
            ),
            Context::Implements => self.classes(ClassFilter::Interface, false),
            Context::TraitUse => self.classes(ClassFilter::Trait, false),
            Context::Catch => self.classes(ClassFilter::Throwable, false),
            Context::Instanceof => self.classes(ClassFilter::Any, false),
            Context::Type { is_return } => {
                self.classes(ClassFilter::Any, false);
                self.type_keywords(*is_return);
            }
            Context::MethodName { declaration } => self.overridable_methods(declaration),
            Context::ClassBody => {
                self.class_body_keywords();
                if !self.typed().is_empty() {
                    self.classes(ClassFilter::Any, false);
                    self.type_keywords(false);
                }
            }
            Context::Expression { statement_start } => {
                self.named_arguments(token);
                self.variables_if_dollar();
                self.keywords(*statement_start, token);
                if !self.typed().is_empty() {
                    self.classes(ClassFilter::Any, false);
                    self.functions();
                    self.constants();
                }
            }
        }
    }

    // Members -------------------------------------------------------------------------------

    fn context_class(&self) -> Option<String> {
        self.analyzer
            .class
            .as_ref()
            .filter(|class| !class.anonymous)
            .map(|class| class.name.clone())
    }

    fn members(&mut self, object: &SyntaxNode) {
        let ty = self.analyzer.type_of(object, self.env);
        let receiver = self.analyzer.receiver_type(&ty);
        let context = self.context_class();
        let typed = self.typed().to_string();
        let level = self.index.level;
        let mut seen: HashSet<String> = HashSet::new();
        // In a test the assertions are what `$this->` is for, so they come before everything else.
        let assertions_first = text_of(object) == "$this" && self.in_test();
        for (member_index, member) in receiver.members().iter().enumerate() {
            if !matches!(member, Type::Class { .. } | Type::Intersection(_)) {
                continue;
            }
            let depth = self.depths(member);
            for found in self.index.methods(member) {
                let name = &found.member.name;
                let assertion = assertions_first && name.starts_with("assert");
                if (found.member.is_static && !assertion)
                    || !self
                        .index
                        .is_accessible(found.member.visibility, &found.self_name, context.as_deref())
                    || name.starts_with("__") && !name.eq_ignore_ascii_case("__invoke")
                    || !seen.insert(format!("m:{}", name.to_ascii_lowercase()))
                {
                    continue;
                }
                let Some(score) = match_score(name, &typed) else {
                    continue;
                };
                let rank = depth_of(&depth, &found.class.decl.name);
                let item = CompletionItem {
                    label: name.clone(),
                    kind: ItemKind::Method,
                    detail: Some(render::callable_text(&found.member.callable, level)),
                    description: Some(crate::short(&found.class.decl.name).to_string()),
                    edit: self.range_edit(name.clone()),
                    additional_edits: Vec::new(),
                    sort_text: format!(
                        "{}{}{:02}{:02}{}",
                        if assertions_first && name.starts_with("assert") {
                            "!"
                        } else {
                            ""
                        },
                        u8::from(found.member.is_static && !assertion),
                        member_index,
                        rank,
                        name.to_ascii_lowercase()
                    ),
                    filter_text: Some(name.clone()),
                    deprecated: found.member.doc.as_ref().is_some_and(|doc| doc.deprecated.is_some()),
                    data: Some(format!("method:{}::{}", found.class.decl.name, name)),
                };
                self.push(score, item);
            }
            for found in self.index.properties(member) {
                let property = &found.member;
                if property.is_static
                    || !self
                        .index
                        .is_accessible(property.visibility, &found.self_name, context.as_deref())
                    || !seen.insert(format!("p:{}", property.name))
                {
                    continue;
                }
                let Some(score) = match_score(&property.name, &typed) else {
                    continue;
                };
                let rank = depth_of(&depth, &found.class.decl.name);
                let ty = property.effective_type(level).map(|ty| found.resolve(ty).display(true));
                let item = CompletionItem {
                    label: property.name.clone(),
                    kind: ItemKind::Property,
                    detail: ty,
                    description: Some(crate::short(&found.class.decl.name).to_string()),
                    edit: self.range_edit(property.name.clone()),
                    additional_edits: Vec::new(),
                    sort_text: format!("0{:02}{:02}{}", member_index, rank, property.name.to_ascii_lowercase()),
                    filter_text: Some(property.name.clone()),
                    deprecated: property.doc.as_ref().is_some_and(|doc| doc.deprecated.is_some()),
                    data: Some(format!("property:{}::{}", found.class.decl.name, property.name)),
                };
                self.push(score, item);
            }
        }
    }

    /// The expectations the project adds with `extend()`, after `expect(...)->`.
    fn custom_expectations(&mut self, object: &SyntaxNode) {
        let ty = self.analyzer.type_of(object, self.env);
        if !crate::pest::is_expectation(&ty) {
            return;
        }
        let typed = self.typed().to_string();
        for expectation in crate::pest::custom_expectations(self.index, &self.analyzer.root) {
            let Some(score) = match_score(&expectation.name, &typed) else {
                continue;
            };
            let name = expectation.name;
            let item = CompletionItem {
                label: name.clone(),
                kind: ItemKind::Method,
                detail: Some("custom expectation".to_string()),
                description: None,
                edit: self.range_edit(name.clone()),
                additional_edits: Vec::new(),
                sort_text: format!("0{:02}{:02}{}", 0, 0, name.to_ascii_lowercase()),
                filter_text: Some(name.clone()),
                deprecated: false,
                data: None,
            };
            self.push(score, item);
        }
    }

    /// Whether the cursor is in a test: a method of a test case or a Pest closure.
    fn in_test(&self) -> bool {
        self.analyzer.pest.is_some()
            || self
                .analyzer
                .class
                .as_ref()
                .is_some_and(|class| crate::phpunit::is_test_class(self.index, &class.name))
    }

    /// The properties `beforeEach` puts on `$this` in a Pest file.
    fn pest_members(&mut self, object: &SyntaxNode) {
        if text_of(object) != "$this" || self.analyzer.pest.is_none() {
            return;
        }
        let typed = self.typed().to_string();
        for name in self.analyzer.pest_property_names() {
            let Some(score) = match_score(&name, &typed) else {
                continue;
            };
            let ty = self.analyzer.pest_property(&name).map(|ty| ty.display(true));
            let item = CompletionItem {
                label: name.clone(),
                kind: ItemKind::Property,
                detail: ty,
                description: Some("beforeEach".to_string()),
                edit: self.range_edit(name.clone()),
                additional_edits: Vec::new(),
                sort_text: format!("0{:02}{:02}{}", 0, 0, name.to_ascii_lowercase()),
                filter_text: Some(name.clone()),
                deprecated: false,
                data: None,
            };
            self.push(score, item);
        }
    }

    fn depths(&self, ty: &Type) -> Vec<String> {
        self.index
            .ancestors(ty)
            .into_iter()
            .map(|ancestor| ancestor.class.decl.name.to_ascii_lowercase())
            .collect()
    }

    fn static_members(&mut self, qualifier: &SyntaxNode) {
        let ty = self.analyzer.qualifier_type(qualifier, self.env);
        let relative = qualifier.kind() == NAME
            && matches!(
                text_of(qualifier).to_ascii_lowercase().as_str(),
                "self" | "static" | "parent"
            );
        let receiver = self.analyzer.receiver_type(&ty);
        let context = self.context_class();
        let wants_properties_only = self.typed().starts_with('$');
        let typed = self.typed().trim_start_matches('$').to_string();
        let level = self.index.level;
        let mut seen: HashSet<String> = HashSet::new();
        for member in receiver.members() {
            if !matches!(member, Type::Class { .. }) {
                continue;
            }
            let depth = self.depths(member);
            let related = relative
                || context.as_ref().is_some_and(|context| {
                    member.class_names().iter().any(|name| {
                        self.index.is_subclass_of(context, name) || self.index.is_subclass_of(name, context)
                    })
                });
            if !wants_properties_only {
                for found in self.index.methods(member) {
                    let method = &found.member;
                    if (!method.is_static && !related)
                        || !self
                            .index
                            .is_accessible(method.visibility, &found.self_name, context.as_deref())
                        || !seen.insert(format!("m:{}", method.name.to_ascii_lowercase()))
                    {
                        continue;
                    }
                    let Some(score) = match_score(&method.name, &typed) else {
                        continue;
                    };
                    let item = CompletionItem {
                        label: method.name.clone(),
                        kind: ItemKind::Method,
                        detail: Some(render::callable_text(&method.callable, level)),
                        description: Some(crate::short(&found.class.decl.name).to_string()),
                        edit: self.range_edit(method.name.clone()),
                        additional_edits: Vec::new(),
                        sort_text: format!(
                            "{}{:02}{}",
                            u8::from(!method.is_static),
                            depth_of(&depth, &found.class.decl.name),
                            method.name.to_ascii_lowercase()
                        ),
                        filter_text: Some(method.name.clone()),
                        deprecated: method.doc.as_ref().is_some_and(|doc| doc.deprecated.is_some()),
                        data: Some(format!("method:{}::{}", found.class.decl.name, method.name)),
                    };
                    self.push(score, item);
                }
                for found in self.index.constants_of(member) {
                    let constant = &found.member;
                    if !self
                        .index
                        .is_accessible(constant.visibility, &found.self_name, context.as_deref())
                        || !seen.insert(format!("c:{}", constant.name))
                    {
                        continue;
                    }
                    let Some(score) = match_score(&constant.name, &typed) else {
                        continue;
                    };
                    let detail = if constant.is_case {
                        constant.value.clone().map(|value| format!("= {value}"))
                    } else {
                        constant.effective_type(level).map(|ty| found.resolve(ty).display(true))
                    };
                    let item = CompletionItem {
                        label: constant.name.clone(),
                        kind: if constant.is_case {
                            ItemKind::EnumMember
                        } else {
                            ItemKind::Constant
                        },
                        detail,
                        description: Some(crate::short(&found.class.decl.name).to_string()),
                        edit: self.range_edit(constant.name.clone()),
                        additional_edits: Vec::new(),
                        sort_text: format!(
                            "1{:02}{}",
                            depth_of(&depth, &found.class.decl.name),
                            constant.name.to_ascii_lowercase()
                        ),
                        filter_text: Some(constant.name.clone()),
                        deprecated: constant.doc.as_ref().is_some_and(|doc| doc.deprecated.is_some()),
                        data: Some(format!("const:{}::{}", found.class.decl.name, constant.name)),
                    };
                    self.push(score, item);
                }
                if let Some(score) = match_score("class", &typed) {
                    self.push(
                        score,
                        CompletionItem {
                            label: "class".to_string(),
                            kind: ItemKind::Keyword,
                            detail: Some("class-string".to_string()),
                            description: None,
                            edit: self.range_edit("class".to_string()),
                            additional_edits: Vec::new(),
                            sort_text: "9class".to_string(),
                            filter_text: None,
                            deprecated: false,
                            data: None,
                        },
                    );
                }
            }
            for found in self.index.properties(member) {
                let property = &found.member;
                if !property.is_static
                    || !self
                        .index
                        .is_accessible(property.visibility, &found.self_name, context.as_deref())
                    || !seen.insert(format!("p:{}", property.name))
                {
                    continue;
                }
                let Some(score) = match_score(&property.name, &typed) else {
                    continue;
                };
                let label = format!("${}", property.name);
                let item = CompletionItem {
                    label: label.clone(),
                    kind: ItemKind::Property,
                    detail: property.effective_type(level).map(|ty| found.resolve(ty).display(true)),
                    description: Some(crate::short(&found.class.decl.name).to_string()),
                    edit: self.range_edit(label.clone()),
                    additional_edits: Vec::new(),
                    sort_text: format!(
                        "0{:02}{}",
                        depth_of(&depth, &found.class.decl.name),
                        property.name.to_ascii_lowercase()
                    ),
                    filter_text: Some(label),
                    deprecated: property.doc.as_ref().is_some_and(|doc| doc.deprecated.is_some()),
                    data: Some(format!("property:{}::{}", found.class.decl.name, property.name)),
                };
                self.push(score, item);
            }
        }
    }

    // Variables -------------------------------------------------------------------------------

    fn variables_if_dollar(&mut self) {
        if self.typed().starts_with('$') {
            self.variables();
        }
    }

    fn variables(&mut self) {
        let typed = self.typed().trim_start_matches('$').to_string();
        let mut names: Vec<(String, Type)> = self
            .env
            .vars
            .iter()
            .map(|(name, ty)| (name.clone(), ty.clone()))
            .collect();
        if self.analyzer.class.is_some() && !names.iter().any(|(name, _)| name == "this") {
            names.push(("this".to_string(), Type::Static));
        }
        for superglobal in [
            "_GET", "_POST", "_SERVER", "_SESSION", "_COOKIE", "_FILES", "_ENV", "_REQUEST", "GLOBALS",
        ] {
            if !names.iter().any(|(name, _)| name == superglobal) {
                names.push((superglobal.to_string(), Type::plain_array()));
            }
        }
        for (name, ty) in names {
            if name == PLACEHOLDER {
                continue;
            }
            let Some(score) = match_score(&name, &typed) else {
                continue;
            };
            let ty = if name == "this" { self.analyzer.this_type() } else { ty };
            let label = format!("${name}");
            let is_superglobal = name.starts_with('_') || name == "GLOBALS";
            self.push(
                score,
                CompletionItem {
                    label: label.clone(),
                    kind: ItemKind::Variable,
                    detail: (!ty.is_unknown()).then(|| ty.display(true)),
                    description: None,
                    edit: self.range_edit(label.clone()),
                    additional_edits: Vec::new(),
                    sort_text: format!("{}{}", u8::from(is_superglobal), name.to_ascii_lowercase()),
                    filter_text: Some(label),
                    deprecated: false,
                    data: None,
                },
            );
        }
    }

    // Names -------------------------------------------------------------------------------------

    fn classes(&mut self, filter: ClassFilter, with_constructor: bool) {
        let typed = self.typed().to_string();
        if let Some(rest) = typed.strip_prefix('\\') {
            return self.qualified_classes(filter, rest, true);
        }
        if typed.contains('\\') {
            return self.qualified_classes(filter, &typed, false);
        }
        let mut candidates: Vec<(u8, Class<'_>)> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for name in self.index.class_names() {
            let short = crate::short(&name.summary.name);
            let Some(score) = match_score(short, &typed) else {
                continue;
            };
            let Some(class) = name.load() else {
                continue;
            };
            if !filter.accepts(self.index, class) {
                continue;
            }
            if !seen.insert(class.decl.name.to_ascii_lowercase()) {
                continue;
            }
            candidates.push((score, class));
        }
        for (score, class) in candidates {
            let fqn = &class.decl.name;
            let short = crate::short(fqn);
            let plan = plan_import(&self.analyzer.resolver, fqn, UseKind::Class, |name| {
                let own = self.analyzer.resolver.qualify(name);
                self.index
                    .class(&own)
                    .is_some_and(|other| !other.decl.name.eq_ignore_ascii_case(fqn))
            });
            let (insert, additional) = match plan {
                ImportPlan::Plain(name) => (name, Vec::new()),
                ImportPlan::Qualified(name) => (format!("\\{name}"), Vec::new()),
                ImportPlan::Import(name) => {
                    let edit = import_edit(self.text, self.real, self.offset, fqn, UseKind::Class);
                    (name, edit.into_iter().collect())
                }
            };
            let namespace = php_index::types::namespace_of(fqn);
            let detail = if with_constructor {
                self.index
                    .find_method(&Type::class(fqn.clone()), "__construct")
                    .map(|found| render::callable_text(&found.member.callable, self.index.level))
            } else {
                None
            };
            self.push(
                score,
                CompletionItem {
                    label: short.to_string(),
                    kind: class_item_kind(class.decl.kind),
                    detail,
                    description: (!namespace.is_empty()).then(|| namespace.to_string()),
                    edit: self.range_edit(insert),
                    additional_edits: additional,
                    sort_text: format!("{}{}", origin_rank(class.file.origin), short.to_ascii_lowercase()),
                    filter_text: Some(short.to_string()),
                    deprecated: class.decl.doc.as_ref().is_some_and(|doc| doc.deprecated.is_some()),
                    data: Some(format!("class:{fqn}")),
                },
            );
        }
    }

    /// Classes for a typed name with a backslash in it.
    fn qualified_classes(&mut self, filter: ClassFilter, typed: &str, absolute: bool) {
        let expected = if absolute {
            typed.to_string()
        } else {
            self.analyzer.resolver.resolve_class(typed)
        };
        let expected_lower = expected.to_ascii_lowercase();
        let written = if absolute {
            format!("\\{typed}")
        } else {
            typed.to_string()
        };
        let mut found: Vec<(Class<'_>, String)> = Vec::new();
        for name in self.index.class_names() {
            if !name.summary.name.to_ascii_lowercase().starts_with(&expected_lower) {
                continue;
            }
            let Some(class) = name.load() else {
                continue;
            };
            if !filter.accepts(self.index, class) {
                continue;
            }
            let insert = format!("{written}{}", &class.decl.name[expected.len()..]);
            found.push((class, insert));
        }
        for (class, insert) in found {
            let fqn = &class.decl.name;
            let namespace = php_index::types::namespace_of(fqn);
            self.push(
                0,
                CompletionItem {
                    label: crate::short(fqn).to_string(),
                    kind: class_item_kind(class.decl.kind),
                    detail: None,
                    description: (!namespace.is_empty()).then(|| namespace.to_string()),
                    edit: self.range_edit(insert.clone()),
                    additional_edits: Vec::new(),
                    sort_text: format!("{}{}", origin_rank(class.file.origin), fqn.to_ascii_lowercase()),
                    filter_text: Some(insert),
                    deprecated: false,
                    data: Some(format!("class:{fqn}")),
                },
            );
        }
    }

    fn use_names(&mut self, kind: UseKind) {
        let typed = self.typed().trim_start_matches('\\').to_string();
        let typed_lower = typed.to_ascii_lowercase();
        let mut names: Vec<(String, ItemKind, u8, String)> = Vec::new();
        match kind {
            UseKind::Class => {
                for class in self.index.class_names() {
                    names.push((
                        class.summary.name.clone(),
                        class_item_kind(class.summary.kind),
                        origin_rank(class.file.origin),
                        format!("class:{}", class.summary.name),
                    ));
                }
            }
            UseKind::Function => {
                for function in self.index.function_names() {
                    names.push((
                        function.summary.name.clone(),
                        ItemKind::Function,
                        origin_rank(function.file.origin),
                        format!("function:{}", function.summary.name),
                    ));
                }
            }
            UseKind::Constant => {
                for constant in self.index.constant_names() {
                    names.push((
                        constant.summary.name.clone(),
                        ItemKind::Constant,
                        origin_rank(constant.file.origin),
                        format!("constant:{}", constant.summary.name),
                    ));
                }
            }
        }
        let prefix_end = typed.rfind('\\').map_or(0, |position| position + 1);
        let mut segments: HashSet<String> = HashSet::new();
        let mut matched: Vec<(String, ItemKind, u8, String)> = Vec::new();
        for (name, item_kind, rank, data) in names {
            if !name.to_ascii_lowercase().starts_with(&typed_lower) {
                continue;
            }
            let rest = &name[prefix_end.min(name.len())..];
            match rest.split_once('\\') {
                Some((segment, _)) => {
                    segments.insert(segment.to_string());
                }
                None => matched.push((name, item_kind, rank, data)),
            }
        }
        for segment in segments {
            let insert = format!("{}{segment}\\", &typed[..prefix_end]);
            self.push(
                0,
                CompletionItem {
                    label: segment.clone(),
                    kind: ItemKind::Module,
                    detail: None,
                    description: None,
                    edit: self.range_edit(insert.clone()),
                    additional_edits: Vec::new(),
                    sort_text: format!("0{}", segment.to_ascii_lowercase()),
                    filter_text: Some(insert),
                    deprecated: false,
                    data: None,
                },
            );
        }
        for (name, item_kind, rank, data) in matched {
            let short = crate::short(&name).to_string();
            self.push(
                0,
                CompletionItem {
                    label: short,
                    kind: item_kind,
                    detail: None,
                    description: Some(php_index::types::namespace_of(&name).to_string()),
                    edit: self.range_edit(name.clone()),
                    additional_edits: Vec::new(),
                    sort_text: format!("1{rank}{}", name.to_ascii_lowercase()),
                    filter_text: Some(name),
                    deprecated: false,
                    data: Some(data),
                },
            );
        }
    }

    fn functions(&mut self) {
        let typed = self.typed().to_string();
        if typed.contains('\\') {
            return;
        }
        let level = self.index.level;
        let mut seen: HashSet<String> = HashSet::new();
        let mut found = Vec::new();
        for name in self.index.function_names() {
            let short = crate::short(&name.summary.name);
            let Some(score) = match_score(short, &typed) else {
                continue;
            };
            if !seen.insert(name.summary.name.to_ascii_lowercase()) {
                continue;
            }
            if let Some(function) = name.load_function() {
                found.push((score, function));
            }
        }
        for (score, function) in found {
            let fqn = &function.decl.name;
            let short = crate::short(fqn);
            let plan = plan_import(&self.analyzer.resolver, fqn, UseKind::Function, |_| false);
            let (insert, additional) = match plan {
                ImportPlan::Plain(name) => (name, Vec::new()),
                ImportPlan::Qualified(name) => (format!("\\{name}"), Vec::new()),
                ImportPlan::Import(name) => {
                    let edit = import_edit(self.text, self.real, self.offset, fqn, UseKind::Function);
                    (name, edit.into_iter().collect())
                }
            };
            let namespace = php_index::types::namespace_of(fqn);
            self.push(
                score,
                CompletionItem {
                    label: short.to_string(),
                    kind: ItemKind::Function,
                    detail: Some(render::callable_text(&function.decl.callable, level)),
                    description: (!namespace.is_empty()).then(|| namespace.to_string()),
                    edit: self.range_edit(insert),
                    additional_edits: additional,
                    sort_text: format!("2{}{}", origin_rank(function.file.origin), short.to_ascii_lowercase()),
                    filter_text: Some(short.to_string()),
                    deprecated: function.decl.doc.as_ref().is_some_and(|doc| doc.deprecated.is_some()),
                    data: Some(format!("function:{fqn}")),
                },
            );
        }
    }

    fn constants(&mut self) {
        let typed = self.typed().to_string();
        if typed.contains('\\') || typed.len() < 2 {
            return;
        }
        let mut seen: HashSet<String> = HashSet::new();
        let mut found = Vec::new();
        for name in self.index.constant_names() {
            let short = crate::short(&name.summary.name);
            let Some(score) = match_score(short, &typed) else {
                continue;
            };
            if !seen.insert(name.summary.name.clone()) {
                continue;
            }
            if let Some(constant) = name.load_constant() {
                found.push((score, constant));
            }
        }
        for (score, constant) in found {
            let fqn = &constant.decl.name;
            let short = crate::short(fqn);
            let plan = plan_import(&self.analyzer.resolver, fqn, UseKind::Constant, |_| false);
            let (insert, additional) = match plan {
                ImportPlan::Plain(name) => (name, Vec::new()),
                ImportPlan::Qualified(name) => (format!("\\{name}"), Vec::new()),
                ImportPlan::Import(name) => {
                    let edit = import_edit(self.text, self.real, self.offset, fqn, UseKind::Constant);
                    (name, edit.into_iter().collect())
                }
            };
            let namespace = php_index::types::namespace_of(fqn);
            self.push(
                score,
                CompletionItem {
                    label: short.to_string(),
                    kind: ItemKind::Constant,
                    detail: constant.decl.ty.as_ref().map(|ty| ty.display(true)),
                    description: (!namespace.is_empty()).then(|| namespace.to_string()),
                    edit: self.range_edit(insert),
                    additional_edits: additional,
                    sort_text: format!("3{}{}", origin_rank(constant.file.origin), short.to_ascii_lowercase()),
                    filter_text: Some(short.to_string()),
                    deprecated: false,
                    data: Some(format!("constant:{fqn}")),
                },
            );
        }
    }

    // Named arguments ----------------------------------------------------------------------------

    fn named_arguments(&mut self, token: &SyntaxToken) {
        if self.typed().starts_with('$') {
            return;
        }
        let Some(argument) = token
            .parent()
            .into_iter()
            .flat_map(|node| node.ancestors())
            .find(|node| node.kind() == ARGUMENT)
        else {
            return;
        };
        if crate::ast::has_token(&argument, COLON) {
            return;
        }
        let Some(list) = argument.parent().filter(|parent| parent.kind() == ARGUMENT_LIST) else {
            return;
        };
        let Some(call) = list.parent() else {
            return;
        };
        let callees = self.analyzer.callees(&call, self.env);
        let Some(callee) = callees.first() else {
            return;
        };
        let arguments: Vec<_> = list.children().filter(|node| node.kind() == ARGUMENT).collect();
        let position = arguments.iter().position(|node| node == &argument).unwrap_or(0);
        let named: Vec<String> = arguments
            .iter()
            .filter(|node| has_token(node, COLON))
            .filter_map(|node| {
                crate::ast::tokens(node)
                    .find(|token| !token.kind().is_trivia())
                    .map(|token| token.text().to_string())
            })
            .collect();
        let level = self.index.level;
        let typed = self.typed().to_string();
        for (index, param) in callee.callable.params_at(level).enumerate() {
            if index < position && !param.variadic || named.contains(&param.name) || param.variadic {
                continue;
            }
            let Some(score) = match_score(&param.name, &typed) else {
                continue;
            };
            self.push(
                score,
                CompletionItem {
                    label: format!("{}:", param.name),
                    kind: ItemKind::Parameter,
                    detail: param.effective_type(level).map(|ty| ty.display(true)),
                    description: None,
                    edit: self.range_edit(format!("{}: ", param.name)),
                    additional_edits: Vec::new(),
                    sort_text: format!("0{index:02}"),
                    filter_text: Some(param.name.clone()),
                    deprecated: false,
                    data: None,
                },
            );
        }
    }

    // Keywords -----------------------------------------------------------------------------------

    fn keyword_item(&mut self, keyword: &str, group: u8) {
        let Some(score) = match_score(keyword, self.typed()) else {
            return;
        };
        let item = CompletionItem {
            label: keyword.to_string(),
            kind: ItemKind::Keyword,
            detail: None,
            description: None,
            edit: self.range_edit(keyword.to_string()),
            additional_edits: Vec::new(),
            sort_text: format!("{group}{keyword}"),
            filter_text: None,
            deprecated: false,
            data: None,
        };
        self.push(score, item);
    }

    fn keywords(&mut self, statement_start: bool, token: &SyntaxToken) {
        let ancestors: Vec<SyntaxNode> = token.parent().into_iter().flat_map(|node| node.ancestors()).collect();
        let in_function = ancestors.iter().any(|node| crate::ast::is_function_like(node.kind()));
        let in_class = self.analyzer.class.is_some();
        let mut words: Vec<&str> = vec![
            "new",
            "clone",
            "fn",
            "function",
            "static",
            "match",
            "isset",
            "empty",
            "array",
            "list",
            "print",
            "true",
            "false",
            "null",
            "include",
            "include_once",
            "require",
            "require_once",
            "yield",
            "throw",
            "exit",
        ];
        if in_class {
            words.extend(["self", "parent"]);
        }
        if statement_start {
            words.extend([
                "if", "else", "elseif", "foreach", "for", "while", "do", "switch", "return", "echo", "try", "global",
                "unset", "break", "continue",
            ]);
            if !in_function {
                words.extend([
                    "namespace",
                    "use",
                    "class",
                    "interface",
                    "trait",
                    "enum",
                    "abstract",
                    "final",
                    "readonly",
                    "const",
                    "declare",
                ]);
            }
        }
        for word in words {
            self.keyword_item(word, 4);
        }
    }

    fn type_keywords(&mut self, is_return: bool) {
        let mut words = vec![
            "int", "string", "bool", "float", "array", "callable", "iterable", "object", "mixed", "null", "false",
            "true", "self", "parent",
        ];
        if is_return {
            words.extend(["void", "never", "static"]);
        }
        for word in words {
            self.keyword_item(word, 4);
        }
    }

    fn class_body_keywords(&mut self) {
        let is_enum = self
            .analyzer
            .class
            .as_ref()
            .is_some_and(|class| class.kind == ClassKind::Enum);
        let mut words = vec![
            "public",
            "protected",
            "private",
            "static",
            "abstract",
            "final",
            "readonly",
            "function",
            "const",
            "use",
            "var",
        ];
        if is_enum {
            words.push("case");
        }
        for word in words {
            self.keyword_item(word, 0);
        }
    }
}

fn depth_of(depths: &[String], class_name: &str) -> usize {
    depths
        .iter()
        .position(|name| name.eq_ignore_ascii_case(class_name))
        .unwrap_or(depths.len())
        .min(99)
}

/// Which classes a position accepts.
#[derive(Clone, Copy, Debug)]
enum ClassFilter {
    Any,
    Instantiable,
    Attribute,
    Interface,
    Extendable,
    Trait,
    Throwable,
}

impl ClassFilter {
    fn accepts(self, index: &Index, class: Class<'_>) -> bool {
        let decl = class.decl;
        match self {
            ClassFilter::Any => true,
            ClassFilter::Instantiable => decl.is_instantiable(),
            ClassFilter::Attribute => {
                decl.kind == ClassKind::Class
                    && decl
                        .attributes
                        .iter()
                        .any(|attribute| attribute.name.eq_ignore_ascii_case("Attribute"))
            }
            ClassFilter::Interface => decl.kind == ClassKind::Interface,
            ClassFilter::Extendable => decl.kind == ClassKind::Class && !decl.is_final,
            ClassFilter::Trait => decl.kind == ClassKind::Trait,
            ClassFilter::Throwable => decl.kind == ClassKind::Class && index.is_subclass_of(&decl.name, "Throwable"),
        }
    }
}

/// The documentation for an item, from the `data` its completion carried.
pub fn resolve_documentation(index: &Index, data: &str) -> Option<String> {
    use crate::target::Target;
    let (kind, rest) = data.split_once(':')?;
    let tree = parse("<?php").syntax();
    let analyzer = Analyzer::new(index, &tree, 0);
    let target = match kind {
        "class" => Target::Class(rest.to_string()),
        "function" => Target::Function(rest.to_string()),
        "constant" => Target::Constant(rest.to_string()),
        "method" | "property" | "const" => {
            let (class, member) = rest.split_once("::")?;
            let receiver = Type::class(class);
            match kind {
                "method" => Target::Method {
                    receiver,
                    name: member.to_string(),
                },
                "property" => Target::Property {
                    receiver,
                    name: member.to_string(),
                },
                _ => Target::ClassConst {
                    receiver,
                    name: member.to_string(),
                },
            }
        }
        _ => return None,
    };
    let description = analyzer.describe(&target).into_iter().next()?;
    Some(crate::nav::hover_markdown(&description))
}
