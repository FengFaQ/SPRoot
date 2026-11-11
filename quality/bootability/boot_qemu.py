#!/usr/bin/env python3
"""C. 开机行为验证（Bootability, qemu）—— B0 体系，2026-11-11。

回答的问题：**修补后的内核还能不能跑起来？**

分层做，能到哪层报哪层，绝不把"跑不到"说成"通过"：

  L1 可加载性     qemu 能否把 Image 作为 -kernel 装载（不报 invalid image）
  L2 早期启动     串口（-serial）是否出现内核日志 / 启动横幅
  L3 init/用户态  是否走到用户态 init（本样本 RAMDISK_SZ=0，预期到不了）

**判据只有相对差异**：原厂 kernel 与 patched kernel 在完全相同的 qemu 参数下
对比。两者表现一致 = patch 未破坏启动路径（有力证据）；patched 更早失败 = 回归。
单看 patched 失败**不能**判定镜像有问题 —— Android 内核在 qemu 上启动是
非常规用法（缺 dtb / 缺 HAL / 缺 vendor 分区），失败是常态。

用法::

    python3 quality/bootability/boot_qemu.py \
        --orig kernel.orig --patched kernel.patched \
        --record-dir quality/bootability/records

退出码：0 = 无回归（两者一致或 patched 不更差）；1 = patched 出现回归；
        3 = qemu 不可用（如实报不可行，不是失败）。
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import time

# Boot markers, ordered by how far into boot they appear. L2 passes if ANY
# early marker shows up; L3 only on a userspace handoff marker.
L2_MARKERS = (
    "Booting Linux",
    "Linux version",
    "bootconsole",
    "earlycon",
    "Kernel command line",
    "Memory:",
    "CPU:",
    "smp: Brought up",
)
# L3 = userspace actually started. Deliberately strict: a first version matched
# "kernel_init" in the panic backtrace and reported L3=True for a kernel that
# never left kernel space — a false positive that would have hidden the real
# finding. These markers only appear AFTER the root filesystem is mounted and
# the kernel hands off to /init (PID 1 in user mode).
L3_MARKERS = (
    "Run /init as init process",
    "Run /sbin/init as init process",
    "Freeing unused kernel memory",
    "systemd[1]",
    "Starting init:",
)
# Explicit kernel-side failure markers: if these appear, we know *why* boot
# stopped, and it is a kernel-space reason (missing rootfs), not a userspace one.
KERNEL_FAILURE_MARKERS = (
    "Kernel panic",
    "VFS: Cannot open root device",
    "Unable to mount root fs",
    "not syncing",
)


class BootRun:
    """One qemu invocation and what it produced."""

    def __init__(self, label: str, cmd: list[str], timeout: int):
        self.label = label
        self.cmd = cmd
        self.timeout = timeout
        self.stdout = ""
        self.stderr = ""
        self.exit_code: int | None = None
        self.timed_out = False
        self.seconds = 0.0
        self.error: str | None = None

    # ---- level assessment ------------------------------------------------
    def _haystack(self) -> str:
        return (self.stdout or "") + "\n" + (self.stderr or "")

    def level1_loadable(self) -> bool:
        """qemu accepted the image (did not reject it as invalid)."""
        if self.error:
            return False
        hay = self._haystack().lower()
        # qemu's explicit rejections for a bad -kernel image:
        if "invalid image" in hay or "cannot load" in hay or "not a valid" in hay:
            return False
        # An immediate segfault/triple-fault with no output at all is a load
        # failure in practice.
        if self.exit_code not in (None, 0) and not hay.strip():
            return False
        return True

    def level2_early(self) -> tuple[bool, list[str]]:
        hay = self._haystack()
        hits = [m for m in L2_MARKERS if m in hay]
        return bool(hits), hits

    def level3_userspace(self) -> tuple[bool, list[str]]:
        hay = self._haystack()
        hits = [m for m in L3_MARKERS if m in hay]
        return bool(hits), hits

    def kernel_failures(self) -> list[str]:
        """Kernel-space failure markers (missing rootfs, panic …)."""
        hay = self._haystack()
        return [m for m in KERNEL_FAILURE_MARKERS if m in hay]

    def boot_depth(self) -> str:
        """Furthest *kernel-side* milestone reached, for same-vs-same comparison.

        Distinguishing depth matters: both images might reach L2 but one dies at
        `smp: Brought up` while the other gets all the way to the rootfs panic.
        "same L2" would then be misleadingly reassuring.
        """
        hay = self._haystack()
        if self.level3_userspace()[0]:
            return "userspace"
        if "Unable to mount root fs" in hay or "VFS: Cannot open root device" in hay:
            return "rootfs-mount-attempted"
        if "Kernel panic" in hay:
            return "panic"
        if "smp: Brought up" in hay or "smp: Booting" in hay:
            return "smp-up"
        if "Freeing unused kernel memory" in hay:
            return "kernel-init"
        if self.level2_early()[0]:
            return "early-printk"
        return "none"

    def as_dict(self) -> dict:
        l2, l2hits = self.level2_early()
        l3, l3hits = self.level3_userspace()
        return {
            "label": self.label,
            "cmd": self.cmd,
            "timeout_seconds": self.timeout,
            "exit_code": self.exit_code,
            "timed_out": self.timed_out,
            "seconds": round(self.seconds, 2),
            "error": self.error,
            "levels": {
                "L1_loadable": self.level1_loadable(),
                "L2_early_boot": l2,
                "L3_userspace": l3,
            },
            "boot_depth": self.boot_depth(),
            "kernel_failures": self.kernel_failures(),
            "L2_markers": l2hits,
            "L3_markers": l3hits,
            "stdout_tail": self.stdout[-4000:],
            "stderr_tail": self.stderr[-2000:],
        }


def normalise_serial(text: str) -> list[str]:
    """Strip run-to-run noise so two boots can actually be compared line by line.

    A raw ``diff`` of two qemu boots never matches: timestamps, the RTC wall
    clock and stack-depth figures all differ between runs (verified by the Lead:
    only 3 of 273 lines legitimately differ, and all 3 are noise). Comparing the
    raw logs would therefore always report "different" — which is the same as
    not comparing at all.

    Normalised away:
      * ``[   0.554621]`` / ``[    0.5]``    kernel monotonic timestamps
      * ``[0:46:cryptomgr_probe]``          ``[cpu:pid:comm]`` prefix — the PID
                                            differs between runs by construction
      * ``audit(0.212:1)``                   audit record clock
      * ``setting system clock to <ISO>``    RTC wall clock
      * ``used greatest stack depth: 15056 bytes left``   stack-depth sampling
      * ``Kernel Offset: 0x2179800000``      KASLR slide (random per boot)
      * bare ISO-8601 timestamps anywhere
    Line count and ordering are preserved, so a structural difference (a driver
    that no longer probes) still shows up.
    """
    out: list[str] = []
    for line in text.splitlines():
        s = line
        s = re.sub(r"^\[\s*\d+\.\d+\]", "[T]", s)                  # [    0.554621]
        s = re.sub(r"\[T\]\[\d+:\d+:[^\]]*\]", "[T][cpu:pid:comm]", s)  # [0:46:x]
        s = re.sub(r"audit\(\d+\.\d+:\d+\)", "audit(T)", s)        # audit(0.212:1)
        s = re.sub(
            r"setting system clock to [0-9T:\-]+ UTC \(\d+\)",
            "setting system clock to <ISO> UTC (<epoch>)",
            s,
        )
        s = re.sub(                                                # stack depth
            r"used greatest stack depth: \d+ bytes left",
            "used greatest stack depth: <N> bytes left",
            s,
        )
        # "<comm> (<pid>) …" — the PID in parentheses also varies per boot.
        s = re.sub(r"^(\[T\]\[cpu:pid:comm\]\s*\w+) \(\d+\)", r"\1 (<pid>)", s)
        s = re.sub(r"Kernel Offset: 0x[0-9a-f]+", "Kernel Offset: <KASLR>", s)
        s = re.sub(r"\b\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}\b", "<ISO>", s)
        out.append(s.rstrip())
    return out


def diff_serial(orig: str, patched: str) -> dict:
    """Compare two serial logs after normalisation, in two independent ways.

    **Order-sensitive** (``identical_after_normalisation``): line i vs line i.
    Async work (crypto self-tests, ``cryptomgr_probe``, workqueue probes) lands
    at nondeterministic points, so two boots of the *same* kernel can differ by
    a few reordered lines. Order therefore cannot be the sole criterion.

    **Multiset** (``same_line_multiset``): every normalised line is counted, so
    "the same 290 lines came out, three of them in a different order" is
    recognised as content-identical. This is the *strong* claim: the kernel
    emitted exactly the same messages, i.e. no code path changed.

    And to make sure the multiset check cannot hide a real loss, we still report
    the order-sensitive diffs explicitly, plus any line counts that differ.
    """
    a = normalise_serial(orig)
    b = normalise_serial(patched)

    diffs: list[dict] = []
    for i in range(max(len(a), len(b))):
        la = a[i] if i < len(a) else "<missing>"
        lb = b[i] if i < len(b) else "<missing>"
        if la != lb:
            diffs.append({"line": i + 1, "orig": la[:200], "patched": lb[:200]})

    from collections import Counter

    ca, cb = Counter(a), Counter(b)
    only_orig = [ln for ln in (ca - cb)]
    only_patched = [ln for ln in (cb - ca)]

    return {
        "orig_lines": len(a),
        "patched_lines": len(b),
        "identical_after_normalisation": not diffs and len(a) == len(b),
        "same_line_multiset": not only_orig and not only_patched and len(a) == len(b),
        "lines_only_in_orig": [ln[:200] for ln in only_orig[:20]],
        "lines_only_in_patched": [ln[:200] for ln in only_patched[:20]],
        "diff_count": len(diffs),
        "diffs": diffs[:40],
    }


def find_qemu() -> str | None:
    for name in ("qemu-system-aarch64",):
        p = shutil.which(name)
        if p:
            return p
    return None


def run_qemu(qemu: str, image: str, label: str, timeout: int, extra: list[str]) -> BootRun:
    cmd = [
        qemu,
        "-M", "virt",
        "-cpu", "cortex-a57",
        "-m", "2048",
        "-kernel", image,
        "-nographic",
        "-serial", "mon:stdio",
        "-no-reboot",
        "-append", "console=ttyAMA0 earlycon=pl011,0x9000000 ignore_loglevel loglevel=7 panic=1",
    ] + extra
    run = BootRun(label, cmd, timeout)
    t0 = time.time()
    try:
        proc = subprocess.run(
            cmd,
            capture_output=True,
            timeout=timeout,
            text=True,
            errors="replace",
        )
        run.stdout = proc.stdout or ""
        run.stderr = proc.stderr or ""
        run.exit_code = proc.returncode
    except subprocess.TimeoutExpired as e:
        run.timed_out = True
        run.stdout = (e.stdout or b"").decode("utf-8", "replace") if isinstance(e.stdout, bytes) else (e.stdout or "")
        run.stderr = (e.stderr or b"").decode("utf-8", "replace") if isinstance(e.stderr, bytes) else (e.stderr or "")
    except OSError as e:
        run.error = str(e)
    run.seconds = time.time() - t0
    return run


def compare(orig: BootRun, patched: BootRun) -> dict:
    """The only meaningful judgement: did patched get *worse* than orig?

    Depth ordering matters as much as the boolean levels: both images may report
    L2 while one stops at `smp: Brought up` and the other reaches the rootfs
    panic. Treating those as "same" would be reassuring and wrong, so the boot
    depth is compared too.
    """
    o1, p1 = orig.level1_loadable(), patched.level1_loadable()
    o2, _ = orig.level2_early()
    p2, _ = patched.level2_early()
    o3, _ = orig.level3_userspace()
    p3, _ = patched.level3_userspace()
    o_depth, p_depth = orig.boot_depth(), patched.boot_depth()

    DEPTH_ORDER = [
        "none",
        "early-printk",
        "smp-up",
        "kernel-init",
        "panic",
        "rootfs-mount-attempted",
        "userspace",
    ]

    regression: list[str] = []
    notes: list[str] = []

    if o1 and not p1:
        regression.append("L1: 原厂可加载，patched 被 qemu 拒绝（镜像结构可能被破坏）")
    if o2 and not p2:
        regression.append("L2: 原厂有早期启动日志，patched 没有（启动路径被破坏）")
    if o3 and not p3:
        regression.append("L3: 原厂能到用户态，patched 到不了")
    if (
        not regression
        and o_depth in DEPTH_ORDER
        and p_depth in DEPTH_ORDER
        and DEPTH_ORDER.index(p_depth) < DEPTH_ORDER.index(o_depth)
    ):
        regression.append(
            f"启动深度：原厂到 `{o_depth}`，patched 只到 `{p_depth}` "
            f"（更早停止 = 回归，即使两者的 L2/L3 布尔值相同）"
        )

    if not regression and o_depth == p_depth:
        notes.append(
            f"原厂与 patched 在相同 qemu 参数下到达**完全相同**的启动深度 "
            f"`{o_depth}` —— 这是 patch 未破坏启动路径的有力证据"
            f"（但不等价于真机可开机）"
        )

    # The strongest available evidence: after removing run-to-run noise
    # (timestamps/cpu:pid/RTC/stack depth/KASLR), the serial logs match.
    serial = diff_serial(orig.stdout, patched.stdout)
    if serial["identical_after_normalisation"] and serial["orig_lines"] > 5:
        notes.append(
            f"串口日志归一化后**逐行完全相同**（{serial['orig_lines']} 行，"
            f"已剔除时间戳/PID/RTC/栈深/KASLR 抖动）—— 比「都是 L2」更强的证据："
            f"patch 连一条内核日志都没有改变"
        )
    elif serial["same_line_multiset"] and serial["orig_lines"] > 5:
        notes.append(
            f"串口日志归一化后**内容集合完全相同**（{serial['orig_lines']} 行，"
            f"{serial['diff_count']} 行仅顺序不同，属异步 probe 时序抖动）—— "
            f"内核发出的消息一条不多一条不少，比「都是 L2」更强"
        )
    elif serial["diff_count"] and not regression:
        extra = ""
        if serial["lines_only_in_orig"]:
            extra += f"; 仅原厂有 {len(serial['lines_only_in_orig'])} 条"
        if serial["lines_only_in_patched"]:
            extra += f"; 仅 patched 有 {len(serial['lines_only_in_patched'])} 条"
        notes.append(
            f"串口日志归一化后有 {serial['diff_count']} 行不同{extra}"
            f"（见 json 的 serial_diff）—— 不必然是回归，但需人工过目"
        )

    if not o2 and not p2:
        notes.append(
            "两者都停在内核早期（无 L2 日志）：Android GKI 内核缺少 devicetree/HAL，"
            "在 qemu 上起不来属预期，**不构成镜像问题**"
        )
    if not o3 and not p3:
        if "rootfs-mount-attempted" in (o_depth, p_depth):
            notes.append(
                "两者都走到 rootfs 挂载并 panic：内核**完整跑完了内核态初始化**，"
                "只是没有根文件系统。本样本 boot.img 的 RAMDISK_SZ=0（无 ramdisk），"
                "这是样本属性而非 patch 问题 —— 反过来说明 patch 没有破坏启动路径"
            )
        else:
            notes.append(
                "两者都到不了 L3（用户态）：本样本无 ramdisk，Android 内核没有根文件系统"
                "就无法进入用户态 —— 这本身是重要结论，不是本验证的失败"
            )

    return {
        "orig_levels": {"L1": o1, "L2": o2, "L3": o3},
        "patched_levels": {"L1": p1, "L2": p2, "L3": p3},
        "orig_boot_depth": o_depth,
        "patched_boot_depth": p_depth,
        "identical_behaviour": o1 == p1 and o2 == p2 and o3 == p3 and o_depth == p_depth,
        "serial_diff": serial,
        "regression": regression,
        "notes": notes,
        "verdict": "REGRESSION" if regression else "NO_REGRESSION",
    }


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description="C. 开机行为验证 (B0, qemu)")
    ap.add_argument("--orig", required=True)
    ap.add_argument("--patched", required=True)
    ap.add_argument("--record-dir", default="quality/bootability/records")
    ap.add_argument("--timeout", type=int, default=25, help="每次 qemu 运行秒数")
    ap.add_argument("--json")
    ap.add_argument("--quiet", action="store_true")
    args = ap.parse_args(argv)

    qemu = find_qemu()
    if not qemu:
        msg = (
            "qemu-system-aarch64 不可用 —— L1~L3 **无法进行**。\n"
            "这不是'跳过'而是'不可行'：没有模拟器就无法验证内核能否装载/启动。\n"
            "真机验证应怎么做：\n"
            "  1) fastboot flash boot <patched boot.img> 后观察是否进入系统；\n"
            "  2) `adb shell dmesg | head -100` 看内核是否正常启动；\n"
            "  3) 逐个验证驱动：摄像头/ wifi/ 触摸/ 蓝牙 是否仍工作；\n"
            "  4) 失败时 `adb shell ls /sys/class/*/` 与 `logcat -b all` 对照原厂镜像。"
        )
        print(msg, file=sys.stderr)
        res = {"qemu_available": False, "reason": msg, "levels": "L1~L3 infeasible"}
        if args.json:
            os.makedirs(os.path.dirname(os.path.abspath(args.json)), exist_ok=True)
            json.dump(res, open(args.json, "w", encoding="utf-8"), ensure_ascii=False, indent=2)
        return 3

    if not args.quiet:
        print(f"qemu: {qemu}")
        print(f"逐层对照运行（每次 {args.timeout}s）…")

    orig_run = run_qemu(qemu, args.orig, "orig", args.timeout, [])
    patched_run = run_qemu(qemu, args.patched, "patched", args.timeout, [])
    cmp = compare(orig_run, patched_run)

    os.makedirs(args.record_dir, exist_ok=True)
    stamp = time.strftime("%Y%m%d-%H%M%S")
    for run in (orig_run, patched_run):
        rec = os.path.join(args.record_dir, f"{stamp}-{run.label}.json")
        with open(rec, "w", encoding="utf-8") as fh:
            json.dump(run.as_dict(), fh, ensure_ascii=False, indent=2)
        txt = os.path.join(args.record_dir, f"{stamp}-{run.label}.log")
        with open(txt, "w", encoding="utf-8", errors="replace") as fh:
            fh.write("CMD: " + " ".join(run.cmd) + "\n")
            fh.write(f"EXIT: {run.exit_code}  TIMED_OUT: {run.timed_out}\n")
            fh.write("=== STDOUT ===\n" + (run.stdout or "") + "\n")
            fh.write("=== STDERR ===\n" + (run.stderr or "") + "\n")

    result = {
        "qemu_available": True,
        "qemu": qemu,
        "orig": orig_run.as_dict(),
        "patched": patched_run.as_dict(),
        "comparison": cmp,
    }
    if args.json:
        os.makedirs(os.path.dirname(os.path.abspath(args.json)), exist_ok=True)
        with open(args.json, "w", encoding="utf-8") as fh:
            json.dump(result, fh, ensure_ascii=False, indent=2)

    if not args.quiet:
        print()
        print("== C. 开机行为验证（原厂 vs patched）==")
        for run in (orig_run, patched_run):
            d = run.as_dict()
            lv = d["levels"]
            print(
                f"  {run.label:<8} L1={lv['L1_loadable']!s:<5} L2={lv['L2_early_boot']!s:<5} "
                f"L3={lv['L3_userspace']!s:<5} exit={d['exit_code']} "
                f"timeout={d['timed_out']} {d['seconds']}s"
            )
            if d["L2_markers"]:
                print(f"           L2 命中: {d['L2_markers'][:4]}")
        print()
        sd = cmp.get("serial_diff") or {}
        if sd:
            print(
                f"  串口: 归一化后 {sd['orig_lines']} vs {sd['patched_lines']} 行, "
                f"顺序不同={sd['diff_count']}, 逐行相同={sd['identical_after_normalisation']}, "
                f"内容集合相同={sd['same_line_multiset']}"
            )
            for d in (sd.get("diffs") or [])[:5]:
                print(f"    L{d['line']}: orig={d['orig'][:80]!r}")
                print(f"    {' ' * len(str(d['line']))}  new={d['patched'][:80]!r}")
            for ln in sd.get("lines_only_in_orig") or []:
                print(f"    ONLY-ORIG: {ln[:120]!r}")
            for ln in sd.get("lines_only_in_patched") or []:
                print(f"    ONLY-PATCHED: {ln[:120]!r}")
        for n in cmp["notes"]:
            print(f"  NOTE: {n}")
        for r in cmp["regression"]:
            print(f"  REGRESSION: {r}")
        print()
        print(f"判定：{cmp['verdict']}")
    return 0 if not cmp["regression"] else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
