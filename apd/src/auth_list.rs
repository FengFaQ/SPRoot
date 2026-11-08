//! Authorisation table for SPRoot (contract v1.1 §3.2/§3.3, E3/E4).
//!
//! The table is an AP-managed **userspace** file; the kernel never sees it
//! (non-goal N1: "授权表 = AP 用户态强制 + UI 审计，内核不参与"). It reuses
//! APatch's `package_config` csv layout verbatim (`pkg,exclude,allow,uid,
//! to_uid,sctx`), but v1.1 E4 removed the separate `exclude_list` side file:
//! `exclude` is a row field of the single [`defs::ALLOW_LIST_FILE`].
//!
//! Consistency model (contract §3.2):
//! * **single writer** — the resident daemon (uid-listener thread, `auth`
//!   subcommands, socket sessions) owns every mutation, serialised by an
//!   exclusive `flock` in [`package::ConfigLock`];
//! * readers do not lock: writes go through tmp+rename, so a reader sees the
//!   old or the new complete generation, never a torn one;
//! * the historical AP-side safety semantics are preserved 1:1 (torn-read
//!   fuse, empty-packages.list fuse, uid 0/2000 protection).
//!
//! Authorisation decision sources (v1.1 E3, replaces the never-implementable
//! `SP_REQUEST_UID` kernel env injection):
//! * third-party app → daemon socket `SO_PEERCRED` (`daemon.rs`) → [`is_authorized`];
//! * manager path → `root-exec` already root → not tabled;
//! * hidden-directory `su` → root context only → not tabled.

use std::os::unix::io::AsRawFd;

use anyhow::{bail, Result};
use log::{error, info, warn};

use crate::package::{self, ConfigLock, PackageConfig};

/// uids the refresh must never revoke (upstream behaviour, contract §3.2).
/// 0 = root itself, 2000 = adb shell.
const PROTECTED_UIDS: [i32; 2] = [0, 2000];

fn is_protected(uid: i32) -> bool {
    PROTECTED_UIDS.contains(&uid)
}

/// Hold the exclusive advisory lock for the whole read-modify-write cycle.
pub fn lock_for_write() -> ConfigLock {
    ConfigLock::acquire()
}

/// Synchronise the on-disk table with the system package list, then prune
/// invalid rows. Same contract as upstream `refresh_ap_package_list`, minus the
/// kernel (and with the v1.1 uid-prune wiring that was missing from the
/// intermediate SK port):
///
/// 1. prune rows whose package vanished + refresh uids (`synchronize_package_uid`);
/// 2. refuse to rewrite when the config read was torn **and** non-critical
///    grants exist — a fusing rewrite would wipe the table (same protection as
///    upstream's kernel-side deny);
/// 3. a valid-but-empty config is an explicit "revoke everything" and proceeds
///    (the file becomes header-only).
pub fn refresh_allow_list() {
    let _guard = lock_for_write();

    // Step 1: prune uninstalled packages / refresh uids. Errors here are
    // fail-open by design (an empty/blank packages.list is a torn read and must
    // not be allowed to wipe the table) — the local fuse below still applies.
    if let Err(e) = package::synchronize_package_uid_locked() {
        warn!("[refresh_allow_list] package sync skipped: {e}");
    }

    let read = package::read_ap_package_config_validated();
    let configs = &read.configs;

    let non_critical_grants = configs
        .iter()
        .filter(|c| c.allow == 1 && c.exclude == 0 && !is_protected(c.uid))
        .count();

    if !read.valid && non_critical_grants > 0 {
        error!(
            "[refresh_allow_list] allow_list unreadable but {} non-critical \
             grant(s) exist; aborting rewrite to avoid wiping the table",
            non_critical_grants
        );
        return;
    }
    if !read.valid {
        warn!(
            "[refresh_allow_list] allow_list unreadable, proceeding with {} \
             parsed entrie(s)",
            configs.len()
        );
    }

    // Drop obviously invalid rows (negative uids) so the committed table is clean.
    let clean: Vec<PackageConfig> = configs
        .iter()
        .filter(|c| {
            if c.uid < 0 || c.to_uid < 0 {
                warn!(
                    "[refresh_allow_list] dropping invalid row {} uid={}",
                    c.pkg, c.uid
                );
                false
            } else {
                true
            }
        })
        .cloned()
        .collect();

    if clean.len() != configs.len() {
        if let Err(e) = package::write_ap_package_config(&clean) {
            error!("[refresh_allow_list] rewrite failed: {e}");
            return;
        }
        info!(
            "[refresh_allow_list] pruned {} invalid row(s), table committed",
            configs.len() - clean.len()
        );
    } else {
        info!(
            "[refresh_allow_list] table consistent, {} entrie(s)",
            clean.len()
        );
    }
}

/// Authorisation check for an incoming su session (contract §3.2 "授权执行点").
///
/// `request_uid = Some(uid)`: real requester identity from the daemon socket's
/// `SO_PEERCRED` (kernel-filled, unspoofable).
/// `request_uid = None`: root-context invocation (`/data/<key16>/<rand>` or
/// `bin/su` executed by an already-root caller, or AP itself) → allow, because
/// the file is 0700 root and only root can reach it.
pub fn authorize(request_uid: Option<i32>) -> bool {
    match request_uid {
        Some(uid) => is_authorized(uid),
        None => crate::supercall::is_root(),
    }
}

/// Critical uids always pass; everyone else must have an explicit
/// `allow == 1 && exclude == 0` row.
pub fn is_authorized(uid: i32) -> bool {
    if is_protected(uid) {
        return true;
    }
    let read = package::read_ap_package_config_validated();
    if !read.valid {
        // Fail-closed for the *auth decision*: refusing to decide would silently
        // deny; upstream's kernel refused the syscall, so visible behaviour matches.
        warn!("[is_authorized] table unreadable, denying uid {uid}");
        return false;
    }
    read.configs
        .iter()
        .any(|c| c.uid == uid && c.allow == 1 && c.exclude == 0)
}

/// Per-uid module exclusion bit (contract §3.6 as amended by E4: same file).
pub fn is_excluded(uid: i32) -> bool {
    let read = package::read_ap_package_config_validated();
    read.configs
        .iter()
        .any(|c| c.uid == uid && c.exclude == 1)
}

/// Read the whole table (manager UI / `auth list`).
pub fn list() -> Result<Vec<PackageConfig>> {
    let read = package::read_ap_package_config_validated();
    if !read.valid {
        bail!("allow_list unreadable");
    }
    Ok(read.configs)
}

fn load_for_write() -> Result<Vec<PackageConfig>> {
    let read = package::read_ap_package_config_validated();
    if !read.valid {
        // Never rewrite from a torn snapshot (same fuse as refresh).
        bail!("allow_list unreadable, refusing to write");
    }
    Ok(read.configs)
}

/// Grant (or re-enable) root for `uid`. All writers take the exclusive lock.
pub fn grant(uid: i32, pkg: &str, to_uid: i32, sctx: &str) -> Result<()> {
    let _lock = lock_for_write();
    let mut configs = load_for_write()?;
    match configs.iter_mut().find(|c| c.uid == uid) {
        Some(c) => {
            c.allow = 1;
            c.exclude = 0;
            if !pkg.is_empty() {
                c.pkg = pkg.to_string();
            }
            c.to_uid = to_uid;
            if !sctx.is_empty() {
                c.sctx = sctx.to_string();
            }
        }
        None => configs.push(PackageConfig {
            pkg: pkg.to_string(),
            exclude: 0,
            allow: 1,
            uid,
            to_uid,
            sctx: sctx.to_string(),
        }),
    }
    package::write_ap_package_config(&configs)?;
    info!("[auth] granted uid {uid}");
    Ok(())
}

/// Revoke root for `uid` (keeps the row so the manager UI can still show it).
pub fn revoke(uid: i32) -> Result<()> {
    let _lock = lock_for_write();
    let mut configs = load_for_write()?;
    match configs.iter_mut().find(|c| c.uid == uid) {
        Some(c) => c.allow = 0,
        None => warn!("[auth] revoke: uid {uid} has no row"),
    }
    package::write_ap_package_config(&configs)?;
    info!("[auth] revoked uid {uid}");
    Ok(())
}

/// Set the per-uid module-exclusion bit (replaces the removed kernel kstorage).
pub fn set_exclude(uid: i32, excluded: bool) -> Result<()> {
    let _lock = lock_for_write();
    let mut configs = load_for_write()?;
    let value = i32::from(excluded);
    match configs.iter_mut().find(|c| c.uid == uid) {
        Some(c) => c.exclude = value,
        None => configs.push(PackageConfig {
            pkg: String::new(),
            exclude: value,
            allow: 0,
            uid,
            to_uid: 0,
            sctx: String::new(),
        }),
    }
    package::write_ap_package_config(&configs)?;
    info!("[auth] uid {uid} exclude={excluded}");
    Ok(())
}

/// `auth list --json` output: the table as a JSON array (no key material).
pub fn list_json() -> Result<String> {
    Ok(serde_json::to_string_pretty(&list()?)?)
}

/// Touch the lock file once after deploy so its mode is deterministic and the
/// daemon's very first refresh does not race with file creation.
pub fn prepare_storage() -> Result<()> {
    let guard = lock_for_write();
    if let Ok(file) = std::fs::File::options()
        .write(true)
        .create(true)
        .truncate(false)
        .open(crate::defs::ALLOW_LIST_LOCK_FILE)
    {
        unsafe {
            libc::flock(file.as_raw_fd(), libc::LOCK_UN);
        }
    }
    drop(guard);
    Ok(())
}
