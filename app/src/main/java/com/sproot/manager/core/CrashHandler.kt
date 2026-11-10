package com.sproot.manager.core

import android.content.Context
import android.os.Build
import android.os.Looper
import android.util.Log
import com.sproot.manager.BuildConfig
import java.io.File
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * 全局崩溃兜底（人类真机反馈 ①，2026-11-10）。
 *
 * 目的：**下次崩了能自己拿到证据**，不用等人类抓 logcat。
 * - 任何未捕获异常：写 `crash-<时间>.txt`（机型/版本/线程/完整堆栈/最近 200 行 AppLog），
 *   并尽力复制一份到 `Downloads/SPRoot/`；私有目录 `filesDir/crash/` 保底。
 * - **后台线程**的未捕获异常只记录、不闪退（能降级就降级；初始化路径都在后台线程）；
 *   主线程异常写证据后交回系统默认处理（主线程栈已坏，继续跑不安全）。
 * - 本对象自身全程 runCatching，绝不在崩溃处理里再抛异常。
 */
object CrashHandler {

    private const val TAG = "Crash"
    private const val DIR = "crash"

    @Volatile
    private var installed = false

    fun install(context: Context) {
        if (installed) return
        val app = context.applicationContext
        val previous = Thread.getDefaultUncaughtExceptionHandler()
        Thread.setDefaultUncaughtExceptionHandler { thread, error ->
            val report = runCatching { writeReport(app, thread, error) }.getOrNull()
            runCatching {
                AppLog.error(TAG, "未捕获异常（thread=${thread.name}）证据=${report?.absolutePath ?: "写入失败"}", error)
            }
            val isMain = thread === Looper.getMainLooper().thread
            if (!isMain) {
                // 后台线程：记录后吞掉，避免"首次初始化异常 → 整个 App 闪退"
                return@setDefaultUncaughtExceptionHandler
            }
            previous?.uncaughtException(thread, error)
        }
        installed = true
        AppLog.info(TAG, "崩溃兜底已安装（后台线程异常只记录不闪退；证据写入 filesDir/crash 并尽力导出 Downloads/SPRoot）")
    }

    /** 已保存的崩溃报告（设置页可导出/查看用；最新在前）。 */
    fun reports(context: Context): List<File> =
        File(context.filesDir, DIR).listFiles()
            ?.filter { it.isFile }
            ?.sortedByDescending { it.lastModified() }
            ?: emptyList()

    private fun writeReport(app: Context, thread: Thread, error: Throwable): File {
        val stamp = SimpleDateFormat("yyyyMMdd-HHmmss-SSS", Locale.US).format(Date())
        val text = buildString {
            append("SPRoot Manager 崩溃报告\n")
            append("时间: ").append(SimpleDateFormat("yyyy-MM-dd HH:mm:ss.SSS", Locale.US).format(Date())).append('\n')
            append("线程: ").append(thread.name).append('\n')
            append("版本: ").append(BuildConfig.VERSION_NAME).append(" (").append(BuildConfig.BUILD_TYPE).append(")\n")
            append("机型: ").append(Build.MANUFACTURER).append(' ').append(Build.MODEL).append('\n')
            append("Android: ").append(Build.VERSION.RELEASE).append(" (API ").append(Build.VERSION.SDK_INT).append(")\n")
            append("ABI: ").append(Build.SUPPORTED_ABIS.joinToString(",")).append('\n')
            append("\n--- 堆栈 ---\n")
            append(Log.getStackTraceString(error))
            append("\n--- 最近日志（最多 200 行）---\n")
            append(
                runCatching {
                    AppLog.currentLogFile()?.readLines()?.takeLast(200)?.joinToString("\n")
                }.getOrNull() ?: "（AppLog 不可用）",
            )
        }
        val dir = File(app.filesDir, DIR).apply { mkdirs() }
        val local = File(dir, "crash-$stamp.txt").apply { writeText(text) }
        // 尽力导出到公共 Downloads/SPRoot（MediaStore 失败则仅保留私有目录）
        runCatching { Exporter.exportToDownloads(app, local, "crash-$stamp.txt") }
        return local
    }
}
