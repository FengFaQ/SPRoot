# SPRoot 新管理器客户端方案（SPROOT-NEW-DOC-04）

- 文档编号：NEW-DOC-04
- 作者：客户端（app-client）
- 状态：v0.1（随 task-4/task-5 代码同步）；契约依据：《SKRoot↔AP 调用契约》v1.1-FROZEN（§8）、《SPROOT-DOC-C1 修补引擎 IO 契约》v1.0
- 工程目录：仓库根 `app/`（Gradle 工程根即仓库根；`./gradlew :app:assembleRelease`）
- 交付形态：**只由 GitHub Actions 编译产出 APK**（人类硬规则：禁止本地编译）

---

## 0. 一句话结论

新管理器是「**薄 UI + 厚 CLI/JNI**」的 Kotlin + Compose + Miuix 应用：修补链（选镜像 → magiskboot 解包 → JNI 引擎修补 → 重打包 → 校验 → 导出）**全程在 App 进程内、无需 root**；特权链（部署/模块/授权/刷入）**只经 AP 的 `root-exec` 一条入口**。两条链在代码上分属 `PatchPipeline` 与 `ApdClient`，不混用。

---

## 1. 技术选型与版本矩阵（均有实证来源，非臆造）

| 维度 | 选型 | 依据 |
|---|---|---|
| UI 框架 | Miuix（Compose Multiplatform UI） | 用户指定 `compose-miuix-ui/miuix` |
| Miuix 坐标/版本 | `top.yukonga.miuix.kmp:miuix-ui-android:0.9.4`、`miuix-icons-android:0.9.4` | Maven Central `maven-metadata.xml`：latest/release = 0.9.4（2026-09-20）；只走 Maven Central，无 JitPack |
| compileSdk | **37**（minor 0） | `miuix-ui-android-0.9.4.aar` 的 `aar-metadata.properties`：`minCompileSdk=37`，低于 37 直接构建失败 |
| build-tools | 37.0.0 | 与 compileSdk 同代 |
| minSdk / targetSdk | 26 / 37 | 库自身 minSdk=24；26 为本项目下限（image/SAF/Keystore 能力充足） |
| AGP | 9.4.1 | miuix v0.9.4 仓库 `gradle/libs.versions.toml` |
| Kotlin / compose 编译器插件 | 2.4.20（同 ref） | miuix 0.9.4 以 Kotlin 2.4.20 编译 → 消费者必须 ≥2.4.x（Kotlin 元数据不向前兼容） |
| Compose | androidx Compose BOM `2026.08.00`（=1.12.0） | miuix 0.9.4 依赖 `org.jetbrains.compose.foundation:foundation:1.12.0` → Android 侧 `androidx.compose.foundation:foundation:1.12.0`；BOM 恰好钉同一版本 |
| Gradle | 9.7.1（wrapper 已提交） | miuix 0.9.4 的 wrapper 版本 |
| JDK | 21（CI `setup-java`） | miuix `jvmToolchain(21)`；AGP 9 要求 |
| material3 冲突处理 | `exclude(group="org.jetbrains.compose.material3", module="material3-window-size-class")` | miuix runtime 变体把 material3 约束抬到 1.5.0-alpha22，会造成编译期 1.4.0 / 运行期 alpha 不一致；miuix 源码对 `WindowSizeClass` 零引用（见 `docs/research/miuix-integration.md` §1.3/E8） |
| 未使用 | miuix-preference / blur / glass / glass-icons / squircle | blur 需 minSdk 33；glass 系列 0.9.4 未发布；preference 当前手写卡片足够 |
| 导航 | 不用 navigation-compose | 仅 4 个平级页，状态驱动即可，少一个依赖 = CI 少一类失败面 |

### 1.1 AGP 9 的两个必填开关（CI 实证踩坑）

```properties
# 内置 Kotlin 用的是 AGP 自带 KGP 2.2.10，与 miuix 的 2.4.20 元数据不兼容 → 关闭内置，改用经典 kotlin-android 插件
android.builtInKotlin=false
# AGP 9 默认 newDsl=true 与 KGP 2.4.20 校验冲突（CI run 42485200777：ApplicationExtensionImpl cannot be cast to BaseExtension）
android.newDsl=false
```

> 这两行是**本工程能编译的前提**；`gradle.properties` 中是硬约束，不是可选优化。

---

## 2. 工程结构

```
仓库根/
├── app/                                   # Android 应用模块
│   ├── build.gradle.kts                   # AGP9 DSL：compileSdk{version=release(37)}、签名、packaging
│   └── src/main/
│       ├── AndroidManifest.xml            # 无网络权限；extractNativeLibs 由 packaging 决定
│       ├── jniLibs/arm64-v8a/README.md    # 必需的原生资产清单与约束
│       ├── res/{values,values-night,drawable}
│       └── java/com/sproot/manager/
│           ├── MainActivity.kt / SPRootApp.kt
│           ├── core/                      # 逻辑层（无 ViewModel 依赖，纯状态 + 线程）
│           │   ├── Models.kt              # PatchStage/PatchUiState/PatchMode/CapabilityItem/EngineExitCode
│           │   ├── AppLog.kt              # 日志：内存 State + 同步落盘
│           │   ├── PatchPipeline.kt       # 修补链编排（无 root）
│           │   ├── PatchEngine.kt         # JNI 引擎封装 + 能力自检
│           │   ├── MagiskBoot.kt          # magiskboot 执行封装（ProcessBuilder 数组）
│           │   ├── Capabilities.kt        # 入口能力自检
│           │   ├── Elf.kt / MiniJson.kt   # ELF/压缩魔数探测；NDJSON 扁平解析
│           │   ├── Exporter.kt            # 产物导出 Downloads/SPRoot/
│           │   ├── BackupStore.kt         # sha1 内容寻址备份/还原
│           │   ├── InstallState.kt        # 6 态安装状态机（持久化）
│           │   ├── SecretStore.kt         # 48 位密钥：Keystore(AES-GCM) 加密存储
│           │   └── ApdClient.kt           # 特权链唯一入口（root-exec）
│           └── ui/
│               ├── AppRoot.kt             # Scaffold + 底栏 4 页
│               ├── AppTheme.kt            # MiuixTheme + ThemeController
│               ├── components/Widgets.kt  # SectionCard/InfoRow/LogPanel/剪贴板等
│               └── pages/{Home,Patch,Modules,Settings}Page.kt
├── keystore/                              # 方案 A：私仓内签名材料（CI 直读）
└── .github/workflows/apk.yml              # CI：编译 + 签名 + apkigner 校验 + artifact
```

依赖最小心集：`miuix-ui`、`miuix-icons`、Compose BOM(runtime/foundation/ui)、`activity-compose`。**不引入** Hilt/Room/Navigation/OkHttp/Coil。

---

## 3. 页面骨架与交互

底部导航四页（Miuix `NavigationBar`），顶栏 `SmallTopAppBar`；主题 `MiuixTheme(ThemeController)`，设置页可切「跟随系统/浅色/深色」。

### 3.1 主页（Home）
1. **安装状态卡**（来自 `InstallState`）：`未修补 / 修补中 / 已修补·待刷入 / 已刷入·待重启 / 已激活 / 错误` + 产物路径 + sha256 + 最近更新时间 + 备注。
2. **SPRoot 状态卡**：修补引擎状态（是否加载 + 版本）、AP 守护（路径/架构/是否部署）、Root 密钥（脱敏展示，只显示前 4+后 4）。
3. **设备信息卡**：机型/Android/ABI/内核/App 版本。
4. **环境自检卡**：手动触发 `Capabilities.collect()`，逐条 ✓/✗ + 详情，阻断项标红。
5. **下一步卡**：引导到修补页。

### 3.2 修补页（Patch）——本项目的核心
自上而下：
1. **选择镜像**：SAF `OpenDocument("*/*")` + `takePersistableUriPermission`；展示文件名/URI。
2. **修补模式**（PatchMode）：`仅导出` / `导出并给出刷入指引`；`从备份还原`、`卸载还原` 以 `enabled=false` 占位（避免"点了必然失败"）。
3. **进度**：`阶段标题 + 阶段说明 + 百分比 + LinearProgressIndicator`；引擎进度来自 NDJSON，占 35%→72% 区间。
4. **错误区（常驻）**：`errorTitle/errorDetail` 一经写入不自动消失，仅"重置"清除；提供「展开/收起详情」「复制全部（含日志）」「日志文件路径」。
5. **结果区**：产物路径、大小、sha256、**内核等长校验**、引擎报告、备份 sha1、Root 密钥（脱敏/显示/复制）。
6. **刷入指引卡**（`EXPORT_AND_GUIDE` 模式）：魅族 21 为 A/B 双槽，命令模板 `fastboot flash --slot=all boot <file>`；**AVB 风险提示位**（`--disable-verity --disable-verification flash vbmeta` 模板，标注"待 patcher 结论"）。
7. **两级状态**：「我已刷入（标记待重启）」→「已重启并生效（标记激活）」，状态由用户手动回报（无 root 时 App 无法读取 fastboot 结果）。
8. **常驻日志面板**：级别配色、自动滚底、复制全部。

### 3.3 模块页（Modules）
- 顶部**能力门禁**：AP 未就绪时展示不可用原因并禁用所有入口。
- `apd version --json` 探测（经 root-exec）；模块列表为空态 + 后续接入说明；`root-exec` 探针按钮用于快速验证特权链。

### 3.4 设置页（Settings）
外观（主题）、Root 密钥（显示/复制/重新生成 + 旧内核失效警告）、日志（路径/复制/清空）、**原镜像备份**（sha1 列表 / 还原到 Downloads / 删除）、AP 部署（`apd deploy --source <nativeLibDir>`，显示 exit code 与完整输出）、关于（版本/工具链/契约版本）。

---

## 4. 修补调用链（task-5 核心）

### 4.1 阶段序列

```
0 环境自检    ABI=arm64 / 引擎 libskrootpatch.so(ELF A64+可加载) / magiskboot(ELF A64+可执行)
              / 存储≥3GB / 密钥可读写 / 输入 ANDROID! 魔数
1 准备镜像    contentResolver 复制 → cacheDir/patch/job-<ts>/boot.img；sha256 记录
2 备份        filesDir/backups/<sha1>.img（内容寻址，同镜像复用）+ index.tsv
3 解包        magiskboot unpack boot.img   (cwd=工作目录)
4 解压内核    若 kernel 头是 gzip/lz4/zstd/xz/bzip2/lzop → magiskboot decompress kernel kernel.raw
5 修补        PatchEngine.patch(kernel.raw → kernel.patched, key48, json=progress.ndjson, workdir)
6 回压        若步骤4做过解压 → magiskboot compress=<fmt> kernel.patched kernel；否则直接覆盖 kernel
7 重打包      magiskboot repack boot.img → new-boot.img
8 校验导出    产物存在/非空 → 修补内核与输入内核**长度相等** → sha256 变化 → 导出 Downloads/SPRoot/
9 状态        失败 → InstallState.ERROR；成功 → PATCHED_PENDING_FLASH
```

全程**无 root、无 shell、无网络**。任何一步失败：错误详情 + 完整输出 + 工作目录（保留供诊断）+ 日志文件路径，全部常驻展示。

### 4.2 JNI 绑定契约（与 C1 CLI 语义一一对应）

`libskrootpatch.so` 必须导出 4 个静态 native 函数（详见 `core/PatchEngine.kt` 注释）：

```c
JNIEXPORT jstring JNICALL Java_com_sproot_manager_core_PatchEngine_nativeVersion(JNIEnv*, jclass);
JNIEXPORT jint    JNICALL Java_com_sproot_manager_core_PatchEngine_nativePatch(
        JNIEnv*, jclass, jstring inPath, jstring outPath, jstring key, jboolean genKey,
        jstring jsonPath, jstring workDir, jboolean verbose);
JNIEXPORT jstring JNICALL Java_com_sproot_manager_core_PatchEngine_nativeLastError(JNIEnv*, jclass);
JNIEXPORT jstring JNICALL Java_com_sproot_manager_core_PatchEngine_nativeLastRootKey(JNIEnv*, jclass);
```

- 参数 ↔ `--in/--out/--key|--gen-key/--json/--workdir/--verbose`；
- 返回值 = C1 §3 退出码枚举（0/101/102/201/202/210/211/220/230/240/250），Kotlin 侧逐码映射为中文可读文案；
- **进度只走 `jsonPath` 的 NDJSON 文件轮询**（150ms），不依赖 stdout（JNI 进程内无法可靠抓 stdout）；
- 失败时引擎**不得**留下 out 文件（C1 §3 铁律），Kotlin 侧仍二次校验"有产物 + 等长 + sha 变化"。

### 4.3 为什么 magiskboot 走 nativeLibraryDir

- `libmagiskboot.so` 随 APK 分发（`jniLibs/arm64-v8a/`），`packaging.jniLibs.useLegacyPackaging = true` 保证解压到 `nativeLibraryDir`（真实文件、可 exec）；
- **不在 filesDir exec ELF**（旧版缺陷 ⑨；Android 10+ noexec + SELinux W^X）；
- 调用方式是 `ProcessBuilder(listOf(bin, "unpack", "boot.img"))`，**不经 `sh -c`、不拼字符串**（缺陷 ⑩）；
- stdout+stderr 合并逐行回调并实时落盘；超时强杀并明确报 timeout；`--version/--help` 用于能力探测。

### 4.4 产物导出与刷入

- 导出路径：`Downloads/SPRoot/boot_patched_<yyyyMMdd-HHmmss>.img`
  - Android 10+：`MediaStore.Downloads` + `RELATIVE_PATH=Download/SPRoot`（无需存储权限）
  - Android 9-：公共 Downloads 目录（`WRITE_EXTERNAL_STORAGE` maxSdkVersion=28）
- 导出后 UI 展示：大小、sha256、内核等长校验结果、fastboot 命令（A/B 双槽 `--slot=all`）、AVB 风险提示位。
- 「导出成功」与「刷入成功」是**两级状态**：刷入由用户手动确认后状态机才前进。

---

## 5. 特权链（App ↔ AP，契约 v1.1 §8）

唯一入口 `ApdClient.rootExec()`，只做三件事：

1. 把 48 位密钥写入 `filesDir/exec-keys/key-< nanoTime >.tmp`（0600），**argv 只传路径**；
2. `apd root-exec --superkey-file <tmp> [--uid N] [--scontext CTX] [--timeout SEC] -- <cmd argv...>`；
3. **只按 exit code 判成败**；调用返回后立即删除临时文件。

| exit | 语义 | UI 动作 |
|---|---|---|
| 0 | 成功 | 继续 |
| 1 | 一般失败 | 展示 stderr 摘要 + 日志落盘 |
| 2 | 用法错误 | 视为 bug，展示原始 argv |
| 124 | 超时 | 提示用户，保留日志 |
| 125 | 提权失败/未授权（静默） | **「密钥无效或需重新修补」，不自动重试** |
| 126/127 | 不可执行/命令不存在 | 提示需重新部署 AP |

并发：`ReentrantLock(fair=true)` 串行化全部 root 操作（§8.1-4）。日志与 UI 均**不含密钥原文/前缀**（argv 只含临时文件路径）。

---

## 6. 错误与日志设计（针对旧版"一闪即退"）

| 旧版问题 | 新方案 |
|---|---|
| 失败即卸载日志面板（Patches.kt:480/565） | 错误区**常驻**于修补页，只有"重置"清除 |
| 真实 stderr 被注释/丢弃 | magiskboot 输出逐行回调 → 日志 + 错误详情原文 |
| 只在内存里 | 每行同步 flush 到 `filesDir/logs/session-<ts>.log`，进程被杀也不丢 |
| 无法上报 | 「复制全部」= 错误标题+详情+内存日志+日志文件路径 |
| 无阶段信息 | 每阶段开始/结束 + **耗时汇总**（结构化：阶段+耗时+结果） |
| 静默跳过缺失依赖 | 入口能力自检把缺失列为**阻断项**并给出绝对路径与修复建议 |

错误码分层：`PatchEngine`（C1 枚举）与 `ApdClient`（§8.5 枚举）各自维护稳定映射，未知码原样带出，绝不吞。

---

## 7. CI（GitHub Actions，唯一编译通道）

`.github/workflows/apk.yml`（v2 分支）：
1. `actions/checkout` + `setup-java 21` + `gradle/actions/setup-gradle`；
2. 安装 `platforms;android-37` + `build-tools;37.0.0`；
3. **（非阻断）从 CI artifact 拉取 `libskrootpatch.so` / `libapd.so` 入 `app/src/main/jniLibs/arm64-v8a/`**：artifact 未就绪时 SKIP，不影响构建；就绪后自动进 APK；
4. `./gradlew :app:assembleRelease`；
5. `apksigner verify --print-certs` 断言 `CN=FengFa`（APK Signature Scheme v3 已验证）；
6. 上传 APK artifact + 打印 APK 内 `lib/arm64-v8a/*` 清单。

签名（方案 A）：`keystore/sproot-release.jks` + `keystore.properties`（私仓内）；`app/build.gradle.kts` 读取该 properties，缺失时 release 产出 unsigned 但**不影响编译**；同时支持 `SPROOT_KEYSTORE*` 环境变量覆盖。

---

## 8. 旧版失败根因规避清单（逐条落地）

| # | 约束 | 落地位置 |
|---|---|---|
| ① | 入口能力自检（存在/可执行/ELF AArch64） | `Capabilities` + `Elf`（引擎、magiskboot、输入魔数） |
| ② | 引擎用 aarch64 `.so` + JNI，不用可执行文件 | `PatchEngine`（`System.loadLibrary`） |
| ③ | 禁构建期网络下载 | 无网络权限；工作流不下载二进制（artifact 仅可选拉取）；jniLibs 由 CI/仓库提供 |
| ④ | 外部镜像（kpimg 类）可选 | 无该依赖；引擎自包含 |
| ⑤ | 日志落盘 + 失败 UI 常驻可复读 | `AppLog` + 修补页错误区 + LogPanel |
| ⑥ | 禁脚本/禁 `exit $?`/禁 abort 0 | 无任何 shell 脚本；只用 exit code |
| ⑦ | 成功校验产物存在 + 等长 + sha256 | `PatchPipeline` 步骤 8 |
| ⑧ | 密钥不绑 DEBUG | `SecretStore`（Keystore AES-GCM，debug/release 同路径） |
| ⑨ | 不在 filesDir exec ELF | magiskboot 在 `nativeLibraryDir`；引擎走 JNI |
| ⑩ | 单一调用路径，不拼 `sh -c` | `MagiskBoot.exec(ProcessBuilder 数组)`；`ApdClient.rootExec` |

---

## 9. 待办与开放项

1. **native 资产落地**：`libskrootpatch.so`（patcher 的 engine CI 产物）、`libapd.so`（sys-guard 的 apd CI 产物）→ 确认 artifact 名称后由 workflow 自动拉取（当前步骤为候选名猜测，SKIP 不影响构建）。
2. **AVB 结论**：patcher 正在确认魅族 21 是否需要重刷 vbmeta / `--disable-verification`；UI 已预留提示位与命令模板。
3. **模块列表**：待 `apd module list --json` 的 JSON schema 定稿后接入（当前空态 + 说明，不伪造条目）。
4. **刷入执行**：首阶段保持"导出 + 人工 fastboot"；后续若接入 AP 内刷入（`dd`/`mtd`），必须走 root-exec 且带 bootloop 守卫。
5. **真机验证**：`PatchPipeline` 的 magiskboot 解包/回压序列需在魅族 21 的 boot.img 上实测（CI 只能验证编译）；失败时错误区/日志/工作目录三件套可直接取证。
6. **文档缺口**：`NEW-DOC-04` 之后若 UI 大改，需同步更新 §3 与 §4 阶段图。

---

*文档完 · 客户端 · 2026-11-08*
