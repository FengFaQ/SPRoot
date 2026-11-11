#!/usr/bin/env python3
"""B. 驱动完整性检查（Driver Integrity）—— B0 体系，2026-11-11。

回答的问题：**patch 有没有踩到驱动代码？**

这是「能开机但某硬件不工作」的唯一可静态预防点。静态检查（旧体系）看不出
这个问题：镜像等长、sha 变了、report 齐全，但摄像头 probe 被盖掉就是盖掉了。

做法：
  1. 从**原厂** Image 解出内置符号名表（见 ``common.load_kallsyms``）
  2. 统计驱动相关符号（``*_probe``、``*_driver``、``*_drv``、i2c/spi/usb/mmc/
     drm/mipi/thermal/battery/charger/sensor 前缀等）
  3. 对每一处 patch 改动，判断它是否落在**驱动符号**上（归属靠引擎 report 的
     符号名 + 结构启发；裸 Image 拿不到地址表时如实降级）
  4. 断言：除引擎声明要 hook 的那几个（``avc_denied``/``audit_log_start``/
     ``filldir64``/``do_execve*`` …）之外，**没有**驱动符号被触及

用法::

    python3 quality/bootability/driver_integrity.py \
        --orig kernel.orig --patched kernel.patched \
        [--report kernel.patched.report.json] --json out/driver-integrity.json

退出码：0 = 无驱动符号被非预期触及；1 = 有；2 = 用法/输入错误。
"""

from __future__ import annotations

import argparse
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import common  # noqa: E402

# Symbols the engine is *supposed* to touch. Anything else that classifies as a
# driver symbol is a finding. Keep this list explicit and small: it is the
# allow-list that makes the check meaningful.
EXPECTED_HOOK_TARGETS = {
    "do_execveat_common",
    "do_execveat",
    "do_execve",
    "do_execve_common",
    "__do_execve_file",
    "filldir64",
    "compat_filldir",
    "avc_denied",
    "audit_log_start",
    "__arm64_sys_openat",  # --launcher rc
}

# Kernel-internal exceptions: these look driver-ish by substring but are core
# kernel code the engine legitimately patches (security/CFI/huawei bypass).
KNOWN_NON_DRIVER_HOOKS = {
    "__cfi_check",
    "__cfi_check_fail",
    "__cfi_slowpath",
    "__cfi_slowpath_diag",
    "__ubsan_handle_cfi_check_fail",
    "__ubsan_handle_cfi_check_fail_abort",
    "report_cfi_failure",
    "hkip_check_uid_root",
    "hkip_check_gid_root",
    "hkip_check_xid_root",
    "kti_randomize_init",
    "die",
    "__drm_puts_coredump",
    "__drm_printfn_coredump",
}

# Driver symbols that legitimately contain "drm_" but are *not* display drivers
# (they are coredump helpers the engine uses as code caves on >= 6.1).
DRM_COREDUMP_EXCEPTIONS = {"__drm_puts_coredump", "__drm_printfn_coredump"}


def driver_symbols(kallsyms: common.Kallsyms) -> list[common.Symbol]:
    return [s for s in kallsyms.symbols if common.is_driver_symbol(s.name)]


def audit(
    orig: bytes, patched: bytes, report: dict | None, engine_log: str | None = None
) -> dict:
    if len(orig) != len(patched):
        raise ValueError(f"length mismatch: {len(orig)} vs {len(patched)}")

    # ---- 1. symbol inventory -------------------------------------------
    try:
        ks = common.load_kallsyms(orig)
        symbols_ok = True
        symbols_error = None
    except common.KallsymsError as e:
        ks = None
        symbols_ok = False
        symbols_error = str(e)

    drv = driver_symbols(ks) if ks else []

    # ---- 2. symbol addresses -------------------------------------------
    # Preferred source: the engine's own resolution log (it must resolve
    # addresses to patch at all). Secondary: the report. Without either, the
    # offset→symbol question is unanswerable and we say so.
    addr_table: dict[str, tuple[int, int]] = {}
    addr_source = "none"
    if engine_log:
        addr_table = common.parse_engine_symbol_log(engine_log)
        if addr_table:
            addr_source = "engine-log"
    if not addr_table and report:
        for h in report.get("hooks") or []:
            if not isinstance(h, dict):
                continue
            name = h.get("name")
            off = h.get("patch_offset")
            ln = h.get("len")
            try:
                if name and off is not None:
                    addr_table[str(name)] = (int(str(off), 0), int(ln or 0))
            except (TypeError, ValueError):
                continue
        if addr_table:
            addr_source = "report"

    # ---- 3. which symbols did the engine claim to touch? ----------------
    touched: set[str] = set()
    if report:
        for h in report.get("hooks") or []:
            if isinstance(h, dict) and h.get("name"):
                touched.add(str(h["name"]))

    # ---- 4. what actually changed --------------------------------------
    runs = common.runs_of(orig, patched, max_merge_gap=8)

    # ---- 5. verdict ----------------------------------------------------
    blocking: list[str] = []
    warnings: list[str] = []
    findings: list[dict] = []

    unexpected = sorted(
        t
        for t in touched
        if t not in EXPECTED_HOOK_TARGETS and t not in KNOWN_NON_DRIVER_HOOKS
    )
    for name in unexpected:
        if common.is_driver_symbol(name) and name not in DRM_COREDUMP_EXCEPTIONS:
            blocking.append(
                f"DI-1: engine patched `{name}`, which classifies as a driver "
                f"symbol ({common.classify_symbol(name)}). A driver hook can "
                f"boot but silently kill that hardware."
            )
        else:
            warnings.append(
                f"DI-2: engine patched `{name}` — not in the expected hook list; "
                f"classified as {common.classify_symbol(name)}"
            )

    # DI-3: for every changed run, which symbol does it fall inside?
    if addr_table:
        for s, e in runs:
            hit = common.symbol_at(s, addr_table)
            if hit is None:
                findings.append(
                    {
                        "offset": f"0x{s:08x}",
                        "length": e - s,
                        "symbol": None,
                        "note": "not covered by any resolved symbol",
                    }
                )
                continue
            sym_name, sym_start, sym_size = hit
            inside = sym_size and sym_start <= s < sym_start + sym_size
            findings.append(
                {
                    "offset": f"0x{s:08x}",
                    "length": e - s,
                    "symbol": sym_name,
                    "symbol_start": f"0x{sym_start:08x}",
                    "symbol_size": sym_size,
                    "inside_symbol": bool(inside),
                    "subsystem": common.classify_symbol(sym_name),
                    "is_driver": common.is_driver_symbol(sym_name),
                }
            )
            if (
                inside
                and common.is_driver_symbol(sym_name)
                and sym_name not in EXPECTED_HOOK_TARGETS
                and sym_name not in DRM_COREDUMP_EXCEPTIONS
            ):
                blocking.append(
                    f"DI-3: change at 0x{s:08x} falls inside driver symbol "
                    f"`{sym_name}` [{sym_start:#x}, {sym_start + sym_size:#x}) — "
                    f"hardware may stop working while the device still boots"
                )
    else:
        warnings.append(
            "DI-3 skipped: no symbol→address mapping available (bare Image has "
            "names but not the address arrays; pass --engine-log for the "
            "engine's resolved addresses). Reported as a gap, NOT as a pass. "
            "To close it on device: compare against /proc/kallsyms or System.map."
        )

    return {
        "symbols": {
            "decoded": symbols_ok,
            "error": symbols_error,
            "total": len(ks) if ks else 0,
            "method": ks.method if ks else None,
            "driver_related": len(drv),
            "address_mapping_available": bool(addr_table),
            "address_source": addr_source,
            "addresses_resolved": len(addr_table),
        },
        "engine_claimed_targets": sorted(touched),
        "unexpected_targets": unexpected,
        "changed_regions": len(runs),
        "overlaps": findings,
        "blocking": blocking,
        "warnings": warnings,
        "sample_driver_symbols": [s.name for s in drv[:20]],
    }


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description="B. 驱动完整性检查 (B0)")
    ap.add_argument("--orig", required=True)
    ap.add_argument("--patched", required=True)
    ap.add_argument("--report")
    ap.add_argument(
        "--engine-log",
        help="引擎 stdout（含 `sym:ADDR, size:SIZE` 行）；用于把改动偏移映射回符号",
    )
    ap.add_argument("--json")
    ap.add_argument("--quiet", action="store_true")
    args = ap.parse_args(argv)

    try:
        orig = open(args.orig, "rb").read()
        patched = open(args.patched, "rb").read()
    except OSError as e:
        print(f"ERROR: {e}", file=sys.stderr)
        return 2

    report = None
    if args.report:
        try:
            report = json.load(open(args.report, encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as e:
            print(f"WARN: cannot read report {args.report}: {e}", file=sys.stderr)

    engine_log = None
    if args.engine_log:
        try:
            engine_log = open(args.engine_log, encoding="utf-8", errors="replace").read()
        except OSError as e:
            print(f"WARN: cannot read engine log {args.engine_log}: {e}", file=sys.stderr)

    try:
        res = audit(orig, patched, report, engine_log)
    except ValueError as e:
        print(f"ERROR: {e}", file=sys.stderr)
        return 2

    if args.json:
        os.makedirs(os.path.dirname(os.path.abspath(args.json)), exist_ok=True)
        with open(args.json, "w", encoding="utf-8") as fh:
            json.dump(res, fh, ensure_ascii=False, indent=2)

    if not args.quiet:
        s = res["symbols"]
        print("== B. 驱动完整性检查 ==")
        print(f"符号表: decoded={s['decoded']} total={s['total']} "
              f"driver_related={s['driver_related']} "
              f"address_mapping={s['address_mapping_available']}")
        if s["error"]:
            print(f"  符号解码受限: {s['error']}")
        print(f"引擎声明触碰: {', '.join(res['engine_claimed_targets']) or '(无)'}")
        print(f"非预期目标: {', '.join(res['unexpected_targets']) or '(无)'}")
        print(f"改动区段: {res['changed_regions']} 处")
        for w in res["warnings"]:
            print(f"  WARN: {w}")
        for b in res["blocking"]:
            print(f"  BLOCKING: {b}")
        print()
        if res["blocking"]:
            print(f"判定：FAIL（{len(res['blocking'])} 项驱动完整性阻断发现）")
            return 1
        print(f"判定：PASS（{len(res['warnings'])} 项 gap/警告，见上）")
    return 0 if not res["blocking"] else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
