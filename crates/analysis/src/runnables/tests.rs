use super::{RunnableKind, RunnableScope, evaluable, runnables};
use crate::testing::{Fixture, PEST, PHPUNIT};

fn run(code: &str) -> Vec<(RunnableKind, RunnableScope, String, String, String)> {
    let fixture = Fixture::new(&[("phpunit.php", PHPUNIT), ("pest.php", PEST)]).with_current(code);
    let root = php_syntax::parse(code).syntax();
    runnables(&fixture.index, &root)
        .into_iter()
        .map(|runnable| {
            let name = code[usize::from(runnable.range.start())..usize::from(runnable.range.end())].to_string();
            (runnable.kind, runnable.scope, runnable.label, name, runnable.filter)
        })
        .collect()
}

#[test]
fn a_phpunit_class_and_its_tests_are_runnable() {
    let code = r#"<?php
namespace Tests\Unit;
use PHPUnit\Framework\TestCase;
use PHPUnit\Framework\Attributes\Test;
final class FooTest extends TestCase {
    public function testPlain(): void {}
    #[Test] public function marked(): void {}
    /** @test */ public function documented(): void {}
    public function helper(): void {}
    private function testPrivate(): void {}
}
abstract class BaseTest extends TestCase { public function testIt(): void {} }
final class Plain { public function testIt(): void {} }
"#;
    let found = run(code);
    let labels: Vec<(&str, &str, &str)> = found
        .iter()
        .map(|(_, _, label, name, _)| (label.as_str(), name.as_str(), ""))
        .collect();
    assert_eq!(
        labels,
        vec![
            ("FooTest", "FooTest", ""),
            ("FooTest::testPlain", "testPlain", ""),
            ("FooTest::marked", "marked", ""),
            ("FooTest::documented", "documented", ""),
        ]
    );
    assert_eq!(found[0].4, "/^Tests\\\\Unit\\\\FooTest::/");
    assert_eq!(
        found[1].4,
        "/^Tests\\\\Unit\\\\FooTest::testPlain( with data set .*)?$/"
    );
    assert_eq!(found[1].1, RunnableScope::Method);
}

#[test]
fn pest_tests_describe_blocks_and_arch_tests_are_runnable() {
    let code = r#"<?php
it('does x', function () {});
test('plain one', fn () => 1);
describe('outer', function () {
    it('inner works', function () {});
    describe('deeper', function () {
        test('deepest', fn () => 1);
    });
});
arch('no globals')->expect('App')->not->toUse(['dd']);
arch()->preset()->php();
it('uses a dataset')->with('names');
it($dynamic, function () {});
"#;
    let found = run(code);
    let shown: Vec<(&str, &str)> = found
        .iter()
        .map(|(_, _, label, _, filter)| (label.as_str(), filter.as_str()))
        .collect();
    assert_eq!(
        shown,
        vec![
            ("it does x", "/::__pest_evaluable_it_does_x( with data set .*)?$/"),
            ("plain one", "/::__pest_evaluable_plain_one( with data set .*)?$/"),
            ("outer", "/::__pest_evaluable__outer__→_/"),
            (
                "outer → it inner works",
                "/::__pest_evaluable__outer__→_it_inner_works( with data set .*)?$/"
            ),
            ("outer → deeper", "/::__pest_evaluable__outer__→__deeper__→_/"),
            (
                "outer → deeper → deepest",
                "/::__pest_evaluable__outer__→__deeper__→_deepest( with data set .*)?$/"
            ),
            ("no globals", "/::__pest_evaluable_no_globals( with data set .*)?$/"),
            ("arch preset", "/::__pest_evaluable_preset/"),
            (
                "it uses a dataset",
                "/::__pest_evaluable_it_uses_a_dataset( with data set .*)?$/"
            ),
        ]
    );
    assert_eq!(found[0].1, RunnableScope::Test);
    assert_eq!(found[2].1, RunnableScope::Describe);
    assert_eq!(found[6].1, RunnableScope::Arch);
    assert!(found.iter().all(|entry| entry.0 == RunnableKind::Pest));
}

#[test]
fn descriptions_become_method_names_the_way_pest_writes_them() {
    assert_eq!(
        evaluable("it can use_underscores & more"),
        "__pest_evaluable_it_can_use__underscores___more"
    );
    assert_eq!(evaluable("`d` → it x"), "__pest_evaluable__d__→_it_x");
}

#[test]
fn console_commands_are_runnable_in_laravel_and_symfony_projects() {
    use crate::testing::Fixture;
    use php_syntax::parse;

    let mut files = php_index::framework::testing::SYMFONY.to_vec();
    files.extend_from_slice(&[
        (
            "vendor/symfony/Command.php",
            "<?php namespace Symfony\\Component\\Console\\Command; class Command {} namespace Symfony\\Component\\Console\\Attribute; #[\\Attribute] class AsCommand { public function __construct(public string $name) {} }",
        ),
        (
            "vendor/laravel/Command.php",
            "<?php namespace Illuminate\\Console; class Command extends \\Symfony\\Component\\Console\\Command\\Command {}",
        ),
    ]);
    let symfony = "<?php namespace App\\Command; use Symfony\\Component\\Console\\Attribute\\AsCommand; use Symfony\\Component\\Console\\Command\\Command;\n#[AsCommand(name: 'app:report')]\nclass ReportCommand extends Command {}\nabstract class Base extends Command {}";
    let laravel = "<?php namespace App\\Console; use Illuminate\\Console\\Command;\nclass SendEmails extends Command { protected $signature = 'mail:send {user} {--queue}'; }";
    for (code, flavor, expected) in [(symfony, "symfony", "app:report"), (laravel, "laravel", "mail:send")] {
        let mut project = files.clone();
        project.push(("src/Current.php", code));
        let mut fixture = Fixture::framework(&project);
        let index = &mut fixture.index;
        let mut frameworks = index.frameworks();
        frameworks.laravel = flavor == "laravel";
        frameworks.symfony = flavor == "symfony";
        index.set_frameworks(std::path::Path::new("/project"), frameworks);
        let found = runnables(index, &parse(code).syntax());
        assert_eq!(found.len(), 1, "{flavor}: {found:?}");
        assert_eq!(found[0].filter, expected);
        assert_eq!(found[0].scope, RunnableScope::Command);
        assert_eq!(
            found[0].kind,
            if flavor == "laravel" {
                RunnableKind::Artisan
            } else {
                RunnableKind::Console
            }
        );
    }
}
