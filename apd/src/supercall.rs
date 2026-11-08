//! SK root escalation for SPRoot (contract v1.1 §3.1 / §2.3, asset for
//! `docs/phase2_skroot_ap_call_contract_v1.1.md`).
//!
//! Escalation is `execve(k47, NULL, NULL)` where `k47` is the **first 47
//! characters of the 48-char root key** (v1.1 E1: the key string itself, NOT a
//! hidden-directory path). The SK do_execve hook compares the exec filename
//! byte-by-byte against the 47-char key embedded in a kernel trampoline; on a
//! match it rewrites the caller's credentials in place (uid/gid 0, caps full,
//! securebits 0, TIF_SECCOMP + task->seccomp cleared) and returns control to
//! the original exec path, which then fails with ENOENT because `k47` is not a
//! real file. Therefore:
//!
//! * never judge success by the execve return value (success path is -ENOENT);
//! * the only success criterion is `getuid() == 0` after the call;
//! * the kernel side fails silently (plain ENOENT, no counters, no audit), so
//!   AP-side logs must never contain the key or any prefix of it.
//!
//! All former SUPERCALL interfaces are gone (v1.0 already removed su/grant/
//! revoke/nums/list/reset_path/get_safemode/kstorage): the authorisation table
//! is an AP-managed userspace file (contract §3.2), su path registration and
//! the safe-mode kernel query were removed (§3.4/§3.5), and the v1.0
//! `SP_REQUEST_UID` env-injection idea is dropped in v1.1 (E3) — the requesting
//! uid now comes from the daemon's socket `SO_PEERCRED` (see `daemon.rs`).

use std::ffi::CString;

use log::{info, warn};

/// Env var set **only by the resident daemon** before it execs the su binary
/// for an authorised socket session. It must never be trusted when set by
/// anything else: the daemon is root and forks the session child itself, so
/// the value cannot be spoofed by the requesting app. Name kept distinct from
/// v1.0's kernel-injected `SP_REQUEST_UID` because that mechanism does not
/// exist (v1.1 E3).
pub const REQUEST_UID_ENV: &str = "SPROOT_REQUEST_UID";

/// Number of key characters actually embedded in / compared by the kernel.
/// The public key is 48 chars; the last one is display-only (A1 §4.1, E1).
pub const KERNEL_KEY_LEN: usize = 47;

/// Return the escalation token: the key truncated to [`KERNEL_KEY_LEN`].
///
/// Accepts both 47- and 48-char keys so callers never have to care. Anything
/// shorter than 47 is returned unchanged (the kernel compare will fail).
pub fn escalation_token(key: &str) -> &str {
    let key = key.trim();
    match key.char_indices().nth(KERNEL_KEY_LEN) {
        Some((idx, _)) => &key[..idx],
        None => key,
    }
}

/// Attempt SK escalation via `execve(k47, NULL, NULL)`.
///
/// Returns `true` when the process is uid 0 after the attempt (including the
/// already-root short-circuit, which never touches the key). Returns `false`
/// on invalid/absent key — silent, matching the kernel's silent-reject
/// behaviour; the caller may log one sanitised line.
pub fn escalate(key: &str) -> bool {
    // SAFETY: plain libc calls, no invariants beyond a valid CString.
    if unsafe { libc::getuid() } == 0 {
        return true;
    }
    let token = escalation_token(key);
    if token.is_empty() {
        return false;
    }
    let Ok(c_key) = CString::new(token) else {
        return false;
    };
    // SAFETY: `c_key` is a valid NUL-terminated string; argv/envp are NULL as
    // the contract requires (v1.1 §3.1). On success the SK hook rewrites creds
    // and execve returns -ENOENT without replacing the image.
    unsafe {
        libc::execve(c_key.as_ptr(), std::ptr::null(), std::ptr::null());
    }
    // The only success criterion (contract v1.1 §3.1).
    unsafe { libc::getuid() == 0 }
}

/// Re-privilege the running AP process via the key path (contract v1.1 §3.1).
/// No-op when already uid 0 (the common boot-chain / root-context case).
pub fn privilege_apd_profile(superkey: &Option<String>) {
    match superkey.as_deref() {
        Some(key) if !key.trim().is_empty() => {
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

/// True when the current process is (already) root.
pub fn is_root() -> bool {
    // SAFETY: trivially safe libc call.
    unsafe { libc::getuid() == 0 }
}

/// Upper bound on the size of a key file the daemon is willing to read.
const MAX_KEY_FILE_LEN: u64 = 4096;

/// Read the 48-char key from a 0600 file (`--superkey-file`, contract v1.1 §4).
///
/// Never logs the path contents on failure: a missing key file is a normal
/// first-boot state (the manager has not deployed yet) and must stay silent
/// (启动时序 §3.4 window).
pub fn read_key_file(path: &std::path::Path) -> Option<String> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.len() > MAX_KEY_FILE_LEN => {
            warn!("[key] key file unexpectedly large, ignoring");
            None
        }
        Ok(_) => match std::fs::read_to_string(path) {
            Ok(content) => {
                let key = content.trim().to_string();
                if key.is_empty() {
                    warn!("[key] key file empty");
                    None
                } else {
                    Some(key)
                }
            }
            Err(e) => {
                warn!("[key] key file unavailable ({})", e.kind());
                None
            }
        },
        Err(_) => None,
    }
}
