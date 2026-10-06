use std::path::PathBuf;

use crate::LineIndex;
use crate::infer::Analyzer;
use crate::references::{Current, references_at};
use crate::rename::{prepare_rename, rename};
use crate::testing::{Files, Fixture, PHPUNIT, split_cursor};

const PROVIDERS: &str = r#"<?php
namespace Tests\Support;

final class Providers {
    public static function numbers(): iterable { return [[1]]; }
}
"#;

fn fixture(current: &str) -> Fixture {
    Fixture::new(&[("phpunit.php", PHPUNIT), ("Providers.php", PROVIDERS)]).with_current(current)
}

/// The names the definitions at the cursor point at.
fn definitions(code: &str) -> Vec<String> {
    let fixture = fixture(code);
    let (text, root, offset) = split_cursor(code);
    let analyzer = Analyzer::new(&fixture.index, &root, offset);
    analyzer
        .definitions(offset)
        .into_iter()
        .map(|place| {
            let path = place.path.unwrap_or_else(|| PathBuf::from("/project/current.php"));
            let source = fixture.sources.get(&path).unwrap_or(&text);
            let file = path.file_name().unwrap().to_string_lossy().into_owned();
            format!("{file} {}", &source[place.span.start as usize..place.span.end as usize])
        })
        .collect()
}

#[test]
fn a_data_provider_attribute_leads_to_its_method() {
    let code = r#"<?php
namespace Tests;
use PHPUnit\Framework\TestCase;
use PHPUnit\Framework\Attributes\DataProvider;
final class FooTest extends TestCase {
    #[DataProvider('prov$0ide')]
    public function testIt(int $a): void {}
    public static function provide(): iterable { return []; }
}
"#;
    assert_eq!(definitions(code), vec!["current.php provide"]);
}

#[test]
fn an_external_provider_is_found_on_the_class_it_names() {
    let code = r#"<?php
namespace Tests;
use PHPUnit\Framework\TestCase;
use PHPUnit\Framework\Attributes\DataProviderExternal;
use Tests\Support\Providers;
final class FooTest extends TestCase {
    #[DataProviderExternal(Providers::class, 'num$0bers')]
    public function testIt(int $a): void {}
}
"#;
    assert_eq!(definitions(code), vec!["Providers.php numbers"]);
}

#[test]
fn tags_in_a_doc_comment_lead_to_methods_too() {
    let code = r#"<?php
namespace Tests;
use PHPUnit\Framework\TestCase;
final class FooTest extends TestCase {
    public function testFirst(): void {}
    /**
     * @depends testFirst
     * @dataProvider pro$0vide
     */
    public function testIt(int $a): void {}
    public static function provide(): iterable { return []; }
}
"#;
    assert_eq!(definitions(code), vec!["current.php provide"]);
    let depends = code
        .replace("pro$0vide", "provide")
        .replace("testFirst\n", "test$0First\n");
    assert_eq!(definitions(&depends), vec!["current.php testFirst"]);
}

#[test]
fn a_depends_attribute_leads_to_the_test_it_needs() {
    let code = r#"<?php
namespace Tests;
use PHPUnit\Framework\TestCase;
use PHPUnit\Framework\Attributes\Depends;
final class FooTest extends TestCase {
    public function testFirst(): void {}
    #[Depends('testFi$0rst')]
    public function testSecond(): void {}
}
"#;
    assert_eq!(definitions(code), vec!["current.php testFirst"]);
}

fn usages(code: &str) -> Vec<String> {
    let fixture = fixture(code);
    let (text, root, offset) = split_cursor(code);
    let sources = Files(fixture.sources.clone());
    let path = PathBuf::from("/project/current.php");
    let Some(found) = references_at(
        &fixture.index,
        &sources,
        &Current {
            path: &path,
            text: &text,
            root: &root,
        },
        offset,
    ) else {
        return vec!["nothing".to_string()];
    };
    let mut out = Vec::new();
    for file in &found.files {
        let source = if file.path == path {
            text.clone()
        } else {
            fixture.sources[&file.path].clone()
        };
        let lines = LineIndex::new(&source);
        for hit in &file.hits {
            let line = lines
                .line_col(&source, hit.range.start(), crate::PositionEncoding::Utf16)
                .line;
            out.push(format!(
                "{}:{} {}",
                file.path.file_name().unwrap().to_string_lossy(),
                line + 1,
                &source[usize::from(hit.range.start())..usize::from(hit.range.end())]
            ));
        }
    }
    out
}

#[test]
fn usages_of_a_provider_include_the_strings_that_name_it() {
    let code = r#"<?php
namespace Tests;
use PHPUnit\Framework\TestCase;
use PHPUnit\Framework\Attributes\DataProvider;
final class FooTest extends TestCase {
    #[DataProvider('provide')]
    public function testA(int $a): void {}
    /** @dataProvider provide */
    public function testB(int $a): void {}
    public static function pro$0vide(): iterable { return []; }
}
"#;
    assert_eq!(
        usages(code),
        vec![
            "current.php:6 provide",
            "current.php:8 provide",
            "current.php:10 provide"
        ]
    );
}

#[test]
fn a_provider_is_found_from_the_string_in_another_class() {
    let code = r#"<?php
namespace Tests;
use PHPUnit\Framework\TestCase;
use PHPUnit\Framework\Attributes\DataProviderExternal;
use Tests\Support\Providers;
final class FooTest extends TestCase {
    #[DataProviderExternal(Providers::class, 'num$0bers')]
    public function testIt(int $a): void {}
}
"#;
    assert_eq!(usages(code), vec!["Providers.php:5 numbers", "current.php:7 numbers"]);
}

#[test]
fn renaming_a_provider_follows_the_strings() {
    let code = "<?php\nnamespace Tests;\nuse PHPUnit\\Framework\\TestCase;\nuse PHPUnit\\Framework\\Attributes\\DataProvider;\nfinal class FooTest extends TestCase {\n    #[DataProvider('pro$0vide')]\n    public function testA(int $a): void {}\n    /** @dataProvider provide */\n    public function testB(int $a): void {}\n    public static function provide(): iterable { return []; }\n}\n";
    let fixture = fixture(code);
    let (text, root, offset) = split_cursor(code);
    let prepared = prepare_rename(&fixture.index, &root, &text, offset).expect("renamable");
    assert_eq!(prepared.placeholder, "provide");
    let sources = Files(fixture.sources.clone());
    let path = PathBuf::from("/project/current.php");
    let done = rename(
        &fixture.index,
        &sources,
        &Current {
            path: &path,
            text: &text,
            root: &root,
        },
        offset,
        "rows",
    )
    .expect("renamed");
    let mut edits = done.files[0].edits.clone();
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.range.start()));
    let mut renamed = text.clone();
    for edit in edits {
        renamed.replace_range(
            usize::from(edit.range.start())..usize::from(edit.range.end()),
            &edit.text,
        );
    }
    assert_eq!(
        renamed,
        "<?php\nnamespace Tests;\nuse PHPUnit\\Framework\\TestCase;\nuse PHPUnit\\Framework\\Attributes\\DataProvider;\nfinal class FooTest extends TestCase {\n    #[DataProvider('rows')]\n    public function testA(int $a): void {}\n    /** @dataProvider rows */\n    public function testB(int $a): void {}\n    public static function rows(): iterable { return []; }\n}\n"
    );
}

#[test]
fn test_classes_and_methods_are_recognized() {
    let code = r#"<?php
namespace Tests;
use PHPUnit\Framework\TestCase;
use PHPUnit\Framework\Attributes\Test;
abstract class Base extends TestCase {}
final class FooTest extends Base {
    public function testPlain(): void {}
    #[Test] public function marked(): void {}
    /** @test */ public function documented(): void {}
    public function helper(): void {}
    protected function testProtected(): void {}
    public static function testStatic(): void {}
}
final class NotATest { public function testIt(): void {} }
"#;
    let fixture = fixture(code);
    assert!(super::is_test_class(&fixture.index, "Tests\\FooTest"));
    assert!(super::is_test_class(&fixture.index, "Tests\\Base"));
    assert!(!super::is_test_class(&fixture.index, "Tests\\NotATest"));
    let class = fixture.index.class("Tests\\FooTest").unwrap();
    let tests: Vec<&str> = class
        .decl
        .methods
        .iter()
        .filter(|method| super::is_test_method(class.decl, method))
        .map(|method| method.name.as_str())
        .collect();
    assert_eq!(tests, vec!["testPlain", "marked", "documented"]);
}

fn completions(code: &str) -> Vec<String> {
    use crate::completion::{CompletionOptions, complete};
    let fixture = fixture(code);
    let (offset, text) = lsc_text::testing::cursor(code);
    complete(&fixture.index, &text, offset, CompletionOptions::default())
        .items
        .into_iter()
        .map(|item| item.label)
        .collect()
}

#[test]
fn provider_names_complete_inside_the_attribute_string() {
    let code = r#"<?php
namespace Tests;
use PHPUnit\Framework\TestCase;
use PHPUnit\Framework\Attributes\DataProvider;
final class FooTest extends TestCase {
    #[DataProvider('$0')]
    public function testA(int $a): void {}
    public function testB(): void {}
    public static function providesNumbers(): iterable { return []; }
    public function providesWords(): iterable { return []; }
    protected function hidden(): iterable { return []; }
}
"#;
    assert_eq!(completions(code), vec!["providesNumbers", "providesWords"]);
    let typed = code.replace("'$0'", "'wor$0'");
    assert_eq!(completions(&typed), vec!["providesWords"]);
}

#[test]
fn external_providers_and_dependencies_complete_from_the_right_class() {
    let external = r#"<?php
namespace Tests;
use PHPUnit\Framework\TestCase;
use PHPUnit\Framework\Attributes\DataProviderExternal;
use Tests\Support\Providers;
final class FooTest extends TestCase {
    #[DataProviderExternal(Providers::class, '$0')]
    public function testA(int $a): void {}
}
"#;
    assert_eq!(completions(external), vec!["numbers"]);
    let depends = r#"<?php
namespace Tests;
use PHPUnit\Framework\TestCase;
use PHPUnit\Framework\Attributes\Depends;
final class FooTest extends TestCase {
    public function testFirst(): void {}
    #[Depends('$0')]
    public function testSecond(): void {}
    public function helper(): void {}
}
"#;
    assert_eq!(completions(depends), vec!["testFirst"]);
    let doc = r#"<?php
namespace Tests;
use PHPUnit\Framework\TestCase;
final class FooTest extends TestCase {
    public function testFirst(): void {}
    /** @depends $0 */
    public function testSecond(): void {}
}
"#;
    assert_eq!(completions(doc), vec!["testFirst"]);
}

#[test]
fn group_names_come_from_every_test_of_the_project() {
    let other = "<?php\nuse PHPUnit\\Framework\\Attributes\\Group;\n#[Group('slow')]\nclass A {}\n";
    let code = r#"<?php
namespace Tests;
use PHPUnit\Framework\Attributes\Group;
#[Group('fast')]
final class FooTest {
    #[Group('$0')]
    public function testA(): void {}
}
"#;
    let fixture = Fixture::new(&[("phpunit.php", PHPUNIT), ("Other.php", other)]).with_current(code);
    let (offset, text) = lsc_text::testing::cursor(code);
    let labels: Vec<String> = crate::completion::complete(
        &fixture.index,
        &text,
        offset,
        crate::completion::CompletionOptions::default(),
    )
    .items
    .into_iter()
    .map(|item| item.label)
    .collect();
    assert_eq!(labels, vec!["fast", "slow"]);
}

const SERVICE: &str = r#"<?php
namespace App;
class Mailer {
    public function send(string $to, string $body): bool {}
    public function count(): int {}
    public function name(): ?string {}
    public function reset(): void {}
    final public function sealed(): void {}
    public static function make(): static {}
}
"#;

fn mock_fixture(current: &str) -> Fixture {
    Fixture::new(&[
        ("phpunit.php", PHPUNIT),
        ("Providers.php", PROVIDERS),
        ("Mailer.php", SERVICE),
    ])
    .with_current(current)
}

fn type_at(code: &str, name: &str) -> String {
    let fixture = mock_fixture(code);
    let (_, root, offset) = split_cursor(code);
    let analyzer = Analyzer::new(&fixture.index, &root, offset);
    analyzer
        .env_at(offset)
        .get(name)
        .map_or_else(|| "<unset>".to_string(), |ty| ty.display(false))
}

#[test]
fn mocks_and_stubs_are_the_double_and_the_class() {
    let code = r#"<?php
namespace Tests;
use App\Mailer;
use PHPUnit\Framework\TestCase;
final class FooTest extends TestCase {
    public function testIt(): void {
        $mock = $this->createMock(Mailer::class);
        $stub = self::createStub(Mailer::class);
        $built = $this->getMockBuilder(Mailer::class)->onlyMethods(['send'])->getMock();
        $0
    }
}
"#;
    let parts = |name: &str| {
        let shown = type_at(code, name);
        let mut parts: Vec<String> = shown.split('&').map(str::to_string).collect();
        parts.sort();
        parts
    };
    assert_eq!(
        parts("mock"),
        vec!["App\\Mailer", "PHPUnit\\Framework\\MockObject\\MockObject"]
    );
    assert_eq!(
        parts("stub"),
        vec!["App\\Mailer", "PHPUnit\\Framework\\MockObject\\Stub"]
    );
    assert_eq!(
        parts("built"),
        vec!["App\\Mailer", "PHPUnit\\Framework\\MockObject\\MockObject"]
    );
}

#[test]
fn a_method_of_a_double_is_named_in_the_string_that_configures_it() {
    let code = r#"<?php
namespace Tests;
use App\Mailer;
use PHPUnit\Framework\TestCase;
final class FooTest extends TestCase {
    public function testIt(): void {
        $mock = $this->createMock(Mailer::class);
        $mock->expects($this->once())->method('se$0nd');
    }
}
"#;
    assert_eq!(
        {
            let fixture = mock_fixture(code);
            let (_, root, offset) = split_cursor(code);
            let analyzer = Analyzer::new(&fixture.index, &root, offset);
            analyzer
                .definitions(offset)
                .into_iter()
                .map(|place| {
                    let source = &fixture.sources[place.path.as_ref().unwrap()];
                    source[place.span.start as usize..place.span.end as usize].to_string()
                })
                .collect::<Vec<_>>()
        },
        vec!["send"]
    );
    let completing = code.replace("'se$0nd'", "'$0'");
    let fixture = mock_fixture(&completing);
    let (offset, text) = lsc_text::testing::cursor(&completing);
    let labels: Vec<String> = crate::completion::complete(
        &fixture.index,
        &text,
        offset,
        crate::completion::CompletionOptions::default(),
    )
    .items
    .into_iter()
    .map(|item| item.label)
    .collect();
    assert_eq!(labels, vec!["count", "name", "reset", "send"]);
}

#[test]
fn renaming_a_method_follows_the_strings_of_doubles() {
    let code = r#"<?php
namespace Tests;
use App\Mailer;
use PHPUnit\Framework\TestCase;
final class FooTest extends TestCase {
    public function testIt(): void {
        $mock = $this->createMock(Mailer::class);
        $mock->method('count')->willReturn(1);
        $partial = $this->getMockBuilder(Mailer::class)->onlyMethods(['count', 'name'])->getMock();
    }
}
"#;
    let fixture = Fixture::new(&[("phpunit.php", PHPUNIT), ("Mailer.php", SERVICE), ("FooTest.php", code)]);
    let sources = Files(fixture.sources.clone());
    let current = "<?php\nnamespace App;\n$m = new Mailer();\n$m->cou$0nt();\n";
    let fixture = fixture.with_current(current);
    let (text, root, offset) = split_cursor(current);
    let path = PathBuf::from("/project/current.php");
    let done = rename(
        &fixture.index,
        &sources,
        &Current {
            path: &path,
            text: &text,
            root: &root,
        },
        offset,
        "total",
    )
    .expect("renamed");
    let touched: Vec<String> = done
        .files
        .iter()
        .flat_map(|file| {
            let source = if file.path == path {
                text.clone()
            } else {
                fixture.sources[&file.path].clone()
            };
            file.edits
                .iter()
                .map(|edit| source[usize::from(edit.range.start())..usize::from(edit.range.end())].to_string())
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(touched.iter().filter(|text| *text == "count").count(), 4, "{touched:?}");
}

#[test]
fn assertions_come_first_among_the_members_of_this_in_a_test() {
    let code = r#"<?php
namespace Tests;
use PHPUnit\Framework\TestCase;
final class FooTest extends TestCase {
    public function aaaHelper(): void {}
    public function testIt(): void {
        $this->$0
    }
}
"#;
    let labels = completions(code);
    assert_eq!(
        &labels[..5],
        [
            "assertInstanceOf",
            "assertIsString",
            "assertNotNull",
            "assertSame",
            "assertTrue"
        ]
    );
    assert!(labels.contains(&"aaaHelper".to_string()));
    let plain = r#"<?php
namespace Tests;
final class Plain {
    public function aaaHelper(): void {}
    public function assertSomething(): void {}
    public function run(): void {
        $this->$0
    }
}
"#;
    assert_eq!(completions(plain), vec!["aaaHelper", "assertSomething", "run"]);
}

#[test]
fn covers_attributes_complete_a_class_with_its_class_constant() {
    let code = r#"<?php
namespace Tests;
use PHPUnit\Framework\Attributes\CoversClass;
#[CoversClass(Mail$0)]
final class FooTest {}
"#;
    let fixture = mock_fixture(code);
    let (offset, text) = lsc_text::testing::cursor(code);
    let list = crate::completion::complete(
        &fixture.index,
        &text,
        offset,
        crate::completion::CompletionOptions::default(),
    );
    let edits: Vec<&str> = list.items.iter().map(|item| item.edit.new_text.as_str()).collect();
    assert_eq!(edits, vec!["Mailer::class"]);
}
