//! What the name or variable under a position refers to.

use php_index::{Name, Type};
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, SyntaxToken, TextRange, TokenAtOffset};

use crate::ast::{child_of, has_token, start, text_of};
use crate::infer::Analyzer;

/// A thing a position can refer to.
#[derive(Clone, Debug, PartialEq)]
pub enum Target {
    Class(Name),
    Function(Name),
    Constant(Name),
    /// A method on the classes a type stands for.
    Method {
        receiver: Type,
        name: String,
    },
    Property {
        receiver: Type,
        name: String,
    },
    ClassConst {
        receiver: Type,
        name: String,
    },
    /// A local variable or parameter, with the type it has there.
    Variable {
        name: String,
        ty: Type,
    },
    /// A parameter named by an argument of a call.
    Parameter {
        callee: Callee,
        name: String,
    },
    /// A Pest dataset, by the name it is declared and used under.
    Dataset(String),
    /// A string that names a config key, a route, a view and the like.
    Key {
        kind: php_index::framework::keys::KeyKind,
        name: String,
        scope: Option<String>,
    },
}

/// A function or method as the declaration that owns it names it: a method by the class that
/// declares it, not the class it was called on.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Callee {
    Function(Name),
    Method { class: Name, name: String },
}

/// A target with the range of the token it was found on.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub target: Target,
    pub range: TextRange,
}

/// The token a navigation request is about: the one the offset is in or at the end of.
pub fn token_at(root: &SyntaxNode, offset: u32) -> Option<SyntaxToken> {
    let usable = |token: &SyntaxToken| {
        !token.kind().is_trivia()
            && (token.kind() == VARIABLE
                || token.parent().is_some_and(|parent| parent.kind() == NAME)
                || matches!(token.kind(), IDENT))
    };
    match root.token_at_offset(php_syntax::TextSize::from(offset)) {
        TokenAtOffset::None => None,
        TokenAtOffset::Single(token) => usable(&token).then_some(token),
        TokenAtOffset::Between(left, right) => {
            if usable(&right) {
                Some(right)
            } else if usable(&left) {
                Some(left)
            } else {
                None
            }
        }
    }
}

impl Analyzer<'_> {
    /// The targets under an offset. A member of a union has one per class that has it.
    pub fn targets_at(&self, offset: u32) -> Vec<Found> {
        if let Some(found) = self.test_string_target(offset) {
            return vec![found];
        }
        if let Some(key) = crate::frameworks::keys::key_at(self, offset) {
            return vec![Found {
                target: Target::Key {
                    kind: key.kind,
                    name: key.value,
                    scope: key.scope,
                },
                range: key.range,
            }];
        }
        let Some(token) = token_at(&self.root, offset) else {
            return Vec::new();
        };
        let range = token.text_range();
        self.targets_of_token(&token)
            .into_iter()
            .map(|target| Found { target, range })
            .collect()
    }

    /// The method or function a string of a test names, with the range of the text in the quotes.
    fn test_string_target(&self, offset: u32) -> Option<Found> {
        let string = crate::phpunit::strings::string_at(self, offset)?;
        let target = match &string.target {
            crate::phpunit::strings::StringTarget::Method { class, .. } => {
                let receiver = Type::class(class.clone());
                self.index.find_method(&receiver, &string.value)?;
                Target::Method {
                    receiver,
                    name: string.value.clone(),
                }
            }
            crate::phpunit::strings::StringTarget::Function => {
                let candidates = self.resolver.function_candidates(&string.value);
                Target::Function(self.index.first_function(&candidates)?.decl.name.clone())
            }
            crate::phpunit::strings::StringTarget::Dataset => Target::Dataset(string.value.clone()),
            crate::phpunit::strings::StringTarget::Group => return None,
        };
        Some(Found {
            target,
            range: string.range,
        })
    }

    /// The targets a token stands for.
    pub fn targets_of_token(&self, token: &SyntaxToken) -> Vec<Target> {
        if token.kind() == VARIABLE {
            self.variable_targets(token)
        } else if token.kind() == IDENT && token.parent().is_some_and(|parent| parent.kind() == ARGUMENT) {
            self.named_argument_targets(token)
        } else {
            self.name_targets(token)
        }
    }

    fn named_argument_targets(&self, token: &SyntaxToken) -> Vec<Target> {
        let Some(argument) = token.parent() else {
            return Vec::new();
        };
        let named = crate::ast::tokens(&argument)
            .find(|candidate| !candidate.kind().is_trivia())
            .is_some_and(|first| &first == token)
            && has_token(&argument, COLON);
        let Some(call) = argument.parent().and_then(|list| list.parent()) else {
            return Vec::new();
        };
        if !named || !matches!(call.kind(), CALL_EXPR | NEW_EXPR) {
            return Vec::new();
        }
        let env = self.env_around(&call);
        let name = token.text().to_string();
        let mut out: Vec<Target> = Vec::new();
        for callee in self.callees(&call, &env) {
            let callee = match callee.name.rsplit_once("::") {
                Some((class, method)) => Callee::Method {
                    class: class.to_string(),
                    name: method.to_string(),
                },
                None if callee.name == "closure" => continue,
                None => Callee::Function(callee.name.clone()),
            };
            let target = Target::Parameter {
                callee,
                name: name.clone(),
            };
            if !out.contains(&target) {
                out.push(target);
            }
        }
        out
    }

    fn variable_targets(&self, token: &SyntaxToken) -> Vec<Target> {
        let Some(parent) = token.parent() else {
            return Vec::new();
        };
        let name = token.text().trim_start_matches('$').to_string();
        match parent.kind() {
            VARIABLE_EXPR => {
                if let Some(grand) = parent.parent().filter(|grand| grand.kind() == STATIC_PROPERTY_EXPR) {
                    let qualifier = grand.children().next();
                    if qualifier.as_ref() != Some(&parent) {
                        if let Some(qualifier) = qualifier {
                            let env = self.env_around(&grand);
                            return vec![Target::Property {
                                receiver: self.receiver_type(&self.qualifier_type(&qualifier, &env)),
                                name,
                            }];
                        }
                    }
                }
                if name == "this" {
                    return self
                        .class
                        .as_ref()
                        .map(|class| Target::Class(class.name.clone()))
                        .into_iter()
                        .collect();
                }
                let env = self.env_around(&parent);
                let ty = env.get(&name).cloned().unwrap_or(Type::Unknown);
                vec![Target::Variable { name, ty }]
            }
            PROPERTY_ELEMENT => vec![Target::Property {
                receiver: self.this_type(),
                name,
            }],
            PARAMETER | CLOSURE_USE_VARIABLE | STATIC_VARIABLE => {
                let env = self.env_at(start(&parent) + 1);
                let ty = env.get(&name).cloned().unwrap_or(Type::Unknown);
                vec![Target::Variable { name, ty }]
            }
            _ => Vec::new(),
        }
    }

    fn name_targets(&self, token: &SyntaxToken) -> Vec<Target> {
        let Some(name_node) = token.parent().filter(|parent| parent.kind() == NAME) else {
            return Vec::new();
        };
        let Some(parent) = name_node.parent() else {
            return Vec::new();
        };
        let text = text_of(&name_node);
        let is_first_child = parent.children().next().as_ref() == Some(&name_node);
        match parent.kind() {
            CLASS_DECLARATION | INTERFACE_DECLARATION | TRAIT_DECLARATION | ENUM_DECLARATION => {
                vec![Target::Class(self.resolver.qualify(&text))]
            }
            FUNCTION_DECLARATION => vec![Target::Function(self.resolver.qualify(&text))],
            METHOD_DECLARATION => vec![Target::Method {
                receiver: self.this_type(),
                name: text,
            }],
            ENUM_CASE => vec![Target::ClassConst {
                receiver: self.this_type(),
                name: text,
            }],
            CONST_ELEMENT => {
                let in_class = parent
                    .parent()
                    .is_some_and(|grand| grand.kind() == CLASS_CONST_DECLARATION);
                if in_class {
                    vec![Target::ClassConst {
                        receiver: self.this_type(),
                        name: text,
                    }]
                } else {
                    vec![Target::Constant(self.resolver.qualify(&text))]
                }
            }
            NAMED_TYPE | NEW_EXPR | ATTRIBUTE | TRAIT_USE => self.class_targets(&text),
            TRAIT_PRECEDENCE | TRAIT_ALIAS => self.adaptation_targets(&name_node, &text),
            BINARY_EXPR => {
                if significant_sibling(&name_node, true) == Some(INSTANCEOF_KW) {
                    self.class_targets(&text)
                } else {
                    self.expression_name_targets(&text)
                }
            }
            PROPERTY_FETCH_EXPR => {
                if is_first_child {
                    return self.expression_name_targets(&text);
                }
                let Some(object) = parent.children().next() else {
                    return Vec::new();
                };
                let env = self.env_around(&parent);
                let receiver = self.receiver_type(&self.type_of(&object, &env));
                let is_call = parent.parent().is_some_and(|grand| {
                    grand.kind() == CALL_EXPR && grand.children().next().as_ref() == Some(&parent)
                });
                if is_call {
                    vec![Target::Method { receiver, name: text }]
                } else {
                    vec![Target::Property { receiver, name: text }]
                }
            }
            SCOPED_ACCESS_EXPR => {
                if is_first_child {
                    return self.class_targets(&text);
                }
                let Some(qualifier) = parent.children().next() else {
                    return Vec::new();
                };
                let env = self.env_around(&parent);
                let receiver = self.receiver_type(&self.qualifier_type(&qualifier, &env));
                let is_call = parent.parent().is_some_and(|grand| {
                    grand.kind() == CALL_EXPR && grand.children().next().as_ref() == Some(&parent)
                });
                if is_call {
                    vec![Target::Method { receiver, name: text }]
                } else if text == "class" {
                    Vec::new()
                } else {
                    vec![Target::ClassConst { receiver, name: text }]
                }
            }
            STATIC_PROPERTY_EXPR => self.class_targets(&text),
            USE_CLAUSE => {
                if is_alias_name(&name_node) {
                    return Vec::new();
                }
                self.use_targets(&parent, &text)
            }
            USE_GROUP => Vec::new(),
            CALL_EXPR => {
                if is_first_child {
                    let candidates = self.resolver.function_candidates(&text);
                    return match self.index.first_function(&candidates) {
                        Some(function) => vec![Target::Function(function.decl.name.clone())],
                        None => Vec::new(),
                    };
                }
                self.expression_name_targets(&text)
            }
            EXTENDS_CLAUSE | IMPLEMENTS_CLAUSE => self.class_targets(&text),
            _ => self.expression_name_targets(&text),
        }
    }

    /// A name inside `use T { a as b; T::a insteadof U; }`: the trait and the class it takes a method
    /// from are classes, the method before `as` or after `::` is a method of the trait.
    fn adaptation_targets(&self, name: &SyntaxNode, text: &str) -> Vec<Target> {
        let previous = significant_sibling(name, true);
        let next = significant_sibling(name, false);
        if next == Some(DOUBLE_COLON) {
            return self.class_targets(text);
        }
        if previous == Some(DOUBLE_COLON) {
            let qualifier = name
                .prev_sibling()
                .filter(|node| node.kind() == NAME)
                .map(|node| text_of(&node));
            let receiver = match qualifier {
                Some(qualifier) => self.class_type(&qualifier),
                None => self.this_type(),
            };
            return vec![Target::Method {
                receiver,
                name: text.to_string(),
            }];
        }
        if follows_token(name, AS_KW) {
            return Vec::new();
        }
        match previous {
            Some(INSTEADOF_KW | COMMA) => self.class_targets(text),
            _ => vec![Target::Method {
                receiver: self.this_type(),
                name: text.to_string(),
            }],
        }
    }

    fn class_targets(&self, text: &str) -> Vec<Target> {
        match self.class_type(text) {
            Type::Class { name, .. } => vec![Target::Class(name)],
            Type::Static => self
                .class
                .as_ref()
                .map(|class| Target::Class(class.name.clone()))
                .into_iter()
                .collect(),
            _ => Vec::new(),
        }
    }

    fn expression_name_targets(&self, text: &str) -> Vec<Target> {
        if matches!(text.to_ascii_lowercase().as_str(), "true" | "false" | "null") {
            return Vec::new();
        }
        let candidates = self.resolver.constant_candidates(text);
        match self.index.first_constant(&candidates) {
            Some(constant) => vec![Target::Constant(constant.decl.name.clone())],
            None => Vec::new(),
        }
    }

    fn use_targets(&self, clause: &SyntaxNode, text: &str) -> Vec<Target> {
        let statement = clause.ancestors().find(|node| node.kind() == USE_STATEMENT);
        let kind_token = |node: &SyntaxNode| {
            if has_token(node, FUNCTION_KW) {
                Some(FUNCTION_KW)
            } else if has_token(node, CONST_KW) {
                Some(CONST_KW)
            } else {
                None
            }
        };
        let kind = kind_token(clause).or_else(|| statement.as_ref().and_then(kind_token));
        let mut full = text.trim_start_matches('\\').to_string();
        if let Some(group) = clause.parent().filter(|parent| parent.kind() == USE_GROUP) {
            if let Some(prefix) = child_of(&group, NAME) {
                full = format!("{}\\{}", text_of(&prefix).trim_end_matches('\\'), full);
            }
        }
        match kind {
            Some(FUNCTION_KW) => vec![Target::Function(full)],
            Some(CONST_KW) => vec![Target::Constant(full)],
            _ => vec![Target::Class(full)],
        }
    }
}

/// The kind of the token or node right before or after one, skipping trivia.
fn significant_sibling(node: &SyntaxNode, before: bool) -> Option<php_syntax::SyntaxKind> {
    let mut current = if before {
        node.prev_sibling_or_token()
    } else {
        node.next_sibling_or_token()
    };
    while let Some(element) = current {
        if !element.kind().is_trivia() {
            return Some(element.kind());
        }
        current = if before {
            element.prev_sibling_or_token()
        } else {
            element.next_sibling_or_token()
        };
    }
    None
}

fn follows_token(node: &SyntaxNode, kind: php_syntax::SyntaxKind) -> bool {
    let mut current = node.prev_sibling_or_token();
    while let Some(element) = current {
        if element.kind() == kind {
            return true;
        }
        current = element.prev_sibling_or_token();
    }
    false
}

/// The second name of `use Foo\Bar as Baz`, which declares an alias and refers to nothing.
fn is_alias_name(name: &SyntaxNode) -> bool {
    significant_sibling(name, true) == Some(AS_KW)
}
