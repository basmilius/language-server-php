//! What the index remembers about a file: its declarations with their signatures, PHPDoc, modifiers
//! and locations. Everything here is plain data that serializes into the cache, with class names
//! already resolved, so a [`FileSymbols`] needs nothing of the file it came from.

use php_syntax::PhpVersion;
use serde::{Deserialize, Serialize};

use crate::types::{Name, Type};

/// A byte range of the file the declaration is in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub start: u32,
    pub end: u32,
}

/// The versions a declaration exists in, from `@since`, `@removed` and the availability attributes of
/// the stubs. Versions are `major * 100 + minor`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Availability {
    pub from: Option<u16>,
    /// The first version without it.
    pub until: Option<u16>,
}

impl Availability {
    pub fn contains(&self, level: PhpVersion) -> bool {
        let level = encode_version(level);
        self.from.is_none_or(|from| level >= from) && self.until.is_none_or(|until| level < until)
    }

    pub fn is_unbounded(&self) -> bool {
        self.from.is_none() && self.until.is_none()
    }
}

pub fn encode_version(version: PhpVersion) -> u16 {
    u16::from(version.major) * 100 + u16::from(version.minor)
}

pub fn decode_version(version: u16) -> PhpVersion {
    PhpVersion::new((version / 100) as u8, (version % 100) as u8)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Visibility {
    #[default]
    Public,
    Protected,
    Private,
}

impl Visibility {
    pub fn keyword(self) -> &'static str {
        match self {
            Visibility::Public => "public",
            Visibility::Protected => "protected",
            Visibility::Private => "private",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ClassKind {
    Class,
    Interface,
    Trait,
    Enum,
}

impl ClassKind {
    pub fn keyword(self) -> &'static str {
        match self {
            ClassKind::Class => "class",
            ClassKind::Interface => "interface",
            ClassKind::Trait => "trait",
            ClassKind::Enum => "enum",
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttributeArg {
    pub name: Option<String>,
    /// The expression as written.
    pub value: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attribute {
    pub name: Name,
    pub args: Vec<AttributeArg>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Template {
    pub name: String,
    pub bound: Option<Type>,
    pub default: Option<Type>,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocParam {
    pub name: String,
    pub ty: Option<Type>,
    pub description: String,
    pub variadic: bool,
    pub by_ref: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocProperty {
    pub name: String,
    pub ty: Option<Type>,
    pub read_only: bool,
    pub write_only: bool,
    pub description: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocMethod {
    pub name: String,
    pub is_static: bool,
    pub ret: Option<Type>,
    pub params: Vec<DocParam>,
    pub description: String,
}

/// When an `@assert` tag holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum AssertWhen {
    /// The call returned at all.
    Always,
    IfTrue,
    IfFalse,
}

/// `@psalm-assert Type $subject` and its `phpstan-` and `-if-true` and `-if-false` forms.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DocAssert {
    pub when: AssertWhen,
    /// `$param`, `$this` or `$param->property`, as written.
    pub subject: String,
    pub ty: Type,
    /// `!Type`: the subject is not of the type.
    pub negated: bool,
    /// `=Type`: the subject is the type itself and not only a subtype.
    pub equality: bool,
}

/// A tag without a structure of its own, kept for display.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawTag {
    /// Without the `@`.
    pub name: String,
    pub text: String,
}

/// A parsed doc comment.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Doc {
    pub summary: String,
    pub description: String,
    pub params: Vec<DocParam>,
    pub ret: Option<(Type, String)>,
    pub var: Option<(Type, Option<String>, String)>,
    pub throws: Vec<(Type, String)>,
    pub templates: Vec<Template>,
    pub extends: Vec<Type>,
    pub implements: Vec<Type>,
    pub uses: Vec<Type>,
    pub mixins: Vec<Type>,
    pub properties: Vec<DocProperty>,
    pub methods: Vec<DocMethod>,
    /// `Some` with the explanation (possibly empty) when `@deprecated` is there.
    pub deprecated: Option<String>,
    pub internal: bool,
    pub since: Option<u16>,
    pub removed: Option<u16>,
    pub see: Vec<String>,
    pub asserts: Vec<DocAssert>,
    pub tags: Vec<RawTag>,
}

impl Doc {
    pub fn is_empty(&self) -> bool {
        *self == Doc::default()
    }
}

/// A type that depends on the language level, from `#[LanguageLevelTypeAware]` in the stubs.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LeveledType {
    /// From this version on, this type; sorted by version, newest first.
    pub levels: Vec<(u16, Type)>,
    pub default: Option<Type>,
}

impl LeveledType {
    pub fn at(&self, level: PhpVersion) -> Option<&Type> {
        let level = encode_version(level);
        self.levels
            .iter()
            .find(|(from, _)| level >= *from)
            .map(|(_, ty)| ty)
            .or(self.default.as_ref())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Param {
    /// Without the `$`.
    pub name: String,
    pub ty: Option<Type>,
    pub doc_ty: Option<Type>,
    pub leveled: Option<LeveledType>,
    /// The default value as written.
    pub default: Option<String>,
    pub variadic: bool,
    pub by_ref: bool,
    /// A constructor parameter that also declares a property.
    pub promoted: Option<Promotion>,
    pub description: String,
    pub attributes: Vec<Attribute>,
    pub availability: Availability,
    pub span: Span,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Promotion {
    pub visibility: Visibility,
    pub readonly: bool,
}

impl Param {
    /// Whether a call may leave the argument out: it has a default or is variadic, or the doc comment
    /// says so, as the stubs do with `[optional]` and an `@method` line does with `= value`.
    pub fn is_optional(&self) -> bool {
        let note = self.description.trim_start();
        self.default.is_some() || self.variadic || note.starts_with("[optional]") || note.starts_with('=')
    }

    /// The type calls and bodies see: the PHPDoc type when there is one, else the native one.
    pub fn effective_type(&self, level: PhpVersion) -> Option<&Type> {
        self.doc_ty
            .as_ref()
            .or_else(|| self.leveled.as_ref().and_then(|leveled| leveled.at(level)))
            .or(self.ty.as_ref())
    }

    /// The type as the signature writes it.
    pub fn native_type(&self, level: PhpVersion) -> Option<&Type> {
        self.leveled
            .as_ref()
            .and_then(|leveled| leveled.at(level))
            .or(self.ty.as_ref())
    }
}

/// What functions and methods share: parameters and a return type.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Callable {
    pub params: Vec<Param>,
    pub ret: Option<Type>,
    pub doc_ret: Option<Type>,
    pub leveled_ret: Option<LeveledType>,
    pub by_ref_return: bool,
    pub is_generator: bool,
    /// The body asks for its arguments as a list, so it takes more than it names.
    pub reads_all_arguments: bool,
}

impl Callable {
    pub fn effective_return(&self, level: PhpVersion) -> Option<&Type> {
        self.doc_ret
            .as_ref()
            .or_else(|| self.leveled_ret.as_ref().and_then(|leveled| leveled.at(level)))
            .or(self.ret.as_ref())
    }

    pub fn native_return(&self, level: PhpVersion) -> Option<&Type> {
        self.leveled_ret
            .as_ref()
            .and_then(|leveled| leveled.at(level))
            .or(self.ret.as_ref())
    }

    /// The parameters that exist at a level.
    pub fn params_at(&self, level: PhpVersion) -> impl Iterator<Item = &Param> {
        self.params
            .iter()
            .filter(move |param| param.availability.contains(level))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Function {
    /// Qualified with its namespace.
    pub name: Name,
    pub callable: Callable,
    pub doc: Option<Box<Doc>>,
    pub attributes: Vec<Attribute>,
    pub availability: Availability,
    pub name_span: Span,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Method {
    pub name: String,
    pub visibility: Visibility,
    pub is_static: bool,
    pub is_abstract: bool,
    pub is_final: bool,
    pub callable: Callable,
    pub doc: Option<Box<Doc>>,
    pub attributes: Vec<Attribute>,
    pub availability: Availability,
    pub name_span: Span,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Property {
    /// Without the `$`.
    pub name: String,
    pub visibility: Visibility,
    /// The visibility of writes, when asymmetric.
    pub set_visibility: Option<Visibility>,
    pub is_static: bool,
    pub is_readonly: bool,
    pub is_abstract: bool,
    pub ty: Option<Type>,
    pub doc_ty: Option<Type>,
    pub leveled: Option<LeveledType>,
    pub default: Option<String>,
    pub promoted: bool,
    /// Which hooks it has: `get`, `set`.
    pub hooks: Vec<String>,
    pub doc: Option<Box<Doc>>,
    pub attributes: Vec<Attribute>,
    pub availability: Availability,
    pub name_span: Span,
    pub span: Span,
}

impl Property {
    pub fn effective_type(&self, level: PhpVersion) -> Option<&Type> {
        self.doc_ty
            .as_ref()
            .or_else(|| self.leveled.as_ref().and_then(|leveled| leveled.at(level)))
            .or(self.ty.as_ref())
    }

    pub fn native_type(&self, level: PhpVersion) -> Option<&Type> {
        self.leveled
            .as_ref()
            .and_then(|leveled| leveled.at(level))
            .or(self.ty.as_ref())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassConst {
    pub name: String,
    pub visibility: Visibility,
    pub is_final: bool,
    /// An enum case.
    pub is_case: bool,
    pub ty: Option<Type>,
    pub doc_ty: Option<Type>,
    pub leveled: Option<LeveledType>,
    pub value: Option<String>,
    pub doc: Option<Box<Doc>>,
    pub attributes: Vec<Attribute>,
    pub availability: Availability,
    pub name_span: Span,
    pub span: Span,
}

impl ClassConst {
    pub fn effective_type(&self, level: PhpVersion) -> Option<&Type> {
        self.doc_ty
            .as_ref()
            .or_else(|| self.leveled.as_ref().and_then(|leveled| leveled.at(level)))
            .or(self.ty.as_ref())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Adaptation {
    /// `A::foo insteadof B, C`.
    InsteadOf {
        trait_name: Name,
        method: String,
        excluded: Vec<Name>,
    },
    /// `[A::]foo as [visibility] [alias]`.
    Alias {
        trait_name: Option<Name>,
        method: String,
        alias: Option<String>,
        visibility: Option<Visibility>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraitUse {
    /// With the arguments of `@use Trait<Foo>`.
    pub ty: Type,
    pub adaptations: Vec<Adaptation>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassDecl {
    pub name: Name,
    pub kind: ClassKind,
    pub is_abstract: bool,
    pub is_final: bool,
    pub is_readonly: bool,
    /// The parent of a class, the parents of an interface.
    pub extends: Vec<Type>,
    pub implements: Vec<Type>,
    pub trait_uses: Vec<TraitUse>,
    /// The type of an enum's cases' values.
    pub backing: Option<Type>,
    pub methods: Vec<Method>,
    pub properties: Vec<Property>,
    pub constants: Vec<ClassConst>,
    pub doc: Option<Box<Doc>>,
    pub attributes: Vec<Attribute>,
    pub availability: Availability,
    pub name_span: Span,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConstDecl {
    pub name: Name,
    pub value: Option<String>,
    pub ty: Option<Type>,
    pub doc: Option<Box<Doc>>,
    pub availability: Availability,
    pub name_span: Span,
    pub span: Span,
}

/// The declarations of one file.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSymbols {
    pub classes: Vec<ClassDecl>,
    pub functions: Vec<Function>,
    pub constants: Vec<ConstDecl>,
    /// A `define()` the file makes that the declarations above do not hold, because its name is
    /// only known when the code runs.
    pub dynamic_define: bool,
    /// Groups, datasets and test case bindings, when the file has any.
    pub tests: Option<Box<crate::test_facts::TestFacts>>,
}

impl FileSymbols {
    pub fn is_empty(&self) -> bool {
        self.classes.is_empty() && self.functions.is_empty() && self.constants.is_empty()
    }
}

impl ClassDecl {
    pub fn is_instantiable(&self) -> bool {
        self.kind == ClassKind::Class && !self.is_abstract
    }

    pub fn method(&self, name: &str) -> Option<&Method> {
        self.methods
            .iter()
            .find(|method| method.name.eq_ignore_ascii_case(name))
    }

    pub fn property(&self, name: &str) -> Option<&Property> {
        self.properties.iter().find(|property| property.name == name)
    }

    pub fn constant(&self, name: &str) -> Option<&ClassConst> {
        self.constants.iter().find(|constant| constant.name == name)
    }
}

/// What the index keeps of a class while the rest of its file stays on disk: enough to find it by
/// name, to list it, and to know which classes sit below it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClassSummary {
    pub name: Name,
    pub kind: ClassKind,
    pub is_abstract: bool,
    pub deprecated: bool,
    pub availability: Availability,
    /// The classes it extends, implements or uses, by name.
    pub parents: Vec<Name>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NameSummary {
    pub name: Name,
    pub deprecated: bool,
    pub availability: Availability,
}

/// The names of a file, in the order of its [`FileSymbols`], so a position here is a position there.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileSummary {
    pub classes: Vec<ClassSummary>,
    pub functions: Vec<NameSummary>,
    pub constants: Vec<NameSummary>,
    pub dynamic_define: bool,
    pub tests: Option<Box<crate::test_facts::TestFacts>>,
}

impl FileSummary {
    pub fn of(symbols: &FileSymbols) -> FileSummary {
        let deprecated = |doc: &Option<Box<Doc>>| doc.as_ref().is_some_and(|doc| doc.deprecated.is_some());
        FileSummary {
            dynamic_define: symbols.dynamic_define,
            tests: symbols.tests.clone(),
            classes: symbols
                .classes
                .iter()
                .map(|class| ClassSummary {
                    name: class.name.clone(),
                    kind: class.kind,
                    is_abstract: class.is_abstract,
                    deprecated: deprecated(&class.doc),
                    availability: class.availability,
                    parents: class
                        .extends
                        .iter()
                        .chain(&class.implements)
                        .chain(class.trait_uses.iter().map(|usage| &usage.ty))
                        .flat_map(|ty| ty.class_names().into_iter().map(str::to_string))
                        .collect(),
                })
                .collect(),
            functions: symbols
                .functions
                .iter()
                .map(|function| NameSummary {
                    name: function.name.clone(),
                    deprecated: deprecated(&function.doc),
                    availability: function.availability,
                })
                .collect(),
            constants: symbols
                .constants
                .iter()
                .map(|constant| NameSummary {
                    name: constant.name.clone(),
                    deprecated: deprecated(&constant.doc),
                    availability: constant.availability,
                })
                .collect(),
        }
    }
}
