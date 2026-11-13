# APatch 管理器架构与「手机端本地修补 boot 镜像」全链路调研

- 调研对象：`bmax121/APatch`（本地工作副本 `APatch/`）
- 上游基线：**`e8e4651c16fb5075882a25813c8732285fdb3505`**（main，`2026-10-13 00:44:30 +0800`，`feat(app): confirm module installs and allow cancelling downloads`）
- 配套文档：`docs/research/kernelsu-architecture.md`（同结构调研 KernelSU，用于横向对比）
- 既有内部文档（本文做了源码级复核与修正）：`docs/AP管理器与镜像修补链分析.md`、`docs/phase1_ap_apd_analysis_and_sk_adaptation.md`
- 下文 `path:line` 均相对 `APatch/` 根；凡源码无法证实的显式标「未确认」

---

## 0. 结论摘要

1. **APatch 的修补链不是「管理器自己实现」，也不是「apd 实现」**，而是三家分工：管理器（Kotlin）负责**准备目录 + 拼命令 + 收日志**，`kptools`（KernelPatch 的**预编译二进制**）负责 **bootimg unpack/repack + kernel patch 注入**，`kpimg`（预编译内核补丁镜像）是被注入的 payload。`apd` 在这条链里**完全不参与**——它只管模块/root/属性，修补全程是 root shell 里的 shell 脚本。
2. **APatch 的修补工具是「构建时从 GitHub Releases 下载的产物」**：`app/build.gradle.kts:238-259`（`downloadKpimg`/`downloadKptools`/`downloadCompatKpatch`）从 `bmax121/KernelPatch` releases 拉 `kpimg-android`、`kptools-android`、`kpatch-android`(0.10.7 兼容版)，落到 `app/src/main/assets/kpimg` 与 `app/libs/arm64-v8a/libkptools.so`；另外按 KMI 下载 7 份 `kernelpatch.ko` 供 jailbreak（`downloadJailbreakKo`，268 行起）。也就是说 —— **不可从本仓源码复现 kptools 的行为**（KernelPatch 是另一个仓库）。
3. **修补的执行体是 `assets/` 里的 shell 脚本**：`boot_patch.sh`（本地修补/直刷）、`boot_unpatch.sh`（还原）、`boot_extract.sh`（定位当前 boot 分区）、`util_functions.sh`（Magisk 风格工具函数）、`InstallAP.sh`/`UninstallAP.sh`（Recovery 刷机包路径，`dd` 写分区）。
4. **命令级链路**：`prepare()` 把 `nativeLibraryDir` 里的 `libkptools.so`/`libbusybox.so`/`libkpatch.so`/`libbootctl.so` **软链成无前缀名**（`kptools`/`busybox`/…）到 `files/patch/`，再把 assets 里 5 个文件写进去 → `./busybox sh boot_patch.sh <superkey> <boot.img> [true]` → 脚本内 `./kptools unpack` → 检查 `CONFIG_KALLSYMS` → `cp boot.img ori.img`（仅首次）→ `mv kernel kernel.ori` → `./kptools -p -i kernel.ori [-S <key>] -k kpimg -o kernel` → `./kptools repack` → 需要时 `flash_image new-boot.img <分区>`。
5. **root 鉴权正在从 SuperKey 迁移到「签名授权」**：`resolveSuperKey()` 先试 `Natives.nativeReady("su")`——把字面量 `"su"` 当 key 传给内核，若通过说明内核已按 **APK 签名**授权管理器；此时清掉历史 SuperKey。旧内核才回退用 Keystore 加密保存的 SuperKey。**这正是 SPRoot「SuperKey 移除方案」需要参考的迁移路径**（见 `docs/SuperKey移除方案-草案.md`）。
6. **root shell 靠「superkey 作为 argv 传给 `/system/bin/truncate`」实现**：`createRootShell()` = `builder.build("/system/bin/truncate", superKey, "-Z", "u:r:magisk:s0")`；失败才回退 `libkpatch.so <key> su -Z …` → `su` → `sh`。这是 APatch 最有特色也最脆弱的一环（见 §5.2）。
7. **授权模型是「内核持有 uid 名单 + 管理器持有一份 CSV 镜像」**：内核侧 `Natives.suUids()` / `suProfile(uid)` / `grantSu(uid,0,scontext)` / `revokeSu(uid)` / `setUidExclude`；管理器另存 `/data/adb/ap/package_config`（CSV：`pkg,exclude,allow,uid,to_uid,sctx`），双写以支持离线展示与迁移。
8. **模块管理（APM）由 `apd` 承担，与 KernelSU 的 ksud 几乎逐行同构**：`/data/adb/modules`、`/data/adb/modules_update`、`module.prop`、`customize.sh`、`post-fs-data.sh`/`service.sh`/`boot-completed.sh`、`disable/remove/update`、metamodule、module_configs。
9. **APatch 独有 `KPM`（Kernel Patch Module）**：内核态可加载模块，持久化在 `/data/adb/ap/kpm/<id>/<id>.kpm`，`disable` 文件控制，运行态信息从内核读（`loadKernelPatchModule`/`kernelPatchModuleInfo`/`kernelPatchModuleControl`），是 APatch 相对 KernelSU 的差异化能力。
10. 对 SPRoot：**最该学的是「状态机 + 双轨密钥迁移 + 备份/ori.img 闭环」；最该改的是「预编译外部工具 + shell 脚本拼装 + superkey 命令行」这三件套**。

### 组件职责一览

| 组件 | 位置 | 语言 | 职责 |
|---|---|---|---|
| 管理器 App | `app/` | Kotlin / Compose | 页面、模式选择、命令拼装、日志、授权开关、KPM/APM UI、WebUI 宿主 |
| `apd` | `apd/` | Rust | 特权 CLI：模块管理、事件、resetprop、sepolicy、uid-listener、late-load |
| `libkptools.so` | 构建时下载 → `app/libs/arm64-v8a/` | 预编译 ELF | bootimg unpack/repack + kernel patch 注入 + `-l` 信息查询 |
| `kpimg` | 构建时下载 → `app/src/main/assets/` | 预编译二进制 | KernelPatch 内核补丁 payload |
| `libkpatch.so` | 构建时下载（0.10.7） | 预编译 ELF | 旧内核兼容的 su 入口 |
| `libbootctl.so` | **直接提交在仓库里**（`app/libs/arm64-v8a/libbootctl.so`，154 KB） | 预编译 ELF | boot HAL 交互、切换 slot、mark-boot-successful |
| `libbusybox.so` | **直接提交在仓库里**（1.7 MB） | 预编译 ELF | 脚本执行解释器（`ASH_STANDALONE=1`） |
| `libapjni.so` | `app/src/main/cpp/` | C++ (JNI) | supercall 封装（KernelPatch uapi） |
| 修补脚本 | `app/src/main/assets/*.sh` | Shell | 修补/还原/定位分区的全部编排逻辑 |
| Recovery 包脚本 | `scripts/update_binary.sh`、`update_script.sh` → 打包进 `resources/META-INF/...` | Shell | 卡刷路径（`dd` 写 boot） |

---

## 1. 管理器技术栈与页面结构

### 1.1 技术栈（实测 `gradle/libs.versions.toml`、`build.gradle.kts`、`app/build.gradle.kts`）

| 项 | 值 | 依据 |
|---|---|---|
| 语言 | Kotlin `2.4.10`，KSP `2.3.11` | `gradle/libs.versions.toml` |
| UI | Compose BOM `2026.08.00` + Material3 | 同上 |
| 导航 | **compose-destinations `2.3.0`**（KSP 生成 `*Destination`，`@Destination` 注解） | 同上；`ui/screen/*.kt` 顶部 `@Destination` |
| root 交互 | libsu `6.0.0`（`core` / `service` / `nio`） | 同上 |
| 图片 | coil3 `3.5.0` | 同上 |
| WebView | androidx.webkit `1.17.0` | 同上 |
| 构建 | AGP `9.3.1`，`minSdk 26` / `targetSdk 36` / `compileSdk 37`，BuildTools `36.1.0`，NDK `29.0.14206865`，**仅 `arm64-v8a`** | `build.gradle.kts:9-15`、`app/build.gradle.kts:111-118` |
| 签名 | `org.lsposed.lsplugin.apksign` `1.4`（同 KernelSU 用法） | `app/build.gradle.kts:12` |
| 资源优化 | `org.lsposed.lsplugin.resopt` `1.6` | 同上 |
| 打包 | `packaging.jniLibs.useLegacyPackaging = true`（保证 `lib*.so` 可直接 exec） | `app/build.gradle.kts:129-133` |
| native 来源 | `android.sourceSets.main { jniLibs.directories += "libs" }` —— **`app/libs/arm64-v8a/` 才是 native 主目录**（`libapd.so` 由 `buildApd` 任务拷入，其余由下载任务放入） | `app/build.gradle.kts:159-162, 312-317` |

关键构建副作用（`preBuild` 依赖，`app/build.gradle.kts:296-302`）：

```kotlin
tasks.getByName("preBuild").dependsOn(
    "downloadKpimg", "downloadKptools", "downloadCompatKpatch", "downloadJailbreakKo", "mergeScripts",
)
```

- `downloadKpimg` → `https://github.com/bmax121/KernelPatch/releases/download/$kernelPatchVersion/kpimg-android`
- `downloadKptools` → `…/kptools-android` → `app/libs/arm64-v8a/libkptools.so`
- `downloadCompatKpatch` → `…/0.10.7/kpatch-android` → `libkpatch.so`
- `downloadJailbreakKo` → 7 个 KMI（`android12-5.10` … `android16-6.12`）的 `<kmi>_kernelpatch.ko` 进 assets

> 结论：**APatch 的「修补引擎」在本仓里是不可审计的二进制**。这点与 KernelSU（修补逻辑全部是本仓 Rust 源码）形成鲜明对比。
>
> 补齐 `app/libs/arm64-v8a/` 的四种来源（`.gitignore` 只忽略 `libkptools.so`/`libapjni.so`/`libkpatch.so`/`libapd.so`）：
> - **入仓**：`libbootctl.so`、`libbusybox.so`（`git ls-files` 可证）；
> - **下载**：`libkptools.so`、`libkpatch.so`、`assets/kpimg`、7 个 `kernelpatch.ko`；
> - **源码构建**：`libapjni.so`（CMake，`app/src/main/cpp/CMakeLists.txt`）、`libapd.so`（cargo ndk，`app/build.gradle.kts:312-317`）；
> - **已不存在但仍被引用**：`installApatch()` 最后一行执行 `${nativeDir}/libmagiskpolicy.so --magisk --live`（`APatchApp.kt:161`），但该 `.so` 既不在 `libs/`、也没有 CMake 目标或下载任务——`magiskpolicy` 现在是 **apd 的 multicall 别名**（`apd/src/assets.rs:16-18` 把 `/data/adb/ap/bin/magiskpolicy` 软链到 `/data/adb/apd`，逻辑在 `apd/src/sepolicy.rs`）。这一行属于遗留调用，失败被 libsu 忽略（`.add(*cmds)` 未检查返回值）。

### 1.2 页面结构

一级导航是 5 个底部 Tab，且**每个 Tab 声明自己依赖哪种 patch 状态**（`ui/screen/BottomBarDestination.kt`）：

```kotlin
enum class BottomBarDestination(
    val direction: DirectionDestinationSpec,
    @param:StringRes val label: Int,
    val iconSelected: ImageVector,
    val iconNotSelected: ImageVector,
    val kPatchRequired: Boolean,   // 需要内核补丁（KernelPatch）
    val aPatchRequired: Boolean,   // 需要 APatch 用户态（apd）
) {
    Home(HomeScreenDestination, R.string.home, …, false, false),
    KModule(KPModuleScreenDestination, R.string.kpm, …, true, false),
    SuperUser(SuperUserScreenDestination, R.string.su_title, …, true, false),
    AModule(APModuleScreenDestination, R.string.apm, …, false, true),
    Settings(SettingScreenDestination, R.string.settings, …, false, false),
}
```

| 一级页 | 二级/相关 | 作用 |
|---|---|---|
| Home | `Install`、`InstallModeSelect`、`Patches`、`AboutScreen` | 状态卡片（KernelPatch 状态 + apd 状态）、安装/更新/卸载、jailbreak 入口、版本信息 |
| KPM | `KPModule` + `ExecuteKPM` | 内核态模块：列表（运行态 live / 磁盘持久化）、加载/卸载、控制、在线更新 |
| SuperUser | — | uid 授权/排除、per-app profile（`to_uid` + SELinux `scontext`）、刷新 |
| AModule | `ExecuteAPMAction`、`Online` | APatch 模块（zip）安装/开关/卸载/action、在线模块源 |
| Settings | — | 全局 namespace、sucompat、selinux hide、jailbreak、密钥、主题、清理 |

`Install` 与 `InstallModeSelect` 的关系是「先选模式再进向导」（`ui/screen/InstallModeSelect.kt:129-157`）：

```kotlin
navigator.navigate(PatchesDestination(PatchesViewModel.PatchMode.PATCH_ONLY))
navigator.navigate(PatchesDestination(PatchesViewModel.PatchMode.INSTALL_TO_NEXT_SLOT))
navigator.navigate(PatchesDestination(PatchesViewModel.PatchMode.PATCH_AND_INSTALL))
```

### 1.3 应用级状态机（比 KernelSU 的布尔判断更完整）

`APatchApp.kt:47-56` 定义了 10 个状态，`Home` 与各页面据此门禁：

```kotlin
enum class State {
    UNKNOWN_STATE,
    KERNELPATCH_INSTALLED, KERNELPATCH_NEED_UPDATE, KERNELPATCH_NEED_REBOOT, KERNELPATCH_UNINSTALLING,
    ANDROIDPATCH_NOT_INSTALLED, ANDROIDPATCH_INSTALLED, ANDROIDPATCH_INSTALLING,
    ANDROIDPATCH_NEED_UPDATE, ANDROIDPATCH_UNINSTALLING,
}
```

状态由 `superKey` 的 setter 驱动（`APatchApp.kt:181-200`）：`Natives.nativeReady(value)` 决定 KP 状态，`Natives.su(0, null)` 决定 AP 状态。

---

## 2. 本地修补完整链路（命令级）

### 2.0 参与修补的文件清单

| 文件 | 来源 | 运行时位置 |
|---|---|---|
| `kptools` | `app/libs/arm64-v8a/libkptools.so`（下载产物） | `files/patch/kptools`（软链） |
| `busybox` | `libbusybox.so`（**未确认具体下载/编译任务**，`prepare()` 按名匹配） | `files/patch/busybox` |
| `kpatch` | `libkpatch.so`（0.10.7 兼容 su） | `files/patch/kpatch` |
| `bootctl` | `libbootctl.so`（**未确认来源**） | `files/patch/bootctl` |
| `kpimg` | `app/src/main/assets/kpimg`（下载产物） | `files/patch/kpimg` |
| `boot_patch.sh` / `boot_unpatch.sh` / `boot_extract.sh` / `util_functions.sh` | `app/src/main/assets/` | `files/patch/` |
| `boot.img`（用户选）或分区设备（直装） | 用户 / `boot_extract.sh` | `files/patch/boot.img` |
| `ori.img` | `boot_patch.sh` 首次修补时 `cp` | 先 `files/patch/ori.img` → 修正后搬 `/data/adb/ap/ori.img` |

`prepare()` 的全部动作（`ui/viewmodel/PatchesViewModel.kt:86-112`）：

```kotlin
private fun prepare() {
    patchDir.deleteRecursively()
    patchDir.mkdirs()
    val execs = listOf("libkptools.so", "libbusybox.so", "libkpatch.so", "libbootctl.so")
    val info = apApp.applicationInfo
    val libs = File(info.nativeLibraryDir).listFiles { _, name -> execs.contains(name) } ?: emptyArray()
    for (lib in libs) {
        val name = lib.name.substring(3, lib.name.length - 3)   // 去掉 "lib" 前缀与 ".so" 后缀
        Os.symlink(lib.path, "$patchDir/$name")
    }
    for (script in listOf("boot_patch.sh", "boot_unpatch.sh", "boot_extract.sh", "util_functions.sh", "kpimg")) {
        apApp.assets.open(script).writeTo(File(patchDir, script))
    }
}
```

（`patchDir` = `FileSystemManager.getLocal().getFile(apApp.filesDir.parent, "patch")`，即 `/data/user/0/me.bmax.apatch/patch`；用 libsu-nio 的 `ExtendedFile` 以便对 root 文件读写。）

### 2.1 `boot_patch.sh` 的分步动作（`app/src/main/assets/boot_patch.sh`）

```sh
# Usage: boot_patch.sh <superkey> <bootimage> [FLASH_TO_DEVICE]
SUPERKEY="$1"; BOOTIMAGE=$2; FLASH_TO_DEVICE=$3

command -v ./kptools >/dev/null 2>&1 || { >&2 echo "- Command kptools not found!"; exit 1; }

./kptools unpack "$BOOTIMAGE" "$@"            # 解包出 kernel / ramdisk / …
if [ ! $(./kptools -i kernel -f | grep CONFIG_KALLSYMS=y) ]; then
    echo "- APatch requires CONFIG_KALLSYMS to be Enabled."; exit 0
fi

if [ $(./kptools -i kernel -l | grep patched=false) ]; then
    cp "$BOOTIMAGE" "ori.img"                 # 仅「尚未被 patch」时备份原厂
fi
mv kernel kernel.ori

KPT_ARGS=""
[ "$SUPERKEY" != "su" ] && KPT_ARGS="-S $SUPERKEY"     # ← 签名模式下不传 key
./kptools -p -i kernel.ori $KPT_ARGS -k kpimg -o kernel "$@"

./kptools repack "$BOOTIMAGE"

if [ ! $(./kptools -i kernel.ori -f | grep CONFIG_KALLSYMS_ALL=y) ]; then
    echo "- Detected CONFIG_KALLSYMS_ALL is not set! ... maybe your device won't boot."
fi

if [ "$FLASH_TO_DEVICE" = "true" ]; then
    if [ -b "$BOOTIMAGE" ] || [ -c "$BOOTIMAGE" ] && [ -f "new-boot.img" ]; then
        flash_image new-boot.img "$BOOTIMAGE"      # util_functions.sh 里的 dd 封装
    fi
fi
```

要点：
- **强依赖 `CONFIG_KALLSYMS`**，并在缺 `KALLSYMS_ALL` 时警告「可能不开机」。
- **`ori.img` 只在首次修补（`patched=false`）时生成**，靠 `kptools -i kernel -l` 的输出文本判断——文本解析，脆弱点之一。
- 修补参数 `-S <superkey>` 仅旧内核传；签名授权时代 `-S` 不出现。

### 2.2 管理器侧：四种模式的命令差异（`PatchesViewModel.doPatch`，`ui/viewmodel/PatchesViewModel.kt:399-535`）

```kotlin
enum class PatchMode { PATCH_ONLY, PATCH_AND_INSTALL, INSTALL_TO_NEXT_SLOT, UNPATCH }
```

| 模式 | 语义 | 第三条参数 `FLASH_TO_DEVICE` | 收尾 |
|---|---|---|---|
| `PATCH_ONLY` | 只出产物，用户自行刷 | 不传 | `new-boot.img` → 下载目录，命名 `apatch_patched_<apVer>_<kpVer>_<4位随机>.img`（`MediaStore` 或直接 `File` 拷贝，API≥Q 走 `createDownloadUri`/`insertDownload`） |
| `PATCH_AND_INSTALL` | 直装到当前 boot 分区 | `true` | 标记 `NEED_REBOOT`（`/dev/.need_reboot`） |
| `INSTALL_TO_NEXT_SLOT` | 写到另一个 slot 并切槽 | `true`（且 `boot_extract.sh true`） | `bootctl get-current-slot` → 目标槽 → `bootctl set-active-boot-slot <slot>` → 写 `/data/adb/post-fs-data.d/post_ota.sh`（开机成功后 `bootctl mark-boot-successful` 并清理 patchDir） |
| `UNPATCH` | 还原原厂 | — | `cp /data/adb/ap/ori.img new-boot.img` + `./busybox sh ./boot_unpatch.sh` + 删 `/data/adb/apd`、`/data/adb/ap` |

命令拼装的核心（同一函数内，两条**并存**的执行路径）：

```kotlin
// 关键判定：新内核可以用 truncate+superkey 自我提权时，走 ProcessBuilder；否则退回 libsu shell
val KPCheck = shell.newJob().add("truncate ${APApplication.superKey} -Z u:r:magisk:s0 -c whoami").exec()

if (KPCheck.isSuccess && !isSuExecutable()) {
    patchCommand.addAll(0, listOf("truncate", APApplication.superKey, "-Z", APApplication.MAGISK_SCONTEXT, "-c"))
    patchCommand.addAll(listOf(superkey, srcBoot.path, "true"))
} else {                                     // 旧内核：没有新 su 通道，走 libsu 的 root shell
    patchCommand = mutableListOf("./busybox", "sh", "boot_patch.sh")
    patchCommand.addAll(listOf(superkey, srcBoot.path, "true"))
    isKpOld = true
}

// 追加 KPM 注入参数（每个新 KPM）
patchCommand.addAll(listOf("-M", newExtrasFileName[i]))   // 模块文件
if (extra.args.isNotEmpty())  patchCommand.addAll(listOf("-A", extra.args))
if (extra.event.isNotEmpty()) patchCommand.addAll(listOf("-V", extra.event))
patchCommand.addAll(listOf("-T", extra.type.desc))
// 已存在、需要保留的 KPM
for (extra in existedExtras) { patchCommand.addAll(listOf("-E", extra.name)); … }
```

执行（两条路径的实际差异）：

```kotlin
if (isKpOld) {
    val resultString = "\"" + patchCommand.joinToString(separator = "\" \"") + "\""   // ← 手工拼 shell 字符串
    shell.newJob().add("export ASH_STANDALONE=1", "cd $patchDir", resultString).to(logs, logs).exec()
} else {
    builder.environment().put("ASH_STANDALONE", "1")
    builder.directory(patchDir)
    builder.redirectErrorStream(true)
    val process = builder.start()
    Thread { BufferedReader(InputStreamReader(process.inputStream)).use { … } }.start()
    succ = process.waitFor() == 0
}
```

修补成功后的收尾（`PatchesViewModel.kt:508-520`）：

```kotlin
shell.newJob().add(
    "[ -f $patchDir/ori.img ] && mkdir -p /data/adb/ap && cp $patchDir/ori.img /data/adb/ap/ && rm -f $patchDir/ori.img || true"
)
```

—— 因为 `post_ota.sh` 会在重启后 `rm -rf $patchDir`，备份必须先搬走。

### 2.3 分区定位（`boot_extract.sh`）与 OTA/切槽

- `extractAndParseBootimg()` 执行 `./busybox sh ./boot_extract.sh [true]`，从输出里解析 `SLOT=…` 与 `BOOTIMAGE=…`（`PatchesViewModel.kt:228-268`）；`INSTALL_TO_NEXT_SLOT` 传入 `true` 以取另一槽。
- 切槽成功后写开机标记脚本（`PatchesViewModel.kt:555-575`）：

```kotlin
"mkdir -p /data/adb/post-fs-data.d && rm -rf /data/adb/post-fs-data.d/post_ota.sh && touch …/post_ota.sh",
"echo \"chmod 0777 $patchDir/bootctl\" >> …/post_ota.sh",
"echo \"$patchDir/bootctl mark-boot-successful\" >> …/post_ota.sh",
"echo \"rm -rf $patchDir\" >> …/post_ota.sh",
"echo \"rm -f /data/adb/post-fs-data.d/post_ota.sh\" >> …/post_ota.sh",
```

- **Recovery 路径（APK 同时是卡刷包）**：`scripts/update_binary.sh`/`update_script.sh` 打包进 `resources/META-INF/com/google/android/{update-binary,updater-script}`（`app/build.gradle.kts:286-294`）。卡刷时 `update-binary` 用 `unzip` 从 **APK 自身**里解出 `lib/arm64-v8a/libbusybox.so`、`assets/*`、`lib/*`，再 `exec busybox sh assets/InstallAP.sh`（或 `UninstallAP.sh`）。`InstallAP.sh` 自己 `dd if=/dev/block/by-name/boot$slot of=boot.img` → `libkptools.so unpack` → `-p --kpimg ./assets/kpimg` → `repack` → `dd if=new-boot.img of=/dev/block/by-name/boot$slot`，并把原厂镜像 `mv boot.img /data/boot.img`。
  - 也就是说：**同一个 APK 既能当管理器安装，也能进 Recovery 直接刷卡刷**——这是 APatch 相对 KernelSU 的一个产品化优势（KernelSU 的仓库里没有卡刷包）。
  - 注意：`util_functions.sh:408 install_apatch()` 是一段**死代码**——它 `SOURCE` 后调用 `. ./boot_patch.sh "$BOOTIMAGE"`，但当前 `boot_patch.sh` 的签名是 `boot_patch.sh <superkey> <bootimage> [flash]`，且脚本内**没有任何 `SOURCEDMODE` 处理**（全仓只有 `util_functions.sh:412` 设置该变量）。全仓也没有任何脚本调用 `install_apatch`。

### 2.4 与 KernelSU 修补链的机制差异（同一件事的两种做法）

| 维度 | APatch | KernelSU |
|---|---|---|
| 修补实现 | **预编译 `kptools` + shell 脚本** | **纯 Rust 源码**（`android-bootimg`） |
| 可审计性 | 低（依赖外部 release 产物） | 高（本仓可读可改） |
| bootimg unpack/repack | `kptools unpack/repack` | `BootImage::parse` / `BootImagePatchOption::patch` |
| 注入内容 | `kpimg`（内核补丁）写进 **kernel 段** | `kernelsu.ko` + `ksuinit` 写进 **ramdisk**（或 v2 写 kernel） |
| 监督 | 脚本内 `grep CONFIG_KALLSYMS` | Rust 内 `enforce_bootimage_version` 等硬校验 |
| 刷写 | `flash_image`（dd 包装）；Recovery 里也是 dd | `open` + `BLKROSET` + `write_all` + `sync_all` |
| 备份 | `ori.img`（单份，落在 `/data/adb/ap/`） | `ksu_backup_<sha1>`（内容寻址）+ ramdisk 内 `stock_image.sha1` |
| 产物命名 | `apatch_patched_<apVer>_<kpVer>_<rand>.img` | `kernelsu_patched_<YYYYmmdd_HHMMSS>.img` |
| 前置条件 | `CONFIG_KALLSYMS`（必需）/ `KALLSYMS_ALL`（建议） | GKI（直装时）/ boot 版本 ≥ 3 |

---

## 3. root 授权模型

### 3.1 三条通道（按优先级）

```
① 签名授权（新内核默认）
   Natives.nativeReady("su")  ← 字面量 "su" 作为 key，内核按 APK 签名放行
        ↓ 失败
② 旧 SuperKey（Keystore 加密保存，兼容旧内核）
   APatchKeyHelper.readSPSuperKey() → Natives.nativeReady(<key>)
        ↓ 失败
③ 无（UNKNOWN_STATE，功能全部禁用）
```

`resolveSuperKey()`（`APatchApp.kt:261-281`）：

```kotlin
private fun resolveSuperKey(): String {
    APatchKeyHelper.setSharedPreferences(sharedPreferences)
    val savedKey = APatchKeyHelper.readSPSuperKey()

    // Signature authorization (new default).
    if (Natives.nativeReady("su")) {
        if (!savedKey.isNullOrEmpty()) {
            APatchKeyHelper.clearConfigKey()
            Log.i(TAG, "signature auth ready, cleared legacy SuperKey")
        }
        return "su"
    }
    // Legacy kernel patched with a real SuperKey: reuse the stored one.
    if (!savedKey.isNullOrEmpty() && Natives.nativeReady(savedKey)) {
        return savedKey
    }
    return "su"
}
```

SuperKey 的静态存储用 **AndroidKeyStore + AES/GCM**（`util/APatchKeyHelper.java`：`KEY_ALIAS = "APatchSecurityKey"`、每次加密随机 12 字节 IV 前置、`decryptLegacy` 兼容旧单 IV 格式），并提供 `shouldSkipStoreSuperKey` 让用户选择不落盘。

### 3.2 root shell 的真实形态：「superkey 作为 argv」

```kotlin
// util/APatchCli.kt:37-64
fun createRootShell(globalMnt: Boolean = false): Shell {
    val builder = Shell.Builder.create().setInitializers(RootShellInitializer::class.java)
    return try {
        builder.build(SUPERCMD, APApplication.superKey, "-Z", APApplication.MAGISK_SCONTEXT)
        //            ↑ "/system/bin/truncate"（APatchApp.kt:52）
    } catch (e: Throwable) {
        try {                                   // 兼容旧 kpatch
            if (globalMnt) builder.build(getKPatchPath(), APApplication.superKey, "su", "-Z", APApplication.MAGISK_SCONTEXT, "--mount-master")
            else           builder.build(getKPatchPath(), APApplication.superKey, "su", "-Z", APApplication.MAGISK_SCONTEXT)
        } catch (e: Throwable) {
            try { if (globalMnt) builder.build("su", "-mm") else builder.build("su") }
            catch (e: Throwable) { builder.build("sh") }        // ← 静默降级
        }
    }
}
```

即：把一个**普通系统二进制 `truncate`** 当作 `su` 使用，内核 hook 看到 `argv[1] == superkey`（或签名已授权）就以 `u:r:magisk:s0` 提权执行 `argv[3..]`。这解释了为什么 APatch 里到处出现 `truncate <key> -Z u:r:magisk:s0 -c <cmd>`。

`SUPERCMD` 与 su 路径常量（`APatchApp.kt:50-66`）：

```kotlin
const val SUPERCMD = "/system/bin/truncate"
const val APD_PATH = "/data/adb/apd"
const val APATCH_FOLDER = "/data/adb/ap/"
const val SU_PATH_FILE = APATCH_FOLDER + "su_path"
const val PACKAGE_CONFIG_FILE = APATCH_FOLDER + "package_config"
private const val DEFAULT_SU_PATH = "/system/bin/kp"
private const val LEGACY_SU_PATH = "/system/bin/su"
const val MAGISK_SCONTEXT = "u:r:magisk:s0"
const val DEFAULT_SCONTEXT = "u:r:untrusted_app:s0"
```

`/data/adb/ap/su_path` 是**内核侧 sucompat 的注册表**（内核去哪找 su 二进制）；`installApatch()` 会 `touch` 并写入 `LEGACY_SU_PATH`，卸载时 `Natives.resetSuPath(DEFAULT_SU_PATH)`。

### 3.3 授权操作（`ui/viewmodel/SuperUserViewModel.kt`）

```kotlin
// 拉全量包（必须 root）：RootServices (AIDL) + 内核 uid 名单 + 本地 CSV
val allPackages = IAPRootService.Stub.asInterface(binder).getPackages(0)
val uids = Natives.suUids().toList()
val actProfile = if (uids.contains(uid)) Natives.suProfile(uid) else null
val config = configs.getOrDefault(uid,
    PkgConfig.Config(appInfo.packageName, Natives.isUidExcluded(uid), 0, Natives.Profile(uid = uid)))

// 授权
Natives.grantSu(app.uid, 0, newConfig.profile.scontext)   // toUid=0, scontext=u:r:magisk:s0
Natives.setUidExclude(app.uid, 0)
// 撤销
Natives.revokeSu(app.uid)
// 排除（不授权、且不参与某些特性）
Natives.setUidExclude(app.uid, newConfig.exclude)
```

- 内核是**唯一权威**（`suUids()` 返回的就是内核 allow list）；`package_config` 只是管理器侧镜像，用于「内核不可用时仍能显示上次状态」以及跨版本迁移。
- CSV 格式（`util/PkgConfig.kt`）：`pkg,exclude,allow,uid,to_uid,sctx`，带 header 与 BOM 容忍，`Config.isDefault()` 的行不写入。
- 包枚举用 `services/RootServices.java`（libsu `RootService`）+ `IAPRootService.aidl`（`getPackages(int)`），与 KernelSU 的 `KsuService` 思路完全一致；APatch 用反射 `getInstalledPackagesAsUser`，KernelSU 用 `PackageManager` + 多用户遍历。

### 3.4 与 KernelSU 授权模型的对比

| 维度 | APatch | KernelSU |
|---|---|---|
| 管理器鉴权 | **签名授权**（`Natives.nativeReady("su")`）或 SuperKey | APK v2 签名块校验（`EXPECTED_SIZE/HASH` 编译进内核） |
| root 请求 | 无交互，`grantSu/revokeSu` 即时生效 | 无交互，写 app profile 即时生效 |
| 名单存储 | 内核名单 + `/data/adb/ap/package_config`(CSV) | 内核 `/data/adb/ksu/.allowlist`（二进制 magic v4） |
| per-app 特权 | `to_uid` + `scontext`（简单两维） | uid/gid/groups/caps/scontext/namespace/rules（完整 profile） |
| su 入口 | `truncate <key> -Z <sctx>`（劫持通用二进制） | `/data/adb/ksud debug su`；普通 app 走 `/system/bin/su` → ksud |
| 审计 | 无（**未确认**是否有等价 sulog） | `sulog` 事件流（`KSU_IOCTL_GET_SULOG_FD`） |
| 密钥暴露面 | superkey 出现在 argv（`/proc/<pid>/cmdline`） | 无密钥；管理器身份靠签名 |

---

## 4. 模块管理

### 4.1 目录布局（`apd/src/defs.rs`）

```
/data/adb/
├── apd                        DAEMON_PATH
├── modules/<id>/              MODULE_DIR（0o700）
├── modules_update/<id>/       MODULE_UPDATE_DIR（注释明示「不可改，否则要改 module_installer.sh」）
├── metamodule/                METAMODULE_DIR
└── ap/                        WORKING_DIR
    ├── bin/                   BINARY_DIR（busybox / kptools / resetprop→apd / magiskpolicy→apd / apd 链接）
    ├── log/                   APATCH_LOG_FOLDER
    ├── .aprc                  AP_RC_PATH
    ├── module_configs/        MODULE_CONFIG_DIR（persist.config / tmp.config）
    ├── package_config         ← 管理器写入的 CSV
    ├── su_path                ← 内核 sucompat 注册
    ├── kpm/<id>/<id>.kpm      ← KPM 持久化
    └── ori.img                ← 原厂 boot 备份
```

### 4.2 APM 安装流程（`apd/src/module.rs:363+`，`_install_module`）

与 KernelSU 的 `install_module_to_system` 几乎逐行同构，但有一处关键差异：**APatch 会先判断 `needs_mount`，只有「带 `system/` 且无 `skip_mount`」的模块才受 metamodule 安装安全检查约束**。

```rust
let needs_mount = {
    let has_system = archive.file_names().any(|name| name.starts_with("system/"));
    let has_skip_mount = archive.file_names().any(|name| name == "skip_mount");
    has_system && !has_skip_mount
};
if !is_metamodule && needs_mount && let Err(is_disabled) = metamodule::check_install_safety() { … bail!(…) }
```

其余步骤：`ensure_boot_completed()` → `assets::ensure_binaries()` → 读 `module.prop` 取 `id` → 正则 `^[a-zA-Z][a-zA-Z0-9._-]+$`（注释明确「same rule as KernelSU and module_config.rs」）→ metamodule 单实例检查 → 解压到 `modules_update/<id>` → 用 **`apd/assets/installer.sh`**（`include_str!("../assets/installer.sh")`）执行安装脚本 → 写 `module.prop` 到 `modules/<id>` + `update` 标志。

### 4.3 apd 的 CLI 面（`apd/src/cli.rs`）

```rust
Module { Install{zip} | Uninstall{id} | UndoUninstall{id} | Enable{id} | Disable{id}
         | Action{id} | Lua{id, function} | List | Config{internal?, subcommand} },
PostFsData, Services, BootCompleted, UidListener, Insmod{module, params},
SoftReboot, LateLoad{module?, kmi?, magica?, post_magica, package_name?},
Resetprop{args}, Sepolicy(args),
```

对比 ksud：APatch 多 `UidListener`（root 名单同步）、`Lua`（模块 lua 动作）、`Insmod`（无版本检查加载 .ko）；少 `BootPatch/BootRestore`（修补在 shell 脚本里）、`BootInfo`、`Feature`、`Profile`（profile 由 JNI supercall 直接做）。

### 4.4 KPM（Kernel Patch Module）——APatch 的差异化能力

- 持久化：`/data/adb/ap/kpm/<id>/<id>.kpm`，禁用靠 `<id>/disable`（`APatchApp.kt:71` `KPMS_DIR`，`KPModuleViewModel.kt:131-160`）。
- 运行态：`Natives.loadKernelPatchModule(path, args)` / `unloadKernelPatchModule` / `kernelPatchModuleList` / `kernelPatchModuleInfo(name)` / `kernelPatchModuleControl(name, arg)` → 返回 `KPMCtlRes{rc, outMsg}`。
- 双视图：`fetchModuleList()` 同时合并「内核里已加载的 live 列表」与「磁盘上持久化的 `.kpm`」；`load_source=file` 只表示加载来源，不代表已安装；`installed` 仅在磁盘扫描命中时置真。
- 元数据解析：`kptools -l -M <file>` 得到 INI（name/version/license/author/description）。
- 修补期注入：`PatchesViewModel` 把 `-M <file> -T <type> [-A args] [-V event]` 传给 `boot_patch.sh` → `kptools -p`，即**KPM 可以编进内核镜像开机即加载**（`loadSource = "embedded"`）。

### 4.5 模块 WebUI

与 KernelSU 同款方案且在**同一批文件**上有交集：`ui/webui/{WebViewInterface.kt, SuFilePathHandler.java, MimeUtil.java, MonetColorsProvider.kt, Insets.kt, AppIconUtil.kt}` —— 用 `WebViewAssetLoader` + libsu `SuFileInputStream` 读取模块 `webroot/`，JS 侧通过 `WebViewInterface` 暴露 exec 等能力。

---

## 5. 对 SPRoot 的结论

> SPRoot 本身是 APatch 的改造分支（`apd` → `AP`，SuperKey → SKRoot 48 位密钥，见 `docs/phase1_ap_apd_analysis_and_sk_adaptation.md`、`docs/SuperKey移除方案-草案.md`）。因此本节以「APatch 已有 → SPRoot 要改什么」为主线。

### 5.1 可以直接借鉴的 5 个设计点

**① 双轨密钥迁移（签名优先、SuperKey 兜底、迁移后立即清密钥）**

`resolveSuperKey()` 的三段式是所有「换鉴权根」项目都该抄的模板：
1. 先用新机制探测（`nativeReady("su")`）；
2. 新机制可用 → **主动清除旧密钥**（`APatchKeyHelper.clearConfigKey()`），避免旧密钥长期驻留；
3. 新机制不可用但旧密钥仍能通过 → 用旧密钥完成升级（用户可无感升级内核后再切）。

SPRoot 的 SuperKey→SKRoot 迁移正好缺这一段「内核仍是旧版时怎么办」。建议照抄这个顺序，并把「是否清密钥」做成可审计事件（写进管理器日志/`/data/adb/ap/log`）。

**② 修补模式显式枚举 + 每个模式独立的收尾动作**

`PatchMode{PATCH_ONLY, PATCH_AND_INSTALL, INSTALL_TO_NEXT_SLOT, UNPATCH}` 把「只出包 / 直装 / 装另一槽 / 还原」拆成互斥路径，每条路径有自己的产物落点、是否需要重启、是否需要写开机标记脚本。SPRoot 的修补引擎契约（`skroot-patch --in/--out`）目前只有「出包」形态，建议在管理器层补齐这 4 个模式（尤其 `INSTALL_TO_NEXT_SLOT` 的 `bootctl` 切槽 + `mark-boot-successful` 闭环）。

**③ 备份闭环：`ori.img` + 未修补判定 + 搬运 + 还原**

APatch 的闭环是：`kptools -i kernel -l | grep patched=false` → `cp boot.img ori.img` → 打补丁 → 成功后把 `ori.img` 搬到 `/data/adb/ap/` → `UNPATCH` 时 `cp /data/adb/ap/ori.img new-boot.img` + `boot_unpatch.sh`。其中两个细节值得抄：
- **只在「尚未修补」时备份**（避免二次修补把已修补镜像当原厂备份）；
- **备份搬运**（因为 `post_ota.sh` 会删 patchDir；`migrateStockBootBackup()` 还兜底处理「用户 PATCH_ONLY 后自己 fastboot 刷」导致备份遗留在 app 私有目录的情况）。

**④ 应用级状态机 + 页面级能力门禁**

`State` 10 态 + `BottomBarDestination.kPatchRequired/aPatchRequired` 让「内核补丁没装」和「用户态没装」两种缺失在 UI 上**结构性地**区分开，而不是每个页面各写 `if (rootAvailable())`。SPRoot 会有「SK 内核未修补 / AP 未安装 / 需重启 / 需更新」四种以上状态，建议直接采用这个模型（甚至可以把 `kPatchRequired` 泛化成 `requires: Set<Capability>`）。

**⑤ KPM 的「运行态 / 持久化态」双视图**

`installed`（磁盘有 `.kpm`）+ `loadedIds`（内核里已加载）+ `disabled`（`disable` 文件）+ `storedVersion`（磁盘元数据）四元组，配合 `loadSource`（`embedded` / `file`）区分「开机内置」与「运行期加载」。SPRoot 的 SK/KPM 生态若要同时支持「编进内核」和「运行期加载」，这套模型可以直接复用。

> **额外加分项（不强求，但成本低收益高）**：把 APK 同时做成**卡刷包**——`resources/META-INF/com/google/android/{update-binary,updater-script}` + `unzip` 自解包 + 复用同一批 `lib*.so`/`kpimg`（见 §2.3）。用户无法开机时能直接在 Recovery 里刷同一个文件，等于给「手机端修补」配了一条离线兜底通道。

### 5.2 必须避开 / 必须改造的 3 个点

**① 不要沿用「superkey 作为 argv 传给 `/system/bin/truncate`」**

```kotlin
builder.build(SUPERCMD, APApplication.superKey, "-Z", APApplication.MAGISK_SCONTEXT)
```

三个硬伤：
- **密钥出现在进程命令行**，`/proc/<pid>/cmdline` 对同 uid 及 root 可读，也容易进 logcat/crash 报告；
- **依赖一个具体系统二进制**（`/system/bin/truncate`）与内核对 argv 的识别逻辑，OEM 换 toybox 版本/路径即失效；
- 与「移除 SuperKey」的目标**直接冲突**：只要还走这条路径，密钥就必须存在于用户态内存与命令行。
SPRoot 应改为：SKRoot 密钥只在 **JNI → supercall** 的调用参数里出现（或干脆由内核派生），root shell 通过一个专用入口获得（如 `apd`/`AP` 自己作为 shell 进程，`Shell.Builder.build("/data/adb/AP", "su", …)`），**永不经过 argv 之外的通用二进制**。

**② 不要把修补工具做成「构建时下载的预编译二进制 + assets 里的 shell 脚本」**

- `preBuild` 依赖 4 个网络下载任务，任意一个 release 被删/网络不通就无法构建；
- 修补逻辑（unpack/repack/注入/校验）藏在不可审计的 `kptools` 里，出问题只能靠 `grep` 文本输出定位；
- `boot_patch.sh` 用 `$(... | grep patched=false)`、`command -v ./kptools` 这类文本判定，且**没有任何结构化错误码**；
- 与 SPRoot 的引擎契约（`--json` NDJSON + 退出码，见 `docs/SPROOT-DOC-C1-修补引擎IO契约.md`）方向相反。

建议：把 bootimg 解包/回包 vendor 进 `engine/`（或直接采用 KernelSU 那条 `android-bootimg` Rust 路线），对外只暴露**结构化 stdout/JSON + 明确退出码**，脚本最多作为「人肉调试入口」保留。

**③ 不要保留「ProcessBuilder 与 libsu shell 双路径 + 字符串拼命令」**

`doPatch` 里同一件事有两套执行路径（`isKpOld` 分支），且旧路径是：

```kotlin
val resultString = "\"" + patchCommand.joinToString(separator = "\" \"") + "\""
shell.newJob().add("export ASH_STANDALONE=1", "cd $patchDir", resultString).exec()
```

- 手工给每个参数加引号再拼成一整条 shell 字符串，**路径含引号/换行/`$` 时必然出错**（用户选的 boot 文件名可控）；
- 两条路径的环境变量、日志采集、退出码语义都不完全一致（一条 `waitFor()==0`，一条 `Shell.Result.isSuccess`），排查成本翻倍；
- 新路径又依赖「先用 `truncate … -c whoami` 探测能否提权」这种副作用探测。

建议：**统一为 `ProcessBuilder` + argv 数组**（无 shell），root 权限由调用方（AP/SK 的 su 入口）保证；退出码与日志走统一封装；探测逻辑删掉（能力探测改为读内核版本/feature 位）。

### 5.3 一句总结

APatch 的管理器在**产品化**上比 KernelSU 更成熟（状态机、模式枚举、KPM 双视图、备份闭环、签名授权迁移），但**工程实现**上把「修补」这件最关键的事外包给了不可审计的二进制与 shell 脚本。SPRoot 应取前者、弃后者：**保留 APatch 的产品结构与迁移路径，把修补链换成 KernelSU 式的自研可审计实现**。

---

## 6. 证据

### 6.1 关键文件清单（相对 `APatch/`）

| 主题 | 文件 | 关键行 |
|---|---|---|
| 技术栈/构建 | `gradle/libs.versions.toml`、`build.gradle.kts:1-62`、`app/build.gradle.kts` | 1-62；`app/build.gradle.kts:111-162, 243-302, 312-345` |
| 下载 kptools/kpimg/kpatch/ko | `app/build.gradle.kts` | 243-283 |
| native 目录 | `app/build.gradle.kts` | 159-162 |
| 应用状态机 / SUPERCMD / 路径常量 | `app/src/main/java/me/bmax/apatch/APatchApp.kt` | 47-56、50-66、181-200、261-281、306 |
| 安装 apd 到系统 | `APatchApp.kt` | `uninstallApatch()` 98-123、`installApatch()` 124-166 |
| root shell | `app/src/main/java/me/bmax/apatch/util/APatchCli.kt` | 26-64、74-100、152-170、196-215 |
| 修补流程 | `app/src/main/java/me/bmax/apatch/ui/viewmodel/PatchesViewModel.kt` | 52-57（PatchMode）、86-112（prepare）、114-135（parseKpimg）、138-243（parseBootimg）、228-268（extractAndParseBootimg）、351-395（doUnpatch）、399-535（doPatch）、536-580（切槽/落盘） |
| 修补脚本 | `app/src/main/assets/{boot_patch.sh, boot_unpatch.sh, boot_extract.sh, util_functions.sh}` | `boot_patch.sh:24-110` |
| Recovery 脚本 | `app/src/main/assets/{InstallAP.sh, UninstallAP.sh}`、`scripts/{update_binary.sh, update_script.sh}` | 全文件 |
| 授权 | `app/src/main/java/me/bmax/apatch/ui/viewmodel/SuperUserViewModel.kt` | 120-240 |
| 密钥存储 | `app/src/main/java/me/bmax/apatch/util/APatchKeyHelper.java` | 全文件 |
| 名单镜像 | `app/src/main/java/me/bmax/apatch/util/PkgConfig.kt` | 1-80 |
| 包枚举服务 | `app/src/main/java/me/bmax/apatch/services/RootServices.java` | 1-80 |
| JNI 面 | `app/src/main/java/me/bmax/apatch/Natives.kt` | 36-160 |
| 导航/门禁 | `app/src/main/java/me/bmax/apatch/ui/screen/BottomBarDestination.kt` | 全文件 |
| 模式选择 | `app/src/main/java/me/bmax/apatch/ui/screen/InstallModeSelect.kt` | 129-157 |
| KPM | `app/src/main/java/me/bmax/apatch/ui/viewmodel/KPModuleViewModel.kt` | 99-180 |
| 模块安装 | `app/src/main/java/me/bmax/apatch/util/APatchCli.kt` | 274-320 |
| apd CLI | `apd/src/cli.rs` | 26-143 |
| apd 模块安装 | `apd/src/module.rs` | 28、363-470、508+ |
| apd 常量 | `apd/src/defs.rs` | 全文件 |
| 模块安装器 | `apd/assets/installer.sh` | 全文件 |

### 6.2 关键命令摘录（可直接复现）

```bash
# 1) 本地修补（PATCH_ONLY），实际形态
cd /data/user/0/me.bmax.apatch/patch
export ASH_STANDALONE=1
./busybox sh boot_patch.sh "su" "/data/user/0/me.bmax.apatch/patch/boot.img"
# 2) 直装 / 装下一槽（第三参数 true 触发 flash_image）
./busybox sh boot_patch.sh "su" "/dev/block/by-name/boot_a" "true"
# 3) 带 KPM 注入
./busybox sh boot_patch.sh "su" "<boot.img>" "true" -M <hash>.kpm -T kernel -V pre-kernel-init [ -A "<args>" ]
# 4) 还原
cp /data/adb/ap/ori.img new-boot.img && ./busybox sh ./boot_unpatch.sh /dev/block/by-name/boot_a
# 5) 切槽（INSTALL_TO_NEXT_SLOT）
./bootctl get-current-slot && ./bootctl set-active-boot-slot 1
# 6) 脚本内部的 kptools 调用序列
./kptools unpack boot.img
./kptools -i kernel -f | grep CONFIG_KALLSYMS=y
./kptools -i kernel -l | grep patched=false
./kptools -p -i kernel.ori [-S <superkey>] -k kpimg -o kernel
./kptools repack boot.img
# 7) root shell（新内核路径）
/system/bin/truncate "<superkey 或 'su'>" -Z u:r:magisk:s0
# 8) 模块
/data/adb/apd module install <zip> | module enable/disable/uninstall/undo-uninstall/action/list
```

### 6.3 复现命令

```bash
cd APatch && git log -1 --format='%H %ci %s'     # e8e4651c… 2026-10-13
grep -rn "boot_patch.sh\|kptools\|SUPERCMD" app/src/main --include=*.kt --include=*.kts
grep -n "downloadKptools\|downloadKpimg\|downloadCompatKpatch" app/build.gradle.kts
sed -n '24,110p' app/src/main/assets/boot_patch.sh
```

### 6.4 明确标注「未确认」的三处 + 一处已澄清

1. **KernelPatch 侧的签名授权实现**：`nativeReady("su")` 的语义（比对 APK 签名/包名/证书哈希？是否也校验 nonce / 防重放？）在 `bmax121/KernelPatch` 仓库，本仓只有调用方 —— **未确认**。这直接决定 SPRoot 能否照抄「双轨迁移」。
2. **是否有等价于 KernelSU `sulog` 的审计通道**：本仓未发现 su 事件流或审计 UI；`Settings` 有日志（`/data/adb/ap/log`）但未见 su 请求审计 —— **未确认**（倾向「没有」）。
3. **`resopt`/R8 对 `assets/*.sh`、`libs/*.so` 的实际影响**（是否会被裁剪或重命名）—— 未做构建验证。
4. **已澄清**：`libbootctl.so`（154 KB）与 `libbusybox.so`（1.7 MB）是**入仓二进制**（`git ls-files app/libs/arm64-v8a` 可证）；`libmagiskpolicy.so` 已被 apd 的 multicall 取代，`APatchApp.kt:161` 对它的调用是**遗留死引用**（文件不存在，失败被忽略）。

### 6.5 与 KernelSU 的一页速查对照

| 关注点 | APatch | KernelSU | SPRoot 建议 |
|---|---|---|---|
| 修补引擎 | 外部 `kptools` 二进制 | 本仓 Rust | **学 KernelSU**（自研可审计） |
| 修补编排 | assets shell 脚本 | Rust 内函数 | 学 KernelSU（结构化） |
| 备份 | `ori.img` 单份 | sha1 内容寻址 | 两者结合 |
| 直刷分区 | `flash_image`(dd) | `BLKROSET`+`write` | 学 KernelSU（更少依赖） |
| 管理器鉴权 | 签名（新）/SuperKey（旧） | APK v2 签名块 | 学 APatch 的**双轨迁移** |
| su 通道 | `truncate <key> -Z` | `ksud debug su` | **都不要**：专用 su 入口 |
| 授权存储 | 内核名单 + `package_config` CSV | 内核 `.allowlist` 二进制 | CSV 更利于迁移/调试 |
| 模块 | apd（与 ksud 同构） | ksud | 保持 apd 血统 |
| 内核模块生态 | KPM（运行期加载 + 编入内核） | LKM `.ko` + late-load | 学 APatch 的 KPM 双视图 |
