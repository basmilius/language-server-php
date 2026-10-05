//! The names a project's `.env` files define. Values are never read: they hold secrets.

use std::path::{Path, PathBuf};

use super::Section;
use crate::index::Index;
use crate::model::Span;

#[derive(Clone, Debug, PartialEq)]
pub struct EnvName {
    pub name: String,
    pub path: PathBuf,
    /// The name, not the value.
    pub span: Span,
}

#[derive(Default)]
pub struct EnvNames {
    pub names: Vec<EnvName>,
}

impl EnvNames {
    pub fn find(&self, name: &str) -> Option<&EnvName> {
        self.names.iter().find(|entry| entry.name == name)
    }

    /// Every definition of a name, `.env` and `.env.example` both.
    pub fn find_all(&self, name: &str) -> Vec<&EnvName> {
        self.names.iter().filter(|entry| entry.name == name).collect()
    }
}

fn is_env_file(name: &str) -> bool {
    name == ".env" || name.starts_with(".env.")
}

impl Section for EnvNames {
    fn build(index: &Index) -> Self {
        let mut env = EnvNames::default();
        let mut files: Vec<PathBuf> = index
            .files_in(index.framework_root())
            .into_iter()
            .filter(|path| {
                path.file_name()
                    .is_some_and(|name| is_env_file(&name.to_string_lossy()))
            })
            .collect();
        // `.env` first, so a name is shown where it is actually set.
        files.sort_by_key(|path| path.file_name().is_some_and(|name| name != ".env"));
        for path in files {
            let Some(text) = index.read_text(&path) else {
                continue;
            };
            let mut offset = 0;
            for line in text.split_inclusive('\n') {
                let trimmed = line.trim_start();
                let indent = line.len() - trimmed.len();
                let body = trimmed.strip_prefix("export ").map_or(trimmed, str::trim_start);
                let skipped = line.len() - body.len();
                if !body.starts_with('#') {
                    if let Some((name, _)) = body.split_once('=') {
                        let name = name.trim_end();
                        if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.') {
                            let start = (offset + skipped.max(indent)) as u32;
                            env.names.push(EnvName {
                                name: name.to_string(),
                                path: path.clone(),
                                span: Span {
                                    start,
                                    end: start + name.len() as u32,
                                },
                            });
                        }
                    }
                }
                offset += line.len();
            }
        }
        env
    }

    fn depends_on(root: &Path, path: &Path) -> bool {
        path.parent() == Some(root)
            && path
                .file_name()
                .is_some_and(|name| is_env_file(&name.to_string_lossy()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::framework::testing::project;

    #[test]
    fn reads_the_names_and_never_the_values() {
        let index = project(&[
            (
                ".env",
                "APP_NAME=Laravel\n# a comment\nexport DB_HOST=localhost\n\nAPP_KEY=base64:secret\n",
            ),
            (".env.example", "APP_NAME=\nMAIL_FROM=x\n"),
        ]);
        let env = index.section::<EnvNames>();
        let names: Vec<&str> = env.names.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(names, ["APP_NAME", "DB_HOST", "APP_KEY", "APP_NAME", "MAIL_FROM"]);
        let host = env.find("DB_HOST").expect("a name");
        let text = "APP_NAME=Laravel\n# a comment\nexport DB_HOST=localhost\n";
        assert_eq!(&text[host.span.start as usize..host.span.end as usize], "DB_HOST");
        assert_eq!(env.find_all("APP_NAME").len(), 2);
    }
}
