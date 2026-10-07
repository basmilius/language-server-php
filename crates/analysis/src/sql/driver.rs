//! The database a project talks to, from what its configuration says statically: the dialect
//! SQL in its strings is read in when the settings name none.

use std::path::{Path, PathBuf};

use php_index::framework::Frameworks;
use sql_embed::Dialect;

/// The dialect a driver or connection name stands for.
fn dialect_of_driver(name: &str) -> Option<Dialect> {
    let name = name.trim().trim_matches(['"', '\'']).to_ascii_lowercase();
    let name = name
        .strip_prefix("pdo_")
        .or_else(|| name.strip_prefix("pdo-"))
        .unwrap_or(&name);
    match name {
        "mysql" | "mysqli" | "mysql2" => Some(Dialect::Mysql),
        "mariadb" => Some(Dialect::Mariadb),
        "pgsql" | "postgres" | "postgresql" => Some(Dialect::Postgres),
        "sqlite" | "sqlite3" => Some(Dialect::Sqlite),
        _ => None,
    }
}

/// The value of a key in the `.env` files of a folder, `.env.local` before `.env`.
fn env_value(root: &Path, key: &str) -> Option<String> {
    for name in [".env.local", ".env"] {
        let Ok(text) = std::fs::read_to_string(root.join(name)) else {
            continue;
        };
        for line in text.lines() {
            let line = line.trim_start();
            let line = line.strip_prefix("export ").unwrap_or(line);
            let Some((name, value)) = line.split_once('=') else {
                continue;
            };
            if name.trim() == key {
                let value = value.trim();
                let value = value.split(" #").next().unwrap_or(value).trim();
                return Some(value.trim_matches(['"', '\'']).to_string());
            }
        }
    }
    None
}

/// The dialect of a connection URL. Only its scheme and the `serverVersion` it may name are read:
/// the rest holds a password.
fn dialect_of_url(url: &str) -> Option<Dialect> {
    let (scheme, rest) = url.split_once("://")?;
    let dialect = dialect_of_driver(scheme)?;
    let version = rest
        .split_once('?')
        .map(|(_, query)| query)
        .and_then(|query| query.split('&').find_map(|pair| pair.strip_prefix("serverVersion=")))
        .unwrap_or_default();
    if dialect == Dialect::Mysql && version.to_ascii_lowercase().contains("mariadb") {
        return Some(Dialect::Mariadb);
    }
    Some(dialect)
}

/// The default connection of a Laravel application: `DB_CONNECTION`, else the fallback
/// `config/database.php` gives it.
fn laravel(root: &Path) -> Option<Dialect> {
    if let Some(connection) = env_value(root, "DB_CONNECTION") {
        return dialect_of_driver(&connection);
    }
    let config = std::fs::read_to_string(root.join("config/database.php")).ok()?;
    let at = config.find("'default'")?;
    let rest = &config[at..];
    let line = rest.lines().next()?;
    let value = line
        .rsplit(['\'', '"'])
        .nth(1)
        .filter(|value| !value.is_empty() && !value.contains("DB_CONNECTION"))?;
    dialect_of_driver(value)
}

/// The connection classes of Raxos a project registers.
fn raxos(files: &[PathBuf]) -> Option<Dialect> {
    let mut found: Option<Dialect> = None;
    for path in files {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        if !text.contains("Raxos\\Database\\Connection") {
            continue;
        }
        for (class, dialect) in [
            ("MariaDb", Dialect::Mariadb),
            ("MySql", Dialect::Mysql),
            ("SQLite", Dialect::Sqlite),
        ] {
            let registers =
                text.contains(&format!("{class}::createFromOptions(")) || text.contains(&format!("new {class}("));
            if !registers {
                continue;
            }
            match found {
                Some(known) if known != dialect => return None,
                _ => found = Some(dialect),
            }
        }
    }
    found
}

/// The dialect of the database a project is configured for: Laravel's `DB_CONNECTION` or
/// `config/database.php`, the scheme of a `DATABASE_URL`, or the connection classes of Raxos the
/// project registers (read from `files`, its own PHP files). Nothing when it names none or names
/// several.
pub fn project_dialect(root: &Path, frameworks: Frameworks, files: &[PathBuf]) -> Option<Dialect> {
    if frameworks.laravel || frameworks.eloquent {
        if let Some(dialect) = laravel(root) {
            return Some(dialect);
        }
    }
    if let Some(url) = env_value(root, "DATABASE_URL") {
        if let Some(dialect) = dialect_of_url(&url) {
            return Some(dialect);
        }
    }
    if frameworks.raxos {
        return raxos(files);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_dialect_of_a_driver_or_a_url() {
        assert_eq!(dialect_of_driver("pgsql"), Some(Dialect::Postgres));
        assert_eq!(dialect_of_driver("pdo_mysql"), Some(Dialect::Mysql));
        assert_eq!(dialect_of_driver("sqlsrv"), None);
        assert_eq!(
            dialect_of_url("postgresql://app:secret@127.0.0.1:5432/app?serverVersion=16&charset=utf8"),
            Some(Dialect::Postgres)
        );
        assert_eq!(
            dialect_of_url("mysql://app:secret@127.0.0.1:3306/app?serverVersion=10.11.2-MariaDB"),
            Some(Dialect::Mariadb)
        );
        assert_eq!(dialect_of_url("not a url"), None);
    }

    #[test]
    fn reads_what_a_project_is_configured_for() {
        let dir = std::env::temp_dir().join(format!("php-sql-driver-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("config")).expect("a folder");
        std::fs::write(
            dir.join("config/database.php"),
            "<?php return [\n    'default' => env('DB_CONNECTION', 'sqlite'),\n];\n",
        )
        .expect("written");
        let laravel = Frameworks {
            laravel: true,
            ..Frameworks::default()
        };
        assert_eq!(project_dialect(&dir, laravel, &[]), Some(Dialect::Sqlite));
        std::fs::write(dir.join(".env"), "APP_NAME=x\nDB_CONNECTION=pgsql\n").expect("written");
        assert_eq!(project_dialect(&dir, laravel, &[]), Some(Dialect::Postgres));
        std::fs::write(
            dir.join(".env"),
            "DATABASE_URL=\"mysql://u:p@h/db?serverVersion=8.0\"\n",
        )
        .expect("written");
        assert_eq!(project_dialect(&dir, Frameworks::default(), &[]), Some(Dialect::Mysql));
        let source = dir.join("Database.php");
        std::fs::write(
            &source,
            "<?php use Raxos\\Database\\Connection\\MariaDb;\nDb::register(MariaDb::createFromOptions([]));\n",
        )
        .expect("written");
        std::fs::remove_file(dir.join(".env")).expect("removed");
        let raxos = Frameworks {
            raxos: true,
            ..Frameworks::default()
        };
        assert_eq!(project_dialect(&dir, raxos, &[source]), Some(Dialect::Mariadb));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
