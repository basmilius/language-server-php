//! Language levels: which PHP version a file is written for, a table of the syntax each version
//! brought, deprecated or removed, and a pass over a tree that reports what the level does not allow.
//!
//! The lexer and the parser accept the union of every version and never look at a level. Supporting a
//! new PHP version means adding rows to [`FEATURES`] (and the syntax to the parser, when there is any).

use std::fmt;
use std::sync::OnceLock;

use rowan::TextRange;

use crate::SyntaxKind::{self, *};
use crate::{SyntaxElement, SyntaxNode, SyntaxToken};

/// A PHP version, as far as syntax cares: major and minor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PhpVersion {
    pub major: u8,
    pub minor: u8,
}

impl PhpVersion {
    pub const V7_4: PhpVersion = PhpVersion::new(7, 4);
    pub const V8_0: PhpVersion = PhpVersion::new(8, 0);
    pub const V8_1: PhpVersion = PhpVersion::new(8, 1);
    pub const V8_2: PhpVersion = PhpVersion::new(8, 2);
    pub const V8_3: PhpVersion = PhpVersion::new(8, 3);
    pub const V8_4: PhpVersion = PhpVersion::new(8, 4);
    pub const V8_5: PhpVersion = PhpVersion::new(8, 5);
    pub const V8_6: PhpVersion = PhpVersion::new(8, 6);

    /// The newest version the parser knows.
    pub const LATEST: PhpVersion = PhpVersion::V8_6;

    /// The level of a file without a configured one: the newest release. PHP 8.6 is a release
    /// candidate until November 2026, after which this follows `LATEST` again.
    pub const DEFAULT: PhpVersion = PhpVersion::V8_5;

    pub const fn new(major: u8, minor: u8) -> PhpVersion {
        PhpVersion { major, minor }
    }

    /// Reads `8.4`, `8.4.2`, `8` or a constraint prefix such as `^8.1` or `>=8.1`. A bare major
    /// version means its first release.
    pub fn parse(text: &str) -> Option<PhpVersion> {
        let digits = text.trim().trim_start_matches(|c: char| !c.is_ascii_digit());
        let mut parts = digits.split('.');
        let major = parts.next()?.parse().ok()?;
        let minor = match parts.next() {
            Some(minor) => minor.trim_end_matches(|c: char| !c.is_ascii_digit()).parse().ok()?,
            None => 0,
        };
        Some(PhpVersion { major, minor })
    }

    /// The release before this one, which is what a test needs to see a feature absent.
    pub fn previous(self) -> PhpVersion {
        match (self.major, self.minor) {
            (8, 0) => PhpVersion::V7_4,
            (major, 0) => PhpVersion::new(major.saturating_sub(1), 0),
            (major, minor) => PhpVersion::new(major, minor - 1),
        }
    }
}

impl fmt::Display for PhpVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// One piece of syntax with the versions that changed its status.
pub struct Feature {
    /// A stable identifier, for tests and for a client that wants to switch a check off.
    pub id: &'static str,
    /// What the feature is called in a message, as a subject: "Property hooks".
    pub name: &'static str,
    /// Whether `name` is plural, which picks "are" over "is".
    pub plural: bool,
    /// The first version that has it; `None` when it is older than any level that matters.
    pub since: Option<PhpVersion>,
    /// The version that deprecated it.
    pub deprecated: Option<PhpVersion>,
    /// The version that removed it.
    pub removed: Option<PhpVersion>,
    /// The kinds of element `detect` wants to see.
    pub kinds: &'static [SyntaxKind],
    /// The range of the construct when the element is an instance of the feature.
    pub detect: fn(&SyntaxElement) -> Option<TextRange>,
    /// A snippet that uses the feature, which a test parses and checks the row against.
    pub example: &'static str,
}

/// How serious a finding is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LevelSeverity {
    Error,
    Warning,
}

/// A feature a file uses that its language level does not allow, or one that level deprecates.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LevelDiagnostic {
    pub range: TextRange,
    pub message: String,
    pub severity: LevelSeverity,
    /// Set for a deprecation, which a client may draw struck through.
    pub deprecated: bool,
    pub feature: &'static str,
}

/// Reports what `level` does not allow in the tree: syntax from a newer version as errors, syntax
/// removed by then as errors, and deprecated syntax as warnings.
pub fn check_language_level(root: &SyntaxNode, level: PhpVersion) -> Vec<LevelDiagnostic> {
    let index = dispatch_index();
    let mut found = Vec::new();
    for element in root.descendants_with_tokens() {
        let Some(candidates) = index.get(element.kind() as usize) else {
            continue;
        };
        for &row in candidates {
            let feature = &FEATURES[row];
            let Some(range) = (feature.detect)(&element) else {
                continue;
            };
            if let Some(diagnostic) = judge(feature, range, level) {
                found.push(diagnostic);
            }
        }
    }
    found.sort_by_key(|diagnostic| (diagnostic.range.start(), diagnostic.range.end()));
    found
}

fn judge(feature: &'static Feature, range: TextRange, level: PhpVersion) -> Option<LevelDiagnostic> {
    let verb = |plural: &'static str, singular: &'static str| if feature.plural { plural } else { singular };
    let name = feature.name;
    let diagnostic = |message: String, severity: LevelSeverity, deprecated: bool| {
        Some(LevelDiagnostic {
            range,
            message,
            severity,
            deprecated,
            feature: feature.id,
        })
    };
    if let Some(since) = feature.since.filter(|since| level < *since) {
        return diagnostic(
            format!("{name} {} only available since PHP {since}", verb("are", "is")),
            LevelSeverity::Error,
            false,
        );
    }
    if let Some(removed) = feature.removed.filter(|removed| level >= *removed) {
        return diagnostic(
            format!("{name} {} removed in PHP {removed}", verb("were", "was")),
            LevelSeverity::Error,
            false,
        );
    }
    let since = feature.deprecated.filter(|since| level >= *since)?;
    diagnostic(
        format!("{name} {} deprecated since PHP {since}", verb("are", "is")),
        LevelSeverity::Warning,
        true,
    )
}

fn dispatch_index() -> &'static Vec<Vec<usize>> {
    static INDEX: OnceLock<Vec<Vec<usize>>> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut index = vec![Vec::new(); SyntaxKind::ALL.len()];
        for (row, feature) in FEATURES.iter().enumerate() {
            for kind in feature.kinds {
                index[*kind as usize].push(row);
            }
        }
        index
    })
}

fn node_of(element: &SyntaxElement) -> Option<&SyntaxNode> {
    element.as_node()
}

fn token_of(element: &SyntaxElement) -> Option<&SyntaxToken> {
    element.as_token()
}

fn range_of_token(element: &SyntaxElement) -> Option<TextRange> {
    token_of(element).map(SyntaxToken::text_range)
}

fn range_of_node(element: &SyntaxElement) -> Option<TextRange> {
    node_of(element).map(SyntaxNode::text_range)
}

fn first_token(node: &SyntaxNode) -> Option<SyntaxToken> {
    node.children_with_tokens()
        .filter_map(SyntaxElement::into_token)
        .find(|token| !token.kind().is_trivia())
}

fn token_child(node: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxToken> {
    node.children_with_tokens()
        .filter_map(SyntaxElement::into_token)
        .find(|token| token.kind() == kind)
}

fn node_child(node: &SyntaxNode, kind: SyntaxKind) -> Option<SyntaxNode> {
    node.children().find(|child| child.kind() == kind)
}

fn parent_kind(node: &SyntaxNode) -> Option<SyntaxKind> {
    node.parent().map(|parent| parent.kind())
}

fn modifier_list_with(element: &SyntaxElement, token: SyntaxKind, parents: &[SyntaxKind]) -> Option<TextRange> {
    let list = node_of(element)?;
    if !parents.contains(&parent_kind(list)?) {
        return None;
    }
    token_child(list, token).map(|found| found.text_range())
}

/// Whether the node sits where PHP wants a constant expression: the default of a parameter or a
/// property, a constant, a static variable, an enum case value or the arguments of an attribute.
fn in_constant_expression(node: &SyntaxNode) -> bool {
    for ancestor in node.ancestors().skip(1) {
        match ancestor.kind() {
            PARAMETER | PROPERTY_ELEMENT | CONST_ELEMENT | STATIC_VARIABLE | ENUM_CASE | ATTRIBUTE => return true,
            BLOCK | CLASS_BODY | SOURCE_FILE | CLOSURE_EXPR | ARROW_FUNCTION_EXPR | METHOD_DECLARATION
            | FUNCTION_DECLARATION => {
                return false;
            }
            _ => {}
        }
    }
    false
}

fn type_text_is(node: &SyntaxNode, word: &str) -> bool {
    node.text().to_string().eq_ignore_ascii_case(word)
}

fn is_return_type_atom(node: &SyntaxNode) -> bool {
    node.ancestors()
        .skip(1)
        .take_while(|ancestor| matches!(ancestor.kind(), UNION_TYPE | NULLABLE_TYPE | RETURN_TYPE))
        .any(|ancestor| ancestor.kind() == RETURN_TYPE)
}

fn cast_word(token: &SyntaxToken) -> String {
    token.text().trim_matches(['(', ')', ' ', '\t']).to_ascii_lowercase()
}

/// The syntax PHP versions added, deprecated or removed, oldest first. This table is all that changes
/// for a new version, next to the syntax itself.
pub static FEATURES: &[Feature] = &[
    Feature {
        id: "match-expression",
        name: "Match expressions",
        plural: true,
        since: Some(PhpVersion::V8_0),
        deprecated: None,
        removed: None,
        kinds: &[MATCH_EXPR],
        detect: |element| first_token(node_of(element)?).map(|token| token.text_range()),
        example: "$a = match ($x) { 1 => 'a', default => 'b' };",
    },
    Feature {
        id: "nullsafe-operator",
        name: "The nullsafe operator",
        plural: false,
        since: Some(PhpVersion::V8_0),
        deprecated: None,
        removed: None,
        kinds: &[NULLSAFE_ARROW],
        detect: range_of_token,
        example: "$a = $b?->c;",
    },
    Feature {
        id: "named-arguments",
        name: "Named arguments",
        plural: true,
        since: Some(PhpVersion::V8_0),
        deprecated: None,
        removed: None,
        kinds: &[ARGUMENT],
        detect: |element| {
            let argument = node_of(element)?;
            token_child(argument, COLON)?;
            first_token(argument).map(|token| token.text_range())
        },
        example: "foo(name: 1);",
    },
    Feature {
        id: "constructor-promotion",
        name: "Constructor property promotion",
        plural: false,
        since: Some(PhpVersion::V8_0),
        deprecated: None,
        removed: None,
        kinds: &[MODIFIER_LIST],
        detect: |element| {
            let list = node_of(element)?;
            (parent_kind(list)? == PARAMETER).then(|| list.text_range())
        },
        example: "class A { public function __construct(private int $x) {} }",
    },
    Feature {
        id: "attributes",
        name: "Attributes",
        plural: true,
        since: Some(PhpVersion::V8_0),
        deprecated: None,
        removed: None,
        kinds: &[ATTRIBUTE_LIST],
        detect: range_of_node,
        example: "#[Attr] class A {}",
    },
    Feature {
        id: "union-types",
        name: "Union types",
        plural: true,
        since: Some(PhpVersion::V8_0),
        deprecated: None,
        removed: None,
        kinds: &[UNION_TYPE],
        detect: range_of_node,
        example: "function f(int|string $a) {}",
    },
    Feature {
        id: "static-return-type",
        name: "The static return type",
        plural: false,
        since: Some(PhpVersion::V8_0),
        deprecated: None,
        removed: None,
        kinds: &[NAMED_TYPE],
        detect: |element| {
            let node = node_of(element)?;
            (is_return_type_atom(node) && node.first_token()?.kind() == STATIC_KW).then(|| node.text_range())
        },
        example: "class A { function f(): static {} }",
    },
    Feature {
        id: "throw-expression",
        name: "Throw expressions",
        plural: true,
        since: Some(PhpVersion::V8_0),
        deprecated: None,
        removed: None,
        kinds: &[THROW_EXPR],
        detect: |element| {
            let node = node_of(element)?;
            (parent_kind(node)? != EXPR_STATEMENT).then(|| node.text_range())
        },
        example: "$a = $b ?? throw new E;",
    },
    Feature {
        id: "non-capturing-catch",
        name: "Catching an exception without a variable",
        plural: false,
        since: Some(PhpVersion::V8_0),
        deprecated: None,
        removed: None,
        kinds: &[CATCH_CLAUSE],
        detect: |element| {
            let clause = node_of(element)?;
            if token_child(clause, VARIABLE).is_some() {
                return None;
            }
            node_child(clause, NAMED_TYPE)
                .or_else(|| node_child(clause, UNION_TYPE))
                .map(|found| found.text_range())
        },
        example: "try {} catch (Exception) {}",
    },
    Feature {
        id: "trailing-comma-in-parameters",
        name: "A trailing comma in a parameter list",
        plural: false,
        since: Some(PhpVersion::V8_0),
        deprecated: None,
        removed: None,
        kinds: &[PARAMETER_LIST, CLOSURE_USE],
        detect: |element| {
            let list = node_of(element)?;
            let significant: Vec<_> = list
                .children_with_tokens()
                .filter(|child| !child.kind().is_trivia())
                .collect();
            let [.., comma, close] = significant.as_slice() else {
                return None;
            };
            (comma.kind() == COMMA && close.kind() == RPAREN).then(|| comma.text_range())
        },
        example: "function f($a, ) {}",
    },
    Feature {
        id: "real-cast",
        name: "The (real) cast",
        plural: false,
        since: None,
        deprecated: None,
        removed: Some(PhpVersion::V8_0),
        kinds: &[CAST],
        detect: |element| {
            let token = token_of(element)?;
            (cast_word(token) == "real").then(|| token.text_range())
        },
        example: "$a = (real) $b;",
    },
    Feature {
        id: "unset-cast",
        name: "The (unset) cast",
        plural: false,
        since: None,
        deprecated: None,
        removed: Some(PhpVersion::V8_0),
        kinds: &[CAST],
        detect: |element| {
            let token = token_of(element)?;
            (cast_word(token) == "unset").then(|| token.text_range())
        },
        example: "$a = (unset) $b;",
    },
    Feature {
        id: "enums",
        name: "Enumerations",
        plural: true,
        since: Some(PhpVersion::V8_1),
        deprecated: None,
        removed: None,
        kinds: &[ENUM_DECLARATION],
        detect: |element| first_token(node_of(element)?).map(|token| token.text_range()),
        example: "enum Suit { case Hearts; }",
    },
    Feature {
        id: "readonly-properties",
        name: "Readonly properties",
        plural: true,
        since: Some(PhpVersion::V8_1),
        deprecated: None,
        removed: None,
        kinds: &[MODIFIER_LIST],
        detect: |element| modifier_list_with(element, READONLY_KW, &[PROPERTY_DECLARATION, PARAMETER]),
        example: "class A { public readonly int $x; }",
    },
    Feature {
        id: "first-class-callable-syntax",
        name: "First-class callable syntax",
        plural: false,
        since: Some(PhpVersion::V8_1),
        deprecated: None,
        removed: None,
        kinds: &[ARGUMENT_LIST],
        detect: |element| token_child(node_of(element)?, ELLIPSIS).map(|token| token.text_range()),
        example: "$f = strlen(...);",
    },
    Feature {
        id: "new-in-initializers",
        name: "The new expression in an initializer",
        plural: false,
        since: Some(PhpVersion::V8_1),
        deprecated: None,
        removed: None,
        kinds: &[NEW_EXPR],
        detect: |element| {
            let node = node_of(element)?;
            for ancestor in node.ancestors().skip(1) {
                match ancestor.kind() {
                    PARAMETER | STATIC_VARIABLE | ATTRIBUTE => return Some(node.text_range()),
                    CONST_ELEMENT => {
                        return (parent_kind(&ancestor)? == CONST_STATEMENT).then(|| node.text_range());
                    }
                    BLOCK | CLASS_BODY | SOURCE_FILE | CLOSURE_EXPR | ARROW_FUNCTION_EXPR => return None,
                    _ => {}
                }
            }
            None
        },
        example: "function f($a = new Foo) {}",
    },
    Feature {
        id: "intersection-types",
        name: "Intersection types",
        plural: true,
        since: Some(PhpVersion::V8_1),
        deprecated: None,
        removed: None,
        kinds: &[INTERSECTION_TYPE],
        detect: range_of_node,
        example: "function f(A&B $a) {}",
    },
    Feature {
        id: "never-type",
        name: "The never return type",
        plural: false,
        since: Some(PhpVersion::V8_1),
        deprecated: None,
        removed: None,
        kinds: &[NAMED_TYPE],
        detect: |element| {
            let node = node_of(element)?;
            (is_return_type_atom(node) && type_text_is(node, "never")).then(|| node.text_range())
        },
        example: "function f(): never { exit; }",
    },
    Feature {
        id: "explicit-octal-notation",
        name: "The 0o prefix for octal numbers",
        plural: false,
        since: Some(PhpVersion::V8_1),
        deprecated: None,
        removed: None,
        kinds: &[INT_LITERAL],
        detect: |element| {
            let token = token_of(element)?;
            token
                .text()
                .get(..2)
                .filter(|prefix| prefix.eq_ignore_ascii_case("0o"))
                .map(|_| token.text_range())
        },
        example: "$a = 0o17;",
    },
    Feature {
        id: "final-class-constants",
        name: "Final class constants",
        plural: true,
        since: Some(PhpVersion::V8_1),
        deprecated: None,
        removed: None,
        kinds: &[MODIFIER_LIST],
        detect: |element| modifier_list_with(element, FINAL_KW, &[CLASS_CONST_DECLARATION]),
        example: "class A { final public const X = 1; }",
    },
    Feature {
        id: "readonly-classes",
        name: "Readonly classes",
        plural: true,
        since: Some(PhpVersion::V8_2),
        deprecated: None,
        removed: None,
        kinds: &[MODIFIER_LIST],
        detect: |element| modifier_list_with(element, READONLY_KW, &[CLASS_DECLARATION]),
        example: "readonly class A {}",
    },
    Feature {
        id: "dnf-types",
        name: "Disjunctive normal form types",
        plural: true,
        since: Some(PhpVersion::V8_2),
        deprecated: None,
        removed: None,
        kinds: &[PAREN_TYPE],
        detect: range_of_node,
        example: "function f((A&B)|null $a) {}",
    },
    Feature {
        id: "standalone-null-false-true-types",
        name: "The null, false and true types on their own",
        plural: true,
        since: Some(PhpVersion::V8_2),
        deprecated: None,
        removed: None,
        kinds: &[NAMED_TYPE],
        detect: |element| {
            let node = node_of(element)?;
            let alone = matches!(
                parent_kind(node)?,
                PARAMETER | RETURN_TYPE | PROPERTY_DECLARATION | CLASS_CONST_DECLARATION
            );
            let word = ["null", "false", "true"].iter().any(|word| type_text_is(node, word));
            (alone && word).then(|| node.text_range())
        },
        example: "function f(): null {}",
    },
    Feature {
        id: "trait-constants",
        name: "Constants in traits",
        plural: true,
        since: Some(PhpVersion::V8_2),
        deprecated: None,
        removed: None,
        kinds: &[CLASS_CONST_DECLARATION],
        detect: |element| {
            let node = node_of(element)?;
            let body = node.parent()?;
            (body.parent()?.kind() == TRAIT_DECLARATION)
                .then(|| first_token(node).map_or(node.text_range(), |token| token.text_range()))
        },
        example: "trait T { const X = 1; }",
    },
    Feature {
        id: "dollar-brace-interpolation",
        name: "The ${...} form of string interpolation",
        plural: false,
        since: None,
        deprecated: Some(PhpVersion::V8_2),
        removed: None,
        kinds: &[DOLLAR_BRACE_INTERPOLATION],
        detect: range_of_node,
        example: "$a = \"${b}\";",
    },
    Feature {
        id: "typed-class-constants",
        name: "Typed class constants",
        plural: true,
        since: Some(PhpVersion::V8_3),
        deprecated: None,
        removed: None,
        kinds: &[CLASS_CONST_DECLARATION],
        detect: |element| {
            let node = node_of(element)?;
            let ty = node.children().find(|child| {
                matches!(
                    child.kind(),
                    NAMED_TYPE | NULLABLE_TYPE | UNION_TYPE | INTERSECTION_TYPE | PAREN_TYPE
                )
            })?;
            Some(ty.text_range())
        },
        example: "class A { const int X = 1; }",
    },
    Feature {
        id: "dynamic-class-constant-fetch",
        name: "Fetching a class constant by a dynamic name",
        plural: false,
        since: Some(PhpVersion::V8_3),
        deprecated: None,
        removed: None,
        kinds: &[SCOPED_ACCESS_EXPR],
        detect: |element| {
            let node = node_of(element)?;
            token_child(node, LBRACE)?;
            (parent_kind(node)? != CALL_EXPR).then(|| node.text_range())
        },
        example: "$a = Foo::{$name};",
    },
    Feature {
        id: "readonly-anonymous-classes",
        name: "Readonly anonymous classes",
        plural: true,
        since: Some(PhpVersion::V8_3),
        deprecated: None,
        removed: None,
        kinds: &[MODIFIER_LIST],
        detect: |element| modifier_list_with(element, READONLY_KW, &[ANONYMOUS_CLASS]),
        example: "$a = new readonly class {};",
    },
    Feature {
        id: "property-hooks",
        name: "Property hooks",
        plural: true,
        since: Some(PhpVersion::V8_4),
        deprecated: None,
        removed: None,
        kinds: &[PROPERTY_HOOK_LIST],
        detect: range_of_node,
        example: "class A { public int $x { get => 1; } }",
    },
    Feature {
        id: "asymmetric-visibility",
        name: "Asymmetric visibility",
        plural: false,
        since: Some(PhpVersion::V8_4),
        deprecated: None,
        removed: None,
        kinds: &[MODIFIER_LIST],
        detect: |element| {
            let list = node_of(element)?;
            let open = token_child(list, LPAREN)?;
            let close = token_child(list, RPAREN)?;
            let keyword = open.prev_token().filter(|token| !token.kind().is_trivia())?;
            Some(TextRange::new(keyword.text_range().start(), close.text_range().end()))
        },
        example: "class A { public private(set) int $x = 0; }",
    },
    Feature {
        id: "new-without-parentheses",
        name: "Accessing a member of a new expression without parentheses",
        plural: false,
        since: Some(PhpVersion::V8_4),
        deprecated: None,
        removed: None,
        kinds: &[
            PROPERTY_FETCH_EXPR,
            INDEX_EXPR,
            SCOPED_ACCESS_EXPR,
            STATIC_PROPERTY_EXPR,
            CALL_EXPR,
        ],
        detect: |element| {
            let node = node_of(element)?;
            let first = node.first_child()?;
            (first.kind() == NEW_EXPR).then(|| first.text_range())
        },
        example: "$a = new Foo()->bar();",
    },
    Feature {
        id: "implicit-nullable-parameter",
        name: "A parameter type that is implicitly nullable through a null default",
        plural: false,
        since: None,
        deprecated: Some(PhpVersion::V8_4),
        removed: None,
        kinds: &[PARAMETER],
        detect: |element| {
            let parameter = node_of(element)?;
            let ty = parameter
                .children()
                .find(|child| matches!(child.kind(), NAMED_TYPE | UNION_TYPE | INTERSECTION_TYPE))?;
            let default = parameter.children().find(|child| child.kind() == NAME)?;
            if !type_text_is(&default, "null") {
                return None;
            }
            let text = ty.text().to_string().to_ascii_lowercase();
            let nullable = text == "mixed" || text == "null" || text.split('|').any(|part| part.trim() == "null");
            (!nullable).then(|| ty.text_range())
        },
        example: "function f(int $a = null) {}",
    },
    Feature {
        id: "pipe-operator",
        name: "The pipe operator",
        plural: false,
        since: Some(PhpVersion::V8_5),
        deprecated: None,
        removed: None,
        kinds: &[PIPE_GT],
        detect: range_of_token,
        example: "$a = $b |> strlen(...);",
    },
    Feature {
        id: "clone-with",
        name: "Clone with property overrides",
        plural: false,
        since: Some(PhpVersion::V8_5),
        deprecated: None,
        removed: None,
        kinds: &[CLONE_EXPR],
        detect: |element| {
            let node = node_of(element)?;
            node_child(node, ARGUMENT_LIST).map(|list| list.text_range())
        },
        example: "$a = clone($b, ['x' => 1]);",
    },
    Feature {
        id: "void-cast",
        name: "The (void) cast",
        plural: false,
        since: Some(PhpVersion::V8_5),
        deprecated: None,
        removed: None,
        kinds: &[CAST],
        detect: |element| {
            let token = token_of(element)?;
            (cast_word(token) == "void").then(|| token.text_range())
        },
        example: "(void) foo();",
    },
    Feature {
        id: "final-promoted-properties",
        name: "The final modifier on a promoted property",
        plural: false,
        since: Some(PhpVersion::V8_5),
        deprecated: None,
        removed: None,
        kinds: &[MODIFIER_LIST],
        detect: |element| modifier_list_with(element, FINAL_KW, &[PARAMETER]),
        example: "class A { function __construct(final public int $x) {} }",
    },
    Feature {
        id: "attributes-on-constants",
        name: "Attributes on global constants",
        plural: true,
        since: Some(PhpVersion::V8_5),
        deprecated: None,
        removed: None,
        kinds: &[ATTRIBUTE_LIST],
        detect: |element| {
            let node = node_of(element)?;
            (parent_kind(node)? == CONST_STATEMENT).then(|| node.text_range())
        },
        example: "#[Attr] const X = 1;",
    },
    Feature {
        id: "closures-in-constant-expressions",
        name: "Closures in constant expressions",
        plural: true,
        since: Some(PhpVersion::V8_5),
        deprecated: None,
        removed: None,
        kinds: &[CLOSURE_EXPR, ARROW_FUNCTION_EXPR],
        detect: |element| {
            let node = node_of(element)?;
            in_constant_expression(node)
                .then(|| first_token(node).map_or(node.text_range(), |token| token.text_range()))
        },
        example: "function f($a = static function () {}) {}",
    },
    Feature {
        id: "first-class-callable-in-constant-expressions",
        name: "First-class callable syntax in constant expressions",
        plural: false,
        since: Some(PhpVersion::V8_5),
        deprecated: None,
        removed: None,
        kinds: &[ARGUMENT_LIST],
        detect: |element| {
            let node = node_of(element)?;
            let ellipsis = token_child(node, ELLIPSIS)?;
            in_constant_expression(node).then(|| ellipsis.text_range())
        },
        example: "const F = strlen(...);",
    },
    Feature {
        id: "asymmetric-visibility-on-static-properties",
        name: "Asymmetric visibility on static properties",
        plural: false,
        since: Some(PhpVersion::V8_5),
        deprecated: None,
        removed: None,
        kinds: &[MODIFIER_LIST],
        detect: |element| {
            let list = node_of(element)?;
            token_child(list, STATIC_KW)?;
            let open = token_child(list, LPAREN)?;
            open.prev_token()
                .filter(|token| !token.kind().is_trivia())
                .map(|token| token.text_range())
        },
        example: "class A { public private(set) static int $x = 0; }",
    },
    Feature {
        id: "backtick-operator",
        name: "The backtick operator",
        plural: false,
        since: None,
        deprecated: Some(PhpVersion::V8_5),
        removed: None,
        kinds: &[SHELL_EXEC_EXPR],
        detect: range_of_node,
        example: "$a = `ls`;",
    },
    Feature {
        id: "non-canonical-casts",
        name: "The (integer), (boolean), (double) and (binary) casts",
        plural: true,
        since: None,
        deprecated: Some(PhpVersion::V8_5),
        removed: None,
        kinds: &[CAST],
        detect: |element| {
            let token = token_of(element)?;
            matches!(cast_word(token).as_str(), "integer" | "boolean" | "double" | "binary").then(|| token.text_range())
        },
        example: "$a = (integer) $b;",
    },
    Feature {
        id: "semicolon-after-case",
        name: "A semicolon after a case",
        plural: false,
        since: None,
        deprecated: Some(PhpVersion::V8_5),
        removed: None,
        kinds: &[CASE_CLAUSE, DEFAULT_CLAUSE],
        detect: |element| token_child(node_of(element)?, SEMICOLON).map(|token| token.text_range()),
        example: "switch ($a) { case 1; break; }",
    },
    Feature {
        id: "partial-function-application",
        name: "Partial function application",
        plural: false,
        since: Some(PhpVersion::V8_6),
        deprecated: None,
        removed: None,
        kinds: &[ARGUMENT],
        detect: |element| {
            let argument = node_of(element)?;
            let question = token_child(argument, QUESTION);
            let lone_ellipsis = token_child(argument, ELLIPSIS).filter(|_| argument.children().next().is_none());
            question.or(lone_ellipsis).map(|token| token.text_range())
        },
        example: "$f = str_replace('a', ?, ...);",
    },
    Feature {
        id: "readonly-property-defaults",
        name: "Default values of readonly properties",
        plural: true,
        since: Some(PhpVersion::V8_6),
        deprecated: None,
        removed: None,
        kinds: &[PROPERTY_ELEMENT],
        detect: |element| {
            let property = node_of(element)?;
            let assign = token_child(property, ASSIGN)?;
            let declaration = property
                .parent()
                .filter(|parent| parent.kind() == PROPERTY_DECLARATION)?;
            let own =
                node_child(&declaration, MODIFIER_LIST).is_some_and(|list| token_child(&list, READONLY_KW).is_some());
            let class = declaration
                .ancestors()
                .find(|ancestor| matches!(ancestor.kind(), CLASS_DECLARATION | ANONYMOUS_CLASS));
            let of_class = class
                .and_then(|class| node_child(&class, MODIFIER_LIST))
                .is_some_and(|list| token_child(&list, READONLY_KW).is_some());
            (own || of_class).then(|| TextRange::new(assign.text_range().start(), property.text_range().end()))
        },
        example: "class A { public readonly int $x = 1; }",
    },
    Feature {
        id: "constant-object-property-writes",
        name: "Writes to a property of an object in a constant",
        plural: true,
        since: Some(PhpVersion::V8_6),
        deprecated: None,
        removed: None,
        kinds: &[ASSIGN_EXPR],
        detect: |element| {
            let assign = node_of(element)?;
            let target = assign
                .children()
                .next()
                .filter(|node| node.kind() == PROPERTY_FETCH_EXPR)?;
            let object = target.children().next()?;
            matches!(object.kind(), NAME | SCOPED_ACCESS_EXPR).then(|| target.text_range())
        },
        example: "FOO->bar = 1;",
    },
    Feature {
        id: "override-on-constants",
        name: "#[Override] on a class constant or an enum case",
        plural: false,
        since: Some(PhpVersion::V8_6),
        deprecated: None,
        removed: None,
        kinds: &[ATTRIBUTE],
        detect: |element| {
            let attribute = node_of(element)?;
            let owner = attribute.parent()?.parent()?;
            if !matches!(owner.kind(), CLASS_CONST_DECLARATION | ENUM_CASE) {
                return None;
            }
            let name = node_child(attribute, NAME)?;
            let written = name.text().to_string();
            written
                .trim_start_matches('\\')
                .eq_ignore_ascii_case("Override")
                .then(|| attribute.text_range())
        },
        example: "class A extends B { #[\\Override] const X = 1; }",
    },
    Feature {
        id: "enum-debug-info",
        name: "__debugInfo() on an enum",
        plural: false,
        since: Some(PhpVersion::V8_6),
        deprecated: None,
        removed: None,
        kinds: &[METHOD_DECLARATION],
        detect: |element| {
            let method = node_of(element)?;
            let name = node_child(method, NAME)?;
            let in_enum = method
                .ancestors()
                .skip(1)
                .find(|ancestor| {
                    matches!(
                        ancestor.kind(),
                        CLASS_DECLARATION
                            | ENUM_DECLARATION
                            | TRAIT_DECLARATION
                            | INTERFACE_DECLARATION
                            | ANONYMOUS_CLASS
                    )
                })
                .is_some_and(|owner| owner.kind() == ENUM_DECLARATION);
            (in_enum && name.text().to_string().eq_ignore_ascii_case("__debugInfo")).then(|| name.text_range())
        },
        example: "enum E { public function __debugInfo(): array { return []; } }",
    },
];
