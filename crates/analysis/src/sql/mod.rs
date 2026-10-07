//! SQL inside the strings of PHP: which strings hold it, what kind of SQL each is and which tables
//! a part of a query sees. A string is SQL when a person marks it (a `language=SQL` comment, a
//! heredoc labeled `SQL`), when it is passed to a function or method that takes SQL (resolved
//! through the types of the receiver, `sinks.php`), directly or through a variable, or, with the
//! heuristic on, when it reads as a whole statement. The pieces of the string go into an
//! [`sql_embed::Fragment`]; everything SQL answers about it comes from `sql-embed`.

mod driver;
mod markers;
mod scope;
mod sinks;
mod strings;

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};

use php_index::Index;
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, TextRange};
use sql_embed::{Dialect, Fragment, FragmentKind, ScopeTable};

pub use driver::project_dialect;

use crate::ast::{start, text_of};
use crate::context::FileContext;
use markers::Marker;
use sinks::{Format, Sink};
use strings::Pieces;

/// Which signals make a string SQL.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Detection {
    /// `language=SQL` comments and heredocs labeled `SQL`.
    pub markers: bool,
    /// The arguments of the functions and methods that take SQL.
    pub sinks: bool,
    /// A string that reads as a whole statement wherever it is passed, assigned or returned.
    pub heuristic: bool,
    /// How sure the heuristic has to be, from 0 to 1.
    pub threshold: f32,
}

impl Default for Detection {
    fn default() -> Detection {
        Detection {
            markers: true,
            sinks: true,
            heuristic: true,
            threshold: 0.8,
        }
    }
}

/// What made a string SQL.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    Marker,
    Sink,
    Heuristic,
}

/// A string of the document that holds SQL.
#[derive(Clone, Debug)]
pub struct Embedded {
    /// The string expression in the document: the literals, the quotes and what is concatenated.
    pub range: TextRange,
    pub fragment: Fragment,
    /// The dialect a marker or the function the string is passed to names.
    pub dialect: Option<Dialect>,
    /// The dialect is one a person wrote in a marker, which goes before the settings.
    pub marked_dialect: bool,
    pub reason: Reason,
}

/// What the function or method a string is passed to says about it.
#[derive(Clone, Debug)]
struct Use {
    /// `None` for a part of a query whose kind is whatever reads best.
    kind: Option<FragmentKind>,
    dialect: Option<Dialect>,
    format: Option<Format>,
    tables: Vec<ScopeTable>,
}

/// The words a statement starts with, as far as a string that is only marked has to be read as one
/// rather than as an expression.
const STATEMENT_WORDS: &[&str] = &[
    "alter",
    "analyze",
    "begin",
    "call",
    "commit",
    "create",
    "delete",
    "do",
    "drop",
    "explain",
    "grant",
    "insert",
    "lock",
    "merge",
    "pragma",
    "release",
    "rename",
    "replace",
    "revoke",
    "rollback",
    "savepoint",
    "select",
    "set",
    "show",
    "start",
    "table",
    "truncate",
    "unlock",
    "update",
    "use",
    "vacuum",
    "values",
    "with",
];

/// The first word of SQL text, past whitespace, comments and opening parentheses.
fn first_word(text: &str) -> String {
    let mut rest = text;
    loop {
        let trimmed = rest.trim_start_matches(|character: char| character.is_whitespace() || character == '(');
        if let Some(comment) = trimmed.strip_prefix("--") {
            rest = comment.split_once('\n').map_or("", |(_, after)| after);
        } else if let Some(comment) = trimmed.strip_prefix("/*") {
            rest = comment.split_once("*/").map_or("", |(_, after)| after);
        } else {
            rest = trimmed;
            break;
        }
    }
    rest.chars()
        .take_while(char::is_ascii_alphabetic)
        .collect::<String>()
        .to_ascii_lowercase()
}

fn starts_statement(text: &str) -> bool {
    STATEMENT_WORDS.contains(&first_word(text).as_str())
}

/// Whether text reads as a query rather than as a sentence that starts with the same word: its
/// first word is in capitals, or it has the punctuation of SQL or something put into it.
/// "select the users from the list" parses, but no query is written that way.
fn looks_like_a_query(text: &str, has_holes: bool) -> bool {
    let word: String = text
        .trim_start()
        .chars()
        .take_while(char::is_ascii_alphabetic)
        .collect();
    has_holes
        || (word.len() > 1 && word.chars().all(|character| character.is_ascii_uppercase()))
        || text.contains(['*', '=', ',', '(', '?', ';', '<', '>', '`'])
        || text
            .match_indices(':')
            .any(|(at, _)| text[at + 1..].starts_with(|character: char| character.is_ascii_alphabetic()))
}

/// The node an expression stands in for its parent: itself, or the parentheses around it.
fn outermost(node: &SyntaxNode) -> SyntaxNode {
    let mut current = node.clone();
    while let Some(parent) = current.parent().filter(|parent| parent.kind() == PAREN_EXPR) {
        current = parent;
    }
    current
}

/// The name a call is made by: the function, the method, or `__construct` for `new`.
fn call_name(call: &SyntaxNode) -> Option<String> {
    match call.kind() {
        NEW_EXPR => Some("__construct".to_string()),
        CALL_EXPR => {
            let callee = call.children().next()?;
            match callee.kind() {
                NAME => Some(text_of(&callee)),
                PROPERTY_FETCH_EXPR | SCOPED_ACCESS_EXPR => callee
                    .children()
                    .filter(|child| child.kind() == NAME)
                    .last()
                    .map(|name| text_of(&name)),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The argument node an expression is, with the call it belongs to.
fn argument_of(expression: &SyntaxNode) -> Option<(SyntaxNode, SyntaxNode)> {
    let argument = outermost(expression)
        .parent()
        .filter(|parent| parent.kind() == ARGUMENT)?;
    let call = argument
        .parent()
        .filter(|list| list.kind() == ARGUMENT_LIST)?
        .parent()
        .filter(|call| matches!(call.kind(), CALL_EXPR | NEW_EXPR))?;
    Some((argument, call))
}

/// The call an expression is passed to, also as an item of an array it is passed in
/// (`->orderBy([literal('score desc')])`).
fn passed_to(expression: &SyntaxNode) -> Option<SyntaxNode> {
    let mut node = outermost(expression);
    while let Some(parent) = node
        .parent()
        .filter(|parent| matches!(parent.kind(), ARRAY_ITEM | ARRAY_EXPR | PAREN_EXPR))
    {
        node = parent;
    }
    argument_of(&node).map(|(_, call)| call)
}

/// Whether a call is to `sprintf()` or `vsprintf()` of PHP itself.
fn is_sprintf(finder: &Finder, call: &SyntaxNode) -> bool {
    let Some(callee) = call.children().next().filter(|callee| callee.kind() == NAME) else {
        return false;
    };
    let name = text_of(&callee);
    let short = name.trim_start_matches('\\').to_ascii_lowercase();
    if short != "sprintf" && short != "vsprintf" {
        return false;
    }
    let analyzer = finder.ctx.analyzer(call);
    let candidates = analyzer.resolver.function_candidates(&name);
    match finder.ctx.index.first_function(&candidates) {
        Some(found) => found.decl.name.eq_ignore_ascii_case(&short),
        None => true,
    }
}

/// Finds the strings of one document that hold SQL. Calls are resolved once and the uses of the
/// variables of a function are read once, however many strings ask.
pub(crate) struct Finder<'a> {
    pub ctx: FileContext<'a>,
    detection: Detection,
    resolved: RefCell<HashMap<TextRange, Vec<&'static Sink>>>,
    variables: RefCell<HashMap<TextRange, Variables>>,
    first_strings: RefCell<HashMap<TextRange, Option<TextRange>>>,
}

/// What the functions of a scope do with their variables: the SQL each is passed as, and which
/// ones get more appended with `.=`.
#[derive(Default, Clone)]
struct Variables {
    uses: HashMap<String, Use>,
    appended: HashSet<String>,
    /// The ones that are the format of a `sprintf()`, whose placeholders are holes.
    formatted: HashSet<String>,
}

impl<'a> Finder<'a> {
    fn new(index: &'a Index, root: &SyntaxNode, detection: Detection) -> Finder<'a> {
        Finder {
            ctx: FileContext::new(index, root),
            detection,
            resolved: RefCell::default(),
            variables: RefCell::default(),
            first_strings: RefCell::default(),
        }
    }

    /// The entries of the table a call matches: by the class of its receiver, else by what the
    /// call resolves to (a facade, a mixin), else, for a function, by its name.
    pub(crate) fn matches(&self, call: &SyntaxNode) -> Vec<&'static Sink> {
        let range = call.text_range();
        if let Some(found) = self.resolved.borrow().get(&range) {
            return found.clone();
        }
        let found = self.resolve(call);
        self.resolved.borrow_mut().insert(range, found.clone());
        found
    }

    fn resolve(&self, call: &SyntaxNode) -> Vec<&'static Sink> {
        let Some(name) = call_name(call) else {
            return Vec::new();
        };
        let entries = sinks::named(crate::short(&name));
        if entries.is_empty() {
            return Vec::new();
        }
        let index = self.ctx.index;
        let analyzer = self.ctx.analyzer(call);
        let env = analyzer.env_around(call);
        let of_class = |classes: &[String]| -> Vec<&'static Sink> {
            entries
                .iter()
                .filter(|sink| {
                    sink.class.as_ref().is_some_and(|wanted| {
                        classes
                            .iter()
                            .any(|class| class.eq_ignore_ascii_case(wanted) || index.is_subclass_of(class, wanted))
                    })
                })
                .collect()
        };
        let classes: Vec<String> = match call.kind() {
            NEW_EXPR => match call.children().find(|child| child.kind() == NAME) {
                Some(class) => vec![analyzer.resolver.resolve_class(&text_of(&class))],
                None => Vec::new(),
            },
            _ => {
                let Some(callee) = call.children().next() else {
                    return Vec::new();
                };
                match callee.kind() {
                    NAME => {
                        let candidates = analyzer.resolver.function_candidates(&name);
                        let resolved = index.first_function(&candidates).map(|found| found.decl.name.clone());
                        return entries
                            .iter()
                            .filter(|sink| sink.class.is_none())
                            .filter(|sink| match &resolved {
                                Some(resolved) => resolved.eq_ignore_ascii_case(&sink.name),
                                None => candidates
                                    .iter()
                                    .any(|candidate| candidate.eq_ignore_ascii_case(&sink.name)),
                            })
                            .collect();
                    }
                    PROPERTY_FETCH_EXPR => match callee.children().next() {
                        Some(object) => {
                            let ty = analyzer.receiver_type(&analyzer.type_of(&object, &env));
                            ty.class_names().into_iter().map(str::to_string).collect()
                        }
                        None => Vec::new(),
                    },
                    SCOPED_ACCESS_EXPR => match callee.children().next() {
                        Some(qualifier) => {
                            let ty = analyzer.receiver_type(&analyzer.qualifier_type(&qualifier, &env));
                            ty.class_names().into_iter().map(str::to_string).collect()
                        }
                        None => Vec::new(),
                    },
                    _ => Vec::new(),
                }
            }
        };
        let found = of_class(&classes);
        if !found.is_empty() || call.kind() == NEW_EXPR {
            return found;
        }
        let mut through: Vec<String> = Vec::new();
        for resolved in analyzer.callees(call, &env) {
            if let Some((class, _)) = resolved.name.split_once("::") {
                through.push(class.to_string());
            }
            if let Some(receiver) = &resolved.receiver {
                through.extend(receiver.class_names().into_iter().map(str::to_string));
            }
        }
        of_class(&through)
    }

    /// What the call a string is passed to as `argument` makes of it.
    fn sink_use(&self, argument: &SyntaxNode, call: &SyntaxNode) -> Option<Use> {
        let arguments = scope::arguments(call);
        let count = arguments.len();
        let (position, name) = arguments
            .iter()
            .enumerate()
            .find(|(_, (_, node))| node == argument)
            .map(|(position, (name, _))| (position, name.clone()))?;
        for sink in self.matches(call) {
            let Some(place) = sink.place_of(position, name.as_deref(), count) else {
                continue;
            };
            let Some(kind) = sink.kind_at(place) else {
                continue;
            };
            if sink.refine {
                let outer = passed_to(call);
                let kind = outer.as_ref().and_then(|outer| self.context_kind(outer)).or(kind);
                let tables = outer.map(|outer| scope::tables(self, &outer)).unwrap_or_default();
                return Some(Use {
                    kind,
                    dialect: sink.dialect,
                    format: sink.format,
                    tables,
                });
            }
            let partial = kind.is_none_or(|kind| kind.is_partial() && kind != FragmentKind::TableReference);
            let tables = if partial { scope::tables(self, call) } else { Vec::new() };
            return Some(Use {
                kind,
                dialect: sink.dialect,
                format: sink.format,
                tables,
            });
        }
        None
    }

    /// What an expression a wrapper such as `literal()` makes reads as when it is passed to a call:
    /// an item of `ORDER BY` for `orderBy()`, by the name of the call alone, since the wrapper's
    /// result goes to the builder of the same library.
    fn context_kind(&self, call: &SyntaxNode) -> Option<FragmentKind> {
        let name = call_name(call)?;
        sinks::named(&name).iter().find_map(|sink| sink.context)
    }

    /// What the strings a variable holds are, from the calls of its function it is passed to.
    fn variables_of(&self, scope: &SyntaxNode) -> Variables {
        let range = scope.text_range();
        if let Some(found) = self.variables.borrow().get(&range) {
            return found.clone();
        }
        let mut variables = Variables::default();
        for node in scope.descendants() {
            match node.kind() {
                CALL_EXPR if is_sprintf(self, &node) => {
                    let format = scope::arguments(&node)
                        .first()
                        .and_then(|(_, argument)| argument.children().last())
                        .map(|expression| unwrap_parens(&expression))
                        .filter(|expression| expression.kind() == VARIABLE_EXPR);
                    if let Some(variable) = format {
                        variables.formatted.insert(text_of(&variable));
                    }
                }
                CALL_EXPR | NEW_EXPR => {
                    let takes_sql = call_name(&node)
                        .is_some_and(|name| sinks::named(crate::short(&name)).iter().any(Sink::takes_sql));
                    if !takes_sql {
                        continue;
                    }
                    for (_, argument) in scope::arguments(&node) {
                        let Some(expression) = argument.children().last() else {
                            continue;
                        };
                        let expression = unwrap_parens(&expression);
                        if expression.kind() != VARIABLE_EXPR {
                            continue;
                        }
                        let name = text_of(&expression);
                        if variables.uses.contains_key(&name) {
                            continue;
                        }
                        if let Some(found) = self.sink_use(&argument, &node) {
                            variables.uses.insert(name, found);
                        }
                    }
                }
                ASSIGN_EXPR if assignment_operator(&node) == Some(DOT_ASSIGN) => {
                    if let Some(target) = node.children().next().filter(|target| target.kind() == VARIABLE_EXPR) {
                        variables.appended.insert(text_of(&target));
                    }
                }
                _ => {}
            }
        }
        self.variables.borrow_mut().insert(range, variables.clone());
        variables
    }

    /// The first string expression of a statement, which a marker before the statement is for.
    fn first_string_of(&self, statement: &SyntaxNode) -> Option<TextRange> {
        let range = statement.text_range();
        if let Some(found) = self.first_strings.borrow().get(&range) {
            return *found;
        }
        let found = statement
            .descendants()
            .find(strings::is_string_leaf)
            .map(|leaf| strings::root_of(&leaf).text_range());
        self.first_strings.borrow_mut().insert(range, found);
        found
    }

    /// The marker of a string: a comment right before it, before the call that formats it, or
    /// before its statement when it is the first string there, or the label of a heredoc in it.
    fn marker(&self, root: &SyntaxNode, context: &SyntaxNode, pieces: &Pieces) -> Option<Marker> {
        if let Some(marker) = pieces.labels.iter().find_map(|label| markers::label_marker(label)) {
            return Some(marker);
        }
        if let Some(marker) = markers::before(root).or_else(|| markers::before(context)) {
            return Some(marker);
        }
        let statement = markers::statement_of(context);
        if self.first_string_of(&statement) == Some(root.text_range()) {
            return markers::before(&statement);
        }
        None
    }

    /// Whether a string is an argument Doctrine reads as DQL.
    fn is_dql(&self, expression: &SyntaxNode) -> bool {
        let Some((argument, _)) = argument_of(expression) else {
            return false;
        };
        let analyzer = self.ctx.analyzer(&argument);
        crate::frameworks::dql::is_dql_argument(&analyzer, &argument)
    }

    /// What a string is, if it is SQL.
    fn decide(&self, root: &SyntaxNode) -> Option<Embedded> {
        let (context, mut format) = match argument_of(root) {
            Some((argument, call))
                if is_sprintf(self, &call)
                    && scope::arguments(&call).first().map(|(_, node)| node) == Some(&argument) =>
            {
                (call, Some(Format::Printf))
            }
            _ => (root.clone(), None),
        };
        let probe = strings::pieces(root, None);
        if !probe.has_text() {
            return None;
        }
        let marker = if self.detection.markers {
            self.marker(root, &context, &probe)
        } else {
            None
        };
        let mut appended = false;
        let mut found = None;
        if self.detection.sinks || marker.is_some() {
            if let Some((argument, call)) = argument_of(&context) {
                found = self.sink_use(&argument, &call);
            } else if let Some((variable, scope)) = assigned_variable(&context) {
                let variables = self.variables_of(&scope);
                appended = variables.appended.contains(&variable);
                if variables.formatted.contains(&variable) {
                    format = Some(Format::Printf);
                }
                found = variables.uses.get(&variable).cloned();
            }
        }
        if let Some(sink) = &found {
            format = sink.format.or(format);
        }
        let pieces = if format.is_some() {
            strings::pieces(root, format)
        } else {
            probe
        };
        if let Some(marker) = marker {
            let tables = found.as_ref().map(|sink| sink.tables.clone()).unwrap_or_default();
            let fragment = match found.as_ref().map(|sink| sink.kind) {
                Some(Some(kind)) => pieces.fragment(kind, &tables, appended),
                _ if starts_statement(&pieces.plain_text()) => {
                    pieces.fragment(FragmentKind::Statements, &tables, appended)
                }
                _ => best_part(&pieces, &tables, appended),
            };
            return Some(Embedded {
                range: root.text_range(),
                fragment,
                dialect: marker.dialect.or(found.and_then(|sink| sink.dialect)),
                marked_dialect: marker.dialect.is_some(),
                reason: Reason::Marker,
            });
        }
        if let Some(sink) = found.filter(|_| self.detection.sinks) {
            return Some(Embedded {
                range: root.text_range(),
                fragment: match sink.kind {
                    Some(kind) => pieces.fragment(kind, &sink.tables, appended),
                    None => best_part(&pieces, &sink.tables, appended),
                },
                dialect: sink.dialect,
                marked_dialect: false,
                reason: Reason::Sink,
            });
        }
        let text = pieces.plain_text();
        if self.detection.heuristic
            && heuristic_place(&context)
            && starts_statement(&text)
            && looks_like_a_query(&text, pieces.has_holes())
            && !self.is_dql(&context)
        {
            let fragment = pieces.fragment(FragmentKind::Statements, &[], appended);
            if fragment.confidence(Dialect::Generic) >= self.detection.threshold {
                return Some(Embedded {
                    range: root.text_range(),
                    fragment,
                    dialect: None,
                    marked_dialect: false,
                    reason: Reason::Heuristic,
                });
            }
        }
        None
    }
}

/// The kinds a marked string that is no whole statement may be, in the order they are preferred.
const PARTS: [FragmentKind; 3] = [FragmentKind::Expression, FragmentKind::Condition, FragmentKind::Clauses];

/// A marked part of a query, read as the kind that has the fewest syntax errors: an expression, a
/// condition, or the clauses that end a query (`JOIN ...`, `WHERE ...`).
fn best_part(pieces: &Pieces, tables: &[ScopeTable], appended: bool) -> Fragment {
    let env = sql_embed::Environment::new(sql_embed::Settings::default(), None, None);
    let mut best: Option<(usize, Fragment)> = None;
    for kind in PARTS {
        let fragment = pieces.fragment(kind, tables, appended);
        let errors = sql_embed::Analysis::new(&env, &fragment)
            .diagnostics()
            .iter()
            .filter(|diagnostic| diagnostic.code == "syntax")
            .count();
        if best.as_ref().is_none_or(|(fewest, _)| errors < *fewest) {
            let done = errors == 0;
            best = Some((errors, fragment));
            if done {
                break;
            }
        }
    }
    best.map_or_else(
        || pieces.fragment(FragmentKind::Expression, &[], appended),
        |(_, fragment)| fragment,
    )
}

fn unwrap_parens(node: &SyntaxNode) -> SyntaxNode {
    let mut current = node.clone();
    while current.kind() == PAREN_EXPR {
        match current.children().next() {
            Some(inner) => current = inner,
            None => break,
        }
    }
    current
}

fn assignment_operator(assign: &SyntaxNode) -> Option<php_syntax::SyntaxKind> {
    assign
        .children_with_tokens()
        .filter_map(php_syntax::SyntaxElement::into_token)
        .map(|token| token.kind())
        .find(|kind| !kind.is_trivia())
}

/// The variable a string is assigned to with `=`, and the function or file around it.
fn assigned_variable(expression: &SyntaxNode) -> Option<(String, SyntaxNode)> {
    let node = outermost(expression);
    let assign = node.parent().filter(|parent| parent.kind() == ASSIGN_EXPR)?;
    if assignment_operator(&assign) != Some(ASSIGN) || assign.children().last().as_ref() != Some(&node) {
        return None;
    }
    let target = assign
        .children()
        .next()
        .filter(|target| target.kind() == VARIABLE_EXPR)?;
    let scope = assign
        .ancestors()
        .find(|ancestor| crate::ast::is_function_like(ancestor.kind()))
        .or_else(|| assign.ancestors().last())?;
    Some((text_of(&target), scope))
}

/// Whether a string stands where the heuristic looks: passed to a call, assigned with `=`,
/// returned, or the value of a constant or a property.
fn heuristic_place(expression: &SyntaxNode) -> bool {
    let node = outermost(expression);
    let Some(parent) = node.parent() else {
        return false;
    };
    match parent.kind() {
        ARGUMENT | RETURN_STATEMENT | CONST_ELEMENT | PROPERTY_ELEMENT => true,
        ASSIGN_EXPR => assignment_operator(&parent) == Some(ASSIGN) && parent.children().last().as_ref() == Some(&node),
        _ => false,
    }
}

/// The string expressions of a document, each once, in order.
fn roots(root: &SyntaxNode) -> Vec<SyntaxNode> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for leaf in root.descendants().filter(strings::is_string_leaf) {
        let found = strings::root_of(&leaf);
        if seen.insert(found.text_range()) {
            out.push(found);
        }
    }
    out
}

/// Every string of a document that holds SQL.
pub fn embedded(index: &Index, root: &SyntaxNode, detection: Detection) -> Vec<Embedded> {
    let finder = Finder::new(index, root, detection);
    roots(root)
        .iter()
        .filter_map(|candidate| finder.decide(candidate))
        .collect()
}

/// The string at an offset, when it holds SQL.
pub fn embedded_at(index: &Index, root: &SyntaxNode, offset: u32, detection: Detection) -> Option<Embedded> {
    let node = crate::ast::node_at(root, offset);
    let leaf = node.ancestors().find(strings::is_string_leaf)?;
    let candidate = strings::root_of(&leaf);
    if !(start(&candidate)..=crate::ast::end(&candidate)).contains(&offset) {
        return None;
    }
    Finder::new(index, root, detection).decide(&candidate)
}

#[cfg(test)]
mod tests;
