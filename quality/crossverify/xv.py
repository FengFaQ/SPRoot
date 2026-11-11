#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""SPRoot 内核补丁 · 独立交叉验证（第二套实现）

────────────────────────────────────────────────────────────────────────────
为什么存在这个文件
────────────────────────────────────────────────────────────────────────────
`quality/bootability/` 是 task-18 的产物（patch_audit / driver_integrity / boot_qemu）。
本目录 `quality/crossverify/` 是**完全独立的第二套实现**，刻意不复用它的任何代码、
不 import 它、不共享辅助函数。

理由：一套检查系统对自己说谎时，自己发现不了。
本项目已经栽过同类跟头——
  · AVB-3 断言写成永远 PASS，全绿了一整代
  · patch_audit 首跑 attributed_regions=0（没传 --report），仍然 rc=0 / ok:true
两套独立实现互相校验，比单套更难自欺。

实现风格刻意不同：
  · bootability 用自研 AArch64 解码 + 启发式符号归属
  · 本套用 **aarch64-linux-gnu-objdump 真实反汇编** + 控制流出口判定
  · 时间戳归一化后的启动日志逐行 diff（直接 diff 会因时间戳全不等而失效）

────────────────────────────────────────────────────────────────────────────
检查项
────────────────────────────────────────────────────────────────────────────
  XV-1  逐字节 diff：区段数 / 总字节 / 等长
  XV-2  每处改动能否归因到引擎 report 声明的地址（容差规则显式定义）
  XV-3  每个 code cave 是否有**控制流出口**（b / br / ret 任一），
        且出口目标落在 cave 之外（= 回到原流程）。无出口 = 内核必挂死
  XV-4  改动是否触碰"非代码区"（用 cave 内/外 + 可执行性启发式判断）
  XV-5  启动日志对比：时间戳归一化后逐行 diff，只允许白名单内的抖动差异
  XV-6  kallsyms 可用性探测（决定驱动符号检查能否做，诚实报边界）

退出码：0 = 全部 PASS；1 = 有 FAIL
────────────────────────────────────────────────────────────────────────────
"""

import argparse
import json
import os
import re
import struct
import subprocess
import sys
import tempfile

# 启动日志里允许出现差异的行（时间/性能抖动，不是功能差异）
NOISE_PATTERNS = [
    r"audit\(\d+\.\d+:\d+\)",                    # audit 时间戳
    r"setting system clock to \S+",              # RTC 墙上时间
    r"used greatest stack depth: \d+ bytes",     # 栈深度抖动
    r"random: crng init done",
    r"Freeing unused kernel memory: \d+K",
    r"clocksource: Switched to clocksource \S+",
    r"^\[?\s*\d+\.\d+\]?",                       # 裸时间戳残留
]


# ══════════════════════════════════════════════════════════════════════════
# XV-1 / XV-2：逐字节 diff + 归因
# ══════════════════════════════════════════════════════════════════════════
def diff_regions(a, b):
    """返回 [(offset, length, orig_bytes, new_bytes)]。"""
    if len(a) != len(b):
        raise ValueError("images differ in length: %d vs %d" % (len(a), len(b)))
    out = []
    i, n = 0, len(a)
    while i < n:
        if a[i] != b[i]:
            s = i
            while i < n and a[i] != b[i]:
                i += 1
            out.append((s, i - s, a[s:i], b[s:i]))
        else:
            i += 1
    return out


def collect_declared(report):
    """从引擎 report 收集所有声明过的地址 -> 标签。

    覆盖 hooks[]、hook_table[]、launcher、caves[]。
    地址字段可能是 "0x..." 字符串或 int，两种都吃。
    """
    declared = []

    def add(v, label):
        if isinstance(v, str) and v.lower().startswith("0x"):
            try:
                declared.append((int(v, 16), label))
            except ValueError:
                pass
        elif isinstance(v, int) and v > 0:
            declared.append((v, label))

    for h in report.get("hooks") or []:
        label = h.get("group") or h.get("name") or "?"
        for k in ("patch_offset", "target_addr", "hook_addr", "trampoline"):
            add(h.get(k), label)
        for p in h.get("patches") or []:
            if isinstance(p, dict):
                for k in ("offset", "addr", "hook_addr"):
                    add(p.get(k), label)

    for t in report.get("hook_table") or []:
        if isinstance(t, dict):
            add(t.get("addr"), t.get("group") or "hook_table")

    launcher = report.get("launcher") or {}
    for k in ("hook_addr", "cave_offset"):
        add(launcher.get(k), "launcher")
    for c in launcher.get("caves") or []:
        if isinstance(c, dict):
            add(c.get("offset"), "launcher:cave")

    for c in report.get("caves") or []:
        if isinstance(c, dict):
            add(c.get("offset"), "cave")

    return declared


def attribute(regions, declared, tol=0x400, patched=None):
    """给每个改动区段找归属；容差规则显式写出，免得后人猜。

    两条独立的归属路径（任一命中即算已归属）：
      A. **地址邻近**：区段起点与某声明地址距离 <= tol
      B. **跳转目标命中**（关键补充）：该区段里的 `b` 指令目标落在某个声明地址
         ± tol 范围内 —— 这说明"这里被改成一个跳到引擎 cave 的跳转"，
         即使这个区段本身的位置没被 report 显式列出来。

    ⚠️ 为什么必须有 B：引擎的 `hooks[].patch_offset` 记的是**跳板/cave 侧**地址，
    而被它改写的**原函数入口**（例如 `do_execve` 的 `sub sp,sp,#0x70`）不在其中。
    只走 A 会把这类"入口 → cave"的 hook 判成未解释（实测 3 处）。
    它们恰恰是最正常的 hook：入口换成 `b cave`，cave 里做活后跳回。

    返回 (attributed, unexplained, detail_list)。
    """
    attr, unexplained, detail = 0, [], []
    for off, ln, _o, newb in regions:
        hits = [(a, g) for a, g in declared if abs(a - off) <= tol]
        reason = "addr-proximity"
        if not hits and patched is not None:
            # 解析区段内的 AArch64 `b` 指令，看目标是否落在声明地址附近
            for i in range(0, max(0, ln - 3), 4):
                w = struct.unpack_from("<I", newb, i)[0]
                if (w >> 26) != 0x5:          # 不是 B 指令
                    continue
                imm = w & 0x03FFFFFF
                if imm & (1 << 25):           # 符号扩展
                    imm -= (1 << 26)
                tgt = off + i + imm * 4
                near = [(a, g) for a, g in declared if abs(a - tgt) <= tol]
                if near:
                    hits, reason = near, "branch-target"
                    break
        if hits:
            attr += 1
            detail.append({"offset": hex(off), "len": ln,
                           "group": hits[0][1], "via": reason})
        else:
            unexplained.append((off, ln))
    return attr, unexplained, detail


# ══════════════════════════════════════════════════════════════════════════
# XV-3：cave 控制流出口（用真实反汇编，不手写位掩码）
# ══════════════════════════════════════════════════════════════════════════
def disassemble(blob):
    """用 objdump 反汇编一段裸字节，返回文本。失败返回 None。

    注意：objdump 对裸 binary 有时会提前报 "Address ... is out of bounds"
    而截断输出。这里给 blob 尾部补 padding，并把 stop-address 明确设到原始长度，
    避免因截断导致漏判跳转（曾因此把真实 cave 误判为"无出口"）。
    """
    fd, tmp = tempfile.mkstemp(suffix=".bin")
    try:
        with os.fdopen(fd, "wb") as f:
            f.write(blob)
            f.write(b"\x00" * 64)      # padding，防 objdump 提前截断
        p = subprocess.run(
            ["aarch64-linux-gnu-objdump", "-D", "-b", "binary", "-m", "aarch64",
             "--start-address=0", "--stop-address=%d" % len(blob), tmp],
            capture_output=True, timeout=60)
        if p.returncode != 0:
            return None
        return p.stdout.decode("utf-8", "replace")
    except (OSError, subprocess.SubprocessError):
        return None
    finally:
        try:
            os.unlink(tmp)
        except OSError:
            pass


def cave_has_exit(text, cave_off, cave_len):
    """判定 cave 是否有离开自身的控制流。

    三种出口都算：
      b / b.<cond>  <target>  —— 有条件/无条件跳转（target 在 cave 外即离开）
      br xN                   —— 间接跳转
      ret                     —— 返回
      cbz/cbnz/tbz/tbnz <target> —— 条件分支（也是跳转）

    注意 objdump 对裸 binary 的反汇编偏移是**相对 blob 起点**，
    所以"目标在 cave 内"的判断用相对偏移，不用绝对地址。

    ⚠️ 上一版只匹配无条件 `b`，漏了 `b.cs` / `b.ne` / `cbnz` 等条件跳转，
    导致真实 shellcode（用 `b.cs 0xa0` 做错误分支）被误判"无出口"。
    这是实现缺陷，不是镜像缺陷。
    """
    if text is None:
        return None, "objdump 不可用，无法判定"

    exits = []
    # 所有跳转/分支类指令，objdump 实际输出样例：
    #   3c:  54000322   b.cs   0xa0  // b.hs, b.nlast      ← 无逗号
    #   54:  54000261   b.ne   0xa0  // b.any
    #   58:  35ffff8f   cbnz   w15, 0x48                    ← 有逗号
    #   10:  365a6d6a   tbz    w10, #11, 0x4dbc            ← 两个逗号
    # 所以按"最后一个 0x.... 是目标"来抓，比逗号分割可靠。
    branch_re = re.compile(
        r"^\s*([0-9a-f]+):\s+[0-9a-f]{8}\s+"
        r"(b\.[a-z]+|b|bl|cbz|cbnz|tbz|tbnz)\s+.*?(0x[0-9a-f]+)\s*$",
        re.M)
    for m in branch_re.finditer(text):
        src = int(m.group(1), 16)
        dst = int(m.group(3), 16)
        if not (0 <= dst < cave_len):
            exits.append((m.group(2), src, dst))
    # 间接跳转 br xN
    for m in re.finditer(r"^\s*([0-9a-f]+):\s+[0-9a-f]{8}\s+br\s+x\d+", text, re.M):
        exits.append(("br", int(m.group(1), 16), None))
    # 返回 ret
    for m in re.finditer(r"^\s*([0-9a-f]+):\s+[0-9a-f]{8}\s+ret\b", text, re.M):
        exits.append(("ret", int(m.group(1), 16), None))

    if exits:
        kinds = sorted({e[0] for e in exits})
        return True, "%d 个出口（%s）" % (len(exits), ", ".join(kinds))
    return False, "** 无任何控制流出口（b/b.cond/br/ret），内核执行到此处会挂死 **"


# ══════════════════════════════════════════════════════════════════════════
# XV-5：启动日志归一化对比
# ══════════════════════════════════════════════════════════════════════════
def normalize_log(path, stop_at_panic=True):
    """去掉行首时间戳与 CPU 标记，并按噪声白名单抹平抖动行。

    stop_at_panic=True：只比较**第一次 panic 之前**的启动序列。
    原因：panic 后 init 会反复重试挂载 → 日志长度随时间无界增长，
    两个 kernel 的重试次数不一定相同（7932 vs 9476 行），
    全量对比必然出现成千上万行"差异"，那是噪声不是功能差异。
    """
    out = []
    with open(path, encoding="utf-8", errors="replace") as f:
        for line in f:
            s = re.sub(r"^\[\s*\d+\.\d+\]\[[^\]]*\]\s*", "", line.rstrip("\n"))
            if stop_at_panic and "Kernel panic" in s:
                break
            if any(re.search(p, s) for p in NOISE_PATTERNS):
                continue
            out.append(s)
    return out


def compare_logs(a, b):
    """返回 (ok, added, removed)。"""
    la, lb = normalize_log(a), normalize_log(b)
    import difflib
    diff = list(difflib.unified_diff(la, lb, lineterm="", n=0))
    added = [d[1:] for d in diff if d.startswith("+") and not d.startswith("+++")]
    removed = [d[1:] for d in diff if d.startswith("-") and not d.startswith("---")]
    return (not added and not removed), added, removed


# ══════════════════════════════════════════════════════════════════════════
# XV-6：kallsyms 可用性
# ══════════════════════════════════════════════════════════════════════════
def kallsyms_probe(data):
    markers = [b"kallsyms_names", b"kallsyms_offsets",
               b"kallsyms_num_syms", b"kallsyms_token_table"]
    return {m.decode(): (m in data) for m in markers}


# ══════════════════════════════════════════════════════════════════════════
def main(argv=None):
    ap = argparse.ArgumentParser(
        description="SPRoot 内核补丁独立交叉验证（第二套实现，与 quality/bootability 零共享代码）")
    # ⚠️ --orig/--patched 设为非必填：否则 `--self-test` 会被 argparse 挡住
    #    （自检不需要真实镜像，只在内存里造夹具）。真正运行时再手工校验存在性。
    ap.add_argument("--orig", help="原厂 kernel Image")
    ap.add_argument("--patched", help="修补后 kernel Image")
    ap.add_argument("--report", help="引擎 report.json（用于 XV-2 归因）")
    ap.add_argument("--log-orig", help="原厂启动串口日志（XV-5）")
    ap.add_argument("--log-patched", help="修补后启动串口日志（XV-5）")
    ap.add_argument("--json", help="把结果写成 JSON")
    ap.add_argument("--self-test", action="store_true", help="负样本自检")
    args = ap.parse_args(argv)

    if args.self_test:
        return self_test()

    missing = [n for n in ("orig", "patched") if not getattr(args, n)]
    if missing:
        ap.error("缺少必需参数：%s"
                 % ", ".join("--" + m for m in missing))

    orig = open(args.orig, "rb").read()
    patched = open(args.patched, "rb").read()

    results = {}

    # ── XV-1 ──────────────────────────────────────────────────────────
    try:
        regions = diff_regions(orig, patched)
        xv1 = {"ok": True, "regions": len(regions),
               "bytes": sum(r[1] for r in regions),
               "equal_length": len(orig) == len(patched)}
    except ValueError as e:
        print("[XV-1] FAIL  %s" % e)
        return 1
    print("[XV-1] %s  改动 %d 区段 / %d 字节，等长=%s"
          % ("PASS" if xv1["ok"] else "FAIL", xv1["regions"], xv1["bytes"],
             xv1["equal_length"]))
    results["XV-1"] = xv1

    # ── XV-2 ──────────────────────────────────────────────────────────
    if args.report:
        report = json.load(open(args.report, encoding="utf-8"))
        declared = collect_declared(report)
        attr, unexp, detail = attribute(regions, declared, patched=patched)
        # ⚠️ 这里必须是 len(unexp) == 0 —— unexp 是列表，
        #    写成 `unexp == 0` 恒为 False（列表永不等于 int），
        #    会表现为"未解释 0 处却判 FAIL"的自伤 bug。
        xv2 = {"ok": len(unexp) == 0, "declared": len(declared),
               "attributed": attr, "unexplained": len(unexp),
               "detail": detail}
        print("[XV-2] %s  声明地址 %d 个；可归属 %d/%d 处，未解释 %d 处"
              % ("PASS" if xv2["ok"] else "FAIL", len(declared), attr,
                 len(regions), len(unexp)))
        via_target = [d for d in detail if d["via"] == "branch-target"]
        if via_target:
            print("        其中 %d 处经『跳转目标命中』归属（原函数入口 → 引擎 cave）："
                  % len(via_target))
            for d in via_target:
                print("          %s len=%d -> %s" % (d["offset"], d["len"], d["group"]))
        for off, ln in unexp[:10]:
            print("        未解释: %#010x len=%d" % (off, ln))
    else:
        xv2 = {"ok": None, "skipped": "未提供 --report"}
        print("[XV-2] SKIP  未提供 --report（无法归因；不等于通过）")
    results["XV-2"] = xv2

    # ── XV-3 ──────────────────────────────────────────────────────────
    # ⚠️ 关键设计点：cave 的边界必须用**引擎声明的范围**，
    #    不能用「diff 出来的改动区段」——后者会被字节差异切成碎片
    #    （例如 shellcode 里有原地未变的 zero 字节，diff 就会把 176 字节的
    #     一个 cave 切成 110+63+5 三片，拿中间那片去判"有无出口"必然误报）。
    #
    # 所以：先从 report 取每个声明地址，把**相邻声明地址之间的区域**合并成候选 cave；
    # 没有 report 时，退化为「把间隙 < 32 字节的改动区段合并」。
    def merge_caves(regs, gap=64):
        out = []
        for off, ln, _a, _b in regs:
            if out and off - (out[-1][0] + out[-1][1]) <= gap:
                o, l = out[-1]
                out[-1] = (o, max(o + l, off + ln) - o)
            else:
                out.append((off, ln))
        return out

    if args.report:
        declared = collect_declared(report)
        # 声明地址排序后，相邻两个之间的"中间区"就是引擎往里面写东西的 cave 区
        addrs = sorted({a for a, _g in declared})
        spans = []
        for i in range(len(addrs) - 1):
            a, b = addrs[i], addrs[i + 1]
            if 0 < b - a <= 0x2000:        # 只合并邻近的（同一 cave 内）
                spans.append((a, b - a))
        # 只保留"真的有改动落入"的 span
        caves = []
        for s_off, s_len in spans:
            hit = any(not (off + ln <= s_off or off >= s_off + s_len)
                      for off, ln, _a, _b in regions)
            if hit:
                caves.append((s_off, s_len))
        # 合并重叠
        merged = []
        for o, l in sorted(caves):
            if merged and o <= merged[-1][0] + merged[-1][1]:
                mo, ml = merged[-1]
                merged[-1] = (mo, max(mo + ml, o + l) - mo)
            else:
                merged.append((o, l))
        caves = merged
    else:
        caves = [(o, l) for o, l in merge_caves(regions) if l >= 32]

    xv3 = {"ok": True, "caves": []}
    print("[XV-3] cave 范围由%s界定，共 %d 处"
          % (" report 声明地址" if args.report else " diff 区段合并", len(caves)))
    for off, ln in caves:
        text = disassemble(patched[off:off + ln])
        ok, msg = cave_has_exit(text, off, ln)
        if ok is None:
            xv3["ok"] = False
            print("[XV-3] SKIP  %#010x len=%d：%s" % (off, ln, msg))
        elif not ok:
            xv3["ok"] = False
            print("[XV-3] FAIL  %#010x len=%d：%s" % (off, ln, msg))
        else:
            print("[XV-3] PASS  %#010x len=%d：%s" % (off, ln, msg))
        xv3["caves"].append({"offset": hex(off), "len": ln, "ok": ok,
                             "detail": msg})
    results["XV-3"] = xv3

    # ── XV-4 ──────────────────────────────────────────────────────────
    # 启发式：AArch64 内核 Image 的 .text 通常在头部到 code_size 范围内。
    # Image header 的 image_size 字段（offset 0x10）给出有效镜像大小。
    image_size = None
    if len(orig) >= 0x14 and orig[:4] == b"MZ\x40\xfa":
        image_size = struct.unpack_from("<Q", orig, 0x10)[0]
    beyond = []
    if image_size:
        beyond = [(o, l) for o, l, _a, _b in regions if o + l > image_size]
        xv4 = {"ok": not beyond, "image_size": image_size,
               "regions_beyond": len(beyond)}
        print("[XV-4] %s  image_size=%#x；越界改动 %d 处"
              % ("PASS" if xv4["ok"] else "FAIL", image_size, len(beyond)))
        for o, l in beyond[:10]:
            print("        越界: %#010x len=%d" % (o, l))
    else:
        xv4 = {"ok": None, "skipped": "无法解析 Image header 的 image_size"}
        print("[XV-4] SKIP  无法解析 image_size（非标准 ARM64 Image？）")
    results["XV-4"] = xv4

    # ── XV-5 ──────────────────────────────────────────────────────────
    if args.log_orig and args.log_patched:
        ok, added, removed = compare_logs(args.log_orig, args.log_patched)
        xv5 = {"ok": ok, "added": len(added), "removed": len(removed)}
        print("[XV-5] %s  归一化后差异：+%d / -%d 行"
              % ("PASS" if ok else "FAIL", len(added), len(removed)))
        for line in (added[:5] + removed[:5]):
            print("        | %s" % line[:110])
    else:
        xv5 = {"ok": None, "skipped": "未提供启动日志"}
        print("[XV-5] SKIP  未提供 --log-orig/--log-patched")
    results["XV-5"] = xv5

    # ── XV-6 ──────────────────────────────────────────────────────────
    ks = kallsyms_probe(orig)
    avail = any(ks.values())
    print("[XV-6] INFO  kallsyms 特征串: %s"
          % ", ".join("%s=%s" % (k, "有" if v else "无") for k, v in ks.items()))
    if not avail:
        print("         → 该内核未暴露 kallsyms 特征串，**无法按符号名做驱动检查**；")
        print("           驱动完整性只能靠'地址区间重叠'策略，此为诚实能力边界。")
    results["XV-6"] = {"ok": None, "kallsyms": ks, "symbol_check_possible": avail}

    # ── 汇总 ──────────────────────────────────────────────────────────
    hard = [k for k, v in results.items() if v.get("ok") is False]
    print("")
    print("=" * 66)
    if hard:
        print("判定：FAIL —— %s 未通过" % ", ".join(sorted(hard)))
    else:
        print("判定：PASS —— 所有可执行的检查项均通过")
    print("=" * 66)

    if args.json:
        with open(args.json, "w", encoding="utf-8") as f:
            json.dump(results, f, ensure_ascii=False, indent=2)
        print("结果已写入 %s" % args.json)

    return 1 if hard else 0


# ══════════════════════════════════════════════════════════════════════════
# 自检：负样本铁律
# ══════════════════════════════════════════════════════════════════════════
def self_test():
    """每条判据都必须在坏输入上判红、好输入上判绿。

    注意断言的是**判据函数对坏输入返回 False**，不是"某个布尔为 True"——
    后者所有用例都写 True 时，判据退化成恒真也发现不了（本项目栽过）。
    """
    cases = []
    fail = 0

    # XV-1 diff
    a = bytes([0, 0, 0, 0, 0, 0])
    b = bytes([0, 1, 0, 0, 2, 0])
    rs = diff_regions(a, b)
    cases.append(("XV-1 坏输入：长度不等应抛异常",
                  _raises(lambda: diff_regions(a, b[:4])), True))
    cases.append(("XV-1 好输入：等长 2 处改动",
                  len(rs) == 2 and rs[0][0] == 1 and rs[1][0] == 4, True))

    # XV-2 归因
    regs = [(0x1000, 4, b"", b""), (0x9000, 4, b"", b"")]
    decl = [(0x1000, "do_execve")]
    attr, unexp, _d = attribute(regs, decl, tol=0x400)
    cases.append(("XV-2 坏输入：有未声明改动 → 应报未解释",
                  len(unexp) == 1 and attr == 1, True))
    attr2, unexp2, _d2 = attribute([(0x1000, 4, b"", b"")], decl, tol=0x400)
    cases.append(("XV-2 好输入：全部可归因 → 未解释 0",
                  len(unexp2) == 0, True))

    # XV-2b：**跳转目标归属**（本轮实测抓到的关键补充）
    # 构造：0x9000 处一条 `b 0x1050`，0x1050 落在声明地址 0x1000 的 ±0x400 内
    # → 虽然 0x9000 本身远离声明地址，但应经"跳转目标命中"归属成功。
    imm = (0x1050 - 0x9000) // 4
    w = 0x14000000 | (imm & 0x03FFFFFF)
    branch_new = struct.pack("<I", w)
    a3, u3, d3 = attribute([(0x9000, 4, b"", branch_new)], [(0x1000, "do_execve")],
                           tol=0x400, patched=b"\x00" * 0x10000)
    cases.append(("XV-2b 好输入：b 跳到声明地址附近 → 经跳转目标归属",
                  len(u3) == 0 and any(x["via"] == "branch-target" for x in d3), True))
    # 负样本：b 跳到完全无关的远处 → 仍应判未解释
    imm2 = (0x100000 - 0x9000) // 4
    w2 = 0x14000000 | (imm2 & 0x03FFFFFF)
    a4, u4, _d4 = attribute([(0x9000, 4, b"", struct.pack("<I", w2))],
                            [(0x1000, "do_execve")], tol=0x400,
                            patched=b"\x00" * 0x200000)
    cases.append(("XV-2b 坏输入：b 跳到无关远处 → 应仍判未解释",
                  len(u4) == 1, True))

    # XV-3 cave 出口
    # AArch64 `b` 编码 = 0x14000000 | (imm26)，小端存放。
    # `b +0x100` → imm26 = 0x40 → 机器字 0x14000040 → 字节序列 40 00 00 14
    # （上一版把 hex 字符串写反成 "14000040"，objdump 解成 .inst/undefined，
    #  于是"有出口"被判成 False —— 这条自检正是用来抓这种编码错误的。）
    blob_exit = bytes.fromhex("40000014") + bytes(4)      # b +0x100（跳出 8 字节 cave）
    # `b +0`（跳回自身）→ 机器字 0x14000000 → 字节序列 00 00 00 14
    blob_loop = bytes.fromhex("00000014") + bytes(4)
    t1 = disassemble(blob_exit)
    t2 = disassemble(blob_loop)
    if t1 is not None:
        ok1, msg1 = cave_has_exit(t1, 0, len(blob_exit))
        cases.append(("XV-3 好输入：b 跳出 cave → 有出口", ok1 is True, True))
    if t2 is not None:
        ok2, _ = cave_has_exit(t2, 0, len(blob_loop))
        cases.append(("XV-3 坏输入：b 跳自身（死循环）→ 无有效出口",
                      ok2 is False, True))
        # 证明"有出口"与"无出口"确实被判成不同结果
        cases.append(("XV-3 鉴别力：出口真值 != 死循环真值",
                      cave_has_exit(t1, 0, 8)[0] != cave_has_exit(t2, 0, 8)[0],
                      True))

    # XV-5 日志归一化
    tmpd = tempfile.mkdtemp()
    p1 = os.path.join(tmpd, "l1.log")
    p2 = os.path.join(tmpd, "l2.log")
    p3 = os.path.join(tmpd, "l3.log")
    open(p1, "w", encoding="utf-8").write(
        "[    0.000000][0:0] Booting Linux\n[    0.100000][0:1] driver probe ok\n")
    # 仅时间戳不同 → 应判相同
    open(p2, "w", encoding="utf-8").write(
        "[    0.000000][0:0] Booting Linux\n[    0.200000][0:1] driver probe ok\n")
    # 多一行真实差异 → 应判不同
    open(p3, "w", encoding="utf-8").write(
        "[    0.000000][0:0] Booting Linux\n[    0.100000][0:1] driver probe FAILED\n")
    ok_same, _, _ = compare_logs(p1, p2)
    ok_diff, add, _ = compare_logs(p1, p3)
    cases.append(("XV-5 好输入：仅时间戳不同 → 判相同", ok_same, True))
    cases.append(("XV-5 坏输入：驱动行为不同 → 判不同", not ok_diff, True))

    # XV-6 kallsyms
    cases.append(("XV-6 坏输入：无 kallsyms 特征 → 可用性 False",
                  any(kallsyms_probe(b"nothing here").values()), False))
    cases.append(("XV-6 好输入：含特征串 → 可用性 True",
                  any(kallsyms_probe(b"xxkallsyms_namesyy").values()), True))

    print("== crossverify --self-test ==")
    for title, got, want in cases:
        ok = got == want
        print("  %-52s 实得%-5s 期望%-5s %s"
              % (title, str(got), str(want), "OK" if ok else "**FAIL**"))
        if not ok:
            fail += 1
    print("")
    if fail:
        print("判定：SELF-TEST FAIL —— %d 条判据失去鉴别力" % fail)
        return 1
    print("判定：SELF-TEST PASS —— 全部判据在坏输入判红、好输入判绿")
    return 0


def _raises(fn):
    try:
        fn()
    except Exception:
        return True
    return False


if __name__ == "__main__":
    sys.exit(main())
