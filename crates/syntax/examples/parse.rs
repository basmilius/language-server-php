//! `cargo run -p php-syntax --example parse -- file.php [--tree] [--trivia] [--compact]`
//! Prints the syntax errors of a file and, with `--tree`, its tree.

use php_syntax::{DumpOptions, dump, dump_compact, parse};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.iter().find(|arg| !arg.starts_with("--")) else {
        eprintln!("usage: parse <file> [--tree] [--trivia] [--compact]");
        std::process::exit(2);
    };
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            eprintln!("{path}: {error}");
            std::process::exit(1);
        }
    };
    let parsed = parse(&text);
    if args.iter().any(|arg| arg == "--compact") {
        print!("{}", dump_compact(&parsed.syntax()));
    }
    if args.iter().any(|arg| arg == "--tree") {
        let options = DumpOptions {
            trivia: args.iter().any(|arg| arg == "--trivia"),
            ranges: true,
        };
        print!("{}", dump(&parsed.syntax(), options));
    }
    for error in parsed.errors() {
        let offset = u32::from(error.range.start()) as usize;
        let line = text[..offset].matches('\n').count() + 1;
        println!("{path}:{line}: {} ({:?})", error.message, error.range);
    }
    assert_eq!(
        parsed.syntax().text().to_string(),
        text,
        "the tree must hold every byte"
    );
}
