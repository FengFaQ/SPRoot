package com.sproot.manager.core

import android.content.Context
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * 安装/修补状态机（借鉴 APatch 的 10 态双轴思路，简化为单轴 6 态）。
 *
 * ```
 * 未修补 ──修补成功──> 已修补·待刷入 ──用户确认已刷入──> 已刷入·待重启 ──确认已重启──> 已激活
 *    └──修补失败──> 错误 ──重试成功──> 已修补·待刷入
 * ```
 *
 * 为什么需要它（旧版缺陷）：旧管理器没有"待刷入/待重启"这类中间态，
 * 用户无法区分"产物生成了"和"已经刷进设备"，容易被"成功"文案误导。
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
        ERROR("错误", "上次修补失败，详情见修补页（错误常驻可复读）"),
    }

    data class Snapshot(
        val status: Status = Status.UNPATCHED,
        val artifactDisplayPath: String? = null,
        val artifactSha256: String? = null,
        val artifactBytes: Long = 0L,
        val kernelEqualLength: Boolean? = null,
        val updatedAt: Long = 0L,
        val note: String? = null,
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
            ),
        )
    }

    fun markFlashed(context: Context) {
        save(
            context,
            load(context).copy(status = Status.FLASHED_PENDING_REBOOT, updatedAt = System.currentTimeMillis(), note = "用户确认已刷入"),
        )
    }

    fun markActive(context: Context) {
        save(
            context,
            load(context).copy(status = Status.ACTIVE, updatedAt = System.currentTimeMillis(), note = "用户确认已重启生效"),
        )
    }

    fun markError(context: Context, reason: String) {
        save(
            context,
            load(context).copy(status = Status.ERROR, updatedAt = System.currentTimeMillis(), note = reason.take(200)),
        )
    }
}
