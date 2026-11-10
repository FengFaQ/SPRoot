//! `apd selftest [--json]` — capability / diagnostic self-check.
//!
//! Two call sites are supported (lead requirement, 2026-11-08):
//!
//! * **App first-boot self-check**: the manager `exec`s the APK-bundled
//!   `nativeLibraryDir/libapd.so selftest --superkey-file <tmp>` **before**
//!   anything is deployed. In that stage most deployment checks are *expected*
//!   to be absent, so they must not report `FAIL` — the check reports
//!   `stage=undeployed`, only self/escalation capabilities are critical, and
//!   the exit code stays 0 when the binary itself is healthy.
//! * **Deployed health check**: after `deploy`, the same command (or
//!   `apd daemon --selftest`) reports `stage=deployed` and every deployment
//!   invariant (key mode/mlock, allow list, hidden su, daemon, version match,
//!   AVB audit) becomes critical.
//!
//! The binary path is self-identified via `current_exe()`; `/data/adb/apd` is
//! never assumed to be the running executable.
//!
//! Exit code: 0 = all critical checks pass, 1 = at least one critical failure
//! (CLI contract: CALL v1.1 §8.5). JSON keys are stable for QA automation:
//! top-level `ok` / `stage` / `checks[]`, each check `name` / `ok` /
//! `critical` / `detail`.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use anyhow::Result;
use log::warn;
use serde::Serialize;

use crate::{auth_list, daemon, defs, deploy, secret, selinux};

/// Outcome of one check.
///
/// `status` was added by the 2026-11-10 S1 fix so "cannot be determined" is
/// renderable without either lying (`ok: true` — what the old `avb_untouched`
/// comparison effectively did) or crying wolf (`[FAIL]` for an unreadable block
/// device). `ok` is kept as the machine-readable criticality input and for
/// existing consumers; `status` is additive and never contradicts it:
///
/// | status  | ok    | meaning                                          |
/// |---------|-------|--------------------------------------------------|
/// | ok      | true  | asserted and measured                            |
/// | fail    | false | asserted and violated (a real regression)        |
/// | unknown | false | could not be determined — **not** a pass         |
/// | pending | true  | not applicable yet (`stage=undeployed`)          |
#[derive(Serialize)]
struct Check {
    name: &'static str,
    ok: bool,
    critical: bool,
    status: &'static str,
    detail: String,
}

impl Check {
    fn new(name: &'static str, ok: bool, critical: bool, detail: String) -> Self {
        Check {
            name,
            ok,
            critical,
            status: if ok { "ok" } else { "fail" },
            detail,
        }
    }

    /// A check that could not be determined. Never rendered as `ok`, and never
    /// counted as a critical failure (project convention: an unobservable
    /// environment must not be reported as a defect — but it must not be
    /// reported as a pass either).
    fn unknown(name: &'static str, detail: String) -> Self {
        Check {
            name,
            ok: false,
            critical: false,
            status: "unknown",
            detail,
        }
    }

    /// Placeholder for a deployment-dependent check while nothing is deployed.
    fn pending(name: &'static str, detail: &str) -> Self {
        Check {
            name,
            ok: true,
            critical: false,
            status: "pending",
            detail: format!("not deployed yet ({detail})"),
        }
    }
}

fn mode_of(path: &Path) -> Option<u32> {
    fs::metadata(path).ok().map(|m| m.permissions().mode() & 0o777)
}

/// `zygiskd` PID, or `None` when it is not running / not observable.
///
/// NeoZygisk and OnyxZygisk both name the helper process `zygiskd`; the check
/// must never turn a missing component into a false failure (this round is a
/// placeholder — see `defs::APATCH_PACKAGE_CONFIG` docs).
fn zygiskd_pid() -> Option<u32> {
    for entry in fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(raw) = fs::read(format!("/proc/{pid}/cmdline")) else {
            continue;
        };
        if raw.is_empty() {
            continue;
        }
        let cmdline = String::from_utf8_lossy(&raw);
        let argv0 = cmdline.split('\0').next().unwrap_or("");
        let base = argv0.rsplit('/').next().unwrap_or(argv0);
        if base.contains("zygiskd") {
            return Some(pid);
        }
    }
    None
}

/// `version=` from a Magisk/APatch `module.prop` body (first match).
fn parse_module_prop_version(text: &str) -> Option<String> {
    text.lines().find_map(|line| {
        line.trim()
            .strip_prefix("version=")
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
    })
}

/// Bundled Zygisk module version, if one is installed.
///
/// Phase 1: SPRoot does not ship/own a Zygisk module yet, so this normally
/// returns `None`; the selftest reports "not installed" instead of failing.
fn zygisk_module_prop() -> Option<(String, String)> {
    let mut candidates: Vec<PathBuf> = fs::read_dir(defs::MODULE_DIR)
        .ok()?
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .to_ascii_lowercase()
                .contains("zygisk")
        })
        .map(|entry| entry.path().join("module.prop"))
        .collect();
    candidates.sort();
    for prop in candidates {
        if let Ok(text) = fs::read_to_string(&prop)
            && let Some(version) = parse_module_prop_version(&text)
        {
            return Some((prop.display().to_string(), version));
        }
    }
    None
}

/// Verdict for the raw bytes behind `--superkey-file` (NEW-DOC-05 §12).
///
/// * `len == 47 && nul_free` — canonical AP-side ABI → pass, no warning;
/// * `len == 48 && nul_free` — **legacy** manager/APK format. The runtime uses
///   the first 47 bytes (`supercall::escalation_token`, logged), so an upgraded
///   install must **pass with a `legacy48` annotation**, not be reported as
///   `needs_repair` (Lead 2026-11-09). A re-deploy rewrites the file to 47;
/// * anything else (wrong length, embedded NUL, mode ≠ 0600) — fail; those are
///   either a manual edit or a truncated write, not a legacy format.
///
/// Returns `(ok, legacy48)` so the caller can emit the warning + detail.
fn superkey_abi_verdict(mode: u32, len: usize, nul_free: bool) -> (bool, bool) {
    let legacy48 = len == 48 && nul_free;
    (mode == 0o600 && nul_free && (len == 47 || legacy48), legacy48)
}

fn is_executable(path: &Path) -> bool {
    mode_of(path).map(|m| m & 0o111 != 0).unwrap_or(false)
}

/// True when `/proc/<pid>/cmdline` is exactly the resident daemon
/// (`/data/adb/apd daemon ...`). A stale instance-lock pid then cannot be
/// mistaken for a running service (pid reuse or a plain `apd` invocation).
fn is_daemon_process(pid: u32) -> bool {
    let Ok(raw) = fs::read(format!("/proc/{pid}/cmdline")) else {
        return false;
    };
    let cmdline = String::from_utf8_lossy(&raw);
    let args: Vec<&str> = cmdline.split('\0').collect();
    let Some(argv0) = args.first() else {
        return false;
    };
    let is_apd = *argv0 == "apd" || argv0.ends_with("/apd");
    // `apd daemon --selftest` is this very check, not the resident service.
    is_apd
        && args.iter().any(|arg| *arg == "daemon")
        && !args.iter().any(|arg| *arg == "--selftest")
}

/// What the `apd` entry at `path` actually is.
///
/// The three states are deliberately distinct: a *missing* entry and a *wrong*
/// entry need different remediation text, and neither may be conflated with the
/// passing case.
#[derive(Debug, PartialEq, Eq)]
enum SymlinkState {
    /// Exists and resolves to [`defs::DAEMON_PATH`].
    Correct,
    /// Nothing there.
    Missing,
    /// Present but resolves elsewhere.
    Wrong(String),
}

fn symlink_target_state(path: &str) -> SymlinkState {
    let p = Path::new(path);
    match fs::symlink_metadata(p) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => SymlinkState::Missing,
        Err(_) => SymlinkState::Missing,
        Ok(_) => match fs::canonicalize(p) {
            Ok(target) if target.display().to_string() == defs::DAEMON_PATH => {
                SymlinkState::Correct
            }
            Ok(target) => SymlinkState::Wrong(target.display().to_string()),
            // A dangling link: it exists but resolves nowhere, which is a wrong
            // entry rather than an absent one.
            Err(_) => SymlinkState::Wrong("<dangling>".to_string()),
        },
    }
}

/// Can we actually look at the directories that `apd` would be resolved from?
///
/// Same role as [`proc_scan_available`] for the daemon check: "not found" is only
/// a *defect* when we were allowed to look. When the directories are unreadable
/// (EACCES on a parent, or the mount is absent) the honest answer is `unknown`,
/// not `fail` — but it is never `ok` either.
fn path_probe_is_determinable() -> bool {
    [defs::BINARY_DIR, defs::ADB_DIR].iter().any(|dir| {
        let path = Path::new(dir);
        // A readable directory settles it; a *missing* directory (NotFound) also
        // settles it (the entry really is absent, which is the defect we report).
        fs::read_dir(path).is_ok() || !path.exists()
    })
}

/// PID of the running daemon: the instance-lock pid first, then a `/proc` scan.
/// `None` only means "no daemon observed"; the caller decides whether that is
/// determinable (see [`proc_scan_available`]).
fn running_daemon_pid() -> Option<u32> {
    if let Some(pid) = daemon::daemon_pid() {
        if is_daemon_process(pid) {
            return Some(pid);
        }
    }
    for entry in fs::read_dir("/proc").ok()?.flatten() {
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|name| name.parse::<u32>().ok())
        else {
            continue;
        };
        if is_daemon_process(pid) {
            return Some(pid);
        }
    }
    None
}

/// Can we enumerate `/proc` at all? If not, "daemon not found" is unknown, not
/// a failure — `rc_started` must never report a false negative.
fn proc_scan_available() -> bool {
    fs::read_dir("/proc").is_ok()
}

/// Where the currently running binary lives — reported, never assumed.
fn running_from() -> (PathBuf, &'static str) {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("<unknown>"));
    let text = exe.to_string_lossy();
    let origin = if text.starts_with(defs::ADB_DIR) {
        "filesystem(/data/adb)"
    } else if text.contains("/lib/") || text.contains("arm64-v8a") {
        "nativeLibraryDir(APK)"
    } else {
        "other"
    };
    (exe, origin)
}

// ---------------------------------------------------------------------------
// AVB audit verdict (2026-11-10 S1). Pure function: inputs are the recorded
// deployment state and a fresh observation of the boot partition, so every
// branch below is reachable from a unit test with no device.
//
// Decision table (`ok` / `critical`):
//
// | recorded avb        | fresh observation            | ok    | why                     |
// |---------------------|------------------------------|-------|-------------------------|
// | any                 | unobservable (no device)     | false | unknown ≠ pass          |
// | avb_unobservable    | unobservable                 | false | deploy could not measure|
// | avb_no_footer       | no footer                    | false | nothing was verified    |
// | avb_no_footer       | footer appeared              | false | partition rewritten     |
// | avb_ok              | no footer now                | false | footer destroyed        |
// | avb_ok              | footer, digests differ       | false | footer/vbmeta modified  |
// | avb_ok              | footer, digests equal        | true  | measured and unchanged  |
// | missing/unknown str | any                          | false | state.json tampered/old |
//
// `critical` is always true here: once `deploy` recorded a measured value, a
// later divergence is real evidence of an AVB-touching operation. When the
// observation is impossible we still fail (unknown is not a pass) but mark it
// non-critical, so an unobservable block device does not send the device into
// `needs_repair` forever — the detail says `unknown` and QA sees it.
// ---------------------------------------------------------------------------
fn avb_verdict(
    state: Option<&deploy::DeployState>,
    observed: &deploy::AvbObservation,
) -> Check {
    const NO_PASS: &str = "unknown — cannot be verified, NOT a pass";

    let Some(state) = state else {
        return Check::unknown(
            "avb_untouched",
            format!("{NO_PASS}: no state.json (deploy has not recorded an observation)"),
        );
    };

    let recorded_code = state.avb.as_str();
    let live = observed.code();

    // 1. Could we measure at all?
    if live == "avb_unobservable" {
        return Check::unknown(
            "avb_untouched",
            format!(
                "{NO_PASS}: boot partition unreadable ({})",
                observed.problem.as_deref().unwrap_or("-")
            ),
        );
    }

    // 2. Deploy itself never got a measurement — nothing to compare against.
    //    `avb_malformed` is likewise not a baseline: a footer whose fields are
    //    illegal tells us nothing about what the boot partition "should" look
    //    like.
    if recorded_code != "avb_ok" && recorded_code != "avb_no_footer" {
        return Check::unknown(
            "avb_untouched",
            format!(
                "unknown — recorded audit={recorded_code:?} is not a measurement \
                 (deploy could not observe AVB); NOT a pass"
            ),
        );
    }

    // 1b. A footer exists now but its fields are invalid (out of bounds,
    //     overlapping, illegal vbmeta offset/size). That is a *positive*
    //     observation of corruption — reported as FAIL, and never as the
    //     "no footer" branch below (auditor finding 2026-11-10 #3).
    if live == "avb_malformed" {
        let was_ok = recorded_code == "avb_ok";
        return Check::new(
            "avb_untouched",
            false,
            was_ok,
            format!(
                "AVB footer present but structurally invalid ever since \
                 (recorded={recorded_code}): {}{}",
                observed.problem.as_deref().unwrap_or("-"),
                if was_ok {
                    "; the boot partition was modified after deploy"
                } else {
                    "; no valid baseline exists, so this cannot be verified"
                }
            ),
        );
    }

    // 3. A previously observed footer is gone now.
    if recorded_code == "avb_ok" && live == "avb_no_footer" {
        return Check::new(
            "avb_untouched",
            false,
            true,
            format!(
                "AVB footer was present at deploy ({} B) but is missing now ({}): \
                 the boot partition was rewritten",
                state.avb_partition_bytes.unwrap_or(0),
                observed.problem.as_deref().unwrap_or("-")
            ),
        );
    }

    // 4. A footer appeared where deploy saw none.
    if recorded_code == "avb_no_footer" && live == "avb_ok" {
        return Check::new(
            "avb_untouched",
            false,
            true,
            format!(
                "AVB footer absent at deploy but present now ({}): the boot partition \
                 was rewritten",
                observed.source
            ),
        );
    }

    // 5. Both observations say "no footer": a legitimate un-AVB'd device, but
    //    there is nothing to have untouched, so this must not read as a pass.
    if live == "avb_no_footer" {
        return Check::unknown(
            "avb_untouched",
            format!(
                "measured: boot partition has no AVB footer ({}); nothing was verified \
                 — not a pass",
                observed.problem.as_deref().unwrap_or("-")
            ),
        );
    }

    // 6. Both sides claim a footer: compare the actual bytes' digests.
    let footer_match = match (&state.avb_footer_sha256, &observed.footer_sha256) {
        (Some(recorded), Some(live)) => recorded == live,
        _ => false,
    };
    let vbmeta_match = match (&state.avb_vbmeta_sha256, &observed.vbmeta_sha256) {
        (Some(recorded), Some(live)) => recorded == live,
        _ => false,
    };
    let len_match = state.avb_partition_bytes == Some(observed.partition_bytes);

    if footer_match && vbmeta_match && len_match {
        return Check::new(
            "avb_untouched",
            true,
            true,
            format!(
                "measured OK: AVB footer + vbmeta unchanged since deploy \
                 (source={} partition={} B footer_sha256={} vbmeta_sha256={})",
                observed.source,
                observed.partition_bytes,
                short_sha(state.avb_footer_sha256.as_deref()),
                short_sha(state.avb_vbmeta_sha256.as_deref()),
            ),
        );
    }

    let mut diffs = Vec::new();
    if !len_match {
        diffs.push(format!(
            "partition size {} B != recorded {:?}",
            observed.partition_bytes, state.avb_partition_bytes
        ));
    }
    if !footer_match {
        diffs.push(format!(
            "footer digest {} != recorded {}",
            short_sha(observed.footer_sha256.as_deref()),
            short_sha(state.avb_footer_sha256.as_deref())
        ));
    }
    if !vbmeta_match {
        diffs.push(format!(
            "vbmeta digest {} != recorded {}",
            short_sha(observed.vbmeta_sha256.as_deref()),
            short_sha(state.avb_vbmeta_sha256.as_deref())
        ));
    }
    Check::new(
        "avb_untouched",
        false,
        true,
        format!(
            "AVB structures changed since deploy ({}): {}", observed.source, diffs.join("; ")
        ),
    )
}

fn short_sha(value: Option<&str>) -> String {
    match value {
        // `&sha[..16]` would panic on a non-ASCII string whose byte 16 is not a
        // char boundary, and release builds use `panic = "abort"` — a corrupted
        // state.json must never abort the diagnostic command (auditor 2026-11-10).
        Some(sha) if sha.chars().count() >= 16 => {
            format!("{}…", sha.chars().take(16).collect::<String>())
        }
        Some(sha) => sha.to_string(),
        None => "<none>".to_string(),
    }
}

pub fn run(json: bool) -> Result<()> {
    let mut checks: Vec<Check> = Vec::new();

    // Deployment stage decides the criticality of everything below.
    let deployed = Path::new(defs::DAEMON_PATH).exists();
    let stage = if deployed { "deployed" } else { "undeployed" };

    // ---- process identity ------------------------------------------------
    let uid = unsafe { libc::getuid() };
    checks.push(Check::new(
        "root",
        uid == 0,
        true,
        format!("uid={uid} (root-exec/deploy expect 0)"),
    ));

    // ---- self binary (never hardcode /data/adb/apd) -----------------------
    let (exe, origin) = running_from();
    let exe_ok = exe.is_file() && is_executable(&exe);
    checks.push(Check::new(
        "self_binary",
        exe_ok,
        true,
        format!(
            "path={} origin={} mode={:?} version={} commit={}",
            exe.display(),
            origin,
            mode_of(&exe),
            defs::VERSION_NAME.trim(),
            defs::VERSION_COMMIT.trim()
        ),
    ));

    // ---- deployed binary ------------------------------------------------
    let deployed_bin = if deployed {
        Check::new(
            "deployed_binary",
            is_executable(Path::new(defs::DAEMON_PATH)),
            true,
            format!(
                "{} mode={:?}",
                defs::DAEMON_PATH,
                mode_of(Path::new(defs::DAEMON_PATH))
            ),
        )
    } else {
        Check::pending("deployed_binary", "first-boot self-check")
    };
    checks.push(deployed_bin);

    // ---- key file (0600, 47/48 alnum, lockable) --------------------------
    let key_check = if deployed || Path::new(defs::SPROOT_KEY_FILE).exists() {
        let mut detail = "missing".to_string();
        let mut ok = false;
        if Path::new(defs::SPROOT_KEY_FILE).exists() {
            let mode = mode_of(Path::new(defs::SPROOT_KEY_FILE)).unwrap_or(0);
            match secret::read_key_file(Path::new(defs::SPROOT_KEY_FILE)) {
                Some(key) => {
                    let len = key.len();
                    let alnum = key.as_str().chars().all(|c| c.is_ascii_alphanumeric());
                    ok = mode == 0o600 && (len == 47 || len == 48) && alnum && key.is_locked();
                    detail = format!(
                        "mode={mode:o} len={len} alnum={alnum} mlocked={}{}",
                        key.is_locked(),
                        if len == 48 {
                            " legacy48=true (历史 48 位文件，运行时按前 47 使用)"
                        } else {
                            ""
                        }
                    );
                }
                None => detail = format!("mode={mode:o} unreadable/empty"),
            }
        }
        Check::new("key_file", ok, true, detail)
    } else {
        Check::pending("key_file", "first-boot self-check")
    };
    checks.push(key_check);
    checks.push(Check::new(
        "mlock_supported",
        secret::mlock_supported(),
        false,
        "platform allows locking key pages".to_string(),
    ));

    // ---- self-start chain contract (NEW-DOC-05 §3.1.3) -------------------
    // Raw bytes, not the parsed key: the AP-side ABI (47 plain-text bytes, no
    // NUL) is what the injected rc's `--superkey-file` path must lead to, so it
    // is verified here rather than inferred from a successful read.
    let key_path = Path::new(defs::SPROOT_KEY_FILE);
    let superkey_check = match fs::read(key_path) {
        Ok(mut bytes) => {
            let mode = mode_of(key_path).unwrap_or(0);
            let len = bytes.len();
            let nul_free = !bytes.contains(&0);
            // The raw copy is key material: wipe it as soon as it is measured.
            // SAFETY: `bytes` is a live, uniquely-owned buffer we no longer use.
            unsafe { secret::wipe_bytes(bytes.as_mut_ptr(), bytes.len()) };
            let (ok, legacy48) = superkey_abi_verdict(mode, len, nul_free);
            if legacy48 {
                // Old manager / old APK wrote 48 bytes. It still escalates
                // (first 47 used, logged by `escalation_token`), so this is a
                // warning, not a broken deployment: never fail an upgraded
                // install (Lead 2026-11-09). Re-deploy rewrites it to 47.
                warn!(
                    "[selftest] {} 是历史 48 字节文件：AP 侧 ABI 为 47 字节，运行时按前 47 使用；建议重部署或重新生成密钥",
                    defs::SPROOT_KEY_FILE
                );
            }
            let detail = if legacy48 {
                format!(
                    "{} mode={mode:o} len=48 nul_free=true legacy48=true (检测到 48 位历史文件，已按前 47 位使用；建议重部署/重新生成密钥)",
                    defs::SPROOT_KEY_FILE
                )
            } else {
                format!(
                    "{} mode={mode:o} len={len} nul_free={nul_free} (AP-side ABI: 47 plain bytes)",
                    defs::SPROOT_KEY_FILE
                )
            };
            Check::new("superkey_file", ok, deployed, detail)
        }
        Err(_) if deployed => Check::new(
            "superkey_file",
            false,
            true,
            format!(
                "missing: {} (deploy must write it; rc starts the daemon with this path)",
                defs::SPROOT_KEY_FILE
            ),
        ),
        Err(_) => Check::pending("superkey_file", "first-boot self-check"),
    };
    checks.push(superkey_check);

    // Audit copy of the ramdisk rc template; init reads /init.sproot.rc, so a
    // missing audit copy is drift evidence, not a boot blocker.
    let rc_path = Path::new(defs::SPROOT_RC_FILE);
    let rc_check = if rc_path.is_file() {
        Check::new(
            "rc_template_present",
            true,
            false,
            format!(
                "{} present (audit copy; ramdisk /init.sproot.rc is authoritative)",
                defs::SPROOT_RC_FILE
            ),
        )
    } else if deployed {
        Check::new(
            "rc_template_present",
            false,
            false,
            format!(
                "missing: {} (deploy writes this audit copy)",
                defs::SPROOT_RC_FILE
            ),
        )
    } else {
        Check::pending("rc_template_present", "first-boot self-check")
    };
    checks.push(rc_check);

    // Did the rc actually start the daemon? Never critical: deployment without
    // a reboot is a normal intermediate state, and an unreadable /proc must not
    // be reported as a failure ("unknown (not applicable)").
    let rc_started_check = if !deployed {
        Check::new(
            "rc_started",
            true,
            false,
            "unknown (not applicable)".to_string(),
        )
    } else if let Some(pid) = running_daemon_pid() {
        Check::new(
            "rc_started",
            true,
            false,
            format!("daemon running as service: pid={pid}"),
        )
    } else if proc_scan_available() {
        Check::new(
            "rc_started",
            false,
            false,
            "daemon not observed (rc start pending or device not rebooted)".to_string(),
        )
    } else {
        Check::new(
            "rc_started",
            true,
            false,
            "unknown (not applicable)".to_string(),
        )
    };
    checks.push(rc_started_check);

    // ---- authorisation table (tri-state JSON, NEW-DOC-05 §13) -------------
    // The check name stays `allow_list` for schema stability (QA/UI already map
    // it); the detail now reports the authoritative JSON store and its states.
    let acl_check = if deployed {
        let stats = auth_list::stats();
        Check::new(
            "allow_list",
            stats.valid,
            true,
            format!(
                "{} entries={} allow={} deny={} ask={} legacy_migrated={} (legacy csv {} kept as read-only migration source)",
                defs::AUTH_LIST_FILE,
                stats.entries,
                stats.allow,
                stats.deny,
                stats.ask,
                stats.migrated_legacy,
                defs::ALLOW_LIST_FILE,
            ),
        )
    } else {
        Check::pending("allow_list", "first-boot self-check")
    };
    checks.push(acl_check);

    // ---- Zygisk / APatch compatibility signal (Phase 1 placeholder) ------
    // NeoZygisk/OnyxZygisk identify the host as APatch via `apd -V` (10762) and
    // read `/data/adb/ap/package_config`; this check surfaces whether a Zygisk
    // helper is present/readable. Deliberately **not critical** this round: the
    // built-in Zygisk packaging decision is still pending (Lead 2026-11-10).
    let package_config = Path::new(defs::APATCH_PACKAGE_CONFIG);
    let package_config_exists = package_config.is_file();
    let package_config_mode = mode_of(package_config).unwrap_or(0);
    // We write 0644: readable by a non-root Zygisk helper (group/other read),
    // and never writable by anyone but root.
    let package_config_readable = package_config_exists
        && (package_config_mode & 0o044) != 0
        && (package_config_mode & 0o022) == 0;
    let zygiskd = zygiskd_pid();
    let module_prop = zygisk_module_prop();
    checks.push(Check::new(
        "zygisk",
        zygiskd.is_some() && package_config_readable,
        false,
        format!(
            "apatch_compat_version={} zygiskd_pid={zygiskd:?} running={} module={} version={} package_config={} exists={package_config_exists} mode={package_config_mode:o} readable={package_config_readable} (Phase 1 placeholder: built-in Zygisk packaging pending)",
            defs::APATCH_COMPAT_VERSION,
            zygiskd.is_some(),
            module_prop.as_ref().map(|(path, _)| path.as_str()).unwrap_or("-"),
            module_prop.as_ref().map(|(_, version)| version.as_str()).unwrap_or("-"),
            defs::APATCH_PACKAGE_CONFIG,
        ),
    ));

    // ---- hidden su -------------------------------------------------------
    let su_link = format!("{}su", defs::BINARY_DIR);
    let su_check = if deployed {
        let ok = (|| {
            let link = fs::symlink_metadata(&su_link).ok()?;
            if !link.file_type().is_symlink() {
                return None;
            }
            let target = fs::read_link(&su_link).ok()?;
            Some(mode_of(&target)? == 0o700)
        })()
        .unwrap_or(false);
        Check::new("hidden_su", ok, true, format!("{su_link} -> 0700 target"))
    } else {
        Check::pending("hidden_su", "first-boot self-check")
    };
    checks.push(su_check);

    // ---- directory modes -------------------------------------------------
    for (name, path, want) in [
        ("dir_sproot", defs::SPROOT_DIR, 0o700u32),
        ("dir_ap", defs::WORKING_DIR, 0o700),
        ("dir_bin", defs::BINARY_DIR, 0o755),
    ] {
        let mode = mode_of(Path::new(path)).unwrap_or(0);
        checks.push(Check::new(
            name,
            !deployed || mode == want,
            false,
            format!("{path} mode={mode:o} want={want:o}"),
        ));
    }

    // ---- apd resolvable from a module script's PATH (P0, 2026-11-11) ------
    //
    // The failure this guards against is *silent* in production: NeoZygisk runs
    // `Command::new("apd")` inside a module script, gets ENOENT, falls back to
    // `RootImpl::None`, and the Zygisk chain just never works — logged at
    // `debug!`, i.e. invisible on a device. So the check is critical: once
    // deployed, "module scripts cannot find apd" is a real defect, not a warning.
    //
    // Three states, and "unknown" is never a pass:
    //   * resolvable to DAEMON_PATH               -> ok
    //   * resolved elsewhere / not at all / PATH
    //     missing a required entry                -> fail (critical)
    //   * cannot be determined (no /data/adb, no
    //     read permission on the tree)             -> unknown (not critical)
    let apd_path_check = if !deployed {
        Check::pending("apd_in_path", "first-boot self-check")
    } else {
        let path_env = crate::module::module_script_path(
            &std::env::var("PATH").unwrap_or_default(),
        );
        let verdict = crate::module::apd_path_verdict(&path_env, |entry| {
            // Resolve `entry/apd` the way execvp would: follow the symlink and
            // canonicalise, so a link to the wrong place is caught.
            let candidate = Path::new(entry).join(crate::module::APD_IN_PATH_NAME);
            fs::canonicalize(candidate).ok()
        });
        let symlink = crate::module::apd_in_path_symlink();
        // The symlink is asserted **separately** from resolution, because the
        // `/data/adb` PATH entry is a fallback that would otherwise hide its
        // absence entirely: with `/data/adb/apd` present, `apd` still resolves
        // even when `bin/apd` is gone (verified on the CLI). That masking is
        // exactly why a missing symlink must be reported rather than absorbed —
        // it means deploy's contract is violated, and the next PATH change (a
        // module that rewrites PATH, a busybox-only environment) would silently
        // reintroduce the original Zygisk failure.
        let symlink_state = symlink_target_state(&symlink);
        match verdict {
            crate::module::ApdPathVerdict::Resolved => match symlink_state {
                SymlinkState::Correct => Check::new(
                    "apd_in_path",
                    true,
                    true,
                    format!(
                        "`apd` resolves from module-script PATH ({} -> {}); PATH carries {} and {}",
                        symlink,
                        defs::DAEMON_PATH,
                        defs::BINARY_DIR.trim_end_matches('/'),
                        defs::ADB_DIR.trim_end_matches('/'),
                    ),
                ),
                SymlinkState::Missing => Check::new(
                    "apd_in_path",
                    false,
                    true,
                    format!(
                        "{symlink} is MISSING (deploy must create it). `apd` currently still \
                         resolves via the {} PATH entry, so Zygisk works today — but the \
                         documented symlink is gone, so any module that rewrites PATH brings \
                         back the silent RootImpl::None failure. Re-run deploy.",
                        defs::ADB_DIR.trim_end_matches('/')
                    ),
                ),
                SymlinkState::Wrong(other) => Check::new(
                    "apd_in_path",
                    false,
                    true,
                    format!(
                        "{symlink} points to {other}, expected {} (stale/foreign entry; \
                         re-run deploy to recreate it)",
                        defs::DAEMON_PATH
                    ),
                ),
            },
            // Before declaring a failure, ask whether we were *allowed to look*.
            // `canonicalize` fails for ENOENT (a real defect) and also for EACCES
            // on a parent directory (we simply cannot see it). Reporting the
            // latter as FAIL would be crying wolf on an unobservable environment,
            // which the project's `rc_started` convention forbids.
            crate::module::ApdPathVerdict::NotFound
                if !path_probe_is_determinable() =>
            {
                Check::unknown(
                    "apd_in_path",
                    format!(
                        "unknown — cannot enumerate {} or {} (permission denied or missing \
                         mount); `apd` was not found but neither can we prove it is absent. \
                         NOT a pass",
                        defs::BINARY_DIR,
                        defs::ADB_DIR,
                    ),
                )
            }
            crate::module::ApdPathVerdict::NotFound => Check::new(
                "apd_in_path",
                false,
                true,
                format!(
                    "module scripts CANNOT resolve `apd` (no {symlink}, and no {} on PATH) \
                     -> NeoZygisk falls back to RootImpl::None and the Zygisk chain fails \
                     silently",
                    defs::DAEMON_PATH
                ),
            ),
            crate::module::ApdPathVerdict::WrongTarget(found) => Check::new(
                "apd_in_path",
                false,
                true,
                format!(
                    "`apd` on PATH resolves to {found}, expected {} (stale/foreign entry; \
                     re-run deploy to recreate {symlink})",
                    defs::DAEMON_PATH
                ),
            ),
            crate::module::ApdPathVerdict::MissingEntry(entry) => Check::new(
                "apd_in_path",
                false,
                true,
                format!(
                    "module-script PATH is missing {entry} (required so `apd` resolves); \
                     a path string without it is contract drift, not a working install"
                ),
            ),
        }
    };
    checks.push(apd_path_check);

    // ---- daemon + socket -------------------------------------------------
    let mut daemon_alive = false;
    if deployed {
        let pid = daemon::daemon_pid();
        daemon_alive = pid
            .map(|p| Path::new(&format!("/proc/{p}")).exists())
            .unwrap_or(false);
        checks.push(Check::new(
            "daemon",
            daemon_alive,
            false,
            format!("pid={pid:?} alive={daemon_alive}"),
        ));
    } else {
        checks.push(Check::pending("daemon", "first-boot self-check"));
    }

    let sock_name = fs::read_to_string(defs::AUTH_SOCK_FILE)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let sock_state = match &sock_name {
        None => "socket name file missing/unreadable".to_string(),
        Some(name) => match daemon::probe_abstract_bind(name) {
            Ok(()) => format!("@{name} bindable -> daemon NOT listening"),
            Err(e) if e.raw_os_error() == Some(libc::EADDRINUSE) => {
                format!("@{name} in use -> daemon listening")
            }
            Err(e) => format!("@{name} probe failed: {e}"),
        },
    };
    checks.push(Check::new(
        "auth_socket",
        sock_name.is_some() || !deployed,
        false,
        sock_state,
    ));

    // ---- boot-loop guard -------------------------------------------------
    let boot_count = fs::read_to_string(defs::SPROOT_BOOT_COUNT_FILE)
        .ok()
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "-".to_string());
    let safemode = Path::new(defs::SPROOT_SAFEMODE_FILE).exists();
    checks.push(Check::new(
        "bootstate",
        true,
        false,
        format!("boot_count={boot_count} safemode={safemode}"),
    ));

    // ---- deploy state / version consistency / AVB audit ------------------
    let state = deploy::read_state();
    if deployed {
        let commit_match = state
            .as_ref()
            .map(|s| s.commit == defs::VERSION_COMMIT.trim())
            .unwrap_or(false);
        checks.push(Check::new(
            "version_consistency",
            commit_match,
            true,
            format!(
                "deployed_commit={:?} binary_commit={}",
                state.as_ref().map(|s| s.commit.clone()),
                defs::VERSION_COMMIT.trim()
            ),
        ));
        // ---- AVB audit: re-measure, never trust the recorded string --------
        // 2026-11-10 S1 fix. Before this, deploy wrote the literal
        // `"vbmeta_untouched"` and this check compared it to the same literal:
        // structurally always PASS and unrelated to any real vbmeta (the exact
        // anti-pattern AVB-3 shipped once, HANDOFF-2026-11-08 §"教训").
        //
        // Now the check *re-reads the boot partition* and compares its digests
        // to the ones recorded at deploy time. Every branch that cannot measure
        // reports `ok=false` with a reason — an unverifiable AVB claim must never
        // render as `[ok]`. See `avb_verdict` for the decision table and its
        // unit tests (`avb_verdict_*`), including the negative samples.
        let observed = deploy::probe_avb();
        checks.push(avb_verdict(state.as_ref(), &observed));
    } else {
        checks.push(Check::pending(
            "version_consistency",
            "first-boot self-check",
        ));
        checks.push(Check::pending("avb_untouched", "first-boot self-check"));
    }

    // ---- SELinux ---------------------------------------------------------
    checks.push(Check::new(
        "selinux",
        true,
        false,
        format!(
            "present={} context={:?}",
            selinux::selinux_present(),
            selinux::current_context()
        ),
    ));

    let critical_ok = checks.iter().filter(|c| c.critical).all(|c| c.ok);
    // Stable summary for the manager UI (app-client maps these four states):
    //   undeployed    — 未部署（App 首启自检，正常）
    //   deployed      — 已部署但 daemon 未在跑（可能刚重启/被停）
    //   daemon_ready  — 已部署且 daemon 在跑
    //   needs_repair  — 已部署但有 critical 项失败
    let summary = if !deployed {
        "undeployed"
    } else if !critical_ok {
        "needs_repair"
    } else if daemon_alive {
        "daemon_ready"
    } else {
        "deployed"
    };
    if json {
        let payload = serde_json::json!({
            "ok": critical_ok,
            "stage": stage,
            "summary": summary,
            "checks": checks,
        });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
        println!("stage: {stage}");
        for c in &checks {
            // `unknown` must never print as `[ok]` (2026-11-10 S1): an
            // unverifiable assertion is not a passing one.
            println!(
                "[{}] {}{} — {}",
                c.status,
                c.name,
                if c.critical { " (critical)" } else { "" },
                c.detail
            );
        }
        println!("selftest: {}", if critical_ok { "ok" } else { "FAILED" });
    }
    if !critical_ok {
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `module.prop` parsing used by the `zygisk` selftest check.
    #[test]
    fn module_prop_version_parsing() {
        assert_eq!(
            parse_module_prop_version("id=zygisksu\nversion=v3.1\n"),
            Some("v3.1".to_string())
        );
        assert_eq!(
            parse_module_prop_version("# no version here\nid=x\n"),
            None
        );
        assert_eq!(parse_module_prop_version("version=   \n"), None);
        assert_eq!(parse_module_prop_version(""), None);
    }

    /// `superkey_file` verdict must not fail an upgraded install: a legacy
    /// 48-byte file passes with the `legacy48` flag (warn + detail), while a
    /// truly broken file (length/mode/NUL) still fails.
    #[test]
    fn superkey_verdict_accepts_47_and_legacy_48() {        // canonical 47 plain bytes
        assert_eq!(superkey_abi_verdict(0o600, 47, true), (true, false));
        // legacy 48: pass (so summary stays daemon_ready/deployed), flagged
        assert_eq!(superkey_abi_verdict(0o600, 48, true), (true, true));
        // wrong mode / short / long / NUL-tainted → fail, not "legacy"
        assert_eq!(superkey_abi_verdict(0o644, 47, true), (false, false));
        assert_eq!(superkey_abi_verdict(0o600, 46, true), (false, false));
        assert_eq!(superkey_abi_verdict(0o600, 49, true), (false, false));
        assert_eq!(superkey_abi_verdict(0o600, 47, false), (false, false));
        assert_eq!(superkey_abi_verdict(0o600, 48, false), (false, false));
        // 48 with a NUL is the kernel format copied in by hand, not legacy 48
        assert_eq!(superkey_abi_verdict(0o600, 48, false), (false, false));
    }

    // -----------------------------------------------------------------------
    // AVB verdict (2026-11-10 S1). These are the tests that make the check
    // *falsifiable* — the property the previous `state.avb == "vbmeta_untouched"`
    // comparison structurally could not have (write a constant, read it back).
    // -----------------------------------------------------------------------

    fn avb_state(
        code: &str,
        footer: Option<&str>,
        vbmeta: Option<&str>,
        bytes: Option<u64>,
    ) -> deploy::DeployState {
        deploy::DeployState {
            version: "test".into(),
            commit: "0".into(),
            avb: code.into(),
            avb_footer_sha256: footer.map(str::to_string),
            avb_vbmeta_sha256: vbmeta.map(str::to_string),
            avb_source: Some("/dev/block/by-name/boot".into()),
            avb_probe_trace: Some("fixture".into()),
            avb_partition_bytes: bytes,
            deployed_at: 0,
            hidden_file: "x".into(),
            source: "/tmp".into(),
        }
    }

    fn observed(
        partition_bytes: u64,
        footer: Option<&str>,
        vbmeta: Option<&str>,
        problem: Option<&str>,
    ) -> deploy::AvbObservation {
        // Outcome mirrors what a real observation would report for these inputs,
        // so tests exercise the same `code()` mapping as production:
        //   no footer digest at all         -> Unobservable (nothing measured)
        //   footer + vbmeta digests present -> Ok
        //   footer digest but no vbmeta     -> footer-only mode, still Ok
        let (outcome, footer_present) = if footer.is_none() {
            (deploy::AvbOutcome::Unobservable, false)
        } else {
            (deploy::AvbOutcome::Ok, true)
        };
        deploy::AvbObservation {
            source: "/dev/block/by-name/boot".into(),
            partition_bytes,
            footer_sha256: footer.map(str::to_string),
            vbmeta_sha256: vbmeta.map(str::to_string),
            footer_present,
            outcome,
            problem: problem.map(str::to_string),
        }
    }

    /// Like [`observed`] but lets the caller choose the outcome explicitly — used
    /// to exercise `avb_malformed` / `avb_no_footer` branches.
    fn observed_as(
        code: &str,
        partition_bytes: u64,
        problem: Option<&str>,
    ) -> deploy::AvbObservation {
        let outcome = match code {
            "avb_ok" => deploy::AvbOutcome::Ok,
            "avb_no_footer" => deploy::AvbOutcome::NoFooter,
            "avb_malformed" => deploy::AvbOutcome::Malformed,
            _ => deploy::AvbOutcome::Unobservable,
        };
        let present = outcome == deploy::AvbOutcome::Ok;
        deploy::AvbObservation {
            source: "/dev/block/by-name/boot".into(),
            partition_bytes,
            footer_sha256: if present { Some(F.to_string()) } else { None },
            vbmeta_sha256: if present { Some(V.to_string()) } else { None },
            footer_present: present,
            outcome,
            problem: problem.map(str::to_string),
        }
    }

    const F: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const V: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

    /// The only PASS branch: deploy measured, and the re-measurement matches.
    #[test]
    fn avb_verdict_passes_only_on_matching_measurements() {
        let state = avb_state("avb_ok", Some(F), Some(V), Some(8192));
        let live = observed(8192, Some(F), Some(V), None);
        let c = avb_verdict(Some(&state), &live);
        assert!(c.ok, "detail={}", c.detail);
        assert!(c.critical, "a verified AVB claim stays critical: {}", c.detail);
        assert_eq!(c.status, "ok");
        assert!(c.detail.contains("measured OK"), "detail={}", c.detail);
    }

    /// **Negative sample: footer destroyed.** Deploy saw a footer, the device now
    /// has none → FAIL (and critical). Under the old tautology this scenario was
    /// unreachable, because nobody ever read the partition.
    #[test]
    fn avb_verdict_fails_when_footer_disappears() {
        let state = avb_state("avb_ok", Some(F), Some(V), Some(8192));
        // A *measured* no-footer (the partition was read; the magic is gone).
        let live = observed_as("avb_no_footer", 8192, Some("no AVB footer (magic at -64 is ...)"));
        let c = avb_verdict(Some(&state), &live);
        assert!(!c.ok, "missing footer must FAIL: {}", c.detail);
        assert!(c.critical, "a destroyed footer is a real regression: {}", c.detail);
        assert_eq!(c.status, "fail");
        assert!(c.detail.contains("missing now"), "detail={}", c.detail);
    }

    /// **Negative sample: footer or vbmeta modified.** Same size, same footer
    /// magic, different bytes → FAIL. The footer digest alone would catch a
    /// footer rewrite; the vbmeta digest catches payload tampering.
    #[test]
    fn avb_verdict_fails_when_vbmeta_changes() {
        let state = avb_state("avb_ok", Some(F), Some(V), Some(8192));
        let tampered_vbmeta = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
        let c = avb_verdict(Some(&state), &observed(8192, Some(F), Some(tampered_vbmeta), None));
        assert!(!c.ok, "changed vbmeta must FAIL: {}", c.detail);
        assert!(c.critical);
        assert_eq!(c.status, "fail");
        assert!(c.detail.contains("vbmeta digest"), "detail={}", c.detail);

        let tampered_footer = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
        let c = avb_verdict(
            Some(&state),
            &observed(8192, Some(tampered_footer), Some(V), None),
        );
        assert!(!c.ok, "changed footer must FAIL: {}", c.detail);
        assert!(c.detail.contains("footer digest"), "detail={}", c.detail);
    }

    /// **Negative sample: partition resized.** The footer lives at the end, so a
    /// resize necessarily moves it — even with identical digests this must FAIL.
    #[test]
    fn avb_verdict_fails_when_partition_size_changes() {
        let state = avb_state("avb_ok", Some(F), Some(V), Some(8192));
        let c = avb_verdict(Some(&state), &observed(16384, Some(F), Some(V), None));
        assert!(!c.ok, "resize must FAIL: {}", c.detail);
        assert!(c.critical);
        assert!(c.detail.contains("partition size"), "detail={}", c.detail);
    }

    /// A footer appearing where there was none is also a rewrite → FAIL.
    #[test]
    fn avb_verdict_fails_when_footer_appears() {
        let state = avb_state("avb_no_footer", None, None, Some(8192));
        let c = avb_verdict(Some(&state), &observed(8192, Some(F), Some(V), None));
        assert!(!c.ok, "appearing footer must FAIL: {}", c.detail);
        assert!(c.critical);
        assert!(c.detail.contains("was rewritten"), "detail={}", c.detail);
    }

    /// "No footer" on both sides is a legitimate un-AVB'd device, but it is
    /// **not** a pass: nothing was verified. Non-critical so the device is not
    /// pushed into `needs_repair` for a state we cannot act on.
    #[test]
    fn avb_verdict_no_footer_is_not_a_pass() {
        let state = avb_state("avb_no_footer", None, None, Some(8192));
        // Readable partition that genuinely carries no AVB footer.
        let c = avb_verdict(
            Some(&state),
            &observed_as("avb_no_footer", 8192, Some("no AVB footer")),
        );
        assert!(!c.ok, "no footer must not be reported as ok: {}", c.detail);
        assert!(!c.critical, "unverifiable device state must not be critical: {}", c.detail);
        assert_eq!(c.status, "unknown", "must render as unknown, not [ok]");
        assert!(c.detail.contains("not a pass"), "detail={}", c.detail);
    }

    /// **Unknown is never a pass.** An unreadable block device reports
    /// `ok=false` + `status=unknown`, so the text UI prints neither `[ok]` nor
    /// `[fail]` — matching the existing `rc_started` convention ("无法判定报
    /// unknown 不误报失败") while still refusing to claim a pass.
    #[test]
    fn avb_verdict_unobservable_is_unknown_not_pass() {
        let state = avb_state("avb_ok", Some(F), Some(V), Some(8192));
        let c = avb_verdict(Some(&state), &observed(0, None, None, Some("no readable boot partition")));
        assert!(!c.ok, "unobservable must not pass: {}", c.detail);
        assert!(!c.critical, "unknown must not be critical: {}", c.detail);
        assert_eq!(c.status, "unknown");
        assert!(c.detail.contains("unknown"), "detail={}", c.detail);
        assert!(c.detail.contains("NOT a pass"), "detail={}", c.detail);
    }

    /// Deploy itself could not measure (recorded `avb_unobservable`) → still not
    /// a pass, even if the device is readable now: there is no baseline.
    #[test]
    fn avb_verdict_without_recorded_measurement_never_passes() {
        let state = avb_state("avb_unobservable", None, None, None);
        let c = avb_verdict(Some(&state), &observed(8192, Some(F), Some(V), None));
        assert!(!c.ok, "no baseline must not pass: {}", c.detail);
        assert_eq!(c.status, "unknown");
        assert!(c.detail.contains("NOT a pass"), "detail={}", c.detail);
    }

    /// The old sentinel string is now **worthless**: a state.json still holding
    /// `"vbmeta_untouched"` (e.g. written by the buggy build) must fail loudly
    /// rather than keep passing. This is the direct regression test for the S1
    /// bug: with the old code this exact input returned `ok=true`.
    #[test]
    fn avb_verdict_rejects_legacy_self_referential_sentinel() {
        let legacy = avb_state("vbmeta_untouched", None, None, None);
        let c = avb_verdict(Some(&legacy), &observed(8192, Some(F), Some(V), None));
        assert!(
            !c.ok,
            "the legacy constant must no longer be accepted (that was the bug): {}",
            c.detail
        );
        assert_eq!(c.status, "unknown");
        assert!(c.detail.contains("not a measurement"), "detail={}", c.detail);
    }

    /// Missing state.json → not a pass.
    #[test]
    fn avb_verdict_without_state_is_not_a_pass() {
        let c = avb_verdict(None, &observed(8192, Some(F), Some(V), None));
        assert!(!c.ok, "detail={}", c.detail);
        assert_eq!(c.status, "unknown");
        assert!(c.detail.contains("no state.json"), "detail={}", c.detail);
    }

    /// `pending` (stage=undeployed) is the one non-`ok`-renderable exception:
    /// it is "not applicable yet", not an assertion, so it stays `ok: true` with
    /// a distinct status — and it must never be confused with `unknown`.
    #[test]
    fn pending_and_unknown_are_distinct_statuses() {
        let pending = Check::pending("avb_untouched", "first-boot self-check");
        assert!(pending.ok);
        assert!(!pending.critical);
        assert_eq!(pending.status, "pending");

        let unknown = Check::unknown("avb_untouched", "cannot read".into());
        assert!(!unknown.ok);
        assert!(!unknown.critical);
        assert_eq!(unknown.status, "unknown");
        assert_ne!(pending.status, unknown.status);
        // Neither may ever render as the passing label.
        assert_ne!(unknown.status, "ok");
    }

    // -----------------------------------------------------------------------
    // `short_sha` must never panic (auditor finding 2026-11-10).
    //
    // It formats digests read from `state.json`, which an attacker or a bad write
    // can fill with arbitrary bytes. The old `&sha[..16]` sliced by **byte** and
    // panicked whenever byte 16 was not a UTF-8 char boundary — and release builds
    // use `panic = "abort"`, so a corrupted state file would abort `apd selftest`
    // instead of reporting the corruption.
    // -----------------------------------------------------------------------

    #[test]
    fn short_sha_handles_multibyte_and_short_inputs() {
        // Multi-byte chars: 16 *bytes* lands mid-character, 16 *chars* does not.
        let multibyte = "Ω".repeat(40); // 2 bytes per char
        let out = short_sha(Some(&multibyte));
        assert_eq!(out.chars().count(), 17, "16 chars + ellipsis: {out}");
        assert!(out.ends_with('…'), "{out}");

        // 3-byte and 4-byte code points, and a mix.
        for s in [
            "日本語".repeat(20),
            "😀".repeat(20),
            format!("{}{}", "Ω".repeat(9), "日本語日本語日本"), // 9 + 8 = 17 chars
        ] {
            let out = short_sha(Some(&s));
            assert!(out.ends_with('…'), "{out}");
        }

        // A byte-16-boundary-violating prefix in isolation: 15 ASCII + 1 two-byte
        // char means byte offset 16 is *inside* the char — the old code panicked.
        let boundary = format!("{}{}", "a".repeat(15), "Ω".repeat(10));
        let _ = short_sha(Some(&boundary));

        // Short inputs pass through untouched; anything non-ASCII short is safe.
        assert_eq!(short_sha(Some("abc")), "abc");
        assert_eq!(short_sha(Some("Ω")), "Ω");
        assert_eq!(short_sha(None), "<none>");

        // Exactly 16 chars still gets the ellipsis treatment.
        let sixteen = "a".repeat(16);
        assert_eq!(short_sha(Some(&sixteen)), format!("{}…", "a".repeat(16)));
        // 15 chars is returned verbatim.
        assert_eq!(short_sha(Some(&"a".repeat(15))), "a".repeat(15));
    }

    /// The malformed footer is its own outcome — not "no footer", not a pass.
    #[test]
    fn avb_verdict_malformed_footer_is_distinct_from_no_footer() {
        let state = avb_state("avb_ok", Some(F), Some(V), Some(8192));
        let malformed = observed_as("avb_malformed", 8192, Some("vbmeta range exceeds partition"));
        let c = avb_verdict(Some(&state), &malformed);
        assert!(!c.ok, "malformed must fail: {}", c.detail);
        assert!(c.critical, "a footer that became malformed is a regression: {}", c.detail);
        assert_eq!(c.status, "fail");
        assert!(c.detail.contains("structurally invalid"), "detail={}", c.detail);

        // With no valid baseline it must still not pass.
        let no_baseline = avb_state("avb_no_footer", None, None, Some(8192));
        let c = avb_verdict(Some(&no_baseline), &malformed);
        assert!(!c.ok, "detail={}", c.detail);
        assert_eq!(c.status, "fail");
    }

    // -----------------------------------------------------------------------
    // `apd_in_path` symlink classification (P0, 2026-11-11).
    //
    // `symlink_target_state` is the piece that decides whether the deployed
    // `bin/apd` entry is correct. It has to be asserted separately from PATH
    // resolution because the `/data/adb` PATH entry masks a missing symlink
    // (verified on the CLI: deleting `bin/apd` still resolved `apd`). These
    // tests build real links in a temp dir, so no root and no device.
    // -----------------------------------------------------------------------

    struct LinkScratch(PathBuf);
    impl LinkScratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "sproot-apdpath-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            fs::create_dir_all(&dir).expect("create scratch");
            LinkScratch(dir)
        }
        fn path(&self, name: &str) -> String {
            self.0.join(name).display().to_string()
        }
    }
    impl Drop for LinkScratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A missing entry is `Missing`, never silently `Correct`.
    #[test]
    fn symlink_state_reports_missing() {
        let scratch = LinkScratch::new("missing");
        assert_eq!(
            symlink_target_state(&scratch.path("apd")),
            SymlinkState::Missing
        );
    }

    /// A link to the wrong binary is `Wrong`, and the detail carries the target
    /// so an operator can see *where* it points.
    #[test]
    fn symlink_state_reports_wrong_target() {
        let scratch = LinkScratch::new("wrong");
        let real = scratch.path("not-apd");
        fs::write(&real, b"x").unwrap();
        let link = scratch.path("apd");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        match symlink_target_state(&link) {
            SymlinkState::Wrong(found) => assert!(
                found.contains("not-apd"),
                "detail must name the actual target: {found}"
            ),
            other => panic!("expected Wrong, got {other:?}"),
        }
    }

    /// A dangling symlink exists but resolves nowhere: `Wrong`, not `Missing`
    /// (the remediation differs — "recreate the link", not "deploy never ran").
    #[test]
    fn symlink_state_reports_dangling_as_wrong() {
        let scratch = LinkScratch::new("dangling");
        let link = scratch.path("apd");
        std::os::unix::fs::symlink(scratch.path("gone"), &link).unwrap();
        assert_eq!(
            symlink_target_state(&link),
            SymlinkState::Wrong("<dangling>".to_string())
        );
    }

    /// A regular file (not a link) at that path is `Wrong`, not `Correct`:
    /// deploy installs a *symlink* to the daemon, and a stale copy would run
    /// outdated code.
    #[test]
    fn symlink_state_rejects_a_plain_file() {
        let scratch = LinkScratch::new("plainfile");
        let path = scratch.path("apd");
        fs::write(&path, b"#!/bin/sh\n").unwrap();
        match symlink_target_state(&path) {
            SymlinkState::Wrong(found) => assert!(found.contains("apd"), "{found}"),
            other => panic!("a plain file must not count as the daemon link: {other:?}"),
        }
    }
}
