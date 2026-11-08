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
0 环境自检    ABI=arm64 / 引擎 libskpatchjni.so(ELF A64+可加载) / magiskboot(ELF A64+可执行)
              / 存储≥3GB / 密钥可读写 / 输入 ANDROID! 魔数
1 准备镜像    contentResolver 复制 → cacheDir/patch/job-<ts>/boot.img；sha256 记录
2 备份        filesDir/backups/<sha1>.img（内容寻址，同镜像复用）+ index.tsv
3 解包        magiskboot unpack boot.img   (cwd=工作目录)  → kernel
3.5 备份内核  cp kernel kernel.orig（repack 模板基准），记录 sha256
4 解压内核    若 kernel.orig 头是 gzip/lz4/zstd/xz/bzip2/lzop → magiskboot decompress kernel.orig kernel.raw
5 修补        NativeEngine.patch(kernel.raw → kernel.patched, key48, json=progress.ndjson)
6 覆盖内核    **必须**把修补结果写回 `kernel`（压缩则 compress=<fmt> kernel.patched kernel，否则直接复制）
              记录 kernel/kernel.orig/kernel.patched 三者 sha256；若 kernel == kernel.orig → T-12 中止
7 重打包      magiskboot repack boot.img → new-boot.img
7.5 回读断言 把 new-boot.img 复制到 verify/ 再 unpack（必要时再 decompress）：
              回读 kernel sha256 **必须等于** kernel.patched 且 **不等于** kernel.orig/raw，否则 T-12 中止
              同时检查 AVB footer 存在性是否保持、记录产物/输入总长差异
8 校验导出    产物存在/非空 → 修补内核与输入内核**长度相等** → sha256 变化 → 导出 Downloads/SPRoot/
9 状态        失败 → InstallState.ERROR；成功 → PATCHED_PENDING_FLASH
```

> **T-12（补丁未生效）是最隐蔽的失败模式**：`magiskboot repack` 只认工作目录里的 `kernel`，
> 若修补结果写到 `kernel.patched` 而未覆盖 `kernel`，repack 会静默使用原始 kernel——
> 产物长度/结构都正常，但内核段未修补。因此步骤 6 的覆盖与步骤 7.5 的回读断言都是**强制**的，
> 任一不通过即中止且**绝不导出产物**。每步 sha256 都写日志，可复现、可追溯。

全程**无 root、无 shell、无网络**。任何一步失败：错误详情 + 完整输出 + 工作目录（保留供诊断）+ 日志文件路径，全部常驻展示。

### 4.2 JNI 绑定契约（引擎已提供，App 侧对齐）

patcher 的 CI artifact **`skroot-engine-arm64`**（实测内容：`libskpatchjni.so`、`libskrootpatch.so`、`magiskboot`、`magiskboot-meta.txt`、`SHA256SUMS`）提供 JNI 形态，导出符号（`nm -D` 实测）：

```
Java_me_sproot_patch_NativeEngine_patch(JNIEnv*, jclass, jstring inPath, jstring outPath,
                                        jstring key, jstring jsonSink, jstring workdir) -> jint
Java_me_sproot_patch_NativeEngine_engineVersion(JNIEnv*, jclass) -> jstring
```

App 侧对应实现 `me/sproot/patch/NativeEngine.kt`（`@JvmStatic external`），并由 `core/PatchEngine.kt` 包装：

- `inPath` = `--in`（已解压 arm64 内核 Image）｜`outPath` = `--out`｜`key`（null/空 → `--gen-key`）
- `jsonSink` = `--json`（NDJSON 进度落盘，Java 侧轮询 150ms）｜`workdir` = `--workdir`
- 引擎内部固定追加 `--quiet`；**进度与结果不走 stdout**（JNI 下不可见），只走 json + `<out>.report.json`
- 返回值 = C1 §3 退出码枚举，Kotlin 侧逐码映射中文文案；`root_key` 从 `report.json` 取，取不到回落到调用方密钥
- 引擎缺失/ABI 不符时 `ensureLoaded()` 返回**可读错误**（含期望符号名），UI 作为阻断项展示
- **不再**使用旧草案里的 `com.sproot.manager.core.PatchEngine.native*` 符号（那是本项目自拟，未采用）

### 4.3 为什么 magiskboot 走 nativeLibraryDir

- magiskboot 来源：**Magisk v31.1 官方 APK 内的 `lib/arm64-v8a/libmagiskboot.so`**（实为静态链接的 aarch64 可执行 ELF，约 988,984 B；参考 sha256 `97e9ec75…da4bb54`），由 patcher 的 CI artifact `magiskboot-arm64` 提供，工作流拉取后重命名为 `libmagiskboot.so`；
- `libmagiskboot.so` 随 APK 分发（`jniLibs/arm64-v8a/`），`packaging.jniLibs.useLegacyPackaging = true` 保证解压到 `nativeLibraryDir`（真实文件、可 exec）；
- **不在 filesDir exec ELF**（旧版缺陷 ⑨；Android 10+ noexec + SELinux W^X）；
- 调用方式是 `ProcessBuilder(listOf(bin, "unpack", "boot.img"))`，**不经 `sh -c`、不拼字符串**（缺陷 ⑩）；
- stdout+stderr 合并逐行回调并实时落盘；超时强杀并明确报 timeout；`--version/--help` 用于能力探测；
- 目录结构：`unpack` → 引擎 patch `kernel`（必要时 `decompress`/`compress=<fmt>`）→ `repack`（以原 boot.img 为模板）→ 等长/回读校验；
- 许可：GPL-3.0，APK 内 `assets/NOTICE-magiskboot.txt` 标注来源与版本；
- CI 断言：APK 内若存在 `lib/arm64-v8a/libmagiskboot.so`，其 ELF `e_machine` 必须为 183(AArch64)；已拉取却未打进 APK 时构建失败。

### 4.4 产物导出与刷入

- 导出路径：`Downloads/SPRoot/boot_patched_<yyyyMMdd-HHmmss>.img`
  - Android 10+：`MediaStore.Downloads` + `RELATIVE_PATH=Download/SPRoot`（无需存储权限）
  - Android 9-：公共 Downloads 目录（`WRITE_EXTERNAL_STORAGE` maxSdkVersion=28）
- 导出后 UI 展示：大小、sha256、内核等长校验结果、fastboot 命令（A/B 双槽 `--slot=all`）。
- 「导出成功」与「刷入成功」是**两级状态**：刷入由用户手动确认后状态机才前进。

**AVB 结论（已由三家实现核实，推翻早期假设）**：MEIZU21GKI / KSU / KernelPatch 均**不触碰 AVB 密码学**，依赖已解锁 bootloader 跳过验证；`--disable-verification` 才是清数据来源。因此刷入指引固定为：

```
fastboot getvar current-slot        # A/B 机型确认槽位
fastboot flash --slot=all boot <patched>.img
fastboot reboot
```

- **明确禁止**提示 `--disable-verity/--disable-verification`，**不碰 vbmeta**；
- UI 文案：「仅刷 boot 分区；若设备处于锁态将无法开机（属预期），请确认已解锁」。早期版本的「AVB 提示位 + vbmeta 模板」已按本结论删除。

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

### 6.1 G-04 错误码 → 用户文案映射表（客户端职责，已实现）

UI 铁律：**原始码与可读文案同时展示**（错误区"错误码"字段 + 标题），便于用户截图上报与质测断言。

| 来源 | 退出码 | UI 文案（`Models.kt` / `ApdClient.kt`） |
|---|---|---|
| 修补引擎 | 0 | 成功 |
| 修补引擎 | 101 | E_USAGE：调用参数错误（引擎实现或调用方 bug） |
| 修补引擎 | 102 | E_INPUT：输入内核不可读 / 仍是 .img/.gz / 非 arm64 Image |
| 修补引擎 | 201 | E_KALLSYMS：kallsyms 解析失败，该内核暂不支持 |
| 修补引擎 | 202 | E_SYMBOL_MISSING：关键符号缺失，该内核暂不支持 |
| 修补引擎 | 210 | E_OFFSET_DERIVE：cred/uid/seccomp 偏移推导失败 |
| 修补引擎 | 211 | E_INIT_CRED_NOT_FOUND：init_cred 模式搜索失败 |
| 修补引擎 | 220 | E_NO_HOOK_SPACE：shellcode 空间不足 |
| 修补引擎 | 230 | E_UNSUPPORTED_VERSION：内核版本超出 3.10~6.18 支持域 |
| 修补引擎 | 240 | E_WRITE：输出写失败（磁盘/权限） |
| 修补引擎 | 250 | E_INTERNAL：引擎内部未分类错误 |
| 修补引擎 | 其他 | E_UNKNOWN(<code>)：引擎返回了契约外的退出码（原样带出） |
| App 自检 | T-12 | 补丁未生效：产物 kernel 回读与 kernel.patched 不一致（不导出） |
| AP root-exec | 0 | 成功 |
| AP root-exec | 1 | AP 业务失败（展示 stderr 摘要） |
| AP root-exec | 2 | AP 用法错误——视为 bug，展示原始 argv |
| AP root-exec | 124 | 超时（提示用户，保留日志） |
| AP root-exec | 125 | 密钥无效或需重新修补（提权被内核静默拒绝；**不自动重试**） |
| AP root-exec | 126 | 目标命令不可执行（126），需重新部署 AP |
| AP root-exec | 127 | 目标命令不存在（127），需重新部署 AP |
| AP root-exec | 其他 | 未知退出码（<code>），原样展示 |

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

---

## 7A. 模块列表 / AP 自检 / 引擎自检 / 许可打包（v0.2 新增）

### 7A.1 模块列表 Schema v1（客户端提案，待 AP 对齐）

调用：`apd module list --json`（经 `root-exec`，契约 v1.1 §8.3）。**App 不直接读 /data/adb。**

```json
[
  {
    "id": "magisk-module-id",
    "name": "显示名",
    "version": "1.2.3",
    "versionCode": 12,
    "author": "作者",
    "description": "一句话描述",
    "enabled": true,
    "update_pending": false,
    "install_status": "installed"
  }
]
```

> **状态：Schema v1 已由 sys-guard 定稿（v2 commit `8b28e87`）**：上列 8 字段（含 `versionCode`）为正式字段，
> `install_status` 为 **v2 待定**占位（客户端容忍缺失）。排序按 `id` 字典序稳定；stdout 纯 JSON 数组，
> 日志只走 stderr/logcat；无模块时输出 `[]` + exit 0。

**写操作（子命令已由 sys-guard 定稿，v2 `af55f55` / CI 42490703820；详见 NEW-DOC-05 §3.1.2）**：
- `apd module install <zip路径> --json`：App 先把用户选的 zip 复制到私有 cache 并校验 PK 头/大小，再传绝对路径；超时 300s。
  成功：`{"ok":true,"id","name","version","versionCode","metamodule","enabled","update_pending","active_after_reboot","changed","previous_version","uncompressed_bytes","avb_untouched"}`；
  失败：`{"ok":false,"code":<exit>,"id","error"}`，**exit code = code**。
- `apd module uninstall <id> --json`：`{"ok":true,"id","remove_pending","applied_after_reboot","avb_untouched"}`；不存在 → **110**。
- `apd module enable|disable <id>`；全部经 `root-exec`。
- 两阶段语义（解包 → `modules_update/<id>`，**下次 post-fs-data 原子生效**）：UI 明确展示 `update_pending` / `active_after_reboot` / `remove_pending`，不谎称"立即生效"。
- 客户端解析：`MiniJson.objectOf` 取扁平字段（上面 schema 全扁平，够用）；`ok=false` 时用 `error` 文案 + 退出码映射，原始 stdout/stderr 常驻可复制。

#### 7A.1.1 安装/卸载失败 → 中文可操作提示（G-04 延伸）

实现：`core/ModuleFailureHints.kt`。输入 = exit code + apd 原始 stdout/stderr（**不依赖 JSON schema**），
输出 = 「可能原因 / 建议动作 / 依据」三元组，UI 逐条常驻展示。退出码优先（契约 §8.5），其次关键字匹配：

| 触发 | 可能原因 | 建议动作 |
|---|---|---|
| exit=2 | AP 用法错误 | 视为客户端 bug，上报原始输出（勿重试） |
| exit=110 | 目标模块不存在 | 刷新列表确认 ID；手动删除过则重启 AP |
| exit=111 | zip 损坏 | 重新下载并校验 sha256 |
| exit=112 | module.prop 缺失/字段非法 | zip 根目录直接含 module.prop；id 只含字母数字与 `_`/`-` |
| exit=113 | 不安全 zip（zip-slip/符号链接） | 已拒绝安装；换可信来源 |
| exit=114 | 超限（zip>512MiB / 解压>1GiB / 条目>10万） | 精简模块或换更小版本 |
| exit=115 | 磁盘空间不足 | 清理 /data（预留 2 倍解压体积） |
| exit=116 | 模块 ID 冲突 | 先卸载同 ID 或改 module.prop 的 id |
| exit=120 | 模块安装脚本失败 | 模块自身脚本问题：查看原始 stderr/日志，联系作者 |
| exit=121 | 解包/IO（含 booting 门禁） | 设备完整启动后重试 |
| exit=122 | state 写入失败 | 重新部署 AP / 检查 /data 权限 |
| exit=124 | 超时 | 检查空间后重试；持续超时上报 |
| exit=125 | 密钥无效/内核静默拒绝 | 重新「修补 → 刷入」；不自动重试 |
| exit=126 / 127 | 不可执行 / 命令不存在 | 「设置 → AP 部署」重新部署 |
| 输出含 `module.prop` | 模块包缺 module.prop | 确认 zip 根目录直接含 module.prop（不是套文件夹） |
| `already exists` / `duplicate` / `conflict` | 模块 ID 冲突 | 先卸载同 ID 或改 module.prop 的 id |
| `no space` / `ENOSPC` | 空间不足 | 清理 /data（预留 2 倍解压体积） |
| `permission denied` / `EACCES` / `EPERM` | 权限不足（FS/SELinux） | 确认 AP 已部署且内核已修补；刚刷机先重启 |
| `avc: denied` / `selinux` | SELinux 拒绝 | 收集完整 avc 行上报，重新部署/修补 |
| `bad zip` / `corrupt` / `End of central directory` | zip 损坏 | 重新下载并校验 sha256 |
| `not found` / `no such file` | 目标不存在 | 刷新列表确认 ID；重启 AP |
| 其他 | 未识别失败 | 保留原始输出上报，**不猜测** |

客户端解析规则（`core/Modules.kt`）：
- **容忍缺字段**：缺失显示 "—"，未知字段忽略；
- 兼容键：`module_id/id`、`versionName/version`、`summary/description`、`updatePending/update_pending`；
- **空数组 = 无模块（正常）**；非空但解析不出 = schema 不符 → 页面常驻展示 apd **原始 stdout/stderr** 并可复制上报；
- 启用/禁用：`apd module enable|disable <id>`（同样 root-exec），退出码映射见 §6.1；
- 模块列表加载失败时也保留原始输出（绝不静默）。

> **模块安装/卸载（cross-ref）**：`apd module install <zip> --json` / `apd module uninstall <id> --json` 的 JSON schema、两阶段语义（`update_pending`/`active_after_reboot`）与退出码（110–122/124/126/127）以 **NEW-DOC-05 §3.1.2** 为准，本文不重复定义。

### 7A.2 AP 首启自检 `apd selftest --json`

- 启动时 `SPRootApp` 后台执行一次；有密钥走 `root-exec`，无密钥回退直接 exec（只读探测）。
- `stage=undeployed` **exit 0 = 正常状态**（AP 未部署），UI 显示「AP 未部署」，不报错。
- 其它 stage 原样展示（`deployed`/`daemon_ready`/`needs_repair`…）；结果缓存 SharedPreferences，主页/模块页/设置页只读展示，并提供「重跑 AP 自检」。

### 7A.3 B-14 引擎自检入口（qa JNI 验证）

设置页「引擎自检」按钮：读取 `libskrootpatch.so` 路径/存在性/AArch64 判定/是否加载，并调用 `engineVersion()`，输出：

```
库路径：/data/app/.../lib/arm64/libskrootpatch.so
存在=true；AArch64=true；已加载=true
engineVersion()=<引擎返回>
结论：已加载，引擎版本 …，JNI ABI v2
```

结果同时写日志（tag=Settings），供 qa 做 JNI 加载验证（B-14）。

### 7A.4 NOTICE / 许可证随包（单源，满足 qa LIC-1 与 release.yml LIC 门禁）

构建期 CI 组装（不再手工维护多份副本）：

1. `assets/NOTICE` = 仓库根 `NOTICE`（唯一事实源）+ `assets/NOTICE-APK.md`（APK 特有清单，四段式）：
   ① 本项目 AGPL-3.0；② **SKRoot 上游：版权归原作者、保留所有权利（All Rights Reserved）、AGPL-3.0 不适用该部分**（含上游 URL `github.com/abcz316/SKRoot-linuxKernelRoot`）；③ magiskboot（Magisk v31.1、GPL-3.0、来源 URL + APK/二进制 sha256 + 源码 tag）；④ 其他第三方（AndroidX/Compose/Kotlin/miuix=Apache-2.0，asmjit=Zlib，capstone=BSD-3，cJSON=MIT）；
2. 随包许可证全文：`assets/licenses/AGPL-3.0.txt`（= 仓库根 `LICENSE`）、`assets/licenses/LICENSE`（同全文，供门禁按名查找）、`assets/licenses/NOTICE`（同 NOTICE 内容）、`assets/licenses/GPL-3.0.txt`（magiskboot 义务）；
3. 构建期校验材料（`ENGINE-META.txt`、`SYMBOLS-libskrootpatch.txt`、`SHA256SUMS`、`magiskboot-meta.txt`、`apd-version.txt`）一并入 assets；
4. CI 硬断言（构建失败条件）：上列 6 个 asset 必须存在；`assets/NOTICE` 必含 `SKRoot`/`All Rights Reserved`/`AGPL-3.0`/`GPL-3.0`/`magiskboot`/`asmjit`/`capstone`/`cJSON`；`assets/licenses/LICENSE` 必须 >20KB 且含 AGPL 标题。

### 7A.5 原生二进制锁定表 `app/native.lock.json`（可复现构建）

**问题**：早期 CI "动态拉取最新 artifact" → 同一 commit 重跑会得到不同 APK（引擎/apd 的 sha256 会漂移），
`release.yml` 的三方 sha256 一致性与 qa 的 T8（产物可复现）必然失败。

**方案**：`app/native.lock.json` 入 git，锁定每个原生件的 `run_id / artifact / file / sha256 / size / commit`：

```json
{
  "engine":      { "artifact": "skroot-engine-arm64", "run_id": 42489595614,
                   "file": "libskrootpatch.so", "target": "libskrootpatch.so",
                   "sha256": "258ebc22…", "size": 2998040 },
  "apd":         { "artifact": "apd-aarch64-latest", "run_id": 42490165535,
                   "file": "libapd-arm64.so", "target": "libapd.so",
                   "sha256": "62c0075c…", "size": 2061888, "commit": "d45822b6…" },
  "magiskboot":  { "artifact": "magiskboot-arm64", "run_id": 42488321400,
                   "file": "magiskboot", "target": "libmagiskboot.so",
                   "sha256": "97e9ec75…", "size": 988984, "version": "31.1" }
}
```

CI 行为（硬失败）：
1. 按 lock 的 `run_id` + artifact 名**精确下载**（不在 latest 上浮动）；
2. 逐文件校验 `sha256`，不匹配 → `::error::` → 构建失败（禁止出包）；
3. 校验材料（`ENGINE-META.txt`/`SYMBOLS-libskrootpatch.txt`/`SHA256SUMS`/`apd-version.txt`/`magiskboot-meta.txt`）随包入 assets；
4. 升级 native 必须**显式修改 native.lock.json 并单独 commit**（可审计、可回溯）。

> 效果：同一 commit + 同一 lock ⇒ 同一 sha256 的 APK，满足 T8 与 release 门禁的按 commit 断言。

### 7A.6 产物可复现（T8，按 qa 容差口径）

- **native 字节一致**：由 `app/native.lock.json` 保证（run_id + sha256 精确锁定，见 §7A.5）。
- **APK 条目内容一致**：CI 构建时设置 `SOURCE_DATE_EPOCH=$(git log -1 --format=%ct)`（commit 时间基准）；
  签名块与 zip 时间戳不可避免带来整体字节差异，qa 的 `compare_ci_runs.py` 采用「逐条目内容一致」容差判定，
  因此**不追求 APK 字节完全一致**，只要求：native sha256 一致 + APK 内条目内容一致 + 版本/签名一致。

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

1. **native 资产落地**：
   - `libapd.so` ← CI 固定 artifact **`apd-aarch64-latest`**（含 `libapd-arm64.so` + `apd-version.txt`，约 1.35 MB）；工作流已按此名拉取并重命名为 `libapd.so`，同时把 `apd-version.txt` 打到 `app/src/main/assets/`；
   - `libskrootpatch.so` ← 等 patcher 的 `engine-arm64` 工作流修好（qemu 问题）后按 artifact 名拉取；当前工作流按候选名尝试，缺失 SKIP 不阻断构建；
   - 版本一致性：`ApdClient.bundledApdVersion()`（assets）对比 `ApdClient.deployedApdVersion()`（`apd version --json` 的 commit），不一致 → UI 提示「需重新部署」，**不允许静默用旧二进制跑 root-exec**。
2. **AVB**：结论已定（不碰 vbmeta，只刷 boot，依赖已解锁 bootloader），UI 已按结论改写；详见 §4.4。
3. **模块列表**：待 `apd module list --json` 的 JSON schema 定稿后接入（当前空态 + 说明，不伪造条目）。
4. **刷入执行**：首阶段保持"导出 + 人工 fastboot"；后续若接入 AP 内刷入（`dd`/`mtd`），必须走 root-exec 且带 bootloop 守卫。
5. **真机验证**：`PatchPipeline` 的 magiskboot 解包/回压序列需在魅族 21 的 boot.img 上实测（CI 只能验证编译）；失败时错误区/日志/工作目录三件套可直接取证。
6. **文档缺口**：`NEW-DOC-04` 之后若 UI 大改，需同步更新 §3 与 §4 阶段图。

---

*文档完 · 客户端 · 2026-11-08*
