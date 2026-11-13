package com.sproot.manager.core

import android.content.Context
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * 安装/修补状态机（借鉴 APatch 的 10 态双轴思路，简化为单轴 7 态）。
 *
 * ```
 * 未修补 ──修补成功──> 已修补·待刷入 ──daemon 探测就绪──> 已激活
 *    ├──修补失败──> 错误 ──重试成功──> 已修补·待刷入
 *    └──导出失败──> 导出失败（产物已生成，可重新修补导出）
 * ```
 *
 * 为什么需要它（旧版缺陷）：旧管理器没有"待刷入"这类中间态，
 * 用户无法区分"产物生成了"和"已经刷进设备"，容易被"成功"文案误导。
 *
 * ## 实际状态转移（与代码一致，2026-11-13 修订）
 *  - ACTIVE 由 [DaemonBootstrap] 真实探测（apd selftest stage=daemon_ready）跳转：
 *    [markActiveDetected] 是 ACTIVE 的唯一来源（授权成功 ≠ 已安装）。
 *  - PATCHED_PENDING_FLASH / FLASHED_PENDING_REBOOT 仍保留为持久化记录
 *    （markPatchedPendingFlash 写前者；后者作为已刷入待重启的持久化标记保留），
 *    但**不再有手动确认按钮**（v2 UI 精简后删除了旧 PatchPage 的两级状态卡）。
 *  - EXPORT_FAILED（本机新增）：修补/校验全部通过、仅写 Downloads 失败——
 *    产物完好、可重试导出，与"修补本身失败"的 ERROR 语义区分。
 */
object InstallState {

    private const val TAG = "InstallState"
    private const val PREFS = "sproot_install_state"

    enum class Status(val label: String, val hint: String) {
        UNPATCHED("未修补", "还没有可刷入的产物，请到「修补」页选择 boot.img 开始"),
        PATCHING("修补中", "修补进行中，请保持 App 在前台"),
        PATCHED_PENDING_FLASH("已修补 · 待刷入", "产物已导出，尚未刷入设备"),
        FLASHED_PENDING_REBOOT("已刷入 · 待重启", "镜像已刷入，重启后生效"),
        ACTIVE("已激活", "设备已运行修补后的内核"),
        /** 修补/校验全部通过，仅写 Downloads 失败：产物完好，可重试导出（≠ 修补失败） */
        EXPORT_FAILED("导出失败", "产物已生成并保留在应用目录；请授权存储或清理空间后重新修补导出"),
        ERROR("错误", "上次修补失败，详情见修补页（错误常驻可复读）"),
    }

    /** 单个产物（P2 双镜像：boot 与 init_boot 各一条） */
    data class Artifact(
        val label: String,
        val displayPath: String,
        val sha256: String,
        val bytes: Long,
    )

    data class Snapshot(
        val status: Status = Status.UNPATCHED,
        val artifactDisplayPath: String? = null,
        val artifactSha256: String? = null,
        val artifactBytes: Long = 0L,
        val kernelEqualLength: Boolean? = null,
        val updatedAt: Long = 0L,
        val note: String? = null,
        /** 本次修补的全部产物（boot / init_boot）；旧数据为空表示只有 artifactDisplayPath 一个 */
        val artifacts: List<Artifact> = emptyList(),
    ) {
        val updatedAtText: String
            get() = if (updatedAt <= 0L) "—" else
                SimpleDateFormat("yyyy-MM-dd HH:mm:ss", Locale.US).format(Date(updatedAt))
    }

    fun load(context: Context): Snapshot {
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val status = runCatching {
            Status.valueOf(prefs.getString("status", Status.UNPATCHED.name)!!)
        }.getOrDefault(Status.UNPATCHED)
        val equalLengthRaw = prefs.getString("kernel_equal_length", null)
        return Snapshot(
            status = status,
            artifactDisplayPath = prefs.getString("artifact_path", null),
            artifactSha256 = prefs.getString("artifact_sha256", null),
            artifactBytes = prefs.getLong("artifact_bytes", 0L),
            kernelEqualLength = when (equalLengthRaw) {
                null -> null
                "true" -> true
                else -> false
            },
            updatedAt = prefs.getLong("updated_at", 0L),
            note = prefs.getString("note", null),
            artifacts = prefs.getString("artifacts", null)?.lineSequence()
                ?.mapNotNull { line ->
                    val p = line.split('\t')
                    if (p.size >= 4) {
                        Artifact(
                            label = p[0],
                            displayPath = p[1],
                            sha256 = p[2],
                            bytes = p[3].toLongOrNull() ?: 0L,
                        )
                    } else null
                }?.toList().orEmpty(),
        )
    }

    fun save(context: Context, snapshot: Snapshot) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
            .putString("status", snapshot.status.name)
            .putString("artifact_path", snapshot.artifactDisplayPath)
            .putString("artifact_sha256", snapshot.artifactSha256)
            .putLong("artifact_bytes", snapshot.artifactBytes)
            .putString("kernel_equal_length", snapshot.kernelEqualLength?.toString())
            .putLong("updated_at", snapshot.updatedAt)
            .putString("note", snapshot.note)
            .putString(
                "artifacts",
                snapshot.artifacts.joinToString("\n") { "${it.label}\t${it.displayPath}\t${it.sha256}\t${it.bytes}" },
            )
            .apply()
        AppLog.info(TAG, "状态 → ${snapshot.status.label}（${snapshot.note ?: "无备注"}）")
    }

    fun markPatching(context: Context) {
        save(context, load(context).copy(status = Status.PATCHING, updatedAt = System.currentTimeMillis(), note = "修补开始"))
    }

    /**
     * @param audit 审计摘要（AVB 断言结论 / rc 注入结果），写入持久化 note 供 qa 从他处断言；
     *              为空时只写「导出成功，等待刷入」。
     */
    fun markPatchedPendingFlash(
        context: Context,
        artifactDisplayPath: String,
        artifactSha256: String,
        artifactBytes: Long,
        kernelEqualLength: Boolean,
        audit: String? = null,
        artifacts: List<Artifact> = emptyList(),
    ) {
        val note = buildString {
            append("导出成功，等待刷入")
            if (!audit.isNullOrBlank()) append("；").append(audit)
        }.take(400)
        save(
            context,
            load(context).copy(
                status = Status.PATCHED_PENDING_FLASH,
                artifactDisplayPath = artifactDisplayPath,
                artifactSha256 = artifactSha256,
                artifactBytes = artifactBytes,
                kernelEqualLength = kernelEqualLength,
                updatedAt = System.currentTimeMillis(),
                note = note,
                artifacts = artifacts,
            ),
        )
    }

    /**
     * 真实探测到 AP 守护就绪（`apd selftest` stage=daemon_ready / 自引导成功）→
     * 设备确实运行在修补后的内核上。这是主页「已安装」的**唯一可靠来源**
     * （人类 2026-11-10：授权成功 ≠ 已安装；PATCHED_PENDING_FLASH/FLASHED_PENDING_REBOOT 都不算）。
     */
    fun markActiveDetected(context: Context, evidence: String) {
        save(
            context,
            load(context).copy(
                status = Status.ACTIVE,
                updatedAt = System.currentTimeMillis(),
                note = evidence.take(200),
            ),
        )
    }

    /** 导出失败（修补/校验已通过、仅写 Downloads 失败）：产物完好，可重试导出。 */
    fun markExportFailed(context: Context, reason: String) {
        save(
            context,
            load(context).copy(
                status = Status.EXPORT_FAILED,
                updatedAt = System.currentTimeMillis(),
                note = reason.take(200),
            ),
        )
    }

    /**
     * PATCHING 过期调和（BUG-APP-10）：修补中进程被杀后状态机永久卡"修补中"
     * （无实时进程可对账）。App 启动时调用：PATCHING 且 updatedAt 距今超过 30 分钟
     * → 降级 UNPATCHED（重新修补即可，无部分产物需要提示）。
     * @return true = 发生了降级（调用方可记日志）
     */
    fun reconcileStalePatching(context: Context, staleThresholdMs: Long = 30 * 60 * 1000L): Boolean {
        val snap = load(context)
        if (snap.status != Status.PATCHING || snap.updatedAt <= 0L) return false
        if (System.currentTimeMillis() - snap.updatedAt <= staleThresholdMs) return false
        save(
            context,
            snap.copy(
                status = Status.UNPATCHED,
                updatedAt = System.currentTimeMillis(),
                note = "上次修补中断（进程被杀且超过 30 分钟未更新），已重置；请重新修补",
            ),
        )
        AppLog.warn(TAG, "PATCHING 过期调和：updatedAt=${snap.updatedAt} 超过阈值，已降级 UNPATCHED")
        return true
    }

    fun markError(context: Context, reason: String) {
        save(
            context,
            load(context).copy(status = Status.ERROR, updatedAt = System.currentTimeMillis(), note = reason.take(200)),
        )
    }
}
