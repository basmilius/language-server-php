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
    definitions_at(&fixture.index, Some(Path::new(PAGE)), &text, &[], offset)
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
        &[],
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
    let hover = hover_at(&fixture().index, Some(Path::new(PAGE)), &text, &[], offset).expect("a hover");
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
            let _ = definitions_at(&fixture.index, path, text, &[], offset as u32);
            let _ = hover_at(&fixture.index, path, text, &[], offset as u32);
            let _ = complete_at(
                &fixture.index,
                path,
                text,
                &[],
                offset as u32,
                CompletionOptions::default(),
            );
        }
    }
}

mod variables {
    use php_index::framework::testing::{SYMFONY, TWIG as TWIG_LIBRARY};

    use super::super::*;
    use crate::references::{Current, references_at};
    use crate::testing::{CURSOR, Files, Fixture};

    const POST: &str = "<?php\nnamespace App\\Entity;\nclass Post {\n    public string $slug = '';\n    public function getTitle(): string {}\n    public function isPublished(): bool {}\n    public function author(): User {}\n}\nclass User { public function getName(): string {} }\n";
    const CONTROLLER: &str = "<?php\nnamespace App\\Controller;\nuse App\\Entity\\Post;\nuse Symfony\\Bundle\\FrameworkBundle\\Controller\\AbstractController;\nclass PostController extends AbstractController {\n    public function show(Post $post) {\n        return $this->render('blog/post.html.twig', ['post' => $post, 'posts' => [$post]]);\n    }\n}\n";
    const VIEW: &str = "{{ post.title }}{% for item in posts %}{{ item.author.name }}{{ loop.index }}{% endfor %}{% set first = post %}{{ first.slug }}";

    fn fixture() -> Fixture {
        let mut files = SYMFONY.to_vec();
        files.extend_from_slice(TWIG_LIBRARY);
        files.extend_from_slice(&[
            ("src/Entity/Post.php", POST),
            ("src/Controller/PostController.php", CONTROLLER),
            ("templates/blog/post.html.twig", VIEW),
        ]);
        Fixture::framework(&files)
    }

    const PATH: &str = "/project/templates/blog/post.html.twig";

    fn given(fixture: &Fixture) -> Vec<(String, Type)> {
        data::given(&fixture.index, &Files(fixture.sources.clone()), Path::new(PATH))
    }

    #[test]
    fn a_controller_gives_its_template_variables() {
        let fixture = fixture();
        let shown: Vec<String> = given(&fixture)
            .into_iter()
            .map(|(name, ty)| format!("{name}: {}", ty.display(true)))
            .collect();
        assert_eq!(shown, ["post: Post", "posts: list<Post>"]);
    }

    fn at(template: &str) -> (String, u32) {
        let offset = template.find(CURSOR).expect("a cursor") as u32;
        (template.replacen(CURSOR, "", 1), offset)
    }

    #[test]
    fn attributes_are_the_properties_and_getters_twig_reads() {
        let fixture = fixture();
        let given = given(&fixture);
        let path = Some(Path::new(PATH));
        let hover = |template: &str| {
            let (text, offset) = at(template);
            hover_at(&fixture.index, path, &text, &given, offset)
                .map(|hover| hover.markdown)
                .unwrap_or_default()
        };
        assert!(
            hover("{{ post.ti$0tle }}").contains("getTitle"),
            "{}",
            hover("{{ post.ti$0tle }}")
        );
        assert!(hover("{% for item in posts %}{{ item.author.na$0me }}{% endfor %}").contains("getName"));
        assert!(hover("{% set first = post %}{{ fi$0rst }}").contains("Post"));
        let complete = |template: &str| -> Vec<String> {
            let (text, offset) = at(template);
            let mut items: Vec<String> = complete_at(
                &fixture.index,
                path,
                &text,
                &given,
                offset,
                CompletionOptions::default(),
            )
            .map(|list| list.items.into_iter().map(|item| item.label).collect())
            .unwrap_or_default();
            items.sort();
            items
        };
        assert_eq!(complete("{{ post.$0 }}"), ["author", "published", "slug", "title"]);
        assert_eq!(
            complete("{% for item in posts %}{{ loop.$0 }}{% endfor %}"),
            [
                "first",
                "index",
                "index0",
                "last",
                "length",
                "parent",
                "revindex",
                "revindex0"
            ]
        );
        assert!(complete("{{ po$0 }}").contains(&"post".to_string()));
        let (text, offset) = at("{{ post.ti$0tle }}");
        let places = definitions_at(&fixture.index, path, &text, &given, offset);
        assert_eq!(places.len(), 1);
        assert!(places[0].path.as_ref().is_some_and(|path| path.ends_with("Post.php")));
    }

    #[test]
    fn usages_of_a_getter_reach_the_template() {
        let fixture = fixture();
        let path = std::path::PathBuf::from("/project/src/Entity/Post.php");
        let root = php_syntax::parse(POST).syntax();
        let offset = POST.find("getTitle").expect("the getter") as u32 + 1;
        let found = references_at(
            &fixture.index,
            &Files(fixture.sources.clone()),
            &Current {
                path: &path,
                text: POST,
                root: &root,
            },
            offset,
        )
        .expect("usages");
        let in_view: Vec<&str> = found
            .files
            .iter()
            .filter(|file| file.path.ends_with("post.html.twig"))
            .flat_map(|file| file.hits.iter())
            .map(|hit| &VIEW[usize::from(hit.range.start())..usize::from(hit.range.end())])
            .collect();
        assert_eq!(in_view, ["title"]);
    }

    #[test]
    fn a_form_is_its_view_with_its_children() {
        let mut files = SYMFONY.to_vec();
        files.extend_from_slice(TWIG_LIBRARY);
        files.extend_from_slice(&[
            ("vendor/php/ArrayAccess.php", "<?php interface ArrayAccess { public function offsetGet(mixed $offset): mixed; }"),
            (
                "vendor/symfony/Form.php",
                "<?php namespace Symfony\\Component\\Form; interface FormInterface {} class FormView implements \\ArrayAccess { public array $vars = []; public function offsetGet(mixed $name): self {} }",
            ),
            (
                "src/Controller/FormController.php",
                "<?php\nnamespace App\\Controller;\nuse Symfony\\Bundle\\FrameworkBundle\\Controller\\AbstractController;\nuse Symfony\\Component\\Form\\FormInterface;\nclass FormController extends AbstractController {\n    public function edit(FormInterface $form) { return $this->render('form.html.twig', ['form' => $form]); }\n}\n",
            ),
            ("templates/form.html.twig", "{{ form.title.vars }}"),
        ]);
        let fixture = Fixture::framework(&files);
        let path = Path::new("/project/templates/form.html.twig");
        let given = data::given(&fixture.index, &Files(fixture.sources.clone()), path);
        assert_eq!(given.len(), 1);
        assert_eq!(given[0].1.display(true), "FormView");
        let text = "{{ form.title.va$0rs }}";
        let offset = text.find(CURSOR).expect("a cursor") as u32;
        let text = text.replacen(CURSOR, "", 1);
        let hover = hover_at(&fixture.index, Some(path), &text, &given, offset).expect("a hover");
        assert!(hover.markdown.contains("vars"), "{}", hover.markdown);
    }
}
