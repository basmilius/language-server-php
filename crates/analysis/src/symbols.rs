use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxElement, SyntaxKind, SyntaxNode, TextRange};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SymbolKind {
    Namespace,
    Class,
    Interface,
    Trait,
    Enum,
    Method,
    Constructor,
    Property,
    Constant,
    EnumMember,
    Function,
}

/// A declaration in the outline of a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Symbol {
    pub name: String,
    /// The signature or the type: what a reader wants next to the name.
    pub detail: Option<String>,
    pub kind: SymbolKind,
    /// The whole declaration, with its doc comment.
    pub range: TextRange,
    /// The name, which lies inside `range`.
    pub selection_range: TextRange,
    pub deprecated: bool,
    pub children: Vec<Symbol>,
}

/// The outline of a file: namespaces, classes and the things in them, functions and constants.
pub fn document_symbols(root: &SyntaxNode) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    collect_statements(root, &mut symbols);
    symbols
}

/// Collects the declarations among the statements of a node. Conditional declarations, such as a
/// function behind `if (!function_exists(...))`, count as if they stood at this level.
fn collect_statements(container: &SyntaxNode, out: &mut Vec<Symbol>) {
    for child in container.children() {
        match child.kind() {
            NAMESPACE_DECLARATION => out.push(namespace(&child)),
            FUNCTION_DECLARATION => out.extend(function(&child)),
            CLASS_DECLARATION | INTERFACE_DECLARATION | TRAIT_DECLARATION | ENUM_DECLARATION => {
                out.extend(class_like(&child))
            }
            CONST_STATEMENT => out.extend(constants(&child)),
            IF_STATEMENT | ELSEIF_CLAUSE | ELSE_CLAUSE | BLOCK | STATEMENT_LIST | TRY_STATEMENT | CATCH_CLAUSE
            | FINALLY_CLAUSE => {
                collect_statements(&child, out);
            }
            _ => {}
        }
    }
}

fn name_node(node: &SyntaxNode) -> Option<SyntaxNode> {
    node.children().find(|child| child.kind() == NAME)
}

fn text_of(node: &SyntaxNode) -> String {
    node.text().to_string()
}

fn first_token_range(node: &SyntaxNode) -> TextRange {
    node.first_token().map_or(node.text_range(), |token| token.text_range())
}

/// Whether a doc comment in front of a declaration carries `@deprecated`.
fn is_deprecated(node: &SyntaxNode) -> bool {
    for element in node.children_with_tokens() {
        match element {
            SyntaxElement::Token(token) if token.kind() == DOC_COMMENT => return token.text().contains("@deprecated"),
            SyntaxElement::Token(token) if token.kind().is_trivia() => {}
            _ => return false,
        }
    }
    false
}

fn symbol(node: &SyntaxNode, name: &SyntaxNode, kind: SymbolKind, detail: Option<String>) -> Symbol {
    Symbol {
        name: text_of(name),
        detail,
        kind,
        range: node.text_range(),
        selection_range: name.text_range(),
        deprecated: is_deprecated(node),
        children: Vec::new(),
    }
}

fn namespace(node: &SyntaxNode) -> Symbol {
    let name = name_node(node);
    let mut children = Vec::new();
    collect_statements(node, &mut children);
    Symbol {
        name: name.as_ref().map_or_else(|| "(global)".to_string(), text_of),
        detail: None,
        kind: SymbolKind::Namespace,
        range: node.text_range(),
        selection_range: name.map_or_else(|| first_token_range(node), |name| name.text_range()),
        deprecated: false,
        children,
    }
}

/// `(int $a, ?string $b): string`, with the whitespace of the source collapsed.
fn signature(node: &SyntaxNode) -> String {
    let mut out = String::new();
    for kind in [PARAMETER_LIST, RETURN_TYPE] {
        if let Some(part) = node.children().find(|child| child.kind() == kind) {
            let text = text_of(&part);
            let collapsed: Vec<&str> = text.split_whitespace().collect();
            out.push_str(&collapsed.join(" "));
        }
    }
    out.replace("( ", "(").replace(" )", ")").replace(" ,", ",")
}

fn function(node: &SyntaxNode) -> Option<Symbol> {
    let name = name_node(node)?;
    Some(symbol(node, &name, SymbolKind::Function, Some(signature(node))))
}

fn constants(node: &SyntaxNode) -> Vec<Symbol> {
    node.children()
        .filter(|child| child.kind() == CONST_ELEMENT)
        .filter_map(|element| {
            let name = name_node(&element)?;
            let mut found = symbol(node, &name, SymbolKind::Constant, None);
            found.range = element.text_range();
            Some(found)
        })
        .collect()
}

fn class_like(node: &SyntaxNode) -> Option<Symbol> {
    let name = name_node(node)?;
    let kind = match node.kind() {
        INTERFACE_DECLARATION => SymbolKind::Interface,
        TRAIT_DECLARATION => SymbolKind::Trait,
        ENUM_DECLARATION => SymbolKind::Enum,
        _ => SymbolKind::Class,
    };
    let mut found = symbol(node, &name, kind, None);
    if let Some(body) = node.children().find(|child| child.kind() == CLASS_BODY) {
        found.children = members(&body);
    }
    Some(found)
}

fn members(body: &SyntaxNode) -> Vec<Symbol> {
    let mut out = Vec::new();
    for member in body.children() {
        match member.kind() {
            METHOD_DECLARATION => {
                let Some(name) = name_node(&member) else {
                    continue;
                };
                let is_constructor = text_of(&name).eq_ignore_ascii_case("__construct");
                let kind = if is_constructor {
                    SymbolKind::Constructor
                } else {
                    SymbolKind::Method
                };
                out.push(symbol(&member, &name, kind, Some(signature(&member))));
                if is_constructor {
                    out.extend(promoted_properties(&member));
                }
            }
            PROPERTY_DECLARATION => {
                let detail = property_type(&member);
                for element in member.children().filter(|child| child.kind() == PROPERTY_ELEMENT) {
                    let Some(variable) = element
                        .children_with_tokens()
                        .filter_map(SyntaxElement::into_token)
                        .find(|t| t.kind() == VARIABLE)
                    else {
                        continue;
                    };
                    out.push(Symbol {
                        name: variable.text().to_string(),
                        detail: detail.clone(),
                        kind: SymbolKind::Property,
                        range: member.text_range(),
                        selection_range: variable.text_range(),
                        deprecated: is_deprecated(&member),
                        children: Vec::new(),
                    });
                }
            }
            CLASS_CONST_DECLARATION => {
                for element in member.children().filter(|child| child.kind() == CONST_ELEMENT) {
                    if let Some(name) = name_node(&element) {
                        out.push(symbol(&member, &name, SymbolKind::Constant, property_type(&member)));
                    }
                }
            }
            ENUM_CASE => {
                if let Some(name) = name_node(&member) {
                    out.push(symbol(&member, &name, SymbolKind::EnumMember, None));
                }
            }
            _ => {}
        }
    }
    out
}

fn is_type(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        NAMED_TYPE | NULLABLE_TYPE | UNION_TYPE | INTERSECTION_TYPE | PAREN_TYPE
    )
}

fn property_type(member: &SyntaxNode) -> Option<String> {
    member
        .children()
        .find(|child| is_type(child.kind()))
        .map(|ty| text_of(&ty))
}

fn promoted_properties(constructor: &SyntaxNode) -> Vec<Symbol> {
    let Some(list) = constructor.children().find(|child| child.kind() == PARAMETER_LIST) else {
        return Vec::new();
    };
    list.children()
        .filter(|parameter| {
            parameter.kind() == PARAMETER && parameter.children().any(|child| child.kind() == MODIFIER_LIST)
        })
        .filter_map(|parameter| {
            let variable = parameter
                .children_with_tokens()
                .filter_map(SyntaxElement::into_token)
                .find(|t| t.kind() == VARIABLE)?;
            Some(Symbol {
                name: variable.text().to_string(),
                detail: property_type(&parameter),
                kind: SymbolKind::Property,
                range: parameter.text_range(),
                selection_range: variable.text_range(),
                deprecated: false,
                children: Vec::new(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::{Expect, expect};
    use php_syntax::parse;

    fn outline(symbols: &[Symbol], depth: usize, out: &mut String) {
        for symbol in symbols {
            let detail = symbol
                .detail
                .as_deref()
                .map(|detail| format!(" {detail}"))
                .unwrap_or_default();
            let tag = if symbol.deprecated { " (deprecated)" } else { "" };
            out.push_str(&format!(
                "{}{:?} {}{detail}{tag}\n",
                "  ".repeat(depth),
                symbol.kind,
                symbol.name
            ));
            outline(&symbol.children, depth + 1, out);
        }
    }

    fn check(text: &str, expect: Expect) {
        let parsed = parse(text);
        let mut out = String::new();
        outline(&document_symbols(&parsed.syntax()), 0, &mut out);
        expect.assert_eq(&out);
    }

    #[test]
    fn lists_namespaces_classes_and_their_members() {
        check(
            r#"<?php
namespace App\Models;

const VERSION = '1';

/** @deprecated use B */
function old(int $a, ?string $b = null): string {}

if (!function_exists('polyfill')) {
    function polyfill() {}
}

interface HasName { public function name(): string; }

final class User extends Base {
    public const int MAX = 1;
    public ?int $age = null;
    public string $a, $b;
    public function __construct(private readonly string $name, int $plain) {}
    public static function make(): static {}
}

enum Suit: string { case Hearts = 'h'; case Spades = 's'; public function label(): string {} }
trait Stamps { public $at; }
"#,
            expect![[r#"
                Namespace App\Models
                  Constant VERSION
                  Function old (int $a, ?string $b = null): string (deprecated)
                  Function polyfill ()
                  Interface HasName
                    Method name (): string
                  Class User
                    Constant MAX int
                    Property $age ?int
                    Property $a string
                    Property $b string
                    Constructor __construct (private readonly string $name, int $plain)
                    Property $name string
                    Method make (): static
                  Enum Suit
                    EnumMember Hearts
                    EnumMember Spades
                    Method label (): string
                  Trait Stamps
                    Property $at
            "#]],
        );
    }

    #[test]
    fn braced_and_global_namespaces() {
        check(
            "<?php namespace A { class X {} } namespace { function f() {} }",
            expect![[r#"
                Namespace A
                  Class X
                Namespace (global)
                  Function f ()
            "#]],
        );
    }
}
