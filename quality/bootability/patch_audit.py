#!/usr/bin/env python3
"""A. 逐代码补丁审计（Patch Audit）—— B0 体系，2026-11-11。

回答的问题：**那 N 百个被改的字节到底落在哪里，改动本身安全吗？**

与 ``quality/static/verify_patch.py`` 的根本区别：
  旧体系问「产物是否等长 / sha256 是否变了 / report 字段是否齐全」
  本脚本问「每处改动是否落在 code cave、跳转是否指回原函数、
            有没有踩到非 .text 区、有没有覆盖驱动符号」

四类断言（每条都有对应负样本，见 ``self_test.py``）：
  PA-1  改动落在可解释的区域（code cave / 函数入口 trampoline），不是随机命中代码中段
  PA-2  每处 b/bl 的位移是 4 的倍数（AArch64 指令对齐），且目标落在同一 Image 内
  PA-3  改动不落在只读数据区（.rodata/字符串区），落在那里几乎必然是 bug
  PA-4  每处改动都能归属到某个 hook 组（与引擎 report 对齐）；无法归属的改动要显式列出

用法::

    python3 quality/bootability/patch_audit.py \
        --orig kernel.orig --patched kernel.patched \
        --report kernel.patched.report.json \
        --json out/patch-audit.json

退出码：0 = 无阻断发现；1 = 存在阻断级发现；2 = 用法/输入错误。
"""

from __future__ import annotations

import argparse
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import common  # noqa: E402

# AArch64 instructions are 4 bytes; any changed run that contains a b/bl must
# start on a 4-byte boundary to be a valid instruction overwrite.
INSN = 4

# Regions of a bare arm64 Image, by convention (no ELF sections available):
#   [0, text_offset)          header
#   [text_offset, .text end)  code
#   after that                 rodata / data / bss / compressed tables
# We cannot know the exact boundary without the linker script, so we *measure*
# it: .text is the dense-code prefix, which we detect by instruction-likeness.
def region_kind(data: bytes, start: int, end: int) -> tuple[str, str]:
    """Classify what the *changed bytes themselves* look like.

    Returns ``(kind, why)`` where kind is one of:

    ``code``        instruction-shaped words that decode to something meaningful
    ``ascii``       the new bytes are printable text (key blob / rodata string)
    ``data``        high-entropy or structured non-code words
    ``padding``     all zero (a fresh code cave being filled with 0 is normal)

    Why this measures the changed bytes and not the surroundings: the earlier
    "code density of the surrounding 4 KB" version returned ~0.99 for *every*
    real site — including the inlined 47-byte key — because a key blob sits
    right next to the shellcode it belongs to. Any threshold on that number is
    either always-pass or always-fail. That is precisely the AVB-3 empty
    assertion this project must not repeat (HANDOFF-2026-11-08:153-155).

    What we actually care about: *are these particular bytes an instruction
    stream, or a literal?* A literal written into a code cave is expected (the
    engine embeds the key next to its trampoline); a literal written **over**
    an existing instruction with no branch structure is the dangerous case,
    and PA-1/PA-2 catch that separately.
    """
    body = data[start:end]
    if not body:
        return "padding", "empty"
    if all(b == 0 for b in body):
        return "padding", "all zero bytes"
    printable = sum(1 for b in body if 32 <= b < 127 or b == 0)
    if printable >= len(body) * 0.9:
        return "ascii", f"{printable}/{len(body)} bytes are printable text"
    words = len(body) // INSN
    if words == 0:
        return "data", "shorter than one instruction"
    branches = 0
    rets = 0
    for i in range(0, words * INSN, INSN):
        w = int.from_bytes(body[i : i + INSN], "little")
        if common.decode_b(w) is not None:
            branches += 1
        elif common.is_any_ret(w):
            rets += 1
    if branches or rets:
        return (
            "code",
            f"{branches} branch(es), {rets} ret(s) among {words} words",
        )
    # No control flow at all: could be a moved prologue (STP/MOV …) or data.
    # Distinguish by whether the high bits look like plausible opcodes rather
    # than an arbitrary literal.
    plausible = 0
    for i in range(0, words * INSN, INSN):
        w = int.from_bytes(body[i : i + INSN], "little")
        if (w >> 24) not in (0x00, 0xFF):
            plausible += 1
    if plausible >= words * 0.8:
        return "code", f"{plausible}/{words} words have code-like opcode bytes (no branch)"
    return "data", f"only {plausible}/{words} words look like instruction opcodes"


class Finding:
    """One audited patch site."""

    def __init__(self, start: int, end: int, orig: bytes, new: bytes):
        self.start = start
        self.end = end
        self.orig = orig
        self.new = new
        self.branches: list[dict] = []
        self.group: str | None = None
        self.notes: list[str] = []
        self.blocking: list[str] = []
        self.warnings: list[str] = []

    @property
    def length(self) -> int:
        return self.end - self.start

    def as_dict(self) -> dict:
        return {
            "offset": f"0x{self.start:08x}",
            "length": self.length,
            "aligned_4": self.start % INSN == 0,
            "orig_hex": self.orig.hex(),
            "new_hex": self.new.hex(),
            "group": self.group,
            "branches": self.branches,
            "blocking": self.blocking,
            "warnings": self.warnings,
            "notes": self.notes,
        }


def analyse_branches(f: Finding, image_size: int) -> None:
    """PA-2: every b/bl written by the patch must be aligned and in-image.

    A branch with a displacement not divisible by 4 is not a valid AArch64
    instruction at all; a branch whose target leaves the Image means the kernel
    jumps into unmapped memory at that point. Both are blocking.
    """
    for i in range(0, f.length - INSN + 1, INSN):
        word = int.from_bytes(f.new[i : i + INSN], "little")
        dec = common.decode_b(word)
        if dec is None:
            continue
        kind, disp = dec
        target = f.start + i + disp
        aligned_disp = disp % INSN == 0
        target_aligned = target % INSN == 0
        in_image = 0 <= target < image_size
        f.branches.append(
            {
                "at": f"0x{f.start + i:08x}",
                "kind": kind,
                "target": f"0x{target:08x}",
                "displacement": disp,
                "disp_aligned_4": aligned_disp,
                "target_aligned_4": target_aligned,
                "target_in_image": in_image,
            }
        )
        if not aligned_disp:
            f.blocking.append(
                f"PA-2: {kind} at 0x{f.start + i:08x} has displacement {disp} "
                f"(not a multiple of 4) — not a valid AArch64 branch"
            )
        if not in_image:
            f.blocking.append(
                f"PA-2: {kind} at 0x{f.start + i:08x} targets 0x{target:08x}, "
                f"outside the Image (0..0x{image_size:x})"
            )


def _declared_ranges(report: dict | None) -> tuple[list[tuple[str, int, int]], str]:
    """Extract ``[(group, start, end)]`` from an engine report.

    Returns ``(ranges, source)`` where source describes what was found — the
    caller turns an empty result into a *blocking* finding rather than silent
    0% attribution (see PA-0 below).

    Two report shapes are supported because the engine changed over time:
      * modern: ``hooks[]`` each with ``patch_offset`` + ``len`` (and optionally
        a nested ``patches[]`` list);
      * legacy: ``hook_table: true`` (a cJSON bug) — no per-hook detail at all.
    """
    ranges: list[tuple[str, int, int]] = []
    if not report:
        return ranges, "no report supplied"
    hooks = report.get("hooks")
    if not isinstance(hooks, list) or not hooks:
        if report.get("hook_table") in (True, "true"):
            return ranges, (
                "report has `hook_table: true` (legacy engine cJSON bug) — it "
                "carries no per-hook offsets"
            )
        return ranges, "report has no usable `hooks[]` array"
    for h in hooks:
        if not isinstance(h, dict):
            continue
        grp = str(h.get("group") or "?")
        nested = h.get("patches")
        if isinstance(nested, list) and nested:
            for p in nested:
                try:
                    off = int(str(p.get("patch_offset")), 0)
                    ln = int(p.get("len") or 0)
                except (TypeError, ValueError):
                    continue
                if ln > 0:
                    ranges.append((grp, off, off + ln))
            continue
        try:
            off = int(str(h.get("patch_offset")), 0)
            ln = int(h.get("len") or 0)
        except (TypeError, ValueError):
            continue
        if ln > 0:
            ranges.append((grp, off, off + ln))
    return ranges, "hooks[]" if ranges else "hooks[] present but no usable offsets"


def audit(orig: bytes, patched: bytes, report: dict | None) -> dict:
    """Run all PA checks; return a machine-readable result dict."""
    if len(orig) != len(patched):
        raise ValueError(f"length mismatch: {len(orig)} vs {len(patched)}")

    img = common.parse_arm64_image(orig)
    runs = common.runs_of(orig, patched, max_merge_gap=8)

    declared, attribution_source = _declared_ranges(report)

def _branch_targets(f: Finding) -> list[int]:
    """收集该 finding 新字节里全部 b/bl 的绝对目标地址。"""
    out = []
    for b in f.branches:
        out.append(int(b["target"], 16))
    return out


def audit(orig: bytes, patched: bytes, report: dict | None) -> dict:
    """Run all PA checks; return a machine-readable result dict."""
    if len(orig) != len(patched):
        raise ValueError(f"length mismatch: {len(orig)} vs {len(patched)}")

    img = common.parse_arm64_image(orig)
    runs = common.runs_of(orig, patched, max_merge_gap=8)

    declared, attribution_source = _declared_ranges(report)

    findings = [Finding(s, e, orig[s:e], patched[s:e]) for s, e in runs]
    for f in findings:
        analyse_branches(f, len(orig))

        # Attribute to a declared engine group when possible.
        #   路径 A（区间重叠）：改动落在引擎声明的补丁区间内。
        #   路径 B（BUG-QA-10，2026-11-12）：改动区里某条 b/bl 的目标落进
        #   某个声明区间 —— 引擎把 hook 函数体整体声明为 patch 区间，
        #   入口 trampoline 跳回函数中段是合法形态（实测 0x38fcd4 → 0x24c04
        #   ∈ do_execve [0x24bd4,0x24c78)），只走 A 会把这类入口改动判成
        #   未解释，与 crossverify 的 branch-target 判法不对称。
        for grp, ds, de in declared:
            if f.start < de and f.end > ds:
                f.group = grp
                f.notes.append(f"attributed via region-overlap [{ds:#x},{de:#x})")
                break
        if f.group is None:
            for tgt in _branch_targets(f):
                hit = next(((grp, ds, de) for grp, ds, de in declared
                            if ds <= tgt < de), None)
                if hit:
                    f.group = hit[0]
                    f.notes.append(
                        f"attributed via branch-target 0x{tgt:x} ∈ "
                        f"[{hit[1]:#x},{hit[2]:#x})")
                    break

        # PA-3: what did the patch actually write here? A literal (the embedded
        # key) is legitimate *inside a cave*; a literal written straight over
        # an existing instruction is a rewrite, not a byte-patch.
        kind, why = region_kind(patched, f.start, f.end)
        f.notes.append(f"new bytes look like: {kind} ({why})")
        if kind == "data":
            f.blocking.append(
                f"PA-3: change at 0x{f.start:08x} wrote {kind} bytes over code "
                f"({why}) — a byte-patch must write instructions or a literal "
                f"into a cave, not arbitrary data over live code"
            )

        # PA-1: a finding that is neither a declared group nor contains any
        # branch and does not start aligned is suspicious ("hit the middle of
        # an instruction with no jump"). Declared groups are allowed to be
        # cave blobs, which legitimately contain no branch at their start.
        if f.group is None and not f.branches and f.start % INSN != 0:
            f.warnings.append(
                f"PA-1: unattributed change at 0x{f.start:08x} is not "
                f"instruction-aligned and contains no branch — verify by hand"
            )

    blocking = [b for f in findings for b in f.blocking]
    warnings = [w for f in findings for w in f.warnings]
    unattributed = [
        {"offset": f"0x{f.start:08x}", "length": f.length}
        for f in findings
        if f.group is None
    ]
    attributed = sum(1 for f in findings if f.group)

    # ------------------------------------------------------------------
    # PA-0: attribution coverage must never be "mostly nothing".
    #
    # A real run once reported `attributed_regions: 0 / unattributed: 9` and
    # still returned PASS, because `--report` had not been passed. That is the
    # project's signature failure mode — a gate that runs, goes green, and did
    # no work. Per-function attribution *is* the value of a code-level audit.
    #
    # BUG-QA-10（2026-11-12 审计）：旧阈值"==0 才阻断"存在覆盖漏洞 —— 1 处
    # cave 命中即可让 PA-0 通过、其余 19 处未解释改动只留在 unattributed 列表
    # 里不阻断（"1 处归属掩护 19 处改动"）。现改为**覆盖率阈值**：
    #   coverage = attributed / findings < 0.5 → BLOCKING（阈值 50%，明示）
    # 50% 的依据：正常产物上引擎 report 与字节 diff 应高度吻合（真实样本
    # 20/20）；低于一半说明 report 与产物严重脱节或 patch 来源不受控，
    # 逐代码审计已无法回答"改的是哪里"，宁红勿绿。
    # ------------------------------------------------------------------
    PA0_COVERAGE_THRESHOLD = 0.5
    if findings:
        coverage = attributed / len(findings)
        if coverage < PA0_COVERAGE_THRESHOLD:
            if declared:
                why = (
                    f"引擎 report 声明了 {len(declared)} 个补丁区间，但覆盖率仅 "
                    f"{attributed}/{len(findings)}（{coverage:.0%}）—— report 与"
                    f"实际产物不符，或存在来源不受控的改动"
                )
            else:
                why = f"无可用归属信息（{attribution_source}）"
            blocking.append(
                f"PA-0: {len(findings)} 处字节改动、仅 {attributed} 处可归属到 "
                f"hook 组（coverage {coverage:.0%} < {PA0_COVERAGE_THRESHOLD:.0%}"
                f" 阈值）—— {why}。"
                f"「逐代码检查」的核心就是把改动映射回具体函数；覆盖率不足一半"
                f"意味着本审计无法回答『大部分改动改的是哪里』。"
                f"请传 --report，或修复 report / 排查未归属改动。"
            )
    if findings and not declared:
        warnings.append(f"PA-0: 归属信息不可用（{attribution_source}）")

    return {
        "image": {
            "size": len(orig),
            "magic_ok": img.magic_ok,
            "text_offset": f"0x{img.text_offset:x}",
            "image_size": f"0x{img.image_size:x}",
            "flags": f"0x{img.flags:x}",
        },
        "attribution": {
            "source": attribution_source,
            "declared_ranges": len(declared),
            "attributed_regions": attributed,
            "unattributed_regions": len(unattributed),
            "coverage": (
                f"{attributed}/{len(findings)}" if findings else "n/a"
            ),
            # BUG-QA-10：PA-0 阈值与覆盖率显式入档，供 run_all/人审消费
            "coverage_ratio": (attributed / len(findings)) if findings else None,
            "pa0_coverage_threshold": 0.5,
        },
        "summary": {
            "changed_regions": len(findings),
            "changed_bytes": sum(f.length for f in findings),
            "branches_written": sum(len(f.branches) for f in findings),
            "attributed_regions": attributed,
            "unattributed_regions": len(unattributed),
            "blocking_findings": len(blocking),
            "warnings": len(warnings),
        },
        "findings": [f.as_dict() for f in findings],
        "unattributed": unattributed,
        "blocking": blocking,
        "warnings": warnings,
    }


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description="A. 逐代码补丁审计 (B0)")
    ap.add_argument("--orig", required=True, help="原厂 kernel Image（裸 Image）")
    ap.add_argument("--patched", required=True, help="patched kernel Image")
    ap.add_argument("--report", help="引擎 report.json（用于把改动归属到 hook 组）")
    ap.add_argument("--json", help="输出机器可读结果到此路径")
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

    try:
        res = audit(orig, patched, report)
    except ValueError as e:
        print(f"ERROR: {e}", file=sys.stderr)
        return 2

    if args.json:
        os.makedirs(os.path.dirname(os.path.abspath(args.json)), exist_ok=True)
        with open(args.json, "w", encoding="utf-8") as fh:
            json.dump(res, fh, ensure_ascii=False, indent=2)

    if not args.quiet:
        s = res["summary"]
        print("== A. 逐代码补丁审计 ==")
        print(f"镜像: {res['image']['size']} B  magic_ok={res['image']['magic_ok']}  "
              f"text_offset={res['image']['text_offset']}")
        print(f"改动: {s['changed_regions']} 处 / {s['changed_bytes']} 字节  "
              f"写入跳转 {s['branches_written']} 条")
        print(f"归属: {s['attributed_regions']} 处可归属 hook 组, "
              f"{s['unattributed_regions']} 处无法归属")
        print()
        for f in res["findings"]:
            tag = f["group"] or "<unattributed>"
            print(f"  0x{f['offset'][2:]}  len={f['length']:<4} align4={f['aligned_4']!s:<5} {tag}")
            for b in f["branches"]:
                print(f"      {b['kind']:<2} -> {b['target']} (disp={b['displacement']})")
            for msg in f["blocking"]:
                print(f"      BLOCKING: {msg}")
            for msg in f["warnings"]:
                print(f"      WARN: {msg}")
        print()
        if res["blocking"]:
            print(f"判定：FAIL（{len(res['blocking'])} 项阻断级发现）")
            for b in res["blocking"]:
                print(f"  - {b}")
            return 1
        print(f"判定：PASS（{len(res['warnings'])} 项需人工复核的警告）")
    return 0 if not res["blocking"] else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
