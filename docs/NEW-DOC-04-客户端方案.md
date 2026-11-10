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
| miuix 模块 | **ui + icons + nav + preference + blur**（照抄 KSU miuix 版所需最小集） | KSU 的 `manager/gradle/libs.versions.toml` 五模块；本项目 UI 重做后已全部使用（preference 用于设置页，blur 用于顶栏/底栏毛玻璃，nav 依赖备用） |
| miuix-blur 与 minSdk | 项目 minSdk 保持 **26**；manifest 加 `tools:overrideLibrary="top.yukonga.miuix.kmp.blur"`（该库声明 minSdk 33） | 与 KSU 同样处理；代码侧统一 `isRenderEffectSupported()`（SDK_INT>=31）守卫，API<31 自动退化为不透明色 |
| 图标 | **miuix-icons**（`MiuixIcons.Home/Update/Layers/Settings/Refresh/Ok/Report/Phone/Info/Tasks/Store/Lock/Tune`、`MiuixIcons.Basic.Check/Close`） | 曾引入 material-icons-extended 导致 APK 15.4MB；改 miuix-icons 后回落到 11.2MB（两库风格也更一致） |
| 导航 | 不用 navigation-compose；关于页用状态驱动 + BackHandler | 只有 4 个平级页 + 1 个二级页（关于），状态驱动可避免引入 kotlinx-serialization 编译器插件（CI 风险更低）；KSU 的 NavDisplay 未照搬 |

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

**UI 已按人类指令完全照抄 KernelSU 的 miuix 实现**（固定 miuix，无 Material/Miuix 双模式；KSU 来源声明见根 `NOTICE` §6.1）：

- 骨架：`ui/AppRoot.kt` = MiuixTheme + `HorizontalPager`(4 页) + 毛玻璃 `BottomBarMiuix`（宽屏切 `NavigationRailMiuix`），每页自带 `Scaffold` + `BlurredBar(TopAppBar)`（KSU 页面结构）；
- 主题：`ui/theme/{Theme,MiuixTheme,Colors,ThemePrefs}.kt`，照抄 KSU `ui/theme/MiuixTheme.kt` 的 miuix 分支（`ThemeController` + `TonalSpot` + `Spec2025` + 15 色强调色）；Monet/materialkolor 未引入（KSU 的 Monet 依赖 material3）；
- 组件：`ui/components/{BlurExt,BottomBar,StatusTag,WarningCard,ConfirmDialog}.kt` 逐行照抄 KSU 对应 miuix 文件；
- 页面映射：Home→主页状态卡、Install+Flash→修补页、Module→模块页、Settings→设置页、About→关于页（二级页）；
- 底部导航四页（主页 / 授权 / 模块 / 设置，KSU 顺序）+ 关于页与修补页两个二级页；
  设置页设置项**全部替换为 SPRoot 自己的**（主题 / Root 密钥 / 日志 / 原镜像备份 / AP 部署 / 引擎自检 / 工作目录清理 / 关于许可）。

### 3.1 主页（Home）
1. **刷入状态卡**（人类指令 B，2026-11-10）：第一行 **`未安装 / 已安装`**（不再用"已修补/未修补"）；
   **「已安装」= 仅 `ACTIVE`**（设备真的在运行修补后的内核、daemon 可用）：由 App 自引导探测到
   `apd selftest stage=daemon_ready` 时置 `ACTIVE`（`InstallState.markActiveDetected`）；
   `PATCHED_PENDING_FLASH` / `FLASHED_PENDING_REBOOT` / `UNPATCHED` / `ERROR` 一律"未安装"。
   **授权成功绝不等于安装成功**：auth 路径与 InstallState 无任何联动（已核查）。
   下面只保留两项：**引擎 <version> · JNI ABI v2** 与 **GKI <os.version>**。点卡片进入修补页。
   删除：`AP 待部署` 模式行、状态 hint、`SK 产物已导出，尚未刷入设备`、`点击查看产物与刷入指引`。
2. **设备信息卡**：机型/Android/ABI/内核/App 版本。
3. **提示区**：引擎未就绪、上次修补失败、密钥不可用、**AP 守护自启动失败**（A）、历史 48 位密钥。
   「环境自检」与「查看产物/刷入指引」入口已删除（B）。

### 3.1A App 自引导启动 AP 守护（A，替代 init_boot 必要性）

人类质疑「为什么非要改 init_boot」——APatch/SK 都不需要改 init_boot。核实结论：SK 引擎只有提权/隐藏 hook，
没有 `call_usermodehelper` 之类启动器，旧方案靠 ramdisk 的 rc 拉起 daemon 属于绕路。
**SP 提权不依赖 daemon**（内核直接比对密钥），因此 App 可以自己提权拉起 daemon：

```
App 冷启动 / 收到 BOOT_COMPLETED（新增 RECEIVE_BOOT_COMPLETED + BootReceiver）
  → apd selftest --json 探测（stage=daemon_ready 即在跑，幂等，不重复启动）
  → 未运行且已部署 → **libapd.so（引导器，nativeLibraryDir）** root-exec --
    `/data/adb/apd --superkey-file /data/adb/sproot/key daemon`（部署目标，由 root 执行）
    （daemon 常驻；确认就绪后销毁 root-exec 父进程，daemon 在独立进程组存活）

**SELinux 域约束（人类 2026-11-10 指出，踩坑 #7）**：app 域（`untrusted_app`）只对
`nativeLibraryDir`（apk_data_file）有 execute 权限，`/data/adb/*` 一律**只能作为参数**交给 root 执行。
因此 `ApdClient` 明确拆分：`apdBootstrap()` = nativeLibraryDir/libapd.so（root-exec 引导器，永远不变）、
`apdBinary()/deployedApd()` = `/data/adb/apd`（部署目标/命令参数）；`status()` 按引导器判定可执行性；
部署链固定 `libapd.so root-exec -- libapd.so deploy --source <nativeLib>`（首装 `/data/adb/apd` 尚不存在）。
  → 未部署（undeployed/escalation_failed）= 正常初始态，不标红 → App 不重试拉起
  → 拉起失败 → 结果落 SharedPreferences，主页以**错误卡显示原因**（不静默，下次启动/开机重试）
```
`init_boot.img` 因此降级为**可选（高级）**：默认流程只需要 `boot.img`；不再"强烈建议"，
未选也不再当缺陷提示；**保留**其注入代码（选中才注入）。未选 init_boot 时补丁管线
**不再回退改 boot ramdisk 注入 rc**（`PatchPipeline` ⑥.5 跳过），避免无谓 ramdisk 改动/AVB 风险。

### 3.2 修补页（Patch）——本项目的核心
自上而下：
1. **选择镜像**：SAF `OpenDocument("*/*")` + `takePersistableUriPermission`；展示文件名/URI。
   **P2 双产物（两种不同的事，不可混称"修补 init_boot"）**：
   `boot.img` **修补内核**（kallsyms→hook→shellcode→密钥，获得提权能力）；
   `init_boot.img` **注入自启脚本**（Android 13+ 的 init_boot 只有 generic ramdisk、**没有 kernel**，
   无可修补之物；这里只注入 `init.sproot.rc` + `init.rc` import，让 AP daemon 开机自启）。
   `init_boot.img` 为**可选（高级）**（A：App 自引导已负责拉起 daemon，不再依赖 rc 自启）；
   未选不提示缺陷、不"强烈建议"。
   **修补页不独立成 Tab**：主页是唯一入口（KSU 的 Home→Install/Flash 模型）。
2. **进度**：`百分比 + LinearProgressIndicator + 当前阶段 + 引擎阶段`；引擎进度来自 NDJSON，占 35%→80% 区间。
3. **错误区（常驻）**：`errorTitle/errorDetail` 一经写入不自动消失，仅"重置"清除；提供「展开/收起」「复制」。
4. **结果区**：产物路径、大小、sha256、**内核等长校验**、AVB、自启注入（若选了 init_boot）、Root 密钥（脱敏/显示/复制）。
5. **页面最下方一行小字**：`导出 TXT 日志到 Downloads/SPRoot/`，点击导出 session 日志，失败 Toast 提示。
6. **已删除（人类指令 C）**：所有说明性/提示小字、修补模式选择（含"导出并给出刷入指引"）、日志面板、
   已修补产物区块、刷入指引卡、两级状态标记、所有 WarningCard。
7. **两级状态**：「我已刷入（标记待重启）」→「已重启并生效（标记激活）」，状态由用户手动回报（无 root 时 App 无法读取 fastboot 结果）。
8. **常驻日志面板**：级别配色、自动滚底、复制全部。

### 3.3 模块页（Modules）—— 按 KSU `ModuleMiuix` 结构重做（v0.2）

- **顶部**：毛玻璃 TopAppBar（标题「模块」+ 排序/筛选菜单 + 刷新）；列表首项为常驻**搜索栏**
  （按 名称 / id / 作者 / 描述 过滤）。排序菜单：按启用状态分组（默认）/ 按名称 / 有更新优先。
- **分区列表**：`【已启用 N】` 与 `【已禁用 N】` 两个分区；每个 ModuleItem 卡为
  「左侧图标 + 名称/版本/作者/描述，右侧 Switch」；**点卡片进入详情页**（不是点开关）。
- **详情页**（二级页，返回键）：头部信息卡 + 操作卡（`SwitchPreference` 启用/禁用、
  `ArrowPreference` 卸载）+ 模块信息卡（id/version/versionCode/author/enabled/update_pending/
  install_status，可复制 id）。
- **安装**：KSU 式右下角 FAB「+」→ SAF 选 zip → 复制到私有 cache → `validateZip`（PK 头/大小）
  → `apd module install`；安装中/安装结果（含原始 stdout/stderr 可复制）以卡片置顶展示。
- **空态**：大图标 + 文案（区分「尚未安装」与「搜索无匹配」）；AP 未就绪时顶部 Notice 提示并禁用操作。
- **卸载**：二次确认对话框（说明「AP 模块变更通常在下次重启后生效」）。
- 业务数据仍只经 `ModuleRepository`（`apd module ...` root-exec），客户逻辑未改。

### 3.3A 授权管理页（SuperUser，v0.5 重做：所有已装应用 + root 授权开关）

照 KSU `SuperUserMiuix.kt` 版式：TopAppBar「授权」+ 审计日志入口 + 筛选（全部/已授权/未授权）+ 刷新；
搜索栏（应用名/包名/uid）；应用行 = 图标 + 名称 + 包名 + 右侧 **root 授权开关** + 点行进详情；
详情 = 应用信息 + 三态单选（允许/拒绝/待询问·Phase 1 不可交互）+ 审计日志入口 + 撤销 + 权限详情。

**主数据源（人类指令 D）**：**PackageManager 枚举手机上所有已安装用户应用**（排除系统应用与本 App），
默认全部未授权（`AuthState.NONE`）；授权态来自 `apd auth list --json`，按 uid（其次 package）合并；
apd 有但 PM 枚举不到的行追加展示（避免"幽灵授权"不可见）。开关 ON = 授予（`apd auth set --state allow`），
OFF = 撤销（`apd auth revoke`→deny）；乐观更新 + 失败回滚重载。

**不做（等引擎评估）**：「卸载模块 / umount modules」（KSU 的 UMOUNT 能力，内核 `setuid` hook +
`path_umount` 实现）——我们引擎目前没有该能力，**不画开关、不做灰态占位**，等 engine-dev 结论。

**审计日志页保留**：`event/decision/package/cmd_name/iso`（apd 只给脱敏信息：无完整命令、无密钥，UI 不补全不猜测）。

**数据契约**（apd commit `d6c9ecb`，经 `ApdClient.rootExec`）：
- `apd auth list --json` → 裸数组：`{uid, package, label, app_icon_path, state, last_grant_at,
  last_deny_at, request_count, to_uid, sctx, exclude}`；`last_*_at` 为 **unix 秒**（客户端换算毫秒）；
- `apd auth set --uid N --state allow|deny|ask --json` / `apd auth revoke --uid N --json` →
  `{ok, uid, state, previous, package, changed}`；
- `apd auth log --json --limit N` → 裸数组（最新在前）：`{ts, iso, uid, package, event, state,
  decision, previous, cmd_name, cmd_args, cmd_sha256, detail}`；
- **`label`/`app_icon_path` 恒为 null**（apd 无 PackageManager）→ App 侧用 PackageManager 按 package
  解析应用名/图标；包不存在（uid 无对应包）显示 `uid=xxxxx` + 通用图标（不崩、不空白）。

**三态与默认态（重要）**：
- `allow` 允许 / `deny` 拒绝；
- `ask` **Phase 1 不可交互**（daemon 挂起请求+弹窗回传未实现，当前等同拒绝，日志记 `decision=ask_pending`）：
  详情页显示但**禁用**并标注「Phase 1 不可交互」，提示用户改为允许/拒绝，**不假装可用、不做假弹窗**；
- **默认授权态是 `ask`** → 新装应用第一次请求 root 会被拒；授权页顶部常驻说明，避免用户误判"App 坏了"。

**降级**：AP 未部署（125）时页面顶部 Notice「尚未部署 AP 守护（需先刷入修补后的镜像）」，
列表失败原因按退出码给出人话（125/124/126/127/2-契约不匹配）。

### 3.4 设置页（Settings）
外观（主题）、Root 密钥（显示/复制/重新生成 + 旧内核失效警告）、日志（路径/复制/清空）、**原镜像备份**（sha1 列表 / 还原到 Downloads / 删除）、AP 部署（`apd deploy --source <nativeLibDir>`，显示 exit code 与完整输出）、关于（版本/工具链/契约版本）。

**按钮密度（人类真机反馈 ④）**：所有 Action 按钮统一为紧凑 inline 文字按钮
（`CompactTextButton`：minHeight 30dp、insideMargin 10/3dp、13sp、minWidth 0），
相关操作横向排一行（显示/复制/重新生成、部署/重跑自检、还原/删除），不再整行大按钮；
只改尺寸与排布，不改功能。

**密钥生命周期（人类反馈 ③）**：一机一份、固定不变——首次需要时生成 47 字符，之后固定；
只认 47 字符；旧 48 字符格式一次性自动迁移（重新生成 47 + prefs 标记 `key_migrated_v47`，只提示一次）；
损坏/长度不对 → 清除并重新生成；保留 `engineKeyOf()`（47→48 是引擎 ABI，不是历史机制）；
「重新生成」必须二次确认并警告"已修补镜像内嵌旧密钥，必须重新修补并刷入"。

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
6.4 init_boot 自启注入（P2，**仅在用户选了 init_boot（高级）时执行**）  **不做内核修补**（init_boot 无 kernel）：
                   copy → 备份 → magiskboot unpack → 在 init_boot ramdisk 注入 init.sproot.rc + init.rc import
                   → repack → **ramdisk 级回读**（rc 真在最终镜像）→ AVB-1/2/3 →
                   产物 init_boot_patched.img（自启注入产物）
                   未选 init_boot：**跳过注入且不回退改 boot ramdisk**（A：自启由 App 自引导负责）
7 重打包      magiskboot repack boot.img → new-boot.img
7.5 回读断言 把 new-boot.img 复制到 verify/ 再 unpack（必要时再 decompress）：
              回读 kernel sha256 **必须等于** kernel.patched 且 **不等于** kernel.orig/raw，否则 T-12 中止
              同时检查 AVB footer 存在性是否保持、记录产物/输入总长差异
8 校验导出    产物存在/非空 → 修补内核与输入内核**长度相等** → sha256 变化 → 导出 Downloads/SPRoot/
              双镜像时 boot + init_boot 两个产物**全部通过校验后**才导出；任一失败即中止、一个都不导出
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
| AP root-exec | 125 | **尚未部署 AP 守护（需先刷入修补后的镜像）**——首启正常初始态；UI 用 Notice 展示，**不标红、不显示"失败/未通过"**、不自动重试 |
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

### 7A.2 AP 首启自检 `apd selftest --json`

- 启动时 `SPRootApp` 后台执行一次；有密钥走 `root-exec`，无密钥回退直接 exec（只读探测）。
- `stage=undeployed` **exit 0 = 正常状态**（AP 未部署），UI 显示「AP 未部署」，不报错。
- **exit 125 同属正常初始态**（有密钥但内核未刷补丁镜像 → 提权被静默拒绝）：
  `ApdSelfTest` 新增 `isNormalInitialState`，UI 文案「尚未部署 AP 守护（需先刷入修补后的镜像）」。
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

### 9.0 UI 重做与 P2 双镜像（已完成，2026-11-09）

- **UI 完全照抄 KSU miuix 版**：四页骨架 + 关于页；设置项全部替换为 SPRoot 自己的 8 项；
  业务逻辑类（PatchPipeline/ApdClient/InstallState/PatchEngine/Capabilities/SecretStore/BackupStore、
  Elf/MiniJson/ModuleExitCode 等）**未改动逻辑**（P2 与 47 密钥口径属功能修复，另行记录）。
- **P2 双产物**：`boot.img` → **内核修补产物**（kernel 级 T-12）；`init_boot.img` → **自启注入产物**
  （ramdisk 级回读：rc + import 真在最终镜像），init_boot 无 kernel、不做内核修补；
  两产物分别做回读断言与 AVB-1/2/3；全通过才导出；
  `InstallState.artifacts` 持久化 `products[]`。
- **密钥口径**：本地/AP/内核 47 字符（内核 ABI 47+NUL）；引擎 `--key` 入参 48（引擎内部 erase 末位），
  由 `SecretStore.engineKeyOf()` 补位，见 §9.1。
- **APK 体积**：15.4MB → 11.2MB（移除 material-icons-extended，改 miuix-icons）。

### 9.0B 内置模块组件（A 步：物料入仓，2026-11-10）

- assets 打入官方 release 原样 zip（不修改内容）：
  `app/src/main/assets/builtin/neozygisk-v2.4.zip`（NeoZygisk v2.4 build 289，**GPL-3.0**，module id=`zygisksu`）
  与 `mountify-205.zip`（Mountify v2.0.5 build 205，**Unlicense**，module id=`mountify`）；
- 清单 `app/builtin-components.json`：name/version/version_code/module_id/file/size/sha256/source_url/
  upstream/license/license_url（可复现，供设置页展示来源与许可证）；
- CI 门禁 `quality/static/check_builtin_assets.py`：清单 ↔ assets 双向校验（存在性/size/sha256/未登记 zip），
  已接入 ci-static（防"物料被悄悄替换"）；
- 说明：仅入仓，**不构建**任何东西；安装流程（apd module install + Mountify config 注入 APatch）在 B 步。

### 9.0A UI 精简与 App 自引导（已完成，2026-11-10，人类指令 A–E）

- **B 主页**：删除环境自检块与"查看产物/刷入指引"入口；刷入状态卡改「未安装/已安装 + 引擎 + GKI」。
- **C 修补页**：删除全部说明小字/模式选择/日志面板/已修补产物/刷入指引；底部新增 TXT 日志导出到 Downloads/SPRoot/。
- **D 授权页**：主数据源改为 PackageManager 枚举**所有已安装用户应用**（默认未授权），每行 root 授权开关
  （授予/撤销，走 `apd auth set/revoke`），详情三态 + 审计日志；**不做 UMOUNT 开关**（内核能力未实现，等 engine-dev）。
- **E 模块页**：删除「可能原因」「AP 自检」等 KSU 没有的区块。
- **A App 自引导**：冷启动 + BOOT_COMPLETED 探测并拉起 AP 守护（§3.1A）；init_boot 降级为可选高级项，
  未选时补丁管线不再改 boot ramdisk 注入 rc。

1. **native 资产落地**：
   - `libapd.so` ← CI 固定 artifact **`apd-aarch64-latest`**（含 `libapd-arm64.so` + `apd-version.txt`，约 1.35 MB）；工作流已按此名拉取并重命名为 `libapd.so`，同时把 `apd-version.txt` 打到 `app/src/main/assets/`；
   - `libskrootpatch.so` ← 等 patcher 的 `engine-arm64` 工作流修好（qemu 问题）后按 artifact 名拉取；当前工作流按候选名尝试，缺失 SKIP 不阻断构建；
   - 版本一致性：`ApdClient.bundledApdVersion()`（assets）对比 `ApdClient.deployedApdVersion()`（`apd version --json` 的 commit），不一致 → UI 提示「需重新部署」，**不允许静默用旧二进制跑 root-exec**。
2. **AVB**：结论已定（不碰 vbmeta，只刷 boot，依赖已解锁 bootloader），UI 已按结论改写；详见 §4.4。
3. **模块列表**：待 `apd module list --json` 的 JSON schema 定稿后接入（当前空态 + 说明，不伪造条目）。
4. **刷入执行**：首阶段保持"导出 + 人工 fastboot"；后续若接入 AP 内刷入（`dd`/`mtd`），必须走 root-exec 且带 bootloop 守卫。
5. **真机验证**：`PatchPipeline` 的 magiskboot 解包/回压序列需在魅族 21 的 boot.img 上实测（CI 只能验证编译）；失败时错误区/日志/工作目录三件套可直接取证。
6. **文档缺口**：`NEW-DOC-04` 之后若 UI 大改，需同步更新 §3 与 §4 阶段图。
7. **D1 自启链（模板已落地，语义仍保守）**：权威 rc 模板 = **`apd/assets/init.sproot.rc`**（lead 落地，commit `6c4e751`），客户端**只做「读模板原文 → 写入 ramdisk」**，不自行改写：
   - CI 构建时把 `apd/assets/init.sproot.rc` 复制到 `app/src/main/assets/`，并校验模板含 `service sproot-apd`、`property:sys.boot_completed`、`oneshot`、`disabled`；APK 断言 `assets/init.sproot.rc` 必须存在；
   - 注入到 ramdisk：`/init.sproot.rc` = 模板原文；`/init.rc` 确保 `import /init.sproot.rc`（幂等）；
   - 日志记录 `rc_injected / rc_target / rc_source / rc_reason / rc_idempotent_hit`；`rc_source` 应为 `apd/assets/init.sproot.rc`；
   - 模板安全设计（不在客户端改写）：密钥不入 rc（只引用 **`/data/adb/sproot/key`**）、`oneshot+disabled`（最坏自启失效，不会 bootloop）、`override` 幂等、`seclabel` 留空待真机 avc 结论；
     > **2026-11-10 修订（本条曾双重过期）**：① 路径原写 `/data/adb/sproot/superkey` —— **错误**，rc 模板与 `apd/src/defs.rs:54`（`SPROOT_KEY_FILE = /data/adb/sproot/key`）一致，全仓无 `superkey` 路径（该错由 `2e1323a` 修正，见 `docs/HANDOFF-2026-11-08.md` §6.8 第 1 条）；② 内容口径：该文件是 **47 字符纯文本、无 NUL**（不是「48 字符」，也不是「47+NUL」——后者只属内核 shellcode 内嵌格式）。
   - ~~**仍缺一块（非客户端）**：`apd deploy` 尚未实现把密钥写入 `/data/adb/sproot/superkey`（0600），因此当前注入后 daemon 会因读不到 key 安全退出；~~
     > **2026-11-10 修订：此项已闭环**。`apd deploy` 已实现密钥落盘 —— `apd/src/deploy.rs` 的 `write_key_file()`（step 5/10：`key_material()` 严格 47 字节校验 → tmp → 0600 → fsync → rename 原子写，幂等，各分支擦除内存副本）。daemon 不再因缺 key 退出。故 UI 文案里「待 AP 部署完成后生效」仍成立，但原因已不是「密钥未写入」，而是「首次开机前 `/data/adb/apd` 与 rc 尚未就位」。
   - UI 文案：「**已注入开机自启模板（target=…，来源=…）—— 待 AP 部署完成后生效**」，日志同口径，不显示"自启已开启"。

---

*文档完 · 客户端 · 2026-11-08*

---

## 10. 客户端推前自检（quality/static，CI 门禁）

背景：2026-11-09 P2 双镜像开发期间连续两次"CI 才暴露的低级编译错误"
（字符串模板嵌套双引号；组件漏 import / 具名参数改名漏同步），每轮浪费 4–5 分钟 CI 周期。
事后沉淀为 3 个零依赖脚本，已进 **`quality/static/`** 并由 **`.github/workflows/ci-static.yml`**
作为门禁执行——**任何人改 `app/` 都会被拦住**，不再依赖「记得跑」。

| 脚本 | 查什么 | 真实事故形态 |
|---|---|---|
| `check_kt_syntax.py` | 普通字符串/字符字面量/三引号是否闭合；`()[]{}` 是否配对；块注释闭合；**字符串闭合后紧邻 `$` 或 ASCII 标识符**（= 内层双引号未转义） | `VerifyResult(true, "$RCFILE 与 "$IMPORT_LINE" 均…")` —— 词法上被切成 3 段，编译器才报 `Expecting ')'` |
| `check_kt_signatures.py` | 扫描仓库内 `fun Name(...)` 形参名，比对所有 `Name(x = ...)` 调用点的具名参数 | `FlashGuide` 形参改名 `onCopy`，调用点仍写 `onCopyCommand =` → `No parameter with name` |
| `check_kt_imports.py` | 大写类型/Composable 定义所在包 vs 使用点是否 import（跳过注释/同包/全限定名） | `ui/pages/PatchPage.kt` 用 `WarningCard`（定义在 `ui.components`）漏 import → `Unresolved reference` |

用法（本地，与 CI 完全相同）：

```bash
python3 quality/static/check_kt_syntax.py
python3 quality/static/check_kt_signatures.py
python3 quality/static/check_kt_imports.py
# 均可加 --root <dir> 指向别处（便于用负样本自测）
```

规则（v0.5 扩充后，每条都有负样本验证会 FAIL）：

| 检查器 | 规则 | 覆盖的真实事故 |
|---|---|---|
| `check_kt_syntax.py` | 字符串/三引号/字符字面量闭合、`()[]{}` 配对、块注释闭合；**字符串闭合后紧邻 `$`/ASCII 标识符**（嵌套双引号）；支持 `${...}` 模板嵌套（不误报） | `RcInject` 嵌套双引号（run 42636829029） |
| `check_kt_signatures.py` | 具名参数必须在定义中；**唯一签名时校验必填参数是否缺失**（支持尾随 lambda）；忽略字符串/注释里的伪调用 | `FlashGuide` 参数改名未同步 + 漏传 `onCopy`（run 42637756697） |
| `check_kt_imports.py` | ① 跨包缺 import（类型/Composable）；② **已知库扩展函数缺 import**（drawIntoCanvas/nativeCanvas/graphicsLayer/clip/nestedScroll/scrollEndHaptic/layerBackdrop… 白名单可逐轮扩充）；③ **禁用 `Icons.Rounded|Filled|Outlined|Default|AutoMirrored` 与 `androidx.compose.material.icons.*`**（图标统一 miuix-icons / 自绘 ImageVector） | `WarningCard` 漏 import（run 42637756697）；`drawIntoCanvas` 漏 import（run 42722204812）；material-icons 体积/不存在图标问题 |

### 10.1 已知踩坑表（红一次 → 工具强一次）

| # | 事故（run） | 错误形态 | 现状 |
|---|---|---|---|
| 1 | 42636829029 | Kotlin 字符串模板裸写内层双引号 → 词法上切成 3 段 | ✅ `check_kt_syntax` 规则 4 覆盖 |
| 2 | 42637756697 | 跨包用 Composable 漏 import；函数形参改名后调用点未同步 + 漏必填参数 | ✅ `check_kt_imports` A / `check_kt_signatures` 覆盖 |
| 3 | 42722204812 | Compose 扩展函数 `drawIntoCanvas` 漏 import（扩展函数写在别的包） | ✅ `check_kt_imports` B 覆盖（扩展名白名单） |
| 4 | 42721703071 | `Icons.Rounded.*` / miuix 图标名不存在或依赖已移除 | ✅ `check_kt_imports` C 全面禁用 material `Icons.*`；miuix 图标名仍需编译校验（白名单暂不含图标名） |
| 9 | 42815365839 | 同一文件重复 import（脚本替换 `import ...Card` 前缀时命中 `CardDefaults`，插入两次）→ `Conflicting import` | ✅ `check_kt_imports` 新增规则 E（重复 import 直接失败）；负样本验证 FAIL |
| 8 | 42809437255 | KDoc 里写 `/data/adb/*`：Kotlin 块注释**可嵌套**，其中的 `/*` 再开一层 → 注释吞到文件尾（`Unclosed comment`） | ✅ `check_kt_syntax` 规则 3 改为按嵌套深度计数，EOF 未闭合即报错；负样本验证 FAIL |
| 7 | （真机必炸，未进 CI） | **SELinux 域**：`untrusted_app` 只能 exec `nativeLibraryDir`；把部署后的 `/data/adb/apd` 当 root-exec 引导器 → 部署完成后 root-exec 全被拒（"越装越坏"） | ✅ 代码已拆分：`apdBootstrap()`=nativeLibraryDir（唯一引导器）、`apdBinary()/deployedApd()`=部署目标（只作 root 目标命令参数）；`status()` 按引导器判定；部署链固定 `libapd.so → root-exec -- libapd.so deploy`。属静态检查器边界（同 #6），靠契约+真机验证 |
| 6 | 42807799287 | Android 平台 API 存在性（`ProcessBuilder.Redirect.DISCARD` 在目标 bootClasspath 上不可用）——静态文本检查器无法判断"某个 Java/Android API 在当前 compileSdk 是否存在" | ⚠️ 边界：编译器/lint 是唯一防线；已记入 §10 边界（本次改为 `Redirect.to(/dev/null)`） |
| 5 | 42805281686 | **本项目自有扩展函数**跨包使用却删掉了 import（`context.copyToClipboard(...)`；旧检查器只查"定义在别的包的大写类型/Composable"，漏了小写扩展） | ✅ `check_kt_imports` 新增规则 D（自动收集 `fun Receiver.name(...)`，非 private 的跨包使用必须 import）；负样本已验证 FAIL |

### 10.2 推送流程约定（硬规则）

1. **推送前**：跑三个检查器（脚本在 `quality/static/`，同 CI 一致），全绿再推；
2. **推送后**：**立刻盯 CI 结果**（PAT 查 run）；**上一轮未出结果前不推下一批功能改动**——
   红链会掩盖真正的错误，也会拉长"人类拿不到可用 APK"的时间；
3. **每次红了**：把错误形态补进 §10.1，并评估是否能变成检查器规则（能就补 + 负样本验证）；
4. 新增断言的纪律不变：**先用负样本证明它会 FAIL**。

边界（**不替代编译器**）：只做词法/签名/import 级启发式；查不出类型错误、未解析符号（除上述白名单）、
泛型推导、库函数形参名变更等；真正编译仍以 GitHub Actions 为唯一来源。

---

*文档完（v0.2 增补 §9.0 / §10 与 P2 双镜像、UI 重做、47 字符密钥口径）*
