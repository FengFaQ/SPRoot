//! `apd selftest [--json]` — capability / diagnostic self-check.
//!
//! Lead-approved P2 item: gives the manager an "is my deployment healthy?"
//! answer without shell spaghetti, and gives QA an automation surface. Every
//! check is read-only (the socket probe binds and immediately closes; it never
//! disturbs a running daemon).
//!
//! Exit code: 0 = all critical checks pass, 1 = at least one critical failure
//! (matches the CLI contract in CALL v1.1 §8.5).

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

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
}

fn mode_of(path: &str) -> Option<u32> {
    fs::metadata(path).ok().map(|m| m.permissions().mode() & 0o777)
}

fn exists(path: &str) -> bool {
    Path::new(path).exists()
}

pub fn run(json: bool) -> Result<()> {
    let mut checks: Vec<Check> = Vec::new();

    // ---- process identity ------------------------------------------------
    let uid = unsafe { libc::getuid() };
    checks.push(Check::new(
        "root",
        uid == 0,
        true,
        format!("uid={uid} (root-exec/deploy expect 0)"),
    ));

    // ---- deployed binary -------------------------------------------------
    let bin_ok = exists(defs::DAEMON_PATH)
        && mode_of(defs::DAEMON_PATH).map(|m| m & 0o111 != 0).unwrap_or(false);
    checks.push(Check::new(
        "binary",
        bin_ok,
        true,
        format!(
            "{} mode={:?} version={} commit={}",
            defs::DAEMON_PATH,
            mode_of(defs::DAEMON_PATH),
            defs::VERSION_NAME.trim(),
            defs::VERSION_COMMIT.trim()
        ),
    ));

    // ---- key file (0600, 47/48 alnum, lockable) ---------------------------
    let mut key_detail = "missing".to_string();
    let mut key_ok = false;
    if exists(defs::SPROOT_KEY_FILE) {
        let mode = mode_of(defs::SPROOT_KEY_FILE).unwrap_or(0);
        match secret::read_key_file(Path::new(defs::SPROOT_KEY_FILE)) {
            Some(key) => {
                let len = key.len();
                let alnum = key.as_str().chars().all(|c| c.is_ascii_alphanumeric());
                key_ok = mode == 0o600 && (len == 47 || len == 48) && alnum && key.is_locked();
                key_detail = format!(
                    "mode={mode:o} len={len} alnum={alnum} mlocked={}",
                    key.is_locked()
                );
            }
            None => key_detail = format!("mode={mode:o} unreadable/empty"),
        }
    }
    checks.push(Check::new("key_file", key_ok, true, key_detail));
    checks.push(Check::new(
        "mlock_supported",
        secret::mlock_supported(),
        false,
        "platform allows locking key pages".to_string(),
    ));

    // ---- allow list ------------------------------------------------------
    let read = package::read_ap_package_config_validated();
    let acl_mode = mode_of(defs::ALLOW_LIST_FILE).unwrap_or(0);
    checks.push(Check::new(
        "allow_list",
        exists(defs::ALLOW_LIST_FILE) && read.valid,
        true,
        format!("{} mode={acl_mode:o} entries={}", defs::ALLOW_LIST_FILE, read.configs.len()),
    ));

    // ---- hidden su -------------------------------------------------------
    let su_link = format!("{}su", defs::BINARY_DIR);
    let su_ok = (|| {
        let link = fs::symlink_metadata(&su_link).ok()?;
        if !link.file_type().is_symlink() {
            return None;
        }
        let target = fs::read_link(&su_link).ok()?;
        let meta = fs::metadata(&target).ok()?;
        Some(meta.permissions().mode() & 0o777 == 0o700)
    })()
    .unwrap_or(false);
    checks.push(Check::new(
        "hidden_su",
        su_ok,
        true,
        format!("{su_link} -> target (0700 expected)"),
    ));

    // ---- directory modes -------------------------------------------------
    for (name, path, want) in [
        ("dir_sproot", defs::SPROOT_DIR, 0o700u32),
        ("dir_ap", defs::WORKING_DIR, 0o700),
        ("dir_bin", defs::BINARY_DIR, 0o755),
    ] {
        let mode = mode_of(path).unwrap_or(0);
        checks.push(Check::new(
            name,
            mode == want,
            false,
            format!("{path} mode={mode:o} want={want:o}"),
        ));
    }

    // ---- daemon + socket -------------------------------------------------
    let pid = daemon::daemon_pid();
    let alive = pid
        .map(|p| Path::new(&format!("/proc/{p}")).exists())
        .unwrap_or(false);
    checks.push(Check::new(
        "daemon",
        alive,
        false,
        format!("pid={pid:?} alive={alive}"),
    ));

    let sock_path = Path::new(defs::AUTH_SOCK_FILE);
    let sock_name = fs::read_to_string(sock_path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let sock_state = match &sock_name {
        Some(name) => match daemon::probe_abstract_bind(name) {
            Ok(()) => format!("@{name} bindable -> daemon NOT listening"),
            Err(e) if e.raw_os_error() == Some(libc::EADDRINUSE) => {
                format!("@{name} in use -> daemon listening")
            }
            Err(e) => format!("@{name} probe failed: {e}"),
        },
        None => "socket name file missing/unreadable".to_string(),
    };
    checks.push(Check::new(
        "auth_socket",
        sock_name.is_some(),
        false,
        sock_state,
    ));

    // ---- boot-loop guard -------------------------------------------------
    let boot_count = fs::read_to_string(defs::SPROOT_BOOT_COUNT_FILE)
        .ok()
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "-".to_string());
    let safemode = exists(defs::SPROOT_SAFEMODE_FILE);
    checks.push(Check::new(
        "bootstate",
        true,
        false,
        format!("boot_count={boot_count} safemode={safemode}"),
    ));

    // ---- deploy state / version consistency / AVB audit ------------------
    let state = deploy::read_state();
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
        state.as_ref().map(|s| s.avb == "vbmeta_untouched").unwrap_or(false),
        true,
        format!(
            "audit={:?} (no vbmeta/AVB operation must ever be performed)",
            state.as_ref().map(|s| s.avb.clone())
        ),
    ));

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
    if json {
        let payload = serde_json::json!({ "ok": critical_ok, "checks": checks });
        println!("{}", serde_json::to_string_pretty(&payload)?);
    } else {
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

