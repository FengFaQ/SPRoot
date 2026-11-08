//! SK root escalation for SPRoot.
//!
//! Implements the "SKRoot ↔ AP 调用契约" v1.0 §3.1 (SK-ESC, asset
//! a_43f9e853a06b5109): escalation is `execve(key_str, NULL, NULL)` where
//! `key_str` is the path of the `su` file inside the SK hidden directory
//! (`/data/<key16>/su`, contract §3.4). SK's do_execve hook rewrites the
//! caller's credentials in place; on the success path execve itself returns
//! -ENOENT (the hook makes the path look absent to the exec) **and the process
//! is now uid 0**. Therefore:
//!
//! * never judge success by execve's return value;
//! * the only success criterion is `getuid() == 0` after execve returns;
//! * the kernel side fails silently (plain ENOENT, no counters), so AP-side
//!   logs must never contain the key itself or any prefix of it (contract
//!   §2.3 log-sanitisation requirement).
//!
//! All former SUPERCALL interfaces (su/grant/revoke/nums/list/reset_path/
//! get_safemode/kstorage) are gone: the authorisation table is now an
//! AP-managed userspace file (contract §3.2, N1), su path registration and the
//! safemode kernel query were removed (§3.4/§3.5).

use std::ffi::CString;

use log::{info, warn};

/// SK hidden-directory `su` deployment root: `/data/<key16>/...`. The install
/// chain (SPRoot install_su equivalent) deploys this AP multicall binary as
/// `<root>/su`; users exec it and argv[0] basename dispatch lands in
/// `root_shell()` with credentials already granted by the SK hook.
pub const SP_HIDDEN_DIR_PREFIX: &str = "/data/";

/// Safemode file flag (contract §3.5, third detection path). Presence means
/// safe mode; gives users a manual trigger without touching system props.
pub const SP_SAFEMODE_FILE: &str = "/data/adb/sproot/safemode";

/// Attempt SK escalation via `execve(key, NULL, NULL)`.
///
/// Returns `true` when the process is uid 0 after the attempt (including the
/// already-root case, which short-circuits without touching the key at all).
/// Returns `false` on invalid/absent key — silent, matching the kernel's
/// silent-reject behaviour; the caller may log a sanitised line.
pub fn escalate(key: &str) -> bool {
    // SAFETY: plain libc calls, no invariants beyond a valid CString.
    if unsafe { libc::getuid() } == 0 {
        return true;
    }
    if key.is_empty() {
        return false;
    }
    let Ok(c_key) = CString::new(key) else {
        return false;
    };
    // Contract §3.1: execve(key_str, NULL, NULL). On success the SK hook
    // rewrites creds and execve returns -ENOENT; the image is NOT replaced.
    // On a real path (deployed su file being exec'd as a shell) the image IS
    // replaced — that is the root-shell entry, not this function's concern.
    unsafe {
        libc::execve(c_key.as_ptr(), std::ptr::null(), std::ptr::null());
    }
    // The only success criterion (contract §3.1): uid became 0.
    unsafe { libc::getuid() == 0 }
}

/// Re-privilege the running AP process (contract §3.1). Previously this used
/// SUPERCALL_SU (0x1010) to flip the current pid's creds; the SK model has no
/// such syscall, so we escalate via the key path. When AP is already uid 0
/// (the common boot-chain case once the 启动时序 analysis lands) this is a
/// no-op short-circuit.
pub fn privilege_apd_profile(superkey: &Option<String>) {
    match superkey.as_deref() {
        Some(key) if !key.is_empty() => {
            if escalate(key) {
                info!("[privilege_apd_profile] escalated (uid is now 0)");
            } else {
                // Sanitised: never log the key or any prefix (contract §2.3).
                warn!("[privilege_apd_profile] escalation failed (key rejected)");
            }
        }
        _ => {
            warn!("[privilege_apd_profile] no key provided, staying unprivileged");
        }
    }
}

/// Kernel-side safemode query (SUPERCALL_SU_GET_SAFEMODE, 0x1112) was removed
/// per contract §3.5; the file-flag path lives in `utils::is_safe_mode`.
///
/// Kept as a private marker so reviewers grepping for supercall semantics find
/// this note instead of assuming the interface still exists.
const _: () = {
    // contract v1.0: GRANT/REVOKE/COUNT/LIST/SETPATH/SAFEMODE/KSTORAGE removed.
};

/// Authorisation table path: kept identical to APatch's package_config so
/// existing manager-written files migrate with zero conversion (contract
/// §3.2 "csv 零迁移"). The table IS the allow list; the `exclude` column
/// carries the per-uid exclusion bit (replacing kernel kstorage gid=1).
pub const ALLOW_LIST_FILE: &str = "/data/adb/ap/package_config";

/// Lock file guarding allow-list rewrites. Single-writer model (contract
/// §3.2): only the uid-listener thread takes it for writing; readers do not
/// lock (atomic tmp+rename makes every read see a complete generation).
pub const ALLOW_LIST_LOCK_FILE: &str = "/data/adb/ap/package_config.lock";
