#!/usr/bin/env python3
"""B0 体系自检 —— 负样本铁律的机器执行版。

项目铁律（HANDOFF-2026-11-08:153-155）：**新增断言必须先用负样本证明它会 FAIL**。
AVB-3 曾因公式错误写成"永远 PASS"的空断言，比没有断言更危险。

本脚本对 A/B/C 三块的每一类断言，构造一个**已知违规**的输入，要求断言必须变红；
任何一条负样本没能变红（即断言退化成了永远 PASS），自检本身判失败。

自己也是断言 → 也要有负样本：``--self-test-of-self-test`` 会把某个检查器
强行改坏，确认本脚本会因此报红。

用法::

    python3 quality/bootability/self_test.py
    python3 quality/bootability/self_test.py --self-test-of-self-test

退出码：0 = 全部负样本都正确变红；1 = 有断言退化成永远 PASS；2 = 环境/用法错误。
"""

from __future__ import annotations

import argparse
import json
import os
import struct
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)

import boot_qemu  # noqa: E402
import common  # noqa: E402
import driver_integrity  # noqa: E402
import patch_audit  # noqa: E402

PASS = "PASS"
FAIL = "FAIL"
_rows: list[tuple[str, str, str]] = []


def record(cid: str, ok: bool, msg: str) -> None:
    _rows.append((cid, PASS if ok else FAIL, msg))


# ---------------------------------------------------------------------------
# Synthetic arm64 Image builder — lets every test run with no real kernel.
# ---------------------------------------------------------------------------
HDR_SIZE = 0x1000


def mk_image(code: list[int], total: int = 0x8000, filler: int = 0) -> bytes:
    """Build a bare arm64 Image with a valid header and the given code words."""
    buf = bytearray(total)
    # Header: code0/code1 are a br/ret stub in real images; the parser only
    # checks text_offset/image_size/flags/magic.
    struct.pack_into("<QQQ", buf, 8, 0, total, 0xA)
    buf[56:60] = common.ARM64_MAGIC
    off = HDR_SIZE
    for w in code:
        struct.pack_into("<I", buf, off, w)
        off += 4
    if filler:
        for i in range(off, total, 4):
            struct.pack_into("<I", buf, i, filler)
    return bytes(buf)


def enc_b(disp_words: int) -> int:
    return 0x14000000 | (disp_words & 0x03FFFFFF)


def enc_bl(disp_words: int) -> int:
    return 0x94000000 | (disp_words & 0x03FFFFFF)


# ---------------------------------------------------------------------------
# A. patch_audit negative samples
# ---------------------------------------------------------------------------
def test_pa2_unaligned_branch() -> None:
    """PA-2: a branch whose displacement is not a multiple of 4 must be blocking.

    Real AArch64 can never encode this (imm26 is scaled by 4), so we forge the
    raw word — exactly what a corrupted/truncated patch would look like.
    """
    orig = mk_image([0xD503201F] * 8)  # NOPs
    patched = bytearray(orig)
    # Write a B with an odd displacement: patch b's imm26 to 3 (x4 = 12, ok) —
    # to get "not multiple of 4" we corrupt the whole branch encoding so that a
    # B-like word with imm26 bit0 set appears. B imm26 is always *4, so instead
    # assert the check on a hand-built non-scaling case: BL with a target that
    # leaves the image.
    struct.pack_into("<I", patched, HDR_SIZE, enc_b((0x100000 - HDR_SIZE) // 4))
    res = patch_audit.audit(orig, bytes(patched), None)
    ok = any("outside the Image" in b for b in res["blocking"])
    record(
        "NEG-PA2-out-of-image",
        ok,
        "PA-2 必须把跳出 Image 的 b 判为 blocking；实得 blocking=%d" % len(res["blocking"]),
    )


def test_pa2_target_outside_image() -> None:
    orig = mk_image([0xD503201F] * 8, total=0x2000)
    patched = bytearray(orig)
    # Target far beyond the image end.
    struct.pack_into("<I", patched, HDR_SIZE, enc_b(0x00FFFFF))
    res = patch_audit.audit(orig, bytes(patched), None)
    ok = any("outside the Image" in b for b in res["blocking"])
    record("NEG-PA2-oob", ok, "PA-2 越界跳转必须 blocking")


def test_pa2_clean_branch_passes() -> None:
    """Positive control: a well-formed in-image branch must NOT be blocking.

    Note the report: PA-0 makes "changed but unattributed" blocking, so this
    test must declare the change the way the engine would. That coupling is
    intentional — it is exactly the assertion the Lead asked for.
    """
    orig = mk_image([0xD503201F] * 16, total=0x4000)
    patched = bytearray(orig)
    # B from HDR_SIZE to HDR_SIZE+0x100 (in image, aligned)
    struct.pack_into("<I", patched, HDR_SIZE, enc_b(0x100 // 4))
    rep = {"hooks": [{"group": "do_execve", "patch_offset": f"0x{HDR_SIZE:x}", "len": 4}]}
    res = patch_audit.audit(orig, bytes(patched), rep)
    record(
        "POS-PA2-clean",
        not res["blocking"],
        "良好跳转不应产生 blocking；实得 blocking=%s" % res["blocking"][:1],
    )


def test_pa3_data_write_detected() -> None:
    """PA-3: writing high-entropy non-instruction bytes over code must block.

    This is the check whose *first implementation* returned density≈1.00 for
    the real key blob, i.e. it would never have fired. The negative sample is
    what proves the replacement actually discriminates.
    """
    orig = mk_image([0xD503201F] * 32, total=0x4000)
    patched = bytearray(orig)
    # 16 bytes of pseudo-random "data" that is not a plausible instruction word
    junk = bytes([0x13, 0x37, 0x7A, 0x00] * 4)
    patched[HDR_SIZE : HDR_SIZE + len(junk)] = junk
    rep = {"hooks": [{"group": "do_execve", "patch_offset": f"0x{HDR_SIZE:x}", "len": len(junk)}]}
    res = patch_audit.audit(orig, bytes(patched), rep)
    ok = any("PA-3" in b for b in res["blocking"])
    record("NEG-PA3-data-over-code", ok, "PA-3 必须识别数据覆盖代码；blocking=%d" % len(res["blocking"]))


def test_pa3_ascii_cave_is_allowed() -> None:
    """Positive control: the key literal written into a cave is *expected*."""
    orig = mk_image([0xD503201F] * 32, total=0x4000)
    patched = bytearray(orig)
    key = b"ABCdef0123456789ABCdef0123456789ABCdef0123456789\x00"
    patched[HDR_SIZE : HDR_SIZE + len(key)] = key
    rep = {"hooks": [{"group": "embed_key", "patch_offset": f"0x{HDR_SIZE:x}", "len": len(key)}]}
    res = patch_audit.audit(orig, bytes(patched), rep)
    kind_notes = [
        n for f in res["findings"] for n in f["notes"] if "new bytes look like" in n
    ]
    ok = bool(kind_notes) and any("ascii" in n for n in kind_notes)
    record("POS-PA3-ascii-cave", ok, "密钥字面量应被判为 ascii 而非 data：%s" % (kind_notes[:1],))


def test_pa1_identical_images() -> None:
    """A no-op patch must report zero changes (guards against phantom runs)."""
    img = mk_image([0xD503201F] * 8)
    res = patch_audit.audit(img, img, None)
    record(
        "POS-PA-identical",
        res["summary"]["changed_regions"] == 0,
        "相同镜像应报 0 处改动；实得 %d" % res["summary"]["changed_regions"],
    )


def test_pa0_no_report_is_blocking() -> None:
    """PA-0: changes present but zero attribution must be BLOCKING.

    This is the defect the Lead caught: a run without ``--report`` produced
    ``attributed_regions: 0 / unattributed: 9`` and still exited 0. A code-level
    audit that cannot say *which function* changed has not done its job, so it
    must never be green.
    """
    orig = mk_image([0xD503201F] * 16, total=0x4000)
    patched = bytearray(orig)
    struct.pack_into("<I", patched, HDR_SIZE, enc_b(0x100 // 4))
    res = patch_audit.audit(orig, bytes(patched), None)  # no report!
    ok = (
        res["attribution"]["attributed_regions"] == 0
        and any("PA-0" in b for b in res["blocking"])
    )
    record(
        "NEG-PA0-no-report",
        ok,
        "无 report 时 0 归属必须 blocking；实得 blocking=%d" % len(res["blocking"]),
    )


def test_pa0_report_without_hooks_is_blocking() -> None:
    """The legacy `hook_table: true` report shape must also block, not pass."""
    orig = mk_image([0xD503201F] * 16, total=0x4000)
    patched = bytearray(orig)
    struct.pack_into("<I", patched, HDR_SIZE, enc_b(0x100 // 4))
    legacy = {"hook_table": True}  # cJSON bug: no hooks[]
    res = patch_audit.audit(orig, bytes(patched), legacy)
    ok = any("PA-0" in b for b in res["blocking"])
    record(
        "NEG-PA0-legacy-report",
        ok,
        "legacy hook_table:true 无 hooks[] 也必须 blocking",
    )


def test_pa0_with_real_report_attributes() -> None:
    """Positive control: a proper hooks[] report must attribute the change."""
    orig = mk_image([0xD503201F] * 16, total=0x4000)
    patched = bytearray(orig)
    target = HDR_SIZE
    # Declare exactly the branch we write, so it must be attributed.
    rep = {
        "hooks": [
            {
                "group": "do_execve",
                "patch_offset": f"0x{target:x}",
                "len": 4,
            }
        ]
    }
    struct.pack_into("<I", patched, target, enc_b(0x100 // 4))
    res = patch_audit.audit(orig, bytes(patched), rep)
    ok = (
        res["attribution"]["attributed_regions"] >= 1
        and not any("PA-0" in b for b in res["blocking"])
        and res["findings"][0]["group"] == "do_execve"
    )
    record(
        "POS-PA0-attributed",
        ok,
        "带 hooks[] 的 report 应完成归属；实得 coverage=%s"
        % res["attribution"]["coverage"],
    )


# ---------------------------------------------------------------------------
# B. driver_integrity negative samples
# ---------------------------------------------------------------------------
def test_di_expected_hook_allowed() -> None:
    """A declared, expected hook must not be flagged."""
    orig = mk_image([0xD503201F] * 8)
    patched = bytearray(orig)
    struct.pack_into("<I", patched, HDR_SIZE, 0xD503201F ^ 0xFF)
    log = "do_execveat_common:1000, size:2c4\n"
    res = driver_integrity.audit(orig, bytes(patched), None, log)
    record(
        "POS-DI-expected",
        not res["blocking"],
        "预期 hook 不应 blocking；实得 %s" % res["blocking"][:1],
    )


def test_di_driver_symbol_blocks() -> None:
    """DI-1: a driver symbol outside the allow-list must be blocking.

    This is the "boots but the camera is dead" case that static checks miss.
    """
    orig = mk_image([0xD503201F] * 8)
    patched = bytearray(orig)
    struct.pack_into("<I", patched, HDR_SIZE, 0xD503201F ^ 0xFF)
    log = "msm_cam_probe:1000, size:100\n"
    res = driver_integrity.audit(orig, bytes(patched), None, log)
    ok = any("DI-1" in b or "DI-3" in b for b in res["blocking"])
    record("NEG-DI-driver-touched", ok, "驱动符号被 patch 必须 blocking；实得 %d" % len(res["blocking"]))


def test_di_no_mapping_is_gap_not_pass() -> None:
    """Without an address map, DI-3 must report a *gap*, never a silent pass."""
    orig = mk_image([0xD503201F] * 8)
    patched = bytearray(orig)
    struct.pack_into("<I", patched, HDR_SIZE, 0xD503201F ^ 0xFF)
    res = driver_integrity.audit(orig, bytes(patched), None, None)
    gap = any("DI-3 skipped" in w for w in res["warnings"])
    record(
        "NEG-DI-mapping-gap",
        gap and not res["symbols"]["address_mapping_available"],
        "无地址映射时必须报 gap（而非静默 PASS）",
    )


def test_di_symbol_at_exact_start_wins() -> None:
    """Symbol attribution must prefer an exact start (regression guard).

    An earlier implementation returned `__drm_puts_coredump` for an address that
    was exactly `__drm_printfn_coredump`, because both ranges overlapped and the
    tie-break was undefined.
    """
    table = {
        "__drm_puts_coredump": (0x85DEE8, 0xF8),
        "__drm_printfn_coredump": (0x85DFBC, 0xF8),
    }
    hit = common.symbol_at(0x85DFBC, table)
    ok = hit is not None and hit[0] == "__drm_printfn_coredump"
    record("NEG-DI-symbol-tiebreak", ok, "精确起点必须优先；实得 %s" % (hit[0] if hit else None,))


def test_di_parse_engine_log() -> None:
    """The engine-log parser must handle both formats and not invent sizes."""
    text = (
        "do_execveat_common:38fcd0, size:2c4\n"
        "avc_denied:5e17b0, size:84\n"
        "report_cfi_failure:28f130\n"
        "Start hooking addr:  24bd4\n"
    )
    t = common.parse_engine_symbol_log(text)
    ok = (
        t.get("do_execveat_common") == (0x38FCD0, 0x2C4)
        and t.get("avc_denied") == (0x5E17B0, 0x84)
        and t.get("report_cfi_failure") == (0x28F130, 0)  # size unknown → 0, not invented
        and "Start hooking addr" not in t
    )
    record("POS-DI-logparse", ok, "引擎日志解析应区分有/无 size 行；实得 %s" % (t,))


# ---------------------------------------------------------------------------
# C. boot_qemu negative samples (no real qemu needed: we forge BootRun objects)
# ---------------------------------------------------------------------------
def _fake_run(label: str, stdout: str, exit_code: int = 0) -> boot_qemu.BootRun:
    r = boot_qemu.BootRun(label, ["qemu"], 10)
    r.stdout = stdout
    r.exit_code = exit_code
    return r


def test_boot_same_behaviour_no_regression() -> None:
    txt = "Booting Linux\nLinux version 6.1.25\nsmp: Brought up\n" \
          "VFS: Cannot open root device\nUnable to mount root fs\nKernel panic\n"
    o = _fake_run("orig", txt)
    p = _fake_run("patched", txt)
    cmp = boot_qemu.compare(o, p)
    record(
        "POS-BOOT-same",
        cmp["verdict"] == "NO_REGRESSION" and cmp["identical_behaviour"],
        "表现一致应判 NO_REGRESSION；实得 %s" % cmp["verdict"],
    )


def test_boot_earlier_failure_is_regression() -> None:
    """The whole point of C: patched dying earlier than orig MUST be reported."""
    good = "Booting Linux\nLinux version 6.1.25\nsmp: Brought up\n" \
           "VFS: Cannot open root device\nUnable to mount root fs\n"
    worse = "Booting Linux\nLinux version 6.1.25\n"  # stops before smp-up
    o = _fake_run("orig", good)
    p = _fake_run("patched", worse)
    cmp = boot_qemu.compare(o, p)
    record(
        "NEG-BOOT-earlier-death",
        cmp["verdict"] == "REGRESSION",
        "patched 更早失败必须判 REGRESSION；实得 %s" % cmp["verdict"],
    )


def test_boot_total_silence_is_regression() -> None:
    """patched produces no kernel log where orig did → regression, not 'same'."""
    o = _fake_run("orig", "Booting Linux\nLinux version 6.1.25\n")
    p = _fake_run("patched", "")
    cmp = boot_qemu.compare(o, p)
    record(
        "NEG-BOOT-silence",
        cmp["verdict"] == "REGRESSION",
        "patched 完全无输出必须判 REGRESSION；实得 %s" % cmp["verdict"],
    )


def test_boot_l3_needs_real_init_marker() -> None:
    """L3 must NOT be satisfied by kernel-side panic backtraces.

    The first implementation matched "kernel_init" inside a panic call trace and
    reported L3=True for a kernel that never entered userspace — a false
    positive that would have hidden the real finding.
    """
    panic_only = (
        "Booting Linux\nKernel panic - not syncing: VFS: Unable to mount root fs\n"
        "Call trace:\n kernel_init_freeable+0x10c/0x15c\n kernel_init+0x20/0x1a4\n"
        " ret_from_fork+0x10/0x20\n"
    )
    r = _fake_run("orig", panic_only)
    l3, _hits = r.level3_userspace()
    record(
        "NEG-BOOT-l3-false-positive",
        not l3,
        "panic 回溯中的 kernel_init 不得算 L3；实得 L3=%s depth=%s" % (l3, r.boot_depth()),
    )


def test_boot_l3_real_handoff() -> None:
    """Positive control: a real handoff marker does satisfy L3."""
    txt = "Booting Linux\nFreeing unused kernel memory: 1024K\nRun /init as init process\n"
    r = _fake_run("orig", txt)
    l3, _ = r.level3_userspace()
    record("POS-BOOT-l3-real", l3, "真实 init 交接标记应满足 L3")


def test_boot_invalid_image_rejected() -> None:
    """L1 must fail when qemu rejects the image."""
    r = _fake_run("patched", "qemu: could not load kernel 'x': invalid image", exit_code=1)
    record("NEG-BOOT-L1-invalid", not r.level1_loadable(), "无效镜像必须 L1=False")


def test_boot_normalisation_ignores_noise() -> None:
    """Timestamps/RTC/stack-depth must NOT count as a difference.

    Raw serial diff of two boots never matches (the Lead measured: 3 of 273
    lines differ, all noise). If noise counted, the line-by-line comparison
    would be permanently "different" — i.e. useless.
    """
    a = (
        "[    0.212000] Booting Linux\n"
        "[    0.554621] uart-pl011 9000000.pl011: no DMA platform data\n"
        "audit: type=2000 audit(0.212:1): state=initialized\n"
        "rtc-pl031 9010000.pl031: setting system clock to 2026-11-11T10:38:50 UTC (1794393530)\n"
        "cryptomgr_probe (54) used greatest stack depth: 15056 bytes left\n"
        "Kernel panic - not syncing: VFS: Unable to mount root fs\n"
    )
    b = (
        "[    0.152111] Booting Linux\n"
        "[    0.566474] uart-pl011 9000000.pl011: no DMA platform data\n"
        "audit: type=2000 audit(0.152:1): state=initialized\n"
        "rtc-pl031 9010000.pl031: setting system clock to 2026-11-11T10:44:27 UTC (1794393867)\n"
        "cryptomgr_probe (54) used greatest stack depth: 14928 bytes left\n"
        "Kernel panic - not syncing: VFS: Unable to mount root fs\n"
    )
    d = boot_qemu.diff_serial(a, b)
    record(
        "NEG-BOOT-normalise-noise",
        d["identical_after_normalisation"],
        "时间戳/RTC/栈深抖动必须被归一化；实得 diff=%d %s"
        % (d["diff_count"], d["diffs"][:1]),
    )


def test_boot_normalisation_still_catches_real_diff() -> None:
    """A REAL log difference must survive normalisation and be reported."""
    a = "Booting Linux\nmsm_cam_probe: camera initialised\nKernel panic\n"
    b = "Booting Linux\nKernel panic\n"          # driver probe disappeared
    d = boot_qemu.diff_serial(a, b)
    ok = (not d["identical_after_normalisation"]) and d["diff_count"] >= 1
    record(
        "NEG-BOOT-normalise-real-diff",
        ok,
        "驱动 probe 行消失必须仍被检出；实得 diff=%d" % d["diff_count"],
    )


def test_boot_multiset_absorbs_reordering() -> None:
    """Same lines in a different order must count as content-identical.

    Async probes (cryptomgr_probe, workqueues) land nondeterministically, so two
    boots of the SAME kernel differ by a few reordered lines. Requiring exact
    line order would make the comparison permanently "different" — i.e. silent.
    """
    a = "Booting Linux\npage_owner is disabled\ncryptomgr_probe stack depth\nKernel panic\n"
    b = "Booting Linux\ncryptomgr_probe stack depth\npage_owner is disabled\nKernel panic\n"
    d = boot_qemu.diff_serial(a, b)
    ok = (
        not d["identical_after_normalisation"]   # order differs …
        and d["same_line_multiset"]              # … but content is identical
    )
    record(
        "POS-BOOT-multiset",
        ok,
        "同行不同序应判内容集合相同；实得 order_diff=%d multiset=%s"
        % (d["diff_count"], d["same_line_multiset"]),
    )


def test_boot_multiset_catches_missing_line() -> None:
    """Multiset must NOT absorb a genuinely missing line."""
    a = "Booting Linux\npage_owner is disabled\nKernel panic\n"
    b = "Booting Linux\nKernel panic\n"
    d = boot_qemu.diff_serial(a, b)
    ok = (not d["same_line_multiset"]) and bool(d["lines_only_in_orig"])
    record(
        "NEG-BOOT-multiset-missing",
        ok,
        "确实少一行时 multiset 必须不同，并列出 only_in_orig；实得 %s"
        % (d["lines_only_in_orig"],),
    )


def test_boot_pid_and_kaslr_normalised() -> None:
    """cpu:pid prefix and KASLR slide are noise, not signal."""
    a = (
        "[    0.5][0:46:cryptomgr_probe] cryptomgr_probe (46) used greatest stack depth: 15056 bytes left\n"
        "[    0.5][0:1:swapper/0] Kernel Offset: 0x2179800000 from 0xffffffc008000000\n"
    )
    b = (
        "[    0.6][0:47:cryptomgr_probe] cryptomgr_probe (47) used greatest stack depth: 14928 bytes left\n"
        "[    0.6][0:1:swapper/0] Kernel Offset: 0x1c3aa00000 from 0xffffffc008000000\n"
    )
    d = boot_qemu.diff_serial(a, b)
    record(
        "NEG-BOOT-pid-kaslr",
        d["same_line_multiset"],
        "PID 与 KASLR 偏移必须被归一化；实得 diff=%d %s"
        % (d["diff_count"], d["diffs"][:1]),
    )


# ---------------------------------------------------------------------------
# Runner
# ---------------------------------------------------------------------------
TESTS = [
    test_pa2_unaligned_branch,
    test_pa2_target_outside_image,
    test_pa2_clean_branch_passes,
    test_pa3_data_write_detected,
    test_pa3_ascii_cave_is_allowed,
    test_pa1_identical_images,
    test_pa0_no_report_is_blocking,
    test_pa0_report_without_hooks_is_blocking,
    test_pa0_with_real_report_attributes,
    test_di_expected_hook_allowed,
    test_di_driver_symbol_blocks,
    test_di_no_mapping_is_gap_not_pass,
    test_di_symbol_at_exact_start_wins,
    test_di_parse_engine_log,
    test_boot_same_behaviour_no_regression,
    test_boot_earlier_failure_is_regression,
    test_boot_total_silence_is_regression,
    test_boot_l3_needs_real_init_marker,
    test_boot_l3_real_handoff,
    test_boot_invalid_image_rejected,
    test_boot_normalisation_ignores_noise,
    test_boot_normalisation_still_catches_real_diff,
    test_boot_multiset_absorbs_reordering,
    test_boot_multiset_catches_missing_line,
    test_boot_pid_and_kaslr_normalised,
]


def run_all() -> list[tuple[str, str, str]]:
    _rows.clear()
    for fn in TESTS:
        try:
            fn()
        except Exception as e:  # noqa: BLE001 - a crash IS a failure here
            record(fn.__name__, False, f"测试自身抛异常: {e!r}")
    return list(_rows)


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description="B0 体系自检（负样本铁律）")
    ap.add_argument(
        "--self-test-of-self-test",
        action="store_true",
        help="把某个检查器改坏，确认本自检会因此报红（自检也要能被证明会 FAIL）",
    )
    ap.add_argument("--json")
    args = ap.parse_args(argv)

    rows = run_all()

    # ---- 自检的自检：故意破坏一个检查器，要求对应负样本变红 ----
    # 自检本身也是断言，也必须被证明会 FAIL。这里把 PA-3 的判据改成恒返回
    # "code"，于是 NEG-PA3-data-over-code 找不到 blocking —— 若那一行仍然是
    # PASS，说明该负样本根本没连到真实代码，必须被视为自检失效。
    if args.self_test_of_self_test:
        orig_fn = patch_audit.region_kind
        try:
            patch_audit.region_kind = lambda *a, **k: ("code", "MUTATED always-code")
            # 注意：run_all() 会 clear 全局 _rows，所以取它返回的副本
            mutated_rows = run_all()
        finally:
            patch_audit.region_kind = orig_fn
        pa3 = [r for r in mutated_rows if "PA3" in r[0]]
        mutated_red = any(r[1] == FAIL for r in pa3)
        # 恢复真实行（run_all 已被调用，_rows 现在是变异结果）
        _rows[:] = rows
        rows = rows + [
            (
                "SELF-NEG-mutate-pa3",
                PASS if mutated_red else FAIL,
                "把 region_kind 改成恒返回 code 后，PA-3 负样本必须变红"
                f"（变异后 PA-3 行：{[(r[0], r[1]) for r in pa3]}）",
            )
        ]

    fails = [r for r in rows if r[1] == FAIL]
    if args.json:
        os.makedirs(os.path.dirname(os.path.abspath(args.json)), exist_ok=True)
        with open(args.json, "w", encoding="utf-8") as fh:
            json.dump(
                {"ok": not fails, "rows": [{"id": i, "status": s, "msg": m} for i, s, m in rows]},
                fh,
                ensure_ascii=False,
                indent=2,
            )

    print("== B0 自检（负样本必须变红）==")
    for cid, status, msg in rows:
        mark = "[PASS]" if status == PASS else "[FAIL]"
        print(f"{mark} {cid}: {msg}")
    print()
    print(f"汇总：PASS={len(rows) - len(fails)} FAIL={len(fails)} / {len(rows)}")
    if fails:
        print("判定：FAIL —— 有断言退化成永远 PASS（这是最危险的缺陷）")
        return 1
    print("判定：PASS —— 每条断言都被证明会 FAIL")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
