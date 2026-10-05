//! One file analyzed many times: the analyzers for the places in it, kept so the work of
//! following a function body is shared by everything asked in it.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use php_index::Index;
use php_syntax::SyntaxKind::*;
use php_syntax::SyntaxNode;

use crate::ast::{is_class_like, start};
use crate::infer::{Analyzer, SharedCache};

pub struct FileContext<'a> {
    pub index: &'a Index,
    pub root: SyntaxNode,
    analyzers: RefCell<HashMap<(u32, u32, u32), Rc<Analyzer<'a>>>>,
    shared: Rc<SharedCache>,
}

impl<'a> FileContext<'a> {
    pub fn new(index: &'a Index, root: &SyntaxNode) -> FileContext<'a> {
        FileContext {
            index,
            root: root.clone(),
            analyzers: RefCell::new(HashMap::new()),
            shared: Rc::default(),
        }
    }

    /// The analyzer for the place of a node. What it knows (namespace, imports, the class around)
    /// only changes from one top-level statement or class to the next.
    pub fn analyzer(&self, node: &SyntaxNode) -> Rc<Analyzer<'a>> {
        let key = context_key(node);
        if let Some(found) = self.analyzers.borrow().get(&key) {
            return found.clone();
        }
        let analyzer = Rc::new(Analyzer::with_shared(
            self.index,
            &self.root,
            start(node) + 1,
            Rc::clone(&self.shared),
        ));
        self.analyzers.borrow_mut().insert(key, analyzer.clone());
        analyzer
    }
}

/// The start of the top-level statement a node is in, of the class around it and of the Pest
/// closure around it, which is where `$this` is a test case.
fn context_key(node: &SyntaxNode) -> (u32, u32, u32) {
    let pest = crate::pest::calls::enclosing_test_closure(node).map_or(0, |closure| start(&closure) + 1);
    let mut top = 0;
    let mut class = 0;
    for ancestor in node.ancestors() {
        if class == 0 && is_class_like(ancestor.kind()) {
            class = start(&ancestor) + 1;
        }
        let Some(parent) = ancestor.parent() else {
            continue;
        };
        let at_top = match parent.kind() {
            SOURCE_FILE => true,
            STATEMENT_LIST | BLOCK => parent
                .parent()
                .is_some_and(|grand| grand.kind() == NAMESPACE_DECLARATION),
            _ => false,
        };
        if at_top && top == 0 {
            top = start(&ancestor) + 1;
        }
    }
    (top, class, pest)
}
