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
use serde::Serialize;

use crate::{daemon, defs, deploy, package, secret, selinux};

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

fn is_executable(path: &Path) -> bool {
    mode_of(path).map(|m| m & 0o111 != 0).unwrap_or(false)
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
                        "mode={mode:o} len={len} alnum={alnum} mlocked={}",
                        key.is_locked()
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

    // ---- allow list ------------------------------------------------------
    let acl_check = if deployed {
        let read = package::read_ap_package_config_validated();
        Check::new(
            "allow_list",
            Path::new(defs::ALLOW_LIST_FILE).exists() && read.valid,
            true,
            format!(
                "{} mode={:?} entries={}",
                defs::ALLOW_LIST_FILE,
                mode_of(Path::new(defs::ALLOW_LIST_FILE)),
                read.configs.len()
            ),
        )
    } else {
        Check::pending("allow_list", "first-boot self-check")
    };
    checks.push(acl_check);

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
