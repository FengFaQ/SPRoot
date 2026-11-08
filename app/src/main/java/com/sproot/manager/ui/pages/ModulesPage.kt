package com.sproot.manager.ui.pages

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
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
import com.sproot.manager.core.ApdClient
import com.sproot.manager.core.ApdSelfTest
import com.sproot.manager.core.AppLog
import com.sproot.manager.core.Capabilities
import com.sproot.manager.core.ModuleInfo
import com.sproot.manager.core.ModuleRepository
import com.sproot.manager.core.ModulesResult
import com.sproot.manager.core.SecretStore
import com.sproot.manager.ui.components.ActionRow
import com.sproot.manager.ui.components.BodyText
import com.sproot.manager.ui.components.HSpacer
import com.sproot.manager.ui.components.InfoRow
import com.sproot.manager.ui.components.SectionCard
import com.sproot.manager.ui.components.VSpacer
import com.sproot.manager.ui.components.copyToClipboard
import top.yukonga.miuix.kmp.basic.Button
import top.yukonga.miuix.kmp.basic.ButtonDefaults
import top.yukonga.miuix.kmp.basic.Card
import top.yukonga.miuix.kmp.basic.Switch
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.theme.MiuixTheme

/**
 * 模块页：列表（`apd module list --json`）+ 安装（选 zip）+ 卸载 + 启停。
 *
 * 全部走 root-exec；AP 未就绪时禁用所有入口并说明原因（不出现"点了必然失败"的按钮）。
 * 安装流程：SAF 选 zip → 复制到 App 私有 cache → 校验 PK 头/大小 → `apd module install <path> --json`
 * → 结果常驻展示（原始 stdout/stderr 可复制）→ 成功后刷新列表。
 */
@Composable
fun ModulesPage(modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val key = remember { runCatching { SecretStore.get(context).getOrCreateRootKey() }.getOrNull() }
    val apdStatus = remember { ApdClient.status(context) }
    val apdReady = apdStatus.ok
    var selfTest by remember { mutableStateOf(ApdSelfTest.load(context)) }
    var result by remember { mutableStateOf<ModulesResult?>(null) }
    var loading by remember { mutableStateOf(false) }
    var actionMsg by remember { mutableStateOf<String?>(null) }
    var showRaw by remember { mutableStateOf(false) }

    // 安装状态
    var zipPath by remember { mutableStateOf<String?>(null) }
    var zipName by remember { mutableStateOf<String?>(null) }
    var zipError by remember { mutableStateOf<String?>(null) }
    var installing by remember { mutableStateOf(false) }
    var installRaw by remember { mutableStateOf<String?>(null) }
    var confirmUninstallId by remember { mutableStateOf<String?>(null) }

    fun refresh() {
        if (loading || key == null) return
        loading = true
        Thread {
            val r = ModuleRepository.list(context, key)
            result = r
            loading = false
            if (!r.ok) AppLog.error("Modules", "模块列表失败：${r.message}")
        }.start()
    }

    val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri: Uri? ->
        if (uri == null) return@rememberLauncherForActivityResult
        Thread {
            runCatching {
                val name = queryDisplayName(context, uri) ?: "module.zip"
                val target = java.io.File(context.cacheDir, "module-upload-${System.currentTimeMillis()}.zip")
                context.contentResolver.openInputStream(uri)?.use { input ->
                    target.outputStream().use { out -> input.copyTo(out) }
                } ?: throw IllegalStateException("无法读取所选文件")
                val error = ModuleRepository.validateZip(target)
                if (error != null) {
                    target.delete()
                    zipPath = null
                    zipName = name
                    zipError = error
                } else {
                    zipPath = target.absolutePath
                    zipName = name
                    zipError = null
                    AppLog.info("Modules", "已选择模块包：$name（${Capabilities.formatBytes(target.length())}）")
                }
            }.onFailure {
                zipError = "复制所选文件失败：${it.javaClass.simpleName}: ${it.message}"
                AppLog.error("Modules", "复制模块包失败", it)
            }
        }.start()
    }

    LazyColumn(modifier = modifier.fillMaxSize(), contentPadding = PaddingValues(16.dp)) {
        // ---------------- 能力门禁 ----------------
        if (!apdReady) {
            item {
                SectionCard(
                    title = "⚠️ 模块功能不可用",
                    subtitle = "所有模块操作都依赖 AP 守护；当前检测未通过",
                ) {
                    Column {
                        BodyText(apdStatus.detail)
                        VSpacer(8)
                        BodyText("请先到「设置 → AP 部署」完成部署；部署前本页按钮保持禁用。")
                        selfTest?.let {
                            VSpacer(6)
                            InfoRow("AP 自检", it.describe())
                        }
                    }
                }
            }
            item { VSpacer(14) }
        }

        // ---------------- 安装 ----------------
        item {
            SectionCard(
                title = "安装模块",
                subtitle = "选择模块 zip（Magisk/KSU 格式），经 AP 用户态安装；App 不直接解压到 /data/adb",
            ) {
                Column {
                    InfoRow("已选择", zipName ?: "未选择")
                    zipPath?.let { InfoRow("临时副本", it, monospace = true) }
                    zipError?.let {
                        VSpacer(4)
                        InfoRow("校验失败", it, valueColor = MiuixTheme.colorScheme.error)
                    }
                    VSpacer(10)
                    ActionRow {
                        Button(
                            onClick = { picker.launch(arrayOf("application/zip", "application/octet-stream", "*/*")) },
                            enabled = apdReady && !installing && key != null,
                            colors = ButtonDefaults.buttonColorsPrimary(),
                        ) { Text("选择 zip", style = MiuixTheme.textStyles.button) }
                        HSpacer(8)
                        Button(
                            onClick = {
                                val path = zipPath ?: return@Button
                                if (key == null) {
                                    actionMsg = "密钥不可用"
                                    return@Button
                                }
                                installing = true
                                installRaw = null
                                Thread {
                                    val r = ModuleRepository.install(context, key, java.io.File(path))
                                    installRaw = "exit=${r.exitCode}：${r.message}\n" +
                                        (r.rawStdout.takeIf { it.isNotBlank() }?.let { "[stdout]\n$it" } ?: "") +
                                        (r.rawStderr.takeIf { it.isNotBlank() }?.let { "\n[stderr]\n$it" } ?: "")
                                    actionMsg = r.message
                                    installing = false
                                    if (r.ok) {
                                        zipPath?.let { runCatching { java.io.File(it).delete() } }
                                        zipPath = null
                                        zipName = null
                                        refresh()
                                    }
                                }.start()
                            },
                            enabled = apdReady && !installing && zipPath != null && key != null,
                        ) { Text(if (installing) "安装中…（最多 300s）" else "开始安装", style = MiuixTheme.textStyles.button) }
                        HSpacer(8)
                        Button(
                            onClick = {
                                zipPath?.let { runCatching { java.io.File(it).delete() } }
                                zipPath = null
                                zipName = null
                                zipError = null
                                installRaw = null
                            },
                            enabled = !installing,
                        ) { Text("取消", style = MiuixTheme.textStyles.button) }
                    }
                    if (installing) {
                        VSpacer(8)
                        BodyText("正在调用 apd module install（大模块可能数十秒）；请保持 App 在前台。失败会保留原始输出。")
                    }
                    installRaw?.let {
                        VSpacer(8)
                        Card(modifier = Modifier.fillMaxSize()) {
                            Text(
                                text = it,
                                style = MiuixTheme.textStyles.footnote1.copy(fontFamily = FontFamily.Monospace),
                                modifier = Modifier
                                    .fillMaxSize()
                                    .heightIn(max = 240.dp)
                                    .verticalScroll(rememberScrollState()),
                            )
                        }
                    }
                }
            }
        }

        item { VSpacer(14) }

        // ---------------- 列表 ----------------
        item {
            SectionCard(
                title = "已安装模块",
                subtitle = if (result?.ok == true) "共 ${result!!.modules.size} 个（apd module list --json）"
                else "尚未加载；点击「刷新列表」",
            ) {
                Column {
                    ActionRow {
                        Button(
                            onClick = { refresh() },
                            enabled = apdReady && !loading && key != null,
                            colors = ButtonDefaults.buttonColorsPrimary(),
                        ) { Text(if (loading) "加载中…" else "刷新列表", style = MiuixTheme.textStyles.button) }
                        HSpacer(8)
                        Button(
                            onClick = {
                                if (loading) return@Button
                                loading = true
                                Thread {
                                    selfTest = ApdSelfTest.run(context)
                                    loading = false
                                }.start()
                            },
                            enabled = !loading,
                        ) { Text("重跑 AP 自检", style = MiuixTheme.textStyles.button) }
                        HSpacer(8)
                        Button(
                            onClick = {
                                val raw = buildString {
                                    append("--- apd module list 原始输出 ---\n")
                                    append(result?.rawStdout ?: "<未执行>")
                                    if (!result?.rawStderr.isNullOrBlank()) {
                                        append("\n--- stderr ---\n").append(result?.rawStderr)
                                    }
                                }
                                context.copyToClipboard("模块原始输出", raw)
                            },
                            enabled = result != null,
                        ) { Text("复制原始输出", style = MiuixTheme.textStyles.button) }
                    }
                    selfTest?.let {
                        VSpacer(8)
                        InfoRow("AP 自检", "${it.stageLabel} · exit=${it.exitCode} · ${it.summary}")
                    }
                    actionMsg?.let {
                        VSpacer(6)
                        InfoRow("操作结果", it)
                    }
                    result?.takeIf { !it.ok }?.let {
                        VSpacer(8)
                        InfoRow("错误", it.message, valueColor = MiuixTheme.colorScheme.error)
                        ActionRow {
                            Button(onClick = { showRaw = !showRaw }) {
                                Text(if (showRaw) "收起原始输出" else "查看原始输出", style = MiuixTheme.textStyles.button)
                            }
                        }
                        if (showRaw) {
                            VSpacer(6)
                            Card(modifier = Modifier.fillMaxSize()) {
                                Text(
                                    text = (it.rawStdout.ifBlank { "(stdout 为空)" }) +
                                        if (it.rawStderr.isNotBlank()) "\n[stderr]\n" + it.rawStderr else "",
                                    style = MiuixTheme.textStyles.footnote1.copy(fontFamily = FontFamily.Monospace),
                                    modifier = Modifier
                                        .fillMaxSize()
                                        .heightIn(max = 300.dp)
                                        .verticalScroll(rememberScrollState()),
                                )
                            }
                        }
                    }
                }
            }
        }

        item { VSpacer(14) }

        val modules = result?.modules.orEmpty()
        if (result?.ok == true && modules.isEmpty()) {
            item {
                SectionCard(title = "（空）", subtitle = "AP 返回了空模块列表") {
                    BodyText("当前设备没有已安装模块。可用上方「安装模块」导入 zip。")
                }
            }
        }
        items(modules) { module ->
            ModuleCard(
                module = module,
                enabled = apdReady && key != null && !loading && !installing,
                confirmUninstall = confirmUninstallId == module.id,
                onToggle = { enable ->
                    if (key == null) return@ModuleCard
                    Thread {
                        val r = ModuleRepository.setEnabled(context, key, module.id, enable)
                        actionMsg = r.message
                        if (r.ok) refresh() else AppLog.error("Modules", r.message)
                    }.start()
                },
                onUninstall = {
                    if (key == null) return@ModuleCard
                    if (confirmUninstallId != module.id) {
                        confirmUninstallId = module.id
                        actionMsg = "再次点击「确认卸载」以卸载 ${module.id}"
                    } else {
                        confirmUninstallId = null
                        Thread {
                            val r = ModuleRepository.uninstall(context, key, module.id)
                            actionMsg = r.message
                            if (r.ok) refresh() else AppLog.error("Modules", r.message)
                        }.start()
                    }
                },
            )
            VSpacer(10)
        }

        item { VSpacer(14) }
        item {
            SectionCard(title = "说明 / 契约") {
                Column {
                    BodyText("模块生命周期全部由 AP 用户态管理（契约 v1.1 §8.3）：App 只经 root-exec 调用，" +
                        "不直接读写 /data/adb。")
                    VSpacer(6)
                    BodyText("schema v1（sys-guard 已定稿）：{id,name,version,versionCode,author,description,enabled,update_pending}；" +
                        "`install_status` 为 v2 待定字段，客户端占位解析（缺失忽略）。")
                    VSpacer(6)
                    BodyText("安装/卸载子命令的 --json 输出 schema 待 AP 定稿：客户端当前只依赖 exit code，" +
                        "并把原始 stdout/stderr 常驻展示（不猜进度字段）。")
                    VSpacer(6)
                    BodyText("root-exec 退出码：125=密钥无效/需重新修补（不重试）、126/127=需重新部署、124=超时。")
                }
            }
        }
        item { VSpacer(24) }
    }
}

@Composable
private fun ModuleCard(
    module: ModuleInfo,
    enabled: Boolean,
    confirmUninstall: Boolean,
    onToggle: (Boolean) -> Unit,
    onUninstall: () -> Unit,
) {
    Card(modifier = Modifier.fillMaxSize()) {
        Column {
            ActionRow {
                Text(
                    text = module.name,
                    style = MiuixTheme.textStyles.title4,
                    color = MiuixTheme.colorScheme.onSurfaceContainer,
                )
                HSpacer(8)
                Text(
                    text = if (module.enabled) "已启用" else "已禁用",
                    style = MiuixTheme.textStyles.footnote1,
                    color = if (module.enabled) MiuixTheme.colorScheme.primary else MiuixTheme.colorScheme.onSurfaceVariantSummary,
                )
                if (module.updatePending) {
                    HSpacer(8)
                    Text("有更新", style = MiuixTheme.textStyles.footnote1, color = MiuixTheme.colorScheme.error)
                }
            }
            VSpacer(4)
            InfoRow("ID", module.id, monospace = true)
            InfoRow("版本 / 作者", "${module.version}（code ${module.versionCode}）· ${module.author}")
            if (module.description.isNotBlank()) InfoRow("描述", module.description)
            if (module.installStatus.isNotBlank()) InfoRow("install_status（v2 待定）", module.installStatus)
            VSpacer(8)
            ActionRow {
                Switch(
                    checked = module.enabled,
                    onCheckedChange = { onToggle(it) },
                    enabled = enabled,
                )
                HSpacer(8)
                Button(
                    onClick = onUninstall,
                    enabled = enabled,
                    colors = if (confirmUninstall) ButtonDefaults.buttonColorsPrimary() else ButtonDefaults.buttonColors(),
                ) {
                    Text(if (confirmUninstall) "确认卸载" else "卸载", style = MiuixTheme.textStyles.button)
                }
            }
        }
    }
}

private fun queryDisplayName(context: Context, uri: Uri): String? = runCatching {
    context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { cursor ->
        if (cursor.moveToFirst()) {
            val index = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
            if (index >= 0) cursor.getString(index) else null
        } else null
    }
}.getOrNull()
