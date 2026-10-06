//! Reads the declarations out of a syntax tree: classes, interfaces, traits and enums with their
//! members, functions and constants, each with its types and PHPDoc.

use php_syntax::SyntaxKind::*;
use php_syntax::{PhpVersion, SyntaxElement, SyntaxKind, SyntaxNode, SyntaxToken, TextRange};

use crate::model::*;
use crate::phpdoc::{TypeContext, parse_doc, parse_type};
use crate::resolve::{NameResolver, UseKind};
use crate::types::{Name, Type};

#[derive(Clone, Copy, Debug, Default)]
pub struct ExtractOptions {
    /// The file is a stub of the standard library: `@since`, `@removed` and the availability
    /// attributes mean PHP versions.
    pub stub: bool,
}

/// Collects every declaration in a file.
pub fn extract(root: &SyntaxNode, options: ExtractOptions) -> FileSymbols {
    let mut extractor = Extractor {
        options,
        resolver: NameResolver::new(""),
        out: FileSymbols::default(),
    };
    extractor.statements(root);
    extractor.out.dynamic_define = extractor.has_dynamic_define(root);
    if options.stub {
        crate::stub_overlay::apply(&mut extractor.out);
    }
    if extractor.out.tests.as_ref().is_some_and(|facts| facts.is_empty()) {
        extractor.out.tests = None;
    }
    extractor.out
}

struct Extractor {
    options: ExtractOptions,
    resolver: NameResolver,
    out: FileSymbols,
}

/// The class a declaration is inside, as far as reading its types is concerned.
#[derive(Clone, Debug, Default)]
pub struct ClassScope {
    pub name: Name,
    pub parent: Option<Name>,
    pub is_trait: bool,
    pub templates: Vec<String>,
    pub aliases: Vec<(String, Type)>,
}

fn span(range: TextRange) -> Span {
    Span {
        start: u32::from(range.start()),
        end: u32::from(range.end()),
    }
}

fn name_child(node: &SyntaxNode) -> Option<SyntaxNode> {
    node.children().find(|child| child.kind() == NAME)
}

fn tokens(node: &SyntaxNode) -> impl Iterator<Item = SyntaxToken> {
    node.children_with_tokens().filter_map(SyntaxElement::into_token)
}

fn has_token(node: &SyntaxNode, kind: SyntaxKind) -> bool {
    tokens(node).any(|token| token.kind() == kind)
}

fn is_type_node(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        NAMED_TYPE | NULLABLE_TYPE | UNION_TYPE | INTERSECTION_TYPE | PAREN_TYPE
    )
}

/// The doc comment right before a declaration, when the declaration owns it.
fn leading_doc(node: &SyntaxNode) -> Option<SyntaxToken> {
    let mut found = None;
    for element in node.children_with_tokens() {
        match element {
            SyntaxElement::Token(token) if token.kind() == DOC_COMMENT => found = Some(token),
            SyntaxElement::Token(token) if token.kind().is_trivia() => {}
            _ => break,
        }
    }
    found
}

/// A doc comment before a modifier or a type, which a node that starts with attributes keeps inside.
fn doc_of(node: &SyntaxNode) -> Option<SyntaxToken> {
    leading_doc(node)
}

fn collapse(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The stubs spell a method named like a keyword with this prefix, for tools that cannot read the keyword there.
fn unreserved(name: &str) -> String {
    name.strip_prefix("PS_UNRESERVE_PREFIX_").unwrap_or(name).to_string()
}

fn unquote(text: &str) -> String {
    let text = text.trim();
    for quote in ['\'', '"'] {
        if text.len() >= 2 && text.starts_with(quote) && text.ends_with(quote) {
            return text[1..text.len() - 1].to_string();
        }
    }
    text.to_string()
}

/// The text after the first `=` of a node: a default value or the value of a constant.
fn value_text(node: &SyntaxNode) -> Option<String> {
    let mut seen = false;
    let mut out = String::new();
    for element in node.children_with_tokens() {
        if seen {
            match &element {
                SyntaxElement::Token(token) if token.kind() == SEMICOLON => break,
                _ => out.push_str(&element.to_string()),
            }
        } else if matches!(&element, SyntaxElement::Token(token) if token.kind() == ASSIGN) {
            seen = true;
        }
    }
    let out = collapse(&out);
    (!out.is_empty()).then_some(out)
}

/// The type of a literal value, for constants that have no declared type.
fn literal_type(node: &SyntaxNode) -> Option<Type> {
    match node.kind() {
        LITERAL => {
            let token = tokens(node).find(|token| !token.kind().is_trivia())?;
            match token.kind() {
                INT_LITERAL => Some(Type::Int),
                FLOAT_LITERAL => Some(Type::Float),
                STRING_LITERAL | HEREDOC_START | DOUBLE_QUOTE => Some(Type::String),
                _ => None,
            }
        }
        ARRAY_EXPR => Some(Type::plain_array()),
        NAME => match node.text().to_string().to_ascii_lowercase().as_str() {
            "true" => Some(Type::True),
            "false" => Some(Type::False),
            "null" => Some(Type::Null),
            _ => None,
        },
        INTERPOLATED_STRING | HEREDOC => Some(Type::String),
        PREFIX_EXPR => node.children().find_map(|child| literal_type(&child)),
        _ => None,
    }
}

fn value_node(node: &SyntaxNode) -> Option<SyntaxNode> {
    let mut seen = false;
    for element in node.children_with_tokens() {
        match element {
            SyntaxElement::Token(token) if token.kind() == ASSIGN => seen = true,
            SyntaxElement::Node(child) if seen => return Some(child),
            _ => {}
        }
    }
    None
}

impl Extractor {
    fn statements(&mut self, container: &SyntaxNode) {
        for child in container.children() {
            match child.kind() {
                NAMESPACE_DECLARATION => {
                    let saved = std::mem::take(&mut self.resolver);
                    let namespace = name_child(&child)
                        .map(|name| name.text().to_string())
                        .unwrap_or_default();
                    self.resolver = NameResolver::new(&namespace);
                    self.statements(&child);
                    self.resolver = saved;
                }
                USE_STATEMENT => self.use_statement(&child),
                FUNCTION_DECLARATION => self.function(&child),
                CLASS_DECLARATION | INTERFACE_DECLARATION | TRAIT_DECLARATION | ENUM_DECLARATION => {
                    self.class_like(&child);
                }
                CONST_STATEMENT => self.constants(&child),
                EXPR_STATEMENT => {
                    self.define(&child);
                    if !self.options.stub {
                        let mut found = crate::test_facts::TestFacts::default();
                        crate::test_facts::read_statement(&child, &self.resolver, &mut found);
                        if !found.is_empty() {
                            let tests = self.tests();
                            tests.datasets.extend(found.datasets);
                            tests.bindings.extend(found.bindings);
                            tests.expectations.extend(found.expectations);
                        }
                    }
                }
                IF_STATEMENT | ELSEIF_CLAUSE | ELSE_CLAUSE | BLOCK | STATEMENT_LIST | TRY_STATEMENT | CATCH_CLAUSE
                | FINALLY_CLAUSE => self.statements(&child),
                _ => {}
            }
        }
    }

    fn tests(&mut self) -> &mut crate::test_facts::TestFacts {
        self.out.tests.get_or_insert_with(Default::default)
    }

    fn use_statement(&mut self, node: &SyntaxNode) {
        let statement_kind = if has_token(node, FUNCTION_KW) {
            UseKind::Function
        } else if has_token(node, CONST_KW) {
            UseKind::Constant
        } else {
            UseKind::Class
        };
        for child in node.children() {
            match child.kind() {
                USE_CLAUSE => self.use_clause(&child, None, statement_kind),
                USE_GROUP => {
                    let prefix = name_child(&child).map(|name| name.text().to_string());
                    for clause in child.children().filter(|clause| clause.kind() == USE_CLAUSE) {
                        self.use_clause(&clause, prefix.as_deref(), statement_kind);
                    }
                }
                _ => {}
            }
        }
    }

    fn use_clause(&mut self, clause: &SyntaxNode, prefix: Option<&str>, statement_kind: UseKind) {
        let kind = if has_token(clause, FUNCTION_KW) {
            UseKind::Function
        } else if has_token(clause, CONST_KW) {
            UseKind::Constant
        } else {
            statement_kind
        };
        let mut names = clause.children().filter(|child| child.kind() == NAME);
        let Some(target) = names.next() else {
            return;
        };
        let alias = names.next().map(|alias| alias.text().to_string());
        let target = target.text().to_string();
        let full = match prefix {
            Some(prefix) => format!("{}\\{}", prefix.trim_end_matches('\\'), target),
            None => target,
        };
        self.resolver.add_use(kind, &full, alias.as_deref());
    }

    fn type_context<'a>(&'a self, scope: Option<&'a ClassScope>) -> TypeContext<'a> {
        let mut cx = TypeContext::new(&self.resolver);
        if let Some(scope) = scope {
            cx.class_name = Some(&scope.name);
            cx.parent_name = scope.parent.as_deref();
            cx.in_trait = scope.is_trait;
            cx.templates = scope.templates.clone();
            cx.aliases = scope.aliases.clone();
        }
        cx
    }

    fn parse_doc(&self, node: &SyntaxNode, scope: Option<&ClassScope>) -> Option<Box<Doc>> {
        self.parse_doc_with(node, scope, &[])
    }

    fn parse_doc_with(&self, node: &SyntaxNode, scope: Option<&ClassScope>, extra: &[String]) -> Option<Box<Doc>> {
        let token = doc_of(node)?;
        let mut cx = self.type_context(scope);
        cx.templates.extend(extra.iter().cloned());
        let doc = parse_doc(token.text(), &cx, self.options.stub);
        (!doc.is_empty()).then(|| Box::new(doc))
    }

    /// The doc of a function or method, read again with its own `@template` names in scope.
    fn doc_with_templates(&self, node: &SyntaxNode, scope: Option<&ClassScope>) -> Option<Box<Doc>> {
        let first = self.parse_doc(node, scope)?;
        if first.templates.is_empty() {
            return Some(first);
        }
        let names: Vec<String> = first.templates.iter().map(|template| template.name.clone()).collect();
        self.parse_doc_with(node, scope, &names)
    }

    fn native_type(&self, node: &SyntaxNode, scope: Option<&ClassScope>) -> Type {
        let cx = self.type_context(scope);
        native_type(node, &cx)
    }

    fn attributes(&self, node: &SyntaxNode) -> Vec<Attribute> {
        let mut out = Vec::new();
        for list in node.children().filter(|child| child.kind() == ATTRIBUTE_LIST) {
            for attribute in list.children().filter(|child| child.kind() == ATTRIBUTE) {
                let Some(name) = name_child(&attribute) else {
                    continue;
                };
                let args = attribute
                    .children()
                    .find(|child| child.kind() == ARGUMENT_LIST)
                    .map(|list| {
                        list.children()
                            .filter(|child| child.kind() == ARGUMENT)
                            .map(|argument| argument_of(&argument))
                            .collect()
                    })
                    .unwrap_or_default();
                out.push(Attribute {
                    name: self.resolver.resolve_class(&name.text().to_string()),
                    args,
                });
            }
        }
        out
    }

    /// The availability a stub declares through its doc comment and attributes.
    fn availability(&self, doc: Option<&Doc>, attributes: &[Attribute]) -> Availability {
        let mut availability = Availability::default();
        if !self.options.stub {
            return availability;
        }
        if let Some(doc) = doc {
            availability.from = doc.since;
            availability.until = doc.removed;
        }
        for attribute in attributes {
            if !attribute.name.ends_with("PhpStormStubsElementAvailable") {
                continue;
            }
            for (index, arg) in attribute.args.iter().enumerate() {
                let version = unquote(&arg.value);
                let Some(parsed) = PhpVersion::parse(&version) else {
                    continue;
                };
                match arg.name.as_deref() {
                    Some("from") => availability.from = Some(encode_version(parsed)),
                    Some("to") => availability.until = Some(encode_version(parsed) + 1),
                    None if index == 0 => availability.from = Some(encode_version(parsed)),
                    None if index == 1 => availability.until = Some(encode_version(parsed) + 1),
                    _ => {}
                }
            }
        }
        availability
    }

    /// `#[LanguageLevelTypeAware(['8.1' => 'string'], default: 'int')]`.
    fn leveled(&self, attributes: &[Attribute], scope: Option<&ClassScope>) -> Option<LeveledType> {
        if !self.options.stub {
            return None;
        }
        let attribute = attributes
            .iter()
            .find(|attribute| attribute.name.ends_with("LanguageLevelTypeAware"))?;
        let cx = self.type_context(scope);
        let mut leveled = LeveledType::default();
        for (index, arg) in attribute.args.iter().enumerate() {
            match arg.name.as_deref() {
                Some("default") => leveled.default = parse_type(&unquote(&arg.value), &cx),
                None if index == 0 => leveled.levels = parse_level_map(&arg.value, &cx),
                Some("versions") => leveled.levels = parse_level_map(&arg.value, &cx),
                _ => {}
            }
        }
        leveled.levels.sort_by_key(|entry| std::cmp::Reverse(entry.0));
        Some(leveled)
    }

    fn constants(&mut self, node: &SyntaxNode) {
        let doc = self.parse_doc(node, None);
        for element in node.children().filter(|child| child.kind() == CONST_ELEMENT) {
            let Some(name) = name_child(&element) else {
                continue;
            };
            let value = value_node(&element);
            self.out.constants.push(ConstDecl {
                name: self.resolver.qualify(&name.text().to_string()),
                value: value_text(&element),
                ty: value.as_ref().and_then(literal_type),
                doc: doc.clone(),
                availability: self.availability(doc.as_deref(), &[]),
                name_span: span(name.text_range()),
                span: span(element.text_range()),
            });
        }
    }

    /// `define('NAME', value);` at the top of a file or inside an `if`.
    fn define(&mut self, statement: &SyntaxNode) {
        let Some(call) = statement.children().find(|child| child.kind() == CALL_EXPR) else {
            return;
        };
        let Some(callee) = call.children().next() else {
            return;
        };
        if callee.kind() != NAME
            || !callee
                .text()
                .to_string()
                .trim_start_matches('\\')
                .eq_ignore_ascii_case("define")
        {
            return;
        }
        let Some(arguments) = call.children().find(|child| child.kind() == ARGUMENT_LIST) else {
            return;
        };
        let mut arguments = arguments.children().filter(|child| child.kind() == ARGUMENT);
        let (Some(first), Some(second)) = (arguments.next(), arguments.next()) else {
            return;
        };
        let Some(literal) = first.children().next() else {
            return;
        };
        let Some(name) = self.defined_name(&literal) else {
            return;
        };
        if name.is_empty() {
            return;
        }
        let value = second.children().next();
        let doc = self.parse_doc(statement, None);
        self.out.constants.push(ConstDecl {
            name: name.trim_start_matches('\\').to_string(),
            value: value.as_ref().map(|value| collapse(&value.text().to_string())),
            ty: value.as_ref().and_then(literal_type),
            availability: self.availability(doc.as_deref(), &[]),
            doc,
            name_span: span(literal.text_range()),
            span: span(statement.text_range()),
        });
    }

    /// Whether the file calls `define` in a way the constants above do not account for.
    fn has_dynamic_define(&self, root: &SyntaxNode) -> bool {
        let recorded: Vec<Span> = self.out.constants.iter().map(|constant| constant.name_span).collect();
        root.descendants().filter(|node| node.kind() == CALL_EXPR).any(|call| {
            let Some(callee) = call.children().next() else {
                return false;
            };
            let is_define = callee.kind() == NAME
                && callee
                    .text()
                    .to_string()
                    .trim_start_matches('\\')
                    .eq_ignore_ascii_case("define");
            if !is_define {
                return false;
            }
            let first = call
                .children()
                .find(|child| child.kind() == ARGUMENT_LIST)
                .and_then(|list| list.children().find(|child| child.kind() == ARGUMENT))
                .and_then(|argument| argument.children().next());
            match first {
                Some(first) => !recorded.contains(&span(first.text_range())),
                None => false,
            }
        })
    }

    /// The name `define` is given: a string, or `__NAMESPACE__ . '\NAME'`, which is how a file
    /// defines a constant of its namespace.
    fn defined_name(&self, expression: &SyntaxNode) -> Option<String> {
        match expression.kind() {
            LITERAL if expression.text().to_string().starts_with(['\'', '"']) => {
                Some(unquote(&expression.text().to_string()))
            }
            BINARY_EXPR => {
                let mut parts = expression.children();
                let (first, second) = (parts.next()?, parts.next()?);
                if parts.next().is_some()
                    || first.kind() != LITERAL
                    || !first.text().to_string().eq_ignore_ascii_case("__NAMESPACE__")
                    || second.kind() != LITERAL
                {
                    return None;
                }
                let tail = unquote(&second.text().to_string());
                Some(format!("{}{tail}", self.resolver.namespace))
            }
            _ => None,
        }
    }

    fn function(&mut self, node: &SyntaxNode) {
        let Some(name) = name_child(node) else {
            return;
        };
        let doc = self.doc_with_templates(node, None);
        let attributes = self.attributes(node);
        let callable = self.callable(node, doc.as_deref(), &attributes, None);
        let availability = self.availability(doc.as_deref(), &attributes);
        let doc = with_attribute_deprecation(doc, &attributes);
        self.out.functions.push(Function {
            name: self.resolver.qualify(&name.text().to_string()),
            callable,
            doc,
            attributes,
            availability,
            name_span: span(name.text_range()),
            span: span(node.text_range()),
        });
    }

    fn callable(
        &self,
        node: &SyntaxNode,
        doc: Option<&Doc>,
        attributes: &[Attribute],
        scope: Option<&ClassScope>,
    ) -> Callable {
        let params = node
            .children()
            .find(|child| child.kind() == PARAMETER_LIST)
            .map(|list| {
                list.children()
                    .filter(|child| child.kind() == PARAMETER)
                    .filter_map(|parameter| self.param(&parameter, doc, scope))
                    .collect()
            })
            .unwrap_or_default();
        let ret = node
            .children()
            .find(|child| child.kind() == RETURN_TYPE)
            .and_then(|ret| ret.children().find(|child| is_type_node(child.kind())))
            .map(|ty| self.native_type(&ty, scope));
        let doc_ret = doc.and_then(|doc| doc.ret.as_ref().map(|(ty, _)| ty.clone()));
        let is_generator = ret.is_none() && doc_ret.is_none() && contains_yield(node);
        Callable {
            params,
            ret,
            doc_ret,
            leveled_ret: self.leveled(attributes, scope),
            by_ref_return: has_token(node, AMP),
            is_generator,
            reads_all_arguments: reads_all_arguments(node),
        }
    }

    fn param(&self, node: &SyntaxNode, doc: Option<&Doc>, scope: Option<&ClassScope>) -> Option<Param> {
        let variable = tokens(node).find(|token| token.kind() == VARIABLE)?;
        let name = variable.text().trim_start_matches('$').to_string();
        let attributes = self.attributes(node);
        let ty = node
            .children()
            .find(|child| is_type_node(child.kind()))
            .map(|ty| self.native_type(&ty, scope));
        let doc_param = doc.and_then(|doc| doc.params.iter().find(|param| param.name == name));
        let promoted = node.children().find(|child| child.kind() == MODIFIER_LIST).map(|list| {
            let (visibility, _) = visibility_of(&list);
            Promotion {
                visibility: visibility.unwrap_or(Visibility::Public),
                readonly: has_token(&list, READONLY_KW),
            }
        });
        Some(Param {
            default: value_text(node),
            variadic: has_token(node, ELLIPSIS),
            by_ref: has_token(node, AMP),
            promoted,
            description: doc_param.map(|param| param.description.clone()).unwrap_or_default(),
            doc_ty: doc_param.and_then(|param| param.ty.clone()),
            leveled: self.leveled(&attributes, scope),
            availability: self.availability(None, &attributes),
            attributes,
            ty,
            name,
            span: span(node.text_range()),
        })
    }

    fn class_like(&mut self, node: &SyntaxNode) {
        let Some(name_node) = name_child(node) else {
            return;
        };
        let kind = match node.kind() {
            INTERFACE_DECLARATION => ClassKind::Interface,
            TRAIT_DECLARATION => ClassKind::Trait,
            ENUM_DECLARATION => ClassKind::Enum,
            _ => ClassKind::Class,
        };
        let name = self.resolver.qualify(&name_node.text().to_string());
        if !self.options.stub {
            let mut found = crate::test_facts::TestFacts::default();
            crate::test_facts::read_groups(node, &self.resolver, &mut found);
            if !found.groups.is_empty() {
                let tests = self.tests();
                for group in found.groups {
                    if !tests.groups.contains(&group) {
                        tests.groups.push(group);
                    }
                }
            }
        }
        let modifiers = node.children().find(|child| child.kind() == MODIFIER_LIST);
        let has_modifier = |kind: SyntaxKind| modifiers.as_ref().is_some_and(|list| has_token(list, kind));

        let extends_names = clause_names(node, EXTENDS_CLAUSE, &self.resolver);
        let implements_names = clause_names(node, IMPLEMENTS_CLAUSE, &self.resolver);
        let parent = (kind == ClassKind::Class)
            .then(|| extends_names.first().cloned())
            .flatten();

        let mut scope = ClassScope {
            name: name.clone(),
            parent,
            is_trait: kind == ClassKind::Trait,
            templates: Vec::new(),
            aliases: Vec::new(),
        };
        // The class doc names the templates and type aliases the members' docs may use, so it is
        // read twice: once for the names, and again with them in scope. An imported alias is a
        // template the hierarchy binds to the type of the class it comes from.
        let first_pass = self.parse_doc(node, Some(&scope));
        if let Some(doc) = &first_pass {
            scope.templates = doc.templates.iter().map(|template| template.name.clone()).collect();
            scope
                .templates
                .extend(doc.type_imports.iter().map(|import| import.alias.clone()));
            scope.aliases = doc
                .type_aliases
                .iter()
                .map(|alias| (alias.name.clone(), alias.ty.clone()))
                .collect();
        }
        let mut doc = if scope.templates.is_empty() && scope.aliases.is_empty() {
            first_pass
        } else {
            self.parse_doc(node, Some(&scope))
        };
        if let (Some(doc), Some(token)) = (doc.as_mut(), doc_of(node)) {
            let base = u32::from(token.text_range().start());
            for alias in &mut doc.type_aliases {
                alias.name_span.start += base;
                alias.name_span.end += base;
            }
        }
        let attributes = self.attributes(node);
        let availability = self.availability(doc.as_deref(), &attributes);

        let extends = merge_generic_names(extends_names, doc.as_deref().map(|doc| doc.extends.as_slice()));
        let implements = merge_generic_names(implements_names, doc.as_deref().map(|doc| doc.implements.as_slice()));
        let mut decl = ClassDecl {
            name,
            kind,
            is_abstract: has_modifier(ABSTRACT_KW),
            is_final: has_modifier(FINAL_KW),
            is_readonly: has_modifier(READONLY_KW),
            extends,
            implements,
            trait_uses: Vec::new(),
            backing: node
                .children()
                .find(|child| child.kind() == ENUM_BACKING_TYPE)
                .and_then(|backing| backing.children().find(|child| is_type_node(child.kind())))
                .map(|ty| self.native_type(&ty, Some(&scope))),
            methods: Vec::new(),
            properties: Vec::new(),
            constants: Vec::new(),
            doc: with_attribute_deprecation(doc, &attributes),
            attributes,
            availability,
            name_span: span(name_node.text_range()),
            span: span(node.text_range()),
        };
        if let Some(body) = node.children().find(|child| child.kind() == CLASS_BODY) {
            self.members(&body, &scope, &mut decl);
        }
        self.out.classes.push(decl);
    }

    fn members(&self, body: &SyntaxNode, scope: &ClassScope, decl: &mut ClassDecl) {
        let is_interface = decl.kind == ClassKind::Interface;
        for member in body.children() {
            match member.kind() {
                METHOD_DECLARATION => {
                    if let Some(method) = self.method(&member, scope, is_interface) {
                        for param in &method.callable.params {
                            if let Some(promotion) = param.promoted {
                                decl.properties
                                    .push(promoted_property(param, promotion, decl.is_readonly));
                            }
                        }
                        decl.methods.push(method);
                    }
                }
                PROPERTY_DECLARATION => self.properties(&member, scope, decl),
                CLASS_CONST_DECLARATION => self.class_constants(&member, scope, decl),
                ENUM_CASE => {
                    if let Some(case) = self.enum_case(&member, scope) {
                        decl.constants.push(case);
                    }
                }
                TRAIT_USE => {
                    let uses = self.trait_use(&member, decl.doc.as_deref(), Some(scope));
                    decl.trait_uses.extend(uses);
                }
                _ => {}
            }
        }
    }

    fn method(&self, node: &SyntaxNode, scope: &ClassScope, in_interface: bool) -> Option<Method> {
        let name = name_child(node)?;
        let modifiers = node.children().find(|child| child.kind() == MODIFIER_LIST);
        let (visibility, _) = modifiers.as_ref().map(visibility_of).unwrap_or((None, None));
        let has_modifier = |kind: SyntaxKind| modifiers.as_ref().is_some_and(|list| has_token(list, kind));
        let attributes = self.attributes(node);
        let doc = self.doc_with_templates(node, Some(scope));
        let callable = self.callable(node, doc.as_deref(), &attributes, Some(scope));
        let has_body = node.children().any(|child| child.kind() == BLOCK);
        Some(Method {
            name: unreserved(&name.text().to_string()),
            visibility: visibility.unwrap_or(Visibility::Public),
            is_static: has_modifier(STATIC_KW),
            is_abstract: has_modifier(ABSTRACT_KW) || (in_interface && !has_body),
            is_final: has_modifier(FINAL_KW),
            callable,
            availability: self.availability(doc.as_deref(), &attributes),
            doc: with_attribute_deprecation(doc, &attributes),
            attributes,
            name_span: span(name.text_range()),
            span: span(node.text_range()),
        })
    }

    fn properties(&self, node: &SyntaxNode, scope: &ClassScope, decl: &mut ClassDecl) {
        let modifiers = node.children().find(|child| child.kind() == MODIFIER_LIST);
        let (visibility, set_visibility) = modifiers.as_ref().map(visibility_of).unwrap_or((None, None));
        let has_modifier = |kind: SyntaxKind| modifiers.as_ref().is_some_and(|list| has_token(list, kind));
        let ty = node
            .children()
            .find(|child| is_type_node(child.kind()))
            .map(|ty| self.native_type(&ty, Some(scope)));
        let doc = self.parse_doc(node, Some(scope));
        let attributes = self.attributes(node);
        let hooks: Vec<String> = node
            .children()
            .find(|child| child.kind() == PROPERTY_HOOK_LIST)
            .map(|list| {
                list.children()
                    .filter(|hook| hook.kind() == PROPERTY_HOOK)
                    .filter_map(|hook| name_child(&hook))
                    .map(|name| name.text().to_string().to_ascii_lowercase())
                    .collect()
            })
            .unwrap_or_default();
        for element in node.children().filter(|child| child.kind() == PROPERTY_ELEMENT) {
            let Some(variable) = tokens(&element).find(|token| token.kind() == VARIABLE) else {
                continue;
            };
            decl.properties.push(Property {
                name: variable.text().trim_start_matches('$').to_string(),
                visibility: visibility.unwrap_or(Visibility::Public),
                set_visibility,
                is_static: has_modifier(STATIC_KW),
                is_readonly: has_modifier(READONLY_KW) || decl.is_readonly,
                is_abstract: has_modifier(ABSTRACT_KW),
                ty: ty.clone(),
                doc_ty: doc
                    .as_deref()
                    .and_then(|doc| doc.var.as_ref())
                    .map(|(ty, _, _)| ty.clone()),
                leveled: self.leveled(&attributes, Some(scope)),
                default: value_text(&element),
                promoted: false,
                hooks: hooks.clone(),
                availability: self.availability(doc.as_deref(), &attributes),
                doc: doc.clone(),
                attributes: attributes.clone(),
                name_span: span(variable.text_range()),
                span: span(node.text_range()),
            });
        }
    }

    fn class_constants(&self, node: &SyntaxNode, scope: &ClassScope, decl: &mut ClassDecl) {
        let modifiers = node.children().find(|child| child.kind() == MODIFIER_LIST);
        let (visibility, _) = modifiers.as_ref().map(visibility_of).unwrap_or((None, None));
        let is_final = modifiers.as_ref().is_some_and(|list| has_token(list, FINAL_KW));
        let ty = node
            .children()
            .find(|child| is_type_node(child.kind()))
            .map(|ty| self.native_type(&ty, Some(scope)));
        let doc = self.parse_doc(node, Some(scope));
        let attributes = self.attributes(node);
        for element in node.children().filter(|child| child.kind() == CONST_ELEMENT) {
            let Some(name) = name_child(&element) else {
                continue;
            };
            decl.constants.push(ClassConst {
                name: name.text().to_string(),
                visibility: visibility.unwrap_or(Visibility::Public),
                is_final,
                is_case: false,
                ty: ty
                    .clone()
                    .or_else(|| value_node(&element).as_ref().and_then(literal_type)),
                doc_ty: doc
                    .as_deref()
                    .and_then(|doc| doc.var.as_ref())
                    .map(|(ty, _, _)| ty.clone()),
                leveled: self.leveled(&attributes, Some(scope)),
                value: value_text(&element),
                availability: self.availability(doc.as_deref(), &attributes),
                doc: doc.clone(),
                attributes: attributes.clone(),
                name_span: span(name.text_range()),
                span: span(node.text_range()),
            });
        }
    }

    fn enum_case(&self, node: &SyntaxNode, scope: &ClassScope) -> Option<ClassConst> {
        let name = name_child(node)?;
        let doc = self.parse_doc(node, Some(scope));
        let attributes = self.attributes(node);
        Some(ClassConst {
            name: name.text().to_string(),
            visibility: Visibility::Public,
            is_final: true,
            is_case: true,
            ty: Some(Type::class(scope.name.clone())),
            doc_ty: None,
            leveled: None,
            value: value_text(node),
            availability: self.availability(doc.as_deref(), &attributes),
            doc,
            attributes,
            name_span: span(name.text_range()),
            span: span(node.text_range()),
        })
    }

    fn trait_use(&self, node: &SyntaxNode, class_doc: Option<&Doc>, scope: Option<&ClassScope>) -> Vec<TraitUse> {
        let own_doc = self.parse_doc(node, scope);
        let names: Vec<Name> = node
            .children()
            .filter(|child| child.kind() == NAME)
            .map(|name| self.resolver.resolve_class(&name.text().to_string()))
            .collect();
        let mut adaptations = Vec::new();
        if let Some(block) = node.children().find(|child| child.kind() == TRAIT_ADAPTATIONS) {
            for item in block.children() {
                match item.kind() {
                    TRAIT_PRECEDENCE => {
                        let mut parts = item.children().filter(|child| child.kind() == NAME);
                        let (Some(trait_name), Some(method)) = (parts.next(), parts.next()) else {
                            continue;
                        };
                        adaptations.push(Adaptation::InsteadOf {
                            trait_name: self.resolver.resolve_class(&trait_name.text().to_string()),
                            method: method.text().to_string(),
                            excluded: parts
                                .map(|name| self.resolver.resolve_class(&name.text().to_string()))
                                .collect(),
                        });
                    }
                    TRAIT_ALIAS => {
                        let mut before = Vec::new();
                        let mut after = Vec::new();
                        let mut seen_as = false;
                        let mut visibility = None;
                        for element in item.children_with_tokens() {
                            match element {
                                SyntaxElement::Token(token) if token.kind() == AS_KW => seen_as = true,
                                SyntaxElement::Node(child) if child.kind() == NAME => {
                                    if seen_as {
                                        after.push(child.text().to_string());
                                    } else {
                                        before.push(child.text().to_string());
                                    }
                                }
                                SyntaxElement::Node(child) if child.kind() == MODIFIER_LIST => {
                                    visibility = visibility_of(&child).0;
                                }
                                _ => {}
                            }
                        }
                        let (trait_name, method) = match before.as_slice() {
                            [method] => (None, method.clone()),
                            [trait_name, method, ..] => (Some(self.resolver.resolve_class(trait_name)), method.clone()),
                            [] => continue,
                        };
                        adaptations.push(Adaptation::Alias {
                            trait_name,
                            method,
                            alias: after.into_iter().next(),
                            visibility,
                        });
                    }
                    _ => {}
                }
            }
        }
        names
            .into_iter()
            .map(|name| {
                let ty = own_doc
                    .as_deref()
                    .into_iter()
                    .chain(class_doc)
                    .find_map(|doc| {
                        doc.uses
                            .iter()
                            .find(|ty| matches!(ty, Type::Class { name: used, .. } if used.eq_ignore_ascii_case(&name)))
                    })
                    .cloned()
                    .unwrap_or_else(|| Type::class(name));
                TraitUse {
                    ty,
                    adaptations: adaptations.clone(),
                }
            })
            .collect()
    }
}

fn reads_all_arguments(node: &SyntaxNode) -> bool {
    let Some(body) = node.children().find(|child| child.kind() == BLOCK) else {
        return false;
    };
    body.descendants().any(|call| {
        call.kind() == CALL_EXPR
            && call.children().next().is_some_and(|callee| {
                callee.kind() == NAME
                    && matches!(
                        callee
                            .text()
                            .to_string()
                            .trim_start_matches('\\')
                            .to_ascii_lowercase()
                            .as_str(),
                        "func_get_args" | "func_get_arg" | "func_num_args"
                    )
            })
    })
}

fn contains_yield(node: &SyntaxNode) -> bool {
    let Some(body) = node.children().find(|child| child.kind() == BLOCK) else {
        return false;
    };
    let mut stack = vec![body];
    while let Some(current) = stack.pop() {
        for child in current.children() {
            match child.kind() {
                YIELD_EXPR | YIELD_FROM_EXPR => return true,
                FUNCTION_DECLARATION | CLOSURE_EXPR | ARROW_FUNCTION_EXPR | ANONYMOUS_CLASS | CLASS_DECLARATION => {}
                _ => stack.push(child),
            }
        }
    }
    false
}

fn promoted_property(param: &Param, promotion: Promotion, readonly_class: bool) -> Property {
    Property {
        name: param.name.clone(),
        visibility: promotion.visibility,
        set_visibility: None,
        is_static: false,
        is_readonly: promotion.readonly || readonly_class,
        is_abstract: false,
        ty: param.ty.clone(),
        doc_ty: param.doc_ty.clone(),
        leveled: None,
        default: None,
        promoted: true,
        hooks: Vec::new(),
        doc: None,
        attributes: Vec::new(),
        availability: Availability::default(),
        name_span: param.span,
        span: param.span,
    }
}

/// The visibility keyword of a modifier list and, for `private(set)`, the visibility of writes.
fn visibility_of(list: &SyntaxNode) -> (Option<Visibility>, Option<Visibility>) {
    let mut read = None;
    let mut write = None;
    let all: Vec<SyntaxToken> = tokens(list).filter(|token| !token.kind().is_trivia()).collect();
    let mut index = 0;
    while index < all.len() {
        let visibility = match all[index].kind() {
            PUBLIC_KW => Some(Visibility::Public),
            PROTECTED_KW => Some(Visibility::Protected),
            PRIVATE_KW => Some(Visibility::Private),
            _ => None,
        };
        if let Some(visibility) = visibility {
            let is_set = all.get(index + 1).is_some_and(|token| token.kind() == LPAREN)
                && all.get(index + 2).is_some_and(|token| token.text() == "set");
            if is_set {
                write = Some(visibility);
                index += 4;
                continue;
            }
            read = Some(visibility);
        }
        index += 1;
    }
    (read, write)
}

/// The type names of an `extends` or `implements` clause, resolved.
fn clause_names(node: &SyntaxNode, kind: SyntaxKind, resolver: &NameResolver) -> Vec<Name> {
    node.children()
        .find(|child| child.kind() == kind)
        .map(|clause| {
            clause
                .descendants()
                .filter(|child| child.kind() == NAME)
                .map(|name| resolver.resolve_class(&name.text().to_string()))
                .collect()
        })
        .unwrap_or_default()
}

/// The names of a clause as types, with the arguments `@extends Base<Foo>` gives them.
fn merge_generic_names(names: Vec<Name>, doc_types: Option<&[Type]>) -> Vec<Type> {
    names
        .into_iter()
        .map(|name| {
            doc_types
                .and_then(|types| {
                    types
                        .iter()
                        .find(|ty| matches!(ty, Type::Class { name: documented, .. } if documented.eq_ignore_ascii_case(&name)))
                })
                .cloned()
                .unwrap_or_else(|| Type::class(name))
        })
        .collect()
}

fn argument_of(argument: &SyntaxNode) -> AttributeArg {
    let name = tokens(argument)
        .find(|token| !token.kind().is_trivia())
        .filter(|token| !token.kind().is_keyword() || token.kind() == IDENT || token.kind() != ELLIPSIS)
        .filter(|_| has_token(argument, COLON))
        .map(|token| token.text().to_string());
    let value = argument
        .children()
        .last()
        .map(|node| collapse(&node.text().to_string()))
        .unwrap_or_default();
    AttributeArg { name, value }
}

/// `#[Deprecated]` in a stub is a `@deprecated` tag.
fn with_attribute_deprecation(doc: Option<Box<Doc>>, attributes: &[Attribute]) -> Option<Box<Doc>> {
    let deprecated = attributes
        .iter()
        .find(|attribute| attribute.name.ends_with("\\Deprecated") || attribute.name == "Deprecated");
    let Some(deprecated) = deprecated else {
        return doc;
    };
    let mut doc = doc.unwrap_or_default();
    if doc.deprecated.is_none() {
        let reason = deprecated
            .args
            .iter()
            .find(|arg| matches!(arg.name.as_deref(), None | Some("reason")))
            .map(|arg| unquote(&arg.value))
            .unwrap_or_default();
        doc.deprecated = Some(reason);
    }
    Some(doc)
}

/// The entries of `['8.0' => 'string|false', '7.4' => 'string']`.
fn parse_level_map(text: &str, cx: &TypeContext) -> Vec<(u16, Type)> {
    let inner = text.trim().trim_start_matches('[').trim_end_matches(']');
    let mut out = Vec::new();
    for entry in split_entries(inner) {
        let Some((key, value)) = entry.split_once("=>") else {
            continue;
        };
        let Some(version) = PhpVersion::parse(&unquote(key)) else {
            continue;
        };
        if let Some(ty) = parse_type(&unquote(value), cx) {
            out.push((encode_version(version), ty));
        }
    }
    out
}

fn split_entries(text: &str) -> Vec<&str> {
    let mut entries = Vec::new();
    let mut quote = None;
    let mut start = 0;
    for (index, c) in text.char_indices() {
        match (quote, c) {
            (None, '\'' | '"') => quote = Some(c),
            (Some(open), c) if c == open => quote = None,
            (None, ',') => {
                entries.push(&text[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    if !text[start..].trim().is_empty() {
        entries.push(&text[start..]);
    }
    entries
}

/// A declared type from the tree.
pub fn native_type(node: &SyntaxNode, cx: &TypeContext) -> Type {
    match node.kind() {
        NAMED_TYPE => {
            let Some(name) = name_child(node) else {
                return Type::Unknown;
            };
            let raw = name.text().to_string();
            match raw.to_ascii_lowercase().as_str() {
                "int" => Type::Int,
                "float" => Type::Float,
                "string" => Type::String,
                "bool" => Type::Bool,
                "array" => Type::plain_array(),
                "callable" => Type::Callable(None),
                "iterable" => Type::Iterable(Box::new(Type::Mixed), Box::new(Type::Mixed)),
                "object" => Type::Object,
                "mixed" => Type::Mixed,
                "void" => Type::Void,
                "null" => Type::Null,
                "never" => Type::Never,
                "false" => Type::False,
                "true" => Type::True,
                _ => cx.class_type(&raw),
            }
        }
        NULLABLE_TYPE => node
            .children()
            .find(|child| is_type_node(child.kind()))
            .map_or(Type::Unknown, |inner| native_type(&inner, cx).nullable()),
        UNION_TYPE => Type::union(
            node.children()
                .filter(|child| is_type_node(child.kind()))
                .map(|child| native_type(&child, cx)),
        ),
        INTERSECTION_TYPE => Type::Intersection(
            node.children()
                .filter(|child| is_type_node(child.kind()))
                .map(|child| native_type(&child, cx))
                .collect(),
        ),
        PAREN_TYPE => node
            .children()
            .find(|child| is_type_node(child.kind()))
            .map_or(Type::Unknown, |inner| native_type(&inner, cx)),
        _ => Type::Unknown,
    }
}

/// The parameters and return type of a function or method node, with the doc comment above it read
/// at the namespace and imports of `resolver`.
pub fn callable_at(
    node: &SyntaxNode,
    resolver: &NameResolver,
    scope: Option<&ClassScope>,
) -> (Callable, Option<Box<Doc>>) {
    let extractor = Extractor {
        options: ExtractOptions::default(),
        resolver: resolver.clone(),
        out: FileSymbols::default(),
    };
    let doc = extractor.doc_with_templates(node, scope);
    let attributes = extractor.attributes(node);
    let callable = extractor.callable(node, doc.as_deref(), &attributes, scope);
    (callable, doc)
}

/// The resolver in effect at an offset of a file: the namespace around it and the imports above it.
pub fn resolver_at(root: &SyntaxNode, offset: u32) -> NameResolver {
    let mut extractor = Extractor {
        options: ExtractOptions::default(),
        resolver: NameResolver::new(""),
        out: FileSymbols::default(),
    };
    extractor.resolver_until(root, offset);
    extractor.resolver
}

impl Extractor {
    /// Reads namespaces and imports up to an offset, which is what the names at that point mean.
    fn resolver_until(&mut self, container: &SyntaxNode, offset: u32) {
        for child in container.children() {
            let range = child.text_range();
            if u32::from(range.start()) >= offset {
                break;
            }
            match child.kind() {
                NAMESPACE_DECLARATION => {
                    let namespace = name_child(&child)
                        .map(|name| name.text().to_string())
                        .unwrap_or_default();
                    let inside = u32::from(range.end()) > offset || child.children().all(|c| c.kind() != BLOCK);
                    if inside {
                        self.resolver = NameResolver::new(&namespace);
                        self.resolver_until(&child, offset);
                    }
                }
                USE_STATEMENT => self.use_statement(&child),
                STATEMENT_LIST | BLOCK
                    if child
                        .parent()
                        .is_some_and(|parent| parent.kind() == NAMESPACE_DECLARATION) =>
                {
                    self.resolver_until(&child, offset);
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use php_syntax::parse;

    fn symbols(text: &str, stub: bool) -> FileSymbols {
        extract(&parse(text).syntax(), ExtractOptions { stub })
    }

    #[test]
    fn reads_a_class_with_its_members() {
        let file = symbols(
            r#"<?php
namespace App\Models;

use Illuminate\Support\Collection;

/**
 * A user.
 * @template T
 * @extends Base<T>
 */
#[Attr(1, name: 'x')]
final class User extends Base implements \JsonSerializable {
    use Stamps;
    public const int MAX = 1;
    public ?int $age = null;
    /** @var Collection<int, Post> */
    protected Collection $posts;
    public function __construct(private readonly string $name, public int|string $id = 5) {}
    /**
     * Finds one.
     * @param int $id the id
     * @return static|null
     */
    public static function find(int $id): ?static {}
    abstract protected function x(): void;
}
"#,
            false,
        );
        assert_eq!(file.classes.len(), 1);
        let class = &file.classes[0];
        assert_eq!(class.name, "App\\Models\\User");
        assert!(class.is_final);
        assert_eq!(class.extends[0].display(false), "App\\Models\\Base<T>");
        assert_eq!(class.implements[0].display(false), "JsonSerializable");
        assert_eq!(class.trait_uses[0].ty.display(false), "App\\Models\\Stamps");
        assert_eq!(class.attributes[0].name, "App\\Models\\Attr");
        assert_eq!(class.attributes[0].args[1].name.as_deref(), Some("name"));
        assert_eq!(class.doc.as_ref().unwrap().summary, "A user.");
        assert_eq!(class.constants[0].ty.as_ref().unwrap().display(false), "int");
        let posts = class.property("posts").unwrap();
        assert_eq!(posts.visibility, Visibility::Protected);
        assert_eq!(
            posts.doc_ty.as_ref().unwrap().display(false),
            "Illuminate\\Support\\Collection<int, App\\Models\\Post>"
        );
        let name = class.property("name").unwrap();
        assert!(name.promoted && name.is_readonly);
        assert_eq!(
            class.property("id").unwrap().ty.as_ref().unwrap().display(true),
            "int|string"
        );
        let find = class.method("find").unwrap();
        assert!(find.is_static);
        assert_eq!(find.callable.params[0].description, "the id");
        assert_eq!(find.callable.ret.as_ref().unwrap().display(false), "?static");
        assert!(class.method("x").unwrap().is_abstract);
    }

    #[test]
    fn reads_enums_interfaces_and_traits() {
        let file = symbols(
            "<?php enum Suit: string implements HasLabel { case Hearts = 'h'; const X = 1; public function label(): string {} } interface I extends J, K { public function f(); } trait T { public $x; use U { U::a as protected b; } }",
            false,
        );
        let suit = &file.classes[0];
        assert_eq!(suit.kind, ClassKind::Enum);
        assert_eq!(suit.backing, Some(Type::String));
        assert!(suit.constants[0].is_case);
        assert_eq!(suit.constants[0].ty.as_ref().unwrap().display(false), "Suit");
        let interface = &file.classes[1];
        assert_eq!(interface.extends.len(), 2);
        assert!(interface.method("f").unwrap().is_abstract);
        let trait_decl = &file.classes[2];
        assert!(matches!(
            &trait_decl.trait_uses[0].adaptations[0],
            Adaptation::Alias { alias: Some(alias), visibility: Some(Visibility::Protected), .. } if alias == "b"
        ));
    }

    #[test]
    fn reads_hooks_and_asymmetric_visibility() {
        let file = symbols(
            "<?php class A { public private(set) string $x; public string $h { get => 1; set { } } }",
            false,
        );
        let class = &file.classes[0];
        assert_eq!(class.property("x").unwrap().set_visibility, Some(Visibility::Private));
        assert_eq!(class.property("h").unwrap().hooks, vec!["get", "set"]);
    }

    #[test]
    fn reads_functions_and_constants() {
        let file = symbols(
            "<?php namespace N; const A = 1; define('B', 'x'); function &f(int $a = 1, string ...$rest): void {} if (!function_exists('g')) { function g() { yield 1; } }",
            false,
        );
        assert_eq!(file.constants[0].name, "N\\A");
        assert_eq!(file.constants[0].ty, Some(Type::Int));
        assert_eq!(file.constants[1].name, "B");
        assert_eq!(file.constants[1].ty, Some(Type::String));
        let f = &file.functions[0];
        assert_eq!(f.name, "N\\f");
        assert!(f.callable.by_ref_return);
        assert_eq!(f.callable.params[0].default.as_deref(), Some("1"));
        assert!(f.callable.params[1].variadic);
        assert_eq!(file.functions[1].name, "N\\g");
        assert!(file.functions[1].callable.is_generator);
    }

    #[test]
    fn resolves_names_across_namespaces_and_groups() {
        let file = symbols(
            "<?php namespace A { use X\\{Y, Z as W}; class C extends Y {} } namespace B { class D extends W {} }",
            false,
        );
        assert_eq!(file.classes[0].extends[0].display(false), "X\\Y");
        assert_eq!(file.classes[1].name, "B\\D");
        assert_eq!(file.classes[1].extends[0].display(false), "B\\W");
    }

    #[test]
    fn reads_stub_availability_and_leveled_types() {
        let file = symbols(
            r#"<?php
/**
 * @since 8.1
 * @removed 8.4
 */
function old(
    #[PhpStormStubsElementAvailable(to: '7.4')] $legacy,
    #[PhpStormStubsElementAvailable(from: '8.0')] string $modern
): string {}
#[PhpStormStubsElementAvailable(from: '8.0')]
function newer() {}
class C {
    #[LanguageLevelTypeAware(['8.0' => 'string|false'], default: 'string')]
    public $prop;
    #[LanguageLevelTypeAware(['8.2' => 'static'], default: 'self')]
    public function m() {}
    #[Deprecated(reason: 'use n', since: '8.1')]
    public function n() {}
}
"#,
            true,
        );
        let old = &file.functions[0];
        assert!(old.availability.contains(PhpVersion::V8_2));
        assert!(!old.availability.contains(PhpVersion::V8_0));
        assert!(!old.availability.contains(PhpVersion::V8_4));
        assert!(old.callable.params[0].availability.contains(PhpVersion::V7_4));
        assert!(!old.callable.params[0].availability.contains(PhpVersion::V8_0));
        assert!(old.callable.params[1].availability.contains(PhpVersion::V8_0));
        assert!(file.functions[1].availability.contains(PhpVersion::V8_0));
        assert!(!file.functions[1].availability.contains(PhpVersion::V7_4));
        let class = &file.classes[0];
        let prop = class.property("prop").unwrap();
        assert_eq!(
            prop.effective_type(PhpVersion::V8_1).unwrap().display(false),
            "string|false"
        );
        assert_eq!(prop.effective_type(PhpVersion::V7_4).unwrap().display(false), "string");
        let method = class.method("m").unwrap();
        assert_eq!(
            method
                .callable
                .effective_return(PhpVersion::V8_3)
                .unwrap()
                .display(false),
            "static"
        );
        assert_eq!(
            method
                .callable
                .effective_return(PhpVersion::V8_1)
                .unwrap()
                .display(false),
            "C"
        );
        assert_eq!(
            class.method("n").unwrap().doc.as_ref().unwrap().deprecated.as_deref(),
            Some("use n")
        );
    }

    #[test]
    fn reads_the_resolver_at_an_offset() {
        let text = "<?php namespace A; use B\\C; class X {} namespace D; use E\\F; $x = 1;";
        let parsed = parse(text);
        let first = resolver_at(&parsed.syntax(), 30);
        assert_eq!(first.namespace, "A");
        assert_eq!(first.resolve_class("C"), "B\\C");
        let second = resolver_at(&parsed.syntax(), text.len() as u32);
        assert_eq!(second.namespace, "D");
        assert_eq!(second.resolve_class("F"), "E\\F");
        assert_eq!(second.resolve_class("C"), "D\\C");
    }
}
