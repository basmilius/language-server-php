//! Refactors: changes of a program that keep what it does. Each is offered where it applies and
//! works out its edits when asked, over the lossless tree, so the comments and the layout of
//! everything it leaves alone survive. The edits can reach other files and move files, and the
//! lines they touch are laid out by the formatter.

mod diff;
mod draft;
mod exprs;
mod extract_interface;
mod extract_member;
mod extract_method;
mod extract_variable;
mod inline_method;
mod inline_variable;
mod move_class;
mod names;
mod pull_push;
mod rewrite;
mod scope;
mod signature;
#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use php_format::FormatOptions;
use php_index::composer::Composer;
use php_syntax::TextRange;

use crate::inspections::{Cx, InspectionEnv};
use crate::references::Sources;

pub use move_class::{ClassMove, move_files};

/// What a refactor is for, which is how a client decides where to show it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefactorKind {
    Extract,
    Inline,
    Rewrite,
    Move,
}

impl RefactorKind {
    /// The kind as LSP names it.
    pub fn name(self) -> &'static str {
        match self {
            RefactorKind::Extract => "refactor.extract",
            RefactorKind::Inline => "refactor.inline",
            RefactorKind::Rewrite => "refactor.rewrite",
            RefactorKind::Move => "refactor.move",
        }
    }
}

/// One replacement in the text of a file as it was before the refactor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub start: u32,
    pub end: u32,
    pub text: String,
    /// Where `text` starts in the file once every edit is applied.
    pub new_start: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileChange {
    pub path: PathBuf,
    /// In the order of the file, none overlapping.
    pub edits: Vec<Edit>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileMove {
    pub from: PathBuf,
    pub to: PathBuf,
}

/// A file a refactor makes, with its text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileCreate {
    pub path: PathBuf,
    pub text: String,
}

/// The name a person is expected to change next, as a range of the file once the edits are applied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Focus {
    pub path: PathBuf,
    pub start: u32,
    pub end: u32,
}

/// What a refactor changes: the text of files, files it makes, and files that move after the edits
/// are applied.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Change {
    pub files: Vec<FileChange>,
    pub moves: Vec<FileMove>,
    /// Made before the edits of the other files are applied.
    pub creates: Vec<FileCreate>,
    pub focus: Option<Focus>,
}

/// Everything a refactor reads about the file it is asked in and the project around it.
pub struct RefactorEnv<'a> {
    pub env: &'a InspectionEnv<'a>,
    pub path: &'a Path,
    pub sources: &'a dyn Sources,
    pub composer: Option<&'a Composer>,
    pub format: FormatOptions,
}

/// A refactor: a title, what it is for, and a way to get its edits.
pub struct Refactor<'a> {
    /// Stable for the same text and range, so a client can ask for the edits of one later.
    pub id: String,
    pub title: String,
    pub kind: RefactorKind,
    /// Working out the edits costs enough that they are best left until the client asks.
    pub expensive: bool,
    run: Box<dyn Fn() -> Result<Change, String> + 'a>,
}

impl<'a> Refactor<'a> {
    pub(crate) fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        kind: RefactorKind,
        expensive: bool,
        run: impl Fn() -> Result<Change, String> + 'a,
    ) -> Refactor<'a> {
        Refactor {
            id: id.into(),
            title: title.into(),
            kind,
            expensive,
            run: Box::new(run),
        }
    }

    /// The edits, or the reason there are none.
    pub fn run(&self) -> Result<Change, String> {
        (self.run)()
    }
}

/// What the providers of refactors read: the file, the selection and the project.
pub(crate) struct Rcx<'a> {
    pub cx: Cx<'a>,
    pub renv: &'a RefactorEnv<'a>,
    pub range: TextRange,
}

/// Runs something over the refactors at a range, in the order to show them. They borrow what was
/// worked out for the file, so they live as long as the call does.
pub fn with_refactors<R>(renv: &RefactorEnv<'_>, range: TextRange, run: impl FnOnce(Vec<Refactor<'_>>) -> R) -> R {
    let cx = Cx::new(renv.env);
    let rcx = Rcx { cx, renv, range };
    let mut out = Vec::new();
    extract_variable::offer(&rcx, &mut out);
    inline_variable::offer(&rcx, &mut out);
    extract_method::offer(&rcx, &mut out);
    extract_member::offer(&rcx, &mut out);
    inline_method::offer(&rcx, &mut out);
    signature::offer(&rcx, &mut out);
    extract_interface::offer(&rcx, &mut out);
    move_class::offer(&rcx, &mut out);
    pull_push::offer(&rcx, &mut out);
    rewrite::offer(&rcx, &mut out);
    run(out)
}
