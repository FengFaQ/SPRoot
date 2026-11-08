package com.sproot.manager.ui.pages

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
 * 模块页：`apd module list --json` 渲染（Schema v1 见 core/Modules.kt 注释与 NEW-DOC-04 §7.1）。
 *
 * 设计原则（旧版反面）：
 *  - AP 未就绪时**禁用所有入口**并说明原因（不出现"点了必然失败"的按钮）；
 *  - 失败/解析异常时把 **apd 原始 stdout/stderr** 常驻展示，可复制上报；
 *  - 模块详情字段缺失时显示 "—"，不伪造。
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

    fun refresh() {
        if (loading) return
        if (key == null) {
            actionMsg = "Root 密钥不可用，无法调用 AP（去设置页重新生成）"
            return
        }
        loading = true
        Thread {
            val r = ModuleRepository.list(context, key)
            result = r
            loading = false
            if (!r.ok) AppLog.error("Modules", "模块列表失败：${r.message}")
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

        // ---------------- 列表头 ----------------
        item {
            SectionCard(
                title = "已安装模块",
                subtitle = if (result?.ok == true) "共 ${result!!.modules.size} 个（来源：apd module list --json）"
                else "尚未加载；点击「刷新列表」",
            ) {
                Column {
                    ActionRow {
                        Button(
                            onClick = { refresh() },
                            enabled = apdReady && !loading && key != null,
                            colors = ButtonDefaults.buttonColorsPrimary(),
                        ) {
                            Text(if (loading) "加载中…" else "刷新列表", style = MiuixTheme.textStyles.button)
                        }
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
                        InfoRow("操作结果", it, valueColor = MiuixTheme.colorScheme.onSurfaceContainer)
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

        // ---------------- 列表 ----------------
        val modules = result?.modules.orEmpty()
        if (result?.ok == true && modules.isEmpty()) {
            item {
                SectionCard(title = "（空）", subtitle = "AP 返回了空模块列表") {
                    BodyText("当前设备没有已安装模块。模块安装功能将在后续版本开放（已列入待办）。")
                }
            }
        }
        items(modules) { module ->
            ModuleCard(
                module = module,
                enabled = apdReady && key != null && !loading,
                onToggle = { enable ->
                    if (key == null) {
                        actionMsg = "密钥不可用"
                        return@ModuleCard
                    }
                    Thread {
                        val r = ModuleRepository.setEnabled(context, key, module.id, enable)
                        actionMsg = r.message
                        if (r.ok) refresh() else AppLog.error("Modules", r.message)
                    }.start()
                },
            )
            VSpacer(10)
        }

        item { VSpacer(14) }
        item {
            SectionCard(title = "说明") {
                Column {
                    BodyText("模块生命周期全部由 AP 用户态管理（契约 v1.1 §8.3）：App 只经 root-exec 调用，" +
                        "不直接读写 /data/adb。")
                    VSpacer(6)
                    BodyText("Schema v1（客户端提案，待 AP 对齐）：{id,name,version,author,description,enabled,update_pending}；" +
                        "缺字段显示 \"—\"，不伪造。")
                    VSpacer(6)
                    BodyText("root-exec 退出码：125=密钥无效/需重新修补（不重试）、126/127=需重新部署、124=超时。")
                }
            }
        }
        item { VSpacer(24) }
    }
}

@Composable
private fun ModuleCard(module: ModuleInfo, enabled: Boolean, onToggle: (Boolean) -> Unit) {
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
                    Text(
                        text = "有更新",
                        style = MiuixTheme.textStyles.footnote1,
                        color = MiuixTheme.colorScheme.error,
                    )
                }
            }
            VSpacer(4)
            InfoRow("ID", module.id, monospace = true)
            InfoRow("版本 / 作者", "${module.version} · ${module.author}")
            if (module.description.isNotBlank()) {
                InfoRow("描述", module.description)
            }
            VSpacer(8)
            ActionRow {
                Switch(
                    checked = module.enabled,
                    onCheckedChange = { onToggle(it) },
                    enabled = enabled,
                )
                HSpacer(8)
                Text(
                    text = if (enabled) "切换开关经 apd module enable/disable（root-exec）"
                    else "AP 未就绪时禁用",
                    style = MiuixTheme.textStyles.footnote1,
                    color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
                )
            }
        }
    }
}
