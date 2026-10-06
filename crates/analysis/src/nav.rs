//! Hover, go to definition, go to type definition and go to implementation, all over the targets
//! that [`Analyzer::targets_at`] finds.

use std::path::PathBuf;

use php_index::{Class, Doc, FileEntry, Name, Span, Type};
use php_syntax::SyntaxKind::*;
use php_syntax::TextRange;

use crate::infer::Analyzer;
use crate::render;
use crate::target::{Callee, Target};

/// A place in a file. `path` is `None` for the file the question was asked about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub path: Option<PathBuf>,
    pub span: Span,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HoverResult {
    pub markdown: String,
    pub range: TextRange,
}

/// A declaration described for a person: what it is called, how it is written and what its doc says.
pub struct Description {
    pub title: String,
    pub signature: String,
    pub doc: Option<Doc>,
    pub place: Option<Place>,
}

fn place_of(file: &FileEntry, span: Span) -> Place {
    Place {
        path: Some(file.path.clone()),
        span,
    }
}

fn class_place(class: Class<'_>) -> Place {
    place_of(class.file, class.decl.name_span)
}

impl Analyzer<'_> {
    /// Everything the targets under an offset stand for, ready to show.
    pub fn describe(&self, target: &Target) -> Vec<Description> {
        let level = self.level();
        match target {
            Target::Class(name) => self
                .index
                .class(name)
                .map(|class| Description {
                    title: class.decl.name.clone(),
                    signature: render::class_signature(class.decl),
                    doc: class.decl.doc.as_deref().cloned(),
                    place: Some(class_place(class)),
                })
                .into_iter()
                .collect(),
            Target::Function(name) => self
                .index
                .function(name)
                .map(|function| Description {
                    title: function.decl.name.clone(),
                    signature: render::function_signature(
                        crate::short(&function.decl.name),
                        &function.decl.callable,
                        level,
                    ),
                    doc: function.decl.doc.as_deref().cloned(),
                    place: Some(place_of(function.file, function.decl.name_span)),
                })
                .into_iter()
                .collect(),
            Target::Constant(name) => self
                .index
                .constant(name)
                .map(|constant| {
                    let mut signature = format!("const {}", crate::short(&constant.decl.name));
                    if let Some(value) = &constant.decl.value {
                        signature.push_str(&format!(" = {value}"));
                    }
                    Description {
                        title: constant.decl.name.clone(),
                        signature,
                        doc: constant.decl.doc.as_deref().cloned(),
                        place: Some(place_of(constant.file, constant.decl.name_span)),
                    }
                })
                .into_iter()
                .collect(),
            Target::Method { receiver, name }
                if crate::pest::is_expectation(receiver)
                    && receiver
                        .members()
                        .iter()
                        .all(|member| self.index.find_method(member, name).is_none()) =>
            {
                self.custom_expectation(name)
                    .map(|expectation| Description {
                        title: format!("expect()->{name}()"),
                        signature: format!("expect()->extend('{name}', ...)"),
                        doc: None,
                        place: Some(Place {
                            path: expectation.path,
                            span: expectation.span,
                        }),
                    })
                    .into_iter()
                    .collect()
            }
            Target::Method { receiver, name } => receiver
                .members()
                .iter()
                .filter_map(|member| {
                    let found = self.index.find_method(member, name)?;
                    let doc = self.method_doc(&found.class.decl.name, found.member.doc.as_deref(), name);
                    Some(Description {
                        title: format!("{}::{}", found.class.decl.name, found.member.name),
                        signature: render::method_signature(&found.member, level),
                        doc,
                        place: Some(place_of(found.class.file, found.member.name_span)),
                    })
                })
                .collect(),
            Target::Property { receiver, name } => receiver
                .members()
                .iter()
                .filter_map(|member| {
                    let found = self.index.find_property(member, name)?;
                    Some(Description {
                        title: format!("{}::${}", found.class.decl.name, found.member.name),
                        signature: render::property_signature(&found.member, level),
                        doc: found.member.doc.as_deref().cloned(),
                        place: Some(place_of(found.class.file, found.member.name_span)),
                    })
                })
                .collect(),
            Target::ClassConst { receiver, name } => receiver
                .members()
                .iter()
                .filter_map(|member| {
                    let found = self.index.find_constant(member, name)?;
                    Some(Description {
                        title: format!("{}::{}", found.class.decl.name, found.member.name),
                        signature: render::constant_signature(&found.member, level),
                        doc: found.member.doc.as_deref().cloned(),
                        place: Some(place_of(found.class.file, found.member.name_span)),
                    })
                })
                .collect(),
            Target::Parameter { callee, name } => self.describe_parameter(callee, name),
            Target::Dataset(name) => crate::pest::dataset_declarations(self.index, name)
                .into_iter()
                .map(|declaration| Description {
                    title: format!("dataset '{name}'"),
                    signature: format!("dataset('{name}', ...)"),
                    doc: None,
                    place: Some(Place {
                        path: Some(declaration.path),
                        span: declaration.name_span,
                    }),
                })
                .collect(),
            Target::Key { kind, name, scope } => {
                crate::frameworks::keys::describe(self.index, *kind, name, scope.as_deref())
            }
            Target::TypeAlias { class, name } => self
                .index
                .type_alias(class, name, 0)
                .map(|(declaring, alias)| Description {
                    title: format!("{}::{name}", declaring.decl.name),
                    signature: format!("type {name} = {}", alias.ty.display(false)),
                    doc: None,
                    place: Some(place_of(declaring.file, alias.name_span)),
                })
                .into_iter()
                .collect(),
            Target::Variable { name, ty } => vec![Description {
                title: format!("${name}"),
                signature: format!("{} ${name}", ty.display(true)),
                doc: None,
                place: None,
            }],
        }
    }

    fn parameter_type(&self, callee: &Callee, name: &str) -> Option<Type> {
        let level = self.level();
        let callable = match callee {
            Callee::Function(function) => self.index.function(function)?.decl.callable.clone(),
            Callee::Method { class, name: method } => self.index.class(class)?.decl.method(method)?.callable.clone(),
        };
        let param = callable.params_at(level).find(|param| param.name == name)?;
        param.effective_type(level).cloned()
    }

    fn describe_parameter(&self, callee: &Callee, name: &str) -> Vec<Description> {
        let level = self.level();
        let (file, callable) = match callee {
            Callee::Function(function) => {
                let Some(found) = self.index.function(function) else {
                    return Vec::new();
                };
                (found.file, &found.decl.callable)
            }
            Callee::Method { class, name: method } => {
                let Some(class) = self.index.class(class) else {
                    return Vec::new();
                };
                let Some(found) = class.decl.method(method) else {
                    return Vec::new();
                };
                (class.file, &found.callable)
            }
        };
        let Some(param) = callable.params_at(level).find(|param| param.name == name) else {
            return Vec::new();
        };
        let doc = (!param.description.is_empty()).then(|| Doc {
            summary: param.description.clone(),
            ..Doc::default()
        });
        vec![Description {
            title: format!("${name}"),
            signature: render::param_text(param, level),
            doc,
            place: Some(place_of(file, param.span)),
        }]
    }

    /// The doc of a method with the same name further up, for a method that has none of its own.
    /// A method's doc with what it leaves to the method it overrides filled in: all of it when it
    /// says nothing of its own, and in place of `{@inheritdoc}` or with `@inheritDoc`.
    fn method_doc(&self, owner: &Name, own: Option<&Doc>, name: &str) -> Option<Doc> {
        let Some(own) = own else {
            return self.inherited_method_doc(owner, name);
        };
        if !inherits(own) {
            return Some(own.clone());
        }
        let parent = self.inherited_method_doc(owner, name).unwrap_or_default();
        Some(merge_inherited(own, &parent))
    }

    /// The first doc above `owner` that says something of its own, through parents and interfaces.
    fn inherited_method_doc(&self, owner: &Name, name: &str) -> Option<Doc> {
        self.index
            .ancestors(&Type::class(owner.clone()))
            .into_iter()
            .filter(|ancestor| ancestor.class.decl.name != *owner)
            .filter_map(|ancestor| ancestor.class.decl.method(name)?.doc.as_deref().cloned())
            .find(|doc| !inherits(doc))
    }

    pub fn hover(&self, offset: u32) -> Option<HoverResult> {
        let found = self.targets_at(offset);
        let range = found.first()?.range;
        let mut sections = Vec::new();
        for item in &found {
            for description in self.describe(&item.target) {
                sections.push(hover_markdown(&description));
            }
        }
        sections.dedup();
        if sections.is_empty() {
            return None;
        }
        Some(HoverResult {
            markdown: sections.join("\n\n---\n\n"),
            range,
        })
    }

    pub fn definitions(&self, offset: u32) -> Vec<Place> {
        let mut places = Vec::new();
        for found in self.targets_at(offset) {
            if let Target::Variable { name, .. } = &found.target {
                if let Some(place) = self.variable_declaration(name, offset) {
                    places.push(place);
                }
                continue;
            }
            for description in self.describe(&found.target) {
                places.extend(description.place);
            }
        }
        places.dedup();
        places
    }

    /// The first place a variable shows up in its scope, which is where it is declared or first set.
    fn variable_declaration(&self, name: &str, offset: u32) -> Option<Place> {
        let scope = self.scope_at(offset);
        let wanted = format!("${name}");
        scope
            .descendants_with_tokens()
            .filter_map(|element| element.into_token())
            .find(|token| token.kind() == VARIABLE && token.text() == wanted)
            .map(|token| Place {
                path: None,
                span: Span {
                    start: u32::from(token.text_range().start()),
                    end: u32::from(token.text_range().end()),
                },
            })
    }

    pub fn type_definitions(&self, offset: u32) -> Vec<Place> {
        let mut places = Vec::new();
        for found in self.targets_at(offset) {
            let ty = match &found.target {
                Target::Class(_) => {
                    for description in self.describe(&found.target) {
                        places.extend(description.place);
                    }
                    continue;
                }
                Target::Variable { ty, .. } => ty.clone(),
                Target::Property { receiver, name } => self.property_on(receiver, name),
                Target::Method { receiver, name } => {
                    let level = self.level();
                    Type::union(receiver.members().iter().filter_map(|member| {
                        let found = self.index.find_method(member, name)?;
                        let ret = found.member.callable.effective_return(level)?;
                        Some(self.bind_static(&found.resolve(ret), member))
                    }))
                }
                Target::ClassConst { receiver, name } => {
                    let level = self.level();
                    Type::union(receiver.members().iter().filter_map(|member| {
                        let found = self.index.find_constant(member, name)?;
                        if found.member.is_case {
                            return Some(member.clone());
                        }
                        found.member.effective_type(level).map(|ty| found.resolve(ty))
                    }))
                }
                Target::Function(name) => {
                    let level = self.level();
                    match self
                        .index
                        .function(name)
                        .and_then(|function| function.decl.callable.effective_return(level).cloned())
                    {
                        Some(ty) => ty,
                        None => continue,
                    }
                }
                Target::Constant(_) | Target::Dataset(_) | Target::Key { .. } => continue,
                Target::TypeAlias { class, name } => match self.index.type_alias(class, name, 0) {
                    Some((_, alias)) => alias.ty,
                    None => continue,
                },
                Target::Parameter { callee, name } => match self.parameter_type(callee, name) {
                    Some(ty) => ty,
                    None => continue,
                },
            };
            let receiver = self.receiver_type(&ty);
            let mut names: Vec<String> = receiver.class_names().into_iter().map(str::to_string).collect();
            names.extend(class_names_in_args(&receiver));
            for name in names {
                if let Some(class) = self.index.class(&name) {
                    places.push(class_place(class));
                }
            }
        }
        places.dedup();
        places
    }

    pub fn implementations(&self, offset: u32) -> Vec<Place> {
        let mut places = Vec::new();
        for found in self.targets_at(offset) {
            match &found.target {
                Target::Class(name) => {
                    places.extend(self.index.all_subtypes(name).into_iter().map(class_place));
                    // A message's handlers are what it is carried out by.
                    let handlers = self
                        .index
                        .section::<php_index::framework::symfony::messenger::MessageHandlers>();
                    places.extend(handlers.of(name).iter().map(|handler| Place {
                        path: Some(handler.path.clone()),
                        span: handler.name_span,
                    }));
                }
                Target::Method { receiver, name } => {
                    for member in receiver.members() {
                        let Some(declaring) = self.index.find_method(member, name) else {
                            continue;
                        };
                        for subtype in self.index.all_subtypes(&declaring.class.decl.name) {
                            if let Some(method) = subtype.decl.method(name) {
                                places.push(place_of(subtype.file, method.name_span));
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        places.dedup();
        places
    }
}

fn class_names_in_args(ty: &Type) -> Vec<String> {
    let mut out = Vec::new();
    for member in ty.members() {
        if let Type::Class { args, .. } = member {
            for arg in args {
                out.extend(arg.class_names().into_iter().map(str::to_string));
            }
        }
    }
    out
}

const INHERIT_MARKER: &str = "{@inheritdoc}";

/// Whether a doc hands its text to the method it overrides: it says so, or it says nothing.
fn inherits(doc: &Doc) -> bool {
    let marked = |text: &str| text.to_ascii_lowercase().contains(INHERIT_MARKER);
    marked(&doc.summary)
        || marked(&doc.description)
        || doc.tags.iter().any(|tag| tag.name.eq_ignore_ascii_case("inheritdoc"))
        || (doc.summary.is_empty() && doc.description.is_empty() && doc.params.is_empty() && doc.ret.is_none())
}

/// `text` with `{@inheritdoc}`, in any case, replaced by `inherited`.
fn replace_marker(text: &str, inherited: &str) -> Option<String> {
    let at = text.to_ascii_lowercase().find(INHERIT_MARKER)?;
    Some(format!(
        "{}{inherited}{}",
        &text[..at],
        &text[at + INHERIT_MARKER.len()..]
    ))
}

fn join_prose(first: &str, second: &str) -> String {
    [first.trim(), second.trim()]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The parent's doc under what the method's own doc says: its own words, parameters, return and
/// tags win, and the parent fills in the rest.
fn merge_inherited(own: &Doc, parent: &Doc) -> Doc {
    let mut doc = parent.clone();
    let summary_marked = replace_marker(&own.summary, &parent.summary);
    let description_marked = replace_marker(&own.description, &parent.description);
    doc.summary = match &summary_marked {
        Some(summary) if !summary.trim().is_empty() => summary.trim().to_string(),
        Some(_) => parent.summary.clone(),
        None if own.summary.is_empty() => parent.summary.clone(),
        None => own.summary.clone(),
    };
    doc.description = match (&summary_marked, description_marked) {
        (_, Some(description)) => description.trim().to_string(),
        (Some(_), None) => join_prose(&parent.description, &own.description),
        (None, None) if own.description.is_empty() => parent.description.clone(),
        (None, None) => own.description.clone(),
    };
    for param in &own.params {
        match doc.params.iter_mut().find(|known| known.name == param.name) {
            Some(known) => *known = param.clone(),
            None => doc.params.push(param.clone()),
        }
    }
    if own.ret.is_some() {
        doc.ret = own.ret.clone();
    }
    if !own.throws.is_empty() {
        doc.throws = own.throws.clone();
    }
    if !own.see.is_empty() {
        doc.see = own.see.clone();
    }
    doc.deprecated = own.deprecated.clone();
    doc.since = own.since;
    doc.removed = own.removed;
    doc.tags
        .retain(|tag| !own.tags.iter().any(|mine| mine.name.eq_ignore_ascii_case(&tag.name)));
    doc.tags.extend(
        own.tags
            .iter()
            .filter(|tag| !tag.name.eq_ignore_ascii_case("inheritdoc"))
            .cloned(),
    );
    doc
}

/// A title line, the signature in a `php` block and the doc.
pub fn hover_markdown(description: &Description) -> String {
    let mut out = format!("**{}**\n\n```php\n{}\n```", description.title, description.signature);
    if let Some(doc) = &description.doc {
        let text = render::doc_markdown(doc);
        if !text.is_empty() {
            out.push_str("\n\n");
            out.push_str(&text);
        }
    }
    out
}
