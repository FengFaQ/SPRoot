package com.sproot.manager.core

/**
 * 模块列表模型与解析（`apd module list --json`）。
 *
 * ## Schema 提案 v1（客户端侧自定，已请求 sys-guard 对齐；详见 NEW-DOC-04 §7.1）
 * ```json
 * [
 *   {
 *     "id": "magisk-module-id",
 *     "name": "显示名",
 *     "version": "1.2.3",
 *     "versionCode": 12,
 *     "author": "作者",
 *     "description": "一句话描述",
 *     "enabled": true,
 *     "update_pending": false
 *   }
 * ]
 * ```
 * 解析策略：**容忍缺字段**（缺失显示 "—"），未知字段忽略；解析失败时把**原始 stdout 全文**
 * 展示给用户（绝不静默吞掉，旧版"闪一下"的反面）。
 */
data class ModuleInfo(
    val id: String,
    val name: String,
    val version: String,
    val author: String,
    val description: String,
    val enabled: Boolean,
    val updatePending: Boolean,
) {
    companion object {
        /** 从 apd 的 JSON 对象解析；缺字段不外抛 */
        fun fromMap(map: Map<String, String>): ModuleInfo = ModuleInfo(
            id = map["id"] ?: map["module_id"] ?: map["name"] ?: "",
            name = map["name"] ?: map["id"] ?: "(未命名模块)",
            version = map["version"] ?: map["versionName"] ?: "—",
            author = map["author"] ?: "—",
            description = map["description"] ?: map["summary"] ?: "",
            enabled = map.bool("enabled", default = true),
            updatePending = map.bool("update_pending", default = false) ||
                map.bool("updatePending", default = false),
        )

        private fun Map<String, String>.bool(key: String, default: Boolean): Boolean =
            when (this[key]?.trim()?.lowercase()) {
                null -> default
                "true", "1", "yes", "enabled", "on" -> true
                "false", "0", "no", "disabled", "off" -> false
                else -> default
            }
    }
}

/** 模块列表拉取结果（成功/失败都带原始输出，便于 UI 常驻展示） */
data class ModulesResult(
    val ok: Boolean,
    val exitCode: Int,
    val message: String,
    val modules: List<ModuleInfo>,
    val rawStdout: String,
    val rawStderr: String,
) {
    companion object {
        fun failure(exitCode: Int, message: String, stdout: String, stderr: String): ModulesResult =
            ModulesResult(false, exitCode, message, emptyList(), stdout, stderr)
    }
}

/**
 * 模块列表仓库：唯一入口是 `apd module list --json`（经 root-exec，契约 v1.1 §8.3）。
 * App 不直接读写 /data/adb，不猜路径。
 */
object ModuleRepository {

    private const val TAG = "Modules"

    fun list(context: android.content.Context, superKey: String): ModulesResult {
        val result = ApdClient.apd(
            context = context,
            superKey = superKey,
            subCommand = listOf("module", "list", "--json"),
            timeoutSec = 60,
        )
        val stdout = result.stdout
        if (result.timedOut) {
            return ModulesResult.failure(
                ApdClient.EXIT_TIMEOUT,
                "apd module list 超时（124）：AP 未在 60s 内返回",
                stdout,
                result.stderr,
            )
        }
        if (result.exitCode != ApdClient.EXIT_OK) {
            val message = when (result.exitCode) {
                ApdClient.EXIT_ESCALATION -> "密钥无效或需重新修补（125），不自动重试"
                ApdClient.EXIT_NOT_EXECUTABLE -> "AP 不可执行（126），需重新部署"
                ApdClient.EXIT_NOT_FOUND -> "AP 不存在（127），需先部署"
                ApdClient.EXIT_FAIL -> "apd 业务失败（1）：${result.stderr.take(400).ifBlank { "无 stderr" }}"
                else -> "apd module list 失败（exit=${result.exitCode}）"
            }
            AppLog.error(TAG, "$message；stdout=${stdout.take(500)}")
            return ModulesResult.failure(result.exitCode, message, stdout, result.stderr)
        }
        val parsed = MiniJson.arrayOfObjects(stdout).map { ModuleInfo.fromMap(it) }
        if (parsed.isEmpty()) {
            // 空数组 = 没有模块（正常）；非空但解析不出来 = schema 不符，需要暴露原文
            val looksEmpty = stdout.trim().isEmpty() || stdout.contains("[]")
            if (!looksEmpty) {
                AppLog.warn(TAG, "apd module list 输出无法解析为对象数组，原始输出：${stdout.take(1000)}")
                return ModulesResult(
                    ok = false,
                    exitCode = 0,
                    message = "输出无法解析为模块数组（schema 不符），已保留原始输出",
                    modules = emptyList(),
                    rawStdout = stdout,
                    rawStderr = result.stderr,
                )
            }
        }
        AppLog.info(TAG, "模块列表：${parsed.size} 个")
        return ModulesResult(true, 0, "共 ${parsed.size} 个模块", parsed, stdout, result.stderr)
    }

    /** 启用/禁用（写操作，同样经 root-exec；返回是否成功与完整输出） */
    fun setEnabled(context: android.content.Context, superKey: String, moduleId: String, enable: Boolean): ModulesResult {
        val action = if (enable) "enable" else "disable"
        val result = ApdClient.apd(
            context = context,
            superKey = superKey,
            subCommand = listOf("module", action, moduleId),
            timeoutSec = 60,
        )
        val ok = !result.timedOut && result.exitCode == ApdClient.EXIT_OK
        AppLog.info(TAG, "module $action $moduleId → exit=${result.exitCode} ${result.describe()}")
        return ModulesResult(
            ok = ok,
            exitCode = if (result.timedOut) ApdClient.EXIT_TIMEOUT else result.exitCode,
            message = if (ok) "已${if (enable) "启用" else "禁用"} $moduleId" else result.describe(),
            modules = emptyList(),
            rawStdout = result.stdout,
            rawStderr = result.stderr,
        )
    }
}
