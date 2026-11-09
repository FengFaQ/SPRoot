package com.sproot.manager.ui.theme

import android.content.Context

/**
 * 主题偏好持久化（SharedPreferences）。
 *
 * 与 KernelSU 的 `SettingsRepository` 等价，但只存 UI 需要的两项：
 *  - [loadColorMode]/[saveColorMode]：跟随系统 / 浅色 / 深色；
 *  - [loadKeyColor]/[saveKeyColor]：强调色 ARGB（0 = 默认）。
 */
object ThemePrefs {

    private const val PREFS = "sproot_theme"
    private const val KEY_COLOR_MODE = "color_mode"
    private const val KEY_KEY_COLOR = "key_color"

    fun loadColorMode(context: Context): Int =
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getInt(KEY_COLOR_MODE, ColorMode.SYSTEM.value)

    fun saveColorMode(context: Context, value: Int) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putInt(KEY_COLOR_MODE, value).apply()
    }

    fun loadKeyColor(context: Context): Int =
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).getInt(KEY_KEY_COLOR, 0)

    fun saveKeyColor(context: Context, value: Int) {
        context.getSharedPreferences(PREFS, Context.MODE_PRIVATE).edit().putInt(KEY_KEY_COLOR, value).apply()
    }

    fun load(context: Context): AppSettings = AppSettings(
        colorMode = ColorMode.fromValue(loadColorMode(context)),
        keyColor = loadKeyColor(context),
    )
}
