//! The fields a form request validates: the keys of the array its `rules()` returns.

use std::path::PathBuf;

use php_syntax::SyntaxKind::*;
use php_syntax::SyntaxNode;

use super::source::{Literal, array_items, literal_of, method_at, returned_expression, tree_of};
use crate::index::Index;
use crate::model::Span;
use crate::test_facts::argument_expressions;
use crate::types::Type;

const FORM_REQUEST: &str = "Illuminate\\Foundation\\Http\\FormRequest";

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub name: String,
    pub path: PathBuf,
    /// The name inside its quotes.
    pub span: Span,
}

/// The fields of a form request, read from the `rules()` of the nearest class that declares it.
pub fn fields_of(index: &Index, class: &str) -> Vec<Field> {
    let ty = Type::class(class.to_string());
    let Some(found) = index.find_declared_method(&ty, "rules") else {
        return Vec::new();
    };
    if found.class.decl.name.eq_ignore_ascii_case(FORM_REQUEST) {
        return Vec::new();
    }
    let Some(tree) = tree_of(index, &found.class.file.path) else {
        return Vec::new();
    };
    let Some(method) = method_at(&tree, found.member.name_span.start) else {
        return Vec::new();
    };
    let Some(returned) = returned_expression(&method) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for array in arrays_of(&returned) {
        for (key, _) in array_items(&array).unwrap_or_default() {
            if let Some(Literal::Text(name, span)) = key.as_ref().and_then(literal_of) {
                out.push(Field {
                    name,
                    path: found.class.file.path.clone(),
                    span,
                });
            }
        }
    }
    out
}

/// The arrays an expression is made of: the literal itself, or those `array_merge` joins.
fn arrays_of(expression: &SyntaxNode) -> Vec<SyntaxNode> {
    match expression.kind() {
        ARRAY_EXPR => vec![expression.clone()],
        CALL_EXPR => argument_expressions(expression)
            .iter()
            .filter(|argument| argument.kind() == ARRAY_EXPR)
            .cloned()
            .collect(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framework::testing::project;

    #[test]
    fn reads_the_keys_of_the_rules() {
        let index = project(&[
            (
                "vendor/laravel/FormRequest.php",
                "<?php namespace Illuminate\\Foundation\\Http; class FormRequest { public function rules() { return []; } }",
            ),
            (
                "app/Http/Requests/StoreUserRequest.php",
                "<?php namespace App\\Http\\Requests; use Illuminate\\Foundation\\Http\\FormRequest; class StoreUserRequest extends FormRequest { public function rules(): array { return ['name' => 'required', 'email' => ['required', 'email'], 'items.*.id' => 'int']; } }",
            ),
            (
                "app/Http/Requests/UpdateUserRequest.php",
                "<?php namespace App\\Http\\Requests; class UpdateUserRequest extends StoreUserRequest { public function rules(): array { return array_merge(parent::rules(), ['nickname' => 'string']); } }",
            ),
        ]);
        let names =
            |class: &str| -> Vec<String> { fields_of(&index, class).into_iter().map(|field| field.name).collect() };
        assert_eq!(
            names("App\\Http\\Requests\\StoreUserRequest"),
            ["name", "email", "items.*.id"]
        );
        assert_eq!(names("App\\Http\\Requests\\UpdateUserRequest"), ["nickname"]);
    }
}

/// The validation rules a project or a package adds with `Validator::extend('name', ...)` and its
/// kin. `unknown` is set when one is added under a name that is not written out.
#[derive(Default)]
pub struct CustomRules {
    pub names: Vec<String>,
    pub unknown: bool,
}

impl super::Section for CustomRules {
    fn build(index: &Index) -> Self {
        let mut rules = CustomRules::default();
        for file in index.files() {
            let read = match file.origin {
                crate::index::Origin::Project => true,
                crate::index::Origin::Vendor => file.summary().classes.iter().any(|class| {
                    class
                        .parents
                        .iter()
                        .any(|parent| parent.eq_ignore_ascii_case("Illuminate\\Support\\ServiceProvider"))
                }),
                crate::index::Origin::Stub => false,
            };
            if !read {
                continue;
            }
            let Some(text) = index.read_text(&file.path) else {
                continue;
            };
            if text.contains("extend") {
                rules.scan(&text);
            }
        }
        rules
    }

    // Read once: it reads every file of the project.
    fn depends_on(_: &std::path::Path, _: &std::path::Path) -> bool {
        false
    }
}

impl CustomRules {
    fn scan(&mut self, text: &str) {
        for call in ["extend(", "extendImplicit(", "extendDependent("] {
            let mut rest = text;
            while let Some(at) = rest.find(call) {
                let line_start = rest[..at].rfind('\n').map_or(0, |found| found + 1);
                let line = &rest[line_start..at];
                rest = &rest[at + call.len()..];
                let argument = rest.trim_start();
                match argument.chars().next() {
                    Some(quote @ ('\'' | '"')) => {
                        if let Some(end) = argument[1..].find(quote) {
                            let name = argument[1..1 + end].to_string();
                            if !self.names.contains(&name) {
                                self.names.push(name);
                            }
                        }
                    }
                    _ if line.to_ascii_lowercase().contains("validator") => self.unknown = true,
                    _ => {}
                }
            }
        }
    }
}

/// `required_if` is the rule the validator checks with `validateRequiredIf()`.
pub fn rule_method(name: &str) -> String {
    let studly: String = name
        .trim()
        .split(['_', '-', ' '])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut chars = word.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect::<String>())
                .unwrap_or_default()
        })
        .collect();
    let studly = match studly.as_str() {
        "Int" => "Integer".to_string(),
        "Bool" => "Boolean".to_string(),
        _ => studly,
    };
    format!("validate{studly}")
}

/// `validateRequiredIf` is the rule `required_if`.
pub fn rule_name(method: &str) -> Option<String> {
    let rest = method.strip_prefix("validate")?;
    let mut out = String::new();
    for character in rest.chars() {
        if character.is_uppercase() {
            if !out.is_empty() {
                out.push('_');
            }
            out.extend(character.to_lowercase());
        } else {
            out.push(character);
        }
    }
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod rule_tests {
    use super::*;

    #[test]
    fn rule_names_and_methods() {
        assert_eq!(rule_method("required_if"), "validateRequiredIf");
        assert_eq!(rule_method("int"), "validateInteger");
        assert_eq!(rule_name("validateDateFormat").as_deref(), Some("date_format"));
        let mut rules = CustomRules::default();
        rules.scan("Validator::extend('phone', fn () => true); Auth::extend('jwt', $x); $v->extendImplicit(\"filled_if\", $f);");
        assert_eq!(rules.names, ["phone", "jwt", "filled_if"]);
        assert!(!rules.unknown);
        rules.scan("Validator::extend($name, $callback);");
        assert!(rules.unknown);
    }
}
