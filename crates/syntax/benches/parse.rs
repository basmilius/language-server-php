//! `cargo bench -p php-syntax` measures lexing and parsing on a large synthetic file and, when the
//! corpus is fetched, on all of phpstorm-stubs. Benchmarks are not part of `cargo test`.

use std::fs;
use std::hint::black_box;
use std::path::{Path, PathBuf};

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use php_syntax::lexer::lex;
use php_syntax::parse;

const UNIT: &str = r#"
/**
 * A user of the application.
 */
#[Entity('users')]
final class User extends Base implements \JsonSerializable
{
    use Timestamps;

    public const int MAX = 10;

    public function __construct(private readonly string $name, public ?int $age = null) {}

    public function name(): string
    {
        return "Name: {$this->name} $this->age " . strtoupper($this->name);
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
        if ($user->age > 3 && $user->name !== '') {
            $user->age = $user->age * 2 + 1;
        } elseif ($user->age === null) {
            $user->age = 0;
        }
        return $user;
    }
}
"#;

fn large_file(units: usize) -> String {
    let mut text = String::from("<?php\nnamespace App\\Models;\n");
    for index in 0..units {
        text.push_str(&UNIT.replace("User", &format!("User{index}")));
    }
    text
}

fn corpus_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == ".git") {
                continue;
            }
            corpus_files(&path, out);
        } else if path.extension().is_some_and(|extension| extension == "php") {
            out.push(path);
        }
    }
}

fn benchmarks(criterion: &mut Criterion) {
    let large = large_file(1500);
    let mut group = criterion.benchmark_group("large file");
    group.throughput(Throughput::Bytes(large.len() as u64));
    group.bench_function("lex", |bencher| bencher.iter(|| black_box(lex(black_box(&large)))));
    group.bench_function("parse", |bencher| bencher.iter(|| black_box(parse(black_box(&large)))));
    group.finish();

    let medium = large_file(60);
    let mut group = criterion.benchmark_group("typical file");
    group.throughput(Throughput::Bytes(medium.len() as u64));
    group.bench_function("parse", |bencher| bencher.iter(|| black_box(parse(black_box(&medium)))));
    group.finish();

    let mut files = Vec::new();
    corpus_files(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/phpstorm-stubs"),
        &mut files,
    );
    if files.is_empty() {
        eprintln!("phpstorm-stubs are not fetched: skipping the corpus benchmark");
        return;
    }
    let sources: Vec<String> = files.iter().filter_map(|path| fs::read_to_string(path).ok()).collect();
    let total: usize = sources.iter().map(String::len).sum();
    let mut group = criterion.benchmark_group("phpstorm-stubs");
    group.sample_size(20);
    group.throughput(Throughput::Bytes(total as u64));
    group.bench_function("parse all", |bencher| {
        bencher.iter(|| {
            for source in &sources {
                black_box(parse(black_box(source)));
            }
        })
    });
    group.finish();
}

criterion_group!(benches, benchmarks);
criterion_main!(benches);
