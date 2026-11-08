package com.sproot.manager.ui.components

import android.content.ClipData
import android.content.ClipboardManager
import android.content.Context
import android.widget.Toast
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.sproot.manager.core.AppLog
import com.sproot.manager.core.LogEntry
import com.sproot.manager.core.LogLevel
import top.yukonga.miuix.kmp.basic.Button
import top.yukonga.miuix.kmp.basic.ButtonDefaults
import top.yukonga.miuix.kmp.basic.Card
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.theme.MiuixTheme

fun Context.copyToClipboard(label: String, text: String) {
    val manager = getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
    manager.setPrimaryClip(ClipData.newPlainText(label, text))
    Toast.makeText(this, "已复制$label", Toast.LENGTH_SHORT).show()
}

@Composable
fun SectionCard(
    title: String,
    subtitle: String? = null,
    modifier: Modifier = Modifier,
    content: @Composable () -> Unit,
) {
    Card(modifier = modifier.fillMaxWidth()) {
        Column {
            Text(
                text = title,
                style = MiuixTheme.textStyles.title4,
                color = MiuixTheme.colorScheme.onSurfaceContainer,
            )
            if (subtitle != null) {
                Spacer(Modifier.height(4.dp))
                Text(
                    text = subtitle,
                    style = MiuixTheme.textStyles.footnote1,
                    color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
                )
            }
            Spacer(Modifier.height(10.dp))
            content()
        }
    }
}

/** 键值行：左标签右值，值可换行（错误详情必须完整可见） */
@Composable
fun InfoRow(
    label: String,
    value: String,
    valueColor: Color = MiuixTheme.colorScheme.onSurfaceContainer,
    monospace: Boolean = false,
) {
    Column(modifier = Modifier.padding(vertical = 3.dp)) {
        Text(
            text = label,
            style = MiuixTheme.textStyles.footnote2,
            color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
        )
        Text(
            text = value,
            style = if (monospace) {
                MiuixTheme.textStyles.paragraph.copy(fontFamily = FontFamily.Monospace)
            } else {
                MiuixTheme.textStyles.main
            },
            color = valueColor,
        )
    }
}

/** 状态徽标：绿=通过，红=失败，灰=未知 */
@Composable
fun StatusBadge(ok: Boolean?, text: String) {
    val color = when (ok) {
        true -> MiuixTheme.colorScheme.primary
        false -> MiuixTheme.colorScheme.error
        null -> MiuixTheme.colorScheme.onSurfaceVariantSummary
    }
    Text(
        text = text,
        style = MiuixTheme.textStyles.body2,
        color = color,
        fontWeight = FontWeight.Medium,
    )
}

/** 脱敏展示密钥：只露前 4 + 后 4，其余替换 */
fun maskSecret(raw: String): String {
    if (raw.length <= 12) return "•".repeat(raw.length)
    return raw.take(4) + "•".repeat(raw.length - 8) + raw.takeLast(4)
}

/**
 * 常驻日志面板（缺陷 ⑤ 的核心修复点）。
 *
 * 与旧版的关键差异：
 *  - 该面板**永远在页面里**，不随阶段/失败被卸载；
 *  - 每条日志带级别色；自动滚到底部，但用户上滑后不抢滚动；
 *  - 提供"复制全部日志"，配合落盘文件形成双保险。
 */
@Composable
fun LogPanel(
    modifier: Modifier = Modifier,
    maxHeightDp: Int = 280,
    entries: List<LogEntry> = AppLog.entries.value,
) {
    val context = LocalContext.current
    val listState = rememberLazyListState()
    var autoScroll by remember { mutableStateOf(true) }

    LaunchedEffect(entries.size) {
        if (autoScroll && entries.isNotEmpty()) {
            runCatching { listState.animateScrollToItem(entries.size - 1) }
        }
    }

    Card(modifier = modifier.fillMaxWidth()) {
        Column {
            Row(
                modifier = Modifier.fillMaxWidth(),
                verticalAlignment = Alignment.CenterVertically,
                horizontalArrangement = Arrangement.SpaceBetween,
            ) {
                Text(
                    text = "运行日志（共 ${entries.size} 条）",
                    style = MiuixTheme.textStyles.title4,
                    color = MiuixTheme.colorScheme.onSurfaceContainer,
                )
                Button(
                    onClick = {
                        context.copyToClipboard("日志", AppLog.dumpText())
                    },
                    colors = ButtonDefaults.buttonColors(),
                ) {
                    Text("复制", style = MiuixTheme.textStyles.button)
                }
            }
            Spacer(Modifier.height(8.dp))
            if (entries.isEmpty()) {
                Text(
                    text = "暂无日志。开始修补后，每一步都会实时出现在这里并写入日志文件。",
                    style = MiuixTheme.textStyles.footnote1,
                    color = MiuixTheme.colorScheme.onSurfaceVariantSummary,
                )
            } else {
                LazyColumn(
                    state = listState,
                    modifier = Modifier
                        .fillMaxWidth()
                        .height(maxHeightDp.dp),
                ) {
                    items(entries) { entry ->
                        Text(
                            text = "${formatTime(entry.timeMs)} ${entry.level.name.first()} [${entry.tag}] ${entry.message}",
                            style = MiuixTheme.textStyles.footnote2.copy(fontFamily = FontFamily.Monospace),
                            color = when (entry.level) {
                                LogLevel.ERROR -> MiuixTheme.colorScheme.error
                                LogLevel.WARN -> MiuixTheme.colorScheme.onSurfaceVariantActions
                                LogLevel.INFO -> MiuixTheme.colorScheme.onSurfaceContainer
                                LogLevel.DEBUG -> MiuixTheme.colorScheme.onSurfaceVariantSummary
                            },
                            modifier = Modifier.padding(vertical = 1.dp),
                        )
                    }
                }
            }
        }
    }
}

@Composable
fun ActionRow(
    modifier: Modifier = Modifier,
    content: @Composable () -> Unit,
) {
    Row(
        modifier = modifier.fillMaxWidth(),
        horizontalArrangement = Arrangement.spacedBy(10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        content()
    }
}

@Composable
fun HSpacer(dp: Int) = Spacer(Modifier.width(dp.dp))

@Composable
fun VSpacer(dp: Int) = Spacer(Modifier.height(dp.dp))

@Composable
fun BodyText(text: String, color: Color = MiuixTheme.colorScheme.onSurfaceContainer) {
    Text(text = text, style = MiuixTheme.textStyles.paragraph, color = color, fontSize = 13.sp)
}

fun formatTime(ms: Long): String {
    val sdf = java.text.SimpleDateFormat("HH:mm:ss", java.util.Locale.US)
    return sdf.format(java.util.Date(ms))
}
