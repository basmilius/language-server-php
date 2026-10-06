//! Strings that hold a class name: `'App\Models\User'`, and a method of one written as
//! `'App\Http\Controllers\Home::show'` or `'App\Http\Controllers\Home@show'`. Only a qualified name
//! of a class the index knows counts, so a word that happens to be a short class name stays a word.

use php_index::{Index, Name, Type};
use php_syntax::SyntaxKind::*;
use php_syntax::{SyntaxNode, TextRange};

use crate::ast::range_of;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassString {
    /// The class as the index declares it.
    pub class: Name,
    /// The last segment of the name, which a rename replaces.
    pub short_range: TextRange,
    /// The whole name, a leading backslash included, which a move replaces.
    pub name_range: TextRange,
    /// The name starts with a backslash.
    pub leading: bool,
    /// The separators are written twice, as an escaped backslash.
    pub doubled: bool,
    /// A method the class has, after `::` or `@`.
    pub method: Option<(String, TextRange)>,
}

fn is_identifier(text: &str) -> bool {
    text.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        && text.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// The class name a string literal holds, when it is one the index knows.
pub fn read(index: &Index, literal: &SyntaxNode) -> Option<ClassString> {
    if literal.kind() != LITERAL {
        return None;
    }
    let token = literal
        .children_with_tokens()
        .filter_map(|element| element.into_token())
        .find(|token| token.kind() == STRING_LITERAL)?;
    let text = token.text();
    let quote = text.chars().next()?;
    if !matches!(quote, '\'' | '"') || text.len() < 2 {
        return None;
    }
    let raw = &text[1..text.len() - 1];
    if quote == '"' && raw.contains('$') {
        return None;
    }
    let base = u32::from(token.text_range().start()) + 1;
    let (class_raw, member) = match raw.split_once("::").or_else(|| raw.split_once('@')) {
        Some((class, member)) => (class, Some(member)),
        None => (raw, None),
    };
    let leading = class_raw.starts_with('\\');
    let body = class_raw.trim_start_matches('\\');
    let doubled = body.contains("\\\\");
    let separator = if doubled { "\\\\" } else { "\\" };
    let segments: Vec<&str> = body.split(separator).collect();
    if segments.len() < 2 || !segments.iter().all(|segment| is_identifier(segment)) {
        return None;
    }
    let found = index.class(&segments.join("\\"))?;
    let class = found.decl.name.clone();
    let end = base + class_raw.len() as u32;
    let short = segments.last()?;
    let method = member.filter(|member| is_identifier(member)).and_then(|member| {
        let declared = index.find_method(&Type::class(class.clone()), member)?;
        let start = end + if raw[class_raw.len()..].starts_with("::") { 2 } else { 1 };
        Some((
            declared.member.name.clone(),
            range_of(start, start + member.len() as u32),
        ))
    });
    if member.is_some() && method.is_none() {
        return None;
    }
    Some(ClassString {
        class,
        short_range: range_of(end - short.len() as u32, end),
        name_range: range_of(base, end),
        leading,
        doubled,
        method,
    })
}

/// The class string of the literal under an offset.
pub fn at(index: &Index, root: &SyntaxNode, offset: u32) -> Option<ClassString> {
    let literal = crate::ast::node_at(root, offset)
        .ancestors()
        .find(|node| node.kind() == LITERAL)?;
    let found = read(index, &literal)?;
    let within = |range: TextRange| u32::from(range.start()) <= offset && offset <= u32::from(range.end());
    (within(found.name_range) || found.method.as_ref().is_some_and(|(_, range)| within(*range))).then_some(found)
}

/// How a class string writes another class in its place.
pub fn spelled(found: &ClassString, class: &str) -> String {
    let name = if found.doubled {
        class.replace('\\', "\\\\")
    } else {
        class.to_string()
    };
    if found.leading {
        let lead = if found.doubled { "\\\\" } else { "\\" };
        format!("{lead}{name}")
    } else {
        name
    }
}

/// The classes whose qualified name starts with what a string holds before the cursor, once that
/// is a name with a backslash in it.
pub fn complete(
    index: &Index,
    root: &SyntaxNode,
    offset: u32,
    options: crate::completion::CompletionOptions,
) -> Option<crate::completion::CompletionList> {
    use crate::completion::{CompletionItem, CompletionList, TextEdit};
    let token = match root.token_at_offset(php_syntax::TextSize::from(offset)) {
        php_syntax::TokenAtOffset::Single(token) => token,
        php_syntax::TokenAtOffset::Between(left, right) => {
            [left, right].into_iter().find(|token| token.kind() == STRING_LITERAL)?
        }
        php_syntax::TokenAtOffset::None => return None,
    };
    if token.kind() != STRING_LITERAL {
        return None;
    }
    let start = u32::from(token.text_range().start()) + 1;
    let end = u32::from(token.text_range().end()).saturating_sub(1);
    if offset < start || offset > end {
        return None;
    }
    let text = token.text();
    let typed = &text[1..(offset - start + 1) as usize];
    let body = typed.trim_start_matches('\\');
    if !body.contains('\\')
        || !body.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
        || !body.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '\\')
    {
        return None;
    }
    let doubled = body.contains("\\\\");
    let wanted = if doubled {
        body.replace("\\\\", "\\")
    } else {
        body.to_string()
    }
    .to_ascii_lowercase();
    let name_start = start + (typed.len() - body.len()) as u32;
    let rest = &text[(offset - start + 1) as usize..text.len() - 1];
    let name_end = offset
        + rest
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(rest.len()) as u32;
    let mut names: Vec<(String, php_index::ClassKind)> = index
        .class_names()
        .filter(|class| class.summary.name.to_ascii_lowercase().starts_with(&wanted))
        .map(|class| (class.summary.name.to_string(), class.summary.kind))
        .collect();
    names.sort_by(|left, right| left.0.cmp(&right.0));
    names.dedup_by(|left, right| left.0.eq_ignore_ascii_case(&right.0));
    let incomplete = names.len() > options.limit;
    let items = names
        .into_iter()
        .take(options.limit)
        .map(|(name, kind)| {
            let written = if doubled {
                name.replace('\\', "\\\\")
            } else {
                name.clone()
            };
            CompletionItem {
                label: name.clone(),
                kind: crate::completion::class_item_kind(kind),
                detail: None,
                description: None,
                edit: TextEdit {
                    start: name_start,
                    end: name_end,
                    new_text: written.clone(),
                },
                additional_edits: Vec::new(),
                sort_text: name.to_ascii_lowercase(),
                filter_text: Some(written),
                deprecated: false,
                data: None,
            }
        })
        .collect();
    Some(CompletionList { items, incomplete })
}

/// Whether a refactor leaves the class names in a file's strings alone: a migration records what
/// the database holds, which a rename of the class does not change.
pub fn kept_by_refactors(path: &std::path::Path) -> bool {
    path.components().any(|component| component.as_os_str() == "migrations")
}
