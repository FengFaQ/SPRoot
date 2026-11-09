#!/usr/bin/env python3
"""Kotlin 源码语法自检（推前自检 #1）—— 字符串/三引号/括号配对。

背景：真实事故 run 42636829029 —— Kotlin 字符串模板里裸写双引号
    VerifyResult(true, "$RCFILE 与 "$IMPORT_LINE" 均 ...")   # 内层引号提前终止字符串
在 CI（唯一构建源）里才暴露，浪费了一轮 4–5 分钟反馈周期。本脚本用零依赖的
词法扫描提前发现这类问题（不替代编译器，只拦"低级但高频"的语法错误）。

检查项：
  1. 普通字符串 / 字符字面量 / 三引号字符串 是否闭合；
  2. `()` `[]` `{}` 是否配对（忽略字符串、注释内的括号）；
  3. 块注释是否闭合；
  4. **字符串字面量后紧跟标识符/`$`**（= 极可能漏转义的嵌套双引号）：
     `"$A 与 "$B" 均在"` 在词法上被切成 3 个字符串，括号/引号都是平衡的，
     编译器才会报 "unexpected token"。本检查专门拦这一类（真实事故形态）。

用法：
    python3 quality/static/check_kt_syntax.py [--root app/src/main/java]
退出码：0 = 通过；1 = 发现问题（逐条打印 file:line）。
"""
import argparse
import os
import sys

DEFAULT_ROOT = "app/src/main/java"


def check_file(path):
    src = open(path, encoding="utf-8").read()
    n = len(src)
    i = 0
    line = 1
    stack = []
    problems = []
    in_triple = in_str = in_char = in_line_comment = in_block_comment = False
    while i < n:
        c = src[i]
        nxt = src[i + 1] if i + 1 < n else ""
        if c == "\n":
            in_line_comment = False
            line += 1
            i += 1
            continue
        if in_line_comment:
            i += 1
            continue
        if in_block_comment:
            if c == "*" and nxt == "/":
                in_block_comment = False
                i += 2
                continue
            i += 1
            continue
        if in_triple:
            if src.startswith('"""', i):
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
                # 真实事故形态：字符串闭合后**紧邻** `$` 或标识符（无运算符），
                # 说明内层双引号没转义（`"$A 与 "$B" 均"` 会被切成 3 段）。
                follow = src[i + 1] if i + 1 < n else ""
                # 只认 ASCII 标识符/`$`：Kotlin 字符串模板里允许嵌套字符串
                # （`"…${x ?: "默认值"}"`），中文紧邻是模板常见形态，不能误报。
                if follow == "$" or follow == "_" or (follow.isascii() and follow.isalpha()):
                    problems.append(
                        (line, "string literal immediately followed by %r "
                               "(unescaped nested quote? write \" or omit quotes)" % follow)
                    )
            i += 1
            continue
        if in_char:
            if c == "\\":
                i += 2
                continue
            if c == "'":
                in_char = False
            i += 1
            continue
        if c == "/" and nxt == "/":
            in_line_comment = True
            i += 2
            continue
        if c == "/" and nxt == "*":
            in_block_comment = True
            i += 2
            continue
        if src.startswith('"""', i):
            in_triple = True
            i += 3
            continue
        if c == '"':
            in_str = True
            i += 1
            continue
        if c == "'":
            # 字符字面量（'x' / '\n'）；Kotlin 的标签 'label@' 不会以 ' 结尾于 2 字符内
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
                i = j + 1
                continue
            i += 1
            continue
        if c in "([{":
            stack.append((c, line))
        elif c in ")]}":
            pairs = {")": "(", "]": "[", "}": "{"}
            if not stack:
                problems.append((line, "unexpected closing '%s'" % c))
            else:
                top, opened_line = stack.pop()
                if top != pairs[c]:
                    problems.append(
                        (line, "'%s' closes '%s' opened at line %d" % (c, top, opened_line))
                    )
        i += 1
    if in_triple:
        problems.append((line, "unterminated triple-quoted string"))
    if in_str:
        problems.append((line, "unterminated string literal"))
    if in_block_comment:
        problems.append((line, "unterminated block comment"))
    if stack:
        problems.append((line, "%d unclosed bracket(s), innermost %r" % (len(stack), stack[-1])))
    return problems


def main():
    ap = argparse.ArgumentParser(description="Kotlin 字符串/括号配对自检")
    ap.add_argument("--root", default=DEFAULT_ROOT, help="扫描根目录（默认 app/src/main/java）")
    args = ap.parse_args()
    if not os.path.isdir(args.root):
        print("::error::扫描目录不存在：%s" % args.root)
        return 1
    total = 0
    bad = 0
    for dirpath, _dirs, files in os.walk(args.root):
        for name in files:
            if not name.endswith(".kt"):
                continue
            total += 1
            path = os.path.join(dirpath, name)
            for line, msg in check_file(path):
                bad += 1
                print("%s:%d: %s" % (path, line, msg))
    print("check_kt_syntax: %d Kotlin file(s), %d problem(s)" % (total, bad))
    return 0 if bad == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
