package com.sproot.manager.ui.pages

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import com.sproot.manager.core.AppLog
import com.sproot.manager.core.BackupStore
import com.sproot.manager.core.Capabilities
import com.sproot.manager.core.InstallState
import com.sproot.manager.core.PatchMode
import com.sproot.manager.core.PatchPipeline
import com.sproot.manager.core.PatchStage
import com.sproot.manager.ui.components.ActionRow
import com.sproot.manager.ui.components.BodyText
import com.sproot.manager.ui.components.HSpacer
import com.sproot.manager.ui.components.InfoRow
import com.sproot.manager.ui.components.LogPanel
import com.sproot.manager.ui.components.SectionCard
import com.sproot.manager.ui.components.VSpacer
import com.sproot.manager.ui.components.copyToClipboard
import com.sproot.manager.ui.components.maskSecret
import top.yukonga.miuix.kmp.basic.Button
import top.yukonga.miuix.kmp.basic.ButtonDefaults
import top.yukonga.miuix.kmp.basic.Card
import top.yukonga.miuix.kmp.basic.LinearProgressIndicator
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.theme.MiuixTheme

/**
 * 修补页 —— 本项目最重要的一页。
 *
 * 三条不可妥协的 UI 约定（对应旧版"点击修补一闪即退"）：
 *  1) 错误一旦发生，errorTitle/errorDetail **常驻**，只有用户点"重置"才消失；
 *  2) 日志面板始终在页面底部，且日志已同步落盘（设置页可见路径）；
 *  3) 进度按阶段推进 + 引擎百分比，失败时保留完整引擎输出与工作目录。
 *
 * 导出结果区给出「两级状态」：导出了 ≠ 刷进去了；用户确认后状态机才前进。
 */
@Composable
fun PatchPage(modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val state = PatchPipeline.uiState.value
    var showKey by remember { mutableStateOf(false) }
    var showErrorDetail by remember { mutableStateOf(true) }
    var backupInfo by remember { mutableStateOf<BackupStore.Backup?>(null) }

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
        backupInfo = null
    }

    LazyColumn(modifier = modifier.fillMaxSize(), contentPadding = PaddingValues(16.dp)) {
        item {
            SectionCard(
                title = "1 · 选择镜像",
                subtitle = "必须是本机 boot.img（Android boot image）；kernel 本体/压缩包会被拒绝",
            ) {
                Column {
                    InfoRow("当前选择", state.sourceName ?: "未选择")
                    if (state.sourceUri != null) InfoRow("URI", state.sourceUri, monospace = true)
                    VSpacer(10)
                    ActionRow {
                        Button(
                            onClick = { picker.launch(arrayOf("*/*")) },
                            enabled = !state.running,
                            colors = ButtonDefaults.buttonColorsPrimary(),
                        ) {
                            Text("选择 boot.img", style = MiuixTheme.textStyles.button)
                        }
                        HSpacer(8)
                        Button(onClick = { PatchPipeline.reset() }, enabled = !state.running) {
                            Text("重置", style = MiuixTheme.textStyles.button)
                        }
                    }
                }
            }
        }

        item { VSpacer(14) }

        item {
            SectionCard(
                title = "2 · 修补模式",
                subtitle = "借鉴 KSU/APatch 的 PatchMode：导出与刷入分离，避免「成功」文案误导",
            ) {
                Column {
                    ActionRow {
                        PatchMode.entries.filter { it.enabled }.forEachIndexed { index, mode ->
                            if (index > 0) HSpacer(8)
                            Button(
                                onClick = { PatchPipeline.selectMode(mode) },
                                enabled = !state.running,
                                colors = if (state.mode == mode) ButtonDefaults.buttonColorsPrimary()
                                else ButtonDefaults.buttonColors(),
                            ) {
                                Text(mode.label, style = MiuixTheme.textStyles.button)
                            }
                        }
                    }
                    VSpacer(6)
                    BodyText("当前：${state.mode.label} —— ${state.mode.description}")
                    VSpacer(4)
                    BodyText(
                        "「从备份还原」在设置 → 备份中操作；「卸载并还原内核」待 AP 部署能力就绪后开放。",
                        color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
                    )
                }
            }
        }

        item { VSpacer(14) }

        item {
            SectionCard(title = "3 · 修补进度") {
                Column {
                    InfoRow("当前阶段", "${state.stage.title}${if (state.phase.isNotBlank()) " · ${state.phase}" else ""}")
                    InfoRow("进度", "${(state.progress * 100).toInt()}%")
                    VSpacer(8)
                    LinearProgressIndicator(
                        progress = state.progress.coerceIn(0f, 1f),
                        modifier = Modifier.fillMaxWidth(),
                    )
                    VSpacer(12)
                    Button(
                        onClick = { PatchPipeline.start(context) },
                        enabled = !state.running && state.sourceUri != null,
                        colors = ButtonDefaults.buttonColorsPrimary(),
                    ) {
                        Text(if (state.running) "修补中…" else "开始修补", style = MiuixTheme.textStyles.button)
                    }
                    if (state.running) {
                        VSpacer(6)
                        BodyText("修补进行中：请保持 App 在前台。中断不会产生可刷产物。")
                    }
                    state.backupSha1?.let {
                        VSpacer(6)
                        InfoRow("原镜像备份", "sha1=$it（设置 → 备份可还原）", monospace = true)
                    }
                }
            }
        }

        // ---------------- 错误区：一旦出现就不消失 ----------------
        if (state.hasError) {
            item { VSpacer(14) }
            item {
                SectionCard(
                    title = "✗ 修补失败（详情常驻，可复制上报）",
                    subtitle = "该区域不会自动消失；请把完整错误与日志一并反馈",
                ) {
                    Column {
                        InfoRow(
                            label = "错误",
                            value = state.errorTitle ?: "未知错误",
                            valueColor = MiuixTheme.colorScheme.error,
                        )
                        state.errorCode?.let { InfoRow("错误码", it, monospace = true) }
                        VSpacer(6)
                        Row(modifier = Modifier.fillMaxWidth()) {
                            Button(
                                onClick = { showErrorDetail = !showErrorDetail },
                                colors = ButtonDefaults.buttonColors(),
                            ) {
                                Text(if (showErrorDetail) "收起详情" else "展开详情", style = MiuixTheme.textStyles.button)
                            }
                            HSpacer(8)
                            Button(onClick = {
                                context.copyToClipboard(
                                    "错误详情",
                                    (state.errorTitle ?: "") + "\n\n" + (state.errorDetail ?: "") +
                                        "\n\n---- 日志 ----\n" + AppLog.dumpText() +
                                        "\n\n日志文件：" + (state.logPath ?: "无"),
                                )
                            }) {
                                Text("复制全部", style = MiuixTheme.textStyles.button)
                            }
                        }
                        if (showErrorDetail) {
                            VSpacer(8)
                            Card(modifier = Modifier.fillMaxWidth()) {
                                Text(
                                    text = state.errorDetail ?: "(无详情)",
                                    style = MiuixTheme.textStyles.footnote1.copy(fontFamily = FontFamily.Monospace),
                                    color = MiuixTheme.colorScheme.onSurfaceContainer,
                                    modifier = Modifier
                                        .fillMaxWidth()
                                        .heightIn(max = 320.dp)
                                        .verticalScroll(rememberScrollState()),
                                )
                            }
                        }
                        InfoRow("日志文件", state.logPath ?: "未初始化")
                    }
                }
            }
        }

        // ---------------- 成功区 ----------------
        if (state.stage == PatchStage.DONE && state.outputPath != null) {
            item { VSpacer(14) }
            item {
                SectionCard(title = "✓ 修补完成（产物已导出）", subtitle = "导出了 ≠ 刷进去了：请按下方指引手动刷入") {
                    Column {
                        InfoRow("产物", state.outputPath!!, monospace = true)
                        InfoRow("大小", Capabilities.formatBytes(state.outputBytes), monospace = true)
                        InfoRow("sha256", state.outputSha256 ?: "—", monospace = true)
                        InfoRow(
                            "内核等长校验",
                            if (state.kernelEqualLength) "通过（${Capabilities.formatBytes(state.kernelBytes)}，纯字节 patch）"
                            else "未通过（禁止刷入）",
                            valueColor = if (state.kernelEqualLength) MiuixTheme.colorScheme.onSurfaceContainer
                            else MiuixTheme.colorScheme.error,
                        )
                        state.reportPath?.let { InfoRow("引擎报告", it, monospace = true) }
                        state.backupSha1?.let { InfoRow("原镜像备份", "sha1=$it", monospace = true) }

                        val key = state.rootKey
                        if (key != null) {
                            VSpacer(6)
                            InfoRow("Root 密钥（48 位）", if (showKey) key else maskSecret(key), monospace = true)
                            VSpacer(6)
                            ActionRow {
                                Button(onClick = { showKey = !showKey }) {
                                    Text(if (showKey) "隐藏密钥" else "显示密钥", style = MiuixTheme.textStyles.button)
                                }
                                HSpacer(8)
                                Button(onClick = { context.copyToClipboard("Root 密钥", key) }) {
                                    Text("复制密钥", style = MiuixTheme.textStyles.button)
                                }
                            }
                        }

                        if (state.mode == PatchMode.EXPORT_AND_GUIDE) {
                            VSpacer(12)
                            FlashGuideCard(
                                artifactPath = state.outputPath!!,
                                onCopy = { cmd -> context.copyToClipboard("刷入命令", cmd) },
                            )
                        }

                        VSpacer(12)
                        TwoLevelStatus(
                            context = context,
                            pendingStatus = InstallState.load(context).status,
                        )
                    }
                }
            }
        }

        item { VSpacer(14) }
        item { LogPanel() }
        item { VSpacer(24) }
    }
}

/** 刷入指引（AVB 结论：三家实现都不碰 AVB 密码学，靠已解锁 bootloader；只刷 boot 分区） */
@Composable
private fun FlashGuideCard(artifactPath: String, onCopy: (String) -> Unit) {
    val fileName = artifactPath.substringAfterLast('/')
    val slotCmd = "fastboot getvar current-slot"
    val flashCmd = "fastboot flash --slot=all boot $fileName"
    val rebootCmd = "fastboot reboot"
    Card(modifier = Modifier.fillMaxWidth()) {
        Column {
            Text("刷入指引（只刷 boot 分区，不碰 vbmeta）", style = MiuixTheme.textStyles.title4)
            VSpacer(6)
            InfoRow("① 进入 bootloader", "adb reboot bootloader")
            InfoRow("② 查看当前槽位（A/B 机型）", slotCmd, monospace = true)
            InfoRow("③ 刷入 boot（双槽都刷）", flashCmd, monospace = true)
            InfoRow("④ 重启", rebootCmd, monospace = true)
            VSpacer(8)
            ActionRow {
                Button(onClick = { onCopy(flashCmd) }) {
                    Text("复制刷入命令", style = MiuixTheme.textStyles.button)
                }
                HSpacer(8)
                Button(onClick = { onCopy(slotCmd) }) {
                    Text("复制查槽命令", style = MiuixTheme.textStyles.button)
                }
            }
            VSpacer(10)
            Text("⚠️ 风险提示", style = MiuixTheme.textStyles.title4, color = MiuixTheme.colorScheme.error)
            VSpacer(4)
            BodyText("仅刷 boot 分区；不要刷 vbmeta、不要加 --disable-verity/--disable-verification。")
            BodyText("若设备 bootloader 仍处于锁定态，刷入后将无法开机（属预期行为）——请确认已解锁后再刷。")
            BodyText(
                "刷前请记录本页 sha256 与产物路径；刷后若无法开机，用 fastboot 刷回原镜像（本页「原镜像备份」可还原）。",
                color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
            )
        }
    }
}

/**
 * 两级状态：产物导出成功 → 用户确认已刷入 → 用户确认已重启生效。
 * 状态持久化在 [InstallState]，主页/设置页共享同一份事实。
 */
@Composable
private fun TwoLevelStatus(context: Context, pendingStatus: InstallState.Status) {
    var status by remember { mutableStateOf(pendingStatus) }
    Column {
        InfoRow("当前状态", "${status.label} · ${status.hint}")
        VSpacer(8)
        ActionRow {
            Button(
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
                colors = ButtonDefaults.buttonColorsPrimary(),
            ) {
                Text(
                    when (status) {
                        InstallState.Status.PATCHED_PENDING_FLASH -> "我已刷入（标记待重启）"
                        InstallState.Status.FLASHED_PENDING_REBOOT -> "已重启并生效（标记激活）"
                        else -> "状态已更新完毕"
                    },
                    style = MiuixTheme.textStyles.button,
                )
            }
        }
        VSpacer(4)
        BodyText(
            "刷入成功由你手动回报：本 App 无法在不 root 的情况下读取 fastboot 侧结果。",
            color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
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
