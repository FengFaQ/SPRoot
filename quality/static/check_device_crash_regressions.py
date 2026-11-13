#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""真机崩溃回归门禁（THREAD-1 / KEYABI-1）。

背景：2026-11-11 真机（vivo V2344A / Android 15）一次使用中暴露两个崩溃/假失败，
两者都**不会被编译器和既有静态门禁发现**，只在真机运行期炸。本脚本把它们固化成
可静态检查的规则，防止再次回归。

证据文件：
  crash-20261111-065108-576.txt   —— Toast 跨线程崩溃
  sproot-log-20261111-065108.txt  —— 密钥 ABI 48/47 假失败

────────────────────────────────────────────────────────────────────────────
THREAD-1：`Toast.makeText(...)` 不得出现在后台线程里
────────────────────────────────────────────────────────────────────────────
真机堆栈：
    java.lang.NullPointerException: Can't toast on a thread that has not called
    Looper.prepare()
      at android.widget.Toast.<init>(Toast.java:173)
      at com.sproot.manager.ui.pages.PatchPageKt.LogExportLine$lambda$0$0$0(PatchPage.kt:502)

根因：`Thread { ... Toast.makeText(...).show() }.start()` —— Toast 构造需要
当前线程有 Looper，后台 Thread 没有，直接 NPE（主线程才有）。

判据（保守，避免误伤）：
  同一「语句块」内，若 `Thread {` / `Executors.new...` / `.execute {` 出现在
  Toast.makeText **之前**，且期间没有出现 `Looper.getMainLooper()` 或
  `runOnUiThread` / `withContext(Dispatchers.Main)` 之类的主线程切回，
  则判 FAIL。
  行内/上方 3 行内有主线程切回关键字 → 视为安全。

────────────────────────────────────────────────────────────────────────────
KEYABI-1：引擎回传的 root_key（48 字符）不得直接交给 kernelKeyOf（要求 47）
────────────────────────────────────────────────────────────────────────────
真机日志：
    ERROR [Pipeline] 修补失败：修补异常中断
    java.lang.IllegalArgumentException: Root 密钥必须是 47 字符 [A-Za-z0-9]
      （内核 ABI 47+NUL），实际长度=48
        at SecretStore$Companion.kernelKeyOf(SecretStore.kt:89)
        at PatchPipeline.execute(PatchPipeline.kt:833)

根因：引擎 `report.root_key` 是**调引擎时传入的 48 字符**（47 + 1 位填充），
而 `kernelKeyOf()` 的契约是「只接受内核 ABI 的 47 字符」并在长度不符时 `require` 抛异常。
两者直接对接 = 必然崩溃。正确做法是先用 `normalizeEngineKey()` 归一。

判据：
  `kernelKeyOf(` / `engineKeyOf(` 的实参若直接是 `lastRootKey()` 的返回值
  或经 `?:` / `let` 链从它传来，而**中间没有** `normalizeEngineKey`，判 FAIL。

输出：逐条 file:line + 说明；退出码 0 = 通过，1 = 发现问题。
"""

import argparse
import os
import re
import sys

DEFAULT_ROOT = "app/src/main/java"

# ── THREAD-1 ────────────────────────────────────────────────────────────────
# 后台执行上下文出现标志
BG_OPEN_RE = re.compile(r"\bThread\s*\{|Executors\.new\w+\(|\.execute\s*\{|CoroutineScope\(Dispatchers\.IO\)")
# 主线程切回标志（出现即视为安全）
MAIN_HOP_RE = re.compile(
    r"Looper\.getMainLooper\(\)|runOnUiThread|withContext\(\s*Dispatchers\.Main|"
    r"Handler\(\s*Looper\.getMainLooper\(\)|post\s*\{"
)
TOAST_RE = re.compile(r"Toast\.makeText\s*\(")

# ── KEYABI-1 ────────────────────────────────────────────────────────────────
# 48 字符来源：引擎回传的密钥
KEYABI_SOURCE_RE = re.compile(r"lastRootKey\s*\(\s*\)")
# 47 契约函数的调用（含链式 `?.let { kernelKeyOf(it) }` 形态）
KEYABI_SINK_RE = re.compile(r"\b(?:kernelKeyOf|engineKeyOf)\s*\(")
# 允许：中间有归一
KEYABI_OK_RE = re.compile(r"normalizeEngineKey\s*\(")


def _strip_line_comment(text: str) -> str:
    """去掉行尾 `//` 注释（粗略，够用；不处理字符串内的 //，本规则不涉及）。"""
    idx = text.find("//")
    return text[:idx] if idx >= 0 else text


def _read_lines(path: str):
    """读文本行；坏编码/读失败返回 None（门禁不得因单个坏文件崩溃）。

    注意：本函数刻意吞异常而不是抛出 —— 若要检查的文件被外部工具写成
    UTF-16/BOM 异常等，应该记一条明确的 FAIL（见 main），而不是让整个
    CI 步骤因 traceback 而红得莫名其妙。
    """
    try:
        with open(path, encoding="utf-8") as f:
            return f.read().splitlines()
    except (OSError, UnicodeDecodeError):
        return None


def check_thread_toast(path: str) -> list:
    """THREAD-1：后台线程里的 Toast.makeText。"""
    problems = []
    lines = _read_lines(path)
    if lines is None:
        return problems

    # 找每个 `Thread {` 或 `.execute {` 的起始行，配对到块结束（用花括号深度）。
    for start, raw in enumerate(lines):
        code = _strip_line_comment(raw)
        m = BG_OPEN_RE.search(code)
        if not m:
            continue
        # 从 `{` 位置开始数深度，跟踪到配对的 `}`
        depth = 0
        started = False
        block = []
        for i in range(start, min(start + 400, len(lines))):
            seg = _strip_line_comment(lines[i]) if i != start else code[m.end():]
            for ch in seg:
                if ch == "{":
                    depth += 1
                    started = True
                elif ch == "}":
                    depth -= 1
            block.append((i + 1, lines[i]))
            if started and depth <= 0:
                break
        # 在块内找 Toast，且之前没有主线程切回
        seen_main_hop = False
        for lineno, text in block:
            t = _strip_line_comment(text)
            if MAIN_HOP_RE.search(t):
                seen_main_hop = True
            if TOAST_RE.search(t) and not seen_main_hop:
                problems.append((
                    lineno,
                    "THREAD-1：Toast.makeText 出现在后台线程块内（起始于第 %d 行），"
                    "且该处之前没有 Looper.getMainLooper()/runOnUiThread/post{} 等主线程切回。"
                    "Android 上 Toast 构造要求当前线程有 Looper，后台 Thread 会抛 "
                    "NullPointerException: Can't toast on a thread that has not called "
                    "Looper.prepare()。修法：先算好消息文本，再 "
                    "Handler(Looper.getMainLooper()).post { Toast.makeText(...).show() }。"
                    % (start + 1,)
                ))
    return problems


def check_engine_key_abi(path: str) -> list:
    """KEYABI-1：48 字符引擎密钥直接喂给 47 契约。

    检测思路：以**语句为单位**（一句 Kotlin 表达式，可能跨多行），若同一句里
    同时出现 ①引擎回传来源 `lastRootKey()` 与 ②47 契约函数 `kernelKeyOf(` /
    `engineKeyOf(`，且**没有** `normalizeEngineKey(` 归一，则判 FAIL。

    这样既能抓相邻写法 `kernelKeyOf(lastRootKey())`，也能抓链式写法
    `lastRootKey()?.let { kernelKeyOf(it) }`（真机实际形态）。
    """
    problems = []
    lines = _read_lines(path)
    if lines is None:
        return problems

    # 组装「语句」：以行尾不是 , ( ?. ?: = + 且括号/花括号配平作为语句边界。
    stmt = []
    stmt_start = 0
    depth = 0
    for idx, raw in enumerate(lines):
        code = _strip_line_comment(raw)
        if not stmt:
            stmt_start = idx + 1
        stmt.append(code)
        for ch in code:
            if ch in "({[":
                depth += 1
            elif ch in ")}]":
                depth -= 1
        # 语句是否结束：括号配平 且 行尾不是续行标记
        tail = code.rstrip()
        continues = bool(re.search(r"[,(\[=+.]$", tail)) or tail.endswith("?")
        if depth <= 0 and not continues:
            text = "\n".join(stmt)
            if KEYABI_SOURCE_RE.search(text) and KEYABI_SINK_RE.search(text):
                if not KEYABI_OK_RE.search(text):
                    problems.append((
                        stmt_start,
                        "KEYABI-1：同一语句内，引擎回传的 root_key（lastRootKey()）"
                        "直接交给了 kernelKeyOf/engineKeyOf，且没有 normalizeEngineKey 归一。"
                        "引擎 report.root_key 是**调引擎时传入的 48 字符**（47 + 1 位填充），"
                        "而 kernelKeyOf() 只接受内核 ABI 的 47 字符并在长度不符时 require 抛异常"
                        "（真机表现：产物已导出、状态机却置 ERROR 的假失败）。"
                        "修法：先 normalizeEngineKey(raw) 归一，再交给 kernelKeyOf。"
                    ))
            stmt = []
            depth = 0
    return problems


# ── BUSY-1 ──────────────────────────────────────────────────────────────────
# busy 标志（AtomicBoolean.compareAndSet 串行闸门）+ 同函数内的 try/finally 恢复
BUSY_GATE_RE = re.compile(r"busy\.compareAndSet\s*\(\s*false\s*,\s*true\s*\)")
BUSY_RELEASE_RE = re.compile(r"busy\.set\s*\(\s*false\s*\)")
FUN_DECL_RE = re.compile(r"^\s*(?:private\s+|internal\s+|public\s+|protected\s+)?"
                         r"(?:suspend\s+)?fun\s+(\w+)\s*\(")


def _extract_function_blocks(lines):
    """提取 (函数名, 起始行0基, 结束行0基) 列表（按花括号深度，保守实现）。"""
    blocks = []
    i = 0
    while i < len(lines):
        m = FUN_DECL_RE.match(lines[i])
        if not m:
            i += 1
            continue
        depth = 0
        started = False
        end = i
        for j in range(i, len(lines)):
            for ch in lines[j]:
                if ch == "{":
                    depth += 1
                    started = True
                elif ch == "}":
                    depth -= 1
            if started and depth <= 0:
                end = j
                break
        blocks.append((m.group(1), i, end))
        i = end + 1
    return blocks


def check_busy_finally_coverage(path: str) -> list:
    """BUSY-1：含 busy 闸门的函数里，busy.set(false) 必须出现在 finally 块内，
    且函数体内 try 块开始之前的任何 return 都被禁止。

    真机根因（bug-app 2026-11-12 审计）：PatchPipeline.execute() 的密钥获取
    `getOrElse { finishFailure(...); return }` 写在 try 之前 → 该 return 跳过
    finally 的 busy.set(false) → busy 永久 true → 修补功能整个进程内锁死。
    这类缺陷编译器不报、运行期才炸，固化为静态规则。
    """
    problems = []
    lines = _read_lines(path)
    if lines is None:
        return problems
    text = "\n".join(lines)
    if not BUSY_GATE_RE.search(text):
        return problems  # 本文件不用 busy 闸门
    for fname, start, end in _extract_function_blocks(lines):
        body = lines[start:end + 1]
        body_text = "\n".join(body)
        if not BUSY_GATE_RE.search(body_text):
            continue
        # 找 finally { 的行（该函数内）
        finally_line = None
        depth = 0
        j = 0
        while j < len(body):
            code = _strip_line_comment(body[j])
            if re.search(r"\bfinally\s*\{", code):
                finally_line = j
                break
            j += 1
        if finally_line is None:
            # 无 finally 模式：允许"每个非闸门 return 紧邻之前显式 busy.set(false)"
            # （如 start() 的 early-return 分支）；检查每个 return 前 3 行内是否有释放。
            gate_line = None
            for k in range(0, len(body)):
                if BUSY_GATE_RE.search(_strip_line_comment(body[k])):
                    gate_line = k
                    break
            for k in range(0, len(body)):
                # 放行两类不持有闸门的 return：
                # ① 闸门行自身的 compareAndSet 失败快速退出；
                # ② 闸门行之后紧随的 if(!gate) 块内 return（compareAndSet 失败分支，
                #    例如 start() 的 `if (current.running || !busy.compareAndSet(...)) return`）。
                if gate_line is not None and k <= gate_line + 2:
                    # 只放行闸门行与其 if 条件块（缩进更深且在闸门行后的前 2 行内）
                    if k == gate_line:
                        continue
                    gate_indent = len(body[gate_line]) - len(body[gate_line].lstrip())
                    k_indent = len(body[k]) - len(body[k].lstrip())
                    if k_indent > gate_indent:
                        continue
                code = _strip_line_comment(body[k])
                if re.search(r"(?:^|[^\w])return\b", code):
                    window = "\n".join(_strip_line_comment(x) for x in body[max(0, k - 3):k])
                    if not BUSY_RELEASE_RE.search(window):
                        problems.append((
                            start + k + 1,
                            "BUSY-1：函数 %s 的 return（第 %d 行）之前没有 finally，"
                            "且前 3 行内也没有 busy.set(false)——该路径不释放闸门，"
                            "操作入口此后整个进程内锁死。修法：用 try/finally 包住主逻辑。"
                            % (fname, start + k + 1),
                        ))
            continue
        # try { 的行（finally 之前最后一个 try）
        try_line = None
        for k in range(finally_line - 1, -1, -1):
            if re.search(r"(?:^|[=\(\)\s])try\s*\{", _strip_line_comment(body[k])):
                try_line = k
                break
        if try_line is None:
            problems.append((
                start + 1,
                "BUSY-1：函数 %s 使用 busy 闸门且有 finally，但找不到配对的 try 块" % fname,
            ))
            continue
        # try 之前（函数头到 try_line）出现裸 return → 会跳过 finally，判 FAIL。
        # 例外：闸门行自身（busy.compareAndSet 失败快速退出）不持有闸门，放行。
        gate_line = None
        for k in range(0, try_line):
            if BUSY_GATE_RE.search(_strip_line_comment(body[k])):
                gate_line = k
                break
        for k in range(0, try_line):
            if gate_line is not None and k == gate_line:
                continue
            code = _strip_line_comment(body[k])
            if re.search(r"(?:^|[^\w])return\b", code):
                problems.append((
                    start + k + 1,
                    "BUSY-1：函数 %s 在 try/finally 之前出现 return（第 %d 行）——"
                    "该路径会跳过 finally 的 busy.set(false)，busy 永久为 true，"
                    "修补/操作入口整个进程内锁死。修法：把这段逻辑挪进 try 内首个出口。"
                    % (fname, start + k + 1),
                ))
        # finally 块内必须有 busy.set(false)
        finally_text = "\n".join(body[finally_line:])
        if not BUSY_RELEASE_RE.search(finally_text):
            problems.append((
                start + finally_line + 1,
                "BUSY-1：函数 %s 的 finally 块内没有 busy.set(false)——闸门不会被释放" % fname,
            ))
    return problems


# ── EXPORT-1 / EXPORT-2 ─────────────────────────────────────────────────────
EXPORT_CALL_RE = re.compile(r"Exporter\.exportToDownloads\s*\(")
# 校验步骤的标志行（导出必须发生在最后一个校验之后）
VERIFY_MARK_RE = re.compile(
    r"回读校验通过|checkAvb\s*\(|kernelReadbackSha\s*\(|verifyProduct\s*\("
)
# EXPORT-2：导出调用链上必须有权限检查（Exporter 自身或调用文件内）
PERM_CHECK_RE = re.compile(
    r"needsLegacyStoragePermission|checkSelfPermission.*WRITE_EXTERNAL_STORAGE"
)


def check_export_order_and_permission(path: str) -> list:
    """EXPORT-1：同函数内多个 exportToDownloads 之前必须先完成校验（回读/AVB/rc verify），
    导出调用之间不允许再夹校验失败出口造成的"一半成功"。

    判据（行号单调性）：PatchPipeline.execute() 内，第一个 exportToDownloads 的行号
    必须 > 最后一个校验标志（回读校验通过 / checkAvb / kernelReadbackSha / verifyProduct）
    的行号。若导出被挪回校验之前（回归 2026-11-13 修复前的"先导 boot 后校验 init_boot"
    形态），判 FAIL。

    EXPORT-2：文件内出现 exportToDownloads 调用时，Exporter.kt 必须提供
    needsLegacyStoragePermission（或调用文件内有 checkSelfPermission WRITE_EXTERNAL_STORAGE），
    否则 Android 8~9 设备导出必失败且文案不区分原因。
    """
    problems = []
    lines = _read_lines(path)
    if lines is None:
        return problems

    export_lines = [i + 1 for i, l in enumerate(lines) if EXPORT_CALL_RE.search(_strip_line_comment(l))]
    if not export_lines:
        return problems

    # EXPORT-2（文件级）：权限检查可以在调用文件内，也可以在 Exporter.kt 自身
    # （needsLegacyStoragePermission 定义在那里，供调用方导出前查询）。
    def _has_perm_check(file_path, text_lines):
        return bool(PERM_CHECK_RE.search("\n".join(_strip_line_comment(l) for l in text_lines)))

    if not _has_perm_check(path, lines):
        exporter_path = os.path.join(os.path.dirname(path), "Exporter.kt")
        exporter_lines = _read_lines(exporter_path) if os.path.isfile(exporter_path) else None
        if exporter_lines is None or not _has_perm_check(exporter_path, exporter_lines):
            problems.append((
                export_lines[0],
                "EXPORT-2：文件内调用 Exporter.exportToDownloads，但本文件与 Exporter.kt "
                "均没有存储权限检查（needsLegacyStoragePermission / checkSelfPermission"
                "(WRITE_EXTERNAL_STORAGE)）。Android 8~9（minSdk=26）缺运行时授权时导出"
                "必抛 SecurityException，且失败文案无法区分「缺权限」与其它 IO 失败。",
            ))

    # EXPORT-1（仅针对流水线主文件 execute 函数）
    if not path.replace("\\", "/").endswith("core/PatchPipeline.kt"):
        return problems
    blocks = _extract_function_blocks(lines)
    for fname, start, end in blocks:
        if fname != "execute":
            continue
        verify_marks = [
            i + 1 for i in range(start, end + 1)
            if VERIFY_MARK_RE.search(_strip_line_comment(lines[i]))
        ]
        func_exports = [ln for ln in export_lines if start + 1 <= ln <= end + 1]
        if not func_exports or not verify_marks:
            continue
        if min(func_exports) < max(verify_marks):
            problems.append((
                min(func_exports),
                "EXPORT-1：execute() 中 exportToDownloads（第 %d 行）出现在校验步骤"
                "（最后一批校验标志在第 %d 行）之前——导出先行会造成"
                "「boot 已落 Downloads、后续校验失败」的一半成功产物。"
                "铁律：先全部校验（回读/AVB/rc verify），最后连续导出（init_boot 先、boot 后，"
                "boot 失败时回滚已导出的 init_boot）。"
                % (min(func_exports), max(verify_marks)),
            ))
    return problems


# ── KEYFP-1 ─────────────────────────────────────────────────────────────────
# 密钥指纹口径锚点 + 序列化完整性 + 记录点覆盖（真机 125 根因诊断，2026-11-13）
FINGERPRINT_DEF_RE = re.compile(r"fun\s+fingerprintOf\s*\(")
# InstallState 序列化 key（lead 补充 A：save/load 双向都要出现）
KEYFP_PREFS_KEY = '"key_fingerprint"'
# 记录点：markPatchedPendingFlash 调用处必须带 keyFingerprint 实参
MARKPATCHED_CALL_RE = re.compile(r"InstallState\.markPatchedPendingFlash\s*\(")


def check_key_fingerprint_anchors(path: str) -> list:
    """KEYFP-1：密钥指纹（KeyMatch 三方对账）的三条静态断言。

    ①口径锚点：SecretStore.kt 必须定义 fingerprintOf，且实现里对密钥做
      `.take(16)`——这是与 apd `version --json` key_prefix_sha256（cli.rs
      sha256_hex_stdin(key 前 16 字符)）逐字节对齐的唯一保证；口径漂移 =
      三方对账全部失真（125 诊断永远误报/漏报）。
    ②序列化完整性（lead 补充 A）：InstallState 的 save() 与 load() 都必须读写
      "key_fingerprint" prefs key——漏掉一侧 = 指纹静默丢失 = NO_RECORD 误判。
    ③记录点覆盖：PatchPipeline.kt 的 markPatchedPendingFlash 调用必须带
      keyFingerprint 实参——漏记 = 内嵌密钥指纹从未落盘，对账无从谈起。
    """
    problems = []
    lines = _read_lines(path)
    if lines is None:
        return problems
    fname = os.path.basename(path)
    text = "\n".join(lines)

    if fname == "SecretStore.kt":
        if not FINGERPRINT_DEF_RE.search(text):
            problems.append((0, "KEYFP-1：SecretStore.kt 缺少 fingerprintOf 定义——"
                                "KeyMatch 三方对账失去 App 侧指纹来源"))
        else:
            m = FINGERPRINT_DEF_RE.search(text)
            tail = text[m.start():m.start() + 600]
            if not re.search(r"\.take\(\s*16\s*\)", tail):
                problems.append((
                    text[:m.start()].count("\n") + 1,
                    "KEYFP-1：fingerprintOf 实现没有 .take(16)——口径必须与 apd "
                    "key_prefix_sha256（sha256(key 前 16 字符)）逐字节一致，"
                    "否则三方对账字符串永远不相等（125 诊断失效）",
                ))

    if fname == "InstallState.kt":
        if KEYFP_PREFS_KEY not in text:
            problems.append((0, "KEYFP-1：InstallState.kt 未持久化 \"key_fingerprint\"——"
                                "修补密钥指纹从不落盘，KeyMatch 只能永远 NO_RECORD"))
        else:
            # save() 与 load() 双向：各出现一次才算完整（lead 补充 A）
            save_zone = text[text.find("fun save("):text.find("fun markPatching(")]
            load_zone = text[text.find("fun load("):text.find("fun save(")]
            for side, zone in (("load()", load_zone), ("save()", save_zone)):
                if KEYFP_PREFS_KEY not in zone:
                    problems.append((
                        0,
                        "KEYFP-1：InstallState.%s 没有读写 \"key_fingerprint\"——"
                        "序列化单侧遗漏 = 指纹静默丢失（对账失效）" % side,
                    ))

    if fname == "PatchPipeline.kt":
        for m in MARKPATCHED_CALL_RE.finditer(text):
            # 调用体内（下一个平衡右括号前）必须出现 keyFingerprint = 实参
            call_start = m.start()
            i = text.find("(", call_start)
            depth = 0
            j = i
            while j < len(text):
                if text[j] == "(":
                    depth += 1
                elif text[j] == ")":
                    depth -= 1
                    if depth == 0:
                        break
                j += 1
            call_body = text[i:j]
            if "keyFingerprint" not in call_body:
                problems.append((
                    text[:call_start].count("\n") + 1,
                    "KEYFP-1：markPatchedPendingFlash 调用未传 keyFingerprint——"
                    "刷入镜像内嵌密钥的指纹不会落盘，root-exec 125 时无法做"
                    "「密钥已重置 vs 镜像刷写问题」的精确诊断",
                ))
    return problems


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(
        description="真机崩溃回归门禁（THREAD-1 Toast 跨线程 / KEYABI-1 密钥 ABI 48→47）"
    )
    ap.add_argument("--root", default=DEFAULT_ROOT, help="扫描根目录（默认 %s）" % DEFAULT_ROOT)
    ap.add_argument("--self-test", action="store_true", help="用合成夹具验证规则本身有效")
    args = ap.parse_args(argv)

    if args.self_test:
        return _self_test()

    if not os.path.isdir(args.root):
        print("::error::扫描目录不存在：%s" % args.root)
        return 1

    total = 0
    bad = 0
    for dirpath, _dirs, files in os.walk(args.root):
        for name in sorted(files):
            if not name.endswith(".kt"):
                continue
            total += 1
            path = os.path.join(dirpath, name)
            # 坏编码/读不了 → 明确 FAIL，不静默跳过（跳过 = 漏检 = 假绿）
            if _read_lines(path) is None:
                bad += 1
                print("%s:0: READ-1：无法按 UTF-8 读取（门禁无法判定，按失败处理）" % path)
                continue
            for line, msg in (
                check_thread_toast(path)
                + check_engine_key_abi(path)
                + check_busy_finally_coverage(path)
                + check_export_order_and_permission(path)
                + check_key_fingerprint_anchors(path)
            ):
                bad += 1
                print("%s:%d: %s" % (path, line, msg))

    print("check_device_crash_regressions: %d Kotlin file(s), %d problem(s)" % (total, bad))
    return 0 if bad == 0 else 1


# ───────────────────────────── 自检 ────────────────────────────────────────
_GOOD_THREAD = """\
package x
fun ok(ctx: android.content.Context) {
    Thread {
        val msg = "done"
        android.os.Handler(android.os.Looper.getMainLooper()).post {
            android.widget.Toast.makeText(ctx, msg, 0).show()
        }
    }.start()
}
"""

_BAD_THREAD = """\
package x
fun bad(ctx: android.content.Context) {
    Thread {
        android.widget.Toast.makeText(ctx, "done", 0).show()
    }.start()
}
"""

_GOOD_KEYABI = """\
package x
fun ok() {
    val k = PatchEngine.lastRootKey()?.let { SecretStore.normalizeEngineKey(it) } ?: "x"
}
"""

_BAD_KEYABI = """\
package x
fun bad() {
    val k = PatchEngine.lastRootKey()?.let { SecretStore.kernelKeyOf(it) } ?: "x"
}
"""

# KEYFP 夹具（3 断言 × 正/负）；文件名决定触发哪条断言（用 _run_keyfp_snippet）
_GOOD_FINGERPRINT = """\
package x
class SecretStore {
    companion object {
        fun fingerprintOf(key: String): String {
            val digest = java.security.MessageDigest.getInstance("SHA-256")
                .digest(key.take(16).toByteArray(Charsets.UTF_8))
            return digest.joinToString("") { "%02x".format(it) }
        }
    }
}
"""

_BAD_FINGERPRINT = """\
package x
class SecretStore {
    companion object {
        fun fingerprintOf(key: String): String {
            val digest = java.security.MessageDigest.getInstance("SHA-256")
                .digest(key.toByteArray(Charsets.UTF_8))
            return digest.joinToString("") { "%02x".format(it) }
        }
    }
}
"""

_GOOD_INSTALLSTATE_FP = """\
package x
object InstallState {
    fun load(context: Context): Snapshot {
        val prefs = context.getSharedPreferences("p", 0)
        return Snapshot(
            status = Status.UNPATCHED,
            keyFingerprint = prefs.getString("key_fingerprint", null),
        )
    }
    fun save(context: Context, snapshot: Snapshot) {
        context.getSharedPreferences("p", 0).edit()
            .putString("key_fingerprint", snapshot.keyFingerprint)
            .apply()
    }
    fun markPatching(context: Context) {}
}
"""

_BAD_INSTALLSTATE_FP = """\
package x
object InstallState {
    fun load(context: Context): Snapshot {
        val prefs = context.getSharedPreferences("p", 0)
        return Snapshot(status = Status.UNPATCHED)
    }
    fun save(context: Context, snapshot: Snapshot) {
        context.getSharedPreferences("p", 0).edit()
            .putString("note", snapshot.note)
            .apply()
    }
    fun markPatching(context: Context) {}
}
"""

_GOOD_MARKPATCHED_FP = """\
package x
object Pipeline {
    fun done(context: Context) {
        InstallState.markPatchedPendingFlash(
            context = context,
            artifactDisplayPath = "p",
            artifactSha256 = "s",
            artifactBytes = 1L,
            kernelEqualLength = true,
            keyFingerprint = SecretStore.fingerprintOf(rootKey),
        )
    }
}
"""

_BAD_MARKPATCHED_FP = """\
package x
object Pipeline {
    fun done(context: Context) {
        InstallState.markPatchedPendingFlash(
            context = context,
            artifactDisplayPath = "p",
            artifactSha256 = "s",
            artifactBytes = 1L,
            kernelEqualLength = true,
        )
    }
}
"""

def _run_keyfp_snippet(src: str, filename: str):
    import tempfile

    with tempfile.TemporaryDirectory() as d:
        p = os.path.join(d, filename)
        with open(p, "w", encoding="utf-8") as f:
            f.write(src)
        return check_key_fingerprint_anchors(p)

_GOOD_BUSY = """\
package x
object Pipeline {
    private val busy = java.util.concurrent.atomic.AtomicBoolean(false)
    fun execute() {
        if (!busy.compareAndSet(false, true)) return
        try {
            val key = runCatching { SecretStore.get() }.getOrElse {
                fail(); return
            }
            work(key)
        } finally {
            busy.set(false)
        }
    }
}
"""

_BAD_BUSY = """\
package x
object Pipeline {
    private val busy = java.util.concurrent.atomic.AtomicBoolean(false)
    fun execute() {
        if (!busy.compareAndSet(false, true)) return
        val key = runCatching { SecretStore.get() }.getOrElse {
            fail(); return
        }
        try {
            work(key)
        } finally {
            busy.set(false)
        }
    }
}
"""

# EXPORT 夹具：模拟 PatchPipeline 的形态（文件名不影响——检查走 path 参数），
# 这里只测 EXPORT-2（权限检查）与 EXPORT-1（顺序），用 _run_snippet_export(path) 控制文件名。
_GOOD_EXPORT_ORDER = '''\
package x
class Pipeline {
    fun execute() {
        verifyReadback()
        checkAvb()
        // 回读校验通过
        val a = Exporter.exportToDownloads(ctx, init, "i.img")
        val b = Exporter.exportToDownloads(ctx, boot, "b.img")
    }
}
'''

_BAD_EXPORT_ORDER = '''\
package x
class Pipeline {
    fun execute() {
        val b = Exporter.exportToDownloads(ctx, boot, "b.img")
        verifyReadback()
        checkAvb()
        // 回读校验通过
        val a = Exporter.exportToDownloads(ctx, init, "i.img")
    }
}
'''

_NO_PERM_EXPORT = '''\
package x
class Page {
    fun save() {
        Exporter.exportToDownloads(ctx, file, "log.txt")
    }
}
'''

_WITH_PERM_EXPORT = '''\
package x
class Page {
    fun save() {
        if (Exporter.needsLegacyStoragePermission(ctx)) return
        Exporter.exportToDownloads(ctx, file, "log.txt")
    }
}
'''


def _run_snippet(src: str):
    """把片段写成临时文件，跑三条规则，返回 (thread, keyabi, busy) 问题列表。"""
    import tempfile

    with tempfile.TemporaryDirectory() as d:
        p = os.path.join(d, "S.kt")
        with open(p, "w", encoding="utf-8") as f:
            f.write(src)
        return check_thread_toast(p), check_engine_key_abi(p), check_busy_finally_coverage(p)


def _run_export_snippet(src: str, filename: str):
    """EXPORT 规则夹具：允许指定文件名（EXPORT-1 只对 core/PatchPipeline.kt 生效）。"""
    import tempfile

    with tempfile.TemporaryDirectory() as d:
        sub = os.path.join(d, "core")
        os.makedirs(sub)
        p = os.path.join(sub, filename)
        with open(p, "w", encoding="utf-8") as f:
            f.write(src)
        return check_export_order_and_permission(p)


def _self_test() -> int:
    """负样本铁律：每条规则都必须在坏样本上 FAIL、在好样本上 PASS。

    任一坏样本没变红 → 说明该规则已退化成永远 PASS，自检本身判 FAIL。
    """
    cases = [
        ("THREAD-1 正样本：post 到主线程后 Toast", _GOOD_THREAD, "thread", False),
        ("THREAD-1 负样本：后台 Thread 直接 Toast", _BAD_THREAD, "thread", True),
        ("KEYABI-1 正样本：normalizeEngineKey 归一", _GOOD_KEYABI, "keyabi", False),
        ("KEYABI-1 负样本：lastRootKey 直接 kernelKeyOf", _BAD_KEYABI, "keyabi", True),
        ("BUSY-1 正样本：密钥获取在 try 内", _GOOD_BUSY, "busy", False),
        ("BUSY-1 负样本：try 前 return 跳过 finally", _BAD_BUSY, "busy", True),
    ]
    failures = 0
    print("== check_device_crash_regressions --self-test ==")
    for title, src, kind, expect_problem in cases:
        tp, kp, bp = _run_snippet(src)
        got = {"thread": bool(tp), "keyabi": bool(kp), "busy": bool(bp)}[kind]
        ok = got == expect_problem
        print("  %-46s 期望%s  实得%s  %s" % (
            title,
            "有问题" if expect_problem else "无问题",
            "有问题" if got else "无问题",
            "OK" if ok else "**FAIL**",
        ))
        if not ok:
            failures += 1
            print("     → 规则失效：%r 未被正确判定" % title)

    # EXPORT-1/2：需要控制文件名，走独立夹具
    export_cases = [
        ("EXPORT-1 正样本：导出在全部校验之后",
         _GOOD_EXPORT_ORDER, "PatchPipeline.kt", "order", False),
        ("EXPORT-1 负样本：导出先于校验（一半成功）",
         _BAD_EXPORT_ORDER, "PatchPipeline.kt", "order", True),
        ("EXPORT-2 正样本：调用前有 needsLegacyStoragePermission",
         _WITH_PERM_EXPORT, "SomePage.kt", "perm", False),
        ("EXPORT-2 负样本：导出无任何权限检查",
         _NO_PERM_EXPORT, "SomePage.kt", "perm", True),
    ]
    for title, src, fname, kind, expect_problem in export_cases:
        problems = _run_export_snippet(src, fname)
        if kind == "order":
            got = any("EXPORT-1" in m for _, m in problems)
        else:
            got = any("EXPORT-2" in m for _, m in problems)
        ok = got == expect_problem
        print("  %-46s 期望%s  实得%s  %s" % (
            title,
            "有问题" if expect_problem else "无问题",
            "有问题" if got else "无问题",
            "OK" if ok else "**FAIL**",
        ))
        if not ok:
            failures += 1
            print("     → 规则失效：%r 未被正确判定" % title)

    # KEYFP-1：3 断言 × 正/负（文件名决定触发哪条断言）
    keyfp_cases = [
        ("KEYFP-1 正样本：fingerprintOf 含 take(16)",
         _GOOD_FINGERPRINT, "SecretStore.kt", False),
        ("KEYFP-1 负样本：fingerprintOf 丢 take(16)（口径漂移）",
         _BAD_FINGERPRINT, "SecretStore.kt", True),
        ("KEYFP-1 正样本：save/load 双向读写 key_fingerprint",
         _GOOD_INSTALLSTATE_FP, "InstallState.kt", False),
        ("KEYFP-1 负样本：序列化遗漏（双向都没写）",
         _BAD_INSTALLSTATE_FP, "InstallState.kt", True),
        ("KEYFP-1 正样本：markPatchedPendingFlash 带 keyFingerprint",
         _GOOD_MARKPATCHED_FP, "PatchPipeline.kt", False),
        ("KEYFP-1 负样本：markPatchedPendingFlash 漏记指纹",
         _BAD_MARKPATCHED_FP, "PatchPipeline.kt", True),
    ]
    for title, src, fname, expect_problem in keyfp_cases:
        problems = _run_keyfp_snippet(src, fname)
        got = bool(problems)
        ok = got == expect_problem
        print("  %-46s 期望%s  实得%s  %s" % (
            title,
            "有问题" if expect_problem else "无问题",
            "有问题" if got else "无问题",
            "OK" if ok else "**FAIL**",
        ))
        if not ok:
            failures += 1
            print("     → 规则失效：%r 未被正确判定" % title)

    if failures:
        print("判定：SELF-TEST FAIL —— 有规则在负样本上没有变红（已退化为永远 PASS），禁止合入。")
        return 1
    print("判定：SELF-TEST PASS —— 六条规则均在坏样本上变红、在好样本上放行。")
    return 0


if __name__ == "__main__":
    sys.exit(main())
