//! Each refactor against code it applies to and code it must leave alone. A refactor that went
//! through is held to the same two promises: the result parses as before, and the inspections
//! find nothing in it that they did not find in the original.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use php_format::FormatOptions;
use php_index::composer::Composer;
use php_syntax::{TextRange, parse};

use super::{Change, RefactorEnv, with_refactors};
use crate::inspections::tests::index_with;
use crate::inspections::{Externals, InspectionEnv, InspectionSettings, inspect};
use crate::references::Sources;

mod extract_member;
mod extract_method;
mod extract_variable;
mod inline_method;
mod inline_variable;
mod move_class;
mod pull_push;
mod rewrite;
mod signature;

pub(super) const CURSOR: &str = "$0";
const START: char = '«';
const END: char = '»';

struct Files(HashMap<PathBuf, String>);

impl Sources for Files {
    fn candidates(&self, word: &str) -> Vec<PathBuf> {
        let mut found: Vec<PathBuf> = self
            .0
            .iter()
            .filter(|(_, text)| text.to_ascii_lowercase().contains(word))
            .map(|(path, _)| path.clone())
            .collect();
        found.sort();
        found
    }

    fn text(&self, path: &Path) -> Option<String> {
        self.0.get(path).cloned()
    }
}

/// Where the file under test lives and what Composer says about the project.
#[derive(Clone, Copy)]
pub(super) struct Setup {
    pub current: &'static str,
    pub composer: Option<&'static str>,
}

impl Setup {
    pub(super) const PLAIN: Setup = Setup {
        current: "current.php",
        composer: None,
    };

    fn composer(&self) -> Option<Composer> {
        let json: serde_json::Value = serde_json::from_str(self.composer?).ok()?;
        Some(Composer::from_json(Path::new("/project"), &json))
    }
}

/// What a refactor did, by file name relative to the project.
#[derive(Debug)]
pub(super) struct Done {
    pub text: String,
    pub files: BTreeMap<String, String>,
    pub moves: Vec<(String, String)>,
    pub change: Change,
}

pub(super) struct Outcome {
    pub offered: Vec<String>,
    pub result: Option<Result<Done, String>>,
}

/// The source without its markers, and the range the markers (a cursor or a selection) stand for.
pub(super) fn split_range(source: &str) -> (String, TextRange) {
    if let (Some(start), Some(end)) = (source.find(START), source.find(END)) {
        let clean = source.replacen(START, "", 1).replacen(END, "", 1);
        let end = end - START.len_utf8();
        return (clean, TextRange::new((start as u32).into(), (end as u32).into()));
    }
    let offset = source.find(CURSOR).unwrap_or(0);
    (source.replacen(CURSOR, "", 1), TextRange::empty((offset as u32).into()))
}

fn name_of(path: &Path) -> String {
    path.strip_prefix("/project")
        .unwrap_or(path)
        .to_string_lossy()
        .trim_start_matches('/')
        .to_string()
}

/// The index of a project: the files, the standard library of the tests, and the file under test.
fn index_for(files: &[(String, String)], current: &str, current_text: &str) -> php_index::Index {
    let refs: Vec<(&str, &str)> = files
        .iter()
        .map(|(path, text)| (path.as_str(), text.as_str()))
        .collect();
    let mut index = index_with(&refs, "");
    let current_path = PathBuf::from(format!("/project/{current}"));
    index.remove_file(Path::new("/project/current.php"));
    let symbols = php_index::extract::extract(
        &parse(current_text).syntax(),
        php_index::extract::ExtractOptions::default(),
    );
    index.set_file(current_path, php_index::Origin::Project, std::sync::Arc::new(symbols));
    for (path, text) in files.iter().filter(|(path, _)| path.starts_with("vendor/")) {
        let symbols = php_index::extract::extract(&parse(text).syntax(), php_index::extract::ExtractOptions::default());
        index.set_file(
            PathBuf::from(format!("/project/{path}")),
            php_index::Origin::Vendor,
            std::sync::Arc::new(symbols),
        );
    }
    index
}

fn findings_of(files: &[(String, String)], current: &str, current_text: &str) -> Vec<(String, String)> {
    let index = index_for(files, current, current_text);
    let settings = InspectionSettings::default();
    let externals = Externals::none();
    let mut out = Vec::new();
    let mut all: Vec<(String, &str)> = files.iter().map(|(path, text)| (path.clone(), text.as_str())).collect();
    all.push((current.to_string(), current_text));
    for (path, text) in all {
        let tree = parse(text);
        let root = tree.syntax();
        let env = InspectionEnv {
            index: &index,
            text,
            root: &root,
            settings: &settings,
            ready: true,
            externals: &externals,
        };
        for finding in inspect(&env) {
            out.push((
                path.clone(),
                format!("{} {}", finding.diagnostic.code, finding.diagnostic.message),
            ));
        }
        for error in tree.errors() {
            out.push((path.clone(), format!("syntax {}", error.message)));
        }
    }
    out.sort();
    out
}

/// Runs the refactor with this title at the marker of `source`, or returns what was offered.
pub(super) fn run_with(
    setup: Setup,
    files: &[(&str, &str)],
    source: &str,
    title: &str,
    format: &FormatOptions,
) -> Outcome {
    let (clean, range) = split_range(source);
    let owned: Vec<(String, String)> = files
        .iter()
        .map(|(path, text)| ((*path).to_string(), (*text).to_string()))
        .collect();
    let index = index_for(&owned, setup.current, &clean);
    let tree = parse(&clean);
    let root = tree.syntax();
    let settings = InspectionSettings::default();
    let externals = Externals::none();
    let env = InspectionEnv {
        index: &index,
        text: &clean,
        root: &root,
        settings: &settings,
        ready: true,
        externals: &externals,
    };
    let mut texts: HashMap<PathBuf, String> = files
        .iter()
        .map(|(path, text)| (PathBuf::from(format!("/project/{path}")), (*text).to_string()))
        .collect();
    let current_path = PathBuf::from(format!("/project/{}", setup.current));
    texts.insert(current_path.clone(), clean.clone());
    let sources = Files(texts.clone());
    let composer = setup.composer();
    let renv = RefactorEnv {
        env: &env,
        path: &current_path,
        sources: &sources,
        composer: composer.as_ref(),
        format: format.clone(),
    };
    with_refactors(&renv, range, |refactors| {
        let offered: Vec<String> = refactors.iter().map(|refactor| refactor.title.clone()).collect();
        let Some(found) = refactors.iter().find(|refactor| refactor.title == title) else {
            return Outcome { offered, result: None };
        };
        let result = found.run().map(|change| outcome_of(&setup, &texts, &clean, change));
        Outcome {
            offered,
            result: Some(result),
        }
    })
}

/// The change as the texts of the files it reaches.
pub(super) fn outcome_of(setup: &Setup, texts: &HashMap<PathBuf, String>, clean: &str, change: Change) -> Done {
    let mut files_after: BTreeMap<String, String> = BTreeMap::new();
    for file in &change.files {
        let before = texts.get(&file.path).cloned().unwrap_or_default();
        let edits: Vec<crate::completion::TextEdit> = file
            .edits
            .iter()
            .map(|edit| crate::completion::TextEdit {
                start: edit.start,
                end: edit.end,
                new_text: edit.text.clone(),
            })
            .collect();
        files_after.insert(name_of(&file.path), crate::actions::apply(&before, &edits));
    }
    let text = files_after
        .get(setup.current)
        .cloned()
        .unwrap_or_else(|| clean.to_string());
    let moves = change
        .moves
        .iter()
        .map(|moved| (name_of(&moved.from), name_of(&moved.to)))
        .collect();
    Done {
        text,
        files: files_after,
        moves,
        change,
    }
}

/// The current file after a refactor, which must parse and trouble the inspections no more than before.
pub(super) fn applied(files: &[(&str, &str)], source: &str, title: &str) -> String {
    done(files, source, title).text
}

pub(super) fn done(files: &[(&str, &str)], source: &str, title: &str) -> Done {
    done_with(Setup::PLAIN, files, source, title)
}

pub(super) fn done_with(setup: Setup, files: &[(&str, &str)], source: &str, title: &str) -> Done {
    let outcome = run_with(setup, files, source, title, &FormatOptions::default());
    let Some(result) = outcome.result else {
        panic!("no refactor '{title}' among {:?}", outcome.offered);
    };
    let done = match result {
        Ok(done) => done,
        Err(reason) => panic!("'{title}' was refused: {reason}"),
    };
    check_result(&setup, files, source, &done);
    done
}

/// The reason a refactor that is offered cannot be done.
pub(super) fn refused(files: &[(&str, &str)], source: &str, title: &str) -> String {
    refused_with(Setup::PLAIN, files, source, title)
}

pub(super) fn refused_with(setup: Setup, files: &[(&str, &str)], source: &str, title: &str) -> String {
    let outcome = run_with(setup, files, source, title, &FormatOptions::default());
    match outcome.result {
        Some(Err(reason)) => reason,
        Some(Ok(done)) => panic!("'{title}' was not refused, it gave {:?}", done.text),
        None => panic!("no refactor '{title}' among {:?}", outcome.offered),
    }
}

pub(super) fn offered(files: &[(&str, &str)], source: &str) -> Vec<String> {
    offered_with(Setup::PLAIN, files, source)
}

pub(super) fn offered_with(setup: Setup, files: &[(&str, &str)], source: &str) -> Vec<String> {
    run_with(setup, files, source, "\u{0}", &FormatOptions::default()).offered
}

fn check_result(setup: &Setup, files: &[(&str, &str)], source: &str, done: &Done) {
    let (clean, _) = split_range(source);
    let before_files: Vec<(String, String)> = files
        .iter()
        .map(|(p, t)| ((*p).to_string(), (*t).to_string()))
        .collect();
    let before = findings_of(&before_files, setup.current, &clean);
    let moved_to = |path: &str| {
        done.moves
            .iter()
            .find(|(from, _)| from == path)
            .map(|(_, to)| to.clone())
    };
    let mut after_files: Vec<(String, String)> = Vec::new();
    for (path, text) in &before_files {
        let text = done.files.get(path).cloned().unwrap_or_else(|| text.clone());
        after_files.push((moved_to(path).unwrap_or_else(|| path.clone()), text));
    }
    let after_current_name = moved_to(setup.current).unwrap_or_else(|| setup.current.to_string());
    let before_renamed: Vec<(String, String)> = before
        .iter()
        .map(|(path, finding)| (moved_to(path).unwrap_or_else(|| path.clone()), finding.clone()))
        .collect();
    let after = findings_of(&after_files, &after_current_name, &done.text);
    let mut remaining = before_renamed;
    let mut new: Vec<&(String, String)> = Vec::new();
    for item in &after {
        match remaining.iter().position(|old| old == item) {
            Some(at) => {
                remaining.remove(at);
            }
            None => new.push(item),
        }
    }
    assert!(
        new.is_empty(),
        "the refactor brought in new findings: {new:?}\nafter:\n{}\nfiles: {:#?}",
        done.text,
        done.files
    );
}

/// What moving files does to a project: the edits that follow them, checked like any refactor.
pub(super) fn files_moved(
    setup: Setup,
    files: &[(&str, &str)],
    pairs: &[(&str, &str)],
) -> Result<Option<Done>, String> {
    let (_, first_text) = files
        .iter()
        .find(|(path, _)| *path == setup.current)
        .copied()
        .expect("the current file is among the files");
    let owned: Vec<(String, String)> = files
        .iter()
        .filter(|(path, _)| *path != setup.current)
        .map(|(path, text)| ((*path).to_string(), (*text).to_string()))
        .collect();
    let index = index_for(&owned, setup.current, first_text);
    let tree = parse(first_text);
    let root = tree.syntax();
    let settings = InspectionSettings::default();
    let externals = Externals::none();
    let env = InspectionEnv {
        index: &index,
        text: first_text,
        root: &root,
        settings: &settings,
        ready: true,
        externals: &externals,
    };
    let texts: HashMap<PathBuf, String> = files
        .iter()
        .map(|(path, text)| (PathBuf::from(format!("/project/{path}")), (*text).to_string()))
        .collect();
    let sources = Files(texts.clone());
    let current_path = PathBuf::from(format!("/project/{}", setup.current));
    let composer = setup.composer();
    let renv = RefactorEnv {
        env: &env,
        path: &current_path,
        sources: &sources,
        composer: composer.as_ref(),
        format: FormatOptions::default(),
    };
    let pairs: Vec<(PathBuf, PathBuf)> = pairs
        .iter()
        .map(|(from, to)| {
            (
                PathBuf::from(format!("/project/{from}")),
                PathBuf::from(format!("/project/{to}")),
            )
        })
        .collect();
    let Some(change) = super::move_files(&renv, &pairs, false)? else {
        return Ok(None);
    };
    let done = outcome_of(&setup, &texts, first_text, change);
    // The files sit at their new places for the check.
    let mut after: Vec<(String, String)> = Vec::new();
    for (path, text) in files {
        let new_name = pairs
            .iter()
            .find(|(from, _)| name_of(from) == *path)
            .map_or_else(|| (*path).to_string(), |(_, to)| name_of(to));
        after.push((
            new_name,
            done.files.get(*path).cloned().unwrap_or_else(|| (*text).to_string()),
        ));
    }
    let before: Vec<(String, String)> = files
        .iter()
        .map(|(path, text)| ((*path).to_string(), (*text).to_string()))
        .collect();
    let rename = |path: &str| {
        pairs
            .iter()
            .find(|(from, _)| name_of(from) == path)
            .map_or_else(|| path.to_string(), |(_, to)| name_of(to))
    };
    let (first_name, rest_before): (Vec<_>, Vec<_>) =
        before.iter().cloned().partition(|(path, _)| path == setup.current);
    let before_findings: Vec<(String, String)> = findings_of(&rest_before, setup.current, &first_name[0].1)
        .into_iter()
        .map(|(path, finding)| (rename(&path), finding))
        .collect();
    let (first_after, rest_after): (Vec<_>, Vec<_>) = after
        .iter()
        .cloned()
        .partition(|(path, _)| *path == rename(setup.current));
    let after_findings = findings_of(&rest_after, &rename(setup.current), &first_after[0].1);
    let mut remaining = before_findings;
    let mut new: Vec<&(String, String)> = Vec::new();
    for item in &after_findings {
        match remaining.iter().position(|old| old == item) {
            Some(at) => {
                remaining.remove(at);
            }
            None => new.push(item),
        }
    }
    assert!(
        new.is_empty(),
        "moving the files brought in new findings: {new:?}\nfiles: {:#?}",
        done.files
    );
    Ok(Some(done))
}
