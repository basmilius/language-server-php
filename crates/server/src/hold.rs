//! Lets a test keep a background job of the server from reporting back until it says so, which
//! makes each order of "the job finished" and "the client did something" a test of its own.
//! Nothing is registered outside tests, and a job then passes without waiting.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

/// A background job of the server a test can hold.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Job {
    /// The read of the `.sql` files of the workspace folders.
    SqlFiles,

    /// The search for the database a project is configured for.
    SqlDialect,
}

#[derive(Default)]
struct Gate {
    open: Mutex<bool>,
    changed: Condvar,
}

static GATES: Mutex<Vec<(PathBuf, Job, Arc<Gate>)>> = Mutex::new(Vec::new());

/// A held job. It runs on once released or dropped.
pub struct Hold {
    gate: Arc<Gate>,
}

impl Hold {
    /// Lets the job report back.
    pub fn release(&self) {
        *self.gate.open.lock().expect("the gate") = true;
        self.gate.changed.notify_all();
    }
}

impl Drop for Hold {
    fn drop(&mut self) {
        self.release();
    }
}

/// Holds `job` of the project at `root` before it starts.
pub fn close(root: &Path, job: Job) -> Hold {
    let gate = Arc::new(Gate::default());

    GATES
        .lock()
        .expect("the gates")
        .push((root.to_path_buf(), job, gate.clone()));

    Hold { gate }
}

/// Waits here while a test holds `job` of the project at `root`.
pub(crate) fn pass(root: &Path, job: Job) {
    let gate = GATES
        .lock()
        .expect("the gates")
        .iter()
        .find(|(held, held_job, _)| held == root && *held_job == job)
        .map(|(_, _, gate)| gate.clone());

    if let Some(gate) = gate {
        let mut open = gate.open.lock().expect("the gate");

        while !*open {
            open = gate.changed.wait(open).expect("the gate");
        }
    }
}
