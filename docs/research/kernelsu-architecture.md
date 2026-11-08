# KernelSU 管理器架构与「手机端本地修补 boot 镜像」全链路调研

- 调研对象：`tiann/KernelSU`
- 上游基线：**`730f644615969cfd4a84b45d7f62a667b7b95a50`**（main，commit date `2026-11-07 10:33:54 +0800`，`manager: accept x-zip MIME types when opening module zips (#3929)`）
- 获取方式：`git clone --depth 1 https://github.com/tiann/KernelSU /tmp/ksugit`（本机 WSL 内 `github.com` 首页 HEAD 请求会超时，但 git 协议可用；备用方案 `codeload.github.com/tiann/KernelSU/tar.gz/refs/heads/main` 亦可）
- 本地副本：`/tmp/ksugit`（WSL），下文所有 `path:line` 均相对该仓库根
- 用途：作为 SPRoot 新 SP 管理器（Android 侧）的参考实现
- 配套文档：`docs/research/apatch-manager-architecture.md`（同结构调研 APatch 管理器，含两仓逐项对比）

> 说明：本文结论全部来自实际源码；凡源码无法直接证实的，显式标注「未确认」。不引用任何未在源码中出现的 API。

---

## 0. 结论摘要

1. **管理器本体不做任何镜像修补**。它是一个 Kotlin + Jetpack Compose 的「壳」，所有特权动作（修补、刷分区、装模块、改授权）都通过 exec 一个 **Rust 写的 `ksud` 二进制**完成，该二进制以 `libksud.so` 之名打包进 APK 的 `jniLibs`，运行时路径就是 `applicationInfo.nativeLibraryDir/libksud.so`。
2. **APK 里没有 `magiskboot`，也没有 `kptools`**。boot.img 的解包 / cpio 操作 / 回包由 ksud 内的**纯 Rust 实现**完成，底层依赖 `android-bootimg` crate（`5ec1cff/android_bootimg`，git rev 固定）。`userspace/ksud/bin/<arch>/` 下只有 `bootctl`、`busybox`、`waitsys` 三个外部二进制。
3. **`ksuinit` 与 `<kmi>_kernelsu.ko` 不在 APK 顶层**，而是编译期用 `rust-embed` **嵌进 ksud 二进制内部**（源目录 `userspace/ksud/bin/<arch>/` 被 `.gitignore`，由 CI 产出），修补时从内存取出直接写进 ramdisk。因此 APK 只有 `libksud.so` / `libadbroot.so` / `libkernelsu.so` 三个 native 件。
4. **修补链路是「参数化的单命令」**：`libksud.so boot-patch -b <boot.img> -o <outdir> [--kmi ...] [--partition ...] [-m <ko>] [-u] [--allow-shell] [--enable-adbd] [--backup]`；直装模式用 `-f`，ksud 自己 `open("/dev/block/by-name/boot…")` + `BLKROSET` 解除只读 + `write_all` + `sync_all` 写回分区。管理器只负责把文件从 SAF 复制到 cache、拼参数、把 stdout/stderr 流式渲染成日志。
5. **root 授权是「预授权名单」模型，没有交互式弹窗**。内核侧持久化 `/data/adb/ksu/.allowlist`（二进制格式，magic `0x7f4b5355`，`FILE_FORMAT_VERSION 4`）+ per-app profile（uid/gid/groups/caps/SELinux 域/namespace/规则）。用户在管理器「超级用户」页把某个应用的 `allowSu` 打开即完成授权；未授权的 `su` 请求直接被拒绝，只在内核审计日志（sulog）里留痕。
6. **supercall 传输层已不是 prctl**（本 commit）：用户态 `syscall(SYS_reboot, 0xDEADBEEF, 0xCAFEBABE, 0, &fd)` 让内核 hook 装入一个 `anon_inode:[ksu_driver]` fd，之后全部命令走 `ioctl(fd, _IOC('K', n), ...)`（`KSU_IOCTL_*` 表，`KSU_INSTALL_MAGIC1/2` 仅用于取 fd）。
7. **管理器自身的 root shell 也是 ksud 提供的**：libsu `Shell.Builder.build(<libksud.so>, "debug", "su"[, "-g"])`，即 `ksud debug su` → `grant_root()`。因此管理器不依赖 `/system/bin/su` 存在；而普通应用的 `su` 则靠内核 hook 把 `/system/bin/su` 路径替换成 `/data/adb/ksud`（sucompat）。
8. **模块管理语义与 Magisk / APatch 的 apd 高度同构**：`/data/adb/modules`、`/data/adb/modules_update`、`module.prop`、`customize.sh`、`post-fs-data.sh`/`service.sh`/`boot-completed.sh`、`disable/remove/update` 标志位、内置 Magisk 风格 `installer.sh` 用 busybox `sh -c` 执行。
9. **本 commit 还存在另一条更激进的链路**：`boot-patch-v2`（`lkm_image.rs`，116 KB）把 kernel module 直接注进 **kernel image**（capsule magic `KSULKM1`、kallsyms/BTF 恢复、text cave 修补），完全不改 ramdisk。**管理器 Kotlin 侧没有任何调用点**（全仓 grep `boot-patch-v2` 只命中 ksud CLI 与文档），即当前只供 CLI/PC 使用。
10. 对新 SP 管理器最值得抄的是**「薄 UI + 厚 CLI」的进程边界**和**「零外部二进制」的修补引擎选型**；最需要改造的是**无交互授权模型**与**把 `nativeLibraryDir` 当守护进程路径**这一脆弱假设。

### 组件职责一览

| 组件 | 位置 | 语言 | 职责 |
|---|---|---|---|
| 管理器 App | `manager/` | Kotlin / Compose | 页面、参数拼装、日志展示、授权开关、模块 UI、WebUI 宿主 |
| `libksud.so` = ksud | `userspace/ksud/` | Rust | **唯一的特权执行者**：修补、刷写、模块、sepolicy、su、resetprop |
| `ksuinit` | `userspace/ksuinit/` | Rust（`no_std` 风格，无 stdio） | ramdisk 里的 `/init` 第一阶段：加载 ko 后 exec 真正的 init |
| `libkernelsu.so` | `manager/app/src/main/cpp/` | C++ (JNI) | ioctl 封装（`ksu.cc`）+ Java 对象编组（`jni.cc`） |
| `libadbroot.so` | `manager/app/src/main/cpp/adbroot.cc` | C++ | LD_PRELOAD 劫持 `__system_property_find` 等，令 adbd 默认 root |
| 内核模块 | `kernel/` | C | allowlist、app profile、sucompat、sulog、manager 身份识别 |
| UAPI | `uapi/` | C header | 内核/用户态共享 ABI（supercall、app_profile、sulog） |

---

## 1. 管理器技术栈与页面结构

### 1.1 技术栈（实测自 `manager/gradle/libs.versions.toml`、`manager/build.gradle.kts`、`manager/app/build.gradle.kts`）

| 项 | 值 | 依据 |
|---|---|---|
| 语言 | Kotlin `2.4.20`，Java 21 兼容级别 | `libs.versions.toml`、`gradle.properties`… → `manager/build.gradle.kts:11-12` |
| UI | Jetpack Compose（BOM `2026.09.00`）+ Material3 `1.5.0-alpha28` | `libs.versions.toml` |
| 额外 UI 套件 | **Miuix KMP `0.9.4`**（`miuix-ui` / `miuix-icons` / `miuix-nav` / `miuix-preference`） | 同上 |
| 导航 | `miuix-nav`（type-safe，`NavKey` + `@Serializable` + `@Parcelize`） | `ui/navigation3/Routes.kt` |
| 构建 | AGP `9.4.1`，`compileSdk 37` / `targetSdk 37` / `minSdk 31`，BuildTools `37.0.0`，NDK `29.0.14206865` | `manager/build.gradle.kts:7-18` |
| root 交互 | **libsu `6.0.0`**（`core` / `service` / `io`） | `libs.versions.toml` |
| 网络 | OkHttp `5.5.0` | 同上 |
| 序列化 | kotlinx-serialization `1.11.0` + **protobuf `4.36.1`**（用于解析 OTA `payload.bin`） | 同上、`app/src/main/proto/update_metadata.proto` |
| 压缩 | commons-compress `1.28.0`、xz `1.12` | 同上 |
| WebView | androidx.webkit `1.17.0` | 同上 |
| APK 签名 | `org.lsposed.lsplugin.apksign` `1.4`（签名配置走 `sign.example.properties`） | `app/build.gradle.kts:31-36` |
| ABI | `arm64-v8a` / `x86_64` / `riscv64` | `app/build.gradle.kts` `abiFilters` |

规模：`manager/app/src/main/java` 下 **259 个 `.kt`** + 5 个 `.java`（`SuFilePathHandler`、`MimeUtil`、`AppZygotePreload` 等）、3 个 `.cc`、1 个 `.aidl`、1 个 `.proto`。

### 1.2 代码分层（实测目录）

```
manager/app/src/main/java/me/weishu/kernelsu/
├── Natives.kt                  ← JNI 门面：Profile 数据类 + 全部 ioctl 语义（含 MINIMAL_SUPPORTED_KERNEL 表）
├── KernelSUApplication.kt
├── core/
│   ├── tasks/                  ← BootKernelVersion.kt(本地 KMI 解析) / ExtractImage.kt(OTA zip) / Payload.kt(payload.bin)
│   └── utils/DataSourceChannel.kt  ← 可随机读的 HTTP 数据源（边下边解包）
├── data/
│   ├── model/                  ← AppInfo / Module / ModuleUpdateInfo / RepoModule / TemplateInfo
│   ├── repository/             ← SuperUser/Module/ModuleRepo/Settings/Template 五组 Repository+Impl
│   ├── registry/               ← 模块仓库客户端（RegistryClient/Parser/UpdatePolicy/DownloadIntegrity）
│   └── script/                 ← 模块脚本（ScriptPackage/ScriptCommand/ScriptParams/ScriptRunRecord）
├── magica/AppZygotePreload.java ← LKM 模式的 zygote 预加载入口（fork 后 exec ksud late-load）
├── profile/
├── ui/
│   ├── MainActivity.kt          ← 四页 Pager 外壳 + navigation3 目的地注册
│   ├── navigation3/             ← Routes.kt / Navigator.kt / IntentDispatcher.kt
│   ├── screen/                  ← 14 个功能目录（见下）
│   ├── component/ theme/ util/  ← KsuCli.kt 就在这里（核心！）
│   ├── viewmodel/
│   └── webui/                   ← 模块 WebUI 宿主：WebUIActivity + SuFilePathHandler + JS 桥
└── ...
```

### 1.3 页面结构

顶部只有 **4 个一级页**（`MainActivity.kt:416-418` 的 `when` 索引 → `HomePager` / `SuperUserPager` / `ModulePager` / Settings），其余都是 `ui/navigation3/Routes.kt` 中的二级目的地：

| 一级页 | 二级 / 相关页面 | 作用 |
|---|---|---|
| Home | — | 状态卡片：内核版本、管理器版本、SELinux/seccomp、KMI、是否 LKM、是否已安装；安装/更新/卸载入口 |
| SuperUser | `AppProfile`、`AppProfileTemplate`、`TemplateEditor`、`Sulog` | 应用列表 + 每应用 `allowSu` 开关 + root profile（uid/gid/groups/caps/域/namespace/规则）；su 审计日志 |
| Module | `ModuleRepo`、`ModuleRepoDetail`、`ExecuteModuleAction`、`ScriptRun` | 模块列表/安装/开关/卸载/action；内置模块仓库浏览与下载 |
| Settings | `About`、`ColorPalette` | 主题、UI 模式（Material/Miuix）、更新通道 |
| 独立流程 | `Install`、`Flash` | 修补安装向导、刷写进度与日志（`FlashingStatus` 三态 + 时间戳日志） |

`Routes.kt` 的关键写法（type-safe + 可序列化 + 可 Parcel）：

```kotlin
@Serializable
sealed interface Route : NavKey, Parcelable {
    @Parcelize @Serializable data object Main : Route
    @Parcelize @Serializable data object Home : Route
    @Parcelize @Serializable data object SuperUser : Route
    @Parcelize @Serializable data object Module : Route
    @Parcelize @Serializable data object Settings : Route
    @Parcelize @Serializable data object Sulog : Route
    @Parcelize @Serializable data object Install : Route
    @Parcelize @Serializable data class AppProfile(val uid: Int) : Route
    ...
}
```

### 1.4 双 UI 模式（值得注意的工程约束）

每个页面都拆成「无样式状态 + 两套皮」：

- `XxxScreen.kt`（逻辑/状态/动作）、`XxxUiState.kt`（`@Immutable` 状态模型）、`XxxUtils.kt`（纯函数）、
- `XxxMaterial.kt` + `XxxMiuix.kt`（两套渲染），由 `ui/LocalUiMode` 选择。

例：`ui/screen/install/InstallScreen.kt` 在末尾 `when (uiMode) { UiMode.Miuix -> InstallMiuix(...); UiMode.Material -> InstallMaterial(...) }`。这是为了同时满足 AOSP 风格与小米 HyperOS 风格，代价是页面数量翻倍。

---

## 2. 本地修补完整链路（命令级）

### 2.0 APK 里到底有什么（这是「是否内置 magiskboot/ksud/ksuinit」的直接答案）

```
manager/app/src/main/
├── jniLibs/                    ← 只放了 .gitignore（内容：libksud.so），真正的 libksud.so 由构建时 cp 进来
│   └── .gitignore              # "libksud.so"
├── cpp/                        # libkernelsu.so（jni.cc + ksu.cc）、libadbroot.so（adbroot.cc）
├── assets/webview/             # 只用于渲染 Markdown（模块说明/更新日志），非修补资源
├── proto/update_metadata.proto # OTA payload 解析
└── res/…
```

`jniLibs` 的填充来自 `justfile`：

```make
build_ksud:
    cross build --target aarch64-linux-android --release

build_manager: build_ksud
    cp target/aarch64-linux-android/release/ksud manager/app/src/main/jniLibs/arm64-v8a/libksud.so
    cd manager && ./gradlew aDebug
```

CI 也是同一套动作（`.github/workflows/build-manager.yml:270-278` 把三个架构的 `ksud` 产物分别 `cp` 成 `target/<triple>/release/ksud`）。

**结论**：
- **ksud：内置**（作为 `libksud.so`，`packaging.jniLibs.useLegacyPackaging = true` 保证不被压缩、可直接 exec）。
- **ksuinit：内置，但间接**——它不单独出现在 APK，而是被 `rust-embed` 编译进 ksud：

```rust
// userspace/ksud/src/assets.rs
#[cfg(all(target_arch = "aarch64", target_os = "android"))]
#[derive(RustEmbed)]
#[folder = "bin/aarch64"]
struct Asset;               // 该目录下：busybox / bootctl / waitsys / ksuinit / <kmi>_kernelsu.ko
```

`userspace/ksud/bin/.gitignore` 内容就是 `**/*.ko` 与 `**/ksuinit` —— 说明这两个是 CI 现场产出再嵌入（`.github/workflows/ksud-extra.yml:34-41`）。
- **magiskboot / kptools：都没有**（`find userspace/ksud/bin -type f` 仅 `bootctl`、`busybox`、`waitsys`）。

### 2.1 管理器侧：四种安装入口

`ui/screen/install/InstallScreen.kt:80-88` 决定入口的可见性：

```kotlin
val installMethodOptions = remember(rootAvailable, isAbDevice, isGkiDevice, ...) {
    buildList {
        add(InstallMethod.SelectFile(summary = if (isGkiDevice) selectFileTip else selectFileTipNoGki))
        add(InstallMethod.DownloadFile(summary = downloadFileMsg))
        if (rootAvailable && isGkiDevice) {
            add(InstallMethod.DirectInstall)
            if (isAbDevice) add(InstallMethod.DirectInstallToInactiveSlot)
        }
    }
}
```

| 入口 | 前置条件 | 行为 | 最终命令 |
|---|---|---|---|
| SelectFile | 无（未安装也能用） | 用户选本地 boot/init_boot img → 复制到 cache → 修补 → 产物写 Downloads | `libksud.so boot-patch -b /data/user/0/<pkg>/cache/boot.img -o /storage/emulated/0/Download --partition <part>` |
| DownloadFile | 无 | 用户给 OTA/工厂包 URL → **在手机上**解析 zip 的 `payload.bin` 或工厂包的 `boot.img` → 修补 | 同上，但 `-b` 指向解出的 `cache/download-boot.img`，并强制 `--kmi <探测值>`、`--partition <part>` |
| DirectInstall | 已 root 且 GKI | `-f`：ksud 自己定位 boot 分区并原地写回 | `libksud.so boot-patch -f [--partition <part>]` |
| DirectInstallToInactiveSlot | 上者 + A/B | 加 `-u`，对**另一个 slot** 修补，之后可走系统 OTA 切槽 | `libksud.so boot-patch -f -u` |

（`DirectInstall` 成功后 `installBoot()` 会再调一次 `install()` → `ksud install` 把 daemon 装到 `/data/adb/ksud`，见 §2.4。）

### 2.2 命令拼装（`ui/util/KsuCli.kt`，命令级证据）

守护进程路径：

```kotlin
// KsuCli.kt:44-46
private fun getKsuDaemonPath(): String {
    return ksuApp.applicationInfo.nativeLibraryDir + File.separator + "libksud.so"
}
```

`installBoot(...)` 的完整拼装（`KsuCli.kt:369-436`，节选）：

```kotlin
val bootFile = bootUri?.let { uri ->                 // SAF → cache/boot.img
    with(resolver.openInputStream(uri)) {
        val bootFile = File(ksuApp.cacheDir, "boot.img")
        bootFile.outputStream().use { output -> this?.copyTo(output) }
        bootFile
    }
}

var cmd = "boot-patch"
cmd += if (bootFile == null) " -f" else " -b ${bootFile.absolutePath}"
cmd += bootPatchFlags(allowShell, enableAdb, forceBackup)   // --allow-shell / --enable-adbd / --backup
if (ota) cmd += " -u"
val lkmFile = writeLkmFile(lkm)                              // 用户自备 ko
if (lkmFile != null) cmd += " -m ${lkmFile.absolutePath}"
else if (lkm is LkmSelection.KmiString) cmd += " --kmi ${lkm.value}"
if (bootFile != null) cmd += " -o $downloadsDir"             // Environment.DIRECTORY_DOWNLOADS
partition?.let { part -> cmd += " --partition $part" }

val result = flashWithIO("${getKsuDaemonPath()} $cmd", onStdout, onStderr)
```

`flashWithIO` 用一个**新的** root shell（`withNewRootShell`），把 stdout/stderr 通过 libsu 的 `CallbackList` 逐行回调给 Compose 日志页（`FlashScreen` 的 `FlashingStatus.FLASHING/SUCCESS/FAILED`）。

`downloadBoot(...)`（`KsuCli.kt:438-533`）额外做三件事：读文件头 magic 判断是 `CrAU`（raw payload）还是 zip；用 `ExtractImage` 在手机上把目标分区解出来；**用第二条 HTTP 连接单独 probe KMI**（因为 `ZipFile` 会关掉 channel）。

### 2.3 `ksud boot-patch` 内部（`userspace/ksud/src/boot_patch.rs:627-947`）

按执行顺序，逐步对应源码：

| # | 步骤 | 关键实现 |
|---|---|---|
| 1 | 直装时要求 GKI | `if !patch_file { ensure_gki_kernel()? }` |
| 2 | **KMI 决策**（优先级从高到低） | `--kmi` 显式 → `-u` 时从 `/dev/block/by-name/boot{slot}` 解析 → `get_current_kmi()`（ioctl）→ `parse_kmi_from_boot(image)` → `parse_kmi_from_kernel(kernel)` |
| 3 | 定位镜像 | 有 `-b` 就 `canonicalize`；否则 `auto_boot_partition_path(&kmi, ota, is_replace_kernel, &partition)` |
| 4 | 解出 busybox/bootctl | `let _ = assets::ensure_binaries(false)`（写 `/data/adb/ksu/bin/`） |
| 5 | mmap + 解析 | `let boot_image_data = map_file(&boot_image_file)?; BootImage::parse(&boot_image_data)?` |
| 6 | 头版本校验 | `enforce_bootimage_version`：`BootImageVersion::Android(ver) if ver < 3` → 报错 |
| 7 | 选 ko | `--module` > 内置 `{kmi}_kernelsu.ko`（`assets::get_asset`） |
| 8 | 选 ksuinit | `--init` > 内置 `ksuinit` |
| 9 | 解 ramdisk | `extract_ramdisk`：vendor_boot 优先取名为 `init_boot` 的条目（注释明确「匹配原 magiskboot 查找顺序」）；无 ramdisk 则 `Cpio::new()` |
| 10 | 拒绝二次修补 | `ensure!(!cpio.is_magisk_patched(), "Cannot work with Magisk patched image")` |
| 11 | **改写 ramdisk** | `if !is_kernelsu_patched && cpio.exists("init") { cpio.mv("init","init.real") }`；`cpio.add("init", 0o755, ksu_init)`；`cpio.add("kernelsu.ko", 0o755, kernelsu_ko)` |
| 12 | 写配置 | `ksu_config` 文本项：`norc=1` / `allow_shell=1` / `preinit=0` / `force_feature=<id>:<v>` / `block_modules=<list>` / `bundled=1`；顺手 `cpio.rm("allow_shell")` 清历史遗留 |
| 13 | 可选 debug | `--enable-adbd`/`--adb-debug-prop` → 写 `force_debuggable`(空文件) + `adb_debug.prop`（`ro.debuggable=1` 等） |
| 14 | 可选备份 | `do_backup`：算源镜像 sha1 → 复制到 `/data/adb/ksu/ksu_backup_<sha1>` → ramdisk 里写 `stock_image.sha1` |
| 15 | 回包 | `cpio.dump()` → `patcher.replace_ramdisk(...)` 或 `replace_vendor_ramdisk(idx, ...)` → `patcher.patch(&mut new_boot_buf)` |
| 16 | **释放 mmap 再刷写** | `drop(boot_image); drop(boot_image_data);` —— 注释说明「否则某些内核会拒绝后续写」 |
| 17 | 刷写（`-f`） | `flash_partition(&bootdevice, &new_boot_bytes)`；`-u` 时再 `post_ota()` |
| 18 | 落盘产物 | `out_name` 缺省 `kernelsu_patched_%Y%m%d_%H%M%S.img`，写 `-o` 目录 |

刷分区的真实实现（`boot_patch.rs:203-220`）：

```rust
pub(super) fn flash_partition(partition: &str, data: &[u8]) -> Result<()> {
    let mut blk = std::fs::OpenOptions::new().write(true).truncate(false).create(false)
        .open(partition).with_context(|| format!("open {partition}"))?;
    unsafe {
        const BLKROSET: i32 = libc::_IO(0x12, 93);
        let mut val: libc::c_int = 0;
        if libc::ioctl(blk.as_raw_fd(), BLKROSET, &raw mut val) != 0 {
            bail!("Failed to set rw for {partition}: {}", *libc::__errno());
        }
    }
    blk.write_all(data).context("flash boot failed")?;
    blk.sync_all().context("sync boot failed")?;
    Ok(())
}
```

备份（`boot_patch.rs:159-179`）：

```rust
pub(super) fn do_backup(cpio: &mut Cpio, image: &Path) -> Result<()> {
    let sha1 = calculate_sha1(image)?;
    let (mut target_file, target) = find_backup_location(&sha1)?;
    let mut source = OpenOptions::new().read(true).write(false).open(image)?;
    std::io::copy(&mut source, &mut target_file)?;      // 用 io::copy 才能拷块设备
    cpio.add(BACKUP_FILENAME, CpioEntry::regular(0o755, Box::new(sha1)));
    Ok(())
}
```

### 2.4 刷入方式小结（含「ksud 自己怎么被安装」）

| 场景 | 谁写分区 | 落地路径 |
|---|---|---|
| DirectInstall / DirectInstallToInactiveSlot | **ksud 自己**（root 下 `open` + `BLKROSET` + `write_all`） | `/dev/block/by-name/boot{,a,b}` |
| SelectFile / DownloadFile | 不写分区，只出产物 | `/storage/emulated/0/Download/kernelsu_patched_<ts>.img`，用户自行 fastboot/第三方 flasher 刷 |
| 还原 stock | `ksud boot-restore -f`（管理器 `restoreBoot()` 调用） | 从 `/data/adb/ksu/ksu_backup_<sha1>` 或 ramdisk 里的 `stock_image.sha1` 反查 |
| 装/升级 daemon | `ksud install --libadbroot <apk>/lib/libadbroot.so --data-path <deviceProtectedDataDir>` | `std::fs::copy("/proc/self/exe", "/data/adb/ksud")` + `lsetfilecon` + 建 `bin/ksud` 符号链接 + 迁移暂存备份 |

```rust
// userspace/ksud/src/utils.rs:233-243
pub fn install(libadbroot: Option<PathBuf>, data_path: Option<PathBuf>) -> Result<()> {
    ensure_dir_exists(defs::ADB_DIR)?;
    let _ = std::fs::remove_file(defs::DAEMON_PATH);
    std::fs::copy(
        // 用 /proc/self/exe，不要解析 realpath，避免「执行 /data/adb/ksud install 时把自己删掉」
        "/proc/self/exe", defs::DAEMON_PATH)?;
    restorecon::lsetfilecon(defs::DAEMON_PATH, restorecon::KSU_CON)?;
    assets::ensure_binaries(false)?;
    link_ksud_to_bin()?;
    ...
}
```

### 2.5 修补产物如何生效（引导期）

`userspace/ksuinit` 是一个**不带 stdio 的 Rust 程序**，被放进 ramdisk 的 `/init`：

```rust
// userspace/ksuinit/src/main.rs
#[unsafe(no_mangle)]
pub unsafe extern "C" fn main(_argc: i32, argv: *const *const u8, envp: *const *const u8) -> i32 {
    let _ = init::init();
    unsafe { execve(cstr!("/init"), argv, envp); }
    0
}
```

```rust
// userspace/ksuinit/src/init.rs:94-125
pub fn init() -> Result<()> {
    setup_kmsg();
    let _dontdrop = prepare_mount();          // 自己 fsopen/fsmount/move_mount 挂 /proc
    unlimit_kmsg();
    if ksuinit::has_kernelsu() {
        log::info!("KernelSU may be already loaded in kernel, skip!");
    } else {
        log::info!("Loading kernelsu.ko..");
        if let Err(e) = load_module_from_path("/kernelsu.ko") { log::error!(...) }
    }
    unlink("/init")?;                          // 删掉自己
    let real_init = match access("/init.real", Access::EXISTS) {
        Ok(_) => "init.real",
        Err(_) => "/system/bin/init",
    };
    ...
}
```

即：**ramdisk `/init` = ksuinit → 加载 `/kernelsu.ko` → 删自己 → exec `/init.real`（原 init）**。官方文档亦如此描述（`website/docs/guide/module.md:502`：「LKM mode: exec ksuinit, insmod kernelsu.ko, exec stock init」）。

### 2.6 另一条链路：LKM 模式与 `boot-patch-v2`

- `ksud late-load --magica [port] --package-name <pkg>`：借 adb root 直接 `insmod` 内核模块，**完全不改 boot 分区**。管理器在 `Natives`/`AppZygotePreload` 里 fork 后 exec（`cpp/jni.cc:378-421`）：

```cpp
execl(path, "ksud", "late-load", "--magica", "5555", "--package-name", pkg, nullptr);
```

配套 `libadbroot.so` 通过 LD_PRELOAD 覆盖 `__android_log_is_debuggable` 与 `__system_property_find`，让 adbd 默认以 root 跑（`cpp/adbroot.cc:10-40`）。

- `ksud boot-patch-v2 -b boot.img -o out.img [--module ko] [--force]`：**直接把 ko 注入已解压的内核镜像**，靠 capsule(`KSULKM1`, v2) + kallsyms 恢复 + BTF + text cave 找洞（`lkm_image.rs`，`patch_boot` 在 2584 行）。这条链路**当前管理器没有调用**（全仓 `boot-patch-v2` 只出现在 `cli.rs`/`cli_non_android.rs`/`lkm_image.rs`/文档），属于 CLI/PC 能力。

### 2.7 为什么可以没有 magiskboot

`android-bootimg` 提供了 boot image 的 parse/patch（`BootImage::parse`、`BootImagePatchOption::{replace_kernel, replace_ramdisk, replace_vendor_ramdisk, override_cmdline, patch}`）与 `Cpio`（`Cpio::load_from_data` / `add` / `mv` / `rm` / `dump` / `is_magisk_patched` / `exists`）。KernelSU 自研/依赖的这部分能力覆盖了 magiskboot 的 `unpack`/`repack`，因此：
- 不引入 GPL 外部二进制，不必为用户设备准备多 ABI 的 `magiskboot`；
- 可以 `mmap` 分区直读直写，省一次全量拷贝；
- 同一份代码在 PC（`cli_non_android.rs`）与手机共用。

**未确认**：`android-bootimg` 对 `vendor_boot` v4 多 ramdisk 片段、`init_boot` 的写回细节是否与 magiskboot 完全字节等价——未做逐字节比对实验。

---

## 3. root 授权模型

### 3.1 三层结构

```
┌ 用户态 ──────────────────────────────────────────────┐
│ 管理器 SuperUser 页（app 列表 + allowSu 开关）        │
│   ├─ KsuService (libsu RootService, 以 root 枚举包)   │
│   └─ Natives.setAppProfile() → libkernelsu.so → ioctl │
├ 内核态 ──────────────────────────────────────────────┤
│ allowlist（持久化 /data/adb/ksu/.allowlist）           │
│ app_profile（uid/gid/groups/caps/domain/ns/rules）     │
│ sucompat（/system/bin/su → /data/adb/ksud）            │
│ sulog（审计事件环形缓冲，fd 导出）                     │
└───────────────────────────────────────────────────────┘
```

### 3.2 supercall 传输层（不是 prctl）

```rust
// userspace/ksud/src/ksucalls.rs:120-146
fn init_driver_fd() -> Option<RawFd> {
    let fd = scan_driver_fd().ok().flatten();          // 先扫 /proc/self/fd 找 anon_inode:[ksu_driver]
    if fd.is_none() {
        let mut fd = -1;
        with_svc_call(|| unsafe {
            libc::syscall(libc::SYS_reboot,
                ksu_uapi::KSU_INSTALL_MAGIC1,           // 0xDEADBEEF
                ksu_uapi::KSU_INSTALL_MAGIC2,           // 0xCAFEBABE
                0, &mut fd)
        });
        ...
    }
}
fn ksuctl<T>(request: u32, arg: *mut T) -> Result<i32> {
    let fd = *DRIVER_FD.get_or_init(|| init_driver_fd().unwrap_or(-1));
    if fd < 0 { bail!("could not retrieve kernelsu driver fd") }
    let ret = unsafe { libc::ioctl(fd as c_int, request as i32, arg) };
    ...
}
```

UAPI 定义（`uapi/supercall.h`）：

```c
static const __u32 KERNEL_SU_UAPI_VERSION = 6;
static const __u32 KSU_INSTALL_MAGIC1 = 0xDEADBEEF;
static const __u32 KSU_INSTALL_MAGIC2 = 0xCAFEBABE;

static const __u32 KSU_IOCTL_GRANT_ROOT        = _IOC(_IOC_NONE, 'K', 1, 0);
static const __u32 KSU_IOCTL_GET_INFO          = _IOR('K', 2, struct ksu_get_info_cmd);
static const __u32 KSU_IOCTL_SET_SEPOLICY      = _IOC(_IOC_READ|_IOC_WRITE, 'K', 4, 0);
static const __u32 KSU_IOCTL_NEW_GET_ALLOW_LIST= _IOWR('K', 6, struct ksu_new_get_allow_list_cmd);
static const __u32 KSU_IOCTL_GET_APP_PROFILE   = _IOC(_IOC_READ|_IOC_WRITE, 'K', 11, 0);
static const __u32 KSU_IOCTL_SET_APP_PROFILE   = _IOC(_IOC_WRITE, 'K', 12, 0);
static const __u32 KSU_IOCTL_GET_SULOG_FD      = _IOW('K', 20, struct ksu_get_sulog_fd_cmd);
/* … 共 22 个，见源码 180-207 行 */
```

用户态必须跟踪的最小内核版本（`Natives.kt:27`）：`MINIMAL_SUPPORTED_KERNEL = 32513`，注释里逐条记录了破坏性变更（10915 allowlist/profile、12143 新 supercall、32310 新 get_allow_list、32513 引入 uapi version）。管理器用 `isFullFeatured()` = `isManager && kernelUAPIVersion == managerUAPIVersion && rootAvailable()` 做能力门禁。

### 3.3 授权判定

```c
// kernel/supercall/perm.c:27-30
bool allowed_for_su(void)
{
    return is_manager() || ksu_is_allow_uid_for_current(current_uid().val);
}
```

- `KSU_IOCTL_GRANT_ROOT` 的 `perm_check` 就是 `allowed_for_su`（`kernel/supercall/dispatch.c:812`）。
- 真正提权在该 ioctl 的 handler 里：

```c
// kernel/supercall/dispatch.c:31-43
static int do_grant_root(void __user *arg)
{
    __u32 audit_uid = current_uid().val;
    __u32 audit_euid = current_euid().val;
    pr_info("allow root for: %d\n", audit_uid);
    ret = escape_with_root_profile();          // 按 app_profile 切换到 root cred/SELinux 域
    ksu_sulog_emit_grant_root(ret, audit_uid, audit_euid, GFP_KERNEL);
    return ret;
}
```

- allowlist 持久化格式（`kernel/policy/allowlist.c:35-38,72`）：

```c
#define FILE_MAGIC 0x7f4b5355 // ' KSU', u32
#define FILE_FORMAT_VERSION 4 // u32
#define KSU_DEFAULT_SELINUX_DOMAIN "u:r:" KERNEL_SU_DOMAIN ":s0"
#define KERNEL_SU_ALLOWLIST "/data/adb/ksu/.allowlist"
```

- uid 合法性：`forbid_system_uid(uid)` = `uid < SHELL_UID(2000) && uid != SYSTEM_UID(1000)` 一律拒绝（`allowlist.c:101-105`）；管理器侧也有同样的兜底（`AppProfileScreen.kt:118-120`：`uid < 2000 && uid != 1000` → 提示 `suNotAllowed`）。
- root/非 root 两套默认 profile 缓存在内核（`init_default_profiles()`：root 拿 `CAP_FULL_SET`、`KSU_NS_INHERITED`、域 `u:r:ksu:s0`；非 root 默认 `umount_modules = true`）。

### 3.4 管理器身份认证（防山寨管理器）

内核不靠包名认管理器，而是**校验 APK 的 v2 签名块**：

```c
// kernel/manager/apk_sign.c:326-329
bool is_manager_apk(char *path)
{
    return check_v2_signature(path, EXPECTED_SIZE, EXPECTED_HASH);
}
```

```make
# kernel/Kbuild:177-187
KSU_EXPECTED_SIZE ?= 0x033b
KSU_EXPECTED_HASH ?= c371061b19d8c7d7d6133c6a9bafe198fa944e50c1b31c9d8daa8d7f1fc2d2d6
ccflags-y += -DEXPECTED_SIZE=$(KSU_EXPECTED_SIZE)
ccflags-y += -DEXPECTED_HASH=\"$(KSU_EXPECTED_HASH)\"
```

`throne_tracker.c` 扫描 `/data/app/*/base.apk`，命中签名后 `crown_manager()` 把该包 uid 设为 `ksu_manager_appid`（`throne_tracker.c:21,34-45`），并监听包变化事件重新加冕/失效。`is_manager()` = `ksu_manager_appid == current_uid().val % 100000`。

**这意味着**：换签名编译管理器后，内核里的 `EXPECTED_SIZE/HASH` 必须同步重编，否则管理器不被认作 manager —— 对 SP 这类要频繁改签名的项目是一个必须提前设计的点（见 §5.2）。

### 3.5 授权操作路径（**没有交互式弹窗**，这点很关键）

- 内核侧**没有** pending 请求队列：未授权 uid 执行 `su` 走 `sucompat`，只做审计（`ksu_sulog_emit_pending`），不弹窗、不阻塞等待用户点「允许」。
- 管理器的「超级用户」页是**应用清单 + 开关**，不是「待批准列表」：

```kotlin
// ui/screen/appprofile/AppProfileScreen.kt:110-135（节选）
onProfileChange = { updatedProfile ->
    scope.launch {
        val profileToSave = if (primaryAppInfo.special) updatedProfile.copy(allowSu = false) else updatedProfile
        if (profileToSave.allowSu) {
            if (uid < 2000 && uid != 1000) { showMessage(suNotAllowed); return@launch }
            if (!profileToSave.rootUseDefault && profileToSave.rules.isNotEmpty() && !setSepolicy(...)) {
                showMessage(failToUpdateSepolicy); return@launch
            }
        }
        if (!Natives.setAppProfile(profileToSave)) showMessage(failToUpdateAppProfile)
        else profile = profileToSave
    }
}
```

- 应用清单本身**必须用 root 才能枚举**（普通应用看不到其它包的 `FLAG_HAS_CODE` 全量列表），所以用一个 libsu `RootService`：

```kotlin
// ui/KsuService.kt:18-26
class KsuService : RootService() {
    override fun onBind(intent: Intent): IBinder = Stub()
    ...
}
```
```kotlin
// data/repository/SuperUserRepositoryImpl.kt:65-90（节选）
val idsArray = iface.userIds
val slice = iface.getPackages(0)          // ← 以 root 身份 PackageManager 枚举全用户包
...
val profile = Natives.getAppProfile(it.packageName, appInfo.uid)   // ← 每个包再问内核要 profile
```

AIDL 极简（`aidl/me/weishu/kernelsu/IKsuInterface.aidl`）：

```aidl
interface IKsuInterface {
    ParcelableListSlice<PackageInfo> getPackages(int flags);
    int[] getUserIds();
}
```

- 审计：`sulog` 事件类型仅三种——`ROOT_EXECVE=1`、`SUCOMPAT=2`、`IOCTL_GRANT_ROOT=3`（`uapi/sulog.h`）。管理器 `SulogScreen` 通过 `KSU_IOCTL_GET_SULOG_FD` 读事件流展示，**不参与授权决策**。

### 3.6 管理器的 root shell 从哪来

```kotlin
// ui/util/KsuCli.kt:83-105
fun createRootShell(globalMnt: Boolean = false): Shell {
    val builder = Shell.Builder.create()
    return try {
        if (globalMnt) builder.build(getKsuDaemonPath(), "debug", "su", "-g")
        else builder.build(getKsuDaemonPath(), "debug", "su")
    } catch (e: Throwable) {
        try {
            if (globalMnt) builder.build("su", "-mm") else builder.build("su")
        } catch (e: Throwable) {
            builder.build("sh")        // ← 静默降级成非 root（见 §5.2）
        }
    }
}
```

`ksud debug su` → `su::grant_root()` → `ksucalls::grant_root()`（ioctl `GRANT_ROOT`）→ `exec("sh")`：

```rust
// userspace/ksud/src/su.rs:23-36
pub fn grant_root(global_mnt: bool) -> Result<()> {
    crate::ksucalls::grant_root()?;
    let mut command = Command::new("sh");
    let command = unsafe { command.pre_exec(move || { if global_mnt { let _ = utils::switch_mnt_ns(1); } Ok(()) }) };
    add_path_to_env(defs::BINARY_DIR)?;
    Err(command.exec().into())
}
```

对普通 App 则是路径劫持：内核 hook `execveat`/`faccessat`/`stat`，把 `/system/bin/su` 换成 `/data/adb/ksud`（`kernel/feature/sucompat.c:31,69,94-100`，`#define SU_PATH "/system/bin/su"`）。

---

## 4. 模块管理

### 4.1 目录布局（`userspace/ksud/src/defs.rs`，全部为源码常量）

```
/data/adb/
├── ksud                          DAEMON_PATH
├── modules/<id>/                 MODULE_DIR      生效模块
├── modules_update/<id>/          MODULE_UPDATE_DIR  安装暂存
├── metamodule/                   METAMODULE_DIR  元模块（单实例）
├── post-fs-data.d/ services.d/ boot-completed.d/  通用脚本（*.d，可执行位必需）
└── ksu/                          WORKING_DIR
    ├── bin/                      BINARY_DIR  (busybox, bootctl, ksud 符号链接, resetprop→/data/adb/ksud)
    ├── lib/                      LIBRARY_DIR (libadbroot.so)
    ├── log/                      LOG_DIR
    ├── profile/{selinux,templates}  PROFILE_*
    ├── module_configs/<id>/      persist.config / tmp.config
    ├── .ksurc                    KSURC_PATH
    ├── preinit_status            PREINIT_STATUS_PATH
    ├── hot_install_trash/        HOT_INSTALL_TRASH_DIR
    └── ksu_backup_<sha1>         KSU_BACKUP_FILE_PREFIX + sha1
```

模块内约定名（同文件）：`webroot/`（WebUI）、`action.sh`、`disable`、`update`、`remove`、`initrc/`、`preinit/`+`preinit.conf`、`.ksu_hot_install`（热安装标记）、`sepolicy.rule`、`system.prop`、metamodule 的 `metamount.sh`/`metainstall.sh`/`metauninstall.sh`。preinit 会落到 `/metadata/vold/ksu/preinit/<id>/` 等三个候选目录（`PREINIT_DIR_VOLD/WATCHDOG/DEFAULT`）。

### 4.2 安装流程（`userspace/ksud/src/module.rs:686-841`，`install_module_to_system`）

1. `ensure_boot_completed()`：`getprop sys.boot_completed == "1"`，否则 `bail!("Android is Booting!")`。
2. `assets::ensure_binaries(false)`：确保 `/data/adb/ksu/bin/` 下 busybox 等就位，并重建 `resetprop -> /data/adb/ksud` 符号链接（resetprop 已并入 ksud）。
3. 读 zip 内 `module.prop` → 取 `id` → `validate_module_id()`（与管理器侧 `MODULE_ID_REGEX = ^[a-zA-Z][a-zA-Z0-9._-]+$` 同规则，防路径穿越）。
4. metamodule 判定与安全检查：`metamodule::is_metamodule` / `check_install_safety`（已有 metamodule 带自定义 installer 且状态未收敛时**拒绝安装普通模块**）。
5. 目标目录固定为 **`modules_update/<id>`**（注释：「All modules (including metamodules) are installed to MODULE_UPDATE_DIR」），`ensure_clean_dir` + `setsyscon`。
6. 解压 zip；**删掉 zip 里自带的 `.ksu_hot_install`**（注释：「Only installer.sh may request a hot install」）→ 防 zip 伪造热安装。
7. `system/` 目录设 `0755` + `restore_syscon`。
8. `exec_install_script()`：`busybox sh -c <installer.sh>`，环境变量 `OUTFD=1`、`ZIPFILE=<zip realpath>`、`KSU_MODULE=<id>`。
9. 若 installer 请求热安装 → `hot_install::apply`（可不重启生效）；否则把暂存目录的 `module.prop` 复制到 `modules/<id>/` 并 `touch update` 标志。
10. metamodule 额外建符号链接；最后 `regenerate_preinit()`。

### 4.3 脚本执行环境（`module.rs:63-95, 278-364`）

```rust
// 公共环境变量
("ASH_STANDALONE", "1")
("KSU", "true")
("KSU_KERNEL_VER_CODE", ksucalls::get_version())
("KSU_VER_CODE", defs::VERSION_CODE)
("KSU_VER", defs::VERSION_NAME)
("KSU_UAPI_VER", ksucalls::uapi_version())
("KSU_RUNTIME_MODE", ksucalls::runtime_mode())
("PATH", "<原 PATH>:/data/adb/ksu/bin")
("KSU_MODULE", id)          // 仅模块脚本
("KSU_LATE_LOAD", "1")      // late-load 模式下
```

```rust
// 执行方式：busybox sh <script>，cwd=脚本所在目录，pre_exec 里 detach 进程组 + switch_cgroups
let mut command = Command::new(assets::BUSYBOX_PATH);
command.current_dir(path.parent().unwrap()).arg("sh").arg(path)
       .envs(get_common_script_envs(validated_module_id));
```

等待策略三档：`ScriptWait::{NoWait, Forever, Until(deadline)}`——开机阶段共用一个 deadline，超时就不再等（`defs::BOOT_STAGE_TIMEOUT = 35s`），避免一个卡死脚本拖垮开机。

### 4.4 生命周期

| 时机 | 入口 | 内容（`init_event.rs`） |
|---|---|---|
| post-fs-data | `ksud post-fs-data` | 安全模式检查（`is_safe_mode` → 禁用全部模块）；`/data/adb/post-fs-data.d/` 通用脚本；metamodule 的 `post-fs-data.sh` **优先**；各模块 `post-fs-data.sh`；`load_sepolicy_rule()`；`load_system_prop()` |
| services | `ksud services` | 各模块 `service.sh` |
| boot-completed | `ksud boot-completed` | `run_stage("boot-completed", NoWait)` → `boot-completed.sh` |

另外：`exec_stage_script` 会**跳过 metamodule 自身目录**（避免重复执行）；`foreach_module(ModuleType::Active)` 跳过带 `disable`/`remove` 的模块。

### 4.5 热安装与元模块

- **热安装**（`hot_install.rs` 头部注释即规格）：模块在 `customize.sh` 里设 `MODULE_HOT_INSTALL_REQUEST=true`（可选 `MODULE_HOT_RUN_SCRIPT=<script>`），installer.sh 把请求写进暂存目录的标记文件，安装成功后 ksud 把暂存目录 rename 覆盖活的目录、apply `sepolicy.rule`、跑指定脚本；「只影响开机才发生的事（挂 system/、system.prop、post-fs-data.sh）仍等下次重启」。
- **元模块**：同时只允许一个；可接管安装脚本（`metainstall.sh`）与挂载（`metamount.sh`）；安装别的模块前要过 `check_install_safety`。

### 4.6 与 APatch 的 apd 对比

参考基线：本仓 `APatch/`（apd，Rust）与既有文档 `docs/phase1_ap_apd_analysis_and_sk_adaptation.md`（APatch `e8e4651c`）。APatch 侧的完整调研（含修补链、superkey、KPM）见 `docs/research/apatch-manager-architecture.md` §4；本节只做模块维度的速查。

| 维度 | KernelSU (`ksud`) | APatch (`apd`) | 评价 |
|---|---|---|---|
| 语言/部署 | Rust，`/data/adb/ksud` | Rust，`/data/adb/apd` | 同构 |
| APK 内形态 | `libksud.so`（jniLibs，可直接 exec） | `libapd.so`（同法） | **同一套 trick**，SP 可直接沿用 |
| 工作目录 | `/data/adb/ksu/` | `/data/adb/ap/` | 仅前缀不同 |
| 模块目录 | `/data/adb/modules` + `modules_update` | 完全相同 | 与 Magisk 生态兼容 |
| multicall | `resetprop` → 符号链接指向 `/data/adb/ksud` | `resetprop`、`magiskpolicy` → `/data/adb/apd` | KSU 少一个 magiskpolicy（其 sepolicy 走 `ksud sepolicy` 子命令） |
| installer | 内置 `installer.sh`（Magisk 风格） | 内置 `installer.sh`（Magisk 风格） | 语义一致 |
| 脚本 env | `KSU`/`KSU_VER*`/`KSU_MODULE`/`ASH_STANDALONE=1` | `APATCH`/`APATCH_VER*`/… | 语义一致，变量名不同 |
| 生效时机 | 暂存 `modules_update` + `update` 标志，下次 post-fs-data 搬运 | 同 | 一致 |
| 特有 | `metamodule`（单实例）、`preinit`（metadata 落盘，内核侧 runner）、`hot_install`、`module_configs` | `metamodule`、`KPM` 模块、`su_path` 注册文件 | SP 若要多方兼容，需同时实现两套「元模块/扩展」语义 |
| 安全校验 | zip 内 `.ksu_hot_install` 一律删除；`module_id` 正则校验；`verify` 下载完整性在 APK 侧 | 同样正则校验 id；下载确认对话框 | KSU 的「安装器之外不许声明热安装」值得抄 |

---

## 5. 对新项目（SP 管理器）的可借鉴清单

### 5.1 可以直接借鉴的 5 个设计点

**① 「薄 UI + 厚 CLI」的进程边界，且 CLI 以 `lib*.so` 形式塞进 APK**

KernelSU = Compose 壳 + Rust CLI，二者只通过 **argv + stdout/stderr** 通信；CLI 既能在手机上跑，也能在 PC 上跑（`cli_non_android.rs` 共用 `boot_patch.rs`）。对 SP 的价值：
- 修补引擎（`skroot-patch`）已经是独立 CLI 契约（见 `docs/SPROOT-DOC-C1-修补引擎IO契约.md`），**让 APK 只做参数拼装 + 日志流**，不要在 Kotlin 里重写一遍引擎；
- 用 `jniLibs/libskroot.so` 打包，运行时用 `applicationInfo.nativeLibraryDir` 拼绝对路径 exec；好处是 `useLegacyPackaging = true` 后免解压、可直接执行、签名随 APK 一起被校验；
- 代价：必须有「安装/升级/迁移」状态机（见 §5.2 第 3 点）。

**② 修补命令「全参数化、零交互、可复现」**

KernelSU 把所有决策外化成显式 flag：`-b` 输入、`-o` 产物目录、`--out-name` 命名、`-f` 直刷、`-u` 另一 slot、`--partition` 目标分区、`--kmi`、`-m` 自定义 ko、`--backup` 强制备份、`--allow-shell`/`--enable-adbd`/`--adb-debug-prop` 调试面、`--no-install` 只改配置。SP 的 `skroot-patch` 契约已经是这个风格（`--in/--out/--gen-key/--key/--json`），建议再补三类 KernelSU 已有而 SP 目前缺的参数：
- `--partition <boot|init_boot|vendor_boot>`（目标分区覆盖，避免猜错）；
- `--backup`（强制把 stock 备份落到约定位置，而不是只依赖 UI 层先手动备份 `ori.img`）；
- `--no-install`（只改引导参数、不重新注入，用于「换密钥但保持已修补」之类的运维场景）。

**③ 自研/内嵌 bootimg 解析，而不是外挂 `magiskboot`/`kptools`**

KernelSU 证明了「纯 Rust + `android-bootimg`」足以完成 parse → cpio 编辑 → repack → 写分区，且能：
- `mmap` 分区设备直接解析（`map_file`），省一次全量读；
- 在回包后 `drop` mmap 再写，规避「部分内核拒绝写已映射的块设备」这一真实坑（`boot_patch.rs:907-916`）；
- 写分区前用 `BLKROSET` 主动解除只读（比依赖 `blockdev --setrw` 少一个外部依赖）。
SP 现状：修补引擎只管 kernel Image，boot.img 的解包/回包仍要靠 magiskboot/kptools（C1 §0 明确排除）。建议**把 bootimg 组件作为独立 crate 引入 engine/**，与此同时也解决 `ori.img` 备份/还原的一致性。

**④ 备份用「内容寻址 + 引导内指纹」而非单份 `ori.img`**

KernelSU：`do_backup` 算 sha1 → 存 `/data/adb/ksu/ksu_backup_<sha1>`，同时把 sha1 写进 ramdisk 的 `stock_image.sha1`。带来两个好处：
- 还原时能校验「要还原的备份确实是这台设备当初的 stock」；
- 多次 OTA/多次修补不会互相覆盖，可按 sha1 清理（`clean_backup`）。
SP 目前是单份 `ori.img`（APatch 风格）。建议至少保留 `ori.img` 兼容，同时增加 sha1 索引与「备份是否匹配当前分区」校验。

**⑤ 页面骨架：`UiState` + `Utils` + 双皮（Material/Miuix）+ type-safe 路由 + WebUI 扩展点**

- 每个页面拆「状态 / 纯函数 / 渲染」三层，渲染层再分 Material 与 Miuix——对同时覆盖 AOSP 与 HyperOS 的 SP 是现成模板；
- `Routes.kt` 用 `@Serializable sealed interface + NavKey`，参数直接是数据类，无需手工拼 route string；
- **模块自带 WebUI**（`webroot/index.html` + 管理器注入的 JS API）是「不想写 Compose 的模块作者」的逃生通道：管理器侧 `WebUIActivity` + `WebViewAssetLoader` + `SuFilePathHandler`（用 libsu `SuFileInputStream` 读 root 文件）即可，SP 若要支持 KPM 模块的图形界面，这是成本最低的方案。

### 5.2 必须避开 / 必须改造的 3 个点

**① 不要照抄「无交互预授权」模型（必须改造）**

KernelSU 的 su 授权是「用户在应用清单里预先打开开关」，内核**没有 pending 请求队列**，未授权请求直接拒绝、只写审计：

```c
bool allowed_for_su(void) { return is_manager() || ksu_is_allow_uid_for_current(current_uid().val); }
```

后果（真实体验问题）：新装的 root 应用第一次 `su` 只会失败，用户必须自己进管理器翻列表找它、手动打开。SP 若要「应用请求 → 用户批准」的 Magisk 式流程，**内核侧必须新增**：pending 队列（uid/pid/包名/命令行/时间戳 + 一次性 token）、把 pending 暴露给管理器的 ioctl/事件流、以及「超时自动拒绝 + 防重放」。KernelSU 的 `sulog`（`KSU_SULOG_EVENT_{ROOT_EXECVE,SUCOMPAT,IOCTL_GRANT_ROOT}` + `KSU_IOCTL_GET_SULOG_FD`）只能当**审计证据**，不能当授权决策通道——别误以为它已经够用。

**② 不要把 `nativeLibraryDir/lib*.so` 当作长期守护进程路径（必须改造）**

KernelSU 自己在 `getKsuDaemonPath()` 里直接用 `nativeLibraryDir + "/libksud.so"`，只在 `ksud install` 时才把 `/proc/self/exe` 复制成 `/data/adb/ksud`。这带来一串脆弱点：
- APK 升级/降级后 `nativeLibraryDir` 路径变化，之前写进 `init.rc`/脚本/备份里的路径全部失效；
- 用户「清除数据」或系统清理 native lib 后，CLI 消失，管理器会静默降级（`createRootShell` 的 catch 分支会退到 `su`，再退到 `sh`，见 `KsuCli.kt:99-104`）——表现为「界面还在但什么都干不了」，很难排查；
- 因此 KernelSU 才需要 `install()` 在直装成功后立刻把 daemon 拷到 `/data/adb/`，并且 `bin/ksud` 再建符号链接。
SP 应当**显式建模三态**（bundled / installed / stale）并在 UI 上暴露，而不是靠 `try/catch` 猜；同时给 daemon 一个稳定路径（如 `/data/adb/skroot/`）+ 版本/哈希校验 + 原子替换（rename）。

**③ 不要把 `boot-patch-v2`（kernel image 内注入 capsule）当首版目标（必须避开）**

`lkm_image.rs` 有 116 KB，包含：kallsyms marker 搜索（`KALLSYMS_MARKER_SEARCH_WINDOW 4MB`、256 token 表）、BTF 解析（`lkm_image_btf.rs` 24 KB）、ELF reloc fixup（`R_AARCH64_{ABS64,ABS32,JUMP26,CALL26,ADR_PREL_PG_HI21,ADD_ABS_LO12_NC}`）、text cave 搜索、memblock reserve 调用点定位（`lkm_image_bootstrap.S`）。它强绑定 KMI 与内核布局，任一环节猜错就是 bootloop，且**当前版本管理器自己都没接**（无任何 Kotlin 调用点）。
SP 的首版应锁定「ramdisk 方案」（对应 `boot-patch` v1）：改 `/init` + 放 ko + 写配置，失败可 `boot-restore`/刷回 stock。把 kernel image 注入留作二期，并且必须有「自动回滚（bootcount 超限刷回 stock）」兜底。

---

## 6. 证据

### 6.1 关键文件清单（相对仓库根）

| 主题 | 文件 | 关键行 |
|---|---|---|
| 构建/打包 ksud | `justfile` | 1-8 |
| APK 打包配置 | `manager/app/build.gradle.kts` | 115-119（jniLibs）、abiFilters |
| SDK/版本 | `manager/build.gradle.kts` | 8-18 |
| 依赖版本 | `manager/gradle/libs.versions.toml` | 全文件 |
| CLI 门面 | `manager/app/src/main/java/me/weishu/kernelsu/ui/util/KsuCli.kt` | 44-46、83-105、137-142、189-210、212-250、324-330、359-436、438-533、565-600 |
| 安装向导 | `.../ui/screen/install/InstallScreen.kt` | 96-107 |
| 刷写进度 | `.../ui/screen/flash/FlashScreen.kt`、`FlashUtils.kt` | 46-52（`FlashingStatus`） |
| JNI 门面 | `.../Natives.kt` | 23、42-44、59-135、145-180 |
| JNI 实现 | `manager/app/src/main/cpp/jni.cc` | 41-44、159-330、378-421 |
| ioctl 封装 | `manager/app/src/main/cpp/ksu.cc` | 104-170 |
| adb root 劫持 | `manager/app/src/main/cpp/adbroot.cc` | 10-45 |
| root binder 服务 | `.../ui/KsuService.kt`、`aidl/.../IKsuInterface.aidl` | 16-60 |
| 修补主逻辑 | `userspace/ksud/src/boot_patch.rs` | 203-220（写分区）、159-179（备份）、332-385（KMI）、410-419（版本校验）、421-514（参数）、627-947（patch） |
| 资源嵌入 | `userspace/ksud/src/assets.rs` | 全文件 |
| UAPI | `uapi/supercall.h`、`uapi/sulog.h`、`uapi/app_profile.h` | 全文件 |
| supercall 传输 | `userspace/ksud/src/ksucalls.rs` | 80-165 |
| 授权判定 | `kernel/supercall/perm.c`、`kernel/supercall/dispatch.c` | 27-30、31-43、809-813 |
| allowlist | `kernel/policy/allowlist.c` | 35-38、72、96-101 |
| 管理器身份 | `kernel/manager/apk_sign.c`、`throne_tracker.c`、`kernel/Kbuild` | 326-329、21-45、177-187 |
| su 路径劫持 | `kernel/feature/sucompat.c` | 31、69、94-100 |
| 首阶段 init | `userspace/ksuinit/src/main.rs`、`init.rs` | 全文件（init.rs 98-125） |
| 模块管理 | `userspace/ksud/src/module.rs` | 63-95、97-113、273-364、686-841 |
| 模块常量 | `userspace/ksud/src/defs.rs` | android 模块全部常量 |
| 生命周期 | `userspace/ksud/src/init_event.rs` | 11-190 |
| 热安装 | `userspace/ksud/src/hot_install.rs` | 1-40 |
| 内核镜像注入（v2） | `userspace/ksud/src/lkm_image.rs` | 1-110、2571-2660 |
| 官方文档 | `website/docs/guide/{installation,module,module-webui,module-script}.md` | — |

### 6.2 关键代码摘录（命令级）

**A. 管理器 → ksud 的真实 argv（SelectFile + GKI）**

```
/data/user/0/me.weishu.kernelsu/app/lib/arm64/libksud.so \
  boot-patch \
  -b /data/user/0/me.weishu.kernelsu/cache/boot.img \
  -o /storage/emulated/0/Download \
  [--partition boot|init_boot|vendor_boot] \
  [--kmi android13-5.15] [-m /data/user/0/.../cache/kernelsu-tmp-lkm.ko] \
  [--allow-shell] [--enable-adbd] [--backup]
```
（`KsuCli.kt:369-436`；`-o` 的目标目录来自 `Environment.getExternalStoragePublicDirectory(DIRECTORY_DOWNLOADS)`）

**B. 管理器 → ksud 的真实 argv（DirectInstall，写分区）**

```
…/libksud.so boot-patch -f [--partition …] [--allow-shell] [--enable-adbd] [--backup]
```
（`-f` 时 ksud 自行 `auto_boot_partition_path()` 找到 `/dev/block/by-name/boot{slot}`，写回分区；成功后管理器再跑一次 `…/libksud.so install --libadbroot <apk>/lib/libadbroot.so --data-path <deviceProtectedDataDir>`）

**C. 管理器 root shell**

```
…/libksud.so debug su            # 普通
…/libksud.so debug su -g         # 全局 mount namespace
fallback: su / su -mm / sh
```

**D. 内核调用的最小集合（UAPI）**

```c
KSU_IOCTL_GET_INFO / GET_INFO_LEGACY      // 版本、flags(LKM/MANAGER/LATE_LOAD/BUNDLED/PREINIT)、uapi
KSU_IOCTL_GRANT_ROOT                      // perm_check = allowed_for_su
KSU_IOCTL_GET_ALLOW_LIST / NEW_GET_*      // uid 名单
KSU_IOCTL_GET_APP_PROFILE / SET_APP_PROFILE
KSU_IOCTL_SET_SEPOLICY                    // 序列化 sepolicy 命令流
KSU_IOCTL_GET_FEATURE / SET_FEATURE       // 能力开关（内核参数级）
KSU_IOCTL_MANAGE_MARK                     // 进程标记（prctl 时代的 mark 迁移）
KSU_IOCTL_GET_SULOG_FD                    // 审计事件 fd
KSU_IOCTL_GET_WRAPPER_FD / LOOKUP_SYMBOLS / DISABLE_ESCAPE_TO_ROOT / SET_INIT_PGRP
```

### 6.3 复现方式

```bash
git clone --depth 1 https://github.com/tiann/KernelSU /tmp/ksugit
cd /tmp/ksugit && git log -1 --format='%H %ci %s'
git rev-parse HEAD   # 730f644615969cfd4a84b45d7f62a667b7b95a50
```

关键验证命令（可在本仓复跑）：

```bash
# 1) APK 里到底有哪些 native 件 / 有没有 magiskboot
find manager/app/src/main/jniLibs -type f; find userspace/ksud/bin -type f
# 2) 谁在拼 boot-patch 命令
grep -n "boot-patch\|getKsuDaemonPath\|installBoot\|downloadBoot" -r manager/app/src/main/java
# 3) 有没有 boot-patch-v2 的管理器调用点
grep -rn "boot-patch-v2\|BootPatchV2" manager/ || echo "none in manager"
# 4) 授权判定
grep -rn "allowed_for_su" kernel/supercall/perm.c kernel/supercall/dispatch.c
```

### 6.4 明确标注「未确认」的三处

1. **`android-bootimg` 与 magiskboot 的字节级等价性**：未做逐字节对比实验；仅从源码看它覆盖了 parse/repack 的主要路径，且 `extract_ramdisk` 注释声称遵循 magiskboot 的查找顺序。
2. **`boot-patch-v2` 在真实设备上的成功率/兼容矩阵**：本 commit 无管理器接入、无测试矩阵，仓库内也未找到对应测试用例。
3. **管理器 `install()` 与 `boot-patch -f` 的执行顺序在异常情况下的容错**：代码上 `-f` 成功后立即 `install()`，若此时断电/被杀，`/data/adb/ksud` 可能是旧版本（`install()` 内部有 `remove_file` 再 `copy`，非原子）。源码未体现对此的恢复逻辑，实际影响未验证。
