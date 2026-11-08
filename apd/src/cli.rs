use crate::{
    auth_list, daemon, defs, deploy, event, lua, module, module_config, secret, selftest,
    supercall, utils,
};
#[cfg(target_os = "android")]
use android_logger::Config;
use anyhow::{Context, Result};
use clap::Parser;
#[cfg(target_os = "android")]
use log::LevelFilter;
use std::path::PathBuf;

/// APatch cli
#[derive(Parser, Debug)]
#[command(name = "apd", author, version = defs::VERSION_CODE, about = "SPRoot AP binary (module manager + resident daemon)", long_about = None)]
struct Args {
    #[arg(
        short,
        long,
        value_name = "KEY",
        help = "Root key (48 chars; the first 47 are the SK execve escalation token, contract v1.1 E1)"
    )]
    superkey: Option<String>,
    /// Read the key from a 0600 file instead of argv (启动时序 §3.3: keeps the
    /// key out of init's cmdline; file is mlock'ed and argv wiped after read).
    #[arg(long, value_name = "PATH", conflicts_with = "superkey")]
    superkey_file: Option<PathBuf>,
    #[command(subcommand)]
    command: Commands,
}

#[derive(clap::Subcommand, Debug)]
enum Commands {
    /// Manage APatch modules
    Module {
        #[command(subcommand)]
        command: Module,
    },

    /// Trigger `post-fs-data` event
    PostFsData,

    /// Trigger `service` event
    Services,

    /// Trigger `boot-complete` event
    BootCompleted,

    /// Start uid listener for synchronizing root list (manual/debug; the
    /// resident daemon hosts its own one)
    UidListener,

    /// Resident daemon (init only): services stage + uid-listener + auth socket
    /// + userspace boot-completed detection
    Daemon {
        /// run the read-only self-check and exit instead of daemonising
        #[arg(long)]
        selftest: bool,
    },

    /// First-install deployment: install AP/busybox/key/hidden su/allow list.
    /// Run once root is available (the manager calls it via `root-exec`-style
    /// self-escalation; contract v1.1 §7 / NEW-DOC-05 §6.2).
    Deploy {
        /// directory containing the packaged binaries (usually the APK
        /// nativeLibraryDir: libapd.so / libbusybox.so)
        #[arg(long)]
        source: Option<PathBuf>,
    },

    /// Reverse of deploy: remove AP, hidden su, key and private state
    Undeploy {
        /// keep /data/adb/modules and module configs
        #[arg(long)]
        keep_modules: bool,
    },

    /// Run a command as root. This is the manager's ONLY root entry point
    /// (contract v1.1 §8.2); stdio and exit code are passed through.
    RootExec {
        /// drop to this uid before exec (default: stay root)
        #[arg(long)]
        uid: Option<u32>,
        /// SELinux context for the session (not implemented yet, ignored)
        #[arg(long)]
        scontext: Option<String>,
        /// kill the process group (and exit 124) after this many seconds
        #[arg(long)]
        timeout: Option<u64>,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 1..)]
        command: Vec<String>,
    },

    /// Authorisation table maintenance (contract v1.1 §3.2/§3.6)
    Auth {
        #[command(subcommand)]
        command: AuthCmd,
    },

    /// Machine readable deployment/version status (`--json`)
    Version {
        #[arg(long)]
        json: bool,
    },

    /// Read-only capability/diagnostic self-check (manager capability probe +
    /// QA automation). Exit 0 = all critical checks pass, 1 = failed.
    Selftest {
        #[arg(long)]
        json: bool,
    },

    /// Resetprop - Magisk-compatible system property tool
    #[command(disable_help_flag = true)]
    Resetprop {
        /// Arguments passed to resetprop
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
        args: Vec<String>,
    },

    /// MagiskPolicy - SELinux Policy Patch Tool
    Sepolicy(crate::sepolicy::Args),
}

#[derive(clap::Subcommand, Debug)]
enum AuthCmd {
    /// List the allow list as JSON
    List,
    /// Grant root to <uid> (optionally updating pkg/to_uid/scontext)
    Grant {
        uid: i32,
        #[arg(long, default_value = "")]
        pkg: String,
        #[arg(long, default_value_t = 0)]
        to_uid: i32,
        #[arg(long, default_value = "")]
        scontext: String,
    },
    /// Revoke root from <uid>
    Revoke { uid: i32 },
    /// Mark <uid> as excluded from module effects
    Exclude { uid: i32 },
    /// Clear the module-exclusion bit of <uid>
    Unexclude { uid: i32 },
}

#[derive(clap::Subcommand, Debug)]
enum Module {
    /// Install module <ZIP>
    Install {
        /// module zip file path
        zip: String,
    },

    /// Uninstall module <id>
    Uninstall {
        /// module id
        id: String,
    },

    /// UudoUninstall module <id>
    UndoUninstall {
        /// module id
        id: String,
    },

    /// enable module <id>
    Enable {
        /// module id
        id: String,
    },

    /// disable module <id>
    Disable {
        // module id
        id: String,
    },

    /// run action for module <id>
    Action {
        // module id
        id: String,
    },
    /// module lua runner
    Lua {
        // module id
        id: String,
        // lua function
        function: String,
    },
    /// list all modules
    List {
        /// stable machine-readable schema (id/name/version/author/description/enabled/update_pending)
        #[arg(long)]
        json: bool,
    },

    /// manage module configuration
    Config {
        /// target internal module name (resolved as internal.<name>)
        #[arg(long)]
        internal: Option<String>,
        #[command(subcommand)]
        command: ModuleConfigCmd,
    },
}

#[derive(clap::Subcommand, Debug)]
enum ModuleConfigCmd {
    /// Get a config value
    Get {
        /// config key
        key: String,
    },

    /// Set a config value
    Set {
        /// config key
        key: String,
        /// config value (omit to read from stdin)
        value: Option<String>,
        /// read value from stdin (default if value not provided)
        #[arg(long)]
        stdin: bool,
        /// use temporary config (cleared on reboot)
        #[arg(short, long)]
        temp: bool,
    },

    /// List all config entries
    List,

    /// Delete a config entry
    Delete {
        /// config key
        key: String,
        /// delete from temporary config
        #[arg(short, long)]
        temp: bool,
    },

    /// Clear all config entries
    Clear {
        /// clear temporary config
        #[arg(short, long)]
        temp: bool,
    },
}

pub fn run() -> Result<()> {
    #[cfg(target_os = "android")]
    android_logger::init_once(
        Config::default()
            .with_max_level(LevelFilter::Trace) // limit log level
            .with_tag("APatchD")
            .with_filter(
                android_logger::FilterBuilder::new()
                    .filter_level(LevelFilter::Trace)
                    .filter_module("notify", LevelFilter::Warn)
                    .build(),
            ),
    );

    #[cfg(not(target_os = "android"))]
    env_logger::init();

    // the kernel executes su with argv[0] basename dispatching. Contract
    // v1.1 §3.4: only the `su` branch remains — the SK deploy form puts this
    // binary in the hidden dir; `kp` had no SK counterpart and is removed.
    // Match the basename exactly (not `ends_with`) so e.g. a binary called
    // "kasu" cannot accidentally enter the root shell path.
    let arg0 = std::env::args().next().unwrap_or_default();
    let arg0_base = std::path::Path::new(&arg0)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if arg0_base == "su" {
        return crate::apd::root_shell();
    }
    if arg0_base == "resetprop" {
        let all_args: Vec<String> = std::env::args().collect();
        crate::resetprop::resetprop_main(&all_args)
    }
    if arg0_base == "magiskpolicy" {
        let all_args: Vec<String> = std::env::args().collect();
        crate::sepolicy::policy_main(&all_args)
    }

    let mut cli = Args::parse();

    // Key material (P2): as soon as the key enters the process it is wrapped in
    // a `SecretKey` — mlock'ed pages + volatile wipe on drop. The argv copy is
    // wiped immediately after wrapping (only the path is non-secret).
    let mut superkey = match cli.superkey.take() {
        Some(mut raw) => {
            let key = secret::SecretKey::new(&raw);
            secret::wipe_string(&mut raw);
            Some(key)
        }
        None => None,
    };
    // --superkey-file (启动时序 §3.3): read the key from a 0600 file so the key
    // never appears in init's cmdline.
    if superkey.is_none()
        && let Some(ref path) = cli.superkey_file
    {
        superkey = secret::read_key_file(path);
        if superkey.is_none() {
            // Silent-degrade: rc may retry later (启动时序 §3.2 window). A
            // missing key is not an error surface; never log the path content.
            log::warn!("[superkey-file] unavailable (key not loaded)");
        }
    }

    // deploy needs the key *path* (it copies the key to the root-only store),
    // and it must not end up in any generated file — transport it via env.
    if let Some(path) = cli.superkey_file.clone() {
        let _ = crate::deploy::set_key_source_env(&path);
    }

    log::info!("command: {:?}", cli.command);

    if let Some(key) = superkey.as_ref() {
        supercall::privilege_apd_profile(Some(key.as_str()));
    }

    let result = match cli.command {
        Commands::PostFsData => {
            event::on_post_data_fs(superkey.as_ref().map(secret::SecretKey::as_str))
        }

        Commands::BootCompleted => {
            event::on_boot_completed(superkey.as_ref().map(secret::SecretKey::as_str))
        }

        Commands::UidListener => event::start_uid_listener(),

        Commands::Daemon { selftest } => {
            if selftest {
                selftest::run(false)
            } else {
                daemon::run(superkey.clone())
            }
        }

        Commands::Deploy { source } => deploy::deploy(source),

        Commands::Undeploy { keep_modules } => deploy::undeploy(keep_modules),

        Commands::RootExec {
            uid,
            scontext,
            timeout,
            command,
        } => run_root_exec(uid, scontext, timeout, command),

        Commands::Auth { command } => run_auth(command),

        Commands::Version { json } => print_version(json),

        Commands::Selftest { json } => selftest::run(json),

        Commands::Module { command } => {
            #[cfg(any(target_os = "linux", target_os = "android"))]
            {
                utils::switch_mnt_ns(1)?;
            }
            match command {
                Module::Install { zip } => module::install_module(&zip),
                Module::Uninstall { id } => module::uninstall_module(&id),
                Module::UndoUninstall { id } => module::undo_uninstall_module(&id),
                Module::Action { id } => module::run_action(&id),
                Module::Lua { id, function } => {
                    lua::run_lua(&id, &function, false, true).map_err(|e| anyhow::anyhow!("{}", e))
                }
                Module::Enable { id } => module::enable_module(&id),
                Module::Disable { id } => module::disable_module(&id),
                Module::List { json } => {
                    if json {
                        module::list_modules_json()
                    } else {
                        module::list_modules()
                    }
                }
                Module::Config { internal, command } => {
                    let module_id = match internal {
                        Some(internal_name) => format!("internal.{internal_name}"),
                        None => std::env::var("AP_MODULE").map_err(|_| {
                            anyhow::anyhow!(
                                "This command must be run in the context of a module or passed --internal <name>"
                            )
                        })?,
                    };

                    match command {
                        ModuleConfigCmd::Get { key } => {
                            // Use merge_configs to respect priority (temp overrides persist)
                            let config = module_config::merge_configs(&module_id)?;
                            match config.get(&key) {
                                Some(value) => {
                                    println!("{value}");
                                    Ok(())
                                }
                                None => anyhow::bail!("Key '{key}' not found"),
                            }
                        }
                        ModuleConfigCmd::Set {
                            key,
                            value,
                            stdin,
                            temp,
                        } => {
                            // Validate key at CLI layer for better user experience
                            module_config::validate_config_key(&key)?;

                            // Read value from stdin or argument
                            let value_str = match value {
                                Some(v) if !stdin => v,
                                _ => {
                                    // Read value from stdin
                                    use std::io::Read;
                                    let mut buffer = String::new();
                                    std::io::stdin()
                                        .read_to_string(&mut buffer)
                                        .context("Failed to read from stdin")?;
                                    buffer
                                }
                            };

                            // Validate value
                            module_config::validate_config_value(&value_str)?;

                            let config_type = if temp {
                                module_config::ConfigType::Temp
                            } else {
                                module_config::ConfigType::Persist
                            };
                            module_config::set_config_value(
                                &module_id,
                                &key,
                                &value_str,
                                config_type,
                            )
                        }
                        ModuleConfigCmd::List => {
                            let config = module_config::merge_configs(&module_id)?;
                            if config.is_empty() {
                                println!("No config entries found");
                            } else {
                                for (key, value) in config {
                                    println!("{key}={value}");
                                }
                            }
                            Ok(())
                        }
                        ModuleConfigCmd::Delete { key, temp } => {
                            let config_type = if temp {
                                module_config::ConfigType::Temp
                            } else {
                                module_config::ConfigType::Persist
                            };
                            module_config::delete_config_value(&module_id, &key, config_type)
                        }
                        ModuleConfigCmd::Clear { temp } => {
                            let config_type = if temp {
                                module_config::ConfigType::Temp
                            } else {
                                module_config::ConfigType::Persist
                            };
                            module_config::clear_config(&module_id, config_type)
                        }
                    }
                }
            }
        }

        Commands::Services => {
            event::on_services(superkey.as_ref().map(secret::SecretKey::as_str))
        }

        Commands::Resetprop { args } => {
            let mut full_args = vec!["resetprop".to_string()];
            full_args.extend(args);
            crate::resetprop::resetprop_main(&full_args)
        }

        Commands::Sepolicy(sepolicy_args) => crate::sepolicy::execute(&sepolicy_args),
    };

    if let Err(e) = &result {
        log::error!("Error: {:?}", e);
    }
    result
}

// ---------------------------------------------------------------------------
// root-exec (contract v1.1 §8.2): the manager's only root entry point.
// ---------------------------------------------------------------------------

fn run_root_exec(
    uid: Option<u32>,
    scontext: Option<String>,
    timeout: Option<u64>,
    argv: Vec<String>,
) -> Result<()> {
    // Caller identity: AP self-escalated from `--superkey-file` before dispatch.
    if !supercall::is_root() {
        // 125 = escalation failed (contract §8.5); stay silent otherwise.
        std::process::exit(125);
    }
    if argv.is_empty() {
        eprintln!("root-exec: empty command");
        std::process::exit(2);
    }
    // P2: real per-session SELinux context (no libselinux dependency — the
    // helper writes /proc/self/attr/current with raw libc calls, which is
    // async-signal-safe enough for `pre_exec`).
    let context = match scontext {
        Some(ctx) => match std::ffi::CString::new(ctx.clone()) {
            Ok(con) => Some(con),
            Err(_) => {
                eprintln!("root-exec: invalid scontext");
                std::process::exit(2);
            }
        },
        None => None,
    };

    let mut command = std::process::Command::new(&argv[0]);
    command.args(&argv[1..]);
    // Session leader so a timeout can kill the whole process group.
    use std::os::unix::process::CommandExt;
    command.process_group(0);
    unsafe {
        command.pre_exec(move || {
            // Order matters: switch context while still privileged, then drop.
            if let Some(con) = context.as_ref() {
                crate::selinux::setcon_raw(con)?;
            }
            if let Some(target_uid) = uid
                && (libc::setgid(target_uid) != 0 || libc::setuid(target_uid) != 0)
            {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }

    let mut child = command
        .spawn()
        .with_context(|| format!("spawn {}", argv[0]))?;
    let code = match timeout {
        Some(secs) => match utils::wait_timeout(&mut child, std::time::Duration::from_secs(secs))? {
            Some(status) => status.code().unwrap_or(1),
            None => {
                log::warn!("root-exec: timed out after {secs}s, process group killed");
                124
            }
        },
        None => child.wait()?.code().unwrap_or(1),
    };
    std::process::exit(code);
}

// ---------------------------------------------------------------------------
// auth (contract v1.1 §3.2/§8.3): single writer, flock protected.
// ---------------------------------------------------------------------------

fn run_auth(command: AuthCmd) -> Result<()> {
    if !supercall::is_root() {
        std::process::exit(125);
    }
    match command {
        AuthCmd::List => println!("{}", auth_list::list_json()?),
        AuthCmd::Grant {
            uid,
            pkg,
            to_uid,
            scontext,
        } => auth_list::grant(uid, &pkg, to_uid, &scontext)?,
        AuthCmd::Revoke { uid } => auth_list::revoke(uid)?,
        AuthCmd::Exclude { uid } => auth_list::set_exclude(uid, true)?,
        AuthCmd::Unexclude { uid } => auth_list::set_exclude(uid, false)?,
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// version --json (contract v1.1 §8.3): machine readable, no key material.
// ---------------------------------------------------------------------------

fn print_version(json: bool) -> Result<()> {
    let state = deploy::read_state();
    let deployed = state.is_some() && std::path::Path::new(defs::DAEMON_PATH).exists();
    // key_prefix_sha256 (contract §8.3): an integrity marker, never the key or
    // any fragment of it. Only computable when root can read the key file —
    // otherwise the field is null (the contract's "身份要求: 任意").
    let key_prefix_sha256 = secret::read_key_file(std::path::Path::new(defs::SPROOT_KEY_FILE))
        .and_then(|key| {
            let prefix: String = key.as_str().chars().take(16).collect();
            utils::sha256_hex_stdin(prefix.as_bytes())
        });
    if !json {
        println!("{} ({})", defs::VERSION_NAME.trim(), defs::VERSION_CODE.trim());
        println!("deployed: {deployed}");
        return Ok(());
    }
    let payload = serde_json::json!({
        "version": defs::VERSION_NAME.trim(),
        "version_code": defs::VERSION_CODE.trim(),
        "commit": defs::VERSION_COMMIT.trim(),
        "deployed": deployed,
        "key_prefix_sha256": key_prefix_sha256,
        "daemon_pid": daemon::daemon_pid(),
        "boot_stage_done": daemon::boot_stage_done(),
        "deployed_at": state.as_ref().map(|s| s.deployed_at),
        "hidden_file": state.as_ref().map(|s| s.hidden_file.clone()),
    });
    println!("{}", serde_json::to_string_pretty(&payload)?);
    Ok(())
}
