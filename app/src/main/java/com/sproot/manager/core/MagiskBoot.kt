package com.sproot.manager.core

import android.content.Context
import java.io.File
import java.util.concurrent.TimeUnit

/**
 * magiskboot 执行封装（boot.img 解包/重打包、内核解压/回压）。
 *
 * 安全与可靠性约束：
 *  - 二进制以 `libmagiskboot.so` 随 APK 分发，位于 **nativeLibraryDir**，
 *    天然可执行、不受 filesDir noexec / SELinux W^X 限制（旧版在 filesDir 里 exec 是缺陷 ⑨）。
 *  - 调用走 `ProcessBuilder(cmd = [二进制, arg1, arg2...])`，**不经 sh -c、不拼字符串**（缺陷 ⑩），
 *    因此不存在注入与"引号被吃掉"的问题。
 *  - stdout+stderr 合并后**逐行实时**回调并落盘；进程超时被强杀时明确报告 timeout，
 *    绝不把"部分输出"当成功。
 */
object MagiskBoot {

    const val LIB_FILE_NAME = "libmagiskboot.so"

    data class BinaryStatus(
        val path: String,
        val present: Boolean,
        val executable: Boolean,
        val aarch64: Boolean,
        val elfMachine: String,
        val detail: String,
    )

    data class ExecResult(
        val exitCode: Int,
        val timedOut: Boolean,
        val lines: List<String>,
    )

    fun binary(context: Context): File =
        File(context.applicationInfo.nativeLibraryDir, LIB_FILE_NAME)

    fun status(context: Context): BinaryStatus {
        val file = binary(context)
        val present = file.isFile
        val header = if (present) Elf.readHeader(file.absolutePath) else null
        val aarch64 = header?.let { it.isElf && it.is64Bit && it.machine == Elf.EM_AARCH64 } == true
        val executable = present && file.canExecute()
        val detail = when {
            !present -> "APK 内缺少 arm64-v8a/$LIB_FILE_NAME（构建时未打入 jniLibs）"
            header == null || !header.isElf -> "文件存在但不是 ELF（可能被压缩/占位）"
            !aarch64 -> "不是 AArch64 ELF（${header.machineName}），无法在本机执行"
            !executable -> "ELF 存在但缺少执行权限（nativeLibraryDir 异常）"
            else -> "就绪：${file.absolutePath}"
        }
        return BinaryStatus(
            path = file.absolutePath,
            present = present,
            executable = executable,
            aarch64 = aarch64,
            elfMachine = header?.machineName ?: "非 ELF",
            detail = detail,
        )
    }

    /**
     * 执行 magiskboot 子命令（阻塞）。
     * @param onLine 每抓到一行输出回调（调用方负责落盘）；也可从 [ExecResult.lines] 拿全量。
     */
    fun exec(
        context: Context,
        args: List<String>,
        workDir: File,
        timeoutMs: Long = 5 * 60 * 1000L,
        onLine: (String) -> Unit = {},
    ): ExecResult {
        val bin = binary(context)
        require(bin.isFile) { "magiskboot 不存在：${bin.absolutePath}" }
        require(workDir.isDirectory) { "工作目录不存在：${workDir.absolutePath}" }

        val command = listOf(bin.absolutePath) + args
        AppLog.info("MagiskBoot", "exec: ${command.joinToString(" ")}  (cwd=${workDir.absolutePath})")

        val process = ProcessBuilder(command)
            .directory(workDir)
            .redirectErrorStream(true)
            .start()

        val lines = java.util.Collections.synchronizedList(mutableListOf<String>())
        val readerThread = Thread {
            runCatching {
                process.inputStream.bufferedReader().useLines { seq ->
                    seq.forEach { raw ->
                        val line = raw.trimEnd()
                        lines.add(line)
                        onLine(line)
                    }
                }
            }.onFailure { AppLog.warn("MagiskBoot", "读取输出中断：${it.message}") }
        }.apply {
            name = "magiskboot-reader"
            isDaemon = true
            start()
        }

        val finished = runCatching { process.waitFor(timeoutMs, TimeUnit.MILLISECONDS) }
            .getOrDefault(false)
        val timedOut = !finished
        if (timedOut) {
            AppLog.error("MagiskBoot", "命令超时 ${timeoutMs}ms，强制终止：${command.joinToString(" ")}")
            process.destroy()
            runCatching { process.waitFor(2, TimeUnit.SECONDS) }
            process.destroyForcibly()
        }
        runCatching { readerThread.join(3000) }

        val exit = runCatching { process.exitValue() }.getOrDefault(if (timedOut) -9 else -1)
        AppLog.info("MagiskBoot", "exit=$exit timedOut=$timedOut 输出行数=${lines.size}")
        return ExecResult(exit, timedOut, lines.toList())
    }

    fun version(context: Context): String? {
        val status = status(context)
        if (!status.present || !status.aarch64) return null
        val tmp = File(context.cacheDir, "mb-version").apply { mkdirs() }
        val r = runCatching { exec(context, listOf("--help"), tmp, 8000L) }.getOrNull() ?: return null
        return r.lines.firstOrNull()?.takeIf { it.isNotBlank() }
    }
}
