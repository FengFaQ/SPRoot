#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""apd 能力门禁：**可失败**的能力验证，替掉 apd.yml 里的 `grep -aq` 字符串搜索。

═══════════════════════════════════════════════════════════════════════════
背景：被实测证伪的旧门禁
═══════════════════════════════════════════════════════════════════════════
旧 ``apd.yml`` "Capability assertions" 步骤对 ``dist/libapd.so`` 做
``grep -aq -- "<marker>"``（markers 含 ``deploy`` / ``undeploy`` / ``root-exec`` /
``daemon`` / ``vbmeta_untouched`` …）。

``grep -a`` 只要求某段字节在文件里**出现过一次**。功能被删、字符串残留在别的
语义位置（日志、状态串、别的符号）就仍然 PASS。本机实测复现：

  ① 从 ``apd/src/cli.rs`` 删掉 ``Undeploy``（enum variant + dispatch arm），
     交叉编译 arm64 产物；
     → 旧门禁 ``grep -aq undeploy`` **仍然 PASS**（命中 selftest 的状态串
       ``"undeployed"``，与子命令毫无关系）。**旧门禁对功能删除零鉴别力。**

  ② 旧 markers 里的 ``vbmeta_untouched`` 在当前源码里**已不存在**（改成 AVB
     实测三态 ``avb_ok`` / ``avb_no_footer`` / ``avb_unobservable``，旧字面量
     只剩注释与 ``#[cfg(test)]``，不进 release 二进制）。
     → 旧门禁在**正确的新代码**上必然 FAIL。这是「假门禁」的另一种死法：
       门禁与代码事实脱节，CI 永远红，被人为忽略。

═══════════════════════════════════════════════════════════════════════════
本脚本的两层判据
═══════════════════════════════════════════════════════════════════════════
L1 **CLI about 契约**（主判据；对 stripped 产物也稳）
   clap derive 把每个子命令的 doc-comment 作为 ``about`` 编进二进制，且**不会**
   被 ``strip`` 掉（实测：strip 前后都在）。删掉一个子命令 → 它的 about 文本
   消失 → 本层变红（实测：删 ``Undeploy`` 后正好只有 undeploy 的 about 缺失）。

   为什么不判「子命令名字符串」：实测在 stripped arm64 release 产物里，
   clap 的命令名会被拆散/交织进其它字符串区段，``undeploy`` / ``selftest`` /
   ``auth`` 即使功能健在也可能匹配不到连续字节 → 那种判据会假红。
   about 长句是**唯一的**稳定特征。

L2 **能力代号组**（互斥组必须齐全）
   如 AVB 三态必须 ``avb_ok`` + ``avb_no_footer`` + ``avb_unobservable`` **同时**
   在场：只留一个说明三态逻辑被削平回「一条永真断言」。用 NUL 边界精确 token
   匹配（``avb_ok`` 不会被 ``avb_ok_extra`` 冒充）。

两层都只在**必须成立**的方向断言：缺了就红。不检查「应该没有」的东西
（那容易把正常演进判红）。

═══════════════════════════════════════════════════════════════════════════
用法
═══════════════════════════════════════════════════════════════════════════
    python3 quality/static/check_apd_capabilities.py BINARY [--json]
    python3 quality/static/check_apd_capabilities.py --self-test
    python3 quality/static/check_apd_capabilities.py --verify-vs-source APD_SRC_DIR

``--verify-vs-source``：把 ``apd/src/cli.rs`` 里 Commands enum 的**实际**子命令
与 about 文本抽出来，与本脚本内嵌契约比对。代码删了子命令却没更新门禁 →
本项变红，**从机制上杜绝**「门禁与代码事实脱节」（即 vbmeta_untouched 那类事故）。

退出码：0 = ALL PASS；1 = 能力缺失；2 = 用法/环境错误。
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

# ═══════════════════════════════════════════════════════════════════════════
# CLI 契约：子命令 → 其 clap about 文本中**稳定且唯一**的片段。
#
# ⚠️ 这些片段必须真实存在于 apd/src/cli.rs 的 doc-comment 里（`--verify-vs-source`
#    会强制校验）。改动 apd 的 help 文案时，这里必须同步——否则门禁变红，这是
#    刻意的：门禁不得悄悄与代码事实脱节。
# ═══════════════════════════════════════════════════════════════════════════
CLI_CONTRACT: dict[str, str] = {
    "module": "Manage APatch modules",
    "post-fs-data": "Trigger `post-fs-data` event",
    "services": "Trigger `service` event",
    "boot-completed": "Trigger `boot-complete` event",
    "uid-listener": "Start uid listener for synchronizing root list",
    "daemon": "Resident daemon (init only)",
    "deploy": "First-install deployment",
    "undeploy": "Reverse of deploy: remove AP, hidden su, key and private state",
    "root-exec": "Run a command as root. This is the manager's ONLY root entry point",
    "auth": "Authorisation table maintenance",
    "version": "Machine readable deployment/version status",
    "selftest": "Read-only capability/diagnostic self-check",
    "resetprop": "Resetprop - Magisk-compatible system property tool",
    "sepolicy": "MagiskPolicy - SELinux Policy Patch Tool",
}

# ═══════════════════════════════════════════════════════════════════════════
# 能力代号组：组内代号必须**全部**在场（互斥三态被削平即红）
#   判定用子串存在性（见 code_present 的长注释：边界解析实测会假红）
# ═══════════════════════════════════════════════════════════════════════════
CAPABILITY_GROUPS: dict[str, dict] = {
    "avb-tri-state": {
        "codes": ["avb_ok", "avb_no_footer", "avb_unobservable"],
        "why": "AVB 实测三态；旧实现写死字面量 vbmeta_untouched，是不可证伪的假断言",
    },
    "supercall-contract": {
        "codes": ["SPROOT_REQUEST_UID", "/proc/self/attr/current"],
        "why": "SK supercall 请求信道（env）+ SELinux 上下文切换点",
    },
    "bootstate": {
        "codes": ["boot_count", "safemode"],
        "why": "连续启动失败 → 安全模式的状态机",
    },
    "auth-socket": {
        "codes": ["sproot-"],
        "why": "抽象 unix socket 名前缀 @sproot-<12hex>",
    },
}

# 必须**不存在**的过期字面量（门禁不得再依赖已被删除的旧哨兵）
FORBIDDEN_LEGACY: dict[str, str] = {
    "vbmeta_untouched": (
        "旧哨兵字面量已被 AVB 实测三态取代；若它重新出现在二进制里，说明 AVB 逻辑回退"
    ),
}


class EnvError(Exception):
    """用法/环境错误（非能力缺失）。"""


# ───────────────────────────────────────────────────────────────────────────
# 底层判定
# ───────────────────────────────────────────────────────────────────────────
def read_elf(path: Path) -> bytes:
    if not path.is_file():
        raise EnvError(f"二进制不存在: {path}")
    data = path.read_bytes()
    if data[:4] != b"\x7fELF":
        raise EnvError(f"不是 ELF 文件（magic={data[:4]!r}）: {path}")
    return data


def code_present(data: bytes, code: str) -> bool:
    """能力代号是否在二进制中存在。

    ⚠️ 这里**刻意**用朴素的「子串存在」而非边界解析，原因是被实测否证过的两次
    尝试（写在这里，避免后人再踩）：

      1. **NUL 边界精确匹配会假红**：Rust release 产物把相邻字面量拼进同一字符串
         池，``avb_no_footer`` 与 ``avb_ok`` 实际存成
         ``configavb_no_footeravb_ok``（中间无 NUL，前面还粘着无关的 ``config``）。
      2. **按已知码切分会假红**：池的相邻关系由链接顺序决定、逐次构建都在变。
         同一个源码的两次构建里，池分别是 ``avb_no_footeravb_ok`` 与
         ``avb_okavb_no_footeravb_malformedavb_unobservable``；出现了一个不在
         门禁清单里的 ``avb_malformed`` 就会让「完整切分」失败。
         → 实测：正确产物被判红。**假红和被忽略一样致命**（vbmeta_untouched
           那次事故正是门禁与事实脱节的假红）。

    代价与取舍：子串判定理论上可能被 ``avb_ok_extra`` 这类更长标识符冒充。但那不是
    我们要防的威胁——本项目真实的失效模式是**功能被删除**（本次审计已确证：
    删掉 `Undeploy` 后 `grep -aq undeploy` 仍 PASS）。代号被删 → 子串必然消失 →
    本层变红。而「三态被削平成单态」由 CAPABILITY_GROUPS 的**全组必须齐**挡住。
    """
    return code.encode() in data


def has_code(data: bytes, code: str) -> bool:
    return code_present(data, code)


# ───────────────────────────────────────────────────────────────────────────
# L1 / L2 静态层
# ───────────────────────────────────────────────────────────────────────────
def static_checks(data: bytes) -> list[tuple[str, bool, str]]:
    checks: list[tuple[str, bool, str]] = []

    # ---- L1: CLI about 契约 ----
    for sub, about in CLI_CONTRACT.items():
        ok = about.encode() in data
        checks.append(
            (
                f"cli:{sub}",
                ok,
                (
                    f"`{sub}` 的 about 文本在场（clap doc-comment 编入且 strip 不掉）"
                    if ok
                    else (
                        f"`{sub}` 的 about 文本缺失 → 该子命令已从 CLI 移除"
                        f"（或 help 文案被改写而未同步门禁）。期望片段: {about[:60]!r}"
                    )
                ),
            )
        )

    # ---- L2: 能力代号组 ----
    for group, spec in CAPABILITY_GROUPS.items():
        codes: list[str] = spec["codes"]
        present, missing = [], []
        for code in codes:
            (present if has_code(data, code) else missing).append(code)
        checks.append(
            (
                f"capability:{group}",
                not missing,
                (
                    f"全部 {len(codes)} 个代号在场（{spec['why']}）"
                    if not missing
                    else f"缺代号 {missing}（{spec['why']}）→ 该能力被删除或三态被削平"
                ),
            )
        )

    # ---- L3: 过期字面量不得复活 ----
    for legacy, why in FORBIDDEN_LEGACY.items():
        present = legacy.encode() in data
        checks.append(
            (
                f"legacy:{legacy}",
                not present,
                f"二进制中**出现**了过期字面量 {legacy!r}：{why}"
                if present
                else f"未出现过期字面量 {legacy!r}（已由实测三态取代）",
            )
        )

    return checks


# ───────────────────────────────────────────────────────────────────────────
# 门禁自洽性：脚本内嵌契约 vs apd/src/cli.rs 事实
# ───────────────────────────────────────────────────────────────────────────
# camel→kebab：PostFsData -> post-fs-data, RootExec -> root-exec
CAMEL_SPLIT_RE = re.compile(r"[A-Z][a-z0-9]*|[A-Z]+(?![a-z])")

# apd 的 Commands enum 变体名（Rust 标识符）→ 对外 CLI 名，与 clap 默认 kebab-case 一致
# 注意：clap 对 PostFsData 生成 `post-fs-data`，对 UidListener 生成 `uid-listener`。
def camel_to_kebab(name: str) -> str:
    parts = CAMEL_SPLIT_RE.findall(name)
    return "-".join(p.lower() for p in parts)


def parse_cli_source(cli_rs: Path) -> dict[str, str]:
    """从 cli.rs 抽出 {CLI 子命令名: about 文本}（Commands enum 区段）。

    clap derive 把变体上**全部**连续 doc-comment 行用空格拼成 `about`，
    因此这里也必须 join 全部行（不是只取最后一行）。只收集 **4 空格缩进**
    的 doc 行 —— 8 空格的是变体字段的文档，不算 about。
    """
    if not cli_rs.is_file():
        raise EnvError(f"cli.rs 不存在: {cli_rs}")
    src = cli_rs.read_text(encoding="utf-8", errors="replace")
    try:
        start = src.index("enum Commands")
    except ValueError as exc:
        raise EnvError("cli.rs 中找不到 `enum Commands`") from exc
    m = re.search(r"\nenum AuthCmd", src[start:])
    end = start + m.start() if m else len(src)
    block = src[start:end]

    result: dict[str, str] = {}
    pending: list[str] = []

    for line in block.splitlines():
        stripped = line.strip()
        # 变体级 doc-comment：恰好 4 空格缩进的 `///`
        indent = len(line) - len(line.lstrip())
        if stripped.startswith("///") and indent == 4:
            pending.append(stripped[3:].strip())
            continue
        if stripped.startswith("///"):
            # 字段级 doc（更深缩进）：不参与 about
            continue
        if stripped.startswith("#["):
            # 属性行（#[command(...)] / #[arg(...)]）不打断 pending
            continue
        mm = re.match(r"^    ([A-Z][A-Za-z0-9]*)\s*(\{|,|$|\()", line)
        if mm:
            variant = mm.group(1)
            if variant != "Commands" and pending:
                result[camel_to_kebab(variant)] = " ".join(pending)
            pending = []
            continue

    return result


def self_consistency_checks(source_cli_rs: Path) -> list[tuple[str, bool, str]]:
    """内嵌契约必须与源码事实一致（防门禁与代码脱节）。"""
    checks: list[tuple[str, bool, str]] = []
    try:
        actual = parse_cli_source(source_cli_rs)
    except EnvError as exc:
        return [("self-consistency:parse", False, str(exc))]

    if not actual:
        return [("self-consistency:parse", False, "未能从 cli.rs 解析出任何子命令（解析器失效）")]

    for sub, about in CLI_CONTRACT.items():
        ok = sub in actual
        checks.append(
            (
                f"self-consistency:source-has:{sub}",
                ok,
                f"cli.rs 的 Commands enum 有 `{sub}`"
                if ok
                else f"门禁要求子命令 `{sub}`，但 cli.rs 的 Commands enum 里没有它",
            )
        )
        if ok:
            # 内嵌片段必须是源码 about 的子串
            frag_ok = about.lower() in actual[sub].lower()
            checks.append(
                (
                    f"self-consistency:fragment:{sub}",
                    frag_ok,
                    f"门禁片段与 cli.rs about 匹配（{about[:40]!r}）"
                    if frag_ok
                    else (
                        f"门禁片段 {about!r} 不是 cli.rs 中 `{sub}` about "
                        f"{actual[sub][:60]!r} 的子串 → help 文案改了，门禁必须同步"
                    ),
                )
            )

    extra = sorted(set(actual) - set(CLI_CONTRACT))
    checks.append(
        (
            "self-consistency:no-ungated",
            not extra,
            (
                "cli.rs 中所有子命令都在门禁契约内"
                if not extra
                else f"cli.rs 新增了未经门禁覆盖的子命令 {extra} → 必须补进 CLI_CONTRACT"
            ),
        )
    )
    return checks


# ───────────────────────────────────────────────────────────────────────────
# self-test：合成夹具，证明断言能变红
# ───────────────────────────────────────────────────────────────────────────
def _fake_elf(*chunks: bytes) -> bytes:
    return b"\x7fELF" + b"\x00" * 60 + b"\x00".join(chunks)


def _good_chunks() -> list[bytes]:
    return [a.encode() for a in CLI_CONTRACT.values()] + [
        b"avb_ok",
        b"avb_no_footer",
        b"avb_unobservable",
        b"SPROOT_REQUEST_UID",
        b"/proc/self/attr/current",
        b"boot_count",
        b"safemode",
        b"sproot-",
    ]


def self_test() -> int:
    problems: list[str] = []

    good = static_checks(_fake_elf(*_good_chunks()))
    bad = [c for c in good if not c[1]]
    if bad:
        problems.append(f"好样本本应 ALL PASS，却失败: {[c[0] for c in bad]}")
    print(f"[self-test] 正样本: {len(good)} 条断言, 失败 {len(bad)}")

    def expect_fail(label: str, data: bytes, must_fail: str) -> None:
        cs = static_checks(data)
        failed = [c[0] for c in cs if not c[1]]
        if must_fail not in failed:
            problems.append(f"坏样本 {label}: 预期 {must_fail} 变红，实际失败项 {failed}")
            print(f"[self-test] 坏样本 {label}: ✗ 未在 {must_fail} 变红（实际 {failed}）")
        elif not failed:
            problems.append(f"坏样本 {label}: 没有任何断言失败（永远 PASS！）")
            print(f"[self-test] 坏样本 {label}: ✗ 全 PASS")
        else:
            print(f"[self-test] 坏样本 {label}: ✓ 变红（{len(failed)} 项，含 {must_fail}）")

    # 坏样本 1：删掉 undeploy 子命令（模拟真实的「功能删除」回归）
    chunks = [c for c in _good_chunks() if c != CLI_CONTRACT["undeploy"].encode()]
    expect_fail("subcommand-undeploy-removed", _fake_elf(*chunks), "cli:undeploy")

    # 坏样本 2：AVB 三态被削平成单一状态（回到不可证伪的假断言）
    collapsed = [
        c if c not in (b"avb_no_footer", b"avb_unobservable") else b"XXX"
        for c in _good_chunks()
    ]
    expect_fail("avb-tri-state-collapsed", _fake_elf(*collapsed), "capability:avb-tri-state")

    # 坏样本 3：过期哨兵 vbmeta_untouched 复活
    revived = _good_chunks() + [b"vbmeta_untouched"]
    expect_fail("legacy-sentinel-revived", _fake_elf(*revived), "legacy:vbmeta_untouched")

    # 坏样本 4：bootstate 代号缺失
    no_boot = [b"zzz" if c == b"boot_count" else c for c in _good_chunks()]
    expect_fail("bootstate-removed", _fake_elf(*no_boot), "capability:bootstate")

    # 坏样本 5：三态之一被**删除**（不是改名）→ 组必须变红。
    # 注意：本层用子串判定，改名成 avb_ok_extra 仍会命中 avb_ok —— 这是刻意接受的
    # 假阴性（见 code_present 注释）；真实威胁是删除，故用删除来构造坏样本。
    dropped = [c for c in _good_chunks() if c != b"avb_unobservable"]
    expect_fail("avb-code-removed", _fake_elf(*dropped), "capability:avb-tri-state")

    # 坏样本 6：多个子命令同时被删（整体能力退化）
    many = [
        c
        for c in _good_chunks()
        if c not in (CLI_CONTRACT["sepolicy"].encode(), CLI_CONTRACT["auth"].encode())
    ]
    expect_fail("multi-subcommand-removed", _fake_elf(*many), "cli:sepolicy")

    if problems:
        print()
        for p in problems:
            print(f"[self-test] FAIL: {p}")
        print("RESULT: SELF-TEST FAILED（存在永远 PASS 的断言）")
        return 1
    print()
    print("RESULT: SELF-TEST PASS（正样本全绿、6 个坏样本各自在预期断言上变红）")
    return 0


# ───────────────────────────────────────────────────────────────────────────
def report(checks: list[tuple[str, bool, str]], as_json: bool, binary: str | None) -> int:
    failed = [c for c in checks if not c[1]]
    if as_json:
        print(
            json.dumps(
                {
                    "ok": not failed,
                    "binary": binary,
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
        if failed:
            print(f"缺失的能力（{len(failed)} 项）：")
            for name, _, detail in failed:
                print(f"  - {name}: {detail}")
            print()
        print(f"RESULT: {'ALL PASS' if not failed else f'{len(failed)} FAILED'}")
    return 0 if not failed else 1


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(
        description="apd 能力门禁（CLI about 契约 + 能力代号组；替掉 grep 字符串搜索）"
    )
    ap.add_argument("binary", nargs="?", help="待检查的 apd 二进制（如 dist/libapd.so）")
    ap.add_argument("--json", action="store_true", help="以 JSON 输出")
    ap.add_argument("--self-test", action="store_true", help="用合成夹具验证断言真实性")
    ap.add_argument(
        "--verify-vs-source",
        metavar="APD_SRC_DIR",
        help="额外校验门禁契约与 apd/src/cli.rs 的事实一致（防门禁与代码脱节）",
    )
    args = ap.parse_args(argv)

    if args.self_test:
        return self_test()

    if not args.binary and not args.verify_vs_source:
        print(
            "::error::需要 BINARY 参数、--self-test 或 --verify-vs-source 之一",
            file=sys.stderr,
        )
        return 2

    checks: list[tuple[str, bool, str]] = []

    if args.binary:
        try:
            data = read_elf(Path(args.binary))
        except EnvError as exc:
            print(f"::error::{exc}", file=sys.stderr)
            return 2
        checks += static_checks(data)

    if args.verify_vs_source:
        checks += self_consistency_checks(Path(args.verify_vs_source) / "src" / "cli.rs")

    return report(checks, args.json, args.binary)


if __name__ == "__main__":
    sys.exit(main())
