use crate::sepolicy::get_policy_main;
use anyhow::{Context, Result};
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
fn report_kernel(_superkey: Option<String>, event: &str, state: &str) {
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

pub fn on_post_data_fs(superkey: Option<String>) -> Result<()> {
    utils::umask(0);
    // Boot-loop guard (NEW-DOC-05 §5.5, contract §3.5 addendum): count every
    // boot attempt before any module script can run; two consecutive boots
    // without a boot-completed force safe mode so the device cannot loop.
    let boot_attempt = bootstate::post_fs_data_start();
    info!("[post-fs-data] boot attempt #{boot_attempt}");
    report_kernel(superkey.clone(), "post-fs-data", "before");
    use std::process::Stdio;
    #[cfg(unix)]
    // Contract v1.0 §3.4: su-path registration removed (no kernel remap).
    // The daemon is expected to be started with the key already granted via
    // the SK execve hook chain (启动时序分析 determines who execs us).
    privilege_apd_profile(&superkey);

    let mut sepol = get_policy_main(&["magiskpolicy".to_string(), "--live".to_string()])?;
    sepol.magisk_rules();
    sepol
        .to_file("/sys/fs/selinux/load")
        .context("Cannot apply policy")?;

    info!("Re-privilege apd profile after injecting sepolicy");
    privilege_apd_profile(&superkey);

    // Clear all temporary module configs early
    if let Err(e) = crate::module_config::clear_all_temp_configs() {
        warn!("clear temp configs failed: {e}");
    }

    if utils::has_magisk() {
        warn!("Magisk detected, skip post-fs-data!");
        report_kernel(superkey.clone(), "post-fs-data", "after");
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
    let result = utils::run_command("sh", &args, None)?.wait()?;
    if result.success() {
        info!("Successfully deleted .old files.");
    } else {
        info!("Failed to delete .old files.");
    }
    let logcat_path = format!("{}logcat.log", defs::APATCH_LOG_FOLDER);
    let dmesg_path = format!("{}dmesg.log", defs::APATCH_LOG_FOLDER);
    let bootlog = fs::File::create(dmesg_path)?;
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

    let safe_mode = utils::is_safe_mode(superkey.clone());

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
        // Then exec common post-fs-data scripts
        if let Err(e) = module::exec_common_scripts("post-fs-data.d", true) {
            warn!("exec common post-fs-data scripts failed: {}", e);
        }
    }
    let module_update_dir = defs::MODULE_UPDATE_DIR; //save module place
    let module_dir = defs::MODULE_DIR; // run modules place
    let module_update_flag = Path::new(defs::WORKING_DIR).join(defs::UPDATE_FILE_NAME); // if update ,there will be renewed modules file
    assets::ensure_binaries().with_context(|| "binary missing")?;

    if Path::new(defs::MODULE_UPDATE_DIR).exists() {
        module::handle_updated_modules()?;
        fs::remove_dir_all(module_update_dir)?;
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

    // load sepolicy.rule
    if module::load_sepolicy_rule().is_err() {
        warn!("load sepolicy.rule failed");
    }

    if let Err(e) = metamodule::exec_mount_script(module_dir) {
        warn!("execute metamodule mount failed: {e}");
    }

    // exec modules post-fs-data scripts
    // TODO: Add timeout
    if let Err(e) = module::exec_stage_script("post-fs-data", true) {
        warn!("exec post-fs-data scripts failed: {}", e);
    }
    if let Err(e) = lua::exec_stage_lua("post-fs-data", true, superkey.as_deref().unwrap_or("")) {
        warn!("Failed to exec post-fs-data lua: {}", e);
    }
    // load system.prop
    if let Err(e) = module::load_system_prop() {
        warn!("load system.prop failed: {}", e);
    }

    info!("remove update flag");
    let _ = fs::remove_file(module_update_flag);

    run_stage("post-mount", superkey.clone(), true);

    env::set_current_dir("/").with_context(|| "failed to chdir to /")?;
    report_kernel(superkey, "post-fs-data", "after");
    Ok(())
}

fn run_stage(stage: &str, superkey: Option<String>, block: bool) {
    utils::umask(0);

    if utils::has_magisk() {
        warn!("Magisk detected, skip {stage}");
        return;
    }

    if utils::is_safe_mode(superkey.clone()) {
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
    if let Err(e) = lua::exec_stage_lua(stage, block, superkey.as_deref().unwrap_or("")) {
        warn!("Failed to exec {stage} lua: {e}");
    }
}

pub fn on_services(superkey: Option<String>) -> Result<()> {
    info!("on_services triggered!");
    run_stage("service", superkey, false);

    Ok(())
}

fn _removed_run_uid_monitor() {
    // Intentionally removed (contract v1.1 §3.2): the forked `uid-listener`
    // child is replaced by a thread inside `apd daemon`, which owns the single
    // writer lock for the allow list and the single-instance lock for itself.
}

pub fn on_boot_completed(superkey: Option<String>) -> Result<()> {
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
