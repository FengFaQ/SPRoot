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
import com.sproot.manager.core.PatchPipeline
import com.sproot.manager.core.PatchStage
import com.sproot.manager.core.SecretStore
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
 *  1) 错误一旦发生，[errorTitle]/[errorDetail] **常驻**，只有用户点"重置"才消失；
 *  2) 日志面板始终在页面底部，且日志已同步落盘（设置页可见路径）；
 *  3) 进度条按阶段推进并显示当前阶段 + 引擎百分比，不做"假进度"。
 */
@Composable
fun PatchPage(modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val state = PatchPipeline.uiState.value
    var showKey by remember { mutableStateOf(false) }
    var showErrorDetail by remember { mutableStateOf(true) }

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
        // 选中即做一次输入校验，让用户提前看到"这镜像能不能修"
        runCatching {
            AppLog.info("PatchUI", "已选择镜像，开始后台预检")
        }
    }

    LazyColumn(modifier = modifier.fillMaxSize(), contentPadding = PaddingValues(16.dp)) {
        item {
            SectionCard(
                title = "1 · 选择镜像",
                subtitle = "必须是本机 boot.img（Android boot image）；kernel 本体/压缩包会被拒绝",
            ) {
                Column {
                    InfoRow(
                        label = "当前选择",
                        value = state.sourceName ?: "未选择",
                        monospace = false,
                    )
                    if (state.sourceUri != null) {
                        InfoRow("URI", state.sourceUri, monospace = true)
                    }
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
            SectionCard(title = "2 · 修补进度") {
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
                        if (state.errorCode != null) {
                            InfoRow("错误码", state.errorCode, monospace = true)
                        }
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
                SectionCard(title = "✓ 修补完成", subtitle = "产物已导出，可自行用 fastboot 刷入") {
                    Column {
                        InfoRow("产物", state.outputPath!!, monospace = true)
                        state.reportPath?.let { InfoRow("报告", it, monospace = true) }
                        val key = state.rootKey
                        if (key != null) {
                            InfoRow(
                                label = "Root 密钥（48 位）",
                                value = if (showKey) key else maskSecret(key),
                                monospace = true,
                            )
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
                        VSpacer(10)
                        BodyText("刷入方式：adb reboot bootloader → fastboot flash boot <产物路径> → fastboot reboot")
                        VSpacer(4)
                        BodyText("刷入前请确认 boot 分区与当前系统版本严格匹配，否则可能无法开机。")
                    }
                }
            }
        }

        item { VSpacer(14) }
        item { LogPanel() }
        item { VSpacer(24) }
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

/** 设置页/主页共用的密钥展示（预留：当前直接读取） */
fun currentSecret(context: Context): String? =
    runCatching { SecretStore.get(context).readRootKey() }.getOrNull()
