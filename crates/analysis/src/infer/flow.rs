//! Variables over the statements of a scope: what they are assigned, what a branch learns about
//! them from its condition, and what is left after a branch merges back.

use std::collections::HashMap;

use php_index::phpdoc::{TypeContext, parse_var_comment};
use php_index::{AssertWhen, Type};
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxElement, SyntaxNode};

use super::calls::arguments;
use super::expr::literal_string;
use super::{Analyzer, Env};
use crate::ast::{child_of, end, first_token, has_token, start, text_of, tokens};

impl Analyzer<'_> {
    /// The variables in scope at an offset and what they are, following the statements of the
    /// enclosing function or file from its start to the offset.
    pub fn env_at(&self, offset: u32) -> Env {
        let scope = self.scope_at(offset);
        let mut env = Env::default();
        self.guarded((), || self.init_scope(&scope, &mut env, offset));
        let body = match scope.kind() {
            SOURCE_FILE => Some(scope.clone()),
            ARROW_FUNCTION_EXPR => None,
            _ => child_of(&scope, BLOCK),
        };
        if let Some(body) = body {
            self.walk(&body, &mut env, offset);
        }
        env
    }

    fn init_scope(&self, scope: &SyntaxNode, env: &mut Env, offset: u32) {
        match scope.kind() {
            SOURCE_FILE => {}
            FUNCTION_DECLARATION | METHOD_DECLARATION => self.bind_params(scope, env, &[]),
            PROPERTY_HOOK => {
                self.bind_params(scope, env, &[]);
                if !env.vars.contains_key("value") {
                    env.set("value", self.hook_value_type(scope));
                }
            }
            CLOSURE_EXPR => {
                let outer_offset = start(scope).saturating_sub(1);
                let outer = if outer_offset == offset {
                    Env::default()
                } else {
                    self.env_at(outer_offset)
                };
                let hints = self.closure_param_hints(scope, &outer);
                if let Some(uses) = child_of(scope, CLOSURE_USE) {
                    for variable in uses.children().filter(|child| child.kind() == CLOSURE_USE_VARIABLE) {
                        let Some(token) = first_token(&variable, VARIABLE) else {
                            continue;
                        };
                        let name = token.text().trim_start_matches('$').to_string();
                        let ty = outer.get(&name).cloned().unwrap_or(Type::Unknown);
                        env.set(name, ty);
                    }
                }
                self.bind_params(scope, env, &hints);
            }
            ARROW_FUNCTION_EXPR => {
                let outer_offset = start(scope).saturating_sub(1);
                if outer_offset != offset {
                    *env = self.env_at(outer_offset);
                }
                let hints = self.closure_param_hints(scope, env);
                self.bind_params(scope, env, &hints);
            }
            _ => {}
        }
    }

    /// The type a property hook's `$value` has: the type of the property it belongs to.
    fn hook_value_type(&self, hook: &SyntaxNode) -> Type {
        let declaration = hook.ancestors().find(|node| node.kind() == PROPERTY_DECLARATION);
        let Some(declaration) = declaration else {
            return Type::Unknown;
        };
        let class_scope = self.class.as_ref().map(|class| php_index::extract::ClassScope {
            name: class.name.clone(),
            parent: class.parent.clone(),
            is_trait: class.kind == php_index::ClassKind::Trait,
            templates: Vec::new(),
            aliases: Vec::new(),
        });
        let mut cx = TypeContext::new(&self.resolver);
        if let Some(scope) = &class_scope {
            cx.class_name = Some(&scope.name);
            cx.parent_name = scope.parent.as_deref();
        }
        declaration
            .children()
            .find(|child| crate::ast::is_type_node(child.kind()))
            .map_or(Type::Unknown, |ty| php_index::extract::native_type(&ty, &cx))
    }

    /// Binds the parameters of a function-like node. A closure parameter with no type of its own gets
    /// the one its call gives it (`hints`, by position).
    fn bind_params(&self, function: &SyntaxNode, env: &mut Env, hints: &[Type]) {
        let class_scope = self.class.as_ref().map(|class| php_index::extract::ClassScope {
            name: class.name.clone(),
            parent: class.parent.clone(),
            is_trait: class.kind == php_index::ClassKind::Trait,
            templates: self.class_template_names(),
            aliases: self.class_type_aliases(),
        });
        let (callable, _) = php_index::extract::callable_at(function, &self.resolver, class_scope.as_ref());
        let level = self.level();
        let inherited = self.inherited_param_types(function);
        for (position, param) in callable.params.iter().enumerate() {
            let declared = param
                .effective_type(level)
                .filter(|_| param.doc_ty.is_some())
                .cloned()
                .or_else(|| {
                    let narrower = inherited.get(&position)?;
                    self.narrows(param.native_type(level), narrower)
                        .then(|| narrower.clone())
                })
                .or_else(|| param.effective_type(level).cloned())
                .or_else(|| hints.get(position).filter(|hint| !hint.is_unknown()).cloned());
            let mut ty = declared.unwrap_or(Type::Unknown);
            if ty.is_unknown() && position == 0 {
                if let Some(builder) = self.scope_parameter_type(function) {
                    ty = builder;
                }
            }
            if param
                .default
                .as_deref()
                .is_some_and(|default| default.eq_ignore_ascii_case("null"))
                && !ty.is_unknown()
                && !ty.contains_null()
            {
                ty = ty.nullable();
            }
            if param.variadic {
                ty = Type::List(Box::new(ty));
            }
            env.set(param.name.clone(), ty);
        }
    }

    /// The builder a scope method of a model is given.
    fn scope_parameter_type(&self, function: &SyntaxNode) -> Option<Type> {
        let class = self.class.as_ref()?;
        if function.kind() != METHOD_DECLARATION || !self.index.frameworks().eloquent {
            return None;
        }
        let name = function
            .children()
            .find(|child| child.kind() == NAME)
            .map(|name| text_of(&name))?;
        php_index::framework::eloquent::scope_builder_type(self.index, &class.name, &name)
    }

    /// The documented types of the parameters of the method this one overrides, by position, for the
    /// parameters it documents nothing for. An `@implements Handler<Message>` thereby gives
    /// `handle(MessageInterface $message)` the `Message` that `@param T` of the interface stands for.
    fn inherited_param_types(&self, function: &SyntaxNode) -> HashMap<usize, Type> {
        let mut found = HashMap::new();
        let (Some(class), METHOD_DECLARATION) = (&self.class, function.kind()) else {
            return found;
        };
        let Some(name) = function
            .children()
            .find(|child| child.kind() == NAME)
            .map(|name| text_of(&name))
        else {
            return found;
        };
        for ancestor in self.index.ancestors(&Type::class(&class.name)) {
            if ancestor.class.decl.name.eq_ignore_ascii_case(&class.name) {
                continue;
            }
            let Some(method) = ancestor.class.decl.method(&name) else {
                continue;
            };
            for (position, param) in method.callable.params.iter().enumerate() {
                let Some(documented) = &param.doc_ty else {
                    continue;
                };
                let resolved = documented.substitute(&ancestor.subst, None, Some(&ancestor.self_name));
                if !resolved.has_template() {
                    found.entry(position).or_insert(resolved);
                }
            }
        }
        found
    }

    /// Whether `inherited` only says more than `native` already does, so taking it loses nothing.
    fn narrows(&self, native: Option<&Type>, inherited: &Type) -> bool {
        let Some(native) = native else {
            return true;
        };
        if matches!(native, Type::Mixed) {
            return true;
        }
        let Type::Class { name: parent, .. } = native else {
            return false;
        };
        inherited.members().iter().all(
            |member| matches!(member, Type::Class { name, .. } if self.index.is_subclass_of(name, parent.as_str())),
        )
    }

    fn class_type_aliases(&self) -> Vec<(String, Type)> {
        self.class
            .as_ref()
            .map(|class| self.index.type_aliases(&class.name))
            .unwrap_or_default()
    }

    fn class_template_names(&self) -> Vec<String> {
        let Some(class) = &self.class else {
            return Vec::new();
        };
        self.index
            .class(&class.name)
            .and_then(|found| {
                found
                    .decl
                    .doc
                    .as_ref()
                    .map(|doc| doc.templates.iter().map(|template| template.name.clone()).collect())
            })
            .unwrap_or_default()
    }

    /// Walks the statements of a container that come before the cursor. Returns true when the
    /// cursor was reached inside it.
    fn walk(&self, container: &SyntaxNode, env: &mut Env, cursor: u32) -> bool {
        for child in container.children() {
            if start(&child) >= cursor {
                return true;
            }
            if end(&child) > cursor {
                self.enter(&child, env, cursor);
                return true;
            }
            self.apply_with_doc(&child, env);
        }
        false
    }

    /// Descends into the statement the cursor is inside.
    fn enter(&self, node: &SyntaxNode, env: &mut Env, cursor: u32) {
        match node.kind() {
            IF_STATEMENT => self.enter_if(node, env, cursor),
            FOREACH_STATEMENT => {
                let (subject, targets, body) = foreach_parts(node);
                if let (Some(subject), Some(body)) = (subject, body) {
                    if start(&body) <= cursor && cursor <= end(&body) {
                        self.apply_expr(&subject, env);
                        self.bind_foreach(&subject, &targets, has_token(node, FAT_ARROW), env);
                        self.enter_or_walk(&body, env, cursor);
                    }
                }
            }
            CATCH_CLAUSE => {
                self.bind_catch(node, env);
                if let Some(block) = child_of(node, BLOCK) {
                    self.enter_or_walk(&block, env, cursor);
                }
            }
            WHILE_STATEMENT | DO_WHILE_STATEMENT => {
                if let Some(condition) = node
                    .children()
                    .find(|child| !matches!(child.kind(), BLOCK | STATEMENT_LIST))
                {
                    if start(&condition) < cursor {
                        self.narrow(&condition, true, env);
                    }
                }
                for body in node
                    .children()
                    .filter(|child| matches!(child.kind(), BLOCK | STATEMENT_LIST))
                {
                    if start(&body) <= cursor && cursor <= end(&body) {
                        self.enter_or_walk(&body, env, cursor);
                    }
                }
            }
            FOR_STATEMENT => {
                for child in node.children() {
                    if matches!(child.kind(), BLOCK | STATEMENT_LIST) {
                        if start(&child) <= cursor && cursor <= end(&child) {
                            self.enter_or_walk(&child, env, cursor);
                        }
                    } else if end(&child) <= cursor {
                        self.apply_expr(&child, env);
                    }
                }
            }
            EXPR_STATEMENT | ECHO_STATEMENT | RETURN_STATEMENT => {}
            _ => {
                self.walk(node, env, cursor);
            }
        }
    }

    fn enter_or_walk(&self, node: &SyntaxNode, env: &mut Env, cursor: u32) {
        match node.kind() {
            BLOCK | STATEMENT_LIST => {
                self.walk(node, env, cursor);
            }
            _ => self.enter(node, env, cursor),
        }
    }

    fn enter_if(&self, node: &SyntaxNode, env: &mut Env, cursor: u32) {
        let parts: Vec<SyntaxNode> = node.children().collect();
        let Some(condition) = parts.first() else {
            return;
        };
        if end(condition) >= cursor {
            return;
        }
        self.apply_expr(condition, env);
        let mut negated = env.clone();
        self.narrow(condition, false, &mut negated);
        if let Some(body) = parts.get(1) {
            if end(body) >= cursor && start(body) <= cursor {
                self.narrow(condition, true, env);
                self.enter_or_walk(body, env, cursor);
                return;
            }
        }
        for clause in parts.iter().skip(2) {
            match clause.kind() {
                ELSEIF_CLAUSE => {
                    let children: Vec<SyntaxNode> = clause.children().collect();
                    let (Some(clause_condition), Some(clause_body)) = (children.first(), children.get(1)) else {
                        continue;
                    };
                    self.apply_expr(clause_condition, &mut negated);
                    if start(clause_body) <= cursor && cursor <= end(clause_body) {
                        *env = negated.clone();
                        self.narrow(clause_condition, true, env);
                        self.enter_or_walk(clause_body, env, cursor);
                        return;
                    }
                    self.narrow(clause_condition, false, &mut negated);
                }
                ELSE_CLAUSE if start(clause) <= cursor && cursor <= end(clause) => {
                    *env = negated.clone();
                    if let Some(body) = clause.children().next() {
                        self.enter_or_walk(&body, env, cursor);
                    }
                    return;
                }
                _ => {}
            }
        }
    }

    fn apply_with_doc(&self, node: &SyntaxNode, env: &mut Env) {
        let inline = self.inline_var(node);
        self.apply(node, env);
        if let Some((ty, name)) = inline {
            let target = name.or_else(|| assigned_variable(node));
            if let Some(target) = target {
                env.set(target, ty);
            }
        }
    }

    /// The `/** @var Type $name */` right above a statement.
    fn inline_var(&self, node: &SyntaxNode) -> Option<(Type, Option<String>)> {
        let mut previous = node.prev_sibling_or_token();
        while let Some(element) = previous {
            match element {
                SyntaxElement::Token(token) if token.kind() == WHITESPACE => previous = token.prev_sibling_or_token(),
                SyntaxElement::Token(token) if token.kind() == DOC_COMMENT => {
                    let mut cx = TypeContext::new(&self.resolver);
                    cx.class_name = self.class.as_ref().map(|class| class.name.as_str());
                    cx.aliases = self.class_type_aliases();
                    return parse_var_comment(token.text(), &cx);
                }
                _ => return None,
            }
        }
        None
    }

    /// Applies what a statement does to the variables when it is done.
    fn apply(&self, node: &SyntaxNode, env: &mut Env) {
        match node.kind() {
            EXPR_STATEMENT => {
                if let Some(expr) = node.children().next() {
                    self.apply_expr(&expr, env);
                }
            }
            BLOCK | STATEMENT_LIST | DECLARE_STATEMENT | CASE_CLAUSE | DEFAULT_CLAUSE => self.apply_children(node, env),
            NAMESPACE_DECLARATION => self.apply_children(node, env),
            IF_STATEMENT => self.apply_if(node, env),
            FOREACH_STATEMENT => {
                let (subject, targets, body) = foreach_parts(node);
                if let Some(subject) = subject {
                    self.apply_expr(&subject, env);
                    let mut inner = env.clone();
                    self.bind_foreach(&subject, &targets, has_token(node, FAT_ARROW), &mut inner);
                    if let Some(body) = body {
                        self.apply(&body, &mut inner);
                    }
                    *env = merge(&[env.clone(), inner]);
                }
            }
            WHILE_STATEMENT | DO_WHILE_STATEMENT => {
                let mut inner = env.clone();
                if let Some(condition) = node
                    .children()
                    .find(|child| !matches!(child.kind(), BLOCK | STATEMENT_LIST))
                {
                    self.narrow(&condition, true, &mut inner);
                }
                for body in node
                    .children()
                    .filter(|child| matches!(child.kind(), BLOCK | STATEMENT_LIST))
                {
                    self.apply(&body, &mut inner);
                }
                *env = merge(&[env.clone(), inner]);
            }
            FOR_STATEMENT => {
                let mut inner = env.clone();
                for child in node.children() {
                    if matches!(child.kind(), BLOCK | STATEMENT_LIST) {
                        self.apply(&child, &mut inner);
                    } else {
                        self.apply_expr(&child, &mut inner);
                    }
                }
                *env = merge(&[env.clone(), inner]);
            }
            SWITCH_STATEMENT => {
                let mut inner = env.clone();
                for clause in node
                    .children()
                    .filter(|child| matches!(child.kind(), CASE_CLAUSE | DEFAULT_CLAUSE))
                {
                    self.apply(&clause, &mut inner);
                }
                *env = merge(&[env.clone(), inner]);
            }
            TRY_STATEMENT => {
                for child in node.children() {
                    match child.kind() {
                        BLOCK | FINALLY_CLAUSE => self.apply(&child, env),
                        CATCH_CLAUSE => {
                            let mut inner = env.clone();
                            self.bind_catch(&child, &mut inner);
                            if let Some(block) = child_of(&child, BLOCK) {
                                self.apply(&block, &mut inner);
                            }
                            if !ends_with_exit(child_of(&child, BLOCK).as_ref()) {
                                *env = merge(&[env.clone(), inner]);
                            }
                        }
                        _ => {}
                    }
                }
            }
            FINALLY_CLAUSE => self.apply_children(node, env),
            STATIC_VARIABLE_STATEMENT => {
                for variable in node.children().filter(|child| child.kind() == STATIC_VARIABLE) {
                    let Some(token) = first_token(&variable, VARIABLE) else {
                        continue;
                    };
                    let ty = variable
                        .children()
                        .next()
                        .map_or(Type::Unknown, |initializer| self.type_of(&initializer, env));
                    env.set(token.text().trim_start_matches('$'), ty);
                }
            }
            GLOBAL_STATEMENT => {
                for variable in node.children().filter(|child| child.kind() == VARIABLE_EXPR) {
                    env.set(text_of(&variable).trim_start_matches('$'), Type::Unknown);
                }
            }
            UNSET_STATEMENT => {
                for variable in node.children().filter(|child| child.kind() == VARIABLE_EXPR) {
                    env.vars.remove(text_of(&variable).trim_start_matches('$'));
                }
            }
            _ => {}
        }
    }

    fn apply_children(&self, node: &SyntaxNode, env: &mut Env) {
        for child in node.children() {
            self.apply_with_doc(&child, env);
        }
    }

    fn apply_if(&self, node: &SyntaxNode, env: &mut Env) {
        let parts: Vec<SyntaxNode> = node.children().collect();
        let Some(condition) = parts.first() else {
            return;
        };
        self.apply_expr(condition, env);
        let mut negated = env.clone();
        self.narrow(condition, false, &mut negated);
        let before = env.clone();
        let mut branches: Vec<Env> = Vec::new();
        let mut assigned: Vec<String> = Vec::new();
        for part in parts.iter().skip(1) {
            collect_assigned(part, &mut assigned);
        }
        if let Some(body) = parts.get(1) {
            let mut then_env = env.clone();
            self.narrow(condition, true, &mut then_env);
            self.apply(body, &mut then_env);
            if !ends_with_exit(Some(body)) {
                branches.push(then_env);
            }
        }
        let mut has_else = false;
        for clause in parts.iter().skip(2) {
            match clause.kind() {
                ELSEIF_CLAUSE => {
                    let children: Vec<SyntaxNode> = clause.children().collect();
                    let (Some(clause_condition), Some(clause_body)) = (children.first(), children.get(1)) else {
                        continue;
                    };
                    self.apply_expr(clause_condition, &mut negated);
                    let mut branch = negated.clone();
                    self.narrow(clause_condition, true, &mut branch);
                    self.apply(clause_body, &mut branch);
                    if !ends_with_exit(Some(clause_body)) {
                        branches.push(branch);
                    }
                    self.narrow(clause_condition, false, &mut negated);
                }
                ELSE_CLAUSE => {
                    has_else = true;
                    let mut branch = negated.clone();
                    if let Some(body) = clause.children().next() {
                        self.apply(&body, &mut branch);
                        if !ends_with_exit(Some(&body)) {
                            branches.push(branch);
                        }
                    } else {
                        branches.push(branch);
                    }
                }
                _ => {}
            }
        }
        if !has_else {
            branches.push(negated);
        }
        if branches.len() == 1 {
            *env = branches.remove(0);
        } else if !branches.is_empty() {
            let mut merged = merge(&branches);
            // A branch that only learned something about a variable does not change it for good.
            for (name, ty) in &before.vars {
                if !assigned.contains(name) {
                    merged.vars.insert(name.clone(), ty.clone());
                }
            }
            *env = merged;
        }
    }

    fn bind_foreach(&self, subject: &SyntaxNode, targets: &[SyntaxNode], keyed: bool, env: &mut Env) {
        let subject_type = self.type_of(subject, env);
        let (key, value) = self.iterable_types(&subject_type);
        match (targets, keyed) {
            ([key_target, value_target], true) => {
                self.bind_target(key_target, &key, env);
                self.bind_target(value_target, &value, env);
            }
            ([value_target], _) => self.bind_target(value_target, &value, env),
            _ => {}
        }
    }

    fn bind_catch(&self, clause: &SyntaxNode, env: &mut Env) {
        let Some(variable) = first_token(clause, VARIABLE) else {
            return;
        };
        let names: Vec<Type> = clause
            .children()
            .find(|child| crate::ast::is_type_node(child.kind()))
            .map(|ty| {
                ty.descendants()
                    .filter(|node| node.kind() == NAME)
                    .map(|name| self.class_type(&text_of(&name)))
                    .collect()
            })
            .unwrap_or_default();
        env.set(variable.text().trim_start_matches('$'), Type::union(names));
    }

    /// Applies the assignments inside an expression, in the order they happen.
    pub fn apply_expr(&self, expr: &SyntaxNode, env: &mut Env) {
        match expr.kind() {
            ASSIGN_EXPR => self.apply_assign(expr, env),
            CLOSURE_EXPR | ARROW_FUNCTION_EXPR | ANONYMOUS_CLASS => {}
            CALL_EXPR => {
                for child in expr.children() {
                    self.apply_expr(&child, env);
                }
                if let Some(callee) = expr.children().next() {
                    if callee.kind() == NAME && text_of(&callee).trim_start_matches('\\').eq_ignore_ascii_case("assert")
                    {
                        if let Some(condition) = arguments(expr).first().and_then(|arg| arg.expr.clone()) {
                            self.narrow(&condition, true, env);
                        }
                    }
                }
                if expr.parent().is_some_and(|parent| parent.kind() == EXPR_STATEMENT) {
                    self.narrow_by_asserts(expr, None, env);
                }
            }
            _ => {
                for child in expr.children() {
                    self.apply_expr(&child, env);
                }
            }
        }
    }

    fn apply_assign(&self, expr: &SyntaxNode, env: &mut Env) {
        let operands: Vec<SyntaxNode> = expr.children().collect();
        let (Some(target), Some(value)) = (operands.first(), operands.last()) else {
            return;
        };
        if operands.len() < 2 {
            return;
        }
        self.apply_expr(value, env);
        let operator = tokens(expr)
            .find(|token| !token.kind().is_trivia() && token.kind() != AMP)
            .map(|token| token.kind());
        let value_type = if operator == Some(ASSIGN) {
            self.type_of(value, env)
        } else {
            self.type_of(expr, env)
        };
        match target.kind() {
            INDEX_EXPR => self.bind_index_target(target, &value_type, env),
            _ => self.bind_target(target, &value_type, env),
        }
    }

    /// Records `$a[] = x`, `$a[key] = x` and `$a[key][] = x` as what `$a` holds.
    fn bind_index_target(&self, target: &SyntaxNode, value: &Type, env: &mut Env) {
        let mut keys: Vec<Option<SyntaxNode>> = Vec::new();
        let mut base = target.clone();
        while base.kind() == INDEX_EXPR {
            let mut parts = base.children();
            let (Some(inner), key) = (parts.next(), parts.next()) else {
                return;
            };
            keys.push(key);
            base = inner;
        }
        if base.kind() != VARIABLE_EXPR {
            return;
        }
        keys.reverse();
        let name = text_of(&base).trim_start_matches('$').to_string();
        let current = env.get(&name).cloned().unwrap_or(Type::Unknown);
        let key_types: Vec<Option<Type>> = keys
            .iter()
            .map(|key| key.as_ref().map(|key| self.type_of(key, env)))
            .collect();
        if let Some(updated) = assign_into(&current, &key_types, value) {
            env.set(name, updated);
        }
    }

    pub(super) fn bind_target(&self, target: &SyntaxNode, ty: &Type, env: &mut Env) {
        match target.kind() {
            VARIABLE_EXPR => {
                let text = text_of(target);
                let name = text.trim_start_matches('$');
                if name != "this" {
                    env.set(name, ty.clone());
                }
            }
            ARRAY_EXPR | LIST_EXPR => {
                for (position, item) in target.children().filter(|child| child.kind() == ARRAY_ITEM).enumerate() {
                    let children: Vec<SyntaxNode> = item.children().collect();
                    let (key, value) = match children.as_slice() {
                        [key, value] => (Some(key.clone()), value.clone()),
                        [value] => (None, value.clone()),
                        _ => continue,
                    };
                    let key_text = match &key {
                        Some(key) => literal_string(key),
                        None => Some(position.to_string()),
                    };
                    let element = self.element_of(ty, key_text.as_deref());
                    self.bind_target(&value, &element, env);
                }
            }
            _ => {}
        }
    }

    /// Narrows the variables a condition tests, for the branch where it came out as `truth`.
    pub fn narrow(&self, condition: &SyntaxNode, truth: bool, env: &mut Env) {
        match condition.kind() {
            PAREN_EXPR => {
                if let Some(inner) = condition.children().next() {
                    self.narrow(&inner, truth, env);
                }
            }
            PREFIX_EXPR if has_token(condition, BANG) => {
                if let Some(inner) = condition.children().next() {
                    self.narrow(&inner, !truth, env);
                }
            }
            BINARY_EXPR => self.narrow_binary(condition, truth, env),
            VARIABLE_EXPR => {
                let name = text_of(condition).trim_start_matches('$').to_string();
                if truth {
                    let current = env.get(&name).cloned().unwrap_or(Type::Unknown);
                    if !current.is_unknown() {
                        env.set(name, current.filter(|ty| !matches!(ty, Type::Null | Type::False)));
                    }
                }
            }
            ISSET_EXPR => {
                if truth {
                    for variable in condition.children().filter(|child| child.kind() == VARIABLE_EXPR) {
                        let name = text_of(&variable).trim_start_matches('$').to_string();
                        if let Some(current) = env.get(&name).cloned() {
                            env.set(name, current.without_null());
                        }
                    }
                }
            }
            EMPTY_EXPR => {
                if let Some(inner) = condition.children().next() {
                    self.narrow(&inner, !truth, env);
                }
            }
            CALL_EXPR => self.narrow_call(condition, truth, env),
            ASSIGN_EXPR => {
                if let Some(target) = condition.children().next().filter(|node| node.kind() == VARIABLE_EXPR) {
                    self.narrow(&target, truth, env);
                }
            }
            _ => {}
        }
    }

    fn narrow_binary(&self, condition: &SyntaxNode, truth: bool, env: &mut Env) {
        let operands: Vec<SyntaxNode> = condition.children().collect();
        let operator = tokens(condition)
            .find(|token| !token.kind().is_trivia())
            .map(|token| token.kind());
        let (Some(left), Some(right)) = (operands.first(), operands.last()) else {
            return;
        };
        match operator {
            Some(AND_AND | AND_KW) => {
                if truth {
                    self.narrow(left, true, env);
                    self.narrow(right, true, env);
                }
            }
            Some(OR_OR | OR_KW) => {
                if !truth {
                    self.narrow(left, false, env);
                    self.narrow(right, false, env);
                }
            }
            Some(INSTANCEOF_KW) => {
                let Some(name) = condition_variable(left) else {
                    return;
                };
                let class = match right.kind() {
                    NAME => self.class_type(&text_of(right)),
                    _ => match self.type_of(right, env) {
                        Type::ClassString(Some(inner)) => *inner,
                        _ => return,
                    },
                };
                let class = match class {
                    Type::Static => self.this_type(),
                    other => other,
                };
                let current = env.get(&name).cloned().unwrap_or(Type::Unknown);
                let narrowed = self.narrow_instanceof(&current, &class, truth);
                env.set(name, narrowed);
            }
            Some(IDENTICAL | EQ | NOT_IDENTICAL | NEQ) => {
                let positive = matches!(operator, Some(IDENTICAL | EQ)) == truth;
                let variable = match (condition_variable(left), condition_variable(right)) {
                    (Some(name), _) if is_null_literal(right) => name,
                    (_, Some(name)) if is_null_literal(left) => name,
                    _ => return,
                };
                let current = env.get(&variable).cloned().unwrap_or(Type::Unknown);
                if positive {
                    env.set(variable, Type::Null);
                } else if !current.is_unknown() {
                    env.set(variable, current.without_null());
                }
            }
            _ => {}
        }
    }

    fn narrow_call(&self, call: &SyntaxNode, truth: bool, env: &mut Env) {
        self.narrow_by_asserts(call, Some(truth), env);
        let Some(callee) = call.children().next().filter(|callee| callee.kind() == NAME) else {
            return;
        };
        let name = text_of(&callee).trim_start_matches('\\').to_ascii_lowercase();
        let Some(argument) = arguments(call).first().and_then(|arg| arg.expr.clone()) else {
            return;
        };
        let Some(variable) = condition_variable(&argument) else {
            return;
        };
        let kind = match name.as_str() {
            "is_null" => Kind::Null,
            "is_string" => Kind::String,
            "is_int" | "is_integer" | "is_long" => Kind::Int,
            "is_float" | "is_double" => Kind::Float,
            "is_bool" => Kind::Bool,
            "is_array" => Kind::Array,
            "is_object" => Kind::Object,
            "is_callable" => Kind::Callable,
            "is_numeric" => Kind::Numeric,
            _ => return,
        };
        let current = env.get(&variable).cloned().unwrap_or(Type::Unknown);
        env.set(variable, narrow_kind(&current, truth, kind));
    }

    /// Applies the `@assert` tags of the function or method a call names to the variables it was
    /// given. `truth` is the value the call has as a condition, `None` for a call that stands alone.
    fn narrow_by_asserts(&self, call: &SyntaxNode, truth: Option<bool>, env: &mut Env) {
        if call.kind() != CALL_EXPR || super::calls::is_first_class_callable(call) {
            return;
        }
        let callees = self.callees(call, env);
        let [callee] = callees.as_slice() else {
            return;
        };
        let Some(doc) = &callee.doc else {
            return;
        };
        if doc.asserts.is_empty() {
            return;
        }
        let args = arguments(call);
        let (map, _) = self.bind_call(callee, &args, env);
        let params: Vec<&php_index::Param> = callee.callable.params_at(self.level()).collect();
        for assert in &doc.asserts {
            let holds = match assert.when {
                AssertWhen::Always => true,
                AssertWhen::IfTrue => truth == Some(true),
                AssertWhen::IfFalse => truth == Some(false),
            };
            if !holds {
                continue;
            }
            let subject = if assert.subject == "$this" {
                call.children()
                    .next()
                    .filter(|callee| callee.kind() == PROPERTY_FETCH_EXPR)
                    .and_then(|callee| callee.children().next())
            } else {
                let name = assert.subject.trim_start_matches('$');
                params
                    .iter()
                    .position(|param| param.name == name)
                    .and_then(|position| {
                        args.iter()
                            .find(|arg| arg.name.as_deref() == Some(name))
                            .or_else(|| args.get(position).filter(|arg| arg.name.is_none() && !arg.spread))
                    })
                    .and_then(|arg| arg.expr.clone())
            };
            let Some(variable) = subject.as_ref().and_then(condition_variable) else {
                continue;
            };
            let ty = assert
                .ty
                .substitute(&map, callee.receiver.as_ref(), callee.self_name.as_deref());
            if ty.has_template() {
                continue;
            }
            let current = env.get(&variable).cloned().unwrap_or(Type::Unknown);
            env.set(variable, self.narrow_to_type(&current, &ty, !assert.negated));
        }
    }

    /// A type narrowed to a given type, or with that type taken out.
    pub fn narrow_to_type(&self, current: &Type, ty: &Type, positive: bool) -> Type {
        if positive {
            let narrowed = ty.members().iter().map(|member| self.narrow_to_member(current, member));
            return Type::union(narrowed);
        }
        let mut result = current.clone();
        for member in ty.members() {
            result = self.remove_member(&result, member);
        }
        result
    }

    fn narrow_to_member(&self, current: &Type, ty: &Type) -> Type {
        let kind = kind_of(ty);
        match (ty, kind) {
            (Type::Class { .. }, _) => self.narrow_instanceof(current, ty, true),
            (_, Some(kind)) => {
                let narrowed = narrow_kind(current, true, kind);
                let is_array = matches!(kind, Kind::Array);
                if is_array && narrowed == Type::plain_array() {
                    ty.clone()
                } else {
                    narrowed
                }
            }
            _ => ty.clone(),
        }
    }

    fn remove_member(&self, current: &Type, ty: &Type) -> Type {
        match (ty, kind_of(ty)) {
            (Type::Class { .. }, _) => self.narrow_instanceof(current, ty, false),
            (Type::True | Type::False, _) => current.filter(|member| member != ty),
            (_, Some(kind)) => narrow_kind(current, false, kind),
            _ => current.clone(),
        }
    }

    /// A type narrowed by `instanceof class`, or by its absence.
    pub fn narrow_instanceof(&self, current: &Type, class: &Type, positive: bool) -> Type {
        let Type::Class { name: target, .. } = class else {
            return current.clone();
        };
        if positive {
            let narrowed = current.members().iter().map(|member| match member {
                Type::Class { name, .. } => {
                    if self.index.is_subclass_of(name, target) {
                        member.clone()
                    } else if self.index.is_subclass_of(target, name) {
                        class.clone()
                    } else {
                        Type::Intersection(vec![member.clone(), class.clone()])
                    }
                }
                Type::Null | Type::Int | Type::String | Type::Float | Type::Bool | Type::True | Type::False => {
                    Type::Never
                }
                _ => class.clone(),
            });
            let narrowed = Type::union(narrowed);
            if matches!(narrowed, Type::Never) {
                class.clone()
            } else {
                narrowed
            }
        } else {
            current.filter(|member| match member {
                Type::Class { name, .. } => !self.index.is_subclass_of(name, target),
                _ => true,
            })
        }
    }
}

/// What an array type becomes when `value` is stored under a chain of keys, `None` for a key a
/// literal-free type cannot follow. A key of `None` is `[]`.
fn assign_into(current: &Type, keys: &[Option<Type>], value: &Type) -> Option<Type> {
    let Some((first, rest)) = keys.split_first() else {
        return Some(value.clone());
    };
    let empty = matches!(current, Type::Array(key, element) if **key == Type::Never && **element == Type::Never)
        || current.is_unknown();
    match (current, first) {
        _ if empty => {
            let element = assign_into(&Type::Unknown, rest, value)?;
            Some(match first {
                Some(key_type) => Type::Array(Box::new(key_type.clone()), Box::new(element)),
                None => Type::List(Box::new(element)),
            })
        }
        (Type::List(existing), None) => {
            let element = stored_element(existing, rest, value)?;
            Some(Type::List(Box::new(element)))
        }
        (Type::Array(existing_key, existing), Some(key_type)) => {
            let element = stored_element(existing, rest, value)?;
            Some(Type::Array(
                Box::new(Type::union([(**existing_key).clone(), key_type.clone()])),
                Box::new(element),
            ))
        }
        (Type::Array(existing_key, existing), None) => {
            let element = stored_element(existing, rest, value)?;
            Some(Type::Array(existing_key.clone(), Box::new(element)))
        }
        _ => None,
    }
}

/// The element type of an array after storing `value` below it by the remaining keys.
fn stored_element(existing: &Type, rest: &[Option<Type>], value: &Type) -> Option<Type> {
    let added = assign_into(&Type::Unknown, rest, value)?;
    Some(Type::union([existing.clone(), added]))
}

/// The type groups the `is_*` functions test for.
#[derive(Clone, Copy)]
enum Kind {
    Null,
    String,
    Int,
    Float,
    Bool,
    Array,
    Object,
    Callable,
    Numeric,
}

/// The group of types `is_*` tests for that a type belongs to.
fn kind_of(ty: &Type) -> Option<Kind> {
    match ty {
        Type::Null => Some(Kind::Null),
        Type::String | Type::StringLiteral(_) => Some(Kind::String),
        Type::Int | Type::IntLiteral(_) => Some(Kind::Int),
        Type::Float => Some(Kind::Float),
        Type::Bool => Some(Kind::Bool),
        Type::Array(..) | Type::List(_) | Type::Shape(_) => Some(Kind::Array),
        Type::Object => Some(Kind::Object),
        Type::Callable(_) => Some(Kind::Callable),
        Type::Numeric => Some(Kind::Numeric),
        _ => None,
    }
}

fn narrow_kind(current: &Type, positive: bool, kind: Kind) -> Type {
    let matches_kind = |ty: &Type| match kind {
        Kind::Null => matches!(ty, Type::Null),
        Kind::String => matches!(ty, Type::String | Type::StringLiteral(_)),
        Kind::Int => matches!(ty, Type::Int | Type::IntLiteral(_)),
        Kind::Float => matches!(ty, Type::Float),
        Kind::Bool => matches!(ty, Type::Bool | Type::True | Type::False),
        Kind::Array => matches!(ty, Type::Array(..) | Type::List(_) | Type::Shape(_)),
        Kind::Object => matches!(ty, Type::Class { .. } | Type::Object | Type::Static),
        Kind::Callable => matches!(ty, Type::Callable(_)),
        Kind::Numeric => matches!(ty, Type::Int | Type::IntLiteral(_) | Type::Float | Type::Numeric),
    };
    if current.is_unknown() || matches!(current, Type::Mixed) {
        if !positive {
            return current.clone();
        }
        return match kind {
            Kind::Null => Type::Null,
            Kind::String => Type::String,
            Kind::Int => Type::Int,
            Kind::Float => Type::Float,
            Kind::Bool => Type::Bool,
            Kind::Array => Type::plain_array(),
            Kind::Object => Type::Object,
            Kind::Callable => Type::Callable(None),
            Kind::Numeric => Type::Numeric,
        };
    }
    let filtered = current.filter(|ty| matches_kind(ty) == positive);
    if matches!(filtered, Type::Never) {
        current.clone()
    } else {
        filtered
    }
}

/// The variable a condition tests: the variable itself, or the target of an assignment in parentheses.
fn condition_variable(node: &SyntaxNode) -> Option<String> {
    match node.kind() {
        VARIABLE_EXPR => Some(text_of(node).trim_start_matches('$').to_string()),
        PAREN_EXPR => node.children().next().and_then(|inner| condition_variable(&inner)),
        ASSIGN_EXPR => node.children().next().and_then(|target| condition_variable(&target)),
        _ => None,
    }
}

fn is_null_literal(node: &SyntaxNode) -> bool {
    node.kind() == NAME && text_of(node).trim_start_matches('\\').eq_ignore_ascii_case("null")
}

/// The variable a statement assigns, for an inline `@var` without a name.
fn assigned_variable(statement: &SyntaxNode) -> Option<String> {
    let expr = match statement.kind() {
        EXPR_STATEMENT => statement.children().next()?,
        _ => return None,
    };
    if expr.kind() != ASSIGN_EXPR {
        return None;
    }
    condition_variable(&expr.children().next()?)
}

/// The subject, the targets (value, or key and value) and the body of a `foreach`.
fn foreach_parts(node: &SyntaxNode) -> (Option<SyntaxNode>, Vec<SyntaxNode>, Option<SyntaxNode>) {
    let children: Vec<SyntaxNode> = node.children().collect();
    if children.len() < 3 {
        return (children.first().cloned(), Vec::new(), None);
    }
    let subject = children.first().cloned();
    let body = children.last().cloned();
    let targets = children[1..children.len() - 1].to_vec();
    (subject, targets, body)
}

/// Whether a block always leaves: its last statement returns, throws, breaks or continues.
fn ends_with_exit(node: Option<&SyntaxNode>) -> bool {
    let Some(node) = node else {
        return false;
    };
    let last = match node.kind() {
        BLOCK | STATEMENT_LIST => node.children().last(),
        _ => Some(node.clone()),
    };
    let Some(last) = last else {
        return false;
    };
    match last.kind() {
        RETURN_STATEMENT | BREAK_STATEMENT | CONTINUE_STATEMENT | GOTO_STATEMENT => true,
        EXPR_STATEMENT => last
            .children()
            .next()
            .is_some_and(|expr| matches!(expr.kind(), THROW_EXPR | EXIT_EXPR)),
        BLOCK | STATEMENT_LIST => ends_with_exit(Some(&last)),
        _ => false,
    }
}

/// The names a subtree assigns, by assignment, `foreach` or `catch`.
fn collect_assigned(node: &SyntaxNode, out: &mut Vec<String>) {
    for descendant in node.descendants() {
        match descendant.kind() {
            ASSIGN_EXPR => {
                if let Some(target) = descendant.children().next() {
                    collect_targets(&target, out);
                }
            }
            FOREACH_STATEMENT => {
                let (_, targets, _) = foreach_parts(&descendant);
                for target in targets {
                    collect_targets(&target, out);
                }
            }
            CATCH_CLAUSE => {
                if let Some(variable) = first_token(&descendant, VARIABLE) {
                    out.push(variable.text().trim_start_matches('$').to_string());
                }
            }
            _ => {}
        }
    }
}

fn collect_targets(target: &SyntaxNode, out: &mut Vec<String>) {
    match target.kind() {
        VARIABLE_EXPR => out.push(text_of(target).trim_start_matches('$').to_string()),
        INDEX_EXPR => {
            if let Some(base) = target.children().next() {
                collect_targets(&base, out);
            }
        }
        ARRAY_EXPR | LIST_EXPR => {
            for item in target.descendants().filter(|node| node.kind() == VARIABLE_EXPR) {
                out.push(text_of(&item).trim_start_matches('$').to_string());
            }
        }
        _ => {}
    }
}

/// The environment where several branches meet: every variable, with the union of what the branches
/// that have it say.
fn merge(branches: &[Env]) -> Env {
    let mut merged = Env::default();
    let mut names: Vec<&String> = branches.iter().flat_map(|branch| branch.vars.keys()).collect();
    names.sort();
    names.dedup();
    for name in names {
        let ty = Type::union(branches.iter().filter_map(|branch| branch.vars.get(name)).cloned());
        merged.vars.insert(name.clone(), ty);
    }
    merged
}
