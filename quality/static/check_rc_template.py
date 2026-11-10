#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""rc 自启模板**结构化**校验（替掉 apk.yml 里的四条子串 grep）。

背景（假门禁事故）：
    旧 apk.yml:117-120 用四条 `grep -q` 检查 init.sproot.rc：
        service sproot-apd / sys.boot_completed / oneshot / disabled
    这四条**全是子串匹配**，而 `apd/assets/init.sproot.rc` 的开头注释里就写着
    「oneshot + disabled：daemon 退出后 init 不会重启它」（第 11 行）以及
    「⛔ oneshot」「⛔ disabled」的说明（第 39-40 行）。

    后果：把 service 块里**真正的** `oneshot` / `disabled` 指令整行删掉（防 bootloop
    的两个关键指令），四条 grep 仍然全 PASS —— 注释替它们过了门禁。已实测复现：
        $ 删掉 service 块内 oneshot/disabled/override/class 四行
        $ 旧四条 grep → 全 PASS（假绿）

本脚本的判据（每一条都能在坏样本上变红）：
    1. 剥离注释（# 起到行尾）+ 空行后再解析，注释**完全不参与**判定；
    2. 必须存在且仅存在一个 `service sproot-apd <path> ... daemon` 块；
    3. 该 service 块内必须有 `oneshot`、`disabled`、`override`、`class late_start`、
       `user root`、`group root` —— 缺任一即 FAIL（这就是防 bootloop 的实质内容）；
    4. 必须存在 `on property:sys.boot_completed=1` 触发器，且其块内有 `start sproot-apd`；
    5. 必须存在 `on post-fs-data` 块，且块内有 `exec -- /data/adb/apd post-fs-data`
       （Zygisk 启动链关键行：缺它 Zygisk 注入静默失效）；
    6. 密钥不得出现在 rc 里（ramdisk 明文密钥 = 泄露）：不得出现 47 字符密钥形态的字面量。

用法::

    python3 quality/static/check_rc_template.py [FILE] [--self-test]

退出码：0 = 全部通过；1 = 存在缺失/结构错误；2 = 用法错误。

``--self-test`` 用内置合成夹具验证「好样本必须 PASS、坏样本必须 FAIL」，
确认每条断言都不是永远 PASS 的空断言（AVB-3 事故防线）。
"""

from __future__ import annotations

import argparse
import re
import sys

DEFAULT_RC = "apd/assets/init.sproot.rc"

# service 块内必须出现的**指令**（剥离注释后精确匹配首 token，不是子串）
REQUIRED_SERVICE_DIRECTIVES = [
    "class",   # 必须 class late_start
    "user",
    "group",
    "oneshot",
    "disabled",
    "override",
]
REQUIRED_CLASS_VALUE = "late_start"
REQUIRED_USER = "root"
REQUIRED_GROUP = "root"

SERVICE_NAME = "sproot-apd"
BOOT_PROP = "on property:sys.boot_completed=1"
POST_FS_DATA = "on post-fs-data"
POST_FS_DATA_EXEC = "exec -- /data/adb/apd post-fs-data"

# 密钥泄露防线：47~48 位连续 [A-Za-z0-9] 且不含 "." 的裸 token 视为可疑密钥
KEYLIKE_RE = re.compile(r"(?<![A-Za-z0-9_.\-/])[A-Za-z0-9]{47,48}(?![A-Za-z0-9_.\-/])")


class RcError(Exception):
    """结构性错误（缺 service 块、缺触发器、缺关键指令等）。"""


def strip_comments(text: str) -> list[str]:
    """去掉 `#` 行尾注释与空行，返回可解析的有效行列表。

    Android init 的注释语法就是 `#` 到行尾；`#` 出现在字符串里极少见，
    rc 模板里也没有引号字符串，因此按首个 `#` 截断是安全且严格的。
    """
    out: list[str] = []
    for raw in text.splitlines():
        line = raw.split("#", 1)[0].rstrip()
        if line.strip():
            out.append(line)
    return out


def split_blocks(lines: list[str]) -> list[tuple[str, list[str]]]:
    """把有效行切成 (section 头, 块内行)。块 = section 头 + 其缩进的子行。"""
    blocks: list[tuple[str, list[str]]] = []
    heads: list[str] = []
    bodies: list[list[str]] = []

    for line in lines:
        if line[0].isspace():
            if not heads:
                raise RcError(f"缩进行出现在任何 section 之前: {line!r}")
            bodies[-1].append(line.strip())
        else:
            heads.append(line.strip())
            bodies.append([])

    for head, body in zip(heads, bodies):
        blocks.append((head, body))
    return blocks


def directives(body: list[str]) -> list[list[str]]:
    """块内每行按空白切分（保留整行语义，便于取 `class late_start` 的值）。"""
    return [ln.split() for ln in body if ln.split()]


def find_services(blocks: list[tuple[str, list[str]]]) -> list[tuple[str, list[str]]]:
    return [(h, b) for h, b in blocks if h.startswith("service ")]


def check_text(text: str) -> list[tuple[str, bool, str]]:
    """返回 [(检查名, 是否通过, 说明)]；调用方据此决定退出码。"""
    checks: list[tuple[str, bool, str]] = []

    def add(name: str, ok: bool, detail: str) -> None:
        checks.append((name, ok, detail))

    lines = strip_comments(text)

    # ---- 1. 唯一且形态正确的 service sproot-apd 块 ----
    try:
        blocks = split_blocks(lines)
    except RcError as exc:
        add("parse", False, str(exc))
        return checks
    add("parse", True, f"剥离注释后 {len(lines)} 条有效行，{len(blocks)} 个 section")

    services = find_services(blocks)
    mine = [(h, b) for h, b in services if h.split()[1:2] == [SERVICE_NAME]]
    add(
        f"service:{SERVICE_NAME}",
        len(mine) == 1,
        f"应恰好 1 个 `service {SERVICE_NAME}` 块，实际 {len(mine)} 个"
        + ("" if len(mine) == 1 else "（注释里的同名文本不算数：注释已剥离）"),
    )
    if len(mine) != 1:
        return checks

    head, body = mine[0]
    # service 行本身必须真的以 `daemon` 结尾（启动参数，不是注释）
    add(
        "service:daemon-arg",
        head.rstrip().endswith(" daemon"),
        f"`{head}` 应以 ` daemon` 结尾（实际结尾 {head.rstrip()[-20:]!r}）",
    )

    # ---- 2. 块内关键指令（逐条，缺一即红） ----
    ds = directives(body)
    flat = [" ".join(d) for d in ds]
    present_tokens = {d[0] for d in ds if d}

    for req in REQUIRED_SERVICE_DIRECTIVES:
        ok = req in present_tokens
        add(
            f"service-directive:{req}",
            ok,
            f"`service {SERVICE_NAME}` 块内缺少指令 `{req}`"
            if not ok
            else f"`{req}` 存在于 service 块内（注释不计）",
        )

    # class 必须是 late_start；user/group 必须是 root
    class_vals = [d[1] for d in ds if len(d) >= 2 and d[0] == "class"]
    add(
        "service-class:late_start",
        REQUIRED_CLASS_VALUE in class_vals,
        f"`class late_start` 必需（实际 class 值 {class_vals}）——"
        "提前到 early stage 会与 post-fs-data 争用",
    )
    user_vals = [d[1] for d in ds if len(d) >= 2 and d[0] == "user"]
    add("service-user:root", REQUIRED_USER in user_vals, f"`user root` 必需（实际 {user_vals}）")
    group_vals = [d[1] for d in ds if len(d) >= 2 and d[0] == "group"]
    add(
        "service-group:root",
        REQUIRED_GROUP in group_vals,
        f"`group root` 必需（实际 {group_vals}）",
    )

    # ---- 3. boot-completed 触发器 ----
    trig = [(h, b) for h, b in blocks if h == BOOT_PROP]
    add(f"trigger:{BOOT_PROP}", len(trig) == 1, f"应恰好 1 个 `{BOOT_PROP}` 触发器，实际 {len(trig)}")
    if trig:
        starts = [ln for ln in trig[0][1] if ln.split() == ["start", SERVICE_NAME]]
        add(
            "trigger:start-sproot-apd",
            len(starts) == 1,
            f"触发器块内应有 `start {SERVICE_NAME}`，实际 {len(starts)} 条",
        )

    # ---- 4. post-fs-data 的 Zygisk 关键 exec 行 ----
    pfd = [(h, b) for h, b in blocks if h == POST_FS_DATA]
    add(f"section:{POST_FS_DATA}", len(pfd) == 1, f"应恰好 1 个 `{POST_FS_DATA}` 段，实际 {len(pfd)}")
    if pfd:
        has_exec = any(ln.strip() == POST_FS_DATA_EXEC for ln in pfd[0][1])
        add(
            "post-fs-data:exec",
            has_exec,
            f"关键行 `{POST_FS_DATA_EXEC}` 存在"
            if has_exec
            else (
                f"`{POST_FS_DATA}` 段内缺少关键行 `{POST_FS_DATA_EXEC}`"
                "（缺它 → zygiskd 晚于 zygote，Zygisk 注入静默失效）"
            ),
        )

    # ---- 5. 密钥绝不入 rc ----
    leaks = KEYLIKE_RE.findall(text)
    add(
        "no-plaintext-key",
        not leaks,
        "rc 中出现 47/48 位密钥形态字面量（ramdisk 明文密钥 = 泄露）"
        if leaks
        else "未发现 47/48 位密钥形态字面量",
    )

    return checks


def report(checks: list[tuple[str, bool, str]], as_json: bool) -> int:
    failed = [c for c in checks if not c[1]]
    if as_json:
        import json

        print(
            json.dumps(
                {
                    "ok": not failed,
                    "checks": [{"name": n, "ok": o, "detail": d} for n, o, d in checks],
                },
                ensure_ascii=False,
                indent=2,
            )
        )
    else:
        for name, ok, detail in checks:
            print(f"[{'PASS' if ok else 'FAIL'}] {name}: {detail}")
        print()
        print(f"RESULT: {'ALL PASS' if not failed else f'{len(failed)} FAILED'}")
    return 0 if not failed else 1


# ---------------------------------------------------------------------------
# 合成夹具：证明每条断言都能变红（不是永远 PASS 的空断言）
# ---------------------------------------------------------------------------
GOOD_RC = """\
# 头部注释里故意包含 oneshot / disabled / override 这些词，
# 用来证明**注释不参与判定**（旧的四条 grep 正是被这里骗过）。
on post-fs-data
    mkdir /data/adb/sproot 0700 root root
    exec -- /data/adb/apd post-fs-data

service sproot-apd /data/adb/apd --superkey-file /data/adb/sproot/key daemon
    class late_start
    user root
    group root
    oneshot
    disabled
    override

on property:sys.boot_completed=1
    start sproot-apd
"""

# 坏样本 1：service 块里把 oneshot/disabled 等真指令删掉，只留注释
BAD_NO_DIRECTIVES = """\
# oneshot disabled override —— 全是注释，不能算数
on post-fs-data
    exec -- /data/adb/apd post-fs-data

service sproot-apd /data/adb/apd --superkey-file /data/adb/sproot/key daemon
    class late_start
    user root
    group root

on property:sys.boot_completed=1
    start sproot-apd
"""

# 坏样本 2：整个 service 块被删掉，只剩注释里提到 oneshot/disabled
BAD_NO_SERVICE = """\
# service sproot-apd ... oneshot ... disabled  ← 注释里写着，但块没了
on post-fs-data
    exec -- /data/adb/apd post-fs-data

on property:sys.boot_completed=1
    start sproot-apd
"""

# 坏样本 3：缺 boot_completed 触发器
BAD_NO_TRIGGER = GOOD_RC.replace("on property:sys.boot_completed=1", "# 触发器被删掉了")

# 坏样本 4：缺 post-fs-data 的 exec（Zygisk 静默失效）
BAD_NO_EXEC = GOOD_RC.replace("    exec -- /data/adb/apd post-fs-data\n", "")

# 坏样本 5：rc 里明文写了 48 位密钥（泄露）
BAD_KEY_LEAK = GOOD_RC + (
    "\n# 下面这行是事故：密钥被写进 ramdisk\n"
    "ABCdef0123456789ABCdef0123456789ABCdef0123456789\n"
)

# 坏样本 6：service 行丢了 `daemon` 参数
BAD_NO_DAEMON_ARG = GOOD_RC.replace(
    "service sproot-apd /data/adb/apd --superkey-file /data/adb/sproot/key daemon",
    "service sproot-apd /data/adb/apd --superkey-file /data/adb/sproot/key",
)

NEGATIVE_FIXTURES = {
    "no-service-directives": (BAD_NO_DIRECTIVES, "service-directive:"),
    "no-service-block": (BAD_NO_SERVICE, "service:"),
    "no-boot-trigger": (BAD_NO_TRIGGER, "trigger:"),
    "no-post-fs-data-exec": (BAD_NO_EXEC, "post-fs-data:exec"),
    "plaintext-key": (BAD_KEY_LEAK, "no-plaintext-key"),
    "no-daemon-arg": (BAD_NO_DAEMON_ARG, "service:daemon-arg"),
}


def self_test() -> int:
    """正样本必须 ALL PASS；每个坏样本必须在**指定断言**上变红。"""
    problems: list[str] = []

    good = check_text(GOOD_RC)
    bad_checks = [c for c in good if not c[1]]
    if bad_checks:
        problems.append("好样本本应 ALL PASS，却失败了：" + str([c[0] for c in bad_checks]))
    print(f"[self-test] 正样本: {len(good)} 条断言, 失败 {len(bad_checks)}")

    for label, (text, must_fail_prefix) in NEGATIVE_FIXTURES.items():
        checks = check_text(text)
        failed = [c[0] for c in checks if not c[1]]
        hit = [n for n in failed if n.startswith(must_fail_prefix)]
        if not failed:
            problems.append(f"坏样本 {label}: 没有任何断言失败（永远 PASS 的空断言！）")
            print(f"[self-test] 坏样本 {label}: ✗ 全 PASS —— 断言是假的")
            continue
        if not hit:
            problems.append(
                f"坏样本 {label}: 失败的是 {failed}，但预期断言 {must_fail_prefix}* 未触发"
            )
            print(f"[self-test] 坏样本 {label}: ✗ 预期断言未触发（实际 {failed}）")
            continue
        print(f"[self-test] 坏样本 {label}: ✓ 变红（{len(failed)} 条失败，含 {hit[0]}）")

    if problems:
        print()
        for p in problems:
            print(f"[self-test] FAIL: {p}")
        print("RESULT: SELF-TEST FAILED（存在永远 PASS 的断言）")
        return 1
    print()
    print("RESULT: SELF-TEST PASS（正样本绿、6 个坏样本各自变红）")
    return 0


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(
        description="rc 自启模板结构化校验（剥离注释后按 service 块断言）"
    )
    ap.add_argument("file", nargs="?", default=DEFAULT_RC, help=f"rc 模板路径（默认 {DEFAULT_RC}）")
    ap.add_argument("--self-test", action="store_true", help="用合成夹具验证断言真实性")
    ap.add_argument("--json", action="store_true", help="以 JSON 输出")
    args = ap.parse_args(argv)

    if args.self_test:
        return self_test()

    from pathlib import Path

    path = Path(args.file)
    if not path.is_file():
        print(f"::error::rc 模板不存在: {path}")
        return 2
    try:
        text = path.read_text(encoding="utf-8")
    except UnicodeDecodeError as exc:
        print(f"::error::rc 模板不是 UTF-8: {exc}")
        return 2

    return report(check_text(text), args.json)


if __name__ == "__main__":
    sys.exit(main())
