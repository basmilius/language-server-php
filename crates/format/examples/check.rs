//! Formats every PHP file under the given folders and holds the formatter to its promises: the
//! tokens stay, and a second pass changes nothing. Prints the files that break one and a summary.
//!
//! `cargo run --release -p php-format --example check -- <folder>... [--tabs] [--wrap] [--nowrap]
//! [--align] [--width <n>] [--typing] [--show <file>]`. `--wrap` breaks lines at 80, `--align` aligns arrows and
//! assignments, `--typing` types a newline, a brace and a semicolon at points in every file to see
//! that nothing panics, and `--show` prints one file formatted.

use std::path::{Path, PathBuf};

use php_format::{FormatOptions, Indent, Refusal, on_type_edits, same_tokens, try_format};

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().into_owned();
        if path.is_dir() {
            if name != ".git" && name != "node_modules" && name != "vendor" {
                walk(&path, out);
            }
        } else if path.extension().is_some_and(|extension| extension == "php") {
            out.push(path);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut options = FormatOptions::default();
    if args.iter().any(|arg| arg == "--tabs") {
        options.indent = Indent::Tab;
    }
    if args.iter().any(|arg| arg == "--wrap") {
        options.line_length = 80;
    }
    if let Some(at) = args.iter().position(|arg| arg == "--width") {
        options.line_length = args[at + 1].parse().expect("a number after --width");
    }
    if args.iter().any(|arg| arg == "--nowrap") {
        options.line_length = 0;
    }
    if args.iter().any(|arg| arg == "--align") {
        options.align_assignments = true;
        options.align_array_arrows = true;
    }
    if let Some(at) = args.iter().position(|arg| arg == "--unchecked") {
        let text = std::fs::read_to_string(&args[at + 1]).expect("readable");
        let formatted = php_format::format_unchecked(&text, &options).expect("laid out");
        std::fs::write("/tmp/unchecked.php", &formatted).expect("written");
        println!("{}", same_tokens(&text, &formatted));
        return;
    }
    if let Some(at) = args.iter().position(|arg| arg == "--show") {
        let text = std::fs::read_to_string(&args[at + 1]).expect("readable");
        match try_format(&text, &options) {
            Ok(formatted) => print!("{formatted}"),
            Err(refusal) => eprintln!("not formatted: {refusal:?}"),
        }
        return;
    }
    let mut files = Vec::new();
    let width_at = args.iter().position(|arg| arg == "--width");
    for (at, arg) in args.iter().enumerate() {
        if arg.starts_with("--") || width_at.is_some_and(|width| at == width + 1) {
            continue;
        }
        walk(Path::new(arg), &mut files);
    }
    if args.iter().any(|arg| arg == "--typing") {
        let mut probes = 0;
        for path in &files {
            let Ok(text) = std::fs::read_to_string(path) else {
                continue;
            };
            let stride = (text.len() / 12).max(1);
            for offset in (0..=text.len()).step_by(stride).filter(|at| text.is_char_boundary(*at)) {
                for typed in ['\n', '}', ';'] {
                    let _ = on_type_edits(&text, offset, typed, &options);
                    probes += 1;
                }
            }
        }
        println!("{probes} probes in {} files, no panic", files.len());
        return;
    }
    let (mut formatted_files, mut skipped, mut changed, mut broken) = (0, 0, 0, 0);
    let mut refused = [0usize; 4];
    for path in &files {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let once = match try_format(&text, &options) {
            Ok(once) => once,
            Err(refusal) => {
                skipped += 1;
                refused[refusal as usize] += 1;
                if refusal == Refusal::ChangedTokens {
                    println!("tokens would change: {}", path.display());
                }
                continue;
            }
        };
        formatted_files += 1;
        if once != text {
            changed += 1;
        }
        let twice = try_format(&once, &options).ok();
        if twice.as_deref() != Some(once.as_str()) {
            broken += 1;
            println!("not idempotent: {}", path.display());
        } else if !same_tokens(&text, &once) {
            broken += 1;
            println!("tokens changed: {}", path.display());
        }
    }
    println!(
        "{} files: {formatted_files} formatted ({changed} changed), {skipped} skipped, {broken} broken",
        files.len()
    );
    println!(
        "refused: {} syntax errors, {} markup, {} changed tokens, {} empty",
        refused[0], refused[1], refused[2], refused[3]
    );
}
