//! Authorisation-table CSV I/O for SPRoot (contract v1.1 §3.2/§3.3, E4).
//!
//! One file, six columns: `pkg,exclude,allow,uid,to_uid,sctx`. The v1.0
//! `exclude_list` side file is deliberately not implemented (E4): `exclude` is
//! a row field, so "excluded" = `exclude == 1` on the matching row.
//!
//! Consistency model:
//! * every writer takes an exclusive `flock` on [`defs::ALLOW_LIST_LOCK_FILE`];
//! * writes are tmp-file + rename, so readers never see a torn generation;
//! * a torn read (parse failure) must never be committed as an empty table —
//!   the callers (`auth_list`) keep the historical fail-open/fuse semantics.

use std::{
    fs::{File, OpenOptions},
    io::{self, BufRead},
    os::fd::AsRawFd,
    os::unix::fs::PermissionsExt,
    path::Path,
    thread,
    time::Duration,
};

use log::{info, warn};
use serde::{Deserialize, Serialize};

use crate::defs;

#[derive(Deserialize, Serialize, Clone, Debug)]
pub struct PackageConfig {
    pub pkg: String,
    pub exclude: i32,
    pub allow: i32,
    pub uid: i32,
    pub to_uid: i32,
    pub sctx: String,
}

pub struct PackageConfigRead {
    pub configs: Vec<PackageConfig>,
    pub valid: bool,
}

fn strip_bom(s: &str) -> &str {
    s.trim_start_matches('\u{FEFF}')
}

const HEADER: [&str; 6] = ["pkg", "exclude", "allow", "uid", "to_uid", "sctx"];

fn is_header_record(record: &csv::StringRecord) -> bool {
    if record.len() != HEADER.len() {
        return false;
    }
    record.iter().enumerate().all(|(i, field)| {
        let field = field.trim();
        let field = if i == 0 { strip_bom(field) } else { field };
        field == HEADER[i]
    })
}

fn is_blank_record(record: &csv::StringRecord) -> bool {
    record.iter().all(|f| f.trim().is_empty())
}

/// Read + validate the table (5 attempts, 1s apart, so a concurrent rename is
/// tolerated). `valid == false` means the file must not be rewritten from this
/// snapshot — callers decide between "abort rewrite" (refresh) and "deny"
/// (auth decision).
pub fn read_ap_package_config_validated() -> PackageConfigRead {
    let max_retry = 5;
    for _ in 0..max_retry {
        let file = match File::open(defs::ALLOW_LIST_FILE) {
            Ok(file) => file,
            Err(e) => {
                warn!("Error opening allow_list: {}", e);
                thread::sleep(Duration::from_secs(1));
                continue;
            }
        };

        // has_headers(false): the manager and AP both emit a header line, but a
        // headerless file left by an older tool must not silently swallow its
        // first grant row; parse every row and skip the header by content.
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(false)
            .trim(csv::Trim::All)
            .flexible(true)
            .from_reader(file);
        let mut package_configs = Vec::new();
        let mut success = true;

        for record in reader.records() {
            let record = match record {
                Ok(record) => record,
                Err(e) => {
                    warn!("Error reading CSV record: {}", e);
                    success = false;
                    break;
                }
            };
            if is_blank_record(&record) {
                continue;
            }
            if is_header_record(&record) {
                continue;
            }
            match record.deserialize::<PackageConfig>(None) {
                Ok(mut config) => {
                    if config.pkg.starts_with('\u{FEFF}') {
                        config.pkg = strip_bom(&config.pkg).to_owned();
                    }
                    package_configs.push(config)
                }
                Err(e) => {
                    warn!("Error deserializing record: {}", e);
                    success = false;
                    break;
                }
            }
        }

        if success {
            return PackageConfigRead {
                configs: package_configs,
                valid: true,
            };
        }
        thread::sleep(Duration::from_secs(1));
    }
    PackageConfigRead {
        configs: Vec::new(),
        valid: false,
    }
}

#[allow(dead_code)]
pub fn read_ap_package_config() -> Vec<PackageConfig> {
    read_ap_package_config_validated().configs
}

/// Commit a table generation: tmp file in the same directory, fsync, rename.
/// The caller must already hold the write lock (see [`ConfigLock`]).
pub fn write_ap_package_config(package_configs: &[PackageConfig]) -> io::Result<()> {
    let max_retry = 5;
    for _ in 0..max_retry {
        let temp_path = defs::ALLOW_LIST_TMP_FILE;
        let file = match File::create(temp_path) {
            Ok(file) => file,
            Err(e) => {
                warn!("Error creating allow_list.tmp: {}", e);
                thread::sleep(Duration::from_secs(1));
                continue;
            }
        };
        // 0600: the table holds package names and uid mappings only, but the
        // daemon owns it and no other component needs to read it.
        let _ = file.set_permissions(std::fs::Permissions::from_mode(0o600));

        let mut writer = csv::WriterBuilder::new()
            .has_headers(false)
            .from_writer(file);
        let mut success = true;

        if let Err(e) = writer.write_record(HEADER) {
            warn!("Error serializing header: {}", e);
            success = false;
        }

        if success {
            for config in package_configs {
                if let Err(e) = writer.serialize(config) {
                    warn!("Error serializing record: {}", e);
                    success = false;
                    break;
                }
            }
        }

        if success && let Err(e) = writer.flush() {
            warn!("Error flushing writer: {}", e);
            success = false;
        }

        if success && let Err(e) = writer.get_ref().sync_all() {
            warn!("Error syncing allow_list.tmp: {}", e);
            success = false;
        }
        drop(writer);

        if !success {
            thread::sleep(Duration::from_secs(1));
            continue;
        }

        if let Err(e) = std::fs::rename(temp_path, defs::ALLOW_LIST_FILE) {
            warn!("Error renaming allow_list.tmp: {}", e);
            thread::sleep(Duration::from_secs(1));
            continue;
        }
        return Ok(());
    }
    Err(io::Error::other("Failed after max retries"))
}

/// Exclusive advisory lock over the whole read-modify-write cycle.
///
/// `flock` (not POSIX record locks) is used so that the daemon, the
/// uid-listener thread and the `auth` CLI serialise on the same inode and the
/// lock is released automatically when the fd is closed (no stale-lock file
/// problem). Blocking acquire with a bounded retry loop keeps a hung writer
/// from wedging boot; the fallback is "proceed unlocked" only for reads.
pub struct ConfigLock {
    _file: Option<File>,
}

impl ConfigLock {
    /// Take the lock, blocking. Returns a guard even if the lock file cannot be
    /// opened (best effort, matching the historical "proceed unlocked" warning
    /// semantics) — the atomic rename still keeps readers consistent.
    pub fn acquire() -> ConfigLock {
        let file = match OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(defs::ALLOW_LIST_LOCK_FILE)
        {
            Ok(file) => file,
            Err(e) => {
                warn!("Failed to open allow_list lock, proceeding unlocked: {}", e);
                return ConfigLock { _file: None };
            }
        };
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
            warn!(
                "Failed to acquire allow_list lock, proceeding unlocked: {}",
                io::Error::last_os_error()
            );
            return ConfigLock { _file: None };
        }
        ConfigLock { _file: Some(file) }
    }
}

impl Drop for ConfigLock {
    fn drop(&mut self) {
        // Closing the fd releases the flock; explicit unlock keeps intent clear.
        if let Some(file) = &self._file {
            unsafe {
                libc::flock(file.as_raw_fd(), libc::LOCK_UN);
            }
        }
    }
}

fn read_lines<P>(filename: P) -> io::Result<io::Lines<io::BufReader<File>>>
where
    P: AsRef<Path>,
{
    File::open(filename).map(|file| io::BufReader::new(file).lines())
}

/// Prune rows of uninstalled packages and refresh uids from the system package
/// list. **Caller must hold a [`ConfigLock`]** (this variant neither locks nor
/// logs beyond warnings).
pub fn synchronize_package_uid_locked() -> io::Result<()> {
    info!("[synchronize_package_uid] Start synchronizing allow list with system packages...");

    let max_retry = 5;
    for _ in 0..max_retry {
        match read_lines("/data/system/packages.list") {
            Ok(lines) => {
                // Skip bad lines instead of `map_while(Result::ok)`: truncating
                // at the first error would drop the tail of the list and the
                // retain() below would then revoke grants for those packages.
                #[allow(clippy::lines_filter_map_ok)]
                let lines: Vec<_> = lines.filter_map(|line| line.ok()).collect();

                let system_packages: Vec<String> = lines
                    .iter()
                    .filter_map(|line| line.split_whitespace().next())
                    .map(|pkg| pkg.to_string())
                    .collect();

                // An empty/blank packages.list is almost certainly a torn read
                // (PackageManager rewrites it via .tmp + rename) rather than a
                // device with no packages. Retaining against it would wipe
                // every persisted grant, so refuse to sync in that case.
                if system_packages.is_empty() {
                    warn!(
                        "[synchronize_package_uid] packages.list has no parsable entries, aborting sync"
                    );
                    return Err(io::Error::other("packages.list is empty"));
                }

                // A torn allow_list read must fail-open: syncing (and
                // persisting) against an empty snapshot would drop grants.
                let read = read_ap_package_config_validated();
                if !read.valid {
                    warn!("[synchronize_package_uid] allow_list unreadable, skipping sync");
                    return Err(io::Error::other("allow_list is unreadable"));
                }
                let mut package_configs = read.configs;

                let original_len = package_configs.len();
                package_configs.retain(|config| system_packages.contains(&config.pkg));
                let removed_count = original_len - package_configs.len();

                if removed_count > 0 {
                    info!(
                        "Removed {} uninstalled package configurations",
                        removed_count
                    );
                }

                let mut updated = false;

                for line in &lines {
                    let words: Vec<&str> = line.split_whitespace().collect();
                    if words.len() >= 2 {
                        let pkg_name = words[0];
                        if let Ok(uid) = words[1].parse::<i32>() {
                            for config in package_configs
                                .iter_mut()
                                .filter(|config| config.pkg == pkg_name)
                            {
                                if config.uid % 100000 != uid % 100000 {
                                    let new_uid = config.uid / 100000 * 100000 + uid % 100000;
                                    info!(
                                        "Updating uid for package {}: {} -> {}",
                                        pkg_name, config.uid, new_uid
                                    );
                                    config.uid = new_uid;
                                    updated = true;
                                }
                            }
                        } else {
                            warn!("Error parsing uid: {}", words[1]);
                        }
                    }
                }

                if updated || removed_count > 0 {
                    write_ap_package_config(&package_configs)?;
                }
                return Ok(());
            }
            Err(e) => {
                warn!("Error reading packages.list: {}", e);
                thread::sleep(Duration::from_secs(1));
            }
        }
    }
    Err(io::Error::other("Failed after max retries"))
}

/// Public wrapper: takes the write lock, then prunes/updates the table.
///
/// Kept for call sites that do not already own the lock (`auth` subcommands,
/// manual refresh). The daemon's refresh path uses
/// [`synchronize_package_uid_locked`] to avoid a re-entrant flock.
pub fn synchronize_package_uid() -> io::Result<()> {
    let _lock = ConfigLock::acquire();
    synchronize_package_uid_locked()
}
