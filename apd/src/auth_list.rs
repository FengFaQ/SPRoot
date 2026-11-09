//! Three-state root authorisation store for SPRoot (NEW-DOC-05 §13).
//!
//! The AP-managed table is a **single JSON document**
//! ([`defs::AUTH_LIST_FILE`], 0600, atomic tmp+rename) with one row per uid and
//! an explicit tri-state [`AuthState`]: `allow | deny | ask`. `ask` (the
//! default for an app nobody decided about) means "ask the user"; interactive
//! prompting is phase 2, so this round the daemon **refuses** an `ask` request
//! with an explicit `ask_pending` decision and records an audit event — it is
//! never silently treated as `allow` (that would be a privilege escalation by
//! omission).
//!
//! ## Migration (upgrade must not lose grants)
//!
//! The legacy csv ([`defs::ALLOW_LIST_FILE`], `pkg,exclude,allow,uid,to_uid,
//! sctx`) is imported **once**, on the first load after the upgrade, when the
//! JSON store does not exist yet: `allow == 1 && exclude == 0` → `allow`, every
//! other persisted row → `deny` (exactly what the old pure whitelist meant).
//! The legacy file is never written again; it stays on disk as a read-only
//! record. Contract v1.1 §3.2 named the csv (`E4`); the JSON store supersedes
//! it as the decision source — recorded as errata **E6** in NEW-DOC-05 §13.
//!
//! ## Audit log
//!
//! [`defs::AUTH_LOG_FILE`] is an append-only JSONL (0600, capped by
//! [`defs::AUTH_LOG_MAX_BYTES`]). Every line carries only the **sanitised
//! command digest** (`cmd_name` + `cmd_args` + a 16-hex SHA-256 prefix): the
//! full command line, arguments and any key material are never written.
//!
//! ## Consistency
//!
//! One writer at a time via the shared `flock` ([`package::ConfigLock`]) on
//! [`defs::ALLOW_LIST_LOCK_FILE`]; writes are tmp+rename so readers (no lock)
//! only ever see a complete generation. A torn/unreadable table denies (fail
//! closed) and is never rewritten from the torn snapshot.

use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
    sync::Mutex,
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use log::{error, info, warn};
use serde::{Deserialize, Serialize};

use crate::{
    defs,
    package::{self, ConfigLock},
    utils,
};

/// On-disk schema version of the JSON store (bump only with a migration).
const TABLE_VERSION: u32 = 1;

/// uids the table must never deny (upstream behaviour, contract v1.1 §3.2).
/// 0 = root itself, 2000 = adb shell.
const PROTECTED_UIDS: [i32; 2] = [0, 2000];

/// Number of newest events kept when the log is trimmed.
const LOG_KEEP_LINES: usize = 500;

/// In-process serialisation of log appends; cross-process appends only ever
/// add a single small line with `O_APPEND`, so no flock is needed here (taking
/// the store lock would deadlock when a caller already holds it).
static LOG_MUTEX: Mutex<()> = Mutex::new(());

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// Per-uid authorisation state (contract: `allow | deny | ask`).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AuthState {
    Allow,
    Deny,
    Ask,
}

impl AuthState {
    pub fn as_str(self) -> &'static str {
        match self {
            AuthState::Allow => "allow",
            AuthState::Deny => "deny",
            AuthState::Ask => "ask",
        }
    }

    /// Strict parser (lowercase only) so a typo in the CLI/API is rejected
    /// instead of silently becoming a default.
    pub fn parse(value: &str) -> Result<Self> {
        match value.trim() {
            "allow" => Ok(AuthState::Allow),
            "deny" => Ok(AuthState::Deny),
            "ask" => Ok(AuthState::Ask),
            other => bail!("invalid auth state '{other}' (expected: allow|deny|ask)"),
        }
    }
}

/// Outcome of an authorisation decision. Deliberately distinct from
/// [`AuthState`]: `AskPending` is the *result* for an `ask` row (the user has
/// not answered yet), not the stored state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
    AskPending,
}

impl Decision {
    pub fn as_str(self) -> &'static str {
        match self {
            Decision::Allow => "allow",
            Decision::Deny => "deny",
            Decision::AskPending => "ask_pending",
        }
    }
}

/// One row of the authorisation table (also the `auth list --json` element).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AuthEntry {
    pub uid: i32,
    /// Package name; `null` when the uid could not be resolved.
    #[serde(default)]
    pub package: Option<String>,
    /// Application label. Always `null` from the AP side: labels/images need
    /// `PackageManager`, i.e. the manager app resolves them (kept in the ABI so
    /// the UI schema is stable).
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub app_icon_path: Option<String>,
    #[serde(default = "default_state")]
    pub state: AuthState,
    /// Epoch seconds of the last granted / refused request (`null` = never).
    #[serde(default)]
    pub last_grant_at: Option<u64>,
    #[serde(default)]
    pub last_deny_at: Option<u64>,
    #[serde(default)]
    pub request_count: u64,
    /// Root-mapping options carried over from the legacy table.
    #[serde(default)]
    pub to_uid: i32,
    #[serde(default)]
    pub sctx: String,
    /// Module-exclusion flag (contract v1.1 E4: was a csv column).
    #[serde(default)]
    pub exclude: bool,
}

impl AuthEntry {
    fn new(uid: i32) -> Self {
        AuthEntry {
            uid,
            package: None,
            label: None,
            app_icon_path: None,
            state: AuthState::Ask,
            last_grant_at: None,
            last_deny_at: None,
            request_count: 0,
            to_uid: 0,
            sctx: String::new(),
            exclude: false,
        }
    }

    fn touch_grant(&mut self, now: u64) {
        self.last_grant_at = Some(now);
    }

    fn touch_deny(&mut self, now: u64) {
        self.last_deny_at = Some(now);
    }
}

fn default_state() -> AuthState {
    AuthState::Ask
}

/// On-disk document: `{"version":1,"entries":[...]}`.
#[derive(Serialize, Deserialize, Clone, Debug)]
struct AuthStore {
    #[serde(default = "table_version")]
    version: u32,
    #[serde(default)]
    entries: Vec<AuthEntry>,
}

fn table_version() -> u32 {
    TABLE_VERSION
}

/// One audit-log line (never contains a full command or key material).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AuthEvent {
    /// Epoch seconds (machine friendly).
    pub ts: u64,
    /// Same instant as UTC ISO-8601 (`YYYY-MM-DDThh:mm:ssZ`), for the UI.
    pub iso: String,
    pub uid: i32,
    #[serde(default)]
    pub package: Option<String>,
    /// `request | set_state | revoke | migrate | grant`
    pub event: String,
    /// Table state at decision time (`allow|deny|ask`), when applicable.
    #[serde(default)]
    pub state: Option<String>,
    /// `allow|deny|ask_pending` for request events.
    #[serde(default)]
    pub decision: Option<String>,
    /// Previous state for mutation events.
    #[serde(default)]
    pub previous: Option<String>,
    /// Sanitised digest of the requested command: executable basename only…
    #[serde(default)]
    pub cmd_name: Option<String>,
    /// …argument count only (never the arguments themselves)…
    #[serde(default)]
    pub cmd_args: Option<usize>,
    /// …and a 16-hex SHA-256 prefix for correlation/audit.
    #[serde(default)]
    pub cmd_sha256: Option<String>,
    #[serde(default)]
    pub detail: Option<String>,
}

/// Result of a mutation (`auth set` / `auth revoke` JSON output).
#[derive(Serialize, Clone, Debug)]
pub struct Change {
    pub uid: i32,
    pub state: String,
    #[serde(default)]
    pub previous: String,
    #[serde(default)]
    pub package: Option<String>,
    #[serde(default)]
    pub changed: bool,
}

/// Snapshot for `selftest` (never fails; `valid == false` means unreadable).
#[derive(Clone, Copy, Debug, Default)]
pub struct AuthStats {
    pub entries: usize,
    pub allow: usize,
    pub deny: usize,
    pub ask: usize,
    pub valid: bool,
    /// True when the store still originates from a legacy csv import.
    pub migrated_legacy: bool,
}

// ---------------------------------------------------------------------------
// Pure helpers (unit tested)
// ---------------------------------------------------------------------------

fn is_protected(uid: i32) -> bool {
    PROTECTED_UIDS.contains(&uid)
}

/// The decision rule, isolated so the three states can be unit tested without
/// touching `/data`:
///
/// * protected uids (root, adb) → `Allow`;
/// * unreadable table → `Deny` (fail closed, same visible behaviour as the old
///   kernel-side refusal);
/// * `allow` → `Allow`, `deny` → `Deny`, `ask` → `AskPending`;
/// * **no row → `AskPending`** (default `ask`, never an implicit allow).
fn decide(protected: bool, table_valid: bool, state: Option<AuthState>) -> Decision {
    if protected {
        return Decision::Allow;
    }
    if !table_valid {
        return Decision::Deny;
    }
    match state {
        Some(AuthState::Allow) => Decision::Allow,
        Some(AuthState::Deny) => Decision::Deny,
        Some(AuthState::Ask) | None => Decision::AskPending,
    }
}

/// Convert the legacy whitelist rows into tri-state entries.
///
/// The old table granted root only for `allow == 1 && exclude == 0`, so:
/// granted rows become `allow`, every other persisted row becomes `deny`
/// (explicitly revoked apps stay revoked), and `exclude`/`to_uid`/`sctx` are
/// carried over. Duplicate uids keep the first row (later duplicates were
/// already ambiguous in the legacy file).
fn migrate_legacy(rows: &[package::PackageConfig]) -> Vec<AuthEntry> {
    let mut entries: BTreeMap<i32, AuthEntry> = BTreeMap::new();
    for row in rows {
        if row.uid < 0 || row.to_uid < 0 {
            warn!("[auth] migration skips invalid row uid={}", row.uid);
            continue;
        }
        if entries.contains_key(&row.uid) {
            warn!("[auth] migration skips duplicate uid={}", row.uid);
            continue;
        }
        let mut entry = AuthEntry::new(row.uid);
        entry.package = Some(row.pkg.clone()).filter(|pkg| !pkg.is_empty());
        entry.state = if row.allow == 1 && row.exclude == 0 {
            AuthState::Allow
        } else {
            AuthState::Deny
        };
        entry.to_uid = row.to_uid;
        entry.sctx = row.sctx.clone();
        entry.exclude = row.exclude == 1;
        entries.insert(row.uid, entry);
    }
    entries.into_values().collect()
}

/// Sanitised command digest: `(basename, arg_count, sha256_prefix_16)`.
///
/// The command line and its arguments are **never** stored; only a whitelisted
/// executable name (≤48 chars), the argument count and a hash prefix, so two
/// audit lines can be correlated without leaking what was run.
fn sanitize_command(cmd: &str) -> (Option<String>, usize, Option<String>) {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return (None, 0, None);
    }
    let mut parts = trimmed.split_whitespace();
    let first = parts.next().unwrap_or_default();
    let arg_count = parts.count();
    let base = first.rsplit('/').next().unwrap_or(first);
    let name: String = base
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
        .take(48)
        .collect();
    let name = if name.is_empty() { None } else { Some(name) };
    let digest = utils::sha256_hex(trimmed.as_bytes());
    (name, arg_count, Some(digest.chars().take(16).collect()))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// UTC ISO-8601 without pulling in a date crate.
fn iso8601(ts: u64) -> String {
    let days = (ts / 86_400) as i64;
    let rem = ts % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

/// Howard Hinnant's `civil_from_days` (proleptic Gregorian, days since epoch).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

// ---------------------------------------------------------------------------
// Store I/O
// ---------------------------------------------------------------------------

struct Loaded {
    entries: BTreeMap<i32, AuthEntry>,
    valid: bool,
    migrated: bool,
}

fn store_exists() -> bool {
    Path::new(defs::AUTH_LIST_FILE).is_file()
}

/// Read the store; when it does not exist yet, import the legacy csv once.
/// Never writes (callers that mutate persist the result).
fn load_store() -> Loaded {
    match fs::read_to_string(defs::AUTH_LIST_FILE) {
        Ok(text) => match serde_json::from_str::<AuthStore>(&text) {
            Ok(store) => Loaded {
                entries: store.entries.into_iter().map(|e| (e.uid, e)).collect(),
                valid: true,
                migrated: false,
            },
            Err(e) => {
                // Torn/corrupt document: fail closed, never rewrite from it.
                error!(
                    "[auth] {} unreadable ({e}), denying until repaired",
                    defs::AUTH_LIST_FILE
                );
                Loaded {
                    entries: BTreeMap::new(),
                    valid: false,
                    migrated: false,
                }
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            // First run after the tri-state upgrade: one-way csv import.
            let legacy = package::read_ap_package_config_validated();
            if legacy.valid && !legacy.configs.is_empty() {
                let entries: Vec<AuthEntry> = migrate_legacy(&legacy.configs);
                info!(
                    "[auth] migrated {} legacy csv row(s) from {} into {}",
                    entries.len(),
                    defs::ALLOW_LIST_FILE,
                    defs::AUTH_LIST_FILE
                );
                Loaded {
                    entries: entries.into_iter().map(|e| (e.uid, e)).collect(),
                    valid: true,
                    migrated: true,
                }
            } else {
                Loaded {
                    entries: BTreeMap::new(),
                    valid: true,
                    migrated: false,
                }
            }
        }
        Err(e) => {
            error!(
                "[auth] {} not readable ({e}), denying until repaired",
                defs::AUTH_LIST_FILE
            );
            Loaded {
                entries: BTreeMap::new(),
                valid: false,
                migrated: false,
            }
        }
    }
}

/// Atomic write: same-directory tmp (0600 from creation) → fsync → rename.
fn write_atomic(path: &str, tmp: &str, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = Path::new(path).parent() {
        fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(tmp)
        .with_context(|| format!("create {tmp}"))?;
    file.write_all(bytes)?;
    file.set_permissions(fs::Permissions::from_mode(0o600))?;
    file.sync_all()?;
    fs::rename(tmp, path).with_context(|| format!("rename into {path}"))?;
    Ok(())
}

fn save(entries: &BTreeMap<i32, AuthEntry>) -> Result<()> {
    let store = AuthStore {
        version: TABLE_VERSION,
        entries: entries.values().cloned().collect(),
    };
    let text = serde_json::to_string_pretty(&store)?;
    write_atomic(defs::AUTH_LIST_FILE, defs::AUTH_LIST_TMP_FILE, text.as_bytes())
}

/// Package name for a uid, resolved from `/data/system/packages.list`.
/// `None` (never an error) when the mapping is unavailable.
fn package_for_uid(uid: i32) -> Option<String> {
    package::read_system_packages()?
        .into_iter()
        .find(|(_, candidate)| *candidate == uid)
        .map(|(pkg, _)| pkg)
}

fn reload_entries() -> Result<BTreeMap<i32, AuthEntry>> {
    let loaded = load_store();
    ensure!(
        loaded.valid,
        "auth table unreadable ({}), refusing to write",
        defs::AUTH_LIST_FILE
    );
    Ok(loaded.entries)
}

// ---------------------------------------------------------------------------
// Audit log
// ---------------------------------------------------------------------------

fn append_event(event: &AuthEvent) {
    let Ok(line) = serde_json::to_string(event) else {
        return;
    };
    let _guard = LOG_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let path = Path::new(defs::AUTH_LOG_FILE);
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    match OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
    {
        // A single small `write_all` per line; `O_APPEND` keeps concurrent
        // appends from overwriting each other.
        Ok(mut file) => {
            let _ = file.write_all(line.as_bytes());
            let _ = file.write_all(b"\n");
        }
        Err(e) => warn!("[auth] audit log append failed: {e}"),
    }
}

fn record_event(
    uid: i32,
    package: Option<String>,
    event: &str,
    state: Option<AuthState>,
    decision: Option<Decision>,
    previous: Option<AuthState>,
    detail: Option<String>,
) {
    let ts = now_secs();
    append_event(&AuthEvent {
        ts,
        iso: iso8601(ts),
        uid,
        package,
        event: event.to_string(),
        state: state.map(AuthState::as_str).map(str::to_string),
        decision: decision.map(Decision::as_str).map(str::to_string),
        previous: previous.map(AuthState::as_str).map(str::to_string),
        cmd_name: None,
        cmd_args: None,
        cmd_sha256: None,
        detail,
    });
}

/// Rewrite the log down to the newest [`LOG_KEEP_LINES`] events. Best effort;
/// caller either holds the store lock or accepts a racy trim (entries are
/// append-only, so worst case one event is lost during a concurrent boot).
fn trim_log_if_needed() {
    let path = Path::new(defs::AUTH_LOG_FILE);
    let Ok(meta) = fs::metadata(path) else {
        return;
    };
    if meta.len() <= defs::AUTH_LOG_MAX_BYTES {
        return;
    }
    let _guard = LOG_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(text) = fs::read_to_string(path) else {
        return;
    };
    let lines: Vec<&str> = text.lines().filter(|line| !line.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(LOG_KEEP_LINES);
    let mut trimmed = String::new();
    for line in &lines[start..] {
        trimmed.push_str(line);
        trimmed.push('\n');
    }
    match write_atomic(defs::AUTH_LOG_FILE, defs::AUTH_LOG_TMP_FILE, trimmed.as_bytes()) {
        Ok(()) => info!(
            "[auth] log trimmed: {} kept of {} event(s)",
            lines.len() - start,
            lines.len()
        ),
        Err(e) => warn!("[auth] log trim failed: {e:#}"),
    }
}

// ---------------------------------------------------------------------------
// Public API — decisions
// ---------------------------------------------------------------------------

/// Authorisation check for an incoming root request.
///
/// `request_uid = Some(uid)`: real requester identity (daemon socket
/// `SO_PEERCRED`, kernel-filled and unspoofable).
/// `request_uid = None`: root-context invocation (`/data/<key16>/<rand>` or
/// `bin/su` executed by an already-root caller) → allow when the caller really
/// is root (the file is 0700 and reaching it already requires root).
pub fn authorize(request_uid: Option<i32>) -> Decision {
    match request_uid {
        Some(uid) => {
            if is_protected(uid) {
                return Decision::Allow;
            }
            let loaded = load_store();
            if !loaded.valid {
                return Decision::Deny;
            }
            decide(false, true, loaded.entries.get(&uid).map(|e| e.state))
        }
        None => {
            if crate::supercall::is_root() {
                Decision::Allow
            } else {
                Decision::Deny
            }
        }
    }
}

/// Increment `request_count` / touch `last_grant_at` / `last_deny_at` and append
/// exactly one `request` audit event. Called once per real request by the
/// daemon (the `apd.rs` fallback path does not call it, so a socket session is
/// never double counts).
pub fn record_request(uid: i32, decision: Decision, cmd: Option<&str>) {
    let package = package_for_uid(uid);
    let (cmd_name, cmd_args, cmd_sha256) = match cmd {
        Some(command) => sanitize_command(command),
        None => (None, 0, None),
    };
    let ts = now_secs();

    if !is_protected(uid) {
        let _lock = ConfigLock::acquire();
        let loaded = load_store();
        if loaded.valid {
            let mut entries = loaded.entries;
            let entry = entries.entry(uid).or_insert_with(|| {
                let mut fresh = AuthEntry::new(uid);
                fresh.package = package.clone();
                fresh
            });
            if entry.package.is_none() {
                entry.package = package.clone();
            }
            entry.request_count = entry.request_count.saturating_add(1);
            match decision {
                Decision::Allow => entry.touch_grant(ts),
                Decision::Deny | Decision::AskPending => entry.touch_deny(ts),
            }
            if let Err(e) = save(&entries) {
                error!("[auth] request bookkeeping failed: {e:#}");
            }
        } else {
            warn!("[auth] table unreadable, request not counted (uid {uid})");
        }
    }

    // One event per request; the decision string distinguishes deny/ask_pending.
    append_event(&AuthEvent {
        ts,
        iso: iso8601(ts),
        uid,
        package,
        event: "request".to_string(),
        state: None,
        decision: Some(decision.as_str().to_string()),
        previous: None,
        cmd_name,
        cmd_args: Some(cmd_args),
        cmd_sha256,
        detail: None,
    });
}

// ---------------------------------------------------------------------------
// Public API — table queries
// ---------------------------------------------------------------------------

/// The whole table, uid-sorted, with package names filled in when they can be
/// resolved (unknown packages stay `null`, never an error).
pub fn list() -> Result<Vec<AuthEntry>> {
    let loaded = load_store();
    ensure!(
        loaded.valid,
        "auth table unreadable ({})",
        defs::AUTH_LIST_FILE
    );
    let packages = package::read_system_packages();
    let mut entries: Vec<AuthEntry> = loaded.entries.into_values().collect();
    for entry in &mut entries {
        if entry.package.is_none()
            && let Some(packages) = packages.as_ref()
            && let Some((pkg, _)) = packages.iter().find(|(_, uid)| *uid == entry.uid)
        {
            entry.package = Some(pkg.clone());
        }
    }
    entries.sort_by_key(|entry| entry.uid);
    Ok(entries)
}

/// `apd auth list --json`: a bare JSON array (stable ABI for the manager UI).
pub fn list_json() -> Result<String> {
    Ok(serde_json::to_string_pretty(&list()?)?)
}

/// `apd auth log --json --limit N`: newest-first event array.
pub fn log_events(limit: usize) -> Result<Vec<AuthEvent>> {
    let text = match fs::read_to_string(defs::AUTH_LOG_FILE) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => bail!("auth log unreadable ({}): {e}", defs::AUTH_LOG_FILE),
    };
    let mut events: Vec<AuthEvent> = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<AuthEvent>(line) {
            Ok(event) => events.push(event),
            Err(e) => warn!("[auth] skipping malformed audit line: {e}"),
        }
    }
    let limit = limit.max(1);
    let start = events.len().saturating_sub(limit);
    let mut tail: Vec<AuthEvent> = events.split_off(start);
    tail.reverse();
    Ok(tail)
}

/// JSON form of [`log_events`].
pub fn log_json(limit: usize) -> Result<String> {
    Ok(serde_json::to_string_pretty(&log_events(limit)?)?)
}

/// Snapshot for `selftest --json` (total failure is reported via `valid`).
pub fn stats() -> AuthStats {
    let loaded = load_store();
    let mut stats = AuthStats {
        entries: loaded.entries.len(),
        valid: loaded.valid,
        migrated_legacy: loaded.migrated || (!store_exists() && !loaded.entries.is_empty()),
        ..AuthStats::default()
    };
    if loaded.valid {
        for entry in loaded.entries.values() {
            match entry.state {
                AuthState::Allow => stats.allow += 1,
                AuthState::Deny => stats.deny += 1,
                AuthState::Ask => stats.ask += 1,
            }
        }
    }
    stats
}

// ---------------------------------------------------------------------------
// Public API — mutations (single writer, flock protected)
// ---------------------------------------------------------------------------

/// Mutate the state under the write lock and persist — **no audit event**.
///
/// Kept separate so `set_state` and `revoke` each emit exactly one event
/// (`revoke` must not look like `set_state` + `revoke` in the UI history).
fn apply_state(uid: i32, state: AuthState) -> Result<Change> {
    ensure!(uid >= 0, "uid must be >= 0 (got {uid})");
    let package = package_for_uid(uid);
    let _lock = ConfigLock::acquire();
    let mut entries = reload_entries()?;
    let now = now_secs();
    let existed = entries.contains_key(&uid);
    let previous;
    match entries.get_mut(&uid) {
        Some(entry) => {
            previous = entry.state;
            entry.state = state;
            if entry.package.is_none() {
                entry.package = package.clone();
            }
            if state == AuthState::Allow {
                entry.touch_grant(now);
            }
        }
        None => {
            previous = AuthState::Ask;
            let mut entry = AuthEntry::new(uid);
            entry.package = package.clone();
            entry.state = state;
            if state == AuthState::Allow {
                entry.touch_grant(now);
            }
            entries.insert(uid, entry);
        }
    }
    save(&entries)?;
    let package = entries.get(&uid).and_then(|entry| entry.package.clone());
    Ok(Change {
        uid,
        state: state.as_str().to_string(),
        previous: previous.as_str().to_string(),
        package,
        // Creating a row is a change even when the new state equals the default.
        changed: !existed || previous != state,
    })
}

/// Set the tri-state of `uid` (creates the row with default `ask` + resolved
/// package when it does not exist yet) and record one `set_state` event.
pub fn set_state(uid: i32, state: AuthState) -> Result<Change> {
    let change = apply_state(uid, state)?;
    record_event(
        uid,
        change.package.clone(),
        "set_state",
        Some(state),
        None,
        AuthState::parse(&change.previous).ok(),
        None,
    );
    info!(
        "[auth] uid {uid}: {} -> {}",
        change.previous, change.state
    );
    Ok(change)
}

/// Legacy CLI: grant root (equivalent to `set --state allow`, keeping the
/// root-mapping options).
pub fn grant(uid: i32, pkg: &str, to_uid: i32, sctx: &str) -> Result<Change> {
    ensure!(uid >= 0, "uid must be >= 0 (got {uid})");
    ensure!(to_uid >= 0, "to_uid must be >= 0 (got {to_uid})");
    let _lock = ConfigLock::acquire();
    let mut entries = reload_entries()?;
    let now = now_secs();
    let previous;
    let entry = entries.entry(uid).or_insert_with(|| {
        let mut fresh = AuthEntry::new(uid);
        fresh.package = package_for_uid(uid);
        fresh
    });
    previous = entry.state;
    entry.state = AuthState::Allow;
    entry.exclude = false;
    if !pkg.is_empty() {
        entry.package = Some(pkg.to_string());
    } else if entry.package.is_none() {
        entry.package = package_for_uid(uid);
    }
    entry.to_uid = to_uid;
    if !sctx.is_empty() {
        entry.sctx = sctx.to_string();
    }
    entry.touch_grant(now);
    let package = entry.package.clone();
    save(&entries)?;
    record_event(
        uid,
        package.clone(),
        "grant",
        Some(AuthState::Allow),
        None,
        Some(previous),
        None,
    );
    info!("[auth] granted uid {uid}");
    Ok(Change {
        uid,
        state: AuthState::Allow.as_str().to_string(),
        previous: previous.as_str().to_string(),
        package,
        changed: previous != AuthState::Allow,
    })
}

/// Revoke root for `uid` (equals `set --state deny`; the row is kept so the
/// manager can still show the app). Emits a single `revoke` event.
pub fn revoke(uid: i32) -> Result<Change> {
    let change = apply_state(uid, AuthState::Deny)?;
    record_event(
        uid,
        change.package.clone(),
        "revoke",
        Some(AuthState::Deny),
        None,
        AuthState::parse(&change.previous).ok(),
        None,
    );
    info!("[auth] revoked uid {uid}");
    Ok(change)
}

/// Set the per-uid module-exclusion bit (replaces the removed kernel kstorage).
pub fn set_exclude(uid: i32, excluded: bool) -> Result<()> {
    ensure!(uid >= 0, "uid must be >= 0 (got {uid})");
    let _lock = ConfigLock::acquire();
    let mut entries = reload_entries()?;
    let package = package_for_uid(uid);
    match entries.get_mut(&uid) {
        Some(entry) => {
            entry.exclude = excluded;
            if entry.package.is_none() {
                entry.package = package;
            }
        }
        None => {
            let mut entry = AuthEntry::new(uid);
            entry.exclude = excluded;
            entry.package = package;
            entries.insert(uid, entry);
        }
    }
    save(&entries)?;
    info!("[auth] uid {uid} exclude={excluded}");
    Ok(())
}

// ---------------------------------------------------------------------------
// Public API — maintenance
// ---------------------------------------------------------------------------

/// Synchronise the JSON table with the system package list (replaces the csv
/// `refresh_ap_package_list`): prune entries whose package was uninstalled and
/// refresh uids after a package was reinstalled with a new uid.
///
/// Fuses preserved from the csv implementation:
/// * an unreadable/torn `packages.list` **never** prunes (fail open to the
///   existing table, so a torn write cannot wipe grants);
/// * an unreadable JSON store is never rewritten (fail closed for decisions).
pub fn refresh_allow_list() {
    let _lock = ConfigLock::acquire();
    let loaded = load_store();
    if !loaded.valid {
        error!("[auth] table unreadable, refresh aborted (refusing to wipe)");
        return;
    }
    let mut entries = loaded.entries;
    let before = entries.len();

    if let Some(packages) = package::read_system_packages() {
        let by_pkg: BTreeMap<String, i32> = packages.into_iter().collect();
        let mut pruned: BTreeMap<i32, AuthEntry> = BTreeMap::new();
        let mut dropped = 0usize;
        let mut moved = 0usize;
        for (uid, mut entry) in entries {
            let package = entry.package.clone();
            match package.as_deref() {
                Some(pkg) => match by_pkg.get(pkg) {
                    Some(&new_uid) => {
                        if new_uid != uid {
                            if pruned.contains_key(&new_uid) {
                                warn!("[auth] uid collision on reinstall: {pkg} -> {new_uid}, keeping existing row");
                                continue;
                            }
                            entry.uid = new_uid;
                            moved += 1;
                            info!("[auth] uid refresh: {pkg} {uid} -> {new_uid}");
                        }
                        pruned.insert(entry.uid, entry);
                    }
                    None => {
                        dropped += 1;
                        info!("[auth] dropping uid {uid} ({pkg} uninstalled)");
                    }
                },
                // Uid-only rows (manual/legacy) have no package to validate.
                None => {
                    pruned.insert(uid, entry);
                }
            }
        }
        entries = pruned;
        if dropped > 0 || moved > 0 {
            info!("[auth] refresh: dropped={dropped} moved={moved}");
        }
    } else {
        warn!("[auth] packages.list unreadable/empty, skipping prune (fail-open)");
    }

    if loaded.migrated || entries.len() != before {
        if let Err(e) = save(&entries) {
            error!("[auth] refresh save failed: {e:#}");
            return;
        }
    }
    trim_log_if_needed();
    info!("[auth] refresh done: {} entrie(s)", entries.len());
}

/// Create the JSON store / log with deterministic modes and persist a pending
/// legacy migration. Called by `deploy` (step 8) and by the daemon at startup.
pub fn prepare_storage() -> Result<()> {
    let _lock = ConfigLock::acquire();
    let loaded = load_store();
    if !loaded.valid {
        bail!(
            "auth table unreadable ({}); deploy must repair it",
            defs::AUTH_LIST_FILE
        );
    }
    if loaded.migrated || !store_exists() {
        save(&loaded.entries)?;
        if loaded.migrated {
            record_event(
                0,
                None,
                "migrate",
                None,
                None,
                None,
                Some(format!(
                    "legacy csv imported: {} row(s) (allow=1 && exclude=0 -> allow, others -> deny)",
                    loaded.entries.len()
                )),
            );
        }
    }
    let log = Path::new(defs::AUTH_LOG_FILE);
    if !log.exists() {
        write_atomic(defs::AUTH_LOG_FILE, defs::AUTH_LOG_TMP_FILE, b"")?;
    }
    fs::set_permissions(log, fs::Permissions::from_mode(0o600)).ok();
    trim_log_if_needed();
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::PackageConfig;

    fn row(pkg: &str, uid: i32, allow: i32, exclude: i32) -> PackageConfig {
        PackageConfig {
            pkg: pkg.to_string(),
            exclude,
            allow,
            uid,
            to_uid: 0,
            sctx: String::new(),
        }
    }

    /// The upgrade must not lose a single grant: legacy `allow==1 && exclude==0`
    /// rows stay allowed, revoked rows stay denied (they must not silently flip
    /// to the new `ask` default).
    #[test]
    fn migration_keeps_grants_and_denials() {
        let rows = vec![
            row("com.granted.app", 10100, 1, 0),
            row("com.revoked.app", 10101, 0, 0),
            row("com.excluded.app", 10102, 0, 1),
            // already granted AND excluded is not a grant in the legacy meaning
            row("com.granted.excluded", 10103, 1, 1),
        ];
        let migrated = migrate_legacy(&rows);
        assert_eq!(migrated.len(), 4);
        // Migration never invents an allow or an ask.
        assert!(migrated.iter().all(|e| e.state != AuthState::Ask));
        let by_uid: BTreeMap<i32, AuthEntry> = migrated.into_iter().map(|e| (e.uid, e)).collect();
        assert_eq!(by_uid[&10100].state, AuthState::Allow);
        assert_eq!(by_uid[&10100].package.as_deref(), Some("com.granted.app"));
        assert_eq!(by_uid[&10101].state, AuthState::Deny);
        assert_eq!(by_uid[&10102].state, AuthState::Deny);
        assert!(by_uid[&10102].exclude);
        assert_eq!(by_uid[&10103].state, AuthState::Deny);
        assert!(by_uid[&10103].exclude);
    }

    /// Three-state decision table. The critical negative: an unknown uid (no
    /// row) is `ask`, **never** `allow`; and `ask` is refused this round.
    #[test]
    fn decide_is_three_state_and_never_implicit_allow() {
        assert_eq!(decide(true, true, None), Decision::Allow); // protected uid
        assert_eq!(decide(false, true, Some(AuthState::Allow)), Decision::Allow);
        assert_eq!(decide(false, true, Some(AuthState::Deny)), Decision::Deny);
        assert_eq!(
            decide(false, true, Some(AuthState::Ask)),
            Decision::AskPending
        );
        assert_eq!(decide(false, true, None), Decision::AskPending);
        // An unreadable table must deny, not fall through to ask/allow.
        assert_eq!(decide(false, false, None), Decision::Deny);
        assert_eq!(decide(false, false, Some(AuthState::Allow)), Decision::Deny);
    }

    /// Invalid state strings are rejected instead of defaulting.
    #[test]
    fn auth_state_parse_rejects_unknown() {
        assert_eq!(AuthState::parse("allow").unwrap(), AuthState::Allow);
        assert_eq!(AuthState::parse(" deny ").unwrap(), AuthState::Deny);
        assert_eq!(AuthState::parse("ask").unwrap(), AuthState::Ask);
        for bad in ["Allow", "grant", "allowed", "", "0", "yes"] {
            assert!(
                AuthState::parse(bad).is_err(),
                "state '{bad}' must be rejected"
            );
        }
    }

    /// The audit digest must never contain the command line or its arguments.
    #[test]
    fn command_digest_is_redacted() {
        let cmd = "/system/bin/sh -c 'cat /data/adb/sproot/key'";
        let (name, args, digest) = sanitize_command(cmd);
        assert_eq!(name.as_deref(), Some("sh"));
        // Whitespace tokens after the executable: `-c`, `'cat`, `path'`.
        assert_eq!(args, 3);
        let digest = digest.expect("digest");
        assert_eq!(digest.len(), 16);
        assert!(digest.chars().all(|c| c.is_ascii_hexdigit()));
        // Nothing of the command text survives in the event fields.
        for field in [name.unwrap(), digest] {
            assert!(!field.contains("cat"));
            assert!(!field.contains("key"));
            assert!(!field.contains("/system/bin"));
        }
        // Deterministic and distinct for a different command.
        let again = sanitize_command(cmd);
        assert_eq!(again.1, args);
        assert_eq!(again.2, sanitize_command(cmd).2);
        assert_ne!(
            sanitize_command("id").2,
            sanitize_command("id -u").2
        );
    }

    /// Serialisation ABI: `state` is a lowercase string, optional fields are
    /// explicit `null`, and the entry round-trips.
    #[test]
    fn entry_serialisation_roundtrip() {
        let mut entry = AuthEntry::new(10123);
        entry.state = AuthState::Allow;
        entry.request_count = 3;
        entry.last_grant_at = Some(1_762_670_000);
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"state\":\"allow\""));
        assert!(json.contains("\"package\":null"));
        assert!(json.contains("\"label\":null"));
        let back: AuthEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back.uid, 10123);
        assert_eq!(back.state, AuthState::Allow);
        assert_eq!(back.request_count, 3);
    }

    /// Log timestamps: UTC ISO-8601 derived without a date crate.
    #[test]
    fn iso8601_matches_known_epochs() {
        assert_eq!(iso8601(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(iso8601(1_762_670_000), "2025-11-09T06:33:20Z");
    }
}
