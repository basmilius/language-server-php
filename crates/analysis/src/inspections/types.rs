//! What the inspections ask of types: whether a class and everything above it is known, which
//! kinds of value a type allows, and whether one type can stand where another is declared.

use php_index::{ClassKind, Origin, Param, Type};
use php_syntax::SyntaxKind::*;
use php_syntax::SyntaxNode;

use super::Cx;
use super::util::assigns_variable;
use crate::ast::{self, text_of};
use crate::infer::{Analyzer, Env};

/// The kinds of value a type allows, as bits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Kinds(u8);

impl Kinds {
    pub const INT: Kinds = Kinds(1);
    pub const FLOAT: Kinds = Kinds(2);
    pub const STRING: Kinds = Kinds(4);
    pub const BOOL: Kinds = Kinds(8);
    pub const NULL: Kinds = Kinds(16);
    pub const ARRAY: Kinds = Kinds(32);
    pub const OBJECT: Kinds = Kinds(64);
    pub const SCALAR: Kinds = Kinds(1 | 2 | 4 | 8);

    pub fn union(self, other: Kinds) -> Kinds {
        Kinds(self.0 | other.0)
    }

    pub fn intersects(self, other: Kinds) -> bool {
        self.0 & other.0 != 0
    }

    pub fn contains(self, other: Kinds) -> bool {
        self.0 & other.0 == other.0
    }
}

/// The kinds a type allows, or `None` when it says too little to tell.
pub fn kinds_of(ty: &Type) -> Option<Kinds> {
    Some(match ty {
        Type::Int | Type::IntLiteral(_) => Kinds::INT,
        Type::Float => Kinds::FLOAT,
        Type::String | Type::StringLiteral(_) | Type::ClassString(_) => Kinds::STRING,
        Type::Bool | Type::True | Type::False => Kinds::BOOL,
        Type::Null => Kinds::NULL,
        Type::Array(..) | Type::List(_) | Type::Shape(_) => Kinds::ARRAY,
        Type::Class { .. } | Type::Object | Type::Static | Type::SelfType | Type::Parent => Kinds::OBJECT,
        Type::ArrayKey => Kinds::INT.union(Kinds::STRING),
        Type::Numeric => Kinds::INT.union(Kinds::FLOAT).union(Kinds::STRING),
        Type::Scalar => Kinds::SCALAR,
        Type::Iterable(..) => Kinds::ARRAY.union(Kinds::OBJECT),
        Type::Callable(_) => Kinds::STRING.union(Kinds::ARRAY).union(Kinds::OBJECT),
        Type::Union(members) => {
            let mut all = Kinds::default();
            for member in members {
                all = all.union(kinds_of(member)?);
            }
            all
        }
        _ => return None,
    })
}

impl Cx<'_> {
    /// Whether the class and every class above it, through `extends`, `implements`, traits and
    /// mixins, is in the index. A member missing from an incomplete class may live in the part
    /// that is not known.
    pub fn hierarchy_complete(&self, name: &str) -> bool {
        let key = name.to_ascii_lowercase();
        if let Some(known) = self.complete.borrow().get(&key) {
            return *known;
        }
        self.complete.borrow_mut().insert(key.clone(), true);
        let verdict = self.compute_complete(name);
        self.complete.borrow_mut().insert(key, verdict);
        verdict
    }

    fn compute_complete(&self, name: &str) -> bool {
        let Some(class) = self.index.class(name) else {
            return false;
        };
        let decl = class.decl;
        let mut referenced: Vec<&Type> = Vec::new();
        referenced.extend(&decl.extends);
        referenced.extend(&decl.implements);
        referenced.extend(decl.trait_uses.iter().map(|usage| &usage.ty));
        if let Some(doc) = &decl.doc {
            referenced.extend(&doc.extends);
            referenced.extend(&doc.implements);
            referenced.extend(&doc.uses);
            referenced.extend(&doc.mixins);
        }
        referenced
            .iter()
            .flat_map(|ty| ty.class_names())
            .all(|parent| self.hierarchy_complete(parent))
    }

    /// The classes a receiver type stands for, when every member of it is a class that is wholly
    /// known and none is a trait, whose members belong to the class that uses it.
    pub fn known_classes(&self, ty: &Type) -> Option<Vec<String>> {
        let mut out = Vec::new();
        for member in ty.members() {
            let Type::Class { name, .. } = member else {
                return None;
            };
            let class = self.index.class(name)?;
            let declared_once = self.index.class_declarations(name) == 1;
            if class.decl.kind == ClassKind::Trait || !declared_once || !self.hierarchy_complete(name) {
                return None;
            }
            out.push(class.decl.name.clone());
        }
        (!out.is_empty()).then_some(out)
    }

    /// Whether a class or anything above it has one of the magic methods.
    pub fn has_magic(&self, class: &str, methods: &[&str]) -> bool {
        let ty = Type::class(class);
        methods
            .iter()
            .any(|method| self.index.find_method(&ty, method).is_some())
    }

    /// Whether a class is the project's or a package's own, whose declarations are whole, and not
    /// one of the standard library, whose stubs leave out what the engine adds.
    pub fn is_user_class(&self, name: &str) -> bool {
        self.index
            .class(name)
            .is_some_and(|class| class.file.origin != Origin::Stub)
    }
}

impl Cx<'_> {
    /// Whether a value of one type can never be passed where the other is declared. Only what is
    /// certain counts: the kinds of value do not overlap, or both are classes that cannot be the
    /// same object. In the default mode a scalar converts to another, so only strict files see those.
    /// `lenient_null` is for the standard library, which takes `null` for a scalar with a deprecation.
    pub fn type_mismatch(&self, given: &Type, wanted: &Type, lenient_null: bool) -> bool {
        let (Some(given_kinds), Some(wanted_kinds)) = (kinds_of(given), kinds_of(wanted)) else {
            return false;
        };
        let mut accepted = wanted_kinds;
        if wanted_kinds.contains(Kinds::FLOAT) {
            accepted = accepted.union(Kinds::INT);
        }
        if !self.strict_types && wanted_kinds.intersects(Kinds::SCALAR) {
            accepted = accepted.union(Kinds::SCALAR);
            if lenient_null {
                accepted = accepted.union(Kinds::NULL);
            }
        }
        if !given_kinds.intersects(accepted) {
            // An object that converts to a string is still accepted by a string in the default mode.
            return !(given_kinds.contains(Kinds::OBJECT)
                && !self.strict_types
                && wanted_kinds.contains(Kinds::STRING)
                && self.may_convert_to_string(given));
        }
        self.classes_never_match(given, wanted)
    }

    fn may_convert_to_string(&self, given: &Type) -> bool {
        given.members().iter().any(|member| match member {
            Type::Class { name, .. } => {
                !self.hierarchy_complete(name) || self.index.find_method(member, "__toString").is_some()
            }
            _ => true,
        })
    }

    fn classes_never_match(&self, given: &Type, wanted: &Type) -> bool {
        let given_classes: Option<Vec<&str>> = given
            .members()
            .iter()
            .map(|member| match member {
                Type::Class { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        let Some(given_classes) = given_classes else {
            return false;
        };
        let mut wanted_classes: Vec<&str> = Vec::new();
        for member in wanted.members() {
            match member {
                Type::Class { name, .. } => wanted_classes.push(name),
                Type::Null | Type::Int | Type::Float | Type::String | Type::Bool | Type::True | Type::False => {}
                Type::Array(..) | Type::List(_) | Type::Shape(_) => {}
                _ => return false,
            }
        }
        let accepts_string = !self.strict_types && wanted.members().iter().any(|member| matches!(member, Type::String));
        if wanted_classes.is_empty() {
            return false;
        }
        for given_class in given_classes {
            if accepts_string
                && self
                    .index
                    .find_method(&Type::class(given_class), "__toString")
                    .is_some()
            {
                return false;
            }
            for wanted_class in &wanted_classes {
                if !self.disjoint(given_class, wanted_class) {
                    return false;
                }
            }
        }
        true
    }

    /// Whether the classes of two types can never be the same object, when both are all classes.
    pub fn classes_cannot_overlap(&self, left: &Type, right: &Type) -> bool {
        let names = |ty: &'_ Type| -> Option<Vec<String>> {
            ty.members()
                .iter()
                .filter(|member| !matches!(member, Type::Null))
                .map(|member| match member {
                    Type::Class { name, .. } => Some(name.clone()),
                    _ => None,
                })
                .collect()
        };
        let (Some(left), Some(right)) = (names(left), names(right)) else {
            return false;
        };
        !left.is_empty()
            && !right.is_empty()
            && left
                .iter()
                .all(|left| right.iter().all(|right| self.disjoint(left, right)))
    }

    /// Whether no object can be an instance of both classes.
    fn disjoint(&self, left: &str, right: &str) -> bool {
        const IMPLICIT: &[&str] = &[
            "Stringable",
            "UnitEnum",
            "BackedEnum",
            "Traversable",
            "Iterator",
            "IteratorAggregate",
        ];
        if IMPLICIT.iter().any(|name| right.eq_ignore_ascii_case(name)) {
            return false;
        }
        let (Some(left_class), Some(right_class)) = (self.index.class(left), self.index.class(right)) else {
            return false;
        };
        if !self.hierarchy_complete(left) || !self.hierarchy_complete(right) {
            return false;
        }
        if self.index.is_subclass_of(left, right) || self.index.is_subclass_of(right, left) {
            return false;
        }
        let closed = |class: &php_index::Class<'_>| class.decl.is_final || matches!(class.decl.kind, ClassKind::Enum);
        let is_class = |class: &php_index::Class<'_>| class.decl.kind == ClassKind::Class;
        closed(&left_class) || closed(&right_class) || (is_class(&left_class) && is_class(&right_class))
    }
}

/// What PHP itself guarantees an expression is: the type of a literal, of `new`, of a parameter
/// that is never assigned again, of a typed property or of a call whose return type is declared.
/// What the type layer infers from PHPDoc or from the way variables flow is not that sure.
pub(crate) fn sure_type(cx: &Cx, analyzer: &Analyzer<'_>, env: &Env, expr: &SyntaxNode) -> Option<Type> {
    match expr.kind() {
        LITERAL | ARRAY_EXPR | CAST_EXPR => {
            let ty = analyzer.type_of(expr, env);
            (!ty.is_unknown()).then_some(ty)
        }
        NAME => match text_of(expr).to_ascii_lowercase().as_str() {
            "true" => Some(Type::True),
            "false" => Some(Type::False),
            "null" => Some(Type::Null),
            _ => None,
        },
        INTERPOLATED_STRING | HEREDOC => Some(Type::String),
        CLOSURE_EXPR | ARROW_FUNCTION_EXPR => Some(Type::class("Closure")),
        PAREN_EXPR => sure_type(cx, analyzer, env, &expr.children().next()?),
        NEW_EXPR => match analyzer.type_of(expr, env) {
            ty @ Type::Class { .. } => Some(ty),
            _ => None,
        },
        BINARY_EXPR => {
            let operator = ast::tokens(expr).find(|token| !token.kind().is_trivia())?;
            match operator.kind() {
                DOT => Some(Type::String),
                EQ | NEQ | IDENTICAL | NOT_IDENTICAL | LT | GT | LE | GE | AND_AND | OR_OR | INSTANCEOF_KW => {
                    Some(Type::Bool)
                }
                _ => None,
            }
        }
        PREFIX_EXPR if ast::tokens(expr).any(|token| token.kind() == BANG) => Some(Type::Bool),
        VARIABLE_EXPR => sure_parameter_type(cx, analyzer, expr),
        PROPERTY_FETCH_EXPR => sure_property_type(cx, analyzer, env, expr).map(|ty| with_nullsafe(expr, ty)),
        CALL_EXPR => sure_return_type(cx, analyzer, env, expr).map(|ty| with_nullsafe(expr, ty)),
        _ => None,
    }
}

/// The signature of a function as it is written, with the types read from the code and no PHPDoc.
pub(crate) fn callable_of(analyzer: &Analyzer<'_>, function: &SyntaxNode) -> php_index::Callable {
    let class_scope = analyzer.class.as_ref().map(|class| php_index::extract::ClassScope {
        name: class.name.clone(),
        parent: class.parent.clone(),
        is_trait: class.kind == php_index::ClassKind::Trait,
        templates: Vec::new(),
        aliases: Vec::new(),
    });
    php_index::extract::callable_at(function, &analyzer.resolver, class_scope.as_ref()).0
}

fn sure_parameter_type(cx: &Cx, analyzer: &Analyzer<'_>, variable: &SyntaxNode) -> Option<Type> {
    let name = text_of(variable);
    let function = variable
        .ancestors()
        .find(|ancestor| matches!(ancestor.kind(), FUNCTION_DECLARATION | METHOD_DECLARATION))?;
    if variable
        .ancestors()
        .take_while(|ancestor| *ancestor != function)
        .any(|ancestor| matches!(ancestor.kind(), CLOSURE_EXPR | ARROW_FUNCTION_EXPR))
    {
        return None;
    }
    if assigns_variable(&function, &name) {
        return None;
    }
    let callable = callable_of(analyzer, &function);
    let param: &Param = callable
        .params
        .iter()
        .find(|param| format!("${}", param.name) == name)?;
    if param.variadic || param.by_ref {
        return None;
    }
    let ty = param.native_type(cx.index.level)?.clone();
    // A parameter with a default of `null` takes null whatever its type says.
    let ty = if param
        .default
        .as_deref()
        .is_some_and(|default| default.eq_ignore_ascii_case("null"))
    {
        ty.nullable()
    } else {
        ty
    };
    concrete(&ty)
}

fn sure_property_type(cx: &Cx, analyzer: &Analyzer<'_>, env: &Env, access: &SyntaxNode) -> Option<Type> {
    let object = access.children().next()?;
    let name = access.children().filter(|child| child.kind() == NAME).last()?;
    let receiver = analyzer.receiver_type(&analyzer.type_of(&object, env));
    let classes = cx.known_classes(&receiver)?;
    if classes.len() != 1 {
        return None;
    }
    let found = cx.index.find_property(&receiver, &text_of(&name))?;
    if found.member.is_static || found.member.doc_ty.is_some() && found.member.ty.is_none() {
        return None;
    }
    concrete(found.member.native_type(cx.index.level)?)
}

fn sure_return_type(cx: &Cx, analyzer: &Analyzer<'_>, env: &Env, call: &SyntaxNode) -> Option<Type> {
    if crate::infer::is_first_class_callable(call) {
        return Some(Type::class("Closure"));
    }
    let mut callees = analyzer.callees(call, env);
    callees.dedup_by(|left, right| left.name == right.name);
    let [callee] = callees.as_slice() else {
        return None;
    };
    if callee.is_constructor || callee.name == "closure" {
        return None;
    }
    concrete(callee.callable.native_return(cx.index.level)?)
}

/// `$a?->b` is null when `$a` is, anywhere along the chain.
fn with_nullsafe(expr: &SyntaxNode, ty: Type) -> Type {
    let mut current = Some(expr.clone());
    while let Some(node) = current {
        if matches!(node.kind(), PROPERTY_FETCH_EXPR) && ast::tokens(&node).any(|token| token.kind() == NULLSAFE_ARROW)
        {
            return ty.nullable();
        }
        current = match node.kind() {
            PROPERTY_FETCH_EXPR | CALL_EXPR | INDEX_EXPR | PAREN_EXPR => node.children().next(),
            _ => None,
        };
    }
    ty
}

/// A type with nothing in it that depends on where it is read: no `static`, `self` or template.
fn concrete(ty: &Type) -> Option<Type> {
    let sure = ty.members().iter().all(|member| {
        matches!(
            member,
            Type::Int
                | Type::Float
                | Type::String
                | Type::Bool
                | Type::True
                | Type::False
                | Type::Null
                | Type::Array(..)
                | Type::List(_)
                | Type::Object
                | Type::Class { .. }
                | Type::Callable(None)
                | Type::Iterable(..)
        )
    });
    sure.then(|| ty.clone())
}
