//! Foreground S3 transfers: control authorizes requests; the node installs bytes.
pub mod install;
pub mod model;
pub mod store;

pub const MAX_BYTES: i64 = 50 * 1024 * 1024;
