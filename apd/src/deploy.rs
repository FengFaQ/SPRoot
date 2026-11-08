//! Deployment / bootstrap for SPRoot (contract v1.1 §7, NEW-DOC-05 §6.2).
//!
//! The manager app cannot write `/data/adb` before it has root. The bootstrap
//! therefore works in three stages (NEW-DOC-05 §6.1):
//!
//! 1. patching time  — no root, `/data` untouched (manager only patches boot);
//! 2. first boot     — the manager execs this binary as its own uid, the binary
//!                     self-escalates (`supercall::escalate`), then this module
//!                     performs the 9-step deploy below;
//! 3. second boot    — the injected `init.sproot.rc` starts the daemon and the
//!                     normal module lifecycle works.
//!
//! Every step is idempotent and verified; nothing is written outside the
//! documented paths, and a partially failed deploy leaves no half-installed
//! binary behind (tmp + rename everywhere).

use std::{
    fs,
    io::{self, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use log::{info, warn};
use serde::{Deserialize, Serialize};

use crate::{defs, supercall};

/// Minimum accepted key length: the kernel compares the first 47 characters.
const MIN_KEY_LEN: usize = supercall::KERNEL_KEY_LEN;

/// Random hidden-file name length (contract v1.1 E2: random 16 chars, NOT `su`).
const HIDDEN_NAME_LEN: usize = 16;

#[derive(Serialize, Deserialize, Debug)]
pub struct DeployState {
    pub version: String,
    /// Build commit of the deployed binary; compared by the manager against the
    /// APK-bundled `libapd.so` (version consistency check).
    #[serde(default)]
    pub commit: String,
    pub deployed_at: u64,
    /// Random file name inside `/data/<key16>/`, needed by `undeploy`.
    /// Contains no key material (the directory name is derived from the key
    /// file at undeploy time, never persisted).
    pub hidden_file: String,
    pub source: String,
}

/// Entry point for `apd deploy --source <nativeLibDir> --superkey-file <P>`.
pub fn deploy(source: Option<PathBuf>) -> Result<()> {
    ensure!(supercall::is_root(), "deploy requires root (escalation failed)");

    // The key file passed on the command line is the authority; it is copied to
    // the root-only location and never echoed.
    let key_source = std::env::var("SPROOT_KEY_SOURCE").ok().map(PathBuf::from);
    let key = match key_source.as_deref() {
        Some(path) => supercall::read_key_file(path).context("key file missing or unreadable")?,
        None => supercall::read_key_file(Path::new(defs::SPROOT_KEY_FILE))
            .context("no key available (use --superkey-file)")?,
    };
    validate_key(&key)?;

    let source_dir = match source {
        Some(dir) => dir,
        None => std::env::current_exe()
            .context("cannot resolve own executable")?
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("/")),
    };

    info!("[deploy] step 1/9: directories");
    ensure_layout()?;

    info!("[deploy] step 2/9: AP binary");
    let apd_src = find_component(&source_dir, "apd")?;
    install_file(&apd_src, Path::new(defs::DAEMON_PATH), 0o755)?;

    info!("[deploy] step 3/9: busybox");
    let busybox_src = find_component(&source_dir, "busybox")?;
    install_file(
        &busybox_src,
        &concat_path(defs::BINARY_DIR, "busybox"),
        0o755,
    )?;

    info!("[deploy] step 4/9: multicall symlinks");
    ensure_symlink(
        Path::new(defs::DAEMON_PATH),
        &concat_path(defs::BINARY_DIR, "resetprop"),
    )?;
    ensure_symlink(
        Path::new(defs::DAEMON_PATH),
        &concat_path(defs::BINARY_DIR, "magiskpolicy"),
    )?;

    info!("[deploy] step 5/9: key file");
    write_key_file(&key)?;

    info!("[deploy] step 6/9: hidden su deployment");
    let hidden_file = install_su(&key, &apd_src)?;

    info!("[deploy] step 7/9: allow list");
    init_allow_list()?;
    // Auth channel name (0644, no key material); idempotent on re-deploy so a
    // running daemon keeps its already-bound name.
    crate::daemon::ensure_socket_name()?;

    info!("[deploy] step 8/9: state");
    write_state(&hidden_file, &source_dir)?;

    info!("[deploy] step 9/9: smoke checks");
    smoke_check()?;

    println!("deploy: ok");
    Ok(())
}

/// Entry point for `apd undeploy [--keep-modules]`.
pub fn undeploy(keep_modules: bool) -> Result<()> {
    ensure!(supercall::is_root(), "undeploy requires root");

    // 1. symlink first so no stale entry points at a half-removed binary.
    remove_path(&concat_path(defs::BINARY_DIR, "su"));

    // 2. hidden directory: derive the name from the root key, never persisted.
    if let Some(key) = supercall::read_key_file(Path::new(defs::SPROOT_KEY_FILE)) {
        if key.len() >= 16 {
            let hidden_dir = format!("/data/{}", &key[..16]);
            remove_path(&hidden_dir);
        } else {
            warn!("[undeploy] key file too short, hidden directory left in place");
        }
    } else {
        warn!("[undeploy] key file unavailable, hidden directory left in place");
    }

    // 3. private state (key, safemode flag, boot counter, deploy state).
    remove_path(defs::SPROOT_DIR);

    // 4. working tree. Module data (`/data/adb/modules`, configs) is user data:
    //    only removed when the caller did not ask to keep it.
    remove_path(&concat_path(defs::BINARY_DIR, "busybox"));
    remove_path(&concat_path(defs::BINARY_DIR, "resetprop"));
    remove_path(&concat_path(defs::BINARY_DIR, "magiskpolicy"));
    if !keep_modules {
        remove_path(defs::MODULE_DIR);
        remove_path(defs::MODULE_UPDATE_DIR);
        remove_path(defs::METAMODULE_DIR);
    }
    remove_path(defs::WORKING_DIR);

    // 5. the binary itself last.
    remove_path(defs::DAEMON_PATH);

    println!("undeploy: ok");
    Ok(())
}

fn validate_key(key: &str) -> Result<()> {
    let trimmed = key.trim();
    ensure!(
        trimmed.len() >= MIN_KEY_LEN,
        "key too short ({} < {} chars)",
        trimmed.len(),
        MIN_KEY_LEN
    );
    ensure!(
        trimmed.chars().all(|c| c.is_ascii_alphanumeric()),
        "key contains non-alphanumeric characters"
    );
    Ok(())
}

/// `/data/adb/{ap/bin,ap/log,sproot,modules,modules_update}` with 0700/0755.
fn ensure_layout() -> Result<()> {
    fs::create_dir_all(defs::ADB_DIR).ok();
    for dir in [
        defs::WORKING_DIR,
        defs::BINARY_DIR,
        defs::APATCH_LOG_FOLDER,
        defs::MODULE_CONFIG_DIR,
        defs::SPROOT_DIR,
        defs::MODULE_DIR,
        defs::MODULE_UPDATE_DIR,
    ] {
        fs::create_dir_all(dir).with_context(|| format!("create {dir}"))?;
        let mode = if dir == defs::BINARY_DIR { 0o755 } else { 0o700 };
        fs::set_permissions(dir, fs::Permissions::from_mode(mode)).ok();
    }
    Ok(())
}

fn concat_path(dir: &str, name: &str) -> PathBuf {
    Path::new(dir).join(name)
}

/// Copy `src` to `dst` atomically (tmp + rename) and verify byte length.
fn install_file(src: &Path, dst: &Path, mode: u32) -> Result<()> {
    let bytes = fs::read(src).with_context(|| format!("read {}", src.display()))?;
    ensure!(!bytes.is_empty(), "source binary empty: {}", src.display());

    let tmp = dst.with_extension("tmp");
    {
        let mut file = fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        file.write_all(&bytes)?;
        file.set_permissions(fs::Permissions::from_mode(mode))?;
        file.sync_all()?;
    }
    let written = fs::metadata(&tmp)?.len();
    ensure!(
        written == bytes.len() as u64,
        "short write for {}: {written} != {}",
        dst.display(),
        bytes.len()
    );
    fs::rename(&tmp, dst).with_context(|| format!("rename into {}", dst.display()))?;
    Ok(())
}

fn ensure_symlink(target: &Path, link: &Path) -> Result<()> {
    if link.symlink_metadata().is_ok() {
        fs::remove_file(link).ok();
    }
    std::os::unix::fs::symlink(target, link)
        .with_context(|| format!("symlink {} -> {}", link.display(), target.display()))?;
    Ok(())
}

fn write_key_file(key: &str) -> Result<()> {
    let tmp = format!("{}.tmp", defs::SPROOT_KEY_FILE);
    {
        let mut file = fs::File::create(&tmp).with_context(|| format!("create {tmp}"))?;
        file.write_all(key.trim().as_bytes())?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.sync_all()?;
    }
    fs::rename(&tmp, defs::SPROOT_KEY_FILE).with_context(|| "install key file")?;
    Ok(())
}

/// install_su 等价物 (contract v1.1 §3.4/E2): hidden dir + random 16-char file
/// + `/data/adb/ap/bin/su` symlink. Returns the random file name.
fn install_su(key: &str, apd_src: &Path) -> Result<String> {
    ensure!(key.len() >= 16, "key too short for hidden dir prefix");
    let hidden_dir = format!("/data/{}", &key[..16]);

    if Path::new(&hidden_dir).exists() {
        // The directory name is derived from the key, so a re-deploy with the
        // same key reuses it; a key change (re-patch) leaves the old one behind
        // and undeploy with the old key is the documented cleanup.
        info!("[deploy] hidden dir already present, reusing");
    } else {
        fs::create_dir_all(&hidden_dir).with_context(|| format!("create {hidden_dir}"))?;
    }
    fs::set_permissions(&hidden_dir, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("chmod {hidden_dir}"))?;

    let name = random_name(HIDDEN_NAME_LEN);
    let hidden_path = Path::new(&hidden_dir).join(&name);
    install_file(apd_src, &hidden_path, 0o700)?;
    ensure_symlink(&hidden_path, &concat_path(defs::BINARY_DIR, "su"))?;
    Ok(name)
}

fn init_allow_list() -> Result<()> {
    let path = Path::new(defs::ALLOW_LIST_FILE);
    if !path.exists() {
        let mut file =
            fs::File::create(path).with_context(|| format!("create {}", path.display()))?;
        file.write_all(b"pkg,exclude,allow,uid,to_uid,sctx\n")?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.sync_all()?;
    }
    crate::auth_list::prepare_storage()?;
    Ok(())
}

fn write_state(hidden_file: &str, source_dir: &Path) -> Result<()> {
    let state = DeployState {
        version: defs::VERSION_NAME.trim().to_string(),
        commit: defs::VERSION_COMMIT.trim().to_string(),
        deployed_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
        hidden_file: hidden_file.to_string(),
        source: source_dir.display().to_string(),
    };
    let tmp = format!("{}.tmp", defs::SPROOT_STATE_FILE);
    {
        let mut file = fs::File::create(&tmp).with_context(|| format!("create {tmp}"))?;
        file.write_all(serde_json::to_string_pretty(&state)?.as_bytes())?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.sync_all()?;
    }
    fs::rename(&tmp, defs::SPROOT_STATE_FILE).with_context(|| "install state.json")?;
    Ok(())
}

/// Read the deploy state if present (used by `version --json`).
pub fn read_state() -> Option<DeployState> {
    let content = fs::read_to_string(defs::SPROOT_STATE_FILE).ok()?;
    serde_json::from_str(&content).ok()
}

fn smoke_check() -> Result<()> {
    let paths = [
        PathBuf::from(defs::DAEMON_PATH),
        PathBuf::from(defs::SPROOT_KEY_FILE),
        PathBuf::from(defs::ALLOW_LIST_FILE),
        concat_path(defs::BINARY_DIR, "su"),
    ];
    for path in &paths {
        ensure!(
            path.exists(),
            "smoke check failed: {} missing",
            path.display()
        );
    }
    let mode = fs::metadata(defs::SPROOT_KEY_FILE)?.permissions().mode() & 0o777;
    ensure!(mode == 0o600, "key file mode {mode:o} != 600");
    info!("[deploy] smoke checks passed");
    Ok(())
}

/// Locate a component in the source dir: Android extracts jniLibs as
/// `lib<name>.so`, while a plain directory may hold the bare name.
fn find_component(source_dir: &Path, name: &str) -> Result<PathBuf> {
    let candidates = [
        source_dir.join(name),
        source_dir.join(format!("lib{name}.so")),
    ];
    for candidate in &candidates {
        if candidate.is_file() {
            return Ok(candidate.clone());
        }
    }
    bail!(
        "component '{name}' not found in {} (tried {})",
        source_dir.display(),
        candidates
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn random_name(len: usize) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut buf = vec![0u8; len];
    if let Ok(mut file) = fs::File::open("/dev/urandom") {
        use std::io::Read;
        if file.read_exact(&mut buf).is_err() {
            // Fall back to a time-derived seed; never fail deploy on randomness.
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or(0);
            for (i, byte) in buf.iter_mut().enumerate() {
                *byte = (nanos as usize + i * 31) as u8;
            }
        }
    } else {
        warn!("[deploy] /dev/urandom unavailable, using time-seeded name");
    }
    buf.iter()
        .map(|b| ALPHABET[*b as usize % ALPHABET.len()] as char)
        .collect()
}

fn remove_path<P: AsRef<Path>>(path: P) {
    let path = path.as_ref();
    let Ok(meta) = path.symlink_metadata() else {
        return;
    };
    let result = if meta.is_dir() {
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    if let Err(e) = result {
        warn!("[undeploy] remove {} failed: {e}", path.display());
    }
}

/// Kept for the CLI: the deploy entry point needs the key path; AP's global
/// `--superkey-file` is transported through this env var to avoid leaking it
/// into any generated file.
pub fn set_key_source_env(path: &Path) -> io::Result<()> {
    // SAFETY: single-threaded CLI startup, before any child is spawned.
    unsafe { std::env::set_var("SPROOT_KEY_SOURCE", path) };
    Ok(())
}
