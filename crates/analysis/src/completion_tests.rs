use crate::completion::{CompletionItem, CompletionList, CompletionOptions, ItemKind, complete};
use crate::testing::{CURSOR, Fixture};

fn project() -> Fixture {
    Fixture::new(&[
        (
            "src/Models.php",
            r#"<?php
namespace App\Models;

/** A person. */
class User extends Model implements HasName {
    public const ROLE = 'user';
    public string $name = '';
    protected int $secret = 1;
    private int $hidden = 2;
    public static int $count = 0;
    public function __construct(public readonly int $id = 0, string $label = '') {}
    public function posts(): array { return []; }
    public static function find(int $id): ?static { return null; }
    protected function guarded(): void {}
    private function internal(): void {}
    /** @deprecated use posts */
    public function oldPosts(): void {}
}

abstract class Model { public function save(): bool { return true; } public static function boot(): void {} }
interface HasName { public function name(): string; }
trait Stamps { public function touch(): void {} }
enum Role: string { case Admin = 'a'; case Guest = 'g'; public function label(): string { return ''; } }
final class Sealed {}
#[\Attribute]
class Route { public function __construct(public string $path = '/') {} }
class AppException extends \Exception {}
"#,
        ),
        (
            "src/Helpers.php",
            r#"<?php
namespace App\Support;

function helper_one(int $a, string $b = 'x'): string { return ''; }
const HELPER_LIMIT = 10;
class Str { public static function slug(string $s): string { return $s; } }
"#,
        ),
        (
            "src/Other.php",
            "<?php\nnamespace Vendor\\Pkg;\nclass Thing {}\nclass Tool {}\n",
        ),
    ])
}

fn run(fixture: Fixture, code: &str) -> CompletionList {
    let fixture = fixture.with_current(code);
    let offset = code.find(CURSOR).expect("a cursor marker") as u32;
    let text = code.replacen(CURSOR, "", 1);
    complete(&fixture.index, &text, offset, CompletionOptions::default())
}

fn labels(list: &CompletionList) -> Vec<&str> {
    list.items.iter().map(|item| item.label.as_str()).collect()
}

fn item<'a>(list: &'a CompletionList, label: &str) -> &'a CompletionItem {
    list.items
        .iter()
        .find(|item| item.label == label)
        .unwrap_or_else(|| panic!("no item {label} in {:?}", labels(list)))
}

fn applied(code: &str, item: &CompletionItem) -> String {
    let mut text = code.replacen(CURSOR, "", 1);
    let mut edits: Vec<(u32, u32, &str)> = vec![(item.edit.start, item.edit.end, item.edit.new_text.as_str())];
    edits.extend(
        item.additional_edits
            .iter()
            .map(|edit| (edit.start, edit.end, edit.new_text.as_str())),
    );
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.0));
    for (start, end, new_text) in edits {
        text.replace_range(start as usize..end as usize, new_text);
    }
    text
}

#[test]
fn completes_instance_members_by_visibility() {
    let outside = run(
        project(),
        "<?php\nuse App\\Models\\User;\nfunction f(User $u) {\n    $u->$0\n}\n",
    );
    let names = labels(&outside);
    assert!(names.contains(&"name") && names.contains(&"posts") && names.contains(&"save"));
    assert!(!names.contains(&"secret") && !names.contains(&"hidden"), "{names:?}");
    assert!(!names.contains(&"guarded") && !names.contains(&"internal"));
    assert!(!names.contains(&"find"), "static methods do not belong after ->");
    assert!(!names.contains(&"__construct"));
    assert_eq!(item(&outside, "posts").kind, ItemKind::Method);
    assert_eq!(item(&outside, "posts").detail.as_deref(), Some("(): array"));
    assert_eq!(item(&outside, "name").kind, ItemKind::Property);
    assert_eq!(item(&outside, "name").detail.as_deref(), Some("string"));
    assert_eq!(item(&outside, "id").detail.as_deref(), Some("int"));
    assert!(item(&outside, "oldPosts").deprecated);
    assert_eq!(item(&outside, "save").description.as_deref(), Some("Model"));
}

#[test]
fn members_open_up_inside_the_class_and_its_children() {
    let inside = run(
        project(),
        "<?php\nnamespace App\\Models;\nclass User2 extends User {\n    public function go() {\n        $this->$0\n    }\n}\n",
    );
    let names = labels(&inside);
    assert!(names.contains(&"secret") && names.contains(&"guarded"));
    assert!(!names.contains(&"hidden") && !names.contains(&"internal"), "{names:?}");
    let own = run(
        project(),
        "<?php\nnamespace App\\Models;\nclass Self1 extends User {\n    private int $mine = 1;\n    public function go() {\n        $this->mi$0\n    }\n}\n",
    );
    assert_eq!(labels(&own), vec!["mine"]);
}

#[test]
fn filters_by_prefix_and_camel_humps() {
    let list = run(
        project(),
        "<?php\nuse App\\Models\\User;\nfunction f(User $u) {\n    $u->po$0\n}\n",
    );
    assert_eq!(labels(&list), vec!["posts"]);
    let humps = run(
        project(),
        "<?php\nuse App\\Models\\User;\nfunction f(User $u) {\n    $u->oP$0\n}\n",
    );
    assert_eq!(labels(&humps), vec!["oldPosts"]);
}

#[test]
fn completes_static_members_constants_and_cases() {
    let list = run(project(), "<?php\nuse App\\Models\\User;\nUser::$0\n");
    let names = labels(&list);
    assert!(
        names.contains(&"find") && names.contains(&"boot") && names.contains(&"ROLE") && names.contains(&"$count"),
        "{names:?}"
    );
    assert!(!names.contains(&"posts"), "instance methods are not offered after ::");
    assert!(names.contains(&"class"));
    let enum_list = run(project(), "<?php\nuse App\\Models\\Role;\nRole::$0\n");
    assert_eq!(item(&enum_list, "Admin").kind, ItemKind::EnumMember);
    assert!(labels(&enum_list).contains(&"cases") && labels(&enum_list).contains(&"from"));
    assert!(!labels(&enum_list).contains(&"label"));
    let statics = run(project(), "<?php\nuse App\\Models\\User;\nUser::$c$0\n");
    assert_eq!(labels(&statics), vec!["$count"]);
}

#[test]
fn parent_and_self_offer_instance_methods_too() {
    let list = run(
        project(),
        "<?php\nnamespace App\\Models;\nclass Child extends User {\n    public function go() {\n        parent::$0\n    }\n}\n",
    );
    let names = labels(&list);
    assert!(
        names.contains(&"posts") && names.contains(&"guarded") && names.contains(&"find"),
        "{names:?}"
    );
    assert!(!names.contains(&"internal"));
}

#[test]
fn completes_through_chains_and_nullsafe() {
    let list = run(
        project(),
        "<?php\nuse App\\Models\\User;\nfunction f() {\n    User::find(1)?->$0\n}\n",
    );
    assert!(labels(&list).contains(&"name"));
    let instance = run(
        project(),
        "<?php\nuse App\\Models\\User;\nfunction f() {\n    $u = new User();\n    $u->$0\n    $other = 1;\n}\n",
    );
    assert!(labels(&instance).contains(&"posts"));
}

#[test]
fn completes_classes_with_an_import() {
    let code = "<?php\n\nnamespace App\\Http;\n\nuse App\\Models\\User;\n\nclass C {\n    public function f() {\n        $x = new Th$0\n    }\n}\n";
    let list = run(project(), code);
    let thing = item(&list, "Thing");
    assert_eq!(thing.kind, ItemKind::Class);
    assert_eq!(thing.description.as_deref(), Some("Vendor\\Pkg"));
    assert_eq!(thing.edit.new_text, "Thing");
    assert_eq!(thing.additional_edits.len(), 1);
    assert!(applied(code, thing).contains("use App\\Models\\User;\nuse Vendor\\Pkg\\Thing;\n"));
}

#[test]
fn a_class_in_the_same_namespace_or_already_imported_needs_no_import() {
    let same = run(
        project(),
        "<?php\nnamespace App\\Models;\nclass X {\n    function f() { Us$0 }\n}\n",
    );
    assert!(item(&same, "User").additional_edits.is_empty());
    let imported = run(
        project(),
        "<?php\nnamespace App\\Http;\nuse App\\Models\\User;\nclass X {\n    function f() { Us$0 }\n}\n",
    );
    assert!(item(&imported, "User").additional_edits.is_empty());
    let global = run(
        Fixture::new(&[("a.php", "<?php\nclass Plain {}\n")]),
        "<?php\nnamespace App;\nclass X {\n    function f() { Pla$0 }\n}\n",
    );
    assert_eq!(item(&global, "Plain").additional_edits.len(), 1);
    assert_eq!(item(&global, "Plain").additional_edits[0].new_text, "\nuse Plain;\n\n");
}

#[test]
fn a_clashing_short_name_is_written_in_full() {
    let list = run(
        project(),
        "<?php\nnamespace App;\nuse Other\\Thing;\nclass X {\n    function f() { Th$0 }\n}\n",
    );
    let thing = item(&list, "Thing");
    assert_eq!(thing.edit.new_text, "\\Vendor\\Pkg\\Thing");
    assert!(thing.additional_edits.is_empty());
}

#[test]
fn filters_class_contexts() {
    let new = run(project(), "<?php\nnew $0\n");
    let _ = new;
    let new = run(project(), "<?php\n$x = new M$0\n");
    assert!(
        !labels(&new).contains(&"Model"),
        "abstract classes cannot be instantiated"
    );
    let new = run(project(), "<?php\n$x = new Us$0\n");
    assert_eq!(
        item(&new, "User").detail.as_deref(),
        Some("(public readonly int $id = 0, string $label = '')")
    );
    let implements = run(project(), "<?php\nclass A implements H$0\n");
    assert_eq!(labels(&implements), vec!["HasName"]);
    let extends = run(project(), "<?php\nclass A extends Se$0\n");
    assert!(
        !labels(&extends).contains(&"Sealed"),
        "final classes cannot be extended"
    );
    let traits = run(project(), "<?php\nclass A { use St$0 }\n");
    assert_eq!(labels(&traits), vec!["Stamps"]);
    let attributes = run(project(), "<?php\n#[Ro$0]\nclass A {}\n");
    assert_eq!(labels(&attributes), vec!["Route"]);
}

#[test]
fn catch_offers_throwables() {
    let fixture = Fixture::with_level(
        php_syntax::PhpVersion::V8_4,
        &[],
        &[(
            "Core.php",
            "<?php\ninterface Throwable {}\nclass Exception implements Throwable {}\nclass Error implements Throwable {}\nclass Elephant {}\n",
        )],
    );
    let list = run(fixture, "<?php\ntry {} catch (E$0) {}\n");
    let names = labels(&list);
    assert!(
        names.contains(&"Exception") && names.contains(&"Error") && !names.contains(&"Elephant"),
        "{names:?}"
    );
}

#[test]
fn completes_variables_in_scope() {
    let list = run(
        project(),
        "<?php\nuse App\\Models\\User;\nfunction f(User $user, int $count) {\n    $local = 1;\n    $u$0\n}\n",
    );
    assert_eq!(item(&list, "$user").detail.as_deref(), Some("User"));
    assert!(!labels(&list).contains(&"$count"));
    let all = run(
        project(),
        "<?php\nfunction f(int $count) {\n    $local = 'x';\n    $$0\n}\n",
    );
    assert!(labels(&all).contains(&"$count") && labels(&all).contains(&"$local"));
    assert_eq!(
        item(&all, "$local").edit.start as usize,
        "<?php\nfunction f(int $count) {\n    $local = 'x';\n    ".len()
    );
}

#[test]
fn completes_functions_and_constants_with_their_namespace() {
    let code = "<?php\nnamespace App\\Http;\nfunction f() {\n    helper_o$0\n}\n";
    let list = run(project(), code);
    let helper = item(&list, "helper_one");
    assert_eq!(helper.kind, ItemKind::Function);
    assert_eq!(helper.description.as_deref(), Some("App\\Support"));
    assert_eq!(helper.detail.as_deref(), Some("(int $a, string $b = 'x'): string"));
    assert!(applied(code, helper).contains("use function App\\Support\\helper_one;"));
    let constants = run(project(), "<?php\nfunction f() {\n    HELPER_L$0\n}\n");
    assert_eq!(item(&constants, "HELPER_LIMIT").kind, ItemKind::Constant);
}

#[test]
fn global_functions_need_no_import() {
    let fixture = Fixture::with_level(
        php_syntax::PhpVersion::V8_4,
        &[],
        &[("standard.php", "<?php\nfunction strlen(string $string): int {}\n")],
    );
    let list = run(fixture, "<?php\nnamespace App;\nfunction f() {\n    strl$0\n}\n");
    assert!(item(&list, "strlen").additional_edits.is_empty());
    assert_eq!(item(&list, "strlen").edit.new_text, "strlen");
}

#[test]
fn offers_named_arguments() {
    let list = run(
        project(),
        "<?php\nuse function App\\Support\\helper_one;\nhelper_one(1, $0);\n",
    );
    let named = item(&list, "b:");
    assert_eq!(named.kind, ItemKind::Parameter);
    assert_eq!(named.edit.new_text, "b: ");
    assert!(!labels(&list).contains(&"a:"));
    let constructor = run(project(), "<?php\nuse App\\Models\\User;\nnew User(la$0);\n");
    assert_eq!(item(&constructor, "label:").detail.as_deref(), Some("string"));
}

#[test]
fn completes_use_statements() {
    let list = run(project(), "<?php\nuse App\\Mo$0\n");
    assert_eq!(item(&list, "Models").kind, ItemKind::Module);
    assert_eq!(item(&list, "Models").edit.new_text, "App\\Models\\");
    let classes = run(project(), "<?php\nuse App\\Models\\Us$0\n");
    assert_eq!(item(&classes, "User").edit.new_text, "App\\Models\\User");
    assert!(item(&classes, "User").additional_edits.is_empty());
    let functions = run(project(), "<?php\nuse function App\\Support\\$0\n");
    assert_eq!(labels(&functions), vec!["helper_one"]);
}

#[test]
fn qualified_names_are_completed_in_place() {
    let list = run(project(), "<?php\n$x = new \\Vendor\\Pkg\\To$0\n");
    assert_eq!(item(&list, "Tool").edit.new_text, "\\Vendor\\Pkg\\Tool");
}

#[test]
fn offers_keywords_by_place() {
    let statement = run(project(), "<?php\nfunction f() {\n    fo$0\n}\n");
    assert!(labels(&statement).contains(&"foreach"));
    let body = run(project(), "<?php\nclass A {\n    pub$0\n}\n");
    assert!(labels(&body).contains(&"public"), "{:?}", labels(&body));
    let enum_body = run(project(), "<?php\nenum E {\n    ca$0\n}\n");
    assert!(labels(&enum_body).contains(&"case"));
    let types = run(project(), "<?php\nfunction f(in$0) {}\n");
    assert!(labels(&types).contains(&"int"));
    let returns = run(project(), "<?php\nfunction f(): vo$0 {}\n");
    assert!(labels(&returns).contains(&"void"));
    let args = run(
        project(),
        "<?php\nfunction f(): int { return 1; }\nfunction g($a) { $a = tr$0 }\n",
    );
    assert!(labels(&args).contains(&"true"));
}

#[test]
fn stays_quiet_in_comments_and_strings() {
    assert!(run(project(), "<?php\n// Us$0\n").items.is_empty());
    assert!(run(project(), "<?php\n/* Us$0 */\n").items.is_empty());
    assert!(run(project(), "<?php\n$x = 'Us$0';\n").items.is_empty());
    assert!(run(project(), "<html>Us$0</html>\n").items.is_empty());
}

#[test]
fn limits_the_list_and_says_so() {
    let mut source = String::from("<?php\n");
    for number in 0..400 {
        source.push_str(&format!("class Item{number} {{}}\n"));
    }
    let list = complete(
        &Fixture::new(&[("many.php", &source)]).index,
        "<?php\nIte",
        9,
        CompletionOptions { limit: 50 },
    );
    assert_eq!(list.items.len(), 50);
    assert!(list.incomplete);
}

fn override_project() -> Fixture {
    Fixture::new(&[(
        "src/Base.php",
        "<?php\nnamespace Lib;\n\ninterface Handler {\n    public function handle(Request $request, ?Handler $next = null, int ...$flags): Response|null;\n}\nclass Request {}\nclass Response {}\nabstract class Base implements Handler {\n    public function __construct(protected string $name = 'x') {}\n    public function describe(int $depth = 1): string { return ''; }\n    public static function make(): static { return new static(); }\n    protected function hook(): void {}\n    private function secret(): void {}\n    final public function sealed(): void {}\n    abstract protected function render(array $data): string;\n}\n",
    )])
}

#[test]
fn completes_the_signature_of_a_method_to_override() {
    let code = "<?php\nnamespace App;\n\nuse Lib\\Base;\n\nclass Page extends Base\n{\n    public function des$0\n}\n";
    let list = run(override_project(), code);
    let describe = item(&list, "describe");
    assert_eq!(describe.kind, ItemKind::Method);
    assert_eq!(describe.description.as_deref(), Some("Base"));
    assert_eq!(
        applied(code, describe),
        "<?php\nnamespace App;\n\nuse Lib\\Base;\n\nclass Page extends Base\n{\n    public function describe(int $depth = 1): string\n    {\n        return parent::describe($depth);\n    }\n}\n"
    );
}

#[test]
fn offers_what_can_be_overridden_and_nothing_else() {
    let code = "<?php\nnamespace App;\n\nuse Lib\\Base;\n\nclass Page extends Base\n{\n    public function hook(): void {}\n    public function $0\n}\n";
    let list = run(override_project(), code);
    let names = labels(&list);
    assert!(names.contains(&"describe") && names.contains(&"make") && names.contains(&"__construct"));
    assert!(names.contains(&"render") && names.contains(&"handle"), "{names:?}");
    assert!(!names.contains(&"hook"), "already declared");
    assert!(
        !names.contains(&"secret") && !names.contains(&"sealed"),
        "private and final stay"
    );
    assert_eq!(names[0], "handle", "what must be implemented comes first: {names:?}");
    assert_eq!(names[1], "render");
}

#[test]
fn imports_the_classes_a_signature_names_and_adds_missing_modifiers() {
    let code = "<?php\nnamespace App;\n\nuse Lib\\Base;\n\nclass Page extends Base\n{\n    function han$0\n}\n";
    let list = run(override_project(), code);
    let handle = item(&list, "handle");
    assert_eq!(
        applied(code, handle),
        "<?php\nnamespace App;\n\nuse Lib\\Base;\nuse Lib\\Handler;\nuse Lib\\Request;\nuse Lib\\Response;\n\nclass Page extends Base\n{\n    public function handle(Request $request, ?Handler $next = null, int ...$flags): ?Response\n    {\n        \n    }\n}\n"
    );
}

#[test]
fn static_and_abstract_modifiers_are_kept_or_added() {
    let code = "<?php\nnamespace App;\n\nuse Lib\\Base;\n\nclass Page extends Base\n{\n    function mak$0\n}\n";
    let list = run(override_project(), code);
    assert!(applied(code, item(&list, "make")).contains("public static function make(): static\n"));
    let code = "<?php\nnamespace App;\n\nuse Lib\\Base;\n\nabstract class Page extends Base\n{\n    abstract protected function ren$0\n}\n";
    let list = run(override_project(), code);
    assert!(applied(code, item(&list, "render")).contains("abstract protected function render(array $data): string;"));
}

#[test]
fn only_the_name_is_completed_when_the_parameters_are_already_there() {
    let code = "<?php\nnamespace App;\n\nuse Lib\\Base;\n\nclass Page extends Base\n{\n    public function des$0(int $a) {}\n}\n";
    let list = run(override_project(), code);
    assert!(applied(code, item(&list, "describe")).contains("public function describe(int $a) {}"));
}

#[test]
fn a_function_outside_a_class_gets_no_override_suggestions() {
    let list = run(override_project(), "<?php\nfunction des$0\n");
    assert!(list.items.is_empty());
}

#[test]
fn a_string_that_starts_a_qualified_name_completes_classes() {
    let fixture = Fixture::new(&[
        (
            "Home.php",
            "<?php\nnamespace App\\Http;\nclass Home {}\ninterface Handler {}\n",
        ),
        ("User.php", "<?php\nnamespace App\\Models;\nclass User {}\n"),
    ]);
    let labels = |code: &str| -> Vec<String> {
        let offset = code.find("$0").expect("a cursor") as u32;
        let text = code.replacen("$0", "", 1);
        complete(&fixture.index, &text, offset, CompletionOptions::default())
            .items
            .into_iter()
            .map(|item| format!("{} {}", item.label, item.edit.new_text))
            .collect()
    };
    assert_eq!(
        labels("<?php\n$a = 'App\\Http\\$0';\n"),
        [
            "App\\Http\\Handler App\\Http\\Handler",
            "App\\Http\\Home App\\Http\\Home"
        ]
    );
    assert_eq!(
        labels("<?php\n$a = \"\\\\App\\\\Mo$0\";\n"),
        ["App\\Models\\User App\\\\Models\\\\User"]
    );
    assert!(
        labels("<?php\n$a = 'App$0';\n")
            .iter()
            .all(|label| !label.starts_with("App\\Http"))
    );
}
