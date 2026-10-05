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
