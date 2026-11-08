package com.sproot.manager.core

import android.content.Context
import java.io.File

/**
 * D1 · RcInject：向 ramdisk 注入 AP 守护的开机自启 rc。
 *
 * ## 目标与优先级（D-12）
 * - **init_boot.img 的 ramdisk 优先，boot.img 回退**；两者都无 ramdisk → 跳过并记
 *   `rc_injected=false + reason=no_ramdisk`，**不算失败**。
 *
 * ## 注入内容
 * 1. 新增 `init.sproot.rc`（service 定义，启动 `/data/adb/apd daemon`）；
 * 2. 在 `init.rc` 末尾确保恰好一条 `import /init.sproot.rc`（**幂等**：已存在则不重复追加）。
 *
 * ## 实现方式（全程 magiskboot cpio，无 shell 拼接）
 * ```
 * magiskboot cpio ramdisk.cpio "exists init.rc"          # 探测
 * magiskboot cpio ramdisk.cpio "backup ramdisk.cpio.orig" ?   # 参见 Rom 备份
 * magiskboot cpio ramdisk.cpio "extract init.rc init.rc"   # 取出到 cwd
 * magiskboot cpio ramdisk.cpio "add 0750 init.rc init.rc"  # 写回
 * magiskboot cpio ramdisk.cpio "add 0644 init.sproot.rc init.sproot.rc"
 * ```
 * 注：`magiskboot cpio` 的备份/恢复命令在不同版本略有差异，这里采用**我们自己复制
 * ramdisk.cpio 文件**做回滚备份（更可控），不依赖 cpio 子命令语义。
 */
object RcInject {

    private const val TAG = "RcInject"
    private const val RCFILE = "init.sproot.rc"
    private const val IMPORT_LINE = "import /init.sproot.rc"

    data class Result(
        val injected: Boolean,
        val target: String,
        val reason: String,
        val idempotentHit: Boolean,
        val source: String,
    )

    /** 权威模板在仓库中的单一来源（CI 构建时复制到 APK assets） */
    const val TEMPLATE_ASSET = "init.sproot.rc"
    const val TEMPLATE_SOURCE = "apd/assets/init.sproot.rc"

    /**
     * 读取权威 rc 模板。**不自行改写内容**（模板由 lead/sys-guard 维护）；
     * readonly 校验：必须含 service 定义与 boot_completed 触发器，否则视为模板异常。
     * @return (内容, 来源描述)
     */
    fun readTemplate(context: Context): Pair<String, String> {
        val fromAsset = runCatching {
            context.assets.open(TEMPLATE_ASSET).bufferedReader().use { it.readText() }
        }.getOrNull()
        if (fromAsset.isNullOrBlank()) {
            AppLog.error(TAG, "APK 内缺少 assets/$TEMPLATE_ASSET（CI 应从 $TEMPLATE_SOURCE 复制），自启将降级为内置最小模板")
            return fallbackTemplate() to "builtin-fallback（assets/$TEMPLATE_ASSET 缺失）"
        }
        val missing = when {
            !fromAsset.contains("service sproot-apd") -> "service sproot-apd"
            !fromAsset.contains("sys.boot_completed") -> "property:sys.boot_completed"
            !fromAsset.contains("oneshot") -> "oneshot"
            !fromAsset.contains("disabled") -> "disabled"
            else -> null
        }
        if (missing != null) {
            AppLog.error(TAG, "rc 模板缺少关键声明确认失败：$missing（仍按原文注入，请核对模板）")
        }
        return fromAsset to TEMPLATE_SOURCE
    }

    /** 兜底模板（仅在 APK 缺 assets 时使用；保持与权威模板同样的安全设计） */
    private fun fallbackTemplate(): String = """
        # SPRoot AP daemon — builtin fallback（权威模板缺失，需修 CI 打包）
        on post-fs-data
            mkdir /data/adb/sproot 0700 root root
        service sproot-apd /data/adb/apd daemon --superkey-file /data/adb/sproot/superkey
            class late_start
            user root
            group root
            oneshot
            disabled
            override
        on property:sys.boot_completed=1
            start sproot-apd
    """.trimIndent() + "\n"

    /**
     * 尝试注入。
     *
     * @param workDir magiskboot 的工作目录（repack 前）
     * @param ramdisk ramdisk.cpio 文件（存在才注入）
     * @param target "init_boot" 或 "boot"
     */
    fun inject(
        context: Context,
        workDir: File,
        ramdisk: File?,
        target: String,
    ): Result {
        if (ramdisk == null || !ramdisk.isFile || ramdisk.length() == 0L) {
            AppLog.warn(TAG, "无可注入 ramdisk（target=$target）→ 跳过自启注入，不视为失败")
            return Result(false, target, "no_ramdisk", false, TEMPLATE_SOURCE)
        }
        val (templateContent, templateSource) = readTemplate(context)

        // 回滚备份：ramdisk.cpio → ramdisk.cpio.orig（只备份一次，便于回滚后再次注入幂等）
        val backup = File(workDir, "ramdisk.cpio.orig")
        if (!backup.isFile) {
            ramdisk.copyTo(backup, overwrite = true)
            AppLog.info(TAG, "ramdisk 回滚备份：${backup.name}（${ramdisk.length()} B）")
        }

        // 写本地 rc 文件（magiskboot add 从 cwd 读）：**模板原文，不做改写**
        val rcLocal = File(workDir, RCFILE)
        rcLocal.writeText(templateContent)
        AppLog.info(TAG, "rc 模板来源=$templateSource，字节数=${templateContent.toByteArray().size}")

        // 1) ramdisk 里是否存在 init.rc
        val exists = MagiskBoot.exec(
            context,
            listOf("cpio", ramdisk.name, "exists init.rc"),
            workDir,
            timeoutMs = 60_000,
        ) { AppLog.debug("cpio", it) }
        val hasInitRc = exists.exitCode == 0

        if (!hasInitRc) {
            AppLog.warn(TAG, "ramdisk 无 init.rc（target=$target）→ 跳过，不视为失败")
            return Result(false, target, "no_init_rc", false, templateSource)
        }

        // 2) 取出 init.rc 到 cwd（magiskboot extract 只写到当前工作目录的文件名）
        val extract = MagiskBoot.exec(
            context,
            listOf("cpio", ramdisk.name, "extract init.rc init.rc"),
            workDir,
            timeoutMs = 60_000,
        ) { AppLog.debug("cpio", it) }
        val initRc = File(workDir, "init.rc")
        if (extract.exitCode != 0 || !initRc.isFile) {
            AppLog.error(TAG, "extract init.rc 失败 exit=${extract.exitCode}（保留原 ramdisk，不自启）")
            return Result(false, target, "extract_failed", false, templateSource)
        }

        // 3) 幂等：已 import 则只补 service 文件，不重复追加
        val text = initRc.readText()
        val alreadyImported = text.lineSequence().any { it.trim() == IMPORT_LINE }
        if (!alreadyImported) {
            val newText = if (text.endsWith("\n")) text + IMPORT_LINE + "\n" else text + "\n" + IMPORT_LINE + "\n"
            initRc.writeText(newText)
            AppLog.info(TAG, "init.rc 追加 \"$IMPORT_LINE\"")
        } else {
            AppLog.info(TAG, "init.rc 已含 \"$IMPORT_LINE\"（幂等，不重复追加）")
        }

        // 4) 写回：init.rc + init.sproot.rc
        val addInit = MagiskBoot.exec(
            context,
            listOf("cpio", ramdisk.name, "add 0750 init.rc init.rc"),
            workDir,
            timeoutMs = 60_000,
        ) { AppLog.debug("cpio", it) }
        if (addInit.exitCode != 0) {
            AppLog.error(TAG, "add init.rc 失败 exit=${addInit.exitCode}")
            return Result(false, target, "add_init_rc_failed", false, templateSource)
        }
        val addRc = MagiskBoot.exec(
            context,
            listOf("cpio", ramdisk.name, "add 0644 $RCFILE $RCFILE"),
            workDir,
            timeoutMs = 60_000,
        ) { AppLog.debug("cpio", it) }
        if (addRc.exitCode != 0) {
            AppLog.error(TAG, "add $RCFILE 失败 exit=${addRc.exitCode}")
            return Result(false, target, "add_rc_failed", false, templateSource)
        }

        val verify = MagiskBoot.exec(
            context,
            listOf("cpio", ramdisk.name, "exists $RCFILE"),
            workDir,
            timeoutMs = 60_000,
        ) { AppLog.debug("cpio", it) }
        val ok = verify.exitCode == 0
        AppLog.info(TAG, "自启注入 target=$target ok=$ok idempotent=$alreadyImported")
        return Result(ok, target, if (ok) "injected" else "verify_failed", alreadyImported, templateSource)
    }

    /** 回滚：用 ramdisk.cpio.orig 覆盖 ramdisk.cpio（AVB-2 等长冲突时使用） */
    fun rollback(workDir: File, ramdisk: File): Boolean {
        val backup = File(workDir, "ramdisk.cpio.orig")
        if (!backup.isFile) {
            AppLog.warn(TAG, "无 ramdisk 备份，无法回滚")
            return false
        }
        backup.copyTo(ramdisk, overwrite = true)
        AppLog.warn(TAG, "已回滚 ramdisk 到注入前状态（rc_injected=false, reason=avb_length_conflict）")
        return true
    }
}
