package com.sproot.manager.ui.pages

import android.os.Build
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import com.sproot.manager.core.ApdClient
import com.sproot.manager.core.ApdSelfTest
import com.sproot.manager.core.AppLog
import com.sproot.manager.core.Capabilities
import com.sproot.manager.core.CapabilityItem
import com.sproot.manager.core.InstallState
import com.sproot.manager.core.SecretStore
import com.sproot.manager.ui.components.ActionRow
import com.sproot.manager.ui.components.HSpacer
import com.sproot.manager.ui.components.BodyText
import com.sproot.manager.ui.components.InfoRow
import com.sproot.manager.ui.components.SectionCard
import com.sproot.manager.ui.components.StatusBadge
import com.sproot.manager.ui.components.VSpacer
import com.sproot.manager.ui.components.maskSecret
import top.yukonga.miuix.kmp.basic.Button
import top.yukonga.miuix.kmp.basic.ButtonDefaults
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.theme.MiuixTheme

/**
 * 主页：一屏看清「能不能修、修完能不能提权」。
 * 状态卡只展示事实与自检结果，不展示任何猜测性"已 Root"文案。
 */
@Composable
fun HomePage(
    modifier: Modifier = Modifier,
    onGoPatch: () -> Unit = {},
) {
    val context = LocalContext.current
    var caps by remember { mutableStateOf<List<CapabilityItem>>(emptyList()) }
    var checked by remember { mutableStateOf(false) }
    var checking by remember { mutableStateOf(false) }

    val rootKey = remember { runCatching { SecretStore.get(context).getOrCreateRootKey() }.getOrNull() }
    val apd = remember { ApdClient.status(context) }
    val apdVersion = remember { runCatching { ApdClient.versionProbe(context) }.getOrNull() }
    var installStatus by remember { mutableStateOf(InstallState.load(context)) }

    LazyColumn(modifier = modifier.fillMaxSize(), contentPadding = PaddingValues(16.dp)) {
        item {
            SectionCard(
                title = "安装状态 · ${installStatus.status.label}",
                subtitle = installStatus.status.hint,
            ) {
                Column {
                    InfoRow("产物", installStatus.artifactDisplayPath ?: "无")
                    if (installStatus.artifactSha256 != null) {
                        InfoRow("sha256", installStatus.artifactSha256!!, monospace = true)
                    }
                    InfoRow("最近更新", installStatus.updatedAtText)
                    installStatus.note?.let { InfoRow("备注", it) }
                    VSpacer(10)
                    ActionRow {
                        Button(
                            onClick = { installStatus = InstallState.load(context) },
                            colors = ButtonDefaults.buttonColors(),
                        ) { Text("刷新状态", style = MiuixTheme.textStyles.button) }
                        HSpacer(8)
                        Button(
                            onClick = { onGoPatch() },
                            colors = ButtonDefaults.buttonColorsPrimary(),
                        ) { Text("前往修补", style = MiuixTheme.textStyles.button) }
                    }
                }
            }
        }

        item { VSpacer(14) }

        item {
            SectionCard(
                title = "SPRoot 状态",
                subtitle = "内核修补在 App 进程内完成（无需 root）；特权操作统一经 AP（root-exec）下发",
            ) {
                InfoRow(
                    label = "修补引擎",
                    value = if (checked) {
                        caps.firstOrNull { it.key == "engine" }?.detail ?: "未检测"
                    } else {
                        "未检测（点击下方「开始自检」）"
                    },
                )
                InfoRow(
                    label = "AP 守护（root-exec 唯一入口）",
                    value = buildString {
                        append(apd.detail)
                        ApdSelfTest.load(context)?.let {
                            append("\n自检：").append(it.stageLabel).append("（exit=").append(it.exitCode).append("）")
                        }
                        apdVersion?.let { v ->
                            append("\nversion: ")
                            append(v.stdout.lineSequence().firstOrNull()?.take(160) ?: "(exit=${v.exitCode})")
                        }
                    },
                )
                InfoRow(
                    label = "Root 密钥",
                    value = if (rootKey != null) {
                        "已就绪：${maskSecret(rootKey)}（47 字符，内核 ABI 47+NUL；前 16 位即 /data 隐藏目录前缀）"
                    } else {
                        "不可用 —— 请到设置页重新生成"
                    },
                )
            }
        }

        item { VSpacer(14) }

        item {
            SectionCard(title = "设备信息") {
                InfoRow("机型", "${Build.MANUFACTURER} ${Build.MODEL}（${Build.DEVICE}）")
                InfoRow(
                    "系统",
                    "Android ${Build.VERSION.RELEASE} (API ${Build.VERSION.SDK_INT})，${
                        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.VANILLA_ICE_CREAM) "" else ""
                    }build=${Build.ID}",
                )
                InfoRow("ABI", Build.SUPPORTED_ABIS.joinToString(", "))
                InfoRow("内核", System.getProperty("os.version") ?: "未知")
                InfoRow("App", "${com.sproot.manager.BuildConfig.VERSION_NAME} (${com.sproot.manager.BuildConfig.BUILD_TYPE})")
            }
        }

        item { VSpacer(14) }

        item {
            SectionCard(
                title = "环境自检",
                subtitle = "每次修补前都会自动跑一遍；这里可手动复检",
            ) {
                Column {
                    Button(
                        onClick = {
                            if (checking) return@Button
                            checking = true
                            Thread {
                                val result = Capabilities.collect(context)
                                caps = result
                                checked = true
                                checking = false
                                AppLog.info("Home", "手动自检完成：${result.count { it.ok }}/${result.size} 通过")
                            }.start()
                        },
                        colors = ButtonDefaults.buttonColorsPrimary(),
                    ) {
                        Text(if (checking) "自检中…" else if (checked) "重新自检" else "开始自检", style = MiuixTheme.textStyles.button)
                    }
                    VSpacer(8)
                    if (checked && caps.isEmpty()) {
                        BodyText("自检无结果（异常）")
                    }
                    caps.forEach { item -> CapabilityRow(item) }
                    if (!checked) {
                        BodyText("尚未自检。修补前必须通过：ABI / 引擎 / magiskboot / 存储 / 密钥。")
                    }
                }
            }
        }

        item { VSpacer(14) }

        item {
            SectionCard(title = "下一步") {
                Column {
                    BodyText("① 准备好本机 boot.img（从同版本固件中提取，勿用他人机型镜像）")
                    VSpacer(6)
                    BodyText("② 到「修补」页选择镜像并开始修补，全过程错误会常驻显示")
                    VSpacer(10)
                    Button(onClick = onGoPatch, colors = ButtonDefaults.buttonColorsPrimary()) {
                        Text("前往修补页", style = MiuixTheme.textStyles.button)
                    }
                }
            }
        }

        item { VSpacer(24) }
    }
}

@Composable
private fun CapabilityRow(item: CapabilityItem) {
    Column(modifier = Modifier.fillMaxSize()) {
        VSpacer(8)
        Text(
            text = "${if (item.ok) "✓" else "✗"} ${item.label}",
            style = MiuixTheme.textStyles.main,
            color = if (item.ok) MiuixTheme.colorScheme.onSurfaceContainer else MiuixTheme.colorScheme.error,
        )
        Text(
            text = item.detail,
            style = MiuixTheme.textStyles.footnote1,
            color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
        )
        if (!item.ok && item.blocking) {
            StatusBadge(ok = false, text = "阻断项：不修复则无法修补")
        }
    }
}
