//! Each action applied to a file gives the file the fix is meant to give.

use php_syntax::{TextRange, parse};

use super::{ActionInput, ActionKind, apply, with_actions};
use crate::inspections::tests::index_with;
use crate::inspections::{Externals, InspectionEnv, InspectionSettings, Override, inspect};

use crate::testing::CURSOR;

pub(super) struct Applied {
    pub title: String,
    pub kind: ActionKind,
    pub preferred: bool,
    pub text: String,
}

/// Every action offered at the cursor of `source`, each applied to the text.
pub(super) fn offered(files: &[(&str, &str)], source: &str, settings: &InspectionSettings) -> Vec<Applied> {
    let offset = source.find(CURSOR).unwrap_or(0);
    let clean = source.replacen(CURSOR, "", 1);
    let index = index_with(files, &clean);
    let tree = parse(&clean);
    let root = tree.syntax();
    let externals = Externals::none();
    let env = InspectionEnv {
        index: &index,
        text: &clean,
        root: &root,
        settings,
        ready: true,
        externals: &externals,
    };
    let findings = inspect(&env);
    let range = TextRange::empty((offset as u32).into());
    let input = ActionInput {
        env: &env,
        range,
        findings: &findings,
    };
    with_actions(&input, |actions| {
        actions
            .iter()
            .map(|action| Applied {
                title: action.title.clone(),
                kind: action.kind,
                preferred: action.preferred,
                text: apply(&clean, &action.edits()),
            })
            .collect()
    })
}

fn all_checks_on() -> InspectionSettings {
    let mut settings = InspectionSettings::default();
    settings.set(
        "missing-strict-types",
        Override {
            enabled: Some(true),
            severity: None,
        },
    );
    settings
}

pub(super) fn applied(files: &[(&str, &str)], source: &str, title: &str) -> String {
    let actions = offered(files, source, &all_checks_on());
    let titles: Vec<&str> = actions.iter().map(|action| action.title.as_str()).collect();
    match actions.iter().find(|action| action.title == title) {
        Some(action) => action.text.clone(),
        None => panic!("no action '{title}' among {titles:?}"),
    }
}

fn titles(files: &[(&str, &str)], source: &str) -> Vec<String> {
    offered(files, source, &InspectionSettings::default())
        .into_iter()
        .map(|action| action.title)
        .collect()
}

#[test]
fn imports_a_class_from_the_only_place_it_lives() {
    let files = [("Tool.php", "<?php\nnamespace Lib;\nclass Tool {}\n")];
    let source =
        "<?php\nnamespace App;\n\nclass A\n{\n    public function f()\n    {\n        return new T$0ool();\n    }\n}\n";
    let actions = offered(&files, source, &InspectionSettings::default());
    let import = actions
        .iter()
        .find(|action| action.title == "Import 'Lib\\Tool'")
        .expect("an import");
    assert!(import.preferred, "the only candidate is the preferred fix");
    assert_eq!(import.kind, ActionKind::QuickFix);
    assert_eq!(
        import.text,
        "<?php\nnamespace App;\n\nuse Lib\\Tool;\n\nclass A\n{\n    public function f()\n    {\n        return new Tool();\n    }\n}\n"
    );
}

#[test]
fn offers_every_candidate_for_an_ambiguous_name() {
    let files = [
        ("A.php", "<?php\nnamespace One;\nclass Thing {}\n"),
        ("B.php", "<?php\nnamespace Two;\nclass Thing {}\n"),
    ];
    let source = "<?php\nnamespace App;\n\nnew Th$0ing();\n";
    let actions = offered(&files, source, &InspectionSettings::default());
    let imports: Vec<&Applied> = actions
        .iter()
        .filter(|action| action.title.starts_with("Import"))
        .collect();
    assert_eq!(imports.len(), 2);
    assert!(imports.iter().all(|action| !action.preferred));
}

#[test]
fn a_global_class_can_be_imported_or_written_in_full() {
    let source = "<?php\nnamespace App;\n\n$e = new Exce$0ption();\n";
    assert_eq!(
        applied(&[], source, "Import 'Exception'"),
        "<?php\nnamespace App;\n\nuse Exception;\n\n$e = new Exception();\n"
    );
    assert_eq!(
        applied(&[], source, "Write '\\Exception' in full"),
        "<?php\nnamespace App;\n\n$e = new \\Exception();\n"
    );
}

#[test]
fn imports_a_function_and_a_constant() {
    let files = [(
        "f.php",
        "<?php\nnamespace Lib;\nfunction helper() {}\nconst LIMIT = 5;\n",
    )];
    let source = "<?php\nnamespace App;\n\nhel$0per();\n";
    assert_eq!(
        applied(&files, source, "Import function 'Lib\\helper'"),
        "<?php\nnamespace App;\n\nuse function Lib\\helper;\n\nhelper();\n"
    );
    let source = "<?php\nnamespace App;\n\necho LIM$0IT;\n";
    assert_eq!(
        applied(&files, source, "Import constant 'Lib\\LIMIT'"),
        "<?php\nnamespace App;\n\nuse const Lib\\LIMIT;\n\necho LIMIT;\n"
    );
}

#[test]
fn removes_an_unused_import_statement_or_clause() {
    let source = "<?php\nnamespace App;\n\nuse Foo\\Used;\nuse Foo\\Un$0used;\n\nnew Used();\n";
    assert_eq!(
        applied(&[], source, "Remove unused import"),
        "<?php\nnamespace App;\n\nuse Foo\\Used;\n\nnew Used();\n"
    );
    let grouped = "<?php\nnamespace App;\n\nuse Foo\\{A, B$0, C};\n\nnew A();\nnew C();\n";
    assert_eq!(
        applied(&[], grouped, "Remove unused import"),
        "<?php\nnamespace App;\n\nuse Foo\\{A, C};\n\nnew A();\nnew C();\n"
    );
    let several = "<?php\nuse A\\One, A\\T$0wo;\n\nnew One();\n";
    assert_eq!(
        applied(&[], several, "Remove unused import"),
        "<?php\nuse A\\One;\n\nnew One();\n"
    );
}

#[test]
fn organizes_imports_sorts_groups_and_drops_what_is_unused() {
    let source = "<?php\nnamespace App;\n\nuse const Foo\\LIMIT;\nuse function Foo\\zeta;\nuse Foo\\Zed;\nuse Foo\\Unused;\nuse Bar\\Alpha;\nuse function Foo\\alpha;\nuse Foo\\{Mid, Beta};\n\nnew Zed(); new Alpha(); new Mid(); new Beta();\nzeta(); alpha();\necho LIMIT;$0\n";
    assert_eq!(
        applied(&[], source, "Organize imports"),
        "<?php\nnamespace App;\n\nuse Bar\\Alpha;\nuse Foo\\{Beta, Mid};\nuse Foo\\Zed;\n\nuse function Foo\\alpha;\nuse function Foo\\zeta;\n\nuse const Foo\\LIMIT;\n\nnew Zed(); new Alpha(); new Mid(); new Beta();\nzeta(); alpha();\necho LIMIT;\n"
    );
}

#[test]
fn imports_with_comments_between_them_are_left_alone() {
    let source = "<?php\nuse B\\Two;\n// keep this\nuse A\\One;\n\nnew One(); new Two();$0\n";
    let found = offered(&[], source, &InspectionSettings::default());
    let organize = found
        .iter()
        .find(|action| action.title == "Organize imports")
        .expect("offered");
    assert_eq!(organize.text, source.replace(CURSOR, ""));
}

#[test]
fn removes_unused_variables_private_members_and_unreachable_code() {
    let source = "<?php\nfunction f() {\n    $un$0used = 5;\n    $kept = g();\n    return $kept;\n}\nfunction g() {}\n";
    assert_eq!(
        applied(&[], source, "Remove unused variable"),
        "<?php\nfunction f() {\n    $kept = g();\n    return $kept;\n}\nfunction g() {}\n"
    );
    let call = "<?php\nfunction f() {\n    $un$0used = g();\n}\nfunction g() {}\n";
    assert_eq!(
        applied(&[], call, "Remove the assignment, keep the call"),
        "<?php\nfunction f() {\n    g();\n}\nfunction g() {}\n"
    );
    let member = "<?php\nclass A\n{\n    public function open() {}\n\n    /** Not used. */\n    private function de$0ad() {}\n}\n";
    assert_eq!(
        applied(&[], member, "Remove unused declaration"),
        "<?php\nclass A\n{\n    public function open() {}\n}\n"
    );
    let dead = "<?php\nfunction f() {\n    return 1;\n    ec$0ho 2;\n    echo 3;\n}\n";
    assert_eq!(
        applied(&[], dead, "Remove unreachable code"),
        "<?php\nfunction f() {\n    return 1;\n}\n"
    );
}

#[test]
fn replaces_an_assignment_in_a_condition() {
    let source = "<?php\nfunction f($a) {\n    if ($b$0 = 5) {}\n}\n";
    assert_eq!(
        applied(&[], source, "Replace '=' with '==='"),
        "<?php\nfunction f($a) {\n    if ($b === 5) {}\n}\n"
    );
}

#[test]
fn removes_a_param_tag_for_a_parameter_that_is_gone() {
    let source = "<?php\n/**\n * Does it.\n * @param int $a\n * @param string $gone$0\n * @return void\n */\nfunction f(int $a): void {}\n";
    assert_eq!(
        applied(&[], source, "Remove the @param tag"),
        "<?php\n/**\n * Does it.\n * @param int $a\n * @return void\n */\nfunction f(int $a): void {}\n"
    );
}

#[test]
fn implements_missing_methods_with_their_signatures() {
    let files = [(
        "S.php",
        "<?php\nnamespace Lib;\ninterface Shape\n{\n    public function area(float $scale = 1.0): float;\n    public function label(?Tag $tag, string ...$rest): string;\n}\nclass Tag {}\n",
    )];
    let source = "<?php\nnamespace App;\n\nuse Lib\\Shape;\n\nclass Sq$0uare implements Shape\n{\n    public function own(): void {}\n}\n";
    assert_eq!(
        applied(&files, source, "Implement the missing methods"),
        "<?php\nnamespace App;\n\nuse Lib\\Shape;\nuse Lib\\Tag;\n\nclass Square implements Shape\n{\n    public function own(): void {}\n\n    public function area(float $scale = 1.0): float\n    {\n        throw new \\LogicException('Not implemented.');\n    }\n\n    public function label(?Tag $tag, string ...$rest): string\n    {\n        throw new \\LogicException('Not implemented.');\n    }\n}\n"
    );
}

#[test]
fn implements_into_an_empty_class_on_one_line() {
    let files = [("S.php", "<?php\ninterface Shape { public function area(): void; }\n")];
    let source = "<?php\nclass Sq$0uare implements Shape {}\n";
    assert_eq!(
        applied(&files, source, "Implement the missing methods"),
        "<?php\nclass Square implements Shape {\n    public function area(): void\n    {\n        // TODO: Implement area() method.\n    }\n}\n"
    );
}

#[test]
fn creates_a_method_and_a_property_on_a_class_of_the_file() {
    let source = "<?php\nclass A\n{\n    public function run(int $count): void\n    {\n        $this->ma$0ke($count, 'x');\n    }\n}\n";
    assert_eq!(
        applied(&[], source, "Create method 'make()'"),
        "<?php\nclass A\n{\n    public function run(int $count): void\n    {\n        $this->make($count, 'x');\n    }\n\n    private function make(int $count, string $arg2): void\n    {\n        // TODO: Implement make() method.\n    }\n}\n"
    );
    let property = "<?php\nclass A\n{\n    private int $n = 1;\n\n    public function run(): void\n    {\n        $this->to$0tal = 5;\n    }\n}\n";
    assert_eq!(
        applied(&[], property, "Create property '$total'"),
        "<?php\nclass A\n{\n    private int $n = 1;\n    private int $total;\n\n    public function run(): void\n    {\n        $this->total = 5;\n    }\n}\n"
    );
}

#[test]
fn adds_strict_types() {
    assert_eq!(
        applied(
            &[],
            "<?php$0\n\nnamespace App;\n\nclass A {}\n",
            "Add declare(strict_types=1)"
        ),
        "<?php\n\ndeclare(strict_types=1);\n\nnamespace App;\n\nclass A {}\n"
    );
}

#[test]
fn changes_the_visibility_of_a_member() {
    let source = "<?php\nclass A\n{\n    private function se$0cret(): void {}\n}\n";
    assert_eq!(
        applied(&[], source, "Make it public"),
        "<?php\nclass A\n{\n    public function secret(): void {}\n}\n"
    );
    let bare = "<?php\nclass A\n{\n    function op$0en(): void {}\n}\n";
    assert_eq!(
        applied(&[], bare, "Make it private"),
        "<?php\nclass A\n{\n    private function open(): void {}\n}\n"
    );
    let found = titles(&[], source);
    assert!(found.contains(&"Make it protected".to_string()));
    assert!(!found.contains(&"Make it private".to_string()));
}

#[test]
fn makes_a_property_readonly_when_only_the_constructor_writes_it() {
    let source = "<?php\nfinal class A\n{\n    private int $pri$0ce;\n    private int $count = 0;\n\n    public function __construct(int $price)\n    {\n        $this->price = $price;\n    }\n\n    public function bump(): void\n    {\n        $this->count++;\n    }\n}\n";
    assert_eq!(
        applied(&[], source, "Make '$price' readonly"),
        "<?php\nfinal class A\n{\n    private readonly int $price;\n    private int $count = 0;\n\n    public function __construct(int $price)\n    {\n        $this->price = $price;\n    }\n\n    public function bump(): void\n    {\n        $this->count++;\n    }\n}\n"
    );
    let changed = source
        .replace("private int $pri$0ce;", "private int $price;")
        .replace("private int $count = 0;", "private int $cou$0nt = 0;");
    assert!(!titles(&[], &changed).contains(&"Make '$count' readonly".to_string()));
}

#[test]
fn promotes_a_constructor_parameter_into_a_property() {
    let source = "<?php\nclass A\n{\n    /** The price. */\n    private int $price;\n    protected ?string $name;\n\n    public function __construct(int $pri$0ce, ?string $name = null)\n    {\n        $this->price = $price;\n        $this->name = $name;\n        $this->boot();\n    }\n}\n";
    assert_eq!(
        applied(&[], source, "Convert to constructor property promotion"),
        "<?php\nclass A\n{\n    protected ?string $name;\n\n    public function __construct(private int $price, ?string $name = null)\n    {\n        $this->name = $name;\n        $this->boot();\n    }\n}\n"
    );
}

#[test]
fn adds_a_return_type_from_what_the_function_returns() {
    let files = [("Box.php", "<?php\nnamespace Lib;\nclass Box {}\n")];
    let source = "<?php\nuse Lib\\Box;\n\nfunction ma$0ke()\n{\n    return new Box();\n}\nfunction nothing() { echo 1; }\nfunction maybe($x) { if ($x) { return 1; } return null; }\nfunction mixed_up($x) { return $x; }\n";
    assert_eq!(
        applied(&files, source, "Add return type 'Box'"),
        "<?php\nuse Lib\\Box;\n\nfunction make(): Box\n{\n    return new Box();\n}\nfunction nothing() { echo 1; }\nfunction maybe($x) { if ($x) { return 1; } return null; }\nfunction mixed_up($x) { return $x; }\n"
    );
    let void = "<?php\nfunction not$0hing() { echo 1; }\n";
    assert_eq!(
        applied(&[], void, "Add return type 'void'"),
        "<?php\nfunction nothing(): void { echo 1; }\n"
    );
    let nullable = "<?php\nfunction may$0be($x) { if ($x) { return 1; } return null; }\n";
    assert_eq!(
        applied(&[], nullable, "Add return type '?int'"),
        "<?php\nfunction maybe($x): ?int { if ($x) { return 1; } return null; }\n"
    );
    let unknown = "<?php\nfunction mixed_$0up($x) { return $x; }\n";
    assert!(
        !titles(&[], unknown)
            .iter()
            .any(|title| title.starts_with("Add return type"))
    );
}

#[test]
fn writes_the_doc_block_a_declaration_lacks() {
    let files = [(
        "Err.php",
        "<?php\nnamespace Lib;\nclass Failure extends \\Exception {}\n",
    )];
    let source = "<?php\nuse Lib\\Failure;\n\nclass A\n{\n    public function ru$0n(int $a, ?string $b = null, ...$rest): bool\n    {\n        if ($a) {\n            throw new Failure();\n        }\n        return true;\n    }\n}\n";
    assert_eq!(
        applied(&files, source, "Add PHPDoc"),
        "<?php\nuse Lib\\Failure;\n\nclass A\n{\n    /**\n     * @param int $a\n     * @param string|null $b\n     * @param mixed ...$rest\n     * @return bool\n     * @throws Failure\n     */\n    public function run(int $a, ?string $b = null, ...$rest): bool\n    {\n        if ($a) {\n            throw new Failure();\n        }\n        return true;\n    }\n}\n"
    );
    let property = "<?php\nclass A\n{\n    private ?int $co$0unt = null;\n}\n";
    assert_eq!(
        applied(&[], property, "Add PHPDoc"),
        "<?php\nclass A\n{\n    /** @var int|null */\n    private ?int $count = null;\n}\n"
    );
}

#[test]
fn updates_a_doc_block_with_the_parameters_it_lacks() {
    let source = "<?php\n/**\n * Does it.\n *\n * @param int $a\n * @param string $c\n * @return void\n */\nfunction f$0(int $a, array $b, string $c, bool $d): void {}\n";
    assert_eq!(
        applied(&[], source, "Update PHPDoc"),
        "<?php\n/**\n * Does it.\n *\n * @param int $a\n * @param array $b\n * @param string $c\n * @param bool $d\n * @return void\n */\nfunction f(int $a, array $b, string $c, bool $d): void {}\n"
    );
}

#[test]
fn actions_follow_the_findings_the_settings_leave_on() {
    let source = "<?php\nuse Foo\\Un$0used;\n";
    let mut settings = InspectionSettings::default();
    settings.set(
        "unused-import",
        Override {
            enabled: Some(false),
            severity: None,
        },
    );
    let found: Vec<String> = offered(&[], source, &settings)
        .into_iter()
        .map(|action| action.title)
        .collect();
    assert!(!found.contains(&"Remove unused import".to_string()));
}
