package com.sproot.manager.core

import android.content.Context
import java.io.File
import java.util.concurrent.TimeUnit
import java.util.concurrent.locks.ReentrantLock

/**
 * AP 二进制客户端 —— 管理器**唯一**的 root 提权入口（契约 v1.1 §8）。
 *
 * ## 职责边界（lead 裁决：收敛到 AP）
 * Kotlin 侧只做三件事：
 *  1. 把 48 位密钥写入**私有目录**临时文件（0600），调用返回后立即删除；
 *  2. `apd root-exec --superkey-file <tmp> [--uid] [--scontext] [--timeout] -- <cmd...>` 起进程；
 *  3. **只看 exit code** 判成败（不解析人类文本），并把 stderr/stdout 原样落盘。
 *
 * 绝不做：execve 提权、JNI 提权、在 argv 或日志里出现密钥、`sh -c` 拼串。
 *
 * 退出码映射（§8.5）：
 *  - 0   成功
 *  - 1   一般失败 → 展示 stderr 摘要
 *  - 2   用法错误 → 视为 bug，展示原始 argv
 *  - 124 超时
 *  - 125 提权失败/未授权（静默）→ "密钥无效或需重新修补"，**不重试**
 *  - 126 命令不可执行 → 需重新部署
 *  - 127 命令不存在 → 需重新部署
 */
object ApdClient {

    private const val TAG = "Apd"

    const val EXIT_OK = 0
    const val EXIT_FAIL = 1
    const val EXIT_USAGE = 2
    const val EXIT_TIMEOUT = 124
    const val EXIT_ESCALATION = 125
    const val EXIT_NOT_EXECUTABLE = 126
    const val EXIT_NOT_FOUND = 127

    /** 管理器侧串行化 root 操作（§8.1-4：AP 不保证并发安全） */
    private val rootLock = ReentrantLock(true)

    data class Result(
        val exitCode: Int,
        val stdout: String,
        val stderr: String,
        val timedOut: Boolean,
        val argv: List<String>,
    ) {
        val ok: Boolean get() = exitCode == EXIT_OK && !timedOut

        /** 给 UI 的稳定解释；未知码也原样带上，不吞。 */
        fun describe(): String = when {
            timedOut -> "AP 调用超时（124）"
            exitCode == EXIT_OK -> "成功"
            exitCode == EXIT_FAIL -> "AP 业务失败（1）"
            exitCode == EXIT_USAGE -> "AP 用法错误（2）——调用参数有问题，属 bug"
            exitCode == EXIT_TIMEOUT -> "AP 内部超时（124）"
            exitCode == EXIT_ESCALATION -> "密钥无效或需重新修补（125，提权被内核静默拒绝；请勿自动重试）"
            exitCode == EXIT_NOT_EXECUTABLE -> "目标命令不可执行（126），需重新部署 AP"
            exitCode == EXIT_NOT_FOUND -> "目标命令不存在（127），需重新部署 AP"
            else -> "未知退出码（$exitCode）"
        }
    }

    /** apd 二进制位置：首装前用 APK 自带副本，部署后用 /data/adb/apd */
    fun apdBinary(context: Context): File {
        val deployed = File("/data/adb/apd")
        return if (deployed.isFile) deployed else File(context.applicationInfo.nativeLibraryDir, "libapd.so")
    }

    fun status(context: Context): CapabilityItem {
        val bin = apdBinary(context)
        val present = bin.isFile
        val aarch64 = present && Elf.isAarch64(bin.absolutePath)
        val exec = present && bin.canExecute()
        return CapabilityItem(
            key = "apd",
            label = "AP 守护二进制（root-exec 唯一入口）",
            ok = present && aarch64 && exec,
            detail = buildString {
                append("路径=${bin.absolutePath}；存在=$present；可执行=$exec；架构=")
                append(if (present) (Elf.readHeader(bin.absolutePath)?.machineName ?: "非 ELF") else "—")
                if (!present) append("\n首次使用前需完成部署（设置 → 部署 AP）")
            },
            // 骨架阶段不阻断修补（修补本身是用户态纯文件操作）；部署完成后再置 true
            blocking = false,
        )
    }

    /**
     * 唯一 root 调用入口。阻塞，必须在后台线程调用。
     *
     * @param context 用于解析未部署时的 APK 内副本与临时密钥文件目录
     * @param superKey 48 位密钥（内部截断逻辑由 AP 处理，App 原样写入临时文件）
     * @param command 目标命令 argv（**已经是数组**，不做任何字符串拼接）
     * @param uid 可选 --uid
     * @param sContext 可选 --scontext
     * @param timeoutSec 可选 --timeout（AP 侧超时，管理器另有外层超时）
     */
    fun rootExec(
        context: Context,
        superKey: String,
        command: List<String>,
        uid: Int? = null,
        sContext: String? = null,
        timeoutSec: Int? = null,
        outputLineLimit: Int = 2000,
    ): Result {
        require(command.isNotEmpty()) { "root-exec 需要目标命令" }
        return rootLock.withLockCompat {
            val bin = apdBinary(context)
            val keyFile = writeKeyFile(context, superKey)
            try {
                // --superkey-file 是 apd 顶层参数（clap）：必须放在子命令 root-exec 之前，
                // 否则 `apd root-exec --superkey-file ...` 会以 rc=2（unexpected argument）直接失败。
                val argv = mutableListOf(bin.absolutePath, "--superkey-file", keyFile.absolutePath, "root-exec")
                uid?.let { argv += listOf("--uid", it.toString()) }
                sContext?.let { argv += listOf("--scontext", it) }
                timeoutSec?.let { argv += listOf("--timeout", it.toString()) }
                argv += "--"
                argv += command

                // argv 脱敏后再记录日志：密钥只在临时文件里，argv 中只有路径
                AppLog.info(TAG, "root-exec: ${argv.joinToString(" ")}")

                val process = ProcessBuilder(argv)
                    .redirectErrorStream(false)
                    .start()
                val stdout = StringBuilder()
                val stderr = StringBuilder()
                val reader = Thread {
                    runCatching {
                        process.inputStream.bufferedReader().useLines { seq ->
                            seq.take(outputLineLimit).forEach { line ->
                                stdout.append(line).append('\n')
                                AppLog.debug(TAG, "stdout| $line")
                            }
                        }
                    }
                }.apply { isDaemon = true; start() }
                val errReader = Thread {
                    runCatching {
                        process.errorStream.bufferedReader().useLines { seq ->
                            seq.take(outputLineLimit).forEach { line ->
                                stderr.append(line).append('\n')
                                AppLog.warn(TAG, "stderr| $line")
                            }
                        }
                    }
                }.apply { isDaemon = true; start() }

                val outerTimeout = ((timeoutSec ?: 60) + 15) * 1000L
                val finished = runCatching { process.waitFor(outerTimeout, TimeUnit.MILLISECONDS) }
                    .getOrDefault(false)
                if (!finished) {
                    process.destroy()
                    runCatching { process.waitFor(2, TimeUnit.SECONDS) }
                    process.destroyForcibly()
                }
                runCatching { reader.join(2000) }
                runCatching { errReader.join(2000) }
                val exit = if (finished) runCatching { process.exitValue() }.getOrDefault(EXIT_FAIL)
                else EXIT_TIMEOUT
                if (exit != EXIT_OK) {
                    AppLog.error(TAG, "root-exec 失败：${describeExit(exit)}")
                }
                Result(
                    exitCode = exit,
                    stdout = stdout.toString(),
                    stderr = stderr.toString(),
                    timedOut = !finished,
                    argv = argv,
                )
            } finally {
                // 契约要求：调用返回后立即删除临时密钥文件
                if (!keyFile.delete()) {
                    AppLog.warn(TAG, "临时密钥文件删除失败：${keyFile.absolutePath}（下次启动清理）")
                }
            }
        }
    }

    /** 便捷：root-exec 包裹 `apd <subcommand...>` */
    fun apd(
        context: Context,
        superKey: String,
        subCommand: List<String>,
        uid: Int? = null,
        sContext: String? = null,
        timeoutSec: Int? = null,
    ): Result = rootExec(
        context = context,
        superKey = superKey,
        command = listOf(apdBinary(context).absolutePath) + subCommand,
        uid = uid,
        sContext = sContext,
        timeoutSec = timeoutSec,
    )

    /** APK 内置 apd 版本信息（assets/apd-version.txt，由 CI 从 artifact 一并打入） */
    fun bundledApdVersion(context: Context): Map<String, String> = runCatching {
        context.assets.open("apd-version.txt").bufferedReader().use { reader ->
            reader.readLines().mapNotNull { line ->
                val i = line.indexOf('=')
                if (i > 0) line.substring(0, i).trim() to line.substring(i + 1).trim() else null
            }.toMap()
        }
    }.getOrDefault(emptyMap())

    /** 设备上已部署 apd 的版本信息（`apd version --json`） */
    fun deployedApdVersion(context: Context): Map<String, String> {
        val result = versionProbe(context)
        return runCatching {
            listOf("version", "commit", "commit_short", "deployed")
                .mapNotNull { key -> MiniJson.string(result.stdout, key)?.let { key to it } }
                .toMap()
        }.getOrDefault(emptyMap())
    }

    /**
     * 版本一致性检查（sys-guard 契约要求）：
     * 内置 commit ≠ 已部署 commit 时**必须提示重新部署**，不允许静默用旧二进制跑 root-exec。
     * @return null 表示一致或无法判定（不阻断）；否则返回给人看的提示。
     */
    fun versionMismatchHint(context: Context): String? {
        val bundled = bundledApdVersion(context)["commit"]?.takeIf { it.isNotBlank() } ?: return null
        val deployed = deployedApdVersion(context)["commit"]?.takeIf { it.isNotBlank() } ?: return null
        if (bundled == deployed) return null
        return "版本不一致：内置 AP ${bundled.take(8)} / 已部署 ${deployed.take(8)} → 请重新部署（设置 → AP 部署）"
    }

    /** 直接 exec（非 root；仅用于只读探测，如 version/selftest） */
    fun runDirect(context: Context, args: List<String>, timeoutSec: Int = 30): Result {
        val bin = apdBinary(context)
        val argv = listOf(bin.absolutePath) + args
        if (!bin.isFile) {
            return Result(
                exitCode = EXIT_NOT_FOUND,
                stdout = "",
                stderr = "apd 未部署：${bin.absolutePath}",
                timedOut = false,
                argv = argv,
            )
        }
        return runCatching {
            AppLog.info(TAG, "direct: ${argv.joinToString(" ")}")
            val process = ProcessBuilder(argv).redirectErrorStream(false).start()
            val stdout = StringBuilder()
            val stderr = StringBuilder()
            val t1 = Thread {
                runCatching {
                    process.inputStream.bufferedReader().useLines { seq ->
                        seq.take(2000).forEach { line -> stdout.append(line).append('\n') }
                    }
                }
            }.apply { isDaemon = true; start() }
            val t2 = Thread {
                runCatching {
                    process.errorStream.bufferedReader().useLines { seq ->
                        seq.take(2000).forEach { line -> stderr.append(line).append('\n') }
                    }
                }
            }.apply { isDaemon = true; start() }
            val finished = process.waitFor(timeoutSec.toLong(), TimeUnit.SECONDS)
            if (!finished) {
                process.destroy()
                runCatching { process.waitFor(2, TimeUnit.SECONDS) }
                process.destroyForcibly()
            }
            runCatching { t1.join(2000) }
            runCatching { t2.join(2000) }
            Result(
                exitCode = if (finished) runCatching { process.exitValue() }.getOrDefault(EXIT_FAIL) else EXIT_TIMEOUT,
                stdout = stdout.toString(),
                stderr = stderr.toString(),
                timedOut = !finished,
                argv = argv,
            )
        }.getOrElse {
            Result(EXIT_FAIL, "", it.stackTraceToString(), false, argv)
        }
    }

    /** 非 root 探测：`apd version --json`（§8.3，任意身份） */    fun versionProbe(context: Context): Result {
        val bin = apdBinary(context)
        if (!bin.isFile) {
            return Result(
                exitCode = EXIT_NOT_FOUND,
                stdout = "",
                stderr = "apd 未部署：${bin.absolutePath}",
                timedOut = false,
                argv = listOf(bin.absolutePath, "version", "--json"),
            )
        }
        val argv = listOf(bin.absolutePath, "version", "--json")
        return runCatching {
            val process = ProcessBuilder(argv).redirectErrorStream(true).start()
            val out = process.inputStream.bufferedReader().readText()
            val finished = process.waitFor(10, TimeUnit.SECONDS)
            Result(
                exitCode = if (finished) process.exitValue() else EXIT_TIMEOUT,
                stdout = out,
                stderr = "",
                timedOut = !finished,
                argv = argv,
            )
        }.getOrElse {
            Result(EXIT_FAIL, "", it.stackTraceToString(), false, argv)
        }
    }

    private fun writeKeyFile(context: Context, superKey: String): File {
        require(SecretStore.isValidKey(superKey)) { "密钥必须是 48 位 [A-Za-z0-9]" }
        val dir = File(context.filesDir, "exec-keys").apply { mkdirs() }
        val file = File(dir, "key-${System.nanoTime()}.tmp")
        file.writeText(superKey)
        // 0600：仅本 uid 可读（AP 以同 uid 启动，可读）
        val applied = file.setReadable(false, false) &&
            file.setWritable(false, false) &&
            file.setExecutable(false, false)
        file.setReadable(true, true)
        file.setWritable(true, true)
        if (!applied) AppLog.warn(TAG, "设置密钥文件权限失败（尽力而为）：${file.absolutePath}")
        return file
    }

    private fun describeExit(code: Int): String = when (code) {
        EXIT_OK -> "OK"
        EXIT_FAIL -> "一般失败(1)"
        EXIT_USAGE -> "用法错误(2)"
        EXIT_TIMEOUT -> "超时(124)"
        EXIT_ESCALATION -> "提权失败(125)"
        EXIT_NOT_EXECUTABLE -> "不可执行(126)"
        EXIT_NOT_FOUND -> "命令不存在(127)"
        else -> "exit=$code"
    }

    private inline fun <T> ReentrantLock.withLockCompat(block: () -> T): T {
        lock()
        try {
            return block()
        } finally {
            unlock()
        }
    }
}
