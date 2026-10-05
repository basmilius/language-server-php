//! Completion of the methods a class can override or must implement: after `function ` in a class
//! body, the methods of its parents, interfaces and abstract traits that it does not declare yet,
//! each with the signature written out.

use std::collections::HashSet;

use php_index::{ClassKind, Method, Origin, Type, UseKind, Visibility};
use php_syntax::SyntaxKind::*;
use php_syntax::SyntaxNode;

use super::{Builder, CompletionItem, ItemKind, TextEdit, match_score, origin_rank};
use crate::ast::{self, child_of};
use crate::imports::{ImportPlan, import_edit, plan_import};
use crate::render;

impl Builder<'_> {
    pub(super) fn overridable_methods(&mut self, declaration: &SyntaxNode) {
        let Some(class) = self
            .analyzer
            .class
            .as_ref()
            .filter(|class| !class.anonymous && class.kind != ClassKind::Interface)
        else {
            return;
        };
        let class_name = class.name.clone();
        let own = own_method_names(declaration);
        let typed = self.typed().to_string();
        let level = self.index.level;
        let ancestors = self.index.ancestors(&Type::class(class_name));
        let mut seen: HashSet<String> = HashSet::new();
        for ancestor in ancestors.iter().skip(1) {
            let decl = ancestor.class.decl;
            for method in &decl.methods {
                let lower = method.name.to_ascii_lowercase();
                if !method.availability.contains(level)
                    || method.visibility == Visibility::Private
                    || method.is_final
                    || own.contains(&lower)
                    || ancestor.excluded.contains(&lower)
                    || !seen.insert(lower)
                {
                    continue;
                }
                let must_implement = method.is_abstract || decl.kind == ClassKind::Interface;
                if ancestor.via_trait && !must_implement {
                    continue;
                }
                let Some(score) = match_score(&method.name, &typed) else {
                    continue;
                };
                let (edit, additional_edits) = self.override_edits(declaration, method, must_implement);
                self.push(
                    score,
                    CompletionItem {
                        label: method.name.clone(),
                        kind: ItemKind::Method,
                        detail: Some(render::callable_text(&method.callable, level)),
                        description: Some(crate::short(&decl.name).to_string()),
                        edit,
                        additional_edits,
                        sort_text: format!(
                            "{}{}{}",
                            u8::from(!must_implement),
                            origin_rank(Origin::Project),
                            method.name.to_ascii_lowercase()
                        ),
                        filter_text: Some(method.name.clone()),
                        deprecated: method.doc.as_ref().is_some_and(|doc| doc.deprecated.is_some()),
                        data: Some(format!("method:{}::{}", decl.name, method.name)),
                    },
                );
            }
        }
    }

    /// The text that replaces the typed word, and the edits elsewhere that go with it: the
    /// modifiers the declaration lacks and the imports of the classes its signature names.
    fn override_edits(
        &self,
        declaration: &SyntaxNode,
        method: &Method,
        must_implement: bool,
    ) -> (TextEdit, Vec<TextEdit>) {
        let mut additional: Vec<TextEdit> = Vec::new();
        let already_has_arguments = self.text[self.offset as usize..]
            .trim_start_matches([' ', '\t'])
            .starts_with('(');
        if already_has_arguments {
            return (self.range_edit(method.name.clone()), Vec::new());
        }
        let modifiers: Vec<_> = child_of(declaration, MODIFIER_LIST)
            .map(|list| ast::tokens(&list).map(|token| token.kind()).collect())
            .unwrap_or_default();
        let mut missing = String::new();
        if !modifiers
            .iter()
            .any(|kind| matches!(kind, PUBLIC_KW | PROTECTED_KW | PRIVATE_KW))
        {
            missing.push_str(method.visibility.keyword());
            missing.push(' ');
        }
        if method.is_static && !modifiers.contains(&STATIC_KW) {
            missing.push_str("static ");
        }
        if !missing.is_empty() {
            let start = ast::start(declaration);
            additional.push(TextEdit {
                start,
                end: start,
                new_text: missing,
            });
        }
        let mut imports: Vec<TextEdit> = Vec::new();
        let signature = self.native_signature(method, &mut imports);
        additional.extend(merge_imports(imports));
        let is_abstract = modifiers.contains(&ABSTRACT_KW);
        let rest_of_line_empty = self.text[self.offset as usize..]
            .split('\n')
            .next()
            .is_some_and(|rest| rest.trim().is_empty());
        let body = if is_abstract {
            ";".to_string()
        } else if rest_of_line_empty {
            self.method_body(method, must_implement)
        } else {
            String::new()
        };
        (
            self.range_edit(format!("{}{}{}", method.name, signature, body)),
            additional,
        )
    }

    /// `\n{` and the lines inside it, in the indentation of the line being typed on.
    fn method_body(&self, method: &Method, must_implement: bool) -> String {
        let line_start = self.text[..self.word.start].rfind('\n').map_or(0, |at| at + 1);
        let indent: String = self.text[line_start..]
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();
        let step = if indent.starts_with('\t') { "\t" } else { "    " };
        let inner = if must_implement || method.is_abstract {
            String::new()
        } else {
            let arguments: Vec<String> = method
                .callable
                .params
                .iter()
                .map(|param| {
                    if param.variadic {
                        format!("...${}", param.name)
                    } else {
                        format!("${}", param.name)
                    }
                })
                .collect();
            let call = format!("parent::{}({});", method.name, arguments.join(", "));
            let returns_nothing = method.name.eq_ignore_ascii_case("__construct")
                || matches!(
                    method.callable.native_return(self.index.level),
                    Some(Type::Void | Type::Never) | None
                );
            if returns_nothing {
                call
            } else {
                format!("return {call}")
            }
        };
        format!("\n{indent}{{\n{indent}{step}{inner}\n{indent}}}")
    }

    /// `(Type $a, int $b = 5): Ret` with native types only, since PHPDoc types are not PHP.
    fn native_signature(&self, method: &Method, imports: &mut Vec<TextEdit>) -> String {
        let level = self.index.level;
        let params: Vec<String> = method
            .callable
            .params_at(level)
            .map(|param| {
                let mut out = String::new();
                if let Some(ty) = param.native_type(level) {
                    if let Some(text) = self.native_type(ty, imports) {
                        out.push_str(&text);
                        out.push(' ');
                    }
                }
                if param.by_ref {
                    out.push('&');
                }
                if param.variadic {
                    out.push_str("...");
                }
                out.push('$');
                out.push_str(&param.name);
                if let Some(default) = &param.default {
                    out.push_str(" = ");
                    out.push_str(default);
                }
                out
            })
            .collect();
        let mut out = format!("({})", params.join(", "));
        if let Some(text) = method
            .callable
            .native_return(level)
            .and_then(|ty| self.native_type(ty, imports))
        {
            out.push_str(": ");
            out.push_str(&text);
        }
        out
    }

    /// A native type as PHP writes it, or `None` for one that has no spelling there.
    fn native_type(&self, ty: &Type, imports: &mut Vec<TextEdit>) -> Option<String> {
        Some(match ty {
            Type::Mixed => "mixed".to_string(),
            Type::Void => "void".to_string(),
            Type::Never => "never".to_string(),
            Type::Null => "null".to_string(),
            Type::Bool => "bool".to_string(),
            Type::True => "true".to_string(),
            Type::False => "false".to_string(),
            Type::Int => "int".to_string(),
            Type::Float => "float".to_string(),
            Type::String => "string".to_string(),
            Type::Object => "object".to_string(),
            Type::Array(..) | Type::List(_) | Type::Shape(_) => "array".to_string(),
            Type::Iterable(..) => "iterable".to_string(),
            Type::Callable(_) => "callable".to_string(),
            Type::Static => "static".to_string(),
            Type::SelfType => "self".to_string(),
            Type::Parent => "parent".to_string(),
            Type::Class { name, .. } => self.written_class(name, imports),
            Type::Union(members) => {
                let nullable = members.len() == 2 && members.contains(&Type::Null);
                if nullable {
                    let other = members.iter().find(|member| **member != Type::Null)?;
                    format!("?{}", self.native_type(other, imports)?)
                } else {
                    let parts: Option<Vec<String>> =
                        members.iter().map(|member| self.native_type(member, imports)).collect();
                    parts?.join("|")
                }
            }
            Type::Intersection(members) => {
                let parts: Option<Vec<String>> =
                    members.iter().map(|member| self.native_type(member, imports)).collect();
                parts?.join("&")
            }
            _ => return None,
        })
    }

    /// A class as it can be written here, with the import it needs added to `imports`.
    fn written_class(&self, fqn: &str, imports: &mut Vec<TextEdit>) -> String {
        let plan = plan_import(&self.analyzer.resolver, fqn, UseKind::Class, |name| {
            let own = self.analyzer.resolver.qualify(name);
            self.index
                .class(&own)
                .is_some_and(|other| !other.decl.name.eq_ignore_ascii_case(fqn))
        });
        match plan {
            ImportPlan::Plain(name) => name,
            ImportPlan::Qualified(name) => format!("\\{name}"),
            ImportPlan::Import(name) => {
                if let Some(edit) = import_edit(self.text, self.real, self.offset, fqn, UseKind::Class) {
                    if !imports.contains(&edit) {
                        imports.push(edit);
                    }
                }
                name
            }
        }
    }
}

/// The methods a class body declares besides the one being typed.
fn own_method_names(declaration: &SyntaxNode) -> HashSet<String> {
    let Some(body) = declaration.parent() else {
        return HashSet::new();
    };
    body.children()
        .filter(|child| child.kind() == METHOD_DECLARATION && child != declaration)
        .filter_map(|method| child_of(&method, NAME).map(|name| ast::text_of(&name).to_ascii_lowercase()))
        .collect()
}

/// New `use` lines that land at the same place go in as one edit, in order, since the order of
/// separate inserts at one position is up to the client.
fn merge_imports(mut imports: Vec<TextEdit>) -> Vec<TextEdit> {
    imports.sort_by(|left, right| (left.start, left.new_text.trim()).cmp(&(right.start, right.new_text.trim())));
    let mut merged: Vec<TextEdit> = Vec::new();
    for edit in imports {
        match merged.last_mut() {
            Some(last) if last.start == edit.start && last.end == edit.end => {
                last.new_text.push_str(edit.new_text.trim_start_matches('\n'));
            }
            _ => merged.push(edit),
        }
    }
    merged
}
