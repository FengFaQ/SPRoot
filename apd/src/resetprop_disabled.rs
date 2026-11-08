//! Stub used when the `resetprop` feature is disabled (license-clean build).
//!
//! Never fails silently: the CLI subcommand and internal callers get an explicit
//! error explaining why the capability is absent.

use anyhow::{Result, bail};

pub fn resetprop_main(_args: &[String]) -> ! {
    eprintln!(
        "resetprop: this build was compiled with --no-default-features (license-clean build).\n\
         The resetprop feature is disabled because its upstream dependency prop-rs has an \
         UNKNOWN license (no LICENSE file) and cannot ship in an AGPL-3.0 release.\n\
         Use the default build for full resetprop compatibility; see \
         apd/THIRD_PARTY_LICENSES.md and NOTICE §8."
    );
    std::process::exit(1);
}

pub fn load_system_prop_file(_path: &std::path::Path) -> Result<()> {
    bail!(
        "resetprop/system.prop support is disabled in this license-clean build \
         (prop-rs UNKNOWN license excluded); use the default build"
    )
}

pub fn set_prop(_name: &str, _value: &str) -> Result<()> {
    bail!("resetprop support is disabled in this license-clean build")
}
