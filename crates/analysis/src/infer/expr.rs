//! The type of an expression.

use php_index::Type;
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxKind, SyntaxNode};

use super::{Analyzer, Env};
use crate::ast::{child_of, first_token, has_token, text_of, tokens};

impl Analyzer<'_> {
    /// The type of an expression node in an environment.
    pub fn type_of(&self, node: &SyntaxNode, env: &Env) -> Type {
        self.guarded(Type::Unknown, || self.type_of_inner(node, env))
    }

    fn type_of_inner(&self, node: &SyntaxNode, env: &Env) -> Type {
        match node.kind() {
            VARIABLE_EXPR => self.variable_type(node, env),
            LITERAL => literal_type(node),
            NAME => self.constant_type(&text_of(node)),
            ARRAY_EXPR | LIST_EXPR => self.array_type(node, env),
            NEW_EXPR => self.new_type(node, env),
            CALL_EXPR => self.call_type(node, env),
            PROPERTY_FETCH_EXPR => self.property_type(node, env),
            SCOPED_ACCESS_EXPR => self.scoped_access_type(node, env),
            STATIC_PROPERTY_EXPR => self.static_property_type(node, env),
            INDEX_EXPR => self.index_type(node, env),
            ASSIGN_EXPR => self.assign_type(node, env),
            TERNARY_EXPR => self.ternary_type(node, env),
            BINARY_EXPR => self.binary_type(node, env),
            PREFIX_EXPR => self.prefix_type(node, env),
            POSTFIX_EXPR => node
                .children()
                .next()
                .map_or(Type::Unknown, |operand| self.type_of(&operand, env)),
            CAST_EXPR => cast_type(node),
            CLONE_EXPR => node
                .children()
                .next()
                .map_or(Type::Unknown, |operand| self.type_of(&operand, env)),
            PAREN_EXPR => node
                .children()
                .next()
                .map_or(Type::Unknown, |inner| self.type_of(&inner, env)),
            CLOSURE_EXPR | ARROW_FUNCTION_EXPR => self.closure_type(node, env),
            MATCH_EXPR => Type::union(
                node.children()
                    .filter(|arm| arm.kind() == MATCH_ARM)
                    .filter_map(|arm| arm.children().last())
                    .map(|result| self.type_of(&result, env)),
            ),
            INTERPOLATED_STRING | HEREDOC | SHELL_EXEC_EXPR => Type::String,
            PRINT_EXPR => Type::Int,
            ISSET_EXPR | EMPTY_EXPR => Type::Bool,
            EXIT_EXPR | THROW_EXPR => Type::Never,
            EVAL_EXPR | INCLUDE_EXPR | YIELD_EXPR | YIELD_FROM_EXPR => Type::Mixed,
            _ => Type::Unknown,
        }
    }

    fn variable_type(&self, node: &SyntaxNode, env: &Env) -> Type {
        let text = text_of(node);
        let name = text.trim_start_matches('$');
        if name == "this" {
            return match self.this_type() {
                Type::Unknown => Type::Unknown,
                _ => Type::Static,
            };
        }
        if let Some(ty) = env.get(name) {
            return ty.clone();
        }
        superglobal_type(name).unwrap_or(Type::Unknown)
    }

    fn constant_type(&self, raw: &str) -> Type {
        match raw.trim_start_matches('\\').to_ascii_lowercase().as_str() {
            "true" => return Type::True,
            "false" => return Type::False,
            "null" => return Type::Null,
            _ => {}
        }
        let candidates = self.resolver.constant_candidates(raw);
        match self.index.first_constant(&candidates) {
            Some(constant) => constant.decl.ty.clone().unwrap_or(Type::Unknown),
            None => Type::Unknown,
        }
    }

    fn array_type(&self, node: &SyntaxNode, env: &Env) -> Type {
        let items: Vec<SyntaxNode> = node.children().filter(|child| child.kind() == ARRAY_ITEM).collect();
        if items.is_empty() {
            return Type::Array(Box::new(Type::Never), Box::new(Type::Never));
        }
        let mut values = Vec::new();
        let mut keys = Vec::new();
        let mut string_keys: Vec<(String, Type)> = Vec::new();
        let mut all_literal_string_keys = true;
        let mut any_key = false;
        for item in &items {
            let children: Vec<SyntaxNode> = item.children().collect();
            let spread = has_token(item, ELLIPSIS);
            if spread {
                let spread_type = children.first().map_or(Type::Unknown, |expr| self.type_of(expr, env));
                let (key, value) = self.iterable_types(&spread_type);
                values.push(value);
                keys.push(key);
                all_literal_string_keys = false;
                continue;
            }
            let (key, value) = match children.as_slice() {
                [key, value] => (Some(key.clone()), value.clone()),
                [value] => (None, value.clone()),
                _ => continue,
            };
            let value_type = self.type_of(&value, env);
            match key {
                Some(key) => {
                    any_key = true;
                    let key_type = self.type_of(&key, env);
                    match literal_string(&key) {
                        Some(literal) => string_keys.push((literal, value_type.clone())),
                        None => all_literal_string_keys = false,
                    }
                    keys.push(key_type);
                }
                None => {
                    all_literal_string_keys = false;
                    keys.push(Type::Int);
                }
            }
            values.push(value_type);
        }
        if any_key && all_literal_string_keys && !string_keys.is_empty() && string_keys.len() <= 24 {
            return Type::Shape(
                string_keys
                    .into_iter()
                    .map(|(key, ty)| php_index::types::ShapeField {
                        key: Some(key),
                        optional: false,
                        ty,
                    })
                    .collect(),
            );
        }
        let value = Type::union(values);
        if !any_key {
            return Type::List(Box::new(value));
        }
        Type::Array(Box::new(Type::union(keys)), Box::new(value))
    }

    fn property_type(&self, node: &SyntaxNode, env: &Env) -> Type {
        let Some(object) = node.children().next() else {
            return Type::Unknown;
        };
        let nullsafe = has_token(node, NULLSAFE_ARROW);
        let Some(name) = child_of(node, NAME) else {
            return Type::Unknown;
        };
        let receiver = self.type_of(&object, env);
        let receiver = self.receiver_type(&receiver);
        let mut result = self.property_on(&receiver, &text_of(&name));
        if result.is_unknown() && text_of(&object) == "$this" {
            if let Some(found) = self.pest_property(&text_of(&name)) {
                result = found;
            }
        }
        if nullsafe && self.type_of(&object, env).contains_null() {
            return result.nullable();
        }
        result
    }

    /// `$this` and `static` mean the enclosing class where members are looked up.
    pub fn receiver_type(&self, ty: &Type) -> Type {
        let this = self.this_type();
        let resolved = Type::union(ty.members().iter().map(|member| match member {
            Type::Static | Type::SelfType => this.clone(),
            other => other.clone(),
        }));
        resolved.without_null()
    }

    /// The type of a property on each class a type stands for.
    pub fn property_on(&self, receiver: &Type, name: &str) -> Type {
        let mut found = Vec::new();
        for member in receiver.members() {
            if !matches!(member, Type::Class { .. } | Type::Intersection(_)) {
                continue;
            }
            if let Some(property) = self.index.find_property(member, name) {
                let declared = property.member.effective_type(self.level()).cloned();
                let ty = declared.map_or(Type::Unknown, |ty| property.resolve(&ty));
                found.push(self.bind_static(&ty, member));
            }
        }
        if found.is_empty() {
            Type::Unknown
        } else {
            Type::union(found)
        }
    }

    fn scoped_access_type(&self, node: &SyntaxNode, env: &Env) -> Type {
        let Some(qualifier) = node.children().next() else {
            return Type::Unknown;
        };
        let Some(member) = node
            .children()
            .filter(|child| child.kind() == NAME)
            .last()
            .filter(|member| member != &qualifier)
        else {
            return Type::Unknown;
        };
        let name = text_of(&member);
        let class = self.qualifier_type(&qualifier, env);
        if name == "class" {
            let inner = match &class {
                Type::Static => self.this_type(),
                other => other.clone(),
            };
            return Type::ClassString(Some(Box::new(inner)));
        }
        let receiver = self.receiver_type(&class);
        let mut found = Vec::new();
        for member_type in receiver.members() {
            if let Some(constant) = self.index.find_constant(member_type, &name) {
                if constant.member.is_case {
                    found.push(member_type.clone());
                    continue;
                }
                let ty = constant
                    .member
                    .effective_type(self.level())
                    .cloned()
                    .unwrap_or(Type::Unknown);
                found.push(constant.resolve(&ty));
            }
        }
        if found.is_empty() {
            Type::Unknown
        } else {
            Type::union(found)
        }
    }

    fn static_property_type(&self, node: &SyntaxNode, env: &Env) -> Type {
        let Some(qualifier) = node.children().next() else {
            return Type::Unknown;
        };
        let Some(variable) = node
            .children()
            .filter(|child| child.kind() == VARIABLE_EXPR)
            .last()
            .filter(|v| v != &qualifier)
        else {
            return Type::Unknown;
        };
        let class = self.qualifier_type(&qualifier, env);
        let receiver = self.receiver_type(&class);
        self.property_on(&receiver, text_of(&variable).trim_start_matches('$'))
    }

    /// The class a `Foo::` or `$object::` or `static::` stands for.
    pub fn qualifier_type(&self, qualifier: &SyntaxNode, env: &Env) -> Type {
        match qualifier.kind() {
            NAME => self.class_type(&text_of(qualifier)),
            _ => match self.type_of(qualifier, env) {
                Type::ClassString(Some(inner)) => *inner,
                other => other,
            },
        }
    }

    fn index_type(&self, node: &SyntaxNode, env: &Env) -> Type {
        let mut children = node.children();
        let (Some(base), index) = (children.next(), children.next()) else {
            return Type::Unknown;
        };
        let base_type = self.type_of(&base, env);
        let key = index.as_ref().and_then(literal_string_or_int);
        self.element_of(&base_type, key.as_deref())
    }

    /// The type of `$value[$key]`, with a literal key when there is one.
    pub fn element_of(&self, base: &Type, key: Option<&str>) -> Type {
        let mut out = Vec::new();
        for member in base.members() {
            match member {
                Type::Array(_, value) | Type::List(value) | Type::Iterable(_, value) => out.push((**value).clone()),
                Type::Shape(fields) => {
                    let field = key.and_then(|key| fields.iter().find(|field| field.key.as_deref() == Some(key)));
                    match field {
                        Some(field) => out.push(field.ty.clone()),
                        None => out.extend(fields.iter().map(|field| field.ty.clone())),
                    }
                }
                Type::String => out.push(Type::String),
                Type::Class { .. } => {
                    if let Some(found) = self.index.find_method(member, "offsetGet") {
                        let ret = found.member.callable.effective_return(self.level()).cloned();
                        out.push(ret.map_or(Type::Unknown, |ret| found.resolve(&ret)));
                    }
                }
                _ => {}
            }
        }
        if out.is_empty() {
            Type::Unknown
        } else {
            Type::union(out)
        }
    }

    fn assign_type(&self, node: &SyntaxNode, env: &Env) -> Type {
        let operator = tokens(node)
            .find(|token| !token.kind().is_trivia() && token.kind() != AMP && is_assign_operator(token.kind()));
        let operands: Vec<SyntaxNode> = node.children().collect();
        let (Some(target), Some(value)) = (operands.first(), operands.last()) else {
            return Type::Unknown;
        };
        match operator.map(|token| token.kind()) {
            Some(ASSIGN) | None => self.type_of(value, env),
            Some(COALESCE_ASSIGN) => Type::union([self.type_of(target, env).without_null(), self.type_of(value, env)]),
            Some(DOT_ASSIGN) => Type::String,
            Some(_) => self.type_of(target, env),
        }
    }

    fn ternary_type(&self, node: &SyntaxNode, env: &Env) -> Type {
        let operands: Vec<SyntaxNode> = node.children().collect();
        match operands.as_slice() {
            [condition, then, otherwise] => {
                let mut then_env = env.clone();
                self.narrow(condition, true, &mut then_env);
                let mut else_env = env.clone();
                self.narrow(condition, false, &mut else_env);
                Type::union([self.type_of(then, &then_env), self.type_of(otherwise, &else_env)])
            }
            [condition, otherwise] => {
                let mut else_env = env.clone();
                self.narrow(condition, false, &mut else_env);
                Type::union([
                    self.type_of(condition, env)
                        .filter(|ty| !matches!(ty, Type::Null | Type::False)),
                    self.type_of(otherwise, &else_env),
                ])
            }
            _ => Type::Unknown,
        }
    }

    fn binary_type(&self, node: &SyntaxNode, env: &Env) -> Type {
        let operands: Vec<SyntaxNode> = node.children().collect();
        let operator = tokens(node)
            .find(|token| !token.kind().is_trivia())
            .map(|token| token.kind());
        let (Some(left), Some(right)) = (operands.first(), operands.last()) else {
            return Type::Unknown;
        };
        match operator {
            Some(COALESCE) => Type::union([self.type_of(left, env).without_null(), self.type_of(right, env)]),
            Some(DOT) => Type::String,
            Some(
                EQ | NEQ | IDENTICAL | NOT_IDENTICAL | LT | GT | LE | GE | AND_AND | OR_OR | AND_KW | OR_KW | XOR_KW
                | INSTANCEOF_KW,
            ) => Type::Bool,
            Some(SPACESHIP | SHL | SHR | AMP | PIPE | CARET) => Type::Int,
            Some(PLUS | MINUS | STAR | SLASH | PERCENT | POW) => {
                let left_type = self.type_of(left, env);
                let right_type = self.type_of(right, env);
                arithmetic(operator, &left_type, &right_type)
            }
            Some(PIPE_GT) => Type::Unknown,
            _ => Type::Unknown,
        }
    }

    fn prefix_type(&self, node: &SyntaxNode, env: &Env) -> Type {
        let Some(operand) = node.children().next() else {
            return Type::Unknown;
        };
        let operator = tokens(node)
            .find(|token| !token.kind().is_trivia())
            .map(|token| token.kind());
        match operator {
            Some(BANG) => Type::Bool,
            Some(TILDE) => Type::Int,
            Some(MINUS | PLUS) => match self.type_of(&operand, env) {
                Type::Int | Type::IntLiteral(_) => Type::Int,
                Type::Float => Type::Float,
                _ => Type::Numeric,
            },
            _ => self.type_of(&operand, env),
        }
    }

    fn closure_type(&self, node: &SyntaxNode, env: &Env) -> Type {
        let (callable, _) = php_index::extract::callable_at(node, &self.resolver, None);
        let level = self.level();
        let hints = self.closure_param_hints(node, env);
        let params = callable
            .params
            .iter()
            .enumerate()
            .map(|(position, param)| php_index::types::CallableParam {
                ty: param
                    .effective_type(level)
                    .cloned()
                    .or_else(|| hints.get(position).filter(|hint| !hint.is_unknown()).cloned())
                    .unwrap_or(Type::Mixed),
                optional: param.default.is_some(),
                variadic: param.variadic,
                by_ref: param.by_ref,
                name: Some(param.name.clone()),
            })
            .collect();
        let ret = callable.effective_return(level).cloned().or_else(|| {
            let inferred = self.infer_body_return(node);
            (!inferred.is_unknown()).then_some(inferred)
        });
        Type::Callable(Some(Box::new(php_index::types::CallableType {
            closure: true,
            params,
            ret,
        })))
    }

    /// The key and value types of something a `foreach` can walk.
    pub fn iterable_types(&self, ty: &Type) -> (Type, Type) {
        let mut keys = Vec::new();
        let mut values = Vec::new();
        for member in ty.members() {
            match member {
                Type::Array(key, value) | Type::Iterable(key, value) => {
                    keys.push((**key).clone());
                    values.push((**value).clone());
                }
                Type::List(value) => {
                    keys.push(Type::Int);
                    values.push((**value).clone());
                }
                Type::Shape(fields) => {
                    keys.push(Type::ArrayKey);
                    values.extend(fields.iter().map(|field| field.ty.clone()));
                }
                Type::Class { .. } => {
                    let (key, value) = self.class_iteration(member);
                    keys.push(key);
                    values.push(value);
                }
                _ => {
                    keys.push(Type::Unknown);
                    values.push(Type::Unknown);
                }
            }
        }
        if values.is_empty() {
            return (Type::Unknown, Type::Unknown);
        }
        (Type::union(keys), Type::union(values))
    }

    fn class_iteration(&self, ty: &Type) -> (Type, Type) {
        for ancestor in self.index.ancestors(ty) {
            let name = ancestor.class.decl.name.to_ascii_lowercase();
            if matches!(
                name.as_str(),
                "iterator" | "iteratoraggregate" | "traversable" | "generator"
            ) {
                let templates: Vec<&str> = ancestor
                    .class
                    .decl
                    .doc
                    .iter()
                    .flat_map(|doc| doc.templates.iter().map(|template| template.name.as_str()))
                    .collect();
                let args: Vec<Type> = templates
                    .iter()
                    .map(|template| ancestor.subst.get(*template).cloned().unwrap_or(Type::Unknown))
                    .collect();
                match args.as_slice() {
                    [key, value, ..] => return (key.clone(), value.clone()),
                    [value] => return (Type::Unknown, value.clone()),
                    [] => {}
                }
            }
        }
        if let Some(found) = self.index.find_method(ty, "getIterator") {
            if let Some(ret) = found.member.callable.effective_return(self.level()) {
                let resolved = found.resolve(ret);
                if resolved != *ty && matches!(resolved, Type::Class { .. }) {
                    return self.iterable_types(&resolved);
                }
            }
        }
        if let Some(found) = self.index.find_method(ty, "current") {
            if let Some(ret) = found.member.callable.effective_return(self.level()) {
                return (Type::Unknown, found.resolve(ret));
            }
        }
        (Type::Unknown, Type::Unknown)
    }
}

fn is_assign_operator(kind: SyntaxKind) -> bool {
    matches!(
        kind,
        ASSIGN
            | PLUS_ASSIGN
            | MINUS_ASSIGN
            | STAR_ASSIGN
            | SLASH_ASSIGN
            | DOT_ASSIGN
            | PERCENT_ASSIGN
            | POW_ASSIGN
            | COALESCE_ASSIGN
            | AMP_ASSIGN
            | PIPE_ASSIGN
            | CARET_ASSIGN
            | SHL_ASSIGN
            | SHR_ASSIGN
    )
}

fn arithmetic(operator: Option<SyntaxKind>, left: &Type, right: &Type) -> Type {
    let is_int = |ty: &Type| matches!(ty, Type::Int | Type::IntLiteral(_));
    let is_array = |ty: &Type| matches!(ty, Type::Array(..) | Type::List(_) | Type::Shape(_));
    if operator == Some(PLUS) && (is_array(left) || is_array(right)) {
        return if is_array(left) { left.clone() } else { right.clone() };
    }
    match (is_int(left), is_int(right), operator) {
        (true, true, Some(SLASH | POW)) => Type::union([Type::Int, Type::Float]),
        (true, true, _) => Type::Int,
        _ if *left == Type::Float || *right == Type::Float => Type::Float,
        _ => Type::union([Type::Int, Type::Float]),
    }
}

fn literal_type(node: &SyntaxNode) -> Type {
    let Some(token) = tokens(node).find(|token| !token.kind().is_trivia()) else {
        return Type::Unknown;
    };
    match token.kind() {
        INT_LITERAL => Type::Int,
        FLOAT_LITERAL => Type::Float,
        STRING_LITERAL | DOUBLE_QUOTE | HEREDOC_START => Type::String,
        MAGIC_CONSTANT => {
            if token.text().eq_ignore_ascii_case("__LINE__") {
                Type::Int
            } else {
                Type::String
            }
        }
        _ => Type::Unknown,
    }
}

fn cast_type(node: &SyntaxNode) -> Type {
    let Some(token) = first_token(node, CAST) else {
        return Type::Unknown;
    };
    let text = token
        .text()
        .trim_matches(|c: char| c == '(' || c == ')' || c.is_whitespace())
        .to_ascii_lowercase();
    match text.as_str() {
        "int" | "integer" => Type::Int,
        "float" | "double" | "real" => Type::Float,
        "string" | "binary" => Type::String,
        "bool" | "boolean" => Type::Bool,
        "array" => Type::plain_array(),
        "object" => Type::Object,
        "unset" | "void" => Type::Null,
        _ => Type::Unknown,
    }
}

/// The text of a string literal node without its quotes.
pub fn literal_string(node: &SyntaxNode) -> Option<String> {
    if node.kind() != LITERAL {
        return None;
    }
    let token = tokens(node).find(|token| token.kind() == STRING_LITERAL)?;
    let text = token.text();
    let inner = text.get(1..text.len().checked_sub(1)?)?;
    Some(inner.to_string())
}

fn literal_string_or_int(node: &SyntaxNode) -> Option<String> {
    if let Some(text) = literal_string(node) {
        return Some(text);
    }
    if node.kind() == LITERAL {
        let token = tokens(node).find(|token| token.kind() == INT_LITERAL)?;
        return Some(token.text().to_string());
    }
    None
}

fn superglobal_type(name: &str) -> Option<Type> {
    match name {
        "_GET" | "_POST" | "_COOKIE" | "_FILES" | "_REQUEST" | "_SESSION" | "_ENV" | "_SERVER" | "GLOBALS" => {
            Some(Type::plain_array())
        }
        _ => None,
    }
}
