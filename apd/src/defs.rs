use const_format::concatcp;

pub const ADB_DIR: &str = "/data/adb/";
pub const WORKING_DIR: &str = concatcp!(ADB_DIR, "ap/");
pub const BINARY_DIR: &str = concatcp!(WORKING_DIR, "bin/");
pub const APATCH_LOG_FOLDER: &str = concatcp!(WORKING_DIR, "log/");

pub const AP_RC_PATH: &str = concatcp!(WORKING_DIR, ".aprc");
pub const GLOBAL_NAMESPACE_FILE: &str = concatcp!(ADB_DIR, ".global_namespace_enable");
/// Daemon path. The binary keeps the APatch-era path so module scripts, the
/// manager and the injected init rc all reference one stable location.
pub const DAEMON_PATH: &str = concatcp!(ADB_DIR, "apd");

pub const MODULE_DIR: &str = concatcp!(ADB_DIR, "modules/");

// warning: this directory should not change, or you need to change the code in module_installer.sh!!!
pub const MODULE_UPDATE_DIR: &str = concatcp!(ADB_DIR, "modules_update/");

pub const TEMP_DIR: &str = "/debug_ramdisk";
pub const TEMP_DIR_LEGACY: &str = "/sbin";

pub const MODULE_WEB_DIR: &str = "webroot";
pub const MODULE_ACTION_SH: &str = "action.sh";
pub const DISABLE_FILE_NAME: &str = "disable";
pub const UPDATE_FILE_NAME: &str = "update";
pub const REMOVE_FILE_NAME: &str = "remove";

// Metamodule support
pub const METAMODULE_MOUNT_SCRIPT: &str = "metamount.sh";
pub const METAMODULE_METAINSTALL_SCRIPT: &str = "metainstall.sh";
pub const METAMODULE_METAUNINSTALL_SCRIPT: &str = "metauninstall.sh";
pub const METAMODULE_DIR: &str = concatcp!(ADB_DIR, "metamodule/");

// Module config
pub const MODULE_CONFIG_DIR: &str = concatcp!(WORKING_DIR, "module_configs/");
/// Per-module install state (audit trail written by `module install --json`).
pub const MODULE_STATE_DIR: &str = concatcp!(WORKING_DIR, "module_state/");
pub const PERSIST_CONFIG_NAME: &str = "persist.config";
pub const TEMP_CONFIG_NAME: &str = "tmp.config";

pub const PTS_NAME: &str = "pts";

// ---------------------------------------------------------------------------
// SPRoot (SK) private state. Contract v1.1 §4 + NEW-DOC-05 §3.1.3: the key
// file is 0600 root-only and the only carrier of the root key outside the
// patched kernel. Content = 47-char escalation token, plain text, no NUL.
// ---------------------------------------------------------------------------
pub const SPROOT_DIR: &str = concatcp!(ADB_DIR, "sproot/");
/// Canonical key file — the exact path the injected `init.sproot.rc` passes to
/// `--superkey-file` (NEW-DOC-05 §3.1.3). 0600 root:root. The AP-side content
/// is the 47-char escalation token as plain text (no NUL); the kernel-side
/// shellcode format (47 chars + NUL) is a different thing and is never this
/// file. `secret` still strips a stray NUL on read, defensively.
pub const SPROOT_KEY_FILE: &str = concatcp!(SPROOT_DIR, "key");
/// Audit copy of the rc template written by `deploy`; the ramdisk
/// `/init.sproot.rc` is the copy init actually consumes.
pub const SPROOT_RC_FILE: &str = concatcp!(SPROOT_DIR, "init.sproot.rc");
/// Presence means safe mode (contract v1.1 §3.5, third detection path).
pub const SPROOT_SAFEMODE_FILE: &str = concatcp!(SPROOT_DIR, "safemode");
/// Boot-loop guard counter (NEW-DOC-05 §5.5): incremented on every
/// post-fs-data, cleared on boot-completed; >= 2 forces safe mode.
pub const SPROOT_BOOT_COUNT_FILE: &str = concatcp!(SPROOT_DIR, "boot_count");
/// Deployment state, machine readable, never contains the key or its prefix.
pub const SPROOT_STATE_FILE: &str = concatcp!(SPROOT_DIR, "state.json");

// ---------------------------------------------------------------------------
// AVB audit (2026-11-10 S1 fix). The previous `avb_untouched` check wrote the
// literal `"vbmeta_untouched"` into state.json and then compared it back to the
// same literal — a self-referential assertion that could never fail (the exact
// AVB-3 anti-pattern recorded in HANDOFF-2026-11-08 §"教训"). The replacement
// reads the real boot partition: AVB footer magic + the vbmeta section digest.
// ---------------------------------------------------------------------------
/// Boot partition block devices probed for the AVB footer, in priority order.
/// `boot_a` covers A/B devices whose `by-name/boot` link is absent; the
/// `bootdevice`/`platform/soc` forms cover pre-GKI vendor layouts.
pub const AVB_BOOT_CANDIDATES: [&str; 4] = [
    "/dev/block/by-name/boot",
    "/dev/block/bootdevice/by-name/boot",
    "/dev/block/by-name/boot_a",
    "/dev/block/bootdevice/by-name/boot_a",
];
/// Diagnostic/test override: point the AVB auditor at a regular file (an
/// unpacked `boot.img` or a copy of the partition). Written by nobody in
/// production; when it is in effect the check detail says so, so an overridden
/// run can never be mistaken for a real device measurement.
pub const AVB_BOOT_OVERRIDE_ENV: &str = "SPROOT_AVB_BOOT_IMAGE";

// ---------------------------------------------------------------------------
// Authorisation table (contract v1.1 §3.2/§3.6, E4): ONE csv file
// (pkg,exclude,allow,uid,to_uid,sctx). `exclude` is a row field — the separate
// `exclude_list` file of v1.0 §3.6 is deliberately NOT implemented.
// ---------------------------------------------------------------------------
pub const ALLOW_LIST_FILE: &str = concatcp!(WORKING_DIR, "allow_list");
pub const ALLOW_LIST_TMP_FILE: &str = concatcp!(WORKING_DIR, "allow_list.tmp");
/// flock target serialising every writer (daemon, uid-listener, `auth` CLI).
pub const ALLOW_LIST_LOCK_FILE: &str = concatcp!(WORKING_DIR, "allow_list.lock");

/// Single-instance lock for the resident daemon (`apd daemon`).
pub const DAEMON_LOCK_FILE: &str = concatcp!(WORKING_DIR, "daemon.lock");

// ---------------------------------------------------------------------------
// Three-state authorisation store + audit log (NEW-DOC-05 §13, 2026-11-10).
// The legacy csv above stays on disk as the one-way **migration source**; the
// JSON document below is authoritative for every decision since the tri-state
// upgrade (`allow | deny | ask`). One writer (flock), tmp+rename for readers.
// ---------------------------------------------------------------------------
pub const AUTH_LIST_FILE: &str = concatcp!(SPROOT_DIR, "auth_list.json");
pub const AUTH_LIST_TMP_FILE: &str = concatcp!(SPROOT_DIR, "auth_list.json.tmp");
/// Append-only JSONL audit trail for grant/deny/ask events (0600, capped).
pub const AUTH_LOG_FILE: &str = concatcp!(SPROOT_DIR, "auth_log.jsonl");
pub const AUTH_LOG_TMP_FILE: &str = concatcp!(SPROOT_DIR, "auth_log.jsonl.tmp");
/// Over this size the log is rewritten down to the newest 500 events.
pub const AUTH_LOG_MAX_BYTES: u64 = 256 * 1024;

// ---------------------------------------------------------------------------
// APatch / Zygisk compatibility layer (external ecosystem signal source).
//
// NeoZygisk (GPL-3.0) and OnyxZygisk (AGPL-3.0) integrate with APatch through
// exactly two signals (`zygiskd/src/root_impl/apatch.rs`):
//   1. `apd -V` → the **second whitespace token** is parsed as `i32` and must be
//      >= `minAPatchVersion = 10762` (NeoZygisk build.gradle.kts:21);
//   2. `/data/adb/ap/package_config` → csv `pkg,exclude,allow,uid,to_uid,sctx`;
//      a row with `exclude == 1` means "exclude from modifications" (umount).
//
// SPRoot therefore deliberately advertises APatch lineage for that ecosystem.
// **This is a compatibility layer, not a bug** (Lead 2026-11-10, human-approved
// direction): the authoritative authorisation store remains
// `auth_list.json`; the csv below is a derived, apd-written artifact only.
// ---------------------------------------------------------------------------
/// Numeric APatch version advertised to Zygisk implementations. Must stay
/// >= `minAPatchVersion` (10762) or NeoZygisk reports `TooOld` and aborts.
pub const APATCH_COMPAT_VERSION: i32 = 10762;
/// `-V` version string: clap prints `<name> <version>` → `apd 10762`.
pub const APATCH_COMPAT_VERSION_STR: &str = "10762";
/// Derived APatch-compatible signal file read by NeoZygisk/OnyxZygisk.
pub const APATCH_PACKAGE_CONFIG: &str = concatcp!(WORKING_DIR, "package_config");
pub const APATCH_PACKAGE_CONFIG_TMP: &str = concatcp!(WORKING_DIR, "package_config.tmp");
/// Header must match the APatch csv layout byte for byte: external parsers rely
/// on positional columns (`parts[1]` = exclude, `parts[2]` = allow,
/// `parts[3]` = uid), so a missing/renamed column silently breaks them.
pub const APATCH_PACKAGE_CONFIG_HEADER: &str = "pkg,exclude,allow,uid,to_uid,sctx";

/// File holding the abstract unix socket name used by the su authorisation
/// channel (`@sproot-<12 hex>`). World readable so the client library can find
/// the daemon; the name itself carries no key material (contract v1.1 §0.4).
pub const AUTH_SOCK_FILE: &str = concatcp!(BINARY_DIR, ".su.sock");

/// Default timeout for a single module/common script run, seconds. Module
/// scripts run inline with init's post-fs-data, so an unbounded script is a
/// boot hang; the plan (NEW-DOC-05 §5.2/§5.5) requires a per-script budget.
pub const SCRIPT_TIMEOUT_SECS: u64 = 60;

/// Property polled by the daemon to detect boot completion (contract v1.1 §7:
/// userspace event engine — the SK kernel has no event source).
pub const BOOT_COMPLETED_PROP: &str = "sys.boot_completed";

pub const VERSION_CODE: &str = include_str!(concat!(env!("OUT_DIR"), "/VERSION_CODE"));
pub const VERSION_NAME: &str = include_str!(concat!(env!("OUT_DIR"), "/VERSION_NAME"));
/// Commit the binary was built from (`unknown` outside a git checkout). Used by
/// `version --json` so the manager can compare the APK-bundled libapd.so with
/// the deployed `/data/adb/apd` (NEW-DOC-05 §6.2 version consistency check).
pub const VERSION_COMMIT: &str = include_str!(concat!(env!("OUT_DIR"), "/VERSION_COMMIT"));
