package com.sproot.manager.core

import android.content.Context
import java.io.File

/**
 * App 自引导启动 AP 守护（人类指令 A，2026-11-10）。
 *
 * ## 为什么不需要 init_boot 自启注入
 * SP 的提权**不依赖 daemon**（内核直接比对密钥）；但 daemon 负责模块 service 阶段、
 * uid-listener、授权 socket、userspace boot-completed —— 需要有人把它拉起来。
 * SK 引擎没有 `call_usermodehelper` 之类的启动器，旧方案靠 init_boot ramdisk 的 rc
 * 拉起 daemon，属于绕路（APatch/SK 都不需要改 init_boot）。
 *
 * 因此改为：**App 自己通过 root-exec 提权把 daemon 拉起来**：
 * ```
 * App 冷启动 / 收到 BOOT_COMPLETED
 *   → apd selftest --json 探测（stage=daemon_ready 即在跑）
 *   → 不在跑且已部署 → apd root-exec -- /data/adb/apd --superkey-file /data/adb/sproot/key daemon
 *     （daemon 是常驻进程，root-exec 父进程不等待 → 启动确认后销毁父进程，daemon 留在后台）
 * ```
 *
 * 幂等：daemon 自身有 flock 单实例锁；App 侧也先探测，已在跑就不重复启动。
 * 失败必须可见：结果落 SharedPreferences，主页读取后以错误卡显示原因（不静默）。
 * init_boot 注入代码保留为**高级选项**，不再是自启的必要条件。
 */
object DaemonBootstrap {

    private const val TAG = "Bootstrap"
    private const val PREFS = "sproot_daemon_bootstrap"
    private const val DEPLOYED_DAEMON = "/data/adb/apd"
    private const val DEPLOYED_KEY = "/data/adb/sproot/key"

    /** 两次自动尝试的最小间隔（冷启动 + 切回前台可能连续触发） */
    private const val MIN_INTERVAL_MS = 15_000L

    enum class State { RUNNING, STARTED, NOT_DEPLOYED, FAILED }

    data class Result(
        val state: State,
        val message: String,
        val stage: String,
        val at: Long,
    ) {
        val alive: Boolean get() = state == State.RUNNING || state == State.STARTED
        val isNormalInitial: Boolean get() = state == State.NOT_DEPLOYED

        fun describe(): String = "state=${state.name} stage=$stage：$message"
    }

    /** 最近一次结果（主页只读展示；无记录返回 null）。 */
    fun load(context: Context): Result? {
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val state = prefs.getString("state", null) ?: return null
        val parsed = runCatching { State.valueOf(state) }.getOrNull() ?: return null
        return Result(
            state = parsed,
            message = prefs.getString("message", "") ?: "",
            stage = prefs.getString("stage", "") ?: "",
            at = prefs.getLong("at", 0L),
        )
    }

    private fun save(context: Context, result: Result): Result {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit()
            .putString("state", result.state.name)
            .putString("message", result.message)
            .putString("stage", result.stage)
            .putLong("at", result.at)
            .apply()
        AppLog.info(TAG, "daemon 自引导：${result.describe()}")
        return result
    }

    /**
     * 幂等确保 AP 守护在运行（**阻塞**，调用方放后台线程）。
     *
     * @param source 触发来源（cold_start / boot_completed / manual），仅用于日志
     */
    @Synchronized
    fun ensure(context: Context, source: String): Result {
        val now = System.currentTimeMillis()
        val cached = load(context)
        val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)
        val lastAt = prefs.getLong("at", 0L)
        // 刚确认过在跑 → 不重复探测（省掉一次 apd 进程）
        if (cached?.state == State.RUNNING && now - lastAt in 0..MIN_INTERVAL_MS) {
            return cached
        }

        // ① 探测：apd selftest 的 stage 权威判定（daemon_ready = 在跑）
        val probe = runCatching { ApdSelfTest.run(context) }.getOrElse { error ->
            return save(
                context,
                Result(
                    State.FAILED,
                    "自检异常：${error.javaClass.simpleName}: ${error.message}",
                    "unknown",
                    now,
                ),
            )
        }
        when (probe.stage) {
            "daemon_ready" -> return save(context, Result(State.RUNNING, "AP 守护已在运行", probe.stage, now))
            "undeployed", "escalation_failed", "no_ap_binary" -> return save(
                context,
                Result(State.NOT_DEPLOYED, probe.stageLabel, probe.stage, now),
            )
        }

        // ② 已部署才拉起（/data/adb/apd 是 deploy 产物）
        if (!File(DEPLOYED_DAEMON).isFile) {
            return save(
                context,
                Result(State.NOT_DEPLOYED, "AP 未部署（$DEPLOYED_DAEMON 不存在）", probe.stage, now),
            )
        }
        val key = runCatching { SecretStore.get(context).readRootKey() }
            .getOrNull()
            ?: runCatching { SecretStore.get(context).getOrCreateRootKey() }.getOrNull()
        if (key == null) {
            return save(context, Result(State.FAILED, "无可用 root 密钥：请到设置页重新生成", probe.stage, now))
        }

        // ③ 拉起：daemon 常驻，root-exec 父进程不等待 → 确认后销毁父进程（daemon 在独立进程组存活）
        val keyFile = writeKeyFile(context, key)
        var proc: Process? = null
        var lastProbe: ApdSelfTest.Result = probe
        try {
            val argv = listOf(
                ApdClient.apdBinary(context).absolutePath,
                "--superkey-file", keyFile.absolutePath,
                "root-exec", "--",
                DEPLOYED_DAEMON, "--superkey-file", DEPLOYED_KEY, "daemon",
            )
            AppLog.info(TAG, "启动 AP 守护（source=$source）：apd root-exec -- $DEPLOYED_DAEMON ... daemon")
            val builder = ProcessBuilder(argv)
            builder.redirectOutput(ProcessBuilder.Redirect.DISCARD)
            builder.redirectError(ProcessBuilder.Redirect.DISCARD)
            proc = builder.start()

            // 轮询确认（最多 ~2.4s；daemon 首次启动要做 auth store/事件阶段）
            repeat(6) { index ->
                Thread.sleep(400)
                val p = runCatching { ApdSelfTest.run(context) }.getOrNull() ?: return@repeat
                lastProbe = p
                if (p.stage == "daemon_ready") {
                    return save(context, Result(State.STARTED, "已自引导启动 AP 守护（pid 见自检）", p.stage, System.currentTimeMillis()))
                }
                if (p.stage == "escalation_failed" || p.stage == "no_ap_binary") {
                    return save(context, Result(State.NOT_DEPLOYED, p.stageLabel, p.stage, System.currentTimeMillis()))
                }
            }
            return save(
                context,
                Result(
                    State.FAILED,
                    "已尝试拉起 AP 守护，但自检仍为 ${lastProbe.stageLabel}（${lastProbe.summary}）；" +
                        "下次启动/开机将自动重试",
                    lastProbe.stage,
                    System.currentTimeMillis(),
                ),
            )
        } catch (t: Throwable) {
            return save(
                context,
                Result(
                    State.FAILED,
                    "拉起 AP 守护失败：${t.javaClass.simpleName}: ${t.message}",
                    lastProbe.stage,
                    System.currentTimeMillis(),
                ),
            )
        } finally {
            // daemon 在独立进程组：销毁 root-exec 父进程不影响 daemon
            runCatching { proc?.destroy() }
            runCatching { if (keyFile.exists() && !keyFile.delete()) AppLog.warn(TAG, "临时密钥文件删除失败：${keyFile.absolutePath}") }
        }
    }

    /** 0600 临时密钥文件（与 ApdClient.rootExec 同一 ABI：47 字符明文）。 */
    private fun writeKeyFile(context: Context, superKey: String): File {
        val dir = File(context.filesDir, "exec-keys").apply { mkdirs() }
        val file = File(dir, "bootstrap-${System.nanoTime()}.key")
        file.writeText(superKey)
        file.setReadable(false, false)
        file.setWritable(false, false)
        file.setExecutable(false, false)
        file.setReadable(true, true)
        file.setWritable(true, true)
        return file
    }
}
