//! Fixtures for tests: a small index built from PHP sources, and a cursor marker in the text.

use std::path::PathBuf;
use std::sync::Arc;

use php_index::extract::{ExtractOptions, extract};
use php_index::{Index, Origin};
use php_syntax::{PhpVersion, SyntaxNode, parse};

/// Where a test puts the cursor.
pub const CURSOR: &str = "$0";

pub struct Fixture {
    pub index: Index,
    /// The text of each file by path, for looking at what a span points to.
    pub sources: std::collections::HashMap<PathBuf, String>,
    /// A folder written for the test, removed again with the fixture.
    disk: Option<PathBuf>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(dir) = &self.disk {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

impl Fixture {
    pub fn new(files: &[(&str, &str)]) -> Fixture {
        Fixture::with_level(PhpVersion::V8_4, files, &[])
    }

    pub fn with_level(level: PhpVersion, files: &[(&str, &str)], stubs: &[(&str, &str)]) -> Fixture {
        let mut index = Index::new(level);
        let mut sources = std::collections::HashMap::new();
        for (path, text) in files {
            sources.insert(PathBuf::from(format!("/project/{path}")), text.to_string());
            let symbols = extract(&parse(text).syntax(), ExtractOptions::default());
            index.set_file(
                PathBuf::from(format!("/project/{path}")),
                Origin::Project,
                Arc::new(symbols),
            );
        }
        for (path, text) in stubs {
            let symbols = extract(&parse(text).syntax(), ExtractOptions { stub: true });
            index.set_file(PathBuf::from(format!("/stubs/{path}")), Origin::Stub, Arc::new(symbols));
        }
        Fixture {
            index,
            sources,
            disk: None,
        }
    }

    /// A project with every framework on and the files held in memory, so what the framework layer
    /// reads from other files sees the same text.
    pub fn framework(files: &[(&str, &str)]) -> Fixture {
        let index = php_index::framework::testing::project(files);
        let sources = files
            .iter()
            .map(|(path, text)| (PathBuf::from(format!("/project/{path}")), text.to_string()))
            .collect();
        Fixture {
            index,
            sources,
            disk: None,
        }
    }

    /// Like `with_level`, with the files on disk too, for what reads a body from the file it is in.
    pub fn on_disk(files: &[(&str, &str)], stubs: &[(&str, &str)]) -> Fixture {
        static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let dir = std::env::temp_dir().join(format!(
            "php-analysis-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let mut index = Index::new(PhpVersion::V8_4);
        let mut sources = std::collections::HashMap::new();
        for (path, text) in files {
            let full = dir.join(path);
            std::fs::create_dir_all(full.parent().expect("a parent")).expect("a test folder");
            std::fs::write(&full, text).expect("a test file");
            sources.insert(full.clone(), text.to_string());
            let symbols = extract(&parse(text).syntax(), ExtractOptions::default());
            index.set_file(full, Origin::Project, Arc::new(symbols));
        }
        for (path, text) in stubs {
            let symbols = extract(&parse(text).syntax(), ExtractOptions { stub: true });
            index.set_file(PathBuf::from(format!("/stubs/{path}")), Origin::Stub, Arc::new(symbols));
        }
        Fixture {
            index,
            sources,
            disk: Some(dir),
        }
    }

    /// Adds the file being edited to the index, the way the server does for an open document.
    pub fn with_current(mut self, text: &str) -> Fixture {
        let clean = text.replace(CURSOR, "");
        let symbols = extract(&parse(&clean).syntax(), ExtractOptions::default());
        self.index.set_open_text(
            std::path::Path::new("/project/current.php"),
            Some(Arc::from(clean.as_str())),
        );
        self.index.set_file(
            PathBuf::from("/project/current.php"),
            Origin::Project,
            Arc::new(symbols),
        );
        self
    }
}

/// The text without the cursor marker, its tree and the offset where the marker was.
pub fn split_cursor(text: &str) -> (String, SyntaxNode, u32) {
    let offset = text.find(CURSOR).expect("the text has a cursor marker");
    let clean = text.replacen(CURSOR, "", 1);
    let root = parse(&clean).syntax();
    (clean, root, offset as u32)
}

/// The files of a fixture as the place a search for usages reads from.
pub struct Files(pub std::collections::HashMap<PathBuf, String>);

impl crate::references::Sources for Files {
    fn candidates(&self, word: &str) -> Vec<PathBuf> {
        self.0
            .iter()
            .filter(|(_, text)| text.to_ascii_lowercase().contains(word))
            .map(|(path, _)| path.clone())
            .collect()
    }

    fn text(&self, path: &std::path::Path) -> Option<String> {
        self.0.get(path).cloned()
    }
}

/// A stand-in for the parts of PHPUnit that tests refer to by name.
pub const PHPUNIT: &str = r#"<?php
namespace PHPUnit\Framework\MockObject {
    interface Stub {
        public function method(string $constraint): InvocationStubber;
    }
    interface MockObject extends Stub {
        public function expects(object $invocationRule): InvocationMocker;
    }
    interface InvocationStubber {
        public function willReturn(mixed $value, mixed ...$nextValues): InvocationStubber;
        public function with(mixed ...$arguments): InvocationStubber;
    }
    interface InvocationMocker extends InvocationStubber {
        public function method(string $constraint): InvocationStubber;
    }
    /** @template MockedType of object */
    final class MockBuilder {
        public function onlyMethods(array $methods): static {}
        /** @return MockedType&MockObject */
        public function getMock(): MockObject {}
    }
}

namespace PHPUnit\Framework {
    use PHPUnit\Framework\MockObject\{MockBuilder, MockObject, Stub};

    abstract class Assert {
        /**
         * @template ExpectedType of object
         * @param class-string<ExpectedType> $expected
         * @phpstan-assert =ExpectedType $actual
         */
        final public static function assertInstanceOf(string $expected, mixed $actual, string $message = ''): void {}
        /** @phpstan-assert !null $actual */
        final public static function assertNotNull(mixed $actual, string $message = ''): void {}
        /** @phpstan-assert string $actual */
        final public static function assertIsString(mixed $actual, string $message = ''): void {}
        final public static function assertSame(mixed $expected, mixed $actual, string $message = ''): void {}
        final public static function assertTrue(mixed $condition, string $message = ''): void {}
    }

    abstract class TestCase extends Assert {
        protected function setUp(): void {}
        public function once(): object {}
        /**
         * @template RealInstanceType of object
         * @param class-string<RealInstanceType> $type
         * @return MockObject&RealInstanceType
         */
        final protected function createMock(string $type): MockObject {}
        /**
         * @template RealInstanceType of object
         * @param class-string<RealInstanceType> $type
         * @return RealInstanceType&Stub
         */
        final protected static function createStub(string $type): Stub {}
        /**
         * @template RealInstanceType of object
         * @param class-string<RealInstanceType> $className
         * @return MockBuilder<RealInstanceType>
         */
        final protected function getMockBuilder(string $className): MockBuilder {}
    }
}

namespace PHPUnit\Framework\Attributes {
    #[\Attribute] final class Test {}
    #[\Attribute] final class DataProvider { public function __construct(string $methodName) {} }
    #[\Attribute] final class DataProviderExternal { public function __construct(string $className, string $methodName) {} }
    #[\Attribute] final class Depends { public function __construct(string $methodName) {} }
    #[\Attribute] final class DependsExternal { public function __construct(string $className, string $methodName) {} }
    #[\Attribute] final class Group { public function __construct(string $name) {} }
    #[\Attribute] final class CoversClass { public function __construct(string $className) {} }
    #[\Attribute] final class UsesClass { public function __construct(string $className) {} }
    #[\Attribute] final class CoversMethod { public function __construct(string $className, string $methodName) {} }
    #[\Attribute] final class CoversFunction { public function __construct(string $functionName) {} }
}
"#;

/// A stand-in for the functions and classes of Pest that tests are written with.
pub const PEST: &str = r#"<?php
namespace Pest {
    /**
     * @template TValue
     *
     * @property OppositeExpectation $not
     * @property \Pest\Expectations\EachExpectation $each
     *
     * @mixin \Pest\Mixins\Expectation<TValue>
     */
    final class Expectation {
        /**
         * @param TValue $value
         */
        public function __construct(public mixed $value) {}
        /**
         * @template TAndValue
         * @param TAndValue $value
         * @return self<TAndValue>
         */
        public function and(mixed $value): Expectation {}
        /** @return self<TValue> */
        public function sequence(mixed ...$callbacks): self {}
        public function __call(string $method, array $parameters) {}
    }
    final class OppositeExpectation {
        public function toBe(mixed $expected): Expectation {}
    }
}
namespace Pest\Mixins {
    /**
     * @template TValue
     * @mixin \Pest\Expectation<TValue>
     */
    final class Expectation {
        /** @return self<TValue> */
        public function toBe(mixed $expected): self {}
        /** @return self<TValue> */
        public function toBeInstanceOf(string $class): self {}
        /** @return self<TValue> */
        public function toBeNull(): self {}
        /** @return self<TValue> */
        public function toHaveCount(int $count): self {}
    }
}
namespace Pest\Expectations {
    /**
     * @template TValue
     * @mixin \Pest\Expectation<TValue>
     */
    final class EachExpectation {
        /** @return self<TValue> */
        public function toBeInt(): self {}
    }
}
namespace Pest {
    final class TestCall {
        public function with(\Closure|iterable|string ...$data): self {}
        public function group(string ...$groups): self {}
    }
    final class BeforeEachCall {
        public function group(string ...$groups): self {}
    }
    final class DescribeCall {}
    final class UsesCall {
        public function in(string ...$targets): self {}
    }
}
namespace {
    /**
     * @template TValue
     * @param TValue|null $value
     * @return \Pest\Expectation<TValue|null>
     */
    function expect(mixed $value = null): \Pest\Expectation {}
    function it(string $description, ?\Closure $closure = null): \Pest\TestCall {}
    function test(?string $description = null, ?\Closure $closure = null): \Pest\TestCall {}
    function describe(string $description, \Closure $tests): \Pest\DescribeCall {}
    function beforeEach(?\Closure $closure = null): \Pest\BeforeEachCall {}
    function afterEach(?\Closure $closure = null): \Pest\BeforeEachCall {}
    function dataset(string $name, \Closure|iterable $dataset): void {}
    function uses(string ...$classAndTraits): \Pest\UsesCall {}
    function arch(?string $description = null, ?\Closure $closure = null): \Pest\TestCall {}
}
"#;
