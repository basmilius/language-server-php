//! The table of the functions and methods that take SQL, read from `sinks.php`.

use std::collections::HashMap;
use std::sync::OnceLock;

use php_index::RawTag;
use php_index::extract::{ExtractOptions, extract};
use php_syntax::parse;
use sql_embed::{Dialect, FragmentKind};

/// Which arguments a tag points at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Place {
    At(usize),
    /// A variadic parameter: this place and every one after it.
    From(usize),
    Every,
}

impl Place {
    fn holds(self, position: usize) -> bool {
        match self {
            Place::At(at) => at == position,
            Place::From(from) => position >= from,
            Place::Every => true,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Format {
    Printf,
    Wpdb,
}

#[derive(Clone, Debug)]
pub(crate) struct Sink {
    /// The class that declares the method, `None` for a function.
    pub class: Option<String>,
    /// The method, or the qualified name of the function.
    pub name: String,
    params: Vec<String>,
    /// The kind of each place that takes SQL; `None` for a part whose kind is whatever reads best.
    args: Vec<(Option<FragmentKind>, Place)>,
    pub refine: bool,
    pub context: Option<FragmentKind>,
    /// The places of the table and of its alias, for a call that adds a table to its query.
    pub table: Option<(Place, Option<Place>)>,
    optional_first: bool,
    pub format: Option<Format>,
    pub dialect: Option<Dialect>,
}

impl Sink {
    /// The place among the parameters of an argument: its position, moved when the first
    /// parameter is optional and the call leaves it out, or the parameter it names.
    pub fn place_of(&self, position: usize, name: Option<&str>, count: usize) -> Option<usize> {
        match name {
            Some(name) => self.params.iter().position(|param| param == name),
            None if self.optional_first && count < self.params.len() => Some(position + 1),
            None => Some(position),
        }
    }

    /// The kind of SQL the argument at a place is.
    /// The kind of SQL the argument at a place is: `Some(None)` for a part of a query of no kind
    /// the call says.
    pub fn kind_at(&self, place: usize) -> Option<Option<FragmentKind>> {
        self.args.iter().find(|(_, at)| at.holds(place)).map(|(kind, _)| *kind)
    }

    pub fn takes_sql(&self) -> bool {
        !self.args.is_empty()
    }

    pub fn table_at(&self, place: usize) -> bool {
        self.table.is_some_and(|(table, _)| table.holds(place))
    }

    pub fn alias_at(&self, place: usize) -> bool {
        self.table
            .and_then(|(_, alias)| alias)
            .is_some_and(|alias| alias.holds(place))
    }
}

pub(crate) fn kind_named(word: &str) -> Option<FragmentKind> {
    Some(match word {
        "statements" => FragmentKind::Statements,
        "condition" => FragmentKind::Condition,
        "having" => FragmentKind::Having,
        "select" => FragmentKind::SelectList,
        "expression" => FragmentKind::Expression,
        "order" => FragmentKind::OrderBy,
        "group" => FragmentKind::GroupBy,
        "table" => FragmentKind::TableReference,
        "set" => FragmentKind::SetList,
        _ => return None,
    })
}

fn place_named(word: &str, params: &[String], variadic: Option<usize>) -> Option<Place> {
    if word == "*" {
        return Some(Place::Every);
    }
    let name = word.strip_prefix('$')?;
    let position = params.iter().position(|param| param == name)?;
    Some(if variadic == Some(position) {
        Place::From(position)
    } else {
        Place::At(position)
    })
}

fn read(class: Option<&str>, name: &str, params: &[(String, bool)], tags: &[RawTag]) -> Option<Sink> {
    let names: Vec<String> = params.iter().map(|(name, _)| name.clone()).collect();
    let variadic = params.iter().position(|(_, variadic)| *variadic);
    let mut sink = Sink {
        class: class.map(str::to_string),
        name: name.to_string(),
        params: names.clone(),
        args: Vec::new(),
        refine: false,
        context: None,
        table: None,
        optional_first: false,
        format: None,
        dialect: None,
    };
    let mut any = false;
    for tag in tags {
        let mut words = tag.text.split_whitespace();
        match tag.name.as_str() {
            "sql" => {
                let kind = match words.next() {
                    Some("part") => None,
                    Some(word) => match kind_named(word) {
                        Some(kind) => Some(kind),
                        None => continue,
                    },
                    None => continue,
                };
                let Some(place) = words.next() else {
                    continue;
                };
                let Some(place) = place_named(place, &names, variadic) else {
                    continue;
                };
                sink.args.push((kind, place));
                if let Some(dialect) = words.next().and_then(Dialect::parse) {
                    sink.dialect = Some(dialect);
                }
            }
            "sql-refine" => sink.refine = true,
            "sql-context" => sink.context = words.next().and_then(kind_named),
            "sql-table" => {
                let Some(table) = words.next().and_then(|word| place_named(word, &names, variadic)) else {
                    continue;
                };
                let alias = words.next().and_then(|word| place_named(word, &names, variadic));
                sink.table = Some((table, alias));
            }
            "sql-optional-first" => sink.optional_first = true,
            "sql-format" => {
                sink.format = match words.next() {
                    Some("wpdb") => Some(Format::Wpdb),
                    Some("printf") => Some(Format::Printf),
                    _ => None,
                }
            }
            _ => continue,
        }
        any = true;
    }
    any.then_some(sink)
}

/// Every entry, by the lowercase name of its method or the short name of its function.
fn table() -> &'static HashMap<String, Vec<Sink>> {
    static TABLE: OnceLock<HashMap<String, Vec<Sink>>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let symbols = extract(&parse(include_str!("sinks.php")).syntax(), ExtractOptions::default());
        let mut out: HashMap<String, Vec<Sink>> = HashMap::new();
        let params_of = |callable: &php_index::Callable| -> Vec<(String, bool)> {
            callable
                .params
                .iter()
                .map(|param| (param.name.clone(), param.variadic))
                .collect()
        };
        for function in &symbols.functions {
            let tags = function.doc.as_ref().map(|doc| doc.tags.as_slice()).unwrap_or_default();
            if let Some(sink) = read(None, &function.name, &params_of(&function.callable), tags) {
                out.entry(crate::short(&function.name).to_ascii_lowercase())
                    .or_default()
                    .push(sink);
            }
        }
        for class in &symbols.classes {
            for method in &class.methods {
                let tags = method.doc.as_ref().map(|doc| doc.tags.as_slice()).unwrap_or_default();
                if let Some(sink) = read(Some(&class.name), &method.name, &params_of(&method.callable), tags) {
                    out.entry(method.name.to_ascii_lowercase()).or_default().push(sink);
                }
            }
        }
        out
    })
}

/// The entries of a method or function of this name, as a cheap test before a call is resolved.
pub(crate) fn named(name: &str) -> &'static [Sink] {
    table()
        .get(&name.to_ascii_lowercase())
        .map(Vec::as_slice)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_every_entry_of_the_table() {
        let query = named("query");
        let pdo = query
            .iter()
            .find(|sink| sink.class.as_deref() == Some("PDO"))
            .expect("PDO::query");
        assert_eq!(pdo.kind_at(0), Some(Some(FragmentKind::Statements)));
        let pg = named("pg_query").first().expect("pg_query");
        assert_eq!(pg.class, None);
        assert_eq!(pg.dialect, Some(Dialect::Postgres));
        assert_eq!(pg.place_of(0, None, 1), Some(1), "the connection is left out");
        assert_eq!(pg.place_of(1, None, 2), Some(1));
        assert_eq!(pg.kind_at(1), Some(Some(FragmentKind::Statements)));
        let literal = named("literal").first().expect("literal");
        assert_eq!(literal.name, "Raxos\\Database\\Query\\literal");
        assert!(literal.refine);
        let join = named("join")
            .iter()
            .find(|sink| sink.class.as_deref() == Some("Doctrine\\DBAL\\Query\\QueryBuilder"))
            .expect("a join");
        assert_eq!(join.kind_at(1), Some(Some(FragmentKind::TableReference)));
        assert_eq!(join.kind_at(3), Some(Some(FragmentKind::Condition)));
        assert!(join.table_at(1) && join.alias_at(2));
        let select = named("select")
            .iter()
            .find(|sink| sink.class.as_deref() == Some("Doctrine\\DBAL\\Query\\QueryBuilder"))
            .expect("a select");
        assert_eq!(
            select.kind_at(4),
            Some(Some(FragmentKind::Expression)),
            "every argument"
        );
        let raw = named("raw")
            .iter()
            .find(|sink| sink.class.as_deref() == Some("Raxos\\Contract\\Database\\Query\\QueryInterface"))
            .expect("raw");
        assert_eq!(raw.kind_at(0), Some(None), "a part of no kind");
        let prepare = named("prepare")
            .iter()
            .find(|sink| sink.class.as_deref() == Some("wpdb"))
            .expect("wpdb::prepare");
        assert_eq!(prepare.format, Some(Format::Wpdb));
        assert!(named("nothing").is_empty());
        let count: usize = table().values().map(Vec::len).sum();
        assert!(count > 150, "{count}");
    }
}
