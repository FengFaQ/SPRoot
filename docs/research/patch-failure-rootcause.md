# 修补链「点击修补 → 终端一闪即报错终止」根因调研

- 调查对象：旧方案（改造 APatch 管理器 APK），基线 `app/` 仓 commit `e8e4651`
- 工作目录：`SPRoot/`；`app/` 是 git 仓根，`app/app/` 是 Gradle module（下文路径以 `SPRoot/` 为根）
- 取证方式：静态代码/产物取证（无设备 logcat、工作区无构建出的 APK）→ 文中逐条标注 **已证实** / **推断**

---

## 一、结论（按概率排序）

### P0-a（最可能，已证实存在、必现）`skroot-patch` 引擎根本没有被打包进 APK

`boot_patch.sh` 已把上游的 `kptools -p ... -k kpimg` 换成外部可执行文件 `./skroot-patch`，执行前有硬性存在性检查，缺失即 `exit 1`：

```sh
# app/app/src/main/assets/boot_patch.sh:76-81
if [ ! -f ./skroot-patch ]; then
  >&2 echo "- SPRoot: skroot-patch engine not found in patch dir!"
  exit 1
fi

./skroot-patch --in kernel.ori --out kernel --key "$SUPERKEY"
```

而客户端把 `skroot-patch` 的来源指向一个**不存在**的 native 库：

```kotlin
// PatchesViewModel.kt:89-104
val execs = listOf(
    "libkptools.so", "libbusybox.so", "libkpatch.so", "libbootctl.so",
    // SPRoot(P2): skroot-patch 修补引擎（DOC-C1 §7-D 会签：独立 ELF，jniLibs 内置）
    "libskrootpatch.so"
)
val libs = File(info.nativeLibraryDir).listFiles { _, name -> execs.contains(name) } ?: emptyArray()
for (lib in libs) {
    val name = lib.name.substring(3, lib.name.length - 3)
    Os.symlink(lib.path, "$patchDir/$name")
}
```

- `app/app/libs/arm64-v8a/` 实存文件只有两个：`libbootctl.so`(154248 B)、`libbusybox.so`(1710600 B)（**已证实**，目录列表）。
- 全仓（含 `backup/`、`quality/`、`SPRoot-AP/`）按名字搜索 `libskrootpatch*` / `*skrootpatch*`：**0 命中**（**已证实**）。
- `app/app/src/main/jniLibs/` 目录**不存在**（**已证实**）；即使按注释所说放到 jniLibs，`libs` 才是本工程 jniLibs 源（`app/app/build.gradle.kts:161 jniLibs.directories += "libs"`）。
- `app/app/build.gradle.kts` 中**没有任何**把 `engine/src/skroot-patch` 拷进 `libs/` 的任务；grep `engine|skroot` 于 `build.gradle.kts`：**0 命中**（**已证实**）。现有 preBuild 依赖只有 `downloadKpimg / downloadKptools / downloadCompatKpatch / downloadJailbreakKo / mergeScripts`（`build.gradle.kts:296-303`）。
- `libs/arm64-v8a/.gitignore` 只忽略 `libkptools.so / libapjni.so / libkpatch.so / libapd.so`，**不含** `libskrootpatch.so` → 该文件不是「构建生成物被忽略」，而是**从未被任何环节产出**（**已证实**）。

因此 `prepare()` 的 `listFiles{}` 过滤不到 `libskrootpatch.so`，**静默跳过** symlink（无日志、无异常），`patchDir` 里始终没有 `skroot-patch`；`boot_patch.sh` 走到第 76 行即 `exit 1`。这与「输出闪一下（脚本前几行 banner + unpack 日志）→ 报错终止」的现象完全一致（**推断**现象对应关系）。

### P0-b（次可能，已证实是构建期网络依赖）`kptools` 时有时无

`libkptools.so` 同样不在仓里，由 Gradle 从 GitHub Releases 下载（**已证实**）：

```kotlin
// app/app/build.gradle.kts:245-250
registerDownloadTask(
    taskName = "downloadKptools",
    srcUrl = "https://github.com/bmax121/KernelPatch/releases/download/$kernelPatchVersion/kptools-android",
    destPath = "${project.projectDir}/libs/arm64-v8a/libkptools.so",
    project = project
)
```

若构建机无外网/下载失败但任务被 skip，APK 内无 `libkptools.so` → `prepare()` 静默跳过 → `boot_patch.sh:42` 在打印 banner 后**立刻** `exit 1`：

```sh
# boot_patch.sh:42
command -v ./kptools >/dev/null 2>&1 || { >&2 echo "- Command kptools not found!"; exit 1; }
```

这条的输出比 P0-a 更短（只有 3 行 banner），更贴近「闪一下」。二者只能靠 logcat 首行区分（见 §五）。

### P0-c（条件性必现，已证实）release 构建根本无法进入修补：SK 密钥恒为 null

```kotlin
// SKKeyProvider.kt:24-31
fun currentKey(): String? {
    cached?.let { return it }
    if (!BuildConfig.DEBUG) return null
    return generateKey().also { ... }
}
```

`doPatch` 拿不到密钥就直接返回（`PatchesViewModel.kt:421-429`）。若用户装的是 **release** 包，点击修补不会有任何终端输出，只有一行 `SPRoot: SK key unavailable, patch aborted` —— 与「终端闪一下」不符，故反推用户装的是 debug 包（**推断**）。但这是新管理器必须修掉的硬伤：能力被 `DEBUG` 开关绑死。

### P1（已证实是"错误不可见"的直接原因，不是失败原因）失败后日志面板立即卸载，只留一句 `Patch failed.`

```kotlin
// PatchesViewModel.kt:477-485（失败分支）
if (!succ) {
    val msg = " Patch failed."
    error = msg
//  error += result.err.joinToString("\n")     ← 真实 stderr 被注释掉
    logs.add(error)
    logs.add("****************************")
    patching = false
    return@launch
}
```

- `patchdone` 只在**成功尾部**（`:565`）和 `doUnpatch`（`:383`）置 true；失败分支没有置 true。
- UI 日志面板的可见条件是 `patching || patchdone`：

```kotlin
// Patches.kt:261-274
if (viewModel.patching || viewModel.patchdone) {
    SelectionContainer { Text(text = viewModel.patchLog, ...) }
}
```

失败 → `patching=false`、`patchdone=false` → **整块日志从 Compose 树中移除**，屏幕上只剩顶部 `ErrorView(" Patch failed.")`。这就是「终端输出一闪即消失」的机制（**已证实**代码路径）。
- 全程 `patchLog` 只存在于内存（`:407-413` 的 `CallbackList`、`:465-474` 的读线程），`Log.i(TAG, ...)` 写 logcat，**没有任何落盘**（**已证实**，全文件无文件写入日志的代码）。

### P2（已证实，会让"失败变成功"，方向相反但同样危险）脚本退出码自吞

`boot_patch.sh` **没有** `set -e`（只有 `set -x`/`set +x`，`:47`/`:50`），且多处 `>&2 echo ...; exit $?` 的 `$?` 是 `echo` 的状态：

```sh
# boot_patch.sh:51-54
if [ $patch_rc -ne 0 ]; then
    >&2 echo "- Unpack error: $patch_rc"
    exit $?          # ← echo 的 0，脚本以 0 退出
fi
# 同型 bug：:84-87（Patch error）、:98-101（Repack error）、:108-111（Flash error）
```

外加 `:57-62` 的「Patcher has Aborted!」也以 `exit 0` 收场；`:92-96` 的 KALLSYMS_ALL 检查夹在 `repack` 与 `if [ $? -ne 0 ]` 之间，导致 repack 的返回码**永远测不到**（`if` 语句自身状态被覆盖）。这些是上游 APatch 的原样继承（上游 `upstream/APatch/app/src/main/assets/boot_patch.sh:51,81,93-96` 同款），**不是本次根因**，但会让新管理器把失败当成功、把未刷入的 boot 当已刷入（**已证实**；对现象的影响为**推断**：若引擎以 `ENOEXEC` 失败，`:84-87` 会把 126 变成 0，App 反而显示成功）。

### P3（已证实是潜在崩溃点）`kpimg` 是资产硬依赖，但它已被 `.gitignore` 排除

```kotlin
// PatchesViewModel.kt:107-112
for (script in listOf("boot_patch.sh", "boot_unpatch.sh", "boot_extract.sh", "util_functions.sh", "kpimg")) {
    val dest = File(patchDir, script)
    apApp.assets.open(script).writeTo(dest)   // 缺一个就抛 FileNotFoundException
}
```

- `app/app/src/main/assets/` 实存只有：`.gitignore`、`UninstallAP.sh`、`boot_extract.sh`、`boot_unpatch.sh`、`util_functions.sh`、`InstallAP.sh`、`boot_patch.sh` —— **没有 `kpimg`**（**已证实**）；`assets/.gitignore` 第 1 行就是 `kpimg`，靠 `downloadKpimg` 构建期下载。
- 而 SK 链路**根本不需要 kpimg**：新版 `boot_patch.sh` 已不再出现 `-k kpimg`（上游第 75 行有，SPRoot 版被删）。即「为了展示 kpimg 元信息」把整个修补链拖成一个硬依赖；缺 kpimg 时 `ensurePrepared()` 捕获异常后只留 `error = "prepare failed: ..."`（`:274-277`），连终端都不会出现（**已证实**代码路径）。

### 结论判定

> 最可能根因：**P0-a** —— `boot_patch.sh` 新增的 `./skroot-patch` 在 APK 内不存在（从未被打包/构建），`prepare()` 缺文件时静默跳过 symlink，脚本第 76 行 `exit 1`；失败后日志面板被 `patching/patchdone` 条件立即卸载、真实 stderr 被丢弃，于是用户只看到「闪一下 + 报错」。
> 并列第二：**P0-b**（`libkptools.so` 构建期下载失败）——同一条静默跳过 + 立即 `exit 1` 链路，输出更短。
> 阻断性硬伤：**P0-c**（release 无密钥，永不修补）。

---

## 二、证据链

### 2.1 修补链实际调用顺序（已证实）

1. `Patches.kt:250-252` 点击「开始修补」→ `viewModel.doPatch(mode)`
2. `PatchesViewModel.doPatch`（`:395`）→ `ensurePrepared()`（`:398`）→ `prepare()`（`:86`）：
   - `patchDir = /data/data/<pkg>/patch`（`:75`，`filesDir.parent`）
   - 删目录重建 → 从 `nativeLibraryDir` 过滤 `execs` 并 `Os.symlink` → `kpimg` 等资产落盘
   - `parseKpimg()`：`cd $patchDir; ./kptools -l -k kpimg`（`:117-119`）
   - 安装类模式再 `extractAndParseBootimg()`（`:224-254`，走 `./busybox sh ./boot_extract.sh`）
3. `ProcessBuilder` 直跑 shell（`:450-463`），`directory(patchDir)`、`ASH_STANDALONE=1`、`redirectErrorStream(true)`
4. `PatchesViewModel.kt:416/434-439` 组命令：
   - `PATCH_AND_INSTALL` / `INSTALL_TO_NEXT_SLOT`：`["./busybox","sh","boot_patch.sh", <skKey>, <bootimg>, "true"]`
   - `PATCH_ONLY`：`["sh","-c","./busybox sh boot_patch.sh \"$0\" \"$@\"", <skKey>, <bootimg>]`
5. `boot_patch.sh`：banner(:27-29) → 参数校验(:37-38) → kptools 检查(:42) → `kptools unpack`(:48) → KALLSYMS 检查(:57-62) → `mv kernel kernel.ori`(:69) → **`skroot-patch` 检查(:76-79)** → 引擎(:81) → `kptools repack`(:90) → 刷入或导出(:103-117)

### 2.2 缺失文件矩阵（已证实）

| 文件 | 仓内是否存在 | 期望来源 | 缺失后果 |
|---|---|---|---|
| `libkptools.so` | ❌（gitignore） | Gradle `downloadKptools`（GitHub Releases，需外网） | `boot_patch.sh:42` exit 1 |
| `libbusybox.so` | ✅ 1.71 MB（提交入库） | 仓内 | — |
| `libbootctl.so` | ✅ 154 KB（提交入库） | 仓内 | — |
| `libkpatch.so` | ❌（gitignore） | Gradle `downloadCompatKpatch` | 仅旧内核兼容分支用（本版已删）→ 无影响 |
| `libapd.so` | ❌（gitignore） | `cargo ndk` + `buildApd` 任务 | 守护进程缺失，与修补无关 |
| **`libskrootpatch.so`** | ❌ **全仓 0 命中** | **无任何任务/脚本产出** | `boot_patch.sh:76` exit 1 |
| `kpimg`（assets） | ❌（gitignore） | Gradle `downloadKpimg` | `prepare()` 抛异常，`prepare failed` |
| `new-boot.img` | 运行时由 `kptools repack` 生成 | kptools | `PATCH_ONLY` 导出失败 |

`build.gradle.kts:129-131 useLegacyPackaging = true`：native 库会被解压到 `nativeLibraryDir`，因此 `File(info.nativeLibraryDir).listFiles{}` 对**已入库**的 `libbusybox.so/libbootctl.so` 可用（**已证实**配置；运行时行为为**推断**）。这也说明 symlink 方案本身没坏，坏的是「有的文件根本不在」。

### 2.3 `engine/src/skroot-patch` 这个唯一产物也不能在手机上跑（已证实）

```
offset 0:  7f 45 4c 46 02 01 01 03 | e_type=0x0003 (ET_DYN/PIE)
offset 16: 03 00 3e 00             | e_machine=0x003e = EM_X86_64  ← 不是 AArch64(0xB7)
字符串段: 含 "ld-linux"（glibc 动态解释器），无 libc.so.6/libpthread 常规串
大小: 2,121,080 B，位于 engine/src/skroot-patch（非 app 仓，不进 APK）
```

构建脚本也印证：`engine/src/Makefile:5-6 CXX := g++ / CC := gcc`、`:25 CXXFLAGS := -std=c++20 -O2 -Wall`、`:27 LIBS := -ldl -pthread` —— **无 `--target=aarch64-linux-android`、无 `-static`、无 NDK clang**（**已证实**）。即：这是**宿主 x86-64 glibc 版**，即便有人手工改名塞进 `libs/arm64-v8a/libskrootpatch.so`，在 arm64 Android 上 `execve` 也会以 `ENOEXEC`/解释器缺失失败（**推断**，机制确定）。

综合：`engine/src/skroot-patch` 是「能编译、能在 PC 上跑」的中间产物，**既未交付到 APK，架构也不对**。这是 DOC-C1 §7-D 会签「独立 ELF，APK jniLibs 内置」的**落地缺口**（契约见 `docs/SPROOT-DOC-C1-修补引擎IO契约.md:179`）。

引擎参数面与脚本调用是一致的（`engine/src/skroot_patch_main.cpp:164-169` 解析 `--in/--out/--key/--gen-key/--json/--workdir`），并且**主动拒绝** `.img`/`.gz`：

```
L194-195: if (fs::exists(in) && (ext == ".img" || ext == ".gz"))  → E102 "unpack & decompress first"
```

→ 新链必须保证 `kptools unpack` 产出的 `kernel` 是**已解压的 Image**（**推断**，需在真机确认 kptools unpack 是否已解压）。

### 2.4 「闪一下」的机制（已证实）

- 日志仅内存：`patchLog`（`PatchesViewModel.kt:73`）；写入点 `:409-411`（CallbackList）、`:469-471`（读线程）。
- 面板可见性：`Patches.kt:261`。失败分支 `patching=false` 且 `patchdone=false`（`:483`）→ 面板消失。
- 错误文本：`error=" Patch failed."`（`:478`），真实 stderr 在第 480 行被注释掉。
- 结论：**既没有持久化日志，也没有把退出码/原因带到 UI**。这正是「一闪即报错、查不到原因」的产品级原因。

---

## 三、失败模式清单（新管理器必须逐条规避）

| # | 失败模式 | 旧实现证据 | 新管理器要求 |
|---|---|---|---|
| F1 | **外部可执行缺失却静默继续**：`prepare()` 对 `execs` 里每个名字没有存在性校验，缺了就少建一个 symlink，错误延迟到脚本层一行 `echo` | `PatchesViewModel.kt:97-104`；`boot_patch.sh:42,76` | 启动/点击时做**能力自检**：逐项校验 `kptools`/引擎/解包器存在 + 可执行 + ELF 头 `e_machine=0xB7`；缺一项即在 UI 阻断并给出「缺什么、期望放哪、怎么装」，**绝不进脚本** |
| F2 | **引擎交付形态与契约脱节**：契约写「独立 ELF 内置 jniLibs」，仓库里既无该文件也无打包任务；唯一产物是宿主 x86-64 | `PatchesViewModel.kt:90-93`；`docs/SPROOT-DOC-C1…:179`；`engine/src/skroot-patch` ELF 头 | 二选一并写进构建：① **首选** 引擎以 aarch64 **Android .so + JNI** 交付（见 §四）；② 若坚持独立 ELF，必须 `ndk clang --target=aarch64-linux-android -static` 产出，并加 CI 断言（`file`/ELF 头校验）**且**有 Gradle 拷贝任务 + `.gitignore` 之外的入包校验 |
| F3 | **构建期网络下载当交付**：`kptools`/`kpimg` 靠 `preBuild` 从 GitHub 拉；失败即「半残 APK」 | `build.gradle.kts:237-260,296-303`；`assets/.gitignore`、`libs/arm64-v8a/.gitignore` | 所有植入手机的可执行文件**要么入库、要么由本仓构建产出**；下载仅允许作为「带 sha256 校验 + 失败即 fail build」的辅助，禁止 skip-on-failure |
| F4 | **资产硬依赖与非必需资产耦合**：SK 链不用 `kpimg`，缺它却让 `prepare()` 崩 | `PatchesViewModel.kt:107-112`；`boot_patch.sh` 已无 `-k kpimg` | 元信息展示类资产降级为 optional（缺失仅隐藏 UI 字段），**不得**阻断修补 |
| F5 | **失败路径无日志落盘、UI 立即卸载日志** | `patching/patchdone` 条件 `Patches.kt:261`；`patchdone` 未在失败分支置位（`:477-485` vs `:565`） | 每次修补写 `/data/adb/<pkg>/logs/patch-<ts>.log`（命令、cwd、env、每行 stdout/stderr、退出码、产物清单）；UI 的日志视图**常驻可复读/可复制**，失败后仍停留并定位到错误行 |
| F6 | **错误码被 echo 吞掉**：`echo ...; exit $?`；`exit 0` 表示 abort；检查被后续命令覆盖 | `boot_patch.sh:51-54,57-62,84-87,98-101` | 脚本禁写 `exit $?`；统一 `rc=$?` → `exit $rc`；abort 用非 0；`set -eu` + `trap` 打印行号；repack/flash 的 rc 立即捕获 |
| F7 | **未校验产物就宣布成功**：脚本打印 "Successfully Patched/Flashed!" 与是否真的产出 `new-boot.img`、是否真的写了块设备无关 | `boot_patch.sh:103-117`（`-b || -c && -f` 优先级坑；非块设备时 else 分支静默跳过仍打印 Flashed） | 成功判据必须是**可验证事实**：`new-boot.img` 存在且大小合理 + 回读 `kernel` 的 `patched=true` + 刷入后 `blockdev --getsize64` 一致；否则报失败 |
| F8 | **能力被 BuildConfig.DEBUG 绑死**：release 永远拿不到密钥 | `SKKeyProvider.kt:26` | 密钥来源要么由引擎 `--gen-key` 产出并落安全存储，要么由用户输入；**不得**依赖 debug 分支 |
| F9 | **exec 位置选错会被 SELinux 拦**：Android 10+ 对 app 私有目录（`filesDir`）的可执行文件执行受限，靠「symlink 到 nativeLibraryDir」绕开 | `PatchesViewModel.kt:75,103`（当前之所以能跑，是因为 symlink 目标在 `nativeLibraryDir`=apk_lib 上下文） | 新管理器**不要**把 ELF 放进 `/data/data/<pkg>/…` 直接 exec；优先 JNI 加载 `.so`；若必须 exec，目标须在 `nativeLibraryDir`，并在失败时区分「ENOEXEC / EACCES / 解释器缺失」三类错误码（**推断**，需真机实测确认当前策略） |
| F10 | **壳层传参歧义**：`PATCH_ONLY` 用 `sh -c "./busybox sh boot_patch.sh \"$0\" \"$@\""` 传参，`PATCH_AND_INSTALL` 走 ProcessBuilder 直传，两条路径参数语义不同；且 `shift 2` 后 `"$@"` 残留 `true` 会被透传给 `kptools unpack` | `PatchesViewModel.kt:416,434-439`；`boot_patch.sh:34,48` | 只保留**一条**调用路径：直接 `ProcessBuilder(listOf(engine, ...))` 或直接 `execve`，不要经过 `sh -c` 拼串；不要往 `unpack` 透传位置参数 |
| F11 | **日志/环境不可复现**：`ASH_STANDALONE=1`、cwd、`PATH` 未记录；`set -x` 追踪混在 stdout/stderr 里被 merge | `PatchesViewModel.kt:459-461`；`boot_patch.sh:47,50` | 记录完整 cmdline + env + cwd；stdout/stderr **分开**收（诊断需要原始 stderr）；`set -x` 输出只进日志文件不进 UI 主视图 |

---

## 四、新管理器「本地修补」的最小依赖集

### 4.1 现状对照（已证实）

- APatch 旧链的最小集实际是：`kptools`（unpack/repack/patch 三合一）+ `kpimg`（KP 核心镜像）+ `busybox`（脚本壳）+ 一个 patch 引擎；四个外部件里两个靠构建期下载，一个被 SK 链废弃却仍是硬依赖。
- SK 上游（`upstream/SKRoot-linuxKernelRoot/Pro(众测开放中)/SKRootPro_4.6.7.apk`）**根本没有 shell/busybox/kptools 脚本链**：`lib/arm64-v8a/` 只有 `libpermissionmanager.so`(10 MB)、`libdfroot.so`、`libresetprop.so`；`assets/` 只有一个 `ZygotePreloadTools.apk`。即 **引擎以 JNI 库形态在进程内跑**（**已证实** apk 条目；「JNI 调用」为**推断**，与 A1 §9-2「并入同一 so 导出」的倾向一致）。

### 4.2 建议的最小集

```
必需（3 件，全部入库/本仓可构建）
1) 引擎：aarch64-android .so，JNI 导出（System.loadLibrary），参数面照 DOC-C1
   --in <decompressed Image> --out <patched Image> (--gen-key|--key K) [--json <ndjson>]
2) boot.img 解包/重打包：magiskboot(静态 aarch64) 或 kptools 的 bootimg 子命令 —— 二者选一，入库+校验 sha256
   （DOC-C1 §0 已明确：unpack/repack 不属于引擎职责）
3) 无 busybox：用 /system/bin/sh + toybox 的 grep/sed/tr，或干脆全部逻辑进 JNI
可选（缺失只降级 UI，不阻断）
4) kpimg            → 仅"引擎版本信息"展示，缺失必须可继续
5) bootctl          → 仅切槽/OTA 场景
```

关键取舍：**不要**再让「独立 ELF + shell 脚本 + symlink + 资产下载」四层耦合；新链的最小可判定失败面越短越好。

---

## 五、建议的最小可验证修补链（含验证方法）

### 5.1 链路（每步都有可断言的成功判据）

```
[0] 能力自检（UI 入口，失败即阻断，不落盘任何东西）
    - engine.so 可 load（System.loadLibrary 成功）
    - 解包器存在 且 ELF 头 e_machine==0xB7 且可执行
    - 若需密钥：引擎 --gen-key 能返回 48 字符（不依赖 DEBUG）
[1] 取 boot：优先分区设备(/dev/block/by-name/boot*)读原始字节 → 落 workDir/boot.img
    校验：文件大小 == 分区大小；sha256 记录
[2] unpack（magiskboot/kptools）→ kernel（+ramdisk/dtb）
    校验：kernel 存在；`kptools -l -i kernel` 或自带头解析得 banner 非空
[3] 引擎 patch：--in kernel --out kernel.patched --key K --json events.ndjson
    校验：退出码∈契约枚举；rc!=0 ⇒ out 必须不存在（C1 §3 铁律）
    校验：size(kernel.patched)==size(kernel)（字节级 patch 不改大小，C1 §2.1）
[4] repack → new-boot.img
    校验：new-boot.img 存在、头部魔数 == ANDROID!、size ≤ 分区 size
[5] 回读校验：`kptools -l -i kernel.patched | grep patched=true`
    校验：report.json 的 root_key == 传入的 K（防"刷了别人的 key"）
[6] 落盘/刷入二选一：
    - PATCH_ONLY：导出到 Downloads 并给出路径
    - 刷入：仅当目标 `-b`；`blockdev --setrw` → `dd conv=notrunc,fsync` → 回读 sha256 == new-boot.img
    校验失败 ⇒ 必须报失败，不允许打印 "Successfully"
[7] 全程日志：/data/adb/<pkg>/logs/patch-YYYYmmdd-HHMMSS.log（cmd/env/cwd/每行/stdout-stderr 分列/rc/产物 sha256）
```

### 5.2 必须在真机/CI 上验证的断言

| 断言 | 目的 | 现行反例 |
|---|---|---|
| APK 内 `lib/arm64-v8a/` 必须含引擎 .so 且 `readelf -h` 显示 `AArch64` | 防 F2 | `engine/src/skroot-patch` 是 x86-64，且未入包 |
| 断网构建一次，产物仍包含所有可执行依赖 | 防 F3 | `kptools`/`kpimg` 靠下载 |
| 删掉 kpimg 后修补仍能进行（仅 UI 少一行信息） | 防 F4 | 现在直接 `prepare failed` |
| 人为删掉引擎后点修补：UI 必须给出「缺引擎」而非「Patch failed.」 | 防 F1/F5 | 现在脚本里 `exit 1` + 面板瞬移 |
| 人为让引擎返回 126/1：App 必须显示失败且日志留存 | 防 F6/F7 | 现在 `exit $?`→0，可能误报成功 |
| release 包（`BuildConfig.DEBUG=false`）能完成一次完整修补 | 防 F8 | 现在恒 null |
| 复现命令（可在设备 shell 手动执行，用于定位是 P0-a 还是 P0-b） | 取证 | 见下 |

```sh
# 设备上（adb shell su）确认是哪一件缺失：
ls -l /data/data/<pkg>/patch/            # 有没有 skroot-patch / kptools
# 期望看到 skroot-patch、kptools、busybox、kpimg…；缺失项即命中 P0-a/P0-b/P3
adb logcat -s PatchViewModel:V SKKeyProvider:V PatchViewModel:I   # 首行 "- ... not found!" 即定位
```

---

## 六、局限与未完成项（不编造）

1. 工作区内**没有构建出的 APK**（`app/build/`、`app/app/build/` 均不存在；全仓仅 `upstream/` 下两个 SK 官方 APK），因此「该用户装的 APK 里 assets/libs 是否齐全」无法直接解包核对 → P0-a/P0-b/P3 的**必然性**只能由「源码 + 仓库文件 + 构建配置」推出（已证实），APK 级验证待补。
2. 无设备 logcat / `patchLog` 原文，P0-a 与 P0-b 的先后只能靠首行报错文本区分（见 §5.2 复现命令）。
3. `kptools unpack` 是否已解压 kernel（决定能否直接喂引擎）、以及 Android 10+ 对 `filesDir` 内 ELF 的 SELinux 执行策略，均未在真机验证（**推断**）。
4. 上游 `upstream/APatch` 版 `boot_patch.sh` 与本仓差异仅为：新增 SK 密钥必填校验、`echo "- Patching kernel (SPRoot SK engine)"`、`skroot-patch` 分支替换 `kptools -p`（`upstream/APatch/app/src/main/assets/boot_patch.sh:69-82` vs `app/app/src/main/assets/boot_patch.sh:71-87`）→ 本次回归的**唯一实质改动**就是引擎替换，指向 P0-a。
