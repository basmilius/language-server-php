//! The file an analysis is about. A few answers depend on where a file is, such as the test case a
//! folder of Pest tests is bound to, and the tree alone does not say. The front end names the file
//! for the length of a request, and analyzers made meanwhile read it.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

thread_local! {
    static CURRENT: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Puts the previous file back when it goes out of scope.
#[must_use = "the file is only named while the guard lives"]
pub struct DocumentGuard {
    previous: Option<PathBuf>,
}

impl Drop for DocumentGuard {
    fn drop(&mut self) {
        CURRENT.with(|current| *current.borrow_mut() = self.previous.take());
    }
}

/// Names the file the analyses made from now on are about.
pub fn enter(path: Option<&Path>) -> DocumentGuard {
    let previous = CURRENT.with(|current| current.replace(path.map(Path::to_path_buf)));
    DocumentGuard { previous }
}

/// The file named by the innermost guard.
pub fn current() -> Option<PathBuf> {
    CURRENT.with(|current| current.borrow().clone())
}
