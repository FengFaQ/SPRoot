package com.sproot.manager.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import com.sproot.manager.ui.components.BottomBarMiuix
import com.sproot.manager.ui.components.NavigationRailMiuix
import com.sproot.manager.ui.components.rememberBlurBackdrop
import com.sproot.manager.ui.components.rememberMainPagerState
import com.sproot.manager.ui.components.shouldShowSplitPane
import com.sproot.manager.ui.pages.AboutPager
import com.sproot.manager.ui.pages.HomePager
import com.sproot.manager.ui.pages.ModulePager
import com.sproot.manager.ui.pages.PatchPager
import com.sproot.manager.ui.pages.SettingsPager
import com.sproot.manager.ui.theme.AppSettings
import com.sproot.manager.ui.theme.LocalEnableBlur
import com.sproot.manager.ui.theme.SPRootTheme
import com.sproot.manager.ui.theme.ThemePrefs
import top.yukonga.miuix.kmp.basic.Scaffold
import top.yukonga.miuix.kmp.blur.layerBackdrop

/** 主分页数量（主页 / 修补 / 模块 / 设置）。 */
private object MainPagerConfig {
    const val PAGE_COUNT = 4
}

/**
 * 应用根 —— 骨架照抄 KernelSU `ui/MainActivity.kt` 的 miuix 分支：
 *
 * ```
 * MiuixTheme(ThemeController)                         ← ui/theme/MiuixTheme.kt
 *  ├─ 关于页（二级页面：BackHandler + 全屏 AboutPager）  ← 对应 KSU 的 Route.About
 *  └─ Scaffold(bottomBar = BlurredBar(NavigationBar))  （或分屏 SideRail）
 *      └─ HorizontalPager(4 页)                        ← 手势横滑 + 底栏联动
 *          └─ 每页自带 Scaffold + BlurredBar(TopAppBar) ← 与 KSU 页面结构一致
 * ```
 *
 * 去掉的 KSU 复杂度（均为与 miuix 版式无关的业务能力）：
 * Material/Miuix 双模式、浮动底栏、导航角标、Navigation3 全量路由。
 * 关于页用状态驱动 + BackHandler（KSU 用 miuix-nav NavDisplay，本项目只一个二级页，
 * 状态驱动可少引入 kotlinx-serialization 编译器插件，CI 风险更低）。
 */
@Composable
fun AppRoot(modifier: Modifier = Modifier) {
    val context = LocalContext.current
    var appSettings by remember { mutableStateOf(ThemePrefs.load(context)) }
    var showAbout by rememberSaveable { mutableStateOf(false) }

    SPRootTheme(appSettings = appSettings) {
        if (showAbout) {
            BackHandler(enabled = true) { showAbout = false }
            AboutPager(onBack = { showAbout = false })
        } else {
            MainScreen(
                modifier = modifier,
                appSettings = appSettings,
                onSettingsChange = { next ->
                    appSettings = next
                    ThemePrefs.saveColorMode(context, next.colorMode.value)
                    ThemePrefs.saveKeyColor(context, next.keyColor)
                },
                onOpenAbout = { showAbout = true },
            )
        }
    }
}

@Composable
private fun MainScreen(
    modifier: Modifier = Modifier,
    appSettings: AppSettings,
    onSettingsChange: (AppSettings) -> Unit,
    onOpenAbout: () -> Unit,
) {
    val useNavigationRail = shouldShowSplitPane()
    val pagerState = rememberPagerState(initialPage = 0) { MainPagerConfig.PAGE_COUNT }
    val mainPagerState = rememberMainPagerState(
        pagerState = pagerState,
        animatePageChanges = !useNavigationRail,
    )
    val blurBackdrop = rememberBlurBackdrop(LocalEnableBlur.current)

    // 分页内返回：不在主页时先回主页（KSU MainScreenBackHandler 的简化版）
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
                val isCurrentPage = page == mainPagerState.selectedPage
                when (page) {
                    0 -> HomePager(
                        bottomInnerPadding = bottomInnerPadding,
                        isCurrentPage = isCurrentPage,
                        onGoPatch = { mainPagerState.animateToPage(1) },
                    )

                    1 -> PatchPager(
                        bottomInnerPadding = bottomInnerPadding,
                        isCurrentPage = isCurrentPage,
                    )

                    2 -> ModulePager(
                        bottomInnerPadding = bottomInnerPadding,
                        isCurrentPage = isCurrentPage,
                    )

                    else -> SettingsPager(
                        appSettings = appSettings,
                        onSettingsChange = onSettingsChange,
                        bottomInnerPadding = bottomInnerPadding,
                        isCurrentPage = isCurrentPage,
                        onOpenAbout = onOpenAbout,
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
