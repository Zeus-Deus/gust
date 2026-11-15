//! gust-agent — device transfers for AI agents, served as `gust --mcp`
//! (DESIGN §6.28). No Qt and no display: it runs before any window exists.
//!
//! Every helper program is reached through `gust-remote`; every copy goes
//! through `gust-ops`. This crate only decides what is allowed and speaks
//! the protocol.

// stdout belongs to the protocol: one stray line corrupts the session.
#![deny(clippy::print_stdout)]

mod admit;
mod devices;
pub mod mcp;
mod tools;

pub use mcp::run;
