package com.sproot.manager.core

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit

/**
 * 开机自引导（人类指令 A）：收到 `BOOT_COMPLETED` 时确保 AP 守护在运行。
 *
 * 与冷启动同一入口 [DaemonBootstrap.ensure]（幂等，先探测后拉起）。
 * 用 `goAsync()` 把工作放到后台线程，避免超过广播主线程时限。
 *
 * ## 硬预算（8s）
 * goAsync 的广播时限约 10s。DaemonBootstrap.ensure 单次探测最坏 ~20s
 * （rootExec 外层超时 = selftest timeoutSec 5 + margin 15），6 次轮询最坏 ~2 分钟——
 * 直接调 ensure 会把 receiver 拖过时限（系统判 ANR / 冻结进程）。
 * 因此这里用墙钟预算：8s 内没跑完就放弃本次（结果记 FAILED，
 * 下次开机/冷启动自动重试），并在 finally 里立刻 pending.finish()。
 */
class BootReceiver : BroadcastReceiver() {

    companion object {
        /** 广播路径的墙钟预算（< goAsync ~10s 时限，留 2s 给 finish() 与进程收尾） */
        const val BOOT_BUDGET_MS = 8_000L
    }

    override fun onReceive(context: Context, intent: Intent) {
        // ① 全捕获：onReceive 里任何异常都不能抛（会 ANR/崩溃）；耗时工作一律 goAsync + 后台线程
        runCatching {
            if (intent.action != Intent.ACTION_BOOT_COMPLETED) return
            val pending = goAsync()
            val done = CountDownLatch(1)
            val finished = AtomicBoolean(false)
            Thread {
                try {
                    runCatching { DaemonBootstrap.ensure(context, "boot_completed", startAttempts = 1) }
                        .onFailure { AppLog.warn("Boot", "开机自引导失败：${it.message}") }
                } finally {
                    finished.set(true)
                    done.countDown()
                }
            }.apply { isDaemon = true }.start()
            Thread {
                // ② 硬预算：8s 内 ensure 没跑完就先 finish 广播（超时限会被判 ANR）；
                //    工作线程随后自然结束（daemon 线程，不阻塞进程退出），结果由下次启动重试覆盖。
                val inTime = done.await(BOOT_BUDGET_MS, TimeUnit.MILLISECONDS)
                if (!inTime && !finished.get()) {
                    AppLog.warn("Boot", "开机自引导超过 ${BOOT_BUDGET_MS}ms 预算，先放行广播（下次启动重试）")
                }
                runCatching { pending.finish() }
            }.apply { isDaemon = true }.start()
        }.onFailure {
            AppLog.error("Boot", "onReceive 异常（已吞掉，不闪退）：${it.message}", it)
        }
    }
}
