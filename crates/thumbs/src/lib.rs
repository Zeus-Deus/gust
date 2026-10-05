//! gust-thumbs — freedesktop thumbnail cache + sandboxed thumbnailers.
//!
//! Untrusted bytes are decoded under bwrap+prlimit or not at all, and the
//! parent validates the helper's output through a held descriptor before
//! trusting it (AGENTS.md, DESIGN.md §11).

pub mod audio;
pub mod cache;
pub mod render;
pub mod sandbox;

pub use cache::{thumbnail_path, ThumbSize};
pub use sandbox::{validate_output, SandboxSpec, ValidationError};
