package com.sproot.manager.core

import android.content.ContentValues
import android.content.Context
import android.os.Build
import android.os.Environment
import android.provider.MediaStore
import java.io.File

/**
 * 产物导出：写到用户可访问的 Downloads/SPRoot/。
 *
 * 为什么不用 getExternalFilesDir：刷入需要用户 / PC 能找到文件；
 * app 私有外部目录在 Android 11+ 只能通过 SAF 访问，PC fastboot 场景取不到。
 *
 * Android 10+ 走 MediaStore（无需存储权限）；Android 9 及以下回退到公共 Downloads 目录
 * （需要 WRITE_EXTERNAL_STORAGE，manifest 中已按 maxSdkVersion=28 声明）。
 */
object Exporter {

    private const val SUB_DIR = "SPRoot"

    data class Exported(val displayPath: String, val absolutePath: String, val bytes: Long)

    fun exportToDownloads(context: Context, source: File, displayName: String): Exported {
        require(source.isFile) { "待导出文件不存在：${source.absolutePath}" }
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
            exportViaMediaStore(context, source, displayName)
        } else {
            exportLegacy(source, displayName)
        }
    }

    private fun exportViaMediaStore(context: Context, source: File, displayName: String): Exported {
        val resolver = context.contentResolver
        val values = ContentValues().apply {
            put(MediaStore.MediaColumns.DISPLAY_NAME, displayName)
            put(MediaStore.MediaColumns.MIME_TYPE, "application/octet-stream")
            put(MediaStore.MediaColumns.RELATIVE_PATH, "${Environment.DIRECTORY_DOWNLOADS}/$SUB_DIR")
            put(MediaStore.MediaColumns.IS_PENDING, 1)
        }
        val uri = resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values)
            ?: throw IllegalStateException("MediaStore 拒绝创建 Downloads/$SUB_DIR/$displayName（可能是系统限制）")
        try {
            resolver.openOutputStream(uri)?.use { out ->
                source.inputStream().use { input -> input.copyTo(out) }
            } ?: throw IllegalStateException("无法打开 MediaStore 输出流：$uri")
            resolver.update(uri, ContentValues().apply { put(MediaStore.MediaColumns.IS_PENDING, 0) }, null, null)
            val path = "${Environment.DIRECTORY_DOWNLOADS}/$SUB_DIR/$displayName"
            AppLog.info("Export", "产物已导出到 $path")
            return Exported(path, uri.toString(), source.length())
        } catch (t: Throwable) {
            runCatching { resolver.delete(uri, null, null) }
            throw t
        }
    }

    private fun exportLegacy(source: File, displayName: String): Exported {
        @Suppress("DEPRECATION")
        val downloads = Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS)
        val dir = File(downloads, SUB_DIR).apply { mkdirs() }
        val target = File(dir, displayName)
        source.copyTo(target, overwrite = true)
        AppLog.info("Export", "产物已导出到 ${target.absolutePath}")
        return Exported(target.absolutePath, target.absolutePath, target.length())
    }
}
