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
