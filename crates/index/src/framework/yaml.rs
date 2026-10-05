//! The YAML of Symfony's configuration, read into a tree that remembers where each scalar is.

use yaml_rust2::parser::{Event, MarkedEventReceiver, Parser};
use yaml_rust2::scanner::Marker;

use crate::model::Span;

#[derive(Clone, Debug, PartialEq)]
pub enum Node {
    Scalar { value: String, span: Span },
    Map(Vec<(Node, Node)>),
    Seq(Vec<Node>),
}

impl Node {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Node::Scalar { value, .. } => Some(value),
            _ => None,
        }
    }

    pub fn span(&self) -> Option<Span> {
        match self {
            Node::Scalar { span, .. } => Some(*span),
            _ => None,
        }
    }

    /// The entries of a mapping, the keys as strings.
    pub fn entries(&self) -> Vec<(&str, Span, &Node)> {
        match self {
            Node::Map(entries) => entries
                .iter()
                .filter_map(|(key, value)| match key {
                    Node::Scalar { value: name, span } => Some((name.as_str(), *span, value)),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    pub fn get(&self, key: &str) -> Option<&Node> {
        self.entries()
            .into_iter()
            .find(|(name, _, _)| *name == key)
            .map(|(_, _, value)| value)
    }

    pub fn items(&self) -> &[Node] {
        match self {
            Node::Seq(items) => items,
            _ => &[],
        }
    }
}

enum Open {
    Map(Vec<(Node, Node)>, Option<Node>),
    Seq(Vec<Node>),
}

struct Builder<'a> {
    text: &'a str,
    /// The byte offset each character starts at.
    offsets: Vec<usize>,
    stack: Vec<Open>,
    root: Option<Node>,
}

impl Builder<'_> {
    fn byte_of(&self, mark: &Marker) -> usize {
        self.offsets.get(mark.index()).copied().unwrap_or(self.text.len())
    }

    fn push(&mut self, node: Node) {
        match self.stack.last_mut() {
            Some(Open::Seq(items)) => items.push(node),
            Some(Open::Map(entries, key)) => match key.take() {
                Some(key) => entries.push((key, node)),
                None => *key = Some(node),
            },
            None => self.root = Some(node),
        }
    }
}

impl MarkedEventReceiver for Builder<'_> {
    fn on_event(&mut self, event: Event, mark: Marker) {
        match event {
            Event::Scalar(value, ..) => {
                let mut start = self.byte_of(&mark);
                if self.text[start..].starts_with(['\'', '"']) {
                    start += 1;
                }
                let span = Span {
                    start: start as u32,
                    end: (start + value.len()) as u32,
                };
                self.push(Node::Scalar { value, span });
            }
            Event::SequenceStart(..) => self.stack.push(Open::Seq(Vec::new())),
            Event::MappingStart(..) => self.stack.push(Open::Map(Vec::new(), None)),
            Event::SequenceEnd => {
                if let Some(Open::Seq(items)) = self.stack.pop() {
                    self.push(Node::Seq(items));
                }
            }
            Event::MappingEnd => {
                if let Some(Open::Map(entries, _)) = self.stack.pop() {
                    self.push(Node::Map(entries));
                }
            }
            Event::Alias(_) => self.push(Node::Scalar {
                value: String::new(),
                span: Span::default(),
            }),
            _ => {}
        }
    }
}

/// The first document of a text, or `None` when it is not valid YAML.
pub fn parse(text: &str) -> Option<Node> {
    let mut offsets: Vec<usize> = text.char_indices().map(|(offset, _)| offset).collect();
    offsets.push(text.len());
    let mut builder = Builder {
        text,
        offsets,
        stack: Vec::new(),
        root: None,
    };
    Parser::new_from_str(text).load(&mut builder, false).ok()?;
    builder.root
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# comment\nparameters:\n    locale: 'en'\n    items: [a, b]\nservices:\n    _defaults:\n        autowire: true\n    App\\:\n        resource: '../src/'\n    app.mailer:\n        class: App\\Mailer\n        arguments: ['%locale%', \"@logger\"]\n";

    #[test]
    fn reads_maps_sequences_and_where_a_scalar_is() {
        let root = parse(SAMPLE).expect("valid");
        let parameters = root.get("parameters").expect("a map");
        assert_eq!(parameters.get("locale").and_then(Node::as_str), Some("en"));
        assert_eq!(parameters.get("items").map(|items| items.items().len()), Some(2));
        let services = root.get("services").expect("services");
        let names: Vec<&str> = services.entries().iter().map(|(name, _, _)| *name).collect();
        assert_eq!(names, ["_defaults", "App\\", "app.mailer"]);
        let (_, span, _) = services.entries()[2];
        assert_eq!(&SAMPLE[span.start as usize..span.end as usize], "app.mailer");
        let value = parameters.get("locale").and_then(Node::span).expect("a span");
        assert_eq!(&SAMPLE[value.start as usize..value.end as usize], "en");
        let argument = services
            .get("app.mailer")
            .and_then(|node| node.get("arguments"))
            .expect("arguments");
        assert_eq!(argument.items()[1].as_str(), Some("@logger"));
    }

    #[test]
    fn what_is_not_yaml_is_nothing() {
        assert!(parse("a: [b").is_none());
    }
}
