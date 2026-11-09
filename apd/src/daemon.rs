//! Resident daemon for SPRoot (contract v1.1 §3.2 socket, §7, NEW-DOC-05 §5.3).
//!
//! Started by the injected `init.sproot.rc` (`service sproot-apd
//! /data/adb/apd --superkey-file /data/adb/sproot/key daemon`, property
//! trigger `sys.boot_completed=1`). Responsibilities:
//!
//! 1. **Stage ②** — dispatch the `services` stage right after start (non
//!    blocking module scripts, same semantics as APatch);
//! 2. **uid-listener** — inotify on `/data/system/packages.list.tmp`, refresh
//!    the userspace allow list (single writer, `package::ConfigLock`);
//! 3. **authorisation socket** — abstract unix socket `@sproot-<12hex>`;
//!    `SO_PEERCRED` gives the kernel-filled requester uid (v1.1 E3: this
//!    replaces the unimplementable `SP_REQUEST_UID` env injection);
//! 4. **Stage ③** — poll `sys.boot_completed` and dispatch the boot-completed
//!    stage (userspace event engine: the SK kernel has no event source), then
//!    clear the boot-loop counter;
//! 5. stay alive (init respawns us on crash; keep the daemon itself minimal so a
//!    buggy module script can never take it down — scripts are children/grand-
//!    children of forked processes, not of the daemon loop).
//!
//! The process is started as root by init and additionally escalates through
//! the SK key so the `avc_denied` trampoline treats it as a "key process"
//! (SELinux zero-touch design, 时序稿 §3.5).

use std::{
    io::{self, Read, Write},
    os::fd::{AsRawFd, FromRawFd, RawFd},
    os::unix::net::UnixStream,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::Duration,
};

use anyhow::{Context, Result};
use log::{error, info, warn};

use crate::{auth_list, defs, event, secret::SecretKey, supercall, utils};

/// How long the daemon waits for `sys.boot_completed=1` before giving up on the
/// boot-completed stage (it stays alive and retries on the next poll cycle).
const BOOT_WAIT_SECS: u64 = 180;
const PROP_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Set once the boot-completed stage has been dispatched.
static BOOT_DONE: AtomicBool = AtomicBool::new(false);

/// `apd daemon` entry point.
pub fn run(superkey: Option<SecretKey>) -> Result<()> {
    if !supercall::is_root() {
        // Silent: the rc may have started us before the manager deployed
        // (first-boot window, 时序稿 §3.4). Do not spam logs, do not crash.
        info!("[daemon] not root yet (key missing or rejected), exiting silently");
        return Ok(());
    }
    // Held for the whole process lifetime: closing the fd would release the
    // flock and let a second daemon start. The `_` name documents that the
    // guard's value (not its contents) is what matters.
    let _instance_lock = match acquire_instance_lock() {
        Some(lock) => lock,
        None => {
            info!("[daemon] another instance is already running, exiting");
            return Ok(());
        }
    };

    info!("[daemon] starting (pid={})", std::process::id());

    // (0) Persist the tri-state authorisation store / audit log and finish a
    //     pending legacy-csv migration before any request can arrive
    //     (NEW-DOC-05 §13: upgrade must not lose grants).
    if let Err(e) = auth_list::prepare_storage() {
        warn!("[daemon] auth store prepare failed: {e:#}");
    }

    // (1) Stage ② — non-blocking module service scripts.
    if let Err(e) = event::on_services(superkey.as_ref().map(SecretKey::as_str)) {
        warn!("[daemon] services stage failed: {e:#}");
    }

    // (2) uid-listener thread: keeps the allow list in sync with the system
    //     package list. It is a thread (not a forked child as in the interim
    //     SK port) so only one writer can ever exist in this process, and the
    //     single-instance lock keeps a second daemon out.
    thread::spawn(move || {
        if let Err(e) = event::start_uid_listener() {
            error!("[daemon] uid-listener stopped: {e:#}");
        }
    });

    // (3) Authorisation socket for third-party root requests.
    thread::spawn(move || {
        if let Err(e) = run_auth_socket() {
            error!("[daemon] auth socket stopped: {e:#}");
        }
    });

    // (4) Stage ③ — userspace boot-completed detection. The key is cloned into
    // a *new* locked copy for this thread; both copies zeroise on drop.
    let boot_key = superkey.clone();
    thread::spawn(move || {
        let mut waited = 0u64;
        while waited < BOOT_WAIT_SECS {
            if utils::getprop(defs::BOOT_COMPLETED_PROP).as_deref() == Some("1") {
                break;
            }
            thread::sleep(PROP_POLL_INTERVAL);
            waited += 1;
        }
        if waited >= BOOT_WAIT_SECS {
            warn!("[daemon] boot-completed not observed within {BOOT_WAIT_SECS}s");
            return;
        }
        if let Err(e) = event::on_boot_completed(boot_key.as_ref().map(SecretKey::as_str)) {
            warn!("[daemon] boot-completed stage failed: {e:#}");
        }
        BOOT_DONE.store(true, Ordering::SeqCst);
    });

    // (5) Stay alive. Init restarts us if we ever return/crash.
    info!("[daemon] up; uid-listener + auth socket + boot watcher started");
    loop {
        thread::sleep(Duration::from_secs(3600));
    }
}

/// Exclusive non-blocking lock on `daemon.lock`. The returned file must live as
/// long as the daemon (fd close releases the flock).
fn acquire_instance_lock() -> Option<std::fs::File> {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(defs::DAEMON_LOCK_FILE)
        .ok()?;
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return None;
    }
    // Record the pid for the manager (`version --json`).
    let _ = std::fs::write(defs::DAEMON_LOCK_FILE, format!("{}\n", std::process::id()));
    Some(file)
}

// ---------------------------------------------------------------------------
// Authorisation socket
// ---------------------------------------------------------------------------

/// Abstract socket name, persisted for the client library (no key material).
fn socket_name() -> String {
    match ensure_socket_name() {
        Ok(name) => name,
        Err(e) => {
            warn!("[daemon] socket name unavailable ({e:#}), using fallback");
            "sproot-default".to_string()
        }
    }
}

/// Read the persisted socket name or create a fresh random one (idempotent).
pub fn ensure_socket_name() -> Result<String> {
    if let Ok(name) = std::fs::read_to_string(defs::AUTH_SOCK_FILE) {
        let name = name.trim();
        if !name.is_empty() {
            return Ok(name.to_string());
        }
    }
    let name = generate_socket_name();
    write_socket_name(&name)?;
    Ok(name)
}

fn run_auth_socket() -> Result<()> {
    let name = socket_name();
    let listener = bind_abstract(&name)
        .with_context(|| format!("bind abstract socket @{name} (daemon must be root)"))?;
    info!("[daemon] auth socket listening on @{name}");

    loop {
        let fd = unsafe { libc::accept(listener, std::ptr::null_mut(), std::ptr::null_mut()) };
        if fd < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            error!("[daemon] accept failed: {err}");
            thread::sleep(Duration::from_secs(1));
            continue;
        }
        thread::spawn(move || {
            if let Err(e) = handle_client(fd) {
                warn!("[daemon] client session error: {e:#}");
            }
        });
    }
}

/// `socket(AF_UNIX, SOCK_STREAM)` + `bind()` to an abstract name (`@name`).
fn bind_abstract(name: &str) -> io::Result<RawFd> {
    unsafe {
        let fd = libc::socket(libc::AF_UNIX, libc::SOCK_STREAM, 0);
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut addr: libc::sockaddr_un = std::mem::zeroed();
        addr.sun_family = libc::AF_UNIX as libc::sa_family_t;
        let name_bytes = name.as_bytes();
        let max = addr.sun_path.len() - 2; // leading NUL + trailing NUL
        if name_bytes.len() > max {
            libc::close(fd);
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "abstract socket name too long",
            ));
        }
        // sun_path[0] = NUL marks the abstract namespace.
        for (i, byte) in name_bytes.iter().enumerate() {
            addr.sun_path[i + 1] = *byte as libc::c_char;
        }
        let len = (std::mem::size_of::<libc::sa_family_t>() + 1 + name_bytes.len()) as libc::socklen_t;
        if libc::bind(fd, &addr as *const _ as *const libc::sockaddr, len) != 0 {
            let err = io::Error::last_os_error();
            libc::close(fd);
            return Err(err);
        }
        if libc::listen(fd, 16) != 0 {
            let err = io::Error::last_os_error();
            libc::close(fd);
            return Err(err);
        }
        Ok(fd)
    }
}

/// Read-only socket probe for `selftest`: bind then immediately close.
/// `EADDRINUSE` means a daemon is already listening on that abstract name.
pub(crate) fn probe_abstract_bind(name: &str) -> io::Result<()> {
    let fd = bind_abstract(name)?;
    unsafe { libc::close(fd) };
    Ok(())
}

fn peer_uid(fd: RawFd) -> io::Result<u32> {
    unsafe {
        let mut cred: libc::ucred = std::mem::zeroed();
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let rc = libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            &mut cred as *mut _ as *mut libc::c_void,
            &mut len,
        );
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(cred.uid)
    }
}

fn handle_client(fd: RawFd) -> Result<()> {
    // SAFETY: fd is owned by this function (accept() transferred ownership).
    let mut stream = unsafe { UnixStream::from_raw_fd(fd) };

    let uid = peer_uid(stream.as_raw_fd()).context("read peer credentials")?;
    let request = read_request(&mut stream)?;
    let cmd = request.trim();
    if cmd.is_empty() {
        return respond(&mut stream, false, "", "empty request", 2, "deny");
    }

    // Authorisation point (contract v1.1 §3.2(a), NEW-DOC-05 §13): SO_PEERCRED
    // uid + the tri-state table. `ask` is explicitly refused (`ask_pending`)
    // until the interactive prompt exists — it must never fail open.
    let decision = auth_list::authorize(Some(uid as i32));
    // One audit event + request_count / last_*_at update per real request.
    auth_list::record_request(uid as i32, decision, Some(cmd));
    match decision {
        auth_list::Decision::Allow => {}
        auth_list::Decision::AskPending => {
            warn!(
                "[daemon] denied uid {uid}: state=ask (interactive prompt not implemented yet -> ask_pending)"
            );
            return respond(&mut stream, false, "", "ask_pending", 1, "ask_pending");
        }
        auth_list::Decision::Deny => {
            warn!("[daemon] denied uid {uid} (state=deny or table unreadable)");
            return respond(&mut stream, false, "", "", 1, "deny");
        }
    }

    let su_path = format!("{}su", defs::BINARY_DIR);
    let su = Path::new(&su_path);
    if !su.exists() {
        return respond(&mut stream, false, "", "su not deployed", 126, "allow");
    }

    let (stdout, stderr, code) = match run_su(su, cmd, uid) {
        Ok(tuple) => tuple,
        Err(e) => (String::new(), format!("{e:#}"), 1),
    };
    respond(&mut stream, true, &stdout, &stderr, code, "allow")
}

fn read_request(stream: &mut UnixStream) -> Result<String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .ok();
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > 64 * 1024 || buf.contains(&b'\n') {
                    break;
                }
            }
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e.into()),
        }
    }
    let text = String::from_utf8_lossy(&buf).to_string();
    // Accept both `{"cmd":"..."}` and a plain command line.
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(text.trim()) {
        if let Some(cmd) = value.get("cmd").and_then(|v| v.as_str()) {
            return Ok(cmd.to_string());
        }
    }
    Ok(text)
}

fn run_su(su: &Path, cmd: &str, uid: u32) -> Result<(String, String, i32)> {
    use std::os::unix::process::CommandExt;
    let mut child = std::process::Command::new(su)
        .arg("-c")
        .arg(cmd)
        // Only the daemon (root) writes this; the session child cannot spoof it
        // because the daemon forks it directly (contract v1.1 §3.2(a)).
        .env(supercall::REQUEST_UID_ENV, uid.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        // Session leader: `utils::wait_timeout` kills the whole group on timeout.
        .process_group(0)
        .spawn()
        .with_context(|| format!("spawn {}", su.display()))?;

    let timeout = Duration::from_secs(crate::defs::SCRIPT_TIMEOUT_SECS);
    let status = utils::wait_timeout(&mut child, timeout)?;
    let code = match status {
        Some(status) => status.code().unwrap_or(1),
        None => {
            warn!("[daemon] su session timed out after {}s, killed", timeout.as_secs());
            124
        }
    };
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut out) = child.stdout.take() {
        let _ = out.read_to_string(&mut stdout);
    }
    if let Some(mut err) = child.stderr.take() {
        let _ = err.read_to_string(&mut stderr);
    }
    Ok((stdout, stderr, code))
}

fn respond(
    stream: &mut UnixStream,
    ok: bool,
    stdout: &str,
    stderr: &str,
    code: i32,
    decision: &str,
) -> Result<()> {
    let payload = serde_json::json!({
        "ok": ok,
        "stdout": stdout,
        "stderr": stderr,
        "exit": code,
        // NEW-DOC-05 §13: allow | deny | ask_pending (additive field; older
        // clients simply ignore it).
        "decision": decision,
    });
    stream
        .write_all(format!("{payload}\n").as_bytes())
        .context("write response")?;
    stream.flush().ok();
    Ok(())
}

/// Generate a random socket name (`sproot-<12 hex>`), used by `deploy`.
pub fn generate_socket_name() -> String {
    const HEX: &[u8] = b"0123456789abcdef";
    let mut bytes = [0u8; 12];
    if let Ok(mut file) = std::fs::File::open("/dev/urandom") {
        let _ = file.read_exact(&mut bytes);
    } else {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or(0);
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = (now as usize + i) as u8;
        }
    }
    let suffix: String = bytes.iter().map(|b| HEX[(*b % 16) as usize] as char).collect();
    format!("sproot-{suffix}")
}

/// Write the socket name file during deploy (0644 so the client library of a
/// third-party app can read it; the name carries no key material).
pub fn write_socket_name(name: &str) -> Result<()> {
    std::fs::create_dir_all(defs::BINARY_DIR)?;
    let path = Path::new(defs::AUTH_SOCK_FILE);
    std::fs::write(path, name)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644))?;
    Ok(())
}

/// True once the boot-completed stage has been dispatched in this process.
pub fn boot_stage_done() -> bool {
    BOOT_DONE.load(Ordering::SeqCst)
}

/// Pid recorded by the running daemon (from the instance lock file).
pub fn daemon_pid() -> Option<u32> {
    std::fs::read_to_string(defs::DAEMON_LOCK_FILE)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
}
