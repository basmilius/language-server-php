//! The type the position of the cursor asks for: the other side of a comparison, the property an
//! assignment fills, the parameter an argument is for, the declared return type, or the subject of a
//! `match` or `switch`. The values of that type come before every other name.

use php_index::{ClassKind, Type};
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, SyntaxToken};

use super::{Builder, CompletionItem, ItemKind, match_score};
use crate::ast::{has_token, tokens};

impl Builder<'_> {
    pub(super) fn expected_values(&mut self, token: &SyntaxToken) {
        if self.typed().starts_with('$') {
            return;
        }
        let Some(expected) = self.expected_type(token) else {
            return;
        };
        for member in expected.members() {
            match member {
                Type::Class { name, .. } => self.enum_cases(name),
                Type::Bool => self.preferred_keywords.extend(["true", "false"]),
                Type::True => self.preferred_keywords.push("true"),
                Type::False => self.preferred_keywords.push("false"),
                Type::Null => self.preferred_keywords.push("null"),
                _ => {}
            }
        }
    }

    fn expected_type(&self, token: &SyntaxToken) -> Option<Type> {
        let name = token.parent().filter(|node| node.kind() == NAME)?;
        let parent = name.parent()?;
        let level = self.index.level;
        let ty = match parent.kind() {
            BINARY_EXPR
                if tokens(&parent).any(|token| matches!(token.kind(), EQ | NEQ | IDENTICAL | NOT_IDENTICAL)) =>
            {
                let other = parent.children().find(|child| child != &name)?;
                self.analyzer.type_of(&other, self.env)
            }
            // A variable takes whatever is assigned to it, so only a property says what it wants.
            ASSIGN_EXPR if has_token(&parent, ASSIGN) => {
                let target = parent
                    .children()
                    .next()
                    .filter(|target| target != &name)
                    .filter(|target| matches!(target.kind(), PROPERTY_FETCH_EXPR | STATIC_PROPERTY_EXPR))?;
                self.analyzer.type_of(&target, self.env)
            }
            ARGUMENT => self.argument_type(&parent)?,
            RETURN_STATEMENT => {
                let function = crate::ast::enclosing_function(&parent)?;
                let callable = crate::inspections::types::callable_of(self.analyzer, &function);
                callable.effective_return(level)?.clone()
            }
            MATCH_ARM if !has_token(&parent, FAT_ARROW) => {
                let subject = parent
                    .parent()
                    .filter(|node| node.kind() == MATCH_EXPR)?
                    .children()
                    .next()?;
                self.analyzer.type_of(&subject, self.env)
            }
            CASE_CLAUSE => {
                let switch = parent.ancestors().find(|node| node.kind() == SWITCH_STATEMENT)?;
                self.analyzer.type_of(&switch.children().next()?, self.env)
            }
            _ => return None,
        };
        (!ty.is_unknown()).then_some(ty)
    }

    /// The type of the parameter an argument fills, by its name or its position.
    fn argument_type(&self, argument: &SyntaxNode) -> Option<Type> {
        let list = argument.parent().filter(|node| node.kind() == ARGUMENT_LIST)?;
        let callee = self.analyzer.callees(&list.parent()?, self.env).into_iter().next()?;
        let level = self.index.level;
        let params: Vec<_> = callee.callable.params_at(level).collect();
        let param = if has_token(argument, COLON) {
            let named = tokens(argument).find(|token| !token.kind().is_trivia())?;
            params.iter().find(|param| param.name == named.text()).copied()
        } else {
            let position = list
                .children()
                .filter(|node| node.kind() == ARGUMENT)
                .position(|node| &node == argument)?;
            params
                .get(position)
                .or_else(|| params.last().filter(|param| param.variadic))
                .copied()
        }?;
        let ty = param.effective_type(level)?;
        Some(ty.substitute(&callee.subst, callee.receiver.as_ref(), callee.self_name.as_deref()))
    }

    /// The cases of an enum, written as `Status::Case` with the import the name needs.
    fn enum_cases(&mut self, name: &str) {
        let Some(class) = self
            .index
            .class(name)
            .filter(|class| class.decl.kind == ClassKind::Enum)
        else {
            return;
        };
        let fqn = class.decl.name.clone();
        let short = crate::short(&fqn).to_string();
        let namespace = php_index::types::namespace_of(&fqn).to_string();
        let (written, additional) = self.class_reference(&fqn);
        let typed = self.typed().to_string();
        for (position, found) in self.index.constants_of(&Type::class(fqn.clone())).iter().enumerate() {
            let case = &found.member;
            if !case.is_case {
                continue;
            }
            let label = format!("{short}::{}", case.name);
            let Some(score) = match_score(&case.name, &typed)
                .into_iter()
                .chain(match_score(&label, &typed))
                .min()
            else {
                continue;
            };
            let item = CompletionItem {
                label: label.clone(),
                kind: ItemKind::EnumMember,
                detail: case.value.clone().map(|value| format!("= {value}")),
                description: (!namespace.is_empty()).then(|| namespace.clone()),
                edit: self.range_edit(format!("{written}::{}", case.name)),
                additional_edits: additional.clone(),
                sort_text: format!("!0{position:03}"),
                filter_text: Some(label),
                deprecated: case.doc.as_ref().is_some_and(|doc| doc.deprecated.is_some()),
                data: Some(format!("const:{fqn}::{}", case.name)),
            };
            self.push(score, item);
        }
    }
}
