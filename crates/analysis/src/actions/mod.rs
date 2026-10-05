//! Quick fixes and intentions: what an editor offers at a place of a file, as edits of its text.
//! A fix repairs what an inspection found. An intention rewrites code that is fine as it is. The
//! edits of an action are worked out only when asked for, which is what lets a client resolve the
//! expensive ones late.

mod docs;
pub mod edits;
mod fixes;
pub(crate) mod imports;
pub(crate) mod members;
pub(crate) mod type_text;

use php_syntax::TextRange;

use crate::completion::TextEdit;
use crate::inspections::{Cx, Finding, InspectionEnv};

pub use docs::doc_stub_at;
pub use edits::apply;

/// What an action is for, which is how a client decides where to show it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActionKind {
    QuickFix,
    Refactor,
    RefactorRewrite,
    Source,
    SourceOrganizeImports,
}

impl ActionKind {
    /// The kind as LSP names it.
    pub fn name(self) -> &'static str {
        match self {
            ActionKind::QuickFix => "quickfix",
            ActionKind::Refactor => "refactor",
            ActionKind::RefactorRewrite => "refactor.rewrite",
            ActionKind::Source => "source",
            ActionKind::SourceOrganizeImports => "source.organizeImports",
        }
    }
}

/// An action: a title, what it is for, and a way to get its edits.
pub struct Action<'a> {
    /// Stable for the same text and range, so a client can ask for the edits of one later.
    pub id: String,
    pub title: String,
    pub kind: ActionKind,
    pub preferred: bool,
    /// The index in the findings this repairs.
    pub finding: Option<usize>,
    /// Working out the edits costs enough that they are best left until the client asks.
    pub expensive: bool,
    edits: Box<dyn Fn() -> Vec<TextEdit> + 'a>,
}

impl Action<'_> {
    pub fn edits(&self) -> Vec<TextEdit> {
        (self.edits)()
    }
}

/// What the actions at a place read.
pub struct ActionInput<'a> {
    pub env: &'a InspectionEnv<'a>,
    /// The selection, empty for a cursor.
    pub range: TextRange,
    /// What the inspections found in the whole file.
    pub findings: &'a [Finding],
}

/// Runs something over the actions at a range, in the order to show them. The actions borrow what
/// was worked out for the file, so they live as long as the call does.
pub fn with_actions<R>(input: &ActionInput<'_>, run: impl FnOnce(Vec<Action<'_>>) -> R) -> R {
    let cx = Cx::new(input.env);
    let mut actions = Vec::new();
    fixes::quick_fixes(input, &cx, &mut actions);
    members::intentions(input, &cx, &mut actions);
    docs::intentions(input, &cx, &mut actions);
    organize_imports_action(&cx, &mut actions);
    run(actions)
}

fn organize_imports_action<'a>(cx: &'a Cx<'a>, out: &mut Vec<Action<'a>>) {
    let has_imports = cx
        .nodes
        .iter()
        .any(|node| node.kind() == php_syntax::SyntaxKind::USE_STATEMENT);
    if !has_imports {
        return;
    }
    out.push(Action {
        id: "source.organizeImports".to_string(),
        title: "Organize imports".to_string(),
        kind: ActionKind::SourceOrganizeImports,
        preferred: false,
        finding: None,
        expensive: true,
        edits: Box::new(move || imports::organize_imports(cx)),
    });
}

#[cfg(test)]
mod tests;
