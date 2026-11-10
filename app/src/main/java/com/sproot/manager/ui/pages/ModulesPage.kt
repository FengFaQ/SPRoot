package com.sproot.manager.ui.pages

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.OpenableColumns
import androidx.activity.compose.BackHandler
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.background
import androidx.compose.foundation.border
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
import androidx.compose.foundation.layout.fillMaxSize
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
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
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
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.sproot.manager.core.ApdClient
import com.sproot.manager.core.AppLog
import com.sproot.manager.core.Capabilities
import com.sproot.manager.core.ModuleInfo
import com.sproot.manager.core.ModuleRepository
import com.sproot.manager.core.ModulesResult
import com.sproot.manager.core.SecretStore
import com.sproot.manager.ui.components.BlurredBar
import com.sproot.manager.ui.components.ConfirmDialog
import com.sproot.manager.ui.components.StatusTag
import com.sproot.manager.ui.components.WarningCard
import com.sproot.manager.ui.components.WarningLevel
import com.sproot.manager.ui.components.copyToClipboard
import com.sproot.manager.ui.components.rememberBlurBackdrop
import com.sproot.manager.ui.theme.LocalEnableBlur
import top.yukonga.miuix.kmp.basic.Card
import top.yukonga.miuix.kmp.basic.CardDefaults
import top.yukonga.miuix.kmp.basic.DropdownImpl
import top.yukonga.miuix.kmp.basic.FloatingActionButton
import top.yukonga.miuix.kmp.basic.HorizontalDivider
import top.yukonga.miuix.kmp.basic.Icon
import top.yukonga.miuix.kmp.basic.IconButton
import top.yukonga.miuix.kmp.basic.ListPopupColumn
import top.yukonga.miuix.kmp.basic.MiuixScrollBehavior
import top.yukonga.miuix.kmp.basic.PopupPositionProvider
import top.yukonga.miuix.kmp.basic.PullToRefresh
import top.yukonga.miuix.kmp.basic.Scaffold
import top.yukonga.miuix.kmp.basic.Switch
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.basic.TextButton
import top.yukonga.miuix.kmp.basic.TopAppBar
import top.yukonga.miuix.kmp.basic.rememberPullToRefreshState
import top.yukonga.miuix.kmp.blur.layerBackdrop
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.basic.Search
import top.yukonga.miuix.kmp.icon.extended.Add
import top.yukonga.miuix.kmp.icon.extended.Back
import top.yukonga.miuix.kmp.icon.extended.Delete
import top.yukonga.miuix.kmp.icon.extended.Layers
import top.yukonga.miuix.kmp.icon.extended.Refresh
import top.yukonga.miuix.kmp.icon.extended.Sort
import top.yukonga.miuix.kmp.icon.extended.UploadCloud
import top.yukonga.miuix.kmp.overlay.OverlayListPopup
import top.yukonga.miuix.kmp.preference.ArrowPreference
import top.yukonga.miuix.kmp.preference.SwitchPreference
import top.yukonga.miuix.kmp.theme.MiuixTheme
import top.yukonga.miuix.kmp.theme.MiuixTheme.colorScheme
import top.yukonga.miuix.kmp.utils.overScrollVertical
import top.yukonga.miuix.kmp.utils.scrollEndHaptic

/** 模块列表排序方式（对应 KSU 的 sortEnabledFirst/sortActionFirst 两个菜单项的简化版）。 */
private enum class ModuleSort(val label: String) {
    GROUPED("按启用状态分组"),
    NAME("按名称排序"),
    UPDATE("有更新优先"),
}

/**
 * 模块页 —— 按 KernelSU `ui/screen/module/ModuleMiuix.kt` 的结构重做（人类指令 2026-11-10：
 * 「完全照抄 KSU，而不是现在的样子」）：
 *
 * ```
 * Scaffold(BlurredBar(TopAppBar("模块" + 排序菜单 + 刷新)) + FAB「安装模块」)
 *  └─ PullToRefresh
 *      └─ LazyColumn(12dp 横距)
 *          ├─ 搜索栏（BasicTextField + Search 图标，按 名称/ID/作者/描述 过滤）
 *          ├─ 【已启用】分区头 + ModuleItem 卡 × N
 *          ├─ 【已禁用】分区头 + ModuleItem 卡 × N
 *          ├─ 空态（大图标 + 文案）
 *          └─ 安装中/安装结果卡（保留原始输出，可复制）
 * ```
 *
 * ModuleItem 与 KSU 一致：卡内左侧「图标 + 名称/版本/作者/描述」，右侧 Switch；
 * **点卡片进入详情页**（不是点开关）；底部细分隔线 + 卸载入口（二次确认）。
 * 详情页同样是 KSU 的二级页结构（返回键 + 卡片 + 操作项）。
 *
 * 业务数据仍走 `ModuleRepository`（`apd module ...` root-exec），逻辑未改；
 * 仅把「安装」从页面内大卡片改为 KSU 式 FAB（选 zip → 校验 → 安装）。
 */
@Composable
fun ModulePager(
    bottomInnerPadding: Dp = 0.dp,
    isCurrentPage: Boolean = true,
) {
    val context = LocalContext.current
    // 分层铁律（NEW-DOC-01 §3.1）：组合期不碰 Keystore、不起进程（与 AuthPage/SettingsPage 同一修法）。
    // getOrCreateRootKey() 会进 Keystore，ApdClient.status() 会做 ELF 探测 —— 都必须在 IO 线程。
    // probed 记录「探测已完成」，用于驱动 LaunchedEffect 里的首次 refresh()；
    // 结果与改动前一致（仍然是 apd 就绪 + key 可用才拉列表），只是不再阻塞主线程。
    var key by remember { mutableStateOf<String?>(null) }
    var apdReady by remember { mutableStateOf(false) }
    var probed by remember { mutableStateOf(false) }
    LaunchedEffect(Unit) {
        // 与本文件其它加载逻辑一致：后台线程取 key / 探测 apd，主线程零阻塞。
        Thread {
            val loadedKey = runCatching { SecretStore.get(context).getOrCreateRootKey() }.getOrNull()
            val ready = runCatching { ApdClient.status(context).ok }.getOrDefault(false)
            key = loadedKey
            apdReady = ready
            probed = true
        }.start()
    }

    var result by remember { mutableStateOf<ModulesResult?>(null) }
    var loading by remember { mutableStateOf(false) }

    var zipName by remember { mutableStateOf<String?>(null) }
    var installing by remember { mutableStateOf(false) }
    var installRaw by remember { mutableStateOf<String?>(null) }
    var installError by remember { mutableStateOf<String?>(null) }

    var query by remember { mutableStateOf("") }
    var sort by remember { mutableStateOf(ModuleSort.GROUPED) }
    var showSortPopup by remember { mutableStateOf(false) }
    var detailModule by remember { mutableStateOf<ModuleInfo?>(null) }
    var confirmUninstall by remember { mutableStateOf<ModuleInfo?>(null) }

    val scrollBehavior = MiuixScrollBehavior()
    val backdrop = rememberBlurBackdrop(LocalEnableBlur.current)
    val barColor = if (backdrop != null) Color.Transparent else colorScheme.surface
    val pullState = rememberPullToRefreshState()

    fun refresh() {
        // 委托属性（var key by remember）无法智能转换，先取局部快照再判空。
        val k = key ?: return
        if (loading) return
        loading = true
        Thread {
            val r = ModuleRepository.list(context, k)
            result = r
            loading = false
            if (!r.ok) AppLog.error("Modules", "模块列表失败：${r.message}")
        }.start()
    }

    /** 选 zip → 复制到私有 cache → 校验 PK 头/大小 → apd module install（与旧逻辑一致）。 */
    fun installFromUri(uri: Uri) {
        val k = key ?: run {
            installError = "Root 密钥不可用：请到「设置 → Root 密钥」重新生成"
            return
        }
        installing = true
        installRaw = null
        installError = null
        Thread {
            runCatching {
                val name = queryDisplayName(context, uri) ?: "module.zip"
                val target = java.io.File(context.cacheDir, "module-upload-${System.currentTimeMillis()}.zip")
                context.contentResolver.openInputStream(uri)?.use { input ->
                    target.outputStream().use { out -> input.copyTo(out) }
                } ?: throw IllegalStateException("无法读取所选文件")
                zipName = name
                val error = ModuleRepository.validateZip(target)
                if (error != null) {
                    target.delete()
                    installError = "校验失败：$error"
                    installing = false
                    return@Thread
                }
                AppLog.info("Modules", "已选择模块包：$name（${Capabilities.formatBytes(target.length())}）")
                val r = ModuleRepository.install(context, k, target)
                installRaw = "exit=${r.exitCode}：${r.message}\n" +
                    (r.rawStdout.takeIf { it.isNotBlank() }?.let { "[stdout]\n$it" } ?: "") +
                    (r.rawStderr.takeIf { it.isNotBlank() }?.let { "\n[stderr]\n$it" } ?: "")
                if (r.ok) {
                    target.delete()
                    zipName = null
                    refresh()
                } else {
                    installError = r.message
                }
            }.onFailure {
                installError = "安装异常：${it.javaClass.simpleName}: ${it.message}"
                AppLog.error("Modules", "安装模块失败", it)
            }
            installing = false
        }.start()
    }

    val picker = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri: Uri? ->
        if (uri == null) {
            AppLog.warn("Modules", "用户取消了模块 zip 选择")
            return@rememberLauncherForActivityResult
        }
        runCatching {
            context.contentResolver.takePersistableUriPermission(
                uri,
                Intent.FLAG_GRANT_READ_URI_PERMISSION,
            )
        }
        installFromUri(uri)
    }

    LaunchedEffect(isCurrentPage, probed) {
        if (isCurrentPage && probed && apdReady && key != null && result == null && !loading) refresh()
    }

    // ---------------- 详情页（二级页，KSU ModuleMiuix 的详情等价物） ----------------
    val detail = detailModule
    if (detail != null) {
        BackHandler(enabled = true) { detailModule = null }
        ModuleDetailPager(
            module = detail,
            enabled = apdReady && key != null && !loading && !installing,
            onBack = { detailModule = null },
            onToggle = { enable ->
                val k = key ?: return@ModuleDetailPager
                Thread {
                    val r = ModuleRepository.setEnabled(context, k, detail.id, enable)
                    if (r.ok) {
                        refresh()
                        detailModule = detail.copy(enabled = enable)
                    } else {
                        AppLog.error("Modules", r.message)
                    }
                }.start()
            },
            onUninstall = { confirmUninstall = detail },
        )
        ConfirmDialog(
            show = confirmUninstall != null,
            title = "卸载模块",
            message = confirmUninstall?.let {
                "将卸载「${it.name}」（id=${it.id}）。AP 模块变更通常在下次重启后生效，可稍后用「已刷入/重启」验证。"
            },
            confirmText = "卸载",
            onConfirm = {
                val target = confirmUninstall
                confirmUninstall = null
                val k = key
                if (target != null && k != null) {
                    Thread {
                        val r = ModuleRepository.uninstall(context, k, target.id)
                        if (r.ok) {
                            detailModule = null
                            refresh()
                        } else {
                            AppLog.error("Modules", r.message)
                        }
                    }.start()
                }
            },
            onDismiss = { confirmUninstall = null },
        )
        return
    }

    Scaffold(
        topBar = {
            BlurredBar(backdrop) {
                TopAppBar(
                    color = barColor,
                    title = "模块",
                    actions = {
                        Box {
                            IconButton(onClick = { showSortPopup = true }) {
                                Icon(
                                    imageVector = MiuixIcons.Sort,
                                    contentDescription = "排序 / 筛选",
                                    tint = colorScheme.onSurface,
                                )
                            }
                            OverlayListPopup(
                                show = showSortPopup,
                                popupPositionProvider = top.yukonga.miuix.kmp.basic.ListPopupDefaults.ContextMenuPositionProvider,
                                alignment = PopupPositionProvider.Align.TopEnd,
                                onDismissRequest = { showSortPopup = false },
                                content = {
                                    ListPopupColumn {
                                        ModuleSort.entries.forEachIndexed { index, option ->
                                            DropdownImpl(
                                                text = option.label,
                                                optionSize = ModuleSort.entries.size,
                                                isSelected = sort == option,
                                                onSelectedIndexChange = {
                                                    sort = option
                                                    showSortPopup = false
                                                },
                                                index = index,
                                            )
                                        }
                                    }
                                },
                            )
                        }
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
        floatingActionButton = {
            FloatingActionButton(
                modifier = Modifier
                    .padding(bottom = bottomInnerPadding + 20.dp, end = 20.dp)
                    .border(0.05.dp, colorScheme.outline.copy(alpha = 0.5f), CircleShape),
                shadowElevation = 0.dp,
                onClick = { picker.launch(arrayOf("application/zip", "application/octet-stream", "*/*")) },
            ) {
                Icon(
                    imageVector = MiuixIcons.Add,
                    contentDescription = "安装模块",
                    modifier = Modifier.size(32.dp),
                    tint = colorScheme.onPrimary,
                )
            }
        },
        popupHost = { },
        contentWindowInsets = WindowInsets.systemBars.add(WindowInsets.displayCutout)
            .only(WindowInsetsSides.Horizontal),
    ) { innerPadding ->
        Box(modifier = if (backdrop != null) Modifier.layerBackdrop(backdrop) else Modifier) {
            val refreshTexts = listOf("下拉刷新", "释放刷新", "正在刷新", "刷新完成")
            PullToRefresh(
                isRefreshing = loading,
                pullToRefreshState = pullState,
                onRefresh = { refresh() },
                refreshTexts = refreshTexts,
                modifier = Modifier.fillMaxSize(),
                contentPadding = PaddingValues(
                    top = innerPadding.calculateTopPadding(),
                    bottom = bottomInnerPadding + 88.dp,
                ),
            ) {
                val modules = result?.modules.orEmpty()
                val filtered = remember(modules, query, sort) { filterAndSort(modules, query, sort) }
                val enabledModules = filtered.filter { it.enabled }
                val disabledModules = filtered.filter { !it.enabled }

                LazyColumn(
                    modifier = Modifier
                        .fillMaxHeight()
                        .scrollEndHaptic()
                        .overScrollVertical()
                        .nestedScroll(scrollBehavior.nestedScrollConnection)
                        .padding(horizontal = 12.dp),
                    contentPadding = PaddingValues(
                        top = innerPadding.calculateTopPadding() + 8.dp,
                        bottom = bottomInnerPadding + 96.dp,
                    ),
                    overscrollEffect = null,
                ) {
                    item { SearchCard(query = query, onQueryChange = { query = it }) }

                    // probed 前不画这张卡：apd 探测在 IO 线程，组合期结果尚未就绪，
                    // 否则会先闪一下「暂不可用」再消失（伪装成状态变化）。
                    if (probed && !apdReady) {
                        item {
                            WarningCard(
                                message = "模块功能暂不可用：尚未部署 AP 守护（需先刷入修补后的镜像）；" +
                                    "部署完成后列表与操作才会启用",
                                level = WarningLevel.Notice,
                            )
                        }
                    }

                    // 安装中 / 安装结果（对应 KSU 的 flash 流程结果，保留原始输出）
                    if (installing || installRaw != null || installError != null) {
                        item {
                            InstallResultCard(
                                installing = installing,
                                zipName = zipName,
                                error = installError,
                                raw = installRaw,
                                onCopyRaw = {
                                    context.copyToClipboard(
                                        "模块安装原始输出",
                                        installRaw ?: installError ?: "",
                                    )
                                },
                            )
                        }
                    }

                    when {
                        // probed 前（Keystore/apd 探测还在 IO 线程）也按加载中展示：
                        // 否则会先闪一帧「没有安装任何模块」再变成加载态。
                        !probed -> {
                            item { LoadingRow() }
                        }

                        result == null && apdReady && key != null -> {
                            item { LoadingRow() }
                        }

                        loading && modules.isEmpty() -> {
                            item { LoadingRow() }
                        }

                        result?.ok == false -> {
                            item {
                                WarningCard(
                                    message = "模块列表加载失败：${result?.message ?: "未知错误"}",
                                    level = WarningLevel.Error,
                                )
                            }
                        }

                        modules.isEmpty() -> {
                            item { EmptyState(query.isNotBlank()) }
                        }

                        else -> {
                            if (enabledModules.isNotEmpty()) {
                                item { SectionHeader("已启用", enabledModules.size) }
                                items(enabledModules, key = { "enabled-${it.id}" }) { module ->
                                    ModuleItemCard(
                                        module = module,
                                        enabled = apdReady && key != null && !loading && !installing,
                                        onOpenDetail = { detailModule = module },
                                        onToggle = { enable ->
                                            val k = key ?: return@ModuleItemCard
                                            Thread {
                                                val r = ModuleRepository.setEnabled(context, k, module.id, enable)
                                                if (r.ok) refresh() else AppLog.error("Modules", r.message)
                                            }.start()
                                        },
                                        onUninstall = {
                                            detailModule = module
                                            confirmUninstall = module
                                        },
                                    )
                                }
                            }
                            if (disabledModules.isNotEmpty()) {
                                item { SectionHeader("已禁用", disabledModules.size) }
                                items(disabledModules, key = { "disabled-${it.id}" }) { module ->
                                    ModuleItemCard(
                                        module = module,
                                        enabled = apdReady && key != null && !loading && !installing,
                                        onOpenDetail = { detailModule = module },
                                        onToggle = { enable ->
                                            val k = key ?: return@ModuleItemCard
                                            Thread {
                                                val r = ModuleRepository.setEnabled(context, k, module.id, enable)
                                                if (r.ok) refresh() else AppLog.error("Modules", r.message)
                                            }.start()
                                        },
                                        onUninstall = {
                                            detailModule = module
                                            confirmUninstall = module
                                        },
                                    )
                                }
                            }
                            if (filtered.isEmpty()) {
                                item { EmptyState(true) }
                            }
                        }
                    }

                    item { Spacer(Modifier.height(8.dp)) }
                }
            }
        }
    }
}

// ---------------------------------------------------------------- 搜索栏

/** 顶部搜索栏：KSU 的 `SuperSearchBar` 的静态简化版（图标 + 输入框 + 清空）。 */
@Composable
private fun SearchCard(query: String, onQueryChange: (String) -> Unit) {
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .padding(bottom = 12.dp),
        insideMargin = PaddingValues(horizontal = 12.dp, vertical = 4.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Icon(
                imageVector = MiuixIcons.Basic.Search,
                contentDescription = null,
                tint = colorScheme.onSurfaceVariantSummary,
                modifier = Modifier
                    .padding(end = 8.dp)
                    .size(20.dp),
            )
            Box(modifier = Modifier.weight(1f)) {
                if (query.isEmpty()) {
                    Text(
                        text = "搜索模块（名称 / id / 作者 / 描述）",
                        fontSize = 15.sp,
                        color = colorScheme.onSurfaceVariantSummary,
                    )
                }
                BasicTextField(
                    value = query,
                    onValueChange = onQueryChange,
                    modifier = Modifier.fillMaxWidth(),
                    singleLine = true,
                    textStyle = TextStyle(
                        fontSize = 15.sp,
                        color = colorScheme.onSurface,
                    ),
                    cursorBrush = SolidColor(colorScheme.primary),
                )
            }
            if (query.isNotEmpty()) {
                TextButton(text = "清空", onClick = { onQueryChange("") })
            }
        }
    }
}

// ---------------------------------------------------------------- 分区头

@Composable
private fun SectionHeader(title: String, count: Int) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(start = 4.dp, end = 4.dp, top = 4.dp, bottom = 8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            text = title,
            fontSize = 14.sp,
            fontWeight = FontWeight.Medium,
            color = colorScheme.onSurface,
        )
        Spacer(Modifier.size(6.dp))
        Text(
            text = "$count",
            fontSize = 13.sp,
            color = colorScheme.onSurfaceVariantSummary,
        )
    }
}

// ---------------------------------------------------------------- 模块条目

/**
 * 单个模块卡 —— 结构照抄 KSU `ModuleItem`：
 * 左侧「图标 + 名称 + 版本 + 作者 + 描述」，右侧 Switch；点卡片进详情；
 * 底部细分隔线 + 卸载入口（确认在详情/列表二次点击后弹对话框）。
 */
@Composable
private fun ModuleItemCard(
    module: ModuleInfo,
    enabled: Boolean,
    onOpenDetail: () -> Unit,
    onToggle: (Boolean) -> Unit,
    onUninstall: () -> Unit,
) {
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .padding(bottom = 12.dp),
        insideMargin = PaddingValues(16.dp),
        onClick = onOpenDetail,
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            ModuleIcon()
            Column(
                modifier = Modifier
                    .weight(1f)
                    .padding(start = 12.dp, end = 8.dp),
            ) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(
                        text = module.name,
                        fontSize = 17.sp,
                        fontWeight = FontWeight(550),
                        color = colorScheme.onSurface,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.weight(1f, fill = false),
                    )
                    if (module.updatePending) {
                        Spacer(Modifier.size(6.dp))
                        StatusTag(
                            label = "有更新",
                            backgroundColor = colorScheme.tertiaryContainer,
                            contentColor = colorScheme.onTertiaryContainer,
                        )
                    }
                }
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
                if (module.description.isNotBlank()) {
                    Text(
                        text = module.description,
                        fontSize = 13.sp,
                        color = colorScheme.onSurfaceVariantSummary,
                        maxLines = 2,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.padding(top = 2.dp),
                    )
                }
            }
            Switch(
                checked = module.enabled,
                onCheckedChange = onToggle,
                enabled = enabled,
            )
        }
        HorizontalDivider(
            modifier = Modifier.padding(vertical = 8.dp),
            thickness = 0.5.dp,
            color = colorScheme.outline.copy(alpha = 0.5f),
        )
        Row(verticalAlignment = Alignment.CenterVertically) {
            Text(
                text = if (module.enabled) "已启用" else "已禁用",
                fontSize = 12.sp,
                color = if (module.enabled) colorScheme.primary else colorScheme.onSurfaceVariantSummary,
            )
            if (module.installStatus.isNotBlank()) {
                Spacer(Modifier.size(8.dp))
                Text(
                    text = module.installStatus,
                    fontSize = 12.sp,
                    color = colorScheme.onSurfaceVariantSummary,
                )
            }
            Spacer(Modifier.weight(1f))
            TextButton(
                text = "详情",
                onClick = onOpenDetail,
                enabled = enabled,
            )
            TextButton(
                text = "卸载",
                onClick = onUninstall,
                enabled = enabled,
            )
        }
    }
}

/** 模块图标：KSU 用模块自带图标；我们后端暂无图标字段，用统一的 Layers 图标占位。 */
@Composable
private fun ModuleIcon() {
    Box(
        modifier = Modifier
            .size(44.dp)
            .background(colorScheme.secondaryContainer.copy(alpha = 0.8f), RoundedCornerShape(12.dp)),
        contentAlignment = Alignment.Center,
    ) {
        Icon(
            imageVector = MiuixIcons.Layers,
            contentDescription = null,
            tint = colorScheme.onSecondaryContainer,
            modifier = Modifier.size(24.dp),
        )
    }
}

// ---------------------------------------------------------------- 详情页

/**
 * 模块详情（KSU `ModuleMiuix` 的详情/`ExecuteModuleActionMiuix` 操作页的简化版）：
 * 头部信息卡 + 操作卡（启用/禁用、卸载）+ 原始字段卡。
 */
@Composable
private fun ModuleDetailPager(
    module: ModuleInfo,
    enabled: Boolean,
    onBack: () -> Unit,
    onToggle: (Boolean) -> Unit,
    onUninstall: () -> Unit,
) {
    val context = LocalContext.current
    val scrollBehavior = MiuixScrollBehavior()
    val backdrop = rememberBlurBackdrop(LocalEnableBlur.current)
    val barColor = if (backdrop != null) Color.Transparent else colorScheme.surface

    Scaffold(
        topBar = {
            BlurredBar(backdrop) {
                TopAppBar(
                    color = barColor,
                    title = module.name,
                    navigationIcon = {
                        IconButton(onClick = onBack) {
                            Icon(
                                imageVector = MiuixIcons.Back,
                                contentDescription = "返回模块列表",
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
                contentPadding = PaddingValues(
                    top = innerPadding.calculateTopPadding() + 12.dp,
                    bottom = 24.dp,
                ),
                overscrollEffect = null,
            ) {
                item {
                    Card(modifier = Modifier.fillMaxWidth()) {
                        Column(modifier = Modifier.padding(16.dp)) {
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                ModuleIcon()
                                Column(modifier = Modifier.padding(start = 12.dp)) {
                                    Row(verticalAlignment = Alignment.CenterVertically) {
                                        Text(
                                            text = module.name,
                                            fontSize = 20.sp,
                                            fontWeight = FontWeight.SemiBold,
                                            color = colorScheme.onSurface,
                                        )
                                        if (module.updatePending) {
                                            Spacer(Modifier.size(6.dp))
                                            StatusTag(
                                                label = "有更新",
                                                backgroundColor = colorScheme.tertiaryContainer,
                                                contentColor = colorScheme.onTertiaryContainer,
                                            )
                                        }
                                    }
                                    Text(
                                        text = "版本 ${module.version}（code ${module.versionCode}）· ${module.author}",
                                        fontSize = 13.sp,
                                        color = colorScheme.onSurfaceVariantSummary,
                                    )
                                    Text(
                                        text = if (module.enabled) "已启用" else "已禁用",
                                        fontSize = 13.sp,
                                        color = if (module.enabled) colorScheme.primary else colorScheme.onSurfaceVariantSummary,
                                    )
                                }
                            }
                            if (module.description.isNotBlank()) {
                                Spacer(Modifier.height(10.dp))
                                Text(
                                    text = module.description,
                                    fontSize = 14.sp,
                                    color = colorScheme.onSurfaceVariantSummary,
                                )
                            }
                        }
                    }
                }
                item { Spacer(Modifier.height(12.dp)) }
                item {
                    Card(modifier = Modifier.fillMaxWidth()) {
                        SwitchPreference(
                            title = "启用模块",
                            summary = "启用/禁用由 AP 用户态处理，通常在下次重启后生效",
                            checked = module.enabled,
                            onCheckedChange = onToggle,
                        )
                        ArrowPreference(
                            title = "卸载模块",
                            summary = "移除模块目录并标记 remove_pending，重启后生效",
                            startAction = {
                                Icon(
                                    imageVector = MiuixIcons.Delete,
                                    contentDescription = null,
                                    tint = colorScheme.error,
                                    modifier = Modifier
                                        .padding(end = 12.dp)
                                        .size(24.dp),
                                )
                            },
                            onClick = { if (enabled) onUninstall() },
                        )
                    }
                }
                item { Spacer(Modifier.height(12.dp)) }
                item {
                    Card(modifier = Modifier.fillMaxWidth()) {
                        Column(modifier = Modifier.padding(16.dp)) {
                            Text(
                                text = "模块信息",
                                fontSize = 15.sp,
                                fontWeight = FontWeight.Medium,
                                color = colorScheme.onSurface,
                            )
                            InfoLine("id", module.id, monospace = true)
                            InfoLine("version", module.version)
                            InfoLine("versionCode", module.versionCode)
                            InfoLine("author", module.author)
                            InfoLine("enabled", module.enabled.toString())
                            InfoLine("update_pending", module.updatePending.toString())
                            if (module.installStatus.isNotBlank()) {
                                InfoLine("install_status", module.installStatus)
                            }
                            TextButton(
                                text = "复制 id",
                                onClick = { context.copyToClipboard("模块 id", module.id) },
                            )
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun InfoLine(label: String, value: String, monospace: Boolean = false) {
    Column(modifier = Modifier.padding(vertical = 3.dp)) {
        Text(
            text = label,
            fontSize = 12.sp,
            color = colorScheme.onSurfaceVariantSummary,
        )
        Text(
            text = value,
            fontSize = 14.sp,
            fontFamily = if (monospace) FontFamily.Monospace else FontFamily.Default,
            color = colorScheme.onSurface,
        )
    }
}

// ---------------------------------------------------------------- 安装/结果/空态

@Composable
private fun InstallResultCard(
    installing: Boolean,
    zipName: String?,
    error: String?,
    raw: String?,
    onCopyRaw: () -> Unit,
) {
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .padding(bottom = 12.dp),
        colors = CardDefaults.defaultColors(
            color = colorScheme.surfaceContainer,
            contentColor = colorScheme.onSurface,
        ),
    ) {
        Column(modifier = Modifier.padding(16.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Icon(
                    imageVector = MiuixIcons.UploadCloud,
                    contentDescription = null,
                    tint = colorScheme.onSurface,
                    modifier = Modifier
                        .padding(end = 8.dp)
                        .size(20.dp),
                )
                Text(
                    text = when {
                        installing -> "正在安装模块…（最多 300s）"
                        error != null -> "安装失败"
                        else -> "安装结果"
                    },
                    fontSize = 15.sp,
                    fontWeight = FontWeight.Medium,
                    color = colorScheme.onSurface,
                )
            }
            zipName?.let {
                Text(
                    text = "模块包：$it",
                    fontSize = 12.sp,
                    color = colorScheme.onSurfaceVariantSummary,
                )
            }
            installing.takeIf { it }?.let {
                Text(
                    text = "请保持 App 在前台；失败会保留原始输出。",
                    fontSize = 12.sp,
                    color = colorScheme.onSurfaceVariantSummary,
                )
            }
            error?.let {
                Text(text = it, fontSize = 13.sp, color = colorScheme.error)
            }
            raw?.let {
                Spacer(Modifier.height(6.dp))
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
                TextButton(text = "复制原始输出", onClick = onCopyRaw)
            }
        }
    }
}

@Composable
private fun LoadingRow() {
    Box(
        modifier = Modifier
            .fillMaxWidth()
            .padding(vertical = 48.dp),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text = "正在加载模块列表…",
            fontSize = 14.sp,
            color = colorScheme.onSurfaceVariantSummary,
        )
    }
}

/** 空态：大图标 + 文案（搜索无结果与"没有安装模块"用不同文案）。 */
@Composable
private fun EmptyState(searchEmpty: Boolean) {
    Column(
        modifier = Modifier
            .fillMaxWidth()
            .padding(vertical = 56.dp),
        horizontalAlignment = Alignment.CenterHorizontally,
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        Box(
            modifier = Modifier
                .size(88.dp)
                .background(colorScheme.surfaceContainer, CircleShape),
            contentAlignment = Alignment.Center,
        ) {
            Icon(
                imageVector = MiuixIcons.Layers,
                contentDescription = null,
                tint = colorScheme.onSurfaceVariantSummary,
                modifier = Modifier.size(40.dp),
            )
        }
        Text(
            text = if (searchEmpty) "没有匹配的模块" else "尚未安装任何模块",
            fontSize = 16.sp,
            fontWeight = FontWeight.Medium,
            color = colorScheme.onSurface,
        )
        Text(
            text = if (searchEmpty) {
                "换个关键词试试；搜索覆盖名称 / id / 作者 / 描述"
            } else {
                "点击右下角「+」选择模块 zip 安装；AP 未部署前安装入口不可用"
            },
            fontSize = 13.sp,
            color = colorScheme.onSurfaceVariantSummary,
        )
    }
}

// ---------------------------------------------------------------- 工具

/** 搜索过滤 + 排序（对应 KSU 的搜索/排序菜单能力，纯客户端计算）。 */
private fun filterAndSort(
    modules: List<ModuleInfo>,
    query: String,
    sort: ModuleSort,
): List<ModuleInfo> {
    val q = query.trim().lowercase()
    val filtered = if (q.isEmpty()) {
        modules
    } else {
        modules.filter { module ->
            module.name.lowercase().contains(q) ||
                module.id.lowercase().contains(q) ||
                module.author.lowercase().contains(q) ||
                module.description.lowercase().contains(q)
        }
    }
    return when (sort) {
        ModuleSort.GROUPED -> filtered.sortedWith(
            compareByDescending<ModuleInfo> { it.enabled }.thenBy { it.name.lowercase() },
        )

        ModuleSort.NAME -> filtered.sortedBy { it.name.lowercase() }
        ModuleSort.UPDATE -> filtered.sortedWith(
            compareByDescending<ModuleInfo> { it.updatePending }.thenBy { it.name.lowercase() },
        )
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
