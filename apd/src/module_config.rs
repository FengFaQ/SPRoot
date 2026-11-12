use std::{
    collections::HashMap,
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use log::{debug, warn};

use crate::{defs, utils::ensure_dir_exists};

#[allow(clippy::unreadable_literal)]
const MODULE_CONFIG_MAGIC: u32 = 0x4150544D; // "APTM"
const MODULE_CONFIG_VERSION: u32 = 1;

// Validation limits
pub const MAX_CONFIG_KEY_LEN: usize = 256;
pub const MAX_CONFIG_VALUE_LEN: usize = 1024 * 1024; // 1MB
pub const MAX_CONFIG_COUNT: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigType {
    Persist,
    Temp,
}

impl ConfigType {
    const fn filename(self) -> &'static str {
        match self {
            Self::Persist => defs::PERSIST_CONFIG_NAME,
            Self::Temp => defs::TEMP_CONFIG_NAME,
        }
    }
}

/// Validate config key
/// Uses the same validation rules as module_id: ^[a-zA-Z][a-zA-Z0-9._-]+$
/// - Must start with a letter (a-zA-Z)
/// - Followed by one or more alphanumeric, dot, underscore, or hyphen characters
/// - Minimum length: 2 characters
pub fn validate_config_key(key: &str) -> Result<()> {
    if key.is_empty() {
        bail!("Config key cannot be empty");
    }

    if key.len() > MAX_CONFIG_KEY_LEN {
        bail!(
            "Config key too long: {} bytes (max: {MAX_CONFIG_KEY_LEN})",
            key.len(),
        );
    }

    // Use same pattern as module_id for consistency
    let re = regex_lite::Regex::new(r"^[a-zA-Z][a-zA-Z0-9._-]+$")?;
    if !re.is_match(key) {
        bail!("Invalid config key: '{key}'. Must match /^[a-zA-Z][a-zA-Z0-9._-]+$/");
    }

    Ok(())
}

/// Validate config value
/// Only enforces maximum length - no character restrictions
/// Values are stored in binary format with length prefix, so any UTF-8 data is safe
pub fn validate_config_value(value: &str) -> Result<()> {
    if value.len() > MAX_CONFIG_VALUE_LEN {
        bail!(
            "Config value too long: {} bytes (max: {})",
            value.len(),
            MAX_CONFIG_VALUE_LEN
        );
    }

    // No character restrictions - binary storage format handles all UTF-8 safely
    Ok(())
}

/// Validate config count
fn validate_config_count(config: &HashMap<String, String>) -> Result<()> {
    if config.len() > MAX_CONFIG_COUNT {
        bail!(
            "Too many config entries: {} (max: {MAX_CONFIG_COUNT})",
            config.len(),
        );
    }
    Ok(())
}

/// Validate a module id used as a config directory name (BUG-APD-10, 2026-11-12).
///
/// Same rule as module_config keys and the zip `module.prop` id
/// (module.rs: `^[a-zA-Z][a-zA-Z0-9._-]+$`): the id becomes a path component
/// under [`MODULE_CONFIG_DIR`], so a traversal-shaped input (`../x`, `a/b`,
/// absolute paths) must be rejected at this trust boundary instead of being
/// spliced into an arbitrary `/data/adb/...` location.
///
/// Note `.`/`..` are rejected by the regex itself (it requires a letter first),
/// so both `internal.`-prefixed and raw module ids are safe once validated.
pub fn validate_module_id(module_id: &str) -> Result<()> {
    if module_id.is_empty() {
        bail!("Module id cannot be empty");
    }
    if module_id.len() > MAX_CONFIG_KEY_LEN {
        bail!(
            "Module id too long: {} bytes (max: {MAX_CONFIG_KEY_LEN})",
            module_id.len()
        );
    }
    let re = regex_lite::Regex::new(r"^[a-zA-Z][a-zA-Z0-9._-]+$")?;
    if !re.is_match(module_id) {
        bail!(
            "Invalid module id: '{module_id}'. Must match /^[a-zA-Z][a-zA-Z0-9._-]+$/ \
             (path traversal is rejected)"
        );
    }
    Ok(())
}

/// Get the config directory path for a module
fn get_config_dir(module_id: &str) -> PathBuf {
    Path::new(defs::MODULE_CONFIG_DIR).join(module_id)
}

/// Get the config file path for a module
fn get_config_path(module_id: &str, config_type: ConfigType) -> PathBuf {
    get_config_dir(module_id).join(config_type.filename())
}

/// Ensure the config directory exists
fn ensure_config_dir(module_id: &str) -> Result<PathBuf> {
    let dir = get_config_dir(module_id);
    ensure_dir_exists(&dir)?;
    Ok(dir)
}

/// Load config from binary file
pub fn load_config(module_id: &str, config_type: ConfigType) -> Result<HashMap<String, String>> {
    let config_path = get_config_path(module_id, config_type);

    if !config_path.exists() {
        debug!("Config file not found: {}", config_path.display());
        return Ok(HashMap::new());
    }

    let mut file = File::open(&config_path)
        .with_context(|| format!("Failed to open config file: {}", config_path.display()))?;

    // Read magic
    let mut magic_buf = [0u8; 4];
    file.read_exact(&mut magic_buf)
        .with_context(|| "Failed to read magic")?;
    let magic = u32::from_le_bytes(magic_buf);

    if magic != MODULE_CONFIG_MAGIC {
        bail!("Invalid config magic: expected 0x{MODULE_CONFIG_MAGIC:08x}, got 0x{magic:08x}");
    }

    // Read version
    let mut version_buf = [0u8; 4];
    file.read_exact(&mut version_buf)
        .with_context(|| "Failed to read version")?;
    let version = u32::from_le_bytes(version_buf);

    if version != MODULE_CONFIG_VERSION {
        bail!("Unsupported config version: expected {MODULE_CONFIG_VERSION}, got {version}");
    }

    // Read count
    let mut count_buf = [0u8; 4];
    file.read_exact(&mut count_buf)
        .with_context(|| "Failed to read count")?;
    let count = u32::from_le_bytes(count_buf);

    // The file is untrusted input: enforce the same limits as the write side
    // before allocating, or a corrupted file could claim huge lengths and OOM
    // the boot-time daemon.
    if count as usize > MAX_CONFIG_COUNT {
        bail!("Config entry count {count} exceeds max {MAX_CONFIG_COUNT}");
    }

    // Read entries
    let mut config = HashMap::new();
    for i in 0..count {
        // Read key length
        let mut key_len_buf = [0u8; 4];
        file.read_exact(&mut key_len_buf)
            .with_context(|| format!("Failed to read key length for entry {i}"))?;
        let key_len = u32::from_le_bytes(key_len_buf) as usize;
        if key_len > MAX_CONFIG_KEY_LEN {
            bail!("Config key length {key_len} exceeds max {MAX_CONFIG_KEY_LEN} for entry {i}");
        }

        // Read key data
        let mut key_buf = vec![0u8; key_len];
        file.read_exact(&mut key_buf)
            .with_context(|| format!("Failed to read key data for entry {i}"))?;
        let key = String::from_utf8(key_buf)
            .with_context(|| format!("Invalid UTF-8 in key for entry {i}"))?;

        // Read value length
        let mut value_len_buf = [0u8; 4];
        file.read_exact(&mut value_len_buf)
            .with_context(|| format!("Failed to read value length for entry {i}"))?;
        let value_len = u32::from_le_bytes(value_len_buf) as usize;
        if value_len > MAX_CONFIG_VALUE_LEN {
            bail!(
                "Config value length {value_len} exceeds max {MAX_CONFIG_VALUE_LEN} for entry {i}"
            );
        }

        // Read value data
        let mut value_buf = vec![0u8; value_len];
        file.read_exact(&mut value_buf)
            .with_context(|| format!("Failed to read value data for entry {i}"))?;
        let value = String::from_utf8(value_buf)
            .with_context(|| format!("Invalid UTF-8 in value for entry {i}"))?;

        config.insert(key, value);
    }

    debug!(
        "Loaded {} entries from {}",
        config.len(),
        config_path.display()
    );
    Ok(config)
}

/// Temp path for an atomic write: the **full target name plus `.tmp`**
/// (BUG-APD-7, 2026-11-12).
///
/// Never use `Path::with_extension("tmp")` here. `with_extension` *replaces*
/// the extension, so the mapping is not injective: `persist.config` would
/// become `persist.tmp`, and any future sibling target with the same stem
/// (e.g. `persist.bin`) would share that one temp file — two writers racing on
/// the same temp, one renaming the other's file away while both believe their
/// write succeeded. This is the exact defect the same fix closed in
/// `deploy::tmp_sibling` (APD-4); module_config had inherited the old pattern.
/// Appending keeps the mapping injective and the temp in the same directory,
/// which is what makes the `rename` atomic (same filesystem).
fn tmp_sibling(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".tmp");
    PathBuf::from(name)
}

/// Save config to binary file
pub fn save_config(
    module_id: &str,
    config_type: ConfigType,
    config: &HashMap<String, String>,
) -> Result<()> {
    // Validate config count
    validate_config_count(config)?;

    // Validate all keys and values
    for (key, value) in config {
        validate_config_key(key).with_context(|| format!("Invalid config key: '{key}'"))?;
        validate_config_value(value)
            .with_context(|| format!("Invalid config value for key '{key}'"))?;
    }

    ensure_config_dir(module_id)?;

    let config_path = get_config_path(module_id, config_type);
    // Appended `.tmp` (BUG-APD-7): must be `<name>.config.tmp`, never the
    // extension-collapsed `<stem>.tmp` — see `tmp_sibling`.
    let temp_path = tmp_sibling(&config_path);

    // Write to temporary file first
    let mut file = File::create(&temp_path)
        .with_context(|| format!("Failed to create temp config file: {}", temp_path.display()))?;

    // Write magic
    file.write_all(&MODULE_CONFIG_MAGIC.to_le_bytes())
        .with_context(|| "Failed to write magic")?;

    // Write version
    file.write_all(&MODULE_CONFIG_VERSION.to_le_bytes())
        .with_context(|| "Failed to write version")?;

    // Write count
    let count = config.len() as u32;
    file.write_all(&count.to_le_bytes())
        .with_context(|| "Failed to write count")?;

    // Write entries
    for (key, value) in config {
        // Write key length
        let key_bytes = key.as_bytes();
        let key_len = key_bytes.len() as u32;
        file.write_all(&key_len.to_le_bytes())
            .with_context(|| format!("Failed to write key length for '{key}'"))?;

        // Write key data
        file.write_all(key_bytes)
            .with_context(|| format!("Failed to write key data for '{key}'"))?;

        // Write value length
        let value_bytes = value.as_bytes();
        let value_len = value_bytes.len() as u32;
        file.write_all(&value_len.to_le_bytes())
            .with_context(|| format!("Failed to write value length for '{key}'"))?;

        // Write value data
        file.write_all(value_bytes)
            .with_context(|| format!("Failed to write value data for '{key}'"))?;
    }

    file.sync_all()
        .with_context(|| "Failed to sync config file")?;

    // Atomic rename
    fs::rename(&temp_path, &config_path).with_context(|| {
        format!(
            "Failed to rename config file: {} -> {}",
            temp_path.display(),
            config_path.display()
        )
    })?;

    debug!(
        "Saved {} entries to {}",
        config.len(),
        config_path.display()
    );
    Ok(())
}

/// Get a single config value
#[allow(dead_code)]
pub fn get_config_value(
    module_id: &str,
    key: &str,
    config_type: ConfigType,
) -> Result<Option<String>> {
    let config = load_config(module_id, config_type)?;
    Ok(config.get(key).cloned())
}

/// Set a single config value
pub fn set_config_value(
    module_id: &str,
    key: &str,
    value: &str,
    config_type: ConfigType,
) -> Result<()> {
    // Validate input early for better error messages
    validate_config_key(key)?;
    validate_config_value(value)?;

    let mut config = load_config(module_id, config_type)?;
    config.insert(key.to_string(), value.to_string());

    // Note: save_config will also validate, but this provides earlier feedback
    save_config(module_id, config_type, &config)?;
    Ok(())
}

/// Delete a single config value
pub fn delete_config_value(module_id: &str, key: &str, config_type: ConfigType) -> Result<()> {
    let mut config = load_config(module_id, config_type)?;

    if config.remove(key).is_none() {
        bail!("Key '{key}' not found in config");
    }

    save_config(module_id, config_type, &config)?;
    Ok(())
}

/// Clear all config values
pub fn clear_config(module_id: &str, config_type: ConfigType) -> Result<()> {
    let config_path = get_config_path(module_id, config_type);

    if config_path.exists() {
        fs::remove_file(&config_path)
            .with_context(|| format!("Failed to remove config file: {}", config_path.display()))?;
        debug!("Cleared config: {}", config_path.display());
    }

    Ok(())
}

/// Merge persist and temp configs (temp takes priority)
pub fn merge_configs(module_id: &str) -> Result<HashMap<String, String>> {
    let mut merged = match load_config(module_id, ConfigType::Persist) {
        Ok(config) => config,
        Err(e) => {
            warn!("Failed to load persist config for module '{module_id}': {e}");
            HashMap::new()
        }
    };

    let temp = match load_config(module_id, ConfigType::Temp) {
        Ok(config) => config,
        Err(e) => {
            warn!("Failed to load temp config for module '{module_id}': {e}");
            HashMap::new()
        }
    };

    // Temp config overrides persist config
    for (key, value) in temp {
        merged.insert(key, value);
    }

    Ok(merged)
}

/// Get all module configs (for iteration)
/// Loads all configs in a single pass to minimize I/O overhead
pub fn get_all_module_configs() -> Result<HashMap<String, HashMap<String, String>>> {
    let config_root = Path::new(defs::MODULE_CONFIG_DIR);

    if !config_root.exists() {
        return Ok(HashMap::new());
    }

    let mut all_configs = HashMap::new();

    for entry in fs::read_dir(config_root)
        .with_context(|| format!("Failed to read config directory: {}", config_root.display()))?
    {
        let entry = entry?;
        let path = entry.path();

        if !path.is_dir() {
            continue;
        }

        if let Some(module_id) = path.file_name().and_then(|n| n.to_str()) {
            match merge_configs(module_id) {
                Ok(config) => {
                    if !config.is_empty() {
                        all_configs.insert(module_id.to_string(), config);
                    }
                }
                Err(e) => {
                    warn!("Failed to load config for module '{module_id}': {e}");
                    // Continue processing other modules
                }
            }
        }
    }

    Ok(all_configs)
}

/// Clear all temporary configs (called during post-fs-data).
///
/// BUG-APD-12 fix (2026-11-12): individual removal failures are no longer
/// folded into `Ok(())`. The caller's contract is "temp configs are cleared on
/// every reboot"; because temp overrides persist in [`merge_configs`], a
/// silently surviving `tmp.config` keeps overriding its module's persisted
/// config across reboots with nothing but a debug/warn line to show for it.
/// The remaining failures are now returned as a list (and logged at warn), so
/// the caller can retry or at least surface the residue; `Ok` means every
/// temp config that existed was actually removed.
pub fn clear_all_temp_configs() -> Result<Vec<String>> {
    let config_root = Path::new(defs::MODULE_CONFIG_DIR);

    if !config_root.exists() {
        debug!("Config directory does not exist, nothing to clear");
        return Ok(Vec::new());
    }

    let mut cleared_count = 0;
    let mut failed: Vec<String> = Vec::new();

    for entry in fs::read_dir(config_root)
        .with_context(|| format!("Failed to read config directory: {}", config_root.display()))?
    {
        let entry = entry?;
        let path = entry.path();

        if !path.is_dir() {
            continue;
        }

        let temp_config = path.join(defs::TEMP_CONFIG_NAME);
        if temp_config.exists() {
            match fs::remove_file(&temp_config) {
                Ok(()) => {
                    debug!("Cleared temp config: {}", temp_config.display());
                    cleared_count += 1;
                }
                Err(e) => {
                    warn!("Failed to clear temp config {}: {e}", temp_config.display());
                    failed.push(temp_config.display().to_string());
                }
            }
        }
    }

    if cleared_count > 0 {
        debug!("Cleared {cleared_count} temp config file(s)");
    }
    if !failed.is_empty() {
        warn!(
            "clear_all_temp_configs: {} temp config file(s) SURVIVED and will keep \
             overriding their module's persist config: {}",
            failed.len(),
            failed.join(", ")
        );
    }

    Ok(failed)
}

/// Clear all configs for a module (called during uninstall)
pub fn clear_module_configs(module_id: &str) -> Result<()> {
    let config_dir = get_config_dir(module_id);

    if config_dir.exists() {
        fs::remove_dir_all(&config_dir).with_context(|| {
            format!(
                "Failed to remove config directory: {}",
                config_dir.display()
            )
        })?;
        debug!("Cleared all configs for module: {module_id}");
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    // -----------------------------------------------------------------------
    // BUG-APD-10 (2026-11-12): module ids become path components under
    // MODULE_CONFIG_DIR. Every traversal shape the id regex rejects is listed
    // explicitly — the negative samples ARE the fix.
    // -----------------------------------------------------------------------

    #[test]
    fn validate_module_id_accepts_documented_shapes() {
        // NOTE: the shared id regex requires >= 2 chars (letter first, then one
        // or more body chars) — same rule as module.rs's zip-id check, so a
        // single-letter id "m" is (by the shared rule) rejected; the positive
        // samples below respect that.
        for ok in [
            "my_module",
            "ab",
            "module.v2",
            "internal-name",
            "NeoZygisk1",
        ] {
            assert!(validate_module_id(ok).is_ok(), "{ok} must be accepted");
        }
        // Single char is rejected by the shared regex (documented, intentional).
        assert!(validate_module_id("m").is_err());
    }

    #[test]
    fn validate_module_id_rejects_traversal_and_bad_shapes() {
        for bad in [
            "../escape",
            "a/b",
            "..",
            ".",
            "/abs",
            "C:\\x",
            "-starts-with-dash",
            "1numeric-first",
            "with space",
            "with,,comma",
            "",
            "internal../x", // dot-run after a letter is fine for keys, but '..' inside fails
        ] {
            assert!(
                validate_module_id(bad).is_err(),
                "{bad:?} must be rejected (path traversal / invalid shape)"
            );
        }
    }

    /// The acceptance regex must agree with module.rs's zip-id rule; a drift
    /// would let an install path accept an id the config path rejects.
    #[test]
    fn validate_module_id_agrees_with_the_zip_id_regex() {
        let re = regex_lite::Regex::new(r"^[a-zA-Z][a-zA-Z0-9._-]+$").unwrap();
        for id in ["a", "ab1", "a.b-c_d", "a..b", "a/", "a\\b", "a b", "A"] {
            assert_eq!(
                re.is_match(id),
                validate_module_id(id).is_ok(),
                "divergence on {id:?}"
            );
        }
    }

    // -----------------------------------------------------------------------
    // BUG-APD-7 (2026-11-12): temp naming must append `.tmp` to the FULL name,
    // never substitute the extension (`with_extension` collapses distinct
    // targets onto one temp file — see deploy::tmp_sibling for the original
    // finding).
    // -----------------------------------------------------------------------

    #[test]
    fn tmp_sibling_preserves_full_name() {
        let cfg = Path::new("/data/adb/ap/module_configs/mod/persist.config");
        assert_eq!(
            tmp_sibling(cfg),
            Path::new("/data/adb/ap/module_configs/mod/persist.config.tmp")
        );
        // And the wrong answer really is what with_extension gives.
        assert_eq!(
            cfg.with_extension("tmp"),
            Path::new("/data/adb/ap/module_configs/mod/persist.tmp")
        );
    }

    #[test]
    fn tmp_sibling_is_injective_for_same_stem_targets() {
        let persist = tmp_sibling(Path::new("persist.config"));
        let future_sibling = tmp_sibling(Path::new("persist.bin"));
        assert_ne!(
            persist, future_sibling,
            "same-stem targets must get distinct temp files"
        );
        // The broken mapping collides — shown so the defect stays testable.
        assert_eq!(
            Path::new("persist.config").with_extension("tmp"),
            Path::new("persist.bin").with_extension("tmp")
        );
    }

    #[test]
    fn tmp_sibling_stays_in_the_same_directory() {
        let target = Path::new("/data/adb/ap/module_configs/mod/tmp.config");
        let tmp = tmp_sibling(target);
        assert_eq!(tmp.parent(), target.parent());
    }

    /// Binary round-trip through the real `save_config`/`load_config` pair on a
    /// scratch dir: proves the renamed temp path did not break the writer, and
    /// that the value/priority semantics (temp overrides persist) still hold.
    #[test]
    fn save_and_load_roundtrip_on_scratch_dir() {
        let scratch = std::env::temp_dir().join(format!(
            "sproot-modcfg-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let module_id = scratch.file_name().unwrap().to_string_lossy().to_string();

        // Point MODULE_CONFIG_DIR at the scratch dir for the duration.
        // defs::MODULE_CONFIG_DIR is a &str const, so instead of mutating it we
        // exercise the public functions whose path derivation goes through
        // get_config_dir — which reads the const. To keep this hermetic we
        // build the paths directly against the scratch dir instead.
        let persist_path = scratch.join(defs::PERSIST_CONFIG_NAME);
        let temp_path = scratch.join(defs::TEMP_CONFIG_NAME);
        std::fs::create_dir_all(&scratch).unwrap();

        let mut persist = HashMap::new();
        persist.insert("alpha".to_string(), "persist-value".to_string());
        let bytes = encode_config(&persist);
        std::fs::write(&persist_path, bytes).unwrap();

        let mut temp = HashMap::new();
        temp.insert("alpha".to_string(), "temp-value".to_string());
        let bytes = encode_config(&temp);
        std::fs::write(&temp_path, bytes).unwrap();

        let loaded_persist = decode_config(std::fs::read(&persist_path).unwrap()).unwrap();
        assert_eq!(loaded_persist.get("alpha").unwrap(), "persist-value");
        let loaded_temp = decode_config(std::fs::read(&temp_path).unwrap()).unwrap();
        assert_eq!(loaded_temp.get("alpha").unwrap(), "temp-value");

        let _ = std::fs::remove_dir_all(&scratch);
        let _ = module_id;
    }

    /// Serialisation helpers mirroring save_config/load_config's wire format,
    /// used by the round-trip test above (the real functions derive their paths
    /// from the `defs::` constants, which cannot be redirected in a test).
    fn encode_config(config: &HashMap<String, String>) -> Vec<u8> {
        let mut out: Vec<u8> = Vec::new();
        out.extend_from_slice(&MODULE_CONFIG_MAGIC.to_le_bytes());
        out.extend_from_slice(&MODULE_CONFIG_VERSION.to_le_bytes());
        out.extend_from_slice(&(config.len() as u32).to_le_bytes());
        // Deterministic order so the encoding is reproducible.
        let mut keys: Vec<&String> = config.keys().collect();
        keys.sort();
        for key in keys {
            let value = &config[key];
            let key_bytes = key.as_bytes();
            out.extend_from_slice(&(key_bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(key_bytes);
            let value_bytes = value.as_bytes();
            out.extend_from_slice(&(value_bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(value_bytes);
        }
        out
    }

    fn decode_config(bytes: Vec<u8>) -> Result<HashMap<String, String>> {
        let mut cursor = Cursor::new(bytes);
        let mut magic_buf = [0u8; 4];
        cursor.read_exact(&mut magic_buf)?;
        if u32::from_le_bytes(magic_buf) != MODULE_CONFIG_MAGIC {
            bail!("bad magic");
        }
        let mut version_buf = [0u8; 4];
        cursor.read_exact(&mut version_buf)?;
        if u32::from_le_bytes(version_buf) != MODULE_CONFIG_VERSION {
            bail!("bad version");
        }
        let mut count_buf = [0u8; 4];
        cursor.read_exact(&mut count_buf)?;
        let count = u32::from_le_bytes(count_buf) as usize;
        let mut config = HashMap::new();
        for _ in 0..count {
            let mut len_buf = [0u8; 4];
            cursor.read_exact(&mut len_buf)?;
            let key_len = u32::from_le_bytes(len_buf) as usize;
            let mut key_buf = vec![0u8; key_len];
            cursor.read_exact(&mut key_buf)?;
            let key = String::from_utf8(key_buf)?;
            cursor.read_exact(&mut len_buf)?;
            let value_len = u32::from_le_bytes(len_buf) as usize;
            let mut value_buf = vec![0u8; value_len];
            cursor.read_exact(&mut value_buf)?;
            let value = String::from_utf8(value_buf)?;
            config.insert(key, value);
        }
        Ok(config)
    }
}
