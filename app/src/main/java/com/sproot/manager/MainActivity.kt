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
        // ① 首屏构建失败要落证据（崩溃兜底会写 crash 报告），不让"首次打开闪退"无迹可查
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
        }.onFailure {
            com.sproot.manager.core.AppLog.error("Activity", "首屏 setContent 失败", it)
        }
    }
}
