//! Inspections: the problems a PHP developer meets every day, found by reading a file against the
//! index. Each has a code, a default severity and a switch, and an inspection that cannot be sure
//! stays silent, so what is reported is something to look at and not a guess.

mod calls;
pub(crate) mod classes;
pub(crate) mod flow;
mod frameworks;
mod members;
mod names;
mod phpdoc;
mod phpunit;
mod scopes;
pub(crate) mod types;
pub(crate) mod unused;
pub(crate) mod util;

use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeMap, HashMap};
use std::rc::Rc;

use php_index::{Index, UseKind};
use php_syntax::{SyntaxKind, SyntaxNode, TextRange};

use crate::context::FileContext;
use crate::diagnostics::{Diagnostic, DiagnosticSeverity};

/// What an inspection is, apart from where it found something.
#[derive(Clone, Copy, Debug)]
pub struct InspectionInfo {
    pub code: &'static str,
    pub severity: DiagnosticSeverity,
    pub enabled: bool,
    pub summary: &'static str,
}

const fn info(
    code: &'static str,
    severity: DiagnosticSeverity,
    enabled: bool,
    summary: &'static str,
) -> InspectionInfo {
    InspectionInfo {
        code,
        severity,
        enabled,
        summary,
    }
}

use DiagnosticSeverity::{Error, Hint, Information, Warning};

/// Every inspection, in the order the README lists them.
pub const INSPECTIONS: &[InspectionInfo] = &[
    info(
        "undefined-class",
        Error,
        true,
        "A class, interface, trait or enum that does not exist",
    ),
    info("undefined-function", Error, true, "A function that does not exist"),
    info("undefined-constant", Error, true, "A constant that does not exist"),
    info(
        "undefined-class-constant",
        Error,
        true,
        "A class constant or enum case that does not exist",
    ),
    info("undefined-method", Error, true, "A method the class does not have"),
    info(
        "undefined-property",
        Warning,
        true,
        "A property the class does not declare",
    ),
    info(
        "undefined-variable",
        Warning,
        true,
        "A variable that is read and never assigned in its scope",
    ),
    info(
        "undefined-named-argument",
        Error,
        true,
        "A named argument the callee has no parameter for",
    ),
    info("unused-import", Warning, true, "A `use` statement nothing refers to"),
    info("unused-private-method", Warning, true, "A private method nothing calls"),
    info(
        "unused-private-property",
        Warning,
        true,
        "A private property nothing reads or writes",
    ),
    info(
        "unused-private-constant",
        Warning,
        true,
        "A private constant nothing reads",
    ),
    info(
        "unused-variable",
        Warning,
        true,
        "A local variable that is assigned and never read",
    ),
    info(
        "unused-parameter",
        Hint,
        true,
        "A parameter of a private or final method that is never used",
    ),
    info("deprecated", Warning, true, "A use of something marked deprecated"),
    info(
        "wrong-argument-count",
        Error,
        true,
        "Too few or too many arguments in a call",
    ),
    info(
        "argument-type-mismatch",
        Error,
        true,
        "An argument whose type the parameter cannot take",
    ),
    info(
        "return-type-mismatch",
        Error,
        true,
        "A returned value the declared return type cannot take",
    ),
    info(
        "missing-return",
        Error,
        true,
        "A function with a return type that can end without returning",
    ),
    info(
        "unreachable-code",
        Warning,
        true,
        "Statements after something that always leaves",
    ),
    info(
        "assignment-in-condition",
        Warning,
        true,
        "A constant assigned in a condition, where a comparison was meant",
    ),
    info(
        "incompatible-comparison",
        Warning,
        true,
        "A strict comparison between types that never match",
    ),
    info(
        "abstract-method-not-implemented",
        Error,
        true,
        "An abstract method a concrete class does not implement",
    ),
    info(
        "interface-method-not-implemented",
        Error,
        true,
        "An interface method a concrete class does not implement",
    ),
    info(
        "incompatible-override",
        Error,
        true,
        "A method whose signature cannot replace the one it overrides",
    ),
    info(
        "readonly-reassigned",
        Error,
        true,
        "A readonly property that is written again",
    ),
    info("enum-misuse", Error, true, "Something an enum does not allow"),
    info(
        "static-call-of-instance-method",
        Error,
        true,
        "An instance method called statically",
    ),
    info(
        "instance-call-of-static-method",
        Information,
        true,
        "A static method called through an object",
    ),
    info(
        "this-in-static-context",
        Error,
        true,
        "`$this` where there is no object",
    ),
    info(
        "phpdoc-unknown-parameter",
        Warning,
        true,
        "A `@param` for a parameter the function does not have",
    ),
    info(
        "phpdoc-type-mismatch",
        Warning,
        true,
        "A PHPDoc type that contradicts the declared type",
    ),
    info(
        "missing-data-provider",
        Error,
        true,
        "A data provider that does not exist",
    ),
    info(
        "missing-test-dependency",
        Warning,
        true,
        "A test a `#[Depends]` names that does not exist",
    ),
    info(
        "data-provider-arity",
        Warning,
        true,
        "A data set with more or fewer values than the test takes",
    ),
    info(
        "missing-double-method",
        Error,
        true,
        "A method of a test double that the class does not have",
    ),
    info(
        "double-return-type-mismatch",
        Warning,
        true,
        "A value `willReturn()` gives that the method cannot return",
    ),
    info(
        "unknown-config-key",
        Warning,
        true,
        "A config key that is not in the project's config files",
    ),
    info(
        "unknown-route",
        Warning,
        true,
        "A route name that no route file declares",
    ),
    info("unknown-view", Warning, true, "A view that has no template"),
    info(
        "unknown-validation-rule",
        Warning,
        true,
        "A validation rule the validator does not have and the project does not add",
    ),
    info(
        "unknown-cast",
        Warning,
        true,
        "A cast of a model that is neither one the model knows by name nor a class",
    ),
    info(
        "unknown-relation",
        Warning,
        true,
        "A relation an Eloquent query names that its model does not have",
    ),
    info(
        "unknown-twig-function",
        Warning,
        true,
        "A Twig function no extension of the project or its packages declares",
    ),
    info(
        "unknown-twig-filter",
        Warning,
        true,
        "A Twig filter no extension of the project or its packages declares",
    ),
    info(
        "unknown-twig-test",
        Warning,
        true,
        "A Twig test no extension of the project or its packages declares",
    ),
    info(
        "unbalanced-directive",
        Error,
        true,
        "A Blade directive or a Twig tag that opens a block that is never closed, or one that closes nothing",
    ),
    info("unknown-template", Warning, true, "A Twig template that does not exist"),
    info(
        "unknown-translation",
        Warning,
        true,
        "A translation key of a language file that does not have it",
    ),
    info(
        "missing-strict-types",
        Hint,
        false,
        "A file without `declare(strict_types=1)`",
    ),
];

pub fn inspection_info(code: &str) -> Option<&'static InspectionInfo> {
    INSPECTIONS.iter().find(|info| info.code == code)
}

/// What a client chose for one inspection.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Override {
    pub enabled: Option<bool>,
    pub severity: Option<DiagnosticSeverity>,
}

/// The switches and severities a client configured, by code. What is not named keeps its default.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InspectionSettings {
    overrides: BTreeMap<String, Override>,
}

impl InspectionSettings {
    pub fn set(&mut self, code: &str, choice: Override) {
        self.overrides.insert(code.to_string(), choice);
    }

    /// The severity an inspection reports at, or `None` while it is switched off.
    pub fn severity_of(&self, info: &InspectionInfo) -> Option<DiagnosticSeverity> {
        let choice = self.overrides.get(info.code).copied().unwrap_or_default();
        if !choice.enabled.unwrap_or(info.enabled) {
            return None;
        }
        Some(choice.severity.unwrap_or(info.severity))
    }

    pub fn is_empty(&self) -> bool {
        self.overrides.is_empty()
    }
}

/// Which declarations of the standard library a project cannot see because its extension is not
/// required, so that a name only they hold is not reported as undefined.
pub struct Externals<'a> {
    pub class: &'a dyn Fn(&str) -> bool,
    pub function: &'a dyn Fn(&str) -> bool,
    pub constant: &'a dyn Fn(&str) -> bool,
}

fn never(_: &str) -> bool {
    false
}

impl Externals<'_> {
    /// Nothing is held back.
    pub fn none() -> Externals<'static> {
        Externals {
            class: &never,
            function: &never,
            constant: &never,
        }
    }
}

/// What an inspection run reads.
pub struct InspectionEnv<'a> {
    pub index: &'a Index,
    pub text: &'a str,
    pub root: &'a SyntaxNode,
    pub settings: &'a InspectionSettings,
    /// The index holds the project and the standard library. Until it does, a name it lacks may
    /// only be one it has not read yet, so nothing is reported as undefined.
    pub ready: bool,
    pub externals: &'a Externals<'a>,
}

/// The edit that mends a finding, as far as the finding alone says. Quick fixes read it back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fix {
    None,
    /// The name as written, to import from wherever the index holds a class, function or constant of it.
    Import {
        name: String,
        kind: UseKind,
        at: u32,
    },
    /// The `use` statement or clause that nothing needs.
    RemoveImport {
        range: TextRange,
    },
    /// A declaration to delete with its doc comment.
    RemoveDeclaration {
        range: TextRange,
    },
    /// An assignment to a variable nothing reads: the whole statement when `pure`, else only `$x = `.
    RemoveAssignment {
        statement: TextRange,
        prefix: TextRange,
        pure: bool,
    },
    /// Statements after a jump.
    RemoveRange {
        range: TextRange,
    },
    /// The `=` of a condition that was meant to compare.
    ReplaceAssignment {
        operator: TextRange,
    },
    /// A `@param` line for a parameter that is not there.
    RemoveDocLine {
        range: TextRange,
    },
    /// A class that must implement more methods; the offset is the start of its declaration.
    ImplementMembers {
        class: u32,
    },
    /// A member to create on a class the project owns.
    CreateMember {
        class: String,
        name: String,
        is_static: bool,
        is_method: bool,
        at: u32,
    },
    /// `declare(strict_types=1);` at the top of the file.
    AddStrictTypes,
    /// The visibility of a member to widen.
    Visibility {
        member: TextRange,
        to: &'static str,
    },
}

/// A diagnostic with what a quick fix needs to repair it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub diagnostic: Diagnostic,
    pub fix: Fix,
}

/// What every inspection of a run shares: the file, its nodes, what the settings say and where
/// findings go.
pub(crate) struct Cx<'a> {
    pub index: &'a Index,
    pub text: &'a str,
    pub root: SyntaxNode,
    pub file: FileContext<'a>,
    pub ready: bool,
    pub externals: &'a Externals<'a>,
    pub nodes: Vec<SyntaxNode>,
    severities: HashMap<&'static str, DiagnosticSeverity>,
    error_ranges: Vec<TextRange>,
    found: RefCell<Vec<Finding>>,
    complete: RefCell<HashMap<String, bool>>,
    pub strict_types: bool,
    dynamic_defines: OnceCell<bool>,
    usages: RefCell<HashMap<u32, Rc<unused::Usage>>>,
}

impl<'a> Cx<'a> {
    pub(crate) fn new(env: &'a InspectionEnv<'a>) -> Cx<'a> {
        let severities = INSPECTIONS
            .iter()
            .filter_map(|info| env.settings.severity_of(info).map(|severity| (info.code, severity)))
            .collect();
        let nodes: Vec<SyntaxNode> = env.root.descendants().collect();
        let error_ranges = nodes
            .iter()
            .filter(|node| node.kind() == SyntaxKind::ERROR)
            .map(|node| node.text_range())
            .collect();
        Cx {
            index: env.index,
            text: env.text,
            root: env.root.clone(),
            file: FileContext::new(env.index, env.root),
            ready: env.ready,
            externals: env.externals,
            nodes,
            severities,
            error_ranges,
            found: RefCell::new(Vec::new()),
            complete: RefCell::new(HashMap::new()),
            strict_types: declares_strict_types(env.root),
            dynamic_defines: OnceCell::new(),
            usages: RefCell::new(HashMap::new()),
        }
    }

    /// Whether some file defines constants while it runs, so that one the index lacks may exist.
    pub fn has_dynamic_defines(&self) -> bool {
        *self.dynamic_defines.get_or_init(|| self.index.has_dynamic_defines())
    }

    pub fn on(&self, code: &str) -> bool {
        self.severities.contains_key(code)
    }

    /// Whether a range touches a part of the file the parser could not read.
    pub fn in_error(&self, range: TextRange) -> bool {
        self.error_ranges.iter().any(|error| error.intersect(range).is_some())
    }

    pub fn report(&self, code: &'static str, range: TextRange, message: String, fix: Fix) {
        let Some(severity) = self.severities.get(code).copied() else {
            return;
        };
        if self.in_error(range) {
            return;
        }
        self.found.borrow_mut().push(Finding {
            diagnostic: Diagnostic {
                range,
                message,
                severity,
                deprecated: code == "deprecated",
                unnecessary: code.starts_with("unused-") || code == "unreachable-code",
                code,
            },
            fix,
        });
    }
}

/// Whether the file starts with `declare(strict_types=1)`.
pub(crate) fn declares_strict_types(root: &SyntaxNode) -> bool {
    root.children()
        .filter(|node| node.kind() == SyntaxKind::DECLARE_STATEMENT)
        .flat_map(|statement| statement.children().collect::<Vec<_>>())
        .filter(|directive| directive.kind() == SyntaxKind::DECLARE_DIRECTIVE)
        .any(|directive| {
            let text = directive.text().to_string().replace(char::is_whitespace, "");
            text.eq_ignore_ascii_case("strict_types=1")
        })
}

/// Runs the inspections the settings leave on over a file.
pub fn inspect(env: &InspectionEnv) -> Vec<Finding> {
    let cx = Cx::new(env);
    names::run(&cx);
    members::run(&cx);
    scopes::run(&cx);
    unused::run(&cx);
    calls::run(&cx);
    flow::run(&cx);
    classes::run(&cx);
    phpdoc::run(&cx);
    phpunit::run(&cx);
    frameworks::run(&cx);
    let mut found = cx.found.into_inner();
    found.sort_by_key(|finding| (finding.diagnostic.range.start(), finding.diagnostic.range.end()));
    found.dedup_by(|later, earlier| {
        later.diagnostic.range == earlier.diagnostic.range && later.diagnostic.code == earlier.diagnostic.code
    });
    found
}

#[cfg(test)]
pub(crate) mod tests;
