package com.sproot.manager.core

import android.content.Context
import android.os.Build
import android.os.StatFs
import java.io.File

/**
 * 入口能力自检（缺陷 ①：每次修补前必须跑，绝不"先跑了再说"）。
 *
 * 设计要点：
 *  - 每条检查都要给出 **可读的 detail**（含绝对路径、实际值），失败时直接展示给用户；
 *  - blocking=true 的条目只要一条不通过就**不进入修补**，而不是让引擎/进程在后面莫名其妙失败；
 *  - 结果同时写入日志，便于事后复盘。
 */
object Capabilities {

    private const val TAG = "Capabilities"

    /** 最小可用存储要求（覆盖 boot.img + 解包 + 修补产物，留 2 倍余量） */
    private const val MIN_FREE_BYTES = 3L * 1024L * 1024L * 1024L

    fun collect(context: Context, inputFile: File? = null): List<CapabilityItem> {
        val items = mutableListOf<CapabilityItem>()

        // 1. ABI
        val abis = Build.SUPPORTED_ABIS
        val arm64 = abis.any { it == "arm64-v8a" }
        items += CapabilityItem(
            key = "abi",
            label = "设备 ABI（需 arm64-v8a）",
            ok = arm64,
            detail = "SUPPORTED_ABIS=${abis.joinToString(", ")}" + if (arm64) "" else "（本 App 只交付 arm64 产物）",
        )

        // 2. 修补引擎
        val engineFile = PatchEngine.libraryFile(context)
        val engineHeader = if (engineFile.isFile) Elf.readHeader(engineFile.absolutePath) else null
        val engineIsAarch64 = engineHeader?.let { it.isElf && it.is64Bit && it.machine == Elf.EM_AARCH64 } == true
        val engineLoadError = PatchEngine.ensureLoaded()
        val engineVersion = if (engineLoadError == null) runCatching { PatchEngine.nativeVersion() }.getOrNull() else null
        items += CapabilityItem(
            key = "engine",
            label = "SK 修补引擎 libskrootpatch.so",
            ok = engineFile.isFile && engineIsAarch64 && engineLoadError == null,
            detail = buildString {
                append("路径=${engineFile.absolutePath}")
                append("；存在=${engineFile.isFile}")
                append("；架构=${engineHeader?.machineName ?: "非 ELF"}")
                append("；加载=")
                append(if (engineLoadError == null) "成功" else "失败")
                engineVersion?.let { append("；版本=$it") }
                engineLoadError?.let { append("\n").append(it) }
            },
        )

        // 3. magiskboot
        val mb = MagiskBoot.status(context)
        items += CapabilityItem(
            key = "magiskboot",
            label = "magiskboot（boot.img 解包/重打包）",
            ok = mb.present && mb.aarch64 && mb.executable,
            detail = mb.detail,
        )

        // 4. 存储空间
        val stat = runCatching { StatFs(context.cacheDir.absolutePath) }.getOrNull()
        val free = stat?.availableBytes ?: 0L
        items += CapabilityItem(
            key = "storage",
            label = "可用存储（≥3 GB）",
            ok = free >= MIN_FREE_BYTES,
            detail = "cacheDir 可用 ${formatBytes(free)}",
        )

        // 5. Root 密钥可用（写入真实记录，确保 release 也能取到）
        val keyOk = runCatching { SecretStore.get(context).getOrCreateRootKey() }.isSuccess
        items += CapabilityItem(
            key = "root_key",
            label = "Root 密钥（48 位，Keystore 加密存储）",
            ok = keyOk,
            detail = if (keyOk) "已就绪（debug/release 一致，不依赖 BuildConfig.DEBUG）" else "读写失败，见日志",
        )

        // 6. 输入镜像（若已选择）
        if (inputFile != null) {
            val exist = inputFile.isFile && inputFile.length() > 0
            val header = if (exist) Elf.readHeader(inputFile.absolutePath) else null
            val bootMagic = exist && Elf.isAndroidBootImage(inputFile)
            items += CapabilityItem(
                key = "input",
                label = "待修补镜像",
                ok = exist && bootMagic,
                detail = buildString {
                    append("路径=${inputFile.absolutePath}；大小=${formatBytes(inputFile.length())}")
                    when {
                        !exist -> append("；不存在或为空")
                        bootMagic -> append("；识别为 Android boot image")
                        header?.isElf == true -> append("；看起来是 ELF 内核而非 boot.img（请选择 boot.img，不是 kernel 本体）")
                        else -> append("；未识别出 ANDROID! 魔数，可能不是标准 boot.img（A-only/GKI 变体）")
                    }
                },
            )
        }

        items.forEach { item ->
            if (item.ok) AppLog.debug(TAG, "自检通过 ${item.label}: ${item.detail}")
            else AppLog.error(TAG, "自检失败 ${item.label}: ${item.detail}")
        }
        return items
    }

    fun blockingFailures(items: List<CapabilityItem>): List<CapabilityItem> =
        items.filter { it.blocking && !it.ok }

    fun formatBytes(bytes: Long): String {
        if (bytes < 1024) return "$bytes B"
        val kb = bytes / 1024.0
        if (kb < 1024) return String.format(java.util.Locale.US, "%.1f KiB", kb)
        val mb = kb / 1024.0
        if (mb < 1024) return String.format(java.util.Locale.US, "%.1f MiB", mb)
        return String.format(java.util.Locale.US, "%.2f GiB", mb / 1024.0)
    }
}
