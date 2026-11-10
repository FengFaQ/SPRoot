#!/usr/bin/env python3
"""Kotlin 具名参数 / 必填参数自检（推前自检 #2）。

背景（真实事故，2026-11-09）：
  - `FlashGuide` 形参改名 `onCopy`，调用点仍写 `onCopyCommand = ...`
    → CI 才报 `No parameter with name 'onCopyCommand' found`；
  - 同一次还在同一调用点漏了必填参数 → `No value passed for parameter 'onCopy'`。
本脚本在本地即可发现这两类（依据仓库内函数定义，不替代编译器）。

规则：
  1. 解析所有 `fun Name(...)` 的形参名，并区分是否带默认值（必填/可选）；
  2. 对 `Name(...)` 调用点：
     - 具名参数 `x = ...` 必须存在于某个同名定义；不存在则报错；
     - 该名字只有**唯一签名**时，按「位置参数填前 N 个形参 + 具名参数」计算，
       若仍缺必填形参 → 报错（可捕捉 `No value passed for parameter`）；
  3. 只是启发式（不处理重载解析/扩展接收者/类型推断），真正的编译仍以 GitHub Actions 为准。

用法：python3 quality/static/check_kt_signatures.py [--root app/src/main/java]
退出码：0 通过；1 发现问题。
"""
import argparse
import os
import re
import sys

DEFAULT_ROOT = "app/src/main/java"

# 与 Kotlin 标准库/常见作用域函数同名的标识符：仓库内同名成员函数（如 AppLog.error(tag,msg)）
# 会让"裸调用 error(...)"被误判，直接跳过这些名字。
IGNORE_NAMES = {
    "error", "require", "requireNotNull", "check", "checkNotNull", "println", "print",
    "listOf", "mapOf", "setOf", "arrayOf", "mutableListOf", "mutableMapOf", "lazy",
    "runCatching", "withContext", "with", "apply", "also", "let", "run", "repeat", "TODO",
}


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


def param_has_default(param):
    """形参文本里是否含顶层 `=`（默认值）；`->`/`==` 不算。"""
    depth = 0
    i = 0
    while i < len(param):
        c = param[i]
        if c in "([{<":
            depth += 1
        elif c in ")]}>":
            # 泛型 `>` 与箭头 `->` 里的 `>` 不是闭合括号：`->` 不计深度
            prev_ch = param[i - 1] if i > 0 else ""
            if not (c == ">" and prev_ch == "-"):
                depth = max(0, depth - 1)
        elif c == "=" and depth == 0:
            prev = param[i - 1] if i > 0 else ""
            nxt = param[i + 1] if i + 1 < len(param) else ""
            if prev not in "=!<>" and nxt != "=":
                return True
        i += 1
    return False


def mask_code(src):
    """把字符串/字符/注释内容替换成空格（保留换行与长度），用于定位"代码里的调用点"。

    否则三引号字符串里的字样会被误当成调用（真实误报：
    AppLog 的 session 头模板里含 write( 文本）。
    """
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
            out[i] = " "
            if src.startswith('"""', i):
                out[i + 1] = out[i + 2] = " "
                i += 3
                in_triple = False
                continue
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
            # 字符字面量（'x'）；Kotlin 标签不以 ' 结尾于 2 字符内
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


def collect_defs(sources):
    """name -> list of ordered param names（去重相同签名）。"""
    defs = {}
    for path, s in sources.items():
        masked = mask_code(s)
        for m in re.finditer(r"\bfun\s+(?:<[^>]+>\s+)?(\w+)\s*\(", masked):
            name = m.group(1)
            op = m.end() - 1
            cp = match_paren(s, op)
            if cp < 0:
                continue
            ordered = []
            for prm in split_top(s[op + 1:cp]):
                mm = re.match(r"(?:@\w+(?:\([^)]*\))?\s+)*(?:vararg\s+)?(\w+)\s*:", prm)
                if mm:
                    ordered.append((mm.group(1), not param_has_default(prm)))
            if not ordered:
                continue
            signatures = defs.setdefault(name, [])
            key = tuple(n for n, _ in ordered)
            if all(tuple(n for n, _ in sig) != key for sig in signatures):
                signatures.append(ordered)
    return defs


def check_call(name, args, signatures, has_trailing_lambda=False):
    """返回问题列表；signatures 是同名函数形参列表（可能多份=重载）。"""
    problems = []
    named = []
    positional = 0
    seen_named = False
    for a in args:
        am = re.match(r"^(\w+)\s*=(?!=)", a)
        if am:
            seen_named = True
            named.append(am.group(1))
        elif not seen_named:
            positional += 1
        # 位置参数出现在具名参数之后 = 语法错误，交给编译器
    all_param_names = set()
    for sig in signatures:
        all_param_names |= {n for n, _ in sig}
    for n in named:
        if n not in all_param_names:
            problems.append(
                "named arg '%s' not in defined params %s" % (n, sorted(all_param_names))
            )
    if len(signatures) == 1:
        sig = signatures[0]
        provided = set(named)
        for idx, (pname, _required) in enumerate(sig):
            if idx < positional:
                provided.add(pname)
        if has_trailing_lambda and sig:
            provided.add(sig[-1][0])
        missing = [n for n, required in sig if required and n not in provided]
        if missing:
            problems.append("missing required param(s): %s" % missing)
    return problems


def main():
    ap = argparse.ArgumentParser(description="Kotlin 具名参数/必填参数自检")
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
        masked = mask_code(s)
        for name, signatures in defs.items():
            if name in IGNORE_NAMES:
                continue
            for m in re.finditer(r"(?<![.\w])" + re.escape(name) + r"\s*\(", masked):
                if s[max(0, m.start() - 4):m.start()] == "fun ":
                    continue
                op = m.end() - 1
                cp = match_paren(s, op)
                if cp < 0:
                    continue
                call_args = split_top(s[op + 1:cp])
                if not call_args:
                    continue
                tail = s[cp + 1:].lstrip()
                has_trailing_lambda = tail.startswith("{")
                problems = check_call(name, call_args, signatures, has_trailing_lambda)
                if problems:
                    line = s[:m.start()].count("\n") + 1
                    bad += 1
                    print("%s:%d: call %s(...): %s" % (path, line, name, "; ".join(problems)))
    print("check_kt_signatures: %d function(s), %d problem(s)" % (len(defs), bad))
    return 0 if bad == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
