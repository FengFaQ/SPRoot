#!/usr/bin/env python3
"""Kotlin 跨包缺 import 自检（推前自检 #3）—— 同类事故：组件在 ui.components，页面在 ui.pages。

背景：真实事故（P2 双镜像提交）u/pages/PatchPage.kt 使用 `WarningCard`/`WarningLevel`
（定义在 ui.components）但漏了 import → CI 才报 `Unresolved reference`。
本脚本用"定义包 vs 使用点是否 import"的启发式提前发现。

规则（保守，只查大写的类型/Composable 名，避免误报）：
  1. 收集仓库内所有 `class/interface/object` 与首字母大写的 `fun` 定义及其包；
  2. 对每个 .kt 文件，若使用了某名字（`Name(` 调用或 `: Name` 类型位置），且该名字
     定义在**别的包**、文件既未 import 该名字、也未写全限定名 → 报 file:line；
  3. 跳过注释行；同文件内声明行也跳过。

用法：
    python3 quality/static/check_kt_imports.py [--root app/src/main/java]
退出码：0 = 通过；1 = 发现问题。
"""
import argparse
import os
import re
import sys

DEFAULT_ROOT = "app/src/main/java"


def main():
    ap = argparse.ArgumentParser(description="Kotlin 跨包缺 import 自检")
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

    defs = {}  # name -> set(package)
    for path, s in sources.items():
        m = re.search(r"^\s*package\s+([\w.]+)", s, re.M)
        pkg = m.group(1) if m else ""
        for m2 in re.finditer(
            r"^\s*(?:@\w+(?:\([^)]*\))?\s+)*"
            r"(?:private\s+|internal\s+|public\s+)?(?:data\s+)?(?:sealed\s+)?"
            r"(?:enum\s+)?(?:annotation\s+)?(?:class|interface|object)\s+(\w+)",
            s,
            re.M,
        ):
            defs.setdefault(m2.group(1), set()).add(pkg)
        for m2 in re.finditer(r"^\s*(?:@\w+(?:\([^)]*\))?\s+)*(?:suspend\s+)?fun\s+([A-Z]\w*)\s*\(", s, re.M):
            defs.setdefault(m2.group(1), set()).add(pkg)

    bad = 0
    for path, s in sources.items():
        m = re.search(r"^\s*package\s+([\w.]+)", s, re.M)
        pkg = m.group(1) if m else ""
        imports = set()
        for m2 in re.finditer(r"^\s*import\s+([\w.]+)(?:\s+as\s+(\w+))?", s, re.M):
            imports.add(m2.group(2) or m2.group(1).split(".")[-1])
        lines = s.split("\n")
        for name, pkgs in defs.items():
            if pkg in pkgs or name in imports:
                continue
            for idx, text in enumerate(lines, start=1):
                stripped = text.strip()
                if stripped.startswith("//") or stripped.startswith("*") or stripped.startswith("/*"):
                    continue
                if re.search(r"\b(fun|class|interface|object)\s+" + re.escape(name) + r"\b", text):
                    continue
                if not re.search(r"(?<![\w.])" + re.escape(name) + r"\s*\(|\b:\s*" + re.escape(name) + r"\b", text):
                    continue
                if re.search(r"\b" + re.escape(pkg) + r"\." + re.escape(name), text):
                    continue
                if re.search(r"\b\w+\." + re.escape(name), text):
                    continue
                bad += 1
                print(
                    "%s:%d: uses '%s' defined in %s but no import"
                    % (path, idx, name, sorted(pkgs))
                )
                break
    print("check_kt_imports: %d definition(s), %d problem(s)" % (len(defs), bad))
    return 0 if bad == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
