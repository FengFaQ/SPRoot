//! `resetprop` façade.
//!
//! The real implementation needs `prop-rs` / `prop-rs-android`, which carry an
//! **UNKNOWN license** (no LICENSE file, no `license` field in the upstream
//! git repo). An AGPL-3.0 project cannot ship that, so the dependency is
//! feature-isolated:
//!
//! * default build (`--features resetprop`, also the default) → full Magisk
//!   `resetprop` compatibility, but the artifact contains the UNKNOWN-licensed
//!   crates and is **not** a release candidate;
//! * license-clean build (`--no-default-features`) → this stub, which keeps the
//!   binary buildable and returns an explicit "feature disabled" error instead
//!   of silently dropping the subcommand.
//!
//! Closure plan (decision deferred to humans): obtain written permission from
//! the upstream repo, or reimplement the bionic property-area access locally
//! (draft existed in `native_prop.rs`, removed from the tree to avoid dead code).

#[cfg(feature = "resetprop")]
#[path = "resetprop_prop_rs.rs"]
mod imp;

#[cfg(not(feature = "resetprop"))]
#[path = "resetprop_disabled.rs"]
mod imp;

#[allow(unused_imports)] // some entry points are only called by the full build
pub use imp::{load_system_prop_file, resetprop_main, set_prop};
