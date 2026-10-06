//! A template written out as one PHP document, so the type layer can read it the way it reads a
//! file: the directives become the control structures the compiler makes of them, echoes become
//! `echo` statements, and the variables the template is given are declared at the top. Only the code
//! copied from the template maps back to it; what is written around it maps to nothing.

use php_syntax::TextRange;

use super::scan::{Directive, Node, Tag};

/// A run of the document copied from the template.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Segment {
    virt: u32,
    source: u32,
    len: u32,
}

/// The PHP document of a template, with the runs that map back to it.
#[derive(Clone, Debug, Default)]
pub struct Virtual {
    pub text: String,
    segments: Vec<Segment>,
}

impl Virtual {
    /// The offset in the document of an offset in the template, when it is in copied code. The end
    /// of a run counts, so a cursor right after a name finds it.
    pub fn to_virtual(&self, source: u32) -> Option<u32> {
        self.segments
            .iter()
            .find(|segment| segment.source <= source && source <= segment.source + segment.len)
            .map(|segment| segment.virt + (source - segment.source))
    }

    pub fn to_source(&self, virt: u32) -> Option<u32> {
        let at = self
            .segments
            .partition_point(|segment| segment.virt + segment.len < virt);
        self.segments[at..]
            .iter()
            .take_while(|segment| segment.virt <= virt)
            .find(|segment| virt <= segment.virt + segment.len)
            .map(|segment| segment.source + (virt - segment.virt))
    }

    /// A range of the document as a range of the template, when all of it was copied in one run.
    pub fn range_to_source(&self, range: TextRange) -> Option<TextRange> {
        let (start, end) = (u32::from(range.start()), u32::from(range.end()));
        let segment = self
            .segments
            .iter()
            .find(|segment| segment.virt <= start && end <= segment.virt + segment.len)?;
        Some(TextRange::new(
            (segment.source + (start - segment.virt)).into(),
            (segment.source + (end - segment.virt)).into(),
        ))
    }

    /// Whether a range of the document lies in code copied from the template.
    pub fn is_copied(&self, range: TextRange) -> bool {
        self.range_to_source(range).is_some()
    }
}

/// What a block directive opened, for the directive that closes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Open {
    pub name: String,
    closers: Vec<String>,
    pub at: u32,
    pub end: u32,
    /// A `@forelse` that reached its `@empty`.
    pub emptied: bool,
}

/// A directive that closes nothing, or a block that is never closed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Imbalance {
    Unclosed { name: String, at: u32, end: u32 },
    Unopened { name: String, at: u32, end: u32 },
}

pub struct Builder<'a> {
    source: &'a str,
    head: String,
    head_segments: Vec<Segment>,
    body: String,
    body_segments: Vec<Segment>,
    stack: Vec<Open>,
    imbalances: Vec<Imbalance>,
    /// Directives the compiler does not know that the template closes with `@end<name>`, which a
    /// project or a package registers as a conditional (`Blade::if`) or a block of its own.
    custom: std::collections::HashSet<String>,
    /// The directives the project registers itself, as written, which replace the compiler's own.
    registered: Vec<String>,
}

/// How a directive takes part in the blocks of a template.
enum Role {
    /// Opens a block that the named directives close.
    Opens(&'static [&'static str]),
    /// Opens a block of a directive the template closes with `@end<name>`.
    OpensCustom,
    /// Goes on with a block another directive opened.
    Continues(&'static [&'static str]),
    Closes,
    Alone,
}

const IF_FAMILY: &[&str] = &["endif"];

fn role(name: &str, args: Option<&str>) -> Role {
    match name {
        "if" => Role::Opens(IF_FAMILY),
        "unless" => Role::Opens(&["endunless"]),
        "isset" => Role::Opens(&["endisset"]),
        "empty" if args.is_some() => Role::Opens(&["endempty"]),
        "empty" => Role::Continues(&["forelse"]),
        "foreach" => Role::Opens(&["endforeach"]),
        "forelse" => Role::Opens(&["endforelse"]),
        "for" => Role::Opens(&["endfor"]),
        "while" => Role::Opens(&["endwhile"]),
        "switch" => Role::Opens(&["endswitch"]),
        "auth" => Role::Opens(&["endauth"]),
        "guest" => Role::Opens(&["endguest"]),
        "env" => Role::Opens(&["endenv"]),
        "production" => Role::Opens(&["endproduction"]),
        "can" => Role::Opens(&["endcan"]),
        "cannot" => Role::Opens(&["endcannot"]),
        "canany" => Role::Opens(&["endcanany"]),
        "once" => Role::Opens(&["endonce"]),
        "error" => Role::Opens(&["enderror"]),
        "session" => Role::Opens(&["endsession"]),
        "fragment" => Role::Opens(&["endfragment"]),
        "component" => Role::Opens(&["endcomponent"]),
        "componentfirst" => Role::Opens(&["endcomponentfirst"]),
        "push" if args.is_none_or(|args| top_level_commas(args) == 0) => Role::Opens(&["endpush"]),
        "prepend" if args.is_none_or(|args| top_level_commas(args) == 0) => Role::Opens(&["endprepend"]),
        "pushonce" => Role::Opens(&["endpushonce"]),
        "prependonce" => Role::Opens(&["endprependonce"]),
        "pushif" => Role::Opens(&["endpushif"]),
        "section" if args.is_some_and(|args| top_level_commas(args) == 0) => {
            Role::Opens(&["endsection", "stop", "show", "append", "overwrite"])
        }
        "slot" if args.is_some_and(|args| top_level_commas(args) == 0) => Role::Opens(&["endslot"]),
        "lang" if args.is_none() => Role::Opens(&["endlang"]),
        "elseif" | "else" => Role::Continues(&[]),
        "elseauth" => Role::Continues(&["auth", "guest"]),
        "elseguest" => Role::Continues(&["guest", "auth"]),
        "elsecan" | "elsecannot" | "elsecanany" => Role::Continues(&["can", "cannot", "canany"]),
        "elsepushif" | "elsepush" => Role::Continues(&["pushif"]),
        "case" | "default" => Role::Continues(&["switch"]),
        "endif" | "endunless" | "endisset" | "endempty" | "endforeach" | "endforelse" | "endfor" | "endwhile"
        | "endswitch" | "endauth" | "endguest" | "endenv" | "endproduction" | "endcan" | "endcannot" | "endcanany"
        | "endonce" | "enderror" | "endsession" | "endfragment" | "endcomponent" | "endcomponentfirst" | "endpush"
        | "endprepend" | "endpushonce" | "endprependonce" | "endpushif" | "endsection" | "stop" | "show" | "append"
        | "overwrite" | "endslot" | "endlang" => Role::Closes,
        _ => Role::Alone,
    }
}

/// Whether the compiler has a directive of this name, with or without arguments.
fn is_known(name: &str) -> bool {
    !matches!(role(name, None), Role::Alone)
        || !matches!(role(name, Some("")), Role::Alone)
        || EXPRESSION_DIRECTIVES.contains(&name)
        || matches!(
            name,
            "use" | "inject" | "unset" | "break" | "continue" | "parent" | "csrf"
        )
}

/// A block that is an `if` once compiled, which `@else` and `@elseif` can go on with.
fn is_conditional(name: &str) -> bool {
    !matches!(name, "foreach" | "forelse" | "for" | "while" | "switch")
}

/// The directives that take PHP between their parentheses and do nothing more the type layer needs
/// to see.
const EXPRESSION_DIRECTIVES: &[&str] = &[
    "include",
    "includeif",
    "includewhen",
    "includeunless",
    "includefirst",
    "each",
    "extends",
    "extendsfirst",
    "yield",
    "stack",
    "json",
    "js",
    "dd",
    "dump",
    "method",
    "vite",
    "class",
    "style",
    "checked",
    "selected",
    "disabled",
    "required",
    "readonly",
    "bool",
    "choice",
    "lang",
    "section",
    "slot",
    "props",
    "aware",
    "hassection",
    "sectionmissing",
    "livewire",
    "persist",
    "teleport",
    "entangle",
    "this",
];

/// The commas of an argument list outside brackets and quotes.
pub fn top_level_commas(args: &str) -> usize {
    let mut depth = 0i32;
    let mut quote: Option<u8> = None;
    let mut count = 0;
    let bytes = args.as_bytes();
    let mut position = 0;
    while position < bytes.len() {
        let byte = bytes[position];
        match quote {
            Some(mark) => {
                if byte == b'\\' {
                    position += 1;
                } else if byte == mark {
                    quote = None;
                }
            }
            None => match byte {
                b'\'' | b'"' => quote = Some(byte),
                b'(' | b'[' | b'{' => depth += 1,
                b')' | b']' | b'}' => depth -= 1,
                b',' if depth == 0 => count += 1,
                _ => {}
            },
        }
        position += 1;
    }
    count
}

impl<'a> Builder<'a> {
    pub fn new(source: &'a str, nodes: &[Node], registered: &[String], conditionals: &[String]) -> Builder<'a> {
        let names: std::collections::HashSet<String> = nodes
            .iter()
            .filter_map(|node| match node {
                Node::Directive(directive) => Some(directive.name.to_ascii_lowercase()),
                _ => None,
            })
            .collect();
        let mut custom: std::collections::HashSet<String> =
            conditionals.iter().map(|name| name.to_ascii_lowercase()).collect();
        custom.extend(
            names
                .iter()
                .filter_map(|name| name.strip_prefix("end"))
                .filter(|name| !matches!(*name, "" | "php" | "verbatim") && !is_known(name))
                .map(str::to_string),
        );
        Builder {
            source,
            head: String::from("<?php\n"),
            head_segments: Vec::new(),
            body: String::new(),
            body_segments: Vec::new(),
            stack: Vec::new(),
            imbalances: Vec::new(),
            custom,
            registered: registered.to_vec(),
        }
    }

    /// How a directive takes part in blocks, with the blocks the template makes of its own.
    fn role_of(&self, name: &str, args: Option<&str>) -> Role {
        let known = role(name, args);
        if !matches!(known, Role::Alone) {
            return known;
        }
        if self.custom.contains(name)
            || name
                .strip_prefix("unless")
                .is_some_and(|rest| self.custom.contains(rest))
        {
            return Role::OpensCustom;
        }
        if name.strip_prefix("else").is_some_and(|rest| self.custom.contains(rest)) {
            return Role::Continues(&[]);
        }
        if name.strip_prefix("end").is_some_and(|rest| self.custom.contains(rest)) {
            return Role::Closes;
        }
        Role::Alone
    }

    /// Writes text of its own into the declarations at the top.
    pub fn head(&mut self, text: &str) {
        self.head.push_str(text);
    }

    /// Copies a run of the template into the declarations at the top.
    pub fn head_copy(&mut self, start: u32, end: u32) {
        let Some(text) = self.source.get(start as usize..end as usize) else {
            return;
        };
        self.head_segments.push(Segment {
            virt: self.head.len() as u32,
            source: start,
            len: end - start,
        });
        self.head.push_str(text);
    }

    pub fn emit(&mut self, text: &str) {
        self.body.push_str(text);
    }

    pub fn copy(&mut self, start: u32, end: u32) {
        let Some(text) = self.source.get(start as usize..end as usize) else {
            return;
        };
        self.body_segments.push(Segment {
            virt: self.body.len() as u32,
            source: start,
            len: end - start,
        });
        self.body.push_str(text);
    }

    fn args_text(&self, directive: &Directive) -> Option<&'a str> {
        let (start, end) = directive.args?;
        self.source.get(start as usize..end as usize)
    }

    /// `(args)` as written.
    fn emit_args(&mut self, directive: &Directive) {
        if let Some((start, end)) = directive.args {
            self.emit("(");
            self.copy(start, end);
            self.emit(")");
        } else {
            self.emit("(null)");
        }
    }

    /// `[args]`, which is PHP for any list of arguments.
    fn emit_list(&mut self, directive: &Directive) {
        self.emit("[");
        if let Some((start, end)) = directive.args {
            self.copy(start, end);
        }
        self.emit("]");
    }

    pub fn node(&mut self, node: &Node) {
        match node {
            Node::Echo { start, end } => {
                self.emit("echo (");
                self.copy(*start, *end);
                self.emit(");\n");
            }
            Node::Php { start, end, echo } => {
                if *echo {
                    self.emit("echo (");
                    self.copy(*start, *end);
                    self.emit(");\n");
                } else {
                    self.copy(*start, *end);
                    self.emit("\n;\n");
                }
            }
            Node::PhpBlock { start, end, .. } => {
                self.copy(*start, *end);
                self.emit("\n;\n");
            }
            Node::Directive(directive) => self.directive(directive),
            Node::Tag(tag) => self.tag(tag),
        }
    }

    fn tag(&mut self, tag: &Tag) {
        for attribute in tag.attributes.iter().filter(|attribute| attribute.bound) {
            if let Some((start, end)) = attribute.value {
                self.emit("[");
                self.copy(start, end);
                self.emit("];\n");
            }
        }
    }

    fn directive(&mut self, directive: &Directive) {
        if self.registered.contains(&directive.name) {
            return;
        }
        let name = directive.name.to_ascii_lowercase();
        let args = self.args_text(directive);
        match self.role_of(&name, args) {
            Role::Opens(closers) => {
                self.stack.push(Open {
                    name: name.clone(),
                    closers: closers.iter().map(|closer| closer.to_string()).collect(),
                    at: directive.at,
                    end: directive.end,
                    emptied: false,
                });
                self.open_block(&name, directive);
            }
            Role::OpensCustom => {
                let base = name
                    .strip_prefix("unless")
                    .filter(|rest| self.custom.contains(*rest))
                    .unwrap_or(&name)
                    .to_string();
                self.stack.push(Open {
                    closers: vec![format!("end{base}")],
                    name: base,
                    at: directive.at,
                    end: directive.end,
                    emptied: false,
                });
                self.emit("if (");
                self.emit_list(directive);
                self.emit("):\n");
            }
            Role::Continues(openers) => {
                let fits = self.stack.last().is_some_and(|open| {
                    if openers.is_empty() {
                        is_conditional(&open.name)
                    } else {
                        openers.contains(&open.name.as_str())
                    }
                });
                if !fits {
                    self.imbalances.push(Imbalance::Unopened {
                        name: directive.name.clone(),
                        at: directive.at,
                        end: directive.end,
                    });
                    return;
                }
                self.continue_block(&name, directive);
            }
            Role::Closes => {
                let fits = self.stack.last().is_some_and(|open| open.closers.contains(&name));
                if !fits {
                    self.imbalances.push(Imbalance::Unopened {
                        name: directive.name.clone(),
                        at: directive.at,
                        end: directive.end,
                    });
                    return;
                }
                let open = self.stack.pop().expect("a block is open");
                self.close_block(&open);
            }
            Role::Alone => self.single(&name, directive),
        }
    }

    fn open_block(&mut self, name: &str, directive: &Directive) {
        match name {
            "if" => {
                self.emit("if ");
                self.emit_args(directive);
                self.emit(":\n");
            }
            "unless" => {
                self.emit("if (!");
                self.emit_args(directive);
                self.emit("):\n");
            }
            "isset" => {
                self.emit("if (isset");
                self.emit_args(directive);
                self.emit("):\n");
            }
            "empty" => {
                self.emit("if (empty");
                self.emit_args(directive);
                self.emit("):\n");
            }
            "foreach" | "forelse" => {
                self.emit("foreach ");
                self.emit_args(directive);
                self.emit(":\n");
                self.emit(LOOP);
            }
            "for" => {
                self.emit("for ");
                self.emit_args(directive);
                self.emit(":\n");
            }
            "while" => {
                self.emit("while ");
                self.emit_args(directive);
                self.emit(":\n");
            }
            "switch" => {
                self.emit("switch ");
                self.emit_args(directive);
                self.emit(":\n");
            }
            "error" => {
                self.emit("if (");
                self.emit_list(directive);
                self.emit("):\n$message = '';\n");
            }
            "session" => {
                self.emit("if (");
                self.emit_list(directive);
                self.emit("):\n/** @var mixed $value */\n$value = null;\n");
            }
            _ => {
                self.emit("if (");
                self.emit_list(directive);
                self.emit("):\n");
            }
        }
    }

    fn continue_block(&mut self, name: &str, directive: &Directive) {
        match name {
            "elseif" => {
                self.emit("elseif ");
                self.emit_args(directive);
                self.emit(":\n");
            }
            "else" | "elsepush" => self.emit("else:\n"),
            "empty" => {
                if let Some(open) = self.stack.last_mut() {
                    open.emptied = true;
                }
                self.emit("endforeach;\nif (true):\n");
            }
            "case" => {
                self.emit("case ");
                self.emit_args(directive);
                self.emit(":\n");
            }
            "default" => self.emit("default:\n"),
            _ => {
                self.emit("elseif (");
                self.emit_list(directive);
                self.emit("):\n");
            }
        }
    }

    fn close_block(&mut self, open: &Open) {
        match open.name.as_str() {
            "foreach" => self.emit("endforeach;\n"),
            "forelse" if !open.emptied => self.emit("endforeach;\n"),
            "for" => self.emit("endfor;\n"),
            "while" => self.emit("endwhile;\n"),
            "switch" => self.emit("endswitch;\n"),
            _ => self.emit("endif;\n"),
        }
    }

    fn single(&mut self, name: &str, directive: &Directive) {
        match name {
            "php" => {
                if let Some((start, end)) = directive.args {
                    self.copy(start, end);
                    self.emit(";\n");
                }
            }
            "use" => self.use_statement(directive),
            "inject" => self.inject(directive),
            "unset" => {
                self.emit("unset");
                self.emit_args(directive);
                self.emit(";\n");
            }
            "break" | "continue" => {
                if directive.args.is_some() {
                    self.emit("if ");
                    self.emit_args(directive);
                    self.emit(" ");
                }
                self.emit(name);
                self.emit(";\n");
            }
            _ if directive.args.is_some() && EXPRESSION_DIRECTIVES.contains(&name) => {
                self.emit_list(directive);
                self.emit(";\n");
            }
            _ => {}
        }
    }

    /// `@use('App\Models\User', 'Author')` is a `use` statement, which only works at the top.
    fn use_statement(&mut self, directive: &Directive) {
        let Some((start, end)) = directive.args else {
            return;
        };
        let strings = string_contents(self.source, start, end);
        let Some(&(class_start, class_end)) = strings.first() else {
            return;
        };
        self.head("use ");
        self.head_copy(class_start, class_end);
        if let Some(&(alias_start, alias_end)) = strings.get(1) {
            self.head(" as ");
            self.head_copy(alias_start, alias_end);
        }
        self.head(";\n");
    }

    /// `@inject('metrics', 'App\Services\Metrics')` gives the template a variable of that class.
    fn inject(&mut self, directive: &Directive) {
        let Some((start, end)) = directive.args else {
            return;
        };
        let strings = string_contents(self.source, start, end);
        let (Some(&(name_start, name_end)), Some(&(class_start, class_end))) = (strings.first(), strings.get(1)) else {
            return;
        };
        let name = self.source[name_start as usize..name_end as usize].to_string();
        if !name.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_') {
            return;
        }
        self.emit("$");
        self.emit(&name);
        self.emit(" = new \\");
        self.copy(class_start, class_end);
        self.emit("();\n");
    }

    /// The document, and the directives that close nothing or open what is never closed.
    pub fn finish(mut self) -> (Virtual, Vec<Imbalance>) {
        while let Some(open) = self.stack.pop() {
            self.close_block(&open);
            self.imbalances.push(Imbalance::Unclosed {
                name: open.name.clone(),
                at: open.at,
                end: open.end,
            });
        }
        let shift = self.head.len() as u32;
        let mut segments = self.head_segments;
        segments.extend(self.body_segments.into_iter().map(|segment| Segment {
            virt: segment.virt + shift,
            ..segment
        }));
        let mut text = self.head;
        text.push_str(&self.body);
        (Virtual { text, segments }, self.imbalances)
    }
}

/// What a `foreach` gives its body besides the variables it names.
const LOOP: &str = "$loop = new \\stdClass();\n";

/// The text inside each quoted string of an argument list.
fn string_contents(source: &str, start: u32, end: u32) -> Vec<(u32, u32)> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut position = start as usize;
    while position < end as usize {
        let byte = bytes[position];
        if byte == b'\'' || byte == b'"' {
            let inner = position + 1;
            let close = source[inner..end as usize]
                .bytes()
                .position(|found| found == byte)
                .map_or(end as usize, |found| inner + found);
            out.push((inner as u32, close as u32));
            position = close + 1;
        } else {
            position += 1;
        }
    }
    out
}
