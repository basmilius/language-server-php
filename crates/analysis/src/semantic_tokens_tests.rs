use expect_test::{Expect, expect};
use php_syntax::{PhpVersion, TextRange, TextSize, parse};

use crate::semantic_tokens::{TOKEN_MODIFIERS, TOKEN_TYPES, semantic_tokens};
use crate::testing::Fixture;

fn render(files: &[(&str, &str)], stubs: &[(&str, &str)], text: &str, range: Option<(u32, u32)>) -> String {
    let fixture = Fixture::with_level(PhpVersion::V8_4, files, stubs).with_current(text);
    let root = parse(text).syntax();
    let range = range.map(|(start, end)| TextRange::new(TextSize::from(start), TextSize::from(end)));
    let mut out = String::new();
    for token in semantic_tokens(&fixture.index, &root, range) {
        let modifiers: Vec<&str> = TOKEN_MODIFIERS
            .iter()
            .enumerate()
            .filter(|(bit, _)| token.modifiers & (1 << bit) != 0)
            .map(|(_, name)| *name)
            .collect();
        out.push_str(&format!(
            "{} {}{}\n",
            &text[token.start as usize..token.end as usize],
            TOKEN_TYPES[token.ty as usize],
            if modifiers.is_empty() {
                String::new()
            } else {
                format!(" [{}]", modifiers.join(", "))
            }
        ));
    }
    out
}

fn check(files: &[(&str, &str)], stubs: &[(&str, &str)], text: &str, expected: Expect) {
    expected.assert_eq(&render(files, stubs, text, None));
}

const LIB: &str = "<?php\nnamespace Lib;\n\ninterface Shape { public function area(): float; }\nabstract class Base implements Shape {\n    public const MAX = 1;\n    public static int $count = 0;\n    public function __construct(public readonly string $name) {}\n    abstract public function draw(): void;\n    /** @deprecated */\n    public static function old(): void {}\n}\nenum Suit { case Hearts; }\ntrait Loud {}\nfunction helper(): void {}\nconst LIMIT = 1;\n";

#[test]
fn colors_declarations_by_what_they_declare() {
    let text = "<?php\nnamespace App;\n\nuse Lib\\Base;\n\nfinal class Circle extends Base implements \\Lib\\Shape\n{\n    use \\Lib\\Loud;\n    private int $radius = 1;\n    public function area(): float { return 1.0; }\n    public function draw(): void {}\n}\n";
    check(
        &[("lib.php", LIB)],
        &[],
        text,
        expect![[r#"
            App namespace
            Lib namespace
            Base class [abstract]
            Circle class [declaration]
            Base class [abstract]
            Lib namespace
            Shape interface
            Lib namespace
            Loud struct
            $radius property [declaration]
            area method [declaration]
            draw method [declaration]
        "#]],
    );
}

#[test]
fn colors_uses_of_members_variables_and_functions() {
    let text = "<?php\nnamespace App;\n\nuse Lib\\Base;\nuse Lib\\Suit;\nuse function Lib\\helper;\n\nfunction f(Base $shape, int $n) {\n    $local = $shape->name;\n    Base::old();\n    echo Base::MAX, Base::$count, Suit::Hearts->name, LIMIT, $n;\n    helper();\n    strlen('x');\n    $f = fn($x) => $x + $local;\n    foo(name: 1);\n}\n";
    check(
        &[("lib.php", LIB)],
        &[("standard/s.php", "<?php\nfunction strlen(string $string): int {}\n")],
        text,
        expect![[r#"
            App namespace
            Lib namespace
            Base class [abstract]
            Lib namespace
            Suit enum
            Lib namespace
            helper function
            f function [declaration]
            Base class [abstract]
            $shape parameter [declaration]
            $n parameter [declaration]
            $local variable
            $shape parameter
            name property [readonly]
            Base class [abstract]
            old method [static, deprecated]
            Base class [abstract]
            MAX property [readonly, static]
            Base class [abstract]
            $count property [static]
            Suit enum
            Hearts enumMember [readonly, static]
            name property
            $n parameter
            helper function
            strlen function [defaultLibrary]
            $f variable
            $x parameter [declaration]
            $x parameter
            $local variable
            name parameter
        "#]],
    );
}

#[test]
fn colors_the_whole_name_of_an_attribute_as_a_decorator() {
    let lib = "<?php\nnamespace Lib\\Meta;\n\n#[\\Attribute]\nfinal class Model {}\n";
    let text = "<?php\nnamespace App;\n\nuse Lib\\Meta\\Model;\n\n#[Meta\\Model(Model::class, 'x')]\n#[\\Lib\\Meta\\Model, Missing]\nfinal class Team\n{\n    #[Model]\n    public int $id = 1;\n}\n";
    check(
        &[("lib.php", lib)],
        &[],
        text,
        expect![[r#"
            App namespace
            Lib\Meta namespace
            Model class
            Meta\Model decorator
            Model class
            \Lib\Meta\Model decorator
            Missing decorator
            Team class [declaration]
            Model decorator
            $id property [declaration]
        "#]],
    );
}

#[test]
fn colors_the_tags_and_types_of_doc_comments() {
    let text = "<?php\nnamespace App;\n\nuse Lib\\Base;\n\n/**\n * @template T of Base\n * @property-read int $size\n */\nclass Box\n{\n    /**\n     * Packs things.\n     *\n     * @param T $item The item\n     * @param array<int, \\Lib\\Shape> $others\n     * @return Base|null\n     * @see Base::draw()\n     */\n    public function pack($item, array $others) {}\n}\n";
    check(
        &[("lib.php", LIB)],
        &[],
        text,
        expect![[r#"
            App namespace
            Lib namespace
            Base class [abstract]
            @template keyword [documentation]
            T typeParameter [documentation]
            Base class [abstract, documentation]
            @property-read keyword [documentation]
            $size property [declaration, documentation]
            Box class [declaration]
            @param keyword [documentation]
            T typeParameter [documentation]
            $item parameter [documentation]
            @param keyword [documentation]
            Shape interface [documentation]
            $others parameter [documentation]
            @return keyword [documentation]
            Base class [abstract, documentation]
            @see keyword [documentation]
            Base class [abstract, documentation]
            draw method [abstract, documentation]
            pack method [declaration]
            $item parameter [declaration]
            $others parameter [declaration]
        "#]],
    );
}

#[test]
fn answers_a_range_with_the_tokens_inside_it() {
    let text = "<?php\nfunction a(int $one) { return $one; }\nfunction b(int $two) { return $two; }\n";
    let second = text.find("function b").unwrap() as u32;
    let found = render(&[], &[], text, Some((second, text.len() as u32)));
    assert_eq!(
        found,
        "b function [declaration]\n$two parameter [declaration]\n$two parameter\n"
    );
}
