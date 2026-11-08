package com.sproot.manager.core

/**
 * 模块列表模型与解析（`apd module list --json`）。
 *
 * ## Schema v1 定稿（sys-guard 2026-11-08 确认，v2 commit 8b28e87）
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
 *     "update_pending": false,
 *     "install_status": "installed"   // v2 待定字段（客户端占位解析，缺失忽略）
 *   }
 * ]
 * ```
 * 解析策略：**容忍缺字段**（缺失显示 "—"），未知字段忽略；解析失败时把**原始 stdout 全文**
 * 展示给用户（绝不静默吞掉，旧版"闪一下"的反面）。
 *
 * ## 写操作（契约 v1.1 §8.3 + sys-guard 计划中的子命令）
 * - `apd module install <zip路径> [--json]`（路径由 App 复制到私有 cache 后传入）
 * - `apd module uninstall <id> [--json]`
 * - `apd module enable|disable <id>`
 * 全部经 `root-exec`；子命令的 `--json` 输出 schema 未定稿时，客户端**只依赖 exit code**
 * 并把原始 stdout/stderr 常驻展示（schema 定稿后再解析进度/失败码）。
 */
data class ModuleInfo(
    val id: String,
    val name: String,
    val version: String,
    val versionCode: String,
    val author: String,
    val description: String,
    val enabled: Boolean,
    val updatePending: Boolean,
    val installStatus: String,
) {
    companion object {
        /** 从 apd 的 JSON 对象解析；缺字段不外抛 */
        fun fromMap(map: Map<String, String>): ModuleInfo = ModuleInfo(
            id = map["id"] ?: map["module_id"] ?: map["name"] ?: "",
            name = map["name"] ?: map["id"] ?: "(未命名模块)",
            version = map["version"] ?: map["versionName"] ?: "—",
            versionCode = map["versionCode"] ?: map["version_code"] ?: "—",
            author = map["author"] ?: "—",
            description = map["description"] ?: map["summary"] ?: "",
            enabled = map.bool("enabled", default = true),
            updatePending = map.bool("update_pending", default = false) ||
                map.bool("updatePending", default = false),
            installStatus = map["install_status"] ?: map["installStatus"] ?: "",
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

/** 模块操作（列表/安装/卸载/启停）统一结果；成功失败都带原始输出 */
data class ModulesResult(
    val ok: Boolean,
    val exitCode: Int,
    val message: String,
    val modules: List<ModuleInfo> = emptyList(),
    val rawStdout: String = "",
    val rawStderr: String = "",
) {
    companion object {
        fun failure(exitCode: Int, message: String, stdout: String, stderr: String): ModulesResult =
            ModulesResult(false, exitCode, message, emptyList(), stdout, stderr)
    }
}

/**
 * 模块仓库：唯一入口是 `apd module ...`（经 root-exec，契约 v1.1 §8.3）。
 * App 不直接读写 /data/adb，不猜路径。
 */
object ModuleRepository {

    private const val TAG = "Modules"

    fun list(context: android.content.Context, superKey: String): ModulesResult {
        val result = run(context, superKey, listOf("module", "list", "--json"), timeoutSec = 60)
        val stdout = result.stdout
        if (result.timedOut) {
            return ModulesResult.failure(ApdClient.EXIT_TIMEOUT, "apd module list 超时（124）：AP 未在 60s 内返回", stdout, result.stderr)
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

    /** 启用/禁用（root-exec） */
    fun setEnabled(context: android.content.Context, superKey: String, moduleId: String, enable: Boolean): ModulesResult {
        val action = if (enable) "enable" else "disable"
        val result = run(context, superKey, listOf("module", action, moduleId), timeoutSec = 60)
        val ok = !result.timedOut && result.exitCode == ApdClient.EXIT_OK
        AppLog.info(TAG, "module $action $moduleId → exit=${result.exitCode} ${result.describe()}")
        return ModulesResult(
            ok = ok,
            exitCode = if (result.timedOut) ApdClient.EXIT_TIMEOUT else result.exitCode,
            message = if (ok) "已${if (enable) "启用" else "禁用"} $moduleId" else result.describe(),
            rawStdout = result.stdout,
            rawStderr = result.stderr,
        )
    }

    /**
     * 安装模块（zip 已由调用方复制到 App 私有 cache 并校验过）。
     * 大模块安装可能耗时数十秒，超时 300s；进度依赖 apd 的 --json（schema 待定，暂只显示阶段文案）。
     */
    fun install(context: android.content.Context, superKey: String, zip: java.io.File): ModulesResult {
        val result = run(
            context,
            superKey,
            listOf("module", "install", zip.absolutePath, "--json"),
            timeoutSec = 300,
        )
        val ok = !result.timedOut && result.exitCode == ApdClient.EXIT_OK
        val parsedMessage = MiniJson.objectOf(result.stdout)["message"]
            ?: MiniJson.objectOf(result.stdout)["status"]
        val message = when {
            result.timedOut -> "安装超时（124）：apd 未在 300s 内完成，请查看日志"
            ok -> "安装完成${parsedMessage?.let { "：$it" } ?: ""}"
            result.exitCode == ApdClient.EXIT_ESCALATION -> "安装失败：密钥无效或需重新修补（125），不自动重试"
            result.exitCode == ApdClient.EXIT_NOT_FOUND -> "安装失败：AP 不存在（127），需先部署"
            result.exitCode == ApdClient.EXIT_NOT_EXECUTABLE -> "安装失败：AP 不可执行（126），需重新部署"
            else -> "安装失败（exit=${result.exitCode}）：${result.stderr.take(500).ifBlank { "无 stderr" }}"
        }
        AppLog.info(TAG, "module install exit=${result.exitCode} ok=$ok")
        return ModulesResult(
            ok = ok,
            exitCode = if (result.timedOut) ApdClient.EXIT_TIMEOUT else result.exitCode,
            message = message,
            rawStdout = result.stdout,
            rawStderr = result.stderr,
        )
    }

    /** 卸载模块（root-exec） */
    fun uninstall(context: android.content.Context, superKey: String, moduleId: String): ModulesResult {
        val result = run(context, superKey, listOf("module", "uninstall", moduleId), timeoutSec = 120)
        val ok = !result.timedOut && result.exitCode == ApdClient.EXIT_OK
        AppLog.info(TAG, "module uninstall $moduleId → exit=${result.exitCode} ${result.describe()}")
        return ModulesResult(
            ok = ok,
            exitCode = if (result.timedOut) ApdClient.EXIT_TIMEOUT else result.exitCode,
            message = if (ok) "已卸载 $moduleId" else result.describe(),
            rawStdout = result.stdout,
            rawStderr = result.stderr,
        )
    }

    /** 校验用户选择的 zip：魔数 PK\x03\x04、非空、大小上限 */
    fun validateZip(file: java.io.File): String? {
        if (!file.isFile) return "文件不存在"
        if (file.length() <= 0) return "文件为空"
        if (file.length() > 512L * 1024 * 1024) return "文件过大（>512MB）"
        val ok = runCatching {
            java.io.RandomAccessFile(file, "r").use { raf ->
                val buf = ByteArray(4)
                raf.readFully(buf)
                buf[0] == 0x50.toByte() && buf[1] == 0x4B.toByte() && // 'P' 'K'
                    (buf[2] == 0x03.toByte() || buf[2] == 0x05.toByte() || buf[2] == 0x07.toByte())
            }
        }.getOrDefault(false)
        return if (ok) null else "不是有效 zip（缺少 PK 头；模块包应为 .zip）"
    }

    private fun run(
        context: android.content.Context,
        superKey: String,
        subCommand: List<String>,
        timeoutSec: Int,
    ): ApdClient.Result = ApdClient.apd(
        context = context,
        superKey = superKey,
        subCommand = subCommand,
        timeoutSec = timeoutSec,
    )
}
