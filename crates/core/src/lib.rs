//! gust-core — listing, sorting, MIME, theme tokens, search.
//!
//! No Qt, no UI. Everything here is safe to call from a worker thread.

pub mod clipboard;
pub mod defaults;
pub mod devices;
pub mod drop;
pub mod entry;
pub mod grep;
pub mod lister;
pub mod location;
pub mod mime;
pub mod open;
pub mod peek;
pub mod search;
pub mod selection;
pub mod stall;
pub mod theme;
pub mod views;

pub use entry::{
    Completeness, DirectorySnapshot, Entry, EntryId, EntryKind, Metadata, SortField, SortSpec,
};
pub use lister::{fill_metadata, list_names, CancelToken};
pub use location::{Cap, Capabilities, LocationKey, ProviderId};
pub use theme::Tokens;
