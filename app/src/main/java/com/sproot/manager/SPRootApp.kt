package com.sproot.manager

import android.app.Application
import com.sproot.manager.core.AppLog
import com.sproot.manager.core.CrashHandler
import com.sproot.manager.core.DaemonBootstrap

class SPRootApp : Application() {
    override fun onCreate() {
        super.onCreate()
        // ① 崩溃兜底最先装：之后任何初始化异常都能落证据（后台线程只记录不闪退）
        runCatching { CrashHandler.install(this) }
            .onFailure { android.util.Log.e("SPRootApp", "崩溃兜底安装失败", it) }
        runCatching { AppLog.init(this) }
            .onFailure { android.util.Log.e("SPRootApp", "日志初始化失败", it) }
        runCatching {
            AppLog.info("App", "SPRoot Manager 启动 v${BuildConfig.VERSION_NAME} (${BuildConfig.BUILD_TYPE})")
        }
        // 启动期清理 root-exec 临时密钥残留（ApdClient/DaemonBootstrap 的 finally 删除失败时
        // 承诺"下次启动清理"——兑现它；正常删除成功的场景此目录为空，成本可忽略）。
        runCatching {
            val dir = java.io.File(filesDir, "exec-keys")
            val stale = dir.listFiles()?.filter { it.isFile } ?: emptyList()
            stale.forEach { f ->
                if (!f.delete()) AppLog.warn("App", "临时密钥残留删除失败：${f.absolutePath}")
            }
            if (stale.isNotEmpty()) AppLog.warn("App", "启动清理 exec-keys 残留 ${stale.size} 个")
        }.onFailure { android.util.Log.e("SPRootApp", "exec-keys 启动清理失败", it) }
        // PATCHING 过期调和（BUG-APP-10）：修补中进程被杀 → 状态机永久卡"修补中"。
        // 启动时 PATCHING 且超过 30 分钟未更新 → 降级 UNPATCHED（重新修补即可）。
        runCatching {
            if (com.sproot.manager.core.InstallState.reconcileStalePatching(this)) {
                AppLog.warn("App", "检测到过期的 PATCHING 状态，已调和（详见 InstallState 日志）")
            }
        }.onFailure { android.util.Log.e("SPRootApp", "PATCHING 过期调和失败", it) }
        // App 自引导（人类指令 A）：冷启动时确保 AP 守护在运行。
        // 内部先 apd selftest 探测（stage=daemon_ready 即在跑），未部署视为正常初始态；
        // 已部署但未运行 → 通过 root-exec 拉起 daemon。结果落 SharedPreferences，主页只读展示。
        // ① 全捕获：任何异常都不得冒泡到线程默认处理器/主线程。
        Thread {
            runCatching { DaemonBootstrap.ensure(this, "cold_start") }
                .onFailure { AppLog.warn("App", "冷启动自引导异常（不影响使用）：${it.message}") }
        }.apply { isDaemon = true }.start()
    }
}
