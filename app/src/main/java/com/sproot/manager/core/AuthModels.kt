package com.sproot.manager.core

/**
 * Root 授权模型（D：授权管理页）。
 *
 * ## 契约状态（重要）
 * AP 侧的权威接口由 apd-dev 落地中：`apd auth list/set/log/revoke --json`，
 * 三态 `allow|deny|ask`，列表字段 `{uid, package, state, last_grant_at, deny_count, request_count}`。
 * **在该契约落地前，[AuthRepository] 只返回占位假数据**（`fake = true`），
 * UI 会明确标注"占位数据"，绝不假装已经接入。
 *
 * ## 三态语义
 * - `allow`：允许提权（`apd root-exec` 直接放行）；
 * - `deny`：拒绝（不弹窗、直接失败）；
 * - `ask`：每次询问（**依赖 daemon 挂起请求 + 管理器回传弹窗，本轮未实现**，
 *   UI 显示但禁用，并标注"待实现"，不假装可用）。
 */
enum class AuthState(val wire: String, val label: String) {
    ALLOW("allow", "允许"),
    DENY("deny", "拒绝"),
    ASK("ask", "每次询问");

    companion object {
        /** 宽容解析：未知/缺失一律回落到 [DENY]（最保守），并保留原始 wire 供日志排查。 */
        fun fromWire(raw: String?): AuthState =
            entries.firstOrNull { it.wire == raw?.trim()?.lowercase() } ?: DENY
    }
}

/** 单个被管理的应用（uid + 包名 + 当前授权态 + 统计）。 */
data class AuthApp(
    val uid: Int,
    val packageName: String,
    val label: String,
    val state: AuthState,
    val lastGrantAt: Long = 0L,
    val denyCount: Int = 0,
    val requestCount: Int = 0,
)

/** 授权操作（set/revoke）的返回；[supported] 区分"接口未接入"与"业务失败"。 */
data class AuthOpResult(
    val ok: Boolean,
    val supported: Boolean,
    val message: String,
    val raw: String = "",
)
