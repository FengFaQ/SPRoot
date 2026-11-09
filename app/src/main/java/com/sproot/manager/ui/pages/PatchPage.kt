package com.sproot.manager.ui.pages

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
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
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.sproot.manager.core.AppLog
import com.sproot.manager.core.BackupStore
import com.sproot.manager.core.Capabilities
import com.sproot.manager.core.EnginePhase
import com.sproot.manager.core.InstallState
import com.sproot.manager.core.PatchMode
import com.sproot.manager.core.PatchPipeline
import com.sproot.manager.core.PatchStage
import com.sproot.manager.core.PatchUiState
import com.sproot.manager.ui.components.BlurredBar
import com.sproot.manager.ui.components.ConfirmDialog
import com.sproot.manager.ui.components.LogPanel
import com.sproot.manager.ui.components.copyToClipboard
import com.sproot.manager.ui.components.maskSecret
import com.sproot.manager.ui.components.WarningCard
import com.sproot.manager.ui.components.WarningLevel
import com.sproot.manager.ui.components.rememberBlurBackdrop
import com.sproot.manager.ui.theme.LocalEnableBlur
import top.yukonga.miuix.kmp.basic.BasicComponent
import top.yukonga.miuix.kmp.basic.ButtonDefaults
import top.yukonga.miuix.kmp.basic.Card
import top.yukonga.miuix.kmp.basic.CardDefaults
import top.yukonga.miuix.kmp.basic.Icon
import top.yukonga.miuix.kmp.basic.LinearProgressIndicator
import top.yukonga.miuix.kmp.basic.MiuixScrollBehavior
import top.yukonga.miuix.kmp.basic.Scaffold
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.basic.TextButton
import top.yukonga.miuix.kmp.basic.TopAppBar
import top.yukonga.miuix.kmp.blur.layerBackdrop
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.extended.Copy
import top.yukonga.miuix.kmp.icon.extended.ExpandLess
import top.yukonga.miuix.kmp.icon.extended.ExpandMore
import top.yukonga.miuix.kmp.icon.extended.Folder
import top.yukonga.miuix.kmp.icon.extended.Report
import top.yukonga.miuix.kmp.icon.extended.Reset
import top.yukonga.miuix.kmp.icon.extended.Update
import top.yukonga.miuix.kmp.preference.CheckboxPreference
import top.yukonga.miuix.kmp.theme.MiuixTheme
import top.yukonga.miuix.kmp.theme.MiuixTheme.colorScheme
import top.yukonga.miuix.kmp.utils.overScrollVertical
import top.yukonga.miuix.kmp.utils.scrollEndHaptic

/**
 * 修补页 —— 版式照抄 KernelSU 的 `InstallMiuix + FlashMiuix` 组合：
 *
 * ```
 * Scaffold(BlurredBar(TopAppBar("修补")))
 *  └─ LazyColumn(12dp 横距, 12dp 卡片间距)
 *      ├─ Card 选择镜像        BasicComponent(File 图标 + 文件名 + 右侧箭头)
 *      ├─ Card 修补模式        CheckboxPreference × N（KSU 的选择项样式）
 *      ├─ Card 修补进度        百分比 + LinearProgressIndicator + 引擎 phase/pct + 开始按钮
 *      ├─ Card 错误（常驻）    WarningCard 配色 + 可展开详情 + 复制全部
 *      ├─ Card 产物            sha256/等长/AVB/rc/密钥
 *      ├─ Card 刷入指引        FlashMiuix 命令卡（等宽命令 + 复制）
 *      ├─ Card 两级状态        我已刷入 → 已重启生效
 *      └─ LogPanel             常驻日志（缺陷 ⑤ 的修复点，不随失败卸载）
 * ```
 *
 * 三条不可妥协的 UI 约定（对应旧版"点击修补一闪即退"）：
 *  1) 错误一旦发生，errorTitle/errorDetail **常驻**，只有用户点"重置"才消失；
 *  2) 日志面板始终在页面底部，且日志已同步落盘（设置页可见路径）；
 *  3) 进度 = 阶段百分比 + 引擎 NDJSON 的 pct/phase（引擎事件单调递增，绝不回退）。
 */
@Composable
fun PatchPager(
    bottomInnerPadding: Dp = 0.dp,
    isCurrentPage: Boolean = true,
) {
    val context = LocalContext.current
    val state = PatchPipeline.uiState.value
    val scrollBehavior = MiuixScrollBehavior()
    val enableBlur = LocalEnableBlur.current
    val backdrop = rememberBlurBackdrop(enableBlur)
    val barColor = if (backdrop != null) Color.Transparent else colorScheme.surface

    var showKey by remember { mutableStateOf(false) }
    var showErrorDetail by remember { mutableStateOf(true) }
    var showResetConfirm by remember { mutableStateOf(false) }

    val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri: Uri? ->
        if (uri == null) {
            AppLog.warn("PatchUI", "用户取消了文件选择")
            return@rememberLauncherForActivityResult
        }
        runCatching {
            context.contentResolver.takePersistableUriPermission(
                uri,
                android.content.Intent.FLAG_GRANT_READ_URI_PERMISSION,
            )
        }
        PatchPipeline.selectInput(uri, queryDisplayName(context, uri))
    }
    // P2 双镜像：init_boot.img（可选；ramdisk 的权威落点）
    val initBootPicker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri: Uri? ->
        if (uri == null) {
            AppLog.warn("PatchUI", "用户取消了 init_boot 文件选择")
            return@rememberLauncherForActivityResult
        }
        runCatching {
            context.contentResolver.takePersistableUriPermission(
                uri,
                android.content.Intent.FLAG_GRANT_READ_URI_PERMISSION,
            )
        }
        PatchPipeline.selectInitBootInput(uri, queryDisplayName(context, uri))
    }

    ConfirmDialog(
        show = showResetConfirm,
        title = "重置修补页状态",
        message = "将清除当前错误与进度（日志与备份保留）。修补进行中无法重置。",
        confirmText = "重置",
        onConfirm = {
            PatchPipeline.reset()
            showResetConfirm = false
        },
        onDismiss = { showResetConfirm = false },
    )

    Scaffold(
        topBar = {
            BlurredBar(backdrop) {
                TopAppBar(
                    color = barColor,
                    title = "修补",
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
                        verticalArrangement = Arrangement.spacedBy(12.dp),
                    ) {
                        SelectImageCard(
                            state = state,
                            onPick = { picker.launch(arrayOf("*/*")) },
                            onPickInitBoot = { initBootPicker.launch(arrayOf("*/*")) },
                            onReset = { showResetConfirm = true },
                        )
                        if (state.sourceUri != null && state.initBootSourceUri == null) {
                            WarningCard(
                                message = "未选择 init_boot.img：将无法注入开机自启" +
                                    "（本机 ramdisk 可能位于 init_boot，例如魅族 21）",
                                level = WarningLevel.Notice,
                            )
                        }
                        ModeCard(state = state)
                        ProgressCard(state = state, onStart = { PatchPipeline.start(context) })
                        if (state.hasError) {
                            ErrorCard(
                                state = state,
                                showDetail = showErrorDetail,
                                onToggleDetail = { showErrorDetail = !showErrorDetail },
                                onCopyAll = {
                                    context.copyToClipboard(
                                        "错误详情",
                                        (state.errorTitle ?: "") + "\n\n" + (state.errorDetail ?: "") +
                                            "\n\n---- 日志 ----\n" + AppLog.dumpText() +
                                            "\n\n日志文件：" + (state.logPath ?: "无"),
                                    )
                                },
                            )
                        }
                        if (state.stage == PatchStage.DONE && state.outputPath != null) {
                            SuccessCard(
                                state = state,
                                showKey = showKey,
                                onToggleKey = { showKey = !showKey },
                                onCopyKey = { key -> context.copyToClipboard("Root 密钥", key) },
                                onCopyCommand = { cmd -> context.copyToClipboard("刷入命令", cmd) },
                            )
                        }
                        LogPanel()
                        Spacer(Modifier.height(bottomInnerPadding))
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------- 选择镜像

@Composable
private fun SelectImageCard(
    state: PatchUiState,
    onPick: () -> Unit,
    onPickInitBoot: () -> Unit,
    onReset: () -> Unit,
) {
    Card(modifier = Modifier.fillMaxWidth()) {
        BasicComponent(
            title = "选择 boot.img",
            summary = state.sourceName
                ?: "必须是本机 boot.img（Android boot image）；kernel 本体/压缩包会被拒绝",
            onClick = onPick,
            startAction = {
                Icon(
                    imageVector = MiuixIcons.Folder,
                    contentDescription = null,
                    tint = colorScheme.onSurface,
                    modifier = Modifier.padding(end = 12.dp),
                )
            },
        )
        BasicComponent(
            title = "选择 init_boot.img（可选，强烈建议）",
            summary = state.initBootSourceName
                ?: "ramdisk 在 init_boot 的机型（如魅族 21）必须选它，否则开机自启无法注入",
            onClick = onPickInitBoot,
            startAction = {
                Icon(
                    imageVector = MiuixIcons.Folder,
                    contentDescription = null,
                    tint = colorScheme.onSurface,
                    modifier = Modifier.padding(end = 12.dp),
                )
            },
        )
        state.sourceUri?.let { uri ->
            BasicComponent(
                title = "已选镜像 URI",
                summary = uri,
                onClick = onPick,
            )
        }
        BasicComponent(
            title = "重置修补页状态",
            summary = "清除进度与错误（日志/备份保留）",
            onClick = onReset,
            startAction = {
                Icon(
                    imageVector = MiuixIcons.Reset,
                    contentDescription = null,
                    tint = colorScheme.onSurfaceVariantActions,
                    modifier = Modifier.padding(end = 12.dp),
                )
            },
        )
    }
}

// ---------------------------------------------------------------- 修补模式

@Composable
private fun ModeCard(state: PatchUiState) {
    Card(modifier = Modifier.fillMaxWidth()) {
        Column {
            PatchMode.entries.filter { it.enabled }.forEach { mode ->
                CheckboxPreference(
                    title = mode.label,
                    summary = mode.description,
                    checked = state.mode == mode,
                    onCheckedChange = { PatchPipeline.selectMode(mode) },
                )
            }
        }
    }
}

// ---------------------------------------------------------------- 进度

@Composable
private fun ProgressCard(state: PatchUiState, onStart: () -> Unit) {
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(16.dp)) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    text = "修补进度",
                    fontSize = MiuixTheme.textStyles.headline1.fontSize,
                    fontWeight = FontWeight.Medium,
                    color = colorScheme.onSurface,
                    modifier = Modifier.weight(1f),
                )
                Text(
                    text = "${(state.progress * 100).toInt()}%",
                    fontSize = 15.sp,
                    fontWeight = FontWeight.Medium,
                    color = colorScheme.primary,
                )
            }
            Spacer(Modifier.height(10.dp))
            LinearProgressIndicator(
                progress = state.progress.coerceIn(0f, 1f),
                modifier = Modifier.fillMaxWidth(),
            )
            Spacer(Modifier.height(10.dp))
            Text(
                text = "当前阶段：${state.stage.title}",
                fontSize = 14.sp,
                color = colorScheme.onSurface,
            )
            if (state.enginePct >= 0) {
                Text(
                    text = "引擎：${EnginePhase.label(state.enginePhase)}（${state.enginePct}%）",
                    fontSize = 13.sp,
                    color = colorScheme.onSurfaceVariantSummary,
                )
            } else if (state.phase.isNotBlank()) {
                Text(
                    text = state.phase,
                    fontSize = 13.sp,
                    color = colorScheme.onSurfaceVariantSummary,
                )
            }
            state.backupSha1?.let {
                Text(
                    text = "原镜像备份：sha1=$it（设置 → 备份可还原）",
                    fontSize = 12.sp,
                    color = colorScheme.onSurfaceVariantSummary,
                )
            }
            Spacer(Modifier.height(12.dp))
            TextButton(
                text = if (state.running) "修补中…" else "开始修补",
                onClick = onStart,
                enabled = !state.running && state.sourceUri != null,
                colors = ButtonDefaults.textButtonColorsPrimary(),
            )
            if (state.running) {
                Text(
                    text = "修补进行中：请保持 App 在前台。中断不会产生可刷产物。",
                    fontSize = 13.sp,
                    color = colorScheme.onSurfaceVariantSummary,
                )
            }
        }
    }
}

// ---------------------------------------------------------------- 错误（常驻）

@Composable
private fun ErrorCard(
    state: PatchUiState,
    showDetail: Boolean,
    onToggleDetail: () -> Unit,
    onCopyAll: () -> Unit,
) {
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.defaultColors(
            color = colorScheme.errorContainer,
            contentColor = colorScheme.onErrorContainer,
        ),
    ) {
        Column(modifier = Modifier.padding(16.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Icon(
                    imageVector = MiuixIcons.Report,
                    contentDescription = null,
                    tint = colorScheme.onErrorContainer,
                    modifier = Modifier
                        .padding(end = 8.dp)
                        .size(20.dp),
                )
                Text(
                    text = "修补失败（详情常驻，可复制上报）",
                    fontSize = 15.sp,
                    fontWeight = FontWeight.Medium,
                    color = colorScheme.onErrorContainer,
                )
            }
            Spacer(Modifier.height(6.dp))
            Text(
                text = state.errorTitle ?: "未知错误",
                fontSize = 14.sp,
                color = colorScheme.onErrorContainer,
            )
            state.errorCode?.let {
                Text(
                    text = "错误码：$it",
                    fontSize = 12.sp,
                    fontFamily = FontFamily.Monospace,
                    color = colorScheme.onErrorContainer,
                )
            }
            Spacer(Modifier.height(4.dp))
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                TextButton(
                    text = if (showDetail) "收起详情" else "展开详情",
                    onClick = onToggleDetail,
                )
                TextButton(text = "复制全部", onClick = onCopyAll)
            }
            if (showDetail) {
                Spacer(Modifier.height(6.dp))
                Card(modifier = Modifier.fillMaxWidth()) {
                    Text(
                        text = state.errorDetail ?: "（无详情）",
                        fontSize = 12.sp,
                        fontFamily = FontFamily.Monospace,
                        color = colorScheme.onSurface,
                        modifier = Modifier
                            .fillMaxWidth()
                            .heightIn(max = 320.dp)
                            .verticalScroll(rememberScrollState())
                            .padding(12.dp),
                    )
                }
            }
            Spacer(Modifier.height(4.dp))
            Text(
                text = "日志文件：${state.logPath ?: "未初始化"}",
                fontSize = 12.sp,
                fontFamily = FontFamily.Monospace,
                color = colorScheme.onErrorContainer,
            )
        }
    }
}

// ---------------------------------------------------------------- 成功

@Composable
private fun SuccessCard(
    state: PatchUiState,
    showKey: Boolean,
    onToggleKey: () -> Unit,
    onCopyKey: (String) -> Unit,
    onCopyCommand: (String) -> Unit,
) {
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(16.dp)) {
            Text(
                text = "修补完成（产物已导出）",
                fontSize = MiuixTheme.textStyles.headline1.fontSize,
                fontWeight = FontWeight.Medium,
                color = colorScheme.onSurface,
            )
            Text(
                text = "导出了 ≠ 刷进去了：请按下方指引手动刷入；刷入后回到本页标记状态",
                fontSize = 13.sp,
                color = colorScheme.onSurfaceVariantSummary,
            )
            Spacer(Modifier.height(8.dp))
            KeyValueRow("产物", state.outputPath!!, monospace = true)
            KeyValueRow("大小", Capabilities.formatBytes(state.outputBytes), monospace = true)
            KeyValueRow("sha256", state.outputSha256 ?: "—", monospace = true)
            KeyValueRow(
                label = "内核等长校验",
                value = if (state.kernelEqualLength) {
                    "通过（${Capabilities.formatBytes(state.kernelBytes)}，纯字节 patch）"
                } else {
                    "未通过（禁止刷入）"
                },
                valueColor = if (state.kernelEqualLength) colorScheme.onSurface else colorScheme.error,
            )
            if (state.avbStatus.isNotBlank()) {
                KeyValueRow("AVB 校验（AVB-1/2/3）", state.avbStatus)
            }
            if (state.rcTarget.isNotBlank()) {
                KeyValueRow(
                    label = "开机自启（AP daemon）",
                    value = if (state.rcInjected) {
                        "已注入开机自启模板（target=${state.rcTarget}，来源=${state.rcSource}）—— " +
                            "待 AP 部署完成后生效：AP 部署需写入 /data/adb/sproot/key（0600，47 字符）；" +
                            "在此之前自启不会工作（可手动启动 AP）"
                    } else {
                        "未注入（target=${state.rcTarget}，原因=${state.rcReason}）—— 产物仍合规，可手动启动 AP"
                    },
                    valueColor = colorScheme.onSurfaceVariantSummary,
                )
            }
            state.initBootOutputPath?.let { initPath ->
                Spacer(Modifier.height(6.dp))
                KeyValueRow("init_boot 产物", initPath, monospace = true)
                KeyValueRow("init_boot 大小", Capabilities.formatBytes(state.initBootBytes), monospace = true)
                KeyValueRow("init_boot sha256", state.initBootOutputSha256 ?: "—", monospace = true)
            }
            state.reportPath?.let { KeyValueRow("引擎报告", it, monospace = true) }
            state.backupSha1?.let { KeyValueRow("原镜像备份", "sha1=$it", monospace = true) }

            val key = state.rootKey
            if (key != null) {
                Spacer(Modifier.height(6.dp))
                KeyValueRow(
                    label = "Root 密钥（47 字符 + NUL，内核 ABI）",
                    value = if (showKey) key else maskSecret(key),
                    monospace = true,
                )
                Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    TextButton(text = if (showKey) "隐藏密钥" else "显示密钥", onClick = onToggleKey)
                    TextButton(text = "复制密钥", onClick = { onCopyKey(key) })
                }
            }

            if (state.mode == PatchMode.EXPORT_AND_GUIDE) {
                Spacer(Modifier.height(8.dp))
                FlashGuide(
                    bootPath = state.outputPath!!,
                    initBootPath = state.initBootOutputPath,
                    onCopy = onCopyCommand,
                )
            }
            Spacer(Modifier.height(8.dp))
            TwoLevelStatus()
        }
    }
}

/**
 * 刷入指引 —— 版式照抄 FlashMiuix 的命令卡：等宽命令 + 复制按钮。
 * AVB 结论（NEW-DOC-04 §4.4）：三家实现都不碰 AVB 密码学，只刷 boot、不碰 vbmeta。
 */
@Composable
private fun FlashGuide(bootPath: String, initBootPath: String?, onCopy: (String) -> Unit) {
    val bootName = bootPath.substringAfterLast('/')
    val initBootName = initBootPath?.substringAfterLast('/')
    val commands = buildList {
        add("进入 bootloader" to "adb reboot bootloader")
        add("查看当前槽位（A/B 机型）" to "fastboot getvar current-slot")
        add("① 刷入 boot（双槽都刷）" to "fastboot flash --slot=all boot $bootName")
        if (initBootName != null) {
            add("② 刷入 init_boot（双槽都刷；自启 rc 在这里）" to "fastboot flash --slot=all init_boot $initBootName")
        }
        add("重启" to "fastboot reboot")
    }
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.defaultColors(
            color = colorScheme.surfaceContainer,
            contentColor = colorScheme.onSurface,
        ),
    ) {
        Column(modifier = Modifier.padding(16.dp)) {
            Text(
                text = if (initBootPath != null) {
                    "刷入指引（boot + init_boot 两个产物都要刷，按①→②顺序；不碰 vbmeta）"
                } else {
                    "刷入指引（只刷 boot 分区，不碰 vbmeta）"
                },
                fontSize = 15.sp,
                fontWeight = FontWeight.Medium,
                color = colorScheme.onSurface,
            )
            Spacer(Modifier.height(6.dp))
            commands.forEach { (label, cmd) ->
                Text(
                    text = "$label：",
                    fontSize = 13.sp,
                    color = colorScheme.onSurfaceVariantSummary,
                )
                Row(
                    modifier = Modifier.fillMaxWidth(),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Text(
                        text = cmd,
                        fontSize = 12.sp,
                        fontFamily = FontFamily.Monospace,
                        color = colorScheme.onSurface,
                        modifier = Modifier.weight(1f),
                    )
                    TextButton(text = "复制", onClick = { onCopy(cmd) })
                }
            }
            Spacer(Modifier.height(6.dp))
            Text(
                text = "⚠️ 仅刷 boot 分区；不要刷 vbmeta、不要加 --disable-verity/--disable-verification。\n" +
                    "若 bootloader 仍锁着，刷入后将无法开机（属预期）——请确认已解锁后再刷。\n" +
                    "刷前记录本页 sha256；刷坏可用本页「原镜像备份」还原。",
                fontSize = 12.sp,
                color = colorScheme.error,
            )
        }
    }
}

/** 两级状态：产物导出成功 → 用户确认已刷入 → 用户确认已重启生效。 */
@Composable
private fun TwoLevelStatus() {
    val context = LocalContext.current
    var status by remember { mutableStateOf(InstallState.load(context).status) }
    val nextLabel = when (status) {
        InstallState.Status.PATCHED_PENDING_FLASH -> "我已刷入（标记待重启）"
        InstallState.Status.FLASHED_PENDING_REBOOT -> "已重启并生效（标记激活）"
        else -> "状态已更新完毕"
    }
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.defaultColors(
            color = colorScheme.surfaceContainer,
            contentColor = colorScheme.onSurface,
        ),
    ) {
        Column(modifier = Modifier.padding(16.dp)) {
            Text(
                text = "当前状态：${status.label}",
                fontSize = 15.sp,
                fontWeight = FontWeight.Medium,
                color = colorScheme.onSurface,
            )
            Text(
                text = status.hint,
                fontSize = 13.sp,
                color = colorScheme.onSurfaceVariantSummary,
            )
            Spacer(Modifier.height(8.dp))
            TextButton(
                text = nextLabel,
                onClick = {
                    when (status) {
                        InstallState.Status.PATCHED_PENDING_FLASH -> {
                            InstallState.markFlashed(context)
                            status = InstallState.Status.FLASHED_PENDING_REBOOT
                        }

                        InstallState.Status.FLASHED_PENDING_REBOOT -> {
                            InstallState.markActive(context)
                            status = InstallState.Status.ACTIVE
                        }

                        else -> Unit
                    }
                },
                enabled = status == InstallState.Status.PATCHED_PENDING_FLASH ||
                    status == InstallState.Status.FLASHED_PENDING_REBOOT,
                colors = ButtonDefaults.textButtonColorsPrimary(),
            )
            Text(
                text = "刷入成功由你手动回报：无 root 时本 App 读不到 fastboot 侧结果。",
                fontSize = 12.sp,
                color = colorScheme.onSurfaceVariantSummary,
            )
        }
    }
}

/** 键值行（标题 12sp 灰 + 值 14sp；等宽用于 sha256/路径）。 */
@Composable
private fun KeyValueRow(
    label: String,
    value: String,
    monospace: Boolean = false,
    valueColor: Color = colorScheme.onSurface,
) {
    Column(modifier = Modifier.padding(vertical = 4.dp)) {
        Text(
            text = label,
            fontSize = 12.sp,
            color = colorScheme.onSurfaceVariantSummary,
        )
        Text(
            text = value,
            fontSize = 14.sp,
            color = valueColor,
            fontFamily = if (monospace) FontFamily.Monospace else FontFamily.Default,
        )
    }
}

private fun queryDisplayName(context: Context, uri: Uri): String? {
    return runCatching {
        context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { cursor ->
            if (cursor.moveToFirst()) {
                val index = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                if (index >= 0) cursor.getString(index) else null
            } else null
        }
    }.onFailure {
        AppLog.warn("PatchUI", "读取文件名失败：${it.message}")
    }.getOrNull()
}
