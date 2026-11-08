# Third-Party Licenses — apd (SPRoot AP binary)

> Project license: **AGPL-3.0-only** (`apd/Cargo.toml`).
> Generated from `Cargo.lock` with `cargo metadata --filter-platform aarch64-linux-android`
> commit: `5dc6d09` · packages: 115 (transitive, Android target only)

## Summary

| license | count |
|---|---|
| MIT | 100 |
| Apache-2.0 | 88 |
| Zlib | 5 |
| Unlicense | 5 |
| ISC | 2 |
| LLVM-exception | 2 |
| UNKNOWN | 2 |
| BSD-3-Clause | 1 |
| CC0-1.0 | 1 |
| BSL-1.0 | 1 |
| Unicode-3.0 | 1 |

All dependencies are permissive (MIT / Apache-2.0 / BSD / ISC / Zlib / Unlicense / BSL-1.0 / CC0 / Unicode-3.0) **except** the items flagged below.

## Flagged items（architect 需汇总进仓库 NOTICE 前处理）

| package | version | license | source | action |
|---|---|---|---|---|
| `extattr` | 1.0.0 | **GPL-2.0-only** | crates.io | **已移除**：GPL-2.0-only 与 AGPL-3.0 不兼容；已用 raw `libc` 的 `lsetxattr/lgetxattr` 等价替换（`apd/src/restorecon.rs`，wire 行为不变） |
| `prop-rs` | 0.2.0 | **UNKNOWN** | git (AndroidPatch/restprops) | 上游未声明 license（仓库无 LICENSE 文件、Cargo.toml 无 license 字段）。需 architect 向上游确认或要求补 LICENSE；确认前不得发布 |
| `prop-rs-android` | 0.2.0 | **UNKNOWN** | git (同上) | 同上（`resetprop` 依赖，不可简单替换） |

## Full list

| package | version | license | source |
|---|---|---|---|
| `android-properties` | 0.2.2 | MIT | crates.io |
| `android_log-sys` | 0.3.2 | MIT OR Apache-2.0 | crates.io |
| `android_logger` | 0.15.1 | MIT OR Apache-2.0 | crates.io |
| `anstream` | 1.0.0 | MIT OR Apache-2.0 | crates.io |
| `anstyle` | 1.0.14 | MIT OR Apache-2.0 | crates.io |
| `anstyle-parse` | 1.0.0 | MIT OR Apache-2.0 | crates.io |
| `anstyle-query` | 1.1.5 | MIT OR Apache-2.0 | crates.io |
| `anyhow` | 1.0.104 | MIT OR Apache-2.0 | crates.io |
| `autocfg` | 1.5.1 | Apache-2.0 OR MIT | crates.io |
| `bitflags` | 2.13.1 | MIT OR Apache-2.0 | crates.io |
| `block-buffer` | 0.12.1 | MIT OR Apache-2.0 | crates.io |
| `bstr` | 1.13.1 | MIT OR Apache-2.0 | crates.io |
| `bytes` | 1.12.1 | MIT | crates.io |
| `cc` | 1.4.3 | MIT OR Apache-2.0 | crates.io |
| `cfg-if` | 1.0.4 | MIT OR Apache-2.0 | crates.io |
| `clap` | 4.6.6 | MIT OR Apache-2.0 | crates.io |
| `clap_builder` | 4.6.6 | MIT OR Apache-2.0 | crates.io |
| `clap_derive` | 4.6.4 | MIT OR Apache-2.0 | crates.io |
| `clap_lex` | 1.1.0 | MIT OR Apache-2.0 | crates.io |
| `colorchoice` | 1.0.5 | MIT OR Apache-2.0 | crates.io |
| `const-oid` | 0.10.2 | Apache-2.0 OR MIT | crates.io |
| `const_format` | 0.2.36 | Zlib | crates.io |
| `const_format_proc_macros` | 0.2.34 | Zlib | crates.io |
| `cpufeatures` | 0.3.0 | MIT OR Apache-2.0 | crates.io |
| `crc32fast` | 1.5.0 | MIT OR Apache-2.0 | crates.io |
| `crossbeam` | 0.8.4 | MIT OR Apache-2.0 | crates.io |
| `crossbeam-channel` | 0.5.16 | MIT OR Apache-2.0 | crates.io |
| `crossbeam-deque` | 0.8.7 | MIT OR Apache-2.0 | crates.io |
| `crossbeam-epoch` | 0.9.20 | MIT OR Apache-2.0 | crates.io |
| `crossbeam-queue` | 0.3.13 | MIT OR Apache-2.0 | crates.io |
| `crossbeam-utils` | 0.8.22 | MIT OR Apache-2.0 | crates.io |
| `crypto-common` | 0.2.2 | MIT OR Apache-2.0 | crates.io |
| `csv` | 1.4.0 | Unlicense/MIT | crates.io |
| `csv-core` | 0.1.13 | Unlicense/MIT | crates.io |
| `deflate64` | 0.1.12 | MIT | crates.io |
| `digest` | 0.11.3 | MIT OR Apache-2.0 | crates.io |
| `either` | 1.17.0 | MIT OR Apache-2.0 | crates.io |
| `encoding_rs` | 0.8.35 | (Apache-2.0 OR MIT) AND BSD-3-Clause | crates.io |
| `env_filter` | 0.1.4 | MIT OR Apache-2.0 | crates.io |
| `equivalent` | 1.0.2 | Apache-2.0 OR MIT | crates.io |
| `errno` | 0.3.14 | MIT OR Apache-2.0 | crates.io |
| `find-msvc-tools` | 0.1.11 | MIT OR Apache-2.0 | crates.io |
| `flate2` | 1.1.9 | MIT OR Apache-2.0 | crates.io |
| `getopts` | 0.2.24 | MIT OR Apache-2.0 | crates.io |
| `hashbrown` | 0.17.1 | MIT OR Apache-2.0 | crates.io |
| `heck` | 0.5.0 | MIT OR Apache-2.0 | crates.io |
| `hybrid-array` | 0.4.14 | MIT OR Apache-2.0 | crates.io |
| `indexmap` | 2.14.0 | Apache-2.0 OR MIT | crates.io |
| `inotify` | 0.11.5 | ISC | crates.io |
| `inotify-sys` | 0.1.8 | ISC | crates.io |
| `is_executable` | 1.0.6 | MIT OR Apache-2.0 | crates.io |
| `is_terminal_polyfill` | 1.70.2 | MIT OR Apache-2.0 | crates.io |
| `itertools` | 0.14.0 | MIT OR Apache-2.0 | crates.io |
| `itoa` | 1.0.18 | MIT OR Apache-2.0 | crates.io |
| `java-properties` | 2.0.0 | MIT | git |
| `jwalk` | 0.9.0 | MIT | crates.io |
| `konst` | 0.2.20 | Zlib | crates.io |
| `konst_macro_rules` | 0.2.19 | Zlib | crates.io |
| `lazy_static` | 1.5.0 | MIT OR Apache-2.0 | crates.io |
| `libc` | 0.2.189 | MIT OR Apache-2.0 | crates.io |
| `linux-raw-sys` | 0.12.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | crates.io |
| `lock_api` | 0.4.14 | MIT OR Apache-2.0 | crates.io |
| `log` | 0.4.33 | MIT OR Apache-2.0 | crates.io |
| `lua-src` | 550.1.1 | MIT | crates.io |
| `luajit-src` | 210.7.2+b925b3e | MIT | crates.io |
| `lzma-rust2` | 0.16.5 | Apache-2.0 | crates.io |
| `memchr` | 2.8.3 | Unlicense OR MIT | crates.io |
| `memmap2` | 0.9.11 | MIT OR Apache-2.0 | crates.io |
| `mio` | 1.2.2 | MIT | crates.io |
| `mlua` | 0.12.0 | MIT | crates.io |
| `mlua-sys` | 0.11.0 | MIT | crates.io |
| `notify` | 8.2.0 | CC0-1.0 | crates.io |
| `notify-types` | 2.1.0 | MIT OR Apache-2.0 | crates.io |
| `num-traits` | 0.2.19 | MIT OR Apache-2.0 | crates.io |
| `parking_lot` | 0.12.5 | MIT OR Apache-2.0 | crates.io |
| `parking_lot_core` | 0.9.12 | MIT OR Apache-2.0 | crates.io |
| `pkg-config` | 0.3.34 | MIT OR Apache-2.0 | crates.io |
| `policy` | 0.1.0 | Apache-2.0 | git |
| `proc-macro2` | 1.0.107 | MIT OR Apache-2.0 | crates.io |
| `prop-rs` | 0.2.0 | UNKNOWN | git |
| `prop-rs-android` | 0.2.0 | UNKNOWN | git |
| `prost` | 0.13.5 | Apache-2.0 | crates.io |
| `prost-derive` | 0.13.5 | Apache-2.0 | crates.io |
| `quote` | 1.0.47 | MIT OR Apache-2.0 | crates.io |
| `rayon` | 1.12.0 | MIT OR Apache-2.0 | crates.io |
| `rayon-core` | 1.13.0 | MIT OR Apache-2.0 | crates.io |
| `regex-lite` | 0.1.9 | MIT OR Apache-2.0 | crates.io |
| `rustc-hash` | 2.1.3 | Apache-2.0 OR MIT | crates.io |
| `rustix` | 1.1.4 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | crates.io |
| `ryu` | 1.0.23 | Apache-2.0 OR BSL-1.0 | crates.io |
| `same-file` | 1.0.6 | Unlicense/MIT | crates.io |
| `scopeguard` | 1.2.0 | MIT OR Apache-2.0 | crates.io |
| `serde` | 1.0.229 | MIT OR Apache-2.0 | crates.io |
| `serde_core` | 1.0.229 | MIT OR Apache-2.0 | crates.io |
| `serde_derive` | 1.0.229 | MIT OR Apache-2.0 | crates.io |
| `serde_json` | 1.0.151 | MIT OR Apache-2.0 | crates.io |
| `sha2` | 0.11.0 | MIT OR Apache-2.0 | crates.io |
| `shlex` | 2.0.1 | MIT OR Apache-2.0 | crates.io |
| `signal-hook` | 0.4.4 | MIT OR Apache-2.0 | crates.io |
| `signal-hook-registry` | 1.4.8 | MIT OR Apache-2.0 | crates.io |
| `smallvec` | 1.15.2 | MIT OR Apache-2.0 | crates.io |
| `strsim` | 0.11.1 | MIT | crates.io |
| `syn` | 2.0.119 | MIT OR Apache-2.0 | crates.io |
| `syn` | 3.0.3 | MIT OR Apache-2.0 | crates.io |
| `typed-path` | 0.12.3 | MIT OR Apache-2.0 | crates.io |
| `typenum` | 1.20.1 | MIT OR Apache-2.0 | crates.io |
| `unicode-ident` | 1.0.24 | (MIT OR Apache-2.0) AND Unicode-3.0 | crates.io |
| `unicode-width` | 0.2.2 | MIT OR Apache-2.0 | crates.io |
| `unicode-xid` | 0.2.6 | MIT OR Apache-2.0 | crates.io |
| `utf8parse` | 0.2.2 | Apache-2.0 OR MIT | crates.io |
| `walkdir` | 2.5.0 | Unlicense/MIT | crates.io |
| `which` | 8.0.5 | MIT | crates.io |
| `zip` | 8.6.0 | MIT | crates.io |
| `zlib-rs` | 0.6.7 | Zlib | crates.io |
| `zmij` | 1.0.23 | MIT | crates.io |

## Notes

1. `java-properties` (git pin 42a4aa9) ships an MIT LICENSE file; `ap_policy` (git pin 51ff116) ships Apache-2.0.
2. `policy` (the `ap_policy` crate) declares Apache-2.0 in its manifest.
3. Git dependencies are pinned by revision in `Cargo.lock`; vendoring for release should include their LICENSE files.
4. This file is generated (script in commit message); regenerate after any dependency change.
