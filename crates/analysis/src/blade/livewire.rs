//! The `wire:` attributes of a Livewire component's view: `wire:model="title"` and
//! `wire:model="form.title"` name a public property of the component (and of the form object a
//! property holds), `wire:click="save"` and the other actions name a public method of it. The view
//! belongs to the component that renders it, by its `render()` or by the `livewire.{name}`
//! convention, and only when exactly one does.

use std::path::Path;

use php_index::framework::livewire::Livewire;
use php_index::framework::views::Views;
use php_index::model::Visibility;
use php_index::{Index, Name, Type};
use php_syntax::TextRange;

use crate::ast::range_of;
use crate::target::Target;

/// The directives whose value calls a method of the component.
const ACTIONS: &[&str] = &[
    "click",
    "submit",
    "keydown",
    "keyup",
    "keypress",
    "change",
    "input",
    "blur",
    "focus",
    "init",
    "poll",
    "dblclick",
    "mouseenter",
    "mouseleave",
    "contextmenu",
];

/// The methods every component has for Livewire itself, which no template calls.
const LIFECYCLE: &[&str] = &[
    "mount",
    "render",
    "boot",
    "booted",
    "hydrate",
    "dehydrate",
    "rendering",
    "rendered",
    "exception",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Member {
    Property,
    Method,
}

/// A name a `wire:` attribute writes, with the class it is a member of.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WireRef {
    pub member: Member,
    pub owner: Name,
    pub name: String,
    pub range: TextRange,
}

impl WireRef {
    pub fn target(&self) -> Target {
        let receiver = Type::class(self.owner.clone());
        let name = self.name.clone();
        match self.member {
            Member::Property => Target::Property { receiver, name },
            Member::Method => Target::Method { receiver, name },
        }
    }
}

/// The Livewire component whose view a template is.
pub fn component_of(index: &Index, path: &Path) -> Option<Name> {
    let livewire = index.section::<Livewire>();
    if livewire.components.is_empty() {
        return None;
    }
    let views = index.section::<Views>();
    let view = views.views.iter().find(|view| view.path == path)?;
    livewire.of_view(&view.name).map(|component| component.class.clone())
}

fn is_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_'
}

/// The names the `wire:` attributes of a template write, a name being typed included.
pub fn wire_refs(index: &Index, path: Option<&Path>, text: &str) -> Vec<WireRef> {
    let Some(component) = path.and_then(|path| component_of(index, path)) else {
        return Vec::new();
    };
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    for (at, _) in text.match_indices("wire:") {
        if at > 0 && !bytes[at - 1].is_ascii_whitespace() {
            continue;
        }
        let rest = &text[at + "wire:".len()..];
        let directive_length = rest
            .bytes()
            .take_while(|byte| is_name_byte(*byte) || matches!(byte, b'.' | b'-'))
            .count();
        let directive = rest[..directive_length].split('.').next().unwrap_or_default();
        let after = &rest[directive_length..];
        let Some(quote) = after.strip_prefix('=').and_then(|value| value.bytes().next()) else {
            continue;
        };
        if !matches!(quote, b'"' | b'\'') {
            continue;
        }
        let value_start = at + "wire:".len() + directive_length + 2;
        let Some(value_length) = text[value_start..].find(quote as char) else {
            continue;
        };
        let value = &text[value_start..value_start + value_length];
        if directive == "model" {
            model_refs(index, &component, value, value_start as u32, &mut out);
        } else if ACTIONS.contains(&directive) {
            let length = value.bytes().take_while(|byte| is_name_byte(*byte)).count();
            let name = &value[..length];
            if value.starts_with(|c: char| c.is_ascii_digit()) {
                continue;
            }
            out.push(WireRef {
                member: Member::Method,
                owner: component.clone(),
                name: name.to_string(),
                range: range_of(value_start as u32, (value_start + length) as u32),
            });
        }
    }
    out
}

/// `form.title`: each segment a property of the class the one before it holds.
fn model_refs(index: &Index, component: &str, value: &str, start: u32, out: &mut Vec<WireRef>) {
    let mut owner = component.to_string();
    let mut at = start;
    for segment in value.split('.') {
        if !segment.bytes().all(is_name_byte) {
            return;
        }
        out.push(WireRef {
            member: Member::Property,
            owner: owner.clone(),
            name: segment.to_string(),
            range: range_of(at, at + segment.len() as u32),
        });
        at += segment.len() as u32 + 1;
        let next = index
            .find_property(&Type::class(owner.clone()), segment)
            .and_then(|found| found.member.doc_ty.clone().or_else(|| found.member.ty.clone()));
        match next {
            Some(Type::Class { name, .. }) => owner = name,
            _ => return,
        }
    }
}

/// The `wire:` name under an offset.
pub fn wire_at(index: &Index, path: Option<&Path>, text: &str, offset: u32) -> Option<WireRef> {
    wire_refs(index, path, text)
        .into_iter()
        .find(|wire| u32::from(wire.range.start()) <= offset && offset <= u32::from(wire.range.end()))
}

/// What a `wire:` attribute can name there: the public properties or the public methods of its class.
pub fn candidates(index: &Index, wire: &WireRef) -> Vec<(String, Option<String>)> {
    let Some(class) = index.class(&wire.owner) else {
        return Vec::new();
    };
    match wire.member {
        Member::Property => class
            .decl
            .properties
            .iter()
            .filter(|property| property.visibility == Visibility::Public && !property.is_static)
            .map(|property| {
                let detail = property
                    .doc_ty
                    .as_ref()
                    .or(property.ty.as_ref())
                    .map(|ty| ty.display(true));
                (property.name.clone(), detail)
            })
            .collect(),
        Member::Method => class
            .decl
            .methods
            .iter()
            .filter(|method| method.visibility == Visibility::Public && !method.is_static)
            .filter(|method| !method.name.starts_with("__") && !is_lifecycle(&method.name))
            .map(|method| (method.name.clone(), None))
            .collect(),
    }
}

fn is_lifecycle(name: &str) -> bool {
    LIFECYCLE.contains(&name)
        || ["updated", "updating", "hydrate", "dehydrate"]
            .iter()
            .any(|prefix| name.starts_with(prefix) && name.len() > prefix.len())
}
