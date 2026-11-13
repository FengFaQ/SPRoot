package com.sproot.manager.core

import android.content.Context

/**
 * AP 首启自检（`apd selftest --json`）的结果模型与缓存。
 *
 * 语义（契约 v1.1 + sys-guard 自检约定）：
 *  - `stage=undeployed` 时进程 **exit 0**（"AP 未部署" 是正常状态，不是错误）→ UI 显示"未部署"；
 *  - **exit 125（提权被内核静默拒绝）在首启时同样是正常初始态**：说明内核尚未刷入修补镜像，
 *    不是"自检失败"。文案必须是「尚未部署 AP 守护（需先刷入修补后的镜像）」，且不得标红；
 *  - 其它 stage（如 `deployed` / `daemon_ready` / `needs_repair`）按 stage 原样展示；
 *  - 失败（非 0 且非上述正常初始态）→ 显示"自检失败"并保留原始输出。
 *
 * 结果写入 SharedPreferences，供主页/设置页**只读展示**（避免每次进页面都起进程）。
 */
object ApdSelfTest {

    private const val PREFS = "sproot_apd_selftest"
    private const val TAG = "ApdSelfTest"

    data class Result(
        val stage: String,
        val exitCode: Int,
        val ok: Boolean,
        val summary: String,
        val raw: String,
        val at: Long,
    ) {
        val stageLabel: String
            get() = when (stage) {
                "undeployed" -> "AP 未部署"
                "deployed" -> "已部署"
                "daemon_ready" -> "守护已就绪"
                "needs_repair" -> "需修复（重新部署）"
                "no_ap_binary" -> "AP 二进制缺失"
                // 首启最常见：客户端有密钥但内核未刷补丁镜像 → apd root-exec 返回 125。
                // 这是**正常初始态**，不能写成"密钥无效/失败"。
                "escalation_failed" -> "尚未部署 AP 守护（需先刷入修补后的镜像）"
                "timeout" -> "自检超时"
                "parse_failed" -> "自检输出无法解析"
                "unknown" -> "未知状态"
                else -> stage.ifBlank { "未知" }
            }

        /** 正常初始态：AP 未部署（exit 0）或内核未刷补丁导致提权被拒（exit 125） */
        val isNormalInitialState: Boolean
            get() = stage == "undeployed" || stage == "escalation_failed"

        fun describe(): String = buildString {
            append("stage=").append(stage).append("（").append(stageLabel).append("），exit=")
            append(exitCode).append("，")
            append(
                when {
                    isNormalInitialState -> "正常初始态（无需处理；刷入修补镜像并部署 AP 后可用）"
                    ok -> "自检通过"
                    else -> "自检未通过"
                },
            )
            append("，").append(summary)
        }
    }

    fun load(context: Context): Result? {
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val stage = prefs.getString("stage", null) ?: return null
        return Result(
            stage = stage,
            exitCode = prefs.getInt("exit", -1),
            ok = prefs.getBoolean("ok", false),
            summary = prefs.getString("summary", "") ?: "",
            raw = prefs.getString("raw", "") ?: "",
            at = prefs.getLong("at", 0L),
        )
    }

    fun save(context: Context, result: Result) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
            .putString("stage", result.stage)
            .putInt("exit", result.exitCode)
            .putBoolean("ok", result.ok)
            .putString("summary", result.summary)
            .putString("raw", result.raw)
            .putLong("at", result.at)
            .apply()
    }

    /**
     * 执行自检（阻塞，调用方放后台线程）。
     * 有密钥时经 root-exec（完整能力）；无密钥/未部署时回退直接 exec（只读探测）。
     */
    fun run(context: Context, timeoutSec: Int = 60): Result {
        val key = runCatching { SecretStore.get(context).readRootKey() }.getOrNull()
        val viaRootExec = key != null
        val execResult = if (key != null) {
            ApdClient.rootExec(
                context = context,
                superKey = key,
                command = listOf(ApdClient.apdBinary(context).absolutePath, "selftest", "--json"),
                timeoutSec = timeoutSec,
            )
        } else {
            ApdClient.runDirect(context, listOf("selftest", "--json"), timeoutSec = minOf(timeoutSec, 10))
        }
        val interpreted = interpret(execResult, viaRootExec)
        // escalation_failed（125）时把 KeyMatch 的精确诊断并入 summary：
        // 用户看到的是"密钥不一致 → 重新修补刷入"而不是笼统的"密钥无效"。
        val enriched = if (interpreted.stage == "escalation_failed") {
            val hint = runCatching { KeyMatch.escalationExplanation(context) }.getOrNull()
            if (hint != null) interpreted.copy(summary = hint.take(500)) else interpreted
        } else {
            interpreted
        }
        save(context, enriched)
        AppLog.info(TAG, enriched.describe())
        return enriched
    }

    private fun interpret(result: ApdClient.Result, viaRootExec: Boolean): Result {
        val stdout = result.stdout
        val fields = MiniJson.objectOf(stdout)
        val stage = fields["stage"] ?: when {
            result.timedOut -> "timeout"
            result.exitCode == ApdClient.EXIT_NOT_FOUND -> "no_ap_binary"
            result.exitCode == ApdClient.EXIT_ESCALATION && viaRootExec -> "escalation_failed"
            stdout.isBlank() -> "parse_failed"
            else -> "unknown"
        }
        val ok = when {
            result.timedOut -> false
            // 契约：未部署是正常状态（exit 0），不是错误
            stage == "undeployed" -> true
            result.exitCode == ApdClient.EXIT_OK -> true
            else -> false
        }
        val summary = fields["summary"] ?: fields["detail"] ?: result.describe()
        return Result(
            stage = stage,
            exitCode = if (result.timedOut) ApdClient.EXIT_TIMEOUT else result.exitCode,
            ok = ok,
            summary = summary.take(500),
            raw = (stdout + if (result.stderr.isNotBlank()) "\n[stderr]\n" + result.stderr else "").take(4000),
            at = System.currentTimeMillis(),
        )
    }
}
