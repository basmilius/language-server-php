//! `cargo run --release -p php-syntax --example corpus -- stubs|phpt|dir=<path> [--oracle] [--verbose] [--show=<name>]`
//! Parses a fetched corpus (`scripts/fetch-corpus.sh`) and reports files with syntax errors.
//! With `--oracle` the system `php -l` settles whether PHP itself agrees with a verdict.

use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Instant;

use php_syntax::parse;

fn walk(dir: &Path, extension: &str, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_symlink() {
            continue;
        }
        if path.is_dir() {
            if path
                .file_name()
                .is_some_and(|name| name == ".git" || name == "node_modules")
            {
                continue;
            }
            walk(&path, extension, out);
        } else if path.extension().is_some_and(|found| found == extension) {
            out.push(path);
        }
    }
    out.sort();
}

/// The `--FILE--` section of a phpt and whether its expected output announces a parse error.
fn phpt_sections(text: &str) -> Option<(String, bool)> {
    let mut sections: Vec<(String, String)> = Vec::new();
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_end();
        if trimmed.len() > 4
            && trimmed.starts_with("--")
            && trimmed.ends_with("--")
            && trimmed[2..trimmed.len() - 2]
                .chars()
                .all(|c| c.is_ascii_uppercase() || c == '_')
        {
            sections.push((trimmed[2..trimmed.len() - 2].to_string(), String::new()));
        } else if let Some(last) = sections.last_mut() {
            last.1.push_str(line);
        }
    }
    let file = sections.iter().find(|(name, _)| name == "FILE")?.1.clone();
    let expects_error = sections
        .iter()
        .filter(|(name, _)| name.starts_with("EXPECT"))
        .any(|(_, body)| {
            body.contains("Parse error:") || body.contains("syntax error,") || body.contains("Parse error")
        });
    Some((file, expects_error))
}

/// What `php -l` says: `None` when PHP accepts the file, else its first line of complaint.
fn php_verdict(text: &str) -> Option<Option<String>> {
    let mut child = Command::new("php")
        .args(["-l", "-d", "display_errors=1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    child.stdin.take()?.write_all(text.as_bytes()).ok()?;
    let output = child.wait_with_output().ok()?;
    if output.status.success() {
        return Some(None);
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .find(|line| line.contains("error:"))
        .unwrap_or("php rejected it");
    let message = line
        .split("error:")
        .nth(1)
        .unwrap_or(line)
        .split(" in ")
        .next()
        .unwrap_or(line)
        .trim();
    Some(Some(message.to_string()))
}

fn php_accepts(text: &str) -> Option<bool> {
    php_verdict(text).map(|verdict| verdict.is_none())
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str).unwrap_or("stubs");
    let oracle = args.iter().any(|arg| arg == "--oracle");
    let verbose = args.iter().any(|arg| arg == "--verbose");
    let show = args.iter().find_map(|arg| arg.strip_prefix("--show="));
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus");
    let mut files = Vec::new();
    match mode {
        "stubs" => walk(&root.join("phpstorm-stubs"), "php", &mut files),
        dir if dir.starts_with("dir=") => walk(Path::new(&dir[4..]), "php", &mut files),
        list if list.starts_with("list=") => {
            let listed = fs::read_to_string(&list[5..]).unwrap_or_default();
            files.extend(listed.lines().map(PathBuf::from));
        }
        _ => walk(&root.join("php-src"), "phpt", &mut files),
    }
    let started = Instant::now();
    let mut total_bytes = 0usize;
    let mut checked = 0usize;
    let mut false_errors = 0usize;
    let mut missed_errors = 0usize;
    let mut expected_errors = 0usize;
    let mut by_message: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for path in &files {
        let Ok(raw) = fs::read_to_string(path) else {
            continue;
        };
        let (text, expects_error) = if mode == "stubs" || mode.starts_with("dir=") || mode.starts_with("list=") {
            (raw, false)
        } else {
            match phpt_sections(&raw) {
                Some(found) => found,
                None => continue,
            }
        };
        checked += 1;
        total_bytes += text.len();
        let parsed = parse(&text);
        if let Some(needle) = show {
            if path.display().to_string().contains(needle) {
                println!("== {}", path.display());
                for error in parsed.errors() {
                    let offset = u32::from(error.range.start()) as usize;
                    let line_start = text[..offset].rfind('\n').map_or(0, |at| at + 1);
                    let line_end = text[offset..].find('\n').map_or(text.len(), |at| offset + at);
                    println!(
                        "  {}: {}  | {}",
                        offset - line_start,
                        error.message,
                        &text[line_start..line_end]
                    );
                }
            }
        }
        assert_eq!(
            parsed.syntax().text().to_string(),
            text,
            "round trip failed for {}",
            path.display()
        );
        // The stubs declare `exit()` and `die()`, which a user's file may not.
        let has_errors = parsed
            .errors()
            .iter()
            .any(|error| !(mode == "stubs" && error.message.starts_with("Reserved word")));
        let name = path.strip_prefix(&root).unwrap_or(path).display().to_string();
        if expects_error {
            expected_errors += 1;
            if !has_errors {
                let verdict = if oracle {
                    php_verdict(&text)
                } else {
                    Some(Some("unchecked".to_string()))
                };
                if let Some(Some(message)) = verdict {
                    missed_errors += 1;
                    let key = message.split('\'').next().unwrap_or(&message).trim().to_string();
                    by_message.entry(format!("ACCEPTED: {key}")).or_default().push(name);
                }
            }
        } else if has_errors {
            if oracle && php_accepts(&text) == Some(false) {
                expected_errors += 1;
                if args.iter().any(|arg| arg == "--list-agreed") {
                    println!("agreed: {name}");
                }
                continue;
            }
            false_errors += 1;
            let error = parsed
                .errors()
                .iter()
                .find(|error| !error.message.starts_with("Reserved word"))
                .unwrap_or(&parsed.errors()[0]);
            let offset = u32::from(error.range.start()) as usize;
            let line = text[..offset].matches('\n').count() + 1;
            by_message
                .entry(error.message.clone())
                .or_default()
                .push(format!("{name}:{line}"));
        }
    }
    let elapsed = started.elapsed();
    println!("{checked} files, {} MB, {:?}", total_bytes / 1_000_000, elapsed);
    println!(
        "expected to fail: {expected_errors}, parsed with errors but valid: {false_errors}, accepted but invalid: {missed_errors}"
    );
    for (message, places) in &by_message {
        println!("\n{message}: {}", places.len());
        for place in places.iter().take(if verbose { 1000 } else { 4 }) {
            println!("    {place}");
        }
    }
}
