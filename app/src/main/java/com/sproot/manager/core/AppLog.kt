package com.sproot.manager.core

import android.content.Context
import android.util.Log
import androidx.compose.runtime.mutableStateOf
import java.io.BufferedWriter
import java.io.File
import java.io.FileWriter
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * 全局日志。
 *
 * 关键点（对应旧版缺陷 ⑤）：日志**先落盘再生效**——每条日志同步追加到
 * `filesDir/logs/session-<ts>.log` 并 flush，即使进程被杀、UI 被卸载，
 * 用户也能在"设置 → 打开日志目录"里找到完整记录。
 *
 * 截图/复读能力由 UI 层提供，本类只保证「不丢一行」。
 */
object AppLog {

    private const val TAG = "SPRoot"
    private const val MAX_MEMORY_ENTRIES = 4000

    private val lock = Any()
    private var sessionFile: File? = null
    private var writer: BufferedWriter? = null
    private val timeFormat = SimpleDateFormat("HH:mm:ss.SSS", Locale.US)
    private val fileTimeFormat = SimpleDateFormat("yyyy-MM-dd HH:mm:ss.SSS", Locale.US)

    /** Compose 可观察列表；整体替换以保证跨线程写入的可见性。 */
    val entries = mutableStateOf<List<LogEntry>>(emptyList())

    fun init(context: Context) {
        synchronized(lock) {
            if (writer != null) return
            val dir = File(context.filesDir, "logs")
            if (!dir.exists()) dir.mkdirs()
            val stamp = SimpleDateFormat("yyyyMMdd-HHmmss", Locale.US).format(Date())
            val file = File(dir, "session-$stamp.log")
            runCatching {
                val w = BufferedWriter(FileWriter(file, true))
                w.write("# SPRoot Manager session log  file=${file.absolutePath}\n")
                w.write("# package=${context.packageName}  code=${context.packageManager.getPackageInfo(context.packageName, 0).versionName}\n")
                w.write("# device=${android.os.Build.MANUFACTURER} ${android.os.Build.MODEL} android=${android.os.Build.VERSION.RELEASE} abi=${android.os.Build.SUPPORTED_ABIS.joinToString(",")}\n")
                w.flush()
                writer = w
                sessionFile = file
            }.onFailure {
                Log.e(TAG, "open log file failed: ${it.message}", it)
            }
        }
    }

    fun currentLogFile(): File? = sessionFile

    fun debug(tag: String, message: String) = write(LogLevel.DEBUG, tag, message)
    fun info(tag: String, message: String) = write(LogLevel.INFO, tag, message)
    fun warn(tag: String, message: String) = write(LogLevel.WARN, tag, message)
    fun error(tag: String, message: String, throwable: Throwable? = null) {
        val full = if (throwable == null) message else message + "\n" + throwable.stackTraceToString()
        write(LogLevel.ERROR, tag, full)
    }

    fun write(level: LogLevel, tag: String, message: String) {
        val now = System.currentTimeMillis()
        val entry = LogEntry(now, level, tag, message)
        synchronized(lock) {
            val merged = entries.value + entry
            entries.value = if (merged.size > MAX_MEMORY_ENTRIES) merged.takeLast(MAX_MEMORY_ENTRIES) else merged
            val w = writer
            if (w != null) {
                runCatching {
                    // 多行消息逐行缩进写入，保证文件可被 grep
                    val prefix = "${fileTimeFormat.format(Date(now))} ${level.name.padEnd(5)} [$tag] "
                    message.lineSequence().forEachIndexed { index, line ->
                        w.write(if (index == 0) prefix + line else " ".repeat(prefix.length) + line)
                        w.newLine()
                    }
                    w.flush()
                }
            }
        }
        when (level) {
            LogLevel.DEBUG -> Log.d(TAG, "[$tag] $message")
            LogLevel.INFO -> Log.i(TAG, "[$tag] $message")
            LogLevel.WARN -> Log.w(TAG, "[$tag] $message")
            LogLevel.ERROR -> Log.e(TAG, "[$tag] $message")
        }
    }

    fun clearMemory() {
        entries.value = emptyList()
    }

    /** 当前会话日志文本（用于复制到剪贴板/分享） */
    fun dumpText(): String = synchronized(lock) {
        val mm = SimpleDateFormat("HH:mm:ss.SSS", Locale.US)
        entries.value.joinToString("\n") { e -> "${mm.format(Date(e.timeMs))} ${e.level.name} [${e.tag}] ${e.message}" }
    }
}
