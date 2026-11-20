use crate::{FEATURES, LevelSeverity, PhpVersion, check_language_level, parse};

fn diagnostics(text: &str, level: PhpVersion) -> Vec<(String, LevelSeverity, bool)> {
    let parsed = parse(&format!("<?php\n{text}\n"));

    check_language_level(&parsed.syntax(), level)
        .into_iter()
        .map(|found| (found.message, found.severity, found.deprecated))
        .collect()
}

fn features_found(text: &str, level: PhpVersion) -> Vec<&'static str> {
    let parsed = parse(&format!("<?php\n{text}\n"));

    check_language_level(&parsed.syntax(), level)
        .into_iter()
        .map(|found| found.feature)
        .collect()
}

#[test]
fn versions_parse_and_compare() {
    assert_eq!(PhpVersion::parse("8.4"), Some(PhpVersion::V8_4));
    assert_eq!(PhpVersion::parse("8.1.27"), Some(PhpVersion::V8_1));
    assert_eq!(PhpVersion::parse("^8.2"), Some(PhpVersion::V8_2));
    assert_eq!(PhpVersion::parse(">=8.3.0"), Some(PhpVersion::V8_3));
    assert_eq!(PhpVersion::parse("8"), Some(PhpVersion::V8_0));
    assert_eq!(PhpVersion::parse("latest"), None);
    assert!(PhpVersion::V8_1 < PhpVersion::V8_4);
    assert_eq!(PhpVersion::V8_0.previous(), PhpVersion::V7_4);
    assert_eq!(PhpVersion::V8_4.previous(), PhpVersion::V8_3);
    assert_eq!(PhpVersion::V8_4.to_string(), "8.4");
}

#[test]
fn the_same_file_has_version_errors_only_at_the_old_level() {
    let text = "class A { public int $x { get => 1; } }\n$c = $b |> strlen(...);";
    assert!(features_found(text, PhpVersion::V8_5).is_empty());
    let at_8_0 = diagnostics(text, PhpVersion::V8_0);
    let messages: Vec<&str> = at_8_0.iter().map(|found| found.0.as_str()).collect();

    assert_eq!(
        messages,
        [
            "Property hooks are only available since PHP 8.4",
            "The pipe operator is only available since PHP 8.5",
            "First-class callable syntax is only available since PHP 8.1"
        ]
    );

    let at_8_4 = diagnostics(text, PhpVersion::V8_4);
    assert_eq!(at_8_4.len(), 1, "{at_8_4:?}");
    assert!(at_8_4.iter().all(|found| found.1 == LevelSeverity::Error));
}

#[test]
fn ranges_point_at_the_construct() {
    let source = "<?php\nclass A { public int $x { get => 1; } }\n";
    let parsed = parse(source);
    let found = check_language_level(&parsed.syntax(), PhpVersion::V8_3);
    assert_eq!(found.len(), 1);
    let range = found[0].range;

    assert_eq!(
        &source[usize::from(range.start())..usize::from(range.end())],
        "{ get => 1; }"
    );
}

#[test]
fn deprecations_are_warnings_with_the_tag() {
    let found = diagnostics("$a = \"${b}\";", PhpVersion::V8_1);
    assert!(found.is_empty(), "not deprecated yet at 8.1");
    let found = diagnostics("$a = \"${b}\";", PhpVersion::V8_2);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].1, LevelSeverity::Warning);
    assert!(found[0].2);
    assert_eq!(
        found[0].0,
        "The ${...} form of string interpolation is deprecated since PHP 8.2"
    );
}

#[test]
fn removed_syntax_is_an_error_from_the_removal_on() {
    let found = diagnostics("$a = (real) $b;", PhpVersion::V8_1);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].1, LevelSeverity::Error);
    assert_eq!(found[0].0, "The (real) cast was removed in PHP 8.0");
    assert!(diagnostics("$a = (real) $b;", PhpVersion::V7_4).is_empty());
}

#[test]
fn every_feature_row_has_a_working_example() {
    for feature in FEATURES {
        let source = format!("<?php\n{}\n", feature.example);
        let parsed = parse(&source);

        assert_eq!(
            parsed.errors(),
            &[],
            "the example of {} must parse without syntax errors",
            feature.id
        );

        let found_at = |level: PhpVersion| features_found(feature.example, level).contains(&feature.id);

        if let Some(since) = feature.since {
            assert!(
                found_at(since.previous()),
                "{} must be reported below {since}",
                feature.id
            );
            assert!(!found_at(since), "{} must be accepted at {since}", feature.id);
        }

        if let Some(removed) = feature.removed {
            assert!(
                !found_at(removed.previous()),
                "{} must be accepted below {removed}",
                feature.id
            );
            assert!(found_at(removed), "{} must be reported from {removed}", feature.id);
        }

        if let Some(deprecated) = feature.deprecated {
            assert!(
                !found_at(deprecated.previous()),
                "{} must be quiet below {deprecated}",
                feature.id
            );
            assert!(found_at(deprecated), "{} must warn from {deprecated}", feature.id);
        }

        assert!(
            feature.since.is_some() || feature.deprecated.is_some() || feature.removed.is_some(),
            "{} must change status in some version",
            feature.id
        );
    }
}

#[test]
fn feature_ids_are_unique() {
    let mut ids: Vec<_> = FEATURES.iter().map(|feature| feature.id).collect();
    ids.sort_unstable();
    let count = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), count);
}

#[test]
fn a_file_at_the_newest_level_with_old_syntax_is_quiet() {
    let text = "enum A: string { case B = 'b'; }\nfinal class C { public function __construct(private readonly int $x = 0) {} }";
    assert!(features_found(text, PhpVersion::LATEST).is_empty());
}

#[test]
fn partial_application_parses_and_is_new_in_8_6() {
    let text = "$a = str_replace('a', ?, ...);\n$b = $o->m(x: ?);\n$c = new A(f(?));\n$d = f(...);\n$e = f(...$args);\n$f = clone(?);";
    let parsed = parse(&format!("<?php\n{text}\n"));
    assert!(parsed.errors().is_empty(), "{:?}", parsed.errors());
    assert!(features_found(text, PhpVersion::V8_6).is_empty());
    let at_8_5 = features_found(text, PhpVersion::V8_5);
    assert_eq!(at_8_5, ["partial-function-application"; 5]);
}

#[test]
fn a_new_expression_or_attribute_cannot_have_its_own_placeholders() {
    let source = "<?php new A(?, ...); new A(value: ?); new class(...) {}; #[A(...)] #[B(value: ?)] class C {}";
    let parsed = parse(source);
    let errors: Vec<_> = parsed.errors().iter().map(|error| error.message.as_str()).collect();

    assert_eq!(
        errors,
        [
            "Cannot create Closure for new expression",
            "Cannot create Closure for new expression",
            "Cannot create Closure for new expression",
            "Cannot create Closure as attribute argument",
            "Cannot create Closure as attribute argument",
        ]
    );
    assert_eq!(parsed.syntax().text().to_string(), source);
}

#[test]
fn callable_placeholders_require_a_compatible_argument_list_and_receiver() {
    let invalid = [
        ("f(..., 1)", "Variadic placeholder must be last"),
        ("f(..., ...)", "Variadic placeholder may only appear once"),
        ("f(...$args, ?)", "Cannot combine partial application and unpacking"),
        ("f(?, ...$args)", "Cannot combine partial application and unpacking"),
        ("$o?->m(?)", "Cannot combine nullsafe operator with Closure creation"),
        (
            "$o?->a::$b->m(?)",
            "Cannot combine nullsafe operator with Closure creation",
        ),
        (
            "$o?->a::m()->n(?)",
            "Cannot combine nullsafe operator with Closure creation",
        ),
        (
            "($o?->m()[0])->n(...)",
            "Cannot combine nullsafe operator with Closure creation",
        ),
    ];

    for (expression, message) in invalid {
        let source = format!("<?php {expression};");
        let parsed = parse(&source);
        let errors: Vec<_> = parsed.errors().iter().map(|error| error.message.as_str()).collect();

        assert_eq!(errors, [message], "{expression}");
        assert_eq!(parsed.syntax().text().to_string(), source);
    }

    let source = "<?php f(x: ?, ...); f(...$args); ($o?->a)(?); $o?->a::m(?); $o?->a::CONST->m(?); $o?->m(f(?)); exit(status: ?);";
    let parsed = parse(source);

    assert!(parsed.errors().is_empty(), "{:?}", parsed.errors());
}

#[test]
fn a_keyword_cannot_be_an_explicit_import_alias() {
    let source = "<?php use function strlen as readonly; use function Vendor\\readonly;";
    let parsed = parse(source);
    let rejected: Vec<_> = parsed
        .errors()
        .iter()
        .map(|error| &source[usize::from(error.range.start())..usize::from(error.range.end())])
        .collect();

    assert_eq!(rejected, ["readonly"]);
    assert_eq!(parsed.syntax().text().to_string(), source);
}

#[test]
fn what_php_8_6_allows_is_reported_before_it() {
    let text = "readonly class R { public int $y = 2; }\nclass A { public readonly int $x = 1; public int $z = 3; #[\\Override] const C = 1; }\nenum E { public function __debugInfo(): array { return []; } }\nFOO->bar = 1;\n$o->bar = 2;";
    assert!(features_found(text, PhpVersion::V8_6).is_empty());
    assert_eq!(
        features_found(text, PhpVersion::V8_5),
        [
            "readonly-property-defaults",
            "readonly-property-defaults",
            "override-on-constants",
            "enum-debug-info",
            "constant-object-property-writes"
        ]
    );
}

#[test]
fn constant_object_writes_are_checked_at_their_write_site() {
    let text = r#"
        (OBJ)->postfix++;
        ++C::OBJ->prefix;
        OBJ->child->values[0] += 1;
        OBJECTS[0]->indexed = 1;
        $ref =& OBJ->alias;
        unset(OBJ->removed);
        foreach ($items as OBJ->iterated) {}
        [OBJ->assigned] = $items;
        [$key => &OBJ->referenced];
        function &referencedReturn() { return OBJ->returned; }
        function &referencedYield() { yield OBJ->key => OBJ->yielded; }
        $arrow = fn&() => OBJ->arrow;
        class Hooks {
            public $byRef { &get => OBJ->hookByRef; }
            public $byValue { get => OBJ->hookByValue; }
        }
        function plainReturn() { return OBJ->read; }
        function &generatorReturn() { yield 1; return OBJ->generator; }
        function &arrayReturn() { return [OBJ->array]; }
        $items[OBJ->offset] = 1;
        C::create()->writable = 1;
    "#;
    let source = format!("<?php\n{text}");
    let parsed = parse(&source);

    let targets: Vec<_> = check_language_level(&parsed.syntax(), PhpVersion::V8_5)
        .iter()
        .filter(|diagnostic| diagnostic.feature == "constant-object-property-writes")
        .map(|diagnostic| &source[usize::from(diagnostic.range.start())..usize::from(diagnostic.range.end())])
        .collect();

    assert!(parsed.errors().is_empty(), "{:?}", parsed.errors());
    assert!(features_found(text, PhpVersion::V8_6).is_empty());
    assert_eq!(
        targets,
        [
            "(OBJ)->postfix",
            "C::OBJ->prefix",
            "OBJ->child",
            "OBJECTS[0]->indexed",
            "OBJ->alias",
            "OBJ->removed",
            "OBJ->iterated",
            "OBJ->assigned",
            "OBJ->referenced",
            "OBJ->returned",
            "OBJ->yielded",
            "OBJ->arrow",
            "OBJ->hookByRef",
        ]
    );
}

#[test]
fn php_8_6_deprecations_apply_to_the_owning_declaration() {
    let text = r#"
        namespace Names {
            interface is {}
            trait let {}
            const _ = 1;
            const Let = 2;
            const let = 3;
        }
        namespace Enums {
            enum is { case namespace; }
        }
        namespace {
            use function Vendor\call as _;
            use Vendor\{Useful as Let, Other as _};
            use const Vendor\VERSION as is;
            class Is {
                const namespace = 1;
                const is = 1;
                public function __debugInfo(): array|null { return []; }
                public function __construct() {
                    try {} finally {
                        $inner = function () { return 8; };
                        return 1;
                    }
                }
            }
        }
    "#;
    let source = format!("<?php\n{text}");
    let parsed = parse(&source);
    let found = check_language_level(&parsed.syntax(), PhpVersion::V8_6);

    let identifiers: Vec<_> = found
        .iter()
        .filter(|diagnostic| diagnostic.feature == "deprecated-identifier-8-6")
        .map(|diagnostic| &source[usize::from(diagnostic.range.start())..usize::from(diagnostic.range.end())])
        .collect();

    let others: Vec<_> = found
        .iter()
        .map(|diagnostic| diagnostic.feature)
        .filter(|feature| *feature != "deprecated-identifier-8-6")
        .collect();

    assert!(parsed.errors().is_empty(), "{:?}", parsed.errors());
    assert!(features_found(text, PhpVersion::V8_5).is_empty());
    assert_eq!(
        identifiers,
        [
            "is",
            "let",
            "_",
            "let",
            "is",
            "namespace",
            "Let",
            "_",
            "is",
            "Is",
            "namespace"
        ]
    );
    assert_eq!(
        others,
        [
            "nullable-debug-info",
            "return-from-finally",
            "constructor-destructor-return-value"
        ]
    );
}
