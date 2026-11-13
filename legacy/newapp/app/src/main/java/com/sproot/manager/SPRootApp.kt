package com.sproot.manager

import android.app.Application
import com.sproot.manager.core.AppLog

class SPRootApp : Application() {
    override fun onCreate() {
        super.onCreate()
        AppLog.init(this)
        AppLog.info("App", "SPRoot Manager 启动 v${BuildConfig.VERSION_NAME} (${BuildConfig.BUILD_TYPE})")
    }
}
