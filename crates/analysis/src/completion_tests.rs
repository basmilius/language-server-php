use crate::completion::{
    CallInsert, CompletionItem, CompletionList, CompletionOptions, ItemKind, complete, escape_snippet,
};
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
    let (offset, text) = lsc_text::testing::cursor(code);

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
    assert_eq!(labels(&enum_list)[..2], ["Admin", "Guest"], "the cases come first");
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
        CompletionOptions {
            limit: 50,
            ..CompletionOptions::default()
        },
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
        let (offset, text) = lsc_text::testing::cursor(code);

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

fn tickets() -> Fixture {
    Fixture::new(&[(
        "src/Ticket.php",
        r#"<?php
namespace App\Data;

enum ScanStatus: string { case Pending = 'pending'; case Checkin = 'checkin'; public static function fallback(): self { return self::Pending; } }

class Ticket {
    public ScanStatus $status;
    public ?ScanStatus $previous = null;
    public bool $valid = true;
    public function mark(ScanStatus $status, bool $notify = false): void {}

    /**
     * @return array{
     *     current: array<array{
     *         date: string,
     *         value: float
     *     }>,
     *     previous: array<array{
     *         date: string,
     *         value: float
     *     }>,
     *     total?: int
     * }
     * @throws \RuntimeException
     */
    public static function chart(): array { return []; }
}
"#,
    )])
}

#[test]
fn a_comparison_with_an_enum_offers_its_cases_first() {
    let code = "<?php\nnamespace App\\Http;\nuse App\\Data\\Ticket;\nfunction f(Ticket $t) {\n    if ($t->status === $0) {}\n}\n";
    let list = run(tickets(), code);
    let names = labels(&list);
    assert_eq!(names[..2], ["ScanStatus::Pending", "ScanStatus::Checkin"], "{names:?}");
    let pending = item(&list, "ScanStatus::Pending");
    assert_eq!(pending.kind, ItemKind::EnumMember);
    assert_eq!(pending.detail.as_deref(), Some("= 'pending'"));
    assert_eq!(
        applied(code, pending),
        "<?php\nnamespace App\\Http;\nuse App\\Data\\ScanStatus;\nuse App\\Data\\Ticket;\nfunction f(Ticket $t) {\n    if ($t->status === ScanStatus::Pending) {}\n}\n"
    );
    assert!(
        !names.contains(&"Ticket"),
        "a comparison is not an instanceof: {names:?}"
    );

    let typed = run(
        tickets(),
        "<?php\nuse App\\Data\\Ticket;\nfunction f(Ticket $t) {\n    if ($t->status !== Che$0) {}\n}\n",
    );

    assert_eq!(labels(&typed).first(), Some(&"ScanStatus::Checkin"));
}

#[test]
fn instanceof_still_offers_classes() {
    let list = run(
        tickets(),
        "<?php\nuse App\\Data\\Ticket;\nfunction f($t) {\n    if ($t instanceof $0) {}\n}\n",
    );

    assert!(labels(&list).contains(&"Ticket"), "{:?}", labels(&list));
}

#[test]
fn assignments_arguments_returns_and_match_expect_their_type() {
    let first = |code: &str| {
        let list = run(tickets(), code);

        labels(&list).first().map(|label| label.to_string())
    };

    let pending = Some("ScanStatus::Pending".to_string());

    assert_eq!(
        first("<?php\nuse App\\Data\\Ticket;\nfunction f(Ticket $t) {\n    $t->previous = $0\n}\n"),
        pending
    );
    assert_eq!(
        first("<?php\nuse App\\Data\\Ticket;\nfunction f(Ticket $t) {\n    $t->mark($0);\n}\n"),
        pending
    );
    assert_eq!(
        first("<?php\nuse App\\Data\\Ticket;\nfunction f(Ticket $t) {\n    $t->mark(notify: true, status: $0);\n}\n"),
        pending
    );
    assert_eq!(
        first("<?php\nuse App\\Data\\ScanStatus;\nfunction f(): ScanStatus {\n    return $0\n}\n"),
        pending
    );
    assert_eq!(
        first(
            "<?php\nuse App\\Data\\Ticket;\nfunction f(Ticket $t) {\n    return match ($t->status) {\n        $0\n    };\n}\n"
        ),
        pending
    );
    assert_eq!(
        first(
            "<?php\nuse App\\Data\\Ticket;\nfunction f(Ticket $t) {\n    switch ($t->status) {\n        case $0\n    }\n}\n"
        ),
        pending
    );

    let nullable = run(
        tickets(),
        "<?php\nuse App\\Data\\Ticket;\nfunction f(Ticket $t) {\n    $t->previous = $0\n}\n",
    );

    assert_eq!(labels(&nullable)[2], "null", "{:?}", labels(&nullable));
    assert_eq!(
        first(
            "<?php\nuse App\\Data\\Ticket;\nfunction f(Ticket $t) {\n    $t->mark(App\\Data\\ScanStatus::Pending, $0);\n}\n"
        ),
        Some("false".to_string())
    );
}

#[test]
fn completes_the_keys_of_a_shape() {
    let quoted = run(
        tickets(),
        "<?php\nuse App\\Data\\Ticket;\nfunction f() {\n    Ticket::chart()['$0'];\n}\n",
    );

    assert_eq!(labels(&quoted), vec!["current", "previous", "total"]);
    assert_eq!(item(&quoted, "total").description.as_deref(), Some("optional"));
    let nested_code = "<?php\nuse App\\Data\\Ticket;\nfunction f() {\n    $chart = Ticket::chart();\n    $chart['current'][0]['va$0'];\n}\n";
    let nested = run(tickets(), nested_code);
    assert_eq!(labels(&nested), vec!["value"]);
    assert_eq!(item(&nested, "value").detail.as_deref(), Some("float"));
    assert_eq!(
        applied(nested_code, item(&nested, "value")),
        nested_code.replace("['va$0']", "['value']")
    );

    let bare_code = "<?php\nuse App\\Data\\Ticket;\nfunction f() {\n    Ticket::chart()[$0];\n}\n";
    let bare = run(tickets(), bare_code);
    assert_eq!(labels(&bare)[..3], ["current", "previous", "total"]);
    assert_eq!(
        applied(bare_code, item(&bare, "current")),
        bare_code.replace("[$0]", "['current']")
    );
}

#[test]
fn completes_named_arguments_of_an_attribute() {
    let list = run(
        project(),
        "<?php\nuse App\\Models\\Route;\n#[Route(pa$0)]\nfunction f() {}\n",
    );

    assert_eq!(labels(&list).first(), Some(&"path:"), "{:?}", labels(&list));
}

fn calls() -> Fixture {
    Fixture::with_level(
        php_syntax::PhpVersion::V8_4,
        &[(
            "src/Calls.php",
            r#"<?php
namespace App;

function plain(int $first, string $second): void {}
function with_default(int $first, string $second = 'x'): void {}
function only_optional(int $first = 1, string ...$rest): void {}
function nothing(): void {}
function by_reference(array &$items, $flag): void {}

class Service
{
    public function __construct(private int $id, private string $label = '') {}
    public function run(int $times, callable ...$hooks): void {}
    public static function make(string $name, int $retries = 3): static { return new static(1); }
    public function reset(): void {}
}
class Bare {}
abstract class Base { public function __construct(string $a) {} }
"#,
        )],
        &[(
            "standard.php",
            "<?php\nfunction sort(array &$array, int $flags = 0): bool {}\nfunction printf(string $format, mixed ...$values): int {}\nfunction time(): int {}\n",
        )],
    )
}

fn run_calls(code: &str, call_snippets: bool) -> CompletionList {
    let fixture = calls().with_current(code);
    let (offset, text) = lsc_text::testing::cursor(code);

    complete(
        &fixture.index,
        &text,
        offset,
        CompletionOptions {
            call_snippets,
            ..CompletionOptions::default()
        },
    )
}

fn call_text(code: &str, label: &str) -> (String, CallInsert) {
    let list = run_calls(code, true);
    let found = item(&list, label);

    (found.edit.new_text.clone(), found.call)
}

#[test]
fn a_function_call_writes_its_required_arguments_as_placeholders() {
    let prefix = "<?php\nnamespace App;\nfunction f() {\n    ";
    let suffix = "$0\n}\n";
    let at = |label: &str| call_text(&format!("{prefix}{label}{suffix}"), label);

    assert_eq!(
        at("plain"),
        (
            "plain(${1:\\$first}, ${2:\\$second})$0".into(),
            CallInsert::SnippetWithParameters
        )
    );
    assert_eq!(
        at("with_default"),
        (
            "with_default(${1:\\$first})$0".into(),
            CallInsert::SnippetWithParameters
        )
    );
    assert_eq!(
        at("only_optional"),
        ("only_optional($1)$0".into(), CallInsert::SnippetWithParameters)
    );
    assert_eq!(at("nothing"), ("nothing()$0".into(), CallInsert::Snippet));
    assert_eq!(
        at("by_reference"),
        (
            "by_reference(${1:\\$items}, ${2:\\$flag})$0".into(),
            CallInsert::SnippetWithParameters
        )
    );
    assert_eq!(
        at("printf"),
        ("printf(${1:\\$format})$0".into(), CallInsert::SnippetWithParameters)
    );
    assert_eq!(
        at("sort"),
        ("sort(${1:\\$array})$0".into(), CallInsert::SnippetWithParameters)
    );
    assert_eq!(at("time"), ("time()$0".into(), CallInsert::Snippet));
}

#[test]
fn a_function_that_needs_an_import_keeps_the_import_and_gets_arguments() {
    let list = run_calls("<?php\nfunction f() {\n    plai$0\n}\n", true);
    let found = item(&list, "plain");
    assert_eq!(found.edit.new_text, "plain(${1:\\$first}, ${2:\\$second})$0");
    assert!(!found.additional_edits.is_empty());
}

#[test]
fn snippet_text_escapes_what_the_grammar_reserves() {
    assert_eq!(escape_snippet("a$b}c\\d"), "a\\$b\\}c\\\\d");
}

#[test]
fn methods_get_arguments_after_arrows_and_scope_operators() {
    let instance = "<?php\nnamespace App;\nfunction f(Service $service) {\n    $service->";

    assert_eq!(
        call_text(&format!("{instance}ru$0\n}}\n"), "run").0,
        "run(${1:\\$times})$0"
    );
    assert_eq!(
        call_text(&format!("{instance}res$0\n}}\n"), "reset"),
        ("reset()$0".into(), CallInsert::Snippet)
    );

    let nullsafe = "<?php\nnamespace App;\nfunction f(?Service $service) {\n    $service?->ru$0\n}\n";
    assert_eq!(call_text(nullsafe, "run").0, "run(${1:\\$times})$0");
    let scoped = "<?php\nnamespace App;\nfunction f() {\n    Service::ma$0\n}\n";
    assert_eq!(call_text(scoped, "make").0, "make(${1:\\$name})$0");
}

#[test]
fn a_constructor_gets_its_arguments_after_new() {
    let new = |name: &str| {
        call_text(
            &format!("<?php\nnamespace App;\nfunction f() {{\n    new {name}$0\n}}\n"),
            name,
        )
    };

    assert_eq!(
        new("Service"),
        ("Service(${1:\\$id})$0".into(), CallInsert::SnippetWithParameters)
    );
    assert_eq!(new("Bare"), ("Bare()$0".into(), CallInsert::Snippet));
    let qualified = call_text("<?php\nfunction f() {\n    new \\App\\Serv$0\n}\n", "Service");
    assert_eq!(qualified.0, "\\\\App\\\\Service(${1:\\$id})$0");
    let list = run_calls("<?php\nnamespace App;\nfunction f() {\n    new Ba$0\n}\n", true);

    assert!(
        !labels(&list).contains(&"Base"),
        "abstract classes are not offered after new"
    );
}

#[test]
fn nothing_is_added_when_parentheses_follow_the_cursor() {
    for after in ["(", "  (", "\n    ("] {
        let code =
            format!("<?php\nnamespace App;\nfunction f(Service $service) {{\n    $service->ru$0{after}1);\n}}\n");

        let list = run_calls(&code, true);
        assert_eq!(item(&list, "run").edit.new_text, "run");
        assert_eq!(item(&list, "run").call, CallInsert::None);
    }

    let function = run_calls("<?php\nnamespace App;\nfunction f() {\n    plai$0(...);\n}\n", true);
    assert_eq!(item(&function, "plain").edit.new_text, "plain");
}

#[test]
fn names_that_are_not_calls_stay_bare() {
    let cases = [
        ("<?php\nuse function App\\plai$0\n", "plain"),
        ("<?php\nnamespace App;\n#[Servi$0]\nclass X {}\n", "Service"),
        (
            "<?php\nnamespace App;\nfunction f() {\n    $x = Servi$0::class;\n}\n",
            "Service",
        ),
        ("<?php\nnamespace App;\nfunction f(Servi$0 $x) {}\n", "Service"),
        ("<?php\nnamespace App;\nclass X extends Servi$0 {}\n", "Service"),
    ];

    for (code, _) in cases {
        let list = run_calls(code, true);

        for found in &list.items {
            assert_eq!(found.call, CallInsert::None, "{code}: {}", found.label);
            assert!(!found.edit.new_text.contains('('), "{code}: {}", found.edit.new_text);
        }
    }

    let string = run_calls(
        "<?php\nnamespace App;\nfunction f() {\n    function_exists('plai$0');\n}\n",
        true,
    );

    assert!(string.items.is_empty(), "{:?}", labels(&string));
}

#[test]
fn nothing_changes_without_the_option() {
    let list = run_calls("<?php\nnamespace App;\nfunction f() {\n    plai$0\n}\n", false);
    let found = item(&list, "plain");
    assert_eq!(found.edit.new_text, "plain");
    assert_eq!(found.call, CallInsert::None);

    let method = run_calls(
        "<?php\nnamespace App;\nfunction f(Service $s) {\n    $s->ru$0\n}\n",
        false,
    );

    assert_eq!(item(&method, "run").edit.new_text, "run");
}

#[test]
fn label_filter_and_sort_text_stay_the_same() {
    let code = "<?php\nnamespace App;\nfunction f() {\n    plai$0\n}\n";
    let on = run_calls(code, true);
    let off = run_calls(code, false);
    let (on, off) = (item(&on, "plain"), item(&off, "plain"));

    assert_eq!(
        (&on.label, &on.filter_text, &on.sort_text),
        (&off.label, &off.filter_text, &off.sort_text)
    );
}

#[test]
fn no_call_is_added_when_an_identifier_character_follows_the_cursor() {
    for after in ["tFoo($a);", "_x;", "9;"] {
        let code = format!("<?php\nnamespace App;\nfunction f(Service $service) {{\n    $service->ru$0{after}\n}}\n");
        let list = run_calls(&code, true);
        assert_eq!(item(&list, "run").edit.new_text, "run", "{after}");
        assert_eq!(item(&list, "run").call, CallInsert::None, "{after}");
    }
}

#[test]
fn simple_interpolation_gets_no_call_but_braced_interpolation_does() {
    let simple = "<?php\nnamespace App;\nfunction f(Service $service) {\n    echo \"$service->ru$0\";\n}\n";
    assert_eq!(item(&run_calls(simple, true), "run").call, CallInsert::None);
    assert_eq!(item(&run_calls(simple, true), "run").edit.new_text, "run");
    let heredoc = "<?php\nnamespace App;\nfunction f(Service $service) {\n    echo <<<TXT\n$service->ru$0\nTXT;\n}\n";
    assert_eq!(item(&run_calls(heredoc, true), "run").call, CallInsert::None);
    let braced = "<?php\nnamespace App;\nfunction f(Service $service) {\n    echo \"{$service->ru$0}\";\n}\n";

    assert_eq!(
        item(&run_calls(braced, true), "run").edit.new_text,
        "run(${1:\\$times})$0"
    );
}

fn sort_text_of(list: &CompletionList, detail_namespace: &str, label: &str) -> String {
    list.items
        .iter()
        .find(|item| item.label == label && item.description.as_deref() == Some(detail_namespace))
        .unwrap_or_else(|| panic!("no {label} in {detail_namespace}"))
        .sort_text
        .clone()
}

const PROJECT_ROOT: &str = "/project";

fn with_test_declarations() -> Fixture {
    Fixture::new(&[
        (
            "src/Thing.php",
            "<?php\nnamespace App;\nclass Thing { public function run(): void {} }\nclass ThingBar {}\nfunction make_thing(): void {}\nconst THING_MAX = 1;\n",
        ),
        (
            "tests/Thing.php",
            "<?php\nnamespace App\\Tests;\nclass Thing { public function run(): void {} }\nfunction make_thing(): void {}\nconst THING_MAX = 1;\n",
        ),
    ])
}

fn ranked_in(root: &str, code: &str, limit: usize) -> CompletionList {
    let fixture = with_test_declarations().with_current(code);
    let (offset, text) = lsc_text::testing::cursor(code);

    let options = CompletionOptions {
        limit,
        ..CompletionOptions::default()
    };

    crate::completion::complete_in(&fixture.index, Some(std::path::Path::new(root)), &text, offset, options)
}

fn ranked(code: &str) -> CompletionList {
    ranked_in(PROJECT_ROOT, code, 300)
}

#[test]
fn a_class_in_a_test_file_sorts_after_every_production_class_of_its_group() {
    let list = ranked("<?php\nnew Thi$0");
    let production = sort_text_of(&list, "App", "Thing");
    let longer = sort_text_of(&list, "App", "ThingBar");
    let tests = sort_text_of(&list, "App\\Tests", "Thing");
    assert!(production < longer && longer < tests, "{production} {longer} {tests}");
}

#[test]
fn production_items_keep_their_place_when_the_limit_cuts() {
    let list = ranked_in(PROJECT_ROOT, "<?php\nnew Thing$0", 1);
    assert_eq!(list.items.len(), 1);
    assert_eq!(list.items[0].description.as_deref(), Some("App"));
    assert!(list.incomplete);
}

#[test]
fn functions_and_constants_in_test_files_sort_after_production_ones() {
    let list = ranked("<?php\nmake_th$0");
    assert!(sort_text_of(&list, "App", "make_thing") < sort_text_of(&list, "App\\Tests", "make_thing"));
    let list = ranked("<?php\nTHING_M$0");
    assert!(sort_text_of(&list, "App", "THING_MAX") < sort_text_of(&list, "App\\Tests", "THING_MAX"));
}

#[test]
fn members_declared_in_a_test_class_sort_after_the_same_member_of_production() {
    let code = "<?php\nnamespace App;\nfunction f(\\App\\Tests\\Thing $thing) { $thing->ru$0 }\n";
    let as_test = item(&ranked(code), "run").sort_text.clone();
    let as_production = item(&ranked_in("/project/tests", code, 300), "run").sort_text.clone();
    assert!(as_production < as_test, "{as_production} {as_test}");
}

#[test]
fn ranking_without_a_root_changes_nothing() {
    let code = "<?php\nnew Thi$0";
    let fixture = with_test_declarations().with_current(code);
    let (offset, text) = lsc_text::testing::cursor(code);
    let plain = complete(&fixture.index, &text, offset, CompletionOptions::default());
    let rootless = crate::completion::complete_in(&fixture.index, None, &text, offset, CompletionOptions::default());
    assert_eq!(plain, rootless);
}

#[test]
fn offers_nothing_while_the_open_tag_is_typed() {
    for code in ["<?$0", "<?p$0", "<?ph$0", "<?php$0", "<?=$0", "<?PHP$0"] {
        let list = run(project(), code);
        assert!(list.items.is_empty(), "{code}: {:?}", labels(&list));
    }
}

#[test]
fn offers_nothing_in_markup_around_the_tags() {
    for code in [
        "$0<html>",
        "<html>php$0</html>\n<?php\n",
        "<p>$0</p><?php echo 1; ?>",
        "<?php echo 1; ?>$0",
        "<?php echo 1; ?>\nphp$0",
    ] {
        let list = run(project(), code);
        assert!(list.items.is_empty(), "{code}: {:?}", labels(&list));
    }
}

#[test]
fn completes_after_the_open_tag_and_its_whitespace() {
    let list = run(project(), "<?php\nec$0");
    assert!(labels(&list).contains(&"echo"), "{:?}", labels(&list));

    let list = run(project(), "<?php ec$0");
    assert!(labels(&list).contains(&"echo"), "{:?}", labels(&list));

    let list = run(project(), "<?php echo 1; ec$0 ?>");
    assert!(labels(&list).contains(&"echo"), "{:?}", labels(&list));
}

fn declare_edits(list: &CompletionList) -> Vec<(&str, &str)> {
    list.items
        .iter()
        .map(|item| (item.label.as_str(), item.edit.new_text.as_str()))
        .collect()
}

#[test]
fn declare_inserts_its_parentheses_with_a_caret_stop() {
    let list = run(project(), "<?php\necho 1;\ndecla$0");
    let declare = item(&list, "declare");

    assert_eq!(declare.edit.new_text, "declare($1);");
    assert_eq!(declare.call, CallInsert::Snippet);
    assert!(!labels(&list).contains(&"declare(strict_types=1);"));
}

#[test]
fn the_strict_types_line_comes_first_right_after_the_open_tag() {
    let list = run(project(), "<?php\ndecla$0");
    let strict = item(&list, "declare(strict_types=1);");

    assert_eq!(strict.edit.new_text, "declare(strict_types=1);");
    assert_eq!(strict.call, CallInsert::None);
    assert_eq!(labels(&list)[0], "declare(strict_types=1);");
    assert_eq!(
        applied("<?php\ndecla$0", strict).replace("$0", ""),
        "<?php\ndeclare(strict_types=1);"
    );
}

#[test]
fn the_strict_types_line_is_not_offered_twice_or_inside_a_function() {
    let twice = run(project(), "<?php\ndeclare(strict_types=1);\ndecla$0");
    assert!(!labels(&twice).contains(&"declare(strict_types=1);"));

    let later = run(project(), "<?php\nnamespace App;\ndecla$0");
    assert!(!labels(&later).contains(&"declare(strict_types=1);"));

    let inside = run(project(), "<?php\nfunction f() {\n    decla$0\n}\n");
    assert!(!labels(&inside).contains(&"declare(strict_types=1);"));
    assert!(!labels(&inside).contains(&"declare"));
}

#[test]
fn declare_offers_its_directives() {
    let list = run(project(), "<?php\ndeclare($0);");

    assert_eq!(
        declare_edits(&list),
        [
            ("encoding", "encoding='$1'"),
            ("strict_types", "strict_types=1"),
            ("ticks", "ticks=$1"),
        ]
    );

    assert_eq!(item(&list, "ticks").call, CallInsert::Snippet);
    assert_eq!(
        declare_edits(&run(project(), "<?php\ndeclare(str$0);")),
        [("strict_types", "strict_types=1")]
    );
}

#[test]
fn strict_types_offers_one_and_zero() {
    let list = run(project(), "<?php\ndeclare(strict_types=$0);");

    assert_eq!(declare_edits(&list), [("0", "0"), ("1", "1")]);
    assert!(run(project(), "<?php\ndeclare(ticks=$0);").items.is_empty());
}
