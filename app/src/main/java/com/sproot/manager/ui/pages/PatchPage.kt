package com.sproot.manager.ui.pages

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import android.widget.Toast
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.clickable
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
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.sproot.manager.core.AppLog
import com.sproot.manager.core.Capabilities
import com.sproot.manager.core.EnginePhase
import com.sproot.manager.core.Exporter
import com.sproot.manager.core.PatchPipeline
import com.sproot.manager.core.PatchStage
import com.sproot.manager.core.PatchUiState
import com.sproot.manager.ui.components.BlurredBar
import com.sproot.manager.ui.components.ConfirmDialog
import com.sproot.manager.ui.components.copyToClipboard
import com.sproot.manager.ui.components.maskSecret
import com.sproot.manager.ui.components.rememberBlurBackdrop
import com.sproot.manager.ui.theme.LocalEnableBlur
import top.yukonga.miuix.kmp.basic.BasicComponent
import top.yukonga.miuix.kmp.basic.ButtonDefaults
import top.yukonga.miuix.kmp.basic.Card
import top.yukonga.miuix.kmp.basic.CardDefaults
import top.yukonga.miuix.kmp.basic.Icon
import top.yukonga.miuix.kmp.basic.IconButton
import top.yukonga.miuix.kmp.basic.LinearProgressIndicator
import top.yukonga.miuix.kmp.basic.MiuixScrollBehavior
import top.yukonga.miuix.kmp.basic.Scaffold
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.basic.TextButton
import top.yukonga.miuix.kmp.basic.TopAppBar
import top.yukonga.miuix.kmp.blur.layerBackdrop
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.extended.Back
import top.yukonga.miuix.kmp.icon.extended.Folder
import top.yukonga.miuix.kmp.icon.extended.Report
import top.yukonga.miuix.kmp.theme.MiuixTheme
import top.yukonga.miuix.kmp.theme.MiuixTheme.colorScheme
import top.yukonga.miuix.kmp.utils.overScrollVertical
import top.yukonga.miuix.kmp.utils.scrollEndHaptic

/**
 * 修补页（主页触发的二级页）—— 人类指令 2026-11-10 C「大幅精简」后的形态：
 *
 * ```
 * Scaffold(BlurredBar(TopAppBar("修补内核" + 返回)))
 *  └─ LazyColumn(12dp 横距, 12dp 卡距)
 *      ├─ 选择 boot.img（必选）/ init_boot.img（可选·高级）
 *      ├─ 进度（百分比 + 进度条 + 当前阶段 + 引擎阶段 + 开始修补）
 *      ├─ 错误卡（失败时常驻，可展开详情/复制）
 *      ├─ 结果卡（本次产物路径/sha256/等长/AVB/密钥）
 *      └─ 底部一行小字：点击导出 TXT 日志到 Downloads/SPRoot/
 * ```
 *
 * 已删除：所有说明性小字、修补模式选择（导出并给出刷入指引）、日志面板、
 * 已修补产物区块、刷入指引、两级状态标记（刷入状态改由主页「未安装/已安装」表达）。
 */
@Composable
fun PatchPager(
    bottomInnerPadding: Dp = 0.dp,
    isCurrentPage: Boolean = true,
    onBack: () -> Unit = {},
) {
    val context = LocalContext.current
    val state = PatchPipeline.uiState.value
    val scrollBehavior = MiuixScrollBehavior()
    val backdrop = rememberBlurBackdrop(LocalEnableBlur.current)
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
        // DISPLAY_NAME 要从 DocumentsProvider 取（跨进程 IPC，慢 provider 可达数秒 → 主线程会 ANR）。
        // 用后台线程（与本文件其它异步动作一致），保证用户选完文件立刻返回也不会丢选择。
        Thread {
            val name = queryDisplayName(context, uri)
            PatchPipeline.selectInput(uri, name)
        }.start()
    }
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
        Thread {
            val name = queryDisplayName(context, uri)
            PatchPipeline.selectInitBootInput(uri, name)
        }.start()
    }

    ConfirmDialog(
        show = showResetConfirm,
        title = "重置修补页状态",
        message = null,
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
                    title = "修补内核",
                    navigationIcon = {
                        IconButton(onClick = onBack) {
                            Icon(
                                imageVector = MiuixIcons.Back,
                                contentDescription = "返回主页",
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
                        verticalArrangement = Arrangement.spacedBy(12.dp),
                    ) {
                        SelectImageCard(
                            state = state,
                            onPick = { picker.launch(arrayOf("*/*")) },
                            onPickInitBoot = { initBootPicker.launch(arrayOf("*/*")) },
                            onReset = { showResetConfirm = true },
                        )
                        ProgressCard(state = state, onStart = { PatchPipeline.start(context) })
                        if (state.hasError) {
                            ErrorCard(
                                state = state,
                                showDetail = showErrorDetail,
                                onToggleDetail = { showErrorDetail = !showErrorDetail },
                                onCopyAll = {
                                    context.copyToClipboard(
                                        "错误详情",
                                        (state.errorTitle ?: "") + "\n\n" + (state.errorDetail ?: ""),
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
                            )
                        }
                        LogExportLine()
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
            title = "boot.img",
            summary = state.sourceName ?: "未选择",
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
            title = "init_boot.img（可选 · 高级）",
            summary = state.initBootSourceName ?: "未选择",
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
        BasicComponent(
            title = "重置",
            summary = "",
            onClick = onReset,
        )
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
                    text = "进度",
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
                text = state.stage.title,
                fontSize = 14.sp,
                color = colorScheme.onSurface,
            )
            if (state.enginePct >= 0) {
                Text(
                    text = EnginePhase.label(state.enginePhase) + "（" + state.enginePct + "%）",
                    fontSize = 13.sp,
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
        }
    }
}

// ---------------------------------------------------------------- 错误

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
                    modifier = Modifier.padding(end = 8.dp),
                )
                Text(
                    text = state.errorTitle ?: "未知错误",
                    fontSize = 15.sp,
                    fontWeight = FontWeight.Medium,
                    color = colorScheme.onErrorContainer,
                )
            }
            state.errorCode?.let {
                Text(
                    text = it,
                    fontSize = 12.sp,
                    fontFamily = FontFamily.Monospace,
                    color = colorScheme.onErrorContainer,
                )
            }
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                TextButton(
                    text = if (showDetail) "收起" else "详情",
                    onClick = onToggleDetail,
                )
                TextButton(text = "复制", onClick = onCopyAll)
            }
            if (showDetail) {
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
        }
    }
}

// ---------------------------------------------------------------- 结果

@Composable
private fun SuccessCard(
    state: PatchUiState,
    showKey: Boolean,
    onToggleKey: () -> Unit,
    onCopyKey: (String) -> Unit,
) {
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(16.dp)) {
            Text(
                text = "完成（产物已导出）",
                fontSize = MiuixTheme.textStyles.headline1.fontSize,
                fontWeight = FontWeight.Medium,
                color = colorScheme.onSurface,
            )
            Spacer(Modifier.height(4.dp))
            KeyValueRow("boot.img", state.outputPath!!, monospace = true)
            KeyValueRow("size", Capabilities.formatBytes(state.outputBytes), monospace = true)
            KeyValueRow("sha256", state.outputSha256 ?: "—", monospace = true)
            KeyValueRow(
                label = "内核等长",
                value = if (state.kernelEqualLength) "通过" else "未通过",
                valueColor = if (state.kernelEqualLength) colorScheme.onSurface else colorScheme.error,
            )
            if (state.avbStatus.isNotBlank()) {
                KeyValueRow("AVB", state.avbStatus)
            }
            if (state.rcTarget.isNotBlank()) {
                KeyValueRow(
                    label = "自启脚本",
                    value = if (state.rcInjected) {
                        "已注入（" + state.rcTarget + "）"
                    } else {
                        "未注入（" + state.rcReason + "）"
                    },
                )
            }
            state.initBootOutputPath?.let { initPath ->
                KeyValueRow("init_boot.img", initPath, monospace = true)
                KeyValueRow("size", Capabilities.formatBytes(state.initBootBytes), monospace = true)
                KeyValueRow("sha256", state.initBootOutputSha256 ?: "—", monospace = true)
            }
            val key = state.rootKey
            if (key != null) {
                KeyValueRow(
                    label = "密钥",
                    value = if (showKey) key else maskSecret(key),
                    monospace = true,
                )
                Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    TextButton(text = if (showKey) "隐藏" else "显示", onClick = onToggleKey)
                    TextButton(text = "复制", onClick = { onCopyKey(key) })
                }
            }
        }
    }
}

/** 键值行（值用等宽字体；不再带说明性小字）。 */
@Composable
private fun KeyValueRow(
    label: String,
    value: String,
    monospace: Boolean = false,
    valueColor: Color = colorScheme.onSurface,
) {
    Column(modifier = Modifier.padding(vertical = 3.dp)) {
        Text(
            text = label,
            fontSize = 12.sp,
            color = colorScheme.onSurfaceVariantSummary,
        )
        Text(
            text = value,
            fontSize = 13.sp,
            color = valueColor,
            fontFamily = if (monospace) FontFamily.Monospace else FontFamily.Default,
        )
    }
}

// ---------------------------------------------------------------- 日志导出（页面最下方一行）

/** 人类指令（C）：页面最下方一行小字，点击导出 TXT 日志到 Downloads/SPRoot/；失败必须有提示。 */
@Composable
private fun LogExportLine() {
    val context = LocalContext.current
    Text(
        text = "导出 TXT 日志到 Downloads/SPRoot/",
        fontSize = 12.sp,
        color = colorScheme.primary,
        textAlign = TextAlign.Center,
        modifier = Modifier
            .fillMaxWidth()
            .padding(vertical = 8.dp)
            .clickable {
                Thread {
                    runCatching {
                        val file = AppLog.currentLogFile() ?: error("日志文件尚未初始化")
                        val stamp = java.text.SimpleDateFormat("yyyyMMdd-HHmmss", java.util.Locale.US)
                            .format(java.util.Date())
                        Exporter.exportToDownloads(context, file, "sproot-log-$stamp.txt")
                    }.onSuccess {
                        Toast.makeText(context, "日志已导出到 Downloads/SPRoot/", Toast.LENGTH_LONG).show()
                    }.onFailure {
                        Toast.makeText(context, "日志导出失败：" + it.message, Toast.LENGTH_LONG).show()
                    }
                }.start()
            },
    )
}

/**
 * 读取 DocumentsProvider 的 DISPLAY_NAME。
 *
 * ⚠️ **必须在后台线程调用**：`contentResolver.query` 是跨进程 IPC，慢 provider（云盘/网盘
 * 的 DocumentsProvider）响应可长达数秒，在主线程调用会直接 ANR。两个 picker 回调里都用
 * `Thread { … }.start()` 调用它。
 */
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
