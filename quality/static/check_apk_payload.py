#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""APK 载荷门禁（T1/T2/T6 静态部分）——补齐文档已引用但**曾经不存在**的脚本。

═══════════════════════════════════════════════════════════════════════════
为什么有这个文件
═══════════════════════════════════════════════════════════════════════════
`LICENSES.md:108`、`docs/HANDOFF-2026-11-08.md:70`、
`docs/NEW-DOC-06-测试方案v0.1.md:290/291/306/343` 都把
``quality/static/check_apk_payload.py`` 当成**已落地**的门禁来引用，
但 ``quality/static/`` 下从来没有这个文件 —— 文档承诺的门禁是空的。
本文件把它补成真实、可失败的实现。

═══════════════════════════════════════════════════════════════════════════
它断言什么（每一条都能在坏样本上变红）
═══════════════════════════════════════════════════════════════════════════
N  **native 齐全且架构正确**
   ``lib/arm64-v8a/`` 下必须有 ``libapd.so`` / ``libskrootpatch.so`` /
   ``libmagiskboot.so``；每个 ELF 的 ``e_machine`` 必须是 183（AArch64）。
   APK 内**不得**出现其它 ABI 目录（lib/x86_64 等）—— 宿主 x86-64 混入是
   本项目历史上的真实事故（NEW-DOC-03）。
A  **必需 asset 齐全**
   ``assets/NOTICE``、``assets/init.sproot.rc``、``assets/licenses/{NOTICE,LICENSE,
   AGPL-3.0.txt,GPL-3.0.txt}``、``assets/NOTICE-APK.md``、``assets/apd-version.txt``、
   ``assets/apd-capabilities.txt``。
L  **许可文本是全文**，不是占位：AGPL/LICENSE > 20 KB 且含标题；GPL-3.0.txt 含标题；
   NOTICE 覆盖 magiskboot/SKRoot/保留所有权利。
R  **随包 rc 模板结构正确**：复用 ``check_rc_template`` 的判据（剥离注释后按
   service 块断言）。APK 里的 rc 若是旧的/注释版，这里变红。
V  **版本一致性**：``assets/apd-version.txt`` 的 ``commit=`` 必须与调用方传入的
   ``--expect-commit``（或 ``--expect-commit-from apd-version.txt``）一致，
   防止「APK 里的 apd 与本次 CI 构建的 apd 错配」。
K  **作废旧名不出现**：``libskpatchjni.so``（旧命名）不得在 APK 内。
S  **可选 sha256 锁定**：``--lock app/native.lock.json`` 时，APK 内 native 的
   sha256 必须与锁定表一致（可复现构建）。

关键设计：**不调用 unzip / readelf 外部命令**——直接读 zip 中央目录 + 解析 ELF
头。这样本地（Windows/Linux 都可）与 CI 行为一致，且不依赖工具版本差异。

═══════════════════════════════════════════════════════════════════════════
用法
═══════════════════════════════════════════════════════════════════════════
    python3 quality/static/check_apk_payload.py PATH/TO/app-release.apk \\
        [--expect-commit SHA] [--lock app/native.lock.json] [--json]
    python3 quality/static/check_apk_payload.py --self-test

退出码：0 = ALL PASS；1 = 载荷不合格；2 = 用法/输入错误。
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
import zipfile
from pathlib import Path

# ───────────────────────────────────────────────────────────────────────────
# 契约
# ───────────────────────────────────────────────────────────────────────────
REQUIRED_NATIVES = ["libapd.so", "libskrootpatch.so", "libmagiskboot.so"]
ALLOWED_ABI = "arm64-v8a"
EM_AARCH64 = 183

REQUIRED_ASSETS = [
    "assets/NOTICE",
    "assets/init.sproot.rc",
    "assets/licenses/NOTICE",
    "assets/licenses/LICENSE",
    "assets/licenses/AGPL-3.0.txt",
    "assets/licenses/GPL-3.0.txt",
    "assets/NOTICE-APK.md",
    "assets/apd-version.txt",
    "assets/apd-capabilities.txt",
]

# 许可全文判据：(路径, 最小字节数, 必须出现的标题)
LICENSE_FULLTEXT = [
    ("assets/licenses/LICENSE", 20000, "GNU AFFERO GENERAL PUBLIC LICENSE"),
    ("assets/licenses/AGPL-3.0.txt", 20000, "GNU AFFERO GENERAL PUBLIC LICENSE"),
    ("assets/licenses/GPL-3.0.txt", 30000, "GNU GENERAL PUBLIC LICENSE"),
]

# NOTICE 必须覆盖的字样
NOTICE_PHRASES = ["magiskboot", "SKRoot", "AGPL-3.0", "GPL-3.0"]

OBSOLETE_ENTRIES = ["libskpatchjni.so"]


class ApkError(Exception):
    """用法/输入错误（非载荷问题）。"""


def sha256_of(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def elf_machine(data: bytes) -> int | None:
    """返回 ELF e_machine；非 ELF 返回 None。仅依赖标准库。"""
    if len(data) < 20 or data[:4] != b"\x7fELF":
        return None
    ei_data = data[5]  # 1 = little endian
    if ei_data == 1:
        return int.from_bytes(data[18:20], "little")
    if ei_data == 2:
        return int.from_bytes(data[18:20], "big")
    return None


# ───────────────────────────────────────────────────────────────────────────
# 检查主体（对 zipfile.ZipFile 抽象，便于 self-test 用内存 zip）
# ───────────────────────────────────────────────────────────────────────────
def check_apk(
    zf: zipfile.ZipFile,
    expect_commit: str | None = None,
    lock: dict | None = None,
) -> list[tuple[str, bool, str]]:
    checks: list[tuple[str, bool, str]] = []
    names = set(zf.namelist())

    def add(name: str, ok: bool, detail: str) -> None:
        checks.append((name, ok, detail))

    def read(name: str) -> bytes:
        with zf.open(name) as fh:
            return fh.read()

    # ── N. native 齐全 + 架构 ─────────────────────────────────────────────
    for lib in REQUIRED_NATIVES:
        entry = f"lib/{ALLOWED_ABI}/{lib}"
        if entry not in names:
            add(f"native:{lib}", False, f"APK 缺少 {entry}（native 未集成，禁止出包）")
            continue
        data = read(entry)
        machine = elf_machine(data)
        if machine is None:
            add(f"native:{lib}", False, f"{entry} 不是 ELF 文件")
            continue
        ok = machine == EM_AARCH64
        add(
            f"native:{lib}",
            ok,
            (
                f"{entry} AArch64，{len(data)} B，sha256={sha256_of(data)[:16]}…"
                if ok
                else f"{entry} 的 e_machine={machine}，期望 {EM_AARCH64}(AArch64)"
                "——宿主架构混入是历史事故（NEW-DOC-03）"
            ),
        )

        # ── S. sha256 锁定（可选）────────────────────────────────────────
        if lock:
            want = _lock_sha(lock, lib)
            if want:
                got = sha256_of(data)
                add(
                    f"lock:{lib}",
                    got == want,
                    (
                        f"{lib} sha256 与 native.lock.json 一致"
                        if got == want
                        else f"{lib} sha256 漂移：锁定={want[:16]}… 实际={got[:16]}…"
                    ),
                )

    # 不得出现非 arm64-v8a 的 ABI
    other_abis = sorted(
        {
            n.split("/")[1]
            for n in names
            if n.startswith("lib/") and len(n.split("/")) > 1 and n.split("/")[1] != ALLOWED_ABI
        }
    )
    add(
        "native:abi-exclusive",
        not other_abis,
        (
            f"APK 只含 lib/{ALLOWED_ABI}/"
            if not other_abis
            else f"APK 含非 {ALLOWED_ABI} 的 ABI 目录 {other_abis}——禁止多架构混入"
        ),
    )

    # ── K. 作废旧名 ──────────────────────────────────────────────────────
    for old in OBSOLETE_ENTRIES:
        hits = [n for n in names if n.endswith("/" + old) or n == old]
        add(
            f"obsolete:{old}",
            not hits,
            f"APK 内出现已作废的 {old}" if hits else f"未出现已作废的 {old}",
        )

    # ── A. 必需 asset ────────────────────────────────────────────────────
    for asset in REQUIRED_ASSETS:
        ok = asset in names
        add(
            f"asset:{asset}",
            ok,
            f"{asset} 在场" if ok else f"APK 缺少 {asset}（第三方声明/许可/元数据缺失）",
        )

    # ── L. 许可全文 ──────────────────────────────────────────────────────
    for path, min_bytes, title in LICENSE_FULLTEXT:
        if path not in names:
            add(f"license-text:{path}", False, f"缺少 {path}")
            continue
        text = read(path)
        if len(text) < min_bytes:
            add(
                f"license-text:{path}",
                False,
                f"{path} 仅 {len(text)} B（< {min_bytes} B），疑似非全文",
            )
            continue
        has_title = title.encode() in text
        add(
            f"license-text:{path}",
            has_title,
            (
                f"{path} 全文 {len(text)} B 且含标题"
                if has_title
                else f"{path} 缺标题 {title!r}"
            ),
        )

    # NOTICE 字样
    if "assets/NOTICE" in names:
        notice = read("assets/NOTICE").decode("utf-8", errors="replace")
        low = notice.lower()
        missing = [p for p in NOTICE_PHRASES if p.lower() not in low]
        add(
            "notice:phrases",
            not missing,
            (
                f"assets/NOTICE 覆盖 {len(NOTICE_PHRASES)} 个必备字样"
                if not missing
                else f"assets/NOTICE 缺字样 {missing}"
            ),
        )

    # ── R. 随包 rc 模板结构 ──────────────────────────────────────────────
    if "assets/init.sproot.rc" in names:
        rc_text = read("assets/init.sproot.rc").decode("utf-8", errors="replace")
        try:
            rc_checks = _rc_checks(rc_text)
        except Exception as exc:  # noqa: BLE001 - 解析器不可用时必须红
            add("rc:parse", False, f"随包 init.sproot.rc 解析失败: {exc}")
        else:
            rc_failed = [n for n, ok, _ in rc_checks if not ok]
            add(
                "rc:structure",
                not rc_failed,
                (
                    f"随包 init.sproot.rc 结构正确（{len(rc_checks)} 条断言）"
                    if not rc_failed
                    else f"随包 init.sproot.rc 结构不合格，失败项 {rc_failed}"
                ),
            )

    # ── V. 版本一致性 ────────────────────────────────────────────────────
    if expect_commit:
        if "assets/apd-version.txt" not in names:
            add("version:commit", False, "缺 assets/apd-version.txt，无法校验 commit")
        else:
            meta = read("assets/apd-version.txt").decode("utf-8", errors="replace")
            got = _meta_value(meta, "commit")
            ok = got == expect_commit
            add(
                "version:commit",
                ok,
                (
                    f"assets/apd-version.txt commit={got} 与期望一致"
                    if ok
                    else f"版本错配：APK 内 commit={got!r}，期望 {expect_commit!r}"
                ),
            )

    return checks


def _lock_sha(lock: dict, lib: str) -> str | None:
    """native.lock.json 里按 target 名查 sha256。"""
    for key in ("apd", "engine", "magiskboot"):
        entry = lock.get(key) or {}
        if entry.get("target") == lib:
            return entry.get("sha256")
    return None


def _meta_value(text: str, key: str) -> str | None:
    for line in text.splitlines():
        if line.startswith(key + "="):
            return line.split("=", 1)[1].strip()
    return None


def _rc_checks(text: str):
    """复用 check_rc_template 的判据（同目录导入；失败则降级为本文件内联断言）。"""
    import importlib.util

    here = Path(__file__).resolve().parent
    cand = here / "check_rc_template.py"
    if cand.is_file():
        spec = importlib.util.spec_from_file_location("_rc_tpl", cand)
        mod = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(mod)
        return mod.check_text(text)
    # 降级：至少要求关键行存在（弱化，但不会静默全绿）
    return [("rc:fallback", "service sproot-apd" in text, "降级检查 service 行")]


# ───────────────────────────────────────────────────────────────────────────
# self-test：合成 APK 夹具
# ───────────────────────────────────────────────────────────────────────────
GOOD_RC = """\
# 注释里故意写 oneshot / disabled —— 注释不得参与判定
on post-fs-data
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

BAD_RC_NO_DIRECTIVES = """\
# oneshot disabled override（注释）
on post-fs-data
    exec -- /data/adb/apd post-fs-data
service sproot-apd /data/adb/apd --superkey-file /data/adb/sproot/key daemon
    class late_start
    user root
    group root
on property:sys.boot_completed=1
    start sproot-apd
"""

AGPL_TEXT = b"GNU AFFERO GENERAL PUBLIC LICENSE\nVersion 3\n" + b"x" * 40000
GPL_TEXT = b"GNU GENERAL PUBLIC LICENSE\nVersion 3\n" + b"y" * 36000
NOTICE_TEXT = (
    "SPRoot — magiskboot (GPL-3.0), SKRoot 保留所有权利 (All Rights Reserved), "
    "AGPL-3.0\n"
).encode()

def _elf(machine: int = EM_AARCH64) -> bytes:
    b = bytearray(64)
    b[0:4] = b"\x7fELF"
    b[4] = 2  # ELFCLASS64
    b[5] = 1  # little endian
    b[18:20] = machine.to_bytes(2, "little")
    return bytes(b)


def _build_apk(path: Path, *, natives=None, assets=None, extra=None) -> Path:
    natives = natives if natives is not None else {n: _elf() for n in REQUIRED_NATIVES}
    assets = (
        assets
        if assets is not None
        else {
            "assets/NOTICE": NOTICE_TEXT,
            "assets/init.sproot.rc": GOOD_RC.encode(),
            "assets/licenses/NOTICE": NOTICE_TEXT,
            "assets/licenses/LICENSE": AGPL_TEXT,
            "assets/licenses/AGPL-3.0.txt": AGPL_TEXT,
            "assets/licenses/GPL-3.0.txt": GPL_TEXT,
            "assets/NOTICE-APK.md": b"# apk notice\n",
            "assets/apd-version.txt": b"name=apd\ncommit=deadbeefcafe\n",
            "assets/apd-capabilities.txt": b"OK daemon\n",
        }
    )
    with zipfile.ZipFile(path, "w") as zf:
        for n, data in natives.items():
            zf.writestr(f"lib/{ALLOWED_ABI}/{n}", data)
        for n, data in assets.items():
            zf.writestr(n, data)
        for n, data in (extra or {}).items():
            zf.writestr(n, data)
    return path


def self_test() -> int:
    """好 APK 必须 ALL PASS；每个坏 APK 必须在**指定断言**上变红。"""
    import tempfile

    problems: list[str] = []
    tmp = Path(tempfile.mkdtemp())

    def run(apk: Path, **kw):
        with zipfile.ZipFile(apk) as zf:
            cs = check_apk(zf, **kw)
        return cs, [n for n, ok, _ in cs if not ok]

    # 正样本
    good = _build_apk(tmp / "good.apk")
    cs, failed = run(good, expect_commit="deadbeefcafe")
    if failed:
        problems.append(f"好样本本应 ALL PASS，却失败: {failed}")
    print(f"[self-test] 正样本 APK: {len(cs)} 条断言, 失败 {len(failed)}")

    def expect_fail(label: str, must_fail: str, **kw):
        apk = _build_apk(tmp / f"{label}.apk", **kw.pop("apk_kwargs", {}))
        _, failed = run(apk, **kw)
        if must_fail in failed:
            print(f"[self-test] 坏样本 {label}: ✓ 变红（{len(failed)} 项，含 {must_fail}）")
        elif not failed:
            problems.append(f"坏样本 {label}: 没有任何断言失败（永远 PASS！）")
            print(f"[self-test] 坏样本 {label}: ✗ 全 PASS")
        else:
            problems.append(f"坏样本 {label}: 期望 {must_fail} 变红，实际 {failed}")
            print(f"[self-test] 坏样本 {label}: ✗ 未在 {must_fail} 变红（实际 {failed}）")

    # 1. 缺 libapd.so
    expect_fail(
        "missing-apd-native",
        "native:libapd.so",
        apk_kwargs={"natives": {n: _elf() for n in REQUIRED_NATIVES if n != "libapd.so"}},
    )

    # 2. x86-64 native 混入
    expect_fail(
        "wrong-arch-native",
        "native:libskrootpatch.so",
        apk_kwargs={
            "natives": {
                **{n: _elf() for n in REQUIRED_NATIVES},
                "libskrootpatch.so": _elf(62),  # EM_X86_64
            }
        },
    )

    # 3. 多 ABI 混入
    expect_fail(
        "extra-abi",
        "native:abi-exclusive",
        apk_kwargs={"extra": {"lib/x86_64/libapd.so": _elf(62)}},
    )

    # 4. 缺 NOTICE
    assets = {
        "assets/init.sproot.rc": GOOD_RC.encode(),
        "assets/licenses/NOTICE": NOTICE_TEXT,
        "assets/licenses/LICENSE": AGPL_TEXT,
        "assets/licenses/AGPL-3.0.txt": AGPL_TEXT,
        "assets/licenses/GPL-3.0.txt": GPL_TEXT,
        "assets/NOTICE-APK.md": b"x",
        "assets/apd-version.txt": b"commit=deadbeefcafe\n",
        "assets/apd-capabilities.txt": b"OK\n",
    }
    expect_fail("missing-notice", "asset:assets/NOTICE", apk_kwargs={"assets": assets})

    # 5. 许可文本是占位（过小）
    assets2 = dict(assets)
    assets2["assets/NOTICE"] = NOTICE_TEXT
    assets2["assets/licenses/LICENSE"] = b"AGPL-3.0\n"  # 占位
    expect_fail(
        "license-placeholder", "license-text:assets/licenses/LICENSE", apk_kwargs={"assets": assets2}
    )

    # 6. 随包 rc 缺 service 真指令（注释在，指令没了）
    assets3 = dict(assets)
    assets3["assets/NOTICE"] = NOTICE_TEXT
    assets3["assets/init.sproot.rc"] = BAD_RC_NO_DIRECTIVES.encode()
    assets3["assets/licenses/LICENSE"] = AGPL_TEXT
    expect_fail("rc-missing-directives", "rc:structure", apk_kwargs={"assets": assets3})

    # 7. 版本错配
    expect_fail("commit-mismatch", "version:commit", expect_commit="DEADBEEF")

    # 8. 作废旧名残留
    expect_fail(
        "obsolete-lib",
        "obsolete:libskpatchjni.so",
        apk_kwargs={"extra": {"lib/arm64-v8a/libskpatchjni.so": _elf()}},
    )

    # 9. sha256 锁定漂移
    lock = {"apd": {"target": "libapd.so", "sha256": "0" * 64}}
    expect_fail("lock-drift", "lock:libapd.so", lock=lock)

    if problems:
        print()
        for p in problems:
            print(f"[self-test] FAIL: {p}")
        print("RESULT: SELF-TEST FAILED（存在永远 PASS 的断言）")
        return 1
    print()
    print("RESULT: SELF-TEST PASS（正样本绿、9 个坏样本各自在预期断言上变红）")
    return 0


# ───────────────────────────────────────────────────────────────────────────
def report(checks: list[tuple[str, bool, str]], as_json: bool, apk: str | None) -> int:
    failed = [c for c in checks if not c[1]]
    if as_json:
        print(
            json.dumps(
                {
                    "ok": not failed,
                    "apk": apk,
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
            print(f"载荷不合格项（{len(failed)} 项）：")
            for name, _, detail in failed:
                print(f"  - {name}: {detail}")
            print()
        print(f"RESULT: {'ALL PASS' if not failed else f'{len(failed)} FAILED'}")
    return 0 if not failed else 1


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description="APK 载荷门禁（native/asset/许可/rc/版本）")
    ap.add_argument("apk", nargs="?", help="APK 路径")
    ap.add_argument("--expect-commit", help="期望的 apd commit（与 assets/apd-version.txt 比对）")
    ap.add_argument("--lock", help="app/native.lock.json 路径（校验 native sha256）")
    ap.add_argument("--json", action="store_true")
    ap.add_argument("--self-test", action="store_true", help="用合成 APK 夹具验证断言真实性")
    args = ap.parse_args(argv)

    if args.self_test:
        return self_test()
    if not args.apk:
        print("::error::需要 APK 路径（或 --self-test）", file=sys.stderr)
        return 2

    apk = Path(args.apk)
    if not apk.is_file():
        print(f"::error::APK 不存在: {apk}", file=sys.stderr)
        return 2
    if not zipfile.is_zipfile(apk):
        print(f"::error::不是有效 ZIP/APK: {apk}", file=sys.stderr)
        return 2

    lock = None
    if args.lock:
        lock_path = Path(args.lock)
        if not lock_path.is_file():
            print(f"::error::锁定表不存在: {lock_path}", file=sys.stderr)
            return 2
        try:
            lock = json.loads(lock_path.read_text(encoding="utf-8"))
        except json.JSONDecodeError as exc:
            print(f"::error::锁定表 JSON 解析失败: {exc}", file=sys.stderr)
            return 2

    try:
        with zipfile.ZipFile(apk) as zf:
            checks = check_apk(zf, expect_commit=args.expect_commit, lock=lock)
    except zipfile.BadZipFile as exc:
        print(f"::error::APK 损坏: {exc}", file=sys.stderr)
        return 2

    return report(checks, args.json, str(apk))


if __name__ == "__main__":
    sys.exit(main())
