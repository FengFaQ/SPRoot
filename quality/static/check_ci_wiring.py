#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""CI 接线自检（WIRE-1）：确保每个质量门禁都被正确接线且不会静默失活。

背景：本项目历史上出现过多次「门禁存在但从未跑」/「改了门禁脚本但 CI 不重跑」
的静默失效（verify_patch.py 只跑 --help 整整一代；apk.yml 漏 apd/assets/** 导致
改模板后 APK 不重建）。这类问题 CI 全绿、代码正确，只是**没执行**。

本门禁检查四件事：
  WIRE-1  每个 quality/static/*.py 都被至少一个 workflow 真实调用
  WIRE-2  调用了某个门禁的 workflow，其 paths 过滤器必须包含该门禁脚本路径
          （否则改了门禁不会重跑 → 门禁改动无人验证）
  WIRE-3  有 --self-test 的门禁，在 CI 里必须真的带 --self-test 调用
          （否则「自检」只是本地玩具）
  WIRE-4  ci-static.yml 必须无 paths 过滤（它是兜底工作流，应每次 push 都跑）

输出：逐条 PASS/FAIL；退出码 0 = 通过，1 = 有问题。
"""

import os
import re
import sys

try:
    import yaml
except ImportError:  # pragma: no cover
    print("::error::需要 pyyaml（pip install pyyaml）")
    sys.exit(2)

REPO = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
WF_DIR = os.path.join(REPO, ".github", "workflows")
Q_DIR = os.path.join(REPO, "quality", "static")


def _verdict_wire2(paths, has_paths, script):
    """WIRE-2 判据（与 main 里同一逻辑，抽出来便于自检驱动）。

    返回 True = 接线正确；False = 改了门禁不会重跑（应判红）。
    """
    if not has_paths:
        return True  # 无 paths 过滤 = 每次都跑，天然安全
    rel = "quality/static/" + script
    return any(
        p.strip("'\"") == rel
        or p.strip("'\"") in ("quality/static/**", "quality/static/")
        or p.strip("'\"").startswith("quality/static/*")
        for p in paths
    )


def _verdict_wire3(text, script):
    """WIRE-3 判据：CI 里是否带 --self-test 调用该门禁。"""
    for m in re.finditer(re.escape(script), text):
        line_start = text.rfind("\n", 0, m.start()) + 1
        line_end = text.find("\n", m.end())
        line = text[line_start: line_end if line_end != -1 else len(text)]
        if "--self-test" in line:
            return True
        vm = re.search(r"([A-Za-z_]\w*)\s*=\s*\S*" + re.escape(script), line)
        if vm and re.search(
                r'"\$\{?' + re.escape(vm.group(1)) + r'\}?"[^\n]*--self-test', text):
            return True
    return False


def _self_test() -> int:
    """负样本铁律：每条判据都必须在**坏输入上判红、好输入上判绿**。

    关键设计：断言的是「判据函数对坏输入返回 False（=门禁会红）」，
    而不是「某个布尔值为 True」。否则所有用例都写 want=True，
    判据退化成恒真也发现不了——那正是本项目最痛的空断言形态。

    每个用例是 (标题, 判据实际结果, 期望结果)：
      - 坏输入 → 期望 False（门禁判定不通过）
      - 好输入 → 期望 True （门禁判定通过）
    两者必须都在，缺一侧就等于没测出判据的鉴别力。
    """
    cases = []

    # ── WIRE-2 ──────────────────────────────────────────────────────
    # 坏输入：被调用但 paths 不含脚本 → 判据必须 False（否则改门禁不重跑却静默）
    cases.append((
        "WIRE-2 坏输入：paths 不含脚本 → 应判红",
        _verdict_wire2(["app/**"], True, "g.py"),
        False,
    ))
    # 好输入：paths 含脚本 → 判据必须 True
    cases.append((
        "WIRE-2 好输入：paths 含脚本 → 应判绿",
        _verdict_wire2(["quality/static/g.py"], True, "g.py"),
        True,
    ))
    # 好输入：无 paths 过滤（每次都跑）→ 判据必须 True
    cases.append((
        "WIRE-2 好输入：无 paths 过滤 → 应判绿",
        _verdict_wire2([], False, "g.py"),
        True,
    ))
    # 好输入：通配 quality/static/** → 判据必须 True
    cases.append((
        "WIRE-2 好输入：paths 用 quality/static/** 通配 → 应判绿",
        _verdict_wire2(["quality/static/**"], True, "g.py"),
        True,
    ))

    # ── WIRE-3 ──────────────────────────────────────────────────────
    # 坏输入：字面调用但不带 --self-test → 判据必须 False
    cases.append((
        "WIRE-3 坏输入：有自检但 CI 不带 → 应判红",
        _verdict_wire3("python3 quality/static/g.py", "g.py"),
        False,
    ))
    # 好输入：字面调用带 --self-test
    cases.append((
        "WIRE-3 好输入：字面调用带 --self-test → 应判绿",
        _verdict_wire3("python3 quality/static/g.py --self-test", "g.py"),
        True,
    ))
    # 好输入：变量间接调用带 --self-test（真实 workflow 的写法）
    cases.append((
        "WIRE-3 好输入：变量间接调用带 --self-test → 应判绿",
        _verdict_wire3('GATE=quality/static/g.py\npython3 "$GATE" --self-test', "g.py"),
        True,
    ))
    # 坏输入：变量定义有、但调用处没带 --self-test
    cases.append((
        "WIRE-3 坏输入：变量调用不带 --self-test → 应判红",
        _verdict_wire3('GATE=quality/static/g.py\npython3 "$GATE"', "g.py"),
        False,
    ))

    # ── WIRE-4 ──────────────────────────────────────────────────────
    # 坏输入：ci-static 加了 paths → 判据必须 False（兜底能力丢失）
    _p, has_ci_paths = trigger_paths(
        {"on": {"push": {"paths": ["app/**"]}}, "jobs": {}})
    cases.append((
        "WIRE-4 坏输入：ci-static 有 paths → 应判红",
        not has_ci_paths,
        False,
    ))
    # 好输入：ci-static 无 paths
    _p2, has_ci_paths2 = trigger_paths({"on": {"push": {}}, "jobs": {}})
    cases.append((
        "WIRE-4 好输入：ci-static 无 paths → 应判绿",
        not has_ci_paths2,
        True,
    ))

    print("== check_ci_wiring.py --self-test ==")
    bad = 0
    for title, got, want in cases:
        ok = got == want
        print("  %-52s 实得%-5s 期望%-5s %s" % (
            title, str(got), str(want), "OK" if ok else "**FAIL**"))
        if not ok:
            bad += 1
    print("")
    if bad:
        print("判定：SELF-TEST FAIL —— 有判据失去鉴别力（坏输入没判红 / 好输入被误判），禁止合入。")
        return 1
    print("判定：SELF-TEST PASS —— 四类判据均在坏输入判红、好输入判绿。")
    return 0


def load_workflows():
    """返回 {文件名: 解析后的 dict}；解析失败直接判错。"""
    out, bad = {}, []
    for name in sorted(os.listdir(WF_DIR)):
        if not name.endswith((".yml", ".yaml")):
            continue
        p = os.path.join(WF_DIR, name)
        try:
            with open(p, encoding="utf-8") as f:
                out[name] = yaml.safe_load(f) or {}
        except Exception as e:
            bad.append((name, str(e)))
    return out, bad


def job_steps_text(wf: dict) -> str:
    """把 workflow 里所有 run: 文本拼起来（用于找脚本调用）。"""
    chunks = []
    for job in (wf.get("jobs") or {}).values():
        for step in (job.get("steps") or []):
            run = step.get("run")
            if isinstance(run, str):
                chunks.append(run)
    return "\n".join(chunks)


def trigger_paths(wf: dict):
    """收集所有 on.<event>.paths（或 paths-ignore）。返回 (paths_list, has_any_paths)。"""
    on = wf.get("on") or wf.get(True) or {}
    if isinstance(on, str):
        return [], False
    paths, any_paths = [], False
    for ev in ("push", "pull_request"):
        cfg = on.get(ev)
        if isinstance(cfg, dict) and "paths" in cfg:
            any_paths = True
            for p in (cfg.get("paths") or []):
                paths.append(str(p))
    return paths, any_paths


def main(argv=None) -> int:
    argv = sys.argv[1:] if argv is None else argv

    if "--self-test" in argv:
        return _self_test()

    fails = []

    def rec(cid, ok, msg):
        print("[%s] %s  %s" % (cid, "PASS" if ok else "FAIL", msg))
        if not ok:
            fails.append(cid)

    if not os.path.isdir(WF_DIR):
        print("::error::找不到 .github/workflows")
        return 1

    wfs, parse_bad = load_workflows()
    rec("WIRE-0", not parse_bad,
        "workflow YAML 全部解析成功（%d 个）" % len(wfs) if not parse_bad
        else "YAML 解析失败：%s" % parse_bad)

    scripts = sorted(f for f in os.listdir(Q_DIR) if f.endswith(".py")
                     and not f.startswith("_"))
    runtext = {n: job_steps_text(w) for n, w in wfs.items()}

    # ── WIRE-1：每个门禁都被调用 ────────────────────────────────────────
    for s in scripts:
        callers = [n for n, t in runtext.items() if s in t]
        rec("WIRE-1", bool(callers),
            "%s <- %s" % (s, ", ".join(callers) if callers else "⚠️ 未被任何 workflow 调用"))

    # ── WIRE-2：被调用 → paths 必须含该脚本 ─────────────────────────────
    # 判据逻辑在 _verdict_wire2（自检驱动同一函数，避免"两套实现"漂移）
    for s in scripts:
        callers = [n for n, t in runtext.items() if s in t]
        for n in callers:
            paths, has_paths = trigger_paths(wfs[n])
            hit = _verdict_wire2(paths, has_paths, s)
            rec("WIRE-2", hit,
                "%s 调用 %s → paths 包含该脚本（或无 paths 过滤）" % (n, s) if hit
                else "%s 调用 %s，但 paths 不含 quality/static/%s（改门禁不会重跑！paths=%s）"
                     % (n, s, s, paths))

    # ── WIRE-3：有 --self-test 的门禁，CI 必须带 --self-test 调用 ────────
    # 判据逻辑在 _verdict_wire3：需识别 `GATE=...py` + `python3 "$GATE" --self-test`
    # 这类变量间接写法（真实 workflow 就是这么写的）。
    for s in scripts:
        p = os.path.join(Q_DIR, s)
        with open(p, encoding="utf-8", errors="replace") as f:
            src = f.read()
        if "--self-test" not in src:
            continue
        callers = [n for n, t in runtext.items() if s in t]
        if not callers:
            continue  # WIRE-1 已报
        ok = any(_verdict_wire3(runtext[n], s) for n in callers)
        rec("WIRE-3", ok,
            "%s 在 CI 里带 --self-test 调用" % s if ok
            else "%s 有 --self-test 但 CI 未调用它（自检只是本地玩具）" % s)

    # ── WIRE-4：ci-static 是兜底工作流，应无 paths 过滤 ──────────────────
    if "ci-static.yml" in wfs:
        _paths, has_paths = trigger_paths(wfs["ci-static.yml"])
        rec("WIRE-4", not has_paths,
            "ci-static.yml 无 paths 过滤（每次 push 都跑，作兜底）" if not has_paths
            else "ci-static.yml 加了 paths 过滤 → 兜底能力丢失（paths=%s）" % _paths)

    print("")
    print("汇总：FAIL=%d" % len(fails))
    if fails:
        print("判定：FAIL —— CI 接线存在静默失活风险：%s" % ", ".join(sorted(set(fails))))
        return 1
    print("判定：PASS —— 所有质量门禁均已接线且不会静默失活")
    return 0


if __name__ == "__main__":
    sys.exit(main())
