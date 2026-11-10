package com.sproot.manager

import android.app.Application
import com.sproot.manager.core.AppLog
import com.sproot.manager.core.DaemonBootstrap

class SPRootApp : Application() {
    override fun onCreate() {
        super.onCreate()
        AppLog.init(this)
        AppLog.info("App", "SPRoot Manager 启动 v${BuildConfig.VERSION_NAME} (${BuildConfig.BUILD_TYPE})")
        // App 自引导（人类指令 A）：冷启动时确保 AP 守护在运行。
        // 内部先 apd selftest 探测（stage=daemon_ready 即在跑），未部署视为正常初始态；
        // 已部署但未运行 → 通过 root-exec 拉起 daemon。结果落 SharedPreferences，主页只读展示。
        Thread {
            runCatching { DaemonBootstrap.ensure(this, "cold_start") }
                .onFailure { AppLog.warn("App", "冷启动自引导异常（不影响使用）：${it.message}") }
        }.start()
    }
}
