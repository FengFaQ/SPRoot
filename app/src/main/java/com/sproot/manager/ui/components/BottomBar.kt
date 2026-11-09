package com.sproot.manager.ui.components

import androidx.compose.foundation.pager.PagerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalResources
import androidx.compose.ui.platform.LocalWindowInfo
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.job
import kotlinx.coroutines.launch
import top.yukonga.miuix.kmp.basic.NavigationBar
import top.yukonga.miuix.kmp.basic.NavigationBarItem
import top.yukonga.miuix.kmp.basic.NavigationItem
import top.yukonga.miuix.kmp.basic.NavigationRail
import top.yukonga.miuix.kmp.basic.NavigationRailItem
import top.yukonga.miuix.kmp.basic.NavigationRailValue
import top.yukonga.miuix.kmp.basic.rememberNavigationRailState
import top.yukonga.miuix.kmp.blur.LayerBackdrop
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.extended.Home
import top.yukonga.miuix.kmp.icon.extended.Layers
import top.yukonga.miuix.kmp.icon.extended.Lock
import top.yukonga.miuix.kmp.icon.extended.Settings
import top.yukonga.miuix.kmp.theme.MiuixTheme
import top.yukonga.miuix.kmp.utils.springAnimateToPage

/**
 * 主分页状态：照抄 KernelSU `ui/component/bottombar/BottomBar.kt` 的 [MainPagerState]。
 * 自管 selectedPage，避免 pager 惯性滑动与点击切换之间的抖动。
 */
class MainPagerState(
    val pagerState: PagerState,
    private val coroutineScope: CoroutineScope,
    private val animatePageChanges: Boolean,
) {
    var selectedPage by mutableIntStateOf(pagerState.currentPage)
        private set

    var isNavigating by mutableStateOf(false)
        private set

    private var navJob: Job? = null

    fun animateToPage(targetIndex: Int) {
        if (targetIndex == selectedPage) return

        navJob?.cancel()

        selectedPage = targetIndex
        isNavigating = true

        navJob = coroutineScope.launch {
            val myJob = coroutineContext.job
            try {
                if (animatePageChanges) {
                    pagerState.springAnimateToPage(targetIndex)
                } else {
                    pagerState.scrollToPage(targetIndex)
                }
            } finally {
                if (navJob == myJob) {
                    isNavigating = false
                    if (pagerState.currentPage != targetIndex) {
                        selectedPage = pagerState.currentPage
                    }
                }
            }
        }
    }

    fun syncPage() {
        if (!isNavigating && selectedPage != pagerState.currentPage) {
            selectedPage = pagerState.currentPage
        }
    }
}

@Composable
fun rememberMainPagerState(
    pagerState: PagerState,
    coroutineScope: CoroutineScope = rememberCoroutineScope(),
    animatePageChanges: Boolean = true,
): MainPagerState {
    return remember(pagerState, coroutineScope, animatePageChanges) {
        MainPagerState(pagerState, coroutineScope, animatePageChanges)
    }
}

/**
 * 底部导航目的地（3 项）。
 *
 * 人类指令（2026-11-10）：「修补页面不应该独立出来，将修补页面集成在点击主页未修补后跳转」。
 * 因此修补/刷入是**主页触发的二级页**（KSU 的 Home→Install/Flash 模型），不是独立 Tab。
 */
enum class BottomBarDestination(val label: String, val icon: ImageVector) {
    Home("主页", MiuixIcons.Home),
    Auth("授权", MiuixIcons.Lock),

    Module("模块", MiuixIcons.Layers),
    Setting("设置", MiuixIcons.Settings),
}

/** 照抄 KernelSU `BottomBarMiuix`（去掉浮动底栏/角标等 KSU 专有能力）。 */
@Composable
fun BottomBarMiuix(
    blurBackdrop: LayerBackdrop?,
    mainState: MainPagerState,
    modifier: Modifier = Modifier,
) {
    val items = BottomBarDestination.entries.map { destination ->
        NavigationItem(label = destination.label, icon = destination.icon)
    }
    BlurredBar(blurBackdrop) {
        NavigationBar(
            modifier = modifier,
            color = if (blurBackdrop != null) Color.Transparent else MiuixTheme.colorScheme.surface,
            content = {
                items.forEachIndexed { index, item ->
                    NavigationBarItem(
                        modifier = Modifier.weight(1f),
                        icon = item.icon,
                        label = item.label,
                        selected = mainState.selectedPage == index,
                        onClick = { mainState.animateToPage(index) },
                    )
                }
            },
        )
    }
}

/** 分屏（平板/横屏）导航栏：照抄 KernelSU `NavigationRailMiuix`。 */
@Composable
fun NavigationRailMiuix(
    mainState: MainPagerState,
    modifier: Modifier = Modifier,
    initialExpanded: Boolean = true,
) {
    val state = rememberNavigationRailState(
        initialValue = if (initialExpanded) NavigationRailValue.Expanded else NavigationRailValue.Collapsed,
    )
    NavigationRail(
        modifier = modifier,
        state = state,
        color = MiuixTheme.colorScheme.surface,
        expandContentDescription = "展开侧栏",
        collapseContentDescription = "收起侧栏",
    ) {
        BottomBarDestination.entries.forEachIndexed { index, destination ->
            NavigationRailItem(
                selected = mainState.selectedPage == index,
                onClick = { mainState.animateToPage(index) },
                icon = destination.icon,
                label = destination.label,
            )
        }
    }
}

/** 分屏阈值：照抄 KernelSU `util/WindowSize.kt` 的 shouldShowSplitPane()。 */
@Composable
fun shouldShowSplitPane(): Boolean {
    val windowInfo = LocalWindowInfo.current
    val deviceDensity = LocalResources.current.displayMetrics.density
    val widthDp = windowInfo.containerSize.width / deviceDensity
    val heightDp = windowInfo.containerSize.height / deviceDensity
    return widthDp >= 840f || (widthDp >= 600f && heightDp / widthDp < 1.2f)
}
