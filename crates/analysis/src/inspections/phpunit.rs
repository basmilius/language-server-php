//! Tests that name something that is not there: a data provider or a dependency that does not
//! exist, and a data provider whose rows do not fit the parameters of the test.

use php_index::Type;
use php_syntax::SyntaxKind::*;
use php_syntax::SyntaxNode;

use super::Cx;
use crate::ast::child_of;
use crate::infer::DeclRef;
use crate::phpunit::strings::{Role, StringTarget, TestString, strings_in};

pub(super) fn run(cx: &Cx) {
    let doubles = cx.on("missing-double-method") || cx.on("double-return-type-mismatch");
    if !(cx.on("missing-data-provider") || cx.on("missing-test-dependency") || cx.on("data-provider-arity") || doubles)
    {
        return;
    }
    let text = cx.text;
    let mentions_doubles =
        doubles && (text.contains("->method(") || text.contains("onlyMethods") || text.contains("PartialMock"));
    if !(text.contains("ataProvider") || text.contains("epends") || mentions_doubles) {
        return;
    }
    let strings = strings_in(&cx.file);
    for string in &strings {
        let StringTarget::Method { class, role } = &string.target else {
            continue;
        };
        if !matches!(role, Role::DataProvider | Role::Depends | Role::Mocked) || string.value.is_empty() {
            continue;
        }
        check_exists(cx, string, class, *role);
    }
    if cx.on("double-return-type-mismatch") {
        for node in cx.nodes.iter().filter(|node| node.kind() == CALL_EXPR) {
            check_return_value(cx, node, &strings);
        }
    }
    if cx.on("data-provider-arity") {
        for node in cx.nodes.iter().filter(|node| node.kind() == METHOD_DECLARATION) {
            check_arity(cx, node, &strings);
        }
    }
}

fn check_exists(cx: &Cx, string: &TestString, class: &str, role: Role) {
    if !cx.ready || cx.index.class(class).is_none() {
        return;
    }
    let receiver = Type::class(class.to_string());
    if cx.index.find_method(&receiver, &string.value).is_some() || has_magic_call(cx, &receiver) {
        return;
    }
    let (code, what) = match role {
        Role::DataProvider => ("missing-data-provider", "data provider"),
        Role::Mocked => ("missing-double-method", "method"),
        _ => ("missing-test-dependency", "test"),
    };
    cx.report(
        code,
        string.range,
        format!(
            "The {what} '{}' does not exist on {}",
            string.value,
            crate::short(class)
        ),
        super::Fix::None,
    );
}

fn has_magic_call(cx: &Cx, receiver: &Type) -> bool {
    cx.index.find_method(receiver, "__callStatic").is_some()
}

/// `->method('name')->willReturn(value)`: a value the method of the double cannot return.
fn check_return_value(cx: &Cx, call: &SyntaxNode, strings: &[TestString]) {
    let Some(callee) = call
        .children()
        .next()
        .filter(|callee| callee.kind() == PROPERTY_FETCH_EXPR)
    else {
        return;
    };
    let is_will_return = callee
        .children()
        .find(|child| child.kind() == NAME)
        .is_some_and(|name| name.text().to_string().eq_ignore_ascii_case("willReturn"));
    let Some(configured) = callee.children().next().filter(|node| node.kind() == CALL_EXPR) else {
        return;
    };
    let args = crate::infer::arguments(call);
    let [given] = args.as_slice() else {
        return;
    };
    let (true, Some(value)) = (is_will_return, given.expr.as_ref()) else {
        return;
    };
    let Some(string) = strings.iter().find(|string| {
        matches!(string.target, StringTarget::Method { role: Role::Mocked, .. })
            && configured.text_range().contains_range(string.range)
    }) else {
        return;
    };
    let StringTarget::Method { class, .. } = &string.target else {
        return;
    };
    let Some(found) = cx.index.find_method(&Type::class(class.clone()), &string.value) else {
        return;
    };
    let level = cx.index.level;
    let Some(wanted) = found.member.callable.native_return(level) else {
        return;
    };
    if matches!(wanted, Type::Void | Type::Never) || wanted.has_template() {
        return;
    }
    if wanted
        .members()
        .iter()
        .any(|member| matches!(member, Type::Static | Type::SelfType | Type::Parent))
    {
        return;
    }
    let analyzer = cx.file.analyzer(call);
    let env = analyzer.env_around(call);
    let Some(actual) = super::types::sure_type(cx, &analyzer, &env, value) else {
        return;
    };
    if cx.type_mismatch(&actual, wanted, false) {
        cx.report(
            "double-return-type-mismatch",
            value.text_range(),
            format!(
                "{}::{}() returns {}, which cannot be {}",
                crate::short(class),
                found.member.name,
                wanted.display(true),
                actual.display(true)
            ),
            super::Fix::None,
        );
    }
}

/// How many values a data set gives the test: the required and the most the test takes.
fn arity_of(method: &SyntaxNode) -> Option<(usize, Option<usize>)> {
    let list = child_of(method, PARAMETER_LIST)?;
    let parameters: Vec<SyntaxNode> = list.children().filter(|child| child.kind() == PARAMETER).collect();
    let variadic = parameters.iter().any(|parameter| {
        parameter
            .children_with_tokens()
            .any(|element| element.kind() == ELLIPSIS)
    });
    let required = parameters
        .iter()
        .filter(|parameter| {
            let has_default = parameter.children_with_tokens().any(|element| element.kind() == ASSIGN);
            let is_variadic = parameter
                .children_with_tokens()
                .any(|element| element.kind() == ELLIPSIS);
            !has_default && !is_variadic
        })
        .count();
    Some((required, (!variadic).then_some(parameters.len())))
}

fn check_arity(cx: &Cx, method: &SyntaxNode, strings: &[TestString]) {
    let range = method.text_range();
    let mine: Vec<&TestString> = strings
        .iter()
        .filter(|string| range.contains_range(string.range) || range.contains(string.range.start()))
        .collect();
    // A dependency adds its result after the data set, which this does not count.
    if mine.iter().any(|string| {
        matches!(
            string.target,
            StringTarget::Method {
                role: Role::Depends,
                ..
            }
        )
    }) {
        return;
    }
    let Some((required, most)) = arity_of(method) else {
        return;
    };
    for string in mine {
        let StringTarget::Method {
            class,
            role: Role::DataProvider,
        } = &string.target
        else {
            continue;
        };
        let Some(rows) = provider_rows(cx, class, &string.value) else {
            continue;
        };
        for (position, row) in rows.iter().enumerate() {
            let Some(values) = row else {
                continue;
            };
            let message = if *values < required {
                format!(
                    "Data set #{position} of '{}' has {values} value(s), but the test requires {required}",
                    string.value
                )
            } else if most.is_some_and(|most| *values > most) {
                format!(
                    "Data set #{position} of '{}' has {values} value(s), but the test takes {}",
                    string.value,
                    most.unwrap_or_default()
                )
            } else {
                continue;
            };
            cx.report("data-provider-arity", string.range, message, super::Fix::None);
            break;
        }
    }
}

/// The number of values of each row a data provider returns, `None` for a row that is not an array
/// of positional values, and `None` for the whole provider when its rows are not written out.
fn provider_rows(cx: &Cx, class: &str, name: &str) -> Option<Vec<Option<usize>>> {
    let receiver = Type::class(class.to_string());
    let found = cx.index.find_method(&receiver, name)?;
    if found.class.file.origin == php_index::Origin::Stub {
        return None;
    }
    let decl = DeclRef {
        path: found.class.file.path.clone(),
        name_start: found.member.name_span.start,
        name: found.member.name.clone(),
    };
    let analyzer = cx.file.analyzer(&cx.root);
    let (_, function) = analyzer.read_declaration(&decl)?;
    rows_of(&function)
}

fn rows_of(function: &SyntaxNode) -> Option<Vec<Option<usize>>> {
    let mut returns = Vec::new();
    let mut yields = Vec::new();
    for node in function.descendants() {
        let owner = node.ancestors().skip(1).find(|ancestor| {
            crate::ast::is_function_like(ancestor.kind()) || crate::ast::is_class_like(ancestor.kind())
        });
        if owner.as_ref() != Some(function) {
            continue;
        }
        match node.kind() {
            RETURN_STATEMENT => returns.push(node),
            YIELD_EXPR => yields.push(node),
            YIELD_FROM_EXPR => return None,
            _ => {}
        }
    }
    if !yields.is_empty() {
        return Some(
            yields
                .iter()
                .map(|expression| {
                    let value = expression.children().last()?;
                    row_width(&value)
                })
                .collect(),
        );
    }
    let [only] = returns.as_slice() else {
        return None;
    };
    let table = only.children().next().filter(|table| table.kind() == ARRAY_EXPR)?;
    let mut rows = Vec::new();
    for item in table.children().filter(|child| child.kind() == ARRAY_ITEM) {
        if item.children_with_tokens().any(|element| element.kind() == ELLIPSIS) {
            return None;
        }
        rows.push(item.children().last().and_then(|row| row_width(&row)));
    }
    Some(rows)
}

/// The values of a row written as an array of positional values.
fn row_width(row: &SyntaxNode) -> Option<usize> {
    if row.kind() != ARRAY_EXPR {
        return None;
    }
    let mut count = 0;
    for item in row.children().filter(|child| child.kind() == ARRAY_ITEM) {
        let operands = item.children().count();
        let spread = item.children_with_tokens().any(|element| element.kind() == ELLIPSIS);
        if operands != 1 || spread {
            return None;
        }
        count += 1;
    }
    Some(count)
}

#[cfg(test)]
mod tests {
    use crate::inspections::tests::check_with;
    use crate::testing::PHPUNIT;

    fn found(source: &str) -> Vec<(&'static str, String)> {
        check_with(&[("phpunit.php", PHPUNIT)], source)
            .into_iter()
            .filter(|(code, _)| code.contains("provider") || code.contains("dependency"))
            .collect()
    }

    const HEAD: &str = "<?php\nnamespace Tests;\nuse PHPUnit\\Framework\\TestCase;\nuse PHPUnit\\Framework\\Attributes\\{DataProvider, Depends};\nfinal class FooTest extends TestCase {\n";

    #[test]
    fn a_provider_that_does_not_exist_is_reported_on_its_string() {
        let source = format!(
            "{HEAD}    #[DataProvider('gone')]\n    public function testA(int $a): void {{}}\n    /** @dataProvider alsoGone */\n    public function testB(int $a): void {{}}\n    public static function here(): iterable {{ return []; }}\n}}\n"
        );
        assert_eq!(
            found(&source),
            vec![
                ("missing-data-provider", "gone".to_string()),
                ("missing-data-provider", "alsoGone".to_string())
            ]
        );
    }

    #[test]
    fn a_dependency_that_does_not_exist_is_a_warning() {
        let source = format!(
            "{HEAD}    public function testFirst(): void {{}}\n    #[Depends('testFirst')]\n    public function testA(): void {{}}\n    #[Depends('testMissing')]\n    public function testB(): void {{}}\n}}\n"
        );
        assert_eq!(
            found(&source),
            vec![("missing-test-dependency", "testMissing".to_string())]
        );
    }

    #[test]
    fn existing_providers_and_dependencies_are_left_alone() {
        let source = format!(
            "{HEAD}    #[DataProvider('rows')]\n    public function testA(int $a, int $b): void {{}}\n    public static function rows(): iterable {{ return [[1, 2], [3, 4]]; }}\n}}\n"
        );
        assert_eq!(found(&source), vec![]);
    }

    #[test]
    fn rows_that_do_not_fit_the_parameters_are_reported_once_where_they_are_certain() {
        let few = format!(
            "{HEAD}    #[DataProvider('rows')]\n    public function testA(int $a, int $b): void {{}}\n    public static function rows(): iterable {{ return [[1, 2], [3]]; }}\n}}\n"
        );
        assert_eq!(found(&few), vec![("data-provider-arity", "rows".to_string())]);
        let many = format!(
            "{HEAD}    #[DataProvider('rows')]\n    public function testA(int $a): void {{}}\n    public static function rows(): iterable {{ yield 'one' => [1, 2]; yield 'two' => [3, 4]; }}\n}}\n"
        );
        assert_eq!(found(&many), vec![("data-provider-arity", "rows".to_string())]);
    }

    #[test]
    fn rows_that_cannot_be_counted_stay_silent() {
        let defaults = format!(
            "{HEAD}    #[DataProvider('rows')]\n    public function testA(int $a, int $b = 2, int ...$rest): void {{}}\n    public static function rows(): iterable {{ return [[1], [1, 2], [1, 2, 3, 4]]; }}\n}}\n"
        );
        assert_eq!(found(&defaults), vec![]);
        let computed = format!(
            "{HEAD}    #[DataProvider('rows')]\n    public function testA(int $a): void {{}}\n    public static function rows(): iterable {{ return array_map(fn ($x) => [$x, $x], [1]); }}\n}}\n"
        );
        assert_eq!(found(&computed), vec![]);
        let spread = format!(
            "{HEAD}    #[DataProvider('rows')]\n    public function testA(int $a): void {{}}\n    public static function rows(): iterable {{ return [[...[1, 2]], $x]; }}\n}}\n"
        );
        assert_eq!(found(&spread), vec![]);
    }

    const MAILER: &str = "<?php\nnamespace App;\nclass Mailer {\n    public function send(string $to): bool {}\n    public function count(): int {}\n    public function reset(): void {}\n    public function name(): ?string {}\n}\n";

    fn doubles(source: &str) -> Vec<(&'static str, String)> {
        check_with(&[("phpunit.php", PHPUNIT), ("Mailer.php", MAILER)], source)
            .into_iter()
            .filter(|(code, _)| code.contains("double"))
            .collect()
    }

    const DOUBLE_HEAD: &str = "<?php\nnamespace Tests;\nuse App\\Mailer;\nuse PHPUnit\\Framework\\TestCase;\nfinal class FooTest extends TestCase {\n    public function testIt(): void {\n        $mock = $this->createMock(Mailer::class);\n";

    #[test]
    fn a_method_a_double_cannot_configure_is_reported() {
        let source = format!(
            "{DOUBLE_HEAD}        $mock->method('send')->willReturn(true);\n        $mock->method('gone')->willReturn(true);\n    }}\n}}\n"
        );
        assert_eq!(doubles(&source), vec![("missing-double-method", "gone".to_string())]);
    }

    #[test]
    fn a_value_the_method_cannot_return_is_reported_where_it_is_certain() {
        let source = format!(
            "{DOUBLE_HEAD}        $mock->method('count')->willReturn('many');\n        $mock->method('count')->willReturn(3);\n        $mock->method('name')->willReturn(null);\n        $mock->method('send')->willReturn(new \\stdClass());\n        $mock->method('reset')->willReturn(1);\n        $mock->expects($this->once())->method('count')->willReturn([]);\n        $mock->method('send')->willReturn($unknown);\n    }}\n}}\n"
        );
        assert_eq!(
            doubles(&source),
            vec![
                ("double-return-type-mismatch", "new \\stdClass()".to_string()),
                ("double-return-type-mismatch", "[]".to_string()),
            ]
        );
    }
}
