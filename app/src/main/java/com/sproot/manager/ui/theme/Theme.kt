package com.sproot.manager.ui.theme

import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.ReadOnlyComposable
import androidx.compose.runtime.staticCompositionLocalOf

/**
 * 主题模式（照抄 KernelSU 的 miuix 版，但固定 miuix、去掉 Material/Monet 双模式复杂度）。
 *
 * 人类指令：只抄 miuix 实现，不要 `UiMode` 的 Material/Miuix 双模式切换。
 * 因此这里只保留「跟随系统 / 浅色 / 深色」三态 + 强调色；Monet 取色不去复制
 * （KSU 的 Monet 走 materialkolor/material3，本项目不引入 Material 3）。
 */
enum class ColorMode(val value: Int) {
    SYSTEM(0),
    LIGHT(1),
    DARK(2);

    companion object {
        fun fromValue(value: Int): ColorMode = entries.find { it.value == value } ?: SYSTEM
    }

    val isSystem: Boolean get() = this == SYSTEM
    val isDark: Boolean get() = this == DARK
}

/** 设置项「主题」的完整状态。 */
data class AppSettings(
    val colorMode: ColorMode = ColorMode.SYSTEM,
    /** ARGB；0 = 不使用固定强调色（miuix 默认灰阶） */
    val keyColor: Int = 0,
)

val LocalColorMode = staticCompositionLocalOf { 0 }

/** 顶栏/底栏毛玻璃开关（固定开启；低版本自动退化，见 BlurExt）。 */
val LocalEnableBlur = staticCompositionLocalOf { true }

@Composable
@ReadOnlyComposable
fun isInDarkTheme(): Boolean {
    return when (LocalColorMode.current) {
        ColorMode.LIGHT.value -> false
        ColorMode.DARK.value -> true
        else -> isSystemInDarkTheme()
    }
}
