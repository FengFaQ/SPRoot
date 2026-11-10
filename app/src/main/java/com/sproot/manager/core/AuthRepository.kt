package com.sproot.manager.core

import android.content.Context
import android.content.pm.ApplicationInfo
import android.content.pm.PackageManager
import android.graphics.drawable.Drawable
import android.os.Process

/**
 * Root 授权数据层 —— D（授权管理页）的唯一数据入口。
 *
 * 契约来源：apd commit d6c9ecb（apd-dev，`apd auth list/set/revoke/log --json`）。
 * 全部命令 **root-only**，统一经 [ApdClient.rootExec]（契约 v1.1 §8 唯一特权入口）；
 * App 不直接读 /data/adb、不 execve 提权。
 *
 * ## 字段解析要点
 * - `label` / `app_icon_path` 恒为 `null`（apd 侧无 PackageManager）→ 本类用 App 的
 *   PackageManager 按 `package` 解析应用名与图标；包不存在（uid 无对应包）时
 *   label = `uid=xxxxx` + 通用图标（icon=null），**不崩、不显示空白**；
 * - `last_grant_at` / `last_deny_at` / `ts` 为 **unix 秒** → 统一换算成毫秒；
 * - `ask` 在 Phase 1 不可交互（当前等同拒绝），[setState] 拒绝把状态设成 ask 并给出说明；
 * - 日志只有脱敏信息（无完整命令/无密钥），本类不补全、不猜测。
 */
object AuthRepository {

    private const val TAG = "Auth"

    /** 默认授权态说明（页面顶部展示；避免用户困惑"为什么新 App 申请不到 root"）。 */
    const val DEFAULT_STATE_NOTICE =
        "默认授权态是「待询问」(ask)：新安装应用第一次请求 root 会被拒绝（decision=ask_pending），" +
            "请在此把它改为「允许」"

    data class ListResult(
        val ok: Boolean,
        val message: String,
        val apps: List<AuthApp>,
        val raw: String = "",
    )

    data class LogResult(
        val ok: Boolean,
        val message: String,
        val entries: List<AuthLogEntry>,
        val raw: String = "",
    )

    // ------------------------------------------------------------------ 列表

    /** 统一列表：**手机上所有已安装用户应用**（主数据源）+ apd auth list 的授权态合并。 */
    data class UnifiedResult(
        val ok: Boolean,
        /** apd auth list 是否读取成功；失败时列表仍展示（全部未授权）+ 提示 */
        val authOk: Boolean,
        val message: String,
        val apps: List<AuthApp>,
    )

    /**
     * 主数据源 = PackageManager 枚举的所有**用户应用**（人类指令 D：列出所有用户软件、默认未授权）；
     * 授权态来自 `apd auth list --json`，按 uid（其次 package）合并；不在列表里的应用 = [AuthState.NONE]。
     *
     * 阻塞，调用方放后台线程（枚举 + 图标解析 + root-exec）。
     */
    fun unifiedList(context: Context, superKey: String?): UnifiedResult {
        val installed = runCatching { installedUserApps(context) }.getOrElse { error ->
            AppLog.error(TAG, "枚举已安装应用失败", error)
            emptyList()
        }
        val authResult = list(context, superKey)
        val byUid = authResult.apps.associateBy { it.uid }
        val byPackage = authResult.apps.filter { it.packageName.isNotBlank() }.associateBy { it.packageName }
        val merged = installed.map { app ->
            val auth = byUid[app.uid] ?: byPackage[app.packageName]
            if (auth == null) {
                app
            } else {
                app.copy(
                    state = auth.state,
                    lastGrantAt = auth.lastGrantAt,
                    lastDenyAt = auth.lastDenyAt,
                    requestCount = auth.requestCount,
                    toUid = auth.toUid,
                    sctx = auth.sctx,
                    exclude = auth.exclude,
                )
            }
        }
        // apd 里有、但 PM 枚举不到（已卸载/无包名 uid）→ 追加，避免"幽灵授权"不可见
        val installedUids = installed.map { it.uid }.toSet()
        val installedPackages = installed.map { it.packageName }.toSet()
        val extras = authResult.apps.filter {
            it.uid !in installedUids && it.packageName !in installedPackages
        }
        val all = (merged + extras).sortedWith(
            compareByDescending<AuthApp> { it.state == AuthState.ALLOW }
                .thenBy { it.label.lowercase() },
        )
        AppLog.info(TAG, "统一授权列表：已安装=${installed.size} apd=${authResult.apps.size} 合并=${all.size}")
        return UnifiedResult(
            ok = installed.isNotEmpty() || authResult.ok,
            authOk = authResult.ok,
            message = if (authResult.ok) "共 ${all.size} 个应用" else authResult.message,
            apps = all,
        )
    }

    /** 枚举所有已安装的**用户应用**（排除系统应用与本 App），默认状态 [AuthState.NONE]。 */
    fun installedUserApps(context: Context): List<AuthApp> {
        val pm = context.packageManager
        val selfUid = Process.myUid()
        return pm.getInstalledApplications(PackageManager.GET_META_DATA)
            .asSequence()
            .filter { it.uid != selfUid }
            .filter { ai ->
                (ai.flags and ApplicationInfo.FLAG_SYSTEM) == 0 ||
                    (ai.flags and ApplicationInfo.FLAG_UPDATED_SYSTEM_APP) != 0
            }
            .map { ai ->
                val label = runCatching { pm.getApplicationLabel(ai).toString() }
                    .getOrNull()
                    ?.takeIf { it.isNotBlank() }
                    ?: ai.packageName
                val icon = runCatching { pm.getApplicationIcon(ai) }.getOrNull()
                AuthApp(
                    uid = ai.uid,
                    packageName = ai.packageName,
                    label = label,
                    state = AuthState.NONE,
                    icon = icon,
                )
            }
            .sortedBy { it.label.lowercase() }
            .toList()
    }

    /** 列表（阻塞，调用方放后台线程）。 */
    fun list(context: Context, superKey: String?): ListResult {
        if (superKey.isNullOrBlank()) {
            return ListResult(false, "Root 密钥不可用：请到「设置 → Root 密钥」重新生成后重试", emptyList())
        }
        val result = exec(context, superKey, listOf("auth", "list", "--json"), 60)
        if (!result.ok) {
            return ListResult(false, explain(result), emptyList(), rawOf(result))
        }
        val rows = parseObjectArray(result.stdout)
        if (rows.isEmpty() && result.stdout.trim().let { it.isNotEmpty() && it != "[]" }) {
            return ListResult(
                false,
                "apd auth list 输出无法解析（契约不一致，请上报）：\n${result.stdout.take(800)}",
                emptyList(),
                result.stdout,
            )
        }
        val apps = rows.mapNotNull { parseApp(context, it) }
        AppLog.info(TAG, "授权列表：${apps.size} 条（apd auth list --json）")
        return ListResult(true, "共 ${apps.size} 条（apd auth list --json）", apps, result.stdout)
    }

    private fun parseApp(context: Context, row: Map<String, String>): AuthApp? {
        val uid = row["uid"]?.toIntOrNull() ?: return null
        val pkg = row["package"].orNullValue()
        val state = AuthState.fromWire(row["state"])
        val (label, icon) = resolveAppInfo(context, pkg, uid)
        return AuthApp(
            uid = uid,
            packageName = pkg ?: "",
            label = label,
            state = state,
            lastGrantAt = (row["last_grant_at"]?.toLongOrNull() ?: 0L) * 1000L,
            lastDenyAt = (row["last_deny_at"]?.toLongOrNull() ?: 0L) * 1000L,
            requestCount = row["request_count"]?.toIntOrNull() ?: 0,
            icon = icon,
            toUid = row["to_uid"]?.toIntOrNull() ?: 0,
            sctx = row["sctx"].orNullValue() ?: "",
            exclude = row["exclude"] == "true",
        )
    }

    /** 用 PackageManager 解析应用名/图标；uid 无对应包时回落 `uid=xxxxx`（不崩、不空白）。 */
    private fun resolveAppInfo(
        context: Context,
        pkg: String?,
        uid: Int,
    ): Pair<String, Drawable?> {
        if (pkg.isNullOrBlank()) return "uid=$uid" to null
        val pm = context.packageManager
        return runCatching {
            val info = pm.getApplicationInfo(pkg, 0)
            val label = pm.getApplicationLabel(info).toString().ifBlank { pkg }
            label to runCatching { pm.getApplicationIcon(info) }.getOrNull()
        }.getOrElse { error ->
            AppLog.warn(TAG, "解析应用信息失败（package=$pkg）：${error.javaClass.simpleName}: ${error.message}")
            pkg to null
        }
    }

    // ------------------------------------------------------------------ 写操作

    /** 设置授权态。`ask` 在 Phase 1 不可交互 → 返回 supported=false 并说明原因。 */
    fun setState(context: Context, superKey: String?, uid: Int, state: AuthState): AuthOpResult {
        if (state == AuthState.ASK) {
            return AuthOpResult(
                ok = false,
                supported = false,
                message = "「待询问」Phase 1 不可交互（daemon 挂起请求/弹窗回传未实现，当前等同拒绝）；" +
                    "请改为「允许」或「拒绝」",
            )
        }
        if (superKey.isNullOrBlank()) {
            return AuthOpResult(false, false, "Root 密钥不可用：请到设置页重新生成")
        }
        val result = exec(
            context,
            superKey,
            listOf("auth", "set", "--uid", uid.toString(), "--state", state.wire, "--json"),
            60,
        )
        return opResult(result, state)
    }

    /** 撤销授权（等价置为拒绝）。 */
    fun revoke(context: Context, superKey: String?, uid: Int): AuthOpResult {
        if (superKey.isNullOrBlank()) {
            return AuthOpResult(false, false, "Root 密钥不可用：请到设置页重新生成")
        }
        val result = exec(context, superKey, listOf("auth", "revoke", "--uid", uid.toString(), "--json"), 60)
        return opResult(result, AuthState.DENY)
    }

    private fun opResult(result: ApdClient.Result, target: AuthState): AuthOpResult {
        if (!result.ok) {
            return AuthOpResult(false, result.exitCode != 2, explain(result), rawOf(result))
        }
        val fields = MiniJson.objectOf(result.stdout)
        val jsonOk = fields["ok"]?.equals("true", ignoreCase = true) == true
        val previous = fields["previous"].orNullValue()
        val changed = fields["changed"]?.equals("true", ignoreCase = true) == true
        val message = buildString {
            append(if (jsonOk) "已设为「${target.label}」" else "apd 返回 ok=false")
            if (!previous.isNullOrBlank()) append("（原状态：$previous）")
            if (!jsonOk) append("：").append(result.stdout.take(400))
            if (!changed) append(" · 状态未变化")
        }
        AppLog.info(TAG, "auth set/revoke → uid=${fields["uid"] ?: "?"} state=$target ok=$jsonOk changed=$changed")
        return AuthOpResult(jsonOk, true, message, rawOf(result))
    }

    // ------------------------------------------------------------------ 日志

    /** 审计日志（最新在前；apd 只给脱敏信息）。 */
    fun log(context: Context, superKey: String?, limit: Int = 100): LogResult {
        if (superKey.isNullOrBlank()) {
            return LogResult(false, "Root 密钥不可用：请到设置页重新生成", emptyList())
        }
        val result = exec(context, superKey, listOf("auth", "log", "--json", "--limit", limit.toString()), 60)
        if (!result.ok) {
            return LogResult(false, explain(result), emptyList(), rawOf(result))
        }
        val rows = parseObjectArray(result.stdout)
        val entries = rows.mapNotNull { row ->
            val uid = row["uid"]?.toIntOrNull() ?: return@mapNotNull null
            AuthLogEntry(
                ts = (row["ts"]?.toLongOrNull() ?: 0L) * 1000L,
                iso = row["iso"].orNullValue() ?: "",
                uid = uid,
                packageName = row["package"].orNullValue(),
                event = row["event"].orNullValue() ?: "unknown",
                state = row["state"].orNullValue(),
                decision = row["decision"].orNullValue(),
                cmdName = row["cmd_name"].orNullValue(),
                detail = row["detail"].orNullValue(),
            )
        }
        AppLog.info(TAG, "授权日志：${entries.size} 条（apd auth log --json）")
        return LogResult(true, "共 ${entries.size} 条（最新在前）", entries, result.stdout)
    }

    // ------------------------------------------------------------------ 工具

    private fun exec(
        context: Context,
        superKey: String,
        args: List<String>,
        timeoutSec: Int,
    ): ApdClient.Result = ApdClient.rootExec(
        context = context,
        superKey = superKey,
        command = listOf(ApdClient.apdBinary(context).absolutePath) + args,
        timeoutSec = timeoutSec,
    )

    private fun rawOf(result: ApdClient.Result): String =
        (result.stdout + if (result.stderr.isNotBlank()) "\n[stderr]\n" + result.stderr else "").trim()

    /** 退出码 → 人话（125 = 内核未刷补丁镜像的"正常初始态"，不标红为失败契约错误）。 */
    private fun explain(result: ApdClient.Result): String = when {
        result.timedOut -> "apd auth 调用超时（124），请重试"
        result.exitCode == ApdClient.EXIT_ESCALATION ->
            "尚未部署 AP 守护（125，需先刷入修补后的镜像并部署 AP）"
        result.exitCode == ApdClient.EXIT_USAGE ->
            "apd auth 参数与客户端不一致（2，契约不匹配，请上报 apd-dev）：${result.stderr.take(300)}"
        result.exitCode == ApdClient.EXIT_NOT_EXECUTABLE || result.exitCode == ApdClient.EXIT_NOT_FOUND ->
            "AP 调不起来（${result.exitCode}）：请到「设置 → AP 部署」重新部署"
        else -> result.describe() + if (result.stderr.isNotBlank()) "：${result.stderr.take(300)}" else ""
    }

    /** `"null"` / 空白 → null。 */
    private fun String?.orNullValue(): String? =
        this?.trim()?.takeIf { it.isNotEmpty() && !it.equals("null", ignoreCase = true) }

    /**
     * 解析 JSON **裸数组 of objects**（apd auth list/log 的返回形态）。
     * 复用 [MiniJson.objectOf] 解析每个扁平对象；嵌套对象/数组字段按契约不存在。
     */
    private fun parseObjectArray(json: String?): List<Map<String, String>> {
        if (json.isNullOrBlank()) return emptyList()
        val text = json.trim()
        val start = text.indexOf('[')
        if (start < 0) return emptyList()
        val objects = mutableListOf<Map<String, String>>()
        var depth = 0
        var inString = false
        var escaped = false
        var objectStart = -1
        var i = start
        while (i < text.length) {
            val c = text[i]
            when {
                escaped -> escaped = false
                inString && c == '\\' -> escaped = true
                c == '"' -> inString = !inString
                inString -> Unit
                c == '{' -> {
                    if (depth == 0) objectStart = i
                    depth++
                }

                c == '}' -> {
                    depth--
                    if (depth == 0 && objectStart >= 0) {
                        objects += MiniJson.objectOf(text.substring(objectStart, i + 1))
                        objectStart = -1
                    }
                }

                c == ']' && depth == 0 -> return objects
            }
            i++
        }
        return objects
    }
}
