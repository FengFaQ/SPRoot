package com.sproot.manager

import android.app.Application
import com.sproot.manager.core.ApdSelfTest
import com.sproot.manager.core.AppLog

class SPRootApp : Application() {
    override fun onCreate() {
        super.onCreate()
        AppLog.init(this)
        AppLog.info("App", "SPRoot Manager 启动 v${BuildConfig.VERSION_NAME} (${BuildConfig.BUILD_TYPE})")
        // 首启自检：后台跑一次 apd selftest --json（stage=undeployed 视为正常状态）。
        // 结果落 SharedPreferences，主页/模块页只读展示，不阻塞启动。
        Thread {
            runCatching {
                val result = ApdSelfTest.run(this)
                AppLog.info("App", "首启 AP 自检：${result.describe()}")
            }.onFailure {
                AppLog.warn("App", "首启 AP 自检异常（不影响使用）：${it.message}")
            }
        }.start()
    }
}
