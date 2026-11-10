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

#[derive(Serialize)]
struct Check {
    name: &'static str,
    ok: bool,
    critical: bool,
    detail: String,
}

impl Check {
    fn new(name: &'static str, ok: bool, critical: bool, detail: String) -> Self {
        Check {
            name,
            ok,
            critical,
            detail,
        }
    }

    /// Placeholder for a deployment-dependent check while nothing is deployed.
    fn pending(name: &'static str, detail: &str) -> Self {
        Check::new(name, true, false, format!("not deployed yet ({detail})"))
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
        checks.push(Check::new(
            "avb_untouched",
            state
                .as_ref()
                .map(|s| s.avb == "vbmeta_untouched")
                .unwrap_or(false),
            true,
            format!(
                "audit={:?} (no vbmeta/AVB operation must ever be performed)",
                state.as_ref().map(|s| s.avb.clone())
            ),
        ));
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
            println!(
                "[{}] {}{} — {}",
                if c.ok { "ok" } else { "FAIL" },
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
}
