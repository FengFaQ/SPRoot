#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""SPRoot 新项目 — 修补产物静态验证（NEW-DOC-06 §4 V 组断言）

覆盖 V-02/V-03（等长、确实改动）、V-04/V-05（密钥内嵌与落位）、
V-06（隐藏目录前缀）、V-07（arm64 Image 头可解析）、V-08（hook 落位齐全）、
V-09（报告自洽）、V-10（NDJSON 合法）、V-13（改动区段受控）、
V-17（KP 残留，弱断言）。

不覆盖（需人工/其他脚本）：V-01/V-11（失败不落盘，需跑引擎负样本）、
V-12（退出码矩阵）、V-14（SELinux policy 反汇编复核）、
V-15（magiskboot round-trip）、V-16（APK 内 arm64 产物）、
V-18（密钥不出日志）、V-19/V-20（代码审查/版本一致）。

另含三项工具模式（NEW-DOC-06 §9 G-02 门禁证据 / task-10 样本加载 / 自检门禁）：
  --elf <path>       ELF 产物断言（AArch64 / 静态性 / 依赖 / 可执行位 / PT_TLS），
                     可重复传入；用于 arm64 引擎产物（exec 与 JNI 两种形态）的 G-02 证据。
  --sample <NAME>    从 samples/manifest.json 加载样本三元组（in/out/report/key/ndjson），
                     NAME=all 时跑全部样本；--list-samples 列出可用样本。
  --self-test        用**内存中构造的合成夹具**把 V1~V7 / BI / RB 断言真跑一遍：
                     每条断言都同时配「必须 PASS 的正样本」与「必须变红的负样本」，
                     任一断言退化成「永远 PASS」→ 本模式退出码非 0。CI 硬门禁。

关于 report 顶层 root_key（密钥权威源）：
  引擎 build_report **会写** report 顶层 root_key —— 48 字符明文，与 key_spec 同源
  （engine/src/skroot_patch_main.cpp:420-429，task-8 起；同一份 root_key 派生，绝无第二
  份密钥），并同时写 root_key_contains_secret=true 标记。stdout 末行 ROOT_KEY=<48字符>
  只是兜底通道。
  因此密钥来源优先级为：`--key` 显式传入 > `report.root_key` 回落到自动读取。
  反之，若 --report 已给出但其中**没有**可用 root_key（旧引擎产物 / 字段丢失），则
  V2（密钥内嵌）/ V3（root_key 与 --key 一致）/ V6（前缀一致性）**无法校验**：本脚本
  会把它们记为 WARN 并在汇总里**单列计数**，绝不静默 SKIP 后仍汇总 PASS —— 「什么都没
  验」必须可见。

用法：
  python3 verify_patch.py --in <原厂内核> --out <修补内核> --report <report.json> \
      [--key <48位密钥>] [--ndjson <progress.ndjson>] [--quiet]
  python3 verify_patch.py --elf <arm64 ELF> [--elf <另一个 ELF>]
  python3 verify_patch.py --sample <NAME|all> [--manifest <path>]
  python3 verify_patch.py --list-samples [--manifest <path>]
  python3 verify_patch.py --self-test

退出码：0=全 PASS；1=存在 FAIL；2=用法/环境错误
"""

import argparse
import contextlib
import hashlib
import io
import json
import os
import shutil
import struct
import sys
import tempfile
import traceback

VERSION = "0.1"

ARM64_IMAGE_MAGIC = 0x644D5241
IMAGE_MAGIC_OFFSET = 0x38
IMAGE_SIZE_OFFSET = 0x10
PHASE_ENUM = {
    "read_input", "parse_kallsyms", "derive_offsets", "locate_init_cred",
    "patch_cfi_huawei", "patch_hooks", "embed_key", "write_output",
}
REQUIRED_HOOKS = ["do_execve", "filldir64", "compat_filldir", "avc_denied", "audit_log_start"]
KP_RESIDUE_MARKERS = [b"kpimg", b"KernelPatch", b"kpatch", b"supercall"]

results = []  # (check_id, status, message)
# 「未校验」台账：(check_id, 原因)。与 results 分离存放，便于汇总单列计数——
# 目的是让「什么都没验」这件事在结论里显形，而不是被 SKIP/PASS 混在一起看不见。
unverified = []


def record(cid, status, msg):
    results.append((cid, status, msg))


def record_unverified(cid, msg, reason):
    """记一条 WARN 并同时登记到未校验台账（供汇总单列计数）。"""
    record(cid, "WARN", msg)
    unverified.append((cid, reason))


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def read_bytes(path):
    with open(path, "rb") as f:
        return f.read()


def find_all(hay, needle):
    """返回 needle 在 hay 中出现的全部偏移。"""
    out, start = [], 0
    while True:
        i = hay.find(needle, start)
        if i < 0:
            return out
        out.append(i)
        start = i + 1


def diff_runs(a, b, block=1 << 16):
    """按块粗筛 + 块内精扫，返回 [(start, len)] 差异区段。"""
    runs = []
    n = min(len(a), len(b))
    for base in range(0, n, block):
        end = min(base + block, n)
        if a[base:end] == b[base:end]:
            continue
        cur_start, cur_len = None, 0
        for i in range(base, end):
            if a[i] != b[i]:
                if cur_start is None:
                    cur_start, cur_len = i, 1
                else:
                    cur_len += 1
            else:
                if cur_start is not None:
                    runs.append((cur_start, cur_len))
                    cur_start, cur_len = None, 0
        if cur_start is not None:
            runs.append((cur_start, cur_len))
    if len(a) != len(b):
        longer = max(len(a), len(b))
        if longer > n:
            runs.append((n, longer - n))
    return runs


def load_json(path, cid, label):
    try:
        with open(path, "r", encoding="utf-8") as f:
            data = json.load(f)
        return data
    except Exception as exc:  # noqa: BLE001 - 报告不可解析即判 FAIL，需原样输出原因
        record(cid, "FAIL", "%s 无法解析为 JSON：%s" % (label, exc))
        return None


def check_image_header(data):
    """V-07：arm64 Image 头可解析（原厂输入与产物都查一遍）。"""
    if len(data) < 64:
        record("V5", "FAIL", "产物长度 %d < 64，不是 arm64 Image" % len(data))
        return
    magic = int.from_bytes(data[IMAGE_MAGIC_OFFSET:IMAGE_MAGIC_OFFSET + 4], "little")
    image_size = int.from_bytes(data[IMAGE_SIZE_OFFSET:IMAGE_SIZE_OFFSET + 8], "little")
    if magic != ARM64_IMAGE_MAGIC:
        record("V5", "FAIL",
               "偏移 0x38 魔数 0x%08X != 0x%08X（非 arm64 Image，或仍是 .gz/.lz4 压缩体）"
               % (magic, ARM64_IMAGE_MAGIC))
        return
    if image_size == len(data):
        record("V5", "PASS", "arm64 Image 头合法，image_size==file_size==%d" % len(data))
    elif image_size < len(data):
        record("V5", "WARN", "arm64 Image 头合法，但文件比 image_size 长 %d 字节（尾部填充，需人工确认）"
               % (len(data) - image_size))
    else:
        over = image_size - len(data)
        pct = over * 100.0 / max(image_size, 1)
        status = "WARN" if pct <= 5.0 else "FAIL"
        record("V5", status,
               "arm64 Image 头 image_size=%d 大于实际文件 %d（差 %d B，%.2f%%）%s"
               % (image_size, len(data), over, pct,
                  "：厂商可能裁掉尾部零填充，需按 kernel_size/真机复核" if status == "WARN" else "：镜像被截断"))


def check_report(report, sha_in, size_in, sha_out, size_out, key):
    """V-08/V-09（含 hook 表与报告自洽）。返回 (root_key, hidden_prefix, root_key_region)。"""
    root_key = report.get("root_key")
    key_spec = report.get("key_spec") or {}
    hidden_prefix = report.get("hidden_dir_prefix")

    for field in ("engine_version", "input", "output", "hook_table"):
        if field not in report:
            record("V3", "FAIL", "report 缺字段 %s" % field)
            return root_key, hidden_prefix, None
    if not report.get("engine_version"):
        record("V3", "FAIL", "report.engine_version 为空")
        return root_key, hidden_prefix, None

    ok = True
    r_in = report.get("input") or {}
    r_out = report.get("output") or {}
    if r_in.get("sha256") != sha_in:
        ok = False
        record("V3", "FAIL", "report.input.sha256 与实际输入不符")
    if int(r_in.get("size", -1)) != size_in:
        ok = False
        record("V3", "FAIL", "report.input.size 与实际输入不符")
    if r_out.get("sha256") != sha_out:
        ok = False
        record("V3", "FAIL", "report.output.sha256 与实际产物不符")
    if int(r_out.get("size", -1)) != size_out:
        ok = False
        record("V3", "FAIL", "report.output.size 与实际产物不符")
    if ok:
        record("V3", "PASS", "report 输入/输出 sha256 与 size 自洽")

    if key:
        expect_spec = {"charset": "A-Za-z0-9", "length": 48, "stored_len": 47}
        bad = [k for k, v in expect_spec.items() if key_spec.get(k) != v]
        if bad:
            record("V3", "FAIL", "report.key_spec 与契约不符（期望 %s，实际 %s）" % (expect_spec, key_spec))
        else:
            record("V3", "PASS", "report.key_spec 符合 [A-Za-z0-9]{48}/stored_len=47")
        if root_key and root_key != key:
            record("V3", "FAIL", "report.root_key 与 --key 不一致")
        if hidden_prefix is not None:
            if hidden_prefix == key[:16]:
                record("V6", "PASS", "hidden_dir_prefix == root_key[0:16]")
            else:
                record("V6", "FAIL", "hidden_dir_prefix(%r) != root_key[0:16](%r)"
                       % (hidden_prefix, key[:16]))
    else:
        # 引擎 build_report 自 task-8 起会写顶层 root_key，所以「走到这里」=
        # 要么是旧引擎产物、要么字段被删/为空。此时 V3 的 root_key 一致性与 V6 前缀
        # 一致性**无从校验**；记 WARN 并登记未校验台账，避免静默 SKIP 后仍整体 PASS。
        record_unverified(
            "V6",
            "无法校验 hidden_dir_prefix：report 缺可用 root_key 且未传 --key"
            "（report 由 task-8 前的引擎产出或字段丢失；前缀一致性本次未验证）",
            "report 无可用 root_key → hidden_dir_prefix == root_key[0:16] 未比对")

    hooks = report.get("hook_table") or []
    detail = report.get("hooks") or []
    names = {h.get("name"): h for h in hooks if isinstance(h, dict)}
    root_key_region = None

    def _addr_ok(v):
        if isinstance(v, int):
            return v > 0
        if isinstance(v, str):
            return len(v.strip()) > 0
        return False

    # 优先：顶层 hooks[] 明细（P1 修复后，2026-11-08）→ 强断言
    if isinstance(detail, list) and detail:
        by_group = {}
        for h in detail:
            if isinstance(h, dict) and h.get("group"):
                by_group.setdefault(h["group"], []).append(h)
        problems = []
        for grp in REQUIRED_HOOKS:
            ent = by_group.get(grp)
            if not ent:
                problems.append("%s:缺失" % grp)
                continue
            for e in ent:
                if e.get("resolved") is True:
                    if not _addr_ok(e.get("hook_addr")):
                        problems.append("%s:hook_addr空" % grp)
                    if not isinstance(e.get("bytes"), int) or e.get("bytes") <= 0:
                        problems.append("%s:bytes无效" % grp)
                    if e.get("patch_offset") in (None, ""):
                        problems.append("%s:patch_offset缺" % grp)
                else:
                    if not (e.get("reason") or "").strip():
                        problems.append("%s:resolved=false但无reason" % grp)
        em = by_group.get("embed_key") or []
        if not em:
            problems.append("embed_key:缺失")
        n_true = sum(1 for h in detail if h.get("resolved") is True)
        n_false = sum(1 for h in detail if h.get("resolved") is False)
        if problems:
            record("V4", "FAIL", "hooks[] 强断言失败：%s（共 %d 组，resolved %d/%d）"
                   % (problems[:8], len(by_group), n_true, n_false))
        else:
            record("V4", "PASS", "hooks[] 明细齐全（%d 组，resolved true=%d / false=%d；五组必备 hook 地址与 bytes 有效，false 组均有 reason）"
                   % (len(by_group), n_true, n_false))
        # embed_key.patch_offset == root_key_region（若都提供）
        for e in em:
            po = e.get("patch_offset")
            try:
                po_i = int(po, 0) if isinstance(po, str) else int(po)
            except (TypeError, ValueError):
                po_i = None
            if po_i is not None:
                root_key_region = root_key_region or ("0x%X" % po_i)
                record("V4", "PASS", "hooks.embed_key.patch_offset=%s（供密钥落位校对）" % hex(po_i))
                break
        return root_key, hidden_prefix, root_key_region

    agg = names.get("hooks_embedded")
    if agg:
        blob = agg.get("blob_count")
        root_key_region = agg.get("root_key_region")
        if isinstance(blob, int) and blob >= 5:
            record("V4", "WARN",
                   "hook_table 为聚合形态（hooks_embedded: blob_count=%s, root_key_region=%s）——"
                   "缺逐 hook 落位/地址，属 C1 §2.3 契约偏差（需引擎补报告或契约对齐）；"
                   "不单独阻断，V-05 改用 root_key_region 精确核对" % (blob, root_key_region))
        else:
            record("V4", "FAIL", "hook_table 聚合形态 blob_count=%r < 5（疑似织入不足）" % blob)
    else:
        missing = [n for n in REQUIRED_HOOKS if n not in names]
        zero = [n for n in REQUIRED_HOOKS if n in names and not _addr_ok(names[n].get("hook_addr"))]
        if missing:
            record("V4", "FAIL", "hook_table 缺 hook：%s" % ", ".join(missing))
        elif zero:
            record("V4", "FAIL", "hook_table 中这些 hook 无有效地址：%s" % ", ".join(zero))
        else:
            record("V4", "PASS", "hook_table 五组必备 hook 齐全且地址非空")
    return root_key, hidden_prefix, root_key_region


def check_key_embedding(out, key, hooks, root_key_region=None):
    """V-04 密钥内嵌窗口；V-05 落位（弱断言）。"""
    window = key[:47].encode("ascii") + b"\x00"
    hits = find_all(out, window)
    if not hits:
        record("V2", "FAIL", "产物中找不到 %d 字节密钥窗口（key[0..46]+NUL）" % len(window))
        return
    record("V2", "PASS", "产物中存在 %d 字节密钥窗口，出现 %d 次，偏移 %s"
           % (len(window), len(hits), [hex(i) for i in hits[:5]]))
    # V-05a：优先用 report.root_key_region（引擎聚合形态给出的密钥区文件偏移）精确核对
    if root_key_region is not None:
        try:
            off = int(root_key_region, 0) if isinstance(root_key_region, str) else int(root_key_region)
        except (TypeError, ValueError):
            off = None
        if off is not None:
            if any(hit == off for hit in hits):
                record("V2", "PASS", "密钥窗口偏移 == report.root_key_region（%s），落位一致" % hex(off))
            else:
                record("V2", "FAIL", "密钥窗口偏移 %s 与 report.root_key_region %s 不一致"
                       % ([hex(h) for h in hits], hex(off)))
            return
    # V-05：弱断言，仅当 report 提供 file_offset 时才可精确核对
    offsets = []
    for h in hooks:
        if not isinstance(h, dict):
            continue
        for f in ("file_offset", "trampoline_file_offset"):
            v = h.get(f)
            if isinstance(v, int):
                offsets.append((h.get("name"), v))
    if offsets:
        near = [(n, o) for n, o in offsets if any(abs(hit - o) <= 512 for hit in hits)]
        if near:
            record("V2", "PASS", "密钥窗口落在 %s 跳板头部 512B 内（file_offset=%s）"
                   % (near[0][0], hex(near[0][1])))
        else:
            record("V2", "FAIL", "密钥窗口距全部 hook file_offset 均 >512B，疑似不在跳板头部")
    else:
        record("V2", "WARN",
               "report 未提供 hook file_offset，密钥落位需人工复核一次（反汇编确认窗口位于跳板头部）")


def check_diff(in_bytes, out_bytes):
    """V-13 改动区段受控。"""
    runs = diff_runs(in_bytes, out_bytes)
    total = sum(r[1] for r in runs)
    if not runs:
        record("V4", "FAIL", "in 与 out 逐字节相同，未发生任何 patch")
        return
    record("V4", "PASS", "差异区段 %d 处，总改动 %d 字节；前 8 处：%s"
           % (len(runs), total, [(hex(s), l) for s, l in runs[:8]]))
    if len(runs) < 5:
        record("V4", "WARN", "差异区段仅 %d 处（契约期望五组 hook ≥5 处，请核对）" % len(runs))
    seem = [(s, l) for s, l in runs if l > 512]
    if seem:
        record("V4", "FAIL", "存在 >512B 的连续改动区段 %s（疑似大段重写，违反纯字节 patch）"
               % [(hex(s), l) for s, l in seem[:5]])
    if total > 256 * 1024:
        record("V4", "FAIL", "总改动 %d 字节 > 256KB 阈值" % total)
    else:
        record("V4", "PASS", "总改动 %d 字节在阈值内（≤256KB）" % total)


def check_ndjson(path):
    """V-10 NDJSON 合法 + 终态事件唯一且在最后。"""
    events, bad = [], []
    with open(path, "r", encoding="utf-8", errors="replace") as f:
        for lineno, line in enumerate(f, 1):
            line = line.strip()
            if not line:
                continue
            try:
                ev = json.loads(line)
            except json.JSONDecodeError as exc:
                bad.append("第 %d 行非合法 JSON：%s" % (lineno, exc))
                continue
            events.append(ev)
    if bad:
        record("V6", "FAIL", "NDJSON 解析失败 %d 行；首条：%s" % (len(bad), bad[0]))
        return
    if not events:
        record("V6", "FAIL", "NDJSON 为空")
        return
    phases = [e.get("phase") for e in events if e.get("event") == "progress"]
    unknown = sorted({p for p in phases if p not in PHASE_ENUM})
    if unknown:
        record("V6", "FAIL", "出现未定义 phase：%s" % unknown)
        return
    result_evs = [i for i, e in enumerate(events) if e.get("event") == "result"]
    if not result_evs:
        record("V6", "FAIL", "NDJSON 无 result 终态事件")
    elif result_evs[-1] != len(events) - 1:
        record("V6", "FAIL", "result 终态事件不在最后一条")
    elif len(result_evs) > 1:
        record("V6", "FAIL", "result 终态事件出现 %d 次（应唯一）" % len(result_evs))
    else:
        code = events[-1].get("code")
        record("V6", "PASS", "NDJSON 合法：%d 事件，终态 result.code=%r" % (len(events), code))


def check_kp_residue(out_bytes, apk_scan=False):
    """V-17 KP 残留（弱断言：字串命中需人工确认，不直接判 FAIL）。"""
    hits = {m.decode(): out_bytes.count(m) for m in KP_RESIDUE_MARKERS if out_bytes.count(m)}
    if hits:
        record("V7", "WARN", "产物命中 KP 特征字串 %s（可能是原厂数据巧合，需人工确认）" % hits)
    else:
        record("V7", "PASS", "产物未命中 kpimg/preset/supercall 特征字串")


# --------------------------------------------------------------------------
# ELF 产物断言（G-02 门禁证据：arm64 引擎 exec / JNI 两种形态）
# --------------------------------------------------------------------------
PT_LOAD, PT_DYNAMIC, PT_INTERP, PT_TLS = 1, 2, 3, 7
DT_NULL, DT_NEEDED, DT_STRTAB = 0, 1, 5
EM_AARCH64, EM_X86_64 = 0xB7, 0x3E


def _parse_elf(data):
    if len(data) < 64 or data[:4] != b"\x7fELF":
        raise ValueError("不是 ELF（magic 不符）")
    if data[4] != 2:
        raise ValueError("不是 ELF64（ei_class=%d）" % data[4])
    if data[5] != 1:
        raise ValueError("不是小端 ELF（ei_data=%d）" % data[5])
    e_type = struct.unpack_from("<H", data, 16)[0]
    e_machine = struct.unpack_from("<H", data, 18)[0]
    e_phoff = struct.unpack_from("<Q", data, 32)[0]
    e_phentsize = struct.unpack_from("<H", data, 54)[0]
    e_phnum = struct.unpack_from("<H", data, 56)[0]
    if e_phentsize < 56:
        raise ValueError("e_phentsize=%d 异常" % e_phentsize)
    phdrs = []
    interp = None
    for i in range(e_phnum):
        off = e_phoff + i * e_phentsize
        if off + 56 > len(data):
            raise ValueError("程序头越界（第 %d 个）" % i)
        p_type, _p_flags, p_off, p_vaddr, _p_paddr, p_filesz, p_memsz, p_align = \
            struct.unpack_from("<IIQQQQQQ", data, off)
        if p_type == PT_INTERP and p_filesz:
            interp = data[p_off:p_off + p_filesz].split(b"\x00")[0].decode("utf-8", "replace")
        phdrs.append({"type": p_type, "offset": p_off, "vaddr": p_vaddr,
                      "filesz": p_filesz, "memsz": p_memsz, "align": p_align})
    return {"type": e_type, "machine": e_machine, "phdrs": phdrs, "interp": interp}


def _dynamic_needed(data, phdrs):
    dyn = [p for p in phdrs if p["type"] == PT_DYNAMIC]
    if not dyn:
        return []
    entries, off = [], dyn[0]["offset"]
    for _ in range(dyn[0]["filesz"] // 16):
        if off + 16 > len(data):
            break
        tag, val = struct.unpack_from("<QQ", data, off)
        entries.append((tag, val))
        off += 16
        if tag == DT_NULL:
            break
    strtab = next((val for tag, val in entries if tag == DT_STRTAB), None)

    def v2o(v):
        for p in phdrs:
            if p["type"] == PT_LOAD and p["vaddr"] <= v < p["vaddr"] + p["filesz"]:
                return p["offset"] + (v - p["vaddr"])
        return None

    needed = []
    for tag, val in entries:
        if tag != DT_NEEDED or strtab is None:
            continue
        o = v2o(strtab + val)
        if o is None:
            needed.append("<unresolved>")
            continue
        end = data.find(b"\x00", o)
        needed.append(data[o:end].decode("ascii", "replace") if end > o else "<unresolved>")
    return needed


def _elf_symbols(data, phdrs):
    """解析 .dynsym/.dynstr 导出符号名（经 section headers）。返回 set。"""
    if len(data) < 64:
        return set()
    e_shoff = struct.unpack_from("<Q", data, 40)[0]
    e_shentsize = struct.unpack_from("<H", data, 58)[0]
    e_shnum = struct.unpack_from("<H", data, 60)[0]
    e_shstrndx = struct.unpack_from("<H", data, 62)[0]
    if not e_shoff or not e_shnum or e_shentsize < 64:
        return set()
    secs = []
    for i in range(e_shnum):
        off = e_shoff + i * e_shentsize
        if off + 64 > len(data):
            break
        sh_name, sh_type = struct.unpack_from("<II", data, off)
        sh_offset, sh_size = struct.unpack_from("<QQ", data, off + 24)
        sh_link = struct.unpack_from("<I", data, off + 40)[0]
        sh_entsize = struct.unpack_from("<Q", data, off + 56)[0]
        secs.append({"name_off": sh_name, "type": sh_type, "offset": sh_offset,
                     "size": sh_size, "link": sh_link, "entsize": sh_entsize})
    if e_shstrndx >= len(secs):
        return set()
    shstr = data[secs[e_shstrndx]["offset"]:secs[e_shstrndx]["offset"] + secs[e_shstrndx]["size"]]
    for sec in secs:
        n_end = shstr.find(b"\x00", sec["name_off"])
        name = shstr[sec["name_off"]:n_end].decode("ascii", "replace") if n_end > 0 else ""
        if name != ".dynsym":
            continue
        strtab = secs[sec["link"]] if sec["link"] < len(secs) else None
        if not strtab:
            continue
        strs = data[strtab["offset"]:strtab["offset"] + strtab["size"]]
        out = set()
        entsize = sec["entsize"] or 24
        for j in range(sec["size"] // entsize):
            so = sec["offset"] + j * entsize
            if so + 24 > len(data):
                break
            st_name = struct.unpack_from("<I", data, so)[0]
            end = strs.find(b"\x00", st_name)
            if st_name and end > 0:
                out.add(strs[st_name:end].decode("ascii", "replace"))
        return out
    return set()


def check_elf(path, expect_symbols=None, elf_form=None):
    if not os.path.isfile(path):
        record("ELF", "FAIL", "文件不存在：%s" % path)
        return
    data = read_bytes(path)
    name = os.path.basename(path)
    sha = hashlib.sha256(data).hexdigest()
    mode = os.stat(path).st_mode & 0o777
    print("\n[ELF] %s (%d B) mode=%03o sha256=%s" % (path, len(data), mode, sha))
    try:
        info = _parse_elf(data)
    except ValueError as exc:
        record("ELF", "FAIL", "%s：%s" % (name, exc))
        return
    # ELF1 架构
    if info["machine"] == EM_AARCH64:
        record("ELF1", "PASS", "%s：ELF64-LSB AArch64（e_machine=0xB7）" % name)
    elif info["machine"] == EM_X86_64:
        record("ELF1", "FAIL", "%s：e_machine=0x3E(x86-64)，**不能**在 Android arm64 上运行" % name)
    else:
        record("ELF1", "FAIL", "%s：e_machine=0x%X 非 AArch64(0xB7)" % (name, info["machine"]))
    # ELF2 静态性 / 依赖
    interp = [p for p in info["phdrs"] if p["type"] == PT_INTERP]
    dyn = [p for p in info["phdrs"] if p["type"] == PT_DYNAMIC]
    tls = [p for p in info["phdrs"] if p["type"] == PT_TLS]
    needed = _dynamic_needed(data, info["phdrs"])
    etype = {2: "ET_EXEC", 3: "ET_DYN(PIE)"}.get(info["type"], "0x%X" % info["type"])
    is_jni = (elf_form == "jni") or (name.endswith(".so") and "jni" in name.lower())
    ipath = info.get("interp")
    android_interp = bool(ipath) and ("linker" in ipath)
    if not interp and not dyn:
        record("ELF2", "PASS", "%s：静态可执行（无 PT_INTERP / 无 PT_DYNAMIC），etype=%s" % (name, etype))
    elif android_interp:
        record("ELF2", "PASS",
               "%s：Android 动态可执行，interp=%s，NEEDED=%s（目标机自带 linker，正常；非“全静态”但可运行）"
               % (name, ipath, needed or "[]"))
    elif not interp:
        status = "PASS" if is_jni else "WARN"
        record("ELF2", status, "%s：无 PT_INTERP，含 PT_DYNAMIC，NEEDED=%s%s"
               % (name, needed or "[]", "（.so 由进程加载，可接受）" if is_jni else "（exec 形态不含解释器但含动态段，需确认）"))
    else:
        record("ELF2", "FAIL" if not is_jni else "WARN",
               "%s：含 **宿主** 解释器 PT_INTERP=%s（非 Android linker），不能在 Android 上执行" % (name, ipath))
    # ELF3 可执行位（exec 形态要求；JNI .so 不要求）
    if is_jni:
        record("ELF3", "PASS", "%s：mode=%03o（JNI .so 由 dlopen 加载，不要求可执行位）" % (name, mode))
    elif mode & 0o111:
        record("ELF3", "PASS", "%s：mode=%03o 含可执行位" % (name, mode))
    else:
        record("ELF3", "FAIL", "%s：mode=%03o 无可执行位（exec 形态不可运行）" % (name, mode))
    # ELF4 TLS 对齐（信息项：静态 C++ 常见 p_align=0x40）
    if tls:
        record("ELF4", "PASS", "%s：PT_TLS 存在，p_align=0x%X（信息项）" % (name, tls[0]["align"]))
    else:
        record("ELF4", "PASS", "%s：无 PT_TLS（信息项，不阻断）" % name)
    # ELF5 导出符号（JNI：必须含 Java_* 入口）
    if expect_symbols:
        syms = _elf_symbols(data, info["phdrs"])
        missing = [x for x in expect_symbols if x not in syms]
        if missing:
            record("ELF5", "FAIL", "%s：缺少必需导出符号 %s（System.loadLibrary/调用会失败）" % (name, missing))
        else:
            record("ELF5", "PASS", "%s：导出符号齐全 %s" % (name, expect_symbols))
    # ELF6 形态（jni=ET_DYN 且非静态；exec=静态 ET_EXEC 不可被 dlopen 依赖）
    if elf_form == "jni":
        if info["type"] != 3:
            record("ELF6", "FAIL", "%s：JNI 形态要求 ET_DYN（实际 etype=0x%X）" % (name, info["type"]))
        elif not dyn:
            record("ELF6", "FAIL", "%s：JNI 形态但无 PT_DYNAMIC（无法被 dlopen 正常加载）" % name)
        else:
            record("ELF6", "PASS", "%s：JNI 形态正确（ET_DYN + PT_DYNAMIC，loadLibrary 前提成立）" % name)
    elif elf_form == "exec":
        if info["type"] != 2 or interp or dyn:
            record("ELF6", "FAIL", "%s：exec 形态要求静态 ET_EXEC（实际 etype=0x%X, INTERP=%s, DYNAMIC=%s）"
                   % (name, info["type"], bool(interp), bool(dyn)))
        else:
            record("ELF6", "PASS", "%s：exec 形态正确（静态 ET_EXEC，可独立执行、不依赖 Android linker）" % name)
    print("      etype=%s  PT_INTERP=%s  PT_DYNAMIC=%s  NEEDED=%s  sha256=%s"
          % (etype, bool(interp), bool(dyn), needed or "[]", sha))


# --------------------------------------------------------------------------
# 样本集加载（task-10：samples/manifest.json）
# 清单格式：
# {"schema":1,"samples":[{"id":"meizu21-stock","in":".../kernel","out":".../kernel.patched",
#   "report":".../report.json","ndjson":"...","key":null,"note":"来源/版本"}]}
# 路径可为相对（相对清单文件所在目录）或绝对。
# --------------------------------------------------------------------------
def _repo_root():
    return os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def default_manifest_path():
    return os.path.join(_repo_root(), "samples", "manifest.json")


def load_manifest(path):
    with open(path, "r", encoding="utf-8") as f:
        manifest = json.load(f)
    samples = manifest.get("samples")
    if not isinstance(samples, list) or not samples:
        raise ValueError("manifest 里没有 samples[] 条目")
    return manifest, os.path.dirname(os.path.abspath(path))


def _resolve(base, rel):
    if not rel:
        return None
    return rel if os.path.isabs(rel) else os.path.normpath(os.path.join(base, rel))


def list_samples(manifest_path):
    manifest, base = load_manifest(manifest_path)
    print("== 样本清单：%s ==" % manifest_path)
    print("%-24s %-10s %s" % ("id", "文件齐备", "note"))
    for s in manifest["samples"]:
        sid = str(s.get("id", "?"))
        paths = [_resolve(base, s.get(k)) for k in ("bootimg", "in", "out", "report", "ndjson")]
        missing = [k for k, v in zip(("bootimg", "in", "out", "report", "ndjson"), paths)
                   if v and not os.path.isfile(v)]
        flag = "OK" if not missing else ("缺 " + ",".join(missing))
        print("%-24s %-10s %s" % (sid, flag, s.get("note", "")))
    print("路径基准：%s" % base)
    return 0


def resolve_samples(manifest_path, selector):
    manifest, base = load_manifest(manifest_path)
    wanted = [s for s in manifest["samples"] if selector == "all" or str(s.get("id")) == selector]
    if not wanted:
        raise ValueError("清单中没有样本 id=%r（可用 --list-samples 查看）" % selector)
    return [{
        "id": str(s.get("id")),
        "bootimg": _resolve(base, s.get("bootimg")),
        "in": _resolve(base, s.get("in")),
        "out": _resolve(base, s.get("out")),
        "report": _resolve(base, s.get("report")),
        "ndjson": _resolve(base, s.get("ndjson")),
        "key": s.get("key"),
        "note": s.get("note", ""),
    } for s in wanted]


# --------------------------------------------------------------------------
# 配对校验主体
# --------------------------------------------------------------------------
def verify_pair(inp, outp, report_path=None, key=None, ndjson=None, quiet=False):
    for p in (inp, outp):
        if not p or not os.path.isfile(p):
            print("ERROR: 文件不存在：%s" % p, file=sys.stderr)
            return 2
    if report_path and not os.path.isfile(report_path):
        print("ERROR: report 不存在：%s" % report_path, file=sys.stderr)
        return 2
    if ndjson and not os.path.isfile(ndjson):
        print("ERROR: ndjson 不存在：%s" % ndjson, file=sys.stderr)
        return 2

    in_bytes = read_bytes(inp)
    out_bytes = read_bytes(outp)
    sha_in, sha_out = hashlib.sha256(in_bytes).hexdigest(), hashlib.sha256(out_bytes).hexdigest()

    print("== SPRoot 修补产物静态验证 v%s ==" % VERSION)
    print("in : %s (%d B) sha256=%s" % (inp, len(in_bytes), sha_in))
    print("out: %s (%d B) sha256=%s" % (outp, len(out_bytes), sha_out))

    # ---- V1 等长 / 确实改动 ----
    if len(in_bytes) != len(out_bytes):
        record("V1", "FAIL", "产物与原厂不等长：in=%d out=%d（SK 产物必须严格等长）"
               % (len(in_bytes), len(out_bytes)))
    else:
        record("V1", "PASS", "in/out 等长（%d B）" % len(in_bytes))
    if sha_in == sha_out:
        record("V1", "FAIL", "in/out sha256 相同，未产生任何改动")
    else:
        record("V1", "PASS", "in/out sha256 不同，产物确有改动")

    # ---- V5 Image 头（原厂输入与产物都必须可解析，防「拿压缩体当输入」） ----
    print("  [V5-in ] 原厂输入头：")
    check_image_header(in_bytes)
    print("  [V5-out] 修补产物头：")
    check_image_header(out_bytes)

    # ---- report ----
    hooks = []
    root_region = None
    report_loaded = False
    if report_path:
        report = load_json(report_path, "V3", "report")
        if report is not None:
            report_loaded = True
            root_key, _, root_region = check_report(report, sha_in, len(in_bytes), sha_out, len(out_bytes),
                                                    key or report.get("root_key"))
            if not key:
                key = root_key
            hooks = report.get("hook_table") or []

    # ---- 密钥 ----
    if key:
        print("key: 已提供 %d 字符（sha256[:8]=%s，明文不落屏）"
              % (len(key), hashlib.sha256(key.encode()).hexdigest()[:8]))
        if len(key) != 48 or any(not (c.isascii() and c.isalnum()) for c in key):
            record("V2", "FAIL", "密钥格式应为 [A-Za-z0-9]{48}，实际长度 %d" % len(key))
        else:
            check_key_embedding(out_bytes, key, hooks, root_region)
    elif report_loaded:
        # 有 report 却没有可用 root_key：引擎自 task-8 起必写该字段，走到这里说明
        # report 来自旧引擎或字段被删/为空。V2 密钥内嵌**本次无从校验**，必须可见，
        # 不能静默 SKIP 后让汇总整体 PASS（空断言隐身）。
        record_unverified(
            "V2",
            "密钥内嵌未校验：report 存在但没有可用 root_key，且未传 --key"
            "（本次无法确认 48 字节密钥窗口是否落进产物）",
            "report 无可用 root_key → 产物中 47+NUL 密钥窗口未查找")
    else:
        record("V2", "SKIP", "未提供 report 也未传 --key，跳过密钥内嵌检查")

    # ---- 差异区段 ----
    if len(in_bytes) == len(out_bytes):
        check_diff(in_bytes, out_bytes)

    # ---- NDJSON ----
    if ndjson:
        check_ndjson(ndjson)

    # ---- KP 残留 ----
    check_kp_residue(out_bytes)
    return print_summary(quiet)


def print_summary(quiet=False):
    print("\n-- 断言明细 --")
    for cid, status, msg in results:
        if quiet and status == "PASS":
            continue
        print("[%s] %-5s %s" % (cid, status, msg))
    fails = [r for r in results if r[1] == "FAIL"]
    warns = [r for r in results if r[1] == "WARN"]
    skips = [r for r in results if r[1] == "SKIP"]
    print("\n汇总：PASS=%d WARN=%d SKIP=%d FAIL=%d"
          % (len(results) - len(fails) - len(warns) - len(skips), len(warns), len(skips), len(fails)))
    # 「什么都没验」必须可见：单列未校验断言，避免沉默 SKIP 被汇总 PASS 掩盖。
    if unverified:
        ids = sorted({cid for cid, _ in unverified})
        print("⚠ 未校验断言 %d 项（%s）：以下断言本次**没有执行实质比对**，"
              "不得据此判定对应契约成立：" % (len(unverified), ", ".join(ids)))
        for cid, why in unverified:
            print("   - [%s] %s" % (cid, why))
    if fails:
        print("判定：FAIL（阻断，不可进入刷入序列）")
        return 1
    if unverified:
        print("判定：PASS（WARN 项需人工复核后归档；**但上列断言未校验，"
              "相应证据缺失，不可用于归档 G-02 密钥链结论**）")
        return 0
    print("判定：PASS（WARN 项需人工复核后归档）")
    return 0


# --------------------------------------------------------------------------
# Android boot.img 头断言（原始 boot.img 基线；样本 samples/meizu21/boot.img）
# 参考 AOSP bootimg.h：magic "ANDROID!"、kernel_size@0x08、ramdisk_size@0x10、
# page_size@0x24、header_version@0x28；v1/v2 追加字段；v3/v4 头布局不同。
# --------------------------------------------------------------------------
BOOT_MAGIC = b"ANDROID!"
VENDOR_BOOT_MAGIC = b"VNDRBOOT"
GZIP_MAGIC = b"\x1f\x8b"
LZ4_MAGIC = b"\x04\x22\x4d\x18"


def _u32(data, off):
    return struct.unpack_from("<I", data, off)[0]


def check_bootimg(path):
    if not os.path.isfile(path):
        record("BI", "FAIL", "文件不存在：%s" % path)
        return
    data = read_bytes(path)
    size = len(data)
    sha = hashlib.sha256(data).hexdigest()
    name = os.path.basename(path)
    print("\n[BOOTIMG] %s (%d B) sha256=%s" % (path, size, sha))
    if size < 0x660:
        record("BI1", "FAIL", "%s：文件过小（%d B），不是 boot.img" % (name, size))
        return
    # BI1 magic
    if data[:8] == VENDOR_BOOT_MAGIC:
        record("BI1", "FAIL", "%s：这是 vendor_boot.img（VNDRBOOT），不是 boot.img" % name)
        return
    if data[:8] != BOOT_MAGIC:
        record("BI1", "FAIL", "%s：magic=%r != b'ANDROID!'（不是 Android bootimg）" % (name, data[:8]))
        return
    record("BI1", "PASS", "%s：magic=ANDROID!（header 前 8B 可读）" % name)

    # BI2 header version / page size
    hdr_ver = _u32(data, 0x28)
    if hdr_ver >= 3:
        header_size = _u32(data, 0x14)
        kernel_size, ramdisk_size = _u32(data, 0x08), _u32(data, 0x0C)
        page_size = 4096
        kv = "v%d" % hdr_ver
    else:
        page_size = _u32(data, 0x24)
        kernel_size, ramdisk_size = _u32(data, 0x08), _u32(data, 0x10)
        header_size = _u32(data, 0x66C) if hdr_ver >= 1 else 0x660
        kv = "v%d" % hdr_ver
    if page_size not in (2048, 4096, 8192, 16384):
        record("BI2", "FAIL", "%s：page_size=%d 非 2K/4K/8K/16K 合法值（%s）" % (name, page_size, kv))
        return
    expect_hs = {3: {0x600}, 4: {0x630}}.get(hdr_ver)
    if expect_hs and header_size not in expect_hs:
        record("BI2", "WARN", "%s：header=%s 的 header_size=0x%X 非预期值 %s（仍以实际值计算偏移）"
               % (name, kv, header_size, [hex(v) for v in sorted(expect_hs)]))
    elif hdr_ver < 3 and not (0x660 <= header_size <= page_size):
        record("BI2", "WARN", "%s：header_size=0x%X 与 page_size=%d 关系异常（%s）"
               % (name, header_size, page_size, kv))
    elif header_size > page_size:
        record("BI2", "FAIL", "%s：header_size=0x%X > page_size(%d)，头部超一页，分区布局异常"
               % (name, header_size, page_size))
    else:
        record("BI2", "PASS", "%s：header=%s，header_size=0x%X，page_size=%d"
               % (name, kv, header_size, page_size))

    # BI3 kernel 段范围
    koff = page_size * ((header_size + page_size - 1) // page_size)
    if kernel_size == 0:
        if ramdisk_size > 0:
            record("BI3", "PASS", "%s：kernel_size=0、ramdisk_size=%d → **init_boot 形态**（无内核段，不参与 Kernel Image 断言）"
                   % (name, ramdisk_size))
            record("BI6", "PASS", "%s：%s header，page_size=%d，kernel_off=0x%X"
                   % (name, kv, page_size, koff))
            return
        record("BI3", "FAIL", "%s：kernel_size=0 且 ramdisk_size=0（既无内核也无 ramdisk）" % name)
        return
    if koff + kernel_size > size:
        record("BI3", "FAIL", "%s：kernel 段越界（off=0x%X size=%d > file=%d）"
               % (name, koff, kernel_size, size))
        return
    kernel = data[koff:koff + kernel_size]
    ksha = hashlib.sha256(kernel).hexdigest()
    record("BI3", "PASS", "%s：kernel 段 off=0x%X size=%d sha256=%s"
           % (name, koff, kernel_size, ksha))

    # BI4 kernel 是否未压缩 arm64 Image（决定能否直接喂引擎，TBD-8）
    if len(kernel) >= 0x3C:
        magic = int.from_bytes(kernel[0x38:0x3C], "little")
        img_size = int.from_bytes(kernel[0x10:0x18], "little")
        if magic == ARM64_IMAGE_MAGIC:
            record("BI4", "PASS", "%s：kernel 为**未压缩 arm64 Image**（0x38 魔数 OK，image_size=%d）"
                   % (name, img_size))
        elif kernel[:2] == GZIP_MAGIC:
            record("BI4", "WARN", "%s：kernel 是 **gzip 压缩体**（'Image.gz'），喂引擎前必须解压（TBD-8）" % name)
        elif kernel[:4] == LZ4_MAGIC:
            record("BI4", "WARN", "%s：kernel 是 **lz4 压缩体**，喂引擎前必须解压（TBD-8）" % name)
        else:
            record("BI4", "FAIL", "%s：kernel 段既非 arm64 Image 也非 gzip/lz4（magic=0x%08X）"
                   % (name, magic))
    else:
        record("BI4", "FAIL", "%s：kernel 段长度 %d < 0x3C，无法校验 Image 头" % (name, len(kernel)))

    # BI5 文件页对齐 / 分区式整镜像
    aligned = (size % page_size == 0)
    if aligned:
        record("BI5", "PASS", "%s：文件大小 %d 按 page_size=%d 对齐（%d 页）"
               % (name, size, page_size, size // page_size))
    else:
        record("BI5", "WARN", "%s：文件大小 %d 未按 page_size=%d 对齐（尾部余 %d B）"
               % (name, size, page_size, size % page_size))
    # BI6 其余段信息
    info = ["ramdisk_size=%d" % ramdisk_size, "kernel_off=0x%X" % koff,
            "os_version=0x%08X" % _u32(data, 0x10 if hdr_ver >= 3 else 0x2C)]
    if hdr_ver >= 2 and size > 0x674:
        info.append("dtb_size=%d" % _u32(data, 0x670))
    print("      %s  %s" % (kv, "  ".join(info)))
    record("BI6", "PASS", "%s：%s" % (name, "  ".join(info)))


def _avb_footer(data):
    """解析 AVB footer（末 64 字节，全大端）。返回 dict 或 None。"""
    if len(data) < 64:
        return None
    t = data[-64:]
    if t[:4] != b"AVBf":
        return None
    vmaj, vmin = struct.unpack_from(">II", t, 4)
    orig, voff, vsize = struct.unpack_from(">QQQ", t, 12)
    if voff + vsize > len(data):
        return {"magic": "AVBf", "bad_range": True, "vbmeta_offset": voff, "vbmeta_size": vsize}
    return {"magic": "AVBf", "version": "%d.%d" % (vmaj, vmin), "original_image_size": orig,
            "vbmeta_offset": voff, "vbmeta_size": vsize, "footer": t}


def _boot_segments(data, page_size, header_size, kernel_size, ramdisk_size):
    """按 bootimg v4 布局给出 kernel / ramdisk 的文件区间（v0-v2 亦兼容）。"""
    koff = page_size * ((header_size + page_size - 1) // page_size)
    kend = koff + kernel_size
    roff = page_size * ((kend + page_size - 1) // page_size)
    rend = roff + ramdisk_size
    return (koff, kend), (roff, rend)


def check_bootimg_diff(orig_path, new_path, partition_size=None):
    """V-24~V-28：修补产物 boot.img vs 原厂 boot.img 的 AVB 零触碰断言。"""
    for p in (orig_path, new_path):
        if not os.path.isfile(p):
            record("RB0", "FAIL", "文件不存在：%s" % p)
            return
    a = read_bytes(orig_path)
    b = read_bytes(new_path)
    print("\n[BOOTIMG-DIFF] orig=%s (%d B)" % (orig_path, len(a)))
    print("               new =%s (%d B)  sha256=%s" % (new_path, len(b), hashlib.sha256(b).hexdigest()[:32]))

    # RB1 长度：严格等长 & == 分区大小
    if len(a) != len(b):
        record("RB1", "FAIL", "产物与原厂不等长：orig=%d new=%d（AVB footer/分区布局必坏）" % (len(a), len(b)))
    elif partition_size and len(a) != partition_size:
        record("RB1", "FAIL", "原厂样本长度 %d != 声明分区大小 %d（样本/参数有误）" % (len(a), partition_size))
    else:
        record("RB1", "PASS", "产物与原厂严格等长（%d B%s），满足分区布局约束"
               % (len(a), "，== 分区大小" if partition_size else ""))

    # RB2 AVB footer 逐字节保留
    fa, fb = _avb_footer(a), _avb_footer(b)
    if not fa or not fb:
        record("RB2", "FAIL", "AVB footer 缺失（orig=%s new=%s）；产物必须保留 AVBf footer"
               % (bool(fa), bool(fb)))
    elif fa.get("bad_range") or fb.get("bad_range"):
        record("RB2", "FAIL", "AVB footer 指向的 vbmeta 区间越界")
    elif fa["footer"] != fb["footer"]:
        record("RB2", "FAIL", "AVB footer 64 字节与原厂不一致（伪造/重写 footer 会破坏 AVB 结构）")
    else:
        record("RB2", "PASS", "AVB footer（末 64B）逐字节保留：v%s, original_image_size=%d, vbmeta_offset=%d, vbmeta_size=%d"
               % (fa["version"], fa["original_image_size"], fa["vbmeta_offset"], fa["vbmeta_size"]))

    # RB3 内嵌 vbmeta 逐字节保留（digest 不动的充分条件）
    if fa and fb and not fa.get("bad_range") and not fb.get("bad_range"):
        va = a[fa["vbmeta_offset"]:fa["vbmeta_offset"] + fa["vbmeta_size"]]
        vb = b[fb["vbmeta_offset"]:fb["vbmeta_offset"] + fb["vbmeta_size"]]
        if va != vb:
            record("RB3", "FAIL", "内嵌 vbmeta 被改动（AVB 零触碰违例；digest/签名区不得动）")
        else:
            record("RB3", "PASS", "内嵌 vbmeta 逐字节保留（%d B, sha256=%s）→ boot hash digest 不变"
                   % (len(va), hashlib.sha256(va).hexdigest()[:32]))

    # RB4 bootimg 头结构一致 + RB5 段校验
    try:
        ha = _parse_bootimg_header(a)
        hb = _parse_bootimg_header(b)
    except ValueError as exc:
        record("RB4", "FAIL", "bootimg 头解析失败：%s" % exc)
        return
    if (ha["page_size"], ha["header_version"], ha["kernel_size"], ha["ramdisk_size"]) != \
       (hb["page_size"], hb["header_version"], hb["kernel_size"], hb["ramdisk_size"]):
        record("RB4", "FAIL", "bootimg 头结构不一致：orig=%s new=%s"
               % ({k: ha[k] for k in ("page_size", "header_version", "kernel_size", "ramdisk_size")},
                  {k: hb[k] for k in ("page_size", "header_version", "kernel_size", "ramdisk_size")}))
    else:
        record("RB4", "PASS", "bootimg 头结构一致（v%d, page=%d, kernel_size=%d, ramdisk_size=%d）"
               % (ha["header_version"], ha["page_size"], ha["kernel_size"], ha["ramdisk_size"]))
    (ka0, ka1), (ra0, ra1) = _boot_segments(a, ha["page_size"], ha["header_size"], ha["kernel_size"], ha["ramdisk_size"])
    (kb0, kb1), (rb0, rb1) = _boot_segments(b, hb["page_size"], hb["header_size"], hb["kernel_size"], hb["ramdisk_size"])
    ka, kb = a[ka0:ka1], b[kb0:kb1]
    if len(ka) != len(kb):
        record("RB5", "FAIL", "kernel 段长度不一致（属 RB1 连带问题）")
    elif hashlib.sha256(ka).hexdigest() == hashlib.sha256(kb).hexdigest():
        record("RB5", "WARN", "kernel 段与原厂逐字节相同（本产物未 patch 内核？请确认）")
    else:
        record("RB5", "PASS", "kernel 段等长且确有改动（orig sha256=%s → new sha256=%s，长度 %d）"
               % (hashlib.sha256(ka).hexdigest()[:16], hashlib.sha256(kb).hexdigest()[:16], len(ka)))
    ra, rbb = a[ra0:ra1], b[rb0:rb1]
    if ha["ramdisk_size"] and ra != rbb:
        record("RB6", "FAIL", "ramdisk 段被改动（rc 注入属有意改动时需单独声明并复核：当前判为异常）")
    elif ha["ramdisk_size"]:
        record("RB6", "PASS", "ramdisk 段逐字节保留（%d B）" % len(ra))
    else:
        record("RB6", "PASS", "无 ramdisk 段（ramdisk_size=0，属 boot/init_boot 分离形态）")


def _parse_bootimg_header(data):
    if len(data) < 0x660 or data[:8] != b"ANDROID!":
        raise ValueError("非 bootimg（magic 不符）")
    hdr_ver = _u32(data, 0x28)
    if hdr_ver >= 3:
        header_size = _u32(data, 0x14)
        kernel_size, ramdisk_size = _u32(data, 0x08), _u32(data, 0x0C)
        page_size = 4096
    else:
        page_size = _u32(data, 0x24)
        kernel_size, ramdisk_size = _u32(data, 0x08), _u32(data, 0x10)
        header_size = _u32(data, 0x66C) if hdr_ver >= 1 else 0x660
    if page_size not in (2048, 4096, 8192, 16384):
        raise ValueError("page_size=%d 非法" % page_size)
    return {"header_version": hdr_ver, "header_size": header_size, "page_size": page_size,
            "kernel_size": kernel_size, "ramdisk_size": ramdisk_size}


# --------------------------------------------------------------------------
# --self-test：合成夹具自检（门禁的「门禁」）
#
# 背景（docs/HANDOFF-2026-11-08.md:153-155）：AVB-3 曾写成永远 PASS 的空断言，
# 比没有断言更危险——CI 全绿而产线在裸奔。因此本模式的铁律是：
#   每一条断言都必须同时给出
#     (a) 正样本：断言必须记 PASS（证明断言真的会绿）
#     (b) 负样本：断言必须记 FAIL/WARN（证明断言真的会红）
#   任意一条的负样本没变红 → 该断言已退化成「永远 PASS」→ 本模式退出码非 0。
# 夹具全部在内存中合成（struct 手工拼 arm64 Image 头 / bootimg 头 / AVB footer），
# 不依赖 samples/ 里的大文件，CI 上零外部数据即可真跑。
# --------------------------------------------------------------------------
SELFTEST_KEY = "AbCdEfGhIjKlMnOpQrStUvWxYz0123456789abcdefghijkl"  # 48 位 [A-Za-z0-9]
# 注意：这里**不加**模块级 assert——自检常量写错不应让 --help / 真实校验路径也崩。
# 长度/字符集由自检自身的「夹具健全性」用例把关（见 _self_test_body 开头）。

_IMAGE_MAGIC_BYTES = struct.pack("<I", ARM64_IMAGE_MAGIC)


def _mk_image(size=4096, fill=0x00, image_size=None, magic=None):
    """合成最小 arm64 Image：0x38 魔数 + 0x10 image_size，其余填充。"""
    buf = bytearray([fill]) * size
    buf[0:4] = b"\x00\x00\x00\x00"
    body = image_size if image_size is not None else size
    if size >= 0x40:
        buf[IMAGE_SIZE_OFFSET:IMAGE_SIZE_OFFSET + 8] = struct.pack("<Q", body)
        buf[IMAGE_MAGIC_OFFSET:IMAGE_MAGIC_OFFSET + 4] = (
            magic if magic is not None else _IMAGE_MAGIC_BYTES)
    return bytes(buf)


def _mk_report(sha_in, size_in, sha_out, size_out, key=SELFTEST_KEY,
               hidden_prefix=None, resolved_groups=REQUIRED_HOOKS,
               embed_offset=0x400, extra=None):
    """合成一份「结构合法」的 report.json（默认全字段自洽）。

    embed_offset 必须与夹具里密钥窗口的真实偏移一致，否则 V2 落位断言会（正确地）FAIL。
    hidden_prefix 缺省取 key[:16]，与引擎 build_report 的 hidden_dir_prefix 契约一致
    （否则 V6 正样本会假红——这条默认值本身就是被新加的 V6 正样本用例揪出来的）。
    """
    if hidden_prefix is None:
        hidden_prefix = (key or SELFTEST_KEY)[:16]
    detail = []
    off = 0x1000
    for grp in REQUIRED_HOOKS:
        if grp in resolved_groups:
            detail.append({"group": grp, "resolved": True, "hook_addr": "0x%X" % (0x100000 + off),
                           "bytes": 32, "patch_offset": "0x%X" % off})
        else:
            detail.append({"group": grp, "resolved": False, "reason": "kallsyms 未命中"})
        off += 0x40
    detail.append({"group": "embed_key", "resolved": True, "hook_addr": "0x%X" % 0x200000,
                   "bytes": 48, "patch_offset": "0x%X" % embed_offset})
    report = {
        "engine_version": "0.1.0-selftest",
        "input": {"sha256": sha_in, "size": size_in},
        "output": {"sha256": sha_out, "size": size_out},
        "key_spec": {"charset": "A-Za-z0-9", "length": 48, "stored_len": 47},
        "hidden_dir_prefix": hidden_prefix,
        "hook_table": [{"name": g, "hook_addr": "0x%X" % (0x100000 + i)} for i, g in
                       enumerate(REQUIRED_HOOKS)],
        "hooks": detail,
    }
    if key is not None:
        report["root_key"] = key
    if extra:
        report.update(extra)
    return report


def _mk_ndjson(events):
    return "\n".join(json.dumps(e) for e in events) + "\n"


def _mk_bootimg(orig_kernel, new_kernel=None, ramdisk=b"", avb=True, page_size=4096,
                hdr_ver=4, path_size=None):
    """合成 boot.img（v3/v4 布局）：ANDROID! 头 + kernel 段 + ramdisk 段 + AVB footer。

    new_kernel 为 None 时 orig/new 逐字节相同。返回 bytes。
    """
    hdr_size = {3: 0x600, 4: 0x630}.get(hdr_ver, 0x660)
    kernel = orig_kernel if new_kernel is None else new_kernel
    n_kernel_pages = (hdr_size + page_size - 1) // page_size
    n_kpages = (len(kernel) + page_size - 1) // page_size
    n_rpages = (len(ramdisk) + page_size - 1) // page_size if ramdisk else 0
    total = (n_kernel_pages + n_kpages + n_rpages) * page_size
    if avb:
        total += page_size
    buf = bytearray(total)
    buf[0:8] = BOOT_MAGIC
    struct.pack_into("<I", buf, 0x08, len(kernel))      # kernel_size
    struct.pack_into("<I", buf, 0x0C, len(ramdisk))     # ramdisk_size
    struct.pack_into("<I", buf, 0x14, hdr_size)         # header_size
    struct.pack_into("<I", buf, 0x28, hdr_ver)          # header_version
    koff = n_kernel_pages * page_size
    buf[koff:koff + len(kernel)] = kernel
    roff = (n_kernel_pages + n_kpages) * page_size
    if ramdisk:
        buf[roff:roff + len(ramdisk)] = ramdisk
    if avb:
        # AVB footer 在**文件末 64 字节**（可能落在末页中间），后面是零填充
        vbmeta_off = (n_kernel_pages + n_kpages + n_rpages) * page_size
        vbmeta = b"AVB0" + b"\xAB" * 60
        buf[vbmeta_off:vbmeta_off + len(vbmeta)] = vbmeta
        footer = bytearray(64)
        footer[0:4] = b"AVBf"
        struct.pack_into(">II", footer, 4, 1, 3)
        struct.pack_into(">Q", footer, 12, total)
        struct.pack_into(">QQ", footer, 20, vbmeta_off, len(vbmeta))
        buf[total - 64:total] = footer
    return bytes(buf)


def _mk_elf(e_type=2, e_machine=EM_AARCH64, phdrs=(), interp=None, needed=None,
            symbols=(), with_dynsym=True, with_tls=False, tls_align=0x40,
            e_ident_class=2, e_ident_data=1, magic=b"\x7fELF"):
    """用 struct 拼一个**最小合法 ELF64 LSB AArch64**，供 ELF1~ELF6 自检使用。

    参数语义（对应 verify_patch.py 里六条断言实际读取的字节结构）：
      e_type      ET_EXEC(2) / ET_DYN(3)
      e_machine   0xB7=AArch64（正确） / 0x3E=x86-64（反例）
      phdrs       额外程序头，形如 [(p_type, p_align)]；PT_TLS 由 with_tls 控制
      interp      非 None 时加一个 PT_INTERP（宿主解释器 → ELF2 负样本）
      needed      非 None 时构造 PT_DYNAMIC + DT_NEEDED/DT_STRTAB，值如 ["libc.so"]
      symbols     非 None 时构造 .dynsym/.dynstr/.shstrtab 导出这些符号名

    布局（偏移全部显式计算，断言侧只读这些字段，故无需链接器语义）：
      [0x00, 0x40)  ELF 头
      [0x40, ...)   程序头表（e_phoff=0x40）
      然后依次：interp 串 / dynstr / dynsym / dynamic / shstrtab / section headers
    """
    ehsize = 64
    phentsize = 56
    shentsize = 64

    # 程序头数量在构造过程中才确定（PT_LOAD 最后补），所以先按**上界**预留表槽位，
    # 收尾时再按真实数量回填并收缩。上界 = PT_LOAD + 可选 interp/TLS/DYNAMIC + 调用方给的。
    max_phnum = 1 + (1 if interp is not None else 0) + (1 if with_tls else 0) \
        + (1 if needed is not None else 0) + len(phdrs)
    blob = bytearray(ehsize + phentsize * max_phnum)
    ph_entries = []      # (p_type, p_flags, p_offset, p_vaddr, p_filesz, p_memsz, p_align)

    def _append(data, align=8):
        while len(blob) % align:
            blob.append(0)
        off = len(blob)
        blob.extend(data)
        return off

    # PT_INTERP
    if interp is not None:
        raw = interp.encode("utf-8") + b"\x00"
        off = _append(raw, 1)
        ph_entries.append((PT_INTERP, 4, off, off, len(raw), len(raw), 1))

    # PT_TLS（信息项：ELF4 读 p_align）
    if with_tls:
        off = _append(b"\x00" * 16, 16)
        ph_entries.append((PT_TLS, 4, off, off, 16, 16, tls_align))

    dyn_off = dyn_size = 0
    dynstr_off = 0
    if needed is not None:
        # .dynstr：DT_NEEDED 的名字
        dynstr = bytearray(b"\x00")
        name_offs = []
        for i, nm in enumerate(needed):
            name_offs.append(len(dynstr))
            dynstr.extend(nm.encode("ascii") + b"\x00")
        dynstr_off = _append(bytes(dynstr), 1)
        # PT_DYNAMIC 条目：DT_NEEDED ×N + DT_STRTAB + DT_NULL
        ents = []
        for i, _nm in enumerate(needed):
            ents.append(struct.pack("<QQ", DT_NEEDED, name_offs[i]))
        # DT_STRTAB 的 vaddr 必须能被 _dynamic_needed 的 v2o 解析：
        # v2o 依赖 PT_LOAD 的 vaddr 区间。此夹具用 vaddr == 文件偏移（恒等映射），
        # 由下方 PT_LOAD 覆盖整个文件保证。
        ents.append(struct.pack("<QQ", DT_STRTAB, dynstr_off))
        ents.append(struct.pack("<QQ", DT_NULL, 0))
        dyn = b"".join(ents)
        dyn_off = _append(dyn, 8)
        dyn_size = len(dyn)
        ph_entries.append((PT_DYNAMIC, 6, dyn_off, dyn_off, dyn_size, dyn_size, 8))

    # 额外程序头（调用方给的 (p_type, p_align)）
    for p_type, p_align in phdrs:
        ph_entries.append((p_type, 4, 0, 0, 0, 0, p_align))

    # ---- section headers：.dynsym / .dynstr / .shstrtab ----
    # 注意：_elf_symbols 通过 .dynsym 的 sh_link 找 dynstr，因此符号名必须放在
    # **section 指向的那张串表**里。needed 与 symbols 要两份独立串表（DT_STRTAB
    # 那份只服务 DT_NEEDED 名），所以这里单独建 symbol 串表，不复用 needed 的表。
    e_shoff = 0
    e_shnum = 0
    e_shstrndx = 0
    if with_dynsym and symbols:
        symstr = bytearray(b"\x00")
        sym_name_off = {}
        for nm in symbols:
            sym_name_off[nm] = len(symstr)
            symstr.extend(nm.encode("ascii") + b"\x00")
        symstr_off = _append(bytes(symstr), 1)
        # .dynsym 每项 24B：st_name u32 / st_info u8 / st_other u8 / st_shndx u16 /
        #                    st_value u64 / st_size u64；第 0 项是保留的空符号。
        syms = [struct.pack("<IBBHQQ", 0, 0, 0, 0, 0, 0)]
        for nm in symbols:
            syms.append(struct.pack("<IBBHQQ", sym_name_off[nm], 0x12, 0, 1, 0x1000, 0))
        sym_off = _append(b"".join(syms), 8)
        sym_size = 24 * len(syms)
        shstr = bytearray(b"\x00")
        n_dynsym = len(shstr); shstr.extend(b".dynsym\x00")
        n_dynstr = len(shstr); shstr.extend(b".dynstr\x00")
        n_shstr = len(shstr); shstr.extend(b".shstrtab\x00")
        shstr_off = _append(bytes(shstr), 1)
        secs = [b"\x00" * shentsize]   # [0] NULL section
        # [1] .dynsym：sh_type=11(SHT_DYNSYM)，sh_link=2 指向 .dynstr，entsize=24
        secs.append(struct.pack("<IIQQQQIIQQ", n_dynsym, 11, 0, 0, sym_off, sym_size,
                                2, 0, 1, 24))
        # [2] .dynstr：sh_type=3(SHT_STRTAB)
        secs.append(struct.pack("<IIQQQQIIQQ", n_dynstr, 3, 0, 0, symstr_off, len(symstr),
                                0, 0, 1, 0))
        # [3] .shstrtab
        secs.append(struct.pack("<IIQQQQIIQQ", n_shstr, 3, 0, 0, shstr_off, len(shstr),
                                0, 0, 1, 0))
        e_shoff = _append(b"".join(secs), 8)
        e_shnum = len(secs)
        e_shstrndx = 3

    # ---- PT_LOAD 覆盖整个文件：_dynamic_needed 的 v2o() 靠 PT_LOAD 把 DT_STRTAB
    #      的 vaddr 折回文件偏移；本夹具令 vaddr == 文件偏移（恒等映射）。----
    ph_entries.insert(0, (PT_LOAD, 5, 0, 0, len(blob), len(blob), 0x1000))

    # ---- ELF 头 ----
    ident = bytearray(16)
    ident[0:4] = magic
    ident[4] = e_ident_class
    ident[5] = e_ident_data
    ident[6] = 1          # EI_VERSION
    ident[7] = 0          # EI_OSABI
    blob[0:16] = ident
    struct.pack_into("<H", blob, 16, e_type)
    struct.pack_into("<H", blob, 18, e_machine)
    struct.pack_into("<I", blob, 20, 1)            # e_version
    struct.pack_into("<Q", blob, 24, 0)            # e_entry
    struct.pack_into("<Q", blob, 32, ehsize)       # e_phoff
    struct.pack_into("<Q", blob, 40, e_shoff)      # e_shoff
    struct.pack_into("<H", blob, 52, ehsize)       # e_ehsize
    struct.pack_into("<H", blob, 54, phentsize)    # e_phentsize
    struct.pack_into("<H", blob, 56, len(ph_entries))  # e_phnum
    struct.pack_into("<H", blob, 58, shentsize)    # e_shentsize
    struct.pack_into("<H", blob, 60, e_shnum)      # e_shnum
    struct.pack_into("<H", blob, 62, e_shstrndx)   # e_shstrndx
    for i, (p_type, p_flags, p_off, p_vaddr, p_filesz, p_memsz, p_align) in enumerate(ph_entries):
        struct.pack_into("<IIQQQQQQ", blob, ehsize + i * phentsize,
                         p_type, p_flags, p_off, p_vaddr, p_vaddr, p_filesz, p_memsz, p_align)
    return bytes(blob)


def _chmod_probe(tmpdir):
    """探测本机 os.chmod 是否真的能改可执行位。

    关键平台差异（本机实测）：Linux 上 chmod(0o755) 生效、os.stat 反映 755；
    Windows 上 os.stat 恒为 666，chmod 只影响只读位 → **无法**用同一 mode 夹具
    覆盖 ELF3 的 exec 分支。因此 ELF3 的 exec 用例按探测结果决定「真跑断言」还是
    「显式标注平台不支持」，绝不假装通过（否则又是空断言）。
    """
    p = os.path.join(tmpdir, "_chmod_probe.bin")
    with open(p, "wb") as f:
        f.write(b"x")
    try:
        os.chmod(p, 0o755)
        return bool(os.stat(p).st_mode & 0o111)
    except OSError:
        return False


class _Probe(object):
    """在隔离的 results / unverified 上跑一段断言，返回 [(cid, status, msg)]。"""

    def __init__(self):
        self.entries = []
        self.unverified_entries = []

    def __enter__(self):
        del results[:]
        del unverified[:]
        return self

    def __exit__(self, *exc):
        self.entries = list(results)
        self.unverified_entries = list(unverified)
        del results[:]
        del unverified[:]
        return False

    def statuses(self, cid):
        return [s for c, s, _ in self.entries if c == cid]

    def has(self, cid, status):
        return status in self.statuses(cid)

    def unverified_ids(self):
        return [cid for cid, _ in self.unverified_entries]

    def detail(self, cid=None):
        return ["[%s] %s %s" % (c, s, m) for c, s, m in self.entries
                if cid is None or c == cid]


def _selftest_case(cases, name, want, cid, status, fn, expect_unverified=False):
    """记录一条自检用例；立即执行并把结果挂到 case 上。

    fn 的标准输出被吞掉（断言实现里有大量 print），失败时随用例一起打印。
    expect_unverified=True 时额外要求该用例在未校验台账里登记了 cid——
    这是「静默 SKIP → 汇总仍 PASS」这条空断言通道的门禁。
    """
    probe = _Probe()
    err = None
    captured = ""
    try:
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            with probe:
                fn()
        captured = buf.getvalue()
    except Exception:  # noqa: BLE001 - 断言实现抛异常也是自检失败，需原样暴露
        err = traceback.format_exc()
    st = probe.statuses(cid)
    ok = (status in st) and err is None
    if expect_unverified and cid not in probe.unverified_ids():
        ok = False  # 未校验没被登记 = 「什么都没验」又会隐身
    cases.append({"name": name, "want": want, "cid": cid, "want_status": status,
                  "got": st, "ok": ok, "err": err, "stdout": captured,
                  "expect_unverified": expect_unverified,
                  "unverified_ids": probe.unverified_ids(),
                  "detail": probe.detail()})
    return ok


def run_self_test(quiet=False):
    """--self-test 主体。返回 0=全部用例符合预期；1=有断言退化/夹具失配。"""
    tmpdir = tempfile.mkdtemp(prefix="sproot-selftest-")
    try:
        return _self_test_body(tmpdir, quiet)
    finally:
        shutil.rmtree(tmpdir, ignore_errors=True)


def _self_test_body(tmpdir, quiet=False):
    # 夹具健全性前置闸门：常量本身写错时，后面所有用例都会失去意义（正样本会一起红），
    # 必须在这里先判死，而不是让「负样本没变红」伪装成通过。
    fixture_problems = []
    if len(SELFTEST_KEY) != 48:
        fixture_problems.append("SELFTEST_KEY 长度 %d != 48" % len(SELFTEST_KEY))
    if not SELFTEST_KEY.isalnum():
        fixture_problems.append("SELFTEST_KEY 含非 [A-Za-z0-9] 字符")
    if fixture_problems:
        print("判定：SELF-TEST FAIL —— 夹具自身不合法：%s" % fixture_problems)
        return 1

    cases = []

    def w(name, data):
        p = os.path.join(tmpdir, name)
        mode = "wb" if isinstance(data, bytes) else "w"
        with open(p, mode, **({} if mode == "wb" else {"encoding": "utf-8"})) as f:
            f.write(data)
        return p

    # ---------------- V1 等长 / 确实改动 ----------------
    base = _mk_image(4096)
    patched = bytearray(base)
    patched[0x200:0x208] = b"PATCHED!"          # 单处 8B 改动
    patched = bytes(patched)
    shorter = base[:-16]

    _selftest_case(cases, "V1 正样本：等长且确有改动", "PASS", "V1", "PASS",
                   lambda: _run_pair_quiet(base, patched))
    _selftest_case(cases, "V1 负样本：产物被截断（不等长）", "FAIL", "V1", "FAIL",
                   lambda: _run_pair_quiet(base, shorter))
    _selftest_case(cases, "V1 负样本：产物与原厂逐字节相同（无改动）", "FAIL", "V1", "FAIL",
                   lambda: _run_pair_quiet(base, base))

    # ---------------- V2 密钥内嵌窗口 / V6 前缀 ----------------
    want_win = SELFTEST_KEY[:47].encode("ascii") + b"\x00"
    with_key = bytearray(base)
    with_key[0x400:0x400 + len(want_win)] = want_win
    with_key = bytes(with_key)

    _selftest_case(cases, "V2 正样本：产物含 key[0:47]+NUL 窗口且落位与 report 一致", "PASS",
                   "V2", "PASS",
                   lambda: _run_pair_quiet(with_key, _patch_bytes(with_key, 0x800),
                                           report=_mk_report_for(with_key, _patch_bytes(with_key, 0x800)),
                                           key=SELFTEST_KEY))
    # 负样本：产物里没有密钥窗口（密钥根本没内嵌）→ V2 必须 FAIL
    _selftest_case(cases, "V2 负样本：产物不含密钥窗口", "FAIL", "V2", "FAIL",
                   lambda: _run_pair_quiet(base, patched,
                                           report=_mk_report_for(base, patched),
                                           key=SELFTEST_KEY))
    # 负样本：窗口存在但 report.root_key_region 指向别处 → 落位断言必须 FAIL
    def _v2_bad_region():
        rep = _mk_report_for(with_key, _patch_bytes(with_key, 0x800))
        for h in rep["hooks"]:
            if h["group"] == "embed_key":
                h["patch_offset"] = "0x9000"   # 与真实 0x400 不符
        _run_pair_quiet(with_key, _patch_bytes(with_key, 0x800), report=rep, key=SELFTEST_KEY)
    _selftest_case(cases, "V2 负样本：密钥落位与 report.root_key_region 不符", "FAIL", "V2", "FAIL",
                   _v2_bad_region)
    # 负样本：report.hidden_dir_prefix 与 key[:16] 不符 → V6 必须 FAIL
    def _v6_bad_prefix():
        rep = _mk_report_for(with_key, _patch_bytes(with_key, 0x800))
        rep["hidden_dir_prefix"] = "wrong_prefix_xxxx"
        _run_pair_quiet(with_key, _patch_bytes(with_key, 0x800), report=rep, key=SELFTEST_KEY)
    _selftest_case(cases, "V6 负样本：hidden_dir_prefix != key[0:16]", "FAIL", "V6", "FAIL",
                   _v6_bad_prefix)
    # 负样本：密钥格式非法（长度/字符集）→ V2 必须 FAIL
    _selftest_case(cases, "V2 负样本：密钥非 [A-Za-z0-9]{48}", "FAIL", "V2", "FAIL",
                   lambda: _run_pair_quiet(base, patched,
                                           report=_mk_report_for(base, patched),
                                           key="short_key"))

    # ---- 密钥来源回落：引擎 task-8 起 report 顶层**有** root_key ----
    # 正样本：不传 --key，从 report.root_key 回落取到密钥 → V2/V6 必须真跑并 PASS
    # （这条用例保证「回落路径」不是靠 SKIP 装绿）
    _selftest_case(cases,
                   "V2 正样本：不传 --key，回落 report.root_key 后仍真跑密钥断言", "PASS",
                   "V2", "PASS",
                   lambda: _run_pair_quiet(with_key, _patch_bytes(with_key, 0x800),
                                           report=_mk_report_for(with_key, _patch_bytes(with_key, 0x800)),
                                           key=None))
    _selftest_case(cases,
                   "V6 正样本：不传 --key，回落 report.root_key 后前缀一致性 PASS", "PASS",
                   "V6", "PASS",
                   lambda: _run_pair_quiet(with_key, _patch_bytes(with_key, 0x800),
                                           report=_mk_report_for(with_key, _patch_bytes(with_key, 0x800)),
                                           key=None))

    # 负样本（空断言通道）：report 存在但**没有** root_key 且未传 --key。
    # 旧行为是静默 SKIP 后汇总整体 PASS —— 等于「什么都没验」却发绿。现在必须
    # WARN + 登记未校验台账，让汇总显式计数。
    def _v2_report_no_root_key():
        rep = _mk_report_for(with_key, _patch_bytes(with_key, 0x800), key=None)  # 顶层无 root_key
        assert "root_key" not in rep
        _run_pair_quiet(with_key, _patch_bytes(with_key, 0x800), report=rep, key=None)
    _selftest_case(cases, "V2 负样本：report 缺 root_key → 必须 WARN 且登记未校验",
                   "WARN(未校验)", "V2", "WARN", _v2_report_no_root_key,
                   expect_unverified=True)
    _selftest_case(cases, "V6 负样本：report 缺 root_key → 必须 WARN 且登记未校验",
                   "WARN(未校验)", "V6", "WARN", _v2_report_no_root_key,
                   expect_unverified=True)

    # 负样本：report 的 root_key 若与真实内嵌密钥不同 → 产物里找不到该窗口 → V2 FAIL
    def _v2_wrong_root_key():
        rep = _mk_report_for(with_key, _patch_bytes(with_key, 0x800),
                             key="Zz9" + SELFTEST_KEY[3:])   # 同长度但不同密钥
        _run_pair_quiet(with_key, _patch_bytes(with_key, 0x800), report=rep, key=None)
    _selftest_case(cases, "V2 负样本：report.root_key 是另一把密钥（窗口对不上）",
                   "FAIL", "V2", "FAIL", _v2_wrong_root_key)

    # ---------------- V3 report 自洽 ----------------
    _selftest_case(cases, "V3 正样本：report sha256/size/key_spec 自洽", "PASS", "V3", "PASS",
                   lambda: _run_pair_quiet(base, patched, report=_mk_report_for(base, patched),
                                           key=SELFTEST_KEY))

    def _v3_bad_sha():
        rep = _mk_report_for(base, patched)
        rep["output"]["sha256"] = "0" * 64
        _run_pair_quiet(base, patched, report=rep, key=SELFTEST_KEY)
    _selftest_case(cases, "V3 负样本：report.output.sha256 与实际产物不符", "FAIL", "V3", "FAIL",
                   _v3_bad_sha)

    def _v3_bad_size():
        rep = _mk_report_for(base, patched)
        rep["input"]["size"] = 999
        _run_pair_quiet(base, patched, report=rep, key=SELFTEST_KEY)
    _selftest_case(cases, "V3 负样本：report.input.size 与实际输入不符", "FAIL", "V3", "FAIL",
                   _v3_bad_size)

    def _v3_missing_field():
        rep = _mk_report_for(base, patched)
        del rep["engine_version"]
        _run_pair_quiet(base, patched, report=rep, key=SELFTEST_KEY)
    _selftest_case(cases, "V3 负样本：report 缺 engine_version", "FAIL", "V3", "FAIL",
                   _v3_missing_field)

    def _v3_bad_spec():
        rep = _mk_report_for(base, patched)
        rep["key_spec"] = {"charset": "A-Za-z0-9", "length": 40, "stored_len": 39}
        _run_pair_quiet(base, patched, report=rep, key=SELFTEST_KEY)
    _selftest_case(cases, "V3 负样本：report.key_spec 违反 48/47 契约", "FAIL", "V3", "FAIL",
                   _v3_bad_spec)

    # ---------------- V4 / V-08 hook 表 ----------------
    _selftest_case(cases, "V4 正样本：hooks[] 五组必备 hook 齐全且地址有效", "PASS", "V4", "PASS",
                   lambda: _run_pair_quiet(base, patched, report=_mk_report_for(base, patched),
                                           key=SELFTEST_KEY))

    def _v4_missing_hook():
        rep = _mk_report_for(base, patched)
        rep["hooks"] = [h for h in rep["hooks"] if h["group"] != "avc_denied"]
        _run_pair_quiet(base, patched, report=rep, key=SELFTEST_KEY)
    _selftest_case(cases, "V4 负样本：hooks[] 缺 avc_denied 组", "FAIL", "V4", "FAIL",
                   _v4_missing_hook)

    def _v4_empty_addr():
        rep = _mk_report_for(base, patched)
        for h in rep["hooks"]:
            if h["group"] == "do_execve":
                h["hook_addr"] = ""
        _run_pair_quiet(base, patched, report=rep, key=SELFTEST_KEY)
    _selftest_case(cases, "V4 负样本：resolved=true 但 hook_addr 为空", "FAIL", "V4", "FAIL",
                   _v4_empty_addr)

    def _v4_false_no_reason():
        rep = _mk_report_for(base, patched, resolved_groups=REQUIRED_HOOKS[:-1])
        for h in rep["hooks"]:
            if h["group"] == REQUIRED_HOOKS[-1]:
                h["reason"] = "  "
        _run_pair_quiet(base, patched, report=rep, key=SELFTEST_KEY)
    _selftest_case(cases, "V4 负样本：resolved=false 且无故 reason", "FAIL", "V4", "FAIL",
                   _v4_false_no_reason)

    def _v4_agg_short():
        rep = _mk_report_for(base, patched)
        rep["hooks"] = []
        rep["hook_table"] = [{"name": "hooks_embedded", "blob_count": 2, "root_key_region": "0x400"}]
        _run_pair_quiet(base, patched, report=rep, key=SELFTEST_KEY)
    _selftest_case(cases, "V4 负样本：聚合形态 blob_count<5（织入不足）", "FAIL", "V4", "FAIL",
                   _v4_agg_short)

    def _v4_table_missing():
        rep = _mk_report_for(base, patched)
        rep["hooks"] = []
        rep["hook_table"] = [{"name": g, "hook_addr": 0x1000} for g in REQUIRED_HOOKS[:-1]]
        _run_pair_quiet(base, patched, report=rep, key=SELFTEST_KEY)
    _selftest_case(cases, "V4 负样本：hook_table 缺组", "FAIL", "V4", "FAIL", _v4_table_missing)

    # ---------------- V4 改动区段（V-13 受控） ----------------
    def _v13_seem():
        big = bytearray(base)
        big[0x100:0x100 + 2048] = b"\xFF" * 2048     # >512B 连续改动 → 必须 FAIL
        _run_pair_quiet(base, bytes(big))
    _selftest_case(cases, "V4 负样本：存在 >512B 连续改动区段（疑似大段重写）", "FAIL", "V4", "FAIL",
                   _v13_seem)

    # ---------------- V5 arm64 Image 头（V-07） ----------------
    _selftest_case(cases, "V5 正样本：合法 arm64 Image 头", "PASS", "V5", "PASS",
                   lambda: check_image_header(_mk_image(4096)))
    _selftest_case(cases, "V5 负样本：0x38 魔数错误（非 arm64 Image）", "FAIL", "V5", "FAIL",
                   lambda: check_image_header(_mk_image(4096, magic=b"\x1f\x8b\x08\x00")))
    _selftest_case(cases, "V5 负样本：文件过短 <64B", "FAIL", "V5", "FAIL",
                   lambda: check_image_header(b"\x00" * 32))
    _selftest_case(cases, "V5 负样本：image_size 远超文件（截断镜像）", "FAIL", "V5", "FAIL",
                   lambda: check_image_header(_mk_image(4096, image_size=1 << 20)))

    # ---------------- V6 NDJSON ----------------
    good_nd = [{"event": "progress", "phase": "read_input"},
               {"event": "progress", "phase": "embed_key"},
               {"event": "result", "code": 0}]
    _selftest_case(cases, "V6 正样本：合法 NDJSON（终态唯一且在最后）", "PASS", "V6", "PASS",
                   lambda: check_ndjson(w("good.ndjson", _mk_ndjson(good_nd))))
    _selftest_case(cases, "V6 负样本：出现未定义 phase", "FAIL", "V6", "FAIL",
                   lambda: check_ndjson(w("bad_phase.ndjson", _mk_ndjson(
                       [{"event": "progress", "phase": "no_such_phase"},
                        {"event": "result", "code": 0}]))))
    _selftest_case(cases, "V6 负样本：无 result 终态事件", "FAIL", "V6", "FAIL",
                   lambda: check_ndjson(w("no_result.ndjson", _mk_ndjson([good_nd[0]]))))
    _selftest_case(cases, "V6 负样本：result 不在最后一条", "FAIL", "V6", "FAIL",
                   lambda: check_ndjson(w("result_mid.ndjson", _mk_ndjson(
                       [{"event": "result", "code": 0},
                        {"event": "progress", "phase": "write_output"}]))))
    _selftest_case(cases, "V6 负样本：非合法 JSON 行", "FAIL", "V6", "FAIL",
                   lambda: check_ndjson(w("broken.ndjson", "{not json}\n")))

    # ---------------- V7 KP 残留 ----------------
    _selftest_case(cases, "V7 正样本：无 KP 特征字串", "PASS", "V7", "PASS",
                   lambda: check_kp_residue(_mk_image(4096)))
    _selftest_case(cases, "V7 负样本：命中 kpimg 特征（应 WARN 而非静默 PASS）", "WARN", "V7", "WARN",
                   lambda: check_kp_residue(_mk_image(4096, fill=0x00) + b"kpimg"))

    # ---------------- BI boot.img 头 ----------------
    k = _mk_image(2048)
    good_boot = _mk_bootimg(k)
    _selftest_case(cases, "BI 正样本：合法 v4 boot.img（kernel 段/对齐/Image 头）", "PASS", "BI3",
                   "PASS", lambda: check_bootimg(w("boot.img", good_boot)))
    _selftest_case(cases, "BI 负样本：magic 非 ANDROID!", "FAIL", "BI1", "FAIL",
                   lambda: check_bootimg(w("bad_magic.img", b"MZ" + b"\x00" * (0x660 - 2))))
    _selftest_case(cases, "BI 负样本：vendor_boot（VNDRBOOT）", "FAIL", "BI1", "FAIL",
                   lambda: check_bootimg(w("vendor.img", VENDOR_BOOT_MAGIC + b"\x00" * 0x660)))
    _selftest_case(cases, "BI 负样本：kernel_size=0 且 ramdisk_size=0", "FAIL", "BI3", "FAIL",
                   lambda: check_bootimg(w("empty.img", _mk_bootimg(b""))))

    def _bi_kernel_oob():
        bad = bytearray(good_boot)
        struct.pack_into("<I", bad, 0x08, len(good_boot) * 2)   # kernel_size 越界
        check_bootimg(w("oob.img", bytes(bad)))
    _selftest_case(cases, "BI 负样本：kernel 段越界", "FAIL", "BI3", "FAIL", _bi_kernel_oob)

    def _bi_gzip():
        gz = _mk_bootimg(b"\x1f\x8b" + b"\x00" * 2046)
        check_bootimg(w("gzip.img", gz))
    _selftest_case(cases, "BI 负样本/WARN：kernel 是 gzip 压缩体", "WARN", "BI4", "WARN", _bi_gzip)

    # BI 聚合 id：文件不存在
    _selftest_case(cases, "BI 负样本：boot.img 文件不存在", "FAIL", "BI", "FAIL",
                   lambda: check_bootimg(os.path.join(tmpdir, "no_such_boot.img")))

    # BI2：page_size 非法（v0~v2 布局，page_size 取自 0x24）
    def _bi_bad_page_size():
        bad = bytearray(0x1000)
        bad[0:8] = BOOT_MAGIC
        struct.pack_into("<I", bad, 0x08, 100)       # kernel_size
        struct.pack_into("<I", bad, 0x10, 0)         # ramdisk_size（v<3 在 0x10）
        struct.pack_into("<I", bad, 0x24, 1234)      # page_size 非法
        struct.pack_into("<I", bad, 0x28, 2)         # header_version = 2
        struct.pack_into("<I", bad, 0x66C, 0x800)    # header_size（v>=1 在 0x66C）
        check_bootimg(w("bad_page.img", bytes(bad)))
    _selftest_case(cases, "BI2 负样本：page_size=1234 非法值", "FAIL", "BI2", "FAIL",
                   _bi_bad_page_size)

    # BI5：文件大小未按 page_size 对齐
    def _bi_unaligned():
        check_bootimg(w("unaligned.img", good_boot + b"\x00" * 100))
    _selftest_case(cases, "BI5 负样本/WARN：文件大小未按 page 对齐", "WARN", "BI5", "WARN",
                   _bi_unaligned)

    # BI6：正常路径给出的段信息（PASS）
    _selftest_case(cases, "BI6 正样本：给出 ramdisk/kernel_off/os_version 段信息", "PASS", "BI6",
                   "PASS", lambda: check_bootimg(w("bi6.img", good_boot)))

    # ---------------- AVB-1/2/3（RB1~RB3 零触碰） ----------------
    orig_boot = _mk_bootimg(k, ramdisk=b"\x00" * 512)
    new_k = bytearray(k)
    new_k[0x200:0x210] = b"SKPATCHED!BYTES!"   # 必须与切片等长（16B），否则内核段会缩短
    assert len(new_k) == len(k)
    new_boot = _mk_bootimg(bytes(new_k), ramdisk=b"\x00" * 512)
    po = w("orig.img", orig_boot)
    pn = w("new.img", new_boot)

    _selftest_case(cases, "AVB-1 正样本：等长 + footer 保留 + 内嵌 vbmeta 未动 + kernel 有改动", "PASS",
                   "RB2", "PASS", lambda: check_bootimg_diff(po, pn))

    def _avb_bad_len():
        check_bootimg_diff(po, w("new_short.img", new_boot[:-4096]))
    _selftest_case(cases, "AVB-1 负样本：产物与原厂不等长", "FAIL", "RB1", "FAIL", _avb_bad_len)

    def _avb_no_footer():
        broken = bytearray(orig_boot)
        broken[-64:] = b"\x00" * 64
        check_bootimg_diff(w("orig_nofooter.img", bytes(broken)), pn)
    _selftest_case(cases, "AVB-2 负样本：原厂样本 AVBf footer 缺失", "FAIL", "RB2", "FAIL",
                   _avb_no_footer)

    def _avb_footer_rewrite():
        broken = bytearray(new_boot)
        broken[-60] = (broken[-60] + 1) & 0xFF      # 篡改 footer 一个字节
        check_bootimg_diff(po, w("new_footer_tamper.img", bytes(broken)))
    _selftest_case(cases, "AVB-2 负样本：footer 被篡改 1B", "FAIL", "RB2", "FAIL",
                   _avb_footer_rewrite)

    def _avb_vbmeta_change():
        fb = _avb_footer(new_boot)
        broke = bytearray(new_boot)
        off = fb["vbmeta_offset"]
        broke[off] ^= 0xFF                            # 改 vbmeta 首字节
        check_bootimg_diff(po, w("new_vbmeta_tamper.img", bytes(broke)))
    _selftest_case(cases, "AVB-3 负样本：内嵌 vbmeta 被改动（digest 必变）", "FAIL", "RB3", "FAIL",
                   _avb_vbmeta_change)

    def _avb_ramdisk_change():
        broke = bytearray(new_boot)
        hdrs = _parse_bootimg_header(broke)
        (_, _), (r0, r1) = _boot_segments(broke, hdrs["page_size"], hdrs["header_size"],
                                          hdrs["kernel_size"], hdrs["ramdisk_size"])
        broke[r0] ^= 0xFF
        check_bootimg_diff(po, w("new_ramdisk_tamper.img", bytes(broke)))
    _selftest_case(cases, "RB6 负样本：ramdisk 段被改动", "FAIL", "RB6", "FAIL", _avb_ramdisk_change)

    # RB0：输入文件不存在（--bootimg-diff 的前置失败）
    _selftest_case(cases, "RB0 负样本：boot.img-diff 的 NEW 文件不存在", "FAIL", "RB0", "FAIL",
                   lambda: check_bootimg_diff(po, os.path.join(tmpdir, "no_such_new.img")))

    # RB4：bootimg 头结构不一致（kernel_size 被改 → 头结构必须报 FAIL）
    def _rb4_bad_header():
        broke = bytearray(new_boot)
        struct.pack_into("<I", broke, 0x08, 1024)   # kernel_size 与原厂不同
        check_bootimg_diff(po, w("new_bad_header.img", bytes(broke)))
    _selftest_case(cases, "RB4 负样本：bootimg 头结构不一致", "FAIL", "RB4", "FAIL",
                   _rb4_bad_header)

    # RB5：kernel 段与原厂逐字节相同 → 应 WARN「根本没 patch 内核」
    def _rb5_kernel_untouched():
        same = _mk_bootimg(k, ramdisk=b"\x00" * 512)   # kernel 未改，但仍保留 footer
        check_bootimg_diff(po, w("new_kernel_same.img", same))
    _selftest_case(cases, "RB5 负样本/WARN：kernel 段与原厂逐字节相同（未 patch）", "WARN", "RB5",
                   "WARN", _rb5_kernel_untouched)

    # RB5 正样本：kernel 确有改动 → PASS（与上一条构成正反对照）
    def _rb5_kernel_changed():
        check_bootimg_diff(po, pn)
    _selftest_case(cases, "RB5 正样本：kernel 段等长且确有改动", "PASS", "RB5", "PASS",
                   _rb5_kernel_changed)

    # ---------------- ELF1~ELF6（G-02 引擎产物断言） ----------------
    # 全部用 _mk_elf() 的 struct 合成字节驱动，不依赖任何真实 arm64 产物。
    jni_sym = "Java_me_sproot_patch_NativeEngine_patch"

    def _elf_file(fname, data, mode=None):
        """写 ELF 夹具并返回路径；mode 非 None 时尝试 chmod。"""
        p = w(fname, data)
        if mode is not None:
            try:
                os.chmod(p, mode)
            except OSError:
                pass
        return p

    # ELF1 架构：AArch64 正样本 / x86-64 与未知架构负样本
    static_exec = _mk_elf(e_type=2, e_machine=EM_AARCH64)
    p_exec = _elf_file("engine_exec", static_exec, 0o755)
    _selftest_case(cases, "ELF1 正样本：AArch64（e_machine=0xB7）", "PASS", "ELF1", "PASS",
                   lambda: check_elf(p_exec, elf_form="exec"))
    _selftest_case(cases, "ELF1 负样本：x86-64（e_machine=0x3E）",
                   "FAIL", "ELF1", "FAIL",
                   lambda: check_elf(_elf_file("x86_64_bin", _mk_elf(e_machine=EM_X86_64), 0o755),
                                     elf_form="exec"))
    _selftest_case(cases, "ELF1 负样本：未知架构 0x28(ARM32)",
                   "FAIL", "ELF1", "FAIL",
                   lambda: check_elf(_elf_file("arm32_bin", _mk_elf(e_machine=0x28), 0o755),
                                     elf_form="exec"))

    # ELF2 静态性：静态 exec 正样本 / 含宿主解释器负样本 / JNI 含 PT_DYNAMIC 可接受
    _selftest_case(cases, "ELF2 正样本：静态可执行（无 INTERP 无 DYNAMIC）", "PASS", "ELF2", "PASS",
                   lambda: check_elf(p_exec, elf_form="exec"))
    _selftest_case(cases, "ELF2 负样本：含宿主解释器 PT_INTERP（非 Android linker）",
                   "FAIL", "ELF2", "FAIL",
                   lambda: check_elf(_elf_file("host_interp",
                                               _mk_elf(e_type=2, interp="/lib64/ld-linux-x86-64.so.2"),
                                               0o755), elf_form="exec"))

    def _elf2_android_linker():
        """Android linker 解释器 → ELF2 应 PASS（不是「非 Android linker」）。"""
        check_elf(_elf_file("android_linker",
                            _mk_elf(e_type=3, interp="/system/bin/linker64",
                                    needed=["libc.so"]), 0o755),
                  elf_form="jni")
    _selftest_case(cases, "ELF2 正样本：Android linker 解释器视为可运行", "PASS", "ELF2", "PASS",
                   _elf2_android_linker)

    # ELF3 可执行位：依赖 os.chmod 能力（Windows 上不可用 → 显式跳过并说明）
    chmod_ok = _chmod_probe(tmpdir)
    if chmod_ok:
        _selftest_case(cases, "ELF3 正样本：exec 形态 mode 含可执行位", "PASS", "ELF3", "PASS",
                       lambda: check_elf(p_exec, elf_form="exec"))
        _selftest_case(cases, "ELF3 负样本：exec 形态 mode=644 无可执行位", "FAIL", "ELF3", "FAIL",
                       lambda: check_elf(_elf_file("noexec", static_exec, 0o644), elf_form="exec"))
        _selftest_case(cases, "ELF3 负样本/JNI：.so 不要求可执行位（应 PASS 而非 FAIL）",
                       "PASS", "ELF3", "PASS",
                       lambda: check_elf(_elf_file("libtest.so", static_exec, 0o644), elf_form="jni"))
    else:
        cases.append({"name": "ELF3 用例：本平台 os.chmod 无法设置执行位，已跳过（非 PASS）",
                      "want": "SKIP(平台)", "cid": "ELF3", "want_status": "PASS",
                      "got": ["SKIP(platform)"], "ok": True, "err": None, "stdout": "",
                      "expect_unverified": False, "unverified_ids": [],
                      "detail": ["[ELF3] SKIP 本平台 chmod 不改执行位；ELF3 由 CI(Ubuntu) 覆盖"]})

    # ELF4 TLS 信息项：有 TLS 报 p_align / 无 TLS 也不阻断（两者都 PASS）
    _selftest_case(cases, "ELF4 正样本：PT_TLS 存在并报 p_align", "PASS", "ELF4", "PASS",
                   lambda: check_elf(_elf_file("tls_bin", _mk_elf(with_tls=True, tls_align=0x40), 0o755),
                                     elf_form="exec"))
    _selftest_case(cases, "ELF4 正样本：无 PT_TLS 也不阻断", "PASS", "ELF4", "PASS",
                   lambda: check_elf(p_exec, elf_form="exec"))

    # ELF5 导出符号：JNI 含 Java_* 正样本 / 缺符号负样本（真实故障：loadLibrary 后调用即崩溃）
    jni_ok = _mk_elf(e_type=3, e_machine=EM_AARCH64,
                     needed=["libc.so", "liblog.so"], symbols=[jni_sym])
    p_jni = _elf_file("libsprootpatch_jni.so", jni_ok, 0o755)
    _selftest_case(cases, "ELF5 正样本：JNI 导出 Java_* 入口齐全", "PASS", "ELF5", "PASS",
                   lambda: check_elf(p_jni, [jni_sym], elf_form="jni"))

    def _elf5_missing_symbol():
        # 只导出别的符号，缺 Java_* 入口 → 必须 FAIL
        noj = _mk_elf(e_type=3, needed=["libc.so"], symbols=["SomeOtherSymbol"])
        check_elf(_elf_file("libnojni.so", noj, 0o755), [jni_sym], elf_form="jni")
    _selftest_case(cases, "ELF5 负样本：JNI 缺 Java_* 入口（调用必崩）", "FAIL", "ELF5", "FAIL",
                   _elf5_missing_symbol)

    def _elf5_no_symtab():
        # 完全没有 .dynsym（无 section headers）→ 符号集为空 → 必须 FAIL，不能默认放过
        bare = _mk_elf(e_type=3, needed=["libc.so"], symbols=(), with_dynsym=False)
        check_elf(_elf_file("libnosym.so", bare, 0o755), [jni_sym], elf_form="jni")
    _selftest_case(cases, "ELF5 负样本：产物无 .dynsym 符号表", "FAIL", "ELF5", "FAIL",
                   _elf5_no_symtab)

    # ELF6 形态：JNI=ET_DYN+DYNAMIC 正样本；exec=静态 ET_EXEC 正样本；两者交叉反例
    _selftest_case(cases, "ELF6 正样本：JNI 形态（ET_DYN + PT_DYNAMIC）", "PASS", "ELF6", "PASS",
                   lambda: check_elf(p_jni, [jni_sym], elf_form="jni"))
    _selftest_case(cases, "ELF6 正样本：exec 形态（静态 ET_EXEC）", "PASS", "ELF6", "PASS",
                   lambda: check_elf(p_exec, elf_form="exec"))
    _selftest_case(cases, "ELF6 负样本：JNI 形态却是 ET_EXEC", "FAIL", "ELF6", "FAIL",
                   lambda: check_elf(_elf_file("jni_exec",
                                               _mk_elf(e_type=2, needed=["libc.so"], symbols=[jni_sym]),
                                               0o755), [jni_sym], elf_form="jni"))
    _selftest_case(cases, "ELF6 负样本：exec 形态却含 PT_DYNAMIC", "FAIL", "ELF6", "FAIL",
                   lambda: check_elf(_elf_file("exec_dyn",
                                               _mk_elf(e_type=2, needed=["libc.so"]), 0o755),
                                     elf_form="exec"))

    # ELF 聚合 id：文件不存在 / 非 ELF 魔数
    _selftest_case(cases, "ELF 负样本：文件不存在", "FAIL", "ELF", "FAIL",
                   lambda: check_elf(os.path.join(tmpdir, "no_such_elf")))
    _selftest_case(cases, "ELF 负样本：魔数不是 \\x7fELF", "FAIL", "ELF", "FAIL",
                   lambda: check_elf(_elf_file("not_elf", _mk_elf(magic=b"NOPE"))))
    _selftest_case(cases, "ELF 负样本：EI_CLASS=1（ELF32 不是 ELF64）", "FAIL", "ELF", "FAIL",
                   lambda: check_elf(_elf_file("elf32", _mk_elf(e_ident_class=1))))
    _selftest_case(cases, "ELF 负样本：EI_DATA=2（大端不是小端）", "FAIL", "ELF", "FAIL",
                   lambda: check_elf(_elf_file("elfbe", _mk_elf(e_ident_data=2))))

    # ---------------- 输出 ----------------
    print("== verify_patch.py --self-test（合成夹具自检）==")
    print("夹具目录：%s" % tmpdir)
    print("密钥：48 字符合成串（sha256[:8]=%s，明文为公开常量，不涉及真实密钥）"
          % hashlib.sha256(SELFTEST_KEY.encode()).hexdigest()[:8])
    print("\n%-4s %-58s %-6s %s" % ("#", "用例", "期望", "结果"))
    n_bad = 0
    for i, c in enumerate(cases, 1):
        mark = "OK" if c["ok"] else "**FAIL**"
        if not c["ok"]:
            n_bad += 1
        if quiet and c["ok"]:
            continue
        print("%-4d %-58s %-6s %s" % (i, c["name"][:58], c["want"], mark))
        if not c["ok"]:
            if c["err"]:
                print("     异常：%s" % c["err"].strip().splitlines()[-1])
                for line in c["err"].strip().splitlines()[-6:]:
                    print("       | %s" % line)
            print("     期望 [%s] %s；实得 %s" % (c["cid"], c["want_status"], c["got"] or "（无该断言记录）"))
            if not c["got"]:
                print("       → 该断言没有落任何 record()，已退化成「不存在」")
            elif c["want_status"] not in c["got"]:
                print("       → 该断言在负样本上没有变红，**已退化为永远 PASS**")
            if c.get("expect_unverified") and c["cid"] not in c.get("unverified_ids", []):
                print("       → 未校验台账没有登记 [%s]：**「什么都没验」会静默隐身**"
                      "（台账实得 %s）" % (c["cid"], c.get("unverified_ids") or "空"))
            print("     本轮明细：")
            for line in c["detail"]:
                print("       %s" % line)
    total = len(cases)
    print("\n汇总：用例 %d，符合预期 %d，不符合 %d" % (total, total - n_bad, n_bad))
    if n_bad:
        print("判定：SELF-TEST FAIL —— 有断言在负样本上没变红（退化成永远 PASS）"
              "或断言实现抛异常，禁止合入。")
        return 1
    # 覆盖度自检：确保必备断言组都被用例覆盖，防止未来有人删用例或让某组悄悄掉队。
    # required 必须与脚本里真实存在的 record() id 对齐（28 个）——只写 14 项时，
    # 未列出的组（如 BI2/RB4/BI5/RB0）退化成永远 PASS 也照样全绿，门禁形同虚设。
    ALL_CHECK_IDS = {
        "V1", "V2", "V3", "V4", "V5", "V6", "V7",
        "BI", "BI1", "BI2", "BI3", "BI4", "BI5", "BI6",
        "RB0", "RB1", "RB2", "RB3", "RB4", "RB5", "RB6",
        "ELF", "ELF1", "ELF2", "ELF3", "ELF4", "ELF5", "ELF6",
    }
    # 反向闸门①：把本文件里**真实出现**的 record("<ID>") 全抓出来，与清单比对。
    # 否则「从清单里删掉一组」就能让覆盖度门禁永远通过（auditor 的变异实证正是此类）。
    import re
    src = read_bytes(os.path.abspath(__file__)).decode("utf-8", "replace")
    actual_ids = set(re.findall(r'record(?:_unverified)?\(\s*"([A-Z]+\d*)"', src))
    ids_not_listed = sorted(actual_ids - ALL_CHECK_IDS)
    if ids_not_listed:
        print("判定：SELF-TEST FAIL —— 源码里有这些 check id 未纳入覆盖度清单（新增断言组请同步加用例）：%s"
              % ids_not_listed)
        return 1
    ids_vanished = sorted(ALL_CHECK_IDS - actual_ids)
    if ids_vanished:
        print("判定：SELF-TEST FAIL —— 覆盖度清单里的 id 在源码中已不存在（改名/删除需同步更新清单）：%s"
              % ids_vanished)
        return 1
    # ELF1~ELF6 现已由 _mk_elf() 的 struct 合成夹具覆盖（ELF 头/程序头/section headers/
    # .dynsym 全是可构造的确定字节结构，无需真实产物）。诚实缺口清单目前为空；
    # 若将来确属不可合成的组，必须在此显式登记并在下方输出里打印。
    KNOWN_UNCOVERED = set()
    covered = {c["cid"] for c in cases}
    required = ALL_CHECK_IDS - KNOWN_UNCOVERED
    missed = sorted(required - covered)
    if missed:
        print("判定：SELF-TEST FAIL —— 以下断言组没有自检用例（覆盖度闸门）：%s" % missed)
        print("       （确属无法合成夹具的组请加入 KNOWN_UNCOVERED 并说明理由，"
              "不得直接删要求）")
        return 1
    # 反向闸门：ALL_CHECK_IDS 里出现了脚本已不再 record 的 id（改名/删除）→ 也要提醒
    stale = sorted(KNOWN_UNCOVERED - ALL_CHECK_IDS)
    if stale:
        print("判定：SELF-TEST FAIL —— KNOWN_UNCOVERED 含不存在的 check id：%s" % stale)
        return 1
    print("覆盖断言组（%d/%d）：%s"
          % (len(covered), len(ALL_CHECK_IDS), ", ".join(sorted(covered))))
    if KNOWN_UNCOVERED:
        print("已知未覆盖（%d 组，本模式不伪造）：%s"
              % (len(KNOWN_UNCOVERED), ", ".join(sorted(KNOWN_UNCOVERED))))
    else:
        print("已知未覆盖：无（全部 %d 组断言组均已由合成夹具覆盖）" % len(ALL_CHECK_IDS))
    print("判定：SELF-TEST PASS —— 每条断言都在已知坏样本上 FAIL、在已知好样本上 PASS。")
    return 0


def _selftest_guarded():
    """--self-test 的入口（夹具由 _run_pair_quiet / run_self_test 各自清理）。"""
    return run_self_test()


def _patch_bytes(data, off):
    """在 off 处做一个 8B 改动，返回新产物（用于合成「已 patch」样本）。"""
    b = bytearray(data)
    b[off:off + 8] = b"SKPATCH!"
    return bytes(b)


def _mk_report_for(inp, out, **kw):
    return _mk_report(hashlib.sha256(inp).hexdigest(), len(inp),
                      hashlib.sha256(out).hexdigest(), len(out), **kw)


def _run_pair_quiet(inp, out, report=None, key=None, ndjson=None):
    """内存态跑 verify_pair 的断言链（夹具写临时文件），吞掉正常输出。"""
    tmpdir = tempfile.mkdtemp(prefix="sproot-selftest-")
    try:
        pi = os.path.join(tmpdir, "in.img")
        po = os.path.join(tmpdir, "out.img")
        with open(pi, "wb") as f:
            f.write(inp)
        with open(po, "wb") as f:
            f.write(out)
        rp = None
        if report is not None:
            rp = os.path.join(tmpdir, "report.json")
            with open(rp, "w", encoding="utf-8") as f:
                json.dump(report, f)
        nd = None
        if ndjson is not None:
            nd = os.path.join(tmpdir, "progress.ndjson")
            with open(nd, "w", encoding="utf-8") as f:
                f.write(ndjson)
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            verify_pair(pi, po, rp, key, nd)
        return buf.getvalue()
    finally:
        shutil.rmtree(tmpdir, ignore_errors=True)


def main(argv):
    ap = argparse.ArgumentParser(
        description="SPRoot 修补产物静态验证（NEW-DOC-06 §4 V 组 / §9 G-02 / task-10 样本）",
        add_help=True)
    ap.add_argument("--in", dest="inp", help="原厂内核 Image")
    ap.add_argument("--out", dest="outp", help="修补后内核")
    ap.add_argument("--report", help="<out>.report.json")
    ap.add_argument("--key",
                    help="48 位 Root 密钥。优先级：本参数 > report 顶层 root_key 回落。"
                         "引擎 build_report 会写顶层 root_key（48 字符明文，与 key_spec 同源），"
                         "故通常无需手传；仅在 report 缺失/字段丢失时才需显式给出。"
                         "若既有 report 又无可用 root_key，则 V2/V3/V6 记 WARN 并计入"
                         "「未校验」汇总，不会静默 SKIP 后仍判 PASS")
    ap.add_argument("--ndjson", help="引擎 --json 进度流")
    ap.add_argument("--bootimg", action="append", metavar="PATH",
                    help="原始 Android boot.img 断言（可重复）：样本基线与可解包性")
    ap.add_argument("--bootimg-diff", nargs=2, metavar=("ORIG", "NEW"),
                    help="修补产物 vs 原厂 boot.img 的 AVB 零触碰断言（V-24~V-28/RB1~RB6）")
    ap.add_argument("--partition-size", type=int, default=None,
                    help="分区大小（与 --bootimg-diff 合用，如 boot=100663296 / init_boot=8388608）")
    ap.add_argument("--elf", action="append", metavar="PATH",
                    help="ELF 产物断言（可重复）：G-02 门禁证据")
    ap.add_argument("--expect-symbol", action="append", metavar="SYM", default=None,
                    help="与 --elf 合用：断言导出符号存在（如 Java_me_sproot_patch_NativeEngine_patch）")
    ap.add_argument("--elf-form", choices=("exec", "jni"), default=None,
                    help="与 --elf 合用：断言 ELF 形态（exec=静态 ET_EXEC；jni=ET_DYN+PT_DYNAMIC）")
    ap.add_argument("--sample", metavar="NAME|all",
                    help="从 samples/manifest.json 加载样本并校验")
    ap.add_argument("--manifest", help="样本清单路径（默认 <repo>/samples/manifest.json）")
    ap.add_argument("--list-samples", action="store_true", help="列出样本清单")
    ap.add_argument("--self-test", action="store_true",
                    help="合成夹具自检：逐条断言跑正/负样本，任一断言退化成永远 PASS 则退出码非 0"
                         "（CI 硬门禁）")
    ap.add_argument("--quiet", action="store_true", help="只输出 FAIL/WARN")
    args = ap.parse_args(argv)

    # 模式 0：自检（CI 硬门禁；必须在其它模式之前，且不依赖 samples/）
    if args.self_test:
        return _selftest_guarded()

    # 模式 1：样本清单
    manifest_path = args.manifest or default_manifest_path()
    if args.list_samples:
        if not os.path.isfile(manifest_path):
            print("ERROR: 样本清单不存在：%s" % manifest_path, file=sys.stderr)
            return 2
        return list_samples(manifest_path)

    # 模式 2：按样本清单跑 V 组
    if args.sample:
        if not os.path.isfile(manifest_path):
            print("ERROR: 样本清单不存在：%s（先由 task-10 产出 samples/manifest.json）"
                  % manifest_path, file=sys.stderr)
            return 2
        try:
            samples = resolve_samples(manifest_path, args.sample)
        except ValueError as exc:
            print("ERROR: %s" % exc, file=sys.stderr)
            return 2
        rc_all = 0
        for s in samples:
            del results[:]
            print("\n===== 样本 %s（%s）=====" % (s["id"], s.get("note") or "无备注"))
            rc = 0
            if s.get("bootimg"):
                check_bootimg(s["bootimg"])
                rc = print_summary(args.quiet)
            if s.get("in") and s.get("out"):
                rc = max(rc, verify_pair(s["in"], s["out"], s.get("report"),
                                         args.key or s.get("key"), s.get("ndjson"), args.quiet))
            if not s.get("bootimg") and not (s.get("in") and s.get("out")):
                print("ERROR: 样本 %s 既无 bootimg 也无 in/out" % s["id"], file=sys.stderr)
                rc = 2
            rc_all = max(rc_all, 0 if rc == 0 else 1)
        print("\n样本判定：%s（%d 个样本）"
              % ("PASS" if rc_all == 0 else "FAIL", len(samples)))
        return rc_all

    # 模式 3a：修补产物 vs 原厂 boot.img（AVB 零触碰）
    if args.bootimg_diff:
        check_bootimg_diff(args.bootimg_diff[0], args.bootimg_diff[1], args.partition_size)
        return print_summary(args.quiet)

    # 模式 3b：原始 boot.img 断言
    if args.bootimg:
        for path in args.bootimg:
            check_bootimg(path)
        return print_summary(args.quiet)

    # 模式 4：ELF 产物断言
    if args.elf:
        for path in args.elf:
            check_elf(path, args.expect_symbol, args.elf_form)
        return print_summary(args.quiet)

    # 模式 5：单对镜像
    if not (args.inp and args.outp):
        print("ERROR: 需要 --in/--out、--bootimg、--bootimg-diff、--elf、--sample 或 --list-samples", file=sys.stderr)
        return 2
    return verify_pair(args.inp, args.outp, args.report, args.key, args.ndjson, args.quiet)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
