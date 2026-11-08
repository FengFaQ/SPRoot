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
pub const PERSIST_CONFIG_NAME: &str = "persist.config";
pub const TEMP_CONFIG_NAME: &str = "tmp.config";

pub const PTS_NAME: &str = "pts";

// ---------------------------------------------------------------------------
// SPRoot (SK) private state. Contract v1.1 §4: the key file is 0600 root-only
// and the only carrier of the 48-char root key outside the patched kernel.
// ---------------------------------------------------------------------------
pub const SPROOT_DIR: &str = concatcp!(ADB_DIR, "sproot/");
pub const SPROOT_KEY_FILE: &str = concatcp!(SPROOT_DIR, "key");
/// Presence means safe mode (contract v1.1 §3.5, third detection path).
pub const SPROOT_SAFEMODE_FILE: &str = concatcp!(SPROOT_DIR, "safemode");
/// Boot-loop guard counter (NEW-DOC-05 §5.5): incremented on every
/// post-fs-data, cleared on boot-completed; >= 2 forces safe mode.
pub const SPROOT_BOOT_COUNT_FILE: &str = concatcp!(SPROOT_DIR, "boot_count");
/// Deployment state, machine readable, never contains the key or its prefix.
pub const SPROOT_STATE_FILE: &str = concatcp!(SPROOT_DIR, "state.json");

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
