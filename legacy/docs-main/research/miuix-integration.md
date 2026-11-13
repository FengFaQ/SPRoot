# Miuix UI 库集成调研（SPRoot / APatch 管理器）

- 调研对象：<https://github.com/compose-miuix-ui/miuix>
- 调研时间：2026-11-08
- 证据基线：
  - **Maven Central 上真实发布的 `0.9.4` 产物**（AAR 的 `aar-metadata.properties`、Gradle Module Metadata `.module`、官方 `-sources.jar`）——这是「消费者真正拿到的东西」，优先级最高。
  - GitHub 仓库 HEAD（commit `b59ed57`，2026-11-08）——仅用于理解仓库结构 / 模块划分 / 文档。
  - 本机 `SPRoot/APatch` 现有构建配置（AGP 9.3.1、Kotlin 2.4.10、Compose BOM 2026.08.00、compileSdk 37、minSdk 26）。
- **标注约定**：
  - 【实证】= 直接读到文件内容 / HTTP 响应 / 命令输出，文末「证据」给出处。
  - 【推断】= 由实证按 Gradle / Kotlin / AGP 的既定规则推导，**未在本机跑通真实构建**（本机无 Gradle、无 Android SDK，见证据 E0）。落地时请按第 5 节末尾的「验证命令」实测一次。

---

## 1. 结论摘要

1. **可以集成，且与 SPRoot 现有工具链几乎零冲突。** 最新稳定版是 `0.9.4`（Maven Central `release`/`latest` 均为 0.9.4，tag `v0.9.4` 发布于 2026-09-20）【实证 E1/E2】。AAR 元数据要求 `minCompileSdk=37`、`minSdkVersion=24`，而 SPRoot 是 `compileSdk=37` / `minSdk=26`【实证 E3/E4】，**正好满足，无需改 compileSdk**。
2. **Compose 版本天然对齐，不需要动 BOM。** `miuix-ui:0.9.4` 的元数据要求 `org.jetbrains.compose.foundation:foundation:1.12.0`，其在 Android 上重定向到 `androidx.compose.foundation:foundation:1.12.0`【实证 E5】；而 SPRoot 的 `compose-bom:2026.08.00` 恰好把 `ui/foundation/runtime` 钉在 `1.12.0`【实证 E6】。**同版本，无升级、无降级。**
3. **唯一必须处理的硬冲突：传递依赖会把 Material3 从 1.4.0 抬到 1.5.0-alpha22。** `miuix-ui`（runtime 变体）依赖 `org.jetbrains.compose.material3:material3-window-size-class:1.12.0-alpha03` → 重定向 `androidx.compose.material3:material3-window-size-class:1.5.0-alpha22`，该模块**携带 `androidx.compose.material3:material3 → 1.5.0-alpha22` 的版本约束**，而 SPRoot BOM 钉的是 `material3 1.4.0`【实证 E5/E6/E7】。且该依赖只在 **runtimeElements**（`implementation`）里，**compileClasspath 仍是 1.4.0** —— 编译期与运行期版本不一致，是「编译通过、运行 NoSuchMethodError」的典型来源【推断】。规避：`exclude` 掉该传递依赖（已确认 miuix 源码中**零引用**，见证据 E8）。
4. **Kotlin 建议从 2.4.10 升到 2.4.20。** miuix 0.9.4 依赖 `kotlin-stdlib:2.4.20`【实证 E5】，SPRoot 是 2.4.10。Gradle 会把 stdlib 抬到 2.4.20，编译器仍是 2.4.10（KGP 会告警；元数据 `mv` 同为 2.4.x，理论上可读）【推断】。升级只需改版本目录里 `kotlin = "2.4.20"`（compose 编译器插件同 ref，会一起升）。
5. **平台能力边界：blur / glass 在 SPRoot 的 minSdk 26 上不可用，squircle 会自动降级。** `miuix-blur-android:0.9.4` 的 Manifest 写死 `minSdkVersion=33`【实证 E9】；`miuix-glass` / `miuix-glass-icons` 在 0.9.4 **根本没有发布**（Maven Central 404，仅存在于 HEAD）【实证 E10】；squircle 通过 `isRuntimeShaderSupported()`（`SDK_INT >= TIRAMISU`）在低版本静默关闭，不会崩【实证 E11】。

---

## 2. Maven 依赖与仓库配置

### 2.1 坐标与发布形态（【实证】）

| 项 | 值 | 出处 |
| --- | --- | --- |
| groupId | `top.yukonga.miuix.kmp` | `build-logic/src/main/kotlin/BuildConfig.kt` (`LIBRARY_ID`) / `.module` |
| 版本号规范 | `MAJOR.MINOR.PATCH` + 可选 `-rcNN` / `-alphaNN` / `-betaNN` / `-<githash>-SNAPSHOT` | `build-logic/src/main/kotlin/module.publication.gradle.kts` |
| 已发布版本 | `0.9.0, 0.9.1, 0.9.2, 0.9.3, 0.9.4-rc01, 0.9.4` | Maven Central `maven-metadata.xml` |
| 托管仓库 | **Maven Central**（另有 GitHub Packages，非公开消费渠道） | `module.publication.gradle.kts` 中的 `sonatype` / `github` 仓库 |
| JitPack | **不使用**（无需配置，也不要去 JitPack 找） | 同上；Maven Central 元数据齐全 |
| 发布产物 | KMP 根模块 + 平台模块（`-android` / `-desktop` / `-iosarm64` / `-wasm-js` …） | Maven Central 目录列表 |

**版本号注意**：`LIBRARY_VERSION = "0.9.4"` 是仓库 HEAD 里的常量，`-rc01` 后缀来自 `-Prc=01` 构建参数。消费端只用**已发布版本号**，不要用 `+` / `latest.release` 动态版本。

### 2.2 仓库配置（SPRoot 现状已满足）

SPRoot 的 `APatch/settings.gradle.kts` 已有：

```kotlin
dependencyResolutionManagement {
    repositoriesMode = RepositoriesMode.FAIL_ON_PROJECT_REPOS
    repositories {
        google()
        mavenCentral()      // ← miuix 只需要这一个
        maven("https://jitpack.io")
    }
}
```

**Miuix 不需要额外仓库**：不需要 JitPack、不需要 `maven.pkg.github.com`。`google()` 是给它的传递依赖（`androidx.*`、`com.android.*`）用的，项目里已有。

### 2.3 依赖行（Android 单平台项目用 `-android` 后缀，官方文档明确要求）

```kotlin
dependencies {
    // 核心 UI 组件库（必选）
    implementation("top.yukonga.miuix.kmp:miuix-ui-android:0.9.4") {
        // 必须：阻断 Material3 被抬到 1.5.0-alpha22（见 §1.3 / §5.1）
        exclude(group = "org.jetbrains.compose.material3", module = "material3-window-size-class")
    }

    // 可选模块（按需，均只在 0.9.4 已发布的范围内）
    implementation("top.yukonga.miuix.kmp:miuix-preference-android:0.9.4") // 设置页偏好组件
    implementation("top.yukonga.miuix.kmp:miuix-icons-android:0.9.4")      // icon.extended 图标集
    implementation("top.yukonga.miuix.kmp:miuix-squircle-android:0.9.4")   // 圆角「squircle」形状
    implementation("top.yukonga.miuix.kmp:miuix-nav-android:0.9.4")        // 导航容器
    // 以下两个模块在 0.9.4 未发布 / 受 minSdk 限制，SPRoot 不可用：
    // implementation("top.yukonga.miuix.kmp:miuix-blur-android:0.9.4")    // 需 minSdk 33
    // implementation("top.yukonga.miuix.kmp:miuix-glass-android:0.9.4")   // 0.9.4 不存在（404）
}
```

> 说明：`miuix-ui`（不带后缀，KMP 根模块）在 Android 工程里**也能**解析到 androidJvm 变体，但官方 getting-started 对「Android Compose 项目」给的是 `-android` 坐标，用它可避免变体选择歧义【实证 E12：docs/guide/getting-started.md @ v0.9.4】。

### 2.4 版本目录（SPRoot 使用 `libs.versions.toml`，建议这样写入）

`APatch/gradle/libs.versions.toml`：

```toml
[versions]
miuix = "0.9.4"

[libraries]
miuix-ui = { group = "top.yukonga.miuix.kmp", name = "miuix-ui-android", version.ref = "miuix" }
miuix-icons = { group = "top.yukonga.miuix.kmp", name = "miuix-icons-android", version.ref = "miuix" }
miuix-preference = { group = "top.yukonga.miuix.kmp", name = "miuix-preference-android", version.ref = "miuix" }
miuix-squircle = { group = "top.yukonga.miuix.kmp", name = "miuix-squircle-android", version.ref = "miuix" }
miuix-nav = { group = "top.yukonga.miuix.kmp", name = "miuix-nav-android", version.ref = "miuix" }
```

`APatch/app/app/build.gradle.kts`（`exclude` 无法写进版本目录，必须写在依赖处）：

```kotlin
implementation(libs.miuix.ui) {
    exclude(group = "org.jetbrains.compose.material3", module = "material3-window-size-class")
}
```

**替代方案（不想用 exclude 时）**：把 BOM 从 `platform(...)` 改为 `enforcedPlatform(libs.androidx.compose.bom)`，从而把 `material3` 强制钉回 1.4.0。代价是整棵 Compose 依赖都被 BOM 严管（当前恰好一致，但以后想单点升级某个 compose 模块会更麻烦）【推断】。**推荐用 exclude**，因为 miuix 源码根本不引用 `WindowSizeClass`。

---

## 3. 版本兼容矩阵

### 3.1 miuix 0.9.4 的真实要求（全部来自已发布产物元数据）

| 维度 | 要求 | 证据 |
| --- | --- | --- |
| `compileSdk` | **37**（`minCompileSdk=37, minCompileMinorSdk=0`；低于 37 直接构建失败） | 【实证 E3】`aar-metadata.properties` |
| `minSdk`（库自身） | **24** | 【实证 E3】AAR Manifest `<uses-sdk android:minSdkVersion="24">` |
| 消费者 minSdk | 无额外限制（≥24 即可）；但 **blur 模块要求 33** | 【实证 E3/E9】 |
| Kotlin（编译库所用） | **2.4.20**（`kotlin-stdlib:2.4.20`） | 【实证 E5】`.module` |
| Compose（所需最低） | `foundation 1.12.0` → Android 端 `androidx.compose.foundation:foundation:1.12.0` | 【实证 E5】 |
| 其它运行时依赖 | `androidx.navigationevent:navigationevent-compose:1.1.2`、`org.jetbrains.compose.material3:material3-window-size-class:1.12.0-alpha03`、`com.materialkolor:material-color-utilities:5.0.1` | 【实证 E5】 |
| Material3 版本约束 | 传递约束 `androidx.compose.material3:material3 → 1.5.0-alpha22`（**会抬高消费者版本**） | 【实证 E7】 |
| AGP 下限 | `minAndroidGradlePluginVersion=1.0.0`（几乎无约束；库自身用 AGP 9.5.1 构建） | 【实证 E3】 |
| Gradle | 元数据 `formatVersion 1.1`，由 **Gradle 9.7.1** 生成 | 【实证 E5】 |
| JDK | 21（`jvmToolchain(21)`） | 【实证 E13】`module.kotlin-jvm-toolchain.gradle.kts` |
| core library desugaring | **不需要**（`coreLibraryDesugaringEnabled=false`） | 【实证 E3】 |
| Android 资源/字体 | **无**（AAR 内只有 `classes.jar` + `baseline-prof.txt`） | 【实证 E14】 |

### 3.2 对照 SPRoot 现状

| 维度 | miuix 0.9.4 要求 | SPRoot 现状 | 判定 |
| --- | --- | --- | --- |
| compileSdk | 37 | 37（`androidCompileSdkVersion = 37`） | ✅ 精确匹配【实证 E4】 |
| minSdk | 24（库）/ blur 需 33 | 26 | ✅ 核心库可用；❌ blur/glass 禁用 |
| Kotlin | stdlib 2.4.20 | `kotlin = "2.4.10"` | ⚠️ **建议升到 2.4.20**（推断：会有 stdlib 版本告警，元数据同 2.4.x 可读） |
| Compose BOM | foundation/runtime/ui 1.12.0 | BOM `2026.08.00` = 1.12.0 | ✅ 完全一致 |
| material3 | 传递约束 1.5.0-alpha22 | BOM 钉 1.4.0 | ⚠️ **必须 exclude**（否则 runtime 侧被抬到 alpha） |
| AGP | 下限 1.0.0 | 9.3.1 | ✅（推断：AAR 元数据无实质限制） |
| Gradle | 元数据由 9.7.1 产出 | wrapper 9.7.0 | ✅（推断：format 1.1，向后兼容） |
| JDK / jvmTarget | 21 | 21 | ✅ |
| KSP | 无关 | 2.3.11 | — 无影响 |
| Compose 编译器插件 | 需与 Kotlin 同版本（`org.jetbrains.kotlin.plugin.compose`） | `version.ref = kotlin` | ✅ 结构正确，随 Kotlin 一起升 |

**一句话矩阵结论**：只有两个动作项 —— ①`exclude` 掉 `material3-window-size-class`；②把 `kotlin` 提到 `2.4.20`。其余全部「已满足」。

---

## 4. 最小示例代码

> 下面所有签名都用 **0.9.4 官方 `miuix-ui-android-0.9.4-sources.jar`** 逐条核对过（`Text(text=...)`、`Button(onClick, colors, content: RowScope)`、`Card(insideMargin=...)`、`SmallTopAppBar(title: String, ...)`、`Scaffold(topBar, content: (PaddingValues)->Unit)`、`MiuixTheme(controller|colors, textStyles, content)`、`ThemeController(colorSchemeMode, lightColors, darkColors, keyColor, colorSpec, paletteStyle, isDark)`、`MiuixIcons.Basic`）【实证 E15】。**未在本机编译**（无 Gradle/SDK），但参数名与类型与 0.9.4 二进制一致。

### 4.1 主题包装（应用根节点）

```kotlin
package me.bmax.apatch.ui.miuix

import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import top.yukonga.miuix.kmp.theme.ColorSchemeMode
import top.yukonga.miuix.kmp.theme.MiuixTheme
import top.yukonga.miuix.kmp.theme.ThemeController

@Composable
fun MiuixAppTheme(
    content: @Composable () -> Unit,
) {
    // System = 跟随系统亮暗；MonetSystem = 跟随系统 + 壁纸取色（Android 12+ 才真正取到壁纸色，
    // 低于 S 时回落到内置调色板，不会崩）
    val controller = remember { ThemeController(ColorSchemeMode.System) }
    MiuixTheme(controller = controller, content = content)
}
```

### 4.2 一个页面：Scaffold + 小标题栏 + 卡片 + 按钮

```kotlin
package me.bmax.apatch.ui.miuix

import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import top.yukonga.miuix.kmp.basic.Button
import top.yukonga.miuix.kmp.basic.ButtonDefaults
import top.yukonga.miuix.kmp.basic.Card
import top.yukonga.miuix.kmp.basic.Icon
import top.yukonga.miuix.kmp.basic.IconButton
import top.yukonga.miuix.kmp.basic.Scaffold
import top.yukonga.miuix.kmp.basic.SmallTopAppBar
import top.yukonga.miuix.kmp.basic.Text
import top.yukonga.miuix.kmp.icon.MiuixIcons
import top.yukonga.miuix.kmp.icon.basic.Search

@Composable
fun MiuixDemoPage(
    onSearch: () -> Unit = {},
    onPrimary: () -> Unit = {},
) {
    Scaffold(
        topBar = {
            SmallTopAppBar(
                title = "SPRoot",
                subtitle = "KernelPatch 管理器",
                actions = {
                    IconButton(onClick = onSearch) {
                        Icon(MiuixIcons.Basic.Search, contentDescription = "Search")
                    }
                },
            )
        },
        content = { paddingValues ->
            Column(
                modifier = Modifier
                    .fillMaxSize()
                    .padding(top = paddingValues.calculateTopPadding())
                    .padding(horizontal = 26.dp, vertical = 12.dp),
            ) {
                Card(insideMargin = PaddingValues(16.dp)) {
                    Text("卡片内容：Miuix Card 默认 insideMargin 为 0.dp，需要自己给内边距。")
                }
                Spacer(Modifier.height(12.dp))
                Button(onClick = onPrimary) {
                    Text("主按钮")
                }
                Spacer(Modifier.height(8.dp))
                Button(
                    onClick = onPrimary,
                    colors = ButtonDefaults.buttonColorsPrimary(),
                ) {
                    Text("PRIMARY 风格")
                }
            }
        },
    )
}
```

> 上面 `ButtonDefaults` 需 `import top.yukonga.miuix.kmp.basic.ButtonDefaults`。0.9.4 的 `basic/Button.kt` 中确有 `object ButtonDefaults { MinWidth=58.dp; MinHeight=40.dp; CornerRadius=16.dp; InsideMargin=PaddingValues(horizontal=16.dp, vertical=13.dp); fun buttonColors(...); fun buttonColorsPrimary(...) }`【实证 E15】。

### 4.3 挂到 Activity

```kotlin
setContent {
    MiuixAppTheme {
        MiuixDemoPage()
    }
}
```

**注意**：`MiuixTheme` 必须包在**使用 Miuix 组件的整个子树**外层。不包也能渲染（CompositionLocal 有默认值，见 §5.3），但会静默使用「永远浅色 + 黑色文字」，暗色模式失效。

---

## 5. 陷阱清单

按「踩到概率 × 破坏力」排序。每条都标了实证/推断。

### 5.1 【高】Material3 被传递约束抬到 1.5.0-alpha22（编译 1.4.0 / 运行 alpha）
- **成因（实证）**：`miuix-ui-android:0.9.4` 的 `androidRuntimeElements` 含 `org.jetbrains.compose.material3:material3-window-size-class:1.12.0-alpha03`【E5】→ 该 JetBrains 模块在 Android 上重定向到 `androidx.compose.material3:material3-window-size-class:1.5.0-alpha22`【E5】→ 后者在 `androidApiElements`/`androidRuntimeElements` 里带 `dependencyConstraints: androidx.compose.material3:material3 = 1.5.0-alpha22`【E7】。
- **为什么危险（推断）**：Gradle 冲突解析取「最高版本」，约束非 `strictly` 也会抬高；而这条依赖只出现在 `runtimeElements`（`implementation` 语义），**compileClasspath 不包含它**，于是「编译时 1.4.0、运行时 1.5.0-alpha22」——APK 里 Material3 是 alpha 版，SPRoot 现有 Material3 代码可能 `NoSuchMethodError`。
- **处理**：`exclude(group = "org.jetbrains.compose.material3", module = "material3-window-size-class")`（推荐，源码零引用【E8】）；或 `enforcedPlatform(BOM)`。
- **复验**：`./gradlew :app:dependencies --configuration releaseRuntimeClasspath | findstr material3`。

### 5.2 【高】MiuixTheme 会改写 `LocalIndication` / `LocalOverscrollFactory` / `LocalContentColor`
- **实证 E16**：`MiuixTheme` 内部 `CompositionLocalProvider(LocalColors, LocalTextStyles, LocalIndication, LocalContentColor, LocalOverscrollFactory)`。
- **后果**：把 Material3 组件放进 `MiuixTheme` 里，Material 的涟漪/按压反馈、内容色、过度滚动效果都会变成 Miuix 的实现；反之 `MaterialTheme` 不影响 Miuix 组件（Miuix 只读自己的 `LocalColors`/`LocalTextStyles`）。
- **建议**：**按页面分区，不要深层互相嵌套**。Miuix 页面套 `MiuixTheme`，Material3 页面套 `MaterialTheme`，只在 Activity 根节点做二选一。

### 5.3 【中】不包主题不报错，但颜色是错的（静默失败）
- **实证 E16**：`LocalColors` 默认 `lightColorScheme()`、`LocalTextStyles` 默认 `defaultTextStyles()`、`LocalContentColor` 默认 `Color.Black`。
- **后果**：漏包 `MiuixTheme` 时组件照常渲染，只是永远浅色、内容色纯黑；暗色主题/动态取色全失效，且没有编译期或运行期提示。
- **建议**：把 `MiuixTheme` 放进「Miuix 页面」的统一入口，Code Review 时检查。

### 5.4 【中】组件同名冲突（`Text` / `Button` / `Icon` / `Card` / `Scaffold` / `IconButton` 全部同名）
- Miuix 自带 `Text`、`Button`、`Icon`、`IconButton`、`Card`、`Scaffold`、`TopAppBar`…，与 `androidx.compose.material3.*` 完全同名【实证 E17：0.9.4 源码 `basic/` 目录清单】。
- **后果**：同一文件里通配符 import 两个包会「后者覆盖/歧义」，或写成了 Miuix 组件却传 Material 的参数（例如给 `Card` 传 `elevation`），报「unresolved reference / no parameter named ...」。
- **建议**：Miuix 组件**显式单条 import**（不要 `import top.yukonga.miuix.kmp.basic.*`）；需要混用时加 `import ... as MiuixText` 别名。

### 5.5 【中】blur / glass 不可用，squircle 自动降级
- **实证 E9**：`miuix-blur-android:0.9.4` 的 Manifest 写死 `minSdkVersion=33`；【推断】SPRoot `minSdk=26` 上会先在 AAR metadata/Manifest merger 阶段失败（`uses-sdk:minSdkVersion 26 cannot be smaller than version 33 declared in library`）。
- **实证 E10**：`miuix-glass-android:0.9.4`、`miuix-glass-icons-android:0.9.4` **404**（0.9.4 未发布，只在 HEAD 有）。别照 HEAD 的 README 写依赖。
- **实证 E11**：squircle/shader 用 `Build.VERSION.SDK_INT >= TIRAMISU` 判定，`LocalSquircleEnabled && isRuntimeShaderSupported()` 才起效 → 低版本静默关闭，**不崩**。
- **结论**：SPRoot 只用 `miuix-ui` +（可选）`miuix-preference` / `miuix-icons` / `miuix-squircle` / `miuix-nav`。

### 5.6 【中】compileSdk 低于 37 直接构建失败
- **实证 E3**：`minCompileSdk=37`。报错形如 `Dependency 'top.yukonga.miuix.kmp:miuix-ui-android:0.9.4' requires libraries and applications that depend on it to compile against version 37 or later of the Android APIs`。
- SPRoot 已是 37，**无需改**；但若有人在分支上降 compileSdk（例如为了兼容旧 AGP），会立刻炸。

### 5.7 【中】HEAD 文档 ≠ 0.9.4 API（版本漂移）
- 实例（实证 E18）：HEAD 的 `docs/components/basic/topappbar.md` 示例用 `MiuixIcons.Back`，但 **0.9.4 的 `icon/basic` 只有 `ArrowRight / ArrowUpDown / Check / Close / Search / SearchCleanup / Sidebar`，没有 `Back`**；`MiuixIcons` 对象定义在 `miuix-core`（`object MiuixIcons { object Basic; Light; Normal; Regular; Medium; Demibold }`），图标是扩展属性 `val MiuixIcons.Basic.Search`。
- HEAD 的 README 还宣传 `miuix-glass`、Kotlin 2.4.21 / CMP 1.12.2，这些都是 **0.9.4 之后**的东西。
- **建议**：以 `-sources.jar` / Dokka 为准；文档 URL 固定到 tag（`.../blob/v0.9.4/docs/...`）。

### 5.8 【低】无需 Android 资源 / 字体 / proguard 规则
- **实证 E14**：AAR 只有 `classes.jar`(1.9MB) + `baseline-prof.txt`(530KB) + `R.txt` + Manifest + `aar-metadata`。**没有 res/、没有字体、没有 `proguard.txt`/consumer rules、没有 jni**。
- 含义：①不会和 SPRoot 的资源/字体冲突；②文字全部走 `TextStyles` 的默认 `FontFamily`（系统字体），想要 MiSans 之类需自己 `TextStyles.copy(...)` 替换；③`baseline-prof.txt` 会被 AGP 自动合并，无需手工配置。
- **R8（SPRoot release 开了 minify）**：库未提供 keep 规则，理论上 Compose 库不需要；但 **release 包必须实测一次**（推断风险点，重点看首次进入 Miuix 页面是否崩、Popup 是否正常）。

### 5.9 【低】所有 Overlay/Dialog/Popup 必须在 `Scaffold` 内
- **实证 E12/E19**：`Scaffold` 的默认 `popupHost = { MiuixPopupHost() }`，`OverlayDialog`/`OverlayListPopup`/`OverlayDropdownPreference` 等依赖 Scaffold 提供的 `LocalPopupStates`/`LocalDialogStates`。脱离 Scaffold 使用这些组件 = 弹不出来（或运行期异常）。
- 多 Scaffold 嵌套是安全的（每个 Scaffold 管自己的 popup 状态）。

### 5.10 【低】实验性 API + 版本漂移
- **实证 E20**：README/docs 首行警告 “This library is experimental. APIs may change without notice.”。
- **建议**：锁死 `0.9.4`；升级前 diff `-sources.jar`；不要用 `+`/SNAPSHOT（Maven Central 上根本没有 SNAPSHOT，只有 `-rc01` 这类发布件）。

### 5.11 【低】`ThemeController` 是 `@Stable` 可观察状态，务必 `remember`
- **实证 E15**：`ThemeController` 的属性是 `mutableStateOf`，构造参数 `colorSchemeMode/lightColors/darkColors/keyColor/colorSpec/paletteStyle/isDark`。
- 每次重组 new 一个 controller 会导致整棵 Miuix 子树重新计算配色（性能问题，不崩）。用 `remember { ... }`（README 示例也是这么写的）。

### 5.12 落地后的验证命令（本机缺 Gradle/SDK，需在有环境处执行）

```bash
# 1) 依赖图：确认 material3 没被抬到 alpha
./gradlew :app:dependencies --configuration releaseRuntimeClasspath | grep -i material3

# 2) AAR 元数据要求 vs 项目配置
./gradlew :app:checkDebugAarMetadata

# 3) 编译 + 资源合并（minSdk / manifest 冲突会在这里暴露）
./gradlew :app:assembleDebug

# 4) release（R8）实测一遍
./gradlew :app:assembleRelease
```

---

## 6. 证据

### E0 本机环境限制（为何是「元数据级实证 + 推断」而非「编译通过」）
```text
$ gradle -v            →  no gradle on PATH
$ $env:ANDROID_HOME    →  (空)
$ Test-Path $env:LOCALAPPDATA\Android\Sdk  →  False
$ Get-ChildItem ~/.gradle/wrapper/dists    →  (空)
```
因此本报告**没有**执行真实 Gradle 解析/编译；所有「会被抬版本 / 会构建失败」的结论均标【推断】，并在 §5.12 给出可执行复验命令。

### E1 Maven Central 元数据（最新版本）
```text
$ curl -sS https://repo1.maven.org/maven2/top/yukonga/miuix/kmp/miuix-ui/maven-metadata.xml
<groupId>top.yukonga.miuix.kmp</groupId><artifactId>miuix-ui</artifactId>
<latest>0.9.4</latest><release>0.9.4</release>
<versions>0.9.0 0.9.1 0.9.2 0.9.3 0.9.4-rc01 0.9.4</versions>
<lastUpdated>20260920082405</lastUpdated>
```

### E2 GitHub Releases
```text
$ curl -sS -H "Accept: application/vnd.github+json" \
    "https://api.github.com/repos/compose-miuix-ui/miuix/releases?per_page=5"
tag           | name          | published_at          | prerelease
v0.9.4        | v0.9.4        | 2026-09-20T08:33:49Z  | False
v0.9.4-rc01   | v0.9.4-rc01   | 2026-08-13T15:57:44Z  | True
v0.9.3        | v0.9.3        | 2026-07-04T08:18:13Z  | False
v0.9.2        | v0.9.2        | 2026-06-05T11:13:48Z  | False
v0.9.1        | v0.9.1        | 2026-05-14T14:56:22Z  | False
```
仓库 tag 列表（`git ls-remote --tags`）与之一致，最新稳定 tag = `v0.9.4`。

### E3 AAR 元数据与 Manifest（miuix-ui-android:0.9.4）
```text
$ curl -O .../miuix-ui-android/0.9.4/miuix-ui-android-0.9.4.aar   # 1,902,787 bytes
$ unzip -p miuix-ui-android-0.9.4.aar META-INF/com/android/build/gradle/aar-metadata.properties
aarFormatVersion=1.0
aarMetadataVersion=1.0
minCompileSdk=37
minCompileMinorSdk=0
minCompileSdkExtension=0
minAndroidGradlePluginVersion=1.0.0
coreLibraryDesugaringEnabled=false

$ unzip -p miuix-ui-android-0.9.4.aar AndroidManifest.xml
<manifest package="top.yukonga.miuix.kmp.ui">
    <uses-sdk android:minSdkVersion="24" />
</manifest>
```

### E4 SPRoot 现状（`APatch/`）
- `APatch/gradle/libs.versions.toml`：`agp = "9.3.1"`, `kotlin = "2.4.10"`, `ksp = "2.3.11"`, `compose-bom = "2026.08.00"`, `lifecycle = "2.11.0"`, `coil3 = "3.5.0"`, `libsu = "6.0.0"`
- `APatch/build.gradle.kts`：`androidMinSdkVersion = 26`, `androidTargetSdkVersion = 36`, `androidCompileSdkVersion = 37`, `androidBuildToolsVersion = "36.1.0"`, `androidCompileNdkVersion = "29.0.14206865"`
- `APatch/app/app/build.gradle.kts`：`compileOptions`/`kotlin.jvmToolchain` 均为 21；`release.isMinifyEnabled = true`
- `APatch/gradle/wrapper/gradle-wrapper.properties`：`gradle-9.7.0-bin.zip`
- `APatch/settings.gradle.kts`：`google()` + `mavenCentral()` + `jitpack.io`，`RepositoriesMode.FAIL_ON_PROJECT_REPOS`

### E5 miuix-ui 0.9.4 的 Gradle Module Metadata
```text
$ curl -sS .../miuix-ui/0.9.4/miuix-ui-0.9.4.module      # component/createdBy: gradle 9.7.1
metadataApiElements deps:
  top.yukonga.miuix.kmp:miuix-core:0.9.4
  top.yukonga.miuix.kmp:miuix-squircle:0.9.4
  org.jetbrains.compose.foundation:foundation:1.12.0
  org.jetbrains.kotlin:kotlin-stdlib:2.4.20
  androidx.navigationevent:navigationevent-compose:1.1.2
  org.jetbrains.compose.material3:material3-window-size-class:1.12.0-alpha03
  com.materialkolor:material-color-utilities:5.0.1

$ curl -sS .../miuix-ui-android/0.9.4/miuix-ui-android-0.9.4.module
androidApiElements-published (aar):
  miuix-core 0.9.4 | miuix-squircle 0.9.4 | org.jetbrains.compose.foundation:foundation:1.12.0 | kotlin-stdlib:2.4.20
androidRuntimeElements-published (aar):
  上面 4 条 + androidx.navigationevent:navigationevent-compose:1.1.2
           + org.jetbrains.compose.material3:material3-window-size-class:1.12.0-alpha03
           + com.materialkolor:material-color-utilities:5.0.1
  （注意：窗口尺寸类只在 runtime 变体，compile 变体没有）
```

### E6 Compose 版本对齐（两条独立证据）
```text
$ curl -sS .../org/jetbrains/compose/foundation/foundation-android/1.12.0/foundation-android-1.12.0.module
androidRuntimeElements-published:
  androidx.compose.foundation:foundation:1.12.0     ← 关键：与 SPRoot BOM 完全一致
  org.jetbrains.compose.runtime:runtime:1.12.0
  org.jetbrains.compose.ui:ui / ui-text / ui-util:1.12.0
  org.jetbrains.compose.foundation:foundation-layout:1.12.0
  org.jetbrains.compose.animation:animation:1.12.0
  androidx.annotation:annotation:1.8.1 / annotation-experimental:1.4.1
  androidx.collection:collection:1.5.0 / androidx.core:core:1.13.1 / androidx.emoji2:emoji2:1.3.0

$ curl -sS https://dl.google.com/dl/android/maven2/androidx/compose/compose-bom/2026.08.00/compose-bom-2026.08.00.pom
  androidx.compose.foundation:foundation  1.12.0
  androidx.compose.runtime:runtime        1.12.0
  androidx.compose.ui:ui                  1.12.0
  androidx.compose.material3:material3    1.4.0     ← 与 miuix 的传递约束冲突点
```

### E7 Material3 被抬升的完整链路
```text
$ curl -sS .../androidx/compose/material3/material3-window-size-class-android/1.5.0-alpha22/...module
androidApiElements-published:
  dep androidx.annotation:annotation-experimental:1.4.1
  dep androidx.compose.runtime:runtime:1.8.1
  dep androidx.compose.ui:ui:1.8.2 / ui-unit:1.8.1
  dep org.jetbrains.kotlin:kotlin-stdlib:2.1.20
  con androidx.compose.material3:material3:1.5.0-alpha22                     ← 版本约束
  con androidx.compose.material3:material3-adaptive-navigation-suite:1.5.0-alpha22
  con androidx.compose.material3:material3-lint:1.5.0-alpha22
androidRuntimeElements-published: 同上 + dep androidx.window:window:1.0.0 + ui-util:1.8.1

$ curl -sS .../org/jetbrains/compose/material3/material3-window-size-class-android/1.12.0-alpha03/...module
androidRuntimeElements-published:
  dep androidx.compose.material3:material3-window-size-class:1.5.0-alpha22   ← 重定向到上面那个
  dep androidx.window:window:1.0.0
  con org.jetbrains.compose.material3:material3:1.12.0-alpha03
```
约束 `material3 → 1.5.0-alpha22` 用的是 `version.requires`（非 `strictly`）【推断：Gradle 取最高版本，因此 1.4.0 会被抬到 1.5.0-alpha22】。

### E8 miuix 源码对 `WindowSizeClass` 零引用（exclude 的安全性依据）
```text
$ cd /tmp/miuixwork/repo && grep -rn "calculateWindowSizeClass\|WindowSizeClass\|material3.window" --include=*.kt .
（无输出）

$ cd /tmp/uisrc && grep -rin "windowsizeclass\|androidx.window" .
（无输出）   # 0.9.4 官方 sources jar，87 个 .kt
```

### E9 blur 模块 minSdk=33
```text
$ curl -O .../miuix-blur-android/0.9.4/miuix-blur-android-0.9.4.aar   # 185,342 bytes
$ unzip -p miuix-blur-android-0.9.4.aar AndroidManifest.xml
<manifest package="top.yukonga.miuix.kmp.blur">
    <uses-sdk android:minSdkVersion="33" />
</manifest>
$ unzip -p miuix-blur-android-0.9.4.aar META-INF/com/android/build/gradle/aar-metadata.properties
minCompileSdk=37 / minAndroidGradlePluginVersion=1.0.0
```

### E10 各模块在 0.9.4 的发布情况（HTTP 状态码）
```text
miuix-preference-android 0.9.4 -> 200
miuix-icons-android      0.9.4 -> 200
miuix-squircle-android   0.9.4 -> 200
miuix-nav-android        0.9.4 -> 200
miuix-core-android       0.9.4 -> 200
miuix-shader-android     0.9.4 -> 200
miuix-blur-android       0.9.4 -> 200（但 minSdk 33）
miuix-glass-icons-android 0.9.4 -> 404
miuix-glass-android       0.9.4 -> 404
```
（命令：`curl -o /dev/null -w "%{http_code}" .../miuix-<module>-android/0.9.4/....pom`）

### E11 squircle 在低 API 自动降级
`miuix-shader/src/androidMain/kotlin/top/yukonga/miuix/kmp/shader/RuntimeShader.android.kt`（HEAD，与 0.9.4 行为一致）：
```kotlin
actual fun isRuntimeShaderSupported(): Boolean = Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU
actual fun isRenderEffectSupported(): Boolean = Build.VERSION.SDK_INT >= Build.VERSION_CODES.S
```
`miuix-squircle/src/commonMain/.../SquircleBackground.kt:370`：`if (!LocalSquircleEnabled.current || !isRuntimeShaderSupported()) return null`

### E12 官方 getting-started @ v0.9.4
```text
$ curl -sSL https://raw.githubusercontent.com/compose-miuix-ui/miuix/v0.9.4/docs/guide/getting-started.md
Android Compose 项目：
  implementation("top.yukonga.miuix.kmp:miuix-ui-android:<version>")
  ... -preference-android / -icons-android / -blur-android(minSdk 33) / -squircle-android / -nav-android
仓库只需 mavenCentral()
警告：Scaffold 是 Overlay* 组件的宿主
版本号由 Maven Central badge 决定（= 0.9.4）
```

### E13 构建侧常量（HEAD，仅用于理解仓库，不作为 0.9.4 发布参数）
```text
build-logic/src/main/kotlin/BuildConfig.kt:
  LIBRARY_VERSION = "0.9.4"  LIBRARY_ID = "top.yukonga.miuix.kmp"
  COMPILE_SDK = 37  COMPILE_SDK_MINOR = 0  TARGET_SDK = 37  MIN_SDK = 24
  BUILD_TOOLS_VERSION = "37.0.0"  JDK_VERSION = 21
gradle/libs.versions.toml (HEAD): agp = "9.5.1"  kotlin = "2.4.21"  compose-multiplatform = "1.12.2"
gradle/wrapper (HEAD): gradle-9.9.0-bin.zip
```

### E14 AAR 内容清单（无资源/无字体/无 consumer proguard）
```text
$ unzip -l miuix-ui-android-0.9.4.aar
        0  R.txt
      206  AndroidManifest.xml
  1956683  classes.jar
   529963  baseline-prof.txt
      178  META-INF/com/android/build/gradle/aar-metadata.properties
（共 5 个文件，无 res/、无 assets/、无 proguard.txt、无 jni/）
```

### E15 0.9.4 源码签名核对（最小示例的依据）
`miuix-ui-android-0.9.4-sources.jar`（87 个 .kt）逐条核对：
```kotlin
// basic/Scaffold.kt:79
fun Scaffold(modifier, topBar = {}, bottomBar = {}, floatingActionButton = {},
    floatingActionButtonPosition = FabPosition.End, floatingToolbar = {},
    floatingToolbarPosition = ToolbarPosition.BottomCenter, snackbarHost = {},
    popupHost = { MiuixPopupHost() }, containerColor = MiuixTheme.colorScheme.surface,
    contentWindowInsets = WindowInsets.systemBars.union(WindowInsets.displayCutout),
    content: @Composable (PaddingValues) -> Unit)

// basic/TopAppBar.kt:173
fun SmallTopAppBar(title: String, modifier, color, titleColor, subtitle: String = "",
    subtitleColor, navigationIcon = {}, actions: @Composable RowScope.() -> Unit = {},
    scrollBehavior: ScrollBehavior? = null, defaultWindowInsetsPadding = true,
    titlePadding, navigationIconPadding, actionIconPadding, bottomContent = {})

// basic/Text.kt:90  —— 注意首参名是 `text`，不是 `value`
fun Text(text: String, modifier, color, autoSize, fontSize, fontStyle, fontWeight,
    fontFamily, letterSpacing, textDecoration, textAlign, lineHeight, overflow,
    softWrap, maxLines, minLines, onTextLayout, style: TextStyle = LocalTextStyles.current.main)

// basic/Button.kt:50
fun Button(onClick: () -> Unit, modifier, enabled, cornerRadius, minWidth, minHeight,
    colors: ButtonColors = ButtonDefaults.buttonColors(), insideMargin,
    interactionSource, indication, content: @Composable RowScope.() -> Unit)

// basic/Card.kt:50
fun Card(modifier, cornerRadius, insideMargin: PaddingValues = CardDefaults.InsideMargin,
    colors = CardDefaults.defaultColors(), content: @Composable ColumnScope.() -> Unit)
// basic/Card.kt:87 交互版：onClick / onLongPress / pressFeedbackType / showIndication / holdDownState

// theme/MiuixTheme.kt:24 / :54
fun MiuixTheme(controller: ThemeController, textStyles = MiuixTheme.textStyles, content)
fun MiuixTheme(colors: Colors = MiuixTheme.colorScheme, textStyles, content)

// theme/ThemeController.kt:217
class ThemeController(colorSchemeMode = ColorSchemeMode.System,
    lightColors = lightColorScheme(), darkColors = darkColorScheme(), keyColor: Color? = null,
    colorSpec = ThemeColorSpec.Spec2021, paletteStyle = ThemePaletteStyle.TonalSpot,
    isDark: Boolean? = null)

// miuix-core 的 icon/MiuixIcons.kt（0.9.4 sources）
object MiuixIcons { object Basic; Light; Normal; Regular; Medium; Demibold }
```
0.9.4 的 `icon/basic` 仅：`ArrowRight, ArrowUpDown, Check, Close, Search, SearchCleanup, Sidebar`（无 `Back`）。

### E16 MiuixTheme 的 CompositionLocal 副作用
`miuix-ui/src/commonMain/kotlin/top/yukonga/miuix/kmp/theme/MiuixTheme.kt`（0.9.4）：
```kotlin
CompositionLocalProvider(
    LocalColors provides miuixColors,
    LocalTextStyles provides miuixTextStyles,
    LocalIndication provides miuixIndication,          // ← 覆盖涟漪/按压反馈
    LocalContentColor provides miuixColors.onBackground, // ← 覆盖内容色
    LocalOverscrollFactory provides MiuixOverscrollFactory, // ← 覆盖过度滚动
) { content() }
```
`theme/Colors.kt:618`: `internal val LocalColors = staticCompositionLocalOf { lightColorScheme() }`
`theme/TextStyles.kt:250`: `internal val LocalTextStyles = staticCompositionLocalOf { defaultTextStyles() }`
`theme/ContentColor.kt:21`: `val LocalContentColor = compositionLocalOf { Color.Black }`

### E17 组件同名清单
`miuix-ui-0.9.4-sources.jar` → `commonMain/top/yukonga/miuix/kmp/basic/`：
`Badge, BreadcrumbBar, Button, Card, Checkbox, ColorPalette, ColorPicker, Component, Divider, Dropdown, FloatingActionButton, FloatingToolbar, Icon, IconButton, ListPopup, NavigationBar, NavigationRail, NumberPicker, ProgressIndicator, PullToRefresh, RadioButton, Scaffold, ScrollBar, SearchBar, Slider, SmallTitle, Snackbar, Surface, Switch, TabRow, Text, TextField, Tooltip, TopAppBar`
→ 与 Material3 重名的至少有：`Button, Card, Checkbox, Divider, Icon, IconButton, Scaffold, Slider, Surface, Switch, Text`。

### E18 文档/API 漂移实例
```text
# HEAD 文档（docs/components/basic/topappbar.md）示例：
import top.yukonga.miuix.kmp.icon.MiuixIcons
IconButton(onClick = {}) { Icon(MiuixIcons.Back, contentDescription = "Back") }

# 0.9.4 实际：
ls /tmp/uisrc/commonMain/top/yukonga/miuix/kmp/icon/basic/
ArrowRight.kt ArrowUpDown.kt Check.kt Close.kt Search.kt SearchCleanup.kt Sidebar.kt
（没有 Back.kt；MiuixIcons 没有 Back 扩展属性）
```

### E19 Scaffold 是 popup 宿主
`basic/Scaffold.kt:88`：`popupHost: @Composable () -> Unit = { MiuixPopupHost() }`；
`Scaffold.kt:107-108`：`LocalPopupStates provides popupStates, LocalDialogStates provides dialogStates`。

### E20 实验性声明
README（HEAD）原文：
> This library is experimental. APIs may change without notice.
`docs/guide/getting-started.md` @ v0.9.4 首段同样有 `::: warning This library is experimental...`。

### E21 历史/停更坐标的版本区间
```text
$ curl -sS .../top/yukonga/miuix/kmp/miuix/maven-metadata.xml              → ... 0.8.5 0.8.6 0.8.7 0.8.8
$ curl -sS .../top/yukonga/miuix/kmp/miuix-shapes/maven-metadata.xml       → <latest>0.9.0</latest>（仅 0.9.0）
$ curl -sS .../top/yukonga/miuix/kmp/miuix-navigation3-adaptive/maven-metadata.xml → ... 0.8.2 0.8.3 0.8.4 0.8.5
$ curl -sS .../top/yukonga/miuix/kmp/miuix-ui/maven-metadata.xml           → 0.9.0 … 0.9.4（当前主线）
```

### 附：仓库结构（HEAD，`git clone --depth 1` 到 `/tmp/miuixwork/repo`）
```
miuix-core/  miuix-ui/  miuix-preference/  miuix-icons/  miuix-glass-icons/
miuix-shader/  miuix-blur/  miuix-squircle/  miuix-glass/  miuix-nav/  miuix-window/
example/{shared,android,desktop,desktop-nucleus,web,macos}  docs/  baselineprofile/  build-logic/
```
`settings.gradle.kts` 中 `rootProject.name = "compose-miuix-ui"`；HEAD commit `b59ed57`（2026-11-08）。

各模块在 0.9.4 的可消费情况见 E10（`miuix-glass` / `miuix-glass-icons` 未发布）。
Maven Central 的 group 目录里还有若干**历史坐标**，不要用（已改名或已停更）【实证 E21】：

| 目录 | 已发布版本 | 说明 |
| --- | --- | --- |
| `miuix` | …0.8.8 | 0.9.0 之前的旧坐标，已被 `miuix-ui` 取代 |
| `miuix-navigation3-adaptive` / `miuix-navigation3-ui` | …0.8.5 | 0.8.5 后停更 |
| `miuix-shapes` | 仅 0.9.0 | 【推断】0.9.0 之后由 `miuix-squircle` 承接 |
