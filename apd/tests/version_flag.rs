//! `apd -V` is an **external ABI**, not cosmetics.
//!
//! NeoZygisk (GPL-3.0) / OnyxZygisk (AGPL-3.0) identify an APatch host with
//! `Command::new("apd").arg("-V")`, then
//! `stdout.split_whitespace().nth(1).parse::<i32>()` and require
//! `>= minAPatchVersion = 10762` (`zygiskd/src/root_impl/apatch.rs`,
//! NeoZygisk `build.gradle.kts:21`). If this test ever fails, every external
//! Zygisk implementation silently classifies SPRoot as `TooOld` / unsupported,
//! so it runs the *real* parser against the *real* binary.

use std::process::Command;

#[test]
fn version_flag_is_apatch_compatible() {
    let output = Command::new(env!("CARGO_BIN_EXE_apd"))
        .arg("-V")
        .output()
        .expect("apd -V must be runnable");
    assert!(
        output.status.success(),
        "apd -V exited with {:?}: {}",
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // NeoZygisk's exact parse: second whitespace token as i32.
    let version = stdout
        .split_whitespace()
        .nth(1)
        .expect("apd -V must print '<name> <version>'")
        .parse::<i32>()
        .expect("second token must parse as i32 (NeoZygisk/Versions parser)");
    assert!(
        version >= 10762,
        "advertised APatch version {version} < minAPatchVersion 10762 → Zygisk would report TooOld"
    );
    // Keep the shape minimal: first line is exactly `apd <int>`.
    assert_eq!(
        stdout.lines().next().map(str::trim),
        Some(format!("apd {version}").as_str()),
        "first line must be `apd <int>`; got {stdout:?}"
    );
}
