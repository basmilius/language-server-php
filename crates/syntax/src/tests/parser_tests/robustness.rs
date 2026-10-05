use crate::parse;

const SAMPLE: &str = r#"<?php
declare(strict_types=1);

namespace App\Models;

use Foo\{Bar, Baz as Q};

/**
 * A user.
 */
#[Entity('users')]
final class User extends Base implements \JsonSerializable
{
    use Timestamps;

    public const int MAX = 10;

    public function __construct(private readonly string $name, public ?int $age = null) {}

    public int $score {
        get => $this->score ?? 0;
        set(int $value) { $this->score = max(0, $value); }
    }

    public function name(): string
    {
        return "Name: {$this->name} $this->age ${x} " . <<<EOT
            Hello $this->name
              indented {$this->age}
            EOT;
    }

    public static function make(array $data = []): static
    {
        $user = new static(...$data);
        foreach ($data as $key => &$value) {
            match (true) {
                is_string($value), is_int($value) => $user->{$key} = $value,
                default => throw new \InvalidArgumentException("bad $key"),
            };
        }
        return $user |> trim(...);
    }
}

enum Status: string { case Active = 'a'; case Off = 'o'; }

function f(int|string $a, ?A $b = null, ...$rest): ?array {
    if ($a): echo 1; elseif ($b): echo 2; else: echo 3; endif;
    $fn = fn($y) => $y ** 2 ?? [1, 2, 3][0];
    return [$a, 'k' => $fn, ...$rest];
}
?>
<p><?= $x ?></p>
<?php __halt_compiler(); data
"#;

/// Every prefix of a file is a different broken file: none may panic or lose a byte.
#[test]
fn every_prefix_of_a_file_round_trips() {
    for end in (0..=SAMPLE.len()).filter(|end| SAMPLE.is_char_boundary(*end)) {
        let text = &SAMPLE[..end];
        let parsed = parse(text);
        assert_eq!(parsed.syntax().text().to_string(), text, "prefix of {end} bytes");
    }
}

/// Deleting any single character, and swapping in stray tokens, must still give a whole tree.
#[test]
fn single_edits_round_trip() {
    let strays = [
        "}", ")", "(", "{", "$", "'", "\"", "<<<", "?>", "<?php", "/*", "#[", "->", "::", "=>", "function",
    ];
    let chars: Vec<(usize, char)> = SAMPLE.char_indices().collect();
    for (index, (offset, character)) in chars.iter().enumerate() {
        if index % 3 != 0 {
            continue;
        }
        let deleted = format!("{}{}", &SAMPLE[..*offset], &SAMPLE[offset + character.len_utf8()..]);
        assert_eq!(parse(&deleted).syntax().text().to_string(), deleted);
        for stray in strays {
            let inserted = format!("{}{stray}{}", &SAMPLE[..*offset], &SAMPLE[*offset..]);
            assert_eq!(
                parse(&inserted).syntax().text().to_string(),
                inserted,
                "inserted {stray} at {offset}"
            );
        }
    }
}

#[test]
fn the_sample_has_no_errors() {
    let parsed = parse(SAMPLE);
    assert_eq!(parsed.errors(), &[], "the sample is valid PHP");
}

#[test]
fn deeply_nested_input_does_not_overflow_the_stack() {
    let depth = 100_000;
    let text = format!("<?php {}1{};", "(".repeat(depth), ")".repeat(depth));
    let parsed = parse(&text);
    assert_eq!(parsed.syntax().text().to_string(), text);
    let text = format!("<?php {}", "[".repeat(depth));
    assert_eq!(parse(&text).syntax().text().to_string(), text);
    let text = format!("<?php {}", "{ ".repeat(depth));
    assert_eq!(parse(&text).syntax().text().to_string(), text);
    let text = format!("<?php {}", "fn() => ".repeat(depth));
    assert_eq!(parse(&text).syntax().text().to_string(), text);
}
