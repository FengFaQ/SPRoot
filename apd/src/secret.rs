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
    /// Normalise (trim + drop whitespace) and lock the key.
    pub fn new(raw: &str) -> Self {
        let key: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
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

/// Read the key from a 0600 file into a locked buffer.
///
/// Missing/unreadable is **not** an error surface: on first boot the manager
/// has not deployed yet (启动时序 §3.4 window), so callers stay silent.
pub fn read_key_file(path: &Path) -> Option<SecretKey> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.len() > MAX_KEY_FILE_LEN => {
            log::warn!("[key] key file unexpectedly large, ignoring");
            None
        }
        Ok(_) => match std::fs::read_to_string(path) {
            Ok(content) => {
                let key = SecretKey::new(&content);
                if key.is_empty() {
                    log::warn!("[key] key file empty");
                    None
                } else {
                    Some(key)
                }
            }
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
