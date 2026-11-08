//! Authorisation table for SPRoot (contract §3.2/§3.3, asset
//! a_43f9e853a06b5109 v4).
//!
//! The table is an AP-managed **userspace** file; the kernel never sees it
//! (non-goal N1: "授权表 = AP 用户态强制 + UI 审计，内核不参与"). It reuses
//! APatch's `package_config` csv verbatim (pkg,exclude,allow,uid,to_uid,sctx)
//! so files written by the manager keep working with zero migration.
//!
//! Consistency model (contract §3.2, ruling 2):
//! * **single writer** — only the uid-listener rewrites the table, under an
//!   exclusive flock on `package_config.lock`;
//! * readers do not lock: writes go through tmp+rename, so a reader either
//!   sees the old or the new complete generation, never a torn one;
//! * the AP-side safety semantics of the original kernel round-trip are
//!   preserved 1:1 below (torn-read fuse, fail-open, uid 0/2000 protection).

use std::{
    fs::{File, OpenOptions},
    io,
    os::fd::AsRawFd,
};

use log::{error, info, warn};

use crate::package::{read_ap_package_config_validated, write_ap_package_config, PackageConfig};

/// uids the refresh must never revoke (upstream behaviour, contract §3.2).
/// 0 = root itself, 2000 = adb shell.
const PROTECTED_UIDS: [i32; 2] = [0, 2000];

fn is_protected(uid: i32) -> bool {
    PROTECTED_UIDS.contains(&uid)
}

/// Hold the exclusive advisory lock for the whole read-modify-write cycle.
/// The lock file is created lazily; flock is released when the guard drops.
pub struct AuthListWriteGuard {
    _file: File,
}

pub fn lock_for_write() -> io::Result<AuthListWriteGuard> {
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(crate::supercall::ALLOW_LIST_LOCK_FILE)?;
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if rc != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(AuthListWriteGuard { _file: file })
}

/// Synchronise the on-disk table with the freshly synchronised package list
/// (same contract as upstream `refresh_ap_package_list`, minus the kernel):
///
/// 1. prune rows whose package vanished (done by `synchronize_package_uid`);
/// 2. refuse to rewrite when the config read was torn **and** non-critical
///    grants exist — a fusing rewrite here used to wipe every grant in the
///    kernel; in the file model the same mistake would wipe the table;
/// 3. a valid-but-empty config is an explicit "revoke everything" and
///    proceeds (the file simply becomes header-only).
///
/// The "revoke then re-grant against the kernel" half of the upstream dance
/// has no counterpart: the file itself is the only source of truth, so
/// rewriting it *is* the commit.
pub fn refresh_allow_list() {
    let _guard = match lock_for_write() {
        Ok(g) => g,
        Err(e) => {
            error!("[refresh_allow_list] cannot acquire allow-list lock: {e}");
            return;
        }
    };

    let read = read_ap_package_config_validated();
    let configs = &read.configs;

    let non_critical_grants = configs
        .iter()
        .filter(|c| c.allow == 1 && c.exclude == 0 && !is_protected(c.uid))
        .count();

    if !read.valid && non_critical_grants > 0 {
        error!(
            "[refresh_allow_list] package_config unreadable but {} non-critical \
             grant(s) exist; aborting rewrite to avoid wiping the table",
            non_critical_grants
        );
        return;
    }
    if !read.valid {
        warn!(
            "[refresh_allow_list] package_config unreadable, proceeding with {} \
             parsed entrie(s)",
            configs.len()
        );
    }

    // Drop rows that synchronize_package_uid flagged for removal are already
    // pruned by it; drop obviously invalid rows (negative uids) here so the
    // table we commit is clean.
    let clean: Vec<PackageConfig> = configs
        .iter()
        .filter(|c| {
            if c.uid < 0 || c.to_uid < 0 {
                warn!("[refresh_allow_list] dropping invalid row {} uid={}", c.pkg, c.uid);
                false
            } else {
                true
            }
        })
        .cloned()
        .collect();

    if clean.len() != configs.len() {
        if let Err(e) = write_ap_package_config(&clean) {
            error!("[refresh_allow_list] rewrite failed: {e}");
            return;
        }
        info!(
            "[refresh_allow_list] pruned {} invalid row(s), table committed",
            configs.len() - clean.len()
        );
    } else {
        info!("[refresh_allow_list] table consistent, {} entrie(s)", clean.len());
    }
}

/// Authorisation check for an incoming su session (contract §3.2 "授权执行点
/// = AP root_shell() 查 allow_list 判定"). `uid` is the *requesting* uid.
/// Critical uids always pass; everyone else must have an explicit
/// `allow == 1 && exclude == 0` row.
pub fn is_authorized(uid: i32) -> bool {
    if is_protected(uid) {
        return true;
    }
    let read = read_ap_package_config_validated();
    if !read.valid {
        // Fail-closed for the *auth decision* (this is not the refresh path:
        // refusing to decide would silently deny; upstream's kernel refused
        // the syscall, so the visible behaviour matches).
        warn!("[is_authorized] table unreadable, denying uid {uid}");
        return false;
    }
    read.configs
        .iter()
        .any(|c| c.uid == uid && c.allow == 1 && c.exclude == 0)
}

/// Per-uid module exclusion bit (replaces kernel kstorage gid=1, contract
/// §3.6). Reads live here; writes go through the manager/AP config commands.
pub fn is_excluded(uid: i32) -> bool {
    let read = read_ap_package_config_validated();
    read.configs
        .iter()
        .any(|c| c.uid == uid && c.exclude == 1)
}
