//! Devenv: the process boundary that lets shells outlive node releases.
//!
//! The devenv server owns PTY masters and dials the node over a unix socket
//! on the shared run volume. The node keeps every piece of bookkeeping it has
//! today and drives shells through the link. See
//! `docs/plans/pty-survives-deploy.md`.

pub mod launcher;
pub mod link;
pub mod protocol;
pub mod server;
