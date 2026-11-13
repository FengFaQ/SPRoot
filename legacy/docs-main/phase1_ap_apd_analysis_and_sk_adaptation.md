# AP 系统模块（APM）与 apd 深度分析及 SK 适配设计

> 负责人：系统守护 · 第一阶段产出（计划条目 pl_74808cf5863ff842）
> 上游基线：**bmax121/APatch @ `e8e4651c16fb5075882a25813c8732285fdb3505`**（main，2026-10-13，"feat(app): confirm module installs and allow cancelling downloads"）
> 基线验证：gh-proxy 镜像克隆后，已通过标准 github.com 地址 `git fetch origin` 两次复核（2026-11-06 20:43 首次、21:12 复核），origin/main HEAD 与本地 HEAD 均为 `e8e4651c16fb5075882a25813c8732285fdb3505`，逐字节一致。获取途径：gh-proxy 临时通道克隆（当时标准地址不可用）→ 标准地址恢复后复核通过，后续拉取一律使用标准地址。
> 本文所有结论均注明源码位置（相对仓库根），供第二阶段《apd → AP 改造详细设计》直接引用。

---

## 0. 术语与范围

- **APM**：APatch 系统模块（Android Patch Module），zip 包形态，与 Magisk 模块同构（module.prop + system/ + 生命周期脚本）。
- **apd**：APatch 用户态守护进程/CLI，Rust 实现（`apd/` crate），部署路径 `/data/adb/apd`。它既是模块管理器，也是 KernelPatch supercall 的用户态发起者，还是 multicall 二进制（resetprop/magiskpolicy/su 均为它的 argv[0] 别名或符号链接）。
- **SK 化目标**：SPRoot 分支将 apd 更名/改造为 **AP** 二进制，root 鉴权从 KernelPatch SuperKey 改为 SKRoot 48 位 Root 密钥（由内核修补组实现），**模块管理功能行为保持不变**。
- 本文档不涉及内核侧（KPM、修补引擎），SK 内核机制以《SK Root 实现机制深度分析》（内核修补组，pl_47fd9709ab55326e）为准；本文只定义用户态契约的**消费方视角**。

---

## 1. APM 结构与安装流程，apd 承担的角色

### 1.1 模块目录布局（运行态）

```
/data/adb/                      ← ADB_DIR (apd/src/defs.rs)
├── apd                         ← 守护进程本体（SPRoot 中改为 AP）
├── ap/                         ← WORKING_DIR
│   ├── bin/                    ← BINARY_DIR：busybox、resetprop→apd 符号链接、magiskpolicy→apd 符号链接
│   ├── log/                    ← 开机 logcat/dmesg 抓取
│   ├── module_configs/<id>/    ← persist.config / tmp.config（模块配置 API）
│   ├── su_path                 ← 内核 su 路径注册文件
│   └── update                  ← UPDATE_FILE_NAME 标志：有模块待刷新
├── modules/<id>/               ← 生效模块（MODULE_DIR）
│   ├── module.prop
│   ├── system/                 ← systemless 覆盖树
│   ├── post-fs-data.sh / service.sh / boot-completed.sh / post-mount.sh
│   ├── uninstall.sh / action.sh / webroot/
│   ├── disable / remove / update / skip_mount   ← 状态标志文件
│   └── sepolicy.rule           ← 规则注入（见 §4.3 Q4 结论）
├── modules_update/<id>/        ← 安装暂存区（MODULE_UPDATE_DIR），下次 post-fs-data 时由 apd 搬运生效
└── metamodule/                 ← 元模块（接管挂载/安装编排的可选模块）
```

关键常量（`apd/src/defs.rs`）：`MODULE_DIR=/data/adb/modules/`、`MODULE_UPDATE_DIR=/data/adb/modules_update/`（注释明确：**此路径不可改**，installer.sh 强依赖）、`AP_RC_PATH=/data/adb/ap/.aprc`、`METAMODULE_DIR=/data/adb/metamodule/`。

### 1.2 模块包结构（zip 内）

与 Magisk 模块同构：`module.prop`（必需，含 `id`）、可选 `system/`、`post-fs-data.sh`、`service.sh`、`boot-completed.sh`、`post-mount.sh`、`uninstall.sh`、`action.sh`、`webroot/`、`skip_mount`、`sepolicy.rule`、`system.prop`、metamodule 专用 `metainstall.sh/metamount.sh/metauninstall.sh`。

### 1.3 安装流程（apd module install，`apd/src/module.rs::_install_module`）

1. **开机校验**：`ensure_boot_completed()` 要求 `sys.boot_completed==1`，否则拒绝（防止 A/B 场景下安装态混乱）。
2. **自备环境**：`assets::ensure_binaries()` 确保 `/data/adb/ap/bin/` 存在 busybox（0o755），并重建 `resetprop`、`magiskpolicy` 两个指向 `/data/adb/apd` 的符号链接（multicall 入口，见 §2.3）。
3. **解析 zip**：读 `module.prop`，取 `id`，用正则 `^[a-zA-Z][a-zA-Z0-9._-]+$` 拒绝路径穿越（与 KernelSU 同规则）。
4. **metamodule 判定**：若是 metamodule 则做单实例检查（同时只允许一个 metamodule）；普通模块在"活跃 metamodule 带自定义 installer 且有未生效变更"时被拒绝安装（`metamodule::check_install_safety`）。
5. **落盘**：zip 解压到 `modules_update/<id>/`；创建 `modules/<id>/`（0o700）。
6. **执行安装脚本**：`exec_install_script()` 用 `busybox sh -c` 执行安装器，环境变量见 §3.4。安装器来源：metamodule 存在且有 `metainstall.sh` 时拼接「内置 installer.sh + metainstall.sh」，否则用内置 `apd/assets/installer.sh`（Magisk 风格：mount_partitions → api_level_arch_detect → setup busybox → set_perm/set_perm_recursive → print_title 等）。
7. **收尾**：对 `modules_update/<id>/system` 设置权限与 SELinux context；`mark_update()` 写 `/data/adb/ap/update` 标志。**安装不立即生效**，等下次 post-fs-data 由 `handle_updated_modules()` 把 `modules_update/<id>` 搬到 `modules/<id>`（保留原 disable/remove 状态）后生效。

### 1.4 apd 承担的角色小结

| 职责 | 入口 |
|---|---|
| 模块全生命周期管理 | `apd module install/uninstall/undo-uninstall/enable/disable/action/list/config` |
| 安装编排（zip 校验→落盘→installer 执行→标志位） | `_install_module` + `exec_install_script` |
| 开机阶段编排 | `apd post-fs-data / services / boot-complete`（`apd/src/event.rs`） |
| 挂载编排 | 委托 metamodule `metamount.sh`；无 metamodule 时行为待二次核对（见 §5 开放问题） |
| systemless 脚本环境 | busybox ASH_STANDALONE=1 + PATH 注入（§3.4） |
| SELinux 规则 | `apd sepolicy`（magiskpolicy 兼容）+ post-fs-data 时 `magisk_rules()` live 注入 |
| root 鉴权链路 | supercall（supercall.rs）+ uid 监听同步（uid-listener） |
| 属性伪装/设置 | `apd resetprop`（Magisk 兼容子命令） |
| 免重启软重启 | `apd soft-reboot` |

---

## 2. apd 子命令全集与调用方式

### 2.1 子命令全集（`apd/src/cli.rs`，clap derive）

```
apd [-s/--superkey <KEY>] <command>
  module install <zip>            安装模块 zip
  module uninstall <id>           卸载（写 remove 标志）
  module undo-uninstall <id>      撤销卸载
  module enable <id> | disable <id>
  module action <id>              运行 action.sh
  module lua <id> <function>      模块 lua runner
  module list                     列出模块
  module config get|set|list|delete|clear [--internal <name>] [--temp]
  post-fs-data                    触发 post-fs-data 事件（开机编排入口）
  services                        触发 service 事件
  boot-completed                  触发 boot-complete 事件（后拉起 uid-listener）
  uid-listener                    inotify 监听 packages.list 变更并同步内核 root 授权表
  insmod <ko> [params...]         免版本校验加载内核模块（越狱模式）
  soft-reboot                     软重启（保留运行时加载的模块）
  late-load [--module] [--kmi] [--magica <port>] [--post-magica] [--package-name]
                                  股票内核热加载 kernelpatch.ko + magisk 规则（越狱模式）
  resetprop <args...>             Magisk 兼容 resetprop
  sepolicy <args>                 magiskpolicy 兼容（check 等子命令）
```

### 2.2 隐式 multicall（argv[0] 分发，`cli.rs::run()` 开头）

这是 APatch 现有的"multicall"形态——不是 Magisk 那种单二进制多功能分发器，而是 argv[0] 后缀匹配：

| argv[0] 以…结尾 | 实际执行 |
|---|---|
| `kp` 或 `su` | `apd::root_shell()`（root shell） |
| `resetprop` | `resetprop_main(argv)` |
| `magiskpolicy` | `sepolicy::policy_main(argv)` |

配合 `assets::ensure_binaries()` 在 `/data/adb/ap/bin/` 下建立 `resetprop→/data/adb/apd`、`magiskpolicy→/data/adb/apd` 符号链接实现分发；`su` 路径则由内核侧映射（`su_path` 机制，supercall `SUPERCALL_SU_RESET_PATH`）。

### 2.3 APK 侧调用方式（app/src/main/java/me/bmax/apatch/）

- **Root shell 载体**：`APatchCli.kt::createRootShell` 用 libsu 以 `SUPERCMD=/system/bin/truncate` + `superKey` + `-Z u:r:magisk:s0` 拉起 shell —— 即内核把 `/system/bin/truncate` 重映射为 su（`su_path` 机制），带 SuperKey 鉴权与 magisk SELinux context；失败时逐级回退 kpatch su → 系统 su → sh。
- **直接子命令调用**（`execApd`、`rootShellForResult`）：`module list`、`module install <zip>`、`module enable/disable/uninstall/undo-uninstall`、`module action`、`soft-reboot`、`-V`（版本探测，`Version.kt:111,121`）。
- **常量**（`APatchApp.kt:48-76`）：`APD_PATH=/data/adb/apd`、`APATCH_FOLDER=/data/adb/ap/`、`SUPERCMD=/system/bin/truncate`、`MAGISK_SCONTEXT=u:r:magisk:s0`。
- **卸载清理**：`PatchesViewModel.kt:373` 直接 `rm -f /data/adb/apd`。

### 2.4 模块脚本侧调用约定

- 脚本经 `busybox sh` 执行，`exec_script()` 固定 `current_dir=脚本所在目录`、`process_group(0)`、`switch_cgroups()`。
- 模块内可调用 `resetprop`/`magiskpolicy`（PATH 里有 bin/ 下的符号链接，落回 apd multicall）。
- 环境变量见 §3.4。

---

## 3. SK 适配改造设计（功能不变）

### 3.1 改造总原则

1. **行为兼容是硬约束**：所有模块侧可观察行为——子命令语义、退出码、stdout/stderr、脚本环境变量、目录布局、状态标志文件——保持与 APatch 一致；模块生态无需重打包即可在 SPRoot 上运行。
2. **鉴权点集中替换**：SuperKey 只在两处真实进入内核——`supercall.rs`（syscall `__NR_SUPERCALL(45)`，命令字 0x1010/0x1100/0x1101/0x1102/0x1103/0x1041/0x1111/0x1112，ver_and_cmd 编码 `ver<<32 | 0x1158<<16 | cmd`）与 `cli.rs` 的 `-s/--superkey` 全局参数。SK 化就是把这两处换成 SKRoot 调用契约（见 §4），其余代码不动。
3. **rename 与 multicall 一体化**：二进制更名 `apd → AP` 后，argv[0] 分发表从 `{kp,su,resetprop,magiskpolicy}` 扩展为 `{AP, su, resetprop, magiskpolicy, …}`；`DAEMON_PATH`、`APD_PATH`、符号链接目标、`run_uid_monitor()` 里的硬编码 `/data/adb/apd`（event.rs）同步更名。部署路径建议保留 `/data/adb/apd` 的**兼容符号链接**一个版本周期，降低 APK/脚本生态切换成本（是否保留由第二阶段拍板）。

### 3.2 supercall → SK 调用替换清单（supercall.rs）

| 现有 supercall | 用途 | SK 化设计 |
|---|---|---|
| `SUPERCALL_SU (0x1010)` | apd 自提权（privilege_apd_profile，uid→0，scontext `u:r:magisk:s0`） | 换 SKRoot 提权调用（SK 内核接口，密钥鉴权） |
| `SUPERCALL_SU_GRANT_UID/REVOKE_UID (0x1100/0x1101)` | uid 监听同步内核授权表 | 换 SK 授权表接口；**同步逻辑（refresh_ap_package_list 的防撕裂/fail-open 语义）原样保留** |
| `SUPERCALL_SU_NUMS/LIST (0x1102/0x1103)` | 枚举已授权 uid | 换 SK 枚举接口 |
| `SUPERCALL_SU_RESET_PATH (0x1111)` | 注册 su 路径（/system/bin/truncate 重映射） | SK 侧如无等价机制，SU_PATH 重映射由 SKRoot 内核方案定义（依赖内核修补组结论）；用户态保留 `su_path` 文件读写不变 |
| `SUPERCALL_KSTORAGE_WRITE (0x1041)` + gid=1 | AP 模块 exclude 列表 | SK 无 kstorage 则改为用户态存储（`/data/adb/ap/exclude_list`），语义不变 |
| `SUPERCALL_SU_GET_SAFEMODE (0x1112)` | 安全模式查询 | 改为文件标志（`/dev/.safemode` 已存在）或 SK 等价查询 |

`ver_and_cmd()` 的 KP 版本编码随 KernelPatch 一并废弃；`kp_version.rs`（build.rs 从 `app/src/main/cpp/version` 生成）改造后仅用于版本上报字符串。

### 3.3 启动链路 SK 化（event.rs，行为不变）

`post-fs-data` 流程：`report_kernel(truncate ... event ...)` → `init_load_su_path` → `get_policy_main(["magiskpolicy","--live"]).magisk_rules()` 写 `/sys/fs/selinux/load` → `privilege_apd_profile` → 清临时配置 → Magisk 共存检测 → 日志轮转 → safe_mode 分支 → `post-fs-data.d` 公共脚本 → `handle_updated_modules`（搬运 modules_update）→ prune/restorecon → `sepolicy.rule` 注入 → metamodule `metamount.sh` → 模块 `post-fs-data.sh` → lua → `system.prop` → 清 update 标志 → `run_stage("post-mount")` → report_kernel(after)。

SK 化改动点只有三处，其余逐行保留：
1. `report_kernel`：`truncate` 是内核 su 重映射目标，SK 下重映射目标是什么（或是否保留该事件机制）待 SKRoot 内核结论；用户态封装函数签名不变。
2. `privilege_apd_profile`/`init_load_su_path`：走 §3.2 的 SK 替换。
3. `magisk_rules()`（ap_policy crate）：SELinux 规则注入与 root 无关，**原样保留**——这也是 Q4（sepolicy.rule 支持注入、隐藏性不保证）在实现层的落点。

`boot-completed` → `run_uid_monitor()` 里硬编码的 `Command::new("/data/adb/apd")` 改为 `defs::DAEMON_PATH` 常量引用（顺手消除硬编码）。

### 3.4 脚本环境（SK 化后保持逐字节一致）

`module.rs::get_common_script_envs()`：

| 变量 | 值 | SK 化处置 |
|---|---|---|
| `ASH_STANDALONE` | `1` | 保留（busybox standalone 模式） |
| `APATCH` | `true` | **保留字面值**（模块脚本按此探测平台） |
| `APATCH_VER` / `APATCH_VER_CODE` | VERSION_NAME/CODE（OUT_DIR 生成） | 保留变量名，值改由 SPRoot 版本管理（pl_dd267ef011b5d4bc）定义 |
| `PATH` | 原 PATH + `/data/adb/ap/bin` | 保留 |
| `AP_MODULE` | 模块 id（从脚本路径提取） | 保留 |
| 安装脚本额外 | `OUTFD=1`、`ZIPFILE=<canonical zip path>` | 保留 |

执行细节全部保留：busybox sh、`process_group(0)`、`switch_cgroups()`、`current_dir=脚本目录`、`wait/spawn` 语义、`foreach_active_module` 遍历顺序。

### 3.5 模块安装编排（SK 化后不变项清单）

- `ensure_boot_completed` 门禁；`modules_update` → `modules` 的搬运时机（post-fs-data）；`update` 标志位；`disable/remove/skip_mount` 标志语义；metamodule 单实例与安装安全检查；模块 id 正则；0o700 目录权限；`ensure_binaries` 符号链接策略（目标改为 AP 本体）；installer.sh 全文（其中 `APATCH=true` 探测、`grep_prop` 等不依赖鉴权）。
- **明确不变**：模块内脚本永远不需要直接触碰 SK 密钥——它们经 PATH 里的 multicall 符号链接调用 resetprop/magiskpolicy，鉴权发生在 apd/AP 进程自身（已由启动链路或 SK 授权提权）。

### 3.6 KPM 相关子命令处置（Q2 结论：保留能力、暂时搁置）

`insmod`、`late-load`、`magica`、`soft-reboot`、`uid-listener` 全部**原样保留**（人类决策：作为能力保留但搁置）。`late-load` 依赖 kernelpatch.ko + SUPERCALL 家族，SK 化后这些路径在 SPRoot 上预期不可用（无 KP 内核），但代码不删——与 insmod 一样属于"保留能力"。`resetprop`、`sepolicy` 与 KP 无耦合，正常工作。

---

## 4. SK 运行侧 ↔ apd/AP 对接点契约（草案，待内核修补组确认对端）

> 方向：SKRoot（内核修补组实现）是被依赖方，apd/AP 是消费方。以下为 apd 侧视角的对接需求，接口名以内核修补组产出为准（pl_3938cb93855b03d4 架构对齐时合并）。

### 4.1 鉴权与提权

- **密钥传递**：沿用 CLI 全局参数 `-s/--superkey <KEY>` 通道承载 SK Root 密钥（48 位），格式与传递方式（argv vs 环境变量 vs 文件）需与内核修补组对齐；默认建议保持 argv（与现 APK `createRootShell(SUPERCMD, superKey, -Z ctx)` 调用形态一致，APK 侧改动最小）。
- **提权调用**：`privilege_apd_profile()` 从 `sc_su(0x1010)` 换为 SK 提权接口；期望语义等价：pid 不变、uid→0、scontext 可指定 `u:r:magisk:s0`（magisk_rules 依赖该 context）。
- **root shell（argv[0]=su/kp 分支）**：内核侧 su 重映射机制由 SKRoot 定义；用户态 `root_shell()` 逻辑不变，仅内核对端变化。`su_path` 注册（SUPERCALL_SU_RESET_PATH）的对端由 §3.2 决定。

### 4.2 授权表同步（uid-listener）

`start_uid_listener()` 监听 `/data/system/packages.list.tmp` inotify + 信号量，触发 `refresh_ap_package_list`：读 `package_config`（csv：pkg/uid/to_uid/allow/exclude/sctx）→ 全量撤销→重授。对接需求：SK 提供 **grant(uid,profile)/revoke(uid)/list()/count()** 四个等价操作。该模块的容错语义（torn-read 熔断、fail-open、跳过 uid 0/2000）是安全关键，接口对齐时不得弱化。

### 4.3 安全模式与状态标志

`is_safe_mode()`（apd/src/utils.rs）精确逻辑（v2 已核对）：**短路两段**——第一段查 `persist.sys.safemode==1` 或 `ro.sys.safemode==1`（任一命中直接 true）；未命中且带有效 superkey 时调 `sc_su_get_safemode`（0x1112）查内核标志，返回 1 即 true；无 superkey 视为 false。SK 化后：属性两路原样保留（纯用户态，无鉴权）；内核查询路换 SK 等价接口或删除（删除时该函数退化为属性判断，语义收窄但安全侧不放宽——safe_mode 的作用是**禁用**模块，少一路检测只会让 safe mode 更难触发）。

### 4.4 身份与环境要求（SK 授权后调起 apd/AP 的前提）

| 项 | 要求 | 来源 |
|---|---|---|
| 调用身份 | uid 0 或经 SK 提权后的 apd 进程；`module` 子命令先 `switch_mnt_ns(1)`（切 init mount namespace） | cli.rs |
| SELinux context | `u:r:magisk:s0`（APK 显式 `-Z` 指定；post-fs-data 路径先 magisk_rules 再提权） | APatchCli.kt / event.rs |
| 路径 | 二进制 `/data/adb/apd`（或 AP 新路径+兼容链接）；工作目录 `/data/adb/ap/`；模块根 `/data/adb/modules/` | defs.rs |
| busybox | `/data/adb/ap/bin/busybox` 必须就位（ensure_binaries 仅 chmod，不安装——busybox 由 APK 部署，改造后由 SPRoot 首装链路部署） | assets.rs |
| 环境变量 | 脚本侧 `ASH_STANDALONE/APATCH/APATCH_VER*/PATH/AP_MODULE`；`KernelPatch 版本上报`（KERNELPATCH_VERSION/KERNEL_VERSION env 打印）改为 SPRoot 版本 | module.rs / event.rs |
| 返回码 | 0=成功；非 0=失败（anyhow 链，错误经 log 输出）；`module list` stdout 为 JSON 数组 | 全局 |

### 4.5 执行身份矩阵（谁以什么身份跑什么）

| 动作 | 发起方 | 执行身份 |
|---|---|---|
| module install/管理子命令 | APK（root shell） | uid 0，magisk context，init mnt ns |
| post-fs-data/services/boot-completed | init（.aprc/rc 脚本） | uid 0，superkey 由环境/参数传入 |
| 脚本（stage/installer/action） | apd fork | 继承 apd 身份，busybox sh，process_group 0 |
| uid-listener | boot-completed fork | 常驻 uid 0 |

---

## 5. 开放问题（第二阶段输入）

1. **无 metamodule 时的挂载实现（已核对，v2 关闭）**：`metamodule::exec_mount_script()`（apd/src/metamodule.rs:260）在 metamodule 不存在或无 `metamount.sh` 时**静默返回 Ok(())**；apd 本体没有任何内置 OverlayFS/bind-mount 代码（module.rs 中 mount 仅出现在 needs_mount 安全检查）。APK 侧 metamodule 也不是内置资产，而是来自在线仓库元数据标志（ModuleRepo.kt:53,164 `metamodule` 字段）。**结论：上游 APatch 默认状态不挂载任何模块 system/ 目录，挂载能力完全由可选安装的 metamodule 提供。SPRoot 二选一：(a) 随首装 APM 内置默认 metamodule（复用上游生态，推荐）；(b) 在 AP 二进制内实现内置挂载编排（偏离上游行为，违反"功能不变"原则，不推荐）。该决策移交 pl_d9c4a8e7f2295584（APM 部署方案）。**
2. **su 重映射对端**：`SUPERCMD=/system/bin/truncate` 的内核重映射由 KP 实现，SKRoot 是否提供等价 su_path 机制待内核修补组结论。
3. **SK 密钥的运行时形态**：常驻进程缓存 or 每次调用传入，影响 `-s` 参数保留策略与 uid-listener 的密钥可用性。
4. **兼容符号链接策略**：`/data/adb/apd` 保留几个版本周期（涉及 APK 卸载清理 `rm -f /data/adb/apd` 的适配）。
5. **busybox 来源**：APK 内置 busybox 的提取/部署链路在 SPRoot 首装（自举/APM 循环依赖）中的归属，与 pl_d9c4a8e7f2295584（APM 部署方案）联动。

## 6. 结论

- apd 的 SK 化是**窄口改造**：鉴权集中在 supercall.rs + `-s` 参数 + argv[0] su 分支三处；模块管理、脚本环境、安装编排、目录布局全部原样保留，模块生态零迁移。
- multicall 基础已存在（argv[0] 分发 + 符号链接），`apd → AP` 更名与分发表扩展是机械性工作，风险低。
- 与 SKRoot 的对接面共 6 类接口：提权、授权表四操作、su 路径注册、安全模式查询、（可选）事件上报、kstorage 替代存储。已在 §4 列出 apd 侧期望语义，待架构对齐会议合并。

---
*基线核对记录：2026-11-06，gh-proxy 克隆 HEAD `e8e4651c16fb5075882a25813c8732285fdb3505` = origin/main HEAD（标准 github.com fetch 验证，20:43 与 21:12 两次复核一致）。*
