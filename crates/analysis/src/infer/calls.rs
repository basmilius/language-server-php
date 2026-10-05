//! Calls: which function or method a call names, and what it returns once the templates are bound.

use std::collections::HashMap;
use std::path::PathBuf;

use php_index::types::CallableParam;
use php_index::{Callable, Doc, Name, Origin, Param, Type};
use php_syntax::SyntaxKind::*;
use php_syntax::SyntaxNode;

use super::unify::template_names;
use super::{Analyzer, Env};
use crate::ast::{child_of, has_token, text_of, tokens};

/// One argument of a call.
#[derive(Clone, Debug)]
pub struct Arg {
    pub name: Option<String>,
    pub expr: Option<SyntaxNode>,
    pub spread: bool,
}

/// Where a function or method is declared, to read its body when it declares no return type.
#[derive(Clone, Debug)]
pub struct DeclRef {
    pub path: PathBuf,
    /// The offset of the name in the declaration.
    pub name_start: u32,
}

/// A function or method a call resolves to, with what its types need to be read from the call site.
#[derive(Clone, Debug)]
pub struct ResolvedCallable {
    /// As the call names it: `strlen`, `User::find`.
    pub name: String,
    pub callable: Callable,
    pub doc: Option<Box<Doc>>,
    /// The template arguments of the class the method was found through.
    pub subst: HashMap<String, Type>,
    pub self_name: Option<Name>,
    /// The type `static` stands for.
    pub receiver: Option<Type>,
    pub is_constructor: bool,
    /// The class the constructor belongs to, with its template names.
    pub constructed: Option<Name>,
    /// Lent by a `@mixin`: the call reaches it through `__call`, which takes anything.
    pub via_mixin: bool,
    pub decl: Option<DeclRef>,
}

/// The arguments of a call or `new` node.
pub fn arguments(node: &SyntaxNode) -> Vec<Arg> {
    let Some(list) = child_of(node, ARGUMENT_LIST) else {
        return Vec::new();
    };
    list.children()
        .filter(|child| child.kind() == ARGUMENT)
        .map(|argument| {
            let name = has_token(&argument, COLON).then(|| {
                tokens(&argument)
                    .find(|token| !token.kind().is_trivia() && token.kind() != COLON)
                    .map(|token| token.text().to_string())
                    .unwrap_or_default()
            });
            Arg {
                name,
                expr: argument.children().last(),
                spread: has_token(&argument, ELLIPSIS),
            }
        })
        .collect()
}

impl Analyzer<'_> {
    pub(super) fn new_type(&self, node: &SyntaxNode, env: &Env) -> Type {
        let Some(class_node) = node.children().find(|child| child.kind() != ARGUMENT_LIST) else {
            return Type::Unknown;
        };
        if class_node.kind() == ANONYMOUS_CLASS {
            return self.anonymous_class_type(&class_node);
        }
        let class = match class_node.kind() {
            NAME => self.class_type(&text_of(&class_node)),
            _ => match self.type_of(&class_node, env) {
                Type::ClassString(Some(inner)) => *inner,
                Type::Class { .. } => Type::Object,
                _ => return Type::Unknown,
            },
        };
        let class = match class {
            Type::Static => return Type::Static,
            other => other,
        };
        let Type::Class { name, .. } = &class else {
            return class;
        };
        let templates: Vec<String> = self
            .index
            .class(name)
            .and_then(|found| {
                found
                    .decl
                    .doc
                    .as_ref()
                    .map(|doc| doc.templates.iter().map(|template| template.name.clone()).collect())
            })
            .unwrap_or_default();
        if templates.is_empty() {
            return class;
        }
        let Some(found) = self.index.find_method(&class, "__construct") else {
            return class;
        };
        let callable = ResolvedCallable {
            name: format!("{name}::__construct"),
            callable: found.member.callable.clone(),
            doc: found.member.doc.clone(),
            subst: HashMap::new(),
            self_name: Some(found.self_name.clone()),
            receiver: Some(class.clone()),
            is_constructor: true,
            constructed: Some(name.clone()),
            via_mixin: false,
            decl: None,
        };
        let (map, _) = self.bind_call(&callable, &arguments(node), env);
        let args = templates
            .iter()
            .map(|template| map.get(template).cloned().unwrap_or(Type::Unknown))
            .collect();
        Type::Class {
            name: name.clone(),
            args,
        }
    }

    fn anonymous_class_type(&self, node: &SyntaxNode) -> Type {
        let first_name = |kind| {
            child_of(node, kind)
                .and_then(|clause| clause.descendants().find(|child| child.kind() == NAME))
                .map(|name| self.resolver.resolve_class(&text_of(&name)))
        };
        match first_name(EXTENDS_CLAUSE).or_else(|| first_name(IMPLEMENTS_CLAUSE)) {
            Some(name) => Type::class(name),
            None => Type::Object,
        }
    }

    /// Every function or method a call node may be calling.
    pub fn callees(&self, call: &SyntaxNode, env: &Env) -> Vec<ResolvedCallable> {
        match call.kind() {
            NEW_EXPR => self.constructors(call, env),
            CALL_EXPR => {
                let Some(callee) = call.children().next() else {
                    return Vec::new();
                };
                self.callees_of_expression(&callee, env)
            }
            _ => Vec::new(),
        }
    }

    fn constructors(&self, node: &SyntaxNode, env: &Env) -> Vec<ResolvedCallable> {
        let Some(class_node) = node.children().find(|child| child.kind() != ARGUMENT_LIST) else {
            return Vec::new();
        };
        let class = match class_node.kind() {
            NAME => self.class_type(&text_of(&class_node)),
            ANONYMOUS_CLASS => self.anonymous_class_type(&class_node),
            _ => match self.type_of(&class_node, env) {
                Type::ClassString(Some(inner)) => *inner,
                other => other,
            },
        };
        let receiver = self.receiver_type(&class);
        let mut out = Vec::new();
        for member in receiver.members() {
            if let Some(found) = self.index.find_method(member, "__construct") {
                out.push(ResolvedCallable {
                    name: format!("{}::__construct", found.class.decl.name),
                    callable: found.member.callable.clone(),
                    doc: found.member.doc.clone(),
                    subst: found.subst.as_ref().clone(),
                    self_name: Some(found.self_name.clone()),
                    receiver: Some(member.clone()),
                    is_constructor: true,
                    constructed: Some(found.class.decl.name.clone()),
                    via_mixin: false,
                    decl: None,
                });
            }
        }
        out
    }

    pub fn callees_of_expression(&self, callee: &SyntaxNode, env: &Env) -> Vec<ResolvedCallable> {
        match callee.kind() {
            NAME => {
                let candidates = self.resolver.function_candidates(&text_of(callee));
                match self.index.first_function(&candidates) {
                    Some(function) => vec![ResolvedCallable {
                        name: function.decl.name.clone(),
                        callable: function.decl.callable.clone(),
                        doc: function.decl.doc.clone(),
                        subst: HashMap::new(),
                        self_name: None,
                        receiver: None,
                        is_constructor: false,
                        constructed: None,
                        via_mixin: false,
                        decl: (function.file.origin != Origin::Stub).then(|| DeclRef {
                            path: function.file.path.clone(),
                            name_start: function.decl.name_span.start,
                        }),
                    }],
                    None => Vec::new(),
                }
            }
            PROPERTY_FETCH_EXPR => {
                let (Some(object), Some(name)) = (callee.children().next(), child_of(callee, NAME)) else {
                    return Vec::new();
                };
                let receiver = self.receiver_type(&self.type_of(&object, env));
                self.methods_named(&receiver, &text_of(&name))
            }
            SCOPED_ACCESS_EXPR => {
                let Some(qualifier) = callee.children().next() else {
                    return Vec::new();
                };
                let Some(name) = callee
                    .children()
                    .filter(|child| child.kind() == NAME)
                    .last()
                    .filter(|name| name != &qualifier)
                else {
                    return Vec::new();
                };
                let class = self.qualifier_type(&qualifier, env);
                let receiver = self.receiver_type(&class);
                self.methods_named(&receiver, &text_of(&name))
            }
            _ => {
                let ty = self.type_of(callee, env);
                let mut out = Vec::new();
                for member in ty.members() {
                    match member {
                        Type::Callable(Some(signature)) => out.push(ResolvedCallable {
                            name: "closure".to_string(),
                            callable: Callable {
                                params: signature
                                    .params
                                    .iter()
                                    .enumerate()
                                    .map(|(index, param)| param_of_signature(param, index))
                                    .collect(),
                                ret: None,
                                doc_ret: signature.ret.clone(),
                                leveled_ret: None,
                                by_ref_return: false,
                                is_generator: false,
                                reads_all_arguments: false,
                            },
                            doc: None,
                            subst: HashMap::new(),
                            self_name: None,
                            receiver: None,
                            is_constructor: false,
                            constructed: None,
                            via_mixin: false,
                            decl: None,
                        }),
                        Type::Class { .. } => out.extend(self.methods_named(member, "__invoke")),
                        _ => {}
                    }
                }
                out
            }
        }
    }

    fn methods_named(&self, receiver: &Type, name: &str) -> Vec<ResolvedCallable> {
        let mut out = Vec::new();
        for member in receiver.members() {
            if !matches!(member, Type::Class { .. } | Type::Intersection(_)) {
                continue;
            }
            if let Some(found) = self.index.find_method(member, name) {
                out.push(ResolvedCallable {
                    name: format!("{}::{}", found.class.decl.name, found.member.name),
                    callable: found.member.callable.clone(),
                    doc: found.member.doc.clone(),
                    subst: found.subst.as_ref().clone(),
                    self_name: Some(found.self_name.clone()),
                    receiver: Some(found.static_as.clone().unwrap_or_else(|| member.clone())),
                    is_constructor: false,
                    constructed: None,
                    via_mixin: found.mixin,
                    decl: (found.class.file.origin != Origin::Stub).then(|| DeclRef {
                        path: found.class.file.path.clone(),
                        name_start: found.member.name_span.start,
                    }),
                });
            }
        }
        out
    }

    pub(super) fn call_type(&self, node: &SyntaxNode, env: &Env) -> Type {
        let Some(callee_node) = node.children().next() else {
            return Type::Unknown;
        };
        if is_first_class_callable(node) {
            return Type::class("Closure");
        }
        let callees = self.callees(node, env);
        if callees.is_empty() {
            return self.custom_expectation_type(&callee_node, env);
        }
        let args = arguments(node);
        let mut results = Vec::new();
        for callee in &callees {
            results.push(self.return_type_of(callee, &args, env));
        }
        let result = self.container_call_type(&callees, &args, env, Type::union(results));
        let nullsafe = callee_node.kind() == PROPERTY_FETCH_EXPR && has_token(&callee_node, NULLSAFE_ARROW);
        if nullsafe {
            let receiver = callee_node.children().next().map(|object| self.type_of(&object, env));
            if receiver.is_some_and(|receiver| receiver.contains_null()) {
                return result.nullable();
            }
        }
        result
    }

    /// A call of an expectation the project added: it gives the expectation back.
    fn custom_expectation_type(&self, callee: &SyntaxNode, env: &Env) -> Type {
        if callee.kind() != PROPERTY_FETCH_EXPR {
            return Type::Unknown;
        }
        let (Some(object), Some(name)) = (callee.children().next(), child_of(callee, NAME)) else {
            return Type::Unknown;
        };
        if self.custom_expectation(&text_of(&name)).is_none() {
            return Type::Unknown;
        }
        let receiver = self.type_of(&object, env);
        if crate::pest::is_expectation(&receiver) {
            return receiver;
        }
        Type::Unknown
    }

    /// Binds the templates of a callable from the arguments of a call, and collects what is known
    /// of each parameter's argument by name for conditional types. Closures go last: what they
    /// return is read with the parameter types the other arguments gave them.
    pub fn bind_call(
        &self,
        callee: &ResolvedCallable,
        args: &[Arg],
        env: &Env,
    ) -> (HashMap<String, Type>, HashMap<String, Type>) {
        let mut map = callee.subst.clone();
        let mut by_name: HashMap<String, Type> = HashMap::new();
        self.bind_args(callee, args, env, ArgPass::Plain, &mut map, &mut by_name);
        self.bind_args(callee, args, env, ArgPass::Closures, &mut map, &mut by_name);
        let level = self.level();
        for (position, param) in callee.callable.params_at(level).enumerate() {
            let Some(default) = &param.default else {
                continue;
            };
            let omitted = !args.iter().enumerate().any(|(at, arg)| {
                arg.name.as_deref() == Some(param.name.as_str()) || arg.name.is_none() && at == position
            });
            let ty = default_type(default);
            if omitted && !ty.is_unknown() {
                if let Some(expected) = param.effective_type(level) {
                    self.bind_templates(expected, &ty, &mut map);
                }
            }
            by_name.entry(format!("${}", param.name)).or_insert(ty);
        }
        (map, by_name)
    }

    fn bind_args(
        &self,
        callee: &ResolvedCallable,
        args: &[Arg],
        env: &Env,
        pass: ArgPass,
        map: &mut HashMap<String, Type>,
        by_name: &mut HashMap<String, Type>,
    ) {
        let level = self.level();
        let params: Vec<&Param> = callee.callable.params_at(level).collect();
        for (position, arg) in args.iter().enumerate() {
            let Some(expr) = &arg.expr else {
                continue;
            };
            let is_closure = matches!(expr.kind(), CLOSURE_EXPR | ARROW_FUNCTION_EXPR);
            if is_closure != (pass == ArgPass::Closures) {
                continue;
            }
            let Some(param) = param_for(&params, arg, position) else {
                continue;
            };
            let arg_type = self.type_of(expr, env);
            if let Some(expected) = param.effective_type(level) {
                let expected = if param.variadic && arg.spread {
                    Type::List(Box::new(expected.clone()))
                } else {
                    expected.clone()
                };
                self.bind_templates(&expected, &arg_type, map);
            }
            by_name.insert(format!("${}", param.name), arg_type);
        }
    }

    /// The types a call gives the parameters of a closure it is handed, by position: the parameter
    /// the closure is passed for says `callable(T): U`, and `T` is what the other arguments bound.
    pub(super) fn closure_param_hints(&self, closure: &SyntaxNode, outer: &Env) -> Vec<Type> {
        let key = crate::ast::start(closure);
        if let Some(found) = self.hints.borrow().get(&key) {
            return found.clone();
        }
        self.hints.borrow_mut().insert(key, Vec::new());
        let hints = self.compute_hints(closure, outer);
        self.hints.borrow_mut().insert(key, hints.clone());
        hints
    }

    fn compute_hints(&self, closure: &SyntaxNode, outer: &Env) -> Vec<Type> {
        if let Some(hints) = crate::pest::dataset_hints(self, closure) {
            return hints;
        }
        let Some(call) = closure
            .parent()
            .filter(|parent| parent.kind() == ARGUMENT)
            .and_then(|argument| argument.parent())
            .and_then(|list| list.parent())
            .filter(|call| matches!(call.kind(), CALL_EXPR | NEW_EXPR))
        else {
            return Vec::new();
        };
        let args = arguments(&call);
        let Some(position) = args.iter().position(|arg| arg.expr.as_ref() == Some(closure)) else {
            return Vec::new();
        };
        let level = self.level();
        for callee in self.callees(&call, outer) {
            let params: Vec<&Param> = callee.callable.params_at(level).collect();
            let Some(param) = param_for(&params, &args[position], position) else {
                continue;
            };
            let Some(signature) = param.effective_type(level).and_then(|expected| {
                expected.members().iter().find_map(|member| match member {
                    Type::Callable(Some(signature)) => Some(signature.clone()),
                    _ => None,
                })
            }) else {
                continue;
            };
            let mut map = callee.subst.clone();
            let mut by_name = HashMap::new();
            self.bind_args(&callee, &args, outer, ArgPass::Plain, &mut map, &mut by_name);
            let mut hints: Vec<Type> = signature
                .params
                .iter()
                .map(|param| {
                    let ty = param
                        .ty
                        .substitute(&map, callee.receiver.as_ref(), callee.self_name.as_deref());
                    if ty.has_template() { Type::Unknown } else { ty }
                })
                .collect();
            // With ARRAY_FILTER_USE_KEY the callback gets the key alone, which no signature says.
            if callee.name.eq_ignore_ascii_case("array_filter")
                && args
                    .get(2)
                    .and_then(|mode| mode.expr.as_ref())
                    .is_some_and(|mode| text_of(mode).ends_with("ARRAY_FILTER_USE_KEY"))
            {
                hints = vec![hints.get(1).cloned().unwrap_or(Type::Unknown)];
            }
            return hints;
        }
        Vec::new()
    }

    /// What a call returns: the declared or documented return type with templates bound and
    /// conditional types decided.
    pub fn return_type_of(&self, callee: &ResolvedCallable, args: &[Arg], env: &Env) -> Type {
        let level = self.level();
        let (mut map, by_name) = self.bind_call(callee, args, env);
        if callee.is_constructor {
            return match &callee.receiver {
                Some(receiver) => receiver.clone(),
                None => Type::Unknown,
            };
        }
        let declared = callee.callable.effective_return(level).cloned();
        let ret = match declared {
            Some(ret) => ret,
            None => {
                let inferred = self.inferred_return(callee);
                match inferred {
                    Type::Unknown if callee.callable.is_generator => Type::class("Generator"),
                    Type::Unknown => return Type::Unknown,
                    inferred => inferred,
                }
            }
        };
        let ret = self.resolve_conditionals(&ret, &by_name, &map);
        let mut leftover = Vec::new();
        template_names(&ret, &mut leftover);
        for name in leftover {
            if map.contains_key(&name) {
                continue;
            }
            let bound = callee
                .doc
                .as_ref()
                .and_then(|doc| doc.templates.iter().find(|template| template.name == name))
                .and_then(|template| template.bound.clone());
            map.insert(name, bound.unwrap_or(Type::Mixed));
        }
        let static_type = callee.receiver.as_ref();
        let resolved = ret.substitute(&map, static_type, callee.self_name.as_deref());
        match (&resolved, static_type) {
            (Type::Static, None) => self.this_type(),
            _ => resolved,
        }
    }
}

/// The type of a parameter's default value as far as a conditional return type needs it: an argument
/// left out is that value.
fn default_type(default: &str) -> Type {
    match default.trim().to_ascii_lowercase().as_str() {
        "null" => Type::Null,
        "true" => Type::True,
        "false" => Type::False,
        _ => Type::Unknown,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ArgPass {
    Plain,
    Closures,
}

/// The parameter a call's argument is for.
fn param_for<'p>(params: &[&'p Param], arg: &Arg, position: usize) -> Option<&'p Param> {
    match &arg.name {
        Some(name) => params.iter().find(|param| &param.name == name).copied(),
        None => params
            .get(position)
            .or_else(|| params.last().filter(|param| param.variadic))
            .copied(),
    }
}

/// `strlen(...)`, `$object->method(...)` and `Foo::method(...)`: the callable itself, not a call.
pub fn is_first_class_callable(call: &SyntaxNode) -> bool {
    child_of(call, ARGUMENT_LIST)
        .is_some_and(|list| list.children().next().is_none() && tokens(&list).any(|token| token.kind() == ELLIPSIS))
}

fn param_of_signature(param: &CallableParam, index: usize) -> Param {
    Param {
        name: param.name.clone().map_or_else(
            || format!("arg{index}"),
            |name| name.trim_start_matches('$').to_string(),
        ),
        ty: None,
        doc_ty: Some(param.ty.clone()),
        leveled: None,
        default: param.optional.then(|| "...".to_string()),
        variadic: param.variadic,
        by_ref: param.by_ref,
        promoted: None,
        description: String::new(),
        attributes: Vec::new(),
        availability: Default::default(),
        span: Default::default(),
    }
}
