package com.sproot.manager.ui.pages

import android.content.Intent
import android.net.Uri
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
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
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.painterResource
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.sproot.manager.BuildConfig
import com.sproot.manager.R
import com.sproot.manager.core.PatchEngine
import com.sproot.manager.ui.components.BlurredBar
import com.sproot.manager.ui.components.rememberBlurBackdrop
import com.sproot.manager.ui.theme.LocalEnableBlur
import top.yukonga.miuix.kmp.basic.Card
import top.yukonga.miuix.kmp.basic.Icon
import top.yukonga.miuix.kmp.basic.IconButton
import top.yukonga.miuix.kmp.basic.MiuixScrollBehavior
import top.yukonga.miuix.kmp.basic.Scaffold
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.basic.TopAppBar
import top.yukonga.miuix.kmp.blur.layerBackdrop
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.extended.Back
import top.yukonga.miuix.kmp.preference.ArrowPreference
import top.yukonga.miuix.kmp.theme.MiuixTheme
import top.yukonga.miuix.kmp.theme.MiuixTheme.colorScheme
import top.yukonga.miuix.kmp.utils.overScrollVertical
import top.yukonga.miuix.kmp.utils.scrollEndHaptic
import top.yukonga.miuix.kmp.window.WindowDialog

/**
 * 关于页 —— 版式照抄 KernelSU `ui/screen/about/AboutMiuix.kt`：
 * 顶部 App 图标（96dp）+ 名称 + 版本，下面用 miuix `ArrowPreference` 卡片列出
 * 仓库/许可/NOTICE/第三方组件/免责声明；点击后用 `WindowDialog` 展示等宽全文。
 *
 * 许可说明（与仓库根 NOTICE/LICENSES.md 口径一致，正文只在构建期由 CI 拼装，
 * 客户端只读展示，不自行改写）：
 *  - 本项目 AGPL-3.0；
 *  - KernelSU miuix UI 参考实现为 GPL-3.0（兼容 AGPL-3.0，来源声明在 NOTICE）；
 *  - magiskboot 为 GPL-3.0；miuix/Compose/Kotlin 为 Apache-2.0。
 */
@Composable
fun AboutPager(onBack: () -> Unit = {}) {
    val context = LocalContext.current
    val scrollBehavior = MiuixScrollBehavior()
    val backdrop = rememberBlurBackdrop(LocalEnableBlur.current)
    val barColor = if (backdrop != null) Color.Transparent else colorScheme.surface

    var dialogTitle by remember { mutableStateOf<String?>(null) }
    var dialogText by remember { mutableStateOf<String?>(null) }

    fun showAsset(title: String, assetPath: String) {
        dialogTitle = title
        dialogText = runCatching {
            context.assets.open(assetPath).bufferedReader().use { it.readText() }
        }.getOrElse { "无法读取 assets/$assetPath：${it.message}" }
    }

    fun openUrl(url: String) {
        runCatching {
            context.startActivity(Intent(Intent.ACTION_VIEW, Uri.parse(url)))
        }
    }

    WindowDialog(
        show = dialogText != null,
        title = dialogTitle ?: "",
        onDismissRequest = { dialogText = null },
        content = {
            Text(
                text = dialogText.orEmpty(),
                fontSize = 12.sp,
                fontFamily = FontFamily.Monospace,
                color = colorScheme.onSurface,
                modifier = Modifier
                    .fillMaxWidth()
                    .heightIn(max = 420.dp)
                    .verticalScroll(rememberScrollState()),
            )
        },
    )

    Scaffold(
        topBar = {
            BlurredBar(backdrop) {
                TopAppBar(
                    color = barColor,
                    title = "关于",
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
                contentPadding = innerPadding,
                overscrollEffect = null,
            ) {
                item {
                    Column(
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(top = 24.dp, bottom = 12.dp),
                        horizontalAlignment = Alignment.CenterHorizontally,
                        verticalArrangement = Arrangement.spacedBy(6.dp),
                    ) {
                        Image(
                            painter = painterResource(R.drawable.ic_launcher),
                            contentDescription = "SPRoot",
                            modifier = Modifier.size(96.dp),
                        )
                        Text(
                            text = "SPRoot",
                            fontSize = 22.sp,
                            fontWeight = FontWeight.SemiBold,
                            color = colorScheme.onSurface,
                        )
                        Text(
                            text = "版本 ${BuildConfig.VERSION_NAME}（${BuildConfig.BUILD_TYPE}）· code ${BuildConfig.VERSION_CODE}",
                            fontSize = 13.sp,
                            color = colorScheme.onSurfaceVariantSummary,
                        )
                        Text(
                            text = "SK 内核修补 + AP 特权链 · 全程离线",
                            fontSize = 13.sp,
                            color = colorScheme.onSurfaceVariantSummary,
                        )
                    }
                }

                item {
                    Card(modifier = Modifier.fillMaxWidth()) {
                        ArrowPreference(
                            title = "项目仓库",
                            summary = "https://github.com/FengFaQ/SPRoot",
                            onClick = { openUrl("https://github.com/FengFaQ/SPRoot") },
                        )
                        ArrowPreference(
                            title = "UI 参考实现",
                            summary = "KernelSU（GPL-3.0）miuix 版式 · https://github.com/tiann/KernelSU",
                            onClick = { openUrl("https://github.com/tiann/KernelSU") },
                        )
                        ArrowPreference(
                            title = "开源许可",
                            summary = "本项目 AGPL-3.0（含 KernelSU GPL-3.0 / magiskboot GPL-3.0 来源声明）",
                            onClick = { showAsset("NOTICE / 许可", "NOTICE") },
                        )
                        ArrowPreference(
                            title = "第三方组件",
                            summary = "Miuix 0.9.4 · Compose · Kotlin（Apache-2.0）；magiskboot（GPL-3.0）",
                            onClick = { showAsset("第三方组件声明", "NOTICE-APK.md") },
                        )
                    }
                }

                item {
                    Card(
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(top = 12.dp),
                    ) {
                        Column(modifier = Modifier.padding(16.dp)) {
                            Text(
                                text = "工具链与契约",
                                fontSize = MiuixTheme.textStyles.headline1.fontSize,
                                fontWeight = FontWeight.Medium,
                                color = colorScheme.onSurface,
                            )
                            Text(
                                text = "UI：Miuix 0.9.4（Compose Multiplatform 1.12.0 坐标）\n" +
                                    "工具链：AGP 9.4.1 / Kotlin 2.4.20 / Gradle 9.7.1 / JDK 21\n" +
                                    "修补引擎绑定契约：JNI v${PatchEngine.JNI_BINDING_VERSION}（C1 §1 参数语义）\n" +
                                    "AP 调用契约：v1.1 §8（root-exec 唯一入口，只认 exit code）",
                                fontSize = 13.sp,
                                fontFamily = FontFamily.Monospace,
                                color = colorScheme.onSurfaceVariantSummary,
                            )
                        }
                    }
                }

                item {
                    Card(
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(top = 12.dp),
                    ) {
                        Column(modifier = Modifier.padding(16.dp)) {
                            Text(
                                text = "免责声明",
                                fontSize = MiuixTheme.textStyles.headline1.fontSize,
                                fontWeight = FontWeight.Medium,
                                color = colorScheme.onSurface,
                            )
                            Text(
                                text = "刷机有风险：仅刷 boot 分区，不碰 vbmeta；" +
                                    "bootloader 未解锁时刷入将无法开机（属预期）。" +
                                    "请自行确认镜像来源与设备型号，刷前保留原镜像备份（设置 → 备份）。\n\n" +
                                    "本 App 不申请网络权限，不在构建期或运行期下载任何二进制。",
                                fontSize = 13.sp,
                                color = colorScheme.onSurfaceVariantSummary,
                            )
                        }
                    }
                }

                item {
                    Card(
                        modifier = Modifier
                            .fillMaxWidth()
                            .padding(top = 12.dp),
                    ) {
                        ArrowPreference(
                            title = "magiskboot 许可全文",
                            summary = "GPL-3.0（Magisk v31.1 官方二进制，随 APK 分发）",
                            onClick = { showAsset("magiskboot 许可", "licenses/GPL-3.0.txt") },
                        )
                        ArrowPreference(
                            title = "magiskboot 来源声明",
                            summary = "版本 / 来源 URL / sha256",
                            onClick = { showAsset("magiskboot 来源", "NOTICE-magiskboot.txt") },
                        )
                    }
                }

                item { Spacer(Modifier.height(24.dp)) }
            }
        }
    }
}
