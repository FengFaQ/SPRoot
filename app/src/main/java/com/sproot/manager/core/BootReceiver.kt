package com.sproot.manager.core

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/**
 * 开机自引导（人类指令 A）：收到 `BOOT_COMPLETED` 时确保 AP 守护在运行。
 *
 * 与冷启动同一入口 [DaemonBootstrap.ensure]（幂等，先探测后拉起）。
 * 用 `goAsync()` 把工作放到后台线程，避免超过广播主线程时限。
 */
class BootReceiver : BroadcastReceiver() {

    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != Intent.ACTION_BOOT_COMPLETED) return
        val pending = goAsync()
        Thread {
            try {
                runCatching { DaemonBootstrap.ensure(context, "boot_completed") }
                    .onFailure { AppLog.warn("Boot", "开机自引导失败：${it.message}") }
            } finally {
                pending.finish()
            }
        }.start()
    }
}
