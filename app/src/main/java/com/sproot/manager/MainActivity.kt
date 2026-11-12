package com.sproot.manager

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.SystemBarStyle
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.ui.Modifier
import com.sproot.manager.ui.AppRoot

/**
 * 唯一 Activity —— 照抄 KernelSU `MainActivity` 的 miuix 分支（去掉 Navigation3 外壳与
 * Material 分支后的最小骨架）：
 *  - 边到边 + 透明状态栏/导航栏，明暗图标随主题；
 *  - 内容由 [AppRoot] 负责（MiuixTheme + 4 页 HorizontalPager + 毛玻璃底栏）。
 *
 * KSU 的 splash / NavigationEvent / ViewModel 均未引入（本项目无对应业务）。
 */
class MainActivity : ComponentActivity() {

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // ① 首屏构建失败要落证据（CrashHandler 写 crash 报告）。这里不用 runCatching 吞掉：
        //    吞掉 = 永久白屏。改为捕获后设一个兜底错误 UI（compose 组合异常在首帧渲染时抛出，
        //    setContent 本身几乎不抛；真正会抛的是 setContent 内部 lambda 的首次组合，
        //    它发生在 onContent 签名之外，因此这里再兜一层 Recomposer 外的 setText 兜底）。
        runCatching {
        setContent {
            val darkMode = isSystemInDarkTheme()
            DisposableEffect(darkMode) {
                enableEdgeToEdge(
                    statusBarStyle = SystemBarStyle.auto(
                        android.graphics.Color.TRANSPARENT,
                        android.graphics.Color.TRANSPARENT,
                    ) { darkMode },
                    navigationBarStyle = SystemBarStyle.auto(
                        android.graphics.Color.TRANSPARENT,
                        android.graphics.Color.TRANSPARENT,
                    ) { darkMode },
                )
                window.isNavigationBarContrastEnforced = false
                onDispose { }
            }
            AppRoot(modifier = Modifier.fillMaxSize())
        }
        }.onFailure { error ->
            // 首屏组合失败：落证据 + 设兜底错误 UI（不让用户面对白屏无提示）。
            com.sproot.manager.core.AppLog.error("Activity", "首屏 setContent 失败（兜底 UI）", error)
            runCatching {
                android.widget.TextView(this).apply {
                    text = "SPRoot 首屏加载失败\n\n${error.javaClass.simpleName}: ${error.message}\n\n" +
                        "崩溃报告已写入 filesDir/crash/，请反馈日志。"
                    setPadding(48, 96, 48, 48)
                    setTextSize(16f)
                }.also { setContentView(it) }
            }.onFailure {
                // 兜底也失败（Activity 已异常）→ 重新抛出走系统崩溃流程（CrashHandler 已落报告）
                throw error
            }
        }
    }
}
