//! gust-ops — copy/move/trash/rename, jobs, undo journal.
//!
//! Nothing in here touches Qt. Every destructive path has a test for its
//! confirmation and its undo (AGENTS.md).

pub mod anchor;
pub mod archive;
pub mod bulk;
pub mod engine;
pub mod jobs;
pub mod journal;
pub mod permanent;
pub mod recursive;
pub mod trash;
pub mod undo;

pub use anchor::{resolve_parent, DirHandle, FileIdentity};
pub use engine::{classify, copy_regular_file, move_cross_fs, move_same_fs, FileClass, OpError};
pub use jobs::{JobHandle, JobProgress, Request};
pub use journal::{ConflictPolicy, Job, JobKind, JobState, Journal};
pub use recursive::{copy_tree, move_tree, plan_tree, remove_tree, rename_in_place, Plan};
pub use undo::{plan_undo, UndoPlan, UndoRefusal, UndoStep};
