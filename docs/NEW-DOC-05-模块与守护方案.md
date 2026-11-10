# NEW-DOC-05 模块与守护方案（v1.0 草案）

> 文档编号：NEW-DOC-05 · 任务：task-6（系统：模块管理与守护进程方案）
> 撰写：系统守护（sys-guard） · 日期：2026-11-08 · 状态：**待 lead 终审 → 客户端 / 内核修补会签**
> 上游输入（均已通读）：
> - `docs/phase2_skroot_ap_call_contract_draft.md` v1.0（SKRoot↔AP 契约，三方签署，下称**契约 v1.0**）
> - `docs/phase2_boot_sequence_analysis_draft.md` v0.5（启动时序联合稿，下称**时序稿**）
> - `docs/SPROOT-DOC-A1-SKRoot机制深度分析.md` v1.0（下称 **A1**）、`docs/SPROOT-DOC-C1-修补引擎IO契约.md` v1.0（下称 **C1**）
> - `docs/phase1_ap_apd_analysis_and_sk_adaptation.md` v4（apd 深度分析）
> - 源码实证：`app/apd/`（Rust，已部分 SK 化的分支）、`engine/src/`（SK 修补引擎 C++ 实现，**机制的最终权威**）
> 读者：lead、客户端（newapp）、内核修补、质测
>
> **一句话结论**：模块管理**沿用 `app/apd`（Rust）改造为 AP 守护进程**，不重写；新管理器只做 UI + 本地修补 + 密钥保管。启动时序由**管理器在修补流程内注入的 `init.sproot.rc`** 驱动（SK 内核零事件）。Root 授权由「**AP 守护直连（管理器）+ SO_PEERCRED 授权 socket（第三方）**」用户态强制，落地过程中**必须修正契约 v1.0 中 4 处与引擎实现不符 / 不可实施的假设（E1–E4，见 §4.6）**。

---

## 0. 结论摘要（决策表）

| # | 决策项 | 结论 | 一句话理由 |
|---|---|---|---|
| D1 | 模块管理组件 | **沿用 `app/apd`（Rust）→ 改名 AP 二进制（守护 + multicall），不重写** | 模块生态（Magisk 式 zip/脚本/环境/metamodule/sepolicy）语义全部已实现且有 799+295 行代码证据；重写只增加语义漂移风险，不解决新项目的主要风险（修补链可靠性） |
| D2 | 新管理器与 AP 的边界 | 管理器 = **UI + 修补 + 密钥保管**；AP = **一切需要 root 的持久行为**（模块、阶段脚本、授权、su） | 与 APatch 既有分层一致（APK↔apd 走 CLI），且 App 进程不适合承载常驻守护 |
| D3 | 启动事件源 | **rc 注入**（`init.sproot.rc` + `init.rc` 末尾 import 追加），由**管理器在本地修补流程内**完成 | SK 内核零常驻、零事件（契约 v1.0 §4 / A1 §8-4）；ramdisk 是唯一零内核改动的落点 |
| D4 | 三阶段拉起形式 | `on post-fs-data → exec apd post-fs-data`（同步）；`on boot → start sproot_daemon`（常驻，内部依次跑 services 阶段、轮询 boot-completed） | 同步语义对齐模块脚本契约；boot-completed 用用户态轮询而非 rc property 触发器，规避厂商差异 |
| D5 | 提权形态 | `execve(key[0:47], NULL, NULL)`，成功判据 `getuid()==0` | 引擎实现即如此（逐字节比对 filename）；⚠️ **契约 v1.0 §3.1 的「密钥串=隐藏目录 su 路径」表述错误，见 E1** |
| D6 | su 部署 | `/data/<key16>/<随机名>`（AP multicall 副本）+ `/data/adb/ap/bin/su` 符号链接 | 固定名 `su` 可被扫描；随机名 + 隐藏目录才符合 A1 §4.4 的隐藏目标，见 E2 |
| D7 | 授权表 | 单文件 `/data/adb/ap/allow_list`（csv 六列，含 exclude），**daemon 单写者 + flock + tmp/rename**，读侧无锁 | 契约 §3.2 已定；探测点：不再单列 `exclude_list`，见 E4 |
| D8 | 授权判定点 | 管理器直连（AP 自提权）无 socket；第三方 App 走**抽象 socket + SO_PEERCRED** | `SP_REQUEST_UID` 内核注入**不可实施**（E3），唯一可强制的身份来源是 socket peer cred |
| D9 | KP 专属子命令 | `insmod` / `late-load` / `magica` / `soft-reboot` **删除** | 新架构无 kernelpatch.ko / KPM，保留=死代码 + 误触发面；新项目无历史兼容包袱（与 phase1「能力保留」原则的偏离在本表显式记录） |
| D10 | 首装自举 | **三段式**：修补期（无 root，不动 /data）→ 首启管理器自提权部署 → 二次开机生效 | 循环依赖靠「AP 二进制自带自提权 + 管理器持有密钥」解开，不需要内核/部署器额外配合 |

---

## 1. 组件选型：沿用 apd（Rust）改造

### 1.1 决策

`app/apd`（APatch `e8e4651c` 基线，已在本仓库完成 SK 化改造）→ 继续演化为 **AP 二进制**，承担：模块全生命周期、三阶段脚本编排、multicall（su/resetprop/magiskpolicy）、授权表、守护进程。**新管理器（newapp）不承载模块逻辑**，只通过 CLI/root-exec 调用 AP。

### 1.2 证据（为什么是「改造」而不是「重写」）

| 能力 | 现状实现（源码定位） | 重写代价 |
|---|---|---|
| 模块安装全流程（zip 校验 → `modules_update` 暂存 → installer.sh 执行 → 标志位 → 下次 post-fs-data 搬运生效） | `app/apd/src/module.rs:363-506`、`63-96`、`assets/installer.sh`(11KB) | 需在新语言重做 zip 解析、id 正则、metamodule 安全检查、权限/SELinux 收尾 |
| 脚本执行环境（busybox `sh`、`ASH_STANDALONE=1`、`APATCH=true`、`PATH+/data/adb/ap/bin`、`AP_MODULE`、`process_group(0)`、cgroup 迁移、cwd=脚本目录） | `module.rs:99-120`、`205-252`、`utils.rs:110-134` | 逐字节兼容是模块生态硬约束，任何漂移都会让存量模块静默失效 |
| 阶段编排（post-fs-data / service / boot-completed / post-mount，metamodule 优先、common `*.d`、lua） | `event.rs:44-250`、`252-257`、`283-290` | 重写=复刻一套已调优的顺序与容错 |
| metamodule（单实例、metainstall/metamount/metauninstall） | `metamodule.rs` 全文 | 生态只有少数 metamodule 实现者，语义错了会直接挂载失败 |
| 模块配置 API / lua / action / webroot | `module_config.rs`、`lua.rs`、`module.rs:622-631`、`_list_modules` | 保留即用 |
| 授权表用户态化 | `auth_list.rs`（已按契约 §3.2/§3.3 重写）、`package.rs`（csv 读/写/uid 同步语义） | 已完成的改造会白扔 |
| multicall（argv[0] 分发 + 符号链接） | `cli.rs:220-231`、`assets.rs:10-21` | 已存在，只需扩入口 |

### 1.3 否决的备选

| 备选 | 否决理由 |
|---|---|
| 在 newapp（Kotlin）内实现模块管理 | 模块脚本必须在**开机早期、以 root、在 init 命名空间**由独立进程执行，App 进程无法承担；且 zip/ash/env/sepolicy 语义要在 JVM 里复刻，工作量大于收益 |
| 用 C/C++ 重写一个「Magisk 风格 magiskd」 | 与 D1 同样的语义复刻成本，还要重做 Rust 已有的 zip/lua/csv/policy 生态；无收益 |
| 直接嵌入 Magisk/magiskd 源码 | 与 SK「无挂载、SELinux 零触碰」理念冲突（magiskd 依赖 magiskinit + mount 体系），license 与维护面都不可控 |

### 1.4 前置条件与最大风险（必须在本迭代内闭环）

1. **Android 交叉编译链缺失**：本机 `~/.cargo` 仅有 `stable-x86_64-pc-windows-msvc` 工具链，`aarch64-linux-android` target 与 NDK 均未安装（已实测）。→ P0：CI（task-8）固化 `rustup target add aarch64-linux-android` + Android NDK（r27+、NDK clang 作为 linker），产物 `libapd.so` 进 `jniLibs/arm64-v8a`，首次构建记录实际体积（预估 strip+LTO 后 3~6 MB，含 vendored lua54/zip/policy，须实测）。
2. **git 依赖可复现性**：`Cargo.toml:24,51,56` 三个 git 依赖（java-properties / restprops / ap_policy，均已 pin rev），建议 vendor 进仓库或至少锁定 `Cargo.lock` 并禁用 `cargo update`（任务 8 的 CI 要加 `--locked`）。
3. **edition 2024** 要求 rustc ≥ 1.85，CI 镜像需固定版本，避免「本机能编、CI 不能编」。

---

## 2. 职责边界与进程模型

```
┌──────────────────────────────────────────────┐
│ 新管理器 App（newapp，Kotlin/Compose）        │
│  · 本地修补：magiskboot 解包 → SK 引擎 → rc 注入 → 重打包 │
│  · 密钥保管（Android Keystore 加密，仅 App 私有）│
│  · UI：模块/授权/状态；所有提权操作经 root-exec │
└───────────────┬──────────────────────────────┘
                │ ① exec nativeLibraryDir/libapd.so root-exec（普通进程启动→AP 自提权）
                │ ② 部署完成后可选：抽象 socket（第三方 App 授权）
┌───────────────▼──────────────────────────────┐
│ AP 二进制（Rust，/data/adb/apd）              │
│  · CLI 子命令（module/root-exec/deploy/auth） │
│  · daemon：授权 socket + uid-listener + 状态  │
│  · multicall：su / resetprop / magiskpolicy   │
│  · 三阶段编排：post-fs-data / services / boot-completed │
└───────────────▲──────────────────────────────┘
                │ init（原厂，未修改）按注入 rc 拉起
┌───────────────┴──────────────────────────────┐
│ SK 修补内核（五类 hook，实际织入 11/14 组，零常驻，零事件）│
│  do_execve 提权 / filldir 隐藏 / avc_denied  │
└──────────────────────────────────────────────┘
```

### 2.1 进程清单

| 进程 | 由谁启动 | 身份 | 生命周期 | SELinux 预期 |
|---|---|---|---|---|
| `apd post-fs-data` | init（`on post-fs-data` → `exec`，同步阻塞 init） | init 起跑（uid0/init 域）→ `execve(key)` 后 caps 全满 | 一次性，模块脚本跑完退出 | 提权后 avc_denied 静默放行（时序稿 §3.5） |
| `sproot_daemon`（`apd daemon`） | init（`on boot` → `start`） | uid0；启动后 `execve(key)` 完成「密钥进程」标记 | 常驻；异常退出由 init 重启 | 同上；不写 seclabel 时默认 init 域（时序稿 §3.5 推导） |
| `su` 会话 / `root-exec` 子进程 | daemon 或管理器 | 目标 uid，或 root（按授权） | 会话级 | setcon 成功则 `u:r:magisk:s0`，失败保留原 context |
| 模块脚本 | AP fork（busybox sh） | 继承 AP 身份 | 按阶段阻塞/非阻塞 | 继承 |
| 修补引擎（`skroot-patch`） | 管理器 | App uid | 一次性 | 普通 App 域（不碰内核态） |

**进程原则**：daemon 只做「调度 + 授权判定 + 会话派发」，**所有模块脚本都 fork 子进程执行**——脚本崩溃/超时不会拖死 daemon（这是 SK 无内核监管下唯一的 bootloop 防线之一）。

---

### 2.2 边界铁律：修补链 ≠ 特权链（2026-11-08 lead 裁决补入）

| 链路 | 包含 | 权限 | 调用路径 |
|---|---|---|---|
| **修补链** | 选镜像 / 解包 / `skroot-patch` / 重打包 / 导出（或刷入） | **无需 root**（首装也能先出镜像） | 管理器 App 进程内（JNI 引擎或引擎子进程，进程边界见 C1）；**不得**经 `root-exec` |
| **特权链** | `deploy` / 模块安装/启停/卸载 / 授权表读写 / root 会话 / `undeploy` | root | **只能**经 AP（`root-exec` 包裹或 daemon）；契约 v1.1 §8.2 |

理由：修补是「文件操作」，被 root 失败阻断即失去本地修补的核心优势；特权操作是「系统状态变更」，必须走 AP 单写者与授权判定。**两条路径不得混用**（禁止用 root-exec 跑修补命令，也禁止 App 直接写 `/data/adb`）。

---

## 3. 子命令范围与部署路径

### 3.1 子命令清单（新架构口径）

| 类别 | 子命令 | 决策 | 说明 |
|---|---|---|---|
| 模块域（保留） | `module install/uninstall/undo-uninstall/enable/disable/action/list/config` | ✅ 保留 | 行为与 APatch 一致；`install`/`uninstall`/`list` 新增 `--json` 机器接口（schema/退出码见 §3.1.1/§3.1.2） |
| 模块域（保留） | `module lua <id> <fn>` | ✅ 保留 | 存量模块能力 |
| 阶段编排 | `post-fs-data` / `services` / `boot-completed` | ✅ 保留（仅 init 调用） | 加 `--superkey-file`，由 rc 传入 |
| 多调用 | argv[0]=`su` / `resetprop` / `magiskpolicy` | ✅ 保留（`kp` 已删，契约 §3.4） | 分发为 **basename 精确匹配**（P1 已修）。`resetprop` 为**条件能力**：默认构建启用 `resetprop` feature；`--no-default-features` 的许可干净构建下，`resetprop` 子命令与 `system.prop` 返回明确「本构建未启用」错误（不静默缺失） |
| 守护 | **`--superkey-file <f> daemon`（新，顶层参数须在子命令前）** | ➕ 新增 | 常驻：授权 socket + uid-listener + 状态文件；init 拉起 |
| 部署 | **`--superkey-file <f> deploy --source <nativeLibDir>`（新，同上）** | ➕ 新增 | 首装自举（§6.2）：安装 AP/busybox/key/隐藏 su/空授权表 |
| 部署 | **`undeploy [--keep-modules]`（新）** | ➕ 新增 | 逆序清理（§6.4） |
| 授权 | **`auth list/grant/revoke/exclude/unexclude`（新）** | ➕ 新增 | 供管理器 UI；daemon 单写者语义由 flock 保证 |
| 提权执行 | **`--superkey-file <f> root-exec [--uid N] -- <cmd...>`（新，同上）** | ➕ 新增 | 管理器唯一 root 入口：进程以 App uid 启动 → AP 自提权 → exec 目标命令，透传 stdio/退出码 |
| 状态 | **`version --json`（新）** | ➕ 新增 | 管理器探测已部署状态/版本，替代 `-V` 字符串 |
| KP 专属（删除） | `insmod` / `late-load` / `magica` / `soft-reboot` | ❌ 删除 | 依赖 kernelpatch.ko·KPM·supercall（A1 §8-4 无等价物）。连带删除 `Cargo.toml` 的 `goblin/scroll/adb_client`（减体积、减攻击面） |
| 遗留 | `uid-listener` | ⚠️ 并入 daemon | 独立子命令取消，改为 daemon 线程；保留单实例锁 |

> **与 phase1「KPM 等能力保留」原则的偏离**：phase1 的结论建立在「旧管理器 + APatch 生态延续」前提上；新项目从零重写且无 kernelpatch.ko，保留这些子命令只会产生「看似可用、实际必失败」的路径。此为**有意偏离，需 lead 确认**（§10-Q3）。

#### 3.1.1 模块列表 JSON schema（**v1.0 定稿**，与 app-client 对齐 2026-11-08）

`apd module list --json` —— 管理器模块页唯一数据源（经 `root-exec` 调用）。**字段冻结**，变更须走 v1.1 修订：

```json
[
  {
    "id": "example_module",
    "name": "Example Module",
    "version": "1.2.3",
    "versionCode": 12,
    "author": "someone",
    "description": "一句话描述",
    "enabled": true,
    "update_pending": false
  }
]
```

| 字段 | 类型 | 语义 | 缺省 |
|---|---|---|---|
| `id` | string | `module.prop` 的 id（无则用目录名） | — |
| `name` | string | `module.prop` name；缺失回退为 id | id |
| `version` | string | `module.prop` version | `""` |
| `versionCode` | number | `module.prop` versionCode 十进制解析 | `0` |
| `author` | string | `module.prop` author | `""` |
| `description` | string | `module.prop` description | `""` |
| `enabled` | bool | 无 `disable` 标志文件 | `true` |
| `update_pending` | bool | 有 `update` 标志 **或** `modules_update/<id>/` 存在（重启后生效） | `false` |

**契约保证**：① stdout **纯 JSON 数组**，日志只走 stderr/logcat（契约 v1.1 §8.4）；② 无模块输出 `[]` 且 exit 0；③ 不含密钥/路径前缀等敏感字段；④ 数组按 `id` 字典序稳定排序。

`apd selftest --json` 同步定稿：`{ok: bool, stage: "undeployed"|"deployed", summary: "undeployed"|"deployed"|"daemon_ready"|"needs_repair", checks: [{name, ok, critical, detail}]}`；`stage=undeployed`（App 首启自检）exit 0。

#### 3.1.2 模块安装 / 卸载（`--json`，v1.0 定稿，app-client 接线用）

**两阶段语义（Magisk/APatch 兼容，勿误解为“直接写 modules/”）**：`module install` 把 zip 解包到 `/data/adb/modules_update/<id>/` 并执行安装脚本；**下次 post-fs-data** 由 `handle_updated_modules()` 原子搬运到 `/data/adb/modules/<id>/` 生效。因此结果里是 `update_pending: true` / `active_after_reboot: true`，不会在半安装状态下覆盖正在运行的模块。`uninstall` 同样写 `remove` 标志，由下次 post-fs-data 清理（`remove_pending: true`）。

`apd module install <zip> --json` → stdout 单行 JSON：

```json
{"ok":true,"id":"example","name":"Example","version":"1.2.3","versionCode":12,
 "metamodule":false,"enabled":true,"update_pending":true,"active_after_reboot":true,
 "changed":true,"previous_version":null,"uncompressed_bytes":12345,"avb_untouched":true}
```

`apd module uninstall <id> --json` →

```json
{"ok":true,"id":"example","remove_pending":true,"applied_after_reboot":true,"avb_untouched":true}
```

失败统一：`{"ok":false,"code":<exit code>,"id":<id|null>,"error":"<可读原因>"}`，进程 exit code = `code`。

**安装前置校验与限制**：

| 项 | 规则 |
|---|---|
| zip 大小 | ≤ 512 MiB（`MAX_MODULE_ZIP_BYTES`） |
| 解压总量 | ≤ 1 GiB（累计 `entry.size()`，zip-bomb 防护） |
| 条目数 | ≤ 100,000 |
| 路径安全 | 拒绝绝对路径、反斜杠、`.`/`..` 组件（zip-slip） |
| 符号链接 | 拒绝 zip 内 symlink 条目（防解包后逃逸） |
| `module.prop` | 必需 `id`（`^[a-zA-Z][a-zA-Z0-9._-]+$`）、`name`、`version`、`versionCode`（整数）；`author`/`description` 缺失仅告警 |
| 磁盘空间 | `/data` 可用 ≥ 解压量 + 64 MiB（`statvfs` 前置检查） |
| 超时 | 安装脚本 300s，超时 kill 进程组（对应 exit 124 语义） |
| id 冲突 | 同 id = 升级（保留 `disable`/`remove` 状态）；目录内 `module.prop id` 不一致 → 116 |
| 幂等 | 重复安装同版本：成功且 `changed=false`，不破坏既有 `disable`/`remove`/state |
| **AVB** | 安装/卸载**零** vbmeta/AVB 操作；结果带 `avb_untouched:true`，并写 `module_state/<id>.json` 审计 |

**退出码（沿用体系：1xx 参数/输入，2xx 执行，124 超时，126/127 部署）**：

| code | 含义 |
|---|---|
| 0 | 成功 |
| 2 | 用法错误（clap 参数解析） |
| 110 | 输入不存在/不可读（zip 路径、模块 id） |
| 111 | zip 损坏/无法解析 |
| 112 | `module.prop` 缺失 / 必填字段缺失 / id 非法 |
| 113 | 不安全 zip（zip-slip / 符号链接 / 绝对路径） |
| 114 | 超限（zip 大小 / 解压量 / 条目数） |
| 115 | 磁盘空间不足 |
| 116 | id 冲突（目录内 `module.prop id` 不一致，疑似损坏） |
| 120 | 安装脚本执行失败 |
| 121 | 解包 / IO 失败（含「Android is Booting」等状态门禁） |
| 122 | 安装 state 写入失败 |
| 124 | 超时 |
| 125 | 提权失败（`root-exec` 层，非本子命令返回） |
| 126 / 127 | 部署类（apd 未部署 / 命令缺失） |

**stdout 契约**：`--json` 时 stdout 只有 JSON（安装器/进度输出重定向到 stderr/logcat，契约 §8.4）；非 `--json` 保持原 APatch 人类可读输出不变。

> 客户端交叉引用：`docs/NEW-DOC-04-客户端方案.md` §7A.1 已加指针（模块安装/卸载 JSON + 退出码以本节为准）。

#### 3.1.3 自启链契约（rc 模板 ↔ 密钥路径 ↔ deploy，**v1.0 定稿 2026-11-08**）

AP 守护的开机自启是一条**四段固定链路**：rc 模板 → ramdisk 注入 → 固定密钥路径 → deploy 写入。任一环自行改名都不会显式报错，只会表现为「开机后 daemon 静默不存在」，所以逐项钉死（实现：`apd/src/assets.rs`、`apd/src/defs.rs`、`apd/src/deploy.rs`、`apd/src/selftest.rs`）。

**1) rc 模板（唯一来源；密钥绝不入内）**

| 项 | 值 |
|---|---|
| 仓库模板 | `apd/assets/init.sproot.rc`（Lead 独占维护，**勿改内容**） |
| 嵌入方式 | `include_str!`（`assets::INIT_RC_TEMPLATE`）。本 crate **不使用** `rust-embed`；`--no-default-features` 许可干净构建同样内嵌该资产 |
| ramdisk 落点 | `/init.sproot.rc`（管理器 RcInject 按原文注入；`/init.rc` 末尾追加 `import /init.sproot.rc`） |
| 审计副本 | `/data/adb/sproot/init.sproot.rc`（0644 root:root，`apd deploy` 落盘）。init 不读它，仅作 selftest/QA 取证：可证明管理器发出的模板仍引用固定路径 |

模板中的 service 行（**逐字**）：

```
service sproot-apd /data/adb/apd --superkey-file /data/adb/sproot/key daemon
```

注意 `--superkey-file` 是 apd 的**顶层**参数，必须写在子命令 `daemon` 之前；写成 `apd daemon --superkey-file ...` 会被 clap 判 `unexpected argument`（rc=2），真机永远起不来（已在 `fa3298c` 修正）。service 体为 `class late_start` / `user root` / `group root` / `oneshot` / `disabled` / `override`，由 `on property:sys.boot_completed=1` → `start sproot-apd` 触发：daemon 退出后 init 不 respawn（最坏「自启失效」，绝不 bootloop）；AP/密钥未部署时该 service 惰性无害——`on post-fs-data` 只 `mkdir /data/adb/sproot 0700`，绝不写密钥。

**2) 密钥固定路径与权限（AP 侧 ABI：47 字符明文，无 NUL）**

| 项 | 值 |
|---|---|
| 文件 | `/data/adb/sproot/key`（= `defs::SPROOT_KEY_FILE`）。rc 的 `--superkey-file` 与 deploy 的写入目标**必须引用同一个常量**，任何一方直接写字面量都会重演 HANDOFF §6.8 的静默失配 |
| 权限 | 文件 `0600` root:root；目录 `/data/adb/sproot` `0700` root:root |
| 内容 | `key[0:47]`，**47 字节纯文本**，无 NUL、无换行。注意区分：内核 shellcode 内嵌格式是「47 字符 + NUL」，但 **AP 侧文件不带 NUL**（HANDOFF §6.8）；给文件加 NUL 会让旧读取端 `CString::new` 失败，反而搞坏提权 |
| 读取兼容 | `secret::SecretKey::new` 规范化时丢弃 NUL/空白：即使某版写入端误加 NUL，读回仍是 47 字符密钥，`escalation_token` 不会看到内部 NUL |
| 唯一性 | 全机唯一密钥副本；不写入 rc、`state.json`、argv、stdout 或日志，也不保留第二份（早期 `superkey` 命名已废弃） |

**3) deploy / undeploy 的写入与删除责任（`apd deploy`，root）**

| 阶段 | 责任 |
|---|---|
| 目录 | 建 `/data/adb/sproot` 并断言 `0700` root:root（deploy 本身以 root 运行） |
| 写密钥 | 47 字节 → 同目录临时文件（`0600`，创建即定权）→ `fsync` → `rename` 覆盖 → 目录 `fsync`。**绝不先截断再写**：崩溃只会留下旧 key 或完整新 key，绝不会留下空 key。写完立即用 `write_volatile` 擦除内存副本（`secret::wipe_bytes`），比较用的旧内容同样擦除 |
| 幂等 | 逐字节相同 → 跳过（仅重申 0600）；不同 → 原子覆盖并记一行「content differed, overwritten」（**任何日志都不含密钥或前缀**） |
| rc 审计副本 | 内容不同才写 `/data/adb/sproot/init.sproot.rc`（0644），同样 tmp + fsync + rename |
| 冒烟 | 复读密钥并断言：`mode=0600`、`len=47`、无 NUL；rc 副本必须同时包含 `defs::SPROOT_KEY_FILE` 与 `/data/adb/apd` |
| undeploy | **先删密钥文件**（唯一 secret），再删 rc 审计副本，最后删 `/data/adb/sproot/` 目录 |

**4) 失败语义：写不到 key → deploy 明确失败**

密钥、目录、rename、冒烟任一步失败，`deploy` 立即返回非 0 并保留可读错误，**禁止「假装成功」**（打印 `deploy: ok` 却没有 key，会让设备进入「每次开机 rc 拉起一个立即退出的 daemon」的静默故障）。由于全程 tmp+rename 两段式，失败时旧 key 仍在位、可原位重试，且不触碰 `/data/adb/modules` 用户数据。事后判据：`apd selftest --json` 的 `superkey_file`（`stage=deployed` 时 critical）+ `rc_template_present` + `rc_started`（后者非 critical，不可判定时报 `unknown (not applicable)`，绝不误报失败）。

**5) 版本一致性（rc 指向的二进制 ↔ APK 内 `libapd.so`）**

- rc 只引用固定路径 `/data/adb/apd`，**永不**引用 APK 的 `nativeLibraryDir/libapd.so`（APK 路径每次安装都变，且 App 上下文无可执行权限）。
- 自启链因此运行「已部署二进制」。当它比 APK 内置副本旧：`apd version --json` 的 `commit` 与 APK 内 `apd-version.txt.commit` 不一致 → `selftest.version_consistency`（critical）失败、`summary=needs_repair`，管理器必须重新 `deploy`（幂等覆盖，`state.json.commit` 同步更新）；**禁止**静默用旧二进制继续提供 root-exec/部署语义。
- `deploy` 的 `install_file` 为 tmp+rename 原子替换：rc 侧要么 exec 到旧二进制、要么 exec 到完整新二进制，绝不会 exec 到半写文件。
- `/data/adb/apd` 缺失（已刷入新 boot 但尚未 deploy）：`start sproot-apd` 失败只在 init 日志留痕，不阻断开机、不产生用户态错误日志；结合 `oneshot`+`disabled` 不会形成重启风暴。

### 3.2 部署路径（/data/adb 与隐藏目录）

```
/data/adb/
├── apd                        # AP 主二进制  0755 root:root   ← 保持路径不变（模块脚本/生态引用最广）
├── ap/
│   ├── bin/
│   │   ├── busybox            # 0755（APK 首装部署，契约 §5）
│   │   ├── resetprop -> /data/adb/apd      # multicall 符号链接
│   │   ├── magiskpolicy -> /data/adb/apd   # multicall 符号链接
│   │   └── su -> <隐藏目录>/<随机名>        # 仅 root 上下文可用（目标 0700，见 §4.3/E2）
│   ├── allow_list             # 授权表 csv 六列  0600 root
│   ├── allow_list.lock        # flock 锁文件  0600
│   ├── log/                   # 0700；日志轮转（logcat/dmesg 抓取保留）
│   ├── module_configs/<id>/   # persist.config / tmp.config
│   └── update                 # 模块待生效标志
├── modules/<id>/              # 生效模块（0700）
├── modules_update/<id>/       # 安装暂存（**路径不可改**，installer.sh 依赖）
├── metamodule -> modules/<id> # 符号链接
├── post-fs-data.d/ service.d/ boot-completed.d/   # 公共脚本目录
└── sproot/                    # SPRoot 私有状态  0700 root
    ├── key                    # 47 字符明文（无 NUL）0600 root  ← rc 的 --superkey-file 固定路径（§3.1.3）
    ├── init.sproot.rc         # 自启模板审计副本 0644（init 实际读 ramdisk /init.sproot.rc）
    ├── safemode               # 存在即安全模式（契约 §3.5 第三路）
    ├── boot_count             # 未到 boot-completed 的连续启动计数（bootloop 守卫）
    └── state.json             # 部署状态/版本/时间（不含密钥）

/data/<key[0:16]>/            # SK 隐藏目录（filldir 双路隐藏，A1 §4.2/§4.4）
├── <随机16字符名>             # AP multicall 副本 0700 root（= install_su 等价物）
└── ...
```

**权限/标签要点**：隐藏目录 label 伪装 `system_file`（A1 §4.4）；`key` 0600 root；授权表 0600；隐藏目录内 su 副本 0700 root（**非 root 连 exec 权限都没有**；不变量：隐藏目录本身不可列出）。`bin/su` 符号链接本身只是给 root 上下文的便捷入口，不构成对普通 App 的授权面（第三方授权走 §4.4 socket）。

---

## 4. 与 SK 密钥体系的对接（含契约勘误）

### 4.1 提权（execve key）落地

**引擎实现（最终权威）**：`engine/src/patch_do_execve.cpp:76-98` —— shellcode 头部 48 字节即密钥本体；hook 拿到 `do_execve` 的 filename 参数后**逐字节比对到 NUL**，命中则就地改写 cred（uid/gid 全 0、caps 全满、securebits=0、清 TIF_SECCOMP 与 `task->seccomp`），随后执行被替换的原指令并跳回 `do_execve+4`；路径不存在 → execve 返回 `ENOENT`，但进程已是 root。密钥落盘时去尾：`skroot_patch_main.cpp:312-317` 写入 `key[0:47] + '\0'`。

**AP 侧实现**：`app/apd/src/supercall.rs:42-62`（`escalate()`：`getuid()==0` 早退 → `execve(c_key)` → 以 `getuid()==0` 判成功）、`69-83`（`privilege_apd_profile` 日志脱敏）。**该实现正确，保持不动。**

**落地口径（写进实现规范）**：
1. 提权串 = `key[0:47]`（47 字符，`[A-Za-z0-9]`，**不含斜杠**），由 `--superkey`/`--superkey-file` 传入后自行截断；若误传 48 字符全文 → 必然失配（多一个字符）。
2. 成功判据只有 `getuid()==0`；**execve 返回值一律不看**（成功路径必为 -ENOENT）。
3. 失败=静默（内核无计数、无告警）；AP 记一行脱敏日志（不含密钥或任何前缀）。
4. 提权后 AP 应尝试 `setcon("u:r:magisk:s0")`（A1 §4.3：root 进程可自由 setcon）；失败保留原 context，不得 panic。
5. 不缓存密钥、不建会话（契约 §2.3 方案 B）；每次进程启动由 `--superkey-file` 读取，读后 `mlock` + 立即从内存敏感位置擦除是可选加固（P2）。

### 4.2 密钥文件与生命周期

| 项 | 规格 |
|---|---|
| 路径 | `/data/adb/sproot/key`（0600 root:root，仅 root 可读；rc 的 `--superkey-file` 固定路径，§3.1.3） |
| 内容 | `key[0:47]` 原文，**47 字节纯文本**、无 NUL、无换行（AP 侧格式；内核 shellcode 内嵌的「47+NUL」不是本文件；AP 读取时仍丢弃 NUL/空白以兼容误写） |
| 写入者 | 管理器 `deploy` 步骤（首装，§6.2）；此后不由任何模块/脚本改写 |
| 读取者 | rc 拉起的 AP（`--superkey-file`）；管理器仅通过 `root-exec` 使用副本 |
| App 侧存储 | Android Keystore（AES/GCM）加密后存 SharedPreferences；**App 私有目录明文副本用完即删**（每次 root 操作写临时 0600 文件 → 调用 → 删除） |
| 轮换 | 不支持热轮换（内核内嵌密钥，只能重新修补 boot）；`undeploy` + 重新修补 = 换钥 |

### 4.3 su 部署（隐藏目录）

- **形态**：`/data/<key16>/` 下放 AP multicall 副本，文件名**随机 16 字符**（`[A-Za-z0-9]`），0700 root；`/data/adb/ap/bin/su` 为指向它的符号链接，argv[0] basename 为 `su` → `cli.rs:220-231` 分发到 `apd::root_shell()`（`apd.rs:51-308`）。
- **谁能用它**：只有**已经是 root 的上下文**（daemon、root-exec 会话、已提权的 adb shell）。普通 App 即使能走到该路径也会因 0700 得到 `EACCES`；即便未来放宽权限，直接 exec 也**不会提权**（提权需要先 `execve(key)`）。因此该文件**不承担对普通 App 的授权**——授权走 §4.4 的 socket（这也是对契约 §3.4「用户/脚本执行 su 时 argv[0] 分发命中」的收紧解释，见 E2）。
- **部署者**：管理器 `apd deploy`（首装）。**不再依赖独立的 skroot-deploy 组件**——新架构下部署职责全部并入管理器/AP（时序稿 §4 的分工因「修补在手机上由管理器完成」而改变，见 E5）。

### 4.4 授权表用户态化落地

> **2026-11-10 更新（三态升级，见 §13 / 勘误 E6）**：授权表决策源已改为 `/data/adb/sproot/auth_list.json`（三态 `allow|deny|ask` + 审计日志 + CLI）。本节描述的 csv 机制**已降级为一次性迁移源**；单写者/原子写/uid 保护/熔断语义逐条保留在 §13。

**存储**：`/data/adb/ap/allow_list`，csv 六列 `pkg,exclude,allow,uid,to_uid,sctx`（沿用 `package.rs:13-21` 的 `PackageConfig` 布局 + 表头 `package.rs:32`）。

**一致性模型**：
- **单写者** = daemon（uid-listener 线程 + `auth` 子命令 + socket 会话授权记录），全部经 `auth_list::lock_for_write()`（`auth_list.rs:41-52`，flock LOCK_EX）；
- **写** = tmp + rename（`package.rs:127-185`），读者无锁（见到的永远是完整一代）；
- **安全语义保留**（从内核对端迁移，不弱化）：torn-read 熔断（不可读且存在非关键授权 → 拒绝重写，`auth_list.rs:84-91`）、fail-open 边界、uid 0/2000 保护（`auth_list.rs:29-33`）、`synchronize_package_uid()` 的「空 packages.list 视为撕裂、拒绝同步」+ uid 重算（`package.rs:241-330`）。

**判定点（按调用来源分两类，替代契约 §3.2 的单一 SP_REQUEST_UID 方案）**：

| 来源 | 身份获取 | 判定 |
|---|---|---|
| 第三方 App（目标形态） | **抽象 socket `SO_PEERCRED`**（内核填写的真实 uid，不可伪造） | 读 allow_list：`allow==1 && exclude==0` 或受保护 uid → 放行；否则静默拒绝（exit 1，不解释） |
| 管理器（首装/韧性路径） | 无 socket：管理器持有密钥，`root-exec` 直接以 root 运行 | 视为「设备所有者」，不查表（与 Magisk 的 Manager 等价）；操作仍写入审计日志 |
| `/data/<key16>/su` 直连 | 调用者已是 root | 不查表（文档化为「root 上下文入口」，不对外宣传） |

**授权会话派发（daemon）**：socket 收请求 → SO_PEERCRED 取 uid → 查表 → fork 会话子进程：`setuid(0)`（或按 `to_uid`/`sctx` 降权/切 context）→ exec 请求的命令 → 回传 stdout/stderr/exit code。会话与 daemon 解耦（daemon 崩了不影响已建立会话；脚本崩了不影响 daemon）。

**socket 形态与暴露（已知风险，显式接受）**：Linux 抽象 socket `@sproot-<12hex>`（名字 deploy 时随机生成，写入 `/data/adb/ap/bin/.su.sock` 0644，供客户端库读取）。**代价**：`/proc/net/unix` 可枚举抽象名（SK 的 filldir hook 只管目录隐藏，管不到 /proc）。缓解：名字不含密钥任何片段；列为质测「检测面清单」已知项；后续评估（P2）是否要求 App 侧不落 socket 文件、改由管理器 Binder 中转。

### 4.5 安全模式 / 状态 / 日志

- 安全模式三路保留（`utils.rs:70-92`）：`persist.sys.safemode==1` / `ro.sys.safemode==1` / 文件 `/data/adb/sproot/safemode`；命中 → 禁用全部模块、跳过 stage 脚本（契约 §3.5）。
- **新增自动安全模式**（SK 无内核监管的必要补充）：`post-fs-data` 开始处 `boot_count += 1`，`boot-completed` 结束时清零；若 `boot_count >= 2` → 自动 `touch /data/adb/sproot/safemode` 并禁用全部模块（用户仍可手动删除该文件恢复）。
- 事件上报：`report_kernel` 退化为日志（`event.rs:26-31`，契约 §3.3 / 非目标 N2）；阶段完成状态落 `state.json`（供管理器读取），不落 `/sdcard`。
- 日志：`/data/adb/ap/log/`（0700，轮转保留）；`APatchD` tag 保留（生态脚本可能 grep）；**禁止**出现密钥原文/前缀（契约 §2.3）。

### 4.6 契约 v1.0 勘误与偏差（必须会签）

| 编号 | 契约原文 | 引擎/现实 | 本方案口径 | 影响面 |
|---|---|---|---|---|
| **E1** | §3.1「密钥字符串本身是 SK 隐藏目录下的可执行路径（`/data/<key16>/su`）」 | `patch_do_execve.cpp:90-98` 是 filename **逐字节 strcmp** 内嵌 47 字符密钥；`skroot_patch_main.cpp:315-317` 写入 47+NUL | 提权串 = **47 字符密钥本体**（无斜杠）；`/data/<key16>/su` 只是 su 部署位置 | 文字勘误，机制不变；但**实现若照原文写必失败** → 契约 v1.1 必须改 |
| **E2** | §3.4 部署形态：隐藏目录下固定名 `su`，用户/脚本执行即命中 argv[0] 分发 | A1 §4.4：上游 kit 用**随机名**（`lib`+随机字母+`.so` 风格）；固定名在随机目录内仍可被 8 字符文件名扫描命中 | 隐藏目录内**随机 16 字符名** + `/data/adb/ap/bin/su` 符号链接；直连 su 仅限 root 上下文 | 与契约字面偏离，隐藏性收益明确 |
| **E3** | §3.2 + 时序稿 §3.6：`SP_REQUEST_UID` 由 SK do_execve hook 在 cred 改写时注入环境变量，供 AP 还原请求方 uid | **不可实施**：hook 在 do_execve 入口、任何 exec 语义之前，无能力构造新的 envp 数组（引擎亦无相关代码）；且提权后 `getuid()==0`，原 uid 已不可恢复 | 废弃 `SP_REQUEST_UID`；第三方身份用 **socket SO_PEERCRED**；`apd.rs:196-205` 的 env 读取删除 | **契约 v1.1 删除该需求**；`apd.rs` 改造 P0 |
| **E4** | §3.2 授权表=`allow_list` csv（含 exclude 列）+ §3.6 另有 `exclude_list`（每行 uid） | 现实现把 exclude 放在 csv 行内（`PackageConfig.exclude`），`exclude_list` 未实现；两份存储 = 双写不一致风险 | **只保留单一 `allow_list`（含 exclude 列）**，删除 `exclude_list` 独立文件口径 | 简化，需会签确认 |
| **E5** | 时序稿 §4：rc 注入/install_su/key 文件由独立部署器 `skroot-deploy` 交付 | 新架构「手机本地修补」：管理器在修补时即可写 rc；无独立部署器进程 | 部署职责并入 **管理器（rc 注入）+ AP `deploy` 子命令（install_su/key/目录）** | 分工变更，需 lead + 内核修补确认 |
| **E6** | §3.2/E4：授权表 = 单文件 csv `allow_list`（`pkg,exclude,allow,uid,to_uid,sctx`） | 人类要求「root 授权页面」：需每应用三态 + 授权日志 + 搜索 → csv 无法表达状态/时间戳/计数 | 决策源改为 **`/data/adb/sproot/auth_list.json`**（三态 `allow|deny|ask` + `request_count`/`last_*_at`）+ 审计日志 `auth_log.jsonl`；csv 仅一次性迁移源（`allow&&!exclude`→allow，其它→deny），升级不丢授权 | 存储格式变更（决策语义不变，仍需会签）；详见 §13 |

> **未偏差项**（保持逐条一致）：密钥方案 B 无状态传参（§2.3/§4）、错误码禁令/静默拒绝（§2.1/§2.3）、日志脱敏（§2.3）、su 路径契约的 argv[0] 分发（§3.4 前半）、`kp` 别名删除（§3.4）、安全模式三路（§3.5）、busybox 首装归属（§5）。

---

## 5. 启动时序（手机本地修补 + SK 零事件内核）

### 5.1 rc 注入物（定稿建议，由管理器在修补流程内写入 ramdisk）

**新建 `init.sproot.rc`**（cpio 根目录，0644 root）：

> **模板实体已入库**：`apd/assets/init.sproot.rc`（Lead 独占维护；本文不再复制全文，避免双源漂移）。核心 service 行逐字为：
>
> ```
> service sproot-apd /data/adb/apd --superkey-file /data/adb/sproot/key daemon
> ```
>
> 语义（`oneshot` + `disabled` + `override`、`on property:sys.boot_completed=1` 触发、AP/密钥未部署时惰性无害、不写 seclabel）以该文件与 §3.1.3 为准。`apd deploy` 会把它落盘为 `/data/adb/sproot/init.sproot.rc` 审计副本（0644）。

**既有 `init.rc` 末尾文本级追加一行**：`import /init.sproot.rc`（magiskboot `cpio add`；不依赖 kptools embed-extra——SK 无开机解压运行时，时序稿 §3.4 已定）。

#### 5.1′ 备选交付路径：C′ rc 虚拟化（`--launcher rc`，2026-11-10 补写）

> **本节为此前缺失的补写**：C′ 是 2026-11-10 落地的头号架构简化项（`docs/ARCHIVE-2026-11-10.md` §2.3），但本文与 NEW-DOC-01 此前零记载、仍按「RcInject 必做」叙述。**以代码为准**：`engine/src/patch_rc_virtualize.{h,cpp}`、`engine/src/skroot_patch_main.cpp`。

**机制**（clean-room 实现）：hook `__arm64_sys_openat`（双过滤 `uid==0` + `O_RDONLY`）→ 原地把 init 要读的 rc 路径覆写为 `/dev/user_init.rc`（19 B）→ init 读到「原 rc 原文 + 我们的追加段」，**根本不需要改 ramdisk**。

| 维度 | 结论 |
|---|---|
| 默认值 | **`--launcher off`（默认）**；off 时产物与旧引擎**逐字节相同**，`hook_groups_expected` 不变 |
| 组数联动 | `hook_groups_expected_count(ge61, huawei, launcher_rc)` 三参：off → 14/11（+huawei 15/12）；on → 15/12（+huawei 16/13）（`patch_pipeline.h:58-61`） |
| 失败语义 | 显式 `--launcher rc` 但前置不满足（符号缺失/无洞/洞非法）→ **E202 且不产出 `--out`**，绝不静默降级（`skroot_patch_main.cpp:1069-1071`）；`--launcher umh` 未实现 → E101（`:882-891`） |
| 对本文的自启链影响 | **开启后可退役整条 init_boot 修补链**（unpack/repack/T-12/AVB），启动时序不变——rc 内容仍是 `apd/assets/init.sproot.rc` 语义，只是送达方式从「写 ramdisk」改为「运行期路径替换」 |
| 当前状态 | **未验证**：真机 5 项（`/dev` 时机、PAN/UAO、SELinux、init 解析拼接、开机稳定性）**尚未跑**（ARCHIVE §5 P0）；静态断言覆盖不到。**真机窗口关闭前不得宣称可用**，本文仍以 §5.1 的 RcInject 为默认路径 |

**AVB / 分区约束（2026-11-08 AVB 专项结论，三家方案一致：不碰 AVB 密码学）**：
1. **禁止任何 vbmeta/AVB 操作**：不改 `vbmeta.img`、不加 `--disable-verification`、不改 AVB 元数据。仅依赖「已解锁 bootloader 跳过验证」这一前提。
2. **刷入链**：`fastboot flash boot`（双槽 `--slot=all`）+ `reboot`；**不得**出现 `fastboot flash vbmeta` / `fastboot --disable-verification`（后者还会清数据，必须禁用）。
3. **rc 注入目标与尺寸**：优先 `init_boot`（有则用 init_boot，其次 boot 的 ramdisk）；rc 注入会改变 ramdisk cpio 大小 → **必须按目标分区大小补齐**（`init_boot` = 8388608 字节），保证镜像长度与分区一致，避免 AVB/分区校验与刷入异常。
4. **审计证据**：`deploy` 在 `state.json` 写入**实测三态** `avb` 字段（`AvbObservation::code()`，`apd/src/deploy.rs:546-554`）：`"avb_ok"`（定位到 footer 且已摘要 vbmeta 区）/ `"avb_no_footer"`（该分区**没有** AVB footer，属合法设备态但**不构成 PASS**）/ `"avb_unobservable"`（读不到分区，**unknown ≠ pass**）；配套记录 `avb_footer_sha256` / `avb_vbmeta_sha256` / `avb_source` / `avb_partition_bytes`。`apd selftest` 的 `avb_untouched` 检查项由 `avb_verdict()`（`apd/src/selftest.rs:273-`）做「部署时记录值 vs 现场重测」比对，**只有 `avb_ok` 且两侧 footer+vbmeta 摘要相同才 `ok`**，其余一律 `unknown` 或 `critical fail`。
   > **2026-11-10 修订（本条曾有自指哨兵，属假安全）**：原文写「写入 `"avb": "vbmeta_untouched"`，selftest 据此断言」——这是**写入一个常量再把同一个常量读回来比对**的恒真断言（`selftest.rs:774`、`defs.rs:68` 均有此教训记载）。现改为**可证伪的实测比对**：footer 消失 / vbmeta 摘要变化 / 分区尺寸变化 / footer 凭空出现 → 一律 fail。**注意 CI marker 名仍沿用 `vbmeta_untouched` 字符串（见 §7 的 marker 说明），但它是检查项标签，不是 state 值**。

**为什么 services/boot-completed 不各占一个 rc 触发器**：
1. 厂商 `init.rc` 结构差异大，rc 钩子越少越稳（时序稿 §3.4 实测门禁项）；
2. `exec` 命令是**同步阻塞** init 的（模块 post-fs-data 语义需要），但 `service` 是异步且可被 init 监管/重启——常驻 daemon 用 service 比 `exec &` 可靠；
3. boot-completed 用 daemon 轮询 `sys.boot_completed==1` 检测，规避 `on property:` 触发器在部分厂商早期阶段不触发的问题。

### 5.2 阶段表

| 阶段 | 触发 | 执行者 | 阻塞 | 关键内容（`event.rs`） | 超时/失败语义 |
|---|---|---|---|---|---|
| ① post-fs-data | `on post-fs-data` → `exec`（init 等待） | `apd post-fs-data` | **同步**（阻塞 init，必须快） | umask → 提权 → magisk_rules 注入 → 清 temp config → safe_mode 判定 → `post-fs-data.d` → `modules_update` 搬运 → prune/restorecon → sepolicy.rule → metamodule metamount → 模块 post-fs-data.sh → lua → system.prop → post-mount 阶段 | 单脚本超时（默认 60s，可配）+ 全阶段总预算；超时 kill 进程组并记日志；**任何失败不得 return Err 中断后续模块**（现实现已按 warn 容错） |
| ② services | daemon 启动后立即（`on boot`） | daemon 派发 | **异步**（`module.rs:254-257` block=false） | metamodule service.sh → `service.d` → 模块 service.sh → lua | 失败仅日志；不阻塞开机 |
| ③ boot-completed | daemon 轮询 `sys.boot_completed==1`（1s 间隔，上限 180s） | daemon 派发 | 异步 | metamodule boot-completed.sh → `boot-completed.d` → 模块 boot-completed.sh → lua；随后清 `boot_count`、启动 uid-listener 线程 | 轮询超时 → 记日志并保持 daemon 存活（下一轮可补） |

### 5.3 daemon 内部结构（新增，`daemon` 子命令）

```
apd --superkey-file /data/adb/sproot/key daemon
  ├─ 0. 读 key → escalate（execve(key47)）→ 断言 getuid()==0；失败：脱敏日志 + exit 0（静默，等下次开机）
  ├─ 1. 单实例锁（/data/adb/ap/daemon.lock, flock）
  ├─ 2. 运行 services 阶段（fork，不等待）
  ├─ 3. 启动 uid-listener 线程（inotify /data/system/packages.list.tmp，去抖 1s；触发
  │       synchronize_package_uid() → 表重写；单写者 flock）
  ├─ 4. 启动授权 socket（抽象名 @sproot-<12hex>；SO_PEERCRED → allow_list → fork 会话）
  ├─ 5. 主循环：轮询 sys.boot_completed → 到 1 则跑 boot-completed 阶段 + 清 boot_count；
  │       随后常驻（等 socket 请求 / 处理 SIGTERM 清理）
  └─ 6. 状态：/data/adb/sproot/state.json（阶段完成时间、模块数、daemon pid）
```

### 5.4 首启窗口（静默降级，隐蔽性要求）

| 状态 | 行为 |
|---|---|
| rc 已注入，但 `/data/adb/apd` 不存在（首启，管理器尚未部署） | `exec` 失败，init 记 kmsg；**不阻断开机**；不产生用户态日志 |
| AP 在但 `/data/adb/sproot/key` 缺失/不可读 | `--superkey-file` 读取失败 → 现有 `cli.rs:239-252` 静默降级 → 未提权 → 各阶段操作失败但**不 panic、不刷错误日志** → exit 非 0 |
| key 失效（用户重刷原厂内核但 `/data` 残留） | 提权失败 → 同上静默；管理器检测（`version --json` 探测 getuid 结果）提示「需重新修补」 |

### 5.5 bootloop 守卫（SK 无内核监管下的必要补强）

1. `post-fs-data` 入口 `boot_count += 1`；`boot-completed` 完成时清零（§4.5）；
2. `boot_count >= 2` → 自动写 safemode 文件 + 禁用全部模块（`module::disable_all_modules()`）；
3. 用户在管理器/ADB 删除 `safemode` 文件即可恢复；管理器 UI 显示「上次启动未完成，已进入安全模式」；
4. 模块脚本一律 `process_group(0)` + 超时 kill（`module.rs:229-239` 已有进程组，补超时即可）。

---

## 6. 部署与自举（首装顺序与回滚）

### 6.1 三段式时序（循环依赖的解法）

**循环依赖**：模块要 root 才能装/跑；root 要靠修补后的内核；而修补后的内核要 /data 里先有 AP + key 才能拉起守护——AP/key 又需要 root 才能写入。

**解法**：把「获取 root」从「AP 部署完成」里剥离——AP 二进制**自带自提权**（`supercall.rs:42-62`），管理器持有密钥，因此管理器可以**以普通 App 身份 exec AP，让 AP 自己提权**，无需任何预置 root 环境。

| 阶段 | 时机 | 主体 | 动作 | /data 状态 |
|---|---|---|---|---|
| **T1 修补期** | 用户点「修补」 | 管理器（无 root） | 选 boot.img → magiskboot unpack → `skroot-patch --in kernel --out kernel.new --gen-key` → 取回 48 字符 key（Keystore 加密存 SP）→ 写 `init.sproot.rc` + `init.rc` 追 import → magiskboot repack → 导出/刷入 | **完全不动 /data**（只在 App 私有目录有 working dir 与 key） |
| **T2 首启** | 刷入后第一次开机 | init / 管理器 | ① rc `exec` 失败（预期，AP 不存在）；② 用户打开管理器 → 写临时 key 文件（0600，App 私有）→ `exec nativeLibraryDir/libapd.so --superkey-file <tmp> deploy --source <nativeLibDir>` → AP 自提权 → 执行 §6.2 清单 → 删除临时 key 文件 | `/data/adb/{apd,ap/...,sproot/...}` + 隐藏目录就绪；**模块能力尚未生效** |
| **T3 二次开机起** | 之后每次开机 | init | rc → post-fs-data → daemon → services/boot-completed → 模块生效、授权可用 | 稳态 |

**T2 的 UI 必做**：部署完成后明确提示「**需重启一次，模块功能才会生效**」；重启前模块页禁用安装按钮（或安装后提示重启生效，与 `modules_update` 语义一致）。

### 6.2 `apd deploy` 清单（原子、幂等、可校验）

> **2026-11-10 修订：本清单实为 10 步（原写 9 步）**。步数以 `apd/src/deploy.rs` 的运行时日志为准（`:87-133`，逐条打印 `[deploy] step N/10: …`）；旧文把「rc 审计副本」并入 key 步、「socket 名」并入 state 步，导致编号与实际不符。下表按代码 1:1 对齐。

按顺序执行，每步失败即中止并保留日志；已存在且校验通过则跳过（幂等）：

| # | 动作 | 校验 |
|---|---|---|
| 1 | `mkdir -p /data/adb/{ap/{bin,log,module_configs},sproot,modules,modules_update}`，权限 0700/0755（`ensure_layout()`） | 存在性 + mode |
| 2 | 复制 `libapd.so` → `/data/adb/apd`（0755）（`install_file`） | **sha256 与源一致**；并校验 `apd-version.txt`（见下「版本一致性检查」） |
| 3 | 复制 `libbusybox.so` → `/data/adb/ap/bin/busybox`（0755） | sha256 |
| 4 | `ensure_symlink(/data/adb/apd → /data/adb/ap/bin/{resetprop,magiskpolicy})` | `readlink` 目标 |
| 5 | **key file**：从临时 key 文件读密钥 → 写 `/data/adb/sproot/key`（**47 字节纯文本、无 NUL**；0600 root；tmp+fsync+rename 原子写，幂等；各分支擦内存副本） | `mode=0600`、`len=47`、无 NUL；写入失败 → deploy 明确失败（`?` 直接中止，绝不「假装成功」） |
| 6 | **rc audit copy**：写 `/data/adb/sproot/init.sproot.rc`（0644，审计副本，与 ramdisk 内 `/init.sproot.rc` 同源） | 文件存在且与模板 sha256 一致 |
| 7 | **install_su 等价**：`key16 = key[0:16]`；`mkdir /data/<key16>`（0700，label 伪装）→ 复制 AP 副本为 `<随机16字符>`（0700）→ `ln -sfn <隐藏路径> /data/adb/ap/bin/su` | 隐藏文件可执行；`exec -a su` 冒烟 |
| 8 | 初始化 `/data/adb/ap/allow_list`（表头 + 空）+ `.lock`（0600）+ 生成/保持抽象 socket 名 `/data/adb/ap/bin/.su.sock`（0644，名字不含密钥，幂等以免打断已运行 daemon） | 可被 daemon 解析；socket 名可被客户端读取 |
| 9 | 写 `/data/adb/sproot/state.json`（版本、**build commit**、部署时间、隐藏文件名、**`avb` 实测三态审计位**（`avb_ok`/`avb_no_footer`/`avb_unobservable`）+ `avb_footer_sha256`/`avb_vbmeta_sha256`/`avb_source`/`avb_partition_bytes`——**不含密钥任何片段**）+ 清空 `boot_count` | JSON 可解析；`avb` ∈ {`avb_ok`,`avb_no_footer`,`avb_unobservable`}（**不得是常量哨兵**） |
| 10 | 冒烟：`/data/adb/apd version --json`（以 root）+ `su -c id`（经 root-exec）（`smoke_check`） | exit 0 |

> **⚠️ 已知缺口（与 `docs/ARCHIVE-2026-11-10.md` §3.2/§5 的 P0 同源）**：代码 step 4 只建 `resetprop`/`magiskpolicy` 两个符号链接，**未把 `apd` 放进 `/data/adb/ap/bin/`**，导致 NeoZygisk 侧 `Command::new("apd")` 找不到 → `RootImpl::None` → Zygisk 链静默失效。以 ARCHIVE 记录为准，此项仍待修（2026-11-10 时点）。

**版本一致性检查（APK 内 libapd.so ↔ /data/adb/apd，2026-11-08 lead 要求）**：
1. 客户端 CI 从 apd 工作流拉取**固定命名 artifact** `apd-aarch64-latest`（含 `libapd-arm64.so` + `apd-version.txt`，字段：`commit / version / sha256 / size_bytes / run_id`），打进 APK `jniLibs/arm64-v8a/libapd.so`；
2. 管理器调 `apd version --json` 读已部署二进制的 `commit`（由 `build.rs` 编译期注入 `VERSION_COMMIT`），与 APK 内置 `apd-version.txt.commit` 比对；
3. 不一致 → 管理器提示「需重新部署」并允许 `deploy` 覆盖（`deploy` 幂等，写入的 `state.json` 同样带 `commit`）；**禁止**静默用旧二进制提供 root-exec（行为差异会导致授权/部署语义漂移）。

**失败回滚（T2 内）**：deploy 整体是「先写临时目录 → rename 就位」的两段式；**第 2~9 步**任一步失败 → 删除已就位物（只删本步清单内文件，不碰 `/data/adb/modules` 用户数据）→ 状态仍为未部署，可重试。（2026-11-10 修订：原写「第 2~8 步」，随 10 步清单对齐；step 10 是只读冒烟，无需回滚物。）

### 6.3 回滚矩阵

| 场景 | 现象 | 处置 |
|---|---|---|
| 引擎修补失败 | 引擎非 0 退出（C1 §3：**不写 --out**） | 管理器保留原 boot.img 副本 + sha256，删除半成品；UI 展示完整错误（对照 C1 错误码表给出人话建议） |
| 刷入后无法开机 | 卡 logo/重启循环 | **fastboot 刷回原厂 boot**（管理器修补前把原 boot.img 备份到 App 外部私有目录并支持导出）；A/B 机型走 `bootctl` 备槽 + 开机成功后 `mark-boot-successful` |
| 开机但模块导致 bootloop | 进系统后反复重启 | 自动安全模式（§5.5，boot_count≥2）；用户可 `adb shell touch /data/adb/sproot/safemode`（有 adb root 时）或进 recovery 删文件 |
| 管理器误删/未部署 | rc 每次失败 | 不影响开机；重新打开管理器点「部署」即可（幂等） |
| 用户要彻底移除 | — | `apd undeploy --keep-modules` → 刷回原厂 boot；隐藏目录/key/sproot 全清 |
| 关键主二进制损坏 | `/data/adb/apd` 被破坏 | 管理器重新 deploy 覆盖（源 = APK 内置）；**不要把修复寄托在「旧二进制」上** |

### 6.4 `undeploy` 逆序清理

`/data/adb/ap/bin/su` 链接 → 隐藏目录（整个 `/data/<key16>/`）→ `/data/adb/sproot/`（key/state/safemode/boot_count）→ `/data/adb/ap/`（可选保留 module_configs）→ `/data/adb/apd`；最后提示用户刷回原厂 boot（否则 rc 仍指向已删的 AP，每次开机产生一条无害失败）。

---

## 7. 现有 `app/apd` 改造 backlog（带定位，供实现阶段直接开工）

| 优先级 | 项 | 位置 | 说明 |
|---|---|---|---|
| P0 | 新增子命令 `daemon` / `deploy` / `undeploy` / `root-exec` / `auth` / `version --json` | `cli.rs` `Commands` 枚举 | §3.1 清单 |
| P0 | 废弃 `SP_REQUEST_UID` 读取，改 socket peer cred | `apd.rs:196-205` | E3 |
| P0 | uid-listener 并入 daemon（线程化 + 单实例锁） | `event.rs:259-281`、`292-350` | 现为 `boot-completed` fork 子进程，易多实例 |
| P0 | 监听器触发时补 `synchronize_package_uid()`（当前 `refresh_allow_list` 只重写不剪枝） | `event.rs:39-42`、`auth_list.rs:67-128`、`package.rs:241-330` | 卸载 App 的授权行不会被清除（安全隐患：uid 复用后被误授权） |
| P0 | 授权表路径统一 `allow_list`（E4）+ 删除 `exclude_list` 口径 | `supercall.rs:98-103` | 新架构无迁移负担 |
| P0 | 模块脚本超时（`process_group` 已有，补 kill）+ bootloop 计数/自动 safemode | `module.rs:205-252`、`event.rs:44`、`utils.rs:70-92` | §5.5 |
| P1 | `argv[0]` 分发改 basename 精确匹配 | `cli.rs:220-231` | 现 `ends_with("su")` 过宽 |
| P1 | `assets.rs` 符号链接目标改用 `defs::DAEMON_PATH`（现硬编码 `/data/adb/apd`） | `assets.rs:10-21`、`defs.rs:10-13` | 改名时唯一会漏的点 |
| P0（外部依赖） | **Rust Android 交叉编译链**（`aarch64-linux-android` target + NDK） | 不属本仓库改造（`app/apd/` 代码本身） | **依赖 CI**：由 task-8 / patcher / app-client 固化 CI 镜像；本机仅能做 host `cargo check`（x86_64-linux 已通过），Android 产物与体积必须由 CI 实测 |
| P1 | 删除 KP 专属模块与依赖（`insmod`/`late_load`/`magica`/`soft_reboot`；Cargo: goblin/scroll/adb_client） | `main.rs:7,8,10`、`event.rs:355-395` | D9；预计减 1~2MB |
| P1 | 提权后 `setcon("u:r:magisk:s0")`（失败降级） | `apd.rs` 新增 | A1 §4.3 |
| P1 | `-Z` 从「接受并忽略」改为真实 setcon（授权会话用） | `apd.rs:102-110,156-160` | 第三方 root 的 context 需求 |
| P2 | 密钥读取后 `mlock` + 擦除；日志审计（`/data/adb/ap/log/audit.log`） | `cli.rs:239-252` | 加固 |
| P2 | 二进制瘦身（评估把 lua/sepolicy 拆到独立小二进制，缩短 rc 冷启动路径） | `Cargo.toml` | post-fs-data 关键路径耗时 |

---

### P0 实施进度（2026-11-08，代码已落地）

| P0 项 | 状态 | 落点 |
|---|---|---|
| 新子命令 `daemon`/`deploy`/`undeploy`/`root-exec`/`auth`/`version --json` | ✅ 代码完成 | `cli.rs` + 新模块 `daemon.rs`/`deploy.rs` |
| 废弃 `SP_REQUEST_UID` 内核注入 | ✅ | `apd.rs`：改读 daemon 设置的 `SPROOT_REQUEST_UID`（来源=socket `SO_PEERCRED`）；`auth_list::authorize(None)` = root 上下文 |
| uid-listener 并入 daemon | ✅ | `daemon.rs` 线程 + 单实例 flock；`event::start_uid_listener()` 留作手工入口 |
| `synchronize_package_uid` 剪枝接线 | ✅ | `auth_list::refresh_allow_list` → `package::synchronize_package_uid_locked`（同锁内，避免重入 flock） |
| 脚本超时 | ✅ | `utils::wait_timeout`（超时 kill 进程组）→ `module::exec_script`，默认 60s（`defs::SCRIPT_TIMEOUT_SECS`） |
| bootloop 守卫 | ✅ | `bootstate.rs`：post-fs-data +1 / boot-completed 清零 / ≥2 自动写 safemode 并禁用全部模块 |
| 授权表单文件 `allow_list`（E4） | ✅ | `defs.rs` 常量 + `package.rs`/`auth_list.rs`（无 `exclude_list`） |
| 符号链接去硬编码（P1 提前） | ✅ | `assets.rs` → `defs::DAEMON_PATH` |
| CI 交叉编译（禁止本地出 arm64） | ⏳ 工作流已写 | `.github/workflows/apd.yml`（rust + NDK r27c + ELF AArch64/PIE 断言 + artifact）；**待仓库集成**：根仓库 `.gitignore` 目前忽略 `/app/`，CI 取不到 apd 源码（待 lead 决定 un-ignore `app/apd` 或迁移到 tracked 路径） |
| 本地语法门禁 | ✅ 通过 | `cargo check`（WSL rustc 1.99.1 / x86_64-linux）；**仅语法验证，非交付物** |

### P2 实施进度（2026-11-08，代码已落地）

| P2 项 | 状态 | 落点 |
|---|---|---|
| 密钥 mlock + 用完即擦 | ✅ | 新模块 `secret.rs`：`SecretKey`（mlock 页范围 + `write_volatile` 擦除 + drop 解锁）；`supercall::escalate` 用自带缓冲并在 execve 后立即擦除；CLI 从 argv/文件进进程即包裹并擦 argv 副本；`--superkey-file` 也就地 mlock |
| `-Z` 真 setcon | ✅ | 新模块 `selinux.rs`：不依赖 libselinux，`open/write/close` 写 `/proc/self/attr/current`（async-signal-safe，可入 `pre_exec`）；`root-exec --scontext` 在 drop uid **之前**切换；`su -Z` 真实切换（失败仅告警，保留原 context） |
| daemon `--selftest` / `selftest` 子命令 | ✅ | 新模块 `selftest.rs`：检查 root/binary/key_file(0600/47-48/mlocked)/allow_list/hidden_su(0700)/目录权限/daemon pid/socket 探活/bootstate/version 一致性/**AVB 审计**；`--json` 供 App 与 qa；exit 0/1 |
| 体积再瘦身 | ✅ 已改（数值待 CI） | `opt-level = "z"`（原 3）；`env_logger` 移出 Android 构建（`cfg(not(target_os="android"))`）；上一轮已删 goblin/scroll/adb_client（2.11MB→1.35MB） |
| AVB 审计字段 | ✅ | `deploy` 的 `state.json` 增**实测三态** `avb`（`avb_ok`/`avb_no_footer`/`avb_unobservable`，`apd/src/deploy.rs:546-554`）+ 四个观测字段（`avb_footer_sha256`/`avb_vbmeta_sha256`/`avb_source`/`avb_partition_bytes`）+ `commit`；`selftest::avb_verdict()` 做「记录值 vs 现场重测」的**可证伪**比对；部署/卸载代码**零** vbmeta 写操作（grep 证实）。**2026-11-10 修订**：原写常量哨兵 `avb: "vbmeta_untouched"`，与写入后读回自身的恒真断言同源，已废弃 |
| 许可合规（AGPL-3.0） | ✅ | `Cargo.toml` 增 `license = "AGPL-3.0-only"`；**移除 GPL-2.0-only 的 `extattr`**（与 AGPL 不兼容）→ raw `libc` `lsetxattr/lgetxattr` 等价替换；`apd/THIRD_PARTY_LICENSES.md` 全量清单（115 包）。`prop-rs`/`prop-rs-android` 为 **UNKNOWN 许可（发布阻断项）** → 按 lead 裁决**不删 resetprop**，改为 **feature 隔离**：`default = ["resetprop"]`，两个 crate `optional = true`；`--no-default-features` 产出**许可干净构建**（resetprop 返回明确错误） |
| 许可干净构建路径 | ✅ | `apd/Cargo.toml` `[features] default=["resetprop"]`；`apd.yml` 新增 "Clean build check (no default features)"（`cargo check --locked --no-default-features --target aarch64-linux-android`）；capability 断言中 `resetprop` 标为**条件性**（缺省不判失败） |
| 体积实测（CI） | ✅ | `libapd.so` = **2,054,408 B**（run 42488946176）；`llvm-size -A`：.text 1,234,160 / .rodata 335,176 / .eh_frame 240,768 / .rela.dyn 82,992；capability 断言 15/15 OK |

#### P2 体积归因表（`libapd.so` 3.94 MB → 2.05 MB，qa 门禁背书）

> 单位说明：本节用**未压缩 ELF 体积**（3,935,048 → 2,054,904 B，来源 run 42485894887 → 42487199791）。此前记录里的 2.11/1.35/1.17 MB 是 **artifact zip** 体积，两者不同，勿混用。
> 拆分方法：zip 体积差（P1 -757,936 B / P2 -187,523 B）按比例分摊到未压缩总差 (-1,880,144 B)。

| 归因项 | 证据 | 未压缩估算 |
|---|---|---|
| **P1：删 goblin/scroll/adb_client + KP 模块** | `Cargo.lock` 一次性移除 **91 个 crate**（rustls/ring/rsa/der-parser/x509-parser/rcgen/num-bigint/chrono/pem… 全属 `adb_client` 的 TLS/PKI 依赖链） | **≈ -1.50 MB**（主因；PKI/TLS 内联代码） |
| **P2：`opt-level` 3 → `"z"`** | 同代码同依赖，仅 codegen；CI artifact zip -187,523 B | **≈ -0.34 MB** |
| **P2：`env_logger` 移出 Android 构建** | 原本只在 `cfg(not(android))` 引用，LTO 多半已剥；实际收益很小 | **≈ -0.02 MB** |
| 合计 | 3,935,048 → 2,054,904 B | **-1.88 MB（-48%）** |

**功能未删证据（qa 可直接复核）**：
1. `cargo tree --locked --target aarch64-linux-android`：mlua/mlua-sys（Lua 脚本）、zip、csv、serde、notify/inotify 均在；goblin/scroll/adb_client/env_logger 不在 Android 依赖树（P1 有意删除 / P2 移出）。
2. `src/` 24 个模块齐全（daemon/deploy/bootstate/secret/selinux/selftest/root-exec 路径等）。
3. CLI help 14 个子命令：module/post-fs-data/services/boot-completed/uid-listener/daemon/deploy/undeploy/root-exec/auth/version/selftest/resetprop/sepolicy。
4. `selftest` 16 个检查项含 `key_file.mlocked` 与 `avb_untouched`；undeployed/deployed 两阶段语义。
5. CI 新增 **capability marker 断言**：二进制内必须存在静态标记，缺一即 CI 失败（本轮起生效）。**当前列表（以 `.github/workflows/apd.yml:167-171` 为准，共 15 项）**：`daemon` `deploy` `undeploy` `root-exec` `selftest` `auth` `version` **`vbmeta_untouched`** `SPROOT_REQUEST_UID` `/proc/self/attr/current` `sproot-` `allow_list` `boot_count` `safemode` `module`；条件性 marker：`resetprop`（`--no-default-features` 构建缺省不判失败，`apd.yml:174`）。
   > **⚠️ 2026-11-10 发现（已知会红的门禁，需 ci-dev 处置）**：`vbmeta_untouched` 这个 marker **已失效** —— 该字面量已从生产代码中移除（state 改为实测三态 `avb_ok`/`avb_no_footer`/`avb_unobservable`，`apd/src/deploy.rs:546-554`），现仅残留在注释与 `#[cfg(test)]` 中（`selftest.rs:1045` 位于 `mod tests`，不进 release 二进制），**注释不参与编译**。因此 `grep -aq "vbmeta_untouched" libapd.so` 预期 MISS → `apd.yml:182-184` 直接 `exit 1`。
   > **建议替换**（文档先记录，实际改由 ci-dev 在 `apd.yml` 执行）：把 `vbmeta_untouched` 换成三个实测码 `avb_ok` `avb_no_footer` `avb_unobservable`（均为生产字面量），marker 总数 15 → 17。**在 ci-dev 落地前，本门禁处于「预期红」状态**，不要把它当作代码回归。
6. 全程 `--locked` 编译（本地 `cargo check` + CI `cargo build --release --locked`）。

---

## 8. 风险清单

| # | 风险 | 影响 | 概率 | 缓解 | 门禁/验收 |
|---|---|---|---|---|---|
| R1 | rc 注入兼容性（厂商 `init.rc` 非标准、`import` 位置、ramdisk `second_stage` 校验） | 开机不执行 AP → 全部模块能力失效 | 中 | 注入 `init.rc` 尾部 + 原文件备份 + 真机矩阵（魅族 21 优先）；失败可刷回 | 真机：三阶段都跑通并留 `state.json` 证据 |
| R2 | Rust→Android 交叉编译链缺失（当前仅 windows-msvc，无 NDK/target） | AP 二进制产不出来 → 整条模块链无法交付 | **高（已知）** | CI 固化 rust+NDK+`--locked`；先跑通 `cargo build --release --target aarch64-linux-android` | CI 产物可执行 + 体积记录 |
| R3 | 首启窗口无 root 提供者（用户不打开管理器） | 设备「有 SK 内核但无模块能力」，用户困惑 | 高 | UI 引导 + `version --json` 状态探测 + 文档说明；deploy 幂等可重试 | 首启流程实测（含未部署状态提示） |
| R4 | 模块 post-fs-data 脚本慢/挂死阻塞 init | 开机卡住/超时重启 | 中 | 单脚本超时 + 进程组 kill + 总预算 + bootloop 自动 safemode | 构造睡死脚本用例 |
| R5 | daemon 被系统杀/冻结 | 授权会话与 boot-completed 阶段缺失 | 中 | init `service` 自动重启；会话在子进程；阶段完成状态落盘可补跑 | 杀 daemon 后自动恢复实测 |
| R6 | 授权表误清/撕裂 | 授权丢失或越权 | 低 | tmp+rename + flock + torn-read 熔断 + 单写者 + 空 packages.list 拒同步（已实现语义保留） | 故障注入用例（截断文件/空列表） |
| R7 | 隐藏性回归：`/data/adb/ap/*` 与 `/data/adb/modules/*` 明面可见；抽象 socket 可见于 `/proc/net/unix` | 被检测工具识别 | **高（固有）** | 与 Magisk 同级暴露；密钥不落明文、日志脱敏、隐藏目录随机名；socket 名不含密钥 | 质测「检测面清单」逐项确认（含接受项） |
| R8 | 密钥泄露面（App 私有存储被 root/调试工具读取） | 全设备 root 失守 | 低 | Keystore 加密 + 明文仅临时文件 + 用后即删；不做备份导出 | 安全测试项（契约签署栏附加要求） |
| R9 | 固定路径 `/data/adb/apd` 被卸载器/检测器清理 | 守护失效 | 中 | `su`/隐藏副本冗余；deploy 可重建；`state.json` 可探测 | 删除 AP 后重新 deploy 实测 |
| R10 | daemon 与旧 APatch 语义漂移（模块兼容） | 存量模块静默失效 | 中 | 「行为不变」硬约束 + 模块兼容用例集（module.rs 语义逐条） | 模块生态回归用例 |
| R11 | 契约 v1.1 未及时修订（E1–E5） | 实现照旧契约写必然失败/返工 | 高 | 本文 §4.6 作为修订依据，lead 终审后同步契约 | 契约 v1.1 会签 |
| R13 | APK 内置 libapd.so 与已部署 /data/adb/apd 版本漂移 | root-exec/授权/部署语义不一致，故障难定位 | 中 | `apd-version.txt`（固定 artifact）+ `version --json.commit` + `state.json.commit` 三方比对；不一致禁止静默使用 | 版本一致性验收项（§9-8） |
| R12 | 二进制体积/冷启动（rc 关键路径） | 开机变慢、用户可感 | 低 | 体积实测 + 必要时拆小二进制 / 延迟加载 lua | 开机耗时对比（原厂 vs 修补） |
| R14 | rc 注入改动 ramdisk 尺寸 / 误碰 vbmeta | 刷入后 AVB/分区校验异常、无法开机 | 中 | 不碰 AVB 密码学；init_boot 优先 + 按分区大小补齐（8388608）；刷入链只 `fastboot flash boot --slot=all`；`state.json` 留 AVB 审计位 | §9-10 + 真机刷入回滚演练 |
| R15 | 密钥在内存/swap 残留 | 本机 root 凭据泄露 | 低 | mlock + volatile 擦除（SecretKey）；不落明文文件（0600 root 除外）；日志脱敏 | `selftest` mlocked 断言 + 进程内存抽查 |
| R16 | `prop-rs`/`prop-rs-android` 许可 UNKNOWN（无 LICENSE/无字段） | 含默认 feature 的产物**不可作为发布物**（AGPL 合规风险） | 高（已知） | feature 隔离 + `--no-default-features` 许可干净构建（CI 长期守护）+ release 清单标注；闭环二选一（上游授权 / 自研 bionic property area）由人类裁决 | release 候选必须来自干净构建（CI 可编译断言）；人工清单勾选 |

---

## 9. 验收标准（可执行）

1. **部署链**：`deploy` → sha256 校验全过 → `state.json` 就绪 → 二次开机后 `state.json` 含三阶段完成时间。
2. **模块链**：装一个样例模块（systemless + post-fs-data.sh + service.sh + boot-completed.sh 各写一行日志）→ 三阶段脚本都按序执行，日志可见。
3. **提权链**：`root-exec -- id` 输出 `uid=0`；错误密钥 → 静默失败（无 panic、无密钥泄露日志）。
4. **授权链**：`auth grant <uid>` 后 socket 请求成功；`auth revoke` 后 socket 请求 exit 1 且无输出；授权表 tmp+rename 期间并发读不崩。
5. **bootloop 守卫**：故意放一个挂死模块 → 第 2 次启动自动 safemode + 模块全禁用 + 系统可进桌面。
6. **回滚**：刷回原厂 boot → 系统正常；`undeploy` 后 `/data/<key16>`、`/data/adb/sproot`、`/data/adb/apd` 均不存在。
7. **契约一致性**：E1–E5 在契约 v1.1 中已修订或明确接受（会签记录）。
8. **版本一致性**：APK 内置 libapd.so 的 commit 与 `/data/adb/apd` 的 `version --json.commit` 一致；不一致时 UI 有明确「重新部署」提示。
9. **自检可用**：`apd selftest --json` 在健康部署上返回 `ok=true`、全 critical 项通过；删除 key/隐藏 su 后对应 critical 项失败且 exit 1。
10. **AVB 未被触碰**（**2026-11-10 修订：验收判据改为三态，原「`state.json.avb == "vbmeta_untouched"`」在新代码下必然失败**）：`state.json.avb` ∈ {`avb_ok`, `avb_no_footer`, `avb_unobservable`}，且：
    - **判 PASS 的唯一条件**是 `avb` == `avb_ok` **且** `apd selftest --json` 的 `avb_untouched` 检查项 `ok=true`（含义：deploy 时测到 footer 并记下 `avb_footer_sha256`/`avb_vbmeta_sha256`，现场重测两者一致且分区尺寸未变）。
    - `avb_no_footer` / `avb_unobservable` **不是 PASS 也不是 FAIL**，必须显式呈现为 **`unknown`**（`selftest.rs:277` 的 `NO_PASS` 文案「unknown — cannot be verified, NOT a pass」），并在报告里带 `avb_source`/`avb_partition_bytes` 便于定位。
    - **FAIL（critical）**：记录 `avb_ok` 而现场无 footer（分区被重写）、两侧摘要不一致（vbmeta 被动过）、分区尺寸变化、或 footer 凭空出现。
    - 刷入链无任何 vbmeta 命令；rc 注入产物按分区大小补齐（init_boot 8388608）。
11. **密钥无残留**：`selftest` 的 `key_file.mlocked=true`；进程退出后 `/proc/<pid>/cmdline`/日志中无密钥原文或前缀。

---

## 10. 待决/待会签

| # | 事项 | 建议 | 待谁 |
|---|---|---|---|
| Q1 | E1 提权串表述勘误（§4.6） | 契约 v1.1 改为「47 字符密钥本体」 | lead + 内核修补 |
| Q2 | E3 废弃 `SP_REQUEST_UID`，改 socket SO_PEERCRED | 采纳（唯一可实施路径） | lead + 内核修补 |
| Q3 | E4 `exclude_list` 合入 `allow_list` 单文件 | 采纳 | lead |
| Q4 | E5 部署职责并入管理器 + AP（取消独立 skroot-deploy） | 采纳 | lead + 内核修补 + 客户端 |
| Q5 | D9 删除 KP 专属子命令（偏离 phase1「能力保留」） | 采纳（新架构无 KP） | lead |
| Q6 | E2 隐藏目录内 su 随机命名（偏离契约固定名） | 采纳（隐藏性） | lead + 内核修补 |
| Q7 | 第三方授权 socket 的暴露接受（R7） | 接受并列入检测面 | lead + 质测 |
| Q8 | `root-exec` 作为管理器唯一 root 入口（APK↔AP 契约补章） | 采纳 | 客户端会签 |

---

## 11. 发布门禁（`.github/workflows/release.yml`，2026-11-08 新增）

触发器：tag `v*` 或手动 `workflow_dispatch`。**只产出 `release-manifest.json`，从不自动发布**。

| 门禁 | 断言内容 | 失败后果 |
|---|---|---|
| **LIC** | 根 `LICENSE`(AGPL-3.0)/`LICENSES.md`/`NOTICE` 存在；`quality/static/check_licenses.py` 必须 `RESULT: ALL PASS`；`LICENSES.md` 含「保留所有权利」+ SKRoot 来源；`NOTICE` 含 magiskboot 与 AGPL-3.0 说明；**APK 内 `assets/NOTICE*`** 必须覆盖 magiskboot（GPL-3.0 + 源码链接）、SKRoot（保留所有权利）、AGPL-3.0 | job 红，不可发布 |
| **ARCH** | APK 内 native 仅允许 `lib/arm64-v8a/*` 且逐个 `e_machine==183`；engine/magiskboot/apd 独立产物同样全 AArch64；APK manifest `extractNativeLibs=true` | job 红 |
| **VER** | `apd-version.txt.commit` == APK `assets/apd-version.txt.commit`；`libapd.so` sha256 三方一致（APK / apd artifact / version meta）；二进制内嵌 commit 可检索 | job 红 |
| **清单** | 聚合同 commit 的 `SPRoot-APK`/`skroot-engine-arm64`/`magiskboot-arm64`/`apd-aarch64-latest`，输出 `release-manifest.json`（每文件 size/sha256/来源 artifact+run） | job 红 |
| **engine 再许可** | `engine/` 含 SKRoot proprietary 内容且再许可未获书面确认 → **默认阻止**；仅 `workflow_dispatch` + `confirm_engine_redistribute=true` 放行，且 job summary 打印风险提示 | 默认红（有意为之） |

> 已知前置项：当前 APK 仅打包 `assets/NOTICE-magiskboot.txt`，缺 SKRoot/AGPL 段落 → LIC 门禁会按设计拦截；需客户端在打包时把仓库根 `NOTICE`（或合并版）一并放入 `assets/`。
>
> **发布候选构建要求（R16）**：`libapd.so` 必须来自 `--no-default-features` 的**许可干净构建**（依赖树无 UNKNOWN 项），否则即便 LIC 门禁通过也不得发布；默认构建产物仅供开发/日常使用。`release.yml` 需在候选来源上人工确认（engine 再许可开关同理）。

## 12. 密钥 ABI 全链一致性（静态核对，2026-11-09，apd-dev 落实）

> 背景：HANDOFF §6.8 的第 1/2 条静默错误（rc 路径漂移、clap 参数序）本质都是「同一份密钥在链上被不同层用不同口径解释」。密钥**长度**是第二个同类坑：**历史上 App 生成 48 字符、内核只存 47 字符**（客户端已于 2026-11-09 `5ed19c6` 统一为 47 字符）。任何一环把长度理解错，后果都是「提权静默失败 → 看起来像密钥没部署」（rc 125）。本节把全链五处的长度口径钉死，并在 `apd` 内用单测 + 编译期断言锁住第 1~4 处。

### 12.1 三条不可混淆的事实

| # | 事实 | 长度 | 形态 | 证据 |
|---|---|---|---|---|
| **F1** | AP 侧密钥文件 `/data/adb/sproot/key` | **47 字节** | 纯文本、**无 NUL**、无换行，0600 | `apd/src/defs.rs:54`；`apd/src/deploy.rs::write_key_file` |
| **F2** | 内核 shellcode 内嵌密钥 | **47 字节 + NUL**（48 字节槽位，第 48 字节恒 0） | 编译进跳板的 `ROOT_KEY_LEN` 缓冲 | `engine/src/patch_do_execve.cpp:73-100`；`engine/src/skroot_patch_main.cpp:618-623` |
| **F3** | 引擎 `--key` 入参 | **48 字符 `[A-Za-z0-9]`**（`--key-abi=47nul` 是唯一支持值） | CLI 参数；写内核时取 `key[0:47] + '\0'` | `engine/src/skroot_patch_main.cpp:61-64,105-107,434-447,470` |

**推论（最关键）**：内核 hook 把 `execve` 的 filename 与 F2 **逐字节比对且以 NUL 结束**。所以 **提权时 `execve` 的文件名必须恰好是 47 字符**：若客户端拿完整 48 字符去 `execve`，内核读到第 48 字节是 NUL、而文件名第 48 字节非 NUL → **必然失配 → `ENOENT` → 静默失败（表现为 125「密钥无效」）**。契约 v1.1 §8 已定 `apd root-exec` 是唯一 root 入口，客户端**不得自行 `execve` 密钥**。

### 12.2 五处口径核对表

| # | 环节 | 代码位置 | 输入 → 输出 | 一致性 |
|---|---|---|---|---|
| 1 | deploy 归一 | `apd/src/deploy.rs::abi_key()` | 47 原样；48（展示位，末字节须 NUL/空白/`[A-Za-z0-9]`）→ 前 47 且 `warn!`；49+ / <47 / 非 `[A-Za-z0-9]` → `bail!` | ✅ |
| 2 | 落盘字节 | `apd/src/deploy.rs::key_material()` / `write_key_file()` | 只接受**恰好 47** 字节；`[..47]` 式截断已删除（否则会写出与内核 token 不符的密钥）；tmp→0600→fsync→rename 原子；写后擦内存副本 | ✅（`deploy` 的 smoke_check 再断言 `len==47 && 无 NUL && mode==0600`） |
| 3 | daemon 读取 | `apd/src/secret.rs::read_key_file()` → `SecretKey` | 读文件后丢弃空白/NUL（兼容误写的 47+NUL）；canonical 文件即 47 → `SecretKey::len()==47`；48（旧管理器写法的文件）保留 48，交给第 4 处归一 | ✅ |
| 4 | 提权 execve | `apd/src/supercall.rs::escalation_token()` | 47 原样；48 → 前 47 且 `warn!`；**<47 / >48 / 非 ASCII → `Err`**（不再静默透传），`escalate()` 记一条脱敏日志后返回 `false` | ✅（本次新增，**提权成败最后一环**） |
| 5 | 引擎嵌入 | `engine/src/skroot_patch_main.cpp` | `--key` 必须 48 字符 alnum；嵌入取 `key[0:47] + NUL`；`--key-abi` 仅接受 `47nul`，其余一律 E101 | ⏳ engine-dev 回执（静态读码一致，证据 F3） |

**客户端侧口径（`5ed19c6`，2026-11-09）**：`SecretStore.KEY_LENGTH = 47`，生成/存储/`ApdClient` 传输一律 47 并严格校验；唯一例外是调引擎时用 `engineKeyOf()` 把 47 补位到 48（第 48 位被引擎 `erase` 丢弃，见 F3）。因此上表第 1 行的「48 兼容」分支现仅服务**历史 48 字符旧密钥文件/旧 APK**，正常路径不再触发。

**`selftest --json` 的可诊断面（部署后 `superkey_file` 是 critical 项）**：47 字节纯文本 → 通过；**历史 48 字节文件 → 通过 + `legacy48=true` detail + `warn!`**（不判失败，避免老用户升级即 `needs_repair`/exit 1；运行时按前 47 使用，`deploy` 会把它改写成 47）；其它长度 / 内嵌 NUL / 权限非 0600 → 失败。判定抽成纯函数 `selftest::superkey_abi_verdict()` 并由单测钉死。

### 12.3 断言（随 CI 编译的单元测试）

| 断言 / 测试 | 钉住的点 | 负样本（先证伪再交付） |
|---|---|---|
| `deploy::tests::abi_key_accepts_exactly_47` | 1 | — |
| `deploy::tests::abi_key_maps_48_and_47_plus_nul_to_47` | 1（48 / 47+NUL 归一为 47；48th 非 NUL/空白/alnum 报错） | 若改成「48 一律报错」→ 该测试 FAIL，且历史 48 字符密钥/旧 APK 的部署被打断 |
| `deploy::tests::abi_key_rejects_49_short_and_symbols` | 1+2 | 把 `key_material` 改回 `[..47]` 截断 → **FAIL**（实测） |
| `deploy::tests::rc_template_matches_defs_and_writer_path` | 起链 rc / defs / 写入路径三方 | 把 rc 参数序改成 `daemon --superkey-file …` → **FAIL**（实测） |
| `supercall::tests::escalation_token_is_exactly_47_or_an_error` | 4 | 把短密钥改回「原样透传」→ **FAIL**（实测） |
| `supercall::tests::key_file_to_escalation_token_stays_47` | 2+3+4（47 字节文件 → SecretKey 47 → token 47；47+NUL 兼容；48 文件 → token 47） | 链上任一处口径不一致即 FAIL |
| `selftest::tests::superkey_verdict_accepts_47_and_legacy_48` | 2/3 的**可诊断面**：47 通过；历史 48 **通过 + `legacy48` 标注**（非失败）；长度/权限/NUL 异常仍失败 | 把历史 48 判失败 → 该测试 FAIL（老用户升级即 `needs_repair`） |
| 编译期 `const _: () = assert!(KERNEL_KEY_LEN == 47);` | 4（ABI 常量） | 改常量而不重打内核 → **编译失败**，强制走全链评审 |

### 12.4 剩余待办 / 风险

1. ~~**ui-client 48 → 47**~~：**已完成**（`5ed19c6`，2026-11-09）：`SecretStore.KEY_LENGTH = 47`，生成/存储/apd 传输统一 47；仅调引擎 `--key` 时用 `engineKeyOf()` 补位 48（被引擎丢弃）。`apd` 的 48 兼容分支保留作为**历史文件/旧 APK 兜底**，正常路径不再触发；待历史密钥全部重生成后可收窄为「仅 47」。
2. **engine-dev**：F3 需回执确认（尤其 `--key-abi` 拒绝分支与被嵌入的 `key[0:47]` 偏移）；若 patcher 把 `--key` 直接改成收 47（去掉 `erase`），客户端需删掉 `engineKeyOf()` 补位，本节 F3 同步修订。
3. **禁止路径**：任何客户端/脚本不得自行 `execve(密钥)`（契约 v1.1 §8）；唯一条入口是 `apd root-exec`。若未来出现直连需求，必须先经过本节第 4 处的同一归一口径。
4. **变更纪律**：改 `KERNEL_KEY_LEN`、rc 的 `--superkey-file` 路径、`defs::SPROOT_KEY_FILE` 任意一者，都按「全链 ABI 变更」处理：重打内核 + 重新修补 + 同步客户端，并更新本节。

### 12.5 核对结论

五处静态核对**全部一致**（唯一待回执项是第 5 处引擎侧，静态读码一致）。`apd` 侧第 1~4 处由 6 条新增单测（全仓 7 条全绿）+ 1 条编译期断言锁死，且每条新增断言都用负样本证明过会 FAIL。本节与 `docs/HANDOFF-2026-11-08.md` §6.7/§6.8、`docs/NEW-DOC-01` §3.5.4 互为索引。

## 13. 授权模型（三态 + 日志，2026-11-10，apd-dev 落实）

> 需求来源：人类「增加 root 授权功能页面」。本节定义 **授权表 ABI / CLI 契约 / daemon 判定语义**，供 `ui-client` 直接照做前端；实现落在 `apd/src/auth_list.rs`（存储与判定）、`apd/src/cli.rs`（`auth *`）、`apd/src/daemon.rs`（socket 判定点）。

### 13.1 状态机与默认值

| 状态 | 含义 | 请求行为 |
|---|---|---|
| `allow` | 已授权 | 放行 |
| `deny` | 已拒绝 | 拒绝（静默，只留审计） |
| `ask` | **默认**：还没决定 / 每次询问 | **本迭代拒绝**（`ask_pending`，见 13.4），交互式弹窗是 Phase 2 |

- 受保护 uid `0`（root）、`2000`（adb shell）恒为 allow，且不写表。
- **无行 = `ask`**（绝不隐式 allow）；表不可读 = **deny**（fail closed，与旧内核拒绝的一致可见行为）。
- 迁移：旧 `/data/adb/ap/allow_list`（csv，契约 v1.1 §3.2/E4）在升级后**首次加载时一次性导入**：`allow==1 && exclude==0` → `allow`，其它已持久化行 → `deny`（旧纯白名单语义），`exclude/to_uid/sctx` 原样保留。csv **不再写入**，仅作只读迁移源与审计留档。**升级不丢授权。**

> ⚠️ **Phase 1 行为后果（必须让使用者知道）**：默认态是 `ask`，而 Phase 2 之前 `ask` 等同拒绝 → **新装应用第一次请求 root 会被拒绝**（`decision=ask_pending`），用户必须到授权页把该应用手动设为 `allow`。这是 Phase 1 的既定取舍：**安全但需要一次手动放行**（见契约文档 `phase2_skroot_ap_call_contract_v1.1.md` 附件 A §A.3）。

### 13.2 存储 ABI

- 权威文件：`/data/adb/sproot/auth_list.json`（0600 from creation，tmp+rename 原子，`flock` 单写者）。文档结构：

```json
{
  "version": 1,
  "entries": [
    {
      "uid": 10123,
      "package": "com.example.granted",
      "label": null,
      "app_icon_path": null,
      "state": "allow",
      "last_grant_at": 1762670000,
      "last_deny_at": null,
      "request_count": 3,
      "to_uid": 0,
      "sctx": "",
      "exclude": false
    }
  ]
}
```

- `label` / `app_icon_path` 恒为 `null`：`PackageManager` 只在 App 侧可用，UI 用 `package`（或直接按 uid）自行解析；字段保留保证 schema 稳定。
- 审计日志：`/data/adb/sproot/auth_log.jsonl`（0600，JSONL 追加；超过 256 KiB 自动重写为最新 500 条）。**每行不含完整命令与任何密钥**，只有脱敏摘要：`cmd_name`（可执行名，白名单字符，≤48）/ `cmd_args`（参数个数）/ `cmd_sha256`（SHA-256 前 16 hex，纯 Rust 实现，零新增依赖）。

### 13.3 CLI 契约（全部 JSON，root-only；`apd` 已部署在 `/data/adb/apd`）

| 命令 | 输出 |
|---|---|
| `apd auth list --json` | 裸数组：`AuthEntry[]`（见 13.2，`label/app_icon_path` 为 `null`） |
| `apd auth set --uid <n> --state allow\|deny\|ask --json` | `{"ok":true,"uid":10123,"state":"allow","previous":"ask","package":"com.example","changed":true}` |
| `apd auth revoke --uid <n> --json`（位置参数 `<n>` 兼容 v1.1 契约） | 同 `set --state deny` 的对象；行保留（`previous`/`state` 均可能为 `deny`） |
| `apd auth log --json --limit N`（默认 100） | 裸数组，**最新在前**：`AuthEvent[]` |
| `apd auth grant <uid> [--pkg --to-uid --scontext]` / `exclude` / `unexclude` | 旧 CLI 兼容：`grant` ≡ `set --state allow` 并写回映射字段，静默文本输出 |

`AuthEvent` 样例（daemon 放行一次 `id` 请求；时间戳为 UTC ISO-8601）：

```json
{
  "ts": 1762670000,
  "iso": "2025-11-09T06:33:20Z",
  "uid": 10123,
  "package": "com.example.granted",
  "event": "request",
  "state": null,
  "decision": "allow",
  "previous": null,
  "cmd_name": "id",
  "cmd_args": 0,
  "cmd_sha256": "a56145270ce6b3be",
  "detail": null
}
```

`event` 取值：`request`（每次 root 请求，`decision` ∈ `allow|deny|ask_pending`）、`set_state`、`revoke`、`grant`、`migrate`（一次性迁移摘要）。`state`/`previous` 为 `allow|deny|ask`。

**`package` 字段可靠性与 `null` 情形（给 ui-client）**：daemon 以 root 读 `/data/system/packages.list`（`包名 uid ...`，主用户表）做 uid→包名解析；`auth set`/`revoke`/`grant`、`request` 记录与 `auth list` 都会补全。`package` 为 `null` 仅限以下情形，均**不报错**：

1. **uid 无对应包**：系统 uid（如 0/1000/2000）、已卸载但残留的授权行（`refresh` 会另行清理）、非 App uid（脚本/守护）；
2. **多用户 / 工作资料**：secondary user 的 uid = 基础 uid + `100000 × userId`，不在主用户 `packages.list` 中；
3. **`packages.list` 不可读/撕裂**（升级瞬间）：条目仍在，只是包名解析不到；
4. **sharedUserId**：多个包共用 uid 时只返回首个匹配包名。

UI 兜底（推荐）：拿 `uid` 直接调 `PackageManager.getPackagesForUid(uid)` 反查包名、再取 label/图标——**apd 侧刻意不做 label/图标解析**（无 PackageManager、避免 aapt/xml 解析风险与新增依赖），方案 C。

### 13.4 daemon / socket 判定语义

1. socket 收请求 → `SO_PEERCRED` 取 uid（不可伪造）→ 三态判定；
2. `allow` → fork 会话执行；
3. `deny` → 拒绝（exit 1，静默）；
4. **`ask` → 显式 `ask_pending` 拒绝**（本迭代未实现交互式弹窗；**绝不 fail-open**），并写审计 `decision="ask_pending"`；请求计数/`last_deny_at` 一并更新，UI 可据 `request_count` 提示用户；
5. 每个真实请求**只记一条** `request` 事件（`apd.rs` 的 root-context 回退路径不重复计数）；
6. socket 响应新增**加法字段** `"decision": "allow"|"deny"|"ask_pending"`（老客户端忽略之）；
7. 判定点仍只有一处：`auth_list::authorize()`（`apd.rs` 的 `su` 入口同源）。

### 13.5 断言与负样本（`cargo test`，全仓 15 条）

| 测试 | 钉住 |
|---|---|
`auth_list::tests::migration_keeps_grants_and_denials` | 迁移不丢授权：`allow&&!exclude`→allow；其它已存行→deny；`exclude/to_uid/sctx` 保留；不凭空造 allow/ask
`auth_list::tests::decide_is_three_state_and_never_implicit_allow` | 无行→`AskPending`（负样本：改成 allow 即 FAIL）；表不可读→deny；受保护 uid→allow
`auth_list::tests::auth_state_parse_rejects_unknown` | 非法 state 拒绝（大写/`grant`/空/数字），不静默默认
`auth_list::tests::command_digest_is_redacted` | 日志脱敏：只留 `cmd_name`+参数数+16 hex 摘要，命令与 `key` 字样不出现
`auth_list::tests::entry_serialisation_roundtrip` | `state` 为小写字符串、可空字段显式 `null`、round-trip
`auth_list::tests::iso8601_matches_known_epochs` | UTC ISO-8601 无日期依赖
`utils::tests::sha256_matches_known_vectors` | 纯 Rust SHA-256 对照 FIPS 向量

### 13.6 与契约 v1.1 的偏差（勘误 E6，已按「不改冻结正文」追加）

契约 v1.1 §3.2/E4 规定授权表为单文件 csv（`allow_list`）。三态升级后**决策源改为 JSON**（csv 仅迁移源）。正式勘误记录于契约文档 **`docs/phase2_skroot_ap_call_contract_v1.1.md` 附件 A（errata E6，追加于正文之后，v1.1-FROZEN 正文 1–333 行未改动）**，本节与之互相引用。影响：`package.rs` 的 csv 读写/同步路径标记为 legacy（保留不调用）；`selftest` 的 `allow_list` 检查名不变，detail 改为 JSON 计数（`allow=/deny=/ask=` + `legacy_migrated`）。其余安全语义（单写者、原子写、uid 保护、torn-read 熔断）逐条保留。

### 13.7 待办

1. **交互式 `ask` 弹窗（Phase 2）**：需要 App 常驻监听 + daemon↔App 双向事件（daemon 现只有请求侧 socket）。本轮明确以 `ask_pending` 拒绝收尾，已记入本节而非静默放行。
2. `ui-client`：按 13.3 渲染三态列表/日志/搜索；`label`/`app_icon_path` 由 UI 用 `PackageManager` 解析（本侧恒 `null`）。

## 14. APatch / Zygisk 兼容层（外部生态信号，2026-11-10，apd-dev 落实）

> 背景：NeoZygisk（GPL-3.0）与 OnyxZygisk（AGPL-3.0）对接 APatch 只有两条路径（`zygiskd/src/root_impl/apatch.rs`）：① `apd -V` 的第二 token 解析为 `i32` 且 ≥ `minAPatchVersion = 10762`；② 读 `/data/adb/ap/package_config` 的 csv，`exclude==1` 即「排除修改（umount）」。SPRoot 因此**主动伪装成 APatch 血统**，以被现有 Zygisk 生态识别——**这是兼容层，不是 bug**（人类 2026-11-10 已同意方向；代码注释同步说明，避免后人"修掉"它）。真实存储仍是 `auth_list.json`（§13），csv 只是派生信号。

### 14.1 `apd -V`（生态识别信号）

| 项 | 值 |
|---|---|
| 输出 | **`apd 10762`**（首行，clap `<name> <version>` 格式） |
| 常量 | `defs::APATCH_COMPAT_VERSION = 10762` / `APATCH_COMPAT_VERSION_STR = "10762"` |
| 解析契约 | `-V` / `--version` 输出 `split_whitespace().nth(1).parse::<i32>()` 必须成功且 `>= 10762`（低于即被判定 `TooOld`） |
| 真实版本 | 走 **`apd version` / `apd version --json`**（`VERSION_NAME`/`VERSION_CODE`，由 `build.rs` 从 git 生成），不要期望 `-V` 反映它 |

> ⚠️ 修改 `APATCH_COMPAT_VERSION` 必须同时评估上游 `minAPatchVersion`；把 `-V` 改回真实语义版本号会让所有 Zygisk 实现判定「不是 APatch / 太旧」。

### 14.2 派生 `/data/adb/ap/package_config`

- **权威源**：`/data/adb/sproot/auth_list.json`（§13）。`package_config` 是**派生产物**，`apd` 唯一写者（App/ui-client 不直写）。
- **格式**（位置列，外部解析器依赖 `parts[1/2/3]`，逐字节严格）：

```csv
pkg,exclude,allow,uid,to_uid,sctx
com.example.granted,0,0,10123,0,
com.example.excluded,1,0,10125,0,
```

| 列 | 取值 |
|---|---|
| `pkg` | 包名（解析不到时为空串；外部按 uid 匹配，空串合法） |
| `exclude` | `1` = 排除修改（Zygisk umount 信号），`0` = 不排除 |
| `allow` | `state == allow` → `1`；`deny`/`ask` → `0` |
| `uid` / `to_uid` / `sctx` | 原样（`to_uid` 缺省 0，`sctx` 可为空） |

- **写入时机**：每次 auth 提交（`save()` 内，含迁移/请求计数/状态变更）+ `prepare_storage()`（deploy step 8、daemon 启动时全量同步，覆盖「从旧版升级、JSON 已有但 csv 尚未生成」的路径）+ `refresh_allow_list()`（uid 刷新/裁剪后）。
- **原子性/权限**：tmp+rename、**0644**（允许非 root 的 Zygisk helper 读取；只含包名/uid，**不含密钥、不含命令**；可写性为 0）。目录 `/data/adb/ap/` 缺失则创建。
- **一致性**：与 `auth_list.json` 同一次写事务内更新（先权威后派生，两者均 tmp+rename），因此 `exclude` 与 `auth list --json` 的 `exclude` 字段恒一致（单测 `package_config_matches_auth_table` 双向校验）。

### 14.3 `auth set --exclude`（App 开关）

| 命令 | 效果 |
|---|---|
| `apd auth set --uid 10123 --exclude true --json` | 只改 exclude（state 不动） |
| `apd auth set --uid 10123 --state allow --json` | 只改 state（现状） |
| `apd auth set --uid 10123 --state allow --exclude false --json` | 两者一次原子写 |
| `apd auth set --uid 10123` | **报错**（`--state` 与 `--exclude` 至少给一个） |

JSON 返回新增 `exclude` / `previous_exclude`（加法字段）：

```json
{
  "ok": true,
  "uid": 10123,
  "state": "allow",
  "previous": "ask",
  "exclude": true,
  "previous_exclude": false,
  "package": "com.example.granted",
  "changed": true
}
```

裸 `--exclude` 等价 `--exclude true`；`--exclude false` 清除。审计事件仍是单条 `set_state`（`detail` 记 `exclude=<bool>`）。

### 14.4 `selftest --json` 的 `zygisk` 检查（Phase 1 占位）

新增 check `zygisk`（`critical=false`，不阻断）：

- `zygiskd` 是否在运行（扫描 `/proc/*/cmdline` 中 basename 含 `zygiskd` 的进程）；
- 内置组件版本（读 `/data/adb/modules/*zygisk*/module.prop` 的 `version=`；**本轮 SPRoot 尚未内置 Zygisk 组件 → 通常 `version=-`/installed=false**）；
- `/data/adb/ap/package_config` 是否存在、权限是否正确（0644 可读不可写）。

detail 示例：`apatch_compat_version=10762 zygiskd_pid=None running=false module=- version=- package_config=/data/adb/ap/package_config exists=true mode=644 readable=true (Phase 1 placeholder: built-in Zygisk packaging pending)`。真正的 Zygisk 组件内置方案待 Lead 决定后再接，本轮只保证「未安装/未运行」可见。

### 14.5 断言（`cargo test`，全仓 21 条）

| 测试 | 钉住 |
|---|---|
| `tests/version_flag.rs::version_flag_is_apatch_compatible` | **真实二进制**执行 `-V`，按 NeoZygisk 的 `split_whitespace().nth(1).parse::<i32>()` 解析且 ≥ 10762、首行为 `apd <int>` |
| `auth_list::tests::package_config_matches_auth_table` | 派生 csv 与 `auth_list.json` 双向一致（header 逐字节、列位置、allow/exclude 映射） |
| `auth_list::tests::package_config_rejects_malformed_rows_externally` | 外部解析器负样本：少列/列错位/`exclude` 非 0-1/uid 非数字 → 拒绝；含逗号包名不破坏列 |
| `auth_list::tests::require_any_change_rejects_empty_request` | `--state`/`--exclude` 至少给一个 |
| `auth_list::tests::change_serialises_exclude` | JSON 暴露 `exclude` / `previous_exclude` |
| `selftest::tests::module_prop_version_parsing` | 组件版本解析（无版本→None） |

### 14.6 风险与待办

1. **内置 Zygisk 组件方案**（zygiskd 二进制、`module.prop`、启动方式）由 Lead 后续决定；本轮只做信号源 + selftest 占位。
2. `package_config` 为 0644：任何 App 可读「已授权/排除的应用包名列表」。这是与生态兼容的**显式取舍**（APatch 生态自身也依赖该文件），不含密钥与命令；若后续收紧，需先确认 Zygisk helper 的 uid。
3. 上游若提高 `minAPatchVersion`，只改 `defs::APATCH_COMPAT_VERSION` 一处即可（常量 + `-V` 输出 + selftest detail 共用）。

## 附录 A · 证据索引（本文所有结论的源码/文档出处）

| 结论 | 出处 |
|---|---|
| 提权=execve(47 字符)，成功看 getuid()==0 | `engine/src/patch_do_execve.cpp:76-98,113-135`；`engine/src/skroot_patch_main.cpp:312-317`；`docs/SPROOT-DOC-A1` §4.1/§4.3 |
| **密钥 ABI 全链（AP 文件 47 / 内核 47+NUL / 引擎 48→47+NUL / execve 必须 47）** | **§12**；`apd/src/deploy.rs::{abi_key,key_material}`；`apd/src/secret.rs::read_key_file`；`apd/src/supercall.rs::escalation_token`；`engine/src/skroot_patch_main.cpp:61-64,105-107,434-447,618-623` |
| **授权三态 + 日志（ABI/CLI/判定）** | **§13**；`apd/src/auth_list.rs`（`authorize/decide/set_auth/log_events/migrate_legacy`）；`apd/src/cli.rs::AuthCmd`；`apd/src/daemon.rs::handle_client`；`apd/src/defs.rs::{AUTH_LIST_FILE,AUTH_LOG_FILE}` |
| **APatch/Zygisk 兼容层（`-V`=10762 + 派生 package_config + `--exclude` + `zygisk` selftest）** | **§14**；`apd/src/defs.rs::{APATCH_COMPAT_VERSION,APATCH_PACKAGE_CONFIG*}`；`apd/src/cli.rs`（version/clap `-V`、`auth set --exclude`）；`apd/src/auth_list.rs::package_config_csv`；`apd/src/selftest.rs::{zygiskd_pid,zygisk_module_prop}`；`apd/tests/version_flag.rs` |
| 隐藏目录名=key[0:16]、filldir 双路隐藏 | `engine/src/patch_filldir64.cpp:52-62`；A1 §4.2/§4.4 |
| AP 侧提权实现已就位 | `app/apd/src/supercall.rs:42-83` |
| 授权表用户态化（flock/单写者/tmp+rename/保护 uid） | `app/apd/src/auth_list.rs:27-149`；`app/apd/src/package.rs:13-21,127-185,241-330` |
| 三阶段编排现状 | `app/apd/src/event.rs:44-257,283-350` |
| 脚本环境/超时缺口/进程组 | `app/apd/src/module.rs:99-120,205-252`（TODO: timeout 在 `event.rs:198`） |
| 安装流程与 metamodule | `app/apd/src/module.rs:363-506`；`metamodule.rs` 全文 |
| 符号链接硬编码遗留 | `app/apd/src/assets.rs:10-21`；`app/apd/src/defs.rs:10-13` |
| `SP_REQUEST_UID` 现实现 | `app/apd/src/apd.rs:196-205`（待删） |
| 已签契约口径（密钥/授权表/su/安全模式） | `docs/phase2_skroot_ap_call_contract_draft.md` §2.3/§3.1/§3.2/§3.4/§3.5/§4 |
| rc 注入落点与 SELinux 推导 | `docs/phase2_boot_sequence_analysis_draft.md` §3.4/§3.5/§4 |
| 引擎 I/O 与错误码/密钥交付 | `docs/SPROOT-DOC-C1` §1/§2/§3 |
| APatch 模块布局/子命令全集 | `docs/phase1_ap_apd_analysis_and_sk_adaptation.md` §1.1/§2.1/§3.4 |
| 工具链实测（无 NDK / 无 android target） | 本机 `~/.cargo`、`~/.rustup/toolchains` 检查（2026-11-08） |

## 附录 B · 与已签文档一致性对照

| 已签文档 | 条款 | 本方案 | 状态 |
|---|---|---|---|
| 契约 v1.0 | §2.3 密钥方案 B、日志脱敏 | 一致（§4.1/§4.2/§4.5） | ✅ |
| 契约 v1.0 | §3.1 提权 execve 判据 | 一致（判据）；**提权串文字勘误 E1** | ⚠️ E1 |
| 契约 v1.0 | §3.2 授权表用户态 + 单写者 | 一致（§4.4）；`exclude_list` 合表偏差 E4 | ⚠️ E4 |
| 契约 v1.0 | §3.3 读=AP 自己文件 | 一致 | ✅ |
| 契约 v1.0 | §3.4 su 部署/argv0/kp 删除 | 机制一致；**随机命名偏差 E2**；**SP_REQUEST_UID 相关部分作废 E3** | ⚠️ E2/E3 |
| 契约 v1.0 | §3.5 安全模式三路 | 一致 + 自动 safemode 增强（§4.5/§5.5） | ✅（增强） |
| 契约 v1.0 | §4 方案 B 终选 | 一致 | ✅ |
| 契约 v1.0 | §5 busybox 首装归属 | 一致（deploy 清单第 3 步） | ✅ |
| 时序稿 v0.5 | §3.4 rc 落点/import 追加 | 一致（§5.1） | ✅ |
| 时序稿 v0.5 | §3.5 SELinux（init 域起跑 + avc_denied） | 一致 | ✅ |
| 时序稿 v0.5 | §4 部署器清单（skroot-deploy） | **职责并入管理器 + AP**（E5） | ⚠️ E5 |
| C1 v1.0 | §5.1/5.2/5.3 提权形态/su_path/安全模式 | 一致（并修正 E1/E3） | ✅（含勘误） |
| A1 v1.0 | §8 十项特性、§4.4 su 部署 | 一致（随机命名对齐 A1 §4.4 更严） | ✅ |

---

*版本：v1.2（2026-11-08 夜间自主令，sys-guard）。v1.1 → v1.2：① `module list --json` / `selftest --json` schema 定稿（与 app-client 对齐）；② 新增 §11 发布门禁（release.yml）；③ P2 收尾记录（AGPL/许可清单/实测体积）。历史：v1.1（2026-11-08，sys-guard）。v1.0 草案 → v1.1：① 契约 v1.1 已三方会签并**冻结（2026-11-08 21:23 +08，TBD-10 关闭）**；② 补入 §2.2「修补链 ≠ 特权链」边界铁律；③ §7 backlog P0 进入实施（交叉编译链依赖 CI）。下一交付：P0 代码落地 + host `cargo check` 通过记录。*
*版本补充：v1.3（2026-11-09，apd-dev）：新增 §12「密钥 ABI 全链一致性」（五处口径静态核对 + 6 条新断言，逐条以负样本证伪）；`supercall::escalation_token()` 改为返回 `Result`（<47 / >48 / 非 ASCII 硬报错，不再静默透传；48 归一到 47 并告警）。*
*版本补充：v1.4（2026-11-10，apd-dev）：新增 §13「授权模型（三态 + 日志）」——`auth_list.json` 三态存储（`allow|deny|ask`，默认 ask）+ 一次性 csv 迁移（升级不丢授权）+ `auth list/set/log/revoke --json` CLI ABI + daemon `ask_pending` 显式拒绝 + 脱敏审计日志；勘误 **E6**（决策源 csv → JSON，待会签）；§4.4 标注 csv 已降级为迁移源。*
*版本补充：v1.5（2026-11-10，apd-dev）：新增 §14「APatch / Zygisk 兼容层」——`apd -V` 输出 `apd 10762`（NeoZygisk `minAPatchVersion`）、派生写出 `/data/adb/ap/package_config`（0644、位置列、单写者 apd、与 §13 同事务一致）、`auth set --exclude`（含 `--state` 组合与 at-least-one 校验）、`selftest` 新增非 critical 的 `zygisk` 占位检查；新增真实二进制集成测试 `tests/version_flag.rs`。*
