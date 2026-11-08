use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use anyhow::Result;
use jwalk::{Parallelism::Serial, WalkDir};

use crate::defs;

pub const SYSTEM_CON: &str = "u:object_r:system_file:s0";
pub const ADB_CON: &str = "u:object_r:adb_data_file:s0";
pub const UNLABEL_CON: &str = "u:object_r:unlabeled:s0";

const SELINUX_XATTR: &str = "security.selinux";

/// `lsetxattr(2)` wrapper (raw libc).
///
/// Replaces the `extattr` crate, which is **GPL-2.0-only** and therefore
/// incompatible with this project's AGPL-3.0 license. The wire behaviour is
/// byte-identical to the previous implementation: the context value is written
/// as-is (no trailing NUL is appended), which is what APatch shipped and what
/// the kernel's `security_inode_setsecurity` accepts.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn lsetxattr_raw<P: AsRef<Path>>(path: P, name: &str, value: &str) -> std::io::Result<()> {
    let path = path.as_ref();
    let cpath = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "path contains NUL")
    })?;
    let cname = CString::new(name).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "xattr name contains NUL")
    })?;
    let rc = unsafe {
        libc::lsetxattr(
            cpath.as_ptr(),
            cname.as_ptr(),
            value.as_ptr() as *const libc::c_void,
            value.len(),
            0,
        )
    };
    if rc == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

/// `lgetxattr(2)` wrapper (raw libc). Two calls: size probe, then read.
#[cfg(any(target_os = "linux", target_os = "android"))]
fn lgetxattr_raw<P: AsRef<Path>>(path: P, name: &str) -> std::io::Result<Vec<u8>> {
    let path = path.as_ref();
    let cpath = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "path contains NUL")
    })?;
    let cname = CString::new(name).map_err(|_| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "xattr name contains NUL")
    })?;
    let size = unsafe {
        libc::lgetxattr(
            cpath.as_ptr(),
            cname.as_ptr(),
            std::ptr::null_mut(),
            0,
        )
    };
    if size < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let mut buf = vec![0u8; size as usize];
    if size == 0 {
        return Ok(buf);
    }
    let read = unsafe {
        libc::lgetxattr(
            cpath.as_ptr(),
            cname.as_ptr(),
            buf.as_mut_ptr() as *mut libc::c_void,
            buf.len(),
        )
    };
    if read < 0 {
        return Err(std::io::Error::last_os_error());
    }
    buf.truncate(read as usize);
    Ok(buf)
}

pub fn lsetfilecon<P: AsRef<Path>>(path: P, con: &str) -> Result<()> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        use anyhow::Context;
        lsetxattr_raw(&path, SELINUX_XATTR, con).with_context(|| {
            format!(
                "Failed to change SELinux context for {}",
                path.as_ref().display()
            )
        })?;
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        let _ = (path, con);
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn lgetfilecon<P: AsRef<Path>>(path: P) -> Result<String> {
    use anyhow::Context;
    let con = lgetxattr_raw(&path, SELINUX_XATTR).with_context(|| {
        format!(
            "Failed to get SELinux context for {}",
            path.as_ref().display()
        )
    })?;
    Ok(String::from_utf8_lossy(&con).to_string())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn setsyscon<P: AsRef<Path>>(path: P) -> Result<()> {
    lsetfilecon(path, SYSTEM_CON)
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub fn setsyscon<P: AsRef<Path>>(path: P) -> Result<()> {
    let _ = path;
    unimplemented!()
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub fn lgetfilecon<P: AsRef<Path>>(path: P) -> Result<String> {
    let _ = path;
    unimplemented!()
}

pub fn restore_syscon<P: AsRef<Path>>(dir: P) -> Result<()> {
    for dir_entry in WalkDir::new(dir).parallelism(Serial) {
        if let Some(path) = dir_entry.ok().map(|dir_entry| dir_entry.path()) {
            setsyscon(&path)?;
        }
    }
    Ok(())
}

fn restore_syscon_if_unlabeled<P: AsRef<Path>>(dir: P) -> Result<()> {
    for dir_entry in WalkDir::new(dir).parallelism(Serial) {
        if let Some(path) = dir_entry.ok().map(|dir_entry| dir_entry.path())
            && let Result::Ok(con) = lgetfilecon(&path)
            && (con == UNLABEL_CON || con.is_empty())
        {
            lsetfilecon(&path, SYSTEM_CON)?;
        }
    }
    Ok(())
}

pub fn restorecon() -> Result<()> {
    lsetfilecon(defs::DAEMON_PATH, ADB_CON)?;
    restore_syscon_if_unlabeled(defs::MODULE_DIR)?;
    Ok(())
}
