package com.sproot.manager.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.add
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.displayCutout
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import com.sproot.manager.ui.components.BottomBarMiuix
import com.sproot.manager.ui.components.NavigationRailMiuix
import com.sproot.manager.ui.components.rememberBlurBackdrop
import com.sproot.manager.ui.components.rememberMainPagerState
import com.sproot.manager.ui.components.shouldShowSplitPane
import com.sproot.manager.ui.pages.HomePage
import com.sproot.manager.ui.pages.ModulesPage
import com.sproot.manager.ui.pages.PatchPage
import com.sproot.manager.ui.pages.SettingsPage
import com.sproot.manager.ui.theme.AppSettings
import com.sproot.manager.ui.theme.LocalEnableBlur
import com.sproot.manager.ui.theme.SPRootTheme
import com.sproot.manager.ui.theme.ThemePrefs
import top.yukonga.miuix.kmp.basic.MiuixScrollBehavior
import top.yukonga.miuix.kmp.basic.Scaffold
import top.yukonga.miuix.kmp.basic.TopAppBar
import top.yukonga.miuix.kmp.blur.LayerBackdrop
import top.yukonga.miuix.kmp.blur.layerBackdrop
import top.yukonga.miuix.kmp.theme.MiuixTheme

/** 主分页数量（主页 / 修补 / 模块 / 设置）。 */
private object MainPagerConfig {
    const val PAGE_COUNT = 4
}

/**
 * 应用根 —— 骨架照抄 KernelSU `ui/MainActivity.kt` 的 miuix 分支：
 *
 * ```
 * MiuixTheme(ThemeController)                 ← ui/theme/MiuixTheme.kt
 *  └─ Scaffold(bottomBar = BlurredBar(NavigationBar))  （或分屏 SideRail）
 *      └─ HorizontalPager(4 页)               ← 手势横滑 + 底栏联动
 *          └─ 每页自带 Scaffold + BlurredBar(TopAppBar)  ← 与 KSU 页面结构一致
 * ```
 *
 * 去掉了 KSU 的 Material/Miuix 双模式、浮动底栏、角标与导航栈（本项目 4 个平级页 +
 * 关于页为二级页面，二级页面在后续提交接入 miuix-nav）。
 */
@Composable
fun AppRoot(modifier: Modifier = Modifier) {
    val context = LocalContext.current
    var appSettings by remember { mutableStateOf(ThemePrefs.load(context)) }

    SPRootTheme(appSettings = appSettings) {
        MainScreen(
            modifier = modifier,
            appSettings = appSettings,
            onSettingsChange = { next ->
                appSettings = next
                ThemePrefs.saveColorMode(context, next.colorMode.value)
                ThemePrefs.saveKeyColor(context, next.keyColor)
            },
        )
    }
}

@Composable
private fun MainScreen(
    modifier: Modifier = Modifier,
    appSettings: AppSettings,
    onSettingsChange: (AppSettings) -> Unit,
) {
    val useNavigationRail = shouldShowSplitPane()
    val pagerState = rememberPagerState(initialPage = 0) { MainPagerConfig.PAGE_COUNT }
    val mainPagerState = rememberMainPagerState(
        pagerState = pagerState,
        animatePageChanges = !useNavigationRail,
    )
    val blurBackdrop = rememberBlurBackdrop(LocalEnableBlur.current)

    // 分页内返回：不在主页时先回主页（照抄 KSU MainScreenBackHandler 的 miuix 简化版）
    BackHandler(enabled = mainPagerState.selectedPage != 0) {
        mainPagerState.animateToPage(0)
    }
    LaunchedEffect(pagerState.currentPage) {
        mainPagerState.syncPage()
    }

    val pagerContent = @Composable { bottomInnerPadding: androidx.compose.ui.unit.Dp ->
        Box(
            modifier = Modifier
                .fillMaxSize()
                .then(if (blurBackdrop != null) Modifier.layerBackdrop(blurBackdrop) else Modifier)
                .padding(bottom = bottomInnerPadding),
        ) {
            HorizontalPager(
                state = pagerState,
                modifier = Modifier.fillMaxSize(),
                beyondViewportPageCount = 1,
                overscrollEffect = null,
            ) { page ->
                when (page) {
                    0 -> HomeTab(blurBackdrop, onGoPatch = { mainPagerState.animateToPage(1) })
                    1 -> PatchTab(blurBackdrop)
                    2 -> ModuleTab(blurBackdrop)
                    else -> SettingsTab(
                        backdrop = blurBackdrop,
                        appSettings = appSettings,
                        onSettingsChange = onSettingsChange,
                    )
                }
            }
        }
    }

    if (useNavigationRail) {
        Scaffold(modifier = modifier) { _ ->
            Row {
                NavigationRailMiuix(mainState = mainPagerState)
                Box(modifier = Modifier.weight(1f)) {
                    pagerContent(0.dp)
                }
            }
        }
    } else {
        Scaffold(
            modifier = modifier,
            bottomBar = {
                Box(modifier = Modifier.fillMaxWidth()) {
                    BottomBarMiuix(
                        blurBackdrop = blurBackdrop,
                        mainState = mainPagerState,
                        modifier = Modifier.align(Alignment.BottomCenter),
                    )
                }
            },
        ) { innerPadding ->
            pagerContent(innerPadding.calculateBottomPadding())
        }
    }
}

/**
 * 单个分页的骨架：Scaffold + 毛玻璃 TopAppBar + 边到边 insets。
 * 与 KSU 的 `HomeMiuix/ModuleMiuix/SettingsMiuix` 顶部结构一一对应。
 */
@Composable
private fun TabScaffold(
    title: String,
    backdrop: LayerBackdrop?,
    content: @Composable (PaddingValues) -> Unit,
) {
    val scrollBehavior = MiuixScrollBehavior()
    val barColor = if (backdrop != null) Color.Transparent else MiuixTheme.colorScheme.surface
    Scaffold(
        topBar = {
            com.sproot.manager.ui.components.BlurredBar(backdrop) {
                TopAppBar(
                    color = barColor,
                    title = title,
                    scrollBehavior = scrollBehavior,
                )
            }
        },
        contentWindowInsets = WindowInsets.systemBars.add(WindowInsets.displayCutout)
            .only(WindowInsetsSides.Horizontal),
    ) { innerPadding ->
        content(innerPadding)
    }
}

@Composable
private fun HomeTab(backdrop: LayerBackdrop?, onGoPatch: () -> Unit) {
    TabScaffold(title = "SPRoot", backdrop = backdrop) { inner ->
        HomePage(modifier = Modifier.padding(inner), onGoPatch = onGoPatch)
    }
}

@Composable
private fun PatchTab(backdrop: LayerBackdrop?) {
    TabScaffold(title = "修补", backdrop = backdrop) { inner ->
        PatchPage(modifier = Modifier.padding(inner))
    }
}

@Composable
private fun ModuleTab(backdrop: LayerBackdrop?) {
    TabScaffold(title = "模块", backdrop = backdrop) { inner ->
        ModulesPage(modifier = Modifier.padding(inner))
    }
}

@Composable
private fun SettingsTab(
    backdrop: LayerBackdrop?,
    appSettings: AppSettings,
    onSettingsChange: (AppSettings) -> Unit,
) {
    TabScaffold(title = "设置", backdrop = backdrop) { inner ->
        SettingsPage(
            modifier = Modifier.padding(inner),
            colorMode = appSettings.colorMode.value,
            onColorModeChange = { onSettingsChange(appSettings.copy(colorMode = com.sproot.manager.ui.theme.ColorMode.fromValue(it))) },
        )
    }
}
