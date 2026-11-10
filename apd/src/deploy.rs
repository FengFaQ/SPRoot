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
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, bail, ensure};
use log::{info, warn};
use serde::{Deserialize, Serialize};

use crate::{assets, defs, secret, supercall};

/// Key ABI length, **exact**: the AP-side key file holds 47 plain-text bytes and
/// the kernel compares exactly those 47. Anything else is rejected (or, for the
/// documented 48-position display format only, mapped once with a log line)
/// rather than silently truncated — a 48/49-byte input truncated to 47 yields a
/// token the kernel rejects, which turns an ABI mix-up into a silent
/// privilege-escalation failure (HANDOFF §6.8, Skroot-Next lesson).
const KEY_ABI_LEN: usize = supercall::KERNEL_KEY_LEN;

/// Random hidden-file name length (contract v1.1 E2: random 16 chars, NOT `su`).
const HIDDEN_NAME_LEN: usize = 16;

#[derive(Serialize, Deserialize, Debug)]
pub struct DeployState {
    pub version: String,
    /// Build commit of the deployed binary; compared by the manager against the
    /// APK-bundled `libapd.so` (version consistency check).
    #[serde(default)]
    pub commit: String,
    /// AVB audit evidence: deploy never touches vbmeta/AVB structures. The
    /// value is a fixed marker so QA can prove it from state.json.
    #[serde(default)]
    pub avb: String,
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
        Some(path) => secret::read_key_file(path).context("key file missing or unreadable")?,
        None => secret::read_key_file(Path::new(defs::SPROOT_KEY_FILE))
            .context("no key available (use --superkey-file)")?,
    };
    // ABI normalisation happens exactly once, here, and is logged. Every writer
    // below receives a value that is *already* 47 bytes, so no code path can
    // truncate silently: `key_material` refuses anything that is not 47.
    let key_abi = abi_key(key.as_str())?;

    let source_dir = match source {
        Some(dir) => dir,
        None => std::env::current_exe()
            .context("cannot resolve own executable")?
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("/")),
    };

    info!("[deploy] step 1/10: directories");
    ensure_layout()?;

    info!("[deploy] step 2/10: AP binary");
    let apd_src = find_component(&source_dir, "apd")?;
    install_file(&apd_src, Path::new(defs::DAEMON_PATH), 0o755)?;

    info!("[deploy] step 3/10: busybox");
    let busybox_src = find_component(&source_dir, "busybox")?;
    install_file(
        &busybox_src,
        &concat_path(defs::BINARY_DIR, "busybox"),
        0o755,
    )?;

    info!("[deploy] step 4/10: multicall symlinks");
    ensure_symlink(
        Path::new(defs::DAEMON_PATH),
        &concat_path(defs::BINARY_DIR, "resetprop"),
    )?;
    ensure_symlink(
        Path::new(defs::DAEMON_PATH),
        &concat_path(defs::BINARY_DIR, "magiskpolicy"),
    )?;

    // The key file is the start-chain's single point of failure: `?` aborts
    // deploy (non-zero exit, no "pretend success") when it cannot be written.
    info!("[deploy] step 5/10: key file (47 chars, no NUL)");
    write_key_file(key_abi.as_str())?;

    info!("[deploy] step 6/10: rc audit copy");
    install_rc_template()?;

    info!("[deploy] step 7/10: hidden su deployment");
    let hidden_file = install_su(key_abi.as_str(), &apd_src)?;

    info!("[deploy] step 8/10: allow list");
    init_allow_list()?;
    // Auth channel name (0644, no key material); idempotent on re-deploy so a
    // running daemon keeps its already-bound name.
    crate::daemon::ensure_socket_name()?;

    info!("[deploy] step 9/10: state");
    write_state(&hidden_file, &source_dir)?;

    info!("[deploy] step 10/10: smoke checks");
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
    if let Some(key) = secret::read_key_file(Path::new(defs::SPROOT_KEY_FILE)) {
        if key.len() >= 16 {
            let hidden_dir = format!("/data/{}", &key.as_str()[..16]);
            remove_path(&hidden_dir);
        } else {
            warn!("[undeploy] key file too short, hidden directory left in place");
        }
    } else {
        warn!("[undeploy] key file unavailable, hidden directory left in place");
    }

    // 3. private state. The key file is the only secret on disk: unlink it
    //    first so even a partially failed undeploy cannot leave it behind,
    //    then the rc audit copy and the directory itself.
    remove_path(defs::SPROOT_KEY_FILE);
    remove_path(defs::SPROOT_RC_FILE);
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

/// Derive the **exactly 47-byte** key ABI material from the raw input, once, at
/// the deployment boundary.
///
/// * 47 chars — canonical AP-side format, used as is;
/// * 48 bytes — accepted with an explicit `warn!` (never silently). This is
///   either the manager's 48-position display format (`SecretStore` generates
///   48 chars and the last one is display-only — C1 §2.2 / contract v1.1 E1) or
///   the kernel-inlined `47 + NUL` shape; both resolve to the first 47 bytes.
///   The 48th byte must then be NUL, blank or `[A-Za-z0-9]`, otherwise it is a
///   typo, not a display position;
/// * every other length — hard error. 49+ is a *different* key from another ABI
///   (the Skroot-Next mix-up); truncating it to 47 would produce a token the
///   kernel silently rejects, i.e. exactly the silent escalation failure this
///   function exists to prevent.
///
/// The result is a [`secret::SecretKey`] (mlock'ed, wiped on drop) and the only
/// shape [`write_key_file`] accepts.
fn abi_key(key: &str) -> Result<secret::SecretKey> {
    let trimmed = key.trim();
    ensure!(
        trimmed.is_ascii(),
        "key must be ASCII: 密钥 ABI 为 47 个 ASCII 字符 [A-Za-z0-9]"
    );
    let len = trimmed.len();
    if len == KEY_ABI_LEN + 1 {
        let last = trimmed.as_bytes()[KEY_ABI_LEN];
        ensure!(
            last == 0 || (last as char).is_ascii_alphanumeric()
                || (last as char).is_ascii_whitespace(),
            "key length {len} but byte 48 is not NUL/blank/[A-Za-z0-9]: \
             本项目密钥 ABI 为 47 字符（展示为 48 位含 NUL），请勿带 NUL 或多余字符"
        );
        if last == 0 || (last as char).is_ascii_whitespace() {
            warn!(
                "[deploy] 输入 {len} 字节（47 字符 + NUL/空白）：本项目密钥 ABI 为 47 字符，按 47 字节取前 47"
            );
        } else {
            warn!(
                "[deploy] 输入 {len} 位（管理器 48 位展示格式，第 48 位仅供展示）：按 47 字节 ABI 取前 47"
            );
        }
    } else if len != KEY_ABI_LEN {
        bail!(
            "key length {len} invalid: 本项目密钥 ABI 为 47 字符（展示为 48 位含 NUL），\
             请勿带 NUL 或多余字符（48 位仅记录后取前 47，49 位及以上一律拒绝）"
        );
    }
    let material = &trimmed[..KEY_ABI_LEN];
    ensure!(
        material.chars().all(|c| c.is_ascii_alphanumeric()),
        "key contains non-alphanumeric characters: 密钥 ABI 为 47 个 ASCII 字符 [A-Za-z0-9]"
    );
    let key = secret::SecretKey::new(material);
    ensure!(
        key.len() == KEY_ABI_LEN,
        "internal error: normalised key is {} bytes, expected {KEY_ABI_LEN}",
        key.len()
    );
    Ok(key)
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

/// Build the exact on-disk key material: the 47 key bytes, plain text.
///
/// ABI (NEW-DOC-05 §3.1.3, HANDOFF §6.8): the **AP-side file** is 47 chars
/// with no NUL and no newline; the kernel-side shellcode stores 47 chars + NUL,
/// but that terminator is *not* part of this file. A NUL here would make
/// `CString::new` fail in older readers and is therefore forbidden.
fn key_material(key: &str) -> Result<Vec<u8>> {
    let trimmed = key.trim();
    ensure!(
        trimmed.is_ascii(),
        "key must be ASCII for the 47-char on-disk ABI"
    );
    // Strict, no truncation: a 48/49-byte value reaching this writer means it
    // never went through `abi_key`. Refuse it instead of writing a key that
    // would not match the kernel token (silent escalation failure).
    ensure!(
        trimmed.len() == KEY_ABI_LEN,
        "key length {} != {KEY_ABI_LEN}: 本项目密钥 ABI 为 47 字符（展示为 48 位含 NUL），\
         请勿带 NUL 或多余字符",
        trimmed.len()
    );
    ensure!(
        trimmed.chars().all(|c| c.is_ascii_alphanumeric()),
        "key contains non-alphanumeric characters: 密钥 ABI 为 47 个 ASCII 字符 [A-Za-z0-9]"
    );
    let material = trimmed.as_bytes().to_vec();
    ensure!(
        material.len() == KEY_ABI_LEN && !material.contains(&0),
        "internal error: key material is not 47 NUL-free bytes"
    );
    Ok(material)
}

/// Write the canonical key file (`defs::SPROOT_KEY_FILE`).
///
/// * mode 0600, directory 0700, owner root (deploy already runs as root);
/// * content = [`key_material`] (47 chars, plain text, no NUL/newline);
/// * **atomic**: temp file in the same directory (0600 from creation) → fsync
///   → rename over the target, so a crash can never leave a truncated/empty
///   key behind — the old key stays readable until the rename;
/// * **idempotent**: byte-identical content is left untouched (mode is still
///   re-asserted); different content is overwritten and logged (never echoed);
/// * the in-memory copy is wiped with `write_volatile` right after the write.
fn write_key_file(key: &str) -> Result<()> {
    let mut material = key_material(key)?;
    let path = Path::new(defs::SPROOT_KEY_FILE);
    let dir = Path::new(defs::SPROOT_DIR);
    fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))
        .with_context(|| format!("chmod 0700 {}", dir.display()))?;

    let previous = fs::read(path).ok();
    let existed = previous.is_some();
    let unchanged = previous.as_deref() == Some(material.as_slice());
    // The comparison read the old key into the heap too: wipe that copy now,
    // in every branch (no key bytes may outlive this function).
    if let Some(mut previous) = previous {
        // SAFETY: `previous` is a live, uniquely-owned buffer we no longer use.
        unsafe { secret::wipe_bytes(previous.as_mut_ptr(), previous.len()) };
    }
    if unchanged {
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .with_context(|| format!("chmod 0600 {}", path.display()))?;
        info!("[deploy] key file unchanged, skipping write");
        // SAFETY: `material` is a live, uniquely-owned buffer we no longer use.
        unsafe { secret::wipe_bytes(material.as_mut_ptr(), material.len()) };
        return Ok(());
    }

    let tmp = path.with_extension("tmp");
    {
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .with_context(|| format!("create {}", tmp.display()))?;
        file.write_all(&material)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.sync_all()?;
    }
    // Wipe before the rename: no later error path can keep the secret alive.
    // SAFETY: `material` is a live, uniquely-owned buffer we no longer use.
    unsafe { secret::wipe_bytes(material.as_mut_ptr(), material.len()) };
    fs::rename(&tmp, path).with_context(|| format!("install key file {}", path.display()))?;
    // Make the rename itself durable (crash after rename, before write-back).
    if let Ok(dirfd) = fs::File::open(dir) {
        let _ = dirfd.sync_all();
    }
    if existed {
        info!("[deploy] key file content differed, overwritten atomically");
    } else {
        info!("[deploy] key file created (47 bytes, plain text)");
    }
    Ok(())
}

/// Write the rc audit copy (`defs::SPROOT_RC_FILE`, 0644).
///
/// init only consumes the ramdisk `/init.sproot.rc`; this copy is written so
/// `selftest`/QA can prove which template the manager shipped and that it
/// still names the fixed `--superkey-file` path. Atomic like everything else.
fn install_rc_template() -> Result<()> {
    let bytes = assets::INIT_RC_TEMPLATE.as_bytes();
    let dst = Path::new(defs::SPROOT_RC_FILE);
    let dir = Path::new(defs::SPROOT_DIR);
    fs::create_dir_all(dir).with_context(|| format!("create {}", dir.display()))?;

    if fs::read(dst).map(|current| current == bytes).unwrap_or(false) {
        fs::set_permissions(dst, fs::Permissions::from_mode(0o644))
            .with_context(|| format!("chmod 0644 {}", dst.display()))?;
        info!("[deploy] rc audit copy unchanged, skipping write");
        return Ok(());
    }

    let tmp = dst.with_extension("tmp");
    {
        let mut file =
            fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        file.write_all(bytes)?;
        file.set_permissions(fs::Permissions::from_mode(0o644))?;
        file.sync_all()?;
    }
    fs::rename(&tmp, dst)
        .with_context(|| format!("install rc audit copy {}", dst.display()))?;
    info!("[deploy] rc audit copy written ({} bytes)", bytes.len());
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

/// Initialise the authorisation storage.
///
/// * the **legacy csv** (`defs::ALLOW_LIST_FILE`) is created header-only when
///   missing: it is no longer the decision source (NEW-DOC-05 §13), but
///   `deploy`'s smoke check and old tooling still expect the file, and it is
///   the one-way migration source for devices upgrading from the whitelist;
/// * the **tri-state JSON store** + audit log are created by
///   [`crate::auth_list::prepare_storage`] (0600, atomic, flock protected).
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
        avb: "vbmeta_untouched".to_string(),
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
        PathBuf::from(defs::SPROOT_RC_FILE),
        PathBuf::from(defs::ALLOW_LIST_FILE),
        // Tri-state authorisation store + audit log (NEW-DOC-05 §13).
        PathBuf::from(defs::AUTH_LIST_FILE),
        PathBuf::from(defs::AUTH_LOG_FILE),
        // Derived APatch/Zygisk signal (NEW-DOC-05 §14).
        PathBuf::from(defs::APATCH_PACKAGE_CONFIG),
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
    for path in [defs::AUTH_LIST_FILE, defs::AUTH_LOG_FILE] {
        let mode = fs::metadata(path)?.permissions().mode() & 0o777;
        ensure!(mode == 0o600, "{path} mode {mode:o} != 600");
    }
    // The Zygisk side may be a non-root helper: the derived csv must be
    // readable (0644) but never writable, and its first line must be the exact
    // positional header external parsers rely on.
    let package_config = defs::APATCH_PACKAGE_CONFIG;
    let mode = fs::metadata(package_config)?.permissions().mode() & 0o777;
    ensure!(
        mode == 0o644,
        "{package_config} mode {mode:o} != 644 (Zygisk helper must be able to read it)"
    );
    let header = fs::read_to_string(package_config)?
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();
    ensure!(
        header == defs::APATCH_PACKAGE_CONFIG_HEADER,
        "{package_config} header '{header}' != '{}' (positional columns break external parsers)",
        defs::APATCH_PACKAGE_CONFIG_HEADER
    );

    // Start-chain contract (NEW-DOC-05 §3.1.3): exact ABI + fixed path.
    let mut key_bytes = fs::read(defs::SPROOT_KEY_FILE)?;
    let len = key_bytes.len();
    let nul_free = !key_bytes.contains(&0);
    // SAFETY: `key_bytes` is a live, uniquely-owned buffer we no longer use.
    unsafe { secret::wipe_bytes(key_bytes.as_mut_ptr(), key_bytes.len()) };
    ensure!(
        len == KEY_ABI_LEN,
        "key file length {len} != {KEY_ABI_LEN} (47 chars, AP-side format)"
    );
    ensure!(nul_free, "key file contains a NUL (AP-side format is plain text)");

    let rc = fs::read_to_string(defs::SPROOT_RC_FILE)?;
    ensure!(
        rc.contains(defs::SPROOT_KEY_FILE),
        "rc audit copy does not reference {}",
        defs::SPROOT_KEY_FILE
    );
    ensure!(
        rc.contains(defs::DAEMON_PATH),
        "rc audit copy does not reference {}",
        defs::DAEMON_PATH
    );
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

#[cfg(test)]
mod tests {
    use super::*;

    fn alnum_key(len: usize) -> String {
        "a".repeat(len)
    }

    #[test]
    fn abi_key_accepts_exactly_47() {
        let key = alnum_key(KEY_ABI_LEN);
        assert_eq!(abi_key(&key).unwrap().as_str(), key);
    }

    #[test]
    fn abi_key_maps_48_and_47_plus_nul_to_47() {
        // 48 = manager display format: first 47 bytes, always logged (warn!).
        let display = alnum_key(KEY_ABI_LEN + 1);
        assert_eq!(abi_key(&display).unwrap().len(), KEY_ABI_LEN);
        // 47 + NUL = kernel-inlined shape: same 47 bytes.
        let mut kernelled = alnum_key(KEY_ABI_LEN);
        kernelled.push('\0');
        assert_eq!(abi_key(&kernelled).unwrap().len(), KEY_ABI_LEN);
        // A 48th byte that is neither NUL/blank nor [A-Za-z0-9] is a typo.
        let mut typo = alnum_key(KEY_ABI_LEN);
        typo.push('-');
        assert!(abi_key(&typo).is_err());
    }

    #[test]
    fn abi_key_rejects_49_short_and_symbols() {
        // Negative samples for the P0: 49 chars is a *different* key from
        // another ABI, so it must fail loudly instead of being truncated into
        // a token the kernel silently rejects.
        assert!(abi_key(&alnum_key(KEY_ABI_LEN + 2)).is_err());
        assert!(abi_key(&alnum_key(KEY_ABI_LEN - 1)).is_err());
        let mut symbolic = alnum_key(KEY_ABI_LEN - 1);
        symbolic.push('-');
        assert!(abi_key(&symbolic).is_err());
        // The writer itself never truncates either: 48/49 are refused outright.
        assert!(key_material(&alnum_key(KEY_ABI_LEN + 1)).is_err());
        assert!(key_material(&alnum_key(KEY_ABI_LEN + 2)).is_err());
        assert_eq!(
            key_material(&alnum_key(KEY_ABI_LEN)).unwrap(),
            alnum_key(KEY_ABI_LEN).into_bytes()
        );
    }

    #[test]
    fn rc_template_matches_defs_and_writer_path() {
        // Start-chain contract (NEW-DOC-05 §3.1.3): the Lead-owned rc asset,
        // `defs::SPROOT_KEY_FILE` and `write_key_file` must agree byte for byte,
        // and `--superkey-file` must precede the `daemon` subcommand (clap
        // top-level arg: reversed order = rc=2 and the daemon never starts).
        let expected = format!(
            "service sproot-apd {} --superkey-file {} daemon",
            defs::DAEMON_PATH,
            defs::SPROOT_KEY_FILE
        );
        let line = assets::INIT_RC_TEMPLATE
            .lines()
            .find(|l| l.trim_start().starts_with("service sproot-apd "))
            .expect("rc template lost its service line");
        assert_eq!(line.trim(), expected);
        // Pinned ABI paths: changing either breaks the ramdisk template and the
        // already-deployed artifacts, so it must be a deliberate contract bump.
        assert_eq!(defs::SPROOT_KEY_FILE, "/data/adb/sproot/key");
        assert_eq!(defs::DAEMON_PATH, "/data/adb/apd");
        // The writer targets that same constant (no second literal path).
        assert!(defs::SPROOT_KEY_FILE.starts_with(defs::SPROOT_DIR));
    }
}
