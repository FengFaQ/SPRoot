package com.sproot.manager.ui.pages

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import com.sproot.manager.core.ApdClient
import com.sproot.manager.core.AppLog
import com.sproot.manager.core.SecretStore
import com.sproot.manager.ui.components.ActionRow
import com.sproot.manager.ui.components.BodyText
import com.sproot.manager.ui.components.HSpacer
import com.sproot.manager.ui.components.InfoRow
import com.sproot.manager.ui.components.SectionCard
import com.sproot.manager.ui.components.VSpacer
import top.yukonga.miuix.kmp.basic.Button
import top.yukonga.miuix.kmp.basic.ButtonDefaults
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.theme.MiuixTheme

/**
 * 模块页（骨架）。
 *
 * 模块生命周期全部由 AP 用户态管理（契约 v1.1 §8.3）：`apd module install/list/...`，
 * 一律经 root-exec 包裹，App 侧只负责展示与发起，不直接改 /data/adb。
 *
 * 本阶段只做：状态探测（apd version --json）+ 空态说明 + 刷新。
 * 真正的模块列表解析在 task-5 之后接入（需要 apd 输出 JSON 结构定稿）。
 */
@Composable
fun ModulesPage(modifier: Modifier = Modifier) {
    val context = LocalContext.current
    var probe by remember { mutableStateOf<String?>(null) }
    var probing by remember { mutableStateOf(false) }

    LazyColumn(modifier = modifier.fillMaxSize(), contentPadding = PaddingValues(16.dp)) {
        item {
            SectionCard(
                title = "模块管理",
                subtitle = "模块的安装/启用/卸载由 AP 用户态完成；App 不直接写 /data/adb",
            ) {
                Column {
                    InfoRow("AP 状态", ApdClient.status(context).detail)
                    probe?.let { InfoRow("版本探测", it, monospace = true) }
                    VSpacer(10)
                    ActionRow {
                        Button(
                            onClick = {
                                if (probing) return@Button
                                probing = true
                                Thread {
                                    val result = ApdClient.versionProbe(context)
                                    probe = buildString {
                                        append("exit=").append(result.exitCode)
                                        append("：").append(result.describe())
                                        if (result.stdout.isNotBlank()) append("\n").append(result.stdout.trim())
                                        if (result.stderr.isNotBlank()) append("\nstderr: ").append(result.stderr.trim())
                                    }
                                    probing = false
                                }.start()
                            },
                            colors = ButtonDefaults.buttonColorsPrimary(),
                        ) {
                            Text(if (probing) "探测中…" else "刷新状态", style = MiuixTheme.textStyles.button)
                        }
                    }
                }
            }
        }

        item { VSpacer(14) }

        item {
            SectionCard(title = "已安装模块", subtitle = "（空）") {
                Column {
                    BodyText("尚未读取到模块列表。")
                    VSpacer(6)
                    BodyText(
                        "接入顺序：AP 部署完成 → `apd module list --json` 经 root-exec 调用 → 解析 JSON 渲染列表。" +
                            "在此之前显示空态，不猜测、不伪造条目。",
                    )
                    VSpacer(6)
                    BodyText(
                        "注意：root-exec 返回 125 表示密钥无效或需重新修补 → 页面会提示" +
                            "「密钥无效或需重新修补」，且不会自动重试（契约 v1.1 §8.5）。",
                    )
                }
            }
        }

        item { VSpacer(14) }

        item {
            SectionCard(title = "模块操作（未启用）") {
                Column {
                    BodyText("安装 / 卸载 / 启用 / 禁用 / 动作执行：等待 AP 子命令与 JSON 契约定稿后接入。")
                    VSpacer(6)
                    BodyText("所有调用都将走 PrivilegedClient（root-exec），不拼 sh -c、不缓存密钥。")
                }
            }
        }

        item { VSpacer(14) }
        item {
            SectionCard(title = "调试") {
                Column {
                    ActionRow {
                        Button(onClick = {
                            val key = runCatching { SecretStore.get(context).getOrCreateRootKey() }.getOrNull()
                            if (key == null) {
                                AppLog.error("Modules", "密钥不可用，无法执行 root-exec 探针")
                                return@Button
                            }
                            Thread {
                                val result = ApdClient.apd(
                                    context = context,
                                    superKey = key,
                                    subCommand = listOf("version", "--json"),
                                    timeoutSec = 20,
                                )
                                AppLog.info(
                                    "Modules",
                                    "root-exec 探针结果：exit=${result.exitCode} ${result.describe()}",
                                )
                            }.start()
                        }) {
                            Text("root-exec 探针", style = MiuixTheme.textStyles.button)
                        }
                        HSpacer(8)
                        Text(
                            "会走完整「临时密钥文件 → apd root-exec → 删文件」链路",
                            style = MiuixTheme.textStyles.footnote1,
                            color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
                        )
                    }
                }
            }
        }

        item { VSpacer(24) }
    }
}
