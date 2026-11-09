package com.sproot.manager.ui.pages

import android.os.Build
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.WindowInsetsSides
import androidx.compose.foundation.layout.add
import androidx.compose.foundation.layout.displayCutout
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.sproot.manager.BuildConfig
import com.sproot.manager.core.ApdClient
import com.sproot.manager.core.ApdSelfTest
import com.sproot.manager.core.Capabilities
import com.sproot.manager.core.CapabilityItem
import com.sproot.manager.core.InstallState
import com.sproot.manager.core.PatchEngine
import com.sproot.manager.core.SecretStore
import com.sproot.manager.ui.components.BlurredBar
import com.sproot.manager.ui.components.StatusTag
import com.sproot.manager.ui.components.WarningCard
import com.sproot.manager.ui.components.WarningLevel
import com.sproot.manager.ui.components.rememberBlurBackdrop
import com.sproot.manager.ui.theme.LocalEnableBlur
import top.yukonga.miuix.kmp.basic.Card
import top.yukonga.miuix.kmp.basic.CardDefaults
import top.yukonga.miuix.kmp.basic.Icon
import top.yukonga.miuix.kmp.basic.IconButton
import top.yukonga.miuix.kmp.basic.MiuixScrollBehavior
import top.yukonga.miuix.kmp.basic.Scaffold
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.basic.TextButton
import top.yukonga.miuix.kmp.basic.TopAppBar
import top.yukonga.miuix.kmp.blur.LayerBackdrop
import top.yukonga.miuix.kmp.blur.layerBackdrop
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.basic.Check
import top.yukonga.miuix.kmp.icon.basic.Close
import top.yukonga.miuix.kmp.icon.extended.Info
import top.yukonga.miuix.kmp.icon.extended.Lock
import top.yukonga.miuix.kmp.icon.extended.Ok
import top.yukonga.miuix.kmp.icon.extended.Phone
import top.yukonga.miuix.kmp.icon.extended.Refresh
import top.yukonga.miuix.kmp.icon.extended.Report
import top.yukonga.miuix.kmp.icon.extended.Store
import top.yukonga.miuix.kmp.icon.extended.Tasks
import top.yukonga.miuix.kmp.icon.extended.Tune
import top.yukonga.miuix.kmp.icon.extended.Update
import top.yukonga.miuix.kmp.preference.ArrowPreference
import top.yukonga.miuix.kmp.theme.MiuixTheme
import top.yukonga.miuix.kmp.theme.MiuixTheme.colorScheme
import top.yukonga.miuix.kmp.utils.PressFeedbackType
import top.yukonga.miuix.kmp.utils.overScrollVertical
import top.yukonga.miuix.kmp.utils.scrollEndHaptic

/**
 * 主页 —— 完全照抄 KernelSU `ui/screen/home/HomeMiuix.kt` 的版式：
 *
 * ```
 * Scaffold(BlurredBar(TopAppBar))            ← 毛玻璃顶栏
 *  └─ LazyColumn(12dp 横向边距, 12dp 卡片间距)
 *      ├─ WarningCard(引擎/密钥/AP 未部署/上次失败)   ← 正常初始态用 Notice，不红
 *      ├─ StatusCard    大号状态卡（状态 + 引擎版本 + 模式标签 + 右下大图标）
 *      ├─ InfoCard      设备信息（图标 + 标题 + 值 两行式）
 *      ├─ CapabilityCard 环境自检逐项
 *      └─ NextStepCard  去修补页
 * ```
 *
 * 与 KSU 的差异只在数据源（KSU: Natives/LKM；本项目: PatchEngine/ApdClient/InstallState），
 * 版式、组件、间距、字号全部沿用 KSU miuix 版。
 */
@Composable
fun HomePager(
    bottomInnerPadding: Dp = 0.dp,
    isCurrentPage: Boolean = true,
    onGoPatch: () -> Unit = {},
) {
    val context = LocalContext.current
    val scrollBehavior = MiuixScrollBehavior()
    val enableBlur = LocalEnableBlur.current
    val backdrop = rememberBlurBackdrop(enableBlur)
    val blurActive = backdrop != null
    val barColor = if (blurActive) Color.Transparent else colorScheme.surface

    var snapshot by remember { mutableStateOf(HomeSnapshot()) }
    var refreshing by remember { mutableStateOf(false) }

    val refresh: () -> Unit = {
        if (!refreshing) {
            refreshing = true
            Thread({
                val loaded = HomeSnapshot.load(context)
                snapshot = loaded
                refreshing = false
            }, "home-refresh").start()
        }
    }

    // 进入本页（首次 / 切回）时刷新：修补/刷入/部署状态都可能在别的页改变
    LaunchedEffect(isCurrentPage) {
        if (isCurrentPage) refresh()
    }

    Scaffold(
        topBar = {
            BlurredBar(backdrop) {
                TopAppBar(
                    color = barColor,
                    title = "SPRoot",
                    actions = {
                        IconButton(onClick = { if (!refreshing) refresh() }) {
                            Icon(
                                imageVector = MiuixIcons.Refresh,
                                contentDescription = "刷新状态",
                                tint = colorScheme.onSurface,
                            )
                        }
                    },
                    scrollBehavior = scrollBehavior,
                )
            }
        },
        popupHost = { },
        contentWindowInsets = WindowInsets.systemBars.add(WindowInsets.displayCutout)
            .only(WindowInsetsSides.Horizontal),
    ) { innerPadding ->
        Box(modifier = if (backdrop != null) Modifier.layerBackdrop(backdrop) else Modifier) {
            LazyColumn(
                modifier = Modifier
                    .fillMaxHeight()
                    .scrollEndHaptic()
                    .overScrollVertical()
                    .nestedScroll(scrollBehavior.nestedScrollConnection)
                    .padding(horizontal = 12.dp),
                contentPadding = innerPadding,
                overscrollEffect = null,
            ) {
                item {
                    Column(
                        modifier = Modifier.padding(top = 12.dp),
                        horizontalAlignment = Alignment.CenterHorizontally,
                        verticalArrangement = Arrangement.spacedBy(12.dp),
                    ) {
                        HomeWarnings(snapshot = snapshot)
                        StatusCard(snapshot = snapshot, onClick = onGoPatch)
                        InfoCard(snapshot = snapshot)
                        CapabilityCard(
                            snapshot = snapshot,
                            refreshing = refreshing,
                            onRefresh = refresh,
                        )
                        NextStepCard(onGoPatch = onGoPatch)
                        Spacer(Modifier.height(bottomInnerPadding))
                    }
                }
            }
        }
    }
}

/** 主页快照：一次性在后台线程采集，避免主线程碰 Keystore / dlopen / ELF 探测 */
data class HomeSnapshot(
    val install: InstallState.Snapshot = InstallState.Snapshot(),
    val engine: PatchEngine.EngineStatus? = null,
    val apdReady: Boolean = false,
    val apdDetail: String = "",
    val apdSelfTest: ApdSelfTest.Result? = null,
    val keyReady: Boolean = false,
    val caps: List<CapabilityItem> = emptyList(),
    val loaded: Boolean = false,
) {
    companion object {
        fun load(context: android.content.Context): HomeSnapshot {
            val keyReady = runCatching { SecretStore.get(context).getOrCreateRootKey() }.isSuccess
            val engine = runCatching { PatchEngine.status(context) }.getOrNull()
            val apd = runCatching { ApdClient.status(context) }.getOrNull()
            val caps = runCatching { Capabilities.collect(context) }.getOrDefault(emptyList())
            return HomeSnapshot(
                install = InstallState.load(context),
                engine = engine,
                apdReady = apd?.ok == true,
                apdDetail = apd?.detail.orEmpty(),
                apdSelfTest = ApdSelfTest.load(context),
                keyReady = keyReady,
                caps = caps,
                loaded = true,
            )
        }
    }
}

/** 提示区：照 KSU 把"要用户注意的"放在状态卡上方，正常初始态用 Notice 而不是红。 */
@Composable
private fun HomeWarnings(snapshot: HomeSnapshot) {
    if (snapshot.loaded) {
        // ① AP 未部署 = 正常初始态（selftest 的 125 也在这一类），不能显示成失败
        val selfTest = snapshot.apdSelfTest
        val notDeployed = !snapshot.apdReady ||
            selfTest == null ||
            selfTest.isNormalInitialState
        if (notDeployed) {
            WarningCard(
                message = "尚未部署 AP 守护（需先刷入修补后的镜像）",
                level = WarningLevel.Notice,
            )
        }
        // ② 引擎缺失/未加载（阻断修补）
        val engine = snapshot.engine
        if (engine == null || !engine.loaded) {
            WarningCard(
                message = "修补引擎未就绪：" + (engine?.detail ?: "无法读取引擎状态"),
                level = WarningLevel.Error,
            )
        }
        // ③ 上次修补失败（错误常驻提示，详情在修补页）
        if (snapshot.install.status == InstallState.Status.ERROR) {
            WarningCard(
                message = "上次修补失败：" + (snapshot.install.note ?: "详情见修补页"),
                level = WarningLevel.Error,
            )
        }
        // ④ 密钥不可用
        if (!snapshot.keyReady) {
            WarningCard(
                message = "Root 密钥不可用：请到「设置 → Root 密钥」重新生成",
                level = WarningLevel.Error,
            )
        }
        // ⑤ 历史 48 字符密钥（仍在用前 47 位，建议重生成重刷）
        if (SecretStore.legacy48Detected) {
            WarningCard(
                message = "检测到历史 48 字符密钥，已取前 47 位使用；建议重新生成密钥并重新修补镜像",
                level = WarningLevel.Notice,
            )
        }
    }
}

/**
 * 状态卡 —— 照抄 KSU `StatusCard` 的成功态版式：
 * 左上 22sp 状态 + 15sp 版本行，左下 16sp 模式标签，右下 110dp 大图标（offset 27,31）。
 */
@Composable
private fun StatusCard(snapshot: HomeSnapshot, onClick: () -> Unit) {
    val status = snapshot.install.status
    val (container, content) = when (status) {
        InstallState.Status.ACTIVE -> colorScheme.secondaryContainer to colorScheme.onSecondaryContainer
        InstallState.Status.PATCHED_PENDING_FLASH,
        InstallState.Status.FLASHED_PENDING_REBOOT,
        -> colorScheme.tertiaryContainer to colorScheme.onTertiaryContainer

        InstallState.Status.ERROR -> colorScheme.errorContainer to colorScheme.onErrorContainer
        else -> colorScheme.surfaceContainer to colorScheme.onSurface
    }
    val icon: ImageVector = when (status) {
        InstallState.Status.ACTIVE -> MiuixIcons.Ok
        InstallState.Status.ERROR -> MiuixIcons.Report
        else -> MiuixIcons.Update
    }
    val iconTint = when (status) {
        InstallState.Status.ERROR -> colorScheme.error
        InstallState.Status.ACTIVE -> colorScheme.primary
        else -> content
    }
    val engineVersion = snapshot.engine?.version
    val apdMode = when {
        snapshot.apdReady && snapshot.apdSelfTest?.isNormalInitialState == false -> "AP 已部署"
        snapshot.apdReady -> "AP 待部署"
        else -> "AP 未部署"
    }

    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.defaultColors(color = container, contentColor = content),
        onClick = onClick,
        showIndication = true,
        pressFeedbackType = PressFeedbackType.Tilt,
    ) {
        Box(modifier = Modifier.fillMaxWidth().height(132.dp)) {
            Box(
                modifier = Modifier
                    .fillMaxSize()
                    .offset(27.dp, 31.dp),
                contentAlignment = Alignment.BottomEnd,
            ) {
                Icon(
                    modifier = Modifier.size(110.dp),
                    imageVector = icon,
                    tint = iconTint.copy(alpha = 0.22f),
                    contentDescription = null,
                )
            }
            Box(
                modifier = Modifier
                    .fillMaxSize()
                    .padding(16.dp, 10.dp),
                contentAlignment = Alignment.BottomStart,
            ) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(text = apdMode, fontSize = 16.sp, fontWeight = FontWeight.Medium)
                    Spacer(Modifier.size(8.dp))
                    StatusTag(
                        label = "SK",
                        backgroundColor = content.copy(alpha = 0.12f),
                        contentColor = content,
                    )
                }
            }
            Box(
                modifier = Modifier
                    .fillMaxSize()
                    .padding(16.dp, 14.dp),
                contentAlignment = Alignment.TopStart,
            ) {
                Column {
                    Text(
                        text = status.label,
                        fontSize = 22.sp,
                        fontWeight = FontWeight.SemiBold,
                    )
                    Spacer(Modifier.height(1.dp))
                    Text(
                        text = buildString {
                            append("引擎 ")
                            append(engineVersion ?: "未加载")
                            append(" · JNI ABI v")
                            append(PatchEngine.JNI_BINDING_VERSION)
                        },
                        fontSize = 15.sp,
                    )
                    Spacer(Modifier.height(2.dp))
                    Text(
                        text = status.hint,
                        fontSize = 13.sp,
                        maxLines = 2,
                    )
                }
            }
        }
    }
}

/** 设备信息卡 —— 照抄 KSU `InfoCard` 的图标 + 标题 + 值两行式。 */
@Composable
private fun InfoCard(snapshot: HomeSnapshot) {
    @Composable
    fun InfoText(icon: ImageVector, title: String, content: String, bottomPadding: Dp = 24.dp) {
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .padding(bottom = bottomPadding),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(
                imageVector = icon,
                contentDescription = title,
                modifier = Modifier
                    .padding(end = 12.dp)
                    .size(24.dp),
                tint = colorScheme.onSurface,
            )
            Column {
                Text(
                    text = title,
                    fontSize = MiuixTheme.textStyles.headline1.fontSize,
                    fontWeight = FontWeight.Medium,
                    color = colorScheme.onSurface,
                )
                Text(
                    text = content,
                    fontSize = MiuixTheme.textStyles.body2.fontSize,
                    color = colorScheme.onSurfaceVariantSummary,
                    modifier = Modifier.padding(top = 2.dp),
                )
            }
        }
    }

    Card(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(16.dp)) {
            InfoText(
                icon = MiuixIcons.Phone,
                title = "设备",
                content = "${Build.MANUFACTURER} ${Build.MODEL}（${Build.DEVICE}）",
            )
            InfoText(
                icon = MiuixIcons.Info,
                title = "系统",
                content = "Android ${Build.VERSION.RELEASE}（API ${Build.VERSION.SDK_INT}）",
            )
            InfoText(
                icon = MiuixIcons.Tasks,
                title = "内核",
                content = System.getProperty("os.version") ?: "未知",
            )
            InfoText(
                icon = MiuixIcons.Store,
                title = "管理器版本",
                content = "${BuildConfig.VERSION_NAME}（${BuildConfig.BUILD_TYPE}）",
            )
            InfoText(
                icon = MiuixIcons.Lock,
                title = "Root 密钥",
                content = if (snapshot.keyReady) {
                    "47 字符（内核 ABI 47+NUL），Keystore 加密存储"
                } else {
                    "不可用"
                },
            )
            InfoText(
                icon = MiuixIcons.Tune,
                title = "ABI",
                content = Build.SUPPORTED_ABIS.joinToString(", "),
                bottomPadding = 0.dp,
            )
        }
    }
}

/** 环境自检卡：逐项 ✓/✗（阻断项标红），底部可手动复检。 */
@Composable
private fun CapabilityCard(
    snapshot: HomeSnapshot,
    refreshing: Boolean,
    onRefresh: () -> Unit,
) {
    val blocking = snapshot.caps.count { !it.ok && it.blocking }
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(16.dp)) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    text = "环境自检",
                    fontSize = MiuixTheme.textStyles.headline1.fontSize,
                    fontWeight = FontWeight.Medium,
                    color = colorScheme.onSurface,
                    modifier = Modifier.weight(1f),
                )
                if (snapshot.loaded) {
                    Text(
                        text = if (blocking == 0) "全部通过" else "阻断 $blocking 项",
                        fontSize = 13.sp,
                        color = if (blocking == 0) colorScheme.primary else colorScheme.error,
                    )
                }
            }
            Spacer(Modifier.height(10.dp))
            if (!snapshot.loaded) {
                Text(
                    text = "检测中…",
                    fontSize = 13.sp,
                    color = colorScheme.onSurfaceVariantSummary,
                )
            } else {
                snapshot.caps.forEach { item ->
                    Row(
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(vertical = 5.dp),
                        verticalAlignment = Alignment.Top,
                    ) {
                        Icon(
                            imageVector = if (item.ok) MiuixIcons.Basic.Check else MiuixIcons.Basic.Close,
                            contentDescription = null,
                            modifier = Modifier
                                .padding(end = 8.dp, top = 2.dp)
                                .size(16.dp),
                            tint = when {
                                item.ok -> colorScheme.primary
                                item.blocking -> colorScheme.error
                                else -> colorScheme.onSurfaceVariantActions
                            },
                        )
                        Column {
                            Text(
                                text = item.label,
                                fontSize = 14.sp,
                                color = if (item.ok || !item.blocking) colorScheme.onSurface else colorScheme.error,
                            )
                            Text(
                                text = item.detail,
                                fontSize = 12.sp,
                                color = colorScheme.onSurfaceVariantSummary,
                            )
                        }
                    }
                }
                Spacer(Modifier.height(4.dp))
                TextButton(
                    text = if (refreshing) "复检中…" else "重新自检",
                    onClick = onRefresh,
                    enabled = !refreshing,
                    colors = top.yukonga.miuix.kmp.basic.ButtonDefaults.textButtonColorsPrimary(),
                )
            }
        }
    }
}

/** 下一步引导：照 KSU 的 `ArrowPreference` 入口卡（右侧箭头由组件自带）。 */
@Composable
private fun NextStepCard(onGoPatch: () -> Unit) {
    Card(modifier = Modifier.fillMaxWidth()) {
        ArrowPreference(
            title = "去修补 boot.img",
            summary = "选择本机 boot.img → 进程内修补 → 导出可刷产物（全程无需 root）；" +
                "产物与刷入指引都在修补页",
            onClick = onGoPatch,
        )
    }
}
