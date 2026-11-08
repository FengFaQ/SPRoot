package com.sproot.manager.core

import android.content.Context
import android.net.Uri
import androidx.compose.runtime.State
import androidx.compose.runtime.mutableStateOf
import java.io.File
import java.io.RandomAccessFile
import java.security.MessageDigest
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import java.util.concurrent.atomic.AtomicBoolean

/**
 * 手机本地修补流水线（SPROOT-DOC-C1 的"调用方"职责）。
 *
 * ```
 * 选 boot.img
 *   → ①环境自检（ABI / 引擎 / magiskboot / 存储 / 密钥 / 输入魔数）
 *   → ②复制到 cache 工作目录 + sha1 内容寻址备份
 *   → ③magiskboot unpack → kernel
 *   → ④kernel 若仍压缩 → magiskboot decompress → kernel.raw
 *   → ⑤PatchEngine.patch(kernel.raw → kernel.patched)  ← 48 位密钥在此注入（JNI，无需 root）
 *   → ⑥kernel.patched 若原为压缩 → magiskboot compress=<fmt>
 *   → ⑦magiskboot repack → new-boot.img
 *   → ⑧校验（产物存在 / 内核等长 / sha256 / 与输入不同）→ 导出 Downloads/SPRoot/
 *   → ⑨状态机置「已修补·待刷入」（用户确认刷入后才进「已刷入·待重启」）
 * ```
 *
 * 设计铁律（逐条对应旧版失败根因规避清单）：
 *  ① 每步入口先自检，缺件立刻报「缺什么/在哪/怎么修」，绝不静默跳过；
 *  ③ 全程离线：不下载任何东西，所需二进制必须随 APK 分发；
 *  ⑤ 失败即写日志文件 + UI 错误区常驻（error* 只有用户点重置才清）；
 *  ⑥ 不生成也不执行任何 shell 脚本；
 *  ⑦ 成功必须验证产物存在 + 修补内核等长 + sha256 记录（并记入状态机）；
 *  ⑨ 不 exec filesDir 里的 ELF（magiskboot 在 nativeLibraryDir）；
 *  ⑩ 全部 ProcessBuilder 参数数组调用，无 sh -c 拼接。
 *
 * 本流水线**不需要 root**：只读写自己的私有目录与 Downloads。
 * 特权操作（部署/模块/授权/刷入）走 [ApdClient] 的 root-exec，两条链不混用。
 */
object PatchPipeline {

    private const val TAG = "Pipeline"

    private val state = mutableStateOf(PatchUiState())
    val uiState: State<PatchUiState> = state

    private val busy = AtomicBoolean(false)

    /** 仅用于失败时更新状态机（Application context，不会泄漏 Activity） */
    @Volatile
    private var appContext: Context? = null

    /** 阶段耗时统计（结构化日志：阶段 + 耗时 + 结果） */
    private var stageStartedAt = 0L
    private val stageDurations = mutableListOf<Pair<String, Long>>()

    // ------------------------------------------------------------------ 交互
    fun selectMode(mode: PatchMode) {
        if (state.value.running) return
        if (!mode.enabled) {
            AppLog.warn(TAG, "模式「${mode.label}」尚未开放：${mode.description}")
            return
        }
        state.value = state.value.copy(mode = mode)
        AppLog.info(TAG, "修补模式：${mode.label}")
    }

    fun selectInput(uri: Uri?, displayName: String?) {
        val current = state.value
        if (current.running) {
            AppLog.warn(TAG, "修补进行中，忽略重新选择镜像的请求")
            return
        }
        state.value = current.copy(
            sourceUri = uri?.toString(),
            sourceName = displayName,
            stage = PatchStage.IDLE,
            progress = 0f,
            phase = "",
        )
        AppLog.info(TAG, "已选择镜像：${displayName ?: "<未命名>"} uri=$uri")
    }

    /** 重置为初始态（唯一允许清空错误信息的入口） */
    fun reset() {
        if (state.value.running) {
            AppLog.warn(TAG, "修补进行中，禁止重置")
            return
        }
        val mode = state.value.mode
        state.value = PatchUiState(mode = mode)
        stageDurations.clear()
        AppLog.info(TAG, "修补页状态已重置")
    }

    // ------------------------------------------------------------------ 主流程
    fun start(context: Context) {
        val current = state.value
        if (current.running || !busy.compareAndSet(false, true)) {
            AppLog.warn(TAG, "已有修补任务在运行，忽略重复点击")
            return
        }
        val uriString = current.sourceUri
        if (uriString.isNullOrBlank()) {
            state.value = current.copy(
                stage = PatchStage.FAILED,
                errorTitle = "尚未选择镜像",
                errorDetail = "请先点击「选择 boot.img」并选中你手机的 boot 分区镜像（不是 kernel 本体、不是压缩包）。",
            )
            busy.set(false)
            return
        }

        val appContext = context.applicationContext
        AppLog.init(appContext)
        val logFile = AppLog.currentLogFile()

        state.value = current.copy(
            running = true,
            stage = PatchStage.PREFLIGHT,
            progress = 0f,
            phase = "准备中",
            startedAt = System.currentTimeMillis(),
            finishedAt = 0L,
            outputPath = null,
            outputSha256 = null,
            outputBytes = 0L,
            kernelBytes = 0L,
            kernelEqualLength = false,
            backupSha1 = null,
            reportPath = null,
            rootKey = null,
            errorCode = null,
            errorTitle = null,
            errorDetail = null,
            logPath = logFile?.absolutePath,
            mode = current.mode,
        )
        stageDurations.clear()
        InstallState.markPatching(appContext)

        Thread({ execute(appContext, Uri.parse(uriString)) }, "sproot-patch").start()
    }

    private fun execute(context: Context, sourceUri: Uri) {
        var workDir: File? = null
        val key = runCatching { SecretStore.get(context).getOrCreateRootKey() }.getOrElse {
            finishFailure(
                title = "无法取得 Root 密钥",
                detail = "读写本地加密密钥失败：${it.javaClass.simpleName}: ${it.message}\n" +
                    "请到「设置 → Root 密钥」重新生成后再试。",
                code = null,
            )
            return
        }

        try {
            // ---------- ① 环境自检 ----------
            stage(PatchStage.PREFLIGHT, 2f, "检查 ABI / 引擎 / magiskboot / 存储")
            val caps = Capabilities.collect(context)
            val blocking = Capabilities.blockingFailures(caps)
            if (blocking.isNotEmpty()) {
                finishFailure(
                    title = "环境自检未通过（${blocking.size} 项）",
                    detail = blocking.joinToString("\n\n") { "■ ${it.label}\n${it.detail}" },
                    code = null,
                )
                return
            }
            stage(PatchStage.PREFLIGHT, 5f, "环境自检通过")

            // ---------- ② 准备镜像 + 备份 ----------
            stage(PatchStage.COPY_INPUT, 8f, "复制镜像到应用私有工作目录")
            workDir = newWorkDir(context)
            state.value = state.value.copy(workDir = workDir.absolutePath)
            val bootImg = File(workDir, "boot.img")
            val copied = copyUriToFile(context, sourceUri, bootImg)
            AppLog.info(TAG, "输入镜像已落地：${bootImg.absolutePath}（${Capabilities.formatBytes(copied)}）")
            if (copied <= 0L) {
                finishFailure("输入镜像读取失败", "从 URI 读到的字节数为 0。请确认文件可读（不是被其他 App 独占的在线文件）。", null)
                return
            }
            val inputSha = sha256(bootImg)
            AppLog.info(TAG, "输入 boot.img sha256=$inputSha")

            val backup = runCatching { BackupStore.backup(context, bootImg, state.value.sourceName) }
                .onFailure { AppLog.error(TAG, "备份原镜像失败（不阻断修补）", it) }
                .getOrNull()
            state.value = state.value.copy(backupSha1 = backup?.sha1)
            AppLog.info(TAG, "修补前备份：${backup?.sha1 ?: "<失败>"}")

            val inputCap = Capabilities.collect(context, bootImg).last()
            if (!inputCap.ok) {
                finishFailure("镜像校验未通过", inputCap.detail, null)
                return
            }

            // ---------- ③ 解包 ----------
            stage(PatchStage.UNPACK, 12f, "magiskboot unpack")
            val unpack = MagiskBoot.exec(context, listOf("unpack", "boot.img"), workDir!!) { line ->
                AppLog.debug("magiskboot", line)
            }
            if (unpack.timedOut) {
                finishFailure("解包超时", "magiskboot unpack 超过 5 分钟未结束，已强制终止。", null)
                return
            }
            if (unpack.exitCode != 0) {
                finishFailure(
                    "解包失败（magiskboot exit=${unpack.exitCode}）",
                    buildString {
                        append("命令：magiskboot unpack boot.img\n工作目录：${workDir!!.absolutePath}\n")
                        append("完整输出：\n")
                        append(unpack.lines.joinToString("\n").ifBlank { "<无输出>" })
                        append("\n\n提示：常见原因是镜像不是标准 boot.img（可能是 vendor_boot / init_boot），")
                        append("或该机型 boot 头格式不被 magiskboot 支持。")
                    },
                    null,
                )
                return
            }
            val kernel = File(workDir, "kernel")
            if (!kernel.isFile || kernel.length() == 0L) {
                val produced = workDir!!.listFiles()?.joinToString(", ") { it.name } ?: "<空目录>"
                finishFailure(
                    "解包后没有 kernel",
                    "magiskboot unpack 退出码=0，但工作目录缺少 kernel 文件。\n实际产出：$produced\n" +
                        "可能该 boot.img 是 init_boot 或采用非标准布局。",
                    null,
                )
                return
            }
            stage(PatchStage.UNPACK, 25f, "解包完成：kernel ${Capabilities.formatBytes(kernel.length())}")

            // ---------- ③.5 备份原始 kernel（magiskboot repack 用 kernel 作模板） ----------
            // ⚠️ 关键坑（patcher 实测）：repack 只认工作目录里的 `kernel`，若把修补结果写到
            // kernel.patched 而不覆盖 kernel，repack 会**静默**使用原始 kernel（产物看着正常，
            // 实际未修补）。因此这里显式备份 kernel.orig，最后必须覆盖 kernel，并在 ⑥.5 回读断言。
            val kernelOrig = File(workDir, "kernel.orig")
            kernel.copyTo(kernelOrig, overwrite = true)
            val origSha = sha256(kernelOrig)
            AppLog.info(TAG, "kernel.orig 备份完成 sha256=$origSha（repack 模板基准）")

            // ---------- ④ 解压内核 ----------
            val compression = Elf.compressionFormat(kernelOrig)
            val kernelRaw: File
            if (compression != null) {
                stage(PatchStage.DECOMPRESS, 30f, "内核仍为 $compression 压缩，调用 magiskboot decompress")
                val dec = MagiskBoot.exec(context, listOf("decompress", "kernel.orig", "kernel.raw"), workDir!!) { line ->
                    AppLog.debug("magiskboot", line)
                }
                if (dec.timedOut || dec.exitCode != 0) {
                    finishFailure(
                        "内核解压失败（magiskboot exit=${dec.exitCode}）",
                        "格式识别为 $compression。\n完整输出：\n${dec.lines.joinToString("\n").ifBlank { "<无输出>" }}",
                        null,
                    )
                    return
                }
                kernelRaw = File(workDir, "kernel.raw")
                if (!kernelRaw.isFile || kernelRaw.length() == 0L) {
                    finishFailure("内核解压产物缺失", "magiskboot decompress 未生成 kernel.raw。", null)
                    return
                }
            } else {
                stage(PatchStage.DECOMPRESS, 30f, "内核未压缩，直接使用")
                kernelRaw = File(workDir, "kernel.raw")
                kernelOrig.copyTo(kernelRaw, overwrite = true)
            }
            val rawSize = kernelRaw.length()
            val rawSha = sha256(kernelRaw)
            state.value = state.value.copy(kernelBytes = rawSize)
            AppLog.info(TAG, "待修补内核：${kernelRaw.name} size=$rawSize sha256=$rawSha")

            // ---------- ⑤ 修补内核（引擎 JNI） ----------
            stage(PatchStage.PATCH_KERNEL, 35f, "调用 SK 修补引擎（JNI）")
            val patched = File(workDir, "kernel.patched")
            val jsonFile = File(workDir, "progress.ndjson")
            val watcher = NdjsonProgressWatcher(jsonFile) { evt -> onEngineEvent(evt) }
            watcher.start()
            val exit = try {
                PatchEngine.patch(
                    inFile = kernelRaw,
                    outFile = patched,
                    key = key,
                    genKey = false,
                    jsonFile = jsonFile,
                    workDir = workDir!!,
                )
            } finally {
                watcher.stopAndJoin()
            }
            if (exit != EngineExitCode.OK) {
                val lastError = PatchEngine.lastError()
                finishFailure(
                    "修补引擎失败",
                    buildString {
                        append(EngineExitCode.describe(exit))
                        if (!lastError.isNullOrBlank()) append("\n\n引擎最后错误：\n").append(lastError)
                        append("\n\n工作目录（保留供诊断）：").append(workDir!!.absolutePath)
                        append("\n进度事件：").append(watcher.lastLine ?: "<引擎未写进度>")
                    },
                    "E${exit}",
                )
                return
            }
            if (!patched.isFile || patched.length() == 0L) {
                finishFailure(
                    "引擎返回成功但没有产物",
                    "退出码=0，但 ${patched.absolutePath} 不存在或为空。这违反 C1 §3 铁律，请上报。",
                    "E_INTERNAL",
                )
                return
            }
            if (patched.length() != rawSize) {
                finishFailure(
                    "修补产物长度异常（违反 C1 等长铁律）",
                    "输入 $rawSize 字节，输出 ${patched.length()} 字节。SK 修补必须是纯字节 patch、长度不变；" +
                        "该产物禁止刷入，请上报日志。",
                    "E_INTERNAL",
                )
                return
            }
            val patchedSha = sha256(patched)
            if (patchedSha == rawSha) {
                finishFailure(
                    "修补产物与输入完全一致",
                    "sha256 相同，说明引擎没有实际改写内核（可能命中已修补内核或引擎空转）。",
                    "E_INTERNAL",
                )
                return
            }
            state.value = state.value.copy(kernelEqualLength = true)
            stage(PatchStage.PATCH_KERNEL, 72f, "修补完成：等长 $rawSize 字节，sha256 已变化")
            AppLog.info(TAG, "修补后内核 sha256=$patchedSha（等长校验通过）")

            // ---------- ⑥ 覆盖 kernel（关键：repack 只认 kernel） ----------
            if (compression != null) {
                stage(PatchStage.RECOMPRESS, 76f, "回压内核（$compression）并覆盖 kernel")
                val recompress = MagiskBoot.exec(
                    context,
                    listOf("compress=$compression", "kernel.patched", "kernel"),
                    workDir!!,
                ) { line -> AppLog.debug("magiskboot", line) }
                if (recompress.timedOut || recompress.exitCode != 0) {
                    finishFailure(
                        "内核回压失败（magiskboot exit=${recompress.exitCode}）",
                        "格式=$compression\n完整输出：\n${recompress.lines.joinToString("\n").ifBlank { "<无输出>" }}",
                        null,
                    )
                    return
                }
            } else {
                stage(PatchStage.RECOMPRESS, 76f, "覆盖 kernel ← kernel.patched")
                patched.copyTo(kernel, overwrite = true)
            }
            val overwrittenSha = sha256(kernel)
            AppLog.info(
                TAG,
                "已覆盖 kernel：sha256=$overwrittenSha（kernel.orig=$origSha，kernel.patched=$patchedSha）",
            )
            if (overwrittenSha == origSha) {
                finishFailure(
                    "补丁未生效（T-12）",
                    "覆盖后的 kernel 与原 kernel 的 sha256 相同（$origSha）。\n" +
                        "可能原因：回压/覆盖步骤没有真正写入。禁止导出该产物。",
                    "T-12",
                )
                return
            }

            // ---------- ⑦ 重打包 ----------
            stage(PatchStage.REPACK, 82f, "magiskboot repack")
            val repack = MagiskBoot.exec(context, listOf("repack", "boot.img"), workDir!!) { line ->
                AppLog.debug("magiskboot", line)
            }
            if (repack.timedOut || repack.exitCode != 0) {
                finishFailure(
                    "重打包失败（magiskboot exit=${repack.exitCode}）",
                    "完整输出：\n${repack.lines.joinToString("\n").ifBlank { "<无输出>" }}",
                    null,
                )
                return
            }
            val newBoot = File(workDir, "new-boot.img")
            if (!newBoot.isFile || newBoot.length() == 0L) {
                finishFailure(
                    "重打包产物缺失",
                    "magiskboot repack 成功但未生成 new-boot.img。目录内容：\n" +
                        (workDir!!.listFiles()?.joinToString("\n") { it.name } ?: "<空>"),
                    null,
                )
                return
            }

            // ---------- ⑦.5 回读断言（T-12：patch 必须在最终 boot.img 里真实生效） ----------
            stage(PatchStage.VERIFY, 88f, "回读 new-boot.img 校验补丁是否真实生效")
            val verifyDir = File(workDir, "verify").apply {
                check(mkdirs() || isDirectory) { "无法创建校验目录：${absolutePath}" }
            }
            newBoot.copyTo(File(verifyDir, "new-boot.img"), overwrite = true)
            val verifyUnpack = MagiskBoot.exec(context, listOf("unpack", "new-boot.img"), verifyDir) { line ->
                AppLog.debug("magiskboot", line)
            }
            if (verifyUnpack.timedOut || verifyUnpack.exitCode != 0) {
                finishFailure(
                    "回读校验失败：无法 unpack 产物（magiskboot exit=${verifyUnpack.exitCode}）",
                    "完整输出：\n${verifyUnpack.lines.joinToString("\n").ifBlank { "<无输出>" }}",
                    "T-12",
                )
                return
            }
            val verifyKernel = File(verifyDir, "kernel")
            if (!verifyKernel.isFile || verifyKernel.length() == 0L) {
                finishFailure(
                    "回读校验失败：产物里没有 kernel",
                    "unpack 产物目录：\n" + (verifyDir.listFiles()?.joinToString("\n") { it.name } ?: "<空>"),
                    "T-12",
                )
                return
            }
            val verifyRaw: File
            if (compression != null) {
                val dec2 = MagiskBoot.exec(context, listOf("decompress", "kernel", "kernel.check"), verifyDir) { line ->
                    AppLog.debug("magiskboot", line)
                }
                if (dec2.timedOut || dec2.exitCode != 0) {
                    finishFailure(
                        "回读校验失败：产物 kernel 解压失败（magiskboot exit=${dec2.exitCode}）",
                        "完整输出：\n${dec2.lines.joinToString("\n").ifBlank { "<无输出>" }}",
                        "T-12",
                    )
                    return
                }
                verifyRaw = File(verifyDir, "kernel.check")
            } else {
                verifyRaw = verifyKernel
            }
            val verifySha = sha256(verifyRaw)
            val verifyMatchesPatched = verifySha == patchedSha
            val verifyDiffersFromOrig = verifySha != rawSha
            AppLog.info(
                TAG,
                "回读校验：new-boot.img 内 kernel sha256=$verifySha；" +
                    "kernel.patched=$patchedSha；kernel.orig(raw)=$rawSha；" +
                    "匹配修补=$verifyMatchesPatched；不同于原始=$verifyDiffersFromOrig",
            )
            if (!verifyMatchesPatched || !verifyDiffersFromOrig) {
                finishFailure(
                    "补丁未生效（T-12）：产物里的 kernel 不是修补后的 kernel",
                    buildString {
                        append("这是最隐蔽的失败模式：magiskboot repack 会静默使用原始 kernel。\n\n")
                        append("回读 kernel sha256 = $verifySha\n")
                        append("期望（kernel.patched）= $patchedSha\n")
                        append("原始（kernel.orig/raw）= $rawSha\n")
                        append("匹配修补 = $verifyMatchesPatched；不同于原始 = $verifyDiffersFromOrig\n\n")
                        append("已中止，未导出任何产物。工作目录：").append(workDir!!.absolutePath)
                    },
                    "T-12",
                )
                return
            }
            val avbOrig = Elf.hasAvbFooter(bootImg)
            val avbNew = Elf.hasAvbFooter(newBoot)
            AppLog.info(TAG, "AVB footer：原始=$avbOrig 产物=$avbNew（不修改 AVB，仅确认保留）")
            if (avbOrig != avbNew) {
                AppLog.warn(TAG, "AVB footer 存在性变化（原始=$avbOrig 产物=$avbNew），请质测关注")
            }
            if (newBoot.length() != bootImg.length()) {
                AppLog.warn(
                    TAG,
                    "产物总长与输入不同（${newBoot.length()} vs ${bootImg.length()} 字节）；" +
                        "若分区大小固定，刷入前需确认未超分区（magiskboot repack 可能重排对齐）",
                )
            }
            stage(PatchStage.VERIFY, 92f, "回读校验通过：补丁已在产物中真实生效")
            val outSha = sha256(newBoot)
            val stamp = SimpleDateFormat("yyyyMMdd-HHmmss", Locale.US).format(Date())
            val exportName = "boot_patched_$stamp.img"
            val exported = Exporter.exportToDownloads(context, newBoot, exportName)
            AppLog.info(
                TAG,
                "产物就绪：${exported.displayPath} size=${newBoot.length()} sha256=$outSha",
            )

            val rootKey = PatchEngine.lastRootKey() ?: key
            state.value = state.value.copy(
                stage = PatchStage.DONE,
                progress = 1f,
                phase = "修补完成，产物已导出",
                running = false,
                outputPath = exported.displayPath,
                outputSha256 = outSha,
                outputBytes = newBoot.length(),
                kernelEqualLength = true,
                reportPath = File(workDir, "kernel.patched.report.json")
                    .takeIf { it.isFile }?.absolutePath,
                rootKey = rootKey,
                finishedAt = System.currentTimeMillis(),
            )
            InstallState.markPatchedPendingFlash(
                context = context,
                artifactDisplayPath = exported.displayPath,
                artifactSha256 = outSha,
                artifactBytes = newBoot.length(),
                kernelEqualLength = true,
            )
            AppLog.info(TAG, "==== 修补成功 ==== 产物=${exported.displayPath} sha256=$outSha")
            logStageDurations()
        } catch (t: Throwable) {
            finishFailure(
                "修补异常中断",
                "阶段=${state.value.stage.title} 进度=${(state.value.progress * 100).toInt()}%\n" +
                    "${t.javaClass.name}: ${t.message}\n\n${t.stackTraceToString()}",
                null,
            )
        } finally {
            busy.set(false)
            if (workDir != null) {
                val failed = state.value.stage == PatchStage.FAILED
                if (!failed) {
                    AppLog.info(TAG, "清理工作目录：${workDir.absolutePath}")
                    runCatching { workDir.deleteRecursively() }
                } else {
                    AppLog.warn(TAG, "失败现场保留：${workDir.absolutePath}（供诊断，可在设置页清理）")
                }
            }
            state.value = state.value.copy(
                logPath = AppLog.currentLogFile()?.absolutePath,
                finishedAt = if (state.value.finishedAt == 0L) System.currentTimeMillis() else state.value.finishedAt,
            )
        }
    }

    // ------------------------------------------------------------ 引擎进度事件
    private fun onEngineEvent(evt: String) {
        val event = MiniJson.string(evt, "event") ?: return
        when (event) {
            "progress" -> {
                val phase = MiniJson.string(evt, "phase") ?: ""
                val pct = MiniJson.int(evt, "pct") ?: 0
                val detail = MiniJson.string(evt, "detail") ?: ""
                // 引擎 0..100 映射到流水线 35..72 区间
                val mapped = 35f + (pct.coerceIn(0, 100) / 100f) * 37f
                state.value = state.value.copy(
                    stage = PatchStage.PATCH_KERNEL,
                    progress = mapped / 100f,
                    phase = "引擎：$phase（$pct%）${if (detail.isNotBlank()) " · $detail" else ""}",
                )
                AppLog.debug("Engine", "progress phase=$phase pct=$pct detail=$detail")
            }
            "result" -> {
                val code = MiniJson.int(evt, "code") ?: -1
                val error = MiniJson.string(evt, "error")
                val key = MiniJson.string(evt, "root_key")
                AppLog.info("Engine", "result code=$code error=${error ?: "<无>"} keyLen=${key?.length ?: 0}")
            }
            else -> AppLog.debug("Engine", "event=$evt")
        }
    }

    // --------------------------------------------------------------- 工具方法
    private fun stage(stage: PatchStage, progressPercent: Float, phase: String) {
        // 记录上一阶段耗时（结构化日志：阶段 + 耗时 + 结果）
        if (stageStartedAt > 0L) {
            val elapsed = System.currentTimeMillis() - stageStartedAt
            stageDurations += (state.value.stage.title to elapsed)
            AppLog.info(TAG, "[${state.value.stage.title}] 完成，耗时 ${elapsed}ms")
        }
        stageStartedAt = System.currentTimeMillis()
        state.value = state.value.copy(
            stage = stage,
            progress = (progressPercent / 100f).coerceIn(0f, 1f),
            phase = phase,
        )
        AppLog.info(TAG, "[${stage.title}] $phase（${progressPercent.toInt()}%）")
    }

    private fun logStageDurations() {
        if (stageDurations.isEmpty()) return
        val total = stageDurations.sumOf { it.second }
        AppLog.info(
            TAG,
            "阶段耗时汇总（总 ${total}ms）：" + stageDurations.joinToString("；") { "${it.first} ${it.second}ms" },
        )
    }

    private fun finishFailure(title: String, detail: String, code: String?) {
        state.value = state.value.copy(
            stage = PatchStage.FAILED,
            running = false,
            errorTitle = title,
            errorDetail = detail,
            errorCode = code,
            phase = title,
            logPath = AppLog.currentLogFile()?.absolutePath,
            finishedAt = System.currentTimeMillis(),
        )
        AppLog.error(TAG, "修补失败：$title\n$detail")
        logStageDurations()
        runCatching {
            appContext?.let { InstallState.markError(it, title) }
        }
    }

    private fun newWorkDir(context: Context): File {
        val stamp = SimpleDateFormat("yyyyMMdd-HHmmss", Locale.US).format(Date())
        val dir = File(File(context.cacheDir, "patch"), "job-$stamp")
        check(dir.mkdirs() || dir.isDirectory) { "无法创建工作目录：${dir.absolutePath}" }
        return dir
    }

    private fun copyUriToFile(context: Context, uri: Uri, target: File): Long {
        val input = context.contentResolver.openInputStream(uri)
            ?: throw IllegalStateException("contentResolver 无法打开 $uri（可能无读取权限）")
        input.use { stream ->
            target.outputStream().use { out -> stream.copyTo(out) }
        }
        return target.length()
    }

    fun sha256(file: File): String {
        val digest = MessageDigest.getInstance("SHA-256")
        file.inputStream().use { stream ->
            val buffer = ByteArray(1 shl 16)
            while (true) {
                val read = stream.read(buffer)
                if (read <= 0) break
                digest.update(buffer, 0, read)
            }
        }
        return digest.digest().joinToString("") { "%02x".format(it) }
    }

    /**
     * 轮询引擎写出的 NDJSON 进度文件。
     *
     * 为什么轮询文件而不是读 stdout：引擎是 JNI 进程内调用，stdout 无法可靠捕获；
     * C1 契约本来就规定 `--json <path>` 落盘，因此文件是**唯一稳定的进度通道**。
     */
    private class NdjsonProgressWatcher(
        private val file: File,
        private val onEvent: (String) -> Unit,
    ) {
        private val running = AtomicBoolean(true)
        private var thread: Thread? = null

        @Volatile
        var lastLine: String? = null
            private set

        fun start() {
            thread = Thread({
                var position = 0L
                while (running.get()) {
                    if (file.isFile && file.length() > position) {
                        runCatching {
                            RandomAccessFile(file, "r").use { raf ->
                                raf.seek(position)
                                while (true) {
                                    val line = raf.readLine() ?: break
                                    if (line.isNotBlank()) {
                                        lastLine = line
                                        onEvent(line)
                                    }
                                }
                                position = raf.filePointer
                            }
                        }.onFailure { AppLog.warn("Progress", "读取进度失败：${it.message}") }
                    }
                    Thread.sleep(150)
                }
            }, "engine-progress").apply {
                isDaemon = true
                start()
            }
        }

        fun stopAndJoin() {
            running.set(false)
            runCatching { thread?.join(1500) }
            // 收尾：把剩余内容读完
            if (file.isFile) {
                runCatching {
                    file.readLines().filter { it.isNotBlank() }.forEach { line ->
                        if (line != lastLine) onEvent(line)
                    }
                }
            }
        }
    }
}
