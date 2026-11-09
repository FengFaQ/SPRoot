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

/// Pure-Rust SHA-256 (lowercase hex).
///
/// Needed by the authorisation audit log, which hashes the *sanitised command
/// digest* on the daemon hot path: spawning busybox per request (as
/// [`sha256_hex_stdin`] does) would be both slow and unavailable during the
/// first-boot window. Deliberately dependency-free (no crypto crate is added —
/// the licence-clean `--no-default-features` build must stay identical).
pub fn sha256_hex(data: &[u8]) -> String {
    const K: [u32; 64] = [
        0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4,
        0xab1c5ed5, 0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe,
        0x9bdc06a7, 0xc19bf174, 0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f,
        0x4a7484aa, 0x5cb0a9dc, 0x76f988da, 0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7,
        0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967, 0x27b70a85, 0x2e1b2138, 0x4d2c6dfc,
        0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85, 0xa2bfe8a1, 0xa81a664b,
        0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070, 0x19a4c116,
        0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
        0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7,
        0xc67178f2,
    ];
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a, 0x510e527f, 0x9b05688c, 0x1f83d9ab,
        0x5be0cd19,
    ];
    let mut msg = data.to_vec();
    let bit_len = (data.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in msg.chunks(64) {
        let mut w = [0u32; 64];
        for (i, word) in chunk.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }
        let (mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh) =
            (h[0], h[1], h[2], h[3], h[4], h[5], h[6], h[7]);
        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let t1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let t2 = s0.wrapping_add(maj);
            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(t1);
            d = c;
            c = b;
            b = a;
            a = t1.wrapping_add(t2);
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }
    h.iter().map(|word| format!("{word:08x}")).collect()
}

/// Wait for `child` with a deadline. Returns `Ok(Some(status))` when it exited
/// in time, `Ok(None)` after killing the child's **process group** on timeout.
////// Module scripts are spawned with `process_group(0)` (module.rs), so killing
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
pub fn is_safe_mode(superkey: Option<&str>) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Known-answer vectors (FIPS 180-4) for the built-in digest used by the
    /// authorisation audit log.
    #[test]
    fn sha256_matches_known_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(
            sha256_hex(b"The quick brown fox jumps over the lazy dog"),
            "d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592"
        );
        // Multi-block input (crosses the 64-byte padding boundary twice).
        assert_eq!(sha256_hex("a".repeat(1000).as_bytes()).len(), 64);
        assert_ne!(sha256_hex(b"abc"), sha256_hex(b"abd"));
    }
}
