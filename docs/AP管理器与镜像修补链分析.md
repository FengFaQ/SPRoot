# AP 管理器与镜像修补链分析

> 第一阶段分析文档（pl_9c4e0c4f6e866e44）· 客户端方向
> 用途：支撑第二阶段「修补引擎替换设计」与「鉴权切换设计」（SK 48 位超级密钥）
> 状态：**已复核（标准地址验证通过，2026-11-06）**

---

## 0. 基线与来源声明

| 项 | 值 |
|---|---|
| 仓库 | `https://github.com/bmax121/APatch`（app 模块，稀疏检出） |
| 版本 tag | `11282`（latest release，2026-10-15 发布） |
| 锁定 commit | `e8e4651c16fb5075882a25813c8732285fdb3505`（2026-10-13 +0800，"feat(app): confirm module installs and allow cancelling downloads"） |
| 获取方式 | 初版经第三方镜像 `gh-proxy.com` 拉取；**已用标准地址复核通过（2026-11-06，WSL mirrored 模式 + 代理 CA）**：`git ls-remote` 确认 tag 11282 → `e8e4651c`，标准地址克隆与镜像副本 app/ 目录逐文件 diff 一致 |
| 复核要求 | ✅ 已完成复核；后续拉取一律走标准地址，gh-proxy 通道废止 |
| 本地路径 | `upstream/APatch-ghproxy-bak/`（app 模块 sparse checkout） |

---

## 1. AP 管理器的管理与交互形态、界面样式结构

### 1.1 技术栈

- **Kotlin + Jetpack Compose（Material 3）**，无 XML 布局；导航用 `com.ramcosta.composedestinations`（`@Destination<RootGraph>` 注解路由）。
- **libsu（topjohnwu）**：Root shell 封装（`Shell.Builder` / `RootService` IPC）。
- **OkHttp + Coil3**：在线模块仓库下载与图标加载。
- **JNI（libapjni.so）**：内核 supercall 直连通道（详见 §3）。
- 主题：约 24 套颜色主题（`ui/theme/`），动态取色支持。

### 1.2 导航与页面结构

入口 `MainActivity` 观察 `APApplication.apStateLiveData`，按状态决定底部导航可见项（MainActivity.kt:96-99）。

底部导航（`ui/screen/BottomBarDestination.kt`）四大目的地：

| 页面 | 文件 | 职责 |
|---|---|---|
| **Home** | `ui/screen/Home.kt`（899 行） | 状态卡片 `KStatusCard`（内核补丁状态）/ `AStatusCard`（Android 侧 apd 状态）/ `InfoCard`（版本/内核信息）；安装/更新/卸载入口跳转 Patches/Install |
| **SuperUser** | `ui/screen/SuperUser.kt` | Root 授权应用列表（搜索栏 + 下拉菜单），每个应用卡片含 Switch 授权、SELinux context 配置 |
| **APM** | `ui/screen/APM.kt`（776 行） | AP 模块列表管理；卡片组件在 `ui/component/ModuleCardComponents.kt`；点击 WebUI 图标跳 `WebUIActivity`（apatch://webui/<id>） |
| **KPM** | `ui/screen/KPM.kt`（823 行） | 内核模块（KPM）加载/卸载/控制界面 |
| **Settings/About** | `ui/screen/Settings.kt` / `AboutScreen.kt` | 全局命名空间、备份提醒、日志等 |

修补相关二级页：

- `InstallModeSelect.kt`：选修补模式与 boot.img 来源。
- `Patches.kt`（739 行）：修补主界面（详见 §2.1）。
- `Install.kt`（291 行）：模块 zip 安装日志页（进度流式输出，含安装确认对话框与下载取消——正是本次 11282 的 HEAD 提交内容）。
- `Online.kt` + `OnlineModuleViewModel.kt` + `util/ModuleRepo.kt`：在线模块仓库。

### 1.3 状态模型（贯穿全部 UI 的核心）

`APApplication`（APatchApp.kt）维护**双轴状态**：

```
kpState: UNKNOWN → KERNELPATCH_INSTALLED / NEED_UPDATE / NEED_REBOOT / UNINSTALLING
apState: UNKNOWN → ANDROIDPATCH_NOT_INSTALLED / INSTALLING / INSTALLED / NEED_UPDATE / UNINSTALLING
```

两轴均由 `superKey` setter 驱动初始化（见 §3.2）。关键路径常量（APatchApp.kt:45-77）：

```
APD_PATH=/data/adb/apd   APATCH_FOLDER=/data/adb/ap/   SUPERCMD=/system/bin/truncate
KPMS_DIR=/data/adb/ap/kpm/   SUCOMPAT_FILE=/data/adb/ap/sucompat   JAILBREAK_FILE=/data/adb/ap/jailbreak
DEFAULT_SCONTEXT=u:r:untrusted_app:s0   MAGISK_SCONTEXT=u:r:magisk:s0
```

另有「magica 越狱」支线（`magica/MagicaService` + `AppZygotePreload` + `util/APatchCli.kt:installJailbreak`）：permissive SELinux 下以 app-zygote 隔离进程经 adb 提权加载 `kernelpatch.ko`——与镜像修补互为补充的安装路径。

---

## 2. 重点：APK 内如何完成 boot 镜像修补

### 2.1 总链路（UI → VM → 脚本 → kptools）

```
PatchesScreen (Patches.kt)                ← 用户选模式/选 boot.img/选 KPM/填 SuperKey
   └─ PatchesViewModel (PatchesViewModel.kt)   ← 全部业务逻辑，Compose State 驱动
        ├─ prepare()        搭建工作目录 files/patch
        ├─ copyAndParseBootimg() / extractAndParseBootimg()   获取并解析镜像
        ├─ embedKPM()       解析待嵌入 KPM
        └─ doPatch(mode, useKey) / doUnpatch()
             └─ ProcessBuilder / libsu Shell 执行:
                 boot_patch.sh <superkey> <bootimage> [true] [kptools 参数...]
                    └─ kptools unpack / -p (patch) / repack   ← 真正改写内核镜像
```

**修补模式**（PatchesViewModel.kt PatchMode）：
`PATCH_ONLY`（导出镜像到 Downloads）、`PATCH_AND_INSTALL`（刷当前槽）、`INSTALL_TO_NEXT_SLOT`（刷备槽 + bootctl 切换）、`UNPATCH`（还原）。

### 2.2 分层细节

**层 1 · UI（Compose, Patches.kt）**

- 模式由 `InstallModeSelect` 传入；`PATCH_ONLY` 时先 `SelectFileButton` 选 boot.img → `viewModel.copyAndParseBootimg(uri)`。
- 「自定义 SuperKey」开关（Patches.kt:222-231，字符串 `patch_custom_superkey`）展开 `SetSuperKeyView`（Patches.kt:488-560）：密码框输入，`viewModel.checkSuperKeyValidation` 校验通过才回写 `viewModel.superkey`。
- 开始按钮 → `viewModel.doPatch(mode, needKey)`；日志区 `SelectionContainer{Text(patchLog)}` 实时滚动。
- KPM 嵌入：`SelectFileButton` → `viewModel.embedKPM(uri)`，新增项显示为 `ExtraItem` 卡片，可删。

**层 2 · ViewModel（Kotlin，无 native 调用——镜像修补不经 JNI！）**

- `prepare()`（PatchesViewModel.kt:~96）：把 APK native 库目录中 `libkptools.so / libbusybox.so / libkpatch.so / libbootctl.so` **symlink 成无 .so 后缀的可执行文件**（Android 只允许加载执行 nativeLibraryDir 内文件），再从 assets 释放 `boot_patch.sh / boot_unpatch.sh / boot_extract.sh / util_functions.sh / kpimg`。
  - 即：**kptools 与 kpimg（KernelPatch 核心镜像）以 so/asset 形式随 APK 分发**；kptools 二进制本体出自 KernelPatch 仓库，由内核方向同事负责。
- 解析：`kptools unpack` + `kptools -l -i kernel` 输出 INI → `KImgInfo(banner, patched)`、`KPImgInfo(version, compile_time, config, superKey)`；已修补镜像枚举已嵌入 extras（KPM 列表）。解析用 `org.ini4j.Ini`。
- 所有可变工作目录操作经 `workMutex` 串行化（prepare 会 wipe patchDir）。

**层 3 · 脚本（assets/boot_patch.sh）**

关键流程（boot_patch.sh:52-90）：

```sh
SUPERKEY="$1"; BOOTIMAGE=$2; FLASH_TO_DEVICE=$3; shift 2   # 其余参数透传 kptools
./kptools unpack "$BOOTIMAGE" "$@"          # 拆出 kernel
# 校验 CONFIG_KALLSYMS=y，否则中止（修了也起不来）
[ 未修补 ] && cp "$BOOTIMAGE" ori.img        # 原厂镜像备份
mv kernel kernel.ori
KPT_ARGS=""
[ "$SUPERKEY" != "su" ] && KPT_ARGS="-S $SUPERKEY"   # ← 密钥注入点：仅显式 key 才传 -S
./kptools -p -i kernel.ori $KPT_ARGS -k kpimg -o kernel "$@"   # 内核修补
./kptools repack "$BOOTIMAGE"               # 重打包 new-boot.img
# FLASH_TO_DEVICE=true 时 flash_image new-boot.img $BOOTIMAGE
```

`"$@"` 透传的 KPM 参数由 VM 拼装：新嵌入 `-M <kpm文件>`，已存在 `-E <name>`；配 `-A <args>` `-V <event>` `-T <type>`。

**层 4 · native（kptools/kpimg，KernelPatch 侧）**

- `-S <key>` 把 SuperKey 经 `hash_key()`（scdefs.h:7-11，多项式 31 hash，基数 1000000007）压成 long 绑进内核。
- 运行期 supercall 入口：`syscall(45 /*__NR_supercall*/, key, ver_and_cmd(key, cmd))`；`ver_and_cmd = version_code<<32 | 0x1158<<16 | cmd`（supercall.h:14-17）。**内核把 syscall 45（truncate）hook 成 supercall**，这就是 SUPERCMD=/system/bin/truncate 的由来。
- ⚠️ kptools 与 kpimg 不在 app 模块内（来自 KernelPatch 仓库构建产物），SK 替换密钥格式需与内核修补组协同改 kpimg/kptools。

### 2.3 「刷入后能开机」的保障机制（APK 侧承担部分）

1. **前置校验**：`CONFIG_KALLSYMS=y` 必查（boot_patch.sh:62-67；KALLSYMS_ALL 缺失仅告警）。
2. **原厂备份双保险**：patchDir 内 `ori.img`（PATCH_AND_INSTALL 后搬移到 `/data/adb/ap/ori.img`，PatchesViewModel.doPatch 尾部 + APatchCli.migrateStockBootBackup 兜底迁移）；`UNPATCH` 直接 `cp /data/adb/ap/ori.img new-boot.img` + `boot_unpatch.sh` 还原并清除 apd（doUnpatch）。
3. **A/B 槽安全安装**（INSTALL_TO_NEXT_SLOT）：`libbootctl.so` 走 boot HAL（`hal-info` / `get-current-slot` / `set-active-boot-slot`）刷备槽；写 `/data/adb/post-fs-data.d/post_ota.sh`，开机成功后执行 `bootctl mark-boot-successful` 并自删——失败重启自动回退旧槽。
4. **重boot标记**：成功后 `APApplication.markNeedReboot()` 写 `/dev/.need_reboot`，Home 页提示重启。
5. **安装上下文**：shell 全部以 `u:r:magisk:s0` context 运行（-Z 参数），规避 untrusted_app 对块设备的 SELinux 拒绝。

### 2.4 对 SK 替换的关键结论

- **修补参数链已完全参数化**：SuperKey 只是 `boot_patch.sh $1` → `kptools -S $S` 的透传字符串。管理器侧替换成 SK 48 位密钥只需改「密钥来源与生成方」；**密钥生成/注入格式在 kptools+kpimg+内核侧**，须由内核修补组在 SKRoot 修补引擎中定义并输出契约（建议契约点：`kptools -S` 的 key 字符格式约束 + `SUPERCALL_KEY_MAX_LEN` 0x40 上限）。
- 上游已出现**密钥弱化趋势**：`SUPERKEY != "su"` 才传 `-S`，即新内核支持「签名/uid 隐式授权」，SuperKey 正被上游淡化（APatchApp.kt:259-261 上游自注 TODO："remove all usage of superkey"）。这与 SPRoot「SK 48 位密钥驱动」方向相反，改造时**不能沿用上游 su 隐式授权回退**，需删干净（Q3=A）。

---

## 3. 现有 SuperKey 自动鉴权机制全貌

### 3.1 三层结构

```
┌ UI 层        Patches.kt(SetSuperKeyView) / KPModel.KPImgInfo.superKey
├ 应用层       APApplication.superKey（全局单例字符串）
│              ├─ APatchKeyHelper：Keystore(AES/GCM) 加密存 SharedPreferences
│              └─ resolveSuperKey()：su 优先 → legacy key 回退
├ Shell 层     APatchCli.createRootShell: SUPERCMD(truncate) + superKey + -Z magisk:s0
└ JNI 层       apjni.cpp → supercall.h inline → syscall(45, key, ver_and_cmd)
                                    └→ 内核 hash_key(key) 比对修补时绑定的 hash
```

### 3.2 密钥生命周期

**生成/设置**
- 修补时：用户在 SetSuperKeyView 手输（校验：8~63 位、须含字母+数字，PatchesViewModel.kt:202-204）→ `kptools -S` 绑进内核；若不勾选则用 `"su"`（隐式授权，不写 key）。
- 运行期：`SUPERCALL_SKEY_GET/SET/ROOT_ENABLE`（0x100a/b/c，scdefs.h:26-28）可在内核侧读写 key。

**存储（APatchKeyHelper.java）**
- Android Keystore 生成 `APatchSecurityKey`（AES/GCM/NoPadding）。
- `writeSPSuperKey()`：密钥串 GCM 加密（随机 12B IV 前置）→ Base64 → SP 键 `super_key_enc`；旧明文键 `super_key` 读取时自动迁移并清除；`skip_store_super_key` 置 1 可禁止落盘；`clearConfigKey()` 清三键。
- ⚠️ 上游源码注释自认风险：key 可被 root/LSPosed 注解窃取，唯一解法是"不用 superkey"——SPRoot 用一次性 48 位密钥 + SK 侧立即授权，可从根上消解。

**解析（resolveSuperKey，APatchApp.kt:246-268）**
1. `Natives.nativeReady("su")` 成功 → 用 "su"（新内核签名授权），顺带清 legacy key；
2. 否则读 SP 存的 legacy key，`nativeReady(savedKey)` 成功 → 回退用之（升级兼容）；
3. 都不行 → 返回 "su"（State 落 UNKNOWN，Home 页显示未安装）。

**生效（APApplication.superKey setter，APatchApp.kt:167-238）**
- `Natives.nativeReady(value)` 决定 kpState/apState 初始化；
- ready 后起线程：`Natives.su(0,null)` 提权 → 版本比对（`installedKPTime` vs `getKpImg` 构建时间，不等 → NEED_UPDATE；apd 版本 vs 管理器版本 → ANDROIDPATCH_NEED_UPDATE）→ 同步 su path（`/data/adb/ap/su_path`）。

### 3.3 Root shell 构建（createRootShell，APatchCli.kt:36-73）

```
首选:  ["/system/bin/truncate", superKey, "-Z", "u:r:magisk:s0"]   ← truncate 被 hook 为 supercall
回退1: [libkpatch.so, superKey, "su", "-Z", ...]                    ← 旧版 kpatch ELF（≥0.11.0 已弃用）
回退2: ["su"]  回退3: ["sh"]                                        ← 无 root 只读
```

`APatchCli.SHELL`（主 shell）与 `GLOBAL_MNT_SHELL`（--mount-master）启动即建；`refresh()` 反射重置 libsu MainShell 单例后重建（installApatch 末尾调用）。`tryGetRootShell`、`withNewRootShell` 为按需变体。

### 3.4 JNI 层（apjni.cpp，全部首参 superKey）

20 个注册方法（apjni.cpp:328-352 gMethods）按 supercall 命令映射：

| Java(Natives.kt) | JNI → supercall | 命令号 |
|---|---|---|
| nativeReady | sc_ready（HELLO echo "hello1158"） | 0x1000 |
| nativeSu / nativeSuProfile | sc_su / sc_su_uid_profile | 0x1010/… |
| nativeGrantSu / nativeRevokeSu | sc_su_grant_uid / sc_su_revoke_uid | kstorage 组 0 |
| nativeSetUidExclude/Get | sc_set/get_ap_mod_exclude | kstorage 组 1 |
| nativeSuUids / nativeSuPath / nativeResetSuPath | sc_su_allow_uids / get_path / reset_path | … |
| nativeLoad/Unload/ControlKernelPatchModule | sc_kpm_load/unload/control | 0x1020-22 |
| nativeKernelPatchModuleNum/List/Info | sc_kpm_nums/list/info | 0x1030-32 |
| nativeKernelPatchVersion/BuildTime | sc_kp_ver / sc_get_build_time | 0x1008/07 |
| nativeControlFeature | sc_control_feature | 0x1046 |

`nativeSu` 传 `su_profile{uid, to_uid, scontext}`；JNI 层对空 key 统一兜底 `"su"`（`ensureSuperKeyNonNull` + 空串替换）。

### 3.5 授权 UI 流（SuperUserViewModel.kt）

- 列表：`Natives.suUids()` → 每应用 `Natives.suProfile(uid)` + `PkgConfig`（包名、exclude、profile 持久化在 APK 侧）。
- 授权：`Natives.grantSu(app.uid, 0, scontext)`（默认 toUid=0，scontext 默认 untrusted_app，可改 magisk）；撤销 `revokeSu`；排除列表 `setUidExclude`。
- ⚠️ 该列表实际存于**内核 kstorage**（组 0/1），APK 侧只存 UI 配置——SK 化后此链路必须保持内核侧数据结构兼容或整体迁移到 SKRoot 契约。

### 3.6 SuperKey 全部引用点清单（移除/SK 化改造面）

| # | 位置 | 现状 | 改造方向（Q3=A → SK 48 位密钥） |
|---|---|---|---|
| 1 | `APatchApp.kt:167-268` superKey setter + resolveSuperKey | su 隐式授权 + legacy 回退 | 改为：修补后由 SK 引擎产出的 48 位密钥写入；删除 "su"/legacy 回退分支 |
| 2 | `APatchKeyHelper.java` | Keystore 加密 SP 存储 | 保留壳，改存 SK 密钥（或按契约改为首启从 SKRoot 读取/确认） |
| 3 | `Patches.kt:222-231,488-560` SetSuperKeyView + needKey 开关 | 手输 8-63 位 key | 重设计：修补完成后展示/回显 SK 生成密钥（或引导到 SK 授权码录入），校验规则换 48 位格式 |
| 4 | `PatchesViewModel.kt:40,159,202-204,262` superkey 状态 + 校验 + doPatch(useKey) | 默认 "su" 兜底 | 密钥来源改为 SK 修补引擎输出；删除 "su" 默认值 |
| 5 | `APatchCli.kt` createRootShell/tryGetRootShell/Main | truncate+superKey 起 shell | 命令模板不变，superKey 实参换成 SK 密钥（或按 SKRoot 契约改为 AP 二进制派生凭据） |
| 6 | `Natives.kt` 全部方法 + `apjni.cpp` | 首参 superKey → supercall | key 实参统一换 SK 密钥；若 SKRoot 改 syscall 协议则同步改 supercall.h/ver_and_cmd（与内核/系统守护协同） |
| 7 | `KPModel.kt:74` KPImgInfo.superKey + parseKpimg/parseBootimg | 从 kptools -l 读 root_superkey | 改为读 SK 密钥元信息（依赖新 kptools 输出格式契约） |
| 8 | `boot_patch.sh:30,52,83-84` `$SUPERKEY` + `-S` | 透传 | 脚本不改，密钥串换源 |
| 9 | `scdefs.h` hash_key/SUPERCALL_SKEY_* | 内核侧 | 内核修补组职责（此处仅记录 APK 依赖面） |

---

## 4. KPM 在 APK 侧的安装/加载入口（浅层梳理）

> 深度机制分析由内核修补组负责（pl_28d940ceba07ad7a），此处仅记录 APK 侧入口与调用面。

### 4.1 运行时加载入口（KPM.kt）

| 动作 | UI 位置 | 调用链 | 内核命令 |
|---|---|---|---|
| 加载 .kpm | KPM.kt:312 | `Natives.loadKernelPatchModule(path, args)` | SUPERCALL_KPM_LOAD 0x1020 |
| 卸载 | KPM.kt:529 | `Natives.unloadKernelPatchModule(name)` | 0x1021 |
| 控制（ctl 参数） | KPM.kt:486 | `Natives.kernelPatchModuleControl(name, param)` → KPMCtlRes{rc,outMsg} | 0x1022 |
| 列表 | KPModuleViewModel.kt:105-115 | `kernelPatchModuleNum/List/Info` | 0x1030-32 |
| 持久化 KPM | KPM.kt:343,660-662 | 目录 `/data/adb/ap/kpm/<id>/<id>.kpm`，enable/disable 靠 `<id>/disable` 标记文件（rootShellForResult touch/rm） | 文件系统 + apd 脚本 |

### 4.2 修补期嵌入入口

- `PatchesViewModel.embedKPM(uri)`：`kptools -l -M <kpm>` 解析元信息（name/version/license/author/desc）→ 列入 newExtras → doPatch 以 `-M file -A args -V event -T kpm` 传给 boot_patch.sh 嵌入内核镜像。
- 已修补镜像的存量 KPM：parseBootimg 枚举 `extra N` 段显示为 existedExtras，可用 `-E name` 保留/调整。

### 4.3 与 Q2 决策的衔接

人类已定：KPM 能力保留、先搁置、后期讨论。APK 侧无需预先拆除 KPM 入口；但 SKRoot 修补引擎若不实现 KPM 扩展段，`-M/-E` 参数链与 KPMS_DIR 持久化将失效——**需在修补引擎契约中明确 KPM 是否受支持**，APK 侧届时按契约隐藏或保留 KPM 页（建议契约给"能力位"，UI 动态显隐）。

---

## 5. 附：模块管理 UI 与 AP 二进制的对接面（供系统守护组对齐）

- 全部模块操作走 **`APatchCli.execApd()` → `/data/adb/apd module <subcmd>`**（APatchCli.kt:200-260）：`module list / install / enable / disable / uninstall / undo-uninstall / action`；`soft-reboot` 亦经 apd。
- 模块安装：`installModule(uri, MODULE_TYPE.APM)` 拷 zip 到 cache → root shell `apd module install <path>`，stdout/stderr 回调流式进 Install.kt 日志页。
- **sepolicy.rule：APK 侧零引用**（grep 全 app 模块无命中）——规则注入完全由 apd/内核侧处理。Q4=B 要求的「含 sepolicy.rule 模块安装时展示隐藏性不作保证警示」需在 `Install.kt` 安装确认对话框（11282 新增）中加检测：安装前读 zip 内 `sepolicy.rule` 存在即弹警示。此为 pl_56e797e81aac672c 实现阶段工作项。
- 模块 WebUI：`WebUIActivity` + `WebViewInterface` + `SuFilePathHandler`（root 只读文件代理）。
- Root IPC：`services/RootServices.java`（libsu RootService）仅做包列表查询，无 superKey 依赖。

---

## 6. 结论与风险

1. **修补链可替换性好**：SK 密钥只需在「密钥生成方 + kptools -S 契约 + 校验 UI」三点替换，脚本/模式/备份/槽位机制可整体保留；kptools/kpimg 属内核域，契约先行。
2. **鉴权链是最大改造面**：superKey 贯穿 setter→存储→shell→JNI→内核 20 个 supercall，且内核 kstorage 存授权列表——第二阶段「鉴权切换设计」必须与 SKRoot 调用契约（系统守护组输出）联合设计，APK 侧单改无意义。
3. **上游趋势冲突**：上游正在"去 superkey 化"（su 隐式授权），与 SPRoot 48 位密钥方向相反；基线升级策略需在版本管理条目（pl_dd267ef011b5d4bc）中明确我们固定 skroot 分叉点，不盲目跟上游。
4. **来源风险**：本文基于 gh-proxy.com 镜像副本（tag 11282 / e8e4651c），第二阶段设计前必须完成标准地址复核（leader 已确认流程）。

---

*文档完 · 客户端 · 2026-11-06*
