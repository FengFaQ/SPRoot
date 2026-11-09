package com.sproot.manager.ui.pages

import android.content.Context
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
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
import androidx.compose.foundation.shape.CircleShape
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
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import com.sproot.manager.BuildConfig
import com.sproot.manager.core.ApdClient
import com.sproot.manager.core.ApdSelfTest
import com.sproot.manager.core.AppLog
import com.sproot.manager.core.BackupStore
import com.sproot.manager.core.Capabilities
import com.sproot.manager.core.CapabilityItem
import com.sproot.manager.core.PatchEngine
import com.sproot.manager.core.SecretStore
import com.sproot.manager.ui.components.BlurredBar
import com.sproot.manager.ui.components.ConfirmDialog
import com.sproot.manager.ui.components.copyToClipboard
import com.sproot.manager.ui.components.maskSecret
import com.sproot.manager.ui.components.rememberBlurBackdrop
import com.sproot.manager.ui.theme.AppSettings
import com.sproot.manager.ui.theme.ColorMode
import com.sproot.manager.ui.theme.LocalEnableBlur
import com.sproot.manager.ui.theme.keyColorOptions
import top.yukonga.miuix.kmp.basic.Card
import top.yukonga.miuix.kmp.basic.CardDefaults
import top.yukonga.miuix.kmp.basic.Icon
import top.yukonga.miuix.kmp.basic.MiuixScrollBehavior
import top.yukonga.miuix.kmp.basic.Scaffold
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.basic.TextButton
import top.yukonga.miuix.kmp.basic.TopAppBar
import top.yukonga.miuix.kmp.blur.layerBackdrop
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.extended.Backup
import top.yukonga.miuix.kmp.icon.extended.Delete
import top.yukonga.miuix.kmp.icon.extended.Help
import top.yukonga.miuix.kmp.icon.extended.Info
import top.yukonga.miuix.kmp.icon.extended.Lock
import top.yukonga.miuix.kmp.icon.extended.Notes
import top.yukonga.miuix.kmp.icon.extended.Refresh
import top.yukonga.miuix.kmp.icon.extended.Store
import top.yukonga.miuix.kmp.icon.extended.Theme
import top.yukonga.miuix.kmp.preference.ArrowPreference
import top.yukonga.miuix.kmp.preference.OverlayDropdownPreference
import top.yukonga.miuix.kmp.theme.MiuixTheme
import top.yukonga.miuix.kmp.theme.MiuixTheme.colorScheme
import top.yukonga.miuix.kmp.utils.overScrollVertical
import top.yukonga.miuix.kmp.utils.scrollEndHaptic

/**
 * 设置页 —— 版式照抄 KernelSU `ui/screen/settings/SettingsMiuix.kt`：
 * 每张 Card 内是 miuix `*Preference` 组件（Arrow/下拉/开关），视觉与 KSU 设置页一致。
 *
 * **设置项全部换成 SPRoot 自己的**（KSU 的检查更新/模块更新/UI 模式/授权配置等一个不留）：
 *  主题 → Root 密钥 → 日志 → 原镜像备份 → AP 部署 → 引擎自检 → 工作目录清理 → 关于/许可
 */
@Composable
fun SettingsPager(
    appSettings: AppSettings,
    onSettingsChange: (AppSettings) -> Unit,
    bottomInnerPadding: Dp = 0.dp,
    isCurrentPage: Boolean = true,
    onOpenAbout: () -> Unit = {},
) {
    val context = LocalContext.current
    val store = remember { SecretStore.get(context) }
    var key by remember { mutableStateOf(runCatching { store.readRootKey() }.getOrNull()) }
    var showKey by remember { mutableStateOf(false) }
    var showRotateConfirm by remember { mutableStateOf(false) }
    var showDeleteBackup by remember { mutableStateOf<BackupStore.Backup?>(null) }
    var backups by remember { mutableStateOf<List<BackupStore.Backup>>(emptyList()) }
    var backupMsg by remember { mutableStateOf<String?>(null) }
    var deployResult by remember { mutableStateOf<String?>(null) }
    var deploying by remember { mutableStateOf(false) }
    var engineReport by remember { mutableStateOf<String?>(null) }
    var selfTest by remember { mutableStateOf(ApdSelfTest.load(context)) }
    var apdStatus by remember { mutableStateOf<CapabilityItem?>(null) }
    var apdMismatch by remember { mutableStateOf<String?>(null) }
    var cleanMsg by remember { mutableStateOf<String?>(null) }
    var selfTestRunning by remember { mutableStateOf(false) }

    val scrollBehavior = MiuixScrollBehavior()
    val backdrop = rememberBlurBackdrop(LocalEnableBlur.current)
    val barColor = if (backdrop != null) Color.Transparent else colorScheme.surface

    fun reloadBackups() {
        Thread {
            backups = BackupStore.list(context)
            backupMsg = "已加载 ${backups.size} 条备份"
        }.start()
    }

    LaunchedEffect(isCurrentPage) {
        if (!isCurrentPage) return@LaunchedEffect
        if (backups.isEmpty()) reloadBackups()
        // AP 状态/版本探测会起进程（apd version），必须离开主线程
        Thread {
            apdStatus = runCatching { ApdClient.status(context) }.getOrNull()
            apdMismatch = runCatching { ApdClient.versionMismatchHint(context) }.getOrNull()
        }.start()
    }

    ConfirmDialog(
        show = showRotateConfirm,
        title = "重新生成 Root 密钥",
        message = "旧密钥修补并已刷入的内核将无法提权，必须重新「修补 → 刷入」。确定继续？",
        confirmText = "重新生成",
        onConfirm = {
            showRotateConfirm = false
            key = runCatching { store.rotate() }.onFailure {
                AppLog.error("Settings", "轮换密钥失败", it)
            }.getOrNull()
            AppLog.warn("Settings", "已轮换 Root 密钥：旧密钥修补的内核必须重新修补并刷入")
        },
        onDismiss = { showRotateConfirm = false },
    )
    val pendingDelete = showDeleteBackup
    ConfirmDialog(
        show = pendingDelete != null,
        title = "删除原镜像备份",
        message = pendingDelete?.let { "将删除 sha1=${it.sha1.take(12)}…（${Capabilities.formatBytes(it.size)}）。" },
        confirmText = "删除",
        onConfirm = {
            val target = pendingDelete
            showDeleteBackup = null
            if (target != null) {
                Thread {
                    backupMsg = if (BackupStore.delete(context, target)) {
                        "已删除备份 ${target.sha1.take(8)}"
                    } else {
                        "删除失败"
                    }
                    backups = BackupStore.list(context)
                }.start()
            }
        },
        onDismiss = { showDeleteBackup = null },
    )

    Scaffold(
        topBar = {
            BlurredBar(backdrop) {
                TopAppBar(
                    color = barColor,
                    title = "设置",
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
                        Card(modifier = Modifier.fillMaxWidth()) {
                            OverlayDropdownPreference(
                                title = "主题模式",
                                summary = "默认跟随系统亮暗；浅色/深色可强制",
                                items = listOf("跟随系统", "浅色", "深色"),
                                selectedIndex = appSettings.colorMode.value,
                                startAction = {
                                    Icon(
                                        imageVector = MiuixIcons.Theme,
                                        contentDescription = null,
                                        tint = colorScheme.onSurface,
                                        modifier = Modifier.padding(end = 12.dp),
                                    )
                                },
                                onSelectedIndexChange = {
                                    onSettingsChange(appSettings.copy(colorMode = ColorMode.fromValue(it)))
                                },
                            )
                            KeyColorRow(
                                selected = appSettings.keyColor,
                                onSelect = { onSettingsChange(appSettings.copy(keyColor = it)) },
                            )
                        }

                        RootKeyCard(
                            key = key,
                            showKey = showKey,
                            onToggleShow = { showKey = !showKey },
                            onCopy = { key?.let { context.copyToClipboard("Root 密钥", it) } },
                            onRotate = { showRotateConfirm = true },
                        )

                        LogCard(onCopy = { context.copyToClipboard("全部日志", AppLog.dumpText()) })

                        BackupCard(
                            backups = backups,
                            message = backupMsg,
                            onRefresh = { reloadBackups() },
                            onRestore = { backup ->
                                Thread {
                                    val result = runCatching { BackupStore.restoreToDownloads(context, backup) }
                                    backupMsg = result.fold(
                                        onSuccess = { "已还原到 ${it.displayPath}" },
                                        onFailure = { "还原失败：${it.javaClass.simpleName}: ${it.message}" },
                                    )
                                }.start()
                            },
                            onDelete = { showDeleteBackup = it },
                        )

                        ApdDeployCard(
                            selfTest = selfTest,
                            status = apdStatus,
                            mismatch = apdMismatch,
                            deploying = deploying,
                            deployResult = deployResult,
                            selfTestRunning = selfTestRunning,
                            onDeploy = {
                                if (deploying) return@ApdDeployCard
                                deploying = true
                                deployResult = "部署中…"
                                Thread {
                                    val k = runCatching { store.getOrCreateRootKey() }.getOrNull()
                                    if (k == null) {
                                        deployResult = "失败：Root 密钥不可用"
                                        deploying = false
                                        return@Thread
                                    }
                                    val nativeLib = context.applicationInfo.nativeLibraryDir
                                    val r = ApdClient.apd(
                                        context = context,
                                        superKey = k,
                                        subCommand = listOf("deploy", "--source", nativeLib),
                                        timeoutSec = 120,
                                    )
                                    deployResult = buildString {
                                        append("exit=").append(r.exitCode).append("（").append(r.describe()).append("）")
                                        if (r.stdout.isNotBlank()) append("\n").append(r.stdout.trim())
                                        if (r.stderr.isNotBlank()) append("\nstderr: ").append(r.stderr.trim())
                                    }
                                    deploying = false
                                }.start()
                            },
                            onSelfTest = {
                                if (selfTestRunning) return@ApdDeployCard
                                selfTestRunning = true
                                Thread {
                                    selfTest = ApdSelfTest.run(context)
                                    selfTestRunning = false
                                }.start()
                            },
                        )

                        EngineSelfCheckCard(
                            report = engineReport,
                            onRun = {
                                engineReport = "检查中…"
                                Thread {
                                    val status = PatchEngine.status(context)
                                    val version = runCatching { PatchEngine.version() }.getOrNull()
                                    engineReport = buildString {
                                        append("库路径：").append(status.path).append('\n')
                                        append("存在=").append(status.present)
                                        append("；AArch64=").append(status.aarch64)
                                        append("；已加载=").append(status.loaded).append('\n')
                                        append("engineVersion()=").append(version ?: "<null>").append('\n')
                                        append("结论：").append(status.detail)
                                    }
                                    AppLog.info("Settings", "引擎自检：version=$version loaded=${status.loaded}")
                                }.start()
                            },
                        )

                        WorkdirCard(
                            message = cleanMsg,
                            onClean = {
                                Thread {
                                    val targets = listOf(
                                        java.io.File(context.cacheDir, "patch"),
                                        java.io.File(context.filesDir, "exec-keys"),
                                    )
                                    var freed = 0L
                                    targets.forEach { dir ->
                                        freed += dirSize(dir)
                                        dir.deleteRecursively()
                                    }
                                    cleanMsg = "已清理 ${Capabilities.formatBytes(freed)}（cache/patch + files/exec-keys）"
                                    AppLog.info("Settings", "工作目录清理：释放 ${Capabilities.formatBytes(freed)}")
                                }.start()
                            },
                        )

                        Card(modifier = Modifier.fillMaxWidth()) {
                            ArrowPreference(
                                title = "关于与许可",
                                summary = "版本 ${BuildConfig.VERSION_NAME}（${BuildConfig.BUILD_TYPE}）· " +
                                    "引擎 JNI v${PatchEngine.JNI_BINDING_VERSION} · AGPL-3.0",
                                startAction = {
                                    Icon(
                                        imageVector = MiuixIcons.Help,
                                        contentDescription = null,
                                        tint = colorScheme.onSurface,
                                        modifier = Modifier.padding(end = 12.dp),
                                    )
                                },
                                onClick = onOpenAbout,
                            )
                        }

                        Spacer(Modifier.height(bottomInnerPadding))
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------- 外观

/** 强调色选择：KSU 主题页的 15 色板（ui/theme/Colors.kt），0 = 默认灰阶。 */
@Composable
private fun KeyColorRow(selected: Int, onSelect: (Int) -> Unit) {
    Column(modifier = Modifier.padding(horizontal = 16.dp, vertical = 10.dp)) {
        Text(
            text = "强调色",
            fontSize = 16.sp,
            fontWeight = FontWeight.Medium,
            color = colorScheme.onSurface,
        )
        Text(
            text = "用于按钮/选中态；默认由 miuix 按灰阶生成",
            fontSize = 12.sp,
            color = colorScheme.onSurfaceVariantSummary,
        )
        Spacer(Modifier.height(8.dp))
        Row(
            modifier = Modifier
                .fillMaxWidth()
                .horizontalScroll(rememberScrollState()),
            horizontalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            ColorSwatch(
                color = colorScheme.surfaceContainer,
                selected = selected == 0,
                label = "默认",
                onClick = { onSelect(0) },
            )
            keyColorOptions.forEach { argb ->
                ColorSwatch(
                    color = Color(argb),
                    selected = selected == argb,
                    label = null,
                    onClick = { onSelect(argb) },
                )
            }
        }
    }
}

@Composable
private fun ColorSwatch(color: Color, selected: Boolean, label: String?, onClick: () -> Unit) {
    Box(
        modifier = Modifier
            .size(36.dp)
            .background(color, CircleShape)
            .border(
                width = if (selected) 3.dp else 1.dp,
                color = if (selected) colorScheme.primary else colorScheme.outline.copy(alpha = 0.5f),
                shape = CircleShape,
            )
            .clickable(onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        if (label != null) {
            Text(text = label, fontSize = 10.sp, color = colorScheme.onSurface)
        }
    }
}

// ---------------------------------------------------------------- Root 密钥

@Composable
private fun RootKeyCard(
    key: String?,
    showKey: Boolean,
    onToggleShow: () -> Unit,
    onCopy: () -> Unit,
    onRotate: () -> Unit,
) {
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(16.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Icon(
                    imageVector = MiuixIcons.Lock,
                    contentDescription = null,
                    tint = colorScheme.onSurface,
                    modifier = Modifier
                        .padding(end = 12.dp)
                        .size(24.dp),
                )
                Column {
                    Text(
                        text = "Root 密钥",
                        fontSize = 16.sp,
                        fontWeight = FontWeight.Medium,
                        color = colorScheme.onSurface,
                    )
                    Text(
                        text = "47 字符 [A-Za-z0-9]（内核 ABI 47+NUL）；前 16 位即 /data 隐藏目录前缀",
                        fontSize = 12.sp,
                        color = colorScheme.onSurfaceVariantSummary,
                    )
                }
            }
            Spacer(Modifier.height(8.dp))
            Text(
                text = when {
                    key == null -> "不可用（点击下方重新生成）"
                    showKey -> key
                    else -> maskSecret(key)
                },
                fontSize = 13.sp,
                fontFamily = FontFamily.Monospace,
                color = if (key == null) colorScheme.error else colorScheme.onSurface,
                modifier = Modifier.fillMaxWidth(),
            )
            if (SecretStore.legacy48Detected) {
                Text(
                    text = "检测到历史 48 字符密钥，已取前 47 位使用；建议重新生成密钥并重新修补/刷入镜像",
                    fontSize = 12.sp,
                    color = colorScheme.error,
                )
            }
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                TextButton(text = if (showKey) "隐藏" else "显示", onClick = onToggleShow)
                TextButton(text = "复制", onClick = onCopy, enabled = key != null)
                TextButton(text = "重新生成", onClick = onRotate)
            }
            Text(
                text = "⚠️ 重新生成后，已刷入的旧内核将无法提权，必须重新走一遍「修补 → 刷入」。",
                fontSize = 12.sp,
                color = colorScheme.onSurfaceVariantSummary,
            )
        }
    }
}

// ---------------------------------------------------------------- 日志

@Composable
private fun LogCard(onCopy: () -> Unit) {
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(16.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Icon(
                    imageVector = MiuixIcons.Notes,
                    contentDescription = null,
                    tint = colorScheme.onSurface,
                    modifier = Modifier
                        .padding(end = 12.dp)
                        .size(24.dp),
                )
                Column {
                    Text(
                        text = "日志",
                        fontSize = 16.sp,
                        fontWeight = FontWeight.Medium,
                        color = colorScheme.onSurface,
                    )
                    Text(
                        text = "每条日志实时落盘，进程被杀也不丢",
                        fontSize = 12.sp,
                        color = colorScheme.onSurfaceVariantSummary,
                    )
                }
            }
            Spacer(Modifier.height(8.dp))
            Text(
                text = "日志文件：${AppLog.currentLogFile()?.absolutePath ?: "未初始化"}",
                fontSize = 12.sp,
                fontFamily = FontFamily.Monospace,
                color = colorScheme.onSurfaceVariantSummary,
            )
            Text(
                text = "内存条数：${AppLog.entries.value.size}（上限 4000，滚动淘汰）",
                fontSize = 12.sp,
                color = colorScheme.onSurfaceVariantSummary,
            )
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                TextButton(text = "复制全部日志", onClick = onCopy)
                TextButton(
                    text = "清空内存日志",
                    onClick = { AppLog.clearMemory() },
                )
            }
        }
    }
}

// ---------------------------------------------------------------- 备份

@Composable
private fun BackupCard(
    backups: List<BackupStore.Backup>,
    message: String?,
    onRefresh: () -> Unit,
    onRestore: (BackupStore.Backup) -> Unit,
    onDelete: (BackupStore.Backup) -> Unit,
) {
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(16.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Icon(
                    imageVector = MiuixIcons.Backup,
                    contentDescription = null,
                    tint = colorScheme.onSurface,
                    modifier = Modifier
                        .padding(end = 12.dp)
                        .size(24.dp),
                )
                Column {
                    Text(
                        text = "原镜像备份（sha1 内容寻址）",
                        fontSize = 16.sp,
                        fontWeight = FontWeight.Medium,
                        color = colorScheme.onSurface,
                    )
                    Text(
                        text = "每次修补前自动备份 boot.img；可还原到 Downloads/SPRoot/ 供 fastboot 刷回",
                        fontSize = 12.sp,
                        color = colorScheme.onSurfaceVariantSummary,
                    )
                }
            }
            message?.let {
                Text(text = it, fontSize = 12.sp, color = colorScheme.onSurfaceVariantSummary)
            }
            if (backups.isEmpty()) {
                Text(
                    text = "暂无备份。第一次修补时会自动创建。",
                    fontSize = 13.sp,
                    color = colorScheme.onSurfaceVariantSummary,
                    modifier = Modifier.padding(vertical = 4.dp),
                )
            }
            backups.take(10).forEach { backup ->
                Column(modifier = Modifier.padding(vertical = 6.dp)) {
                    Text(
                        text = backup.sha1,
                        fontSize = 12.sp,
                        fontFamily = FontFamily.Monospace,
                        color = colorScheme.onSurface,
                    )
                    Text(
                        text = "${backup.sourceName} · ${Capabilities.formatBytes(backup.size)} · " +
                            java.text.SimpleDateFormat("MM-dd HH:mm", java.util.Locale.US)
                                .format(java.util.Date(backup.createdAt)),
                        fontSize = 12.sp,
                        color = colorScheme.onSurfaceVariantSummary,
                    )
                    Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        TextButton(text = "还原到 Downloads", onClick = { onRestore(backup) })
                        TextButton(text = "删除", onClick = { onDelete(backup) })
                    }
                }
            }
            TextButton(text = "刷新备份列表", onClick = onRefresh)
        }
    }
}

// ---------------------------------------------------------------- AP 部署

@Composable
private fun ApdDeployCard(
    selfTest: ApdSelfTest.Result?,
    status: CapabilityItem?,
    mismatch: String?,
    deploying: Boolean,
    deployResult: String?,
    selfTestRunning: Boolean,
    onDeploy: () -> Unit,
    onSelfTest: () -> Unit,
) {
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(16.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Icon(
                    imageVector = MiuixIcons.Store,
                    contentDescription = null,
                    tint = colorScheme.onSurface,
                    modifier = Modifier
                        .padding(end = 12.dp)
                        .size(24.dp),
                )
                Column {
                    Text(
                        text = "AP 部署（特权链）",
                        fontSize = 16.sp,
                        fontWeight = FontWeight.Medium,
                        color = colorScheme.onSurface,
                    )
                    Text(
                        text = "修补链不需要 root；部署/模块/授权全部经 AP（root-exec 唯一入口）",
                        fontSize = 12.sp,
                        color = colorScheme.onSurfaceVariantSummary,
                    )
                }
            }
            Spacer(Modifier.height(6.dp))
            Text(
                text = status?.detail ?: "AP 状态检测中…",
                fontSize = 12.sp,
                fontFamily = FontFamily.Monospace,
                color = colorScheme.onSurfaceVariantSummary,
            )
            mismatch?.let {
                Text(text = it, fontSize = 12.sp, color = colorScheme.error)
            }
            selfTest?.let {
                Text(
                    text = "AP 自检：${it.stageLabel} · exit=${it.exitCode}",
                    fontSize = 12.sp,
                    color = if (it.isNormalInitialState) colorScheme.onSurfaceVariantSummary else colorScheme.onSurface,
                )
            }
            deployResult?.let {
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
            }
            Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                TextButton(
                    text = if (deploying) "部署中…" else "部署 / 更新 AP",
                    onClick = onDeploy,
                    enabled = !deploying,
                    colors = top.yukonga.miuix.kmp.basic.ButtonDefaults.textButtonColorsPrimary(),
                )
                TextButton(
                    text = if (selfTestRunning) "自检中…" else "重跑 AP 自检",
                    onClick = onSelfTest,
                    enabled = !selfTestRunning,
                )
            }
            Text(
                text = "退出码 125 = 尚未部署 AP 守护（需先刷入修补后的镜像；正常初始态，不自动重试）；" +
                    "126/127 = 需重新部署；124 = 超时。",
                fontSize = 12.sp,
                color = colorScheme.onSurfaceVariantSummary,
            )
        }
    }
}

// ---------------------------------------------------------------- 引擎自检

@Composable
private fun EngineSelfCheckCard(report: String?, onRun: () -> Unit) {
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(16.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Icon(
                    imageVector = MiuixIcons.Info,
                    contentDescription = null,
                    tint = colorScheme.onSurface,
                    modifier = Modifier
                        .padding(end = 12.dp)
                        .size(24.dp),
                )
                Column {
                    Text(
                        text = "引擎自检（B-14）",
                        fontSize = 16.sp,
                        fontWeight = FontWeight.Medium,
                        color = colorScheme.onSurface,
                    )
                    Text(
                        text = "调用 libskrootpatch.so 的 JNI engineVersion()，确认库已进 APK 且 ABI 对齐",
                        fontSize = 12.sp,
                        color = colorScheme.onSurfaceVariantSummary,
                    )
                }
            }
            report?.let {
                Spacer(Modifier.height(6.dp))
                Text(
                    text = it,
                    fontSize = 12.sp,
                    fontFamily = FontFamily.Monospace,
                    color = colorScheme.onSurface,
                )
            }
            TextButton(
                text = "运行引擎自检",
                onClick = onRun,
                colors = top.yukonga.miuix.kmp.basic.ButtonDefaults.textButtonColorsPrimary(),
            )
        }
    }
}

// ---------------------------------------------------------------- 工作目录清理

@Composable
private fun WorkdirCard(message: String?, onClean: () -> Unit) {
    val context = LocalContext.current
    val patchDir = remember { java.io.File(context.cacheDir, "patch") }
    val execKeysDir = remember { java.io.File(context.filesDir, "exec-keys") }
    var sizes by remember { mutableStateOf(0L to 0L) }
    LaunchedEffect(patchDir, execKeysDir) {
        sizes = withContext(Dispatchers.IO) {
            dirSize(patchDir) to dirSize(execKeysDir)
        }
    }
    Card(modifier = Modifier.fillMaxWidth()) {
        Column(modifier = Modifier.padding(16.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Icon(
                    imageVector = MiuixIcons.Delete,
                    contentDescription = null,
                    tint = colorScheme.onSurface,
                    modifier = Modifier
                        .padding(end = 12.dp)
                        .size(24.dp),
                )
                Column {
                    Text(
                        text = "工作目录清理",
                        fontSize = 16.sp,
                        fontWeight = FontWeight.Medium,
                        color = colorScheme.onSurface,
                    )
                    Text(
                        text = "清理失败现场（cache/patch）与 root-exec 临时密钥残留（files/exec-keys）",
                        fontSize = 12.sp,
                        color = colorScheme.onSurfaceVariantSummary,
                    )
                }
            }
            Spacer(Modifier.height(6.dp))
            Text(
                text = "cache/patch：${Capabilities.formatBytes(sizes.first)}\n" +
                    "files/exec-keys：${Capabilities.formatBytes(sizes.second)}",
                fontSize = 12.sp,
                fontFamily = FontFamily.Monospace,
                color = colorScheme.onSurfaceVariantSummary,
            )
            message?.let {
                Text(text = it, fontSize = 12.sp, color = colorScheme.onSurface)
            }
            TextButton(
                text = "清理工作目录",
                onClick = onClean,
                colors = top.yukonga.miuix.kmp.basic.ButtonDefaults.textButtonColorsPrimary(),
            )
        }
    }
}

private fun dirSize(dir: java.io.File): Long =
    if (!dir.exists()) 0L else dir.walkTopDown().filter { it.isFile }.sumOf { it.length() }
