package com.sproot.manager.ui.pages

import android.graphics.drawable.Drawable
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
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
import androidx.compose.foundation.layout.only
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.systemBars
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.graphics.painter.Painter
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
import com.sproot.manager.core.AuthApp
import com.sproot.manager.core.AuthLogEntry
import com.sproot.manager.core.AuthRepository
import com.sproot.manager.core.AuthState
import com.sproot.manager.core.SecretStore
import com.sproot.manager.ui.components.BlurredBar
import com.sproot.manager.ui.components.ConfirmDialog
import com.sproot.manager.ui.components.StatusTag
import com.sproot.manager.ui.components.WarningCard
import com.sproot.manager.ui.components.WarningLevel
import com.sproot.manager.ui.components.rememberBlurBackdrop
import com.sproot.manager.ui.theme.LocalEnableBlur
import top.yukonga.miuix.kmp.basic.Card
import top.yukonga.miuix.kmp.basic.DropdownImpl
import top.yukonga.miuix.kmp.basic.Icon
import top.yukonga.miuix.kmp.basic.IconButton
import top.yukonga.miuix.kmp.basic.ListPopupColumn
import top.yukonga.miuix.kmp.basic.MiuixScrollBehavior
import top.yukonga.miuix.kmp.basic.PopupPositionProvider
import top.yukonga.miuix.kmp.basic.RadioButton
import top.yukonga.miuix.kmp.basic.Scaffold
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.basic.TextButton
import top.yukonga.miuix.kmp.basic.TopAppBar
import top.yukonga.miuix.kmp.blur.layerBackdrop
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.basic.Search
import top.yukonga.miuix.kmp.icon.extended.Back
import top.yukonga.miuix.kmp.icon.extended.Filter
import top.yukonga.miuix.kmp.icon.extended.Lock
import top.yukonga.miuix.kmp.icon.extended.Notes
import top.yukonga.miuix.kmp.icon.extended.Refresh
import top.yukonga.miuix.kmp.overlay.OverlayListPopup
import top.yukonga.miuix.kmp.preference.ArrowPreference
import top.yukonga.miuix.kmp.theme.MiuixTheme.colorScheme
import top.yukonga.miuix.kmp.utils.overScrollVertical
import top.yukonga.miuix.kmp.utils.scrollEndHaptic

/** 筛选：对应 KSU SuperUser 页右上角菜单（全部 / 已允许 / 已拒绝 / 待询问）。 */
private enum class AuthFilter(val label: String) {
    ALL("全部应用"),
    ALLOWED("已允许"),
    DENIED("已拒绝"),
    ASKED("待询问"),
}

/**
 * Root 授权管理页（D）—— 照 KernelSU `ui/screen/superuser/SuperUserMiuix.kt` 版式：
 *
 * ```
 * Scaffold(BlurredBar(TopAppBar("授权" + 筛选 + 刷新 + 审计日志)))
 *  └─ LazyColumn(12dp 横距)
 *      ├─ Notice：默认授权态是「待询问」(ask) → 新 App 首次请求会被拒
 *      ├─ Notice：AP 未部署（125）时优雅降级
 *      ├─ 搜索栏（应用名 / 包名 / uid）
 *      ├─ AuthApp 卡（应用图标 + 名称 + 包名 + 右侧状态，点卡片进详情）
 *      ├─ 空态（大图标 + 文案 + 默认态说明）
 *      └─ 详情页（二级页）：应用信息 + 三态单选 + 权限详情 + 撤销 + 审计日志入口
 * ```
 *
 * ## 数据契约（apd d6c9ecb）
 * `apd auth list/set/revoke/log --json`，全部经 [ApdClient.rootExec]。
 * `label`/`app_icon_path` 恒为 null → 本页用 PackageManager 解析名称/图标（uid 无包时显示 uid=xxxxx）。
 * `ask` Phase 1 不可交互（当前等同拒绝）→ 详情页显示但**禁用**并标「待实现」，不假装可用。
 */
@Composable
fun AuthPager(
    bottomInnerPadding: Dp = 0.dp,
    isCurrentPage: Boolean = true,
) {
    val context = LocalContext.current
    val key = remember { runCatching { SecretStore.get(context).getOrCreateRootKey() }.getOrNull() }
    val apdStatus = remember { ApdClient.status(context) }
    val apdReady = apdStatus.ok

    var listResult by remember { mutableStateOf<AuthRepository.ListResult?>(null) }
    var loading by remember { mutableStateOf(false) }
    var query by remember { mutableStateOf("") }
    var filter by remember { mutableStateOf(AuthFilter.ALL) }
    var showFilterPopup by remember { mutableStateOf(false) }
    var detail by remember { mutableStateOf<AuthApp?>(null) }
    var showLog by remember { mutableStateOf(false) }
    var actionMsg by remember { mutableStateOf<String?>(null) }
    var showRevokeConfirm by remember { mutableStateOf(false) }

    val scrollBehavior = MiuixScrollBehavior()
    val backdrop = rememberBlurBackdrop(LocalEnableBlur.current)
    val barColor = if (backdrop != null) Color.Transparent else colorScheme.surface

    fun load() {
        if (loading) return
        loading = true
        Thread {
            val r = runCatching { AuthRepository.list(context, key) }.getOrElse {
                AuthRepository.ListResult(
                    ok = false,
                    message = "读取授权列表异常：${it.javaClass.simpleName}: ${it.message}",
                    apps = emptyList(),
                )
            }
            listResult = r
            loading = false
            if (!r.ok) AppLog.error("Auth", "授权列表失败：${r.message}")
        }.start()
    }

    LaunchedEffect(isCurrentPage) {
        if (isCurrentPage && listResult == null) load()
    }

    // ---------------- 审计日志页（二级页） ----------------
    if (showLog) {
        BackHandler(enabled = true) { showLog = false }
        AuthLogPager(superKey = key, onBack = { showLog = false })
        return
    }

    // ---------------- 详情页（二级页） ----------------
    val currentDetail = detail
    if (currentDetail != null) {
        BackHandler(enabled = true) { detail = null }
        AuthDetailPager(
            app = currentDetail,
            onBack = { detail = null },
            onSetState = { state ->
                Thread {
                    val r = AuthRepository.setState(context, key, currentDetail.uid, state)
                    actionMsg = r.message
                    if (r.ok) {
                        load()
                        detail = currentDetail.copy(state = state)
                    } else {
                        AppLog.warn("Auth", "设置授权失败：${r.message}")
                    }
                }.start()
            },
            onRevoke = { showRevokeConfirm = true },
            onOpenLog = { showLog = true },
        )
        ConfirmDialog(
            show = showRevokeConfirm,
            title = "撤销授权",
            message = "将「${currentDetail.label}」置为「拒绝」：之后该应用的提权请求会被直接拒绝。",
            confirmText = "撤销",
            onConfirm = {
                showRevokeConfirm = false
                Thread {
                    val r = AuthRepository.revoke(context, key, currentDetail.uid)
                    actionMsg = r.message
                    if (r.ok) {
                        load()
                        detail = currentDetail.copy(state = AuthState.DENY)
                    }
                }.start()
            },
            onDismiss = { showRevokeConfirm = false },
        )
        return
    }

    val apps = listResult?.apps.orEmpty()
    val filtered = remember(apps, query, filter) {
        val q = query.trim().lowercase()
        apps.asSequence()
            .filter { app ->
                when (filter) {
                    AuthFilter.ALL -> true
                    AuthFilter.ALLOWED -> app.state == AuthState.ALLOW
                    AuthFilter.DENIED -> app.state == AuthState.DENY
                    AuthFilter.ASKED -> app.state == AuthState.ASK
                }
            }
            .filter { app ->
                q.isEmpty() ||
                    app.label.lowercase().contains(q) ||
                    app.packageName.lowercase().contains(q) ||
                    app.uid.toString() == q
            }
            .sortedBy { it.label.lowercase() }
            .toList()
    }

    Scaffold(
        topBar = {
            BlurredBar(backdrop) {
                TopAppBar(
                    color = barColor,
                    title = "授权",
                    actions = {
                        IconButton(onClick = { showLog = true }) {
                            Icon(
                                imageVector = MiuixIcons.Notes,
                                contentDescription = "审计日志",
                                tint = colorScheme.onSurface,
                            )
                        }
                        Box {
                            IconButton(onClick = { showFilterPopup = true }) {
                                Icon(
                                    imageVector = MiuixIcons.Filter,
                                    contentDescription = "筛选",
                                    tint = colorScheme.onSurface,
                                )
                            }
                            OverlayListPopup(
                                show = showFilterPopup,
                                popupPositionProvider = top.yukonga.miuix.kmp.basic.ListPopupDefaults.ContextMenuPositionProvider,
                                alignment = PopupPositionProvider.Align.TopEnd,
                                onDismissRequest = { showFilterPopup = false },
                                content = {
                                    ListPopupColumn {
                                        AuthFilter.entries.forEachIndexed { index, option ->
                                            DropdownImpl(
                                                text = option.label,
                                                optionSize = AuthFilter.entries.size,
                                                isSelected = filter == option,
                                                onSelectedIndexChange = {
                                                    filter = option
                                                    showFilterPopup = false
                                                },
                                                index = index,
                                            )
                                        }
                                    }
                                },
                            )
                        }
                        IconButton(onClick = { load() }) {
                            Icon(
                                imageVector = MiuixIcons.Refresh,
                                contentDescription = "刷新",
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
                    top = innerPadding.calculateTopPadding() + 8.dp,
                    bottom = bottomInnerPadding + 16.dp,
                ),
                overscrollEffect = null,
            ) {
                item {
                    WarningCard(
                        message = AuthRepository.DEFAULT_STATE_NOTICE,
                        level = WarningLevel.Notice,
                    )
                }
                item {
                    if (!apdReady) {
                        WarningCard(
                            message = "尚未部署 AP 守护（需先刷入修补后的镜像）：授权命令走 apd root-exec，" +
                                "部署完成前列表与操作不可用",
                            level = WarningLevel.Notice,
                        )
                    }
                }
                listResult?.takeIf { !it.ok }?.let { failed ->
                    item {
                        WarningCard(
                            message = "授权列表加载失败：${failed.message}",
                            level = WarningLevel.Error,
                        )
                    }
                }
                item {
                    Spacer(Modifier.height(12.dp))
                    SearchCard(query = query, onQueryChange = { query = it })
                }
                actionMsg?.let { message ->
                    item {
                        Text(
                            text = message,
                            fontSize = 12.sp,
                            color = colorScheme.onSurfaceVariantSummary,
                            modifier = Modifier.padding(horizontal = 4.dp, vertical = 4.dp),
                        )
                    }
                }
                when {
                    loading && apps.isEmpty() -> {
                        item {
                            Box(
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .padding(vertical = 48.dp),
                                contentAlignment = Alignment.Center,
                            ) {
                                Text(
                                    text = "正在读取授权列表…",
                                    fontSize = 14.sp,
                                    color = colorScheme.onSurfaceVariantSummary,
                                )
                            }
                        }
                    }

                    filtered.isEmpty() -> {
                        item {
                            EmptyAuthState(
                                searchEmpty = query.isNotBlank() || filter != AuthFilter.ALL,
                            )
                        }
                    }

                    else -> {
                        items(filtered, key = { it.uid }) { app ->
                            AuthAppCard(app = app, onClick = { detail = app })
                        }
                    }
                }
                item { Spacer(Modifier.height(8.dp)) }
            }
        }
    }
}

// ---------------------------------------------------------------- 列表项

/** 列表项：应用图标 + 名称 + 包名 + 统计，右侧三态标签；点卡片进详情（KSU SuperUser 版式）。 */
@Composable
private fun AuthAppCard(app: AuthApp, onClick: () -> Unit) {
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .padding(bottom = 12.dp),
        insideMargin = PaddingValues(16.dp),
        onClick = onClick,
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            AppIcon(app = app)
            Column(
                modifier = Modifier
                    .weight(1f)
                    .padding(start = 12.dp, end = 8.dp),
            ) {
                Text(
                    text = app.label,
                    fontSize = 17.sp,
                    fontWeight = FontWeight(550),
                    color = colorScheme.onSurface,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    text = app.packageName.ifBlank { "（无对应包）" },
                    fontSize = 12.sp,
                    color = colorScheme.onSurfaceVariantSummary,
                    fontFamily = FontFamily.Monospace,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
                Text(
                    text = buildString {
                        append("uid ").append(app.uid)
                        if (app.requestCount > 0) append(" · 请求 ").append(app.requestCount).append(" 次")
                    },
                    fontSize = 12.sp,
                    color = colorScheme.onSurfaceVariantSummary,
                )
            }
            StateTag(state = app.state)
        }
    }
}

/** 应用图标：apd 不给图标 → App 侧解析；解析不到时用首字母圆底（uid 无包也适用）。 */
@Composable
private fun AppIcon(app: AuthApp, size: Dp = 44.dp) {
    val icon = app.icon
    if (icon != null) {
        val painter = remember(icon) { DrawablePainter(icon) }
        Image(
            painter = painter,
            contentDescription = null,
            modifier = Modifier
                .size(size)
                .clip(CircleShape),
        )
    } else {
        Box(
            modifier = Modifier
                .size(size)
                .background(colorScheme.secondaryContainer, CircleShape),
            contentAlignment = Alignment.Center,
        ) {
            Text(
                text = app.label.take(1).uppercase().ifBlank { "?" },
                fontSize = 20.sp,
                fontWeight = FontWeight(600),
                color = colorScheme.onSecondaryContainer,
            )
        }
    }
}

/** android Drawable → Compose Painter（不引入 core-ktx/Coil；直接画 native canvas）。 */
private class DrawablePainter(private val drawable: Drawable) : Painter() {
    override val intrinsicSize: Size
        get() = Size(
            drawable.intrinsicWidth.coerceAtLeast(1).toFloat(),
            drawable.intrinsicHeight.coerceAtLeast(1).toFloat(),
        )

    override fun DrawScope.onDraw() {
        drawable.setBounds(0, 0, size.width.toInt().coerceAtLeast(1), size.height.toInt().coerceAtLeast(1))
        drawIntoCanvas { canvas -> drawable.draw(canvas.nativeCanvas) }
    }
}

/** 三态标签：允许=primary、拒绝=error、待询问=tertiary（Phase 1 不可交互）。 */
@Composable
private fun StateTag(state: AuthState) {
    val container = when (state) {
        AuthState.ALLOW -> colorScheme.primaryContainer
        AuthState.DENY -> colorScheme.errorContainer
        AuthState.ASK -> colorScheme.tertiaryContainer
    }
    val content = when (state) {
        AuthState.ALLOW -> colorScheme.onPrimaryContainer
        AuthState.DENY -> colorScheme.onErrorContainer
        AuthState.ASK -> colorScheme.onTertiaryContainer
    }
    StatusTag(
        label = if (state == AuthState.ASK) "${state.label}·Phase1" else state.label,
        backgroundColor = container,
        contentColor = content,
    )
}

// ---------------------------------------------------------------- 详情页

@Composable
private fun AuthDetailPager(
    app: AuthApp,
    onBack: () -> Unit,
    onSetState: (AuthState) -> Unit,
    onRevoke: () -> Unit,
    onOpenLog: () -> Unit,
) {
    val scrollBehavior = MiuixScrollBehavior()
    val backdrop = rememberBlurBackdrop(LocalEnableBlur.current)
    val barColor = if (backdrop != null) Color.Transparent else colorScheme.surface

    Scaffold(
        topBar = {
            BlurredBar(backdrop) {
                TopAppBar(
                    color = barColor,
                    title = app.label,
                    navigationIcon = {
                        IconButton(onClick = onBack) {
                            Icon(
                                imageVector = MiuixIcons.Back,
                                contentDescription = "返回授权列表",
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
                                AppIcon(app = app, size = 56.dp)
                                Column(modifier = Modifier.padding(start = 12.dp)) {
                                    Text(
                                        text = app.label,
                                        fontSize = 20.sp,
                                        fontWeight = FontWeight.SemiBold,
                                        color = colorScheme.onSurface,
                                    )
                                    Text(
                                        text = app.packageName.ifBlank { "（无对应包）" },
                                        fontSize = 12.sp,
                                        fontFamily = FontFamily.Monospace,
                                        color = colorScheme.onSurfaceVariantSummary,
                                    )
                                    Text(
                                        text = "uid ${app.uid} · 请求 ${app.requestCount} 次",
                                        fontSize = 12.sp,
                                        color = colorScheme.onSurfaceVariantSummary,
                                    )
                                }
                            }
                        }
                    }
                }
                item { Spacer(Modifier.height(12.dp)) }
                item {
                    Card(modifier = Modifier.fillMaxWidth()) {
                        Column(modifier = Modifier.padding(16.dp)) {
                            Text(
                                text = "授权状态",
                                fontSize = 15.sp,
                                fontWeight = FontWeight.Medium,
                                color = colorScheme.onSurface,
                            )
                            AuthStateRow(
                                state = AuthState.ALLOW,
                                selected = app.state == AuthState.ALLOW,
                                hint = "允许该应用提权（apd root-exec 直接放行）",
                                onClick = { onSetState(AuthState.ALLOW) },
                            )
                            AuthStateRow(
                                state = AuthState.DENY,
                                selected = app.state == AuthState.DENY,
                                hint = "拒绝提权请求（不弹窗，直接失败）",
                                onClick = { onSetState(AuthState.DENY) },
                            )
                            AuthStateRow(
                                state = AuthState.ASK,
                                selected = app.state == AuthState.ASK,
                                hint = "Phase 1 不可交互：daemon 挂起请求/弹窗回传未实现，当前等同「拒绝」；" +
                                    "请改为允许或拒绝",
                                enabled = false,
                                onClick = { onSetState(AuthState.ASK) },
                            )
                        }
                    }
                }
                item { Spacer(Modifier.height(12.dp)) }
                item {
                    Card(modifier = Modifier.fillMaxWidth()) {
                        ArrowPreference(
                            title = "审计日志",
                            summary = "查看 request / set_state / revoke 等事件（apd 只提供脱敏信息）",
                            onClick = onOpenLog,
                        )
                        ArrowPreference(
                            title = "撤销授权",
                            summary = "等价于置为「拒绝」；不会卸载应用",
                            onClick = onRevoke,
                        )
                    }
                }
                item { Spacer(Modifier.height(12.dp)) }
                item {
                    Card(modifier = Modifier.fillMaxWidth()) {
                        Column(modifier = Modifier.padding(16.dp)) {
                            Text(
                                text = "权限详情",
                                fontSize = 15.sp,
                                fontWeight = FontWeight.Medium,
                                color = colorScheme.onSurface,
                            )
                            InfoLine("state", app.state.wire)
                            InfoLine("package", app.packageName.ifBlank { "—" }, monospace = true)
                            InfoLine("uid", app.uid.toString())
                            InfoLine("to_uid", app.toUid.toString())
                            InfoLine("sctx", app.sctx.ifBlank { "—" })
                            InfoLine("exclude", app.exclude.toString())
                            InfoLine(
                                "last_grant_at",
                                if (app.lastGrantAt <= 0L) "—" else formatUnix(app.lastGrantAt),
                            )
                            InfoLine(
                                "last_deny_at",
                                if (app.lastDenyAt <= 0L) "—" else formatUnix(app.lastDenyAt),
                            )
                            InfoLine("request_count", app.requestCount.toString())
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun AuthStateRow(
    state: AuthState,
    selected: Boolean,
    hint: String,
    onClick: () -> Unit,
    enabled: Boolean = true,
) {
    Row(
        modifier = Modifier
            .fillMaxWidth()
            .padding(vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        RadioButton(
            selected = selected,
            onClick = { if (enabled) onClick() },
        )
        Column(modifier = Modifier.padding(start = 8.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                Text(
                    text = state.label,
                    fontSize = 15.sp,
                    color = if (enabled) colorScheme.onSurface else colorScheme.onSurfaceVariantSummary,
                )
                if (!enabled) {
                    Spacer(Modifier.size(6.dp))
                    StatusTag(
                        label = "Phase 1 不可交互",
                        backgroundColor = colorScheme.tertiaryContainer,
                        contentColor = colorScheme.onTertiaryContainer,
                    )
                }
            }
            Text(
                text = hint,
                fontSize = 12.sp,
                color = colorScheme.onSurfaceVariantSummary,
            )
        }
    }
}

// ---------------------------------------------------------------- 审计日志页

/** 审计日志页：event/decision/package/cmd_name/iso（apd 只给脱敏信息，UI 不补全不猜测）。 */
@Composable
private fun AuthLogPager(superKey: String?, onBack: () -> Unit) {
    val context = LocalContext.current
    var result by remember { mutableStateOf<AuthRepository.LogResult?>(null) }
    var loading by remember { mutableStateOf(false) }
    val scrollBehavior = MiuixScrollBehavior()
    val backdrop = rememberBlurBackdrop(LocalEnableBlur.current)
    val barColor = if (backdrop != null) Color.Transparent else colorScheme.surface

    LaunchedEffect(Unit) {
        if (loading || result != null) return@LaunchedEffect
        loading = true
        Thread {
            result = runCatching { AuthRepository.log(context, superKey) }.getOrElse {
                AuthRepository.LogResult(
                    ok = false,
                    message = "读取审计日志异常：${it.javaClass.simpleName}: ${it.message}",
                    entries = emptyList(),
                )
            }
            loading = false
        }.start()
    }

    Scaffold(
        topBar = {
            BlurredBar(backdrop) {
                TopAppBar(
                    color = barColor,
                    title = "审计日志",
                    navigationIcon = {
                        IconButton(onClick = onBack) {
                            Icon(
                                imageVector = MiuixIcons.Back,
                                contentDescription = "返回",
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
                    WarningCard(
                        message = "日志只含 apd 提供的脱敏信息（无完整命令、无密钥）；" +
                            "cmd_name 是命令名，cmd_args 是参数个数，不展示完整参数",
                        level = WarningLevel.Notice,
                    )
                }
                result?.takeIf { !it.ok }?.let { failed ->
                    item {
                        WarningCard(
                            message = "审计日志读取失败：${failed.message}",
                            level = WarningLevel.Error,
                        )
                    }
                }
                result?.entries?.let { entries ->
                    if (entries.isEmpty() && !loading) {
                        item {
                            Box(
                                modifier = Modifier
                                    .fillMaxWidth()
                                    .padding(vertical = 48.dp),
                                contentAlignment = Alignment.Center,
                            ) {
                                Text(
                                    text = "暂无审计记录",
                                    fontSize = 14.sp,
                                    color = colorScheme.onSurfaceVariantSummary,
                                )
                            }
                        }
                    }
                    items(entries) { entry ->
                        AuthLogCard(entry = entry)
                    }
                }
                if (loading) {
                    item {
                        Box(
                            modifier = Modifier
                                .fillMaxWidth()
                                .padding(vertical = 40.dp),
                            contentAlignment = Alignment.Center,
                        ) {
                            Text(
                                text = "正在读取审计日志…",
                                fontSize = 14.sp,
                                color = colorScheme.onSurfaceVariantSummary,
                            )
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun AuthLogCard(entry: AuthLogEntry) {
    Card(
        modifier = Modifier
            .fillMaxWidth()
            .padding(bottom = 12.dp),
        insideMargin = PaddingValues(16.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            StatusTag(
                label = eventLabel(entry.event),
                backgroundColor = colorScheme.secondaryContainer,
                contentColor = colorScheme.onSecondaryContainer,
            )
            entry.decision?.let { decision ->
                Spacer(Modifier.size(6.dp))
                StatusTag(
                    label = decisionLabel(decision),
                    backgroundColor = if (decision == "deny" || decision == "ask_pending") {
                        colorScheme.errorContainer
                    } else {
                        colorScheme.primaryContainer
                    },
                    contentColor = if (decision == "deny" || decision == "ask_pending") {
                        colorScheme.onErrorContainer
                    } else {
                        colorScheme.onPrimaryContainer
                    },
                )
            }
            Spacer(Modifier.weight(1f))
            Text(
                text = entry.iso.ifBlank { formatUnix(entry.ts) },
                fontSize = 12.sp,
                color = colorScheme.onSurfaceVariantSummary,
            )
        }
        Spacer(Modifier.height(6.dp))
        Text(
            text = entry.packageName ?: "uid=${entry.uid}",
            fontSize = 14.sp,
            fontWeight = FontWeight(550),
            color = colorScheme.onSurface,
            fontFamily = FontFamily.Monospace,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        Text(
            text = buildString {
                append("uid ").append(entry.uid)
                if (!entry.cmdName.isNullOrBlank()) append(" · cmd ").append(entry.cmdName)
                entry.state?.let { append(" · state ").append(it) }
                entry.detail?.let { append(" · ").append(it) }
            },
            fontSize = 12.sp,
            color = colorScheme.onSurfaceVariantSummary,
        )
    }
}

private fun eventLabel(event: String): String = when (event) {
    "request" -> "请求"
    "set_state" -> "改状态"
    "revoke" -> "撤销"
    "grant" -> "授权"
    "migrate" -> "迁移"
    else -> event
}

private fun decisionLabel(decision: String): String = when (decision) {
    "allow" -> "放行"
    "deny" -> "拒绝"
    "ask_pending" -> "待询问→拒"
    else -> decision
}

// ---------------------------------------------------------------- 复用小组件

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
                        text = "搜索应用（名称 / 包名 / uid）",
                        fontSize = 15.sp,
                        color = colorScheme.onSurfaceVariantSummary,
                    )
                }
                BasicTextField(
                    value = query,
                    onValueChange = onQueryChange,
                    modifier = Modifier.fillMaxWidth(),
                    singleLine = true,
                    textStyle = TextStyle(fontSize = 15.sp, color = colorScheme.onSurface),
                    cursorBrush = SolidColor(colorScheme.primary),
                )
            }
            if (query.isNotEmpty()) {
                TextButton(text = "清空", onClick = { onQueryChange("") })
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

@Composable
private fun EmptyAuthState(searchEmpty: Boolean) {
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
                imageVector = MiuixIcons.Lock,
                contentDescription = null,
                tint = colorScheme.onSurfaceVariantSummary,
                modifier = Modifier.size(40.dp),
            )
        }
        Text(
            text = if (searchEmpty) "没有匹配的应用" else "还没有提权请求记录",
            fontSize = 16.sp,
            fontWeight = FontWeight.Medium,
            color = colorScheme.onSurface,
        )
        Text(
            text = if (searchEmpty) {
                "换个关键词或切换筛选条件试试"
            } else {
                "当应用通过 AP 请求 root 时会出现这里。注意默认态是「待询问」：" +
                    "新应用首次请求会被拒绝（ask 当前等同拒绝），请在此改为「允许」"
            },
            fontSize = 13.sp,
            color = colorScheme.onSurfaceVariantSummary,
        )
    }
}

private fun formatUnix(millis: Long): String =
    java.text.SimpleDateFormat("yyyy-MM-dd HH:mm:ss", java.util.Locale.US)
        .format(java.util.Date(millis))
