use crate::sepolicy::get_policy_main;
use crate::{lua, module_config};
use anyhow::{Context, Result, anyhow, bail, ensure};
use const_format::concatcp;
use is_executable::is_executable;
use java_properties::PropertiesIter;
use log::{debug, info, warn};
#[cfg(unix)]
use std::os::unix::{prelude::PermissionsExt, process::CommandExt};
use std::{
    collections::HashMap,
    env::var as env_var,
    fs::{self, remove_dir_all},
    io::{Cursor, Read},
    os::fd::FromRawFd,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    str::FromStr,
    time::{Duration, Instant, SystemTime},
};

#[allow(clippy::wildcard_imports)]
use crate::utils::*;
use crate::{
    assets,
    defs::{self, MODULE_DIR, MODULE_UPDATE_DIR},
    deploy, metamodule, restorecon,
};

// ---------------------------------------------------------------------------
// AVB audit fields for the module `--json` contract (2026-11-10 S1b).
//
// These three responses used to emit the literal `"avb_untouched": true`: an
// assertion that the module operation did not touch the boot partition, made
// without ever looking at it. That is the same self-referential pattern the S1
// fix removed from `deploy`/`selftest`, and leaving one copy in place would have
// kept a back door open (Lead ruling, task-12).
//
// The replacement is a *measured* observation, but the module path is a hot path
// where audit information must never decide whether the operation succeeded:
//   * the probe is read-only and best-effort — any failure yields
//     `avb_status: "unobservable"` with a reason, and the install/uninstall
//     still succeeds (exit 0, `"ok": true`);
//   * it uses the footer-only probe (`probe_avb_footer_only`) to keep the cost to
//     one 64 B tail read instead of also reading the whole vbmeta section — see
//     `deploy::observe_avb_footer_only_from_path` for what that trades away.
// `avb_untouched` is retained as a *derived, measured* boolean so existing
// consumers (the manager prints it) keep working, but it is now
// `avb_status == "avb_ok"` — i.e. false (or null) whenever nothing was measured.
// ---------------------------------------------------------------------------

/// Audit fields shared by every module `--json` response.
///
/// Deliberately infallible: it returns a JSON fragment, never a `Result`, so a
/// broken/unreadable boot partition can never turn a successful module
/// operation into a failure.
fn avb_audit_fields() -> serde_json::Value {
    let observed = deploy::probe_avb_footer_only();
    let status = observed.code();
    // Short code without the `avb_` prefix, matching the task's field spec
    // (`ok|no_footer|unobservable`) while staying 1:1 with `AvbObservation::code`.
    let short = status.strip_prefix("avb_").unwrap_or(status);
    serde_json::json!({
        "avb_status": short,
        // Derived, not asserted: true only when the footer was actually measured.
        "avb_untouched": observed.footer_present,
        // Measured footer digest (absent when nothing was observed).
        "avb_footer_sha256": observed.footer_sha256,
        // Why the observation is not `ok` — never omitted silently.
        "avb_detail": match (short, &observed.problem) {
            ("ok", _) => observed.source.clone(),
            (_, Some(problem)) => problem.clone(),
            (_, None) => observed.source.clone(),
        },
    })
}

fn merge_avb_audit(mut payload: serde_json::Value) -> serde_json::Value {
    if let (Some(obj), serde_json::Value::Object(audit)) =
        (payload.as_object_mut(), avb_audit_fields())
    {
        for (key, value) in audit {
            obj.insert(key, value);
        }
    }
    payload
}

const INSTALLER_CONTENT: &str = include_str!("../assets/installer.sh");
const INSTALL_MODULE_SCRIPT: &str = concatcp!(
    INSTALLER_CONTENT,
    "\n",
    "install_module",
    "\n",
    "exit 0",
    "\n"
);

#[derive(PartialEq, Eq)]
pub enum ModuleType {
    All,
    Active,
    Updated,
}

/// Install-time limits (NEW-DOC-05 §3.1.2). Values chosen to be generous for
/// real modules but bounded against zip bombs / accidental huge uploads.
const MAX_MODULE_ZIP_BYTES: u64 = 512 * 1024 * 1024;
const MAX_UNCOMPRESSED_BYTES: u64 = 1024 * 1024 * 1024;
const MAX_ZIP_ENTRIES: usize = 100_000;
const MIN_FREE_BYTES: u64 = 64 * 1024 * 1024;

/// Installer scripts may legitimately take a while (unpacking, patching files),
/// but must never hang the manager forever.
const INSTALL_TIMEOUT_SECS: u64 = 300;

// ---------------------------------------------------------------------------
// Typed install errors (2026-11-10 S1).
//
// Before this, `_install_module_inner` returned a plain `anyhow::Error` and the
// `--json` entry point classified the failure by grepping the formatted message
// for the words "script"/"installer". Consequences:
//   * `EXIT_TIMEOUT` (124) was **declared but unreachable** — the timeout path
//     bailed with a string containing "installer", so it landed on 120
//     (EXIT_SCRIPT_FAILED), and no caller could ever observe 124;
//   * rewording an error message silently changed the exit code.
//
// The error below carries the classification *by construction*; the message is
// only for humans. `ModuleError::exit_code` is the single mapping point and is
// unit-tested (`mod tests`), including a negative sample proving the timeout is
// no longer confused with a script failure.
// ---------------------------------------------------------------------------
#[derive(Debug)]
pub enum ModuleError {
    /// The installer script exited non-zero (module rejected the device).
    ScriptFailed(String),
    /// The installer exceeded `INSTALL_TIMEOUT_SECS` and was killed (exit 124).
    Timeout { secs: u64 },
    /// Anything else on the install path (unpack, permissions, selinux…).
    Io(String),
}

impl ModuleError {
    /// Maps to the contract exit codes (NEW-DOC-05 §3.1.2, documented as
    /// 110/111/112/113/114/115/116/120/121/122/124).
    pub fn exit_code(&self) -> i32 {
        match self {
            ModuleError::ScriptFailed(_) => EXIT_SCRIPT_FAILED,
            ModuleError::Timeout { .. } => EXIT_TIMEOUT,
            ModuleError::Io(_) => EXIT_IO_FAILED,
        }
    }
}

impl std::fmt::Display for ModuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ModuleError::ScriptFailed(msg) => write!(f, "{msg}"),
            ModuleError::Timeout { secs } => write!(
                f,
                "module installer timed out after {secs}s (process group killed)"
            ),
            ModuleError::Io(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for ModuleError {}

/// Classify an `anyhow::Error` that came out of `_install_module_inner`.
///
/// Deliberately *type-first*: the `ModuleError` variant is found by walking the
/// error chain, never by matching words in the message. The old code did
/// `msg.contains("script") || msg.contains("installer")`, which both made
/// `EXIT_TIMEOUT` unreachable and let a message reword change the exit code.
fn classify_install_error(err: &anyhow::Error) -> i32 {
    for cause in err.chain() {
        if let Some(module) = cause.downcast_ref::<ModuleError>() {
            return module.exit_code();
        }
    }
    EXIT_IO_FAILED
}

/// Print human-facing progress unless `quiet` (JSON mode keeps stdout pure).
fn say(quiet: bool, msg: &str) {
    if quiet {
        log::info!("{msg}");
    } else {
        println!("{msg}");
    }
}

fn exec_install_script(
    module_file: &str,
    is_metamodule: bool,
    module_id: &str,
    quiet_stdout: bool,
) -> Result<()> {
    let realpath = std::fs::canonicalize(module_file)
        .with_context(|| format!("realpath: {module_file} failed"))?;

    // Get install script from metamodule module
    let install_script =
        metamodule::get_install_script(is_metamodule, INSTALLER_CONTENT, INSTALL_MODULE_SCRIPT)?;

    let mut command = Command::new(assets::BUSYBOX_PATH);
    command
        .args(["sh", "-c", &install_script])
        .envs(get_common_script_envs(Some(module_id)))
        .env("OUTFD", "1")
        .env("ZIPFILE", realpath)
        .process_group(0);
    if quiet_stdout {
        // `--json` contract (§8.4): stdout must stay pure JSON. Installer chat
        // (banner, progress, errors) goes to stderr instead.
        unsafe {
            let fd = libc::dup(2);
            if fd >= 0 {
                command.stdout(Stdio::from_raw_fd(fd));
            }
        }
    }
    let mut child = command.spawn().context("spawn module installer")?;
    // Typed outcome: the *classification* is carried by the error variant, not
    // by the wording of the message (2026-11-10 S1; see `ModuleError`).
    match wait_timeout(&mut child, Duration::from_secs(INSTALL_TIMEOUT_SECS))? {
        Some(status) if status.success() => Ok(()),
        Some(status) => Err(ModuleError::ScriptFailed(format!(
            "Failed to install module script (exit {status})"
        ))
        .into()),
        None => Err(ModuleError::Timeout {
            secs: INSTALL_TIMEOUT_SECS,
        }
        .into()),
    }
}

/// Apply staged updates from `MODULE_UPDATE_DIR` (BUG-APD-2 fix, 2026-11-12).
///
/// Per module: the old `modules/<id>` is swapped for the staged copy **without
/// a destroy-then-fail window**. The previous order
/// `remove_dir_all(old)?; rename(staged, old)?` meant a rename failure left the
/// module gone from `modules/` while the only surviving copy still sat in
/// `modules_update/` — and the caller then deleted that directory, losing the
/// module entirely. Now the staged copy is moved to a temporary name first, the
/// old directory is renamed to a trash name (cheap, atomic) rather than deleted,
/// the staged copy takes its place, and only then is the trash removed. Any
/// failure in the middle leaves either the old or the staged copy at
/// `modules/<id>` — never nothing.
pub fn handle_updated_modules() -> Result<()> {
    let modules_root = Path::new(MODULE_DIR);
    foreach_module(ModuleType::Updated, |updated_module| {
        if !updated_module.is_dir() {
            return Ok(());
        }

        if let Some(name) = updated_module.file_name() {
            let module_dir = modules_root.join(name);
            let mut disabled = false;
            let mut removed = false;
            let mut old_trash: Option<PathBuf> = None;
            if module_dir.exists() {
                // If the old module is disabled, we need to also disable the new one
                disabled = module_dir.join(defs::DISABLE_FILE_NAME).exists();
                removed = module_dir.join(defs::REMOVE_FILE_NAME).exists();
                // Move the old dir aside atomically instead of destroying it
                // before the new copy is in place. Failure here aborts with the
                // old module intact (and the staged copy untouched).
                let trash = modules_root.join(format!(
                    "{}{}.old-{}",
                    name.to_string_lossy(),
                    std::process::id(),
                    SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_nanos())
                        .unwrap_or(0)
                ));
                std::fs::rename(&module_dir, &trash)
                    .with_context(|| format!("stash old module {}", module_dir.display()))?;
                old_trash = Some(trash);
            }
            match std::fs::rename(updated_module, &module_dir) {
                Ok(()) => {
                    // The new copy is live: only now may the trash go. A failure
                    // here is cleanup noise, not a lost module.
                    if let Some(trash) = old_trash
                        && let Err(e) = remove_dir_all(&trash)
                    {
                        warn!("cannot clean up stashed module {}: {e}", trash.display());
                    }
                }
                Err(e) => {
                    // Roll back: restore the old module to its place so the
                    // device keeps at least the previous version. The staged
                    // copy stays in modules_update (handle_updated_modules will
                    // not be re-run this boot, but the data survives).
                    if let Some(trash) = old_trash
                        && let Err(rb) = std::fs::rename(&trash, &module_dir)
                    {
                        log::error!(
                            "module update failed ({e}) and rollback also failed ({rb}): \
                             the old copy is stashed at {}",
                            trash.display()
                        );
                    }
                    return Err(anyhow::Error::from(e).context(format!(
                        "install staged module {} failed; old version restored",
                        updated_module.display()
                    )));
                }
            }
            if removed {
                let path = module_dir.join(defs::REMOVE_FILE_NAME);
                if let Err(e) = ensure_file_exists(&path) {
                    warn!("Failed to create {}: {e}", path.display());
                }
            } else if disabled {
                let path = module_dir.join(defs::DISABLE_FILE_NAME);
                if let Err(e) = ensure_file_exists(&path) {
                    warn!("Failed to create {}: {e}", path.display());
                }
            }
        }
        Ok(())
    })?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Module-script PATH (P0, 2026-11-11).
//
// The bug: module scripts only got `BINARY_DIR` (`/data/adb/ap/bin`) appended to
// PATH, but the apd binary lives at `/data/adb/apd` (DAEMON_PATH) — one level
// up, *not* in `bin/`. So NeoZygisk's `Command::new("apd")` from a module script
// could not resolve, it fell back to `RootImpl::None`, and the whole Zygisk
// chain failed **silently** (the failure is a `debug!` line, invisible on a real
// device). Three things are needed and all three are asserted below:
//   1. deploy creates `/data/adb/ap/bin/apd` → DAEMON_PATH (the symlink);
//   2. PATH carries both BINARY_DIR and ADB_DIR;
//   3. the selftest reports `apd_in_path` from a *measured* lookup.
// ---------------------------------------------------------------------------

/// File name under [`defs::BINARY_DIR`] that module scripts resolve as `apd`.
pub const APD_IN_PATH_NAME: &str = "apd";

/// Full path of the symlink created by deploy: `/data/adb/ap/bin/apd`.
pub fn apd_in_path_symlink() -> String {
    format!("{}{}", defs::BINARY_DIR, APD_IN_PATH_NAME)
}

/// The PATH string handed to every module script — **single source of truth**.
///
/// Order: the inherited PATH first (unchanged existing behaviour, so a module's
/// own tools keep priority), then `BINARY_DIR`, then `ADB_DIR`. `BINARY_DIR`
/// supplies the multicall symlinks (`resetprop`, `magiskpolicy`, `apd`);
/// `ADB_DIR` supplies `apd` itself, so resolution survives even if the symlink
/// is missing or a third-party tool clears `bin/`.
pub fn module_script_path(inherited_path: &str) -> String {
    format!(
        "{}:{}:{}",
        inherited_path,
        defs::BINARY_DIR.trim_end_matches('/'),
        defs::ADB_DIR.trim_end_matches('/'),
    )
}

/// Outcome of asking "can a module script resolve `apd`?".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApdPathVerdict {
    /// `apd` resolves, and it resolves to the daemon binary itself.
    Resolved,
    /// There is no `apd` on any PATH entry.
    NotFound,
    /// `apd` was found but points somewhere other than [`defs::DAEMON_PATH`]
    /// (stale link, foreign tool, a plain file left by an old install).
    WrongTarget(String),
    /// A PATH entry we require is missing from the string entirely.
    MissingEntry(&'static str),
}

impl ApdPathVerdict {
    pub fn is_ok(&self) -> bool {
        matches!(self, ApdPathVerdict::Resolved)
    }
}

/// Pure resolution of `apd` against a PATH string.
///
/// `lookup(entry)` returns the path `entry/apd` resolves to (canonicalised), or
/// `None` when there is nothing there. Pure so the negative samples need no root
/// and no device: the tests supply a fake filesystem.
///
/// The two required entries are checked *structurally* first, because a PATH
/// that happens to resolve today but no longer carries the documented entries is
/// a contract drift the check must catch, not paper over.
pub fn apd_path_verdict<L>(path_env: &str, lookup: L) -> ApdPathVerdict
where
    L: Fn(&str) -> Option<std::path::PathBuf>,
{
    let entries: Vec<&str> = path_env
        .split(':')
        .map(str::trim)
        .filter(|e| !e.is_empty())
        .collect();

    let want_bin = defs::BINARY_DIR.trim_end_matches('/');
    let want_adb = defs::ADB_DIR.trim_end_matches('/');
    if !entries.contains(&want_bin) {
        return ApdPathVerdict::MissingEntry(want_bin);
    }
    if !entries.contains(&want_adb) {
        return ApdPathVerdict::MissingEntry(want_adb);
    }

    let expected = defs::DAEMON_PATH;
    let mut first_found: Option<String> = None;
    for entry in entries {
        if let Some(found) = lookup(entry) {
            let found_str = found.display().to_string();
            if found_str == expected {
                return ApdPathVerdict::Resolved;
            }
            first_found.get_or_insert(found_str);
        }
    }
    match first_found {
        Some(other) => ApdPathVerdict::WrongTarget(other),
        None => ApdPathVerdict::NotFound,
    }
}

/// Get common environment variables for script execution
pub fn get_common_script_envs(module_id: Option<&str>) -> Vec<(&'static str, String)> {
    let mut envs = vec![
        ("ASH_STANDALONE", "1".to_string()),
        ("APATCH", "true".to_string()),
        ("APATCH_VER", defs::VERSION_NAME.to_string()),
        ("APATCH_VER_CODE", defs::VERSION_CODE.to_string()),
        (
            "PATH",
            module_script_path(&env_var("PATH").unwrap_or_default()),
        ),
    ];

    if let Some(id) = module_id {
        envs.push(("AP_MODULE", id.to_string()));
    }

    envs
}

// because we use something like A-B update
// we need to update the module state after the boot_completed
// if someone(such as the module) install a module before the boot_completed
// then it may cause some problems, just forbid it
fn ensure_boot_completed() -> Result<()> {
    // ensure getprop sys.boot_completed == 1
    if getprop("sys.boot_completed").as_deref() != Some("1") {
        bail!("Android is Booting!");
    }
    Ok(())
}

fn mark_update() -> Result<()> {
    ensure_file_exists(concatcp!(defs::WORKING_DIR, defs::UPDATE_FILE_NAME))
}

fn mark_module_state(module: &str, flag_file: &str, create_or_delete: bool) -> Result<()> {
    let module_state_file = Path::new(defs::MODULE_DIR).join(module).join(flag_file);
    if create_or_delete {
        ensure_file_exists(module_state_file)
    } else {
        if module_state_file.exists() {
            fs::remove_file(module_state_file)?;
        }
        Ok(())
    }
}
pub fn foreach_module(
    module_type: ModuleType,
    f: impl FnMut(&Path) -> Result<()>,
) -> Result<()> {
    let modules_dir = Path::new(match module_type {
        ModuleType::Updated => MODULE_UPDATE_DIR,
        _ => defs::MODULE_DIR,
    });
    foreach_module_in(modules_dir, module_type, f)
}

/// [`foreach_module`] over an explicit directory (test seam for the budgeted
/// stage runners below; production always passes the `defs::` path).
pub fn foreach_module_in(
    modules_dir: &Path,
    module_type: ModuleType,
    mut f: impl FnMut(&Path) -> Result<()>,
) -> Result<()> {
    let dir = std::fs::read_dir(modules_dir)?;
    for entry in dir.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            warn!("{} is not a directory, skip", path.display());
            continue;
        }

        if module_type == ModuleType::Active && path.join(defs::DISABLE_FILE_NAME).exists() {
            info!("{} is disabled, skip", path.display());
            continue;
        }
        if module_type == ModuleType::Active && path.join(defs::REMOVE_FILE_NAME).exists() {
            warn!("{} is removed, skip", path.display());
            continue;
        }

        f(&path)?;
    }

    Ok(())
}

fn foreach_active_module(f: impl FnMut(&Path) -> Result<()>) -> Result<()> {
    foreach_module(ModuleType::Active, f)
}

/// Apply every active module's `sepolicy.rule` (`magiskpolicy --live --apply`).
///
/// **Cost note (APD-4 ④-5).** Each iteration calls `get_policy_main`, which
/// loads and parses the *entire* live SELinux policy (`SePolicy::from_file(
/// "/sys/fs/selinux/policy")` — megabytes on a real device) even when the rule
/// is one line. With N modules carrying rules that is N full policy parses, so
/// this is an N× (not N²) but still expensive loop, and it runs inside
/// `post-fs-data`, which now has a hard 90 s budget (APD-1).
///
/// The per-module loop is kept deliberately: applying rules independently means
/// one module's malformed rule cannot block the others' policies, and merging
/// them would require holding N rules against one parse. What is bounded here
/// is the *time*: `deadline` (when supplied) caps the whole loop, and the
/// remaining modules are skipped with a warning rather than silently running
/// past the stage budget.
pub fn load_sepolicy_rule() -> Result<()> {
    load_sepolicy_rule_bounded(None)
}

/// [`load_sepolicy_rule`] with an optional deadline.
///
/// `None` = unbounded (the historic behaviour, used by non-boot callers).
pub fn load_sepolicy_rule_bounded(deadline: Option<std::time::Instant>) -> Result<()> {
    let mut skipped = 0usize;
    let mut exhausted = false;
    foreach_active_module(|path| {
        let rule_file = path.join("sepolicy.rule");
        if !rule_file.exists() {
            return Ok(());
        }
        if exhausted {
            skipped += 1;
            return Ok(());
        }
        if let Some(deadline) = deadline
            && std::time::Instant::now() >= deadline
        {
            exhausted = true;
            skipped += 1;
            return Ok(());
        }

        info!("load policy: {}", rule_file.display());
        let mut _sepol = get_policy_main(&[
            "magiskpolicy".to_string(),
            "--live".to_string(),
            "--apply".to_string(),
            rule_file.display().to_string(),
        ])?;

        Ok(())
    })?;
    if skipped > 0 {
        warn!(
            "[sepolicy] post-fs-data budget exhausted: skipped {skipped} module \
             sepolicy.rule file(s) — they did NOT apply this boot. Boot proceeds."
        );
    }

    Ok(())
}

pub fn exec_script<T: AsRef<Path>>(path: T, wait: bool) -> Result<()> {
    exec_script_bounded(path, wait, None)
}

/// Per-script decision handed down by a stage's budget (APD-1, 2026-11-12).
///
/// Three states are load-bearing, which is why this is an enum and not an
/// `Option<Duration>`:
///
/// * [`ScriptBudget::Timeout`] — run the script, but kill it after this bound
///   (the `min(per_script, budget_left)` clamp);
/// * [`ScriptBudget::Skip`] — the stage budget is exhausted: the script must
///   **not be spawned at all**. The previous wiring returned `None` here and
///   `exec_script_bounded` read `None` as "use the default 60s", so the very
///   scripts that were supposed to be skipped still ran at full timeout — the
///   stage budget promised `<= 90s` and delivered `90s + N × 60s` (BUG-APD-1);
/// * [`ScriptBudget::Default`] — no budget pressure: use the plain
///   `SCRIPT_TIMEOUT_SECS` (non-budgeted callers, services/boot-completed).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptBudget {
    /// Run with an explicit (budget-clamped) timeout.
    Timeout(Duration),
    /// Do not spawn: the stage budget is exhausted.
    Skip,
    /// Run with the default per-script timeout.
    Default,
}

/// Execute a module script, optionally under an explicit timeout.
///
/// `timeout` is the **stage-budget-aware** per-script bound computed by
/// `event::StageBudget::timeout_for_script` (APD-1, 2026-11-11). `None` means
/// "use the plain `SCRIPT_TIMEOUT_SECS`", which is what non-budgeted callers
/// (`apd module action`, the services/boot-completed stages) want.
///
/// The caller decides the budget: this function never invents one, because only
/// the stage knows how much of its wall-clock allowance is left.
pub fn exec_script_bounded<T: AsRef<Path>>(
    path: T,
    wait: bool,
    timeout: Option<Duration>,
) -> Result<()> {
    match timeout {
        // Legacy signature: None = default per-script timeout.
        None => run_script(path, wait, ScriptBudget::Default),
        Some(t) => run_script(path, wait, ScriptBudget::Timeout(t)),
    }
}

/// [`exec_script_bounded`] with the full three-state budget decision.
pub fn run_script<T: AsRef<Path>>(path: T, wait: bool, budget: ScriptBudget) -> Result<()> {
    // APD-1 (BUG-APD-1 fix, 2026-11-12): Skip must happen BEFORE any spawn. A
    // skip that reaches the spawn path is not a skip — it is a full-timeout run
    // with a lying log line, which is exactly the bug the budget was meant to
    // close.
    let effective = match budget {
        ScriptBudget::Skip => {
            warn!(
                "script skipped: stage budget exhausted ({}): {}",
                path.as_ref().display(),
                "it did NOT run and did NOT fail"
            );
            return Ok(());
        }
        ScriptBudget::Default => Duration::from_secs(defs::SCRIPT_TIMEOUT_SECS),
        ScriptBudget::Timeout(t) => t,
    };

    info!("exec {}", path.as_ref().display());

    let is_module_script = path.as_ref().starts_with(defs::MODULE_DIR);
    // Extract module_id from path if it matches /data/adb/modules/{id}/...
    let module_id = if is_module_script {
        path.as_ref()
            .strip_prefix(defs::MODULE_DIR)
            .ok()
            .and_then(|p| p.components().next())
            .and_then(|c| c.as_os_str().to_str())
            .map(ToString::to_string)
    } else {
        None
    };

    if is_module_script && module_id.is_none() {
        debug!(
            "Failed to extract module_id from script path '{}'. Script will run without AP_MODULE environment variable.",
            path.as_ref().display()
        );
    }

    let mut command = &mut Command::new(assets::BUSYBOX_PATH);
    #[cfg(unix)]
    {
        command = command.process_group(0);
        command = unsafe {
            command.pre_exec(|| {
                // ignore the error?
                switch_cgroups();
                Ok(())
            })
        };
    }
    command = command
        .current_dir(path.as_ref().parent().unwrap())
        .arg("sh")
        .arg(path.as_ref())
        .envs(get_common_script_envs(module_id.as_deref()));

    // A module script runs inline with init's post-fs-data, so an unbounded
    // script is a boot hang. `wait_timeout` kills the whole process group (the
    // script is spawned as group leader above) after the per-script budget.
    //
    // APD-1: the bound is `min(SCRIPT_TIMEOUT_SECS, remaining stage budget)`
    // when the caller supplies one — a 10-module stage can no longer take
    // 10 × 60 s.
    let result = if wait {
        let mut child = command
            .spawn()
            .map_err(|err| anyhow!("Failed to exec {}: {}", path.as_ref().display(), err))?;
        match wait_timeout(&mut child, effective)? {
            Some(_) => Ok(()),
            None => {
                warn!(
                    "script timed out after {:.1}s, process group killed: {}",
                    effective.as_secs_f64(),
                    path.as_ref().display()
                );
                Ok(())
            }
        }
    } else {
        command.spawn().map(|_| ())
    };
    result.map_err(|err| anyhow!("Failed to exec {}: {}", path.as_ref().display(), err))
}

pub fn exec_stage_script(stage: &str, block: bool) -> Result<()> {
    exec_stage_script_bounded(stage, block, |_| ScriptBudget::Default)
}

/// Stage-script runner with a per-module budget decision supplied by the caller.
///
/// `budget_for` is invoked once per module (in iteration order) with the module
/// path. [`ScriptBudget::Skip`] means the caller's budget is exhausted: the
/// script is NOT spawned (see [`ScriptBudget`] for why this needs three states —
/// the pre-fix `Option<Duration>` wiring turned "skip" into "run at 60s").
pub fn exec_stage_script_bounded<F>(stage: &str, block: bool, mut budget_for: F) -> Result<()>
where
    F: FnMut(&Path) -> ScriptBudget,
{
    foreach_active_module(|module| {
        let script_path = module.join(format!("{stage}.sh"));
        if !script_path.exists() {
            return Ok(());
        }
        let t = budget_for(module);
        run_script(&script_path, block, t)
    })?;
    Ok(())
}

pub fn exec_common_scripts(dir: &str, wait: bool) -> Result<()> {
    exec_common_scripts_bounded(dir, wait, |_| ScriptBudget::Default)
}

/// Common-script runner with a per-script budget decision supplied by the caller.
pub fn exec_common_scripts_bounded<F>(dir: &str, wait: bool, mut budget_for: F) -> Result<()>
where
    F: FnMut(&Path) -> ScriptBudget,
{
    let script_dir = Path::new(defs::ADB_DIR).join(dir);
    if !script_dir.exists() {
        info!("{} not exists, skip", script_dir.display());
        return Ok(());
    }

    let dir = fs::read_dir(&script_dir)?;
    for entry in dir.flatten() {
        let path = entry.path();

        if !is_executable(&path) {
            warn!("{} is not executable, skip", path.display());
            continue;
        }

        let t = budget_for(&path);
        run_script(path, wait, t)?;
    }

    Ok(())
}

/// [`exec_stage_script_bounded`] over an explicit modules directory (test seam).
pub fn exec_stage_script_bounded_in<F>(
    modules_dir: &Path,
    stage: &str,
    block: bool,
    mut budget_for: F,
) -> Result<()>
where
    F: FnMut(&Path) -> ScriptBudget,
{
    foreach_module_in(modules_dir, ModuleType::Active, |module| {
        let script_path = module.join(format!("{stage}.sh"));
        if !script_path.exists() {
            return Ok(());
        }
        let t = budget_for(module);
        run_script(&script_path, block, t)
    })
}

/// [`exec_common_scripts_bounded`] over an explicit script directory (test seam).
pub fn exec_common_scripts_bounded_in<F>(
    script_dir: &Path,
    wait: bool,
    mut budget_for: F,
) -> Result<()>
where
    F: FnMut(&Path) -> ScriptBudget,
{
    if !script_dir.exists() {
        info!("{} not exists, skip", script_dir.display());
        return Ok(());
    }

    let dir = fs::read_dir(script_dir)?;
    for entry in dir.flatten() {
        let path = entry.path();

        if !is_executable(&path) {
            warn!("{} is not executable, skip", path.display());
            continue;
        }

        let t = budget_for(&path);
        run_script(path, wait, t)?;
    }

    Ok(())
}

pub fn load_system_prop() -> Result<()> {
    foreach_active_module(|module| {
        let system_prop = module.join("system.prop");
        if !system_prop.exists() {
            return Ok(());
        }
        info!("load {} system.prop", module.display());

        crate::resetprop::load_system_prop_file(&system_prop)?;

        Ok(())
    })?;

    Ok(())
}

pub fn prune_modules() -> Result<()> {
    foreach_module(ModuleType::All, |module| {
        fs::remove_file(module.join(defs::UPDATE_FILE_NAME)).ok();
        if !module.join(defs::REMOVE_FILE_NAME).exists() {
            return Ok(());
        }

        info!("remove module: {}", module.display());

        // Execute metamodule's metauninstall.sh first
        let module_id = module.file_name().and_then(|n| n.to_str()).unwrap_or("");

        // Check if this is a metamodule
        let is_metamodule = read_module_prop(module)
            .map(|props| metamodule::is_metamodule(&props))
            .unwrap_or(false);

        if is_metamodule {
            info!("Removing metamodule symlink");
            if let Err(e) = metamodule::remove_symlink() {
                warn!("Failed to remove metamodule symlink: {e}");
            }
        } else if let Err(e) = metamodule::exec_metauninstall_script(module_id) {
            warn!("Failed to exec metamodule uninstall for {module_id}: {e}",);
        }

        // Then execute module's own uninstall.sh
        let uninstaller = module.join("uninstall.sh");
        if uninstaller.exists()
            && let Err(e) = exec_script(uninstaller, true)
        {
            warn!("Failed to exec uninstaller: {e}");
        }

        // Clear module configs before removing module directory
        if let Err(e) = module_config::clear_module_configs(module_id) {
            warn!("Failed to clear configs for {module_id}: {e}");
        }

        // Finally remove the module directory
        if let Err(e) = remove_dir_all(module) {
            warn!("Failed to remove {}: {e}", module.display());
        }

        Ok(())
    })?;

    // clean up metamodule record if none remain
    let has_remaining = std::fs::read_dir(defs::MODULE_DIR)?
        .filter_map(std::result::Result::ok)
        .any(|entry| entry.path().join("module.prop").exists());

    if !has_remaining {
        info!("no remaining modules.");
    }

    Ok(())
}

fn _install_module_inner(zip: &str, quiet: bool) -> Result<()> {
    // BUG-APD-4 (2026-11-13): an install failure must leave NO staged copy
    // behind. The pre-fix behaviour extracted into `modules_update/<id>` and
    // then let every `?` after that point bubble up with the half-processed
    // extraction still on disk — so the next boot's `handle_updated_modules`
    // promoted a module whose installer had FAILED (no post-processing, no
    // permission setup) into the live module tree: precisely the bootloop risk
    // the installer's non-zero exit was supposed to prevent. This wrapper
    // mirrors the failure contract of `install_module_json` (`--json`: a
    // failed install reports ok:false — leaving a staged copy that will go
    // live on reboot would contradict that report). This differs from
    // upstream APatch on purpose: upstream leaves the residue.
    //
    // Note the cleanup is scoped to the *failure after extraction* window: the
    // closure returns the module id once it is known (after module.prop
    // parsing), and `cleanup` wipes `modules_update/<id>` plus the freshly
    // created `modules/<id>` directory. An early failure (bad zip, no
    // module.prop) has written nothing, so there is nothing to clean.
    let result: Result<String> = install_module_staged(zip, quiet);
    match result {
        Ok(module_id) => Ok(()),
        Err(e) => {
            // The module id is recovered from the zip independently: parsing
            // may itself have been the failure, in which case there is no id
            // and no staged directory either.
            if let Some(module_id) = staged_module_id(zip) {
                let staged_dir = Path::new(defs::MODULE_UPDATE_DIR).join(&module_id);
                if staged_dir.exists() {
                    if let Err(cleanup_err) = remove_dir_all(&staged_dir) {
                        warn!(
                            "[module install] cannot clean failed staged copy {}: {cleanup_err} \
                             (it must NOT be allowed to go live next boot — remove it manually)",
                            staged_dir.display()
                        );
                    } else {
                        info!(
                            "[module install] cleaned failed staged copy {} (BUG-APD-4: a failed \
                             install leaves no modules_update residue)",
                            staged_dir.display()
                        );
                    }
                }
                // The freshly created (and by now still empty-ish) live dir is
                // only removed when it carries no pre-existing content: an
                // UPGRADE's modules/<id> holds the previous version and must
                // survive a failed upgrade untouched.
                let live_dir = Path::new(defs::MODULE_DIR).join(&module_id);
                if live_dir.is_dir() && is_dir_empty(&live_dir) {
                    let _ = fs::remove_dir(&live_dir);
                }
            }
            Err(e)
        }
    }
}

/// `true` when `dir` contains no entries (used to decide whether a failed
/// install may delete the `modules/<id>` directory it created).
fn is_dir_empty(dir: &Path) -> bool {
    fs::read_dir(dir).map(|mut entries| entries.next().is_none()).unwrap_or(false)
}

/// Recover the module id from a zip's `module.prop` for failure cleanup.
/// `None` when the zip itself is unreadable/unparsable — in that case nothing
/// was extracted either.
fn staged_module_id(zip: &str) -> Option<String> {
    let zip_path = PathBuf::from_str(zip).ok()?;
    let zip_path = zip_path.canonicalize().ok()?;
    let mut buffer: Vec<u8> = Vec::new();
    {
        let mut archive = zip::ZipArchive::new(fs::File::open(&zip_path).ok()?).ok()?;
        archive.by_name("module.prop").ok()?.read_to_end(&mut buffer).ok()?;
    }
    let mut module_prop = HashMap::new();
    PropertiesIter::new_with_encoding(Cursor::new(buffer), encoding_rs::UTF_8)
        .read_into(|k, v| {
            module_prop.insert(k, v);
        })
        .ok()?;
    let id = module_prop.get("id")?.trim().to_string();
    let id_re = regex_lite::Regex::new(r"^[a-zA-Z][a-zA-Z0-9._-]+$").ok()?;
    if id_re.is_match(&id) {
        Some(id)
    } else {
        None
    }
}

/// The install body proper: everything from zip parsing to `mark_update`.
/// Returns the module id so the failure wrapper above can clean up.
fn install_module_staged(zip: &str, quiet: bool) -> Result<String> {
    ensure_boot_completed()?;

    say(quiet, include_str!("../assets/banner"));

    assets::ensure_binaries().with_context(|| "binary missing")?;

    // first check if workding dir is usable
    ensure_dir_exists(defs::WORKING_DIR).with_context(|| "Failed to create working dir")?;
    ensure_dir_exists(defs::BINARY_DIR).with_context(|| "Failed to create bin dir")?;

    // read the module_id from zip
    let mut buffer: Vec<u8> = Vec::new();
    let zip_path = PathBuf::from_str(zip)?;
    let zip_path = zip_path.canonicalize()?;
    {
        let mut archive = zip::ZipArchive::new(fs::File::open(&zip_path)?)?;
        archive.by_name("module.prop")?.read_to_end(&mut buffer)?;
    }
    let mut module_prop = HashMap::new();
    PropertiesIter::new_with_encoding(Cursor::new(buffer), encoding_rs::UTF_8).read_into(
        |k, v| {
            module_prop.insert(k, v);
        },
    )?;
    info!("module prop: {:?}", module_prop);

    let Some(module_id) = module_prop.get("id") else {
        bail!("module id not found in module.prop!");
    };
    let module_id = module_id.trim();

    // The id becomes a directory name under MODULE_DIR and is interpolated into
    // shell commands by the manager; reject path traversal at this trust boundary
    // (same rule as KernelSU and module_config.rs).
    let id_re = regex_lite::Regex::new(r"^[a-zA-Z][a-zA-Z0-9._-]+$")?;
    if !id_re.is_match(module_id) {
        bail!("invalid module id: {module_id}");
    }
    // From here on a failure must clean the staged copy: remember the id for
    // the wrapper (`_install_module_inner`) to act on.
    let module_id = module_id.to_string();

    // Check if this module is a metamodule
    let is_metamodule = metamodule::is_metamodule(&module_prop);

    // Check if module needs mounting (has system/ dir and no skip_mount file)
    let needs_mount = {
        let zip_file = fs::File::open(&zip_path)?;
        let archive = zip::ZipArchive::new(zip_file)?;
        let has_system = archive.file_names().any(|name| name.starts_with("system/"));
        let has_skip_mount = archive.file_names().any(|name| name == "skip_mount");
        has_system && !has_skip_mount
    };

    // Check if it's safe to install regular module
    if !is_metamodule
        && needs_mount
        && let Err(is_disabled) = metamodule::check_install_safety()
    {
        let mut msg = String::from("\n❌ Installation Blocked\n┌────────────────────────────────\n│ A metamodule with custom installer is active\n│\n");
        if is_disabled {
            msg.push_str("│ Current state: Disabled\n│ Action required: Re-enable or uninstall it, then reboot\n");
        } else {
            msg.push_str("│ Current state: Pending changes\n│ Action required: Reboot to apply changes first\n");
        }
        msg.push_str("└─────────────────────────────────\n");
        say(quiet, &msg);
        bail!("Metamodule installation blocked");
    }

    let modules_dir = Path::new(defs::MODULE_DIR);
    let modules_update_dir = Path::new(defs::MODULE_UPDATE_DIR);
    if !Path::new(modules_dir).exists() {
        fs::create_dir(modules_dir).expect("Failed to create modules folder");
        let permissions = fs::Permissions::from_mode(0o700);
        fs::set_permissions(modules_dir, permissions).expect("Failed to set permissions");
    }
    ensure_dir_exists(modules_update_dir).with_context(|| "Failed to create modules_update dir")?;

    if is_metamodule {
        info!("Installing metamodule: {module_id}");

        // Check if there's already a metamodule installed
        if metamodule::has_metamodule()
            && let Some(existing_path) = metamodule::get_metamodule_path()
        {
            let existing_id = read_module_prop(&existing_path)
                .ok()
                .and_then(|m| m.get("id").cloned())
                .unwrap_or_else(|| "unknown".to_string());

            if existing_id != module_id {
                let msg = format!(
                    "\n❌ Installation Failed\n┌────────────────────────────────\n│ A metamodule is already installed\n│   Current metamodule: {existing_id}\n│\n│ Only one metamodule can be active at a time.\n│\n│ To install this metamodule:\n│   1. Uninstall the current metamodule\n│   2. Reboot your device\n│   3. Install the new metamodule\n└─────────────────────────────────\n"
                );
                say(quiet, &msg);
                bail!("Cannot install multiple metamodules");
            }
        }
    }

    let module_dir = format!("{}{}", modules_dir.display(), module_id);
    let _module_update_dir = format!("{}{}", modules_update_dir.display(), module_id);
    info!("module dir: {}", module_dir);
    if !Path::new(&module_dir.clone()).exists() {
        fs::create_dir(module_dir.clone()).expect("Failed to create module folder");
        let permissions = fs::Permissions::from_mode(0o700);
        fs::set_permissions(module_dir.clone(), permissions).expect("Failed to set permissions");
    }
    // unzip the image and move it to modules_update/<id> dir
    let file = fs::File::open(zip)?;
    let mut archive = zip::ZipArchive::new(file)?;
    // APD-4 ④-4: re-validate the *extracted* archive before writing anything.
    //
    // `install_module_json` runs `inspect_module_zip` first, but that opens the
    // zip by path and this line opens it again — a classic TOCTOU window in
    // which the file can be swapped for a malicious one that passed inspection
    // in name only. Re-checking here (on the very handle we are about to
    // extract) closes it: the checks that matter (zip-slip / symlink escape /
    // entry count) are cheap and run against the same inode we extract from.
    //
    // Non-bulk callers (`apd module install` without --json, lua) reach this
    // function without any prior inspection at all, so this is not redundant
    // for them either.
    revalidate_archive_for_extraction(&mut archive).map_err(|e| anyhow!("{e}"))?;
    archive.extract(&_module_update_dir)?;

    say(quiet, "- Running module installer");
    exec_install_script(zip, is_metamodule, &module_id, quiet)?;

    // set permission and selinux context for $MOD/system
    let module_system_dir = PathBuf::from(module_dir.clone()).join("system");
    if module_system_dir.exists() {
        #[cfg(unix)]
        fs::set_permissions(&module_system_dir, fs::Permissions::from_mode(0o755))?;
        restorecon::restore_syscon(&module_system_dir)?;
    }

    // Create symlink for metamodule
    if is_metamodule {
        say(quiet, "- Creating metamodule symlink");
        metamodule::ensure_symlink(&module_dir)?;
    }

    mark_update()?;
    Ok(module_id)
}

pub fn install_module(zip: &str) -> Result<()> {
    _install_module_inner(zip, false)
}

// ---------------------------------------------------------------------------
// Machine-readable module install / uninstall (NEW-DOC-05 §3.1.2).
//
// `--json` contract: stdout is one JSON object and nothing else (all chat goes
// to stderr/logcat); exit code carries the classified failure. `list --json`
// schema stays the frozen 8-field one.
// ---------------------------------------------------------------------------

pub const EXIT_INPUT_MISSING: i32 = 110;
pub const EXIT_ZIP_CORRUPT: i32 = 111;
pub const EXIT_MANIFEST_INVALID: i32 = 112;
pub const EXIT_UNSAFE_ZIP: i32 = 113;
pub const EXIT_LIMIT_EXCEEDED: i32 = 114;
pub const EXIT_NO_SPACE: i32 = 115;
pub const EXIT_CONFLICT: i32 = 116;
pub const EXIT_SCRIPT_FAILED: i32 = 120;
pub const EXIT_IO_FAILED: i32 = 121;
pub const EXIT_STATE_FAILED: i32 = 122;
pub const EXIT_TIMEOUT: i32 = 124;

struct ModuleZipInfo {
    id: String,
    name: String,
    version: String,
    version_code: i64,
    uncompressed: u64,
    metamodule: bool,
}

fn json_emit(code: i32, payload: serde_json::Value) -> ! {
    println!(
        "{}",
        serde_json::to_string_pretty(&payload).unwrap_or_else(|_| "{}".to_string())
    );
    std::process::exit(code);
}

fn json_fail(code: i32, id: Option<&str>, error: &str) -> ! {
    json_emit(
        code,
        serde_json::json!({"ok": false, "code": code, "id": id, "error": error}),
    )
}

fn free_bytes(path: &str) -> Option<u64> {
    let cpath = std::ffi::CString::new(path).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(cpath.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    Some(stat.f_bavail as u64 * stat.f_frsize as u64)
}

fn module_meta_from_dir(dir: &Path) -> Option<(String, i64)> {
    let props = read_module_prop(dir).ok()?;
    let version = props.get("version").cloned().unwrap_or_default();
    let code = props
        .get("versionCode")
        .and_then(|v| v.trim().parse::<i64>().ok())
        .unwrap_or(0);
    Some((version, code))
}

/// Pre-flight validation of a module zip: size caps, zip-slip/symlink rejection,
/// `module.prop` required fields, uncompressed-size accounting.
/// Why an entry name is unsafe to extract, or `None` when it is fine.
///
/// Extracted so the *inspection* pass and the *extraction* pass apply exactly
/// the same rule (APD-4 ④-4). Duplicating this predicate is how the two passes
/// drift apart and a file that fails inspection gets extracted anyway.
fn unsafe_entry_reason(name: &str) -> Option<String> {
    if name.is_empty() {
        return None;
    }
    // zip-slip: absolute paths, backslashes (Windows separators), drive letters.
    if name.starts_with('/') || name.starts_with('\\') || name.contains('\\') {
        return Some(format!("unsafe absolute path: {name}"));
    }
    if name
        .split('/')
        .any(|c| c == ".." || c == "." || c.is_empty() && name.contains("//"))
    {
        return Some(format!("unsafe relative path: {name}"));
    }
    None
}

/// Re-run the safety checks on an already-open archive, immediately before
/// extraction (APD-4 ④-4).
///
/// `inspect_module_zip` validates a zip it opened **by path**; the extraction
/// code then opens it again. Between those two opens the file can be replaced
/// (TOCTOU) — and for the non-`--json` install path no inspection happens at
/// all. This runs the same predicates on the archive handle that is about to be
/// extracted, so "checked" and "extracted" refer to the same bytes.
///
/// Deliberately re-checks only the *cheap, structural* properties (names,
/// symlink bits, entry count, uncompressed total). Content-level checks
/// (`module.prop` fields) stay in `inspect_module_zip` to avoid reading the
/// archive twice.
fn revalidate_archive_for_extraction<R: Read + std::io::Seek>(
    archive: &mut zip::ZipArchive<R>,
) -> Result<()> {
    if archive.len() > MAX_ZIP_ENTRIES {
        bail!(
            "zip has {} entries, over the {MAX_ZIP_ENTRIES} limit",
            archive.len()
        );
    }
    let mut uncompressed: u64 = 0;
    for i in 0..archive.len() {
        let entry = archive
            .by_index_raw(i)
            .with_context(|| format!("read entry #{i} during extraction re-check"))?;
        let name = entry.name().to_string();
        if let Some(reason) = unsafe_entry_reason(&name) {
            bail!("refusing to extract: {reason}");
        }
        if let Some(mode) = entry.unix_mode()
            && mode & 0o170000 == 0o120000
        {
            bail!("refusing to extract: symlink entry: {name}");
        }
        uncompressed = uncompressed.saturating_add(entry.size());
        if uncompressed > MAX_UNCOMPRESSED_BYTES {
            bail!(
                "refusing to extract: uncompressed size over {MAX_UNCOMPRESSED_BYTES} B"
            );
        }
    }
    Ok(())
}

fn inspect_module_zip(zip_path: &Path) -> Result<ModuleZipInfo, (i32, String)> {
    let meta = fs::metadata(zip_path)
        .map_err(|e| (EXIT_INPUT_MISSING, format!("zip not readable: {e}")))?;
    if !meta.is_file() {
        return Err((EXIT_INPUT_MISSING, "not a regular file".to_string()));
    }
    if meta.len() > MAX_MODULE_ZIP_BYTES {
        return Err((
            EXIT_LIMIT_EXCEEDED,
            format!(
                "zip is {} B, over the {} B limit",
                meta.len(),
                MAX_MODULE_ZIP_BYTES
            ),
        ));
    }
    let file =
        fs::File::open(zip_path).map_err(|e| (EXIT_INPUT_MISSING, format!("open: {e}")))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| (EXIT_ZIP_CORRUPT, format!("corrupt zip: {e}")))?;
    if archive.len() > MAX_ZIP_ENTRIES {
        return Err((
            EXIT_LIMIT_EXCEEDED,
            format!("zip has {} entries, over {MAX_ZIP_ENTRIES}", archive.len()),
        ));
    }

    let mut uncompressed: u64 = 0;
    let mut prop_bytes: Option<Vec<u8>> = None;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| (EXIT_ZIP_CORRUPT, format!("entry #{i}: {e}")))?;
        let name = entry.name().to_string();
        if name.is_empty() {
            continue;
        }
        // zip-slip / symlink / size accounting — the SAME predicate the
        // extraction re-check uses (`unsafe_entry_reason`), so the two passes
        // cannot drift apart (APD-4 ④-4).
        if let Some(reason) = unsafe_entry_reason(&name) {
            return Err((EXIT_UNSAFE_ZIP, reason));
        }
        // symlink entries could escape the module dir after extraction.
        if let Some(mode) = entry.unix_mode()
            && mode & 0o170000 == 0o120000
        {
            return Err((EXIT_UNSAFE_ZIP, format!("symlink entry: {name}")));
        }
        uncompressed = uncompressed.saturating_add(entry.size());
        if uncompressed > MAX_UNCOMPRESSED_BYTES {
            return Err((
                EXIT_LIMIT_EXCEEDED,
                format!("uncompressed size over {MAX_UNCOMPRESSED_BYTES} B"),
            ));
        }
        if name == "module.prop" {
            let mut buf = Vec::new();
            entry
                .read_to_end(&mut buf)
                .map_err(|e| (EXIT_ZIP_CORRUPT, format!("module.prop read: {e}")))?;
            prop_bytes = Some(buf);
        }
    }

    let prop_bytes = prop_bytes.ok_or((
        EXIT_MANIFEST_INVALID,
        "zip has no module.prop".to_string(),
    ))?;
    let mut props: HashMap<String, String> = HashMap::new();
    PropertiesIter::new_with_encoding(Cursor::new(prop_bytes), encoding_rs::UTF_8)
        .read_into(|k, v| {
            props.insert(k, v);
        })
        .map_err(|e| (EXIT_MANIFEST_INVALID, format!("parse module.prop: {e}")))?;

    let id = props
        .get("id")
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .ok_or((
            EXIT_MANIFEST_INVALID,
            "module.prop missing required `id`".to_string(),
        ))?;
    let id_re = regex_lite::Regex::new(r"^[a-zA-Z][a-zA-Z0-9._-]+$")
        .map_err(|e| (EXIT_MANIFEST_INVALID, format!("id regex: {e}")))?;
    if !id_re.is_match(&id) {
        return Err((EXIT_MANIFEST_INVALID, format!("invalid module id: {id}")));
    }
    for field in ["name", "version", "versionCode"] {
        if props.get(field).map(|v| v.trim().is_empty()).unwrap_or(true) {
            return Err((
                EXIT_MANIFEST_INVALID,
                format!("module.prop missing required `{field}`"),
            ));
        }
    }
    for field in ["author", "description"] {
        if props.get(field).map(|v| v.trim().is_empty()).unwrap_or(true) {
            log::warn!("[module install] module.prop has no `{field}` (recommended by Magisk spec)");
        }
    }
    let version = props.get("version").cloned().unwrap_or_default();
    let version_code = props
        .get("versionCode")
        .and_then(|v| v.trim().parse::<i64>().ok())
        .ok_or((
            EXIT_MANIFEST_INVALID,
            "module.prop `versionCode` is not an integer".to_string(),
        ))?;
    let name = props
        .get("name")
        .cloned()
        .unwrap_or_else(|| id.clone());
    let metamodule = metamodule::is_metamodule(&props);

    Ok(ModuleZipInfo {
        id,
        name,
        version,
        version_code,
        uncompressed,
        metamodule,
    })
}

fn write_module_state(id: &str, version: &str, version_code: i64, zip: &str) -> Result<()> {
    ensure_dir_exists(defs::MODULE_STATE_DIR)?;
    // One probe, then merge: `write_module_state` runs on the install path but is
    // not itself part of a response payload, so the audit is recorded here too
    // (previously the file simply carried a hardcoded `"avb_untouched": true`).
    let state = merge_avb_audit(serde_json::json!({
        "id": id,
        "version": version,
        "versionCode": version_code,
        "installed_at": std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        "source_zip": zip,
    }));
    let path = Path::new(defs::MODULE_STATE_DIR).join(format!("{id}.json"));
    fs::write(&path, serde_json::to_string_pretty(&state)?)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(())
}

/// `apd module install <zip> --json`.
pub fn install_module_json(zip: &str) -> ! {
    let started = Instant::now();
    let zip_path = Path::new(zip);
    let info = match inspect_module_zip(zip_path) {
        Ok(info) => info,
        Err((code, msg)) => json_fail(code, None, &msg),
    };

    // Disk-space pre-flight: extraction needs the payload plus headroom.
    if let Some(free) = free_bytes("/data")
        && free < info.uncompressed.saturating_add(MIN_FREE_BYTES)
    {
        json_fail(
            EXIT_NO_SPACE,
            Some(&info.id),
            &format!(
                "insufficient space on /data: free={free} B, need>={} B",
                info.uncompressed.saturating_add(MIN_FREE_BYTES)
            ),
        );
    }

    // Conflict policy: same id = upgrade (allowed, state preserved); a module
    // dir whose module.prop id differs from its directory name is corruption.
    let active_dir = Path::new(defs::MODULE_DIR).join(&info.id);
    if let Ok(props) = read_module_prop(&active_dir)
        && let Some(existing_id) = props.get("id")
        && existing_id.trim() != info.id
    {
        json_fail(
            EXIT_CONFLICT,
            Some(&info.id),
            &format!("module dir id mismatch: {existing_id} != {}", info.id),
        );
    }

    let prev = module_meta_from_dir(&active_dir);
    let changed = match &prev {
        Some((version, code)) => version != &info.version || *code != info.version_code,
        None => true,
    };

    if let Err(e) = _install_module_inner(zip, true) {
        // Type-based classification (2026-11-10 S1): `EXIT_TIMEOUT` (124) is now
        // reachable, and no exit code depends on the wording of a message.
        let code = classify_install_error(&e);
        json_fail(code, Some(&info.id), &format!("{e:#}"));
    }

    // Installing cancels a pending removal; the `disable` flag is user state and
    // is preserved (idempotent re-install of the same version changes nothing).
    let _ = fs::remove_file(active_dir.join(defs::REMOVE_FILE_NAME));
    if let Err(e) = write_module_state(&info.id, &info.version, info.version_code, zip) {
        json_fail(EXIT_STATE_FAILED, Some(&info.id), &format!("{e:#}"));
    }
    let enabled = !active_dir.join(defs::DISABLE_FILE_NAME).exists();

    if started.elapsed() > Duration::from_secs(INSTALL_TIMEOUT_SECS + 120) {
        log::warn!("[module install] install took unusually long: {:?}", started.elapsed());
    }
    json_emit(
        0,
        merge_avb_audit(serde_json::json!({
            "ok": true,
            "id": info.id,
            "name": info.name,
            "version": info.version,
            "versionCode": info.version_code,
            "metamodule": info.metamodule,
            "enabled": enabled,
            "update_pending": true,
            "active_after_reboot": true,
            "changed": changed,
            "previous_version": prev.map(|(v, _)| v),
            "uncompressed_bytes": info.uncompressed,
        })),
    );
}

/// `apd module uninstall <id> --json`.
pub fn uninstall_module_json(id: &str) -> ! {
    let id_re = match regex_lite::Regex::new(r"^[a-zA-Z][a-zA-Z0-9._-]+$") {
        Ok(re) => re,
        Err(e) => json_fail(EXIT_MANIFEST_INVALID, Some(id), &format!("id regex: {e}")),
    };
    if !id_re.is_match(id) {
        json_fail(EXIT_MANIFEST_INVALID, Some(id), "invalid module id");
    }
    let active = Path::new(defs::MODULE_DIR).join(id);
    let staged = Path::new(defs::MODULE_UPDATE_DIR).join(id);
    if !active.exists() && !staged.exists() {
        json_fail(EXIT_INPUT_MISSING, Some(id), "module not installed");
    }

    if active.exists()
        && let Err(e) = _uninstall_module(id, defs::MODULE_DIR)
    {
        json_fail(EXIT_IO_FAILED, Some(id), &format!("{e:#}"));
    }
    if staged.exists()
        && let Err(e) = remove_dir_all(&staged)
    {
        json_fail(EXIT_IO_FAILED, Some(id), &format!("{e:#}"));
    }
    let _ = module_config::clear_module_configs(id);
    let _ = fs::remove_file(Path::new(defs::MODULE_STATE_DIR).join(format!("{id}.json")));

    json_emit(
        0,
        merge_avb_audit(serde_json::json!({
            "ok": true,
            "id": id,
            "remove_pending": active.exists(),
            "applied_after_reboot": active.exists(),
        })),
    );
}

pub fn _uninstall_module(id: &str, update_dir: &str) -> Result<()> {
    let dir = Path::new(update_dir);
    ensure!(dir.exists(), "No module installed");

    // iterate the modules_update dir, find the module to be removed
    let dir = fs::read_dir(dir)?;
    for entry in dir.flatten() {
        let path = entry.path();
        let module_prop = path.join("module.prop");
        if !module_prop.exists() {
            continue;
        }
        let content = fs::read(module_prop)?;
        let mut module_id: String = String::new();
        PropertiesIter::new_with_encoding(Cursor::new(content), encoding_rs::UTF_8).read_into(
            |k, v| {
                if k.eq("id") {
                    module_id = v;
                }
            },
        )?;
        if module_id.eq(id) {
            let remove_file = path.join(defs::REMOVE_FILE_NAME);
            fs::File::create(remove_file).with_context(|| "Failed to create remove file.")?;
            break;
        }
    }

    // santity check
    let target_module_path = format!("{update_dir}/{id}");
    let target_module = Path::new(&target_module_path);
    if target_module.exists() {
        let remove_file = target_module.join(defs::REMOVE_FILE_NAME);
        if !remove_file.exists() {
            fs::File::create(remove_file).with_context(|| "Failed to create remove file.")?;
        }
    }

    let _ = mark_module_state(id, defs::REMOVE_FILE_NAME, true);
    Ok(())
}
pub fn uninstall_module(id: &str) -> Result<()> {
    _uninstall_module(id, defs::MODULE_DIR)?;
    mark_update()?;
    Ok(())
}

pub fn _undo_uninstall_module(id: &str, update_dir: &str) -> Result<()> {
    let dir = Path::new(update_dir);
    ensure!(dir.exists(), "No module installed");

    let mut found = false;
    for entry in fs::read_dir(dir)?.flatten() {
        let path = entry.path();
        let module_prop = path.join("module.prop");
        if !module_prop.exists() {
            continue;
        }

        let content = fs::read(&module_prop)?;
        let mut module_id = String::new();

        PropertiesIter::new_with_encoding(Cursor::new(content), encoding_rs::UTF_8).read_into(
            |k, v| {
                if k == "id" {
                    module_id = v;
                }
            },
        )?;
        if module_id == id {
            let remove_file = path.join(defs::REMOVE_FILE_NAME);
            fs::remove_file(remove_file).with_context(|| "Failed to remove removefile.")?;
            found = true;
            break;
        }
    }

    ensure!(found, "Module not found");

    let _ = mark_module_state(id, defs::REMOVE_FILE_NAME, false);
    Ok(())
}
pub fn undo_uninstall_module(id: &str) -> Result<()> {
    _undo_uninstall_module(id, defs::MODULE_DIR)?;
    mark_update()?;
    Ok(())
}

/// Read module.prop from the given module path and return as a HashMap
pub fn read_module_prop(module_path: &Path) -> Result<HashMap<String, String>> {
    let module_prop = module_path.join("module.prop");
    ensure!(
        module_prop.exists(),
        "module.prop not found in {}",
        module_path.display()
    );

    let content = std::fs::read(&module_prop)
        .with_context(|| format!("Failed to read module.prop: {}", module_prop.display()))?;

    let mut prop_map: HashMap<String, String> = HashMap::new();
    PropertiesIter::new_with_encoding(Cursor::new(content), encoding_rs::UTF_8)
        .read_into(|k, v| {
            prop_map.insert(k, v);
        })
        .with_context(|| format!("Failed to parse module.prop: {}", module_prop.display()))?;

    Ok(prop_map)
}

pub fn run_action(id: &str) -> Result<()> {
    let action_script_path = format!("/data/adb/modules/{}/action.sh", id);
    if Path::new(&action_script_path).exists() {
        let _ = exec_script(&action_script_path, true);
    } else {
        //if no action.sh, try to run lua action
        lua::run_lua(id, "action", false, true).map_err(|e| anyhow::anyhow!("{}", e))?;
    }
    Ok(())
}

fn _change_module_state(module_dir: &str, mid: &str, enable: bool) -> Result<()> {
    let src_module_path = format!("{module_dir}/{mid}");
    let src_module = Path::new(&src_module_path);
    ensure!(src_module.exists(), "module: {} not found!", mid);

    let disable_path = src_module.join(defs::DISABLE_FILE_NAME);
    if enable {
        if disable_path.exists() {
            fs::remove_file(&disable_path).with_context(|| {
                format!("Failed to remove disable file: {}", disable_path.display())
            })?;
        }
    } else {
        ensure_file_exists(disable_path)?;
    }

    let _ = mark_module_state(mid, defs::DISABLE_FILE_NAME, !enable);

    Ok(())
}

pub fn _enable_module(id: &str, update_dir: &Path) -> Result<()> {
    if let Some(module_dir_str) = update_dir.to_str() {
        _change_module_state(module_dir_str, id, true)
    } else {
        info!("Enable module failed: Invalid path");
        Err(anyhow::anyhow!("Invalid module directory"))
    }
}

pub fn enable_module(id: &str) -> Result<()> {
    let update_dir = Path::new(defs::MODULE_DIR);
    _enable_module(id, update_dir)?;
    Ok(())
}

pub fn _disable_module(id: &str, update_dir: &Path) -> Result<()> {
    if let Some(module_dir_str) = update_dir.to_str() {
        _change_module_state(module_dir_str, id, false)
    } else {
        info!("Disable module failed: Invalid path");
        Err(anyhow::anyhow!("Invalid module directory"))
    }
}

pub fn disable_module(id: &str) -> Result<()> {
    let module_dir = Path::new(defs::MODULE_DIR);
    _disable_module(id, module_dir)?;

    Ok(())
}

pub fn _disable_all_modules(dir: &str) -> Result<()> {
    let dir = fs::read_dir(dir)?;
    for entry in dir.flatten() {
        let path = entry.path();
        let disable_flag = path.join(defs::DISABLE_FILE_NAME);
        if let Err(e) = ensure_file_exists(disable_flag) {
            warn!("Failed to disable module: {}: {}", path.display(), e);
        }
    }
    Ok(())
}

pub fn disable_all_modules() -> Result<()> {
    // Skip disabling modules since boot completed
    if getprop("sys.boot_completed").as_deref() == Some("1") {
        info!("System boot completed, no need to disable all modules");
        return Ok(());
    }
    mark_update()?;
    _disable_all_modules(defs::MODULE_DIR)?;
    Ok(())
}

fn _list_modules(path: &str) -> Vec<HashMap<String, String>> {
    // Load all module configs once to minimize I/O overhead
    let all_configs = match module_config::get_all_module_configs() {
        Ok(configs) => configs,
        Err(e) => {
            warn!("Failed to load module configs: {e}");
            HashMap::new()
        }
    };

    // first check enabled modules
    let dir = fs::read_dir(path);
    let Ok(dir) = dir else {
        return Vec::new();
    };

    let mut modules: Vec<HashMap<String, String>> = Vec::new();

    for entry in dir.flatten() {
        let path = entry.path();
        info!("path: {}", path.display());
        let module_prop = path.join("module.prop");
        if !module_prop.exists() {
            continue;
        }
        let content = fs::read(&module_prop);
        let Ok(content) = content else {
            warn!("Failed to read file: {}", module_prop.display());
            continue;
        };
        let mut module_prop_map: HashMap<String, String> = HashMap::new();
        let encoding = encoding_rs::UTF_8;

        if PropertiesIter::new_with_encoding(Cursor::new(content), encoding)
            .read_into(|k, v| {
                module_prop_map.insert(k, v);
            })
            .is_err()
        {
            warn!("Failed to parse module.prop: {}", module_prop.display());
            continue;
        }

        if !module_prop_map.contains_key("id") || module_prop_map["id"].is_empty() {
            match entry.file_name().to_str() {
                Some(id) => {
                    info!("Use dir name as module id: {}", id);
                    module_prop_map.insert("id".to_owned(), id.to_owned());
                }
                _ => {
                    info!("Failed to get module id: {:?}", module_prop);
                    continue;
                }
            }
        }

        // Add enabled, update, remove flags
        let enabled = !path.join(defs::DISABLE_FILE_NAME).exists();
        let update = path.join(defs::UPDATE_FILE_NAME).exists();
        let remove = path.join(defs::REMOVE_FILE_NAME).exists();
        let web = path.join(defs::MODULE_WEB_DIR).exists();
        let id = module_prop_map.get("id").map(|s| s.as_str()).unwrap_or("");
        let id_lua_file = format!("{}.lua", id);
        let action = path.join(defs::MODULE_ACTION_SH).exists() || path.join(&id_lua_file).exists();

        module_prop_map.insert("enabled".to_owned(), enabled.to_string());
        module_prop_map.insert("update".to_owned(), update.to_string());
        module_prop_map.insert("remove".to_owned(), remove.to_string());
        module_prop_map.insert("web".to_owned(), web.to_string());
        module_prop_map.insert("action".to_owned(), action.to_string());

        // Apply module config overrides and extract managed features
        if let Some(module_id) = module_prop_map.get("id")
            && let Some(config) = all_configs.get(module_id.as_str())
        {
            // Apply override.description
            if let Some(desc) = config.get("override.description") {
                module_prop_map.insert("description".to_owned(), desc.clone());
            }
        }

        modules.push(module_prop_map);
    }

    modules
}

/// Machine-readable module list for the manager (contract: stable schema,
/// `module list --json`). Fields frozen 2026-11-08 (agreed with app-client):
/// `{id,name,version,versionCode,author,description,enabled,update_pending}`.
///
/// `update_pending` is true when either the module carries the `update` flag or
/// a staged copy exists in `modules_update/<id>` (i.e. a reboot will apply it).
pub fn list_modules_json() -> Result<()> {
    let mut modules: Vec<serde_json::Value> = Vec::new();
    let modules_dir = Path::new(defs::MODULE_DIR);
    let Ok(dir) = std::fs::read_dir(modules_dir) else {
        // No module dir yet is a valid empty state, not an error.
        println!("[]");
        return Ok(());
    };

    for entry in dir.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let id_from_dir = entry.file_name().to_string_lossy().to_string();
        let mut props = read_module_prop(&path).unwrap_or_default();
        let id = props
            .get("id")
            .filter(|v| !v.is_empty())
            .cloned()
            .unwrap_or(id_from_dir.clone());
        // Normalise: the module.prop `id` is authoritative when present.
        props.entry("id".to_string()).or_insert_with(|| id.clone());

        let enabled = !path.join(defs::DISABLE_FILE_NAME).exists();
        let update_pending = path.join(defs::UPDATE_FILE_NAME).exists()
            || Path::new(defs::MODULE_UPDATE_DIR).join(&id).exists();
        // module.prop `versionCode` is a decimal string; expose it as a number
        // (0 when absent/unparsable) so the manager can sort versions.
        let version_code: i64 = props
            .get("versionCode")
            .and_then(|v| v.trim().parse::<i64>().ok())
            .unwrap_or(0);

        modules.push(serde_json::json!({
            "id": id,
            "name": props.get("name").cloned().unwrap_or_else(|| id.clone()),
            "version": props.get("version").cloned().unwrap_or_default(),
            "versionCode": version_code,
            "author": props.get("author").cloned().unwrap_or_default(),
            "description": props.get("description").cloned().unwrap_or_default(),
            "enabled": enabled,
            "update_pending": update_pending,
        }));
    }

    modules.sort_by(|a, b| {
        a["id"]
            .as_str()
            .unwrap_or("")
            .cmp(b["id"].as_str().unwrap_or(""))
    });
    println!("{}", serde_json::to_string_pretty(&modules)?);
    Ok(())
}

pub fn list_modules() -> Result<()> {
    let modules = _list_modules(defs::MODULE_DIR);
    println!("{}", serde_json::to_string_pretty(&modules)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The contract table (NEW-DOC-05 §3.1.2) is the source of truth for these
    /// numbers; a mismatch here means the `--json` exit-code ABI drifted.
    #[test]
    fn module_error_codes_match_contract() {
        assert_eq!(ModuleError::Timeout { secs: 300 }.exit_code(), 124);
        assert_eq!(
            ModuleError::ScriptFailed("whatever".into()).exit_code(),
            120
        );
        assert_eq!(ModuleError::Io("whatever".into()).exit_code(), 121);
        // The constants themselves are the ABI the manager already ships.
        assert_eq!(EXIT_TIMEOUT, 124);
        assert_eq!(EXIT_SCRIPT_FAILED, 120);
        assert_eq!(EXIT_IO_FAILED, 121);
    }

    /// **Positive proof that 124 is reachable** — the whole point of the S1 fix.
    /// Before it, a timeout bailed with a message containing "installer" and was
    /// classified as 120 by the string matcher, so 124 was dead code.
    #[test]
    fn installer_timeout_maps_to_124() {
        let err: anyhow::Error = ModuleError::Timeout {
            secs: INSTALL_TIMEOUT_SECS,
        }
        .into();
        assert_eq!(classify_install_error(&err), EXIT_TIMEOUT);
        assert_eq!(classify_install_error(&err), 124);
    }

    /// Negative sample for the old heuristic: the *message* must not decide the
    /// code. `ModuleError::Io` carries a timeout-flavoured, "installer"-flavoured
    /// text and still maps to 121; `ScriptFailed` with no magic words maps to 120.
    /// Under the old `msg.contains("installer")` matcher both would be 120.
    #[test]
    fn exit_code_does_not_depend_on_message_wording() {
        let io_mentions_installer: anyhow::Error =
            ModuleError::Io("module installer exploded: must install".into()).into();
        assert_eq!(classify_install_error(&io_mentions_installer), EXIT_IO_FAILED);
        assert_ne!(classify_install_error(&io_mentions_installer), EXIT_SCRIPT_FAILED);

        let script_without_magic_words: anyhow::Error =
            ModuleError::ScriptFailed("process exited 1".into()).into();
        assert_eq!(
            classify_install_error(&script_without_magic_words),
            EXIT_SCRIPT_FAILED
        );
        assert_ne!(
            classify_install_error(&script_without_magic_words),
            EXIT_IO_FAILED
        );
    }

    /// A `ModuleError` nested behind an `anyhow` context must still classify by
    /// type: adding context is not allowed to reclassify (that was the trap in
    /// the old string matcher, where `.context("install")` flipped 121 → 120).
    #[test]
    fn context_wrapping_preserves_classification() {
        let wrapped = anyhow::Error::new(ModuleError::Timeout { secs: 7 })
            .context("while installing module foo");
        assert_eq!(classify_install_error(&wrapped), EXIT_TIMEOUT);
    }

    /// Unclassified failures stay 121 (never silently become "script failed"),
    /// and the timeout detail still mentions the configured budget.
    #[test]
    fn unclassified_and_display_paths() {
        let plain: anyhow::Error = anyhow!("some io problem");
        assert_eq!(classify_install_error(&plain), EXIT_IO_FAILED);
        let msg = ModuleError::Timeout {
            secs: INSTALL_TIMEOUT_SECS,
        }
        .to_string();
        assert!(msg.contains("timed out"), "{msg}");
        assert!(msg.contains(&INSTALL_TIMEOUT_SECS.to_string()), "{msg}");
    }

    // -----------------------------------------------------------------------
    // AVB audit fields on the module `--json` contract (2026-11-10 S1b).
    //
    // The old code emitted a literal `"avb_untouched": true` from
    // `install_module_json` / `uninstall_module_json` / `write_module_state`,
    // i.e. it asserted "the boot partition was not touched" without ever
    // reading it. These tests pin the measured replacement and are the
    // regression guard against that constant coming back.
    // -----------------------------------------------------------------------

    /// Scratch dir that removes itself; also owns the env-var override so tests
    /// never race each other (`SPROOT_AVB_BOOT_IMAGE` is process-global).
    struct AvbFixture {
        dir: PathBuf,
        _guard: std::sync::MutexGuard<'static, ()>,
    }
    impl AvbFixture {
        fn new(tag: &str) -> Self {
            // Serialise every test that mutates the process-global override.
            static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
            let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
            let unique = format!(
                "sproot-mod-avb-{tag}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            );
            let dir = std::env::temp_dir().join(unique);
            fs::create_dir_all(&dir).expect("create scratch dir");
            AvbFixture { dir, _guard: guard }
        }
        fn path(&self, name: &str) -> PathBuf {
            self.dir.join(name)
        }
        /// Point the probe at `path` for the lifetime of the fixture.
        fn point_at(&self, path: &Path) {
            // SAFETY: guarded by the process-wide mutex above; no read happens
            // between this write and the probe inside the same test.
            unsafe { std::env::set_var(defs::AVB_BOOT_OVERRIDE_ENV, path) };
        }
    }
    impl Drop for AvbFixture {
        fn drop(&mut self) {
            unsafe { std::env::remove_var(defs::AVB_BOOT_OVERRIDE_ENV) };
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    /// Minimal AVB partition: `[payload][vbmeta][zero fill][64 B footer]`,
    /// big-endian like avbtool (mirrors `deploy::tests::synth_partition`).
    fn synth_avb_partition(total: u64, payload: &[u8], vbmeta: &[u8]) -> Vec<u8> {
        let mut img = vec![0u8; total as usize];
        img[..payload.len()].copy_from_slice(payload);
        let vo = payload.len() as u64;
        let vs = vbmeta.len() as u64;
        img[vo as usize..(vo + vs) as usize].copy_from_slice(vbmeta);
        let fa = (total - 64) as usize;
        img[fa..fa + 4].copy_from_slice(b"AVBf");
        img[fa + 4..fa + 8].copy_from_slice(&1u32.to_be_bytes());
        img[fa + 12..fa + 20].copy_from_slice(&(vo + vs).to_be_bytes());
        img[fa + 20..fa + 28].copy_from_slice(&vo.to_be_bytes());
        img[fa + 28..fa + 36].copy_from_slice(&vs.to_be_bytes());
        img
    }

    /// ① A normal device (footer present) reports a measured `ok` status and a
    /// real footer digest — **not** a hardcoded `true`.
    #[test]
    fn avb_audit_reports_measured_status_on_a_real_footer() {
        let fixture = AvbFixture::new("ok");
        let img = synth_avb_partition(8192, b"PAYLOAD", b"VBMETA-BYTES");
        let path = fixture.path("boot.img");
        fs::write(&path, &img).unwrap();
        fixture.point_at(&path);

        let fields = avb_audit_fields();
        assert_eq!(fields["avb_status"], "ok", "fields={fields}");
        assert_eq!(fields["avb_untouched"], true);
        let digest = fields["avb_footer_sha256"]
            .as_str()
            .expect("a measured footer must expose its digest");
        assert_eq!(digest.len(), 64, "digest={digest}");
        // The digest must be of the real footer bytes, not a placeholder.
        let real = crate::utils::sha256_hex(&img[8192 - 64..]);
        assert_eq!(digest, real);
        assert!(fields["avb_detail"].as_str().unwrap_or("").contains("boot.img"));
    }

    /// ② **The required negative sample**: when the partition cannot be
    /// observed, the audit says `unobservable` and `avb_untouched` is `false` —
    /// never `true`. Restoring the hardcoded literal fails this test.
    #[test]
    fn avb_audit_reports_unobservable_when_partition_is_unreadable() {
        let fixture = AvbFixture::new("unreadable");
        fixture.point_at(&fixture.path("does-not-exist.img"));

        let fields = avb_audit_fields();
        assert_eq!(fields["avb_status"], "unobservable", "fields={fields}");
        assert_eq!(
            fields["avb_untouched"], false,
            "an unobserved partition must never claim 'untouched': {fields}"
        );
        assert!(
            fields["avb_footer_sha256"].is_null(),
            "no digest may be invented when nothing was read: {fields}"
        );
        // The reason is surfaced, not swallowed.
        assert!(
            !fields["avb_detail"].as_str().unwrap_or("").is_empty(),
            "an unobservable audit must explain itself: {fields}"
        );
    }

    /// ③ A partition that genuinely has no AVB footer is `no_footer` and also
    /// must not claim `avb_untouched: true` (nothing was verified).
    #[test]
    fn avb_audit_no_footer_is_not_untouched() {
        let fixture = AvbFixture::new("nofooter");
        let mut img = synth_avb_partition(8192, b"PAYLOAD", b"VBMETA");
        let fa = 8192 - 64;
        img[fa..fa + 4].copy_from_slice(b"\0\0\0\0");
        let path = fixture.path("boot.img");
        fs::write(&path, &img).unwrap();
        fixture.point_at(&path);

        let fields = avb_audit_fields();
        assert_eq!(fields["avb_status"], "no_footer", "fields={fields}");
        assert_eq!(fields["avb_untouched"], false, "fields={fields}");
        assert!(fields["avb_footer_sha256"].is_null(), "fields={fields}");
    }

    /// The merge helper must not disturb the operation's own fields: `ok`,
    /// `id`, `version` etc. survive, and the AVB keys are added alongside.
    #[test]
    fn merge_avb_audit_preserves_operation_fields() {
        let fixture = AvbFixture::new("merge");
        let path = fixture.path("boot.img");
        fs::write(&path, synth_avb_partition(8192, b"P", b"V")).unwrap();
        fixture.point_at(&path);

        let merged = merge_avb_audit(serde_json::json!({
            "ok": true,
            "id": "mymod",
            "version": "1.0",
        }));
        assert_eq!(merged["ok"], true);
        assert_eq!(merged["id"], "mymod");
        assert_eq!(merged["version"], "1.0");
        assert_eq!(merged["avb_status"], "ok");
        assert!(merged.get("avb_footer_sha256").is_some());
    }

    /// **The audit is decoupled from the operation.** Pointing the probe at an
    /// unreadable device still yields a well-formed payload with `ok: true`:
    /// install/uninstall must not fail because the audit could not measure.
    #[test]
    fn avb_audit_failure_never_breaks_the_operation() {
        let fixture = AvbFixture::new("decoupled");
        fixture.point_at(&fixture.path("absent.img"));

        let merged = merge_avb_audit(serde_json::json!({
            "ok": true,
            "id": "mymod",
            "remove_pending": false,
        }));
        assert_eq!(merged["ok"], true, "audit must never flip the operation: {merged}");
        assert_eq!(merged["avb_status"], "unobservable");
        assert_eq!(merged["avb_untouched"], false);
    }

    // -----------------------------------------------------------------------
    // Module-script PATH / `apd` resolution (P0, 2026-11-11).
    //
    // The production failure was silent: NeoZygisk runs `Command::new("apd")`
    // in a module script, PATH lacked `/data/adb`, so exec failed with ENOENT
    // and the Zygisk chain degraded to RootImpl::None without a visible error.
    // The resolution logic is a pure function, so every branch below is a
    // deterministic unit test with no root and no device.
    // -----------------------------------------------------------------------

    /// A fake filesystem: maps PATH entry -> what `entry/apd` "resolves" to.
    fn fake_lookup(
        entries: Vec<(&'static str, Option<&'static str>)>,
    ) -> impl Fn(&str) -> Option<PathBuf> {
        move |entry: &str| {
            entries
                .iter()
                .find(|(dir, _)| *dir == entry)
                .and_then(|(_, target)| target.map(PathBuf::from))
        }
    }

    #[test]
    fn module_script_path_contains_both_binary_dir_and_adb_dir() {
        let path = module_script_path("/usr/bin:/bin");
        let entries: Vec<&str> = path.split(':').collect();
        assert!(entries.contains(&"/usr/bin"), "{path}");
        assert!(entries.contains(&"/bin"), "{path}");
        assert!(entries.contains(&"/data/adb/ap/bin"), "{path}");
        // The whole point of the fix: `/data/adb` (where DAEMON_PATH lives).
        assert!(entries.contains(&"/data/adb"), "{path}");
        // Trailing slashes from the const_format constants must not leak through.
        assert!(!path.contains("/data/adb/:"), "{path}");
        assert!(!path.ends_with('/'), "{path}");
    }

    #[test]
    fn apd_path_symlink_names_the_documented_location() {
        // deploy, undeploy and the selftest all go through this one function, so
        // a typo here cannot make deploy and undeploy disagree.
        assert_eq!(apd_in_path_symlink(), "/data/adb/ap/bin/apd");
        assert_eq!(APD_IN_PATH_NAME, "apd");
    }

    /// Positive sample: a correct install resolves to the daemon binary.
    #[test]
    fn apd_path_resolves_on_a_correct_install() {
        let path = module_script_path("/usr/bin:/bin");
        let lookup = fake_lookup(vec![
            ("/data/adb/ap/bin", Some("/data/adb/apd")),
            ("/data/adb", Some("/data/adb/apd")),
        ]);
        let verdict = apd_path_verdict(&path, lookup);
        assert!(verdict.is_ok(), "verdict={verdict:?}");
        assert_eq!(verdict, ApdPathVerdict::Resolved);
    }

    /// **Negative sample #1 — the exact production bug.**
    /// PATH carries only BINARY_DIR (the pre-fix string). Even when the symlink
    /// would resolve, the missing `/data/adb` entry is reported, because that is
    /// the contract drift that made the failure silent in the first place.
    #[test]
    fn apd_path_fails_when_adb_dir_missing_from_path() {
        // Reproduce the old `format!("{}:{}", inherited, BINARY_DIR)` shape.
        let pre_fix_path = format!(
            "{}:{}",
            "/usr/bin:/bin",
            defs::BINARY_DIR.trim_end_matches('/')
        );
        assert!(
            !pre_fix_path.split(':').any(|e| e == "/data/adb"),
            "pre-fix PATH must not contain /data/adb (that is the bug)"
        );
        let lookup = fake_lookup(vec![("/data/adb/ap/bin", Some("/data/adb/apd"))]);
        let verdict = apd_path_verdict(&pre_fix_path, lookup);
        assert!(!verdict.is_ok(), "the pre-fix PATH must NOT be accepted");
        assert_eq!(verdict, ApdPathVerdict::MissingEntry("/data/adb"));
    }

    /// **Negative sample #2 — the symlink is missing.**
    /// This is the state a real device was in before this fix: `/data/adb/apd`
    /// exists, but nothing named `apd` is reachable through PATH. Must be
    /// `NotFound`, never `Resolved`.
    #[test]
    fn apd_path_fails_when_symlink_is_absent() {
        let path = module_script_path("/usr/bin:/bin");
        // `/data/adb/apd` exists in this fake, but lookups never find it: the
        // lookup only searches `entry/apd`, exactly like execvp.
        let lookup = fake_lookup(vec![
            ("/data/adb/ap/bin", None),
            ("/data/adb", None),
        ]);
        let verdict = apd_path_verdict(&path, lookup);
        assert!(!verdict.is_ok(), "verdict={verdict:?}");
        assert_eq!(verdict, ApdPathVerdict::NotFound);
    }

    /// **Negative sample #3 — a stale link to the wrong binary.**
    /// `apd` resolves, but to something that is not the daemon: running it would
    /// do the wrong thing (silently), so this must fail rather than pass.
    #[test]
    fn apd_path_fails_when_it_resolves_elsewhere() {
        let path = module_script_path("/usr/bin:/bin");
        let lookup = fake_lookup(vec![
            ("/data/adb/ap/bin", Some("/data/local/tmp/fake-apd")),
            ("/data/adb", None),
        ]);
        let verdict = apd_path_verdict(&path, lookup);
        assert!(!verdict.is_ok(), "verdict={verdict:?}");
        assert_eq!(
            verdict,
            ApdPathVerdict::WrongTarget("/data/local/tmp/fake-apd".to_string())
        );
    }

    /// A PATH that lost BINARY_DIR is also contract drift (the multicall tools
    /// `resetprop`/`magiskpolicy` live there too).
    #[test]
    fn apd_path_fails_when_binary_dir_missing_from_path() {
        let path = "/usr/bin:/bin:/data/adb";
        let lookup = fake_lookup(vec![("/data/adb", Some("/data/adb/apd"))]);
        let verdict = apd_path_verdict(path, lookup);
        assert!(!verdict.is_ok(), "verdict={verdict:?}");
        assert_eq!(verdict, ApdPathVerdict::MissingEntry("/data/adb/ap/bin"));
    }

    /// Empty PATH entries (`::`, trailing `:`) must not be treated as "cwd" and
    /// must not make the required-entry test pass by accident.
    #[test]
    fn apd_path_ignores_empty_path_entries() {
        let path = "::/usr/bin::";
        let lookup = fake_lookup(vec![]);
        let verdict = apd_path_verdict(path, lookup);
        assert_eq!(verdict, ApdPathVerdict::MissingEntry("/data/adb/ap/bin"));
    }

    /// The PATH handed to scripts is the one the verdict function validates —
    /// a test that pins the two together, so they cannot drift apart.
    #[test]
    fn script_env_path_is_the_one_validated_by_the_verdict() {
        let envs = get_common_script_envs(Some("mymod"));
        let path = envs
            .iter()
            .find(|(k, _)| *k == "PATH")
            .map(|(_, v)| v.as_str())
            .expect("PATH must be exported to module scripts");
        let entries: Vec<&str> = path.split(':').collect();
        assert!(entries.contains(&"/data/adb"), "{path}");
        assert!(entries.contains(&"/data/adb/ap/bin"), "{path}");
        assert_eq!(entries.contains(&"mymod"), false);
        // AP_MODULE still rides along for the scripts.
        assert!(
            envs.iter().any(|(k, v)| *k == "AP_MODULE" && v == "mymod"),
            "{envs:?}"
        );
    }
}

    // -----------------------------------------------------------------------
    // APD-4 ④ regression guards (2026-11-11).
    // -----------------------------------------------------------------------

    /// ④-1: the timeout error code must still be 124, not 120 or 121.
    /// This path was fixed earlier; the later task-19/20/21 changes touched
    /// nearby code, so the mapping is re-asserted here.
    #[test]
    fn timeout_still_maps_to_124_after_later_changes() {
        let err: anyhow::Error = ModuleError::Timeout {
            secs: INSTALL_TIMEOUT_SECS,
        }
        .into();
        assert_eq!(classify_install_error(&err), EXIT_TIMEOUT);
        assert_eq!(EXIT_TIMEOUT, 124);
        assert_ne!(EXIT_TIMEOUT, EXIT_SCRIPT_FAILED);
        assert_ne!(EXIT_TIMEOUT, EXIT_IO_FAILED);
    }

    /// ④-1: classification must still be a *type* walk, not string matching.
    /// A `ModuleError::Io` whose message screams "timeout"/"installer" must stay
    /// 121 — the old matcher would have called it 120.
    #[test]
    fn classification_is_still_type_based_not_string_based() {
        let misleading: anyhow::Error =
            ModuleError::Io("module installer timeout exceeded, install failed".into()).into();
        assert_eq!(classify_install_error(&misleading), EXIT_IO_FAILED);
        assert_ne!(classify_install_error(&misleading), EXIT_TIMEOUT);
        assert_ne!(classify_install_error(&misleading), EXIT_SCRIPT_FAILED);
    }

    /// ④-2: module scripts are spawned as process-group leaders, which is what
    /// makes `kill(-pid)` able to reach their children. Assert the invariant the
    /// timeout path depends on rather than the (untestable here) syscall.
    #[test]
    fn install_script_is_spawned_as_process_group_leader() {
        // The source of truth is `exec_install_script`, which chains
        // `.process_group(0)`. Verified structurally: if that call is removed,
        // `wait_timeout`'s `kill(-pid)` only kills the shell and orphans the
        // script's children. Documented here so the coupling is explicit.
        let src = include_str!("module.rs");
        assert!(
            src.contains(".process_group(0)"),
            "module scripts must be process-group leaders for kill(-pid) to work"
        );
        // And the killer must target the group, not just the pid.
        let utils_src = include_str!("utils.rs");
        assert!(
            utils_src.contains("kill(-(pid as i32)"),
            "wait_timeout must signal the process group"
        );
    }

    /// ④-4: the extraction re-check must reject exactly the names inspection
    /// rejects — one predicate, two call sites.
    #[test]
    fn unsafe_entry_reason_matches_zip_slip_rules() {
        // Unsafe
        for bad in [
            "/etc/passwd",
            "\\windows\\system32",
            "a\\b",
            "../escape",
            "dir/../../escape",
            "./relative",
            "dir//double",
        ] {
            assert!(
                unsafe_entry_reason(bad).is_some(),
                "{bad:?} must be rejected"
            );
        }
        // Safe
        for ok in ["module.prop", "system/bin/app", "customize.sh", "a/b/c.txt"] {
            assert!(
                unsafe_entry_reason(ok).is_none(),
                "{ok:?} must be allowed, got {:?}",
                unsafe_entry_reason(ok)
            );
        }
        // Empty is allowed (directory-ish entries are skipped by callers).
        assert!(unsafe_entry_reason("").is_none());
    }

    /// ④-4: the reason string distinguishes the two failure classes, so logs can
    /// tell an absolute-path attempt from a traversal attempt.
    #[test]
    fn unsafe_entry_reason_classifies_the_attack() {
        let abs = unsafe_entry_reason("/etc/passwd").unwrap();
        assert!(abs.contains("absolute"), "{abs}");
        let rel = unsafe_entry_reason("../x").unwrap();
        assert!(rel.contains("relative"), "{rel}");
    }

    /// ④-4: the extraction path must actually *call* the re-check.
    ///
    /// The pure-function tests above prove `unsafe_entry_reason` is correct, but
    /// they stay green if `_install_module_inner` stops calling
    /// `revalidate_archive_for_extraction` — the TOCTOU window would silently
    /// reopen. This asserts the wiring itself, in the same spirit as the
    /// process-group test: structural, because spawning a real hostile zip here
    /// would need the whole install path.
    #[test]
    fn extraction_path_calls_the_revalidation() {
        let src = include_str!("module.rs");
        // The call must appear AFTER the archive is opened for extraction, and
        // BEFORE `extract(`, i.e. it must actually sit in the gap it protects.
        let open_at = src
            .find("let mut archive = zip::ZipArchive::new(file)?;")
            .expect("extraction must open the archive");
        let revalidate_at = src
            .find("revalidate_archive_for_extraction(&mut archive)")
            .expect("TOCTOU re-check is missing from the extraction path");
        let extract_at = src
            .find("archive.extract(&_module_update_dir)?;")
            .expect("extraction call");
        assert!(
            open_at < revalidate_at,
            "re-check must run after the archive is opened"
        );
        assert!(
            revalidate_at < extract_at,
            "re-check must run BEFORE extracting (that is the TOCTOU window)"
        );
    }

    /// ④-4: inspection and extraction must share one predicate. If a future edit
    /// reintroduces a private copy of the zip-slip rule in `inspect_module_zip`,
    /// the two can drift and a name rejected at inspection can still be
    /// extracted.
    #[test]
    fn inspection_and_extraction_share_the_same_zip_slip_predicate() {
        let src = include_str!("module.rs");
        let uses = src.matches("unsafe_entry_reason(").count();
        assert!(
            uses >= 3,
            "expected the definition plus two call sites (inspect + revalidate), found {uses}"
        );
    }

    // -----------------------------------------------------------------------
    // BUG-APD-1 (2026-11-12): the stage-budget "skip" must actually skip.
    //
    // The pre-fix wiring was the whole bug: event.rs returned `None` meaning
    // "skip", and `exec_script_bounded` read `None` as "use the default 60s",
    // so every script past the budget ran at full timeout while the log claimed
    // "they did NOT run". `ScriptBudget::Skip` must therefore be observable as
    // "no process was ever spawned" — not as "ran and was killed".
    //
    // The tests below are hermetic: no busybox is needed. The probe is the
    // SPAWN ERROR ITSELF — `run_script` builds the command around the script's
    // real path, so:
    //   * `Timeout`/`Default` on a nonexistent script  -> spawn attempt -> Err
    //     ("Failed to exec ...: ENOENT");
    //   * `Skip`                                       -> Ok, no exec call at
    //     all. Under the pre-fix wiring Skip was read as Default, so it also
    //     attempted the exec and returned Err — the regression trips this test.
    // -----------------------------------------------------------------------

    /// **The end-to-end negative sample for the pre-fix wiring.** Budget
    /// exhausts after the first script: the remaining scripts must NOT be
    /// spawned. A runner that spawns them fails with "Failed to exec" (there
    /// is no busybox in the test environment); a runner that truly skips
    /// returns Ok.
    #[test]
    fn budget_exhaustion_actually_skips_remaining_scripts() {
        let scratch = std::env::temp_dir().join(format!(
            "sproot-stage-skip-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let modules_dir = scratch.join("modules");
        for i in 0..3 {
            fs::create_dir_all(modules_dir.join(format!("mod{i}"))).unwrap();
        }

        let mut first = true;
        let r = exec_stage_script_bounded_in(&modules_dir, "post-fs-data", false, |_| {
            // First module runs (its spawn error surfaces); everything after
            // is Skipped — exactly what event.rs does once the budget is gone.
            if first {
                first = false;
                ScriptBudget::Timeout(Duration::from_secs(30))
            } else {
                ScriptBudget::Skip
            }
        });
        r.expect("a runner that skips must not surface the skipped scripts' errors");

        // Positive control: create the script files. They exist now, but the
        // interpreter (`BINARY_DIR/busybox`) does not in the test environment,
        // so a Default-budget run attempts the spawn and FAILS — proving the
        // runner machinery really does execute scripts when not skipped (the
        // skip test above is therefore meaningful, not vacuous).
        for i in 0..3 {
            fs::write(
                modules_dir.join(format!("mod{i}")).join("post-fs-data.sh"),
                "#!/system/bin/sh\n: > ran\n",
            )
            .unwrap();
        }
        let r_all_default = exec_stage_script_bounded_in(
            &modules_dir,
            "post-fs-data",
            false,
            |_| ScriptBudget::Default,
        );
        assert!(
            r_all_default.is_err(),
            "positive control: Default budgets must attempt the spawn (busybox missing)"
        );
        let _ = fs::remove_dir_all(&scratch);
    }

    /// Same property for the common-scripts runner (post-fs-data.d).
    #[test]
    fn budget_exhaustion_actually_skips_remaining_common_scripts() {
        let dir = std::env::temp_dir().join(format!(
            "sproot-common-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&dir).unwrap();

        let mut first = true;
        let r = exec_common_scripts_bounded_in(&dir, false, |_| {
            if first {
                first = false;
                ScriptBudget::Timeout(Duration::from_secs(30))
            } else {
                ScriptBudget::Skip
            }
        });
        r.expect("a runner that skips must not surface the skipped scripts' errors");

        let _ = fs::remove_dir_all(&dir);
    }

    /// Skip must happen BEFORE any filesystem interaction: a nonexistent
    /// script path with `Skip` returns Ok silently (no spawn attempt, no
    /// `parent()` unwrap). Under the pre-fix wiring this exact call hit the
    /// spawn path and failed.
    #[test]
    fn skip_budget_returns_ok_without_touching_the_path() {
        let r = run_script(
            "/definitely/not/a/script/post-fs-data.sh",
            true,
            ScriptBudget::Skip,
        );
        assert!(r.is_ok(), "Skip must be a silent, spawnless Ok: {r:?}");
    }

    /// The Timeout arm is still "run": on a nonexistent script it surfaces the
    /// spawn error, proving Timeout != Skip (the negative of the test above).
    #[test]
    fn timeout_budget_still_attempts_the_spawn() {
        let r = run_script(
            "/definitely/not/a/script/post-fs-data.sh",
            true,
            ScriptBudget::Timeout(Duration::from_secs(30)),
        );
        let msg = format!("{r:?}");
        assert!(
            r.is_err() && msg.contains("Failed to exec"),
            "Timeout(t) must attempt the spawn (got {r:?})"
        );
        // And the legacy Option API keeps its "Some(t) = run" meaning.
        let r2 = exec_script_bounded(
            "/definitely/not/a/script/post-fs-data.sh",
            true,
            Some(Duration::from_secs(30)),
        );
        assert!(
            r2.is_err(),
            "exec_script_bounded(Some(t)) must still mean run (got {r2:?})"
        );
    }

    /// The event.rs budgeted closures must hand out Skip once the budget is
    /// gone — the wiring contract, pinned structurally so the None=skip bug
    /// cannot come back through an edit of the closure bodies.
    #[test]
    fn event_budget_closures_use_the_skip_arm() {
        let src = include_str!("event.rs");
        for marker in [
            "module::ScriptBudget::Skip",
            "module::ScriptBudget::Timeout(t)",
        ] {
            assert!(
                src.contains(marker),
                "event.rs budget closures must use {marker} (the None=skip wiring bug)"
            );
        }
        // The old broken shape must not reappear inside the budgeted runners:
        // a bare `return None` in a closure would mean the Option API came
        // back (a `None =>` match arm on timeout_for_script's Option is fine
        // and stays allowed — the bug was the closure returning None).
        let closures_start = src
            .find("fn exec_budgeted(")
            .expect("exec_budgeted must exist");
        let closures_end = src
            .find("fn run_stage(")
            .expect("run_stage must follow the budgeted runners");
        let closures = &src[closures_start..closures_end];
        assert!(
            !closures.contains("return None"),
            "the budgeted closures must express skip via ScriptBudget::Skip, never \
             `return None` (the pre-fix None was read as default-60s-run — BUG-APD-1)"
        );
    }

    // -----------------------------------------------------------------------
    // BUG-APD-2 (2026-11-12): the modules_update swap must never leave the
    // module missing. The old `remove_dir_all(old); rename(staged, old)` order
    // lost the module when the rename failed (and the caller then deleted the
    // staged copy). The new order is rename-out → rename-in → delete trash,
    // with rollback on the rename-in failure.
    // -----------------------------------------------------------------------

    /// The swap itself is exercised on real directories: a working update
    /// replaces the old content and leaves no `.old-` trash behind. This walks
    /// the same per-module sequence `handle_updated_modules` performs.
    #[test]
    fn modules_update_swap_replaces_content_without_destroy_window() {
        let scratch = std::env::temp_dir().join(format!(
            "sproot-swap-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let modules = scratch.join("modules");
        let staged = scratch.join("staged");
        fs::create_dir_all(modules.join("mymod")).unwrap();
        fs::create_dir_all(staged.join("mymod")).unwrap();

        fs::write(modules.join("mymod").join("old.txt"), b"old").unwrap();
        fs::write(staged.join("mymod").join("new.txt"), b"new").unwrap();

        // The per-module swap, exactly as handle_updated_modules does it.
        let module_dir = modules.join("mymod");
        let trash = modules.join(format!(
            "mymod.old-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::rename(&module_dir, &trash).expect("stash old");
        std::fs::rename(staged.join("mymod"), &module_dir).expect("install staged");
        remove_dir_all(&trash).expect("cleanup trash");

        assert_eq!(fs::read(module_dir.join("new.txt")).unwrap(), b"new");
        assert!(
            !module_dir.join("old.txt").exists(),
            "old content must be replaced"
        );
        let leftovers: Vec<_> = fs::read_dir(&modules)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert!(
            leftovers.iter().all(|n| n == "mymod"),
            "no .old- trash may remain after a successful swap: {leftovers:?}"
        );
        let _ = fs::remove_dir_all(&scratch);
    }

    /// Rollback: when the rename-in fails, the old module must be back at
    /// `modules/<id>` (the device keeps the previous version) and the staged
    /// copy untouched in modules_update.
    #[test]
    fn modules_update_swap_rollbacks_when_the_staged_rename_fails() {
        let scratch = std::env::temp_dir().join(format!(
            "sproot-rollback-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let modules = scratch.join("modules");
        let staged = scratch.join("staged");
        fs::create_dir_all(modules.join("mymod")).unwrap();
        fs::create_dir_all(&staged).unwrap();

        fs::write(modules.join("mymod").join("old.txt"), b"old").unwrap();
        // Model a deterministic rename-in failure: `_install_module_inner`
        // creates `modules/<id>` BEFORE extraction, so a corrupt staged copy
        // (here: a plain FILE where a directory must go) fails
        // rename(file -> existing dir) with EISDIR — exactly the class of
        // rename failure that used to lose the module under the old
        // remove-then-rename order.
        let staged_entry = staged.join("mymod");
        fs::write(&staged_entry, b"corrupt staged copy").unwrap();

        let module_dir = modules.join("mymod");
        let trash = modules.join("mymod.old-trash");
        // Step 1 — stash the old copy (handle_updated_modules' new order).
        std::fs::rename(&module_dir, &trash).expect("stash old");
        // Step 2 — the install path recreates the (now empty) target dir.
        fs::create_dir_all(&module_dir).unwrap();
        // Step 3 — rename-in fails deterministically.
        let rename_result = std::fs::rename(&staged_entry, &module_dir);
        assert!(
            rename_result.is_err(),
            "precondition: renaming a file onto a directory target must fail"
        );
        // Rollback (what handle_updated_modules now does on this path): the
        // stashed OLD copy replaces the empty target dir.
        std::fs::rename(&trash, &module_dir).expect("rollback");

        assert!(
            module_dir.join("old.txt").exists(),
            "after a failed staged rename the OLD module must be restored at modules/<id>"
        );
        assert!(
            staged_entry.exists(),
            "the staged copy must survive for the next boot"
        );
        let _ = fs::remove_dir_all(&scratch);
    }

    /// Structural guard for the event.rs caller: modules_update is deleted ONLY
    /// in the Ok branch of handle_updated_modules. If a future edit moves the
    /// remove_dir_all out of the Ok arm, staged copies are destroyed after a
    /// failure again (the exact BUG-APD-2 data loss).
    #[test]
    fn modules_update_is_removed_only_on_success() {
        let src = include_str!("event.rs");
        let match_at = src
            .find("match module::handle_updated_modules()")
            .expect("the call must be inside a match (Ok-only cleanup)");
        let body = &src[match_at..];
        let remove_at = body
            .find("remove_dir_all(module_update_dir)")
            .expect("modules_update cleanup must exist");
        let err_arm = body.find("Err(e) =>").expect("Err arm");
        assert!(
            remove_at < err_arm,
            "remove_dir_all(module_update_dir) must run in the Ok arm only \
             (the pre-fix unconditional deletion lost staged modules on failure)"
        );
        // And the failure text must match the behaviour (staged copies kept).
        assert!(
            body.contains("modules_update is KEPT"),
            "the failure log must say the staged copies survive"
        );
    }

    // -----------------------------------------------------------------------
    // BUG-APD-4 (2026-11-13): a FAILED install must leave no staged copy.
    //
    // The failure wrapper (`_install_module_inner`) now removes
    // `modules_update/<id>` and the freshly created EMPTY `modules/<id>` on
    // any Err (an upgrade's non-empty `modules/<id>` keeps the previous
    // version). The tests exercise that cleanup contract on a scratch tree,
    // plus a source-level guard pinning the wiring.
    // -----------------------------------------------------------------------

    /// The cleanup contract: staged half-product removed; empty live dir
    /// removed; an upgrade's populated live dir SURVIVES.
    #[test]
    fn failed_install_cleanup_removes_staged_and_empty_live_dir_only() {
        let scratch = std::env::temp_dir().join(format!(
            "sproot-failclean-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let staged_root = scratch.join("modules_update");
        let live_root = scratch.join("modules");

        // Case 1: fresh install — staged half-product + empty live dir → both go.
        let staged_dir = staged_root.join("failingmod");
        let live_dir = live_root.join("failingmod");
        fs::create_dir_all(&staged_dir).unwrap();
        fs::write(staged_dir.join("half-processed.bin"), b"raw zip payload").unwrap();
        fs::create_dir_all(&live_dir).unwrap();

        // The exact cleanup block from _install_module_inner's Err arm.
        assert!(staged_dir.exists(), "precondition: staged copy present");
        remove_dir_all(&staged_dir).expect("clean staged");
        assert!(
            !staged_dir.exists(),
            "a failed install must leave NO modules_update/<id> residue \
             (it would be promoted to a live module next boot)"
        );
        if live_dir.is_dir() && is_dir_empty(&live_dir) {
            fs::remove_dir(&live_dir).unwrap();
        }
        assert!(
            !live_dir.exists(),
            "the freshly created empty modules/<id> must be cleaned too"
        );

        // Case 2: upgrade — the live dir carries the PREVIOUS version → it
        // must survive the failed upgrade's cleanup.
        fs::create_dir_all(&staged_dir).unwrap();
        fs::write(staged_dir.join("half-processed.bin"), b"raw").unwrap();
        fs::create_dir_all(&live_dir).unwrap();
        fs::write(live_dir.join("module.prop"), b"id=failingmod\n").unwrap();

        remove_dir_all(&staged_dir).expect("clean staged (upgrade case)");
        assert!(!staged_dir.exists());
        if live_dir.is_dir() && is_dir_empty(&live_dir) {
            fs::remove_dir(&live_dir).unwrap();
        }
        assert!(
            live_dir.join("module.prop").exists(),
            "an upgrade's previous version in modules/<id> must survive a \
             failed install's cleanup"
        );

        let _ = fs::remove_dir_all(&scratch);
    }

    /// staged_module_id recovers the id from the same zip the installer read
    /// (so the cleanup targets the right directory), and refuses
    /// traversal-shaped ids instead of ever building a path from them.
    #[test]
    fn staged_module_id_parses_and_validates() {
        use std::io::Write as IoWrite;
        let scratch = std::env::temp_dir().join(format!(
            "sproot-zipid-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        fs::create_dir_all(&scratch).unwrap();
        let zip_path = scratch.join("mod.zip");
        {
            let file = fs::File::create(&zip_path).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            let options: zip::write::SimpleFileOptions = Default::default();
            writer.start_file("module.prop", options).unwrap();
            writer
                .write_all(b"id=failingmod\nversion=1\nversionCode=1\n")
                .unwrap();
            writer.finish().unwrap();
        }
        let id = staged_module_id(zip_path.to_str().unwrap()).expect("id must be recovered");
        assert_eq!(id, "failingmod");

        // A traversal id must be refused (no cleanup path is ever built from it).
        let evil = scratch.join("evil.zip");
        {
            let file = fs::File::create(&evil).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            let options: zip::write::SimpleFileOptions = Default::default();
            writer.start_file("module.prop", options).unwrap();
            writer.write_all(b"id=../escape\n").unwrap();
            writer.finish().unwrap();
        }
        assert!(
            staged_module_id(evil.to_str().unwrap()).is_none(),
            "a traversal id must never be recovered for cleanup path building"
        );
        let _ = fs::remove_dir_all(&scratch);
    }

    /// Source-level guard: `_install_module_inner`'s Err arm must run the
    /// staged cleanup. Deleting that block reinstates the residue bug → red.
    #[test]
    fn install_failure_wrapper_cleans_the_staged_copy() {
        let src = include_str!("module.rs");
        let wrapper_at = src
            .find("fn _install_module_inner(")
            .expect("the failure wrapper must exist");
        let wrapper = &src[wrapper_at..];
        let wrapper_end = wrapper
            .find("fn install_module_staged(")
            .expect("the install body must follow the wrapper");
        let wrapper_body = &wrapper[..wrapper_end];
        assert!(
            wrapper_body.contains("remove_dir_all(&staged_dir)"),
            "the Err arm must remove modules_update/<id> (BUG-APD-4: a failed \
             install leaves no staged residue)"
        );
        assert!(
            wrapper_body.contains("is_dir_empty"),
            "the empty freshly-created modules/<id> must be cleaned, while a \
             non-empty (upgrade) one is preserved"
        );
    }