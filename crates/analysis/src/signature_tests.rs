use php_syntax::PhpVersion;

use crate::signature::signature_help;
use crate::testing::{Fixture, split_cursor};

fn help(files: &[(&str, &str)], stubs: &[(&str, &str)], current: &str) -> String {
    let fixture = Fixture::with_level(PhpVersion::V8_4, files, stubs).with_current(current);
    let (_, root, offset) = split_cursor(current);
    match signature_help(&fixture.index, &root, offset) {
        None => "none".to_string(),
        Some(found) => {
            let mut out = String::new();
            for (position, signature) in found.signatures.iter().enumerate() {
                let marker = if position == found.active_signature { ">" } else { " " };
                out.push_str(&format!(
                    "{marker} {} [{}]\n",
                    signature.label,
                    signature
                        .active_parameter
                        .map_or("-".to_string(), |active| active.to_string())
                ));
            }
            out
        }
    }
}

const LIB: &str = "<?php\nnamespace App;\n\nclass Mailer {\n    /** Sends a message. */\n    public function send(string $to, string $subject = '', int ...$flags): bool {}\n    public static function make(int $port = 25): static {}\n    public function __construct(private string $host, public ?int $port = null) {}\n}\n\nfunction join_all(string $glue, string ...$parts): string {}\n";

#[test]
fn marks_the_parameter_under_the_cursor() {
    let current = "<?php\nuse App\\Mailer;\nfunction f(Mailer $m) {\n    $m->send('a', $0);\n}\n";
    assert_eq!(
        help(&[("lib.php", LIB)], &[], current),
        "> send(string $to, string $subject = '', int ...$flags): bool [1]\n"
    );
    let current = "<?php\nuse App\\Mailer;\nfunction f(Mailer $m) {\n    $m->send($0);\n}\n";
    assert!(help(&[("lib.php", LIB)], &[], current).ends_with("[0]\n"));
}

#[test]
fn variadic_parameters_take_every_extra_argument() {
    let current = "<?php\nuse function App\\join_all;\njoin_all(',', 'a', 'b', 'c'$0);\n";
    assert_eq!(
        help(&[("lib.php", LIB)], &[], current),
        "> join_all(string $glue, string ...$parts): string [1]\n"
    );
}

#[test]
fn named_arguments_pick_their_parameter() {
    let current = "<?php\nuse App\\Mailer;\nfunction f(Mailer $m) {\n    $m->send(subject: $0);\n}\n";
    assert!(help(&[("lib.php", LIB)], &[], current).ends_with("[1]\n"));
}

#[test]
fn constructors_static_calls_and_attributes() {
    let current = "<?php\nuse App\\Mailer;\n$m = new Mailer('h', $0);\n";
    assert_eq!(
        help(&[("lib.php", LIB)], &[], current),
        "> __construct(private string $host, public ?int $port = null) [1]\n"
    );
    let current = "<?php\nuse App\\Mailer;\n$m = Mailer::make($0);\n";
    assert_eq!(
        help(&[("lib.php", LIB)], &[], current),
        "> make(int $port = 25): static [0]\n"
    );
    let attribute = "<?php\nuse App\\Mailer;\n#[Mailer($0)]\nclass X {}\n";
    assert!(help(&[("lib.php", LIB)], &[], attribute).starts_with("> __construct("));
}

#[test]
fn overloads_from_the_stubs_choose_by_argument_count() {
    let stubs = [(
        "standard/strings.php",
        "<?php\nfunction strtok(string $string, string $token): string|false {}\nfunction strtok(string $token): string|false {}\n",
    )];
    let current = "<?php\nstrtok($0);\n";
    let one = help(&[], &stubs, current);
    assert!(
        one.contains("> strtok(string $string, string $token)") || one.contains("> strtok(string $token)"),
        "{one}"
    );
    let current = "<?php\nstrtok('a', $0);\n";
    assert_eq!(
        help(&[], &stubs, current),
        "> strtok(string $string, string $token): string|false [1]\n  strtok(string $token): string|false [1]\n"
    );
}

#[test]
fn nothing_outside_a_call() {
    assert_eq!(help(&[], &[], "<?php\nfoo(1);$0\n"), "none");
    assert_eq!(help(&[], &[], "<?php\n$a = [1, $0];\n"), "none");
}

#[test]
fn works_in_an_unfinished_call() {
    let current = "<?php\nuse function App\\join_all;\njoin_all(',', $0\n";
    assert_eq!(
        help(&[("lib.php", LIB)], &[], current),
        "> join_all(string $glue, string ...$parts): string [1]\n"
    );
}
