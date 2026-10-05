//! Declarations as text: signatures as PHP source would write them and PHPDoc as markdown.

use php_index::{Callable, ClassConst, ClassDecl, ClassKind, Doc, Method, Param, Property, Type};
use php_syntax::PhpVersion;

fn type_text(ty: &Type) -> String {
    ty.display(true)
}

/// `Type &...$name = default` for one parameter, with the type the signature shows: the native one,
/// else the one from PHPDoc.
pub fn param_text(param: &Param, level: PhpVersion) -> String {
    let mut out = String::new();
    if let Some(promotion) = param.promoted {
        out.push_str(promotion.visibility.keyword());
        out.push(' ');
        if promotion.readonly {
            out.push_str("readonly ");
        }
    }
    let ty = param.native_type(level).or_else(|| param.effective_type(level));
    if let Some(ty) = ty {
        out.push_str(&type_text(ty));
        out.push(' ');
    }
    if param.by_ref {
        out.push('&');
    }
    if param.variadic {
        out.push_str("...");
    }
    out.push('$');
    out.push_str(&param.name);
    if let Some(default) = &param.default {
        out.push_str(" = ");
        out.push_str(&shorten(default));
    }
    out
}

fn shorten(value: &str) -> String {
    const LIMIT: usize = 40;
    if value.chars().count() <= LIMIT {
        return value.to_string();
    }
    let cut: String = value.chars().take(LIMIT).collect();
    format!("{cut}...")
}

/// `(int $id, string $name = 'x'): ?User`.
pub fn callable_text(callable: &Callable, level: PhpVersion) -> String {
    let params: Vec<String> = callable
        .params_at(level)
        .map(|param| param_text(param, level))
        .collect();
    let mut out = format!("({})", params.join(", "));
    let ret = callable
        .native_return(level)
        .or_else(|| callable.effective_return(level));
    if let Some(ret) = ret {
        out.push_str(": ");
        out.push_str(&type_text(ret));
    }
    out
}

pub fn function_signature(name: &str, callable: &Callable, level: PhpVersion) -> String {
    let by_ref = if callable.by_ref_return { "&" } else { "" };
    format!("function {by_ref}{name}{}", callable_text(callable, level))
}

pub fn method_signature(method: &Method, level: PhpVersion) -> String {
    let mut out = String::new();
    if method.is_abstract {
        out.push_str("abstract ");
    }
    if method.is_final {
        out.push_str("final ");
    }
    out.push_str(method.visibility.keyword());
    out.push(' ');
    if method.is_static {
        out.push_str("static ");
    }
    let by_ref = if method.callable.by_ref_return { "&" } else { "" };
    out.push_str(&format!(
        "function {by_ref}{}{}",
        method.name,
        callable_text(&method.callable, level)
    ));
    out
}

pub fn property_signature(property: &Property, level: PhpVersion) -> String {
    let mut out = String::new();
    out.push_str(property.visibility.keyword());
    if let Some(set) = property.set_visibility {
        out.push_str(&format!(" {}(set)", set.keyword()));
    }
    out.push(' ');
    if property.is_static {
        out.push_str("static ");
    }
    if property.is_readonly {
        out.push_str("readonly ");
    }
    let ty = property.native_type(level).or_else(|| property.effective_type(level));
    if let Some(ty) = ty {
        out.push_str(&type_text(ty));
        out.push(' ');
    }
    out.push('$');
    out.push_str(&property.name);
    if let Some(default) = &property.default {
        out.push_str(" = ");
        out.push_str(&shorten(default));
    }
    if !property.hooks.is_empty() {
        out.push_str(&format!(
            " {{ {} }}",
            property
                .hooks
                .iter()
                .map(|hook| format!("{hook};"))
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }
    out
}

pub fn constant_signature(constant: &ClassConst, level: PhpVersion) -> String {
    if constant.is_case {
        return match &constant.value {
            Some(value) => format!("case {} = {}", constant.name, shorten(value)),
            None => format!("case {}", constant.name),
        };
    }
    let mut out = String::new();
    out.push_str(constant.visibility.keyword());
    out.push(' ');
    if constant.is_final {
        out.push_str("final ");
    }
    out.push_str("const ");
    if let Some(ty) = constant.effective_type(level).filter(|_| constant.ty.is_some()) {
        out.push_str(&type_text(ty));
        out.push(' ');
    }
    out.push_str(&constant.name);
    if let Some(value) = &constant.value {
        out.push_str(" = ");
        out.push_str(&shorten(value));
    }
    out
}

pub fn class_signature(class: &ClassDecl) -> String {
    let mut out = String::new();
    if class.is_abstract && class.kind == ClassKind::Class {
        out.push_str("abstract ");
    }
    if class.is_final {
        out.push_str("final ");
    }
    if class.is_readonly {
        out.push_str("readonly ");
    }
    out.push_str(class.kind.keyword());
    out.push(' ');
    out.push_str(crate::short(&class.name));
    if let Some(backing) = &class.backing {
        out.push_str(&format!(": {}", type_text(backing)));
    }
    let list = |types: &[Type]| types.iter().map(type_text).collect::<Vec<_>>().join(", ");
    if !class.extends.is_empty() {
        out.push_str(&format!(" extends {}", list(&class.extends)));
    }
    if !class.implements.is_empty() {
        out.push_str(&format!(" implements {}", list(&class.implements)));
    }
    out
}

/// A parameter's PHPDoc type when it says more than the native one.
fn doc_type_text(ty: &Option<Type>) -> String {
    ty.as_ref().map_or_else(|| "mixed".to_string(), type_text)
}

/// The PHPDoc as markdown: the summary and the description, then one `_@tag_` line per tag.
pub fn doc_markdown(doc: &Doc) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !doc.summary.is_empty() {
        parts.push(inline_tags(&doc.summary));
    }
    if !doc.description.is_empty() {
        parts.push(inline_tags(&doc.description));
    }
    let mut tags: Vec<String> = Vec::new();
    if let Some(reason) = &doc.deprecated {
        tags.push(tag_line("deprecated", None, reason));
    }
    for param in &doc.params {
        let name = format!("{}${}", if param.variadic { "..." } else { "" }, param.name);
        tags.push(tag_line(
            "param",
            Some(&format!("{} {name}", doc_type_text(&param.ty))),
            &param.description,
        ));
    }
    if let Some((ty, description)) = &doc.ret {
        tags.push(tag_line("return", Some(&type_text(ty)), description));
    }
    if let Some((ty, name, description)) = &doc.var {
        let code = match name {
            Some(name) => format!("{} ${name}", type_text(ty)),
            None => type_text(ty),
        };
        tags.push(tag_line("var", Some(&code), description));
    }
    for (ty, description) in &doc.throws {
        tags.push(tag_line("throws", Some(&type_text(ty)), description));
    }
    for template in &doc.templates {
        let code = match &template.bound {
            Some(bound) => format!("{} of {}", template.name, type_text(bound)),
            None => template.name.clone(),
        };
        tags.push(tag_line("template", Some(&code), &template.description));
    }
    for extends in &doc.extends {
        tags.push(tag_line("extends", Some(&type_text(extends)), ""));
    }
    for implements in &doc.implements {
        tags.push(tag_line("implements", Some(&type_text(implements)), ""));
    }
    for mixin in &doc.mixins {
        tags.push(tag_line("mixin", Some(&type_text(mixin)), ""));
    }
    for property in &doc.properties {
        let tag = if property.read_only {
            "property-read"
        } else if property.write_only {
            "property-write"
        } else {
            "property"
        };
        tags.push(tag_line(
            tag,
            Some(&format!("{} ${}", doc_type_text(&property.ty), property.name)),
            &property.description,
        ));
    }
    for method in &doc.methods {
        let params: Vec<String> = method
            .params
            .iter()
            .map(|param| format!("{} ${}", doc_type_text(&param.ty), param.name))
            .collect();
        let ret = method
            .ret
            .as_ref()
            .map_or_else(String::new, |ret| format!("{} ", type_text(ret)));
        let prefix = if method.is_static { "static " } else { "" };
        tags.push(tag_line(
            "method",
            Some(&format!("{prefix}{ret}{}({})", method.name, params.join(", "))),
            &method.description,
        ));
    }
    for see in &doc.see {
        tags.push(tag_line("see", None, see));
    }
    for raw in &doc.tags {
        if matches!(
            raw.name.as_str(),
            "author" | "package" | "subpackage" | "copyright" | "license" | "version" | "category"
        ) {
            continue;
        }
        tags.push(tag_line(&raw.name, None, &raw.text));
    }
    if !tags.is_empty() {
        parts.push(tags.join("  \n"));
    }
    parts.join("\n\n")
}

fn tag_line(name: &str, code: Option<&str>, text: &str) -> String {
    let mut out = format!("_@{name}_");
    if let Some(code) = code {
        out.push_str(&format!(" `{code}`"));
    }
    let text = inline_tags(text.trim());
    if !text.is_empty() {
        out.push(' ');
        out.push_str(&text.replace('\n', " "));
    }
    out
}

/// The HTML some doc comments are written in, as markdown. Tags that are not HTML stay as they are,
/// since `array<int>` is a type and not a tag. A `<pre>` or `<code>` over several lines becomes a
/// `php` block with its indentation kept, and a fenced block the author wrote stays as written.
fn html_to_markdown(text: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    for segment in code_segments(text) {
        let part = match segment {
            Segment::Prose(prose) => prose_to_markdown(prose),
            Segment::Html(code) => format!("```php\n{}\n```", dedent(&decode_entities(&code))),
            Segment::Fenced(block) => block.trim().to_string(),
        };
        if !part.is_empty() {
            parts.push(part);
        }
    }
    parts.join("\n\n")
}

enum Segment<'a> {
    Prose(&'a str),
    Html(String),
    Fenced(&'a str),
}

/// Splits the code blocks off the prose around them, in order.
fn code_segments(text: &str) -> Vec<Segment<'_>> {
    let mut segments = Vec::new();
    let mut rest = text;
    while let Some((start, end, segment)) = next_code_block(rest) {
        segments.push(Segment::Prose(&rest[..start]));
        segments.push(segment);
        rest = &rest[end..];
    }
    segments.push(Segment::Prose(rest));
    segments
}

/// The first code block in `text` as its byte range and segment. An inline `<code>` on one line is
/// not a block: it stays in the prose as inline code.
fn next_code_block(text: &str) -> Option<(usize, usize, Segment<'_>)> {
    let lower = text.to_ascii_lowercase();
    let mut html = None;
    let mut from = 0;
    while let Some(found) = lower[from..].find('<') {
        let open = from + found;
        from = open + 1;
        let Some(name) = ["pre", "code"].into_iter().find(|name| {
            lower[open + 1..].starts_with(name)
                && lower[open + 1 + name.len()..].starts_with(|c: char| c == '>' || c.is_whitespace())
        }) else {
            continue;
        };
        let Some(inner_start) = lower[open..].find('>').map(|close| open + close + 1) else {
            break;
        };
        let closing = format!("</{name}>");
        let Some(inner_end) = lower[inner_start..].find(&closing).map(|at| inner_start + at) else {
            continue;
        };
        let inner = &text[inner_start..inner_end];
        if inner.trim().contains('\n') {
            let code = inner.replace("<code>", "").replace("</code>", "");
            html = Some((open, inner_end + closing.len(), Segment::Html(code)));
            break;
        }
    }
    let fence = fenced_block(text);
    match (html, fence) {
        (Some(html), Some(fence)) => Some(if fence.0 < html.0 { fence } else { html }),
        (html, fence) => html.or(fence),
    }
}

/// A block the author fenced with three backticks, each at the start of a line.
fn fenced_block(text: &str) -> Option<(usize, usize, Segment<'_>)> {
    let mut offset = 0;
    let mut open = None;
    for line in text.split_inclusive('\n') {
        if line.trim_start().starts_with("```") {
            match open {
                None => open = Some(offset),
                Some(start) => {
                    let end = offset + line.len();
                    return Some((start, end, Segment::Fenced(&text[start..end])));
                }
            }
        }
        offset += line.len();
    }
    None
}

/// The lines of a block without the blank ones around them and without the indentation they share.
fn dedent(code: &str) -> String {
    let lines: Vec<&str> = code.lines().map(str::trim_end).collect();
    let first = lines.iter().position(|line| !line.is_empty()).unwrap_or(lines.len());
    let last = lines
        .iter()
        .rposition(|line| !line.is_empty())
        .map_or(first, |last| last + 1);
    let lines = &lines[first..last];
    let indent = lines
        .iter()
        .filter(|line| !line.is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or(0);
    lines
        .iter()
        .map(|line| line.get(indent..).unwrap_or(""))
        .collect::<Vec<_>>()
        .join("\n")
}

fn decode_entities(text: &str) -> String {
    text.replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn prose_to_markdown(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('>').filter(|close| *close <= 80) else {
            out.push('<');
            rest = after;
            continue;
        };
        let tag = after[..close].trim().trim_end_matches('/').trim().to_ascii_lowercase();
        let name = tag.split_whitespace().next().unwrap_or("");
        let replacement = match name {
            "p" | "/p" | "/ul" | "/ol" | "/pre" | "pre" | "/h1" | "/h2" | "/h3" | "/h4" | "/table" | "table" => {
                Some("\n\n")
            }
            "br" | "/li" | "/tr" | "/div" | "div" | "tr" => Some("\n"),
            "ul" | "ol" => Some("\n"),
            "li" => Some("\n- "),
            "b" | "/b" | "strong" | "/strong" => Some("**"),
            "i" | "/i" | "em" | "/em" => Some("_"),
            "code" | "/code" | "tt" | "/tt" | "kbd" | "/kbd" => Some("`"),
            "a" | "/a" | "span" | "/span" | "h1" | "h2" | "h3" | "h4" | "td" | "/td" | "th" | "/th" | "thead"
            | "/thead" | "tbody" | "/tbody" | "font" | "/font" | "sup" | "/sup" | "sub" | "/sub" | "u" | "/u" => {
                Some("")
            }
            _ => None,
        };
        match replacement {
            Some(text) => {
                out.push_str(text);
                rest = &after[close + 1..];
            }
            None => {
                out.push('<');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    let decoded = decode_entities(&out);
    let mut collapsed = String::with_capacity(decoded.len());
    let mut blank_run = 0;
    for line in decoded.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
        } else {
            blank_run = 0;
        }
        collapsed.push_str(line.trim_start_matches(' '));
        collapsed.push('\n');
    }
    collapsed.trim().to_string()
}

/// `{@see Foo}` and `{@link url}` as inline code or a link, which is how a client shows a reference.
fn inline_tags(text: &str) -> String {
    html_to_markdown(&inline_tags_raw(text))
}

fn inline_tags_raw(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find("{@") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        let Some(close) = after.find('}') else {
            out.push_str(&rest[open..]);
            return out;
        };
        let inner = &after[..close];
        let (tag, argument) = inner.split_once(char::is_whitespace).unwrap_or((inner, ""));
        match tag {
            "link" if argument.trim_start().starts_with("http") => {
                let argument = argument.trim();
                match argument.split_once(char::is_whitespace) {
                    Some((url, label)) => out.push_str(&format!("[{}]({url})", label.trim())),
                    None => out.push_str(&format!("<{argument}>")),
                }
            }
            "see" | "link" | "uses" | "inheritdoc" | "internal" => {
                let argument = argument.trim();
                if argument.is_empty() {
                    out.push_str(&format!("`{tag}`"));
                } else {
                    out.push_str(&format!("`{argument}`"));
                }
            }
            _ => out.push_str(&format!("`{}`", inner.trim())),
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use expect_test::expect;
    use php_index::extract::{ExtractOptions, extract};
    use php_syntax::parse;

    #[test]
    fn renders_signatures() {
        let file = extract(
            &parse(
                r#"<?php
namespace App;
abstract class Base implements \JsonSerializable {}
final class User extends Base {
    public const int MAX = 10;
    public function __construct(private readonly string $name, public ?int $age = null, int ...$rest) {}
    public static function find(int $id, array $options = ['a' => 1]): ?static {}
    protected readonly ?Post $post;
    public static int $count = 0;
}
enum Suit: string { case Hearts = 'h'; }
function &helper(string $x): void {}
"#,
            )
            .syntax(),
            ExtractOptions::default(),
        );
        let level = PhpVersion::V8_4;
        let user = &file.classes[1];
        expect!["final class User extends Base"].assert_eq(&class_signature(user));
        expect!["public function __construct(private readonly string $name, public ?int $age = null, int ...$rest)"]
            .assert_eq(&method_signature(user.method("__construct").unwrap(), level));
        expect!["public static function find(int $id, array $options = ['a' => 1]): ?static"]
            .assert_eq(&method_signature(user.method("find").unwrap(), level));
        expect!["protected readonly ?Post $post"].assert_eq(&property_signature(user.property("post").unwrap(), level));
        expect!["public static int $count = 0"].assert_eq(&property_signature(user.property("count").unwrap(), level));
        expect!["public const int MAX = 10"].assert_eq(&constant_signature(user.constant("MAX").unwrap(), level));
        expect!["case Hearts = 'h'"].assert_eq(&constant_signature(&file.classes[2].constants[0], level));
        expect!["enum Suit: string"].assert_eq(&class_signature(&file.classes[2]));
        expect!["function &helper(string $x): void"].assert_eq(&function_signature(
            "helper",
            &file.functions[0].callable,
            level,
        ));
    }

    #[test]
    fn turns_the_html_of_the_stubs_into_markdown() {
        assert_eq!(
            html_to_markdown(
                "(PHP 5 &gt;=5.5.0)<br/>\nReturns <b>FALSE</b> or an <i>int</i>.<p>Next, a list of array<int> values</p>"
            ),
            "(PHP 5 >=5.5.0)\n\nReturns **FALSE** or an _int_.\n\nNext, a list of array<int> values"
        );
        assert_eq!(
            inline_tags("Formats like {@link https://php.net/date date()} does"),
            "Formats like [date()](https://php.net/date) does"
        );
    }

    #[test]
    fn turns_code_over_several_lines_into_a_php_block() {
        expect![[r#"
            For example:

            ```php
            class User extends Model {
                #[Column]
                public string $name;
            }
            ```

            Or `inline()` code.

            ```php
            if ($a &lt; $b) {
                return;
            }
            ```

            ```js
              kept();
            ```"#]]
        .assert_eq(&html_to_markdown(
            "For example:\n<code>\nclass User extends Model {\n    #[Column]\n    public string $name;\n}\n</code>\nOr <code>inline()</code> code.\n<pre><code>\n    if ($a &amp;lt; $b) {\n        return;\n    }\n</code></pre>\n```js\n  kept();\n```",
        ));
    }

    #[test]
    fn renders_doc_comments_as_markdown() {
        let file = extract(
            &parse(
                r#"<?php
/**
 * Finds a user. See {@see User::all()}.
 *
 * Longer text.
 *
 * @param int $id The id
 * @param array<string, int> $options
 * @return User|null The user
 * @throws \RuntimeException when the database is gone
 * @deprecated use all()
 */
function find($id, $options) {}
"#,
            )
            .syntax(),
            ExtractOptions::default(),
        );
        let doc = file.functions[0].doc.as_ref().unwrap();
        expect![[r#"
            Finds a user. See `User::all()`.

            Longer text.

            _@deprecated_ use all()  
            _@param_ `int $id` The id  
            _@param_ `array<string, int> $options`  
            _@return_ `?User` The user  
            _@throws_ `RuntimeException` when the database is gone"#]]
        .assert_eq(&doc_markdown(doc));
    }
}
