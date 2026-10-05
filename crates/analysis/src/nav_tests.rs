use expect_test::expect;

use crate::infer::Analyzer;
use crate::nav::Place;
use crate::testing::{Fixture, split_cursor};

fn fixture() -> Fixture {
    Fixture::new(&[(
        "src/User.php",
        r#"<?php
namespace App;

/**
 * A person who can log in.
 *
 * @template T
 */
class User extends Base implements Named {
    /** The display name. */
    public string $name = '';

    /**
     * Finds a user.
     *
     * @param int $id The id
     * @return static|null The user
     * @throws \RuntimeException when the database is gone
     */
    public static function find(int $id): ?static { return null; }

    public function name(): string { return ''; }

    public const ROLE = 'user';
}
abstract class Base { public function name(): string { return ''; } public function base(): void {} }
interface Named { /** Gets the name. */ public function name(): string; }
class Admin extends User { public function name(): string { return 'a'; } }
class Guest extends User {}
/** Makes a thing. */
function make(int $n = 1): User {}
"#,
    )])
}

fn hover(code: &str) -> String {
    let fixture = fixture().with_current(code);
    let (_, root, offset) = split_cursor(code);
    let analyzer = Analyzer::new(&fixture.index, &root, offset);
    analyzer
        .hover(offset)
        .map_or_else(|| "<none>".to_string(), |hover| hover.markdown)
}

fn places(analyzer_code: &str, query: impl Fn(&Analyzer, u32) -> Vec<Place>) -> Vec<String> {
    let fixture = fixture().with_current(analyzer_code);
    let (text, root, offset) = split_cursor(analyzer_code);
    let analyzer = Analyzer::new(&fixture.index, &root, offset);
    query(&analyzer, offset)
        .into_iter()
        .map(|place| {
            let path = place
                .path
                .map_or_else(|| "(current)".to_string(), |path| path.to_string_lossy().into_owned());
            let source = if path == "(current)" {
                text.clone()
            } else {
                fixture
                    .sources
                    .get(std::path::Path::new(&path))
                    .cloned()
                    .unwrap_or_default()
            };
            let word = source
                .get(place.span.start as usize..place.span.end as usize)
                .unwrap_or("?")
                .to_string();
            format!("{path} {word}")
        })
        .collect()
}

#[test]
fn hover_on_a_class_shows_its_signature_and_doc() {
    expect![[r#"
        **App\User**

        ```php
        class User extends Base implements Named
        ```

        A person who can log in.

        _@template_ `T`"#]]
    .assert_eq(&hover("<?php\nuse App\\User;\nnew Us$0er();\n"));
}

#[test]
fn hover_on_a_method_shows_params_return_and_throws() {
    expect![[r#"
        **App\User::find**

        ```php
        public static function find(int $id): ?static
        ```

        Finds a user.

        _@param_ `int $id` The id  
        _@return_ `?static` The user  
        _@throws_ `RuntimeException` when the database is gone"#]]
    .assert_eq(&hover("<?php\nuse App\\User;\nUser::fi$0nd(1);\n"));
}

#[test]
fn hover_on_members_functions_variables_and_constants() {
    let property = hover("<?php\nuse App\\User;\nfunction f(User $u) { $u->na$0me; }\n");
    assert!(
        property.starts_with("**App\\User::$name**\n\n```php\npublic string $name = ''\n```\n\nThe display name."),
        "{property}"
    );
    let function = hover("<?php\nuse function App\\make;\nmake$0(2);\n");
    assert!(
        function.contains("```php\nfunction make(int $n = 1): User\n```") && function.contains("Makes a thing."),
        "{function}"
    );
    let variable = hover("<?php\nuse App\\User;\nfunction f(User $u) { $u$0; }\n");
    assert_eq!(variable, "**$u**\n\n```php\nUser $u\n```");
    let constant = hover("<?php\nuse App\\User;\nUser::RO$0LE;\n");
    assert!(constant.contains("public const string ROLE = 'user'"), "{constant}");
    assert_eq!(hover("<?php\n$x = 1 +$0 2;\n"), "<none>");
}

#[test]
fn a_method_without_a_doc_inherits_the_one_above_it() {
    let text = hover("<?php\nuse App\\Guest;\nfunction f(Guest $g) { $g->na$0me(); }\n");
    assert!(text.contains("App\\User::name"), "{text}");
    let inherited = hover("<?php\nuse App\\Admin;\nfunction f(Admin $g) { $g->na$0me(); }\n");
    assert!(inherited.contains("Gets the name."), "{inherited}");
}

#[test]
fn an_inheritdoc_takes_the_doc_above_it_under_its_own_tags() {
    let code = r#"<?php
interface Visible {
    /**
     * Makes the keys visible.
     *
     * Hidden keys stay hidden.
     *
     * @param string[] $keys The keys
     * @since 1.0.0
     */
    public function show(array $keys): static;
}
class Items implements Visible {
    /**
     * {@inheritdoc}
     *
     * @since 1.0.17
     */
    public function show(array $keys): static { return $this; }
    /** Before {@inheritDoc} after. */
    public function other(): void {}
}
function f(Items $items) { $items->sh$0ow([]); }
"#;
    let fixture = Fixture::new(&[]).with_current(code);
    let (_, root, offset) = split_cursor(code);
    let text = Analyzer::new(&fixture.index, &root, offset)
        .hover(offset)
        .map(|hover| hover.markdown)
        .unwrap_or_default();
    assert!(
        text.contains("Makes the keys visible.\n\nHidden keys stay hidden."),
        "{text}"
    );
    assert!(text.contains("_@param_ `list<string> $keys` The keys"), "{text}");
    assert!(
        text.contains("1.0.17") && !text.contains("1.0.0") && !text.contains("inheritdoc"),
        "{text}"
    );
}

#[test]
fn definitions_point_at_declaration_names() {
    let found = places("<?php\nuse App\\User;\nUser::fi$0nd(1);\n", |analyzer, offset| {
        analyzer.definitions(offset)
    });
    assert_eq!(found, vec!["/project/src/User.php find"]);
    let class = places("<?php\nuse App\\User;\nnew Us$0er();\n", |analyzer, offset| {
        analyzer.definitions(offset)
    });
    assert_eq!(class, vec!["/project/src/User.php User"]);
    let variable = places(
        "<?php\nfunction f($a) {\n    $b = 1;\n    echo $b$0;\n}\n",
        |analyzer, offset| analyzer.definitions(offset),
    );
    assert_eq!(variable, vec!["(current) $b"]);
    let function = places("<?php\nuse function App\\make;\nma$0ke();\n", |analyzer, offset| {
        analyzer.definitions(offset)
    });
    assert_eq!(function, vec!["/project/src/User.php make"]);
}

#[test]
fn definition_and_hover_work_on_the_name_of_an_attribute() {
    let found = places("<?php\n#[\\App\\Us$0er]\nclass A {}\n", |analyzer, offset| {
        analyzer.definitions(offset)
    });
    assert_eq!(found, vec!["/project/src/User.php User"]);
    assert!(hover("<?php\nuse App\\User;\n#[Us$0er]\nclass A {}\n").contains("class User"));
}

#[test]
fn type_definitions_follow_the_type_of_the_expression() {
    let variable = places(
        "<?php\nuse App\\User;\nfunction f(User $u) { $u$0; }\n",
        |analyzer, offset| analyzer.type_definitions(offset),
    );
    assert_eq!(variable, vec!["/project/src/User.php User"]);
    let call = places(
        "<?php\nuse App\\User;\nfunction f(User $u) { $u->na$0me(); }\n",
        |analyzer, offset| analyzer.type_definitions(offset),
    );
    assert!(call.is_empty(), "string has no class: {call:?}");
    let function = places("<?php\nuse function App\\make;\nma$0ke();\n", |analyzer, offset| {
        analyzer.type_definitions(offset)
    });
    assert_eq!(function, vec!["/project/src/User.php User"]);
}

#[test]
fn implementations_list_subclasses_and_overrides() {
    let mut classes = places(
        "<?php\nuse App\\User;\nfunction f(Us$0er $u) {}\n",
        |analyzer, offset| analyzer.implementations(offset),
    );
    classes.sort();
    assert_eq!(
        classes,
        vec!["/project/src/User.php Admin", "/project/src/User.php Guest"]
    );
    let interface = places(
        "<?php\nuse App\\Named;\nfunction f(Nam$0ed $u) {}\n",
        |analyzer, offset| analyzer.implementations(offset),
    );
    assert_eq!(interface.len(), 3, "{interface:?}");
    let mut methods = places(
        "<?php\nuse App\\User;\nfunction f(User $u) { $u->na$0me(); }\n",
        |analyzer, offset| analyzer.implementations(offset),
    );
    methods.sort();
    assert_eq!(methods, vec!["/project/src/User.php name"]);
}

#[test]
fn implementations_from_a_declaration_name() {
    let code = "<?php\nnamespace App;\ninterface Sha$0pe { public function area(): float; }\nclass Circle implements Shape { public function area(): float { return 1.0; } }\n";
    let found = places(code, |analyzer, offset| analyzer.implementations(offset));
    assert_eq!(found.len(), 1, "{found:?}");
}
