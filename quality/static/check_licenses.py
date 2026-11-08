#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
"""许可标注漂移防护（LIC-DRIFT）。

单一事实源 = ``LICENSES.md`` §2.1 表格里的 sha256 指纹。
本脚本断言：

  1. 仓库根存在 ``LICENSE``（AGPL-3.0 全文）、``LICENSES.md``、``NOTICE``；
  2. ``LICENSES.md`` 声明的每个上游文件确实存在，且 **sha256 与文档记录一致**；
  3. ``LICENSES.md`` 中包含「保留所有权利 / All Rights Reserved」与
     「AGPL-3.0 不适用于该部分」的明确表述；
  4. ``NOTICE`` 中包含 magiskboot 的 GPL-3.0 声明与 SKRoot 上游声明。

为什么需要它：`engine/src` 里 15 个 patch 文件是从 SKRoot 上游逐字节拷贝而来的
proprietary 内容。一旦有人改动其中任何一字节，`LICENSES.md` 的「完全相同」标注
就变成错误陈述（法律风险），必须强制人工复核并更新文档。故本脚本让 CI 在漂移时失败。

用法::

    python3 quality/static/check_licenses.py [--repo-root .] [--json]

退出码：0 = 全部通过；1 = 存在漂移或缺失。
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path

# LICENSES.md §2.1 行格式：| 1 | `engine/src/patch_base.cpp` | 7,485 | `sha256...` | 完全相同 |
ROW_RE = re.compile(
    r"^\|\s*\d+\s*\|\s*`(?P<path>[^`]+)`\s*\|\s*(?P<size>[\d,]+)\s*\|\s*`(?P<sha>[0-9a-f]{64})`\s*\|",
    re.MULTILINE,
)

# 每组为「同义词候选」，命中任意一个即可（大小写不敏感）
REQUIRED_GROUPS_LICENSES = [
    ["保留所有权利", "all rights reserved"],
    ["AGPL-3.0"],
    ["SKRoot"],
]
REQUIRED_GROUPS_NOTICE = [
    ["magiskboot"],
    ["GPL-3.0"],
    ["SKRoot"],
    ["保留所有权利", "all rights reserved"],
]


def sha256_of(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo-root", default=".", help="仓库根目录")
    ap.add_argument("--json", action="store_true", help="以 JSON 输出结果")
    args = ap.parse_args()

    root = Path(args.repo_root).resolve()
    checks: list[dict] = []

    def check(name: str, ok: bool, detail: str) -> None:
        checks.append({"name": name, "ok": ok, "detail": detail})

    # ---- 1. 必需文件存在 ----
    for name in ("LICENSE", "LICENSES.md", "NOTICE"):
        p = root / name
        check(f"file:{name}", p.is_file(), str(p) if p.is_file() else f"缺失: {p}")

    lic_path = root / "LICENSES.md"
    if not lic_path.is_file():
        return emit(checks, args.json)

    lic_text = lic_path.read_text(encoding="utf-8", errors="replace")

    # ---- 2. 许可性质表述 ----
    low_lic = lic_text.lower()
    for group in REQUIRED_GROUPS_LICENSES:
        hit = next((p for p in group if p.lower() in low_lic), None)
        check(
            f"licenses-phrase:{group[0]}",
            hit is not None,
            f"LICENSES.md 需包含「{'|'.join(group)}」之一"
            + (f"（命中「{hit}」）" if hit else ""),
        )

    notice_path = root / "NOTICE"
    notice_text = notice_path.read_text(encoding="utf-8", errors="replace") if notice_path.is_file() else ""
    low_notice = notice_text.lower()
    for group in REQUIRED_GROUPS_NOTICE:
        hit = next((p for p in group if p.lower() in low_notice), None)
        check(
            f"notice-phrase:{group[0]}",
            hit is not None,
            f"NOTICE 需包含「{'|'.join(group)}」之一"
            + (f"（命中「{hit}」）" if hit else ""),
        )

    # ---- 3. 上游文件 sha256 指纹一致性（核心） ----
    rows = list(ROW_RE.finditer(lic_text))
    check(
        "licenses:upstream-table",
        len(rows) >= 15,
        f"LICENSES.md §2.1 应声明 ≥15 个上游文件，实际解析到 {len(rows)} 行",
    )

    for m in rows:
        rel = m.group("path")
        expected = m.group("sha")
        size_doc = m.group("size")
        target = root / rel
        if not target.is_file():
            check(f"upstream-file:{rel}", False, f"文件缺失: {target}")
            continue
        actual = sha256_of(target)
        actual_size = target.stat().st_size
        ok = actual == expected
        detail = (
            f"sha256 一致（{actual_size} B）"
            if ok
            else f"sha256 漂移: 文档={expected[:16]}… 实际={actual[:16]}… "
            f"→ 上游文件已被修改，必须复核 LICENSES.md 标注并更新该行"
        )
        check(f"upstream-sha:{rel}", ok, detail)
        if size_doc:
            doc_size = int(size_doc.replace(",", ""))
            check(
                f"upstream-size:{rel}",
                doc_size == actual_size,
                f"文档记录 {doc_size} B / 实际 {actual_size} B",
            )

    return emit(checks, args.json)


def emit(checks: list[dict], as_json: bool) -> int:
    failed = [c for c in checks if not c["ok"]]
    if as_json:
        print(json.dumps({"ok": not failed, "checks": checks}, ensure_ascii=False, indent=2))
    else:
        for c in checks:
            print(f"[{'PASS' if c['ok'] else 'FAIL'}] {c['name']}: {c['detail']}")
        print()
        print(f"RESULT: {'ALL PASS' if not failed else f'{len(failed)} FAILED'}")
    return 0 if not failed else 1


if __name__ == "__main__":
    sys.exit(main())
