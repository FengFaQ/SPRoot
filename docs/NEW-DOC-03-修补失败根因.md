# NEW-DOC-03 旧链路「点击修补 → 一闪即报错」根因报告

- 文档编号：NEW-DOC-03（任务 task-2）
- 作者：patcher（修补组）
- 状态：v1.1（与 lead 已确证的根因清单、`docs/research/patch-failure-rootcause.md`、契约 v1.1 冻结版对齐）
- 结论一句话：**不是某个偶发 bug，而是「交付物缺失 + 静默降级 + 错误不可见」三条链同时成立**——修补引擎从未被打包进 APK，缺件时没有任何能力自检，失败后日志面板被条件立即卸载，用户只看到终端闪一下。

---

## 1. 根因排序（按「必然发生 / 概率」从高到低）

| # | 根因 | 严重度 | 必然性 | 证据（文件:行） |
|---|---|---|---|---|
| **R1** | **`./skroot-patch` 引擎根本不在 APK 内**：客户端 prepare 只对 nativeLibraryDir 里"已存在"的 lib 建 symlink（缺件静默跳过），`boot_patch.sh` 硬检查 `./skroot-patch` 不存在即 `exit 1` | 阻断 | **必然** | `app/app/src/main/java/me/bmax/apatch/ui/viewmodel/PatchesViewModel.kt:88-104`；`app/app/src/main/assets/boot_patch.sh:76-79`；全仓 `libskrootpatch*` 0 命中（`grep -rn libskrootpatch` 仅注释/字符串引用，无文件） |
| **R2** | **唯一产物是宿主 x86-64 ELF**，即使手工塞进 `libs/arm64-v8a/` 也无法在 arm64 上 exec（ENOEXEC） | 阻断 | **必然** | `engine/src/skroot-patch`：`ELF 64-bit LSB pie executable, x86-64`、`e_machine=0x3E`、`interpreter /lib64/ld-linux-x86-64.so.2`；`engine/src/Makefile`（原）`CXX := g++`、无 `--target=aarch64`、无 `-static` |
| **R3** | **关键外部件靠构建期网络下载**：`libkptools.so` / `kpimg` 被 `.gitignore` 排除，由 `preBuild` 从 GitHub Releases 拉；下载失败不 fail build → 半残 APK | 阻断 | 高（网络受限时必然） | `app/app/build.gradle.kts:243-301`（`registerDownloadTask` 无失败硬断言）；`app/app/src/main/assets/.gitignore`（`kpimg`）、`app/app/libs/arm64-v8a/.gitignore`；实测仓内 `app/app/libs/arm64-v8a/` 只有 `libbusybox.so`、`libbootctl.so` |
| **R4** | **`kpimg` 是资产硬依赖，但 SK 链根本不需要**：脚本已无 `-k kpimg`，但 prepare 仍 `assets.open("kpimg")` → 缺文件抛 `FileNotFoundException` → `prepare failed` | 阻断 | 必然（当前仓态） | `PatchesViewModel.kt:107-112`；`boot_patch.sh` 已无 `kpimg` 引用；`app/app/src/main/assets/` 实存 6 个文件无 `kpimg` |
| **R5** | **release 包永远拿不到密钥**：`SKKeyProvider` 仅 `BuildConfig.DEBUG` 分支生成密钥 | 阻断（release） | 必然 | `app/app/src/main/java/me/bmax/apatch/util/SKKeyProvider.kt:26-31` |
| **R6** | **失败后 UI 立刻卸载日志**：失败分支只置 `patching=false`，`patchdone` 仅成功时置位 → 日志面板条件消失；真实 stderr 被注释掉 | 高（诊断不可用，非失败原因） | 必然 | `PatchesViewModel.kt:477-485` vs `:565`；`:480` 注释掉 `error += result.err`；`Patches.kt:261` 面板可见性条件 |
| **R7** | **脚本退出码自吞**：`echo ...; exit $?`（`$?` 是 echo 的 0）、abort 用 `exit 0`、repack 的 rc 被后续 `if` 覆盖 | 高（把失败变"成功"） | 必然（触发时） | `boot_patch.sh:51-54,57-62,84-87,98-101,108-111` |
| **R8** | **命名不一致**：prepare 生成 `skrootpatch`（`libskrootpatch.so`.substring(3,len-3)），脚本检查 `./skroot-patch` | 中（同因 R1） | 必然（若有文件） | `PatchesViewModel.kt:96-98`（`substring(3, length-3)`）vs `boot_patch.sh:76` |

> 与 `docs/research/patch-failure-rootcause.md` 的 P0-a/P0-b/P0-c/P1/P2/P3 一一对应；本文按"旧链路一层的修复动作"重新归并为 R1–R8，并补上 R2 的 ELF 实证与 R8 的命名实证。

---

## 2. 失败链路还原（时序）

```
点击「开始修补」
 └─ doPatch(PATCH_ONLY)                                    PatchesViewModel.kt:395
    └─ ensurePrepared() → prepare()                        :86-114
       ├─ patchDir = filesDir.parent/"patch"，删库重建      :75
       ├─ 遍历 nativeLibraryDir 过滤 execs 名单并 symlink    :97-104
       │   └─ libskrootpatch.so 不存在 → **静默跳过**        ← R1
       └─ assets.open("kpimg")                             :107-112
           └─ 不存在 → FileNotFoundException → "prepare failed" ← R4
    └─ ProcessBuilder(["sh","-c","./busybox sh boot_patch.sh \"$0\" \"$@\"", key, boot])  :434-463
       └─ boot_patch.sh
          ├─ :42  `command -v ./kptools` → 缺 → echo + exit 1   ← R3
          └─ :76  `[ ! -f ./skroot-patch ]` → 缺 → exit 1       ← R1/R8
    └─ rc != 0 → error=" Patch failed."（stderr 被丢弃）      :477-480  ← R6
    └─ patching=false，patchdone 未置位 → 日志面板卸载        :483  ← R6
```

**「一闪」的物理解释**：日志面板仅在 `patching || patchdone` 时可见（`Patches.kt:261`）。失败路径两者都不成立，面板在用户读到内容前就被 Compose 重组成空 → 用户只感知到"终端闪一下 + 报错"，且看不到任何具体原因（原因只写进内存 `patchLog`，随之销毁）。

---

## 3. 失败模式清单 → 新管理器测试项（可直接转 QA 用例）

| # | 失败模式 | 旧实现证据 | 新管理器约束 | 可执行测试项 |
|---|---|---|---|---|
| F1 | 外部可执行缺失静默继续 | `PatchesViewModel.kt:97-104` | 入口**能力自检**：文件存在 + 可执行位 + ELF `e_machine=0xB7`；缺一项即 UI 阻断 | T-01 删掉/改名引擎文件 → 点修补必须给出"缺 engine，期望路径/大小/sha256"，不得进入执行阶段 |
| F2 | 交付形态与契约脱节 | `engine/src/skroot-patch` 为 x86-64；jniLibs 空 | 产物由本仓 CI 构建 + Gradle `assertEngine` 硬断言 AArch64 | T-02 CI 故意把 x64 二进制放到 jniLibs → 构建必须失败 |
| F3 | 构建期下载当交付 | `build.gradle.kts:243-301` | 入包件全部随仓/本仓构建；下载仅允许带 sha256 且失败即 fail build | T-03 断网构建 → 必须成功（不依赖外网） |
| F4 | 非必需资产成硬依赖 | `PatchesViewModel.kt:107-112` | kpimg 等元信息资产缺失只降级 UI | T-04 删除 kpimg → 修补仍可用，仅少了版本展示 |
| F5 | 失败日志不可见/不落盘 | `Patches.kt:261`、`PatchesViewModel.kt:477-485` | 日志落盘 + 失败后 UI 常驻可复读/可复制 + 显示退出码 | T-05 构造 rc=102 → UI 必须显示"E102 + 首行错误 + 日志路径"，且刷新后仍可读 |
| F6 | 退出码被 echo 吞掉 | `boot_patch.sh:51-54` 等 | 取消 shell 脚本层（引擎进程内/直 exec），引擎自报退出码 | T-06 断言新链无 `sh -c`/无 `exit $?`；错误码透传到 UI |
| F7 | 未校验产物即报成功 | `boot_patch.sh:103-117` | 成功判据 = 产物存在 + 等长 + report 回读 | T-07 篡改产物大小 → 必须报失败（C1 §2.1） |
| F8 | 能力绑死 DEBUG | `SKKeyProvider.kt:26` | 密钥由引擎 `--gen-key` 产出，与构建类型无关 | T-08 release 包跑修补 → 必须能生成 48 位密钥并成功 |
| F9 | exec 位置被 SELinux 拦 | `PatchesViewModel.kt:75,103`（symlink 进 filesDir） | 优先 JNI `.so`；如需 exec 必须 `nativeLibraryDir` 真实路径 | T-09 exec 路径断言：不得出现 `filesDir` 下的可执行文件 |
| F10 | 壳层传参歧义 | `PatchesViewModel.kt:416,434-439` | 单一调用路径 + `ProcessBuilder(listOf(...))` | T-10 code review 门禁：禁止 `sh -c` 拼串 |
| F11 | 环境不可复现 | `:459-461` | 日志记录 cmdline/env/cwd；stdout/stderr 分开收 | T-11 日志必须含 cwd + 完整 argv + 退出码 |

---

## 4. 最小可验证修补链（新管理器 MVP）

```
[0] 能力自检（不落盘）
    engine(JNI load 或 nativeLibraryDir ELF/AArch64) + bootimg 解析内置
    失败 → UI 阻断，展示缺失项（F1/F2）
[1] 取 boot：/dev/block/by-name/boot* 或用户选择 boot.img
    校验：大小 == 分区大小；sha256 记录（备份内容寻址）
[2] 内置 bootimg 解析（engine --bootimg info/extract-kernel）    ← 已实现，见 NEW-DOC-02 §7
    校验：header_version ∈ {3,4}；kernel_size>0；compression=raw（否则先解压）
[3] 引擎 patch：--in kernel --out kernel.patched --key K --json ev.ndjson
    校验：rc ∈ 契约枚举；rc≠0 ⇒ out 不存在；size 相等（C1 §2.1）
[4] 内置 repack：--bootimg repack --kernel kernel.patched --out new-boot.img
    校验：boot.img 等长；kernel 段字节 == patched
[5] 刷入/导出：A/B 机 `--slot=all`；AVB 校验必须关闭（NEW-DOC-02 §7.3）
    校验：刷入前落盘原 boot 备份；刷入后回读分区 sha256 比对
```

**已用魅族 21 官方 boot.img 全链实测通过**（详见 NEW-DOC-02 §6.2/§7）：
kernel 抽取 sha256 与 qa ground truth 一致（`4d445384…`），补丁 rc=0、等长、可确定性复现，重打包 boot.img 等长。

---

## 5. 结论与责任边界

1. **旧链路不可修**：R1（缺产物）+ R2（架构错）+ R3（外部下载）+ R4（kpimg 耦合）+ R5（密钥绑 DEBUG）是**设计层**缺陷，逐条打补丁的代价高于重写修补链——这正是新项目"从零实现 SP 管理器、手机本地修补"的依据。
2. **新链的防线**：①产物由 CI 构建 + 构建期断言 AArch64（R1/R2 根治）②bootimg 解析/重打包内置（R3 根治）③单一调用路径 + 失败不落盘（R7）④日志落盘 + UI 常驻（R6）⑤密钥不绑 DEBUG（R5）。
3. **仍需实测的门禁**：刷入后能否开机（AVB/vbmeta 策略，NEW-DOC-02 §7.3）、真实 bionic 下引擎运行（NEW-DOC-02 §6.4）。

---

## 附录：证据复现命令

```bash
# R1/R8：引擎从未入包（全仓 0 命中文件）
grep -rn "libskrootpatch" --include=*.kt --include=*.kts --include=*.sh . | grep -v '//'
ls app/app/src/main/jniLibs 2>&1        # No such file or directory
ls app/app/libs/arm64-v8a/              # 只有 libbusybox.so libbootctl.so

# R2：唯一产物架构
file engine/src/skroot-patch           # x86-64 + interpreter /lib64/ld-linux-x86-64.so.2
readelf -h engine/src/skroot-patch | grep Machine   # X86-64

# R3/R4：资产缺失与忽略规则
cat app/app/src/main/assets/.gitignore # kpimg
cat app/app/libs/arm64-v8a/.gitignore
grep -n "registerDownloadTask" -A3 app/app/build.gradle.kts | head -20

# R5：密钥绑 DEBUG
sed -n '20,32p' app/app/src/main/java/me/bmax/apatch/util/SKKeyProvider.kt

# R6：日志面板条件与 stderr 丢弃
sed -n '475,486p' app/app/src/main/java/me/bmax/apatch/ui/viewmodel/PatchesViewModel.kt
grep -n "patching || patchdone\|patchLog.isNotEmpty" app/app/src/main/java/me/bmax/apatch/ui/screen/Patches.kt

# R7：退出码自吞
sed -n '48,60p;82,101p' app/app/src/main/assets/boot_patch.sh
```
