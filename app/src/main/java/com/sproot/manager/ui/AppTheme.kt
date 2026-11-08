package com.sproot.manager.ui

import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import top.yukonga.miuix.kmp.theme.ColorSchemeMode
import top.yukonga.miuix.kmp.theme.MiuixTheme
import top.yukonga.miuix.kmp.theme.ThemeController

/**
 * 全局主题。
 *
 * - 默认跟随系统亮暗（ColorSchemeMode.System）；
 * - 设置页可强制亮/暗（0=跟随系统，1=亮，2=暗）；
 * - 不启用 Monet：管理器 UI 需要稳定的状态色（成功绿/失败红），壁纸取色会削弱语义。
 */
@Composable
fun AppTheme(
    colorMode: Int = 0,
    content: @Composable () -> Unit,
) {
    val controller = remember(colorMode) {
        when (colorMode) {
            1 -> ThemeController(ColorSchemeMode.Light)
            2 -> ThemeController(ColorSchemeMode.Dark)
            else -> ThemeController(ColorSchemeMode.System)
        }
    }
    MiuixTheme(controller = controller, content = content)
}
