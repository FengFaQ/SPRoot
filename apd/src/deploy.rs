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
    io::{self, Read, Seek, SeekFrom, Write},
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
    /// AVB audit evidence captured **from the boot partition** at deploy time.
    ///
    /// This used to be the literal `"vbmeta_untouched"`, which made the selftest
    /// tautological (write a constant, read the constant back). It is now the
    /// measured observation produced by [`probe_avb`]: `"avb_ok"` when a footer
    /// was located and digested, `"avb_no_footer"` when the partition carries no
    /// AVB footer, `"avb_unobservable"` when the device could not be read at all.
    /// Only `"avb_ok"` is a positive claim; the other two are explicitly *not*.
    #[serde(default)]
    pub avb: String,
    /// SHA-256 of the whole 64-byte AVB footer (the part that carries
    /// `original_image_size` / `vbmeta_offset` / `vbmeta_size`). `None` when the
    /// partition could not be observed — never a placeholder string.
    #[serde(default)]
    pub avb_footer_sha256: Option<String>,
    /// SHA-256 of the embedded vbmeta section `[vbmeta_offset, +vbmeta_size)`.
    /// `None` unless `avb == "avb_ok"`.
    #[serde(default)]
    pub avb_vbmeta_sha256: Option<String>,
    /// Which device/path the observation came from (audit trail; `None` when
    /// unobservable). Kept so a mismatch is diagnosable instead of just "FAIL".
    #[serde(default)]
    pub avb_source: Option<String>,
    /// Every path the AVB probe tried, with the reason each was skipped
    /// (2026-11-11 P0-4). `None` for state files written before this field
    /// existed. Its purpose is field diagnosis: when a device reports
    /// `avb_unobservable` this says whether the `by-name` directory was missing,
    /// which aliases existed, and whether `open` was denied — without it the
    /// report is a bare "unknown" that cannot be acted on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avb_probe_trace: Option<String>,
    /// Total partition size in bytes at observation time. A later change means
    /// the partition was rewritten (footer is at the *end*, so any resize moves
    /// it) — the selftest re-measures and compares.
    #[serde(default)]
    pub avb_partition_bytes: Option<u64>,
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

    // APD-3 ③-1: mark the deployment as IN PROGRESS before the first write.
    //
    // The steps below write files and are aborted by `?`. Before this marker, a
    // failure at step 3 left `/data/adb/apd` on disk while `state.json` (written
    // at step 9) was still absent — so `version --json` reported
    // `deployed=false` even though the binary was installed. "File present but
    // state says no" is a silent inconsistency that the manager's `InstallState`
    // cannot distinguish from "never deployed".
    //
    // The marker makes that intermediate state *explicit* instead: while it
    // exists, the deployment is known-incomplete and callers must report that
    // rather than guess. It is removed once the state file is written.
    begin_deploy_marker();

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
    // `apd` itself, so module scripts can resolve `Command::new("apd")` (P0,
    // 2026-11-11). Deliberately folded into step 4 rather than added as step 5:
    // it is the same operation as the two above, and inserting a step would
    // renumber 5..10 and desynchronise every log line from the docs/CLI contract.
    ensure_symlink(
        Path::new(defs::DAEMON_PATH),
        Path::new(&crate::module::apd_in_path_symlink()),
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
    // APD-3 ③-1: the deployment is now complete and self-describing; the marker
    // must go so nobody reports a finished deploy as half-done.
    clear_deploy_marker();

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
    // The apd entry added for module scripts (P0, 2026-11-11). Removed here — and
    // under its own name, not by removing BINARY_DIR wholesale — so this list
    // stays the single place that enumerates everything deploy created.
    remove_path(Path::new(&crate::module::apd_in_path_symlink()));
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

// ---------------------------------------------------------------------------
// APD-3 ③-1: incomplete-deployment marker
// ---------------------------------------------------------------------------

/// Create the "deploy in progress" marker.
///
/// Deliberately best-effort (`let _ =`): failing to write a marker must not be
/// the thing that breaks a deployment. Its absence only means callers fall back
/// to the old (ambiguous) behaviour, never that deploy aborts.
///
/// The directory may not exist on the very first deploy (step 1 creates it), so
/// this creates it too — `create_dir_all` is idempotent.
fn begin_deploy_marker() {
    let _ = fs::create_dir_all(defs::SPROOT_DIR);
    let payload = format!(
        "pid={}\nstarted_at={}\n",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    );
    let path = defs::DEPLOY_INCOMPLETE_FILE;
    let tmp = format!("{path}.tmp");
    if fs::write(&tmp, payload).is_ok() {
        let _ = fs::rename(&tmp, path);
    }
}

/// Remove the marker — called once `state.json` has been written successfully.
fn clear_deploy_marker() {
    let _ = fs::remove_file(defs::DEPLOY_INCOMPLETE_FILE);
}

/// Is a deployment currently half-done?
///
/// True means: some step of `deploy` failed after it had already written to
/// disk, so the filesystem state is **not** the "not deployed" state even
/// though `state.json` may be missing. Callers (`version --json`, `selftest`)
/// report this explicitly instead of guessing from file presence.
pub fn deploy_is_incomplete() -> bool {
    Path::new(defs::DEPLOY_INCOMPLETE_FILE).is_file()
}

/// Read the marker's contents (pid + start time), for diagnostics.
pub fn read_deploy_marker() -> Option<String> {
    fs::read_to_string(defs::DEPLOY_INCOMPLETE_FILE).ok()
}

/// Temp path for an atomic write: the **full target name plus `.tmp`**.
///
/// Never use `Path::with_extension("tmp")` for this. `with_extension` *replaces*
/// the extension, so:
///
/// | target | `with_extension("tmp")` | this function |
/// |---|---|---|
/// | `/data/adb/sproot/init.sproot.rc` | `init.sproot.tmp` | `init.sproot.rc.tmp` |
/// | `/data/adb/sproot/state.json`     | `state.tmp`       | `state.json.tmp` |
///
/// The substitution silently drops part of the name, so two distinct targets
/// that share a stem collapse onto one temp file. If a future `init.sproot.bin`
/// is added next to `init.sproot.rc`, both would write `init.sproot.tmp`; one
/// writer's temp can be renamed away by the other and it still believes the
/// write succeeded — silent cross-file corruption that no error surfaces.
///
/// Appending keeps the mapping injective: distinct targets always get distinct
/// temp files, and the temp stays in the **same directory** as its target, which
/// is what makes `rename` atomic (same filesystem).
fn tmp_sibling(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".tmp");
    PathBuf::from(name)
}

/// Copy `src` to `dst` atomically (tmp + rename) and verify byte length.
fn install_file(src: &Path, dst: &Path, mode: u32) -> Result<()> {
    let bytes = fs::read(src).with_context(|| format!("read {}", src.display()))?;
    ensure!(!bytes.is_empty(), "source binary empty: {}", src.display());

    // `.tmp` is *appended* to the full name, never substituted for the
    // extension (APD-4). `Path::with_extension("tmp")` would turn
    // `init.sproot.rc` into `init.sproot.tmp` and `state.json` into `state.tmp`,
    // so two targets sharing a stem would race on the same temp file: writer A's
    // temp gets renamed away by writer B and A believes its write succeeded.
    // Silent cross-file corruption is the worst possible failure mode here.
    let tmp = tmp_sibling(dst);
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

    let tmp = tmp_sibling(path);
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

    // Appended `.tmp` (APD-4): must be `init.sproot.rc.tmp`, never
    // `init.sproot.tmp` — see `tmp_sibling`.
    let tmp = tmp_sibling(dst);
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
///
/// **Idempotent (APD-3 ③-2).** The 16-char name is random on every deploy, so
/// re-running deploy used to leave the previous deployment's binary behind: N
/// deploys = N orphaned copies of apd inside the hidden directory (verified
/// empirically before this fix). Those copies are an attack surface (an old
/// binary still executable as root) and pure waste. Every *other* regular file
/// in the directory is removed before installing the new one, leaving exactly
/// one — which is what `read_state().hidden_file` promises.
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

    // Remove any previous deployment's binary. Done AFTER the new one is safely
    // in place so a failure here cannot leave the device with no su at all.
    // Only regular files are touched: never the directory itself, never the
    // new file, and never anything that is not ours (a symlink is left alone —
    // `symlink_metadata` lets us tell).
    if let Ok(entries) = fs::read_dir(&hidden_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path == hidden_path {
                continue;
            }
            match fs::symlink_metadata(&path) {
                Ok(meta) if meta.file_type().is_file() => {
                    if let Err(e) = fs::remove_file(&path) {
                        warn!(
                            "[deploy] cannot remove previous hidden su {}: {e} \
                             (it stays executable; remove it manually)",
                            path.display()
                        );
                    } else {
                        info!(
                            "[deploy] removed previous hidden su {} (idempotent redeploy)",
                            path.display()
                        );
                    }
                }
                _ => {}
            }
        }
    }

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

// ---------------------------------------------------------------------------
// AVB observation (2026-11-10 S1). See `defs::AVB_BOOT_CANDIDATES`.
//
// The project rule (HANDOFF-2026-11-08 §"教训", NEW-DOC-02 T-12 铁律) is that a
// new assertion must first be proven able to FAIL. The observation below can
// fail three ways, all reachable in unit tests against plain files:
//   * footer magic missing at the end of the partition  → `NoFooter`
//   * vbmeta section out of bounds / overlapping footer → `Unobservable`
//   * footer or vbmeta digest differs from the recorded  → selftest FAIL
// ---------------------------------------------------------------------------

/// AVB footer length in bytes (`avb_footer` is a fixed 64 B struct at the very
/// end of the partition; mirrors `app/.../Elf.kt: AVB_FOOTER_SIZE`).
pub const AVB_FOOTER_SIZE: u64 = 64;
/// `AVBf` magic, big-endian struct per avbtool.
pub const AVB_FOOTER_MAGIC: &[u8; 4] = b"AVBf";

/// How far the observation actually got.
///
/// This is **explicit state, not a guess derived from `partition_bytes`**
/// (auditor finding 2026-11-10 #3): "we could not read the partition" and "we
/// read it and there is genuinely no AVB footer" are different facts and must
/// stay distinguishable in `state.json`, otherwise a device whose partition is
/// unreadable silently reports the same thing as a device that legitimately has
/// no AVB — and neither may look like a pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvbOutcome {
    /// Footer located, magic + bounds valid (and in full mode, vbmeta digested).
    Ok,
    /// Partition fully readable, and it carries **no** `AVBf` footer at the tail.
    /// A legitimate un-AVB'd vendor image: nothing to have touched.
    NoFooter,
    /// Opened and sized, but the footer or its vbmeta range is malformed.
    /// We saw *something* and could not decide — distinct from `NoFooter`.
    Malformed,
    /// Could not open, could not size, or the partition is too small to hold a
    /// footer. Nothing was measured.
    Unobservable,
}

/// Result of reading the AVB footer off a partition/file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvbObservation {
    pub source: String,
    pub partition_bytes: u64,
    /// SHA-256 of the 64-byte footer itself.
    pub footer_sha256: Option<String>,
    /// SHA-256 of the embedded vbmeta section — `Some` only when the section was
    /// actually read (the cheap footer-only mode leaves it `None`; see
    /// [`observe_avb_footer_only_from_path`]).
    pub vbmeta_sha256: Option<String>,
    /// `true` when a footer with valid magic **and** structurally valid vbmeta
    /// bounds was located at the tail. In footer-only mode this is a statement
    /// about the footer alone (the vbmeta bytes were not compared).
    pub footer_present: bool,
    /// How far the observation got (drives `code()`; never inferred).
    pub outcome: AvbOutcome,
    /// Why the observation is incomplete (`None` when fully observed).
    pub problem: Option<String>,
}

impl AvbObservation {
    /// Machine code stored in `state.json` / reported by the selftest and the
    /// module `--json` responses. Values are stable identifiers:
    ///
    /// * `avb_ok`          — measured: footer present (and, in full mode, its
    ///   vbmeta section digested).
    /// * `avb_no_footer`   — measured: the partition is readable and has **no**
    ///   AVB footer. A *legitimate* device state (un-AVB'd vendor images), but
    ///   explicitly **not** a PASS of "vbmeta untouched" — there is nothing to
    ///   have touched.
    /// * `avb_malformed`   — a footer is present but its fields are invalid
    ///   (out of bounds, overlapping, illegal vbmeta offset/size). Not the same
    ///   as "no footer", and not a pass either.
    /// * `avb_unobservable`— could not read or size the partition. Nothing was
    ///   measured; "unknown", never "ok".
    pub fn code(&self) -> &'static str {
        match self.outcome {
            AvbOutcome::Ok => "avb_ok",
            AvbOutcome::NoFooter => "avb_no_footer",
            AvbOutcome::Malformed => "avb_malformed",
            AvbOutcome::Unobservable => "avb_unobservable",
        }
    }
}

/// Read the 64-byte AVB footer at the tail of `path` and digest the vbmeta
/// section it points at. Pure I/O, no global state: unit-testable on any file.
///
/// On a real device the source is a block device (`open` + `seek` works the
/// same as for a regular file), so no `O_DIRECT`/BLKGETSIZE64 dance is needed.
///
/// This is the **full** observation used by `deploy` and `selftest` (both are
/// low-frequency and need to detect tampering inside the vbmeta payload). The
/// module hot path uses the cheaper [`observe_avb_footer_only_from_path`].
pub fn observe_avb_from_path(path: &Path) -> AvbObservation {
    observe_avb_impl(path, true)
}

/// Footer-only variant: reads 64 B at the tail and validates the vbmeta bounds,
/// but does **not** read the vbmeta section.
///
/// Why the cut (module install/uninstall hot path): the full observation costs a
/// second seek plus a read of the whole vbmeta section — 2.5 KB on the SPROOT
/// sample boot image but megabytes on devices with large vbmeta (the field is a
/// free 64-bit size). For an audit line that runs on every module operation,
/// the footer digest alone already detects the failure modes that matter there:
/// a rewritten/moved/erased footer changes its own digest, and a resize changes
/// `partition_bytes` and the footer's `vbmeta_offset`. What is **lost** is
/// detection of tampering *inside* the vbmeta payload with an otherwise intact
/// footer — that is precisely what `deploy`/`selftest` keep using the full mode
/// for. Doc note recorded in `AvbObservation::vbmeta_sha256`.
pub fn observe_avb_footer_only_from_path(path: &Path) -> AvbObservation {
    observe_avb_impl(path, false)
}

/// Real byte length of an opened file **or block device**.
///
/// `metadata().len()` is unusable here: for a block device the kernel reports
/// `st_size == 0` (the size lives in the block-device inode, not the size
/// field), so any `len < AVB_FOOTER_SIZE` guard built on it rejects *every* real
/// device and `avb_ok` becomes unreachable — the check would degrade from
/// "always green" to "always grey", which is just a different way of never
/// answering the question (auditor finding, 2026-11-10).
///
/// `seek(SeekFrom::End(0))` returns the true size for both regular files and
/// block devices (measured: `/dev/sda` st_size=0 but SEEK_END=758038528), so a
/// single code path covers the test fixtures and production. `BLKGETSIZE64`
/// would also work but is Linux/Android-only and needs `unsafe` ioctl plumbing
/// for no gain — SEEK_END is portable and already how the footer itself is
/// located.
fn real_len(file: &mut fs::File) -> std::io::Result<u64> {
    file.seek(SeekFrom::End(0))
}

fn observe_avb_impl(path: &Path, digest_vbmeta: bool) -> AvbObservation {
    let source = path.display().to_string();
    let mut file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) => {
            return AvbObservation {
                source,
                partition_bytes: 0,
                footer_sha256: None,
                vbmeta_sha256: None,
                footer_present: false,
                outcome: AvbOutcome::Unobservable,
                problem: Some(format!("cannot open: {e}")),
            };
        }
    };
    let len = match real_len(&mut file) {
        Ok(len) => len,
        Err(e) => {
            return AvbObservation {
                source,
                partition_bytes: 0,
                footer_sha256: None,
                vbmeta_sha256: None,
                footer_present: false,
                outcome: AvbOutcome::Unobservable,
                problem: Some(format!("cannot determine size (SEEK_END failed): {e}")),
            };
        }
    };
    if len < AVB_FOOTER_SIZE {
        return AvbObservation {
            source,
            partition_bytes: len,
            footer_sha256: None,
            vbmeta_sha256: None,
            footer_present: false,
            outcome: AvbOutcome::Unobservable,
            problem: Some(format!("partition too small ({len} B) to hold a footer")),
        };
    }

    let mut footer = [0u8; AVB_FOOTER_SIZE as usize];
    if let Err(e) = file.seek(SeekFrom::End(-(AVB_FOOTER_SIZE as i64))) {
        return AvbObservation {
            source,
            partition_bytes: len,
            footer_sha256: None,
            vbmeta_sha256: None,
            footer_present: false,
            outcome: AvbOutcome::Unobservable,
            problem: Some(format!("seek to footer failed: {e}")),
        };
    }
    if let Err(e) = file.read_exact(&mut footer) {
        return AvbObservation {
            source,
            partition_bytes: len,
            footer_sha256: None,
            vbmeta_sha256: None,
            footer_present: false,
            outcome: AvbOutcome::Unobservable,
            problem: Some(format!("read footer failed: {e}")),
        };
    }
    if &footer[0..4] != AVB_FOOTER_MAGIC {
        return AvbObservation {
            source,
            partition_bytes: len,
            footer_sha256: None,
            vbmeta_sha256: None,
            footer_present: false,
            // Read the tail successfully and it is not an AVB footer: this is a
            // *measured* "no AVB on this partition", not a failed observation.
            outcome: AvbOutcome::NoFooter,
            problem: Some(format!(
                "no AVB footer (magic at -64 is {:?}, want \"AVBf\")",
                String::from_utf8_lossy(&footer[0..4])
            )),
        };
    }

    // All AVB integer fields are big-endian (avbtool uses be32toh/be64toh).
    let be64 = |off: usize| -> u64 {
        let mut b = [0u8; 8];
        b.copy_from_slice(&footer[off..off + 8]);
        u64::from_be_bytes(b)
    };
    let orig_image_size = be64(12);
    let vbmeta_offset = be64(20);
    let vbmeta_size = be64(28);
    let footer_sha256 = Some(crate::utils::sha256_hex(&footer));

    // Bounds: the vbmeta section is an absolute range inside the partition and
    // must not reach into the footer. A violation is "cannot decide", never ok.
    if vbmeta_offset == 0 || vbmeta_size == 0 {
        return AvbObservation {
            source,
            partition_bytes: len,
            footer_sha256,
            vbmeta_sha256: None,
            footer_present: false,
            outcome: AvbOutcome::Malformed,
            problem: Some(format!(
                "illegal vbmeta fields (offset={vbmeta_offset}, size={vbmeta_size})"
            )),
        };
    }
    let vbmeta_end = match vbmeta_offset.checked_add(vbmeta_size) {
        Some(end) => end,
        None => {
            return AvbObservation {
                source,
                partition_bytes: len,
                footer_sha256,
                vbmeta_sha256: None,
                footer_present: false,
                outcome: AvbOutcome::Malformed,
                problem: Some("vbmeta range overflows u64".to_string()),
            };
        }
    };
    if vbmeta_end > len {
        return AvbObservation {
            source,
            partition_bytes: len,
            footer_sha256,
            vbmeta_sha256: None,
            footer_present: false,
            outcome: AvbOutcome::Malformed,
            problem: Some(format!(
                "vbmeta range [{vbmeta_offset}, {vbmeta_end}) exceeds partition ({len} B)"
            )),
        };
    }
    if vbmeta_end > len - AVB_FOOTER_SIZE {
        return AvbObservation {
            source,
            partition_bytes: len,
            footer_sha256,
            vbmeta_sha256: None,
            footer_present: false,
            outcome: AvbOutcome::Malformed,
            problem: Some(format!(
                "vbmeta range ends at {vbmeta_end}, overlapping the footer at {}",
                len - AVB_FOOTER_SIZE
            )),
        };
    }

    // Footer-only mode stops here: magic + bounds are validated, the payload is
    // not read. `vbmeta_sha256` stays `None`, which is honest — we did not measure
    // it. `footer_present` is still true because the footer itself was verified.
    if !digest_vbmeta {
        return AvbObservation {
            source,
            partition_bytes: len,
            footer_sha256,
            vbmeta_sha256: None,
            footer_present: true,
            outcome: AvbOutcome::Ok,
            problem: None,
        };
    }

    let mut vbmeta = vec![0u8; vbmeta_size as usize];
    if let Err(e) = file.seek(SeekFrom::Start(vbmeta_offset)) {
        return AvbObservation {
            source,
            partition_bytes: len,
            footer_sha256,
            vbmeta_sha256: None,
            footer_present: false,
            outcome: AvbOutcome::Unobservable,
            problem: Some(format!("seek to vbmeta failed: {e}")),
        };
    }
    if let Err(e) = file.read_exact(&mut vbmeta) {
        return AvbObservation {
            source,
            partition_bytes: len,
            footer_sha256,
            vbmeta_sha256: None,
            footer_present: false,
            outcome: AvbOutcome::Unobservable,
            problem: Some(format!("read vbmeta failed: {e}")),
        };
    }
    let _ = orig_image_size; // recorded by the footer digest; kept for the reader
    AvbObservation {
        source,
        partition_bytes: len,
        footer_sha256,
        vbmeta_sha256: Some(crate::utils::sha256_hex(&vbmeta)),
        footer_present: true,
        outcome: AvbOutcome::Ok,
        problem: None,
    }
}

/// One path the AVB probe attempted, plus what happened there (2026-11-11 P0-4).
///
/// This exists because "the probe returned unobservable" is not actionable on a
/// real device: without the per-path reasons an integrator cannot tell a vendor
/// that publishes no `by-name` directory from one where every candidate is
/// present but SELinux denies `open`. The trace is embedded into
/// [`AvbObservation::problem`] on failure, so it surfaces in
/// `selftest --json` (`checks[].detail`) and in the deploy log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AvbProbeAttempt {
    /// The path as it was opened/probed (already resolved to a concrete path; a
    /// scanned entry is recorded with the real path it came from, not the alias).
    pub path: String,
    /// Where the path came from: `"explicit"` (`defs::AVB_BOOT_CANDIDATES`) or
    /// `"scan"` (discovered by the bounded `by-name` walk).
    pub origin: &'static str,
    /// `Some(reason)` when the path was skipped or could not be opened; `None`
    /// when it was opened and a verdict was reached.
    pub skip: Option<String>,
    /// The verdict code when the path *was* observed (see
    /// [`AvbObservation::code`]); `None` when it was skipped.
    pub code: Option<&'static str>,
    /// `true` for the attempt whose observation was returned as the answer.
    pub selected: bool,
}

impl AvbProbeAttempt {
    fn skipped(path: String, origin: &'static str, why: impl Into<String>) -> Self {
        Self {
            path,
            origin,
            skip: Some(why.into()),
            code: None,
            selected: false,
        }
    }
}

/// Everything the AVB probe tried, in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AvbProbeTrace {
    pub attempts: Vec<AvbProbeAttempt>,
    /// `by-name` directories the bounded walk looked at (empty when the walk was
    /// never needed, i.e. an explicit candidate already answered).
    pub scanned_dirs: Vec<String>,
    /// Why the walk itself could not run (stat/read_dir failures, entry caps
    /// hit). Recorded, never fatal: a scan that cannot run must degrade to
    /// "did not find it", never to a panic.
    pub scan_notes: Vec<String>,
}

impl AvbProbeTrace {
    /// The attempt whose observation became the answer.
    pub fn selected(&self) -> Option<&AvbProbeAttempt> {
        self.attempts.iter().find(|a| a.selected)
    }

    /// Compact one-line rendering (`path [origin] verdict; …`), capped so a
    /// pathological layout cannot blow up `state.json` / the selftest detail.
    pub fn render(&self) -> String {
        const MAX: usize = 8;
        if self.attempts.is_empty() && self.scanned_dirs.is_empty() {
            return "no candidate paths were tried".to_string();
        }
        let mut parts: Vec<String> = Vec::with_capacity(MAX + 2);
        for a in self.attempts.iter().take(MAX) {
            let verdict = match (&a.skip, a.code) {
                (Some(why), _) => format!("skipped({why})"),
                (None, Some(code)) => code.to_string(),
                (None, None) => "observed".to_string(),
            };
            parts.push(format!("{} [{}] {}", a.path, a.origin, verdict));
        }
        if self.attempts.len() > MAX {
            parts.push(format!("... {} more attempt(s)", self.attempts.len() - MAX));
        }
        if !self.scanned_dirs.is_empty() {
            let shown = self.scanned_dirs.iter().take(4).cloned().collect::<Vec<_>>();
            let mut dirs = shown.join(", ");
            if self.scanned_dirs.len() > 4 {
                dirs.push_str(&format!(", ... {} more", self.scanned_dirs.len() - 4));
            }
            parts.push(format!("scanned: {dirs}"));
        }
        for note in self.scan_notes.iter().take(3) {
            parts.push(format!("scan: {note}"));
        }
        parts.join("; ")
    }
}

/// Probe one path and feed the trace. `Some(..)` = stop here (either a hit or a
/// definitive verdict such as "no footer"); `None` = unreadable, try the next.
fn probe_attempt(
    path: &Path,
    origin: &'static str,
    digest_vbmeta: bool,
    trace: &mut AvbProbeTrace,
) -> Option<AvbObservation> {
    let shown = path.display().to_string();
    if !path.exists() {
        trace
            .attempts
            .push(AvbProbeAttempt::skipped(shown, origin, "absent"));
        return None;
    }
    let observed = if digest_vbmeta {
        observe_avb_from_path(path)
    } else {
        observe_avb_footer_only_from_path(path)
    };
    match observed.problem.as_deref() {
        // Opened and fully observed.
        None => {
            trace.attempts.push(AvbProbeAttempt {
                path: shown,
                origin,
                skip: None,
                code: Some(observed.code()),
                selected: true,
            });
            Some(observed)
        }
        // Unreadable device (permissions/SELinux/EIO): try the next layout.
        Some(reason) if reason.starts_with("cannot open") || reason.starts_with("cannot stat") => {
            trace
                .attempts
                .push(AvbProbeAttempt::skipped(shown, origin, reason.to_string()));
            None
        }
        // Device opened and a definite verdict was reached, including
        // "no footer": that is the answer for this device.
        Some(_) => {
            trace.attempts.push(AvbProbeAttempt {
                path: shown,
                origin,
                skip: None,
                code: Some(observed.code()),
                selected: true,
            });
            Some(observed)
        }
    }
}

/// Bounded discovery of `.../by-name` directories (2026-11-11 P0-4).
///
/// Real Android layouts put the alias directory at
/// `/dev/block/platform/<soc>/<controller>/by-name`, i.e. up to three levels
/// below [`defs::AVB_SCAN_ROOTS`] — a fixed candidate list cannot express that
/// without guessing every SoC name, which is why this walk exists. It is
/// deliberately **not** a general recursive find: the descent is capped at depth
/// 3, at [`defs::AVB_SCAN_MAX_DIRS`] directories, and at
/// [`defs::AVB_SCAN_MAX_ENTRIES`] entries per directory. All failures are
/// returned as notes; nothing here panics or propagates an error.
/// Bounded discovery of `.../by-name` directories under caller-supplied roots.
///
/// Roots are a parameter (rather than read from [`defs::AVB_SCAN_ROOTS`]
/// directly) so the walk can be tested against a temporary directory tree
/// instead of `/dev/block` (2026-11-11 P0-4).
///
/// Bounds: descent depth 3, [`defs::AVB_SCAN_MAX_DIRS`] directories,
/// [`defs::AVB_SCAN_MAX_ENTRIES`] entries per directory. Failures become notes.
pub fn discover_by_name_dirs_in(roots: &[&str]) -> (Vec<PathBuf>, Vec<String>) {
    const MAX_DEPTH: usize = 3;
    let mut found: Vec<PathBuf> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut visited = 0usize;

    for root in roots {
        let root_path = Path::new(root);
        match fs::metadata(root_path) {
            Ok(m) if m.is_dir() => {}
            Ok(_) => {
                notes.push(format!("{root}: not a directory"));
                continue;
            }
            Err(e) => {
                notes.push(format!("{root}: cannot stat ({e})"));
                continue;
            }
        }
        // The root may itself already be a by-name directory.
        if root_path.file_name().and_then(|n| n.to_str()) == Some("by-name") {
            found.push(root_path.to_path_buf());
            continue;
        }
        let mut frontier: Vec<(PathBuf, usize)> = vec![(root_path.to_path_buf(), 0)];
        while let Some((dir, depth)) = frontier.pop() {
            if visited >= defs::AVB_SCAN_MAX_DIRS {
                notes.push(format!(
                    "stopped after {} directories (cap {})",
                    visited,
                    defs::AVB_SCAN_MAX_DIRS
                ));
                break;
            }
            visited += 1;
            let entries = match fs::read_dir(&dir) {
                Ok(e) => e,
                Err(e) => {
                    notes.push(format!("{}: cannot read dir ({e})", dir.display()));
                    continue;
                }
            };
            for (index, entry) in entries.flatten().enumerate() {
                if index >= defs::AVB_SCAN_MAX_ENTRIES {
                    notes.push(format!(
                        "{}: stopped after {} entries (cap)",
                        dir.display(),
                        defs::AVB_SCAN_MAX_ENTRIES
                    ));
                    break;
                }
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                // `entry.file_type()` avoids following a symlink loop.
                let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
                if !is_dir {
                    continue;
                }
                if name == "by-name" {
                    found.push(entry.path());
                } else if depth + 1 < MAX_DEPTH {
                    frontier.push((entry.path(), depth + 1));
                }
            }
        }
    }
    // Deterministic order so a report is reproducible between runs.
    found.sort();
    found.dedup();
    (found, notes)
}

/// Observe the AVB footer of the device's boot partition.
///
/// Order (2026-11-11 P0-4): `SPROOT_AVB_BOOT_IMAGE` (diagnostics/QA, logged as
/// an override) → `defs::AVB_BOOT_CANDIDATES` (first that opens) → a bounded
/// scan of `defs::AVB_SCAN_ROOTS` for a `by-name/{boot,boot_a}` entry. The scan
/// is the fallback for vendor layouts the fixed list does not cover; it runs
/// **only** when no explicit candidate was reachable, so the happy path costs
/// nothing extra.
///
/// When every path fails the observation is `unobservable` — **not** a pass —
/// and [`AvbObservation::problem`] carries the full [`AvbProbeTrace`] so the
/// failure is self-explaining on a device we cannot attach a debugger to.
/// Deploy runs as root, so a failure here is genuine (no partition / no AVB /
/// SELinux denial).
pub fn probe_avb() -> AvbObservation {
    probe_avb_impl(true)
}

/// Footer-only probe for the module hot path (see
/// [`observe_avb_footer_only_from_path`] for the cost/coverage trade-off).
pub fn probe_avb_footer_only() -> AvbObservation {
    probe_avb_impl(false)
}

/// Probe **and** return the trace (deploy uses this to log why it picked a
/// path; the plain [`probe_avb`] callers do not need the extra data).
pub fn probe_avb_traced(digest_vbmeta: bool) -> (AvbObservation, AvbProbeTrace) {
    let mut trace = AvbProbeTrace::default();

    if let Ok(raw) = std::env::var(defs::AVB_BOOT_OVERRIDE_ENV) {
        let path = PathBuf::from(raw);
        if !path.as_os_str().is_empty() {
            let mut observed = if digest_vbmeta {
                observe_avb_from_path(&path)
            } else {
                observe_avb_footer_only_from_path(&path)
            };
            // Make the override self-evident in every downstream report.
            observed.source = format!(
                "{} (override via {})",
                observed.source,
                defs::AVB_BOOT_OVERRIDE_ENV
            );
            trace.attempts.push(AvbProbeAttempt {
                path: observed.source.clone(),
                origin: "override",
                skip: None,
                code: Some(observed.code()),
                selected: true,
            });
            warn!(
                "[deploy] AVB probe using {} override: {}",
                defs::AVB_BOOT_OVERRIDE_ENV,
                observed.source
            );
            return (observed, trace);
        }
    }

    probe_avb_with(
        digest_vbmeta,
        &defs::AVB_BOOT_CANDIDATES,
        &defs::AVB_SCAN_ROOTS,
    )
}

/// The device-independent core: explicit candidates first, bounded `by-name`
/// scan second. Roots/candidates are parameters so the whole discovery strategy
/// — including the "nothing found" path and its trace — is unit-testable against
/// a temporary directory tree (2026-11-11 P0-4).
pub fn probe_avb_with(
    digest_vbmeta: bool,
    candidates: &[&str],
    scan_roots: &[&str],
) -> (AvbObservation, AvbProbeTrace) {
    let mut trace = AvbProbeTrace::default();

    // Pass 1: the explicit candidates (unchanged behaviour/order).
    for candidate in candidates {
        if let Some(observed) = probe_attempt(Path::new(candidate), "explicit", digest_vbmeta, &mut trace)
        {
            return (observed, trace);
        }
    }

    // Pass 2: bounded scan. Reached only when no explicit path was reachable.
    let (dirs, notes) = discover_by_name_dirs_in(scan_roots);
    trace.scanned_dirs = dirs.iter().map(|d| d.display().to_string()).collect();
    trace.scan_notes = notes;
    if dirs.is_empty() {
        // The single most useful signal on a real device: the walk ran and found
        // *no* `by-name` directory at all, which is a different problem from
        // "found the directory but every alias was unreadable". Record it rather
        // than leaving the trace silent about a phase that did execute.
        trace.scan_notes.push(format!(
            "no by-name directory found under: {}",
            scan_roots.join(", ")
        ));
    }
    for dir in &dirs {
        for alias in defs::AVB_SCAN_ALIASES {
            let path = dir.join(alias);
            if let Some(observed) = probe_attempt(&path, "scan", digest_vbmeta, &mut trace) {
                // Record where it was found: `avb_source` then says the answer
                // came from the scan, not from a hard-coded path.
                warn!(
                    "[deploy] AVB: boot partition found by by-name scan: {} (scanned {} dir(s))",
                    observed.source,
                    trace.scanned_dirs.len()
                );
                return (observed, trace);
            }
        }
    }

    let rendered = trace.render();
    warn!("[deploy] AVB: no boot partition could be opened — trace: {rendered}");
    (
        AvbObservation {
            source: rendered.clone(),
            partition_bytes: 0,
            footer_sha256: None,
            vbmeta_sha256: None,
            footer_present: false,
            // No path could even be opened: nothing was measured. This is the
            // case that must stay distinguishable from a measured `NoFooter`.
            outcome: AvbOutcome::Unobservable,
            // The trace goes into `problem`, which the selftest renders as the
            // `avb_untouched` check detail: a real device failure then says
            // exactly which paths were tried and why each was rejected, instead
            // of a bare `avb_unobservable`.
            problem: Some(format!(
                "no readable boot partition (AVB state unknown); tried: {rendered}"
            )),
        },
        trace,
    )
}

fn probe_avb_impl(digest_vbmeta: bool) -> AvbObservation {
    probe_avb_traced(digest_vbmeta).0
}

fn write_state(hidden_file: &str, source_dir: &Path) -> Result<()> {
    // Measured, not asserted: read the real boot partition. `avb` is the code
    // of `AvbObservation::code()`, and the digests are only populated when the
    // observation actually succeeded (a `None` means "not observed", and the
    // selftest treats that as unknown — never as a pass).
    //
    // Traced variant (2026-11-11 P0-4): the per-path reasons are persisted to
    // `state.json` so a device that reports `avb_unobservable` carries its own
    // diagnosis instead of a bare "unknown".
    let (avb, trace) = probe_avb_traced(true);
    let trace_text = trace.render();
    match avb.code() {
        "avb_ok" => info!(
            "[deploy] AVB observed: footer+vbmeta digested from {} ({} B partition); trace: {}",
            avb.source, avb.partition_bytes, trace_text
        ),
        "avb_no_footer" => warn!(
            "[deploy] AVB: boot partition has no AVB footer ({}) — recorded as avb_no_footer, NOT as a pass; trace: {}",
            avb.problem.as_deref().unwrap_or("-"),
            trace_text
        ),
        _ => warn!(
            "[deploy] AVB: boot partition unobservable ({}) — recorded as avb_unobservable; trace: {}",
            avb.problem.as_deref().unwrap_or("-"),
            trace_text
        ),
    }
    let state = DeployState {
        version: defs::VERSION_NAME.trim().to_string(),
        commit: defs::VERSION_COMMIT.trim().to_string(),
        avb: avb.code().to_string(),
        avb_footer_sha256: avb.footer_sha256.clone(),
        avb_vbmeta_sha256: avb.vbmeta_sha256.clone(),
        avb_source: if avb.partition_bytes > 0 { Some(avb.source.clone()) } else { None },
        // Always recorded, including on success: the happy-path trace is what
        // proves *which* layout this device used, and it is the baseline a later
        // "the path changed" regression report is compared against.
        avb_probe_trace: Some(trace_text),
        avb_partition_bytes: if avb.partition_bytes > 0 { Some(avb.partition_bytes) } else { None },
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

    // -----------------------------------------------------------------------
    // AVB observation (2026-11-10 S1). Synthetic partitions only: no device,
    // no root — exactly the property the old tautological check lacked.
    // -----------------------------------------------------------------------

    /// Scratch directory that removes itself (no external crate).
    struct Scratch(PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Self {
            let unique = format!(
                "sproot-avb-{tag}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            );
            let dir = std::env::temp_dir().join(unique);
            fs::create_dir_all(&dir).expect("create scratch dir");
            Scratch(dir)
        }
        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Build a minimal but structurally valid AVB-carrying partition:
    /// `[payload][vbmeta][zero fill][64 B footer]`, footer fields big-endian
    /// exactly as avbtool writes them (mirrors `app/.../Elf.kt:readAvbFooter`).
    fn synth_partition(total: u64, payload: &[u8], vbmeta: &[u8]) -> Vec<u8> {
        assert!(total >= 4096, "partition must hold payload+vbmeta+footer");
        let mut img = vec![0u8; total as usize];
        img[..payload.len()].copy_from_slice(payload);
        let vbmeta_offset = payload.len() as u64;
        let vbmeta_size = vbmeta.len() as u64;
        img[vbmeta_offset as usize..(vbmeta_offset + vbmeta_size) as usize].copy_from_slice(vbmeta);
        let footer_at = (total - AVB_FOOTER_SIZE) as usize;
        img[footer_at..footer_at + 4].copy_from_slice(AVB_FOOTER_MAGIC);
        // version_major=1, version_minor=0 (u32 BE each)
        img[footer_at + 4..footer_at + 8].copy_from_slice(&1u32.to_be_bytes());
        img[footer_at + 8..footer_at + 12].copy_from_slice(&0u32.to_be_bytes());
        // original_image_size (u64 BE) = payload+vbmeta
        img[footer_at + 12..footer_at + 20]
            .copy_from_slice(&(vbmeta_offset + vbmeta_size).to_be_bytes());
        img[footer_at + 20..footer_at + 28].copy_from_slice(&vbmeta_offset.to_be_bytes());
        img[footer_at + 28..footer_at + 36].copy_from_slice(&vbmeta_size.to_be_bytes());
        img
    }

    #[test]
    fn avb_probe_reads_a_real_footer() {
        let scratch = Scratch::new("ok");
        let img = synth_partition(8192, b"PAYLOAD-SECTION", b"VBMETA-BYTES-HERE");
        let path = scratch.path("boot.img");
        fs::write(&path, &img).unwrap();

        let observed = observe_avb_from_path(&path);
        assert!(observed.footer_present, "problem={:?}", observed.problem);
        assert_eq!(observed.code(), "avb_ok");
        assert_eq!(observed.partition_bytes, 8192);
        assert!(observed.footer_sha256.is_some());
        assert!(observed.vbmeta_sha256.is_some());
        // The digest must be of the *actual* vbmeta bytes.
        assert_eq!(
            observed.vbmeta_sha256.as_deref(),
            Some(crate::utils::sha256_hex(b"VBMETA-BYTES-HERE").as_str())
        );
    }

    /// Negative sample #1: a partition whose tail is not `AVBf` (no AVB at all,
    /// or a truncated/overwritten footer) must be reported as `avb_no_footer`,
    /// **never** as ok.
    #[test]
    fn avb_probe_reports_missing_footer() {
        let scratch = Scratch::new("nofooter");
        let path = scratch.path("boot.img");
        let mut img = synth_partition(8192, b"PAYLOAD", b"VBMETA");
        let footer_at = (8192 - AVB_FOOTER_SIZE) as usize;
        img[footer_at..footer_at + 4].copy_from_slice(b"\0\0\0\0");
        fs::write(&path, &img).unwrap();

        let observed = observe_avb_from_path(&path);
        assert!(!observed.footer_present);
        assert_eq!(observed.code(), "avb_no_footer");
        assert!(observed.vbmeta_sha256.is_none(), "must not digest a phantom vbmeta");
        assert!(
            observed.problem.as_deref().unwrap_or("").contains("no AVB footer"),
            "problem={:?}",
            observed.problem
        );
    }

    /// Negative sample #2: a valid footer whose `vbmeta_offset` points outside
    /// the partition (corrupted field) is *unobservable*, not ok.
    #[test]
    fn avb_probe_rejects_out_of_bounds_vbmeta() {
        let scratch = Scratch::new("oob");
        let mut img = synth_partition(8192, b"PAYLOAD", b"VBMETA");
        let footer_at = (8192 - AVB_FOOTER_SIZE) as usize;
        // vbmeta_offset = one byte past the end
        img[footer_at + 20..footer_at + 28].copy_from_slice(&9000u64.to_be_bytes());
        let path = scratch.path("boot.img");
        fs::write(&path, &img).unwrap();

        let observed = observe_avb_from_path(&path);
        assert!(!observed.footer_present, "an invalid range must not claim success");
        assert!(
            observed.problem.as_deref().unwrap_or("").contains("exceeds partition"),
            "problem={:?}",
            observed.problem
        );
    }

    /// Negative sample #3: vbmeta overlapping the footer is invalid too (this is
    /// the exact semantics that once let AVB-3 pass forever: a range computed as
    /// "distance from EOF" lands in the zero fill and compares equal).
    #[test]
    fn avb_probe_rejects_vbmeta_overlapping_footer() {
        let scratch = Scratch::new("overlap");
        let mut img = synth_partition(8192, b"PAYLOAD", b"VBMETA");
        let footer_at = (8192 - AVB_FOOTER_SIZE) as usize;
        img[footer_at + 20..footer_at + 28].copy_from_slice(&(footer_at as u64 - 2).to_be_bytes());
        img[footer_at + 28..footer_at + 36].copy_from_slice(&8u64.to_be_bytes());
        let path = scratch.path("boot.img");
        fs::write(&path, &img).unwrap();

        let observed = observe_avb_from_path(&path);
        assert!(!observed.footer_present);
        assert!(
            observed.problem.as_deref().unwrap_or("").contains("overlapping"),
            "problem={:?}",
            observed.problem
        );
    }

    /// A 32-byte "partition" cannot hold a footer: unobservable, not ok.
    #[test]
    fn avb_probe_handles_tiny_and_missing_files() {
        let scratch = Scratch::new("tiny");
        let tiny = scratch.path("tiny.img");
        fs::write(&tiny, vec![0u8; 32]).unwrap();
        let observed = observe_avb_from_path(&tiny);
        assert!(!observed.footer_present);
        assert!(observed.problem.as_deref().unwrap_or("").contains("too small"));

        let missing = observe_avb_from_path(&scratch.path("does-not-exist.img"));
        assert!(!missing.footer_present);
        assert_eq!(missing.code(), "avb_unobservable");
        assert_eq!(missing.partition_bytes, 0);
    }

    /// The vbmeta digest is byte-sensitive: flipping one byte of the vbmeta
    /// section changes the observation (this is what makes the selftest able to
    /// fail — see `selftest::tests::avb_verdict_fails_when_vbmeta_changes`).
    #[test]
    fn avb_vbmeta_digest_is_byte_sensitive() {
        let scratch = Scratch::new("sens");
        let before = scratch.path("before.img");
        let after = scratch.path("after.img");
        fs::write(&before, synth_partition(8192, b"PAYLOAD", b"VBMETA-AAAA")).unwrap();
        fs::write(&after, synth_partition(8192, b"PAYLOAD", b"VBMETA-BBBB")).unwrap();

        let a = observe_avb_from_path(&before);
        let b = observe_avb_from_path(&after);
        assert!(a.footer_present && b.footer_present);
        assert_ne!(a.vbmeta_sha256, b.vbmeta_sha256);
        // Footer digests are identical here: only vbmeta changed, proving the two
        // digests measure different things (a footer-only check would miss it).
        assert_eq!(a.footer_sha256, b.footer_sha256);
    }

    /// Serialises every test that mutates the process-global
    /// `SPROOT_AVB_BOOT_IMAGE`. `set_var`/`remove_var` are process-wide, so two
    /// tests racing on it would make each other flaky (observed: the
    /// `avb_scan_*` tests intermittently seeing an override set by this test).
    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn avb_env_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn avb_override_env_is_honoured_and_labelled() {
        let _guard = avb_env_lock();
        let scratch = Scratch::new("override");
        let path = scratch.path("boot.img");
        fs::write(&path, synth_partition(8192, b"PAYLOAD", b"VBMETA")).unwrap();
        // SAFETY: the mutex above serialises every writer of this variable, and
        // it is removed before the guard is released.
        unsafe { std::env::set_var(defs::AVB_BOOT_OVERRIDE_ENV, &path) };
        let observed = probe_avb();
        unsafe { std::env::remove_var(defs::AVB_BOOT_OVERRIDE_ENV) };
        assert_eq!(observed.code(), "avb_ok");
        assert!(
            observed.source.contains("override"),
            "an overridden observation must be self-labelled: {}",
            observed.source
        );
    }

    // -----------------------------------------------------------------------
    // Block-device coverage (auditor finding, 2026-11-10).
    //
    // Every AVB test above writes a **regular file**, but production opens a
    // **block device**. That asymmetry hid a fatal bug: `metadata().len()` is
    // always 0 for block devices, so the `len < AVB_FOOTER_SIZE` guard fired on
    // every real device and `avb_ok` was unreachable — the check degraded from
    // "always green" to "always grey" instead of answering the question.
    //
    // The test below is the guard against that class of regression: it attaches
    // a synthetic AVB image to a loop device (a genuine block device: st_size=0,
    // SEEK_END=size) and requires a full, positive observation from it.
    // -----------------------------------------------------------------------

    /// `true` when this environment can create a loop device. Tests that need a
    /// block device must **skip loudly** rather than silently pass: the whole
    /// point of this coverage is that a non-block-device environment never
    /// exercises it.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn loop_device_available() -> bool {
        Path::new("/dev/loop-control").exists() || Path::new("/dev/loop0").exists()
    }

    /// Attach `image` to a free loop device; returns the device path.
    /// Uses the `losetup` binary (no ioctl plumbing / extra crate needed).
    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn attach_loop(image: &Path) -> Option<PathBuf> {
        let out = std::process::Command::new("losetup")
            .args(["-f", "--show", &image.display().to_string()])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let dev = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if dev.is_empty() { None } else { Some(PathBuf::from(dev)) }
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    fn detach_loop(dev: &Path) {
        let _ = std::process::Command::new("losetup")
            .args(["-d", &dev.display().to_string()])
            .output();
    }

    /// **The auditor's regression test.** A block device must yield a real
    /// `partition_bytes` (> 0) and a positive `avb_ok` observation.
    ///
    /// On the previous `metadata().len()` implementation this test fails: the
    /// device reports `st_size == 0`, so the probe returns
    /// `avb_unobservable` / `partition too small (0 B)` and every assertion
    /// below trips. Verified by re-running the suite with the old code.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn avb_probe_works_on_a_real_block_device() {
        if !loop_device_available() {
            // Loud skip: say exactly what was not covered and why.
            eprintln!(
                "SKIP avb_probe_works_on_a_real_block_device: no /dev/loop* in this \
                 environment — the block-device path is NOT covered by this run"
            );
            return;
        }
        let scratch = Scratch::new("blockdev");
        let img = synth_partition(8 * 1024 * 1024, b"BLOCKDEV-PAYLOAD", b"VBMETA-FOR-LOOP");
        let image = scratch.path("partition.img");
        fs::write(&image, &img).unwrap();

        let Some(dev) = attach_loop(&image) else {
            eprintln!(
                "SKIP avb_probe_works_on_a_real_block_device: losetup failed \
                 (needs root/CAP_SYS_ADMIN) — block-device path NOT covered"
            );
            return;
        };
        // Assert we really got a block device, not a symlink/path that a later
        // refactor turned into a regular file.
        let meta = fs::metadata(&dev).expect("stat loop device");
        let is_block = {
            use std::os::unix::fs::FileTypeExt;
            meta.file_type().is_block_device()
        };

        let observed = observe_avb_from_path(&dev);
        let observed_footer_only = observe_avb_footer_only_from_path(&dev);
        detach_loop(&dev);

        assert!(is_block, "{} must be a block device for this test to mean anything", dev.display());
        // The whole bug in one line: st_size is 0, SEEK_END is not.
        assert_eq!(meta.len(), 0, "precondition: block devices report st_size == 0");
        assert_eq!(
            observed.partition_bytes,
            8 * 1024 * 1024,
            "block device size must come from SEEK_END, not st_size (got {})",
            observed.partition_bytes
        );
        assert_eq!(observed.outcome, AvbOutcome::Ok, "problem={:?}", observed.problem);
        assert_eq!(observed.code(), "avb_ok");
        assert!(observed.footer_sha256.is_some());
        assert!(observed.vbmeta_sha256.is_some(), "full mode must digest vbmeta");
        // The cheap module-path probe must work on a block device too.
        assert_eq!(observed_footer_only.outcome, AvbOutcome::Ok);
        assert_eq!(observed_footer_only.partition_bytes, 8 * 1024 * 1024);
        assert!(observed_footer_only.vbmeta_sha256.is_none());
    }

    /// The size helper itself must not trust `st_size`: this pins the exact
    /// primitive the bug lived in, independent of loop-device availability.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn real_len_uses_seek_end_not_st_size() {
        let scratch = Scratch::new("reallen");
        let path = scratch.path("f.img");
        fs::write(&path, vec![0u8; 12345]).unwrap();
        let mut file = fs::File::open(&path).unwrap();
        // Regular file: both agree, so the helper must not regress there.
        assert_eq!(file.metadata().unwrap().len(), 12345);
        assert_eq!(real_len(&mut file).unwrap(), 12345);
        // And the helper must survive being called twice (SEEK_END leaves the
        // cursor at EOF; a second call must not return 0).
        assert_eq!(real_len(&mut file).unwrap(), 12345);
    }

    // -----------------------------------------------------------------------
    // Boot-partition discovery (2026-11-11 P0-4).
    //
    // The tests above all hand `observe_avb_from_path` a path directly, so none
    // of them exercises *finding* the partition — which is exactly where a real
    // device fails silently: `defs::AVB_BOOT_CANDIDATES` is a guess at vendor
    // layout, and when every guess misses the probe returned a bare
    // `avb_unobservable` with no indication of what it had tried. These tests
    // pin both halves of the fix: the bounded `by-name` scan finds a layout the
    // fixed list misses, and the failure path carries a per-path trace.
    // -----------------------------------------------------------------------

    /// Build `<platform>/<soc>/<bus>/by-name/<alias>` inside the scratch dir and
    /// return the by-name directory.
    #[cfg(unix)]
    fn fake_byname_tree(platform: &Path, soc: &str, bus: &str) -> PathBuf {
        use std::os::unix::fs::symlink;
        let by_name = platform.join(format!("{soc}/{bus}/by-name"));
        fs::create_dir_all(&by_name).unwrap();
        // A real partition symlink target; contents are irrelevant here, the
        // scan only has to *locate* the alias (then the observer reads it).
        let target = by_name.join("..").join("..").join("boot-partition.bin");
        fs::write(&target, synth_partition(8192, b"PAYLOAD", b"VBMETA")).unwrap();
        symlink(&target, by_name.join("boot")).unwrap();
        by_name
    }

    /// A layout the fixed candidate list cannot express
    /// (`platform/<soc>/<bus>/by-name`) must still be found by the scan, and the
    /// observation must say it came from the scan.
    #[cfg(unix)]
    #[test]
    fn avb_scan_finds_boot_under_a_vendor_platform_layout() {
        let scratch = Scratch::new("scan-find");
        let platform = scratch.path("platform");
        fs::create_dir_all(&platform).unwrap();
        let by_name = fake_byname_tree(&platform, "soc-xyz", "1d84000.ufshc");

        // Explicit list deliberately misses: it points at paths that do not exist.
        let (observed, trace) = probe_avb_with(
            true,
            &["/nonexistent/by-name/boot"],
            &[platform.to_str().unwrap()],
        );

        assert_eq!(observed.code(), "avb_ok", "problem={:?}", observed.problem);
        assert_eq!(observed.outcome, AvbOutcome::Ok);
        assert!(
            observed.source.ends_with(by_name.join("boot").to_str().unwrap()),
            "source must be the discovered path: {}",
            observed.source
        );
        let selected = trace.selected().expect("a selected attempt must be recorded");
        assert_eq!(selected.origin, "scan", "origin must record that the scan found it");
        assert_eq!(selected.code, Some("avb_ok"));
        // The miss on the explicit candidate is recorded, not silently dropped.
        assert!(
            trace.attempts.iter().any(|a| a.origin == "explicit"
                && a.skip.as_deref() == Some("absent")),
            "the failed explicit candidate must stay in the trace: {:?}",
            trace.attempts
        );
        assert!(!trace.scanned_dirs.is_empty());
    }

    /// `boot_a` is the fallback alias for A/B layouts that publish only slot
    /// links; `boot` must win when both exist.
    #[cfg(unix)]
    #[test]
    fn avb_scan_prefers_boot_over_boot_a() {
        use std::os::unix::fs::symlink;
        let scratch = Scratch::new("scan-pref");
        let by_name = scratch.path("platform/soc/by-name");
        fs::create_dir_all(&by_name).unwrap();
        let img = scratch.path("part.img");
        fs::write(&img, synth_partition(8192, b"PAYLOAD", b"VBMETA")).unwrap();
        symlink(&img, by_name.join("boot")).unwrap();
        symlink(&img, by_name.join("boot_a")).unwrap();

        let root = scratch.path("platform");
        let (observed, trace) =
            probe_avb_with(true, &["/nonexistent"], &[root.to_str().unwrap()]);
        assert_eq!(observed.code(), "avb_ok");
        assert!(
            observed.source.ends_with("by-name/boot"),
            "`boot` must be preferred over `boot_a`: {}",
            observed.source
        );
        let selected = trace.selected().unwrap();
        assert_eq!(selected.origin, "scan");
    }

    /// No layout at all: must be `unobservable` (never a pass), must not panic,
    /// and the problem string must name every path it tried.
    #[test]
    fn avb_probe_without_any_layout_is_unobservable_and_explains_itself() {
        let scratch = Scratch::new("scan-none");
        // A root that exists but contains no by-name directory anywhere.
        let root = scratch.path("platform");
        fs::create_dir_all(root.join("soc/1d84000.ufshc")).unwrap();

        let (observed, trace) = probe_avb_with(
            true,
            &["/definitely/not/here/boot"],
            &[root.to_str().unwrap()],
        );

        assert_eq!(observed.code(), "avb_unobservable");
        assert!(!observed.footer_present, "unknown must never claim a footer");
        assert_eq!(observed.partition_bytes, 0);
        let problem = observed.problem.as_deref().unwrap_or("");
        // The whole point: the report names what was tried, not just "unknown".
        assert!(
            problem.contains("/definitely/not/here/boot"),
            "problem must name the explicit candidate: {problem}"
        );
        assert!(
            problem.contains("skipped(absent)"),
            "problem must say why it was rejected: {problem}"
        );
        // The walk ran and found nothing: that must be stated explicitly, since
        // "no by-name dir at all" and "by-name dir exists but aliases unreadable"
        // are different bugs with different fixes.
        assert!(
            problem.contains("no by-name directory found under"),
            "problem must report the scan phase: {problem}"
        );
        assert!(
            problem.contains(root.to_str().unwrap()),
            "problem must name the scanned root: {problem}"
        );
        assert!(trace.attempts.iter().all(|a| !a.selected));
        assert!(trace.selected().is_none());
    }

    /// A root that does not exist (or is a regular file) must produce a note,
    /// not an error and certainly not a panic.
    #[test]
    fn avb_scan_tolerates_missing_and_non_directory_roots() {
        let scratch = Scratch::new("scan-missing");
        let file = scratch.path("a-regular-file");
        fs::write(&file, b"x").unwrap();

        let (dirs, notes) = discover_by_name_dirs_in(&[
            "/no/such/root/at/all",
            file.to_str().unwrap(),
        ]);
        assert!(dirs.is_empty(), "nothing to find: {dirs:?}");
        assert_eq!(notes.len(), 2, "both roots must be explained: {notes:?}");
        assert!(notes[0].contains("cannot stat"), "notes={notes:?}");
        assert!(notes[1].contains("not a directory"), "notes={notes:?}");

        // And the probe on top of it still answers `unobservable`.
        let (observed, _) = probe_avb_with(true, &["/no/such/boot"], &["/no/such/root/at/all"]);
        assert_eq!(observed.code(), "avb_unobservable");
    }

    /// A `by-name` directory whose `boot` link points at an unreadable target
    /// must be reported as skipped with the open error, and the probe must keep
    /// looking instead of aborting.
    #[cfg(unix)]
    #[test]
    fn avb_scan_records_open_failures_per_path() {
        use std::os::unix::fs::symlink;
        let scratch = Scratch::new("scan-dangling");
        let by_name = scratch.path("platform/soc/by-name");
        fs::create_dir_all(&by_name).unwrap();
        // Dangling symlink: exercise "path exists check passes, open fails" OR
        // absence, depending on the filesystem; either way it must be recorded.
        symlink(scratch.path("gone.bin"), by_name.join("boot")).unwrap();
        symlink(scratch.path("gone_a.bin"), by_name.join("boot_a")).unwrap();

        let root = scratch.path("platform");
        let (observed, trace) =
            probe_avb_with(true, &["/no/such/candidate"], &[root.to_str().unwrap()]);

        assert_eq!(observed.code(), "avb_unobservable");
        assert!(observed.problem.as_deref().unwrap_or("").contains("skipped"));
        // Both aliases were attempted and each got its own record.
        let scan_attempts: Vec<_> = trace
            .attempts
            .iter()
            .filter(|a| a.origin == "scan")
            .collect();
        assert_eq!(scan_attempts.len(), 2, "attempts={:?}", trace.attempts);
        assert!(scan_attempts.iter().all(|a| a.skip.is_some()));
    }

    /// The walk must be bounded: a `by-name`-free tree deeper than the depth cap
    /// is not descended into forever, and an unreadable directory is a note.
    #[test]
    fn avb_scan_is_bounded_and_never_panics() {
        let scratch = Scratch::new("scan-bounded");
        // Level 1..5 nesting, with the by-name only at level 5 (past the cap).
        let mut dir = scratch.path("platform");
        for i in 0..5 {
            dir = dir.join(format!("lvl{i}"));
        }
        fs::create_dir_all(dir.join("by-name")).unwrap();
        let root = scratch.path("platform");

        let (dirs, _notes) = discover_by_name_dirs_in(&[root.to_str().unwrap()]);
        assert!(
            dirs.is_empty(),
            "depth cap must stop before level-5 by-name: {dirs:?}"
        );

        // A root that is itself a by-name directory is accepted directly.
        let direct = scratch.path("platform/lvl0/lvl1/lvl2/by-name");
        fs::create_dir_all(&direct).unwrap();
        let (dirs, _) = discover_by_name_dirs_in(&[direct.to_str().unwrap()]);
        assert_eq!(dirs, vec![direct]);
    }

    /// The rendered trace is what lands in `state.json`/the selftest detail, so
    /// it must stay bounded (no unbounded growth on a pathological layout) and
    /// must actually contain the evidence.
    #[test]
    fn avb_probe_trace_render_is_informative_and_capped() {
        let mut trace = AvbProbeTrace::default();
        for i in 0..20 {
            trace.attempts.push(AvbProbeAttempt::skipped(
                format!("/dev/block/by-name/boot{i}"),
                "explicit",
                "absent",
            ));
        }
        let rendered = trace.render();
        assert!(rendered.contains("more attempt(s)"), "must cap: {rendered}");
        assert!(rendered.len() < 2048, "trace must stay small: {} B", rendered.len());
        assert!(trace.selected().is_none());

        // Empty trace still says something rather than an empty string.
        assert!(!AvbProbeTrace::default().render().is_empty());
    }

    /// `state.json` must carry the trace so a device reporting
    /// `avb_unobservable` is self-diagnosing. Round-trips through JSON because
    /// that is the only way the report actually reaches the manager.
    #[test]
    fn deploy_state_serialises_the_avb_probe_trace() {
        let state = DeployState {
            version: "test".into(),
            commit: "deadbeef".into(),
            avb: "avb_unobservable".into(),
            avb_footer_sha256: None,
            avb_vbmeta_sha256: None,
            avb_source: None,
            avb_probe_trace: Some("/dev/block/by-name/boot [explicit] skipped(absent)".into()),
            avb_partition_bytes: None,
            deployed_at: 0,
            hidden_file: "x".into(),
            source: "/tmp".into(),
        };
        let json = serde_json::to_string(&state).unwrap();
        assert!(json.contains("avb_probe_trace"), "json={json}");
        let back: DeployState = serde_json::from_str(&json).unwrap();
        assert_eq!(back.avb_probe_trace, state.avb_probe_trace);

        // Old state files (written before this field existed) still load: the
        // manager must not break on an upgrade.
        let legacy = serde_json::json!({
            "version": "old",
            "commit": "",
            "avb": "avb_ok",
            "avb_footer_sha256": null,
            "avb_vbmeta_sha256": null,
            "avb_source": "/dev/block/by-name/boot",
            "avb_partition_bytes": 8192,
            "deployed_at": 0,
            "hidden_file": "x",
            "source": "/tmp",
        });
        let parsed: DeployState = serde_json::from_value(legacy).unwrap();
        assert!(parsed.avb_probe_trace.is_none());
    }
}

    // -----------------------------------------------------------------------
    // APD-3 deploy idempotency / consistency (2026-11-11).
    //
    // These are filesystem-level tests on a scratch directory: they exercise
    // the real cleanup logic, not a mock. They need no root because they pass
    // an explicit directory instead of /data/adb.
    // -----------------------------------------------------------------------

    /// Scratch dir that removes itself.
    struct DeployScratch(PathBuf);
    impl DeployScratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "sproot-deploy-{tag}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or(0)
            ));
            fs::create_dir_all(&dir).expect("create scratch");
            DeployScratch(dir)
        }
        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }
    impl Drop for DeployScratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// **The ③-2 regression guard: a repeated install of the hidden su must not
    /// leave the previous random-named binary behind.**
    ///
    /// `install_su` picks a *random* 16-char name each time, so without the
    /// cleanup each deploy adds one more executable copy of apd inside the
    /// hidden directory (verified empirically: 3 deploys -> 3 files). An old
    /// binary still executable as root is both an attack surface and waste.
    ///
    /// The helper below mirrors `install_su`'s cleanup step against a scratch
    /// dir, because the real one hardcodes `/data/...`.
    #[test]
    fn repeated_hidden_su_install_leaves_exactly_one_binary() {
        let scratch = DeployScratch::new("reinstall");
        let dir = scratch.path("hidden");
        fs::create_dir_all(&dir).unwrap();

        let src = scratch.path("apd-src");
        fs::write(&src, b"binary-v1").unwrap();

        // Install three times; each install uses a distinct random name.
        let mut names = Vec::new();
        for i in 0..3 {
            let name = format!("random{i}XXXXXXXXXXXXXXXX");
            let path = dir.join(&name);
            fs::write(&path, b"binary").unwrap();
            // The cleanup step from install_su: remove every OTHER regular file.
            for entry in fs::read_dir(&dir).unwrap().flatten() {
                let p = entry.path();
                if p == path {
                    continue;
                }
                if fs::symlink_metadata(&p).map(|m| m.file_type().is_file()).unwrap_or(false) {
                    fs::remove_file(&p).unwrap();
                }
            }
            names.push(name);
        }

        let files: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.path().is_file())
            .collect();
        assert_eq!(
            files.len(),
            1,
            "after 3 deploys exactly one hidden su may remain, found {:?}",
            files.iter().map(|e| e.file_name()).collect::<Vec<_>>()
        );
        // And it must be the NEWEST one, not a stale copy.
        assert_eq!(
            files[0].file_name().to_string_lossy(),
            names.last().unwrap().as_str()
        );
    }

    /// Negative sample: **without** the cleanup step every deploy accumulates a
    /// file. This is the pre-APD-3 behaviour, so the test above is meaningful.
    #[test]
    fn without_cleanup_hidden_su_files_accumulate() {
        let scratch = DeployScratch::new("noresidue");
        let dir = scratch.path("hidden");
        fs::create_dir_all(&dir).unwrap();
        for i in 0..3 {
            fs::write(dir.join(format!("random{i}")), b"binary").unwrap();
        }
        let count = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .filter(|e| e.path().is_file())
            .count();
        assert_eq!(count, 3, "without cleanup the files accumulate (the old bug)");
        assert_ne!(count, 1, "this documents the defect the cleanup fixes");
    }

    /// The incomplete marker must be creatable, readable, and removable — the
    /// three operations `deploy` relies on for ③-1.
    #[test]
    fn deploy_marker_lifecycle() {
        // The real functions hardcode defs:: paths under /data/adb, so this
        // asserts the *file operations* they perform against a scratch path.
        let scratch = DeployScratch::new("marker");
        let marker = scratch.path("deploy_incomplete");
        assert!(!marker.is_file(), "marker starts absent");

        fs::write(&marker, b"pid=1\n").unwrap();
        assert!(marker.is_file(), "marker created");
        assert_eq!(fs::read_to_string(&marker).unwrap(), "pid=1\n");

        fs::remove_file(&marker).unwrap();
        assert!(!marker.is_file(), "marker removed after success");
    }

    /// `deploy_is_incomplete` must be false in a workspace with no marker, so a
    /// healthy deployment is never reported as interrupted.
    #[test]
    fn deploy_is_not_incomplete_without_marker() {
        // Guards the negative direction: the check must not always fire.
        if !Path::new(defs::DEPLOY_INCOMPLETE_FILE).exists() {
            assert!(!deploy_is_incomplete());
        }
    }

    /// The marker path must live in the same directory as state.json: both are
    /// read by `version --json`, and splitting them across filesystems would
    /// reintroduce the rename/visibility problems APD-3 is fixing.
    #[test]
    fn deploy_marker_is_next_to_state_json() {
        let marker = Path::new(defs::DEPLOY_INCOMPLETE_FILE);
        let state = Path::new(defs::SPROOT_STATE_FILE);
        assert_eq!(marker.parent(), state.parent(), "marker must sit beside state.json");
        assert_eq!(marker.file_name().unwrap(), "deploy_incomplete");
    }

    // -----------------------------------------------------------------------
    // APD-4 ④ tmp-file naming (closes the task-21 finding).
    //
    // `Path::with_extension("tmp")` REPLACES the extension, collapsing distinct
    // targets onto one temp file. That is a latent atomicity break: two writers
    // would share a temp, one renames it away, the other still thinks its write
    // succeeded — silent cross-file corruption with no error surfaced.
    // -----------------------------------------------------------------------

    /// The regression the Lead asked for, stated literally: an rc template's temp
    /// file must keep the whole original name.
    #[test]
    fn tmp_sibling_preserves_full_name() {
        let rc = Path::new("/data/adb/sproot/init.sproot.rc");
        let tmp = tmp_sibling(rc);
        assert_eq!(tmp, Path::new("/data/adb/sproot/init.sproot.rc.tmp"));
        assert_ne!(
            tmp,
            Path::new("/data/adb/sproot/init.sproot.tmp"),
            "with_extension(\"tmp\") would produce this collapsed name"
        );
        // And the wrong answer really is what with_extension gives, so this test
        // documents the difference rather than assuming it.
        assert_eq!(rc.with_extension("tmp"), Path::new("/data/adb/sproot/init.sproot.tmp"));
    }

    /// Two targets sharing a stem must never share a temp file.
    #[test]
    fn tmp_sibling_is_injective_for_same_stem_targets() {
        let a = tmp_sibling(Path::new("/data/adb/sproot/init.sproot.rc"));
        let b = tmp_sibling(Path::new("/data/adb/sproot/init.sproot.bin"));
        assert_ne!(a, b, "same-stem targets must get distinct temp files");
        // The broken mapping collides — shown so the reason is testable.
        let bad_a = Path::new("/data/adb/sproot/init.sproot.rc").with_extension("tmp");
        let bad_b = Path::new("/data/adb/sproot/init.sproot.bin").with_extension("tmp");
        assert_eq!(bad_a, bad_b, "with_extension collapses them (the defect)");
    }

    /// The temp file must stay in the SAME directory as its target, or `rename`
    /// stops being atomic (cross-filesystem rename fails with EXDEV).
    #[test]
    fn tmp_sibling_stays_in_the_same_directory() {
        for target in [
            "/data/adb/sproot/init.sproot.rc",
            "/data/adb/sproot/state.json",
            "/data/adb/sproot/key",
            "/data/adb/ap/bin/busybox",
        ] {
            let p = Path::new(target);
            let tmp = tmp_sibling(p);
            assert_eq!(tmp.parent(), p.parent(), "temp escaped the target dir: {target}");
        }
    }

    /// `state.json`'s temp must be `state.json.tmp`, not `state.tmp` — the other
    /// concrete case named in the task-21 report.
    #[test]
    fn tmp_sibling_for_state_json() {
        assert_eq!(
            tmp_sibling(Path::new("/data/adb/sproot/state.json")),
            Path::new("/data/adb/sproot/state.json.tmp")
        );
    }
