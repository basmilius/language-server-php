//! `.editorconfig`: the indentation and the line length a project asks for, found by walking up
//! from a file and read from the files on the way. Only the keys the formatter has a use for are
//! kept, and the sections are matched with the globs of the specification.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::options::{FormatOptions, Indent};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndentStyle {
    Space,
    Tab,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndentSize {
    Spaces(usize),
    /// `indent_size = tab`: the width of a tab.
    Tab,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineLength {
    Limit(usize),
    Off,
}

/// What the files that govern a path say.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditorConfig {
    pub indent_style: Option<IndentStyle>,
    pub indent_size: Option<IndentSize>,
    pub tab_width: Option<usize>,
    pub max_line_length: Option<LineLength>,
}

struct Section {
    glob: String,
    properties: Vec<(String, String)>,
}

struct Parsed {
    root: bool,
    sections: Vec<Section>,
}

fn parse(text: &str) -> Parsed {
    let mut root = false;
    let mut sections: Vec<Section> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(glob) = line.strip_prefix('[').and_then(|rest| rest.strip_suffix(']')) {
            sections.push(Section {
                glob: glob.to_string(),
                properties: Vec::new(),
            });
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim().to_ascii_lowercase(), value.trim().to_ascii_lowercase());
        match sections.last_mut() {
            Some(section) => section.properties.push((key, value)),
            None if key == "root" => root = value == "true",
            None => {}
        }
    }
    Parsed { root, sections }
}

/// The patterns a glob stands for once its `{a,b}` and `{1..3}` are written out.
fn expand_braces(pattern: &str) -> Vec<String> {
    let chars: Vec<char> = pattern.chars().collect();
    let mut depth = 0usize;
    let mut open = None;
    let mut bracket = false;
    for (at, c) in chars.iter().enumerate() {
        match c {
            '\\' => {}
            '[' => bracket = true,
            ']' => bracket = false,
            '{' if !bracket => {
                if depth == 0 {
                    open = Some(at);
                }
                depth += 1;
            }
            '}' if !bracket && depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    let Some(start) = open else {
                        continue;
                    };
                    let inner: String = chars[start + 1..at].iter().collect();
                    let (before, after): (String, String) =
                        (chars[..start].iter().collect(), chars[at + 1..].iter().collect());
                    let choices = alternatives(&inner);
                    if choices.len() < 2 && !inner.contains("..") {
                        // A lone `{x}` stands for itself.
                        return expand_braces(&after)
                            .into_iter()
                            .map(|rest| format!("{before}{{{inner}}}{rest}"))
                            .collect();
                    }
                    return choices
                        .into_iter()
                        .flat_map(|choice| expand_braces(&format!("{before}{choice}{after}")))
                        .collect();
                }
            }
            _ => {}
        }
    }
    vec![pattern.to_string()]
}

fn alternatives(inner: &str) -> Vec<String> {
    if let Some((from, to)) = inner.split_once("..") {
        if let (Ok(from), Ok(to)) = (from.parse::<i64>(), to.parse::<i64>()) {
            let (low, high) = (from.min(to), from.max(to));
            return (low..=high).map(|number| number.to_string()).collect();
        }
    }
    let mut out = Vec::new();
    let mut depth = 0;
    let mut current = String::new();
    for c in inner.chars() {
        match c {
            '{' => {
                depth += 1;
                current.push(c);
            }
            '}' => {
                depth -= 1;
                current.push(c);
            }
            ',' if depth == 0 => out.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    out.push(current);
    out
}

fn glob_matches(pattern: &[char], text: &[char]) -> bool {
    let Some((&first, rest)) = pattern.split_first() else {
        return text.is_empty();
    };
    match first {
        '*' if rest.first() == Some(&'*') => {
            let rest = &rest[1..];
            (0..=text.len()).any(|skip| glob_matches(rest, &text[skip..]))
        }
        '*' => {
            let limit = text.iter().position(|c| *c == '/').unwrap_or(text.len());
            (0..=limit).any(|skip| glob_matches(rest, &text[skip..]))
        }
        '?' => text.first().is_some_and(|c| *c != '/') && glob_matches(rest, &text[1..]),
        '[' => {
            let Some(close) = rest.iter().position(|c| *c == ']') else {
                return text.first() == Some(&'[') && glob_matches(rest, &text[1..]);
            };
            let class = &rest[..close];
            let Some(&c) = text.first() else {
                return false;
            };
            let (negated, class) = match class.split_first() {
                Some((&'!', tail)) => (true, tail),
                _ => (false, class),
            };
            let mut found = false;
            let mut at = 0;
            while at < class.len() {
                if at + 2 < class.len() && class[at + 1] == '-' {
                    found |= (class[at]..=class[at + 2]).contains(&c);
                    at += 3;
                } else {
                    found |= class[at] == c;
                    at += 1;
                }
            }
            c != '/' && found != negated && glob_matches(&rest[close + 1..], &text[1..])
        }
        '\\' => match rest.split_first() {
            Some((&escaped, tail)) => text.first() == Some(&escaped) && glob_matches(tail, &text[1..]),
            None => text.first() == Some(&'\\') && text.len() == 1,
        },
        c => text.first() == Some(&c) && glob_matches(rest, &text[1..]),
    }
}

/// Whether a section's glob names a file, given as the path below the folder of the `.editorconfig`.
fn section_matches(glob: &str, relative: &str) -> bool {
    let anchored = glob.contains('/');
    let glob = glob.trim_start_matches('/');
    let text: Vec<char> = relative.chars().collect();
    expand_braces(glob).iter().any(|pattern| {
        let pattern = if anchored {
            pattern.clone()
        } else {
            format!("**/{pattern}")
        };
        let chars: Vec<char> = pattern.chars().collect();
        glob_matches(&chars, &text) || !anchored && glob_matches(&pattern.chars().skip(3).collect::<Vec<char>>(), &text)
    })
}

impl EditorConfig {
    /// What the `.editorconfig` files from the folder of `path` up to the first `root = true` say,
    /// reading each through `read`. The ones nearer the file win.
    pub fn for_path(path: &Path, read: &dyn Fn(&Path) -> Option<String>) -> EditorConfig {
        let mut layers: Vec<(PathBuf, Parsed)> = Vec::new();
        let mut folder = path.parent();
        while let Some(dir) = folder {
            if let Some(text) = read(&dir.join(".editorconfig")) {
                let parsed = parse(&text);
                let is_root = parsed.root;
                layers.push((dir.to_path_buf(), parsed));
                if is_root {
                    break;
                }
            }
            folder = dir.parent();
        }
        let mut properties: HashMap<String, String> = HashMap::new();
        for (dir, parsed) in layers.iter().rev() {
            let Ok(relative) = path.strip_prefix(dir) else {
                continue;
            };
            let relative = relative.to_string_lossy().replace('\\', "/");
            for section in &parsed.sections {
                if section_matches(&section.glob, &relative) {
                    for (key, value) in &section.properties {
                        if value == "unset" {
                            properties.remove(key);
                        } else {
                            properties.insert(key.clone(), value.clone());
                        }
                    }
                }
            }
        }
        EditorConfig::from_properties(&properties)
    }

    fn from_properties(properties: &HashMap<String, String>) -> EditorConfig {
        let number = |key: &str| {
            properties
                .get(key)
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|value| *value > 0)
        };
        EditorConfig {
            indent_style: match properties.get("indent_style").map(String::as_str) {
                Some("space") => Some(IndentStyle::Space),
                Some("tab") => Some(IndentStyle::Tab),
                _ => None,
            },
            indent_size: match properties.get("indent_size").map(String::as_str) {
                Some("tab") => Some(IndentSize::Tab),
                _ => number("indent_size").map(IndentSize::Spaces),
            },
            tab_width: number("tab_width"),
            max_line_length: match properties.get("max_line_length").map(String::as_str) {
                Some("off") => Some(LineLength::Off),
                _ => number("max_line_length").map(LineLength::Limit),
            },
        }
    }
}

impl FormatOptions {
    /// The options with what a project's `.editorconfig` asks for laid over them.
    pub fn with_editorconfig(mut self, config: &EditorConfig) -> FormatOptions {
        let width = match (config.indent_size, config.tab_width) {
            (Some(IndentSize::Spaces(width)), _) => Some(width),
            (Some(IndentSize::Tab) | None, Some(width)) => Some(width),
            _ => None,
        };
        match config.indent_style {
            Some(IndentStyle::Tab) => self.indent = Indent::Tab,
            Some(IndentStyle::Space) => {
                let current = match self.indent {
                    Indent::Spaces(width) => width,
                    Indent::Tab => 4,
                };
                self.indent = Indent::Spaces(width.unwrap_or(current));
            }
            None => {
                if let (Indent::Spaces(_), Some(width)) = (self.indent, width) {
                    if matches!(config.indent_size, Some(IndentSize::Spaces(_))) {
                        self.indent = Indent::Spaces(width);
                    }
                }
            }
        }
        match config.max_line_length {
            Some(LineLength::Limit(limit)) => self.line_length = limit,
            Some(LineLength::Off) => self.line_length = 0,
            None => {}
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(files: &[(&str, &str)], path: &str) -> EditorConfig {
        let read = |wanted: &Path| {
            files
                .iter()
                .find(|(file, _)| Path::new(file) == wanted)
                .map(|(_, text)| (*text).to_string())
        };
        EditorConfig::for_path(Path::new(path), &read)
    }

    #[test]
    fn reads_the_keys_that_apply_to_a_php_file() {
        let found = config(
            &[(
                "/p/.editorconfig",
                "root = true\n\n[*]\nindent_style = space\nindent_size = 2\n\n[*.php]\nindent_size = 4\nmax_line_length = 100\n\n[*.md]\nindent_size = 8\n",
            )],
            "/p/src/A.php",
        );
        assert_eq!(found.indent_style, Some(IndentStyle::Space));
        assert_eq!(found.indent_size, Some(IndentSize::Spaces(4)));
        assert_eq!(found.max_line_length, Some(LineLength::Limit(100)));
    }

    #[test]
    fn a_nearer_file_wins_and_root_stops_the_walk() {
        let files = [
            ("/.editorconfig", "[*]\nindent_style = tab\n"),
            ("/p/.editorconfig", "root = true\n[*.php]\nindent_style = space\n"),
            (
                "/p/src/.editorconfig",
                "[*.php]\nindent_size = 3\nmax_line_length = off\n",
            ),
        ];
        let found = config(&files, "/p/src/A.php");
        assert_eq!(found.indent_style, Some(IndentStyle::Space));
        assert_eq!(found.indent_size, Some(IndentSize::Spaces(3)));
        assert_eq!(found.max_line_length, Some(LineLength::Off));
        assert_eq!(config(&files, "/p/other/A.php").indent_size, None);
    }

    #[test]
    fn globs_follow_the_specification() {
        assert!(section_matches("*.php", "src/Deep/A.php"));
        assert!(section_matches("*.{php,inc}", "A.inc"));
        assert!(section_matches("src/*.php", "src/A.php"));
        assert!(!section_matches("src/*.php", "src/Deep/A.php"));
        assert!(section_matches("src/**.php", "src/Deep/A.php"));
        assert!(section_matches("/lib/**", "lib/x/y.txt"));
        assert!(section_matches("file[1-3].php", "file2.php"));
        assert!(!section_matches("file[!1-3].php", "file2.php"));
        assert!(section_matches("f{1..3}.php", "f2.php"));
        assert!(!section_matches("*.php", "A.js"));
    }

    #[test]
    fn unset_removes_what_was_set() {
        let found = config(
            &[(
                "/p/.editorconfig",
                "[*]\nindent_style = tab\n[*.php]\nindent_style = unset\n",
            )],
            "/p/A.php",
        );
        assert_eq!(found.indent_style, None);
    }

    #[test]
    fn lays_over_the_options() {
        let tabs = FormatOptions::default().with_editorconfig(&EditorConfig {
            indent_style: Some(IndentStyle::Tab),
            max_line_length: Some(LineLength::Limit(90)),
            ..EditorConfig::default()
        });
        assert_eq!(tabs.indent, Indent::Tab);
        assert_eq!(tabs.line_length, 90);
        let two = FormatOptions::default().with_editorconfig(&EditorConfig {
            indent_style: Some(IndentStyle::Space),
            indent_size: Some(IndentSize::Spaces(2)),
            max_line_length: Some(LineLength::Off),
            ..EditorConfig::default()
        });
        assert_eq!(two.indent, Indent::Spaces(2));
        assert_eq!(two.line_length, 0);
        let untouched = FormatOptions::default().with_editorconfig(&EditorConfig::default());
        assert_eq!(untouched, FormatOptions::default());
    }
}
