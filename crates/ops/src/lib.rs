//! gust-ops — copy/move/trash/rename, jobs, undo journal.
//!
//! Nothing in here touches Qt. Every destructive path has a test for its
//! confirmation and its undo (AGENTS.md).

pub mod anchor;
pub mod archive;
pub mod attrs;
pub mod bulk;
pub mod create;
pub mod engine;
pub mod fat;
pub mod jobs;
pub mod journal;
pub mod merge;
pub mod permanent;
pub mod rate;
pub mod recursive;
pub mod staging;
pub mod tags;
pub mod trash;
pub mod undo;

/// Tests that repoint `XDG_STATE_HOME` (the staging-claims store) must not
/// overlap: the variable is process-global and cargo runs tests in parallel,
/// so one test's claims would vanish into another's directory.
#[cfg(test)]
pub(crate) static TEST_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub use anchor::{resolve_parent, DirHandle, FileIdentity};
pub use engine::{classify, copy_regular_file, move_cross_fs, move_same_fs, FileClass, OpError};
pub use jobs::{JobHandle, JobProgress, Request};
pub use journal::{ConflictPolicy, Job, JobKind, JobState, Journal};
pub use recursive::{copy_tree, move_tree, plan_tree, remove_tree, rename_in_place, Plan};
pub use undo::{plan_undo, UndoPlan, UndoRefusal, UndoStep};
