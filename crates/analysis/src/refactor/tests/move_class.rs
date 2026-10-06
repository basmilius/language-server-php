use super::{Setup, done_with, offered_with, refused_with};

const COMPOSER: &str = r#"{"autoload":{"psr-4":{"App\\":"src/"}}}"#;

fn setup(current: &'static str) -> Setup {
    Setup {
        current,
        composer: Some(COMPOSER),
    }
}

const USER: &str = "<?php\nnamespace App\\Models;\n\nuse App\\Contracts\\Named;\n\nclass Us$0er implements Named\n{\n    public function other(): Team\n    {\n        return new Team();\n    }\n}\n";

const TEAM: &str = "<?php\nnamespace App\\Models;\n\nclass Team {}\n";
const NAMED: &str = "<?php\nnamespace App\\Contracts;\n\ninterface Named {}\n";

#[test]
fn moves_the_class_and_follows_it_in_its_own_file_and_in_others() {
    let service = "<?php\nnamespace App\\Services;\n\nuse App\\Models\\User;\nuse App\\Models\\Team;\n\nfunction f(User $user, Team $team): ?User\n{\n    return new User();\n}\n";
    let sibling = "<?php\nnamespace App\\Models;\n\nclass Admin extends User\n{\n    /** @return User|null */\n    public function make(): ?User\n    {\n        return \\App\\Models\\User::class === static::class ? new User() : null;\n    }\n}\n";
    let result = done_with(
        setup("src/Models/User.php"),
        &[
            ("src/Models/Team.php", TEAM),
            ("src/Contracts/Named.php", NAMED),
            ("src/Services/f.php", service),
            ("src/Models/Admin.php", sibling),
            (
                "src/Domain/Order.php",
                "<?php\nnamespace App\\Domain;\n\nclass Order {}\n",
            ),
        ],
        USER,
        "Move class to namespace App\\Domain",
    );
    assert_eq!(
        result.moves,
        [("src/Models/User.php".to_string(), "src/Domain/User.php".to_string())]
    );
    assert_eq!(
        result.files["src/Models/User.php"],
        "<?php\nnamespace App\\Domain;\n\nuse App\\Contracts\\Named;\nuse App\\Models\\Team;\n\nclass User implements Named\n{\n    public function other(): Team\n    {\n        return new Team();\n    }\n}\n"
    );
    assert!(
        result.files["src/Services/f.php"].contains("use App\\Domain\\User;\nuse App\\Models\\Team;"),
        "{}",
        result.files["src/Services/f.php"]
    );
    let admin = &result.files["src/Models/Admin.php"];
    assert!(admin.contains("use App\\Domain\\User;"), "{admin}");
    assert!(
        admin.contains("\\App\\Domain\\User::class === static::class ? new User() : null"),
        "{admin}"
    );
    assert!(admin.contains("extends User"), "{admin}");
}

#[test]
fn offers_the_namespaces_of_the_project() {
    let titles = offered_with(
        setup("src/Models/User.php"),
        &[("src/Contracts/Named.php", NAMED), ("src/Models/Team.php", TEAM)],
        USER,
    );
    assert!(
        titles.contains(&"Move class to namespace App\\Contracts".to_string()),
        "{titles:?}"
    );
    assert!(
        !titles
            .iter()
            .any(|title| title == "Move class to namespace App\\Models"),
        "{titles:?}"
    );
}

#[test]
fn a_class_outside_a_composer_root_is_not_offered_to_move() {
    let titles = offered_with(
        Setup {
            current: "lib/User.php",
            composer: Some(COMPOSER),
        },
        &[],
        USER,
    );
    assert!(
        !titles.iter().any(|title| title.starts_with("Move class")),
        "{titles:?}"
    );
}

#[test]
fn a_file_that_does_not_match_its_namespace_can_follow_either() {
    let source = "<?php\nnamespace App\\Wrong;\n\nclass Us$0er {}\n";
    let titles = offered_with(setup("src/Models/User.php"), &[], source);
    assert!(
        titles.contains(&"Change the class to 'App\\Models\\User', as its file says".to_string()),
        "{titles:?}"
    );
    assert!(
        titles.contains(&"Move the file to src/Wrong/User.php".to_string()),
        "{titles:?}"
    );
    let result = done_with(
        setup("src/Models/User.php"),
        &[],
        source,
        "Change the class to 'App\\Models\\User', as its file says",
    );
    assert_eq!(result.text, "<?php\nnamespace App\\Models;\n\nclass User {}\n");
    assert!(result.moves.is_empty());
}

#[test]
fn moving_into_the_namespace_a_file_already_uses_needs_no_import() {
    let other = "<?php\nnamespace App\\Models;\n\nclass Admin\n{\n    public function make(): User\n    {\n        return new User();\n    }\n}\n";
    let source = "<?php\nnamespace App\\Contracts;\n\nclass Us$0er {}\n";
    let result = done_with(
        Setup {
            current: "src/Contracts/User.php",
            composer: Some(COMPOSER),
        },
        &[("src/Models/Admin.php", other)],
        source,
        "Move class to namespace App\\Models",
    );
    let _ = result;
}

#[test]
fn refuses_a_class_that_is_taken() {
    let taken = "<?php\nnamespace App\\Domain;\n\nclass User {}\n";
    let titles = offered_with(
        setup("src/Models/User.php"),
        &[
            ("src/Domain/User.php", taken),
            (
                "src/Domain/Other.php",
                "<?php\nnamespace App\\Domain;\n\nclass Other {}\n",
            ),
        ],
        USER,
    );
    assert!(
        !titles.contains(&"Move class to namespace App\\Domain".to_string()),
        "{titles:?}"
    );
    let _ = refused_with;
}

use super::files_moved;

#[test]
fn renaming_a_file_renames_the_class_and_what_names_it() {
    let service = "<?php\nnamespace App\\Services;\n\nuse App\\Models\\User;\n\nfunction f(User $user): ?User\n{\n    return User::make();\n}\n";
    let user = "<?php\nnamespace App\\Models;\n\nclass User\n{\n    public static function make(): static\n    {\n        return new static();\n    }\n\n    public function again(): User\n    {\n        return new User();\n    }\n}\n";
    let result = files_moved(
        setup("src/Models/User.php"),
        &[("src/Models/User.php", user), ("src/Services/f.php", service)],
        &[("src/Models/User.php", "src/Models/Person.php")],
    )
    .expect("followed")
    .expect("a class that follows its file");
    assert!(result.moves.is_empty(), "the client moves the file itself");
    let person = &result.files["src/Models/User.php"];
    assert!(person.contains("class Person\n"), "{person}");
    assert!(person.contains("public function again(): Person\n"), "{person}");
    assert!(person.contains("return new Person();"), "{person}");
    let service = &result.files["src/Services/f.php"];
    assert!(service.contains("use App\\Models\\Person;"), "{service}");
    assert!(service.contains("function f(Person $user): ?Person"), "{service}");
    assert!(service.contains("return Person::make();"), "{service}");
}

#[test]
fn moving_a_folder_moves_every_class_in_it_together() {
    let a = "<?php\nnamespace App\\Old;\n\nclass A\n{\n    public function b(): B\n    {\n        return new B();\n    }\n}\n";
    let b = "<?php\nnamespace App\\Old;\n\nclass B {}\n";
    let user = "<?php\nnamespace App\\Other;\n\nuse App\\Old\\A;\nuse App\\Old\\B;\n\nfunction f(A $a): B\n{\n    return $a->b();\n}\n";
    let result = files_moved(
        setup("src/Old/A.php"),
        &[("src/Old/A.php", a), ("src/Old/B.php", b), ("src/Other/f.php", user)],
        &[
            ("src/Old/A.php", "src/New/Deep/A.php"),
            ("src/Old/B.php", "src/New/Deep/B.php"),
        ],
    )
    .expect("followed")
    .expect("classes that follow their files");
    assert!(
        result.files["src/Old/A.php"].starts_with("<?php\nnamespace App\\New\\Deep;\n"),
        "{}",
        result.files["src/Old/A.php"]
    );
    assert!(
        result.files["src/Old/B.php"].starts_with("<?php\nnamespace App\\New\\Deep;\n"),
        "{}",
        result.files["src/Old/B.php"]
    );
    assert!(
        result.files["src/Other/f.php"].contains("use App\\New\\Deep\\A;\nuse App\\New\\Deep\\B;"),
        "{}",
        result.files["src/Other/f.php"]
    );
}

#[test]
fn a_file_that_never_followed_its_namespace_is_left_alone() {
    let wrong = "<?php\nnamespace App\\Wrong;\n\nclass User {}\n";
    let result = files_moved(
        setup("src/Models/User.php"),
        &[("src/Models/User.php", wrong)],
        &[("src/Models/User.php", "src/Domain/User.php")],
    )
    .expect("fine");
    assert!(result.is_none());
}

#[test]
fn a_group_import_gives_the_class_up_to_a_statement_of_its_own() {
    let user = "<?php\nnamespace App\\Models;\n\nclass User {}\n";
    let team = "<?php\nnamespace App\\Models;\n\nclass Team {}\n";
    let service =
        "<?php\nnamespace App\\Services;\n\nuse App\\Models\\{User, Team};\n\nfunction f(User $user, Team $team) {}\n";
    let result = files_moved(
        setup("src/Models/User.php"),
        &[
            ("src/Models/User.php", user),
            ("src/Models/Team.php", team),
            ("src/Services/f.php", service),
        ],
        &[("src/Models/User.php", "src/Domain/User.php")],
    )
    .expect("followed")
    .expect("a class that follows its file");
    let service = &result.files["src/Services/f.php"];
    assert!(
        service.contains("use App\\Models\\{Team};") || service.contains("use App\\Models\\Team;"),
        "{service}"
    );
    assert!(service.contains("use App\\Domain\\User;"), "{service}");
}

#[test]
fn a_class_moving_to_the_global_namespace_loses_its_namespace_and_the_names_it_used_get_imports() {
    let helper = "<?php\nnamespace Models;\n\nfunction helper(): int\n{\n    return 1;\n}\n";
    let user = "<?php\nnamespace Models;\n\nclass User\n{\n    public function run(): int\n    {\n        return helper() + strlen('a');\n    }\n}\n";
    let composer = r#"{"autoload":{"psr-4":{"":"src/"}}}"#;
    let result = files_moved(
        Setup {
            current: "src/Models/User.php",
            composer: Some(composer),
        },
        &[("src/Models/User.php", user), ("src/Models/helper.php", helper)],
        &[("src/Models/User.php", "src/User.php")],
    )
    .expect("followed")
    .expect("a class that follows its file");
    let moved = &result.files["src/Models/User.php"];
    assert!(!moved.contains("namespace"), "{moved}");
    assert!(moved.contains("use function Models\\helper;"), "{moved}");
}

#[test]
fn refuses_a_name_another_class_has() {
    let user = "<?php\nnamespace App\\Models;\n\nclass User {}\n";
    let taken = "<?php\nnamespace App\\Domain;\n\nclass User {}\n";
    let reason = files_moved(
        setup("src/Models/User.php"),
        &[("src/Models/User.php", user), ("src/Domain/User.php", taken)],
        &[("src/Models/User.php", "src/Domain/User.php")],
    )
    .expect_err("refused");
    assert_eq!(reason, "A class named 'App\\Domain\\User' already exists");
}

#[test]
fn refuses_a_file_with_two_classes() {
    let user = "<?php\nnamespace App\\Models;\n\nclass User {}\nclass Other {}\n";
    let result = files_moved(
        setup("src/Models/User.php"),
        &[("src/Models/User.php", user)],
        &[("src/Models/User.php", "src/Domain/User.php")],
    );
    assert!(
        matches!(result, Ok(None)),
        "a file that is not one class is not followed: {:?}",
        result.map(|done| done.map(|done| done.files))
    );
}

#[test]
fn a_string_that_holds_the_class_follows_it_in_its_own_spelling() {
    let routes = "<?php\nnamespace App\\Http;\n\n$map = ['App\\Models\\User', \"\\\\App\\\\Models\\\\User\", 'App\\Models\\User::other', 'App\\Models\\Users'];\n";
    let result = done_with(
        setup("src/Models/User.php"),
        &[
            ("src/Models/Team.php", TEAM),
            ("src/Contracts/Named.php", NAMED),
            ("src/Http/routes.php", routes),
            (
                "src/Domain/Order.php",
                "<?php\nnamespace App\\Domain;\n\nclass Order {}\n",
            ),
        ],
        USER,
        "Move class to namespace App\\Domain",
    );
    assert_eq!(
        result.files["src/Http/routes.php"],
        "<?php\nnamespace App\\Http;\n\n$map = ['App\\Domain\\User', \"\\\\App\\\\Domain\\\\User\", 'App\\Domain\\User::other', 'App\\Models\\Users'];\n"
    );
}
