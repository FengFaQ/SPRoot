//! Safely-locked key material (P2: mlock + volatile wipe).
//!
//! The 48-char root key is the single credential of the whole SK model, so the
//! daemon/CLI must not leave copies lying in the heap, in swap, or in a dropped
//! `String`. [`SecretKey`] therefore:
//!
//! 1. normalises the key (strip whitespace) **before** locking;
//! 2. `mlock()`s the page range that backs the string (best effort — a failure
//!    is not fatal, it only means the key may be swappable);
//! 3. zeroises the bytes with `write_volatile` on drop and only then releases
//!    the lock.
//!
//! Cloning produces a *new* locked copy (used by the daemon for its worker
//! threads); nothing ever hands out a `&mut String` that could be reallocated
//! out of the locked range.

use std::path::Path;

/// Maximum key-file size we are willing to read.
const MAX_KEY_FILE_LEN: u64 = 4096;

/// A key string whose heap pages are locked and wiped on drop.
pub struct SecretKey {
    key: String,
    /// (page-aligned start, length) of the mlock()ed range, when locking worked.
    locked: Option<(usize, usize)>,
}

impl SecretKey {
    /// Normalise and lock the key.
    ///
    /// Whitespace and NUL are dropped: the on-disk ABI is "47 chars + NUL"
    /// (48 bytes, contract v1.1 E1), so a raw read of the key file must not
    /// leave the terminator inside the key string (it would fail the
    /// `[A-Za-z0-9]` check and lengthen the escalation token).
    pub fn new(raw: &str) -> Self {
        let key: String = raw
            .chars()
            .filter(|c| !c.is_whitespace() && *c != '\0')
            .collect();
        let locked = unsafe { lock_range(key.as_ptr() as usize, key.len()) };
        SecretKey { key, locked }
    }

    pub fn as_str(&self) -> &str {
        &self.key
    }

    pub fn is_empty(&self) -> bool {
        self.key.is_empty()
    }

    pub fn len(&self) -> usize {
        self.key.len()
    }

    /// True when this copy actually got mlock()ed (selftest/diagnostic).
    pub fn is_locked(&self) -> bool {
        self.locked.is_some()
    }
}

impl Clone for SecretKey {
    fn clone(&self) -> Self {
        // Clones are re-locked; the daemon passes one copy per worker thread.
        SecretKey::new(self.as_str())
    }
}

impl Drop for SecretKey {
    fn drop(&mut self) {
        unsafe {
            wipe_bytes(self.key.as_mut_ptr(), self.key.len());
            if let Some((start, len)) = self.locked {
                libc::munlock(start as *const libc::c_void, len);
            }
        }
    }
}

/// Lock the pages that contain `[ptr, ptr+len)`. mlock needs a page-aligned
/// address, so the containing page range is used; returns `None` on failure.
unsafe fn lock_range(ptr: usize, len: usize) -> Option<(usize, usize)> {
    if len == 0 {
        return None;
    }
    let page = libc::sysconf(libc::_SC_PAGESIZE);
    if page <= 0 {
        return None;
    }
    let page = page as usize;
    let start = ptr & !(page - 1);
    let end = ptr.checked_add(len)?;
    let page_len = end.checked_sub(start)?;
    if libc::mlock(start as *const libc::c_void, page_len) == 0 {
        Some((start, page_len))
    } else {
        None
    }
}

/// Overwrite `len` bytes with zeros in a way the optimiser cannot elide.
pub unsafe fn wipe_bytes(ptr: *mut u8, len: usize) {
    for i in 0..len {
        std::ptr::write_volatile(ptr.add(i), 0u8);
    }
    std::sync::atomic::compiler_fence(std::sync::atomic::Ordering::SeqCst);
}

/// Overwrite and clear a `String` in place (for non-`SecretKey` copies that the
/// OS APIs force us to build, e.g. `Command` arguments).
pub fn wipe_string(s: &mut String) {
    unsafe { wipe_bytes(s.as_mut_ptr(), s.len()) };
    s.clear();
}

/// Normalise raw key bytes into a [`SecretKey`], wiping the source buffer.
///
/// BUG-APD-6 (2026-11-13): the previous `read_key_file` used
/// `fs::read_to_string`, which left the whole key in a plain `String` that was
/// dropped un-wiped — released heap pages holding key bytes, outside the
/// mlock'ed `SecretKey` copy. Every path that carries key material must now go
/// through this function: the caller's buffer is zeroised with
/// `write_volatile` before being dropped, so no plain-String copy of the key
/// ever outlives the read.
fn key_from_buffer(bytes: Vec<u8>) -> Option<SecretKey> {
    // UTF-8 validation is kept (the pre-fix read_to_string behaviour): a
    // non-UTF-8 key file is an error, not silently truncated.
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(e) => {
            log::warn!("[key] key file is not valid UTF-8");
            // from_utf8 hands the original buffer back on error: wipe it.
            let mut bytes = e.into_bytes();
            unsafe {
                wipe_bytes(bytes.as_mut_ptr(), bytes.len());
            }
            return None;
        }
    };
    let mut text = text;
    // Build the locked copy FIRST, then wipe the plain-String source copy
    // before it is dropped (from_utf8 consumes the Vec, so this IS the
    // original allocation — wiping after SecretKey::new still leaves the
    // SecretKey's own mlocked copy intact).
    let key = SecretKey::new(&text);
    unsafe {
        wipe_bytes(text.as_mut_ptr(), text.len());
    }
    text.clear();
    if key.is_empty() {
        log::warn!("[key] key file empty");
        None
    } else {
        Some(key)
    }
}

/// Read the key from a 0600 file into a locked buffer.
///
/// Missing/unreadable is **not** an error surface: on first boot the manager
/// has not deployed yet (启动时序 §3.4 window), so callers stay silent.
///
/// The raw bytes are read into a self-managed `Vec<u8>` and normalised through
/// [`key_from_buffer`], which zeroises the plain-Text source copy — no
/// `read_to_string` of key material anywhere in this file (a source-level
/// regression guard asserts this).
pub fn read_key_file(path: &Path) -> Option<SecretKey> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.len() > MAX_KEY_FILE_LEN => {
            log::warn!("[key] key file unexpectedly large, ignoring");
            None
        }
        Ok(_) => match std::fs::read(path) {
            Ok(bytes) => key_from_buffer(bytes),
            Err(e) => {
                log::warn!("[key] key file unavailable ({})", e.kind());
                None
            }
        },
        Err(_) => None,
    }
}

/// Best-effort `mlock`/`munlock` probe used by the selftest to prove the
/// platform allows locking (some ROMs restrict it).
pub fn mlock_supported() -> bool {
    let buf = vec![0u8; 64];
    match unsafe { lock_range(buf.as_ptr() as usize, buf.len()) } {
        Some((start, len)) => {
            unsafe { libc::munlock(start as *const libc::c_void, len) };
            true
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// BUG-APD-6 (2026-11-13) negative sample: the key must never travel
    /// through an un-wiped `read_to_string` String. Source-level guard, scoped
    /// to the read path only (this test's own source lives in the same file
    /// and names the forbidden call — that is why the search stops at
    /// `mod tests`).
    #[test]
    fn read_key_file_never_uses_read_to_string() {
        let src = include_str!("secret.rs");
        // Strip // comments so prose (which documents the defect) never trips
        // the guard, and bound the region to the I/O functions: `read_key_file`
        // + `key_from_buffer` end where the next free item starts; the tests
        // module (further down) contains the forbidden literal as a *string*
        // and must not be searched.
        let mut code = String::new();
        for line in src.lines() {
            if line.trim_start().starts_with("mod tests") {
                break;
            }
            let code_part = line.split("//").next().unwrap_or(line);
            code.push_str(code_part);
            code.push('\n');
        }
        assert!(
            !code.contains("read_to_string"),
            "key material must not be read via read_to_string (BUG-APD-6: the \
             plain-String copy was dropped un-wiped); use the self-managed \
             buffer + wipe path"
        );
        // And the wipe path must actually be wired into the read path.
        assert!(
            code.contains("key_from_buffer"),
            "read_key_file must normalise through key_from_buffer (which wipes)"
        );
        assert!(
            code.contains("fs::read(path)"),
            "read_key_file must use the self-managed byte buffer (fs::read)"
        );
    }

    /// The wipe helper genuinely zeroes memory (this is the property
    /// `key_from_buffer` relies on for the source copy).
    #[test]
    fn wipe_bytes_zeroes_the_buffer() {
        let mut buf: Vec<u8> = b"secret-key-material".to_vec();
        unsafe {
            wipe_bytes(buf.as_mut_ptr(), buf.len());
        }
        assert!(buf.iter().all(|&b| b == 0), "every byte must be zeroed");
    }

    /// key_from_buffer: a valid key passes through, an empty file is None,
    /// and a non-UTF-8 file is rejected (the UTF-8 check from the old
    /// read_to_string path is preserved).
    #[test]
    fn key_from_buffer_validates_and_passes_through() {
        let key = key_from_buffer(b"a".repeat(47)).expect("a 47-byte key must load");
        assert_eq!(key.len(), 47);

        // 47 + NUL (kernel-inlined shape) normalises to 47.
        let with_nul: Vec<u8> = {
            let mut v = b"a".repeat(47);
            v.push(0);
            v
        };
        let key = key_from_buffer(with_nul).expect("47+NUL must still load");
        assert_eq!(key.len(), 47);

        // Empty is None.
        assert!(key_from_buffer(Vec::new()).is_none(), "empty key is None");

        // Non-UTF-8 is rejected (no silent truncation).
        let invalid = vec![0xFFu8; 10];
        assert!(
            key_from_buffer(invalid).is_none(),
            "non-UTF-8 key bytes must be rejected"
        );
    }
}
