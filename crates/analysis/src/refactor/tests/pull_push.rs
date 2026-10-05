use super::{applied, done, offered, refused};

const BASE: &str = "<?php\nnamespace App;\n\nabstract class Base\n{\n    protected int $count = 0;\n\n    public function name(): string\n    {\n        return 'base';\n    }\n}\n";

#[test]
fn pulls_a_method_up_into_the_parent() {
    let current = "<?php\nnamespace App;\n\nclass Child extends Base\n{\n    public function gre$0et(string $who): string\n    {\n        return 'hello ' . $who . $this->name();\n    }\n\n    public function other(): void {}\n}\n";
    let result = done(&[("Base.php", BASE)], current, "Pull method greet up to Base");
    assert_eq!(
        result.files["current.php"],
        "<?php\nnamespace App;\n\nclass Child extends Base\n{\n    public function other(): void {}\n}\n"
    );
    assert!(
        result.files["Base.php"].ends_with("    public function name(): string\n    {\n        return 'base';\n    }\n\n    public function greet(string $who): string\n    {\n        return 'hello ' . $who . $this->name();\n    }\n}\n"),
        "{}",
        result.files["Base.php"]
    );
}

#[test]
fn a_private_member_becomes_protected_on_the_way() {
    let current = "<?php\nnamespace App;\n\nclass Child extends Base\n{\n    private function he$0lp(): int\n    {\n        return $this->count;\n    }\n}\n";
    let result = done(&[("Base.php", BASE)], current, "Pull method help up to Base");
    assert!(
        result.files["Base.php"].contains("protected function help(): int"),
        "{}",
        result.files["Base.php"]
    );
}

#[test]
fn a_property_moves_up_among_the_properties() {
    let current = "<?php\nnamespace App;\n\nclass Child extends Base\n{\n    protected string $la$0bel = 'x';\n}\n";
    let result = done(&[("Base.php", BASE)], current, "Pull property $label up to Base");
    let base = &result.files["Base.php"];
    assert!(
        base.contains(
            "    protected int $count = 0;\n    protected string $label = 'x';\n\n    public function name()"
        ),
        "{base}"
    );
}

#[test]
fn names_are_imported_in_the_file_the_member_goes_to() {
    let base = "<?php\nnamespace Lib;\n\nabstract class Base {}\n";
    let current = "<?php\nnamespace App;\n\nuse Lib\\Base;\nuse DateTime;\n\nclass Child extends Base\n{\n    public function sta$0mp(): DateTime\n    {\n        return new DateTime();\n    }\n}\n";
    let result = done(&[("Base.php", base)], current, "Pull method stamp up to Base");
    let base = &result.files["Base.php"];
    assert!(base.contains("use DateTime;"), "{base}");
    assert!(base.contains("public function stamp(): DateTime"), "{base}");
}

#[test]
fn refuses_what_the_parent_cannot_support() {
    let current = "<?php\nnamespace App;\n\nclass Child extends Base\n{\n    private int $own = 1;\n\n    public function sh$0ow(): int\n    {\n        return $this->own;\n    }\n}\n";
    assert_eq!(
        refused(&[("Base.php", BASE)], current, "Pull method show up to Base"),
        "The member needs 'own', which Base does not have"
    );
    let calls_parent = "<?php\nnamespace App;\n\nclass Child extends Base\n{\n    public function na$0me(): string\n    {\n        return parent::name() . 'x';\n    }\n}\n";
    assert_eq!(
        refused(&[("Base.php", BASE)], calls_parent, "Pull method name up to Base"),
        "Base has a member of this name already"
    );
}

#[test]
fn never_changes_a_package() {
    let vendor = "<?php\nnamespace Pkg;\n\nabstract class Base {}\n";
    let current =
        "<?php\nnamespace App;\n\nclass Child extends \\Pkg\\Base\n{\n    public function gre$0et(): void {}\n}\n";
    let reason = refused(
        &[("vendor/pkg/Base.php", vendor)],
        current,
        "Pull method greet up to Base",
    );
    assert!(
        reason.contains("is in a package or the standard library, which is not changed"),
        "{reason}"
    );
}

#[test]
fn declares_a_method_in_an_interface_and_keeps_it() {
    let interface = "<?php\nnamespace App;\n\ninterface Greets\n{\n}\n";
    let current = "<?php\nnamespace App;\n\nclass Child implements Greets\n{\n    public function gre$0et(string $who, int $times = 1): string\n    {\n        return $who;\n    }\n}\n";
    let result = done(
        &[("Greets.php", interface)],
        current,
        "Declare greet in interface Greets",
    );
    assert!(!result.files.contains_key("current.php"), "the class keeps its method");
    assert_eq!(
        result.files["Greets.php"],
        "<?php\nnamespace App;\n\ninterface Greets\n{\n    public function greet(string $who, int $times = 1): string;\n}\n"
    );
}

#[test]
fn refuses_to_declare_what_another_implementer_lacks() {
    let interface = "<?php\nnamespace App;\n\ninterface Greets\n{\n}\n";
    let other = "<?php\nnamespace App;\n\nclass Other implements Greets {}\n";
    let current =
        "<?php\nnamespace App;\n\nclass Child implements Greets\n{\n    public function gre$0et(): void {}\n}\n";
    assert_eq!(
        refused(
            &[("Greets.php", interface), ("Other.php", other)],
            current,
            "Declare greet in interface Greets"
        ),
        "Other would be left without the method"
    );
}

#[test]
fn declares_an_abstract_method_in_an_abstract_parent() {
    let current = "<?php\nnamespace App;\n\nclass Child extends Base\n{\n    public function gre$0et(int $n): int\n    {\n        return $n;\n    }\n}\n";
    let result = done(&[("Base.php", BASE)], current, "Declare greet in Base");
    assert!(
        result.files["Base.php"].contains("    abstract public function greet(int $n): int;\n"),
        "{}",
        result.files["Base.php"]
    );
}

#[test]
fn pushes_a_method_down_to_every_subclass() {
    let base = "<?php\nnamespace App;\n\nclass Base\n{\n    public function sh$0are(int $a): int\n    {\n        return $a + 1;\n    }\n\n    public function keep(): void {}\n}\n";
    let one = "<?php\nnamespace App;\n\nclass One extends Base\n{\n}\n";
    let two = "<?php\nnamespace App;\n\nclass Two extends Base\n{\n    public function mine(): void {}\n}\n";
    let result = done_with_current(
        &[("One.php", one), ("Two.php", two)],
        base,
        "Push method share down to 2 subclasses",
    );
    assert_eq!(
        result.files["current.php"],
        "<?php\nnamespace App;\n\nclass Base\n{\n    public function keep(): void {}\n}\n"
    );
    assert_eq!(
        result.files["One.php"],
        "<?php\nnamespace App;\n\nclass One extends Base\n{\n    public function share(int $a): int\n    {\n        return $a + 1;\n    }\n}\n"
    );
    assert!(result.files["Two.php"].ends_with("    public function mine(): void {}\n\n    public function share(int $a): int\n    {\n        return $a + 1;\n    }\n}\n"), "{}", result.files["Two.php"]);
}

fn done_with_current(files: &[(&str, &str)], source: &str, title: &str) -> super::Done {
    done(files, source, title)
}

#[test]
fn refuses_to_push_down_what_is_used_above() {
    let base = "<?php\nnamespace App;\n\nclass Base\n{\n    public function sh$0are(): int\n    {\n        return 1;\n    }\n\n    public function use(): int\n    {\n        return $this->share();\n    }\n}\n";
    let one = "<?php\nnamespace App;\n\nclass One extends Base\n{\n}\n";
    let reason = refused(&[("One.php", one)], base, "Push method share down to the subclass");
    assert!(reason.starts_with("The member is used in"), "{reason}");
}

#[test]
fn offers_push_down_only_where_there_is_something_below() {
    let source = "<?php\nnamespace App;\n\nclass Base\n{\n    public function sh$0are(): int\n    {\n        return 1;\n    }\n}\n";
    assert!(!offered(&[], source).iter().any(|title| title.contains("down")));
    let _ = applied;
}
