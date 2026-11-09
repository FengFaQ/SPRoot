package com.sproot.manager.ui.pages

import android.content.Context
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
import com.sproot.manager.BuildConfig
import com.sproot.manager.core.ApdClient
import com.sproot.manager.core.ApdSelfTest
import com.sproot.manager.core.AppLog
import com.sproot.manager.core.BackupStore
import com.sproot.manager.core.PatchEngine
import com.sproot.manager.core.SecretStore
import com.sproot.manager.ui.components.ActionRow
import com.sproot.manager.ui.components.BodyText
import com.sproot.manager.ui.components.HSpacer
import com.sproot.manager.ui.components.InfoRow
import com.sproot.manager.ui.components.SectionCard
import com.sproot.manager.ui.components.VSpacer
import com.sproot.manager.ui.components.copyToClipboard
import com.sproot.manager.ui.components.maskSecret
import top.yukonga.miuix.kmp.basic.Button
import top.yukonga.miuix.kmp.basic.ButtonDefaults
import top.yukonga.miuix.kmp.basic.Switch
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.theme.MiuixTheme

/**
 * 设置页：主题 / Root 密钥 / 日志 / AP 部署 / 关于。
 */
@Composable
fun SettingsPage(
    modifier: Modifier = Modifier,
    colorMode: Int = 0,
    onColorModeChange: (Int) -> Unit = {},
) {
    val context = LocalContext.current
    val store = remember { SecretStore.get(context) }
    var key by remember { mutableStateOf(runCatching { store.readRootKey() }.getOrNull()) }
    var showKey by remember { mutableStateOf(false) }
    var deployResult by remember { mutableStateOf<String?>(null) }
    var deploying by remember { mutableStateOf(false) }
    var backups by remember { mutableStateOf<List<BackupStore.Backup>>(emptyList()) }
    var backupMsg by remember { mutableStateOf<String?>(null) }
    var engineReport by remember { mutableStateOf<String?>(null) }

    LazyColumn(modifier = modifier.fillMaxSize(), contentPadding = PaddingValues(16.dp)) {
        item {
            SectionCard(title = "外观", subtitle = "Miuix 主题；默认跟随系统亮暗") {
                Column {
                    ActionRow {
                        Button(
                            onClick = { onColorModeChange(0) },
                            colors = if (colorMode == 0) ButtonDefaults.buttonColorsPrimary() else ButtonDefaults.buttonColors(),
                        ) { Text("跟随系统", style = MiuixTheme.textStyles.button) }
                        HSpacer(8)
                        Button(
                            onClick = { onColorModeChange(1) },
                            colors = if (colorMode == 1) ButtonDefaults.buttonColorsPrimary() else ButtonDefaults.buttonColors(),
                        ) { Text("浅色", style = MiuixTheme.textStyles.button) }
                        HSpacer(8)
                        Button(
                            onClick = { onColorModeChange(2) },
                            colors = if (colorMode == 2) ButtonDefaults.buttonColorsPrimary() else ButtonDefaults.buttonColors(),
                        ) { Text("深色", style = MiuixTheme.textStyles.button) }
                    }
                }
            }
        }

        item { VSpacer(14) }

        item {
            SectionCard(
                title = "Root 密钥",
                subtitle = "47 字符 [A-Za-z0-9]（内核 ABI 47+NUL）；前 16 位即隐藏目录前缀。debug/release 行为一致",
            ) {
                Column {
                    InfoRow(
                        label = "当前密钥",
                        value = when {
                            key == null -> "不可用（点击下方重新生成）"
                            showKey -> key!!
                            else -> maskSecret(key!!)
                        },
                        monospace = true,
                    )
                    if (SecretStore.legacy48Detected) {
                        VSpacer(6)
                        InfoRow(
                            label = "旧密钥提示",
                            value = "检测到历史 48 字符密钥，已取前 47 位使用；" +
                                "建议重新生成密钥并重新修补/刷入镜像（旧镜像内嵌密钥可能不一致）",
                            valueColor = MiuixTheme.colorScheme.error,
                        )
                    }
                    VSpacer(10)
                    ActionRow {
                        Button(onClick = { showKey = !showKey }) {
                            Text(if (showKey) "隐藏" else "显示", style = MiuixTheme.textStyles.button)
                        }
                        HSpacer(8)
                        Button(onClick = { key?.let { context.copyToClipboard("Root 密钥", it) } }) {
                            Text("复制", style = MiuixTheme.textStyles.button)
                        }
                        HSpacer(8)
                        Button(
                            onClick = {
                                key = runCatching { store.rotate() }.onFailure {
                                    AppLog.error("Settings", "轮换密钥失败", it)
                                }.getOrNull()
                                AppLog.warn("Settings", "已轮换 Root 密钥：旧密钥修补的内核必须重新修补并刷入")
                            },
                            colors = ButtonDefaults.buttonColorsPrimary(),
                        ) { Text("重新生成", style = MiuixTheme.textStyles.button) }
                    }
                    VSpacer(8)
                    BodyText("⚠️ 重新生成后，已刷入的旧内核将无法提权，必须重新走一遍「修补 → 刷入」。")
                }
            }
        }

        item { VSpacer(14) }

        item {
            SectionCard(title = "日志", subtitle = "每条日志实时落盘，进程被杀也不丢") {
                Column {
                    InfoRow("日志文件", AppLog.currentLogFile()?.absolutePath ?: "未初始化", monospace = true)
                    InfoRow("内存条数", "${AppLog.entries.value.size}（上限 4000，滚动淘汰）")
                    VSpacer(10)
                    ActionRow {
                        Button(onClick = { context.copyToClipboard("全部日志", AppLog.dumpText()) }) {
                            Text("复制全部日志", style = MiuixTheme.textStyles.button)
                        }
                        HSpacer(8)
                        Button(onClick = { AppLog.clearMemory() }) {
                            Text("清空内存日志", style = MiuixTheme.textStyles.button)
                        }
                    }
                }
            }
        }

        item { VSpacer(14) }

        item {
            SectionCard(
                title = "原镜像备份（sha1 内容寻址）",
                subtitle = "每次修补前自动备份 boot.img；可还原到 Downloads/SPRoot/ 供 fastboot 刷回",
            ) {
                Column {
                    ActionRow {
                        Button(onClick = {
                            Thread {
                                backups = BackupStore.list(context)
                                backupMsg = "已加载 ${backups.size} 条备份"
                            }.start()
                        }) { Text("刷新备份列表", style = MiuixTheme.textStyles.button) }
                    }
                    backupMsg?.let {
                        VSpacer(6)
                        InfoRow("结果", it)
                    }
                    if (backups.isEmpty()) {
                        VSpacer(6)
                        BodyText("暂无备份。第一次修补时会自动创建。", color = MiuixTheme.colorScheme.onSurfaceVariantSummary)
                    }
                    backups.take(10).forEach { backup ->
                        VSpacer(8)
                        InfoRow("sha1", backup.sha1, monospace = true)
                        InfoRow("原始文件名", backup.sourceName)
                        InfoRow("大小 / 时间", "${com.sproot.manager.core.Capabilities.formatBytes(backup.size)} · ${java.text.SimpleDateFormat("MM-dd HH:mm", java.util.Locale.US).format(java.util.Date(backup.createdAt))}")
                        ActionRow {
                            Button(
                                onClick = {
                                    Thread {
                                        val result = runCatching {
                                            BackupStore.restoreToDownloads(context, backup)
                                        }
                                        backupMsg = result.fold(
                                            onSuccess = { "已还原到 ${it.displayPath}" },
                                            onFailure = { "还原失败：${it.javaClass.simpleName}: ${it.message}" },
                                        )
                                    }.start()
                                },
                                colors = ButtonDefaults.buttonColors(),
                            ) { Text("还原到 Downloads", style = MiuixTheme.textStyles.button) }
                            HSpacer(8)
                            Button(onClick = {
                                Thread {
                                    backupMsg = if (BackupStore.delete(context, backup)) {
                                        "已删除备份 ${backup.sha1.take(8)}"
                                    } else {
                                        "删除失败"
                                    }
                                    backups = BackupStore.list(context)
                                }.start()
                            }) { Text("删除", style = MiuixTheme.textStyles.button) }
                        }
                    }
                }
            }
        }

        item { VSpacer(14) }

        item {
            SectionCard(
                title = "AP 部署（特权链）",
                subtitle = "修补链不需要 root；部署/模块/授权全部经 AP",
            ) {
                Column {
                    InfoRow("AP 二进制", ApdClient.status(context).detail, monospace = true)
                    ApdClient.versionMismatchHint(context)?.let {
                        VSpacer(6)
                        InfoRow("版本一致性", it, valueColor = MiuixTheme.colorScheme.error)
                    }
                    deployResult?.let {
                        VSpacer(6)
                        InfoRow("最近一次部署结果", it, monospace = true)
                    }
                    VSpacer(10)
                    ActionRow {
                        Button(
                            onClick = {
                                if (deploying) return@Button
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
                            enabled = !deploying,
                            colors = ButtonDefaults.buttonColorsPrimary(),
                        ) {
                            Text(if (deploying) "部署中…" else "部署 / 更新 AP", style = MiuixTheme.textStyles.button)
                        }
                    }
                    VSpacer(8)
                    BodyText("退出码 125 = 密钥无效或需重新修补（不自动重试）；126/127 = 需重新部署；124 = 超时。")
                }
            }
        }

        item { VSpacer(14) }

        item {
            SectionCard(
                title = "引擎自检（B-14 / qa JNI 验证入口）",
                subtitle = "调用 libskrootpatch.so 的 JNI engineVersion()，用于确认库已打进 APK 且 ABI 对齐",
            ) {
                Column {
                    engineReport?.let { InfoRow("自检结果", it, monospace = false) }
                    VSpacer(10)
                    ActionRow {
                        Button(
                            onClick = {
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
                            colors = ButtonDefaults.buttonColorsPrimary(),
                        ) { Text("引擎自检", style = MiuixTheme.textStyles.button) }
                        HSpacer(8)
                        Button(onClick = {
                            Thread {
                                val st = ApdSelfTest.run(context)
                                deployResult = "AP 自检：${st.describe()}"
                            }.start()
                        }) { Text("重跑 AP 自检", style = MiuixTheme.textStyles.button) }
                    }
                    VSpacer(6)
                    ApdSelfTest.load(context)?.let {
                        InfoRow("最近 AP 自检", "${it.stageLabel}（exit=${it.exitCode}）· ${it.summary}")
                    }
                }
            }
        }

        item { VSpacer(14) }

        item {
            SectionCard(title = "关于") {
                Column {
                    InfoRow("SPRoot 管理器", "${BuildConfig.VERSION_NAME} (${BuildConfig.BUILD_TYPE}) / code ${BuildConfig.VERSION_CODE}")
                    InfoRow("UI", "Miuix 0.9.4（Compose Multiplatform 1.12.0 坐标）")
                    InfoRow("工具链", "AGP 9.4.1 / Kotlin 2.4.20 / Gradle 9.7.1 / JDK 21")
                    InfoRow("修补引擎绑定契约", "JNI v${com.sproot.manager.core.PatchEngine.JNI_BINDING_VERSION}（C1 §1 参数语义）")
                    InfoRow("AP 调用契约", "v1.1 §8（root-exec 唯一入口，只认 exit code）")
                    VSpacer(8)
                    BodyText("本 App 全程离线工作：不申请网络权限，不在构建期或运行期下载任何二进制。")
                }
            }
        }

        item {
            SectionCard(title = "诊断开关", subtitle = "骨架阶段占位") {
                var checked by remember { mutableStateOf(false) }
                Column {
                    InfoRow("详细引擎日志", "（占位：接入后控制 --verbose）")
                    Switch(checked = checked, onCheckedChange = { checked = it })
                }
            }
        }

        item { VSpacer(24) }
    }
}

private fun secretOrNull(context: Context): String? =
    runCatching { SecretStore.get(context).readRootKey() }.getOrNull()
