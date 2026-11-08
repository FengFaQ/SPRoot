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

另含两项工具模式（NEW-DOC-06 §9 G-02 门禁证据 / task-10 样本加载）：
  --elf <path>       ELF 产物断言（AArch64 / 静态性 / 依赖 / 可执行位 / PT_TLS），
                     可重复传入；用于 arm64 引擎产物（exec 与 JNI 两种形态）的 G-02 证据。
  --sample <NAME>    从 samples/manifest.json 加载样本三元组（in/out/report/key/ndjson），
                     NAME=all 时跑全部样本；--list-samples 列出可用样本。

用法：
  python3 verify_patch.py --in <原厂内核> --out <修补内核> --report <report.json> \
      [--key <48位密钥>] [--ndjson <progress.ndjson>] [--quiet]
  python3 verify_patch.py --elf <arm64 ELF> [--elf <另一个 ELF>]
  python3 verify_patch.py --sample <NAME|all> [--manifest <path>]
  python3 verify_patch.py --list-samples [--manifest <path>]

退出码：0=全 PASS；1=存在 FAIL；2=用法/环境错误
"""

import argparse
import hashlib
import json
import os
import struct
import sys

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


def record(cid, status, msg):
    results.append((cid, status, msg))


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
        record("V6", "SKIP", "未提供密钥（--key / report.root_key 均无），跳过前缀一致性")

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
    if report_path:
        report = load_json(report_path, "V3", "report")
        if report is not None:
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
    else:
        record("V2", "SKIP", "无密钥可用（--report 无 root_key 且未传 --key），跳过密钥内嵌检查")

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
    if fails:
        print("判定：FAIL（阻断，不可进入刷入序列）")
        return 1
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


def main(argv):
    ap = argparse.ArgumentParser(
        description="SPRoot 修补产物静态验证（NEW-DOC-06 §4 V 组 / §9 G-02 / task-10 样本）",
        add_help=True)
    ap.add_argument("--in", dest="inp", help="原厂内核 Image")
    ap.add_argument("--out", dest="outp", help="修补后内核")
    ap.add_argument("--report", help="<out>.report.json")
    ap.add_argument("--key", help="48 位 Root 密钥（缺省时尝试从 report.root_key 读取）")
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
    ap.add_argument("--quiet", action="store_true", help="只输出 FAIL/WARN")
    args = ap.parse_args(argv)

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
