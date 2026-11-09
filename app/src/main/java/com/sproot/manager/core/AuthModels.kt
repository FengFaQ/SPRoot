package com.sproot.manager.core

import android.graphics.drawable.Drawable

/**
 * Root 授权模型（D：授权管理页）。
 *
 * ## 权威契约（apd commit d6c9ecb，apd-dev 已落地）
 * - 列表：`apd auth list --json` → **裸数组**，每项字段
 *   `{uid, package, label, app_icon_path, state, last_grant_at, last_deny_at,
 *     request_count, to_uid, sctx, exclude}`
 *   - `label`/`app_icon_path` 恒为 `null`（apd 侧无 PackageManager）→ **App 侧按 package 解析**
 *   - `last_grant_at`/`last_deny_at` 为 **unix 秒**
 * - 设置：`apd auth set --uid <uid> --state allow|deny|ask --json` →
 *   `{ok, uid, state, previous, package, changed}`
 * - 撤销：`apd auth revoke --uid <uid> --json` → 同结构
 * - 日志：`apd auth log --json --limit N` → 裸数组（最新在前），每项
 *   `{ts, iso, uid, package, event, state, decision, previous, cmd_name, cmd_args, cmd_sha256, detail}`
 *
 * ## 三态语义（Phase 1）
 * - `allow`：允许提权；
 * - `deny`：拒绝；
 * - `ask`：**Phase 1 不可交互** —— daemon 挂起请求/弹窗回传尚未实现，**当前等同拒绝**
 *   （日志里会记 `decision=ask_pending`）。UI 只展示、不提供"设成 ask"的操作，并提示改为允许/拒绝。
 *
 * ## 默认态
 * 默认授权态是 `ask` → **新安装应用第一次请求 root 会被拒**。授权页必须明确告知用户，
 * 否则会出现"App 申请不到 root，却不知道为什么"。
 */
enum class AuthState(val wire: String, val label: String) {
    ALLOW("allow", "允许"),
    DENY("deny", "拒绝"),
    ASK("ask", "待询问");

    companion object {
        /** 宽容解析：未知/缺失一律回落到 [DENY]（最保守），原始值仍写日志便于排查。 */
        fun fromWire(raw: String?): AuthState =
            entries.firstOrNull { it.wire == raw?.trim()?.lowercase() } ?: DENY
    }
}

/** 单个被管理的应用（uid + 包名 + 当前授权态 + 统计 + App 侧解析出的名称/图标）。 */
data class AuthApp(
    val uid: Int,
    val packageName: String,
    val label: String,
    val state: AuthState,
    /** unix 毫秒（apd 给的是秒，Repository 已换算） */
    val lastGrantAt: Long = 0L,
    val lastDenyAt: Long = 0L,
    val requestCount: Int = 0,
    /** App 侧 PackageManager 解析的应用图标；解析不到（包不存在）为 null → UI 用通用图标 */
    val icon: Drawable? = null,
    val toUid: Int = 0,
    val sctx: String = "",
    val exclude: Boolean = false,
)

/** 审计日志条目（apd 只给脱敏信息：无完整命令、无密钥；UI 不补全不猜测）。 */
data class AuthLogEntry(
    /** unix 毫秒 */
    val ts: Long,
    val iso: String,
    val uid: Int,
    val packageName: String?,
    /** request | set_state | revoke | grant | migrate */
    val event: String,
    /** allow | deny | ask | null */
    val state: String?,
    /** allow | deny | ask_pending | null */
    val decision: String?,
    val cmdName: String?,
    val detail: String?,
)

/** 授权操作（set/revoke）的返回；[supported] 区分"契约不匹配/未接入"与"业务失败"。 */
data class AuthOpResult(
    val ok: Boolean,
    val supported: Boolean,
    val message: String,
    val raw: String = "",
)
