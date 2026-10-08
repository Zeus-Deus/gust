//! gust-remote — mount manager: sshfs, gio, rclone, discovery.
//!
//! Rules that drive this crate (plan §3, §9):
//! - discovery NEVER opens a connection by itself
//! - SSH config parsing is a non-executing, bounded parse of literal aliases
//! - host-key verification is never disabled
//! - no secret in argv, env, logs or config files

pub mod capabilities;
pub mod cloud;
pub mod disconnect;
pub mod mount;
pub mod phones;
pub mod shares;
pub mod ssh_config;
pub mod tailscale;
pub mod upload;

pub use capabilities::probe_from_fsinfo;
pub use cloud::{
    backend_listing, confirmed_on_backend, list_remotes, rclone_available, relative_to_mount,
    vfs_pending, BackendEntry, CloudError, CloudRemote, VfsPending,
};
pub use ssh_config::{parse_ssh_config, SshHost};
pub use upload::{UploadState, UploadTracker};
