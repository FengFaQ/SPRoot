#[allow(unused_imports)]
use std::fs::{Permissions, set_permissions};
#[cfg(unix)]
use std::os::unix::prelude::PermissionsExt;
use std::{
    fs::{File, OpenOptions, create_dir_all, metadata},
    io::{ErrorKind::AlreadyExists, Write},
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    thread,
    time::Duration,
};

use anyhow::{Context, Error, Ok, Result, bail};
use log::info;

use crate::defs;

pub fn ensure_file_exists<T: AsRef<Path>>(file: T) -> Result<()> {
    match File::options().write(true).create_new(true).open(&file) {
        Result::Ok(_) => Ok(()),
        Err(err) => {
            if err.kind() == AlreadyExists && file.as_ref().is_file() {
                Ok(())
            } else {
                Err(Error::from(err))
                    .with_context(|| format!("{} is not a regular file", file.as_ref().display()))
            }
        }
    }
}

pub fn ensure_dir_exists<T: AsRef<Path>>(dir: T) -> Result<()> {
    let result = create_dir_all(&dir).map_err(Error::from);
    if dir.as_ref().is_dir() {
        result
    } else if result.is_ok() {
        bail!("{} is not a regular directory", dir.as_ref().display())
    } else {
        result
    }
}

// todo: ensure
pub fn ensure_binary<T: AsRef<Path>>(path: T) -> Result<()> {
    set_permissions(&path, Permissions::from_mode(0o755))?;
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn getprop(prop: &str) -> Option<String> {
    android_properties::getprop(prop).value()
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub fn getprop(_prop: &str) -> Option<String> {
    unimplemented!()
}
pub fn run_command(
    command: &str,
    args: &[&str],
    stdout: Option<Stdio>,
) -> Result<std::process::Child> {
    let mut command_builder = Command::new(command);
    command_builder.args(args);
    if let Some(out) = stdout {
        command_builder.stdout(out);
    }
    let child = command_builder.spawn()?;
    Ok(child)
}

/// SHA-256 hex digest of `data`, computed with the deployed busybox.
///
/// No crypto crate is added on purpose (dependency-free build, contract v1.1
/// §8.3 only needs an integrity marker). Returns `None` when busybox is not
/// available yet (pre-deploy), in which case callers emit JSON `null`.
pub fn sha256_hex_stdin(data: &[u8]) -> Option<String> {
    let busybox = Path::new(crate::defs::BINARY_DIR).join("busybox");
    if !busybox.exists() {
        return None;
    }
    let mut child = Command::new(busybox)
        .arg("sha256sum")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(data).ok()?;
    }
    let output = child.wait_with_output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .next()
        .map(|digest| digest.to_string())
}

/// Wait for `child` with a deadline. Returns `Ok(Some(status))` when it exited
/// in time, `Ok(None)` after killing the child's **process group** on timeout.
///
/// Module scripts are spawned with `process_group(0)` (module.rs), so killing
/// `-pid` also kills their subshells — otherwise a hung `post-fs-data.sh` would
/// keep the whole boot blocked and defeat the deadline (NEW-DOC-05 §5.2/§5.5).
pub fn wait_timeout(child: &mut Child, timeout: Duration) -> Result<Option<ExitStatus>> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            std::result::Result::Ok(Some(status)) => return Ok(Some(status)),
            std::result::Result::Ok(None) => {}
            Err(e) => return Err(e.into()),
        }
        if std::time::Instant::now() >= deadline {
            let pid = child.id();
            // Negative pid targets the process group when the caller made the
            // child a group leader (`process_group(0)`); the plain pid covers
            // callers that did not. Both are best-effort.
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
                libc::kill(pid as i32, libc::SIGKILL);
            }
            // Reap; ignore errors (already-exited race is harmless).
            let _ = child.wait();
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(50));
    }
}
pub fn is_safe_mode(superkey: Option<String>) -> bool {
    // Contract v1.0 §3.5: three paths, any hit means safe mode.
    // Path 1+2: system properties (upstream behaviour, unchanged).
    let safemode = getprop("persist.sys.safemode")
        .filter(|prop| prop == "1")
        .is_some()
        || getprop("ro.sys.safemode")
            .filter(|prop| prop == "1")
            .is_some();
    info!("safemode: {}", safemode);
    if safemode {
        return true;
    }
    // Path 3: manual file flag (/data/adb/sproot/safemode, contract §3.5
    // ruling note). Replaces the former kernel query (0x1112); gives users a
    // manual trigger without touching system props. `superkey` is kept in the
    // signature for call-site compatibility; the kernel path it used to gate
    // no longer exists (silent removal, contract §3.5 "kernel 不参与").
    let _ = superkey;
    let file_flag = Path::new(defs::SPROOT_SAFEMODE_FILE).exists();
    info!("safemode_file: {}", file_flag);
    file_flag
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn switch_mnt_ns(pid: i32) -> Result<()> {
    use std::os::fd::AsRawFd;

    use anyhow::ensure;
    let path = format!("/proc/{pid}/ns/mnt");
    let fd = File::open(path)?;
    let current_dir = std::env::current_dir();
    let ret = unsafe { libc::setns(fd.as_raw_fd(), libc::CLONE_NEWNS) };
    if let Result::Ok(current_dir) = current_dir {
        let _ = std::env::set_current_dir(current_dir);
    }
    ensure!(ret == 0, "switch mnt ns failed");
    Ok(())
}

fn switch_cgroup(grp: &str, pid: u32) {
    let path = Path::new(grp).join("cgroup.procs");
    if !path.exists() {
        return;
    }

    let fp = OpenOptions::new().append(true).open(path);
    if let Result::Ok(mut fp) = fp {
        let _ = write!(fp, "{pid}");
    }
}

pub fn switch_cgroups() {
    let pid = std::process::id();
    switch_cgroup("/acct", pid);
    switch_cgroup("/dev/cg2_bpf", pid);
    switch_cgroup("/sys/fs/cgroup", pid);

    if getprop("ro.config.per_app_memcg")
        .filter(|prop| prop == "false")
        .is_none()
    {
        switch_cgroup("/dev/memcg/apps", pid);
    }
}

/// Detach the current process into a background daemon so it survives the
/// framework being torn down around it (e.g. `stop` during a soft reboot).
/// Redirects stdin/stdout/stderr to /dev/null and double-forks out of the
/// caller's process group / cgroup.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn daemonize() -> Result<()> {
    use std::os::fd::AsRawFd;

    let pid = unsafe { libc::fork() };
    if pid < 0 {
        bail!("fork error: {}", std::io::Error::last_os_error());
    }
    if pid > 0 {
        // Parent: wait for the child, then exit so the caller sees success.
        let mut status: i32 = 0;
        loop {
            if unsafe { libc::waitpid(pid, &mut status, 0) } < 0 {
                if std::io::Error::last_os_error().raw_os_error() != Some(libc::EINTR) {
                    std::process::exit(1);
                }
            } else {
                break;
            }
        }
        std::process::exit(0);
    }

    unsafe { libc::setsid() };
    switch_cgroups();

    if let Result::Ok(null) = File::open("/dev/null") {
        let fd = null.as_raw_fd();
        unsafe {
            libc::dup2(fd, 0);
            libc::dup2(fd, 1);
            libc::dup2(fd, 2);
        }
    }

    let pid = unsafe { libc::fork() };
    if pid < 0 {
        bail!("fork error: {}", std::io::Error::last_os_error());
    }
    if pid > 0 {
        unsafe { libc::_exit(0) };
    }

    Ok(())
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub fn daemonize() -> Result<()> {
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn umask(mask: u32) {
    unsafe { libc::umask(mask) };
}

#[cfg(not(any(target_os = "linux", target_os = "android")))]
pub fn umask(_mask: u32) {
    unimplemented!("umask is not supported on this platform")
}

pub fn has_magisk() -> bool {
    which::which("magisk").is_ok()
}
pub fn get_tmp_path() -> &'static str {
    if metadata(defs::TEMP_DIR_LEGACY).is_ok() {
        return defs::TEMP_DIR_LEGACY;
    }
    if metadata(defs::TEMP_DIR).is_ok() {
        return defs::TEMP_DIR;
    }
    ""
}
