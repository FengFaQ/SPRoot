//! Minimal SELinux helpers (no libselinux dependency).
//!
//! `setcon()` in libselinux is just a write to `/proc/self/attr/current`, so the
//! same effect is implemented with raw `open/write/close` calls — deliberately
//! async-signal-safe (no allocation) because it is also used from
//! `Command::pre_exec` for `root-exec --scontext`.
//!
//! On SK devices the process is already a "key process" (uid 0, caps full,
//! securebits 0), so `avc_denied` returns 0 for its denials and the write is
//! allowed without any policy change (A1 §8-1, SELinux zero-touch).

use std::ffi::CString;
use std::io;

const PROC_CURRENT: &str = "/proc/self/attr/current";

/// Raw `setcon`: write the NUL-terminated context to /proc/self/attr/current.
///
/// # Safety
/// Must only be called from a context where the caller has the privileges the
/// kernel checks for the attribute write; no Rust allocation happens here.
pub unsafe fn setcon_raw(con: &std::ffi::CStr) -> io::Result<()> {
    let path = CString::new(PROC_CURRENT)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "bad proc path"))?;
    let fd = unsafe { libc::open(path.as_ptr(), libc::O_WRONLY | libc::O_CLOEXEC) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    let bytes = con.to_bytes_with_nul();
    let mut off = 0usize;
    while off < bytes.len() {
        let n = unsafe {
            libc::write(
                fd,
                bytes[off..].as_ptr() as *const libc::c_void,
                bytes.len() - off,
            )
        };
        if n < 0 {
            let err = io::Error::last_os_error();
            unsafe { libc::close(fd) };
            return Err(err);
        }
        off += n as usize;
    }
    unsafe { libc::close(fd) };
    Ok(())
}

/// `setcon` on the current process/thread.
pub fn setcon(context: &str) -> io::Result<()> {
    let con = CString::new(context)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "context contains NUL"))?;
    unsafe { setcon_raw(&con) }
}

/// Current SELinux context (trimmed), or `None` when SELinux is unavailable.
pub fn current_context() -> Option<String> {
    std::fs::read_to_string(PROC_CURRENT)
        .ok()
        .map(|s| s.trim_end_matches('\0').trim().to_string())
        .filter(|s| !s.is_empty())
}

/// True when the kernel reports SELinux as enabled (`/sys/fs/selinux/enforce`
/// readable) — used by the selftest to distinguish "not supported" from
/// "denied".
pub fn selinux_present() -> bool {
    std::path::Path::new("/sys/fs/selinux/enforce").exists()
}
