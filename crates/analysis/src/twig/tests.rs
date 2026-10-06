//! Twig templates: what their names lead to, and the functions and filters of their extensions.

use php_index::framework::testing::{SYMFONY, TWIG as TWIG_LIBRARY};

use super::*;
use crate::testing::{CURSOR, Fixture};

const TWIG: &[(&str, &str)] = &[
    (
        "src/Controller/BlogController.php",
        "<?php\nnamespace App\\Controller;\nuse Symfony\\Component\\Routing\\Attribute\\Route;\nclass BlogController {\n    #[Route('/', name: 'blog_index')]\n    public function index() {}\n}\n",
    ),
    ("translations/messages.en.yaml", "post:\n    title: Title\n"),
    (
        "templates/base.html.twig",
        "<title>{% block title %}Blog{% endblock %}</title>{% block body %}{% endblock %}",
    ),
    ("templates/blog/_post.html.twig", "{{ post }}"),
];

fn fixture() -> Fixture {
    let mut files = SYMFONY.to_vec();
    files.extend_from_slice(TWIG_LIBRARY);
    files.extend_from_slice(TWIG);
    Fixture::framework(&files)
}

const PAGE: &str = "/project/templates/blog/index.html.twig";

fn split(template: &str) -> (String, u32) {
    let offset = template.find(CURSOR).expect("a cursor") as u32;
    (template.replacen(CURSOR, "", 1), offset)
}

fn places(template: &str) -> Vec<String> {
    let (text, offset) = split(template);
    let fixture = fixture();
    definitions_at(&fixture.index, Some(Path::new(PAGE)), &text, offset)
        .into_iter()
        .map(|place| {
            let path = place.path.expect("a file");
            let source = fixture.sources.get(&path).cloned().unwrap_or_default();
            format!(
                "{}: {}",
                path.strip_prefix("/project").unwrap_or(&path).display(),
                &source[place.span.start as usize..place.span.end as usize]
            )
        })
        .collect()
}

fn completions(template: &str) -> Vec<String> {
    let (text, offset) = split(template);
    let mut items: Vec<String> = complete_at(
        &fixture().index,
        Some(Path::new(PAGE)),
        &text,
        offset,
        CompletionOptions::default(),
    )
    .map(|list| list.items.into_iter().map(|item| item.label).collect())
    .unwrap_or_default();
    items.sort();
    items
}

#[test]
fn templates_and_blocks_lead_to_their_files() {
    assert_eq!(
        places("{% extends 'ba$0se.html.twig' %}"),
        ["templates/base.html.twig: "]
    );
    assert_eq!(
        places("{{ include('blog/_po$0st.html.twig', {post: p}) }}"),
        ["templates/blog/_post.html.twig: "]
    );
    assert_eq!(
        places("{% extends 'base.html.twig' %}{% block ti$0tle %}Home{% endblock %}"),
        ["templates/base.html.twig: title"]
    );
    assert_eq!(
        completions("{% extends 'base.html.twig' %}{% block $0 %}"),
        Vec::<String>::new(),
        "an empty name is not a name yet"
    );
    assert_eq!(completions("{% extends 'base.html.twig' %}{% block b$0 %}"), ["body"]);
    assert_eq!(completions("{% include 'blog/$0' %}"), ["blog/_post.html.twig"]);
}

#[test]
fn strings_of_functions_and_filters_name_what_their_php_takes() {
    assert_eq!(
        places("<a href=\"{{ path('blog_in$0dex') }}\">"),
        ["src/Controller/BlogController.php: index"]
    );
    assert_eq!(completions("{{ path('blog$0') }}"), ["blog_index"]);
    assert_eq!(
        places("{{ 'post.ti$0tle'|trans }}"),
        ["translations/messages.en.yaml: title"]
    );
}

#[test]
fn functions_filters_and_tests_come_from_the_extensions() {
    assert_eq!(completions("{{ name|up$0 }}"), ["upper"]);
    assert_eq!(completions("{{ pa$0 }}"), ["path"]);
    assert_eq!(completions("{% if x is ev$0 %}"), ["even"]);
    assert_eq!(
        places("{{ name|len$0gth }}"),
        ["vendor/symfony/twig/CoreExtension.php: length"]
    );
    let (text, offset) = split("{{ items|len$0gth }}");
    let hover = hover_at(&fixture().index, Some(Path::new(PAGE)), &text, offset).expect("a hover");
    assert!(hover.markdown.contains("Twig filter 'length'"), "{}", hover.markdown);
    assert!(hover.markdown.contains("Counts."), "{}", hover.markdown);
}

#[test]
fn no_prefix_of_a_template_breaks_the_reading() {
    let template = "{% extends 'base.html.twig' %}{% block body %}{% for post in posts if post.ok %}{{ path('blog_index', {page: 1})|upper }}{% else %}{{ 'post.title'|trans }}{% endfor %}{% set x = [1, {a: b}] %}{% apply upper %}é{% endapply %}{% endblock %}{# c #}";
    let fixture = fixture();
    for end in (0..=template.len()).filter(|end| template.is_char_boundary(*end)) {
        let text = &template[..end];
        for offset in (0..=text.len()).filter(|offset| text.is_char_boundary(*offset)) {
            let path = Some(Path::new(PAGE));
            let _ = definitions_at(&fixture.index, path, text, offset as u32);
            let _ = hover_at(&fixture.index, path, text, offset as u32);
            let _ = complete_at(&fixture.index, path, text, offset as u32, CompletionOptions::default());
        }
    }
}
