package com.sproot.manager.ui.theme

import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.remember
import androidx.compose.ui.graphics.Color
import top.yukonga.miuix.kmp.theme.ColorSchemeMode
import top.yukonga.miuix.kmp.theme.LocalContentColor
import top.yukonga.miuix.kmp.theme.MiuixTheme
import top.yukonga.miuix.kmp.theme.ThemeColorSpec
import top.yukonga.miuix.kmp.theme.ThemeController
import top.yukonga.miuix.kmp.theme.ThemePaletteStyle

/**
 * 全局主题（照抄 KernelSU `ui/theme/MiuixTheme.kt` 的 miuix 分支）。
 *
 * 与 KSU 的差异（均为「只抄 miuix 版」的直接结果）：
 *  - 不引入 materialkolor / material3，因此没有 Monet 取色与 PaletteStyle 选择；
 *  - 调色板固定 `TonalSpot` + `Spec2025`（miuix 默认观感），可用强调色覆盖 seed；
 *  - 状态栏/导航栏图标明暗由 MainActivity 的 `enableEdgeToEdge(SystemBarStyle.auto)` 处理，
 *    这里不再操作 WindowInsetsController（iOS 式边到边由 Activity 统一负责）。
 */
@Composable
fun SPRootTheme(
    appSettings: AppSettings = AppSettings(),
    content: @Composable () -> Unit,
) {
    val systemDarkTheme = androidx.compose.foundation.isSystemInDarkTheme()
    val darkTheme = appSettings.colorMode.isDark ||
        (appSettings.colorMode.isSystem && systemDarkTheme)
    val keyColor = appSettings.keyColor.takeIf { it != 0 }?.let { Color(it) }

    val controller = remember(appSettings.colorMode, appSettings.keyColor, darkTheme) {
        ThemeController(
            when (appSettings.colorMode) {
                ColorMode.SYSTEM -> ColorSchemeMode.System
                ColorMode.LIGHT -> ColorSchemeMode.Light
                ColorMode.DARK -> ColorSchemeMode.Dark
            },
            keyColor = keyColor,
            isDark = darkTheme,
            paletteStyle = ThemePaletteStyle.TonalSpot,
            colorSpec = ThemeColorSpec.Spec2025,
        )
    }

    MiuixTheme(
        controller = controller,
        content = {
            CompositionLocalProvider(
                LocalContentColor provides MiuixTheme.colorScheme.onBackground,
                LocalColorMode provides appSettings.colorMode.value,
            ) {
                content()
            }
        },
    )
}
