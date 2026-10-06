use super::{Setup, done_with, offered_with, refused_with};

const USER: &str = "<?php\n\ndeclare(strict_types=1);\n\nnamespace App\\Models;\n\nuse App\\Support\\Name;\nuse App\\Support\\Unused;\n\nclass Us$0er extends Base\n{\n    public function __construct(private string $name) {}\n\n    /**\n     * The name, as written.\n     */\n    public function name(): Name\n    {\n        return new Name($this->name);\n    }\n\n    public static function make(string $name = 'x'): static\n    {\n        return new static($name);\n    }\n\n    private function secret(): void {}\n}\n";

fn files() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "src/Models/Base.php",
            "<?php\nnamespace App\\Models;\n\nclass Base {}\n",
        ),
        (
            "src/Support/Name.php",
            "<?php\nnamespace App\\Support;\n\nclass Name { public function __construct(string $value) {} }\n",
        ),
        (
            "src/Support/Unused.php",
            "<?php\nnamespace App\\Support;\n\nclass Unused {}\n",
        ),
    ]
}

fn setup() -> Setup {
    Setup {
        current: "src/Models/User.php",
        composer: Some(r#"{"autoload":{"psr-4":{"App\\":"src/"}}}"#),
    }
}

#[test]
fn makes_an_interface_of_the_public_methods_that_the_class_implements() {
    let result = done_with(setup(), &files(), USER, "Extract interface UserInterface");
    assert_eq!(
        result.files["src/Models/UserInterface.php"],
        "<?php\n\ndeclare(strict_types=1);\n\nnamespace App\\Models;\n\nuse App\\Support\\Name;\n\ninterface UserInterface\n{\n    /**\n     * The name, as written.\n     */\n    public function name(): Name;\n\n    public static function make(string $name = 'x'): static;\n}\n"
    );
    assert!(
        result
            .text
            .contains("class User extends Base implements UserInterface\n"),
        "{}",
        result.text
    );
    let focus = result.change.focus.expect("a focus");
    assert_eq!(&result.text[focus.start as usize..focus.end as usize], "UserInterface");
}

#[test]
fn joins_what_the_class_implements_already() {
    let source = "<?php\nnamespace App\\Models;\n\nclass Te$0am implements \\Countable\n{\n    public function count(): int\n    {\n        return 0;\n    }\n}\n";
    let result = done_with(setup(), &[], source, "Extract interface TeamInterface");
    assert!(
        result
            .text
            .contains("class Team implements \\Countable, TeamInterface\n"),
        "{}",
        result.text
    );
}

#[test]
fn is_offered_on_a_class_with_public_methods_only() {
    let empty = "<?php\nnamespace App\\Models;\n\nclass Em$0pty\n{\n    private function hidden(): void {}\n}\n";
    assert!(
        !offered_with(setup(), &[], empty)
            .iter()
            .any(|title| title.starts_with("Extract interface"))
    );
}

#[test]
fn refuses_a_name_that_is_taken_and_a_parameter_of_the_class_itself() {
    let mut taken = files();
    taken.push((
        "src/Models/UserInterface.php",
        "<?php\nnamespace App\\Models;\n\ninterface UserInterface {}\n",
    ));
    assert_eq!(
        refused_with(setup(), &taken, USER, "Extract interface UserInterface"),
        "'App\\Models\\UserInterface' is taken"
    );
    let source = "<?php\nnamespace App\\Models;\n\nclass Mo$0ney\n{\n    public function add(self $other): static\n    {\n        return $this;\n    }\n}\n";
    assert_eq!(
        refused_with(setup(), &[], source, "Extract interface MoneyInterface"),
        "A parameter of add() names the class itself"
    );
}
