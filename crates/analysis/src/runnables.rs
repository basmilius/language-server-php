//! What a file can run: PHPUnit classes and test methods, Pest tests, describe blocks and
//! architecture tests, and the console commands of Laravel and Symfony. The filter of a test is what
//! `--filter` takes, so a client can run one without reading the file; the filter of a command is its
//! name. The server runs nothing itself.

use php_index::Index;
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, TextRange};

use crate::ast::{child_of, has_token, range_of};
use crate::context::FileContext;
use crate::pest::calls::{Dsl, Statement, body_of, statements_in};
use crate::phpunit::{ATTRIBUTES, is_test_class};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunnableKind {
    PhpUnit,
    Pest,
    /// A command of `php artisan`.
    Artisan,
    /// A command of `bin/console`.
    Console,
}

/// What a runnable stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RunnableScope {
    Class,
    Method,
    Test,
    Describe,
    Arch,
    Command,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Runnable {
    pub kind: RunnableKind,
    pub scope: RunnableScope,
    pub label: String,
    /// The name of the class or method, or the Pest function that declares the test.
    pub range: TextRange,
    /// The regular expression `--filter` takes, with its delimiters.
    pub filter: String,
}

/// Everything a file declares that can be run, in the order of the file.
pub fn runnables(index: &Index, root: &SyntaxNode) -> Vec<Runnable> {
    let ctx = FileContext::new(index, root);
    let mut out = Vec::new();
    phpunit_runnables(&ctx, &mut out);
    pest_runnables(root, &[], &mut out);
    command_runnables(&ctx, &mut out);
    out.sort_by_key(|runnable| runnable.range.start());
    out
}

const LARAVEL_COMMAND: &str = "Illuminate\\Console\\Command";
const SYMFONY_COMMAND: &str = "Symfony\\Component\\Console\\Command\\Command";

/// The console commands a file declares: the name of an `#[AsCommand]`, the first word of a Laravel
/// `$signature` or its `$name`.
fn command_runnables(ctx: &FileContext<'_>, out: &mut Vec<Runnable>) {
    let frameworks = ctx.index.frameworks();
    if !(frameworks.laravel || frameworks.symfony) {
        return;
    }
    for class in ctx.root.descendants().filter(|node| node.kind() == CLASS_DECLARATION) {
        let analyzer = ctx.analyzer(&class);
        let Some(context) = &analyzer.class else {
            continue;
        };
        let is_abstract = child_of(&class, MODIFIER_LIST).is_some_and(|list| has_token(&list, ABSTRACT_KW));
        let (kind, base) = if frameworks.laravel {
            (RunnableKind::Artisan, LARAVEL_COMMAND)
        } else {
            (RunnableKind::Console, SYMFONY_COMMAND)
        };
        if is_abstract || !ctx.index.is_subclass_of(&context.name, base) || context.name.eq_ignore_ascii_case(base) {
            continue;
        }
        let (Some(found), Some(name)) = (ctx.index.class(&context.name), child_of(&class, NAME)) else {
            continue;
        };
        let unquote = |text: &str| text.trim().trim_matches(['\'', '"']).to_string();
        let from_attribute = found.decl.attributes.iter().find_map(|attribute| {
            attribute.name.ends_with("\\AsCommand").then(|| {
                attribute
                    .args
                    .iter()
                    .find(|arg| arg.name.as_deref() == Some("name"))
                    .or_else(|| attribute.args.first().filter(|arg| arg.name.is_none()))
                    .map(|arg| unquote(&arg.value))
            })?
        });
        let from_property = |property: &str| {
            found
                .decl
                .property(property)
                .and_then(|found| found.default.as_ref())
                .map(|default| unquote(default))
        };
        let command = from_attribute
            .or_else(|| {
                from_property("signature").and_then(|signature| signature.split_whitespace().next().map(str::to_string))
            })
            .or_else(|| from_property("name"))
            .or_else(|| from_property("defaultName"))
            .filter(|command| !command.is_empty());
        if let Some(command) = command {
            out.push(Runnable {
                kind,
                scope: RunnableScope::Command,
                label: command.clone(),
                range: name.text_range(),
                filter: command,
            });
        }
    }
}

fn phpunit_runnables(ctx: &FileContext<'_>, out: &mut Vec<Runnable>) {
    for class in ctx.root.descendants().filter(|node| node.kind() == CLASS_DECLARATION) {
        let analyzer = ctx.analyzer(&class);
        let Some(context) = &analyzer.class else {
            continue;
        };
        let is_abstract = child_of(&class, MODIFIER_LIST).is_some_and(|list| has_token(&list, ABSTRACT_KW));
        if is_abstract || !is_test_class(ctx.index, &context.name) {
            continue;
        }
        let Some(name) = child_of(&class, NAME) else {
            continue;
        };
        let qualified = regex_escape(&context.name);
        out.push(Runnable {
            kind: RunnableKind::PhpUnit,
            scope: RunnableScope::Class,
            label: crate::short(&context.name).to_string(),
            range: name.text_range(),
            filter: format!("/^{qualified}::/"),
        });
        let Some(body) = child_of(&class, CLASS_BODY) else {
            continue;
        };
        for method in body.children().filter(|node| node.kind() == METHOD_DECLARATION) {
            let Some(name) = child_of(&method, NAME) else {
                continue;
            };
            let written = name.text().to_string();
            if !is_test_method_node(&method, &written, &analyzer.resolver) {
                continue;
            }
            out.push(Runnable {
                kind: RunnableKind::PhpUnit,
                scope: RunnableScope::Method,
                label: format!("{}::{written}", crate::short(&context.name)),
                range: name.text_range(),
                filter: format!("/^{qualified}::{written}( with data set .*)?$/"),
            });
        }
    }
}

/// Whether a method declaration is a test: public, named `test...`, or marked as one.
fn is_test_method_node(method: &SyntaxNode, name: &str, resolver: &php_index::NameResolver) -> bool {
    let modifiers = child_of(method, MODIFIER_LIST);
    let has = |kind| modifiers.as_ref().is_some_and(|list| has_token(list, kind));
    if has(PRIVATE_KW) || has(PROTECTED_KW) || has(STATIC_KW) || has(ABSTRACT_KW) {
        return false;
    }
    if name.starts_with("test") {
        return true;
    }
    let marked = method
        .children()
        .filter(|node| node.kind() == ATTRIBUTE_LIST)
        .flat_map(|list| list.children().collect::<Vec<_>>())
        .filter(|node| node.kind() == ATTRIBUTE)
        .filter_map(|attribute| child_of(&attribute, NAME))
        .any(|attribute| {
            resolver
                .resolve_class(&attribute.text().to_string())
                .strip_prefix(ATTRIBUTES)
                .is_some_and(|short| short.eq_ignore_ascii_case("Test"))
        });
    marked
        || method
            .children_with_tokens()
            .filter_map(php_syntax::SyntaxElement::into_token)
            .any(|token| token.kind() == DOC_COMMENT && doc_has_tag(token.text(), "@test"))
}

fn doc_has_tag(text: &str, tag: &str) -> bool {
    text.lines().any(|line| {
        let line = line.trim();
        let line = line.strip_prefix("/**").unwrap_or(line);
        let line = line.strip_suffix("*/").unwrap_or(line);
        let line = line.trim().trim_start_matches('*').trim();
        line == tag
            || line
                .strip_prefix(tag)
                .is_some_and(|rest| rest.starts_with(char::is_whitespace))
    })
}

fn pest_runnables(container: &SyntaxNode, describing: &[String], out: &mut Vec<Runnable>) {
    for statement in statements_in(container) {
        match statement.dsl {
            Dsl::Test | Dsl::It | Dsl::Arch => pest_test(&statement, describing, out),
            Dsl::Describe => {
                let Some(description) = statement.description() else {
                    continue;
                };
                let mut inner = describing.to_vec();
                inner.push(description);
                out.push(Runnable {
                    kind: RunnableKind::Pest,
                    scope: RunnableScope::Describe,
                    label: inner.join(" → "),
                    range: call_name_range(&statement),
                    filter: format!("/::{}/", evaluable(&format!("{} → ", describe_prefix(&inner)))),
                });
                if let Some(body) = statement.closure().as_ref().and_then(body_of) {
                    pest_runnables(&body, &inner, out);
                }
            }
            _ => {}
        }
    }
}

fn pest_test(statement: &Statement, describing: &[String], out: &mut Vec<Runnable>) {
    let (scope, description) = match (statement.dsl, statement.description()) {
        (Dsl::It, Some(description)) => (RunnableScope::Test, format!("it {description}")),
        (Dsl::Test, Some(description)) => (RunnableScope::Test, description),
        (Dsl::Arch, Some(description)) => (RunnableScope::Arch, description),
        (Dsl::Arch, None) => {
            // An architecture test without a description is named after the calls chained to it, which
            // only the first of can be written out here, so the filter reaches every test it starts.
            let Some(first) = statement.chain.get(1) else {
                return;
            };
            let prefix = evaluable(&first.name);
            out.push(Runnable {
                kind: RunnableKind::Pest,
                scope: RunnableScope::Arch,
                label: format!("arch {}", first.name),
                range: call_name_range(statement),
                filter: format!("/::{}/", prefix_describing(describing, &prefix)),
            });
            return;
        }
        _ => return,
    };
    let full = if describing.is_empty() {
        description.clone()
    } else {
        format!("{} → {description}", describe_prefix(describing))
    };
    let mut label = describing.to_vec();
    label.push(description);
    out.push(Runnable {
        kind: RunnableKind::Pest,
        scope,
        label: label.join(" → "),
        range: call_name_range(statement),
        filter: format!("/::{}( with data set .*)?$/", evaluable(&full)),
    });
}

/// The test name prefix of the describe blocks around a test: `` `a` → `b` ``.
fn describe_prefix(describing: &[String]) -> String {
    describing
        .iter()
        .map(|description| format!("`{description}`"))
        .collect::<Vec<_>>()
        .join(" → ")
}

fn prefix_describing(describing: &[String], evaluable_name: &str) -> String {
    if describing.is_empty() {
        return evaluable_name.to_string();
    }
    evaluable(&format!("{} → ", describe_prefix(describing)))
}

fn call_name_range(statement: &Statement) -> TextRange {
    statement
        .root()
        .node
        .children()
        .next()
        .map_or_else(|| range_of(0, 0), |name| name.text_range())
}

/// A test description as the name of the method Pest makes of it: what is not a letter, a digit or
/// an underscore becomes an underscore, and an underscore itself is doubled.
pub fn evaluable(description: &str) -> String {
    let mut name = String::from("__pest_evaluable_");
    for c in description.chars() {
        match c {
            '_' => name.push_str("__"),
            c if c.is_ascii_alphanumeric() || !c.is_ascii() => name.push(c),
            _ => name.push('_'),
        }
    }
    name
}

fn regex_escape(name: &str) -> String {
    name.replace('\\', "\\\\")
}

#[cfg(test)]
mod tests;
