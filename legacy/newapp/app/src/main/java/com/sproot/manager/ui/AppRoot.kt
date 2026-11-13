package com.sproot.manager.ui

import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.foundation.layout.padding
import androidx.compose.ui.Modifier
import com.sproot.manager.ui.pages.HomePage
import com.sproot.manager.ui.pages.ModulesPage
import com.sproot.manager.ui.pages.PatchPage
import com.sproot.manager.ui.pages.SettingsPage
import top.yukonga.miuix.kmp.basic.NavigationBar
import top.yukonga.miuix.kmp.basic.NavigationBarItem
import top.yukonga.miuix.kmp.basic.Scaffold
import top.yukonga.miuix.kmp.basic.SmallTopAppBar
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.extended.Home
import top.yukonga.miuix.kmp.icon.extended.Layers
import top.yukonga.miuix.kmp.icon.extended.Settings
import top.yukonga.miuix.kmp.icon.extended.Update

private data class AppTab(val label: String, val icon: androidx.compose.ui.graphics.vector.ImageVector)

/**
 * 应用根：四个页面（主页 / 修补 / 模块 / 设置）。
 *
 * 导航刻意不用 navigation-compose：只有 4 个平级页面，状态驱动即可，
 * 少一个依赖 = CI 少一类解析失败面。
 */
@Composable
fun AppRoot(modifier: Modifier = Modifier) {
    var selectedTab by rememberSaveable { mutableIntStateOf(0) }
    var colorMode by rememberSaveable { mutableIntStateOf(0) }

    val tabs = remember {
        listOf(
            AppTab("主页", MiuixIcons.Home),
            AppTab("修补", MiuixIcons.Update),
            AppTab("模块", MiuixIcons.Layers),
            AppTab("设置", MiuixIcons.Settings),
        )
    }

    // 主题模式在设置页可改，重包一层 MiuixTheme 使切换立即生效
    val pageTitle = tabs[selectedTab].label
    AppThemeShell(colorMode) {
        Scaffold(
            modifier = modifier,
            topBar = { SmallTopAppBar(title = "SPRoot · $pageTitle") },
            bottomBar = {
                NavigationBar {
                    tabs.forEachIndexed { index, tab ->
                        NavigationBarItem(
                            selected = selectedTab == index,
                            onClick = { selectedTab = index },
                            icon = tab.icon,
                            label = tab.label,
                        )
                    }
                }
            },
        ) { padding ->
            val contentModifier = Modifier.padding(padding)
            when (selectedTab) {
                0 -> HomePage(contentModifier, onGoPatch = { selectedTab = 1 })
                1 -> PatchPage(contentModifier)
                2 -> ModulesPage(contentModifier)
                else -> SettingsPage(
                    modifier = contentModifier,
                    colorMode = colorMode,
                    onColorModeChange = { colorMode = it },
                )
            }
        }
    }
}

@Composable
private fun AppThemeShell(colorMode: Int, content: @Composable () -> Unit) {
    AppTheme(colorMode = colorMode, content = content)
}
