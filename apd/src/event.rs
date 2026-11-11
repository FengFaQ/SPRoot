use crate::sepolicy::get_policy_main;
use anyhow::Result;
use libc::SIGPWR;
use log::{info, warn};
use notify::{
    Config, Event, EventKind, INotifyWatcher, RecursiveMode, Watcher,
    event::{ModifyKind, RenameMode},
};
use signal_hook::{consts::signal::*, iterator::Signals};
use std::{
    env, fs,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use crate::{
    assets, auth_list, bootstate, defs, lua, metamodule, module, restorecon,
    supercall::privilege_apd_profile,
    utils::{self, switch_cgroups},
};

// Contract v1.0 §3.4: the kernel su-path registration (0x1111) and the KP
// truncate-remap event reporting are gone. `report_kernel` degenerates to a
// log line with the same call signature so the stage flow reads identically.
fn report_kernel(_superkey: Option<&str>, event: &str, state: &str) {
    info!("[report_kernel] {event}/{state}");
}

/// File-model replacement for the former kernel `refresh_ap_package_list`
/// (contract §3.2): serialise via the listener mutex (single-writer among the
/// two trigger paths), then commit the table through `auth_list` (flock +
/// tmp+rename inside). The mutex stays for compatibility with the original
/// poison-tolerant locking comment; the authoritative serialisation now also
/// lives in the flock.
fn refresh_allow_list(mutex: &std::sync::Arc<std::sync::Mutex<()>>) {
    let _lock = mutex.lock().unwrap_or_else(|e| e.into_inner());
    auth_list::refresh_allow_list();
}

/// Tracks the remaining wall-clock budget for the whole `post-fs-data` stage.
///
/// Why this exists (APD-1, 2026-11-11): `init.sproot.rc` runs
/// `exec -- /data/adb/apd post-fs-data`, and `exec` is init's **blocking**
/// primitive — apd must return before init continues. A per-script timeout
/// (`SCRIPT_TIMEOUT_SECS`) bounds one script but not the stage: N hung modules ×
/// 60 s each block boot for N minutes. This type makes the *stage* bounded.
///
/// Semantics that matter:
///   * the budget is taken once, at stage start, and only ever decreases;
///   * a script's timeout is `min(per_script, budget_left)`;
///   * when the budget is exhausted the caller must **skip** the remaining work
///     and report that, not run it with a zero timeout.
#[derive(Debug)]
pub(crate) struct StageBudget {
    label: &'static str,
    total: Duration,
    started: std::time::Instant,
}

impl StageBudget {
    pub(crate) fn new(label: &'static str, total_secs: u64) -> Self {
        StageBudget {
            label,
            total: Duration::from_secs(total_secs),
            started: std::time::Instant::now(),
        }
    }

    pub(crate) fn elapsed(&self) -> Duration {
        self.started.elapsed()
    }

    /// Remaining budget, saturating at zero (never negative/underflows).
    pub(crate) fn remaining(&self) -> Duration {
        self.total.saturating_sub(self.elapsed())
    }

    /// Whether any budget is left at all.
    pub(crate) fn has_budget(&self) -> bool {
        !self.remaining().is_zero()
    }

    /// Per-script timeout under the remaining budget, or `None` when exhausted.
    pub(crate) fn timeout_for_script(&self, per_script_secs: u64) -> Option<Duration> {
        script_timeout_within_budget(self.remaining(), per_script_secs)
    }

    /// A one-line summary for the end-of-stage log.
    pub(crate) fn summary(&self, skipped: usize) -> String {
        format!(
            "[{}] stage used {:.1}s of {}s budget{}",
            self.label,
            self.elapsed().as_secs_f64(),
            self.total.as_secs(),
            if skipped > 0 {
                format!(
                    ", skipped {skipped} remaining item(s) because the budget ran out \
                     (boot is not blocked; those modules simply did not run this boot)"
                )
            } else {
                String::new()
            }
        )
    }
}

/// Decide the timeout for one script given what is left of the stage budget.
///
/// Returns `None` when there is **no budget left** — the caller must skip the
/// script entirely and report "budget exhausted".
///
/// The `None` (rather than `Some(Duration::ZERO)`) return is load-bearing: a
/// zero timeout reaches `wait_timeout`, which kills the child on the first poll.
/// The log would then read "script timed out", which blames the *script* when
/// the real cause is an exhausted *stage budget*. Keeping the two distinct is
/// what lets an operator tell "this module is broken" from "we ran out of time
/// because earlier modules were slow".
pub(crate) fn script_timeout_within_budget(
    budget_left: Duration,
    per_script_secs: u64,
) -> Option<Duration> {
    if budget_left.is_zero() {
        return None;
    }
    Some(Duration::from_secs(per_script_secs).min(budget_left))
}

/// Wait for a child with a bound, tolerating the "budget consumed it" case.
///
/// Returns `true` when the process exited on its own, `false` when we killed it.
/// Shared by the housekeeping `sh -c` calls so none of them can hang boot.
fn wait_bounded(child: &mut std::process::Child, timeout: Duration, what: &str) -> bool {
    match utils::wait_timeout(child, timeout) {
        Ok(Some(_)) => true,
        Ok(None) => {
            warn!("{what} timed out after {:.1}s, process group killed", timeout.as_secs_f64());
            false
        }
        Err(e) => {
            warn!("{what} wait failed: {e}");
            false
        }
    }
}

pub fn on_post_data_fs(superkey: Option<&str>) -> Result<()> {
    utils::umask(0);
    // APD-1: the whole stage is bounded. `exec` in init is blocking, so an
    // unbounded post-fs-data is an unbounded boot.
    let budget = StageBudget::new("post-fs-data", defs::POST_FS_DATA_BUDGET_SECS);
    let mut skipped_by_budget = 0usize;

    // Boot-loop guard (NEW-DOC-05 §5.5, contract §3.5 addendum): count every
    // boot attempt before any module script can run; two consecutive boots
    // without a boot-completed force safe mode so the device cannot loop.
    let boot_attempt = bootstate::post_fs_data_start();
    info!("[post-fs-data] boot attempt #{boot_attempt}");
    report_kernel(superkey, "post-fs-data", "before");
    use std::process::Stdio;
    #[cfg(unix)]
    // Contract v1.0 §3.4: su-path registration removed (no kernel remap).
    // The daemon is expected to be started with the key already granted via
    // the SK execve hook chain (启动时序分析 determines who execs us).
    privilege_apd_profile(superkey);

    // ---- SELinux policy -------------------------------------------------
    // Both loading the policy and writing it into the kernel can be slow (and
    // `to_file` writes `/sys/fs/selinux/load`, which some devices block on for
    // a long time). Neither can be interrupted from here, so we *measure* them
    // and log the cost — that is what lets an operator prove "this step is why
    // boot was slow" instead of guessing.
    //
    // Both failures are downgraded to `warn!` and the stage continues: under
    // init's `exec` semantics a non-zero exit marks the service as failed, and
    // "the policy did not load" is far less harmful than "boot reported an
    // error". The stage must always return Ok.
    let sepol_started = std::time::Instant::now();
    match get_policy_main(&["magiskpolicy".to_string(), "--live".to_string()]) {
        Ok(mut sepol) => {
            info!(
                "[post-fs-data] policy load took {:.3}s",
                sepol_started.elapsed().as_secs_f64()
            );
            sepol.magisk_rules();
            let apply_started = std::time::Instant::now();
            let applied = sepol.to_file("/sys/fs/selinux/load");
            let apply_elapsed = apply_started.elapsed();
            match applied {
                Ok(()) => info!(
                    "[post-fs-data] policy apply (write /sys/fs/selinux/load) took {:.3}s \
                     [stage elapsed {:.1}s]",
                    apply_elapsed.as_secs_f64(),
                    budget.elapsed().as_secs_f64()
                ),
                // Downgraded from `?`: a policy failure must not fail the stage.
                Err(e) => warn!(
                    "[post-fs-data] policy apply FAILED after {:.3}s: {e} — continuing \
                     without new rules (boot is not blocked)",
                    apply_elapsed.as_secs_f64()
                ),
            }
        }
        // Downgraded from `?` for the same reason.
        Err(e) => warn!(
            "[post-fs-data] policy load/live-policy read FAILED after {:.3}s: {e} — \
             continuing without new rules (boot is not blocked)",
            sepol_started.elapsed().as_secs_f64()
        ),
    }

    info!("Re-privilege apd profile after injecting sepolicy");
    privilege_apd_profile(superkey);

    // Clear all temporary module configs early
    if let Err(e) = crate::module_config::clear_all_temp_configs() {
        warn!("clear temp configs failed: {e}");
    }

    if utils::has_magisk() {
        warn!("Magisk detected, skip post-fs-data!");
        report_kernel(superkey, "post-fs-data", "after");
        info!("{}", budget.summary(0));
        return Ok(());
    }

    // Create log environment
    if !Path::new(defs::APATCH_LOG_FOLDER).exists() {
        fs::create_dir(defs::APATCH_LOG_FOLDER).expect("Failed to create log folder");
        let permissions = fs::Permissions::from_mode(0o700);
        fs::set_permissions(defs::APATCH_LOG_FOLDER, permissions)
            .expect("Failed to set permissions");
    }
    let command_string = format!(
        "rm -rf {}*.old.log; for file in {}*; do mv \"$file\" \"$file.old.log\"; done",
        defs::APATCH_LOG_FOLDER,
        defs::APATCH_LOG_FOLDER
    );
    let mut args = vec!["-c", &command_string];
    // for all file to .old
    // APD-1: was a bare `.wait()` — a filesystem hiccup here blocked boot with
    // no bound at all. This is housekeeping, so a short timeout is generous.
    let housekeeping_timeout = Duration::from_secs(defs::HOUSEKEEPING_TIMEOUT_SECS);
    match utils::run_command("sh", &args, None) {
        Ok(mut child) => {
            let ok = wait_bounded(&mut child, housekeeping_timeout, "log rotation");
            if ok {
                info!("Successfully deleted .old files.");
            } else {
                info!("Failed to delete .old files.");
            }
        }
        Err(e) => warn!("log rotation spawn failed: {e}"),
    }
    let logcat_path = format!("{}logcat.log", defs::APATCH_LOG_FOLDER);
    let dmesg_path = format!("{}dmesg.log", defs::APATCH_LOG_FOLDER);
    let bootlog = match fs::File::create(&dmesg_path) {
        Ok(f) => f,
        Err(e) => {
            // APD-1: downgraded from `?` — failing to open a log file must not
            // turn into a failed post-fs-data (and therefore a failed boot).
            // Still clears the loop counter path honestly and reports.
            warn!(
                "cannot create dmesg log {dmesg_path}: {e} — skipping remaining log setup \
                 and module stages (boot is not blocked)"
            );
            report_kernel(superkey, "post-fs-data", "after");
            info!("{}", budget.summary(skipped_by_budget));
            return Ok(());
        }
    };
    args = vec![
        "-s",
        "9",
        "45s",
        "logcat",
        "-b",
        "main,system,crash",
        "DrmLibFs:S",
        "-f",
        &logcat_path,
        "logcatcher-bootlog:S",
    ];
    let _ = unsafe {
        Command::new("timeout")
            .process_group(0)
            .pre_exec(|| {
                switch_cgroups();
                Ok(())
            })
            .args(args)
            .spawn()
    };
    args = vec!["-s", "9", "120s", "dmesg", "-w"];
    let _result = unsafe {
        Command::new("timeout")
            .process_group(0)
            .pre_exec(|| {
                switch_cgroups();
                Ok(())
            })
            .args(args)
            .stdout(Stdio::from(bootlog))
            .spawn()
    };

    let key = "KERNELPATCH_VERSION";
    match env::var(key) {
        Ok(value) => println!("{}: {}", key, value),
        Err(_) => println!("{} not found", key),
    }

    let key = "KERNEL_VERSION";
    match env::var(key) {
        Ok(value) => println!("{}: {}", key, value),
        Err(_) => println!("{} not found", key),
    }

    let safe_mode = utils::is_safe_mode(superkey);

    if safe_mode {
        // we should still mount modules.img to `/data/adb/modules` in safe mode
        // becuase we may need to operate the module dir in safe mode
        warn!("safe mode, skip common post-fs-data.d scripts");
        // Not redundant with the disable below: ensure_binaries /
        // handle_updated_modules can still fail with `?` before reaching it,
        // and returning early with modules left enabled risks a bootloop.
        if let Err(e) = module::disable_all_modules() {
            warn!("disable all modules failed: {}", e);
        }
    } else {
        // Then exec common post-fs-data scripts (budgeted, APD-1)
        let r = exec_budgeted("post-fs-data.d", &budget, &mut skipped_by_budget, true);
        if let Err(e) = r {
            warn!("exec common post-fs-data scripts failed: {}", e);
        }
    }
    let module_update_dir = defs::MODULE_UPDATE_DIR; //save module place
    let module_dir = defs::MODULE_DIR; // run modules place
    let module_update_flag = Path::new(defs::WORKING_DIR).join(defs::UPDATE_FILE_NAME); // if update ,there will be renewed modules file
    // APD-1: `ensure_binaries` / `handle_updated_modules` used to propagate with
    // `?`, which failed the whole stage. Missing helper binaries is a degraded
    // install, not a reason to report boot-time failure to init.
    if let Err(e) = assets::ensure_binaries() {
        warn!("ensure_binaries failed: {e} — continuing with degraded module support");
    }

    if Path::new(defs::MODULE_UPDATE_DIR).exists() {
        if let Err(e) = module::handle_updated_modules() {
            warn!("handle_updated_modules failed: {e} — new module versions stay in \
                   modules_update for the next boot");
        }
        if let Err(e) = fs::remove_dir_all(module_update_dir) {
            warn!("cannot clean modules_update: {e}");
        }
    }

    if safe_mode {
        warn!("safe mode, skip post-fs-data scripts and disable all modules!");
        if let Err(e) = module::disable_all_modules() {
            warn!("disable all modules failed: {}", e);
        }
        return Ok(());
    }

    if let Err(e) = module::prune_modules() {
        warn!("prune modules failed: {}", e);
    }

    if let Err(e) = restorecon::restorecon() {
        warn!("restorecon failed: {}", e);
    }

    // load sepolicy.rule — bounded by the same stage budget as everything else
    // (APD-4 ④-5): each module costs one full policy parse, so N modules can
    // otherwise eat the entire post-fs-data allowance.
    let sepol_deadline = std::time::Instant::now() + budget.remaining();
    if module::load_sepolicy_rule_bounded(Some(sepol_deadline)).is_err() {
        warn!("load sepolicy.rule failed");
    }

    if let Err(e) = metamodule::exec_mount_script(module_dir) {
        warn!("execute metamodule mount failed: {e}");
    }

    // APD-1: if the budget is already gone, do not even start the module-stage
    // scripts — running them with a degenerate timeout would look like module
    // failures instead of the budget exhaustion it actually is.
    if !budget.has_budget() {
        skipped_by_budget += 1;
        warn!(
            "[post-fs-data] budget exhausted ({:.1}s used of {}s) before module scripts — \
             skipping post-fs-data/post-mount module scripts and lua. Boot is NOT blocked; \
             these modules simply did not run on this boot.",
            budget.elapsed().as_secs_f64(),
            budget.total.as_secs()
        );
    } else {
        // exec modules post-fs-data scripts (budgeted)
        if let Err(e) = exec_stage_budgeted("post-fs-data", &budget, &mut skipped_by_budget, true) {
            warn!("exec post-fs-data scripts failed: {}", e);
        }
        if let Err(e) = lua::exec_stage_lua("post-fs-data", true, superkey.unwrap_or("")) {
            warn!("Failed to exec post-fs-data lua: {}", e);
        }
        // load system.prop
        if let Err(e) = module::load_system_prop() {
            warn!("load system.prop failed: {}", e);
        }
    }

    info!("remove update flag");
    let _ = fs::remove_file(module_update_flag);

    // post-mount runs inside post-fs-data here, so it is bounded by the same
    // budget; once it is spent we skip it rather than starting a stage that
    // would immediately be killed.
    if budget.has_budget() {
        run_stage("post-mount", superkey, true);
    } else {
        warn!("[post-fs-data] budget exhausted, skipping post-mount stage");
    }

    if env::set_current_dir("/").is_err() {
        warn!("failed to chdir to /");
    }
    report_kernel(superkey, "post-fs-data", "after");
    info!("{}", budget.summary(skipped_by_budget));
    // APD-1: always Ok. `exec -- /data/adb/apd post-fs-data` in init makes this
    // process the blocker for the rest of boot; returning Err marks the service
    // failed and, worse, does not unblock anything. Every failure above is
    // downgraded to a warning on purpose.
    Ok(())
}

/// Run `post-fs-data.d` common scripts under the stage budget.
fn exec_budgeted(
    dir: &str,
    budget: &StageBudget,
    skipped: &mut usize,
    wait: bool,
) -> Result<()> {
    let mut exhausted = false;
    let r = module::exec_common_scripts_bounded(dir, wait, |_path| {
        if exhausted {
            return None;
        }
        match budget.timeout_for_script(defs::SCRIPT_TIMEOUT_SECS) {
            Some(t) => Some(t),
            None => {
                exhausted = true;
                *skipped += 1;
                None
            }
        }
    });
    if exhausted {
        warn!(
            "[post-fs-data] budget exhausted inside {dir}: remaining scripts were SKIPPED \
             (not failed) — boot proceeds"
        );
    }
    r
}

/// Run a module stage (`post-fs-data`, `post-mount`) under the stage budget.
fn exec_stage_budgeted(
    stage: &str,
    budget: &StageBudget,
    skipped: &mut usize,
    block: bool,
) -> Result<()> {
    let mut exhausted = false;
    let mut skipped_here = 0usize;
    let r = module::exec_stage_script_bounded(stage, block, |_module| {
        if exhausted {
            skipped_here += 1;
            return None;
        }
        match budget.timeout_for_script(defs::SCRIPT_TIMEOUT_SECS) {
            Some(t) => Some(t),
            None => {
                exhausted = true;
                skipped_here += 1;
                None
            }
        }
    });
    if exhausted {
        *skipped += skipped_here;
        warn!(
            "[post-fs-data] budget exhausted inside stage `{stage}`: skipped {skipped_here} \
             module(s) — they did NOT run and did NOT fail. Boot proceeds."
        );
    }
    r
}

fn run_stage(stage: &str, superkey: Option<&str>, block: bool) {
    utils::umask(0);

    if utils::has_magisk() {
        warn!("Magisk detected, skip {stage}");
        return;
    }

    if utils::is_safe_mode(superkey) {
        warn!("safe mode, skip {stage} scripts");
        if let Err(e) = module::disable_all_modules() {
            warn!("disable all modules failed: {}", e);
        }
        return;
    }

    // execute metamodule stage script first (priority)
    if let Err(e) = metamodule::exec_stage_script(stage, block) {
        warn!("Failed to exec metamodule {stage} script: {e}");
    }

    if let Err(e) = module::exec_common_scripts(&format!("{stage}.d"), block) {
        warn!("Failed to exec common {stage} scripts: {e}");
    }
    if let Err(e) = module::exec_stage_script(stage, block) {
        warn!("Failed to exec {stage} scripts: {e}");
    }
    if let Err(e) = lua::exec_stage_lua(stage, block, superkey.unwrap_or("")) {
        warn!("Failed to exec {stage} lua: {e}");
    }
}

pub fn on_services(superkey: Option<&str>) -> Result<()> {
    info!("on_services triggered!");
    run_stage("service", superkey, false);

    Ok(())
}

fn _removed_run_uid_monitor() {
    // Intentionally removed (contract v1.1 §3.2): the forked `uid-listener`
    // child is replaced by a thread inside `apd daemon`, which owns the single
    // writer lock for the allow list and the single-instance lock for itself.
}

pub fn on_boot_completed(superkey: Option<&str>) -> Result<()> {
    info!("on_boot_completed triggered!");

    run_stage("boot-completed", superkey, false);

    // A boot that reached this stage is healthy: clear the loop counter. The
    // uid-listener used to be forked here; it now lives inside `apd daemon`
    // (single writer, single instance, contract v1.1 §3.2).
    bootstate::boot_completed();
    Ok(())
}

pub fn start_uid_listener() -> Result<()> {
    info!("start_uid_listener triggered!");
    println!("[start_uid_listener] Registering...");

    // create inotify instance
    const SYS_PACKAGES_LIST_TMP: &str = "/data/system/packages.list.tmp";
    let sys_packages_list_tmp = PathBuf::from(&SYS_PACKAGES_LIST_TMP);
    let dir: PathBuf = sys_packages_list_tmp.parent().unwrap().into();

    let (tx, rx) = std::sync::mpsc::channel();
    let tx_clone = tx.clone();
    let mutex = Arc::new(Mutex::new(()));

    {
        let mutex_clone = mutex.clone();
        thread::spawn(move || {
            let mut signals = Signals::new([SIGTERM, SIGINT, SIGPWR]).unwrap();
            if let Some(sig) = signals.forever().next() {
                log::warn!("[shutdown] Caught signal {sig}, refreshing package list...");
                refresh_allow_list(&mutex_clone);
            }
        });
    }

    let mut watcher = INotifyWatcher::new(
        move |ev: notify::Result<Event>| match ev {
            Ok(Event {
                kind: EventKind::Modify(ModifyKind::Name(RenameMode::Both)),
                paths,
                ..
            }) => {
                if paths.contains(&sys_packages_list_tmp) {
                    info!("[uid_monitor] System packages list changed, sending to tx...");
                    tx_clone.send(false).unwrap()
                }
            }
            Err(err) => warn!("inotify error: {err}"),
            _ => (),
        },
        Config::default(),
    )?;

    watcher.watch(dir.as_ref(), RecursiveMode::NonRecursive)?;

    let mut debounce = false;
    while let Ok(delayed) = rx.recv() {
        if delayed {
            debounce = false;
            refresh_allow_list(&mutex);
            report_kernel(None, "uid_listener", "package-list-updated");
        } else if !debounce {
            thread::sleep(Duration::from_secs(1));
            debounce = true;
            tx.send(true)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // APD-1 stage-budget decision function (2026-11-11).
    //
    // The whole point of these tests is the `None` branch. A stage budget that
    // degenerates into `Some(Duration::ZERO)` would call `wait_timeout` with a
    // zero deadline, which kills the child on the first poll - the log then says
    // "script timed out", blaming an innocent script and hiding the real cause
    // ("we ran out of stage budget"). Keeping the two outcomes distinct is what
    // makes post-fs-data diagnosable.
    // -----------------------------------------------------------------------

    /// Positive: plenty of budget left -> the plain per-script timeout.
    #[test]
    fn budget_returns_per_script_when_ample() {
        let got = script_timeout_within_budget(Duration::from_secs(90), 60);
        assert_eq!(got, Some(Duration::from_secs(60)));
    }

    /// Positive: the budget is the binding constraint -> it clamps the timeout.
    #[test]
    fn budget_clamps_per_script_when_short() {
        let got = script_timeout_within_budget(Duration::from_secs(10), 60);
        assert_eq!(got, Some(Duration::from_secs(10)));
    }

    /// **Negative sample (the important one): zero budget must be `None`.**
    /// Returning `Some(0)` would make `wait_timeout` kill immediately and be
    /// indistinguishable from a genuinely failing script.
    #[test]
    fn budget_zero_is_none_not_zero_timeout() {
        let got = script_timeout_within_budget(Duration::ZERO, 60);
        assert_eq!(got, None, "zero budget must skip, not run with a 0s timeout");
        assert_ne!(
            got,
            Some(Duration::ZERO),
            "Some(0) would masquerade as a script timeout"
        );
    }

    /// Exhausted budget (measured as elapsed > total) saturates to `None`, and
    /// `StageBudget::remaining` must never underflow.
    #[test]
    fn budget_exhausted_saturates_to_none() {
        let b = StageBudget {
            label: "test",
            total: Duration::from_millis(1),
            started: std::time::Instant::now() - Duration::from_secs(5),
        };
        assert!(b.remaining().is_zero(), "remaining must saturate at zero");
        assert!(!b.has_budget());
        assert_eq!(b.timeout_for_script(60), None);
    }

    /// A fresh budget has its full allowance and reports a budget.
    #[test]
    fn budget_starts_full() {
        let b = StageBudget::new("test", 90);
        assert!(b.has_budget());
        assert!(b.remaining() <= Duration::from_secs(90));
        assert_eq!(b.timeout_for_script(60), Some(Duration::from_secs(60)));
    }

    /// The clamp is monotonic: a smaller budget never yields a *larger* timeout.
    #[test]
    fn budget_timeout_is_monotonic_in_budget() {
        let per = 60u64;
        let mut prev = Duration::ZERO;
        for secs in [0u64, 1, 5, 30, 60, 61, 120] {
            let t = script_timeout_within_budget(Duration::from_secs(secs), per);
            match t {
                None => assert_eq!(secs, 0, "only a zero budget may skip"),
                Some(d) => {
                    assert!(d <= Duration::from_secs(per), "must not exceed per-script");
                    assert!(d <= Duration::from_secs(secs), "must not exceed the budget");
                    assert!(d >= prev, "timeout must not shrink as budget grows");
                    prev = d;
                }
            }
        }
    }

    /// Sub-second budgets are respected rather than rounded away to zero.
    #[test]
    fn budget_subsecond_is_honoured() {
        let got = script_timeout_within_budget(Duration::from_millis(500), 60);
        assert_eq!(got, Some(Duration::from_millis(500)));
        assert_ne!(got, Some(Duration::from_secs(1)));
    }

    /// The stage summary must name the budget and the skipped count, so the log
    /// can explain *why* modules did not run.
    #[test]
    fn budget_summary_mentions_skips() {
        let b = StageBudget::new("post-fs-data", 90);
        let clean = b.summary(0);
        assert!(clean.contains("post-fs-data"), "{clean}");
        assert!(!clean.contains("skipped"), "{clean}");
        let skipped = b.summary(3);
        assert!(skipped.contains("skipped 3"), "{skipped}");
        assert!(skipped.contains("did not run"), "{skipped}");
    }

    /// The constants themselves are the contract the boot-time behaviour rests on.
    #[test]
    fn budget_constants_are_sane() {
        // A whole-stage budget must be >= the per-script timeout, otherwise the
        // per-script limit could never be reached and every script would be
        // truncated by the stage budget alone.
        assert!(
            defs::POST_FS_DATA_BUDGET_SECS >= defs::SCRIPT_TIMEOUT_SECS,
            "stage budget {} < per-script {} - the per-script limit would be dead code",
            defs::POST_FS_DATA_BUDGET_SECS,
            defs::SCRIPT_TIMEOUT_SECS
        );
        // Housekeeping (log rotation) must be far below the stage budget.
        assert!(defs::HOUSEKEEPING_TIMEOUT_SECS < defs::POST_FS_DATA_BUDGET_SECS);
        // The stage must be bounded by something small enough to look like a
        // boot, not a hang. 5 minutes is the outer edge of tolerable.
        assert!(
            defs::POST_FS_DATA_BUDGET_SECS <= 300,
            "stage budget {}s is too long for a boot-blocking phase",
            defs::POST_FS_DATA_BUDGET_SECS
        );
    }
}
