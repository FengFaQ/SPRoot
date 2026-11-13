package com.sproot.manager.core

import android.content.Context

/**
 * 密钥与镜像匹配校验（真机 125 根因诊断，2026-11-13）。
 *
 * ## 三方指纹（口径统一 = sha256(key[0:16]) lowercase hex，见 SecretStore.fingerprintOf）
 * - **A 修补时密钥**：InstallState.keyFingerprint —— 刷入镜像内嵌的密钥；
 * - **B 当前 App 密钥**：SecretStore 实时计算 —— root-exec 提权时实际用的密钥；
 * - **C 已部署 key 文件**：apd `version --json` 的 `key_prefix_sha256`
 *   （apd/src/cli.rs print_version，任意身份可读）—— /data/adb/sproot/key（deploy 时写入）。
 *
 * ## 诊断矩阵（root-exec 125 时）
 * - B≠A（有 A 记录）→ [Verdict.APP_KEY_CHANGED]：密钥被重置（重装/清数据/轮换/48→47 迁移）
 *   → 重新修补+刷入即可恢复。
 * - B==A 仍 125 → [Verdict.MATCH_BUT_REJECTED]：App 密钥与修补记录一致 → 镜像可能没刷对，
 *   核对产物 sha256；多次失败报日志（可能是内核 hook 路径问题，需进一步定位）。
 * - 无 A 记录 → [Verdict.NO_RECORD]：无法精确诊断（渐进语义，见下）→ 给通用排查步骤。
 * - B≠C → [Verdict.DEPLOYED_KEY_STALE]：deploy 后 App 又换过钥 → 重新部署 AP。
 *
 * ## 渐进语义（物理边界，lead 裁决④确认）
 * A 与 B 同样随卸载/清数据消失——重装后 App 无从知道旧指纹，NO_RECORD 无法避免。
 * 精确诊断从"修补成功之后又发生换钥"的复现场景起生效；NO_RECORD 文案必须诚实：
 * 给通用排查步骤（核对 sha256 / 重刷 / 报日志），不承诺第一次就能精确诊断。
 */
object KeyMatch {

    enum class Verdict {
        /** A==B：当前密钥就是修补时的密钥（若仍 125 → 查镜像刷写，不是查密钥） */
        MATCH,
        /** A 存在但 ≠B：密钥被重置（重装/清数据/轮换/迁移）→ 重新修补+刷入 */
        APP_KEY_CHANGED,
        /** 无 A 记录：无法精确诊断 → 通用排查步骤 */
        NO_RECORD,
        /** B≠C：已部署 key 文件与当前 App 密钥不一致 → 重新部署 AP */
        DEPLOYED_KEY_STALE,
    }

    /** 当前 App 密钥指纹（B）；密钥不可用时 null */
    fun currentFingerprint(context: Context): String? =
        runCatching { SecretStore.get(context).readRootKey() }.getOrNull()
            ?.let { SecretStore.fingerprintOf(it) }

    /** 修补记录里的密钥指纹（A）；无记录时 null */
    fun patchedFingerprint(context: Context): String? =
        InstallState.load(context).keyFingerprint

    /**
     * 已部署 key 文件指纹（C）：解析 `apd version --json` 的 key_prefix_sha256。
     * version --json 契约任意身份可读（app 域直接 exec 引导器）；读不到（未部署/
     * 进程失败/key 文件不存在时字段为 null）返回 null。
     */
    fun deployedFingerprint(context: Context): String? {
        val result = runCatching { ApdClient.versionProbe(context) }.getOrNull() ?: return null
        if (!result.ok) return null
        return MiniJson.string(result.stdout, "key_prefix_sha256")
            ?.takeIf { it.isNotBlank() && it != "null" }
    }

    /** A vs B 对账（125 诊断的主判定）。 */
    fun check(context: Context): Verdict {
        val patched = patchedFingerprint(context)
        val current = currentFingerprint(context)
        if (patched == null || current == null) return Verdict.NO_RECORD
        return if (patched == current) Verdict.MATCH else Verdict.APP_KEY_CHANGED
    }

    /**
     * root-exec 125 的精确解释（5 个 125 出口统一文案）。
     *
     * 文案铁律（用户体验优先）：必须让用户知道**下一步做什么**，不留抽象错误码。
     * B==A 分支按 lead 补充 B 加"多次重刷仍失败请导出日志反馈"——该分支下可能是
     * 内核 hook 路径问题（patch_do_execve 符号选择），不能让用户死循环重刷。
     */
    fun escalationExplanation(context: Context): String {
        val patchedAt = InstallState.load(context).updatedAtText
        return when (check(context)) {
            Verdict.APP_KEY_CHANGED ->
                "当前密钥与刷入镜像内嵌的密钥不一致（密钥可能因重装 App、清除数据、" +
                    "「重新生成」或旧格式迁移被重置；镜像修补于 $patchedAt）。\n" +
                    "解决：直接到「修补」页点「开始修补」重新修补 boot.img 并刷入" +
                    "（会用当前密钥），刷入重启后 root 即可恢复。"
            Verdict.MATCH ->
                "当前密钥与修补记录一致（修补于 $patchedAt），但内核未接受提权。\n" +
                    "请核对实际刷入的镜像 sha256 是否等于修补页显示的产物 sha256" +
                    "（可能刷入了旧产物或刷写失败）；若多次重刷仍失败，" +
                    "请在设置页导出日志并反馈（需要进一步定位内核 hook 路径）。"
            Verdict.NO_RECORD ->
                "提权被内核拒绝（当前密钥与内核不匹配），且本机没有可对账的修补记录，" +
                    "无法自动判断原因。请依次排查：\n" +
                    "① 核对刷入的镜像 sha256 是否等于修补页显示的产物 sha256；\n" +
                    "② 不一致则重新修补并刷入；一致仍失败，请导出日志并反馈。"
            Verdict.DEPLOYED_KEY_STALE ->
                "已部署的 AP key 文件与当前 App 密钥不一致（deploy 之后密钥被更换过）。\n" +
                    "解决：到「设置 → AP 部署」重新部署 AP（deploy 会写入当前密钥），" +
                    "然后重试。"
        }
    }
}
