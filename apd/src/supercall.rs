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

use log::{info, warn};

use crate::secret;

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
/// shorter than 47 is returned unchanged (the kernel compare will fail). A key
/// longer than the 48-position display format is an ABI mix-up: it is still
/// reduced to 47 (the kernel's only notion of the key) but the anomaly is
/// logged — never silent — so a wrong-ABI key does not look like a mere
/// "kernel rejected the token" failure later (HANDOFF §6.8).
pub fn escalation_token(key: &str) -> &str {
    let key = key.trim();
    match key.char_indices().nth(KERNEL_KEY_LEN) {
        Some((idx, _)) => {
            // 48 = 47 + the display-only position (C1 §2.2); only lengths
            // beyond that mean two ABIs were mixed up.
            if key.len() > KERNEL_KEY_LEN + 1 {
                warn!(
                    "[supercall] key is {} chars but the ABI is 47 (+1 display); using the first 47",
                    key.len()
                );
            }
            &key[..idx]
        }
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
    if token.as_bytes().contains(&0) {
        return false;
    }
    // Own buffer so it can be wiped immediately after the call; CString has no
    // zeroising Drop (P2: no key copies left in the heap).
    let mut bytes = token.as_bytes().to_vec();
    bytes.push(0);
    // SAFETY: argv/envp are NULL as the contract requires (v1.1 §3.1); on
    // success the SK hook rewrites creds and execve returns -ENOENT without
    // replacing the image.
    unsafe {
        libc::execve(
            bytes.as_ptr() as *const libc::c_char,
            std::ptr::null(),
            std::ptr::null(),
        );
    }
    // The only success criterion (contract v1.1 §3.1).
    let ok = unsafe { libc::getuid() == 0 };
    unsafe { secret::wipe_bytes(bytes.as_mut_ptr(), bytes.len()) };
    ok
}

/// Re-privilege the running AP process via the key path (contract v1.1 §3.1).
/// No-op when already uid 0 (the common boot-chain / root-context case).
pub fn privilege_apd_profile(superkey: Option<&str>) {
    match superkey {
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
