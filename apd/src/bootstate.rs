//! Boot-loop guard (NEW-DOC-05 §5.5, contract v1.1 §3.5 addendum).
//!
//! The SK kernel has no supervisor: if a module's `post-fs-data.sh` hangs the
//! device never reaches `boot-completed`, and there is no kernel side that can
//! decide to disable modules (unlike KernelPatch, which owns the event loop).
//! The userspace replacement is a monotonic counter:
//!
//! * every `post-fs-data` run increments `boot_count`;
//! * a successful `boot-completed` clears it;
//! * `boot_count >= 2` (two consecutive boots that never completed) writes the
//!   safe-mode file and disables all modules, so the third boot comes up with
//!   the module set disabled instead of looping forever.
//!
//! All operations are best-effort: boot must never fail because the guard files
//! are unwritable.

use std::{fs, io::Write, path::Path};

use log::{info, warn};

use crate::defs;

/// How many consecutive incomplete boots force safe mode.
const BOOT_COUNT_THRESHOLD: u32 = 2;

fn read_count() -> u32 {
    fs::read_to_string(defs::SPROOT_BOOT_COUNT_FILE)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .unwrap_or(0)
}

fn write_count(count: u32) {
    let _ = fs::create_dir_all(defs::SPROOT_DIR);
    if let Ok(mut file) = fs::File::create(defs::SPROOT_BOOT_COUNT_FILE) {
        let _ = writeln!(file, "{count}");
        let _ = file.sync_all();
    }
}

/// Called at the very start of `apd post-fs-data`, before any module script can
/// run. Returns the new attempt count.
pub fn post_fs_data_start() -> u32 {
    let count = read_count().saturating_add(1);
    write_count(count);
    info!("[bootstate] boot attempt #{count}");

    if count >= BOOT_COUNT_THRESHOLD {
        warn!(
            "[bootstate] {count} consecutive boots without boot-completed; forcing safe mode"
        );
        force_safe_mode();
    }
    count
}

/// Called after the boot-completed stage has been dispatched.
pub fn boot_completed() {
    let _ = fs::remove_file(defs::SPROOT_BOOT_COUNT_FILE);
    info!("[bootstate] boot completed, counter cleared");
}

/// Write the safe-mode file (contract v1.1 §3.5 third detection path) and
/// disable every module so the next boot is guaranteed to come up.
pub fn force_safe_mode() {
    let _ = fs::create_dir_all(defs::SPROOT_DIR);
    let _ = fs::File::create(defs::SPROOT_SAFEMODE_FILE);
    if let Err(e) = crate::module::disable_all_modules() {
        warn!("[bootstate] disable all modules failed: {e}");
    }
}

/// True when the manager/debugger dropped the safe-mode flag file.
pub fn safe_mode_file_present() -> bool {
    Path::new(defs::SPROOT_SAFEMODE_FILE).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn threshold_is_at_least_two() {
        // One failed boot must not lock the user out of their modules.
        assert!(BOOT_COUNT_THRESHOLD >= 2);
    }
}
