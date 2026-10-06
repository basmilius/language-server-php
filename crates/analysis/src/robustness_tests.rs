//! Broken code is what an editor sends most of the time: every prefix of a file must be answered
//! without a panic by every position-based analysis.

use std::path::Path;

use php_syntax::{PhpVersion, TextRange, TextSize, parse};

use crate::hierarchy::{prepare_call_hierarchy, prepare_type_hierarchy};
use crate::inlay_hints::{HintOptions, inlay_hints};
use crate::references::{Current, highlights_at};
use crate::rename::prepare_rename;
use crate::semantic_tokens::semantic_tokens;
use crate::signature::signature_help;
use crate::testing::Fixture;

const SAMPLE: &str = r#"<?php
declare(strict_types=1);
namespace App\Http;

use App\Models\{User, Post as Article};
use function strlen;

/**
 * @template T of User
 * @property-read int $size
 * @method static static make(int ...$flags)
 */
abstract class Controller extends Base implements \Countable
{
    use Loud { run as protected go; Loud::x insteadof Quiet; }
    public const MAX = 10;
    public function __construct(private readonly string $name, protected ?User $user = null) {}
    /** @param array<int, Article> $items @return list<T> */
    public function index(array $items, int ...$rest): static
    {
        $total = array_map(fn($x) => $x->title, $items);
        foreach ($items as $key => ['a' => $a]) {
            echo "{$this->name} $key", self::MAX, static::class;
        }
        $f = function () use (&$total): void { $total[] = strlen($this->name); };
        return $this->index(items: $total, rest: 1)->other?->call(...);
    }
    abstract protected function render(): string;
}
enum Suit: string { case Hearts = 'h'; public function label(): string { return $this->value; } }
"#;

#[test]
fn every_prefix_of_a_file_is_answered() {
    let fixture = Fixture::with_level(PhpVersion::V8_4, &[("sample.php", SAMPLE)], &[]);
    let path = Path::new("/project/current.php");
    let mut cut = 0;
    while cut <= SAMPLE.len() {
        if !SAMPLE.is_char_boundary(cut) {
            cut += 1;
            continue;
        }
        let text = &SAMPLE[..cut];
        let root = parse(text).syntax();
        let length = text.len() as u32;
        for token in semantic_tokens(&fixture.index, &root, None) {
            assert!(token.end <= length && token.start < token.end, "prefix {cut}");
        }
        let half = TextRange::new(TextSize::from(length / 3), TextSize::from(length));
        let _ = semantic_tokens(&fixture.index, &root, Some(half));
        let _ = inlay_hints(&fixture.index, &root, None, HintOptions::default());
        let current = Current {
            path,
            text,
            root: &root,
        };
        for offset in [length.saturating_sub(1), length, length / 2] {
            let _ = signature_help(&fixture.index, &root, offset);
            let _ = highlights_at(&fixture.index, &crate::references::NoSources, &current, offset);
            let _ = prepare_rename(&fixture.index, &root, text, offset);
            let _ = prepare_call_hierarchy(&fixture.index, &root, offset);
            let _ = prepare_type_hierarchy(&fixture.index, &root, offset);
        }
        cut += 7;
    }
}
