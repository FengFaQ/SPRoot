#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""G-04 静态可核：错误码 → 用户文案映射（文档 + 代码双查）

依据：NEW-DOC-04 §6.1（G-04 映射表，客户端职责）+ `newapp/.../core/Models.kt`（引擎码）
      + `newapp/.../core/ApdClient.kt`（AP 退出码）。UI 铁律：原始码与中文文案同时展示。

断言：
  EC1 文档 §6.1 表存在，且含全部必需码（引擎 101/102/201/202/210/211/220/230/240/250；
      AP 124/125/126/127；成功码 0）
  EC2 每个码在文档中有**中文文案**（含 CJK）
  EC3 引擎码在 Models.kt 中以常量出现，且文档里的 E_* 符号在代码中可见
  EC4 AP 码 124/125/126/127 在 ApdClient.kt 中以常量出现，对应中文关键词可见
  EC5 UI 双展示：AP 码在代码的中文文案里带码（如「（124）」）；缺失 → WARN（不阻断）

用法：
  python3 check_error_codes.py [--doc PATH] [--models PATH] [--apd PATH] [--quiet]
退出码：0=全 PASS（WARN 允许）；1=存在 FAIL；2=用法/环境错误
"""

import argparse
import json
import os
import re
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
# v2 layout: app/ (not newapp/app/)
DEF_DOC = os.path.join(ROOT, "docs", "NEW-DOC-04-客户端方案.md")
DEF_MODELS = os.path.join(ROOT, "app", "src", "main", "java", "com", "sproot",
                          "manager", "core", "Models.kt")
DEF_APD = os.path.join(ROOT, "app", "src", "main", "java", "com", "sproot",
                       "manager", "core", "ApdClient.kt")

ENGINE_CODES = [101, 102, 201, 202, 210, 211, 220, 230, 240, 250]
AP_CODES = [124, 125, 126, 127]
CJK = re.compile(r"[\u4e00-\u9fff]")
AP_KEYWORDS = {124: ["超时"], 125: ["密钥"], 126: ["不可执行"], 127: ["不存在"]}

rows = []


def rec(cid, status, msg):
    rows.append((cid, status, msg))


def read(path):
    with open(path, "r", encoding="utf-8", errors="replace") as f:
        return f.read()


def load_doc(path):
    """截取 §6.1 表格，返回 {code(str): (来源, 文案)}。"""
    txt = read(path)
    m = re.search(r"^###\s*6\.1.*$", txt, re.M)
    if not m:
        return None
    seg = txt[m.start():]
    seg = seg.split("\n---")[0]
    mapping = {}
    for line in seg.split("\n"):
        line = line.strip()
        if not line.startswith("|"):
            continue
        cells = [c.strip() for c in line.strip("|").split("|")]
        if len(cells) < 3:
            continue
        src, codes, text = cells[0], cells[1], cells[2]
        for code in re.findall(r"\d+", codes):
            mapping.setdefault(code, (src, text))
    return mapping


def main(argv):
    ap = argparse.ArgumentParser(description="G-04 错误码→文案映射静态核查")
    ap.add_argument("--doc", default=DEF_DOC)
    ap.add_argument("--models", default=DEF_MODELS)
    ap.add_argument("--apd", default=DEF_APD)
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--quiet", action="store_true")
    args = ap.parse_args(argv)

    for p in (args.doc, args.models, args.apd):
        if not os.path.isfile(p):
            print("ERROR: 文件不存在：%s" % p, file=sys.stderr)
            return 2

    doc = load_doc(args.doc)
    if doc is None:
        rec("EC1", "FAIL", "文档中找不到 §6.1 映射表（%s）" % args.doc)
        doc = {}
    models = read(args.models)
    apd = read(args.apd)

    # EC1 必需码齐全
    need = ["0"] + [str(c) for c in ENGINE_CODES + AP_CODES]
    missing = [c for c in need if c not in doc]
    if missing:
        rec("EC1", "FAIL", "§6.1 缺码：%s" % ", ".join(missing))
    else:
        rec("EC1", "PASS", "§6.1 含全部必需码（引擎 10 + AP 4 + 成功 0）")

    # EC2 每码中文文案
    no_cjk = [c for c in need if c in doc and not CJK.search(doc[c][1])]
    if no_cjk:
        rec("EC2", "FAIL", "以下码缺中文文案：%s" % ", ".join(no_cjk))
    else:
        rec("EC2", "PASS", "全部码均有中文用户文案")

    # EC3 引擎码 + E_* 符号在 Models.kt
    miss_code, miss_sym = [], []
    for c in ENGINE_CODES:
        if not re.search(r"\b%d\b" % c, models):
            miss_code.append(str(c))
        sym = re.search(r"(E_[A-Z_]+)", doc.get(str(c), ("", ""))[1])
        if sym and sym.group(1) not in models:
            miss_sym.append(sym.group(1))
    if miss_code:
        rec("EC3", "FAIL", "Models.kt 缺引擎码常量：%s" % ", ".join(miss_code))
    elif miss_sym:
        rec("EC3", "FAIL", "Models.kt 缺符号：%s" % ", ".join(miss_sym))
    else:
        rec("EC3", "PASS", "Models.kt 含全部引擎码常量与 E_* 符号")

    # EC4 AP 码 + 关键词在 ApdClient.kt
    miss_ap, miss_kw = [], []
    for c in AP_CODES:
        if not re.search(r"\b%d\b" % c, apd):
            miss_ap.append(str(c))
        for kw in AP_KEYWORDS[c]:
            if kw not in apd:
                miss_kw.append("%d:%s" % (c, kw))
    if miss_ap:
        rec("EC4", "FAIL", "ApdClient.kt 缺 AP 退出码：%s" % ", ".join(miss_ap))
    elif miss_kw:
        rec("EC4", "FAIL", "ApdClient.kt 缺关键文案关键词：%s" % ", ".join(miss_kw))
    else:
        rec("EC4", "PASS", "ApdClient.kt 含 124/125/126/127 常量与对应中文关键词")

    # EC5 UI 双展示（同串带码）—— 仅 WARN
    with_code = [c for c in AP_CODES if re.search(r"[（(]%d[）)]" % c, apd)]
    if len(with_code) == len(AP_CODES):
        rec("EC5", "PASS", "AP 错误文案同串带错误码（%s）" % with_code)
    else:
        rec("EC5", "WARN", "部分 AP 错误文案未同串带码（已有 %s，缺 %s）"
            % (with_code, [c for c in AP_CODES if c not in with_code]))

    fails = [r for r in rows if r[1] == "FAIL"]
    warns = [r for r in rows if r[1] == "WARN"]
    if args.json:
        print(json.dumps({"rows": rows, "fails": len(fails), "warns": len(warns)}, ensure_ascii=False, indent=2))
    else:
        print("== G-04 静态核查（§6.1 + Models.kt + ApdClient.kt） ==")
        for cid, status, msg in rows:
            if args.quiet and status == "PASS":
                continue
            print("[%s] %-5s %s" % (cid, status, msg))
        print("\n汇总：PASS=%d WARN=%d FAIL=%d" % (len(rows) - len(fails) - len(warns), len(warns), len(fails)))
    if fails:
        print("判定：FAIL（G-04 未达标）")
        return 1
    print("判定：PASS（G-04 静态可核通过）")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
