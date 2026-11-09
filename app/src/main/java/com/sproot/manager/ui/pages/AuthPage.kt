package com.sproot.manager.ui.pages

import androidx.activity.compose.BackHandler
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
import androidx.compose.foundation.layout.fillMaxSize
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
import com.sproot.manager.core.AuthApp
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
import top.yukonga.miuix.kmp.basic.CardDefaults
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
import top.yukonga.miuix.kmp.icon.extended.Refresh
import top.yukonga.miuix.kmp.overlay.OverlayListPopup
import top.yukonga.miuix.kmp.preference.ArrowPreference
import top.yukonga.miuix.kmp.theme.MiuixTheme
import top.yukonga.miuix.kmp.theme.MiuixTheme.colorScheme
import top.yukonga.miuix.kmp.utils.overScrollVertical
import top.yukonga.miuix.kmp.utils.scrollEndHaptic

/** 筛选：对应 KSU SuperUser 页右上角的「全部 / 已授权 / 已拒绝」菜单（另加"每次询问"）。 */
private enum class AuthFilter(val label: String) {
    ALL("全部应用"),
    ALLOWED("已授权"),
    DENIED("已拒绝"),
    ASKED("每次询问"),
}

/**
 * Root 授权管理页（D）—— 按 KernelSU `ui/screen/superuser/SuperUserMiuix.kt` 的版式搭骨架：
 *
 * ```
 * Scaffold(BlurredBar(TopAppBar("授权" + 筛选菜单 + 刷新)))
 *  └─ LazyColumn(12dp 横距)
 *      ├─ Notice：授权 API 未接入（占位假数据）/ AP 未部署
 *      ├─ 搜索栏（名称 / 包名 / uid）
 *      ├─ AuthApp 卡（图标 + 名称 + 包名 + 右侧状态标签，点卡片进详情）
 *      ├─ 空态（大图标 + 文案）
 *      └─ 详情页（二级页）：应用信息 + 三态单选（允许/拒绝/每次询问·待实现）+ 撤销授权 + 统计
 * ```
 *
 * ## 数据状态（诚实原则）
 * 数据层 [AuthRepository] 目前 `API_READY=false`，返回**明确标注的占位假数据**；
 * 三态中的 `ask` 依赖 daemon 挂起请求并回传弹窗，本轮未实现 → **显示但禁用，标注"待实现"**，
 * 绝不假装可用。apd-dev 的 `apd auth --json` 契约落地后，只需替换 AuthRepository 实现。
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
                    fake = true,
                    message = "读取授权列表异常：${it.javaClass.simpleName}: ${it.message}",
                    apps = emptyList(),
                )
            }
            listResult = r
            loading = false
            AppLog.info("Auth", "授权列表：ok=${r.ok} fake=${r.fake} count=${r.apps.size}")
        }.start()
    }

    LaunchedEffect(isCurrentPage) {
        if (isCurrentPage && listResult == null) load()
    }

    /** 本地预览更新（契约未接入时）：只改内存列表，并在 UI 明示"仅本地预览"。 */
    fun applyLocalPreview(uid: Int, state: AuthState) {
        listResult = listResult?.let { snapshot ->
            snapshot.copy(apps = snapshot.apps.map { if (it.uid == uid) it.copy(state = state) else it })
        }
        detail = detail?.takeIf { it.uid == uid }?.copy(state = state) ?: detail
    }

    // ---------------- 详情页 ----------------
    val currentDetail = detail
    if (currentDetail != null) {
        BackHandler(enabled = true) { detail = null }
        AuthDetailPager(
            app = currentDetail,
            fake = listResult?.fake != false,
            onBack = { detail = null },
            onSetState = { state ->
                Thread {
                    val r = AuthRepository.setState(context, key, currentDetail.uid, state)
                    actionMsg = r.message
                    if (r.supported) {
                        load()
                    } else {
                        applyLocalPreview(currentDetail.uid, state)
                    }
                }.start()
            },
            onRevoke = { showRevokeConfirm = true },
        )
        ConfirmDialog(
            show = showRevokeConfirm,
            title = "撤销授权",
            message = "将「${currentDetail.label}」置为拒绝（deny），之后该应用的提权请求会被直接拒绝。",
            confirmText = "撤销",
            onConfirm = {
                showRevokeConfirm = false
                Thread {
                    val r = AuthRepository.revoke(context, key, currentDetail.uid)
                    actionMsg = r.message
                    if (r.supported) load() else applyLocalPreview(currentDetail.uid, AuthState.DENY)
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
                    if (!apdReady) {
                        WarningCard(
                            message = "尚未部署 AP 守护（需先刷入修补后的镜像）：授权链路依赖 apd，" +
                                "部署完成后才能真正读写授权状态",
                            level = WarningLevel.Notice,
                        )
                    }
                }
                item {
                    if (listResult?.fake != false) {
                        WarningCard(
                            message = AuthRepository.FAKE_NOTICE,
                            level = WarningLevel.Notice,
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
                    (loading || listResult == null) && apps.isEmpty() -> {
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

/** 列表项：图标圆底 + 名称 + 包名（+uid），右侧三态标签；点卡片进详情（KSU SuperUser 版式）。 */
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
            AppLetterIcon(label = app.label)
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
                    text = app.packageName,
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
                        if (app.denyCount > 0) append(" · 拒绝 ").append(app.denyCount).append(" 次")
                    },
                    fontSize = 12.sp,
                    color = colorScheme.onSurfaceVariantSummary,
                )
            }
            StateTag(state = app.state)
        }
    }
}

@Composable
private fun AppLetterIcon(label: String) {
    Box(
        modifier = Modifier
            .size(44.dp)
            .background(colorScheme.secondaryContainer, CircleShape),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            text = label.take(1).uppercase(),
            fontSize = 20.sp,
            fontWeight = FontWeight(600),
            color = colorScheme.onSecondaryContainer,
        )
    }
}

/** 三态标签：允许=primary、拒绝=error、每次询问=tertiary（标注"待实现"）。 */
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
        label = if (state == AuthState.ASK) "${state.label}·待实现" else state.label,
        backgroundColor = container,
        contentColor = content,
    )
}

// ---------------------------------------------------------------- 详情页

@Composable
private fun AuthDetailPager(
    app: AuthApp,
    fake: Boolean,
    onBack: () -> Unit,
    onSetState: (AuthState) -> Unit,
    onRevoke: () -> Unit,
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
                                AppLetterIcon(label = app.label)
                                Column(modifier = Modifier.padding(start = 12.dp)) {
                                    Text(
                                        text = app.label,
                                        fontSize = 20.sp,
                                        fontWeight = FontWeight.SemiBold,
                                        color = colorScheme.onSurface,
                                    )
                                    Text(
                                        text = app.packageName,
                                        fontSize = 12.sp,
                                        fontFamily = FontFamily.Monospace,
                                        color = colorScheme.onSurfaceVariantSummary,
                                    )
                                    Text(
                                        text = "uid ${app.uid}",
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
                                hint = "每次提权都弹窗询问 —— 需要 daemon 挂起请求并回传弹窗，AP 侧尚未实现",
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
                                text = "统计",
                                fontSize = 15.sp,
                                fontWeight = FontWeight.Medium,
                                color = colorScheme.onSurface,
                            )
                            InfoLine(
                                "last_grant_at",
                                if (app.lastGrantAt <= 0L) "—" else
                                    java.text.SimpleDateFormat("yyyy-MM-dd HH:mm:ss", java.util.Locale.US)
                                        .format(java.util.Date(app.lastGrantAt)),
                            )
                            InfoLine("deny_count", app.denyCount.toString())
                            InfoLine("request_count", app.requestCount.toString())
                            if (fake) {
                                Text(
                                    text = "当前为占位假数据；变更仅本地预览，不会写入 daemon。",
                                    fontSize = 12.sp,
                                    color = colorScheme.error,
                                )
                            }
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
                        label = "待实现",
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
                "当应用通过 AP 请求 root 时，会出现在这里；届时可在此设为允许 / 拒绝"
            },
            fontSize = 13.sp,
            color = colorScheme.onSurfaceVariantSummary,
        )
    }
}
