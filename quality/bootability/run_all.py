#!/usr/bin/env python3
"""B0 开机能力验证 —— 统一入口（一键跑完 A/B/C + 自检）。

这条链和 ``quality/static/`` 是**两套独立体系**：
  旧体系断言「文件等长 / sha 变了 / report 字段齐全」
  本入口回答「patch 落在哪、有没有踩驱动、修完还能不能跑起来」

用法::

    # 完整跑（需要原厂 + patched kernel）
    python3 quality/bootability/run_all.py \
        --orig kernel.orig --patched kernel.patched \
        --engine-log engine.log \
        --out quality/bootability/records

    # 只跑不依赖 qemu 的 A/B
    python3 quality/bootability/run_all.py --orig ... --patched ... --skip-qemu

    # 只跑自检（验证断言有牙齿，不需要样本）
    python3 quality/bootability/run_all.py --self-test-only

退出码：0 = 无阻断发现且无回归；1 = 有问题；2 = 用法错误。
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))


def run(step: str, cmd: list[str]) -> tuple[int, str]:
    print(f"\n{'=' * 70}\n{step}\n{'=' * 70}")
    proc = subprocess.run(cmd, capture_output=True, text=True, errors="replace")
    out = (proc.stdout or "") + (proc.stderr or "")
    print(out.rstrip())
    return proc.returncode, out


def check_ok(path: str | None, label: str) -> None:
    """Fail loudly on a required input that does not exist.

    A missing ``--report`` used to make the audit silently report
    ``attributed_regions: 0`` and still exit 0. The audit itself now blocks on
    that (PA-0), but a caller-side check gives a much better error message and
    stops the whole run before it produces a misleading summary.

    BUG-QA-12（2026-11-12 审计）：旧版此函数只有 docstring 没有 body（``pass``），
    文档承诺的预检从未发生；现已真实实现并在 main() 的 --report/--orig/--patched
    处调用。
    """
    if not path:
        print(f"\n[FATAL] 缺少 --{label}：无法继续（{label} 是本层验证的必需输入）",
              file=sys.stderr)
        sys.exit(2)
    if not os.path.isfile(path):
        print(f"\n[FATAL] --{label} 指向的文件不存在: {path}", file=sys.stderr)
        sys.exit(2)


def arbitration(results: dict[str, dict]) -> tuple[int, list[str]]:
    """BUG-QA-4：两套体系（bootability A/B/C）结论聚合 + 冲突显式化。

    本入口只代表 bootability 一套；crossverify 由 quality/crossverify/run.sh
    独立执行。run.sh 汇入 crossverify 的 JSON 后以本函数同样的规则仲裁：
    任一 FAIL 即 FAIL（更严格者为准），冲突时必须显式输出 CONFLICT。

    返回 (exit_code, conflict_notes)。
    """
    notes: list[str] = []
    failed = [k for k, v in results.items() if v.get("ok") is False]
    passed = [k for k, v in results.items() if v.get("ok") is True]
    infeasible = [k for k, v in results.items() if v.get("ok") is None
                  and (v.get("infeasible") or v.get("skipped"))]
    # 冲突定义：同一对象上既存在 PASS 又存在 FAIL 的层级
    if failed and passed:
        notes.append(
            "CONFLICT: bootability 内部层级结论不一致 —— PASS=%s FAIL=%s；"
            "以更严格（FAIL）为准，PASS 层的证据仍有效，需人工归因差异"
            % (",".join(passed), ",".join(failed)))
    if infeasible and (failed or passed):
        notes.append(
            "NOTE: 部分层不可执行（%s）——不可执行不等于通过，结论覆盖度受限"
            % ",".join(infeasible))
    return (1 if failed else 0), notes


def stage_status(json_path: str, rc: int) -> dict:
    """Read a stage's own verdict from its JSON, never a default.

    The lead audit caught that ``run_all`` used to consider a stage ok without
    reading any field — i.e. a missing/renamed key would default to True. Each
    stage therefore declares its verdict explicitly:
      * patch_audit / driver_integrity: ``blocking`` must be empty
      * boot: ``comparison.regression`` must be empty
    """
    info: dict = {"rc": rc, "ok": rc == 0, "source": None, "detail": ""}
    if not os.path.isfile(json_path):
        info["ok"] = False
        info["detail"] = f"result file missing: {json_path}"
        return info
    try:
        with open(json_path, encoding="utf-8") as fh:
            data = json.load(fh)
    except (OSError, json.JSONDecodeError) as e:
        info["ok"] = False
        info["detail"] = f"unreadable result: {e}"
        return info

    if "blocking" in data:  # A or B
        blk = data["blocking"]
        info["source"] = "blocking[]"
        info["ok"] = rc == 0 and not blk
        info["detail"] = f"{len(blk)} blocking" if blk else "0 blocking"
        if "attribution" in data:
            info["attribution"] = data["attribution"]
    elif "comparison" in data:  # C
        reg = data["comparison"].get("regression") or []
        info["source"] = "comparison.regression"
        info["ok"] = rc == 0 and not reg
        info["detail"] = f"{len(reg)} regression" if reg else "no regression"
        info["identical_behaviour"] = data["comparison"].get("identical_behaviour")
    else:
        # Unknown shape: do NOT default to True.
        info["ok"] = False
        info["detail"] = "result has neither blocking[] nor comparison{} — cannot judge"
    return info


def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description="B0 开机能力验证（统一入口）")
    ap.add_argument("--orig", help="原厂 kernel Image")
    ap.add_argument("--patched", help="patched kernel Image")
    ap.add_argument("--engine-log", help="引擎 stdout（用于把偏移映射回符号）")
    ap.add_argument("--report", help="引擎 report.json")
    ap.add_argument("--out", default=os.path.join(HERE, "records"))
    ap.add_argument("--timeout", type=int, default=25, help="每次 qemu 运行秒数")
    ap.add_argument("--skip-qemu", action="store_true", help="只跑 A/B（无 qemu 时）")
    ap.add_argument("--self-test-only", action="store_true")
    args = ap.parse_args(argv)

    os.makedirs(args.out, exist_ok=True)
    py = sys.executable
    results: dict[str, dict] = {}

    # ---- 0. 自检：先证明断言有牙齿 --------------------------------
    rc, out = run("0. 自检（负样本必须变红）", [
        py, os.path.join(HERE, "self_test.py"), "--json",
        os.path.join(args.out, "self-test.json"),
    ])
    results["self_test"] = {"rc": rc, "ok": rc == 0}
    if rc != 0:
        print("\n!! 自检失败：有断言退化成永远 PASS —— 后续结论不可信，终止。")
        return 1

    if args.self_test_only:
        print("\n（--self-test-only：只跑自检）")
        return 0

    if not (args.orig and args.patched):
        print("ERROR: 需要 --orig 与 --patched（或使用 --self-test-only）", file=sys.stderr)
        return 2

    # ---- A. 逐代码审计（--report 是硬要求）-------------------------
    # 没有 report 时 patch_audit 只能做字节 diff、无法回答「改的是哪里」，
    # 而「归属到具体函数」正是本层的价值。虽然 PA-0 会让它判 FAIL，这里
    # 提前拦一次，给出比 `blocking: 1` 更直接的原因。
    # （BUG-QA-12：预检由 check_ok 真实执行，不再只是 docstring 承诺。）
    check_ok(args.orig, "orig")
    check_ok(args.patched, "patched")
    check_ok(args.report, "report")

    a_json = os.path.join(args.out, "patch-audit.json")
    base = [py, os.path.join(HERE, "patch_audit.py"), "--orig", args.orig,
            "--patched", args.patched, "--json", a_json, "--report", args.report]
    rc, out = run("A. 逐代码补丁审计", base)
    results["patch_audit"] = stage_status(a_json, rc)

    d_json = os.path.join(args.out, "driver-integrity.json")
    dcmd = [py, os.path.join(HERE, "driver_integrity.py"), "--orig", args.orig,
            "--patched", args.patched, "--json", d_json, "--report", args.report]
    if args.engine_log:
        dcmd += ["--engine-log", args.engine_log]
    rc, out = run("B. 驱动完整性检查", dcmd)
    results["driver_integrity"] = stage_status(d_json, rc)

    if args.skip_qemu:
        print("\n（--skip-qemu：C 层未执行）")
        results["boot"] = {"rc": None, "ok": None, "skipped": True}
    else:
        b_json = os.path.join(args.out, "boot-qemu.json")
        rc, out = run("C. 开机行为验证（qemu，原厂 vs patched）", [
            py, os.path.join(HERE, "boot_qemu.py"),
            "--orig", args.orig, "--patched", args.patched,
            "--record-dir", args.out, "--timeout", str(args.timeout),
            "--json", b_json,
        ])
        # rc==3 表示 qemu 不可用：如实记为不可行，不是失败
        if rc == 3:
            results["boot"] = {"rc": rc, "ok": None, "infeasible": True,
                               "detail": "qemu unavailable"}
        else:
            results["boot"] = stage_status(b_json, rc)

    print("\n" + "=" * 70)
    print("汇总")
    print("=" * 70)
    for k, v in results.items():
        if v.get("skipped"):
            print(f"  {k:<20} SKIPPED")
            continue
        if v.get("infeasible"):
            print(f"  {k:<20} INFEASIBLE（qemu 不可用，已如实记录）")
            continue
        mark = "PASS" if v["ok"] else "FAIL"
        src = v.get("source") or "-"
        print(f"  {k:<20} {mark:<5} [{src}] {v.get('detail','')}")
        att = v.get("attribution")
        if att:
            print(
                f"  {'':<20} 归属覆盖 {att['coverage']} "
                f"(已归属 {att['attributed_regions']} / 未归属 {att['unattributed_regions']}, "
                f"来源 {att['source']})"
            )
        if v.get("identical_behaviour") is not None:
            print(f"  {'':<20} 原厂↔patched 行为完全一致: {v['identical_behaviour']}")

    with open(os.path.join(args.out, "summary.json"), "w", encoding="utf-8") as fh:
        json.dump(results, fh, ensure_ascii=False, indent=2)

    rc, conflict_notes = arbitration(results)
    for note in conflict_notes:
        print(f"\n{note}")
    if rc:
        failed = [k for k, v in results.items() if v.get("ok") is False]
        print(f"\n判定：FAIL（{', '.join(failed)}）")
        return 1
    print("\n判定：PASS（A/B 无阻断发现，C 无回归）")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
