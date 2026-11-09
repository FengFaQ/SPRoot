package com.sproot.manager.ui.pages

import android.content.Context
import android.net.Uri
import android.provider.OpenableColumns
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
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
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
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
import com.sproot.manager.core.ApdClient
import com.sproot.manager.core.ApdSelfTest
import com.sproot.manager.core.AppLog
import com.sproot.manager.core.Capabilities
import com.sproot.manager.core.ModuleInfo
import com.sproot.manager.core.ModuleRepository
import com.sproot.manager.core.ModulesResult
import com.sproot.manager.core.SecretStore
import com.sproot.manager.ui.components.BlurredBar
import com.sproot.manager.ui.components.StatusTag
import com.sproot.manager.ui.components.WarningCard
import com.sproot.manager.ui.components.WarningLevel
import com.sproot.manager.ui.components.copyToClipboard
import com.sproot.manager.ui.components.rememberBlurBackdrop
import com.sproot.manager.ui.theme.LocalEnableBlur
import top.yukonga.miuix.kmp.basic.BasicComponent
import top.yukonga.miuix.kmp.basic.ButtonDefaults
import top.yukonga.miuix.kmp.basic.Card
import top.yukonga.miuix.kmp.basic.CardDefaults
import top.yukonga.miuix.kmp.basic.HorizontalDivider
import top.yukonga.miuix.kmp.basic.Icon
import top.yukonga.miuix.kmp.basic.IconButton
import top.yukonga.miuix.kmp.basic.MiuixScrollBehavior
import top.yukonga.miuix.kmp.basic.Scaffold
import top.yukonga.miuix.kmp.basic.Switch
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.basic.TextButton
import top.yukonga.miuix.kmp.basic.TopAppBar
import top.yukonga.miuix.kmp.blur.layerBackdrop
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.extended.Delete
import top.yukonga.miuix.kmp.icon.extended.Refresh
import top.yukonga.miuix.kmp.icon.extended.UploadCloud
import top.yukonga.miuix.kmp.theme.MiuixTheme
import top.yukonga.miuix.kmp.theme.MiuixTheme.colorScheme
import top.yukonga.miuix.kmp.utils.overScrollVertical
import top.yukonga.miuix.kmp.utils.scrollEndHaptic

/**
 * 模块页 —— 版式照抄 KernelSU `ui/screen/module/ModuleMiuix.kt`：
 *
 * ```
 * Scaffold(BlurredBar(TopAppBar("模块" + 刷新动作)))
 *  └─ LazyColumn(12dp 横距, 12dp 卡距)
 *      ├─ WarningCard（AP 未就绪 = Notice，不标红）
 *      ├─ Card 安装模块   BasicComponent(选择 zip) + 开始/取消 + 原始输出
 *      ├─ Card 操作结果    hints（可能原因/建议动作/依据）+ 结构化字段 + 原始输出
 *      ├─ 已安装模块      标题 + 刷新/自检/复制原始输出
 *      ├─ ModuleItem × N  Card(name/version/author + Switch + 分隔线 + 卸载/确认卸载)
 *      └─ Card 说明/契约
 * ```
 *
 * 业务逻辑一行未改：仍走 `ModuleRepository`（`apd module ...` root-exec）；
 * 安装/卸载/启停失败仍由 `ModuleFailureHints` 给出中文可操作提示，原始输出常驻可复制。
 */
@Composable
fun ModulePager(
    bottomInnerPadding: Dp = 0.dp,
    isCurrentPage: Boolean = true,
) {
    val context = LocalContext.current
    val key = remember { runCatching { SecretStore.get(context).getOrCreateRootKey() }.getOrNull() }
    val apdStatus = remember { ApdClient.status(context) }
    val apdReady = apdStatus.ok
    var selfTest by remember { mutableStateOf(ApdSelfTest.load(context)) }
    var result by remember { mutableStateOf<ModulesResult?>(null) }
    var loading by remember { mutableStateOf(false) }
    var actionMsg by remember { mutableStateOf<String?>(null) }
    var showRaw by remember { mutableStateOf(false) }

    var zipPath by remember { mutableStateOf<String?>(null) }
    var zipName by remember { mutableStateOf<String?>(null) }
    var zipError by remember { mutableStateOf<String?>(null) }
    var installing by remember { mutableStateOf(false) }
    var installRaw by remember { mutableStateOf<String?>(null) }
    var confirmUninstallId by remember { mutableStateOf<String?>(null) }

    val scrollBehavior = MiuixScrollBehavior()
    val backdrop = rememberBlurBackdrop(LocalEnableBlur.current)
    val barColor = if (backdrop != null) Color.Transparent else colorScheme.surface

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

    // 首次进入 / 切回本页时自动拉取一次列表（能力就绪才会拉）
    LaunchedEffect(isCurrentPage) {
        if (isCurrentPage && apdReady && key != null && result == null && !loading) refresh()
    }

    Scaffold(
        topBar = {
            BlurredBar(backdrop) {
                TopAppBar(
                    color = barColor,
                    title = "模块",
                    actions = {
                        IconButton(onClick = { if (!loading) refresh() }) {
                            Icon(
                                imageVector = MiuixIcons.Refresh,
                                contentDescription = "刷新列表",
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
                        if (!apdReady) {
                            WarningCard(
                                message = "模块功能暂不可用：尚未部署 AP 守护（需先刷入修补后的镜像），" +
                                    "部署完成后本页所有按钮才会启用",
                                level = WarningLevel.Notice,
                            )
                        }
                        InstallCard(
                            zipName = zipName,
                            zipPath = zipPath,
                            zipError = zipError,
                            installing = installing,
                            installRaw = installRaw,
                            enabled = apdReady && key != null,
                            onPick = { picker.launch(arrayOf("application/zip", "application/octet-stream", "*/*")) },
                            onInstall = {
                                val path = zipPath ?: return@InstallCard
                                if (key == null) {
                                    actionMsg = "密钥不可用"
                                    return@InstallCard
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
                            onCancel = {
                                zipPath?.let { runCatching { java.io.File(it).delete() } }
                                zipPath = null
                                zipName = null
                                zipError = null
                                installRaw = null
                            },
                        )
                        ResultCard(
                            result = result,
                            actionMsg = actionMsg,
                            selfTest = selfTest,
                            showRaw = showRaw,
                            onToggleRaw = { showRaw = !showRaw },
                            onCopyRaw = {
                                val raw = buildString {
                                    append("--- apd module 原始输出 ---\n")
                                    append(result?.rawStdout ?: "<未执行>")
                                    if (!result?.rawStderr.isNullOrBlank()) {
                                        append("\n--- stderr ---\n").append(result?.rawStderr)
                                    }
                                }
                                context.copyToClipboard("模块原始输出", raw)
                            },
                        )
                        ListHeaderCard(
                            loading = loading,
                            moduleCount = result?.modules?.size,
                            enabled = apdReady && key != null && !loading,
                            onRefresh = { refresh() },
                            onSelfTest = {
                                if (loading) return@ListHeaderCard
                                loading = true
                                Thread {
                                    selfTest = ApdSelfTest.run(context)
                                    loading = false
                                }.start()
                            },
                        )
                    }
                }

                val modules = result?.modules.orEmpty()
                if (result?.ok == true && modules.isEmpty()) {
                    item {
                        Card(
                            modifier = Modifier
                                .fillMaxWidth()
                                .padding(top = 12.dp),
                        ) {
                            Column(modifier = Modifier.padding(16.dp)) {
                                Text(
                                    text = "无已安装模块",
                                    fontSize = 15.sp,
                                    fontWeight = FontWeight.Medium,
                                    color = colorScheme.onSurface,
                                )
                                Text(
                                    text = "AP 返回了空列表（正常状态）。可用上方「选择模块 zip」导入。",
                                    fontSize = 13.sp,
                                    color = colorScheme.onSurfaceVariantSummary,
                                )
                            }
                        }
                    }
                }
                items(modules) { module ->
                    ModuleItem(
                        module = module,
                        enabled = apdReady && key != null && !loading && !installing,
                        confirmUninstall = confirmUninstallId == module.id,
                        onToggle = { enable ->
                            if (key == null) return@ModuleItem
                            Thread {
                                val r = ModuleRepository.setEnabled(context, key, module.id, enable)
                                actionMsg = r.message
                                if (r.ok) refresh() else AppLog.error("Modules", r.message)
                            }.start()
                        },
                        onUninstall = {
                            if (key == null) return@ModuleItem
                            if (confirmUninstallId != module.id) {
                                confirmUninstallId = module.id
                                actionMsg = "再次点击「确认卸载」以卸载 ${module.id}（下次重启后生效）"
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
                }

                item {
                    Card(
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(top = 12.dp),
                    ) {
                        Column(modifier = Modifier.padding(16.dp)) {
                            Text(
                                text = "说明 / 契约",
                                fontSize = 15.sp,
                                fontWeight = FontWeight.Medium,
                                color = colorScheme.onSurface,
                            )
                            Text(
                                text = "模块生命周期全部由 AP 用户态管理（契约 v1.1 §8.3）：App 只经 root-exec 调用，" +
                                    "不直接读写 /data/adb。安装/卸载/启停都是两阶段语义——" +
                                    "变更在下次 post-fs-data 原子生效，因此界面会显示「重启后生效」。",
                                fontSize = 13.sp,
                                color = colorScheme.onSurfaceVariantSummary,
                            )
                        }
                    }
                }

                item { Spacer(Modifier.height(bottomInnerPadding + 12.dp)) }
            }
        }
    }
}

// ---------------------------------------------------------------- 安装卡

@Composable
private fun InstallCard(
    zipName: String?,
    zipPath: String?,
    zipError: String?,
    installing: Boolean,
    installRaw: String?,
    enabled: Boolean,
    onPick: () -> Unit,
    onInstall: () -> Unit,
    onCancel: () -> Unit,
) {
    Card(modifier = Modifier.fillMaxWidth()) {
        Column {
            BasicComponent(
                title = "选择模块 zip",
                summary = zipName
                    ?: "Magisk/KSU 模块格式；经 AP 用户态安装，App 不直接解压到 /data/adb",
                onClick = onPick,
                startAction = {
                    Icon(
                        imageVector = MiuixIcons.UploadCloud,
                        contentDescription = null,
                        tint = colorScheme.onSurface,
                        modifier = Modifier.padding(end = 12.dp),
                    )
                },
            )
            Column(modifier = Modifier.padding(horizontal = 16.dp, vertical = 8.dp)) {
                zipPath?.let {
                    Text(
                        text = "临时副本：$it",
                        fontSize = 12.sp,
                        fontFamily = FontFamily.Monospace,
                        color = colorScheme.onSurfaceVariantSummary,
                    )
                }
                zipError?.let {
                    Text(text = "校验失败：$it", fontSize = 13.sp, color = colorScheme.error)
                }
                Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    TextButton(
                        text = if (installing) "安装中…（最多 300s）" else "开始安装",
                        onClick = onInstall,
                        enabled = enabled && !installing && zipPath != null,
                        colors = ButtonDefaults.textButtonColorsPrimary(),
                    )
                    TextButton(text = "取消", onClick = onCancel, enabled = !installing)
                }
                if (installing) {
                    Text(
                        text = "正在调用 apd module install；请保持 App 在前台。失败会保留原始输出。",
                        fontSize = 13.sp,
                        color = colorScheme.onSurfaceVariantSummary,
                    )
                }
                installRaw?.let {
                    Spacer(Modifier.height(8.dp))
                    Card(modifier = Modifier.fillMaxWidth()) {
                        Text(
                            text = it,
                            fontSize = 12.sp,
                            fontFamily = FontFamily.Monospace,
                            color = colorScheme.onSurface,
                            modifier = Modifier
                                .fillMaxWidth()
                                .heightIn(max = 240.dp)
                                .verticalScroll(rememberScrollState())
                                .padding(12.dp),
                        )
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------- 操作结果卡

@Composable
private fun ResultCard(
    result: ModulesResult?,
    actionMsg: String?,
    selfTest: ApdSelfTest.Result?,
    showRaw: Boolean,
    onToggleRaw: () -> Unit,
    onCopyRaw: () -> Unit,
) {
    if (result == null && actionMsg == null && selfTest == null) return
    Card(
        modifier = Modifier.fillMaxWidth(),
        colors = CardDefaults.defaultColors(
            color = colorScheme.surfaceContainer,
            contentColor = colorScheme.onSurface,
        ),
    ) {
        Column(modifier = Modifier.padding(16.dp)) {
            selfTest?.let {
                Text(
                    text = "AP 自检：${it.stageLabel} · exit=${it.exitCode}",
                    fontSize = 13.sp,
                    color = if (it.isNormalInitialState) colorScheme.onSurfaceVariantSummary else colorScheme.onSurface,
                )
            }
            actionMsg?.let {
                Text(
                    text = "操作结果：$it",
                    fontSize = 13.sp,
                    color = if (result?.ok == false) colorScheme.error else colorScheme.onSurface,
                )
            }
            result?.hints?.takeIf { it.isNotEmpty() }?.let { hints ->
                Spacer(Modifier.height(6.dp))
                hints.forEachIndexed { index, hint ->
                    Text(
                        text = "可能原因 ${index + 1}：${hint.cause}\n建议：${hint.action}\n依据：${hint.basis}",
                        fontSize = 12.sp,
                        color = colorScheme.onSurface,
                        modifier = Modifier.padding(vertical = 2.dp),
                    )
                }
            }
            result?.parsed?.takeIf { it.isNotEmpty() }?.let { fields ->
                Spacer(Modifier.height(6.dp))
                fields.forEach { (k, v) ->
                    if (k != "error") {
                        Text(
                            text = "$k：$v",
                            fontSize = 12.sp,
                            fontFamily = FontFamily.Monospace,
                            color = colorScheme.onSurfaceVariantSummary,
                        )
                    }
                }
                fields["error"]?.let {
                    Text(text = "error：$it", fontSize = 12.sp, color = colorScheme.error)
                }
            }
            result?.takeIf { !it.ok }?.let {
                Spacer(Modifier.height(4.dp))
                Text(text = "错误：${it.message}", fontSize = 13.sp, color = colorScheme.error)
                Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    TextButton(
                        text = if (showRaw) "收起原始输出" else "查看原始输出",
                        onClick = onToggleRaw,
                    )
                    TextButton(text = "复制原始输出", onClick = onCopyRaw)
                }
                if (showRaw) {
                    Card(modifier = Modifier.fillMaxWidth()) {
                        Text(
                            text = (it.rawStdout.ifBlank { "(stdout 为空)" }) +
                                if (it.rawStderr.isNotBlank()) "\n[stderr]\n" + it.rawStderr else "",
                            fontSize = 12.sp,
                            fontFamily = FontFamily.Monospace,
                            color = colorScheme.onSurface,
                            modifier = Modifier
                                .fillMaxWidth()
                                .heightIn(max = 300.dp)
                                .verticalScroll(rememberScrollState())
                                .padding(12.dp),
                        )
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------- 列表头卡

@Composable
private fun ListHeaderCard(
    loading: Boolean,
    moduleCount: Int?,
    enabled: Boolean,
    onRefresh: () -> Unit,
    onSelfTest: () -> Unit,
) {
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(16.dp)) {
            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(
                    text = "已安装模块",
                    fontSize = MiuixTheme.textStyles.headline1.fontSize,
                    fontWeight = FontWeight.Medium,
                    color = colorScheme.onSurface,
                    modifier = Modifier.weight(1f),
                )
                Text(
                    text = when {
                        loading -> "加载中…"
                        moduleCount != null -> "$moduleCount 个"
                        else -> "未加载"
                    },
                    fontSize = 13.sp,
                    color = colorScheme.onSurfaceVariantSummary,
                )
            }
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                TextButton(
                    text = if (loading) "刷新中…" else "刷新列表",
                    onClick = onRefresh,
                    enabled = enabled,
                    colors = ButtonDefaults.textButtonColorsPrimary(),
                )
                TextButton(text = "重跑 AP 自检", onClick = onSelfTest, enabled = !loading)
            }
        }
    }
}

// ---------------------------------------------------------------- 模块条目

/** 单个模块卡：照抄 KSU `ModuleItem` 的「名称/版本/作者 + Switch + 分隔线 + 操作」结构。 */
@Composable
private fun ModuleItem(
    module: ModuleInfo,
    enabled: Boolean,
    confirmUninstall: Boolean,
    onToggle: (Boolean) -> Unit,
    onUninstall: () -> Unit,
) {
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .padding(top = 12.dp),
        insideMargin = PaddingValues(16.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(modifier = Modifier.weight(1f).padding(end = 8.dp)) {
                Text(
                    text = module.name,
                    fontSize = 17.sp,
                    fontWeight = FontWeight(550),
                    color = colorScheme.onSurface,
                )
                Text(
                    text = "版本：${module.version}（code ${module.versionCode}）",
                    fontSize = 12.sp,
                    fontWeight = FontWeight(550),
                    color = colorScheme.onSurfaceVariantSummary,
                )
                Text(
                    text = "作者：${module.author}",
                    fontSize = 12.sp,
                    fontWeight = FontWeight(550),
                    color = colorScheme.onSurfaceVariantSummary,
                )
            }
            if (module.updatePending) {
                StatusTag(
                    label = "有更新",
                    backgroundColor = colorScheme.tertiaryContainer,
                    contentColor = colorScheme.onTertiaryContainer,
                )
                Spacer(Modifier.size(6.dp))
            }
            Switch(
                checked = module.enabled,
                onCheckedChange = onToggle,
                enabled = enabled,
            )
        }
        if (module.description.isNotBlank()) {
            Text(
                text = module.description,
                fontSize = 14.sp,
                color = colorScheme.onSurfaceVariantSummary,
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(top = 2.dp),
            )
        }
        HorizontalDivider(
            modifier = Modifier.padding(vertical = 8.dp),
            thickness = 0.5.dp,
            color = colorScheme.outline.copy(alpha = 0.5f),
        )
        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            TextButton(
                text = if (confirmUninstall) "确认卸载" else "卸载",
                onClick = onUninstall,
                enabled = enabled,
                colors = if (confirmUninstall) {
                    ButtonDefaults.textButtonColorsPrimary()
                } else {
                    ButtonDefaults.textButtonColors()
                },
            )
            Text(
                text = if (module.enabled) "已启用" else "已禁用",
                fontSize = 12.sp,
                color = if (module.enabled) colorScheme.primary else colorScheme.onSurfaceVariantSummary,
                modifier = Modifier.align(Alignment.CenterVertically),
            )
            if (module.installStatus.isNotBlank()) {
                Text(
                    text = module.installStatus,
                    fontSize = 12.sp,
                    color = colorScheme.onSurfaceVariantSummary,
                    modifier = Modifier.align(Alignment.CenterVertically),
                )
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
