package com.sproot.manager.core

import android.content.Context
import java.io.File
import java.security.MessageDigest
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * 修补前自动备份原镜像（内容寻址：filenames = sha1）。
 *
 * 借鉴 KSU 的 sha1 内容寻址备份思路：
 *  - 同一份 boot.img 反复修补只存一份；
 *  - 索引文件 `backups/index.tsv` 记录 时间/sha1/大小/原始文件名；
 *  - 设置页提供"还原"入口：把备份复制回 Downloads/SPRoot/ 供 fastboot 刷回。
 *
 * 备份目录：`filesDir/backups/`（应用私有，卸载即消失；但 boot.img 本来就在固件里，
 * 这里只是"免再去翻固件包"的便利副本）。
 */
object BackupStore {

    private const val TAG = "Backup"

    data class Backup(
        val sha1: String,
        val file: File,
        val size: Long,
        val createdAt: Long,
        val sourceName: String,
    )

    private fun dir(context: Context): File = File(context.filesDir, "backups").apply { mkdirs() }

    private fun indexFile(context: Context): File = File(dir(context), "index.tsv")

    /** 备份源文件；若同 sha1 已存在则直接复用。 */
    fun backup(context: Context, source: File, sourceName: String?): Backup {
        val sha1 = digest(source, "SHA-1")
        val target = File(dir(context), "$sha1.img")
        val reused = target.isFile && target.length() == source.length()
        if (!reused) {
            val tmp = File(target.absolutePath + ".partial")
            source.copyTo(tmp, overwrite = true)
            if (!tmp.renameTo(target)) {
                tmp.copyTo(target, overwrite = true)
                tmp.delete()
            }
        }
        val now = System.currentTimeMillis()
        if (!reused) {
            indexFile(context).appendText(
                listOf(
                    now.toString(),
                    sha1,
                    source.length().toString(),
                    (sourceName ?: "boot.img").replace('\t', ' ').replace('\n', ' '),
                ).joinToString("\t") + "\n",
            )
        }
        AppLog.info(
            TAG,
            if (reused) "复用已有备份 sha1=$sha1" else "已备份原镜像 sha1=$sha1（${Capabilities.formatBytes(source.length())}）",
        )
        return Backup(sha1, target, target.length(), now, sourceName ?: "boot.img")
    }

    fun list(context: Context): List<Backup> {
        val index = indexFile(context)
        if (!index.isFile) return emptyList()
        val result = mutableListOf<Backup>()
        index.readLines().forEach { line ->
            val parts = line.split('\t')
            if (parts.size >= 4) {
                val created = parts[0].toLongOrNull() ?: 0L
                val sha1 = parts[1]
                val size = parts[2].toLongOrNull() ?: 0L
                val name = parts[3]
                val file = File(dir(context), "$sha1.img")
                if (file.isFile) result += Backup(sha1, file, size, created, name)
            }
        }
        // 后出现的覆盖先出现的（同一 sha1 只留最新记录）
        return result.distinctBy { it.sha1 }.sortedByDescending { it.createdAt }
    }

    /** 还原：把备份复制到 Downloads/SPRoot/boot_restore_<时间戳>.img */
    fun restoreToDownloads(context: Context, backup: Backup): Exporter.Exported {
        val stamp = SimpleDateFormat("yyyyMMdd-HHmmss", Locale.US).format(Date())
        val name = "boot_restore_${backup.sha1.take(8)}-$stamp.img"
        AppLog.warn(TAG, "还原镜像到 Downloads：sha1=${backup.sha1} size=${backup.size}")
        return Exporter.exportToDownloads(context, backup.file, name)
    }

    fun delete(context: Context, backup: Backup): Boolean {
        val removed = backup.file.delete()
        AppLog.info(TAG, "删除备份 sha1=${backup.sha1} result=$removed")
        return removed
    }

    fun digest(file: File, algorithm: String): String {
        val md = MessageDigest.getInstance(algorithm)
        file.inputStream().use { stream ->
            val buffer = ByteArray(1 shl 16)
            while (true) {
                val read = stream.read(buffer)
                if (read <= 0) break
                md.update(buffer, 0, read)
            }
        }
        return md.digest().joinToString("") { "%02x".format(it) }
    }
}
