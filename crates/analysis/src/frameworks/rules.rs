//! Laravel's validation rules written as strings: `'email' => 'required|email|unique:users,email'`
//! in an array a validator reads (the `rules()` of a form request, or an argument the overlay marks
//! `@rules`: `$request->validate()`, `Validator::make()`, `validator()`). A rule is the method the
//! validator checks it with (`required_if` is `validateRequiredIf()`), `exists:` and `unique:` name
//! a table and its column, and the rules that compare fields name other keys of the array.

use php_index::Type;
use php_index::framework::validation::{CustomRules, rule_method, rule_name};
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, TextRange};

use crate::ast::range_of;
use crate::context::FileContext;
use crate::infer::Analyzer;

const VALIDATOR: &str = "Illuminate\\Validation\\Validator";
const FORM_REQUEST: &str = "Illuminate\\Foundation\\Http\\FormRequest";

/// The rules whose every parameter is a field.
const FIELD_LISTS: &[&str] = &[
    "required_with",
    "required_with_all",
    "required_without",
    "required_without_all",
    "prohibits",
    "missing_with",
    "missing_with_all",
    "present_with",
    "present_with_all",
    "exclude_with",
    "exclude_without",
];

/// The rules whose first parameter is a field.
const FIELD_FIRST: &[&str] = &[
    "required_if",
    "required_unless",
    "required_if_accepted",
    "required_if_declined",
    "prohibited_if",
    "prohibited_unless",
    "exclude_if",
    "exclude_unless",
    "accepted_if",
    "declined_if",
    "missing_if",
    "missing_unless",
    "present_if",
    "present_unless",
    "same",
    "different",
    "gt",
    "gte",
    "lt",
    "lte",
    "in_array",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    Rule {
        name: String,
        range: TextRange,
    },
    Table {
        name: String,
        range: TextRange,
    },
    Column {
        table: String,
        name: String,
        range: TextRange,
    },
    Field {
        name: String,
        range: TextRange,
    },
}

impl Part {
    pub fn range(&self) -> TextRange {
        match self {
            Part::Rule { range, .. }
            | Part::Table { range, .. }
            | Part::Column { range, .. }
            | Part::Field { range, .. } => *range,
        }
    }
}

/// An array of rules by field.
#[derive(Clone, Debug, Default)]
pub struct RuleSet {
    pub fields: Vec<(String, TextRange)>,
    pub parts: Vec<Part>,
}

/// The rule arrays of a file.
pub fn rule_sets(ctx: &FileContext<'_>) -> Vec<RuleSet> {
    if !ctx.index.frameworks().laravel || ctx.index.class(VALIDATOR).is_none() {
        return Vec::new();
    }
    ctx.root
        .descendants()
        .filter(|node| node.kind() == ARRAY_EXPR)
        .filter(|array| is_rules_array(ctx, array))
        .map(|array| read_set(&array))
        .collect()
}

fn is_rules_array(ctx: &FileContext<'_>, array: &SyntaxNode) -> bool {
    if array.parent().is_some_and(|parent| parent.kind() == RETURN_STATEMENT) {
        return returned_by_rules(ctx, array);
    }
    if array.parent().is_some_and(|parent| parent.kind() == ARGUMENT) {
        let analyzer = ctx.analyzer(array);
        return super::keys::is_rules_argument(&analyzer, array);
    }
    false
}

/// The array a form request's `rules()` returns.
fn returned_by_rules(ctx: &FileContext<'_>, array: &SyntaxNode) -> bool {
    let Some(method) = array.ancestors().find(|node| node.kind() == METHOD_DECLARATION) else {
        return false;
    };
    let named_rules = method
        .children()
        .find(|child| child.kind() == NAME)
        .is_some_and(|name| name.text() == "rules");
    if !named_rules {
        return false;
    }
    let analyzer = ctx.analyzer(array);
    analyzer
        .class
        .as_ref()
        .is_some_and(|class| ctx.index.is_subclass_of(&class.name, FORM_REQUEST))
}

fn read_set(array: &SyntaxNode) -> RuleSet {
    let mut set = RuleSet::default();
    let mut values = Vec::new();
    for item in array.children().filter(|node| node.kind() == ARRAY_ITEM) {
        let parts: Vec<SyntaxNode> = item.children().collect();
        let [key, value] = parts.as_slice() else {
            continue;
        };
        if let Some((name, span)) = php_index::test_facts::string_value(key) {
            set.fields.push((name, range_of(span.start, span.end)));
        }
        values.push(value.clone());
    }
    for value in values {
        match value.kind() {
            LITERAL => {
                if let Some((text, span)) = php_index::test_facts::string_value(&value) {
                    let mut at = span.start;
                    for rule in text.split('|') {
                        parse_rule(rule, at, &mut set.parts);
                        at += rule.len() as u32 + 1;
                    }
                }
            }
            ARRAY_EXPR => {
                for element in value
                    .children()
                    .filter(|node| node.kind() == ARRAY_ITEM)
                    .filter_map(|item| item.children().last())
                {
                    if let Some((text, span)) = php_index::test_facts::string_value(&element) {
                        parse_rule(&text, span.start, &mut set.parts);
                    }
                }
            }
            _ => {}
        }
    }
    set
}

/// One rule, `name:param,param`, written at `start`.
fn parse_rule(rule: &str, start: u32, out: &mut Vec<Part>) {
    let leading = rule.len() - rule.trim_start().len();
    let (name, params) = match rule.split_once(':') {
        Some((name, params)) => (name, Some(params)),
        None => (rule, None),
    };
    let trimmed = name.trim();
    let name_start = start + leading as u32;
    out.push(Part::Rule {
        name: trimmed.to_string(),
        range: range_of(name_start, name_start + trimmed.len() as u32),
    });
    let Some(params) = params else {
        return;
    };
    let lower = trimmed.to_ascii_lowercase();
    if matches!(lower.as_str(), "regex" | "not_regex" | "date_format") {
        return;
    }
    let mut at = start + name.len() as u32 + 1;
    let pieces: Vec<(&str, u32)> = params
        .split(',')
        .map(|piece| {
            let here = at;
            at += piece.len() as u32 + 1;
            (piece, here)
        })
        .collect();
    let part_of = |(piece, here): (&str, u32)| -> (String, TextRange) {
        let leading = piece.len() - piece.trim_start().len();
        let text = piece.trim();
        let from = here + leading as u32;
        (text.to_string(), range_of(from, from + text.len() as u32))
    };
    if matches!(lower.as_str(), "exists" | "unique") {
        let Some(first) = pieces.first().copied() else {
            return;
        };
        let (table, range) = part_of(first);
        // `connection.table` and a model class name a table this does not follow.
        if table.contains(['.', '\\']) {
            return;
        }
        out.push(Part::Table {
            name: table.clone(),
            range,
        });
        if let Some(second) = pieces.get(1).copied() {
            let (column, range) = part_of(second);
            if !column.eq_ignore_ascii_case("null") {
                out.push(Part::Column {
                    table,
                    name: column,
                    range,
                });
            }
        }
        return;
    }
    let fields: Vec<(&str, u32)> = if FIELD_LISTS.contains(&lower.as_str()) {
        pieces
    } else if FIELD_FIRST.contains(&lower.as_str()) {
        pieces.into_iter().take(1).collect()
    } else {
        Vec::new()
    };
    for piece in fields {
        let (name, range) = part_of(piece);
        out.push(Part::Field { name, range });
    }
}

/// The part of a rule array under an offset, with its array.
pub fn part_at(ctx: &FileContext<'_>, offset: u32) -> Option<(Part, RuleSet)> {
    let literal = crate::ast::node_at(&ctx.root, offset)
        .ancestors()
        .find(|node| node.kind() == LITERAL)?;
    let array = literal
        .ancestors()
        .filter(|node| node.kind() == ARRAY_EXPR)
        .find(|array| is_rules_array(ctx, array))?;
    let set = read_set(&array);
    let part = set
        .parts
        .iter()
        .find(|part| {
            let range = part.range();
            u32::from(range.start()) <= offset && offset <= u32::from(range.end())
        })?
        .clone();
    Some((part, set))
}

/// The method a rule is checked with, when the validator has it.
pub fn rule_target(analyzer: &Analyzer<'_>, name: &str) -> Option<(Type, String)> {
    let validator = Type::class(VALIDATOR);
    let method = analyzer.index.find_method(&validator, &rule_method(name))?;
    Some((validator, method.member.name.clone()))
}

/// Every rule a project can write: the validator's and the ones it adds.
pub fn rule_names(analyzer: &Analyzer<'_>) -> Vec<String> {
    let mut names: Vec<String> = analyzer
        .index
        .methods(&Type::class(VALIDATOR))
        .iter()
        .filter_map(|method| rule_name(&method.member.name))
        .collect();
    names.extend(analyzer.index.section::<CustomRules>().names.iter().cloned());
    names.extend(["int".to_string(), "bool".to_string()]);
    names.sort();
    names.dedup();
    names
}

/// Whether a rule is certainly not one the validator knows.
pub fn is_unknown_rule(analyzer: &Analyzer<'_>, name: &str) -> bool {
    let custom = analyzer.index.section::<CustomRules>();
    !name.is_empty()
        && !custom.unknown
        && !custom.names.iter().any(|known| known == name)
        && rule_target(analyzer, name).is_none()
}
