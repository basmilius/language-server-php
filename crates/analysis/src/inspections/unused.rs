//! Declarations nothing uses: `use` statements, and private methods, properties and constants.

use std::collections::HashSet;
use std::rc::Rc;

use php_index::UseKind;
use php_syntax::SyntaxKind::*;
use php_syntax::SyntaxNode;

use super::util::has_modifier;
use super::{Cx, Fix};
use crate::ast::{child_of, first_token, has_token, text_of, tokens};

pub(super) fn run(cx: &Cx) {
    if cx.on("unused-import") {
        unused_imports(cx);
    }
    if cx.on("unused-private-method") || cx.on("unused-private-property") || cx.on("unused-private-constant") {
        for node in &cx.nodes {
            if matches!(node.kind(), CLASS_DECLARATION | ENUM_DECLARATION | ANONYMOUS_CLASS) {
                unused_private_members(cx, node);
            }
        }
    }
}

/// A name a `use` statement brings in.
pub(crate) struct ImportClause {
    pub statement: SyntaxNode,
    pub clause: SyntaxNode,
    pub kind: UseKind,
    /// The qualified name without a leading backslash.
    pub full: String,
    /// Something in the file refers to it.
    pub used: bool,
}

/// Every name the file imports, and whether anything uses it.
pub(crate) fn import_clauses(cx: &Cx) -> Vec<ImportClause> {
    let mentioned = mentioned_names(cx);
    let mut out = Vec::new();
    for statement in cx.nodes.iter().filter(|node| node.kind() == USE_STATEMENT) {
        if statement.parent().is_some_and(|parent| parent.kind() == CLASS_BODY) {
            continue;
        }
        let statement_kind = kind_of(statement);
        for clause in statement.descendants().filter(|node| node.kind() == USE_CLAUSE) {
            let Some(import) = import_of(&clause) else {
                continue;
            };
            let kind = if has_token(&clause, FUNCTION_KW) {
                UseKind::Function
            } else if has_token(&clause, CONST_KW) {
                UseKind::Constant
            } else {
                statement_kind
            };
            let key = if kind == UseKind::Constant {
                import.alias.clone()
            } else {
                import.alias.to_ascii_lowercase()
            };
            out.push(ImportClause {
                statement: statement.clone(),
                clause,
                kind,
                full: import.full,
                used: mentioned.contains(&key),
            });
        }
    }
    out
}

fn kind_of(statement: &SyntaxNode) -> UseKind {
    if has_token(statement, FUNCTION_KW) {
        UseKind::Function
    } else if has_token(statement, CONST_KW) {
        UseKind::Constant
    } else {
        UseKind::Class
    }
}

struct Import {
    alias: String,
    full: String,
}

fn unused_imports(cx: &Cx) {
    let clauses = import_clauses(cx);
    let mut statements: Vec<SyntaxNode> = Vec::new();
    for entry in &clauses {
        if !statements.contains(&entry.statement) {
            statements.push(entry.statement.clone());
        }
    }
    for statement in &statements {
        let own: Vec<&ImportClause> = clauses.iter().filter(|entry| entry.statement == *statement).collect();
        let unused: Vec<&&ImportClause> = own.iter().filter(|entry| !entry.used).collect();
        if unused.is_empty() {
            continue;
        }
        if unused.len() == own.len() {
            let names: Vec<&str> = unused.iter().map(|entry| entry.full.as_str()).collect();
            cx.report(
                "unused-import",
                statement.text_range(),
                format!("Unused import '{}'", names.join("', '")),
                Fix::RemoveImport {
                    range: statement.text_range(),
                },
            );
            continue;
        }
        for entry in unused {
            cx.report(
                "unused-import",
                entry.clause.text_range(),
                format!("Unused import '{}'", entry.full),
                Fix::RemoveImport {
                    range: entry.clause.text_range(),
                },
            );
        }
    }
}

/// The alias and the full name a `use` clause brings in.
fn import_of(clause: &SyntaxNode) -> Option<Import> {
    let mut names = clause.children().filter(|child| child.kind() == NAME);
    let written = names.next()?;
    let alias = names.next();
    let mut full = text_of(&written).trim_start_matches('\\').to_string();
    if let Some(group) = clause.parent().filter(|parent| parent.kind() == USE_GROUP) {
        if let Some(prefix) = child_of(&group, NAME) {
            full = format!("{}\\{}", text_of(&prefix).trim_matches('\\'), full);
        }
    }
    let alias = match alias {
        Some(alias) => text_of(&alias),
        None => full.rsplit('\\').next().unwrap_or(&full).to_string(),
    };
    Some(Import { alias, full })
}

/// The first segment of every name written in code, and every word in the comments, since an
/// import a doc comment names is used. Classes and functions fold case; constants do not.
pub(crate) fn mentioned_names(cx: &Cx) -> HashSet<String> {
    let mut out = HashSet::new();
    for node in &cx.nodes {
        if node.kind() == NAME {
            let in_use = node.ancestors().any(|ancestor| ancestor.kind() == USE_STATEMENT);
            if in_use {
                continue;
            }
            for token in tokens(node) {
                let text = token.text();
                if text.starts_with('\\') {
                    continue;
                }
                let first = text.split('\\').next().unwrap_or(text);
                out.insert(first.to_string());
                out.insert(first.to_ascii_lowercase());
            }
        }
    }
    for element in cx.root.descendants_with_tokens() {
        let Some(token) = element.into_token() else {
            continue;
        };
        if matches!(token.kind(), DOC_COMMENT | BLOCK_COMMENT | COMMENT) {
            for word in token
                .text()
                .split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '\\'))
                .filter(|word| !word.is_empty())
            {
                let first = word.trim_start_matches('\\').split('\\').next().unwrap_or(word);
                out.insert(first.to_string());
                out.insert(first.to_ascii_lowercase());
            }
        }
    }
    out
}

fn unused_private_members(cx: &Cx, class: &SyntaxNode) {
    let Some(body) = child_of(class, CLASS_BODY) else {
        return;
    };
    if body.children().any(|member| member.kind() == TRAIT_USE) {
        return;
    }
    let usage = Usage::of(class);
    if usage.dynamic {
        return;
    }
    for member in body.children() {
        match member.kind() {
            METHOD_DECLARATION => check_method(cx, &member, &usage),
            PROPERTY_DECLARATION => check_property(cx, &member, &usage),
            CLASS_CONST_DECLARATION => check_constant(cx, &member, &usage),
            _ => {}
        }
        if member.kind() == METHOD_DECLARATION {
            check_promoted(cx, &member, &usage);
        }
    }
}

/// The usage of a class, worked out once for all the checks that ask.
pub(super) fn usage_of(cx: &Cx, class: &SyntaxNode) -> Rc<Usage> {
    let key = u32::from(class.text_range().start());
    if let Some(found) = cx.usages.borrow().get(&key) {
        return found.clone();
    }
    let usage = Rc::new(Usage::of(class));
    cx.usages.borrow_mut().insert(key, usage.clone());
    usage
}

/// What the code of a class refers to by name.
pub(super) struct Usage {
    /// Names after `->`, `?->` and `::`, and names written as a call, lowercase.
    members: HashSet<String>,
    /// `$name` after `::`, and the `$` removed.
    statics: HashSet<String>,
    /// The text of string literals, lowercase, for callables and property names.
    strings: HashSet<String>,
    /// Names of methods used as a value: written as a string or made into a callable.
    callables: HashSet<String>,
    /// Something reaches members by a name only known when it runs.
    pub dynamic: bool,
}

impl Usage {
    fn of(class: &SyntaxNode) -> Usage {
        let mut usage = Usage {
            members: HashSet::new(),
            statics: HashSet::new(),
            strings: HashSet::new(),
            callables: HashSet::new(),
            dynamic: false,
        };
        for node in class.descendants() {
            match node.kind() {
                PROPERTY_FETCH_EXPR | SCOPED_ACCESS_EXPR => {
                    let first = node.children().next();
                    let member = node
                        .children()
                        .filter(|child| child.kind() == NAME)
                        .last()
                        .filter(|name| Some(name) != first.as_ref());
                    match member {
                        Some(name) => {
                            usage.members.insert(text_of(&name).to_ascii_lowercase());
                            if is_first_class_callable(&node) {
                                usage.callables.insert(text_of(&name).to_ascii_lowercase());
                            }
                        }
                        None => {
                            let dynamic_member = node.children().skip(1).any(|child| {
                                matches!(
                                    child.kind(),
                                    VARIABLE_EXPR
                                        | VARIABLE_VARIABLE
                                        | PAREN_EXPR
                                        | LITERAL
                                        | CALL_EXPR
                                        | BINARY_EXPR
                                        | INDEX_EXPR
                                        | INTERPOLATED_STRING
                                )
                            });
                            if dynamic_member || tokens(&node).any(|token| token.kind() == LBRACE) {
                                usage.dynamic = true;
                            }
                        }
                    }
                }
                STATIC_PROPERTY_EXPR => {
                    for variable in node.children().filter(|child| child.kind() == VARIABLE_EXPR) {
                        usage
                            .statics
                            .insert(text_of(&variable).trim_start_matches('$').to_string());
                    }
                }
                LITERAL => {
                    if let Some(token) = first_token(&node, STRING_LITERAL) {
                        let text = token.text();
                        let inner = text.get(1..text.len().saturating_sub(1)).unwrap_or("");
                        usage.strings.insert(inner.to_ascii_lowercase());
                        for word in inner.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
                            if !word.is_empty() {
                                usage.strings.insert(word.to_ascii_lowercase());
                            }
                        }
                    }
                }
                CALL_EXPR => {
                    let reflects = node
                        .children()
                        .next()
                        .filter(|callee| callee.kind() == NAME)
                        .is_some_and(|callee| {
                            matches!(
                                text_of(&callee).to_ascii_lowercase().as_str(),
                                "get_object_vars"
                                    | "get_class_vars"
                                    | "call_user_func"
                                    | "call_user_func_array"
                                    | "extract"
                            )
                        });
                    usage.dynamic |= reflects;
                }
                INTERPOLATED_STRING | HEREDOC => {
                    for token in tokens(&node).filter(|token| token.kind() == STRING_CONTENT) {
                        for word in token.text().split(|c: char| !(c.is_alphanumeric() || c == '_')) {
                            if !word.is_empty() {
                                usage.strings.insert(word.to_ascii_lowercase());
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        usage
    }

    /// Whether the method is handed around as a value, where its parameters are the receiver's to dictate.
    pub fn is_callback(&self, name: &str) -> bool {
        let lower = name.to_ascii_lowercase();
        self.callables.contains(&lower) || self.strings.contains(&lower)
    }

    fn method_used(&self, name: &str) -> bool {
        let lower = name.to_ascii_lowercase();
        self.members.contains(&lower) || self.strings.contains(&lower)
    }

    fn property_used(&self, name: &str) -> bool {
        self.members.contains(&name.to_ascii_lowercase())
            || self.statics.contains(name)
            || self.strings.contains(&name.to_ascii_lowercase())
    }
}

fn is_first_class_callable(access: &SyntaxNode) -> bool {
    access.parent().is_some_and(|call| {
        call.kind() == CALL_EXPR
            && child_of(&call, ARGUMENT_LIST).is_some_and(|list| {
                list.children().next().is_none() && tokens(&list).any(|token| token.kind() == ELLIPSIS)
            })
    })
}

/// Private to the class, as opposed to `private(set)`, which only limits who writes.
fn is_private(declaration: &SyntaxNode) -> bool {
    let Some(modifiers) = child_of(declaration, MODIFIER_LIST) else {
        return false;
    };
    let kinds: Vec<_> = tokens(&modifiers)
        .filter(|token| !token.kind().is_trivia())
        .map(|token| token.kind())
        .collect();
    let private = kinds
        .iter()
        .enumerate()
        .any(|(position, kind)| *kind == PRIVATE_KW && kinds.get(position + 1) != Some(&LPAREN));
    private && !kinds.contains(&PUBLIC_KW) && !kinds.contains(&PROTECTED_KW) && has_modifier(declaration, PRIVATE_KW)
}

fn has_attributes(declaration: &SyntaxNode) -> bool {
    child_of(declaration, ATTRIBUTE_LIST).is_some()
}

fn check_method(cx: &Cx, method: &SyntaxNode, usage: &Usage) {
    if !cx.on("unused-private-method") || !is_private(method) || has_attributes(method) {
        return;
    }
    let Some(name) = child_of(method, NAME) else {
        return;
    };
    let written = text_of(&name);
    if written.starts_with("__") || usage.method_used(&written) {
        return;
    }
    cx.report(
        "unused-private-method",
        name.text_range(),
        format!("Private method '{written}' is never used"),
        Fix::RemoveDeclaration {
            range: method.text_range(),
        },
    );
}

fn check_property(cx: &Cx, declaration: &SyntaxNode, usage: &Usage) {
    if !cx.on("unused-private-property") || !is_private(declaration) || has_attributes(declaration) {
        return;
    }
    let elements: Vec<SyntaxNode> = declaration
        .children()
        .filter(|child| child.kind() == PROPERTY_ELEMENT)
        .collect();
    for element in &elements {
        let Some(token) = first_token(element, VARIABLE) else {
            continue;
        };
        let name = token.text().trim_start_matches('$').to_string();
        if usage.property_used(&name) {
            continue;
        }
        let fix = if elements.len() == 1 {
            Fix::RemoveDeclaration {
                range: declaration.text_range(),
            }
        } else {
            Fix::None
        };
        cx.report(
            "unused-private-property",
            token.text_range(),
            format!("Private property '${name}' is never used"),
            fix,
        );
    }
}

fn check_promoted(cx: &Cx, method: &SyntaxNode, usage: &Usage) {
    if !cx.on("unused-private-property") {
        return;
    }
    let Some(name) = child_of(method, NAME) else {
        return;
    };
    if !text_of(&name).eq_ignore_ascii_case("__construct") {
        return;
    }
    let Some(list) = child_of(method, PARAMETER_LIST) else {
        return;
    };
    for parameter in list.children().filter(|child| child.kind() == PARAMETER) {
        if !is_private(&parameter) || has_attributes(&parameter) {
            continue;
        }
        let Some(token) = first_token(&parameter, VARIABLE) else {
            continue;
        };
        let name = token.text().trim_start_matches('$').to_string();
        if usage.property_used(&name) {
            continue;
        }
        cx.report(
            "unused-private-property",
            token.text_range(),
            format!("Private property '${name}' is never used"),
            Fix::None,
        );
    }
}

fn check_constant(cx: &Cx, declaration: &SyntaxNode, usage: &Usage) {
    if !cx.on("unused-private-constant") || !is_private(declaration) || has_attributes(declaration) {
        return;
    }
    let elements: Vec<SyntaxNode> = declaration
        .children()
        .filter(|child| child.kind() == CONST_ELEMENT)
        .collect();
    for element in &elements {
        let Some(name) = child_of(element, NAME) else {
            continue;
        };
        let written = text_of(&name);
        if usage.members.contains(&written.to_ascii_lowercase())
            || usage.strings.contains(&written.to_ascii_lowercase())
        {
            continue;
        }
        let fix = if elements.len() == 1 {
            Fix::RemoveDeclaration {
                range: declaration.text_range(),
            }
        } else {
            Fix::None
        };
        cx.report(
            "unused-private-constant",
            name.text_range(),
            format!("Private constant '{written}' is never used"),
            fix,
        );
    }
}
