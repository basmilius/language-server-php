//! The strings PHPUnit reads names from: data providers and dependencies in attributes and in
//! `@dataProvider` and `@depends` tags, and the names of groups.

use php_index::test_facts::{class_constant, string_value};
use php_index::{Index, Name, Type};
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, SyntaxToken, TextRange};

use super::ATTRIBUTES;
use crate::ast::range_of;
use crate::context::FileContext;
use crate::infer::{Analyzer, arguments};
use crate::refs::Symbol;

/// What a method named in a string is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    DataProvider,
    Depends,
    Covers,
    /// A method of the class a test double stands for.
    Mocked,
}

/// What a string names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StringTarget {
    /// A method of `class`, the class the test is in or the one an attribute names.
    Method {
        class: Name,
        role: Role,
    },
    Function,
    Group,
    /// A Pest dataset.
    Dataset,
}

/// A string, or a word of a doc comment, that names something.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TestString {
    /// The text without its quotes.
    pub range: TextRange,
    pub value: String,
    pub target: StringTarget,
    /// The string is where the name is declared, as the name of a dataset is.
    pub declaration: bool,
}

struct Rule {
    attribute: &'static str,
    string_argument: (usize, &'static str),
    /// Where the class the method belongs to is named.
    class_argument: Option<(usize, &'static str)>,
    kind: RuleKind,
}

#[derive(Clone, Copy)]
enum RuleKind {
    Method(Role),
    Function,
    Group,
}

const METHOD: (usize, &str) = (0, "methodName");
const EXTERNAL_METHOD: (usize, &str) = (1, "methodName");
const CLASS: Option<(usize, &str)> = Some((0, "className"));

const RULES: &[Rule] = &[
    Rule {
        attribute: "DataProvider",
        string_argument: METHOD,
        class_argument: None,
        kind: RuleKind::Method(Role::DataProvider),
    },
    Rule {
        attribute: "DataProviderExternal",
        string_argument: EXTERNAL_METHOD,
        class_argument: CLASS,
        kind: RuleKind::Method(Role::DataProvider),
    },
    Rule {
        attribute: "Depends",
        string_argument: METHOD,
        class_argument: None,
        kind: RuleKind::Method(Role::Depends),
    },
    Rule {
        attribute: "DependsUsingDeepClone",
        string_argument: METHOD,
        class_argument: None,
        kind: RuleKind::Method(Role::Depends),
    },
    Rule {
        attribute: "DependsUsingShallowClone",
        string_argument: METHOD,
        class_argument: None,
        kind: RuleKind::Method(Role::Depends),
    },
    Rule {
        attribute: "DependsExternal",
        string_argument: EXTERNAL_METHOD,
        class_argument: CLASS,
        kind: RuleKind::Method(Role::Depends),
    },
    Rule {
        attribute: "DependsExternalUsingDeepClone",
        string_argument: EXTERNAL_METHOD,
        class_argument: CLASS,
        kind: RuleKind::Method(Role::Depends),
    },
    Rule {
        attribute: "DependsExternalUsingShallowClone",
        string_argument: EXTERNAL_METHOD,
        class_argument: CLASS,
        kind: RuleKind::Method(Role::Depends),
    },
    Rule {
        attribute: "CoversMethod",
        string_argument: EXTERNAL_METHOD,
        class_argument: CLASS,
        kind: RuleKind::Method(Role::Covers),
    },
    Rule {
        attribute: "UsesMethod",
        string_argument: EXTERNAL_METHOD,
        class_argument: CLASS,
        kind: RuleKind::Method(Role::Covers),
    },
    Rule {
        attribute: "CoversFunction",
        string_argument: (0, "functionName"),
        class_argument: None,
        kind: RuleKind::Function,
    },
    Rule {
        attribute: "UsesFunction",
        string_argument: (0, "functionName"),
        class_argument: None,
        kind: RuleKind::Function,
    },
    Rule {
        attribute: "Group",
        string_argument: (0, "name"),
        class_argument: None,
        kind: RuleKind::Group,
    },
];

/// The strings of an attribute that name something.
fn attribute_strings(analyzer: &Analyzer<'_>, attribute: &SyntaxNode) -> Vec<TestString> {
    let Some(name) = attribute.children().find(|child| child.kind() == NAME) else {
        return Vec::new();
    };
    let resolved = analyzer.resolver.resolve_class(&name.text().to_string());
    let Some(short) = resolved.strip_prefix(ATTRIBUTES) else {
        return Vec::new();
    };
    let Some(rule) = RULES.iter().find(|rule| rule.attribute.eq_ignore_ascii_case(short)) else {
        return Vec::new();
    };
    let args = arguments(attribute);
    let argument = |(position, name): (usize, &str)| {
        args.iter()
            .find(|arg| arg.name.as_deref() == Some(name))
            .or_else(|| args.get(position).filter(|arg| arg.name.is_none()))
            .and_then(|arg| arg.expr.clone())
    };
    let Some((value, span)) = argument(rule.string_argument).as_ref().and_then(string_value) else {
        return Vec::new();
    };
    let target = match rule.kind {
        RuleKind::Method(role) => {
            let class = match rule.class_argument {
                Some(position) => argument(position).and_then(|expr| class_constant(&expr, &analyzer.resolver)),
                None => analyzer.class.as_ref().map(|class| class.name.clone()),
            };
            let Some(class) = class else {
                return Vec::new();
            };
            StringTarget::Method { class, role }
        }
        RuleKind::Function => StringTarget::Function,
        RuleKind::Group => StringTarget::Group,
    };
    vec![TestString {
        range: range_of(span.start, span.end),
        value,
        target,
        declaration: false,
    }]
}

/// The names `@dataProvider`, `@depends` and `@group` give in a doc comment.
fn doc_strings(analyzer: &Analyzer<'_>, token: &SyntaxToken) -> Vec<TestString> {
    let text = token.text();
    if !text.contains("@dataProvider") && !text.contains("@depends") && !text.contains("@group") {
        return Vec::new();
    }
    let base = u32::from(token.text_range().start());
    let mut out = Vec::new();
    let mut line_start = 0;
    for line in text.split_inclusive('\n') {
        let offset_in_text = line_start;
        line_start += line.len();
        let trimmed = line.trim_start_matches(|c: char| c.is_whitespace() || c == '*' || c == '/');
        let skipped = line.len() - trimmed.len();
        let Some(tag_end) = trimmed.find(|c: char| c.is_whitespace()) else {
            continue;
        };
        let (tag, rest) = trimmed.split_at(tag_end);
        let role = match tag {
            "@dataProvider" => Role::DataProvider,
            "@depends" => Role::Depends,
            "@group" => {
                if let Some(word) = rest.split_whitespace().next() {
                    let at = rest.find(word).unwrap_or(0);
                    let from = base + (offset_in_text + skipped + tag_end + at) as u32;
                    out.push(TestString {
                        range: range_of(from, from + word.len() as u32),
                        value: word.to_string(),
                        target: StringTarget::Group,
                        declaration: false,
                    });
                }
                continue;
            }
            _ => continue,
        };
        let mut words = rest.split_whitespace().peekable();
        let mut cursor = 0;
        let mut word = None;
        for candidate in words.by_ref() {
            let at = rest[cursor..].find(candidate).map_or(cursor, |found| cursor + found);
            cursor = at + candidate.len();
            if matches!(candidate, "clone" | "!clone") {
                continue;
            }
            if candidate != "*/" {
                word = Some((candidate, at));
            }
            break;
        }
        let Some((word, at)) = word else {
            let from = base + (offset_in_text + skipped + line_end_of_tag(trimmed, tag_end)) as u32;
            out.push(TestString {
                range: range_of(from, from),
                value: String::new(),
                target: StringTarget::Method {
                    class: analyzer
                        .class
                        .as_ref()
                        .map(|class| class.name.clone())
                        .unwrap_or_default(),
                    role,
                },
                declaration: false,
            });
            continue;
        };
        let (class, method, method_at) = match word.split_once("::") {
            Some((class, method)) => (
                Some(analyzer.resolver.resolve_class(class)),
                method,
                at + class.len() + 2,
            ),
            None => (analyzer.class.as_ref().map(|class| class.name.clone()), word, at),
        };
        let Some(class) = class else {
            continue;
        };
        let from = base + (offset_in_text + skipped + tag_end + method_at) as u32;
        out.push(TestString {
            range: range_of(from, from + method.len() as u32),
            value: method.to_string(),
            target: StringTarget::Method { class, role },
            declaration: false,
        });
    }
    out
}

/// Where the text after a tag would start: past the space that follows it.
fn line_end_of_tag(trimmed: &str, tag_end: usize) -> usize {
    tag_end + usize::from(trimmed[tag_end..].starts_with([' ', '\t']))
}

/// The strings of a call that name something: the methods of a double, a Pest dataset.
fn call_strings(analyzer: &Analyzer<'_>, call: &SyntaxNode) -> Vec<TestString> {
    let mut out = super::mocks::call_strings(analyzer, call);
    out.extend(crate::pest::call_strings(analyzer, call));
    out
}

/// Every string of a file that names a test, a function or a group.
pub fn strings_in(ctx: &FileContext<'_>) -> Vec<TestString> {
    let mut out = Vec::new();
    for element in ctx.root.descendants_with_tokens() {
        match element {
            php_syntax::SyntaxElement::Node(node) if node.kind() == ATTRIBUTE => {
                out.extend(attribute_strings(&ctx.analyzer(&node), &node));
            }
            php_syntax::SyntaxElement::Token(token) if token.kind() == DOC_COMMENT => {
                if let Some(parent) = token.parent() {
                    out.extend(doc_strings(&ctx.analyzer(&parent), &token));
                }
            }
            php_syntax::SyntaxElement::Node(node) if node.kind() == CALL_EXPR => {
                out.extend(call_strings(&ctx.analyzer(&node), &node));
            }
            _ => {}
        }
    }
    out
}

/// The string around an offset, with the end of the text inside the quotes counting as in it.
pub fn string_at(analyzer: &Analyzer<'_>, offset: u32) -> Option<TestString> {
    let tokens = match analyzer.root.token_at_offset(php_syntax::TextSize::from(offset)) {
        php_syntax::TokenAtOffset::None => return None,
        php_syntax::TokenAtOffset::Single(token) => vec![token],
        php_syntax::TokenAtOffset::Between(left, right) => vec![right, left],
    };
    for token in tokens {
        let found = match token.kind() {
            STRING_LITERAL => {
                let literal = token.parent().and_then(|literal| literal.parent());
                let owner = literal.as_ref().and_then(|argument| {
                    argument
                        .ancestors()
                        .take(6)
                        .find(|ancestor| matches!(ancestor.kind(), ATTRIBUTE | CALL_EXPR))
                });
                match owner {
                    Some(owner) if owner.kind() == ATTRIBUTE => attribute_strings(analyzer, &owner),
                    Some(owner) => call_strings(analyzer, &owner),
                    None => Vec::new(),
                }
            }
            DOC_COMMENT => doc_strings(analyzer, &token),
            _ => Vec::new(),
        };
        let at = |string: &TestString| start_of(string) <= offset && offset <= end_of(string);
        if let Some(string) = found.into_iter().find(at) {
            return Some(string);
        }
    }
    None
}

fn start_of(string: &TestString) -> u32 {
    u32::from(string.range.start())
}

fn end_of(string: &TestString) -> u32 {
    u32::from(string.range.end())
}

/// The class and the method a method string stands for, as the class that declares it names them.
pub fn method_of(index: &Index, string: &TestString) -> Option<(Name, String)> {
    let StringTarget::Method { class, .. } = &string.target else {
        return None;
    };
    let found = index.find_method(&Type::class(class.clone()), &string.value)?;
    Some((found.class.decl.name.clone(), found.member.name.clone()))
}

/// The symbol a string stands for, when it names one a rename and find usages can follow.
pub fn symbol_of(index: &Index, string: &TestString, analyzer: &Analyzer<'_>) -> Option<Symbol> {
    match &string.target {
        StringTarget::Method { .. } => {
            let (class, name) = method_of(index, string)?;
            Some(Symbol::Method { class, name })
        }
        StringTarget::Function => {
            let candidates = analyzer.resolver.function_candidates(&string.value);
            Some(Symbol::Function(index.first_function(&candidates)?.decl.name.clone()))
        }
        StringTarget::Dataset => Some(Symbol::Dataset(string.value.clone())),
        StringTarget::Group => None,
    }
}

/// The symbols under an offset that is in such a string, with the range of the name in it.
pub fn symbols_at_string(ctx: &FileContext<'_>, offset: u32) -> Option<(TextRange, Vec<Symbol>)> {
    let node = crate::ast::node_at(&ctx.root, offset);
    let analyzer = ctx.analyzer(&node);
    let string = string_at(&analyzer, offset)?;
    let symbol = symbol_of(ctx.index, &string, &analyzer)?;
    Some((string.range, vec![symbol]))
}
