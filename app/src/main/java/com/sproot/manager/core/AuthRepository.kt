package com.sproot.manager.core

import android.content.Context

/**
 * Root 授权数据层 —— D（授权管理页）的唯一数据入口。
 *
 * ## 为什么要先说"未接入"
 * 权威接口是 apd-dev 正在实现的 `apd auth list/set/log/revoke --json`。在它落地前，
 * 本类返回**明确标记 fake = true 的占位数据**，UI 会展示提示条；
 * 任何"看起来能改、实际不生效"的实现都是禁止的（本项目最忌讳"看起来成功实则无效"）。
 *
 * ## 接入方式（apd-dev 契约到位后只改本文件）
 * - `list`：`ApdClient.rootExec(key, listOf(apd, "auth", "list", "--json"))`，
 *   用 `MiniJson`/`Modules.kt` 同款扁平解析拿到 `{uid,package,state,last_grant_at,deny_count,request_count}`；
 * - `set`：`auth set <uid> <allow|deny|ask> --json`；`ask` 需 daemon 弹窗能力，未就绪则 `supported=false`；
 * - `revoke`：`auth revoke <uid> --json`；
 * - `log`：`auth log --json`（详情页"最近记录"用，本轮不做）。
 *
 * 业务严格只走 `ApdClient`（root-exec），App 不直接读 /data/adb。
 */
object AuthRepository {

    private const val TAG = "Auth"

    /** 契约标志：true = 仍为占位假数据；apd-dev 契约落地后置 false 并替换实现。 */
    const val API_READY = false

    const val FAKE_NOTICE =
        "授权 API 尚未接入（等待 apd auth list/set/log/revoke --json 契约）：" +
            "当前列表为占位假数据，状态变更只在本地预览、不会写入 daemon"

    data class ListResult(
        val ok: Boolean,
        val fake: Boolean,
        val message: String,
        val apps: List<AuthApp>,
        val raw: String = "",
    )

    private fun fakeApps(): List<AuthApp> = listOf(
        AuthApp(
            uid = 2000,
            packageName = "com.termux",
            label = "Termux",
            state = AuthState.ALLOW,
            lastGrantAt = System.currentTimeMillis() - 86_400_000L,
            requestCount = 12,
        ),
        AuthApp(
            uid = 2001,
            packageName = "bin.mt.plus",
            label = "MT 管理器",
            state = AuthState.DENY,
            denyCount = 3,
            requestCount = 3,
        ),
        AuthApp(
            uid = 2002,
            packageName = "com.topjohnwu.magisk",
            label = "Magisk",
            state = AuthState.ASK,
            requestCount = 1,
        ),
        AuthApp(
            uid = 2003,
            packageName = "com.android.shell",
            label = "Shell",
            state = AuthState.ALLOW,
            lastGrantAt = System.currentTimeMillis() - 3_600_000L,
            requestCount = 8,
        ),
    )

    /** 列表（当前=假数据）。调用方放后台线程。 */
    fun list(context: Context, superKey: String?): ListResult {
        if (API_READY) {
            // 契约落地后在此替换为 apd auth list --json（保留 fake=false 契约）
            error("AuthRepository.API_READY=true 但 apd 契约实现尚未接入")
        }
        AppLog.info(TAG, "授权列表：契约未接入，返回占位假数据（${fakeApps().size} 条）")
        return ListResult(
            ok = true,
            fake = true,
            message = FAKE_NOTICE,
            apps = fakeApps(),
        )
    }

    /** 设置授权态。契约未接入时返回 supported=false（UI 只做本地预览并明确提示）。 */
    fun setState(context: Context, superKey: String?, uid: Int, state: AuthState): AuthOpResult {
        if (state == AuthState.ASK) {
            return AuthOpResult(
                ok = false,
                supported = false,
                message = "「每次询问」需要 daemon 挂起请求并回传弹窗，AP 侧尚未实现（待 apd-dev），当前不可用",
            )
        }
        if (!API_READY) {
            return AuthOpResult(
                ok = false,
                supported = false,
                message = "授权 API 尚未接入（apd auth set --json 待 apd-dev）：仅本地预览，未写入 daemon",
            )
        }
        error("AuthRepository.API_READY=true 但 apd 契约实现尚未接入")
    }

    /** 撤销授权（等价 set deny + 清统计）。 */
    fun revoke(context: Context, superKey: String?, uid: Int): AuthOpResult {
        if (!API_READY) {
            return AuthOpResult(
                ok = false,
                supported = false,
                message = "授权 API 尚未接入（apd auth revoke --json 待 apd-dev）：仅本地预览",
            )
        }
        error("AuthRepository.API_READY=true 但 apd 契约实现尚未接入")
    }
}
