//! Symfony Messenger: the handlers of each message class. A handler is a class marked
//! `#[AsMessageHandler]` (its `__invoke`, or the method the attribute names), a method marked so, or
//! a class that implements `MessageHandlerInterface`; the message is the attribute's `handles`, else
//! the type of the method's first parameter.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::framework::Section;
use crate::framework::source::class_in_attribute;
use crate::index::{Class, Index, Origin};
use crate::model::{Attribute, Method, Span};
use crate::types::Name;

const AS_MESSAGE_HANDLER: &str = "Symfony\\Component\\Messenger\\Attribute\\AsMessageHandler";
const HANDLER_INTERFACE: &str = "Symfony\\Component\\Messenger\\Handler\\MessageHandlerInterface";

#[derive(Clone, Debug, PartialEq)]
pub struct Handler {
    pub class: Name,
    pub method: String,
    pub path: PathBuf,
    pub name_span: Span,
}

#[derive(Default)]
pub struct MessageHandlers {
    by_message: HashMap<String, Vec<Handler>>,
}

impl MessageHandlers {
    pub fn of(&self, message: &str) -> &[Handler] {
        self.by_message
            .get(&message.trim_start_matches('\\').to_ascii_lowercase())
            .map_or(&[], Vec::as_slice)
    }
}

fn handler_attribute(attributes: &[Attribute]) -> Option<&Attribute> {
    attributes
        .iter()
        .find(|attribute| attribute.name.eq_ignore_ascii_case(AS_MESSAGE_HANDLER))
}

fn argument<'a>(attribute: &'a Attribute, name: &str) -> Option<&'a str> {
    attribute
        .args
        .iter()
        .find(|arg| arg.name.as_deref() == Some(name))
        .map(|arg| arg.value.as_str())
}

impl Section for MessageHandlers {
    fn build(index: &Index) -> Self {
        let mut found = MessageHandlers::default();
        if !index.frameworks().symfony
            || index.class(AS_MESSAGE_HANDLER).is_none() && index.class(HANDLER_INTERFACE).is_none()
        {
            return found;
        }
        let mut handlers: Vec<(Name, Handler)> = Vec::new();
        for name in index.class_names() {
            if name.file.origin != Origin::Project {
                continue;
            }
            let Some(class) = name.load() else {
                continue;
            };
            let decl = class.decl;
            let marked = handler_attribute(&decl.attributes);
            let implements = index.is_subclass_of(&decl.name, HANDLER_INTERFACE);
            if marked.is_some() || implements {
                let method_name = marked
                    .and_then(|attribute| argument(attribute, "method"))
                    .map(|value| value.trim().trim_matches(['\'', '"']).to_string())
                    .unwrap_or_else(|| "__invoke".to_string());
                if let Some(method) = decl.method(&method_name) {
                    let handles = marked.and_then(|attribute| argument(attribute, "handles"));
                    if let Some(message) = message_of(index, class, method, handles) {
                        handlers.push((message, handler(class, method)));
                    }
                }
            }
            for method in &decl.methods {
                let Some(attribute) = handler_attribute(&method.attributes) else {
                    continue;
                };
                if let Some(message) = message_of(index, class, method, argument(attribute, "handles")) {
                    handlers.push((message, handler(class, method)));
                }
            }
        }
        for (message, handler) in handlers {
            found
                .by_message
                .entry(message.to_ascii_lowercase())
                .or_default()
                .push(handler);
        }
        found
    }

    fn depends_on(root: &Path, path: &Path) -> bool {
        crate::framework::is_project_php(root, path)
    }
}

fn handler(class: Class<'_>, method: &Method) -> Handler {
    Handler {
        class: class.decl.name.clone(),
        method: method.name.clone(),
        path: class.file.path.clone(),
        name_span: method.name_span,
    }
}

/// The message a handler method takes: what `handles` names, else its first parameter's class.
fn message_of(index: &Index, class: Class<'_>, method: &Method, handles: Option<&str>) -> Option<Name> {
    if let Some(handles) = handles {
        return class_in_attribute(index, class, handles);
    }
    let param = method.callable.params.first()?;
    let ty = param.doc_ty.as_ref().or(param.ty.as_ref())?;
    let names = ty.class_names();
    // A union handles each of its classes; one is enough to lead somewhere.
    names.first().map(|name| name.to_string())
}
