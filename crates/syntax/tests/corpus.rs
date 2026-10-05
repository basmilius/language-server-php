//! Parses the corpora `scripts/fetch-corpus.sh` fetches into `corpus/` and holds the parser to them.
//! Without a corpus these tests report that and pass, so a plain `cargo test` needs no network.

use std::fs;
use std::path::{Path, PathBuf};

use php_syntax::parse;

fn corpus_dir(name: &str) -> Option<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus").join(name);
    if dir.is_dir() {
        Some(dir)
    } else {
        eprintln!(
            "skipped: {} is not fetched (run scripts/fetch-corpus.sh)",
            dir.display()
        );
        None
    }
}

fn walk(dir: &Path, extension: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == ".git") {
                continue;
            }
            walk(&path, extension, out);
        } else if path.extension().is_some_and(|found| found == extension) {
            out.push(path);
        }
    }
    out.sort();
}

#[test]
fn phpstorm_stubs_parse_without_errors() {
    let Some(dir) = corpus_dir("phpstorm-stubs") else {
        return;
    };
    let mut files = Vec::new();
    walk(&dir, "php", &mut files);
    assert!(files.len() > 500, "expected the stubs, found {} files", files.len());
    let mut failures = Vec::new();
    for path in &files {
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        let parsed = parse(&text);
        assert_eq!(
            parsed.syntax().text().to_string(),
            text,
            "round trip of {}",
            path.display()
        );
        // The stubs declare `exit()` and `die()` as functions, which a user's file may not do.
        let error = parsed
            .errors()
            .iter()
            .find(|error| !error.message.starts_with("Reserved word"));
        if let Some(error) = error {
            let offset = u32::from(error.range.start()) as usize;
            let line = text[..offset].matches('\n').count() + 1;
            failures.push(format!("{}:{line}: {}", path.display(), error.message));
        }
    }
    assert!(
        failures.is_empty(),
        "{} stub files have syntax errors:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The `--FILE--` section of a phpt and whether its expected output announces a parse error.
fn phpt_file_section(text: &str) -> Option<(String, bool)> {
    let mut sections: Vec<(String, String)> = Vec::new();
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_end();
        let is_header = trimmed.len() > 4
            && trimmed.starts_with("--")
            && trimmed.ends_with("--")
            && trimmed[2..trimmed.len() - 2]
                .chars()
                .all(|c| c.is_ascii_uppercase() || c == '_');
        if is_header {
            sections.push((trimmed[2..trimmed.len() - 2].to_string(), String::new()));
        } else if let Some(last) = sections.last_mut() {
            last.1.push_str(line);
        }
    }
    let file = sections.iter().find(|(name, _)| name == "FILE")?.1.clone();
    let expects_error = sections
        .iter()
        .filter(|(name, _)| name.starts_with("EXPECT"))
        .any(|(_, body)| body.contains("Parse error"));
    Some((file, expects_error))
}

/// Every php-src test that is valid PHP parses without errors. The tests whose output announces a
/// parse error are left out, and so are the few that PHP rejects at compile time in a way the
/// parser rejects too, which `data/php-src-invalid.txt` lists (found with `php -l`).
#[test]
fn php_src_tests_with_valid_syntax_parse_without_errors() {
    let Some(dir) = corpus_dir("php-src") else {
        return;
    };
    let rejected: Vec<String> =
        fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/php-src-invalid.txt"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect();
    let mut files = Vec::new();
    walk(&dir, "phpt", &mut files);
    assert!(
        files.len() > 3000,
        "expected php-src's tests, found {} files",
        files.len()
    );
    let mut failures = Vec::new();
    let mut checked = 0;
    for path in &files {
        let Ok(raw) = fs::read_to_string(path) else {
            continue;
        };
        let Some((text, expects_error)) = phpt_file_section(&raw) else {
            continue;
        };
        let parsed = parse(&text);
        assert_eq!(
            parsed.syntax().text().to_string(),
            text,
            "round trip of {}",
            path.display()
        );
        let relative = path
            .strip_prefix(&dir)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        // Tens of thousands of nested operators: the parser stops at a fixed depth instead of the stack's.
        let stress = relative.contains("/stack_limit/");
        if expects_error || stress || rejected.contains(&relative) {
            continue;
        }
        checked += 1;
        if let Some(error) = parsed.errors().first() {
            failures.push(format!("{relative}: {}", error.message));
        }
    }
    assert!(checked > 3000);
    assert!(
        failures.is_empty(),
        "{} php-src tests have syntax errors:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
