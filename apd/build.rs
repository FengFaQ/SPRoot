use std::env;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::process::Command;

// v2 layout: this crate lives at <repo>/apd, so the repo root is `..`.
//
// Version sources (single source of truth for both this crate and the manager):
// * VERSION_CODE = managerVersionEpoch (version.properties, optional) + git rev-list --count HEAD
// * VERSION_NAME = git describe --tags --always (falls back to 0.0.0)
//
// The KernelPatch-era `app/src/main/cpp/version` dependency and the generated
// kp_version.rs are gone: SPRoot has no KP runtime and nothing referenced them.

fn get_version_epoch() -> u32 {
    std::fs::read_to_string("../version.properties")
        .ok()
        .and_then(|content| {
            content.lines().find_map(|line| {
                line.strip_prefix("managerVersionEpoch=")
                    .and_then(|v| v.trim().parse::<u32>().ok())
            })
        })
        // No epoch file in the v2 tree yet: plain git count is a valid code.
        .unwrap_or(0)
}

fn run_git(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

fn get_git_version() -> Option<(u32, String)> {
    let count = run_git(&["rev-list", "--count", "HEAD"])?;
    let version_code: u32 = count.trim().parse().ok()?;
    let version_code = get_version_epoch() + version_code;

    let name = run_git(&["describe", "--tags", "--always"]).unwrap_or_else(|| "0.0.0".to_string());
    let name = name.trim().trim_start_matches('v').to_string();
    Some((version_code, name))
}

/// Full commit hash the binary was built from (short form for display).
/// `unknown` when the build is not from a git checkout (source tarball).
fn get_git_commit() -> String {
    run_git(&["rev-parse", "--short=12", "HEAD"])
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

fn main() {
    // Rebuild the version stamp whenever the git state or epoch file changes.
    println!("cargo:rerun-if-changed=../.git/HEAD");
    println!("cargo:rerun-if-changed=../.git/refs/");
    println!("cargo:rerun-if-changed=../version.properties");

    let (code, name) = match get_git_version() {
        Some(version) => version,
        None => {
            // Not a git checkout (e.g. source tarball): keep the build working.
            println!("cargo:warning=git version unavailable, using 0.0.0");
            (get_version_epoch(), "0.0.0".to_string())
        }
    };

    let out_dir = env::var("OUT_DIR").expect("Failed to get $OUT_DIR");
    let out_dir = Path::new(&out_dir);

    File::create(out_dir.join("VERSION_CODE"))
        .expect("Failed to create VERSION_CODE")
        .write_all(code.to_string().as_bytes())
        .expect("Failed to write VERSION_CODE");

    File::create(out_dir.join("VERSION_NAME"))
        .expect("Failed to create VERSION_NAME")
        .write_all(name.trim().as_bytes())
        .expect("Failed to write VERSION_NAME");

    // Commit the manager can compare against the APK-bundled apd-version.txt
    // (version consistency check, NEW-DOC-05 §6.2).
    File::create(out_dir.join("VERSION_COMMIT"))
        .expect("Failed to create VERSION_COMMIT")
        .write_all(get_git_commit().as_bytes())
        .expect("Failed to write VERSION_COMMIT");
}
