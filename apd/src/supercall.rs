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

use anyhow::{Result, bail, ensure};
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
///
/// This is the engine/kernel side of the ABI (`patch_do_execve.cpp`: the hook
/// compares the exec filename byte-by-byte against these 47 chars and then
/// expects the NUL terminator). Changing it means re-patching the kernel, so it
/// is pinned here at compile time: a change in this constant must be a
/// deliberate, all-chain ABI bump (docs/NEW-DOC-05 §12).
pub const KERNEL_KEY_LEN: usize = 47;
const _: () = assert!(KERNEL_KEY_LEN == 47);

/// Return the escalation token: **exactly** [`KERNEL_KEY_LEN`] bytes, or an
/// error — never a silently-passed-through wrong-length string.
///
/// This is the last link of the key ABI chain and the single input to
/// `execve()`, so a wrong length here is a *silent* escalation failure (the
/// kernel compares 47 chars + NUL and just returns `ENOENT`). Rules:
///
/// * 47 chars — the AP-side canonical token, returned as is;
/// * 48 chars — the manager's display format (`SecretStore` generates 48, the
///   last position is display-only, C1 §2.2 / contract v1.1 E1). The first 47
///   bytes are used and the drop is logged (`warn!`), never silent. Feeding the
///   full 48-char name to `execve()` would compare 47 bytes + NUL against a
///   48-byte name → mismatch → rc 125 that looks like "key not deployed";
/// * shorter than 47, longer than 48, or non-ASCII — hard error. A 49+ key is a
///   *different* key from another ABI (the Skroot-Next mix-up); truncating it
///   would be indistinguishable from "key rejected by the kernel".
pub fn escalation_token(key: &str) -> Result<&str> {
    let key = key.trim();
    ensure!(
        !key.is_empty(),
        "no key: escalation needs the 47-char token"
    );
    ensure!(
        key.is_ascii(),
        "key is not ASCII: the 47-char ABI is [A-Za-z0-9]"
    );
    match key.len() {
        KERNEL_KEY_LEN => Ok(key),
        n if n == KERNEL_KEY_LEN + 1 => {
            warn!(
                "[supercall] 输入 {n} 位（管理器 48 位展示格式，第 48 位仅供展示）：按 47 字节 ABI 取前 {KERNEL_KEY_LEN}"
            );
            Ok(&key[..KERNEL_KEY_LEN])
        }
        n if n < KERNEL_KEY_LEN => bail!(
            "key too short for the {KERNEL_KEY_LEN}-char ABI ({n} < {KERNEL_KEY_LEN}): \
             内核按 47 字符比对，短串必然失配，拒绝透传"
        ),
        n => bail!(
            "key is {n} bytes but the ABI is {KERNEL_KEY_LEN} (48 at most, 1 display position): \
             疑似两套密钥 ABI 混用（Skroot-Next 48 字节无 NUL），拒绝截断"
        ),
    }
}

/// Attempt SK escalation via `execve(k47, NULL, NULL)`.
///
/// Returns `true` when the process is uid 0 after the attempt (including the
/// already-root short-circuit, which never touches the key). Returns `false`
/// when the key cannot form a 47-byte token (length/ASCII ABI violation) or is
/// rejected by the kernel — the kernel side is silent by design, so AP always
/// logs exactly one sanitised line and the caller maps `false` to rc 125.
pub fn escalate(key: &str) -> bool {
    // SAFETY: plain libc calls, no invariants beyond a valid CString.
    if unsafe { libc::getuid() } == 0 {
        return true;
    }
    let token = match escalation_token(key) {
        Ok(token) => token,
        Err(e) => {
            // Sanitised: the error carries only length/shape, never key bytes.
            warn!("[supercall] escalation key rejected: {e}");
            return false;
        }
    };
    // The token is now exactly 47 ASCII bytes by construction (the only shape
    // the kernel can match); keep the NUL guard for direct low-level callers.
    debug_assert_eq!(token.len(), KERNEL_KEY_LEN);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: usize) -> String {
        "a".repeat(n)
    }

    /// Point 4 of the key-ABI chain (docs/NEW-DOC-05 §12): whatever goes into
    /// `execve()` must be exactly 47 bytes. 48 is the display format → 47;
    /// everything else is a loud error, never a silent pass-through.
    #[test]
    fn escalation_token_is_exactly_47_or_an_error() {
        // 47: canonical AP-side token, unchanged.
        assert_eq!(escalation_token(&key(47)).unwrap(), key(47));
        // 48: display position dropped, token still exactly 47 (warn! logged).
        assert_eq!(escalation_token(&key(48)).unwrap(), key(47));
        // Short keys used to be passed through and then fail inside the kernel
        // (silently). They must be rejected here instead.
        for n in [0, 1, 16, 46] {
            assert!(
                escalation_token(&key(n)).is_err(),
                "{n} chars must be rejected, not passed through"
            );
        }
        // 49+ = a different key from another ABI: no truncation.
        for n in [49, 50, 96] {
            assert!(escalation_token(&key(n)).is_err(), "{n} chars must be rejected");
        }
        // Non-ASCII cannot satisfy a byte-for-byte 47-char compare.
        assert!(escalation_token(&"é".repeat(47)).is_err());
        // Surrounding whitespace is normalised away, not counted.
        assert_eq!(escalation_token(&format!("  {}\n", key(47))).unwrap(), key(47));
    }

    /// Points 2+3: the canonical file is 47 bytes → `SecretKey` (daemon input)
    /// is 47 bytes → the escalation token is 47 bytes. A 48-byte file (old
    /// manager bug / kernel format mixed in) still normalises to 47, with the
    /// drop logged, so the chain never feeds 48 bytes to the kernel.
    #[test]
    fn key_file_to_escalation_token_stays_47() {
        let path = std::env::temp_dir().join(format!("sproot-key-abi-{}", std::process::id()));
        std::fs::write(&path, key(47)).unwrap();
        let stored = secret::read_key_file(&path).expect("47-byte key file must load");
        assert_eq!(stored.len(), KERNEL_KEY_LEN);
        assert_eq!(escalation_token(stored.as_str()).unwrap(), key(47));

        // 47 + NUL (kernel-inlined shape accidentally copied into the file).
        std::fs::write(&path, format!("{}\0", key(47))).unwrap();
        let stored = secret::read_key_file(&path).expect("47+NUL key file must still load");
        assert_eq!(stored.len(), KERNEL_KEY_LEN);
        assert_eq!(escalation_token(stored.as_str()).unwrap(), key(47));

        // 48 alnum (old manager written file): SecretKey keeps 48, token is 47.
        std::fs::write(&path, key(48)).unwrap();
        let stored = secret::read_key_file(&path).expect("48-byte key file must still load");
        assert_eq!(stored.len(), 48);
        assert_eq!(escalation_token(stored.as_str()).unwrap(), key(47));

        let _ = std::fs::remove_file(&path);
    }
}
