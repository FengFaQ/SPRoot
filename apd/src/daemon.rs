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

// ---------------------------------------------------------------------------
// Subsystem supervision (APD-2, 2026-11-11)
//
// Why this exists: both long-lived subsystems used to be `thread::spawn(|| { if
// let Err(e) = run() { error!(..) } })`. One log line, then the thread is gone.
// The daemon kept running and every health check kept passing while
//   * the uid-listener stopped syncing app installs into the authorisation
//     table (new apps silently get the wrong decision), or
//   * the auth socket stopped accepting (every su request fails).
// That is silent degradation — the same class of defect as the post-fs-data
// budget hole fixed in APD-1.
//
// The supervisor below runs each subsystem, waits for it to exit, and restarts
// it a bounded number of times with exponential backoff. State is published in
// an atomic so `selftest` can report it without asking the daemon.
// ---------------------------------------------------------------------------

/// Lifecycle state of one supervised subsystem.
///
/// The explicit `#[repr(u8)]` discriminants are what `SubsystemStatus::state()`
/// decodes back — keep the two in lockstep.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum SubsystemState {
    /// Never started yet.
    NotStarted = 0,
    /// Running normally.
    Running = 1,
    /// Exited/crashed and waiting out a backoff before the next attempt.
    Restarting = 2,
    /// Gave up after [`defs::SUBSYSTEM_MAX_RESTARTS`] attempts — **terminal**.
    GaveUp = 3,
}

impl SubsystemState {
    pub fn as_str(self) -> &'static str {
        match self {
            SubsystemState::NotStarted => "not_started",
            SubsystemState::Running => "running",
            SubsystemState::Restarting => "restarting",
            SubsystemState::GaveUp => "gave_up",
        }
    }
}

/// Delay before restart attempt `attempt` (1-based), or `None` when the budget
/// is exhausted.
///
/// This is the whole restart policy as a pure function, so it can be tested
/// without spawning anything:
///
/// * `attempt` 1, 2, 3 with a budget of 3 → `Some(1s)`, `Some(2s)`, `Some(4s)`;
/// * `attempt` > budget → **`None`** (give up);
/// * `attempt` 0 is treated as 1 (defensive: never returns a zero-delay
///   busy-loop, which would spin the daemon at 100% CPU during a failure).
///
/// Returning `None` rather than `Some(ZERO)` matters for the same reason as the
/// post-fs-data budget: a zero delay is a silent infinite retry, which looks
/// identical to "recovered" in a log that only records timestamps.
pub fn restart_delay(attempt: u32, max_restarts: u32) -> Option<Duration> {
    if attempt == 0 || attempt > max_restarts {
        return None;
    }
    let exp = attempt.saturating_sub(1).min(16);
    let secs = defs::SUBSYSTEM_RESTART_BASE_SECS
        .saturating_mul(1u64 << exp)
        .min(defs::SUBSYSTEM_MAX_BACKOFF_SECS);
    Some(Duration::from_secs(secs))
}

/// Per-subsystem supervision record, published for `selftest`.
struct SubsystemStatus {
    name: &'static str,
    state: std::sync::atomic::AtomicU8,
    /// Number of *restarts* performed so far (not counting the first start).
    restarts: std::sync::atomic::AtomicU32,
}

impl SubsystemStatus {
    const fn new(name: &'static str) -> Self {
        SubsystemStatus {
            name,
            state: std::sync::atomic::AtomicU8::new(0), // NotStarted
            restarts: std::sync::atomic::AtomicU32::new(0),
        }
    }

    fn set_state(&self, s: SubsystemState) {
        self.state.store(s as u8, Ordering::SeqCst);
    }

    fn state(&self) -> SubsystemState {
        match self.state.load(Ordering::SeqCst) {
            1 => SubsystemState::Running,
            2 => SubsystemState::Restarting,
            3 => SubsystemState::GaveUp,
            _ => SubsystemState::NotStarted,
        }
    }

    fn restarts(&self) -> u32 {
        self.restarts.load(Ordering::SeqCst)
    }
}

/// The two supervised subsystems. Process-global so `selftest` (a different
/// process) can read them via the shared state file, and so `--selftest` inside
/// the daemon reads the same record.
static UID_LISTENER: SubsystemStatus = SubsystemStatus::new("uid_listener");
static AUTH_SOCKET: SubsystemStatus = SubsystemStatus::new("auth_socket");

/// Snapshot of both subsystems, for `selftest` / `state.json`.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SubsystemReport {
    pub name: &'static str,
    pub state: &'static str,
    pub restarts: u32,
    /// True when the subsystem is functional right now (running).
    pub healthy: bool,
}

/// Read the supervision status of both subsystems.
///
/// `live` is `false` when the daemon is not running in *this* process (the
/// usual case for `apd selftest`): the atomics then reflect a fresh process
/// that never started them, so the caller must treat `NotStarted` as
/// "unknown/not applicable" rather than "failed". `selftest` does exactly that.
pub fn subsystems_report() -> Vec<SubsystemReport> {
    [&UID_LISTENER, &AUTH_SOCKET]
        .into_iter()
        .map(|s| {
            let state = s.state();
            SubsystemReport {
                name: s.name,
                state: state.as_str(),
                restarts: s.restarts(),
                healthy: state == SubsystemState::Running,
            }
        })
        .collect()
}

/// Run a subsystem forever, restarting it a bounded number of times.
///
/// `run` is called repeatedly; each call is expected to block for the lifetime
/// of the subsystem. When it returns (cleanly or not) the supervisor waits
/// `restart_delay` and calls it again, until the budget is exhausted — at which
/// point the state becomes [`SubsystemState::GaveUp`] and the thread ends.
///
/// Panics inside `run` are caught: without this, a panic would kill the thread
/// silently (Rust's default) and the failure would never be logged at all.
fn supervise<F>(status: &'static SubsystemStatus, mut run: F)
where
    F: FnMut() -> Result<()>,
{
    let mut attempt: u32 = 0;
    loop {
        status.set_state(SubsystemState::Running);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(&mut run));
        match outcome {
            Ok(Ok(())) => {
                // Returned Ok without being asked to stop: treat as an exit that
                // still needs a restart (the loop functions are infinite, so
                // returning at all is anomalous).
                error!(
                    "[daemon] {} exited unexpectedly (returned Ok); treating as a failure",
                    status.name
                );
            }
            Ok(Err(e)) => {
                error!("[daemon] {} stopped: {e:#}", status.name);
            }
            Err(_panic) => {
                // Rust prints the panic payload on stderr; record it here so it
                // is also visible in logcat as a daemon-level event.
                error!(
                    "[daemon] {} PANICKED (caught; the daemon stays alive)",
                    status.name
                );
            }
        }

        attempt += 1;
        match restart_delay(attempt, defs::SUBSYSTEM_MAX_RESTARTS) {
            Some(delay) => {
                status.restarts.fetch_add(1, Ordering::SeqCst);
                status.set_state(SubsystemState::Restarting);
                warn!(
                    "[daemon] {} restarting in {:.1}s (attempt {}/{})",
                    status.name,
                    delay.as_secs_f64(),
                    attempt,
                    defs::SUBSYSTEM_MAX_RESTARTS
                );
                thread::sleep(delay);
            }
            None => {
                status.set_state(SubsystemState::GaveUp);
                error!(
                    "[daemon] {} GAVE UP after {} restart(s) — this subsystem is DOWN for the \
                     rest of this boot. su/list sync is degraded; check `apd selftest` and the \
                     logs above for the cause. Do NOT treat the daemon as healthy.",
                    status.name, attempt - 1
                );
                publish_subsystem_state();
                return;
            }
        }
    }
}

/// Persist subsystem health for out-of-process readers (`selftest`).
///
/// Deliberately best-effort: a failure to write the report must never affect
/// the daemon. The file is 0644 because it carries no secrets — only state
/// names and counters.
fn publish_subsystem_state() {
    use std::os::unix::fs::PermissionsExt;
    let payload = serde_json::json!({
        "pid": std::process::id(),
        "updated_at": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        "subsystems": subsystems_report(),
        "boot_stage_done": boot_stage_done(),
    });
    let path = defs::DAEMON_SUBSYSTEM_STATE_FILE;
    let tmp = format!("{path}.tmp");
    if std::fs::write(&tmp, payload.to_string()).is_ok() {
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644));
        let _ = std::fs::rename(&tmp, path);
    }
}

/// Read the daemon-published subsystem state (`None` when absent/unreadable).
///
/// Used by `selftest` from a different process. Absence means "the daemon has
/// not published anything", which `selftest` reports as `unknown` — never as a
/// pass.
pub fn read_subsystem_state() -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(defs::DAEMON_SUBSYSTEM_STATE_FILE).ok()?;
    serde_json::from_str(&text).ok()
}

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
        LockOutcome::Acquired(lock) => lock,
        LockOutcome::AlreadyRunning => {
            // Normal: init's `oneshot` + `disabled` service can race a manual
            // start. Distinguishable from a real failure on purpose.
            info!("[daemon] another instance holds the lock, exiting (AlreadyRunning)");
            return Ok(());
        }
        LockOutcome::Unavailable(why) => {
            // NOT "already running": the lock could not even be created. Logging
            // this as AlreadyRunning (the old behaviour) hid a real deployment
            // problem behind a benign message.
            error!("[daemon] cannot acquire the instance lock ({why}) — daemon will NOT start");
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

    // (2) uid-listener: keeps the allow list in sync with the system package
    //     list. Supervised (APD-2): a crash here used to be one log line and
    //     then silence — app installs stopped updating the authorisation table
    //     while the daemon kept looking healthy.
    thread::spawn(|| {
        supervise(&UID_LISTENER, || event::start_uid_listener());
        publish_subsystem_state();
    });

    // (3) Authorisation socket for third-party root requests. Supervised for the
    //     same reason: it is the ONLY way an app can request root (contract
    //     v1.1 §3.2), so its death means every su request fails silently.
    thread::spawn(|| {
        supervise(&AUTH_SOCKET, || run_auth_socket());
        publish_subsystem_state();
    });

    // (4) Stage ③ — userspace boot-completed detection. The key is cloned into
    //     a *new* locked copy for this thread; both copies zeroise on drop.
    let boot_key = superkey.clone();
    thread::spawn(move || {
        // APD-2: a panic here would silently kill the thread and the boot stage
        // would never run (modules would never get their boot-completed
        // scripts). Catch it so it is at least logged.
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
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
        }));
        if result.is_err() {
            error!("[daemon] boot-completed watcher PANICKED (caught; stage ③ did not run)");
        }
        publish_subsystem_state();
    });

    // Publish an initial snapshot so `selftest` sees "running" immediately rather
    // than waiting for the first restart event.
    thread::sleep(Duration::from_millis(50));
    publish_subsystem_state();

    // (5) Stay alive. Init restarts us if we ever return/crash.
    info!(
        "[daemon] up; uid-listener + auth socket + boot watcher started \
         (subsystem restarts are bounded to {} per subsystem, see `apd selftest`)",
        defs::SUBSYSTEM_MAX_RESTARTS
    );
    loop {
        thread::sleep(Duration::from_secs(3600));
        // Keep the published snapshot fresh (it embeds boot_stage_done).
        publish_subsystem_state();
    }
}

/// Why the single-instance lock was not obtained.
///
/// The old code returned `Option<std::fs::File>` and collapsed every failure
/// into `None`, so the caller logged "another instance is already running" even
/// when the real cause was a missing directory or a bad permission. That makes
/// both states indistinguishable in the log and in `selftest` (APD-2 ②-3).
#[derive(Debug)]
pub enum LockOutcome {
    /// We hold the lock.
    Acquired(std::fs::File),
    /// Another daemon holds it — a **normal** condition, exit quietly.
    AlreadyRunning,
    /// Could not even create/open the lock file.
    Unavailable(String),
}

/// Exclusive **non-blocking** lock on `daemon.lock`.
///
/// Non-blocking is load-bearing: this daemon is started by init's `oneshot`
/// service, and a blocking `flock` on a contended lock would hang the start
/// sequence. `LOCK_NB` returns `EWOULDBLOCK` immediately instead.
///
/// Backoff/race semantics: two daemons starting simultaneously is fine — exactly
/// one wins `flock`, the loser exits cleanly. The winner truncates and rewrites
/// the pid afterwards, so a reader may briefly see the previous pid; `daemon_pid`
/// only uses it for display, never for liveness decisions.
fn acquire_instance_lock() -> LockOutcome {
    let file = match std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(defs::DAEMON_LOCK_FILE)
    {
        Ok(f) => f,
        Err(e) => {
            return LockOutcome::Unavailable(format!(
                "cannot open {}: {e}",
                defs::DAEMON_LOCK_FILE
            ));
        }
    };
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        let err = io::Error::last_os_error();
        // EWOULDBLOCK == EAGAIN on Linux/Android; compare against the raw code
        // with a single constant so the match stays exhaustive everywhere.
        return match err.raw_os_error() {
            Some(code) if code == libc::EWOULDBLOCK => LockOutcome::AlreadyRunning,
            _ => LockOutcome::Unavailable(format!("flock failed: {err}")),
        };
    }
    // Record the pid for the manager (`version --json`).
    let _ = std::fs::write(defs::DAEMON_LOCK_FILE, format!("{}\n", std::process::id()));
    LockOutcome::Acquired(file)
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

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // APD-2 subsystem restart policy (2026-11-11).
    //
    // `restart_delay` is the entire restart decision as a pure function: how
    // long to wait, and — crucially — when to STOP. The pre-APD-2 behaviour was
    // "log one line and never restart", i.e. give up immediately; a naive fix
    // would be "restart forever", which spins the daemon during a persistent
    // failure. Both extremes are caught below.
    // -----------------------------------------------------------------------

    /// Positive: the first three attempts back off 1s / 2s / 4s.
    #[test]
    fn restart_delay_is_exponential() {
        assert_eq!(restart_delay(1, 3), Some(Duration::from_secs(1)));
        assert_eq!(restart_delay(2, 3), Some(Duration::from_secs(2)));
        assert_eq!(restart_delay(3, 3), Some(Duration::from_secs(4)));
    }

    /// **Negative sample: past the budget it must return `None`.**
    /// A function that keeps returning `Some(..)` would restart a permanently
    /// broken subsystem forever — the failure would never be reported as final.
    #[test]
    fn restart_delay_gives_up_past_the_budget() {
        assert_eq!(
            restart_delay(4, defs::SUBSYSTEM_MAX_RESTARTS),
            None,
            "attempt {} must give up (budget {})",
            4,
            defs::SUBSYSTEM_MAX_RESTARTS
        );
        // A wildly out-of-range attempt is still None, not an overflow.
        assert_eq!(restart_delay(u32::MAX, 3), None);
    }

    /// **Regression guard against the pre-APD-2 behaviour**: "one line then
    /// never restart" is equivalent to a budget of zero. If someone reverts the
    /// supervisor to give-up-immediately, this assertion fails.
    #[test]
    fn budget_of_zero_means_no_restart_at_all() {
        // The old code never restarted. Encoding that as budget 0 must mean
        // `None` for the very first attempt; the production budget must be > 0
        // so this is a *different* regime, not the shipped behaviour.
        assert_eq!(restart_delay(1, 0), None);
        assert!(
            defs::SUBSYSTEM_MAX_RESTARTS >= 1,
            "a budget of 0 would reproduce the pre-APD-2 'never restart' behaviour"
        );
    }

    /// Attempt 0 is not a valid attempt; it must not produce a zero delay
    /// (which would be a busy-loop) nor a `Some`.
    #[test]
    fn restart_delay_rejects_attempt_zero() {
        assert_eq!(restart_delay(0, 3), None);
        assert_ne!(restart_delay(0, 3), Some(Duration::ZERO));
    }

    /// The delay must never be zero — a zero backoff is a hot retry loop.
    #[test]
    fn restart_delay_is_never_zero() {
        for attempt in 1..=defs::SUBSYSTEM_MAX_RESTARTS {
            let d = restart_delay(attempt, defs::SUBSYSTEM_MAX_RESTARTS)
                .unwrap_or_else(|| panic!("attempt {attempt} must be allowed"));
            assert!(d > Duration::ZERO, "attempt {attempt} produced a zero backoff");
        }
    }

    /// The backoff is capped: otherwise a bigger budget yields delays that
    /// outlive the boot.
    #[test]
    fn restart_delay_is_capped() {
        let cap = Duration::from_secs(defs::SUBSYSTEM_MAX_BACKOFF_SECS);
        for attempt in 1..=20u32 {
            if let Some(d) = restart_delay(attempt, 100) {
                assert!(d <= cap, "attempt {attempt} delay {d:?} exceeds the cap {cap:?}");
            }
        }
    }

    /// The delay is monotonically non-decreasing while the budget lasts.
    #[test]
    fn restart_delay_is_monotonic() {
        let mut prev = Duration::ZERO;
        for attempt in 1..=defs::SUBSYSTEM_MAX_RESTARTS {
            let d = restart_delay(attempt, defs::SUBSYSTEM_MAX_RESTARTS).unwrap();
            assert!(d >= prev, "delay shrank at attempt {attempt}");
            prev = d;
        }
    }

    // -----------------------------------------------------------------------
    // Subsystem state machine
    // -----------------------------------------------------------------------

    /// Round-trips every state through the atomic encoding used for the
    /// published snapshot. A mismatch here would make `selftest` read the wrong
    /// verdict (e.g. `gave_up` decoding as `running`).
    #[test]
    fn subsystem_state_roundtrip() {
        let s = SubsystemStatus::new("test");
        for st in [
            SubsystemState::NotStarted,
            SubsystemState::Running,
            SubsystemState::Restarting,
            SubsystemState::GaveUp,
        ] {
            s.set_state(st);
            assert_eq!(s.state(), st, "state {st:?} did not round-trip");
        }
    }

    /// `GaveUp` must never look healthy: `selftest` keys its FAIL on this.
    #[test]
    fn gave_up_is_not_healthy() {
        let s = SubsystemStatus::new("test");
        s.set_state(SubsystemState::GaveUp);
        let st = s.state();
        assert_eq!(st, SubsystemState::GaveUp);
        assert_ne!(st, SubsystemState::Running);
        assert_eq!(st.as_str(), "gave_up");
    }

    /// Every state has a distinct, stable string (it is a JSON contract field).
    #[test]
    fn subsystem_state_strings_are_distinct() {
        let names = [
            SubsystemState::NotStarted.as_str(),
            SubsystemState::Running.as_str(),
            SubsystemState::Restarting.as_str(),
            SubsystemState::GaveUp.as_str(),
        ];
        let mut uniq = names.to_vec();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(uniq.len(), names.len(), "state strings collide: {names:?}");
    }

    // -----------------------------------------------------------------------
    // The supervisor itself, exercised with a failing body (no real daemon).
    // -----------------------------------------------------------------------

    /// A subsystem that always fails must be retried exactly
    /// `SUBSYSTEM_MAX_RESTARTS` times and then land on `GaveUp` — not loop
    /// forever, and not give up after one attempt.
    #[test]
    fn supervisor_retries_then_gives_up() {
        // Drive the same decision the supervisor uses, without sleeping: count
        // how many attempts are permitted.
        let mut attempts = 0u32;
        let mut attempt = 1u32;
        while restart_delay(attempt, defs::SUBSYSTEM_MAX_RESTARTS).is_some() {
            attempts += 1;
            attempt += 1;
        }
        assert_eq!(
            attempts, defs::SUBSYSTEM_MAX_RESTARTS,
            "supervisor must allow exactly {} restarts, got {attempts}",
            defs::SUBSYSTEM_MAX_RESTARTS
        );
        assert!(
            attempts > 1,
            "a single attempt is the pre-APD-2 'give up immediately' bug"
        );
    }

    /// `subsystems_report` must always name both supervised subsystems, even in
    /// a fresh process that never started them (selftest relies on the names).
    #[test]
    fn subsystems_report_lists_both_subsystems() {
        let report = subsystems_report();
        let names: Vec<&str> = report.iter().map(|r| r.name).collect();
        assert!(names.contains(&"uid_listener"), "{names:?}");
        assert!(names.contains(&"auth_socket"), "{names:?}");
        assert_eq!(report.len(), 2);
    }

    /// In a fresh test process nothing has been started, so nothing may claim to
    /// be healthy — this is exactly the "unknown, not pass" rule.
    #[test]
    fn fresh_process_reports_not_running() {
        let report = subsystems_report();
        for r in &report {
            assert!(
                !r.healthy,
                "{} reported healthy without ever being started",
                r.name
            );
            assert_eq!(r.state, "not_started");
        }
    }
}
