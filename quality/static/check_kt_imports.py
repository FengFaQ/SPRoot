#!/usr/bin/env python3
"""Kotlin import 自检（推前自检 #3）：跨包缺 import / 库扩展函数缺 import / 禁用 material-icons。

覆盖的三类真实事故（2026-11-09/10，均在 CI 才暴露）：
  1. 跨包缺 import：`ui/pages/PatchPage.kt` 用 `WarningCard`（定义在 ui.components）却没导入；
  2. 库扩展函数缺 import：`AuthPage.kt` 的 `DrawablePainter` 用 `drawIntoCanvas` 却没导入
     `androidx.compose.ui.graphics.drawscope.drawIntoCanvas` → `Unresolved reference`；
  3. 图标来源失控：曾用 material-icons-extended（APK +4.7MB）且 `Icons.Rounded.X` 引用不存在的
     图标只在编译期暴露 → 现在**全面禁用 `Icons.*`**，统一走 miuix-icons 或自绘 ImageVector。

规则：
  A. 大写类型/Composable 定义在别的包、使用点未 import（同包/全限定名/注释内除外）；
  B. 已知库扩展函数（KNOWN_EXTENSION_IMPORTS）被使用但缺对应 import；
  D. **本项目自有扩展函数**（`fun Context.xxx(...)` 等）跨包使用必须 import
     （真实事故：ModulesPage 用 `context.copyToClipboard(...)` 却删掉了 import）；
  C. 源码出现 `Icons.Rounded|Filled|Outlined|Default|AutoMirrored` 或 import
     `androidx.compose.material.icons.*` → 直接失败（改用 `MiuixIcons.*`）。
字符串/注释内容不参与匹配（避免文档示例误报）。

用法：python3 quality/static/check_kt_imports.py [--root app/src/main/java]
退出码：0 通过；1 发现问题。
"""
import argparse
import os
import re
import sys

DEFAULT_ROOT = "app/src/main/java"

# 已知"必须显式 import 的库扩展函数/属性" → 正确 import 全名。
# 只收无歧义的名字（`size`/`alpha`/`offset` 这类与集合属性/作用域成员同名的容易误报，不收）。
# 原则：每发生一次"漏 import 扩展函数"事故，就往这里补一条。
KNOWN_EXTENSION_IMPORTS = {
    # Compose UI / Foundation 扩展（本次事故：drawIntoCanvas）
    "drawIntoCanvas": "androidx.compose.ui.graphics.drawscope.drawIntoCanvas",
    "nativeCanvas": "androidx.compose.ui.graphics.nativeCanvas",
    "drawWithContent": "androidx.compose.ui.draw.drawWithContent",
    "drawBehind": "androidx.compose.ui.draw.drawBehind",
    "drawWithCache": "androidx.compose.ui.draw.drawWithCache",
    "graphicsLayer": "androidx.compose.ui.graphics.graphicsLayer",
    "clip": "androidx.compose.ui.draw.clip",
    "border": "androidx.compose.foundation.border",
    "background": "androidx.compose.foundation.background",
    "clickable": "androidx.compose.foundation.clickable",
    "combinedClickable": "androidx.compose.foundation.combinedClickable",
    "horizontalScroll": "androidx.compose.foundation.horizontalScroll",
    "verticalScroll": "androidx.compose.foundation.verticalScroll",
    "nestedScroll": "androidx.compose.ui.input.nestedscroll.nestedScroll",
    "painterResource": "androidx.compose.ui.res.painterResource",
    "stringResource": "androidx.compose.ui.res.stringResource",
    "rememberLauncherForActivityResult": "androidx.activity.compose.rememberLauncherForActivityResult",
    "rememberSaveable": "androidx.compose.runtime.saveable.rememberSaveable",
    "toArgb": "androidx.compose.ui.graphics.toArgb",
    # miuix 扩展
    "scrollEndHaptic": "top.yukonga.miuix.kmp.utils.scrollEndHaptic",
    "overScrollVertical": "top.yukonga.miuix.kmp.utils.overScrollVertical",
    "layerBackdrop": "top.yukonga.miuix.kmp.blur.layerBackdrop",
    "textureBlur": "top.yukonga.miuix.kmp.blur.textureBlur",
    "rememberLayerBackdrop": "top.yukonga.miuix.kmp.blur.rememberLayerBackdrop",
    "springAnimateToPage": "top.yukonga.miuix.kmp.utils.springAnimateToPage",
}

# 禁止出现：material icons（已移除依赖；图标统一 miuix-icons / 自绘 ImageVector）
BANNED_ICON_PATTERN = re.compile(r"\bIcons\.(Rounded|Filled|Outlined|Default|AutoMirrored)\b")
BANNED_ICON_IMPORT = re.compile(r"^\s*import\s+androidx\.compose\.material\.icons", re.M)


def mask_code(src):
    """把字符串/字符/注释内容替换为空格（等长、保留换行）；只用于"代码区"匹配。"""
    out = list(src)
    n = len(src)
    i = 0
    in_triple = in_str = in_char = in_line = in_block = False
    while i < n:
        c = src[i]
        nxt = src[i + 1] if i + 1 < n else ""
        if c == "\n":
            in_line = False
            i += 1
            continue
        if in_line:
            out[i] = " "
            i += 1
            continue
        if in_block:
            out[i] = " "
            if c == "*" and nxt == "/":
                out[i + 1] = " "
                i += 2
                in_block = False
                continue
            i += 1
            continue
        if in_triple:
            if src.startswith('"""', i):
                out[i] = out[i + 1] = out[i + 2] = " "
                i += 3
                in_triple = False
                continue
            out[i] = " "
            i += 1
            continue
        if in_str:
            out[i] = " "
            if c == "\\":
                if i + 1 < n:
                    out[i + 1] = " "
                i += 2
                continue
            if c == '"':
                in_str = False
            i += 1
            continue
        if in_char:
            out[i] = " "
            if c == "\\":
                if i + 1 < n:
                    out[i + 1] = " "
                i += 2
                continue
            if c == "'":
                in_char = False
            i += 1
            continue
        if c == "/" and nxt == "/":
            in_line = True
            out[i] = out[i + 1] = " "
            i += 2
            continue
        if c == "/" and nxt == "*":
            in_block = True
            out[i] = out[i + 1] = " "
            i += 2
            continue
        if src.startswith('"""', i):
            in_triple = True
            out[i] = out[i + 1] = out[i + 2] = " "
            i += 3
            continue
        if c == '"':
            in_str = True
            out[i] = " "
            i += 1
            continue
        if c == "'":
            j = i + 1
            closed = False
            steps = 0
            while j < n and steps < 3:
                if src[j] == "\\":
                    j += 2
                    continue
                if src[j] == "'":
                    closed = True
                    break
                if src[j] == "\n":
                    break
                j += 1
                steps += 1
            if closed:
                for k in range(i, j + 1):
                    out[k] = " "
                i = j + 1
                continue
            i += 1
            continue
        i += 1
    return "".join(out)


def main():
    ap = argparse.ArgumentParser(description="Kotlin import 自检")
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
    own_extensions = {}  # name -> set(package)（本项目自有扩展函数，必须显式 import）
    for path, s in sources.items():
        masked = mask_code(s)
        m = re.search(r"^\s*package\s+([\w.]+)", s, re.M)
        pkg = m.group(1) if m else ""
        for m2 in re.finditer(
            r"^\s*(?:@\w+(?:\([^)]*\))?\s+)*"
            r"(?:private\s+|internal\s+|public\s+)?(?:data\s+)?(?:sealed\s+)?"
            r"(?:enum\s+)?(?:annotation\s+)?(?:class|interface|object)\s+(\w+)",
            masked,
            re.M,
        ):
            defs.setdefault(m2.group(1), set()).add(pkg)
        for m2 in re.finditer(
            r"^\s*(?:@\w+(?:\([^)]*\))?\s+)*(?:suspend\s+)?fun\s+([A-Z]\w*)\s*\(",
            masked,
            re.M,
        ):
            defs.setdefault(m2.group(1), set()).add(pkg)
        for m2 in re.finditer(
            r"^[ \t]*(private\s+|internal\s+|public\s+)?(?:suspend\s+|inline\s+)*"
            r"fun\s+[A-Za-z0-9_<>?, ]+?\.([a-z_]\w*)\s*\(",
            masked,
            re.M,
        ):
            if (m2.group(1) or "").strip() == "private":
                continue
            own_extensions.setdefault(m2.group(2), set()).add(pkg)

    bad = 0
    for path, s in sources.items():
        masked = mask_code(s)
        m = re.search(r"^\s*package\s+([\w.]+)", s, re.M)
        pkg = m.group(1) if m else ""
        imports = set()
        for m2 in re.finditer(r"^\s*import\s+([\w.]+)(?:\s+as\s+(\w+))?", s, re.M):
            imports.add(m2.group(2) or m2.group(1).split(".")[-1])

        # A) 跨包缺 import
        for name, pkgs in defs.items():
            if pkg in pkgs or name in imports:
                continue
            for idx, text in enumerate(masked.split("\n"), start=1):
                if not text.strip():
                    continue
                if re.search(r"\b(fun|class|interface|object)\s+" + re.escape(name) + r"\b", text):
                    continue
                if not re.search(
                    r"(?<![\w.])" + re.escape(name) + r"\s*\(|\b:\s*" + re.escape(name) + r"\b", text
                ):
                    continue
                if re.search(r"\b" + re.escape(pkg) + r"\." + re.escape(name), text):
                    continue
                if re.search(r"\b\w+\." + re.escape(name), text):
                    continue
                bad += 1
                print("%s:%d: uses '%s' defined in %s but no import" % (path, idx, name, sorted(pkgs)))
                break

        # D) 本项目自有扩展函数跨包缺 import
        for name, pkgs in own_extensions.items():
            if pkg in pkgs or name in imports:
                continue
            if re.search(r"\.\s*" + re.escape(name) + r"\s*\(", masked):
                bad += 1
                print(
                    "%s: uses own extension '%s' defined in %s but no import"
                    % (path, name, sorted(pkgs))
                )

        # B) 已知库扩展函数缺 import
        for name, full in KNOWN_EXTENSION_IMPORTS.items():
            if name in imports:
                continue
            if re.search(
                r"(?<![\w.])" + re.escape(name) + r"\s*\(|\.\s*" + re.escape(name) + r"\b",
                masked,
            ):
                bad += 1
                print("%s: uses extension '%s' but missing import %s" % (path, name, full))

        # C) 禁用 material icons（依赖已移除，图标统一 miuix-icons / 自绘）
        for m2 in BANNED_ICON_PATTERN.finditer(masked):
            line = masked[:m2.start()].count("\n") + 1
            bad += 1
            print(
                "%s:%d: 禁止 material Icons（%s）—— 改用 miuix-icons（MiuixIcons.*）或自绘 ImageVector"
                % (path, line, m2.group(0))
            )
            break
        if BANNED_ICON_IMPORT.search(s):
            bad += 1
            print("%s: 禁止 import androidx.compose.material.icons.*（依赖已移除）" % path)

    print("check_kt_imports: %d definition(s), %d problem(s)" % (len(defs), bad))
    return 0 if bad == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
