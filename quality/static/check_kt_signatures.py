#!/usr/bin/env python3
"""Kotlin 具名参数自检（推前自检 #2）—— 调用点的具名参数必须存在于被调函数的定义里。

背景：真实事故（P2 双镜像提交）`FlashGuide` 形参已改名为 onCopy，调用点仍写
`onCopyCommand = ...` → CI 才报 `No parameter with name 'onCopyCommand' found`。
本脚本在本地即可发现「改名后漏改调用点」这类错误。

做法（零依赖、纯源码）：
  1. 扫描仓库内所有 `fun Name(...)` 定义，收集每个函数的形参名（含默认值/注解/vararg）；
  2. 扫描所有 `Name(...)` 调用点，若某个位置参数是 `x = ...` 形式且 `x` 不在形参名集合中，
     报 file:line；
  3. 同名函数多处定义时取并集（本项目没有重载冲突的公开 API）。

用法：
    python3 quality/static/check_kt_signatures.py [--root app/src/main/java]
退出码：0 = 通过；1 = 发现问题。
"""
import argparse
import os
import re
import sys

DEFAULT_ROOT = "app/src/main/java"


def match_paren(s, open_idx):
    depth = 0
    i = open_idx
    in_str = in_triple = False
    while i < len(s):
        c = s[i]
        if in_triple:
            if s.startswith('"""', i):
                in_triple = False
                i += 3
                continue
            i += 1
            continue
        if in_str:
            if c == "\\":
                i += 2
                continue
            if c == '"':
                in_str = False
            i += 1
            continue
        if s.startswith('"""', i):
            in_triple = True
            i += 3
            continue
        if c == '"':
            in_str = True
            i += 1
            continue
        if c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
            if depth == 0:
                return i
        i += 1
    return -1


def split_top(s):
    parts = []
    depth = 0
    cur = ""
    in_str = in_triple = False
    i = 0
    while i < len(s):
        c = s[i]
        if in_triple:
            if s.startswith('"""', i):
                in_triple = False
                cur += '"""'
                i += 3
                continue
            cur += c
            i += 1
            continue
        if in_str:
            if c == "\\":
                cur += s[i:i + 2]
                i += 2
                continue
            if c == '"':
                in_str = False
            cur += c
            i += 1
            continue
        if s.startswith('"""', i):
            in_triple = True
            cur += '"""'
            i += 3
            continue
        if c == '"':
            in_str = True
            cur += c
            i += 1
            continue
        if c in "([{":
            depth += 1
        elif c in ")]}":
            depth -= 1
        if c == "," and depth == 0:
            parts.append(cur)
            cur = ""
            i += 1
            continue
        cur += c
        i += 1
    if cur.strip():
        parts.append(cur)
    return [p.strip() for p in parts if p.strip()]


def collect_defs(sources):
    defs = {}
    for path, s in sources.items():
        for m in re.finditer(r"\bfun\s+(?:<[^>]+>\s+)?(\w+)\s*\(", s):
            name = m.group(1)
            op = m.end() - 1
            cp = match_paren(s, op)
            if cp < 0:
                continue
            names = set()
            for prm in split_top(s[op + 1:cp]):
                mm = re.match(r"(?:@\w+(?:\([^)]*\))?\s+)*(?:vararg\s+)?(\w+)\s*:", prm)
                if mm:
                    names.add(mm.group(1))
            defs.setdefault(name, set()).update(names)
    return defs


def main():
    ap = argparse.ArgumentParser(description="Kotlin 具名参数 vs 函数定义自检")
    ap.add_argument("--root", default=DEFAULT_ROOT)
    args = ap.parse_args()
    if not os.path.isdir(args.root):
        print("::error::扫描目录不存在：%s" % args.root)
        return 1
    sources = {}
    for dirpath, _dirs, files in os.walk(args.root):
        for name in files:
            if name.endswith(".kt"):
                p = os.path.join(dirpath, name)
                sources[p] = open(p, encoding="utf-8").read()

    defs = collect_defs(sources)
    bad = 0
    for path, s in sources.items():
        for name, params in defs.items():
            for m in re.finditer(r"(?<!fun )\b" + re.escape(name) + r"\s*\(", s):
                op = m.end() - 1
                cp = match_paren(s, op)
                if cp < 0:
                    continue
                for a in split_top(s[op + 1:cp]):
                    am = re.match(r"^(\w+)\s*=(?!=)", a)
                    if am and am.group(1) not in params:
                        line = s[:m.start()].count("\n") + 1
                        bad += 1
                        print(
                            "%s:%d: call %s(...) uses named arg '%s' "
                            "(defined params: %s)" % (path, line, name, am.group(1), sorted(params))
                        )
    print("check_kt_signatures: %d function(s), %d problem(s)" % (len(defs), bad))
    return 0 if bad == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
