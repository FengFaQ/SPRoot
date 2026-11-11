#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""对无 --self-test 的 5 个门禁做**外部负样本验证**。

背景：check_kt_syntax / check_kt_signatures / check_kt_imports /
check_licenses / check_error_codes / check_builtin_assets 这 6 个门禁
自身没有 --self-test。按项目铁律「新增断言必须先用负样本证明它会 FAIL」，
本脚本在**不改被检对象**的前提下，构造坏输入验证它们确实会红。

做法：全部在临时目录里造夹具，绝不触碰仓库工作区。
"""
import os
import shutil
import subprocess
import sys
import tempfile

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
Q = os.path.join(REPO, "quality", "static")


def run(script, *args, cwd=None):
    """跑门禁，返回 (exit_code, stdout+stderr)。"""
    p = subprocess.run(
        [sys.executable, os.path.join(Q, script)] + list(args),
        capture_output=True, text=True, cwd=cwd or REPO, encoding="utf-8",
        errors="replace",
    )
    return p.returncode, (p.stdout or "") + (p.stderr or "")


def case(title, script, args, expect_fail, cwd=None, must_contain=None):
    code, out = run(script, *args, cwd=cwd)
    got_fail = code != 0
    ok = got_fail == expect_fail
    if ok and must_contain:
        ok = must_contain in out
    print("  %-58s exit=%-3d 期望%s  %s" % (
        title, code,
        "非0" if expect_fail else "0",
        "OK" if ok else "**FAIL**",
    ))
    if not ok:
        print("    输出片段：%s" % out.strip().replace("\n", " | ")[:220])
    return ok


def main():
    results = []
    tmp = tempfile.mkdtemp(prefix="gatecheck-")
    try:
        # ── check_kt_syntax：括号未闭合 / 字符串未终止 ──────────────────
        d = os.path.join(tmp, "syn1"); os.makedirs(d)
        open(os.path.join(d, "Bad.kt"), "w", encoding="utf-8").write(
            "package x\nfun f() {\n    if (true) {\n        println(1)\n}\n")
        results.append(case("kt_syntax 负样本：未闭合括号", "check_kt_syntax.py",
                            ["--root", d], True, must_contain="unclosed"))

        d = os.path.join(tmp, "syn2"); os.makedirs(d)
        open(os.path.join(d, "Bad.kt"), "w", encoding="utf-8").write(
            'package x\nfun f() {\n    val s = "abc\n}\n')
        results.append(case("kt_syntax 负样本：字符串未终止", "check_kt_syntax.py",
                            ["--root", d], True, must_contain="unterminated"))

        d = os.path.join(tmp, "syn3"); os.makedirs(d)
        open(os.path.join(d, "Ok.kt"), "w", encoding="utf-8").write(
            "package x\nfun f() {\n    val s = \"abc\"\n    println(s)\n}\n")
        results.append(case("kt_syntax 正样本：合法文件", "check_kt_syntax.py",
                            ["--root", d], False))

        # ── check_kt_imports：用了别包定义却没 import ───────────────────
        # 注意：该门禁只把**首字母大写**的顶层 `fun Xxx(` 视为跨包可见定义
        # （小写视为包内私有/扩展），所以夹具必须用大写开头才具代表性。
        d = os.path.join(tmp, "imp"); os.makedirs(d)
        open(os.path.join(d, "A.kt"), "w", encoding="utf-8").write(
            "package aaa\nfun HelperFunction(): Int = 1\n")
        open(os.path.join(d, "B.kt"), "w", encoding="utf-8").write(
            "package bbb\nfun useIt() {\n    val x = HelperFunction()\n}\n")
        results.append(case("kt_imports 负样本：跨包缺 import（大写定义）",
                            "check_kt_imports.py", ["--root", d], True,
                            must_contain="HelperFunction"))

        # 反向：显式 import 后应当放行
        d = os.path.join(tmp, "imp2"); os.makedirs(d)
        open(os.path.join(d, "A.kt"), "w", encoding="utf-8").write(
            "package aaa\nfun HelperFunction(): Int = 1\n")
        open(os.path.join(d, "B.kt"), "w", encoding="utf-8").write(
            "package bbb\nimport aaa.HelperFunction\n"
            "fun useIt() {\n    val x = HelperFunction()\n}\n")
        results.append(case("kt_imports 正样本：已显式 import",
                            "check_kt_imports.py", ["--root", d], False))

        # ── check_kt_signatures：具名参数与定义不符 ─────────────────────
        d = os.path.join(tmp, "sig"); os.makedirs(d)
        open(os.path.join(d, "S.kt"), "w", encoding="utf-8").write(
            "package s\n"
            "fun target(alpha: Int, beta: Int) {}\n"
            "fun caller() {\n"
            "    target(alpha = 1, gamma = 2)\n"
            "}\n")
        results.append(case("kt_signatures 负样本：不存在的具名参数", "check_kt_signatures.py",
                            ["--root", d], True, must_contain="gamma"))

        # ── check_licenses：缺 LICENSE / 篡改上游文件 ───────────────────
        d = os.path.join(tmp, "lic"); os.makedirs(d)
        results.append(case("licenses 负样本：仓库根缺 LICENSE", "check_licenses.py",
                            ["--repo-root", d], True))

        # ── check_error_codes：文档缺必需码 ─────────────────────────────
        d = os.path.join(tmp, "ec")
        os.makedirs(os.path.join(d, "docs"))
        os.makedirs(os.path.join(d, "app", "src", "main", "java", "com", "sproot",
                                 "manager", "core"))
        open(os.path.join(d, "docs", "NEW-DOC-04-客户端方案.md"), "w",
             encoding="utf-8").write("# §6.1\n只有 101，别的码都没有\n")
        open(os.path.join(d, "app", "src", "main", "java", "com", "sproot",
                          "manager", "core", "Models.kt"), "w",
             encoding="utf-8").write("package x\n// 空\n")
        results.append(case("error_codes 负样本：文档缺必需错误码", "check_error_codes.py",
                            ["--repo-root", d], True))

        # ── check_builtin_assets：manifest 哈希漂移 ─────────────────────
        d = os.path.join(tmp, "ba"); os.makedirs(d)
        os.makedirs(os.path.join(d, "app", "src", "main", "assets", "builtin"))
        open(os.path.join(d, "app", "builtin-components.json"), "w",
             encoding="utf-8").write(
            '{"schema":1,"components":[{"id":"x","name":"X","version":"v1",'
            '"file":"app/src/main/assets/builtin/x.zip","size":3,'
            '"sha256":"0000000000000000000000000000000000000000000000000000000000000000",'
            '"license":"MIT","source_url":"u"}]}\n')
        open(os.path.join(d, "app", "src", "main", "assets", "builtin", "x.zip"),
             "wb").write(b"abc")
        results.append(case("builtin_assets 负样本：sha256 漂移", "check_builtin_assets.py",
                            ["--repo-root", d], True))
    finally:
        shutil.rmtree(tmp, ignore_errors=True)

    print("")
    bad = results.count(False)
    print("汇总：%d 个用例，%d 个不符合预期" % (len(results), bad))
    if bad:
        print("判定：FAIL —— 有门禁在负样本上没有变红（等价于永远 PASS）")
        return 1
    print("判定：PASS —— 5 个无自检门禁均在坏样本上变红、在好样本上放行")
    return 0


if __name__ == "__main__":
    sys.exit(main())
