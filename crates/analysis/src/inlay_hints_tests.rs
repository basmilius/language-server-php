use expect_test::{Expect, expect};
use php_syntax::{PhpVersion, TextRange, TextSize, parse};

use crate::inlay_hints::{HintKind, HintOptions, inlay_hints};
use crate::testing::Fixture;

/// The text with every hint in angle brackets at its place.
fn render(files: &[(&str, &str)], text: &str, range: Option<(u32, u32)>) -> String {
    render_in(Fixture::with_level(PhpVersion::V8_4, files, &[]), text, range)
}

fn render_in(fixture: Fixture, text: &str, range: Option<(u32, u32)>) -> String {
    let fixture = fixture.with_current(text);
    let root = parse(text).syntax();
    let range = range.map(|(start, end)| TextRange::new(TextSize::from(start), TextSize::from(end)));
    let mut out = text.to_string();
    let mut hints = inlay_hints(&fixture.index, &root, range, HintOptions::default());
    hints.sort_by_key(|hint| std::cmp::Reverse(hint.offset));
    for hint in hints {
        let marker = match hint.kind {
            HintKind::Parameter => format!("<{}> ", hint.label),
            HintKind::Type => format!("<{}> ", hint.label),
        };
        out.insert_str(hint.offset as usize, &marker);
    }
    out
}

fn check(files: &[(&str, &str)], text: &str, expected: Expect) {
    expected.assert_eq(&render(files, text, None));
}

const LIB: &str = "<?php\nnamespace Lib;\n\nclass Query {\n    public function __construct(private string $table, private int $limit = 10) {}\n    public function where(string $column, mixed $value, bool $strict = false): static {}\n    public function setActive(bool $active): static {}\n    public function name(string $name): static {}\n    /** @param callable(User, int): void $callback */\n    public function each(callable $callback): void {}\n    /** @param callable(User): bool $callback */\n    public function filter(int $max, callable $callback): void {}\n    public function getTable(): string {}\n}\nclass User {}\nfunction repeat(string $text, int $count): string {}\nfunction join_all(string $glue, string ...$parts): string {}\nfunction pick(int $a, int $b) {}\n";

#[test]
fn shows_the_parameter_an_argument_goes_to() {
    let text = "<?php\nuse Lib\\Query;\nuse function Lib\\repeat;\n$q = new Query('users', 5);\n$q->where('age', 18, true);\nrepeat('a', 3);\n";
    check(
        &[("lib.php", LIB)],
        text,
        expect![[r#"
            <?php
            use Lib\Query;
            use function Lib\repeat;
            $q = new Query(<table:> 'users', <limit:> 5);
            $q->where(<column:> 'age', <value:> 18, <strict:> true);
            repeat(<text:> 'a', <count:> 3);
        "#]],
    );
}

#[test]
fn skips_the_arguments_that_say_it_already() {
    let text = "<?php\nuse Lib\\Query;\nuse function Lib\\repeat;\nfunction f(Query $q, string $text, int $count) {\n    repeat($text, $count);\n    repeat($q->getTable(), $count);\n    $q->where($text, 1);\n    $q->name('x');\n    $q->setActive(true);\n    $q->setActive($count);\n    $q->where(column: 'a', value: 2);\n}\n";
    check(
        &[("lib.php", LIB)],
        text,
        expect![[r#"
            <?php
            use Lib\Query;
            use function Lib\repeat;
            function f(Query $q, string $text, int $count) {
                repeat($text, $count);
                repeat(<text:> $q->getTable(), $count);
                $q->where(<column:> $text, <value:> 1);
                $q->name('x');
                $q->setActive(<active:> true);
                $q->setActive($count);
                $q->where(column: 'a', value: 2);
            }
        "#]],
    );
}

#[test]
fn leaves_variadics_and_one_letter_names_alone() {
    let text = "<?php\nuse function Lib\\join_all;\nuse function Lib\\pick;\njoin_all(',', 'a', 'b');\npick(1, 2);\n";
    check(
        &[("lib.php", LIB)],
        text,
        expect![[r#"
            <?php
            use function Lib\join_all;
            use function Lib\pick;
            join_all(<glue:> ',', 'a', 'b');
            pick(1, 2);
        "#]],
    );
}

#[test]
fn gives_closure_parameters_the_type_the_function_promises() {
    let text = "<?php\nuse Lib\\Query;\nfunction f(Query $q) {\n    $q->each(function ($user, $n) {});\n    $q->filter(5, fn($u) => true);\n    $q->each(fn(Lib\\User $typed) => 1);\n}\n";
    check(
        &[("lib.php", LIB)],
        text,
        expect![[r#"
            <?php
            use Lib\Query;
            function f(Query $q) {
                $q->each(function (<User> $user, <int> $n) {});
                $q->filter(<max:> 5, <callback:> fn(<User> $u) => true);
                $q->each(fn(Lib\User $typed) => 1);
            }
        "#]],
    );
}

#[test]
fn only_hints_the_calls_in_a_range() {
    let text = "<?php\nuse function Lib\\repeat;\nrepeat('a', 1);\nrepeat('b', 2);\n";
    let start = text.find("repeat('b'").unwrap() as u32;
    let found = render(&[("lib.php", LIB)], text, Some((start, text.len() as u32)));
    assert_eq!(
        found,
        "<?php\nuse function Lib\\repeat;\nrepeat('a', 1);\nrepeat(<text:> 'b', <count:> 2);\n"
    );
}

#[test]
fn shows_the_model_a_raxos_relation_gives_its_list() {
    let raxos = "<?php\nnamespace Raxos\\Database\\Orm;\n/**\n * @template TKey of array-key\n * @template TValue\n */\nclass ModelArrayList {}\nclass Model {}\nnamespace Raxos\\Database\\Orm\\Attribute;\n#[\\Attribute] class HasMany { public function __construct(public string $referenceModel) {} }\n";
    let text = "<?php\nnamespace App;\nuse Raxos\\Database\\Orm\\{Model, ModelArrayList};\nuse Raxos\\Database\\Orm\\Attribute\\HasMany;\nclass Scan extends Model {}\nclass AppTeam extends Model {\n    #[HasMany(Scan::class)]\n    public ModelArrayList $scans;\n    /** @var ModelArrayList<int, Scan> */\n    #[HasMany(Scan::class)]\n    public ModelArrayList $documented;\n}\n";
    let fixture = Fixture::framework(&[("vendor/raxos/database/Orm.php", raxos)]);
    expect![[r#"
        <?php
        namespace App;
        use Raxos\Database\Orm\{Model, ModelArrayList};
        use Raxos\Database\Orm\Attribute\HasMany;
        class Scan extends Model {}
        class AppTeam extends Model {
            #[HasMany(Scan::class)]
            public ModelArrayList<<int, Scan>>  $scans;
            /** @var ModelArrayList<int, Scan> */
            #[HasMany(Scan::class)]
            public ModelArrayList $documented;
        }
    "#]]
    .assert_eq(&render_in(fixture, text, None));
}
