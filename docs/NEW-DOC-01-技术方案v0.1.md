# 全新 SP 管理器技术方案 v0.1（NEW-DOC-01）

- 文档编号：NEW-DOC-01
- 版本：v0.4（草案，可评审；已并入 RC 根因报告 + MI(miuix 产物级实证) + KSU/APatch 管理器架构调研 + DOC-05 模块守护方案 + **契约 v1.1（2026-11-08 21:23 FROZEN，三方签 + leader 终审）** + lead 决议：JNI 引擎/两链分离/自研 bootimg/AVB 刷入路径/CI 唯一构建源）
- 作者：架构（architect）
- 状态：**草案（可评审）｜待 lead 终审后升 v1.0**
- 项目重大转向：**放弃「改造 APatch 管理器 APK」路线，从零实现全新 SP 管理器；内核修补全程在安卓手机本地完成（不经 PC/云端）**
- 参考对象：KernelSU(KSU) 与 APatch(AP) 管理器的**架构形态与修补方式**（不复制其内核侧机制）
- UI 库指定：<https://github.com/compose-miuix-ui/miuix>
- 关联文档：
  - `docs/SPROOT-DOC-A1-SKRoot机制深度分析.md`（SK 修补原理，下文简称 **A1**）
  - `docs/SPROOT-DOC-C1-修补引擎IO契约.md`（引擎 CLI 契约，简称 **C1**）
  - `docs/AP管理器与镜像修补链分析.md`（旧客户端与修补链，简称 **APL**）
  - `docs/phase2_skroot_ap_call_contract_draft.md`（SK↔AP 调用契约 v0.2，简称 **CALL**）
  - `docs/phase2_boot_sequence_analysis_draft.md`（启动时序联合稿，简称 **BOOT**）
  - **已并入**：`docs/research/patch-failure-rootcause.md`（旧链失败根因，简称 **RC**）→ §5.2/§5.7/§6.2/§7
  - **已并入**：`docs/research/miuix-integration.md`（miuix 产物级实证，简称 **MI**）→ §4 全部重写
  - **已并入**：`docs/NEW-DOC-05-模块与守护方案.md`（模块/守护/契约 v1.1 勘误，简称 **DOC-05**）→ §3.5/§5.3/§5.4/§6.4
  - **已并入**：`docs/research/kernelsu-architecture.md`（KSU 架构实测，简称 **KSU**）→ §1.3/§6.3
  - **已并入**：`docs/research/apatch-manager-architecture.md`（APatch 管理器架构实测，简称 **APM**）→ §1.3/§3.6/§6.3
  - **已并入**：契约 v1.1 **FROZEN**（2026-11-08 21:23，三方签 + leader 终审；E1–E5 + 两链分离 + 两 native 资产边界）→ §3.1.1/§3.5/§5.2/§6.4
  - **已并入**：人类指令「禁止本地编译 APK/镜像，构建统一走 CI」→ §8

> **证据约定**：本文所有技术断言均给出「本地文件路径 + 行号」或「URL + 版本」两类证据之一。凡未取得证据者，一律写入 §8 待决项，不作为设计前提。文末附录 A 汇总。

---

## 0. 摘要与结论（先看这 12 条）

| # | 结论 | 依据 |
|---|---|---|
| 1 | **四层架构**成立：UI 层（miuix/Compose）→ 修补执行层（Kotlin 编排 + 引擎双形态调用）→ Root 授权层（47 字符密钥 execve 提权 + 用户态 allow_list + **root-exec 为唯一 root 入口**）→ 模块管理层（apd 守护改造）。层间只用三类接口：引擎 CLI（C1）、APK↔AP 接口（§3.5，含 `root-exec`/`deploy`/`auth`）、apd 子命令。**组件总数比旧方案少 1 个：不再有独立部署器**（E5）。 | §3、§3.5 |
| 2 | **miuix 可用且已实证到 Maven 产物级**：坐标 `top.yukonga.miuix.kmp:miuix-ui-android:0.9.4`（仅需 `mavenCentral()`），AAR 元数据 `minCompileSdk=37` / 库 `minSdk=24`；现有 BOM `2026.08.00`（foundation 1.12.0）与 miuix 要求**天然对齐**。 | §4.1、§4.7（E1–E6） |
| 3 | **工程形态：Android 单平台**（非 KMP），动作项只有两个：① `exclude("org.jetbrains.compose.material3:material3-window-size-class")`（否则 material3 在 runtime 被抬到 `1.5.0-alpha22`，编译 1.4.0/运行 alpha）；② Kotlin 升 `2.4.20`。**能力边界**：blur 需 minSdk 33、glass 在 0.9.4 未发布（404）。 | §4.2、§4.6（U1–U3）、E3/E5/E7/E8/E9/E10 |
| 4 | **引擎交付 = JNI 进程内（默认）**：`libskrootpatch.so` 以 JNI 加载进 App 进程执行修补（同库保留 CLI `main()` 作诊断/PC 复用）。**修补链不需要 root**。子进程 exec 形态降为诊断/回退模式。 | §5.2、§5.2.1、A-27 |
| 5 | **修补链与特权链两条独立通道**（lead 决议，契约 v1.1）：**修补链**＝App 进程内（选镜像→解包→JNI 引擎→rc 注入→重打包→导出/写分区），权限=普通 App；**特权链**＝`apd root-exec`（deploy/模块/授权/刷入）→ AP 自提权。**两链不合并、不互代**（§3.1）。 | §3.1、§3.5、§3.6 |
| 6 | **两个 native 资产、职责不可互换**：`libskrootpatch.so`（JNI，修补，无 root）vs `libapd.so`→`/data/adb/apd`（守护/特权，root）。 | §5.2 资产表、§3.5.2 |
| 7 | **bootimg 解包/重打包 = 引擎自研**（lead 已裁，替代 magiskboot/kptools）：无第三方 ELF、无 GPL-3.0 传染、无「缺件即失败」。SK 只 patch kernel 段；rc 注入是否做取决于 D-14。 | §5.3、§5.4、A-8、A-23、A-30 |
| 8 | **旧路线失败根因已确证（RC）**：`boot_patch.sh` 调 `./skroot-patch`，但该文件**全仓 0 命中、从未被打包**（`lib/arm64-v8a/` 只有 `libbootctl.so`/`libbusybox.so`；jniLibs 目录不存在；`engine/src/skroot-patch` 是 **x86-64 glibc** ELF，`e_machine=0x3e`）；脚本缺失即 `exit 1`；同时 `Patches.kt:261` 的瞬态条件让失败瞬间卸载日志面板、真实 stderr 被注释 → 用户只见「一闪即退」。新方案落实 **二进制资产清单 + 构建期断言 + 运行期自检 + 日志落盘** 四件套。 | §5.7、RC §一/§二、A-10~A-13 |
| 9 | **失败回滚做三级**：①引擎级（非 0 退出绝不产出 `--out`，C1 §3 铁律）；②编排级（staging 工作目录 + manifest 校验，绝不原地覆盖原厂镜像）；③刷机级（内容寻址原厂备份 + A/B 备槽切换 + 开机成功才 `mark-boot-successful`）。 | §5.6、A-14 |
| 10 | **提权/授权/部署按契约 v1.1 五处勘误口径落定（E1–E5）**：提权串=**47 字符密钥本体**（非路径）；su 部署=隐藏目录内**随机 16 字符名**+`/data/adb/ap/bin/su` 符号链接；废弃 `SP_REQUEST_UID`，第三方身份改 **抽象 socket + `SO_PEERCRED`**；`exclude_list` 并入单一 `allow_list`；**取消独立部署器**，部署职责拆分给「管理器（修补期写 rc）+ `apd deploy`（首启 T2）」。 | §3.5、§5.4、§6.4（E1–E5 差异表）、A-23 |
| 11 | **root 入口唯一化**：管理器**所有**需要 root 的动作（部署、模块、授权、刷入镜像）统一走 `apd root-exec --superkey-file <tmp> -- <cmd>`；管理器自身不实现提权逻辑、不长期持有 root shell。 | §3.5.2 |
| 12 | **最大未知不在 UI、在分区与 AVB**：Android 13+ `init_boot` 与 `boot` 的分区拆分、kernel 压缩格式与重打包尺寸偏移、rc 注入是否做（D-14）、**AVB footer + vbmeta 刷入路径与「无 PC」目标张力（D-15）**、引擎 arm64/JNI 交叉编译（task-3）。 | §7、§9 |

---

## 1. 目标、范围与非目标

### 1.1 目标

1. 交付一个**全新**的 SP 管理器 App：界面用 miuix，核心能力是**在手机本地**把用户选定的 `boot.img` 修成 SK 修补镜像并导出/刷入，同时给出 48 位 Root 密钥。
2. 修补成功率可解释：每一步的输入、输出、失败原因**必须完整可见**（针对旧路线"终端输出一闪即报错"的直接反制）。
3. Root 授权与模块管理沿用已签契约（CALL/BOOT），不重新发明内核接口。

### 1.2 非目标（v0.1）

- **不做 PC/云端修补**：无桌面端、无在线修补服务、无远程密钥协商。
- **不改 SK 内核机制**：五组 hook、48 位密钥、SELinux 零触碰（A1 §8）原样继承，管理器只做「调用方」。
- **不实现内核级授权表**：按 CALL §3.2 裁决，授权表是 AP 用户态文件，管理器只做 UI 展示与审计。
- **不做 KPM 内核模块运行时**：SK 无 KP 内核运行时，KPM 能力无载体（A1 §1）；模块页只保留 APM（apd 模块）语义。
- **不覆盖 32 位设备**：SK 引擎与 AP 链均只 arm64（APL §2.2 层 2 提到 32 位 App 明确不支持）。

### 1.3 与 KSU / APatch 对照：借鉴 / 必须改造 / 禁止（三类三分）

> 证据：KSU 调研（`docs/research/kernelsu-architecture.md`，基线 `730f6446`）+ APatch 管理器调研（`docs/research/apatch-manager-architecture.md`，基线 `e8e4651c`）。本节是**架构级结论**，细节见 §6.3/§6.4。

#### A. 直接借鉴（照抄结构）

| # | 借鉴项 | 来源 | SPRoot 落位 |
|---|---|---|---|
| K1 | **薄 UI + 厚 CLI**：管理器只做界面/状态机/参数拼装/日志渲染，特权逻辑全在执行者进程 | KSU §0-1、§5.1-① | §3.1.1 特权链；`libapd.so`→`/data/adb/apd` |
| K2 | **修补全参数化单命令、可复现**（无交互、无 shell 拼串） | KSU §5.1-② | §5.4 八步 + C1 CLI 参数面 |
| K3 | **纯自研 bootimg 解包/回包**（不用 magiskboot/kptools） | KSU §2.7、§5.1-③ | **lead 已裁：引擎自研**（§5.3） |
| K4 | **内容寻址备份**（sha1 索引 + 引导内指纹） | KSU §5.1-④ | §5.6-③ |
| K5 | **UiState + 状态机 + 页面能力门禁** | KSU §5.1-⑤；APM §1.3、§5.1-④ | §3.6（10 态改造） |
| A1 | **双轨迁移**：新机制优先探测 → 成功即清旧凭证 → 不可用则旧凭证兜底 | APM §5.1-① | §3.5.3 部署向导（先 `version --json` 探测 SK/AP 状态；旧 SuperKey 路径**不保留**，见 C1） |
| A2 | **PatchMode 四模式**（只出包 / 直装 / 装下一槽 / 还原）+ 每模式独立收尾 | APM §2.2、§5.1-② | §5.4 交付模式 + §3.6 |
| A3 | **`ori.img` 备份闭环**（仅未修补时备份、备份搬运、还原流程） | APM §5.1-③ | §5.6-③（升级为 sha1 索引） |
| A4 | **APK 兼作卡刷包**（`META-INF` + update-binary + 复用 lib） | APM §5.1 加分项 | §8.4（CI 产物附加 recovery zip，可选） |

#### B. 必须改造（不能照抄）

| # | 上游做法 | 为什么必须改 | SPRoot 做法 |
|---|---|---|---|
| B1 | SuperKey 作为 **argv** 传给 `/system/bin/truncate`（APM §5.2-①） | 密钥进 `/proc/<pid>/cmdline`/logcat；依赖具体系统二进制；与去 SuperKey 目标冲突 | **`root-exec --superkey-file <tmp 0600>`**，密钥不进 argv；用完即删（§3.5.2） |
| B2 | 预编译二进制 + shell 脚本当引擎（APM §5.2-②） | 不可审计、文本判定、无结构化错误码、构建期下载 | **自研 bootimg + JNI 引擎 + C1 退出码/NDJSON**（§5.2/§5.3） |
| B3 | ProcessBuilder 与 libsu **双路径 + 字符串拼命令**（APM §5.2-③） | 参数转义 bug、语义不一致、排查翻倍 | **单一路径**：JNI（修补）/`root-exec`（特权），不拼 shell 串 |
| B4 | 无交互预授权（KSU §5.2-①） | SK 零常驻无法做 pending 队列 | 接受限制 + 「待授权应用」引导页补偿（§6.3 注） |
| B5 | 把 `nativeLibraryDir` 当**守护进程**固定路径（KSU §5.2-②） | APK 升级即失效 | 只有一次性工具/引擎走该路径；**守护固定 `/data/adb/apd`**（§6.2-M14） |

#### C. 明确禁止（写入纪律）

| # | 禁止项 | 理由/替代 |
|---|---|---|
| X1 | 禁止 superkey/密钥经 argv 或 `truncate` 等通用二进制传递 | 用 `--superkey-file`（临时 0600）+ AP 内部 `execve(key47)` |
| X2 | 禁止「预编译二进制 + shell 脚本」作为修补引擎 | 用 JNI `libskrootpatch.so` + 结构化退出码 |
| X3 | 禁止把 nativeLibraryDir 当守护进程长期路径 | 守护固定 `/data/adb/apd`，管理器建模 bundled/installed/stale |
| X4 | 禁止运行期/构建期网络下载二进制当交付物 | 入库 + CI 构建 + sha256 校验（§5.7-L1/L2） |
| X5 | **禁止本地编译 APK / 镜像**（人类硬规则） | 唯一可信构建源 = GitHub Actions（§8） |
| X6 | 禁止「脚本打印 Successfully」当成功判据 | 只认可验证事实（等长/魔数/回读 sha256，§5.4 步 6/8） |

---

## 2. 现状基线与资产清点

### 2.1 已有资产（本次直接复用，不重造）

| 资产 | 路径 | 状态 | 本方案用法 |
|---|---|---|---|
| SK 修补引擎源码 | `engine/src/`（`skroot_patch_main.cpp`、`patch_pipeline.*`、`analyze/*`、五组 `patch_*.cpp`） | 已在 WSL 编译出 x86-64 产物 `engine/src/skroot-patch`（2,121,080 B，2026-11-06） | task-3 交叉编译为 arm64 后随 APK 分发 |
| 引擎 I/O 契约 | `docs/SPROOT-DOC-C1-修补引擎IO契约.md` | v1.0 草案（12 个退出码、NDJSON 事件、report schema） | 编排层唯一调用面，不再自定义协议 |
| SK 机制分析 | `docs/SPROOT-DOC-A1-*.md` | v1.0 正式稿 | 决定 hack 点、稳定性要求、验收断言 |
| 旧修补链 | `app/app/src/main/assets/boot_patch.sh` + `PatchesViewModel.kt` | 已含 SPRoot 改造注记（P1/P2 标记） | 保留其「模式/备份/槽位」思路，废弃其脚本主链 |
| bootimg 工具源码 | `upstream/KernelPatch/tools/bootimg.{c,h}` | GPL-2.0，含 `repack_bootimg()` | 解包/重打包主选 |
| SK↔AP 契约 | `docs/phase2_skroot_ap_call_contract_draft.md` | v1.0 定稿（双签） | Root 授权层接口基线 |
| 启动时序 | `docs/phase2_boot_sequence_analysis_draft.md` | v0.5 联合稿 | 首装顺序与 rc 注入边界 |

### 2.2 明确**不再使用**的旧路线要素

| 旧要素 | 处置 | 原因 |
|---|---|---|
| APatch 管理器 UI（Material3 + composedestinations + 24 主题） | 全部废弃，UI 重写为 miuix | 人类指定 UI 库；且旧 UI 的日志/错误展示模型正是"一闪即退"帮凶 |
| `boot_patch.sh` 作为主链 | 降级为**对照参考**，新链由 Kotlin 编排 + 直接进程调用 | 脚本层会吞错误、`set -x` 输出进日志却无人滚动查看 |
| KPM 页面/`-M/-E` 嵌入参数 | 删除（非目标） | SK 无内核运行时载体 |
| 构建期 `registerDownloadTask` 下载二进制 | 改为**入库 + CI 构建 + sha256 校验** | 见 §5.7 根因分析 |

---

## 3. 总体架构

### 3.1 四层模型

```
┌──────────────────────────────────────────────────────────────────────┐
│ L1 UI 层（miuix / Compose Multiplatform, android only）              │
│  HomeStatus · PatchFlow · SuperUser · Modules · Settings · LogViewer │
│  只依赖 L2 暴露的 StateFlow/UiState，不碰文件、不碰进程              │
└───────────────▲──────────────────────────────────────────────────────┘
                │ UiState / Command（单向数据流）
┌───────────────┴──────────────────────────────────────────────────────┐
│ L2 修补执行层（Kotlin, :patch 模块）                                 │
│  BootImageRepository  → 引擎 bootimg 模块（JNI）：解包/回包/AVB + 分区探测 │
│  RcInjector           → 写 init.sproot.rc + init.rc 追加 import（cpio）│
│  PatchOrchestrator    → 八步状态机 + 进度聚合 + 失败恢复             │
│  EngineRunner         → 双形态（子进程 / JNI）+ NDJSON 解析 + 退出码映射│
│  ArtifactVerifier     → 等长/sha256/密钥内嵌/hook_table 断言          │
│  WorkdirManager       → staging 目录 + manifest + 清理策略            │
└───────────────▲──────────────────────────────────────────────────────┘
                │ 进程边界（唯一）：argv + NDJSON + exit code
┌───────────────┴──────────────────────────────────────────────────────┐
│ L3 Root 授权层（Kotlin 接口；不自带提权实现）                        │
│  SukeyProvider（生成/Keystore 加密存/临时 0600 文件交付）             │
│  RootExecClient（→ apd root-exec，**管理器唯一 root 入口**，§3.5.2）  │
│  AllowListRepository（UI/审计；真值在 /data/adb/ap/allow_list）       │
│  DeploymentClient（首启 T2 调 apd deploy，替代独立部署器 E5）         │
└───────────────▲──────────────────────────────────────────────────────┘
                │ APK↔AP 接口（§3.5）：root-exec / deploy / auth / version
┌───────────────┴──────────────────────────────────────────────────────┐
│ L4 模块管理层（AP 二进制 = 改造后的 apd，Rust）  ← task-6 边界        │
│  /data/adb/apd（CLI + daemon + multicall）· /data/adb/ap/            │
│  三阶段编排（post-fs-data / services / boot-completed）               │
│  授权 socket（抽象名 @sproot-<12hex> + SO_PEERCRED）                  │
└──────────────────────────────────────────────────────────────────────┘

            ✗ 不再存在独立「部署器 / skroot-deploy」进程（E5）
```

#### 3.1.1 两条通道（lead 决议：**修补链 ≠ 特权链**，不得合并）

```
【通道 ① 修补链】  权限：普通 App（无 root）         【通道 ② 特权链】  权限：root（AP 内自提权）
┌────────────────────────────────────┐            ┌────────────────────────────────────────┐
│ App 进程（UI + Kotlin 编排）        │            │ App 进程（只做参数拼装 + 流式渲染）     │
│  ① 选 boot.img（SAF）               │            │  ① apd version --json（探测）           │
│  ② libskrootpatch.so（JNI，进程内） │            │  ② apd deploy（T2 首启）                │
│  ③ 无第三方工具（解包/回包/rc 注入全在 JNI 引擎内）│         │  ③ apd root-exec -- <cmd>               │
│  ④ rc 注入（cpio）                  │            └───────────────┬────────────────────────┘
│  ⑤ repack → new-boot.img            │                            │ exec（普通进程启动）
│  ⑥ 导出（SAF）/ 写分区（经②通道）   │                            ▼
└────────────────────────────────────┘            ┌────────────────────────────────────────┐
        │ 直接调用，不经过 AP                       │ AP 二进制 /data/adb/apd（root）         │
        ▼                                          │  内部 execve(key[47]) → getuid()==0     │
   filesDir/patch/（私有目录）                     │  deploy / module / auth / root-exec      │
                                                   │  daemon / su 会话 / socket(SO_PEERCRED) │
                                                   └────────────────────────────────────────┘
```

**通道铁律**

1. **修补链全程不经 AP、不需 root**：引擎是 JNI 库、工具是普通子进程、产物写 App 私有目录。**这是「手机本地修补」能对未 root 用户生效的全部原因**（T1 阶段）。
2. **特权链只经 `apd root-exec`**：管理器不自行 `execve(key)`、不持有 root shell（§3.5.2）。
3. **两链唯一的交点**是「步 8 刷入」：修补链产出镜像后，若用户选择直刷，则把 `dd` 命令交给通道②执行——**镜像文件路径由修补链给出，执行权由特权链持有**。
4. **资产不可互换**：JNI 引擎绝不能承担特权行为；AP 二进制绝不能承担内核 patch（§5.2 资产表）。

**分层铁律**

1. UI 不直接 `Runtime.exec`，一切副作用经 L2 的 `UseCase`；保证「重进页面不丢进度、失败可复现」。
2. L2 与引擎之间**只有进程边界**（argv / NDJSON / 退出码），不共享内存、不 `dlopen`——引擎段错误不会杀 UI（§5.2 决策依据）。
3. L3 **不实现提权算法**：管理器只负责把「密钥 → 临时 0600 文件」并调用 `apd root-exec`；真正的 `execve(key47)` 发生在 AP 进程内（DOC-05 §4.1）。判据永远只有 `getuid()==0`。
4. L4 由 AP 独占 `/data/adb/` 写入；管理器只经 AP 子命令改状态（避免双写者）。初始化、密钥落盘、隐藏 su 部署全部在 `apd deploy`（T2）内完成。

### 3.2 组件职责表

| 组件 | 层 | 职责 | 输入 | 输出 | 失败面 |
|---|---|---|---|---|---|
| `HomeViewModel` | L1 | 读内核状态、修补产物状态、AP 状态 | 状态仓 | UiState | — |
| `PatchViewModel` | L1 | 修补向导状态机、日志流、错误面板 | L2 events | UiState + 用户动作 | — |
| `PatchOrchestrator` | L2 | 八步流程编排、取消、回滚 | boot.img Uri + key | 产物路径 + report | 每步可分类型错误 |
| `BootImageRepository` | L2 | 调引擎 bootimg 模块（JNI）：解包/回包、AVB 处理、格式识别、分区与槽位探测 | boot.img | 工作目录布局 + header 元数据 | 格式不支持 |
| `RcInjector` | L2 | 生成 `init.sproot.rc`、`init.rc` 追加 `import`（cpio 内操作） | unpack 目录 | 修改后的 ramdisk | 厂商 rc 结构不支持 |
| `EngineRunner` | L2 | **JNI 调用** `libskrootpatch.so`（`nativeRunPatch`）、进度回调 + NDJSON 文件双写、退出码映射 | 内核路径 + key | ProgressEvent / Result | `UnsatisfiedLinkError` / 退出码 |
| `ArtifactVerifier` | L2 | 产物断言（C1 §4） | 输入/输出内核 | 断言报告 | 断言失败=不导出 |
| `WorkdirManager` | L2 | staging、manifest、失败保留、成功清理、容量预检 | — | 目录句柄 | 磁盘不足 |
| `SukeyProvider` | L3 | 生成 `[A-Za-z0-9]{48}`、Keystore 加密存、按需产出临时 0600 文件（用后即删） | — | 密钥 / 临时文件 | 存储损坏 |
| `RootExecClient` | L3 | **唯一 root 入口**：`apd root-exec --superkey-file <tmp> -- <cmd>`，透传 stdio/退出码 | 命令 | 输出流 / exit code | AP 未部署 / 密钥失配 |
| `AllowListRepository` | L3 | 授权表 UI 与审计（真值在 `/data/adb/ap/allow_list` 单文件） | `apd auth list` | 列表 | AP 不可用 |
| `DeploymentClient` | L3 | 首启 T2：`apd deploy --source <nativeLibDir> --superkey-file <tmp>` + 状态提示 | APK 内二进制 | `/data/adb/*` 就绪 | 部署中断（幂等可重试） |
| `ApdClient` | L4 代理 | 封装 AP 子命令与流式日志、版本探测（`version --json`） | 子命令 | 文本流 / JSON / exit code | AP 缺失 |

### 3.3 两条主调用链

**A. 修补链（本地，全程无网络）**

```
用户点「开始修补」
 └ PatchViewModel.patch(uri, mode)
   └ PatchOrchestrator.run()                       [Kotlin, Dispatchers.IO]
     ├ 1 Preflight    ：ABI=arm64? 空间≥3×boot? 引擎/工具自身 sha256 自检（§5.2.1）
     ├ 2 AcquireInput ：SAF→私有 workdir/boot.img；读 ro.boot.slot_suffix 定槽位
     ├ 3 Unpack       ：JNI nativeUnpack(boot.img) → kernel / ramdisk.cpio / dtb / header.json + AVB 标记
     ├ 4 Inspect      ：引擎 nativeInspect（CONFIG_KALLSYMS / patched? / 版本域）
     ├ 5 Patch        ：JNI nativeRunPatch(in=kernel.raw, out=kernel.patched, key|gen-key, json=双写)
     ├ 6 Verify       ：ArtifactVerifier（等长、sha256≠、密钥内嵌、hook_table 5 项、prefix==key[0:16]）
     ├ 7 RcInject     ：（条件性，D-14）nativeInjectRc：写 init.sproot.rc + init.rc 追加 import
     ├ 8 Repack+Deliver：JNI nativeRepack → new-boot.img（AVB 策略）→ 导出(SAF) / 刷入（**转通道② root-exec**）
     └ 收尾：成功→原子落盘 report + manifest；失败→保留 workdir + 首屏红色错误+完整日志
```

**B. 运行期链（修补刷入并重启后）**

```
init（原厂）import init.sproot.rc（管理器在步 7 注入）
 ├ on post-fs-data → exec /data/adb/apd post-fs-data --superkey-file /data/adb/sproot/key（同步）
 └ on boot        → start sproot_daemon = /data/adb/apd daemon --superkey-file <key>
     └ AP 读 0600 密钥文件 → execve(key[0:47]) 自提权 → getuid()==0
         ├ 阶段②services（fork）→ 轮询 sys.boot_completed → 阶段③boot-completed
         ├ 授权 socket（@sproot-<12hex>，SO_PEERCRED）→ 查 allow_list → fork 会话
         └ uid-listener 线程（packages.list 变化 → 重写 allow_list）
管理器 App（普通应用，无 root 常驻）
 ├ 探测：apd version --json（判断是否已部署/版本/内核是否已修补）
 ├ 部署（T2 首启）：apd deploy --source <nativeLibDir> --superkey-file <tmp>
 ├ 一切 root 动作：apd root-exec --superkey-file <tmp> -- <cmd>（§3.5.2）
 └ 修补：与运行期完全解耦——修补只需文件系统权限，**不需要已存在 root**
```

### 3.4 进程与权限边界

| 阶段 | 需要的权限 | 说明 |
|---|---|---|
| 选镜像/解包/patch/rc 注入/重打包/导出 | **普通应用权限**（无需 root） | 全程在 `filesDir/patch/` 私有目录；这是「本地修补」相对 Magisk/KSU 的核心优势：首装也能先出镜像 |
| 部署 / 模块 / 授权 / 刷入 boot 分区 | **root**，但一律**经 `apd root-exec`** 获取（不在管理器进程内提权） | 首装鸡生蛋问题的解法见 §3.5.3（T2）与 §5.6-③ |
| 挂载/读写 `/data/adb` | root（AP 内部） | 管理器不直接写，避免双写者 |

### 3.5 APK ↔ AP 接口设计（契约 v1.1 口径，新增）

> 本节是「管理器（newapp）↔ AP 二进制（apd）」的接口基线，依据 DOC-05 §3.1/§4/§6 与 lead 2026-11-08 的 E1–E5 决议。**与旧契约 v1.0 的差异汇总见 §6.4。**

#### 3.5.1 接口清单（管理器只用这 6 条）

| 接口 | 用途 | 调用形态 | 备注 |
|---|---|---|---|
| `apd version --json` | 探测部署状态/版本/内核修补状态 | 普通进程执行（无需 root） | 替代旧 `-V` 字符串；返回 `{apd_version, deployed, key_present, kernel_patched?}` |
| `apd deploy --source <nativeLibDir> --superkey-file <tmp>` | **T2 首启自举**（安装 AP/busybox/key/隐藏 su/空授权表） | 普通进程启动 → AP 自提权 → 执行清单 | 幂等；替代已取消的独立部署器（E5） |
| `apd root-exec --superkey-file <tmp> [--uid N] -- <cmd>` | **管理器唯一 root 入口** | 普通进程启动 → AP 自提权 → exec 目标命令 | 透传 stdout/stderr/exit code；管理器不解析内部实现 |
| `apd auth list/grant/revoke/exclude/unexclude` | 授权表 UI | 经 `root-exec` 调用（或 AP 内部直调） | 真值 = `/data/adb/ap/allow_list` 单文件（E4） |
| `apd module …` | 模块生命周期 | 经 `root-exec` 调用 | 行为与 APatch 一致（DOC-05 §3.1） |
| 授权 socket `@sproot-<12hex>` | **第三方 App** 获取 root 会话 | 客户端库连接 → `SO_PEERCRED` → allow_list 判定 | 管理器不用它；它面向第三方（E3） |

#### 3.5.2 铁律：root-exec 是唯一 root 入口

1. 管理器**不得**自行 `execve(key)`、不得自行 setuid、不得常驻 root shell。所有需要 root 的行为 = 拼一条 `root-exec` 命令。
2. 密钥交付：`SukeyProvider` 从 Keystore 解密 → 写 `cacheDir` 下**临时 0600 文件** → 传 `--superkey-file` → **调用结束立即删除**（DOC-05 §4.2）。密钥**不出现在 argv**（解决本文旧 D-4 的 `/proc/cmdline` 暴露面）。
3. 会话身份：`root-exec` 以管理器 uid 启动，AP 内部 `execve(key[47])` 后 `getuid()==0` 再 exec 目标命令；管理器侧只看「目标命令的退出码 + 输出」。
4. 失败语义：密钥失效/AP 缺失 → 静默失败（非 0 退出、无密钥痕迹日志），管理器以「AP 未部署 / 需重新修补」人话呈现。
5. **审计**：`root-exec` 的每次调用由 AP 记入 `/data/adb/ap/log/audit.log`（不含密钥），管理器「设置 → 审计」页只读展示。

#### 3.5.3 首装三段式（循环依赖解法，DOC-05 §6.1）

| 阶段 | 时机 | 主体 | 关键动作 | `/data` 状态 |
|---|---|---|---|---|
| **T1 修补期** | 用户点修补 | 管理器（**无 root**） | 选 boot.img → 引擎 JNI 解包 → 引擎 patch → 取 48 字符 key（Keystore 存）→（D-14）写 rc → 引擎 repack（AVB 策略）→ 导出/刷入 | **完全不动**（只有 App 私有 workdir） |
| **T2 首启** | 刷入后第一次开机、打开管理器 | 管理器 → AP | 写临时 key 文件 → `apd deploy` → AP 自提权后落盘 `/data/adb/*`、`sproot/key`、隐藏 su → 删临时文件 | 就绪；**模块能力尚未生效** |
| **T3 二次开机起** | 之后每次开机 | init + AP daemon | rc → post-fs-data → daemon → services/boot-completed | 稳态 |

> **UI 必做**：T2 完成后明确提示「需重启一次，模块功能才会生效」；重启前模块页禁用安装按钮（DOC-05 §6.1 末段）。

### 3.6 应用级状态机与页面能力门禁（改造自 APatch 10 态）

> 依据 APM §1.3（`APatchApp.kt:47-56` 的 10 态 + `BottomBarDestination.kPatchRequired/aPatchRequired` 门禁模型）。SPRoot 把「内核补丁状态」与「AP 部署状态」两轴显式化，并把「需 root 的能力」抽象为 `Capability` 集合，页面只声明依赖，不再各写 `if (rootAvailable())`。

#### 3.6.1 状态枚举（12 态，双轴 + 迁移过渡态）

```kotlin
enum class KernelAxis {                 // 轴一：SK 内核补丁（由引擎/镜像判定）
    UNKNOWN, STOCK, PATCHED, NEED_REPATCH, PATCHING, PATCH_FAILED
}
enum class ApAxis {                     // 轴二：AP 二进制/部署（由 apd version --json 判定）
    UNKNOWN, NOT_DEPLOYED, DEPLOYED, NEED_UPDATE, DEPLOYING, STALE,
}

data class AppState(
    val kernel: KernelAxis,
    val ap: ApAxis,
    val rebootRequired: Boolean,        // 修补已刷入但未重启 / 部署后需二次开机
    val safemode: Boolean,              // /data/adb/sproot/safemode 存在
    val keyPresent: Boolean,            // Keystore 内密钥存在（仅 App 私有，不代表已刷入）
)
```

#### 3.6.2 能力门禁矩阵（页面只声明 `requires`）

| 能力 | 依赖状态 | 不满足时的 UI 行为 |
|---|---|---|
| `PatchKernel`（修补/导出） | 无（普通 App 权限即可） | 永远可用（这是本条链存在的意义） |
| `FlashImage`（直刷分区） | `ap == DEPLOYED` 或「已有 root/AP 可用」 | 隐藏「直刷」，只留「导出镜像」+ 引导 fastboot |
| `UseRoot`（root-exec 类操作） | `ap == DEPLOYED && kernel == PATCHED && !rebootRequired` | 入口置灰 + 明确原因（未部署/未重启/内核未修补） |
| `ManageModules` | `UseRoot` + `!safemode` | 只读展示 + 「安全模式已启用」横幅 |
| `AuthorizeApps` | `UseRoot` | 只读列表 |
| `Redeploy`（T2） | `kernel == PATCHED && ap in {NOT_DEPLOYED, STALE}` | 显示「部署向导」主 CTA |

**门禁实现约束**：`Capability` 由 `AppState` 单点计算（`deriveCapabilities(state)`），**不在 Composable 内散落条件判断**；新增页面必须声明 `requires`，CI 静态检查（可选）grep 未声明者。

#### 3.6.3 迁移/过渡态与 UI 文案

| 场景 | 状态 | UI |
|---|---|---|
| 首次安装、未修补 | `kernel=STOCK, ap=NOT_DEPLOYED` | 引导「修补内核」（主 CTA）；模块/授权页置灰 |
| 已修补、已刷入、未重启 | `kernel=PATCHED, rebootRequired=true` | 顶部「需重启生效」+ 禁用 root 类入口 |
| 重启后、AP 未部署（T2 之前） | `kernel=PATCHED, ap=STALE/NOT_DEPLOYED` | 「一键部署」（T2）主 CTA + 说明「部署后需再重启」 |
| 部署完成、二次开机 | `kernel=PATCHED, ap=DEPLOYED` | 全功能；Home 显示各阶段状态 |
| 内核被 OTA 覆盖（`STOCK` 但 `/data` 有 AP） | `kernel=STOCK, ap=STALE` | 「需重新修补」+ 一键跳修补；root 类禁用 |
| 安全模式 | 任意 + `safemode=true` | 横幅「上次启动未完成，已禁用模块」+ 「解除安全模式」 |
| 修补失败 | `kernel=PATCH_FAILED` | 常驻错误卡片（§5.7-L5）+ 日志/诊断导出 + 「重试」 |

> 与 §5.8 的分工：**§5.8 是「修补过程内」的状态机**（一次修补的八步）；**本节是「应用生命周期」状态机**（跨重启、跨安装）。两者正交，UI 上分属「首页状态卡」与「修补页进度」。

---

## 4. UI 技术选型与依赖（miuix，已实证到 Maven 产物级）

> 本节以 `docs/research/miuix-integration.md`（调研子智能体，2026-11-08）为准，其证据基线是 **Maven Central 上真实发布的 0.9.4 产物**（AAR `aar-metadata.properties` / `.module` / `-sources.jar`），比 README/HEAD 更权威。凡本文与旧版（v0.1 依据 README+HEAD）冲突处，以本节为准。

### 4.1 版本矩阵（全部来自已发布产物元数据）

| 项 | 值 | 证据（E 编号见 §4.7） |
|---|---|---|
| **坐标（Android 工程）** | `top.yukonga.miuix.kmp:miuix-ui-android:0.9.4` | E12（官方 getting-started @ v0.9.4 明确给 `-android` 坐标） |
| 仓库 | **仅 `mavenCentral()`**，无需 JitPack / GitHub Packages | E12、E1 |
| 最新稳定版 | **0.9.4**（tag `v0.9.4`，2026-09-20） | E1、E2 |
| 已发布模块（0.9.4） | `miuix-ui` / `miuix-core` / `miuix-preference` / `miuix-icons` / `miuix-squircle` / `miuix-nav` / `miuix-shader` / `miuix-blur`(minSdk33) | E10 |
| **未发布（勿用）** | `miuix-glass`、`miuix-glass-icons` → **HTTP 404**（仅 HEAD 存在） | E10 |
| **`minCompileSdk`** | **37**（低于 37 直接构建失败） | E3 |
| **库自身 minSdk** | **24** | E3（AAR Manifest） |
| blur 模块 minSdk | **33**（本工程 minSdk 26 → 不可用） | E9 |
| Kotlin（库编译用） | **2.4.20**（`kotlin-stdlib:2.4.20`） | E5 |
| Compose（所需最低） | `foundation 1.12.0` → Android 侧 `androidx.compose.foundation:foundation:1.12.0` | E5、E6 |
| **Compose BOM 对齐** | 现有 `compose-bom:2026.08.00` 把 `foundation/runtime/ui` 钉在 **1.12.0** —— **与 miuix 要求完全一致，无需升降** | E6 |
| **Material3 传递约束（硬冲突）** | 经 `material3-window-size-class:1.12.0-alpha03` → `androidx...:1.5.0-alpha22` → 约束 `material3 = 1.5.0-alpha22`（BOM 是 1.4.0） | E5、E7 |
| AGP / Gradle / JDK | 库自身用 AGP 9.5.1、Gradle 9.7.1、JDK 21；**消费者侧仅 `minAndroidGradlePluginVersion=1.0.0`**（几无约束）。**本工程定版（app-client，已对齐）**：AGP **9.4.1** / Kotlin **2.4.20** / Gradle **9.7.1** / JDK **21** / compileSdk **37** / minSdk **26** | E3、E13、A-28 |
| 资源/字体/proguard | AAR **无 `res/`、无字体、无 consumer proguard 规则、无 jni** | E14 |
| 稳定度 | **experimental，API 可能变更**（README 与 v0.9.4 文档均有警告） | E20 |

**矩阵结论（只有两个动作项）**：① `exclude` 掉 `material3-window-size-class`；② Kotlin 提到 **2.4.20**。其余（compileSdk 37、minSdk 26、BOM 2026.08.00、JDK 21）**现有配置已满足**（E4）。

### 4.2 依赖声明（Android 单平台工程，可直接抄）

> **v0.1 的「必须用 KMP 仅 android 目标」judgment 已在 v0.2 撤销**：实测 miuix 在纯 Android 工程（`com.android.application` + `androidx.compose` BOM）下可直接消费，只需 exclude 一条传递依赖。KMP 不是必需，反而增加工具链复杂度。故**新工程按 Android 单平台建**（与 task-4 的 `newapp/` 一致）。

`gradle/libs.versions.toml`：

```toml
[versions]
kotlin     = "2.4.20"        # miuix stdlib 要求 2.4.20；编译器与 compose 插件同 ref
miuix      = "0.9.4"
composeBom = "2026.08.00"    # foundation/ui/runtime = 1.12.0，与 miuix 要求一致
agp        = "9.4.1"         # 本工程定版（app-client 已对齐）；miuix 侧无实质下限

[libraries]
miuix-ui         = { group = "top.yukonga.miuix.kmp", name = "miuix-ui-android",         version.ref = "miuix" }
miuix-preference = { group = "top.yukonga.miuix.kmp", name = "miuix-preference-android", version.ref = "miuix" }
miuix-icons      = { group = "top.yukonga.miuix.kmp", name = "miuix-icons-android",      version.ref = "miuix" }
miuix-squircle   = { group = "top.yukonga.miuix.kmp", name = "miuix-squircle-android",   version.ref = "miuix" }
miuix-nav        = { group = "top.yukonga.miuix.kmp", name = "miuix-nav-android",        version.ref = "miuix" }
# 不可用：miuix-blur-android（minSdk 33）、miuix-glass(-icons)-android（0.9.4 未发布）
```

**工具链定版（与 app-client 完全一致，避免两处口径）**：Gradle **9.7.1** / AGP **9.4.1** / Kotlin **2.4.20** / JDK **21** / compileSdk **37** / minSdk **26** / NDK 按引擎产物要求（task-3 出数）。

模块 `build.gradle.kts`（关键点：BOM + exclude + jniLibs 打包）：

```kotlin
android {
    namespace = "org.sproot.manager"
    compileSdk = 37            // ★ miuix minCompileSdk=37，低于此构建失败
    defaultConfig {
        minSdk = 26            // 库要求 ≥24；本工程取 26（与目标机型一致）
        targetSdk = 36
        ndk { abiFilters += "arm64-v8a" }
    }
    packaging { jniLibs { useLegacyPackaging = true } }  // ★ 引擎 ELF 需可执行（§5.2）
    buildFeatures { compose = true }
}

dependencies {
    implementation(platform("androidx.compose:compose-bom:2026.08.00"))
    implementation("androidx.compose.material3:material3")     // 1.4.0（由 BOM 钉）
    implementation(libs.miuix.ui) {
        // ★ 必须：否则 material3 在 runtime 侧被抬到 1.5.0-alpha22（编译 1.4.0 / 运行 alpha）
        exclude(group = "org.jetbrains.compose.material3", module = "material3-window-size-class")
    }
    implementation(libs.miuix.icons)        // 可选
    implementation(libs.miuix.preference)   // 可选（设置页）
    implementation(libs.miuix.squircle)     // 可选（低版本自动降级）
    implementation(libs.miuix.nav)          // 可选（导航）
    implementation("androidx.activity:activity-compose:1.13.0")
}
```

**为什么用 `miuix-ui-android` 而不是根模块 `miuix-ui`**：官方 getting-started 对 Android Compose 工程给的就是 `-android` 坐标（E12）；用根模块虽也能解析到 androidJvm 变体，但存在变体选择歧义（KMP 元数据 vs Android AAR），CI 上更易出现「解析到的变体与预期不符」。

**exclude 的安全性依据**：miuix 0.9.4 官方 sources jar（87 个 .kt）对 `WindowSizeClass`/`androidx.window` **零引用**（E8），因此排除后无缺失类风险。**替代方案**：把 BOM 从 `platform(...)` 改为 `enforcedPlatform(...)`，强制 material3 回 1.4.0——但会把整棵 Compose 依赖锁死，不推荐。

### 4.3 主题与最小可运行示例（API 签名已按 0.9.4 sources 核对）

```kotlin
// 主题：ThemeController 是 @Stable 可观察状态，必须 remember
@Composable
fun SpRootTheme(content: @Composable () -> Unit) {
    val controller = remember {
        // MonetSystem：跟随系统亮暗 + Android 12+ 壁纸取色；低版本回落内置调色板，不崩
        ThemeController(ColorSchemeMode.MonetSystem, keyColor = Color(0xFF3482FF))
    }
    MiuixTheme(controller = controller, content = content)
}

// 页面：Scaffold 同时是 Overlay/Dialog/Popup 的宿主（E19）
@Composable
fun HomeScreen(state: HomeUiState, onPatch: () -> Unit) {
    Scaffold(
        topBar = { SmallTopAppBar(title = "SPRoot", subtitle = "内核级 Root 管理器") },
    ) { padding ->
        Column(Modifier.fillMaxSize().padding(top = padding.calculateTopPadding())) {
            Card(insideMargin = PaddingValues(16.dp)) {          // Card 默认 insideMargin=0
                Text(text = "内核状态：${state.kernelStatus}")   // 注意：首参名为 text，不是 value
            }
            Button(onClick = onPatch, colors = ButtonDefaults.buttonColorsPrimary()) {
                Text(text = "开始修补")
            }
        }
    }
}
```

**必须遵守的 0.9.4 事实**（E15/E18）：
- `Text(text=…)`、`Card(insideMargin=…)`、`SmallTopAppBar(title, subtitle, actions)`、`Button(onClick, colors, content: RowScope)`、`Scaffold(topBar, content: (PaddingValues)->Unit)`。
- `MiuixIcons.Basic` 在 0.9.4 **只有** `ArrowRight/ArrowUpDown/Check/Close/Search/SearchCleanup/Sidebar`（**没有 `Back`**，HEAD 文档示例不可用）。
- **不要照 HEAD 文档写代码**（版本漂移）；以 `miuix-ui-android-0.9.4-sources.jar` 为准。

### 4.4 页面 ↔ 模块映射（UI 层只读状态）

| 页面 | 数据源（L2/L3/L4） | 关键交互 | 验收要点 |
|---|---|---|---|
| Home/状态卡 | `BootStateRepo`（是否已修补/待更新）、`ApdState`、版本信息 | 跳修补/跳授权 | 状态三态（未修补/已修补/待重启）不可测错 |
| 修补向导 | `PatchOrchestrator` 八步 + 进度 | 选镜像→跑→看日志→导出/刷入 | **八步每步独立状态灯**；错误面板常驻、可复制、可导出 |
| 日志页 | `EngineRunner` NDJSON + stderr | 实时滚动、暂停、保存 | 进程结束不丢最后 200 行（旧路线反例） |
| Root 授权页 | `apd auth list`（真值 `allow_list`） | 开关授权、context 选择 | 数据来自 AP，管理器不落第二份真值 |
| 模块页 | `apd module` 子命令 | 安装/启停/卸载/动作脚本 | 与 DOC-05 阶段编排一致 |
| 部署向导（新） | `ApdClient.version --json` / `apd deploy` | 首启 T2 一键部署、重启提示 | 部署幂等；未部署态明确可见 |
| 设置页 | 本地配置 + 密钥管理 | 重新展示密钥（二次确认）、备份/还原、日志导出 | 密钥绝不明文出现在日志/崩溃上报 |

### 4.5 与旧 UI 的对照（为何重写更省）

| 维度 | 旧（APatch 定制） | 新（miuix） |
|---|---|---|
| 依赖面 | Material3 + composedestinations(KSP) + libsu + OkHttp + Coil3 + ini4j + 24 主题 | miuix-ui(+preference/icons/nav) + lifecycle；无 KSP、无网络库 |
| 主题 | 自维护 24 套 | `MiuixTheme` + Monet 动态取色，删除主题维护成本 |
| 日志展示 | `SelectionContainer{Text(patchLog)}` 单块文本，随 ViewModel 生命周期 | 独立 LogViewer + 落盘 + 状态机，进程退出仍可看 |
| 授权页耦合 | 直连 JNI supercall（APL §3.4） | 全部走 apd 子命令，UI 无 native 依赖 |

### 4.6 UI 选型风险与降级策略（按影响排序）

| # | 风险 | 证据 | 影响 | 缓解/降级 |
|---|---|---|---|---|
| **U1** | **`compileSdk` 必须 ≥37** —— `minCompileSdk=37`，低于即构建失败（`requires ... compile against version 37 or later`） | E3 | 新工程/CI 必须提供 Android SDK 37；任何分支降 compileSdk 立刻炸 | 工程 `compileSdk=37` 钉死；CI 镜像预装 SDK 37；task-8 首个冒烟跑 `checkDebugAarMetadata` |
| **U2** | **Material3 被传递约束抬到 `1.5.0-alpha22`**（`material3-window-size-class:1.12.0-alpha03` → 约束 `material3=1.5.0-alpha22`；且该依赖只在 **runtimeElements**，编译期仍 1.4.0） | E5/E7 | 典型「编译通过、运行 `NoSuchMethodError`」；APK 内 Material3 实为 alpha | **必须** `exclude(group="org.jetbrains.compose.material3", module="material3-window-size-class")`（源码零引用，E8）；复验 `./gradlew :app:dependencies --configuration releaseRuntimeClasspath \| grep material3` |
| **U3** | **blur / glass 不可用**：blur AAR `minSdk=33`（本工程 26）；`miuix-glass`/`glass-icons` 0.9.4 **未发布（404）** | E9/E10 | 照 HEAD README 写依赖会构建失败/404 | **降级策略**：≥API33 可选 blur；否则用纯色/半透明背景 + `miuix-squircle`（低版本自动关闭，E11）。glass 系一律不设计、不进 UI 规范 |
| **U4** | **`MiuixTheme` 覆盖 `LocalIndication`/`LocalContentColor`/`LocalOverscrollFactory`** | E16 | MiuixTheme 内嵌 Material3 组件 → 涟漪/内容色/滚动效果被替换 | **按页面分区**：miuix 页面套 `MiuixTheme`，Material3 页面套 `MaterialTheme`；Activity 根只做二选一，不深层嵌套 |
| **U5** | **组件同名冲突**（`Text/Button/Icon/IconButton/Card/Scaffold/Slider/Switch/Surface/Checkbox/Divider` 与 Material3 全同名） | E17 | 通配符 import 歧义；给 Miuix `Card` 传 `elevation` 直接编译错误 | Miuix 组件**显式单条 import**（禁 `import ...basic.*`）；混用处 `as MiuixText` 别名 |
| **U6** | **HEAD 文档 ≠ 0.9.4 API**（`MiuixIcons.Back` 不存在；`Text` 首参名是 `text`；`Card.insideMargin` 默认 0） | E18/E15 | 照文档写编译不过 | 以 `-sources.jar`/Dokka 为准；文档 URL 固定到 `v0.9.4` tag |
| **U7** | **experimental + 版本漂移** | E20 | 升级可能破坏性变更 | 锁死 `0.9.4`；UI 建 `designsystem/` 门面（SpCard/SpButton/SpSection）；升级走独立 PR + 截图回归 |
| **U8** | **R8/minify 未实测**（AAR 无 `proguard.txt`/consumer rules） | E14 | release 首次进 Miuix 页面/Popup 可能崩 | `assembleRelease` + 真机首次进页面必测；必要时补 keep 规则 |
| **U9** | **Overlay/Dialog/Popup 必须挂在 `Scaffold` 内** | E19 | 脱离 Scaffold → 弹不出来/异常 | 页面模板统一 `Scaffold(topBar, content)`；多 Scaffold 嵌套安全 |
| **U10** | Kotlin 2.4.10 → **2.4.20** 升级 | E5 | stdlib 告警/不匹配 | 版本目录 `kotlin="2.4.20"`（compose 编译器插件同 ref 一起升）——两个动作项之一 |

### 4.7 调研证据索引（E1–E21，均出自 `docs/research/miuix-integration.md` §6）

| 编号 | 内容 |
|---|---|
| E1/E2 | Maven Central `maven-metadata.xml`（`release=0.9.4`，`lastUpdated=20260920082405`）；GitHub Releases tag `v0.9.4`（2026-09-20） |
| E3 | `miuix-ui-android-0.9.4.aar` 的 `aar-metadata.properties`：`minCompileSdk=37`、`minCompileMinorSdk=0`、`minAndroidGradlePluginVersion=1.0.0`、`coreLibraryDesugaringEnabled=false`；Manifest `minSdkVersion=24` |
| E4 | 旧 APatch 工程现状：`compileSdk=37`、`minSdk=26`、`kotlin=2.4.10`、BOM `2026.08.00`、JDK 21、Gradle 9.7.0 |
| E5 | `miuix-ui-android-0.9.4.module`：runtime 变体含 `material3-window-size-class:1.12.0-alpha03`、`navigationevent-compose:1.1.2`、`material-color-utilities:5.0.1`、`kotlin-stdlib:2.4.20`、`foundation:1.12.0` |
| E6 | `foundation-android:1.12.0` → `androidx.compose.foundation:foundation:1.12.0`；BOM `2026.08.00` 钉 foundation/runtime/ui=1.12.0、material3=1.4.0 |
| E7 | `material3-window-size-class-android:1.5.0-alpha22` 携带约束 `material3=1.5.0-alpha22`；`org.jetbrains...:1.12.0-alpha03` 重定向到它 |
| E8 | miuix 0.9.4 sources（87 个 .kt）对 `WindowSizeClass`/`androidx.window` 零引用 |
| E9/E10 | blur AAR `minSdkVersion=33`；各模块 0.9.4 发布状态（glass/glass-icons = 404） |
| E11 | `isRuntimeShaderSupported()` = `SDK_INT >= TIRAMISU`；squircle 低版本 `return null`（静默） |
| E12 | 官方 `docs/guide/getting-started.md` @ `v0.9.4`：Android 用 `miuix-ui-android` 坐标，仅需 mavenCentral |
| E13 | HEAD 构建常量（AGP 9.5.1 / Kotlin 2.4.21 / Gradle 9.9.0 / JDK 21）——仅参考 |
| E14 | AAR 内容：`classes.jar`(1.96MB) + `baseline-prof.txt` + `R.txt` + Manifest + aar-metadata；无 res/assets/proguard/jni |
| E15 | 0.9.4 sources 逐条签名确认（`Text(text=…)`/`Button`/`Card(insideMargin)`/`SmallTopAppBar`/`Scaffold`/`MiuixTheme`/`ThemeController`） |
| E16 | `MiuixTheme` 的 `CompositionLocalProvider` 覆盖项与默认值（`lightColorScheme()`/`Color.Black`） |
| E17/E18 | 组件同名清单；`MiuixIcons.Basic` 0.9.4 仅 7 个图标（无 `Back`） |
| E19 | `Scaffold.popupHost = { MiuixPopupHost() }`，`LocalPopupStates/LocalDialogStates` 由 Scaffold 提供 |
| E20/E21 | experimental 声明；历史坐标（`miuix`/`miuix-shapes`/`miuix-navigation3-*`）勿用 |

---

## 5. 本地修补链路（本方案核心）

### 5.1 设计目标与硬约束

| 目标 | 实现约束 |
|---|---|
| 全程本地 | 无任何网络调用；引擎/工具全部 APK 内（**无 shell 脚本、无 busybox、运行期不下载**） |
| 失败必可见 | 引擎退出码 + stderr + workdir 保留 + 首屏错误面板（§5.7） |
| 失败不产半成品 | 引擎不写 `--out`（C1 §3）+ 编排层 staging + 产物断言（§5.6） |
| 可重复 | 同一 boot.img + 同一 key 产物字节一致（除时间戳类字段，引擎输出确定性） |
| 可诊断 | 每次尝试一个 `manifest.json`：输入 sha256、工具版本、每步耗时/结果、引擎 report |

### 5.2 决策 D-1：引擎交付形态 = **JNI 进程内（`libskrootpatch.so`）为主**，子进程为诊断模式

> **lead 2026-11-08 决议（契约 v1.1 会签口径）**：修补链在 App 进程内完成（选镜像→解包→**JNI `libskrootpatch`**→重打包→导出），**不需要 root**；子进程 exec 形态保留为**诊断/回退模式**，不作为默认。本节据此改写（v0.1 曾以子进程为默认，已撤销）。

| 维度 | 形态 A：JNI 进程内（**默认**） | 形态 B：独立 ELF + 子进程（**诊断/回退**） |
|---|---|---|
| 与产品边界一致性 | ✅ 修补链 = App 进程内、无 root、不产生特权进程 | ⚠️ 多一个进程，边界变宽 |
| 引擎实现 | 把 `main()` 重构为 `engine_run(argc, argv, jsonSink|callback)`；**CLI 与 JNI 共用同一实现**，C1 契约不重写（参数面 `--in/--out/--key/--gen-key/--json/--workdir` 与 12 退出码原样保留） | 现有 CLI 二进制直接可用 |
| 崩溃隔离 | ❌ native 段错误 = App 崩溃。**缓解**：引擎侧加强参数/边界校验（越界读、符号缺失、空间不足均在写盘前失败，C1 §3）；修补前自动保存状态，崩溃后重进可看到「上次异常」 | ✅ 段错误只影响子进程 |
| 进度通道 | JNI 回调（`callbackFlow`）或引擎内部仍写 NDJSON 文件由 UI 尾随（**两者都保留**：回调实时、文件可回溯） | NDJSON 逐行读 |
| 执行权限风险 | ✅ 无 exec 环节（`System.loadLibrary`），不受 Android 10+ 私有目录 exec 限制 | ⚠️ 需从 `nativeLibraryDir` 绝对路径 exec；KSU 证明可行，但多一层权限依赖 |
| 供应链风险 | 需 CI 断言 `libskrootpatch.so` 为 `AArch64`（RC F2：当前唯一产物是 x86-64） | 同上 |
| 体积/内存 | UI 进程 RSS +数 MB（asmjit/capstone 已链接进来） | 独立进程，UI 常驻内存更小 |
| 何时启用 | **默认** | ①真机出现 native 崩溃无法定位时；②需要在 PC/CI 上批量跑修补；③JNI 加载失败（`UnsatisfiedLinkError`）时降级 |

**两个 native 资产的职责边界（不可互相替代）**

| 资产 | 形态 | 运行位置 | 职责 | 权限 |
|---|---|---|---|---|
| **`libskrootpatch.so`** | JNI `.so`（`jniLibs/arm64-v8a/`） | **App 进程内** | 内核 Image 字节 patch + 密钥生成（C1 契约） | 普通 App 权限，**无 root** |
| **`libapd.so` → `/data/adb/apd`** | Rust 可执行（部署后落稳定路径） | 独立进程（init 拉起/管理器 `root-exec` 拉起） | 守护、模块、授权、部署、root 执行 | **root**（内部 `execve(key47)` 自提权） |
| ~~magiskboot / kptools~~ | **不引入**（lead 已裁自研 bootimg） | — | — | — |

> **反模式警戒**：不要试图用 `libskrootpatch.so` 承担特权行为（它不是特权执行者）；也不要用 AP 二进制做内核 patch（AP 无修补能力）。二者短信道、不同权限域。

#### 5.2.1 引擎能力自检（每次修补前执行，失败即阻断）

```kotlin
// EngineCapabilityProbe —— 结果落 logs/preflight-<ts>.log
1. 资产存在：nativeLibraryDir/libskrootpatch.so 存在，size == binaries.json 记录值
2. 架构正确：读 ELF header，e_machine == 0xB7(AArch64)，ELFCLASS64，ET_DYN
3. 摘要正确：sha256(file) == binaries.json
4. JNI 加载探针：System.loadLibrary("skrootpatch") 成功；
   nativeVersion() 返回 engine_version / upstream_baseline（非空、与 binaries.json 一致）
   - UnsatisfiedLinkError → 阻断修补，文案「修补组件不可用（架构/依赖不匹配）」+ 诊断导出
5. 自检干跑：nativeSelftest() 返回 0（引擎内部做一次最小自检：符号表解析器/asmjit 初始化/临时目录可写）
6. 【仅诊断模式】执行探针：exec nativeLibraryDir/libskrootpatch.so --selftest
   - 失败分类 ENOEXEC/EACCES/ENOENT/超时；失败不影响 JNI 主路径
```

> **对 task-3 的明确要求**：①同库导出 JNI 入口（`System.loadLibrary("skrootpatch")` 可加载）+ `nativeVersion/nativeSelftest/nativeRunPatch`；②**同时**保留 CLI `main()`（诊断模式 + PC/CI 复用）；③NDK clang `aarch64-linux-android` 构建，CI 用 `readelf` 断言架构（RC §5.2 断言 1）；④JNI 层禁止抛 C++ 异常跨边界（`-fno-exceptions` + 错误码返回）。

**实现要点**

1. 打包：`libskrootpatch.so` 放 `app/src/main/jniLibs/arm64-v8a/`；`binaries.json` 由 CI 生成（`name/version/sha256/source_commit/abi`）。
2. JNI 调用：`external fun nativeRunPatch(inPath: String, outPath: String, keyOrNull: String, jsonPath: String?, workDir: String?): Int`，返回值即 C1 退出码；进度经 `onProgress(phase, pct, detail)` 回传 + NDJSON 文件双写。
3. **线程**：`nativeRunPatch` 在 `Dispatchers.IO` 专用线程执行（引擎内部有阻塞 IO 与大量内存分配）；**并发保护**：进程级 `Mutex`，同一时刻只允许一次修补（引擎不是线程安全的）。
4. **崩溃可观测**：JNI 入口外层记录 `state.json`（步骤、输入 sha256、时间戳）；App 启动时若发现 `state.json` 为「进行中」，展示「上次修补异常中断 + 日志/诊断包」，不静默。
5. 完整性：随包 `binaries.json` 参与 §5.2.1 自检；缺件/摘要不符 → `E-PRE-BIN`。
6. **构建期断言**（Gradle task，失败即 build fail）：`lib/arm64-v8a/libskrootpatch.so` 存在且 `readelf -h` 显示 `Machine: AArch64`；`assets/` 内不得出现任何 `.sh` 修补脚本或第三方 ELF（RC F2/F3 + lead「自研 bootimg」决议）。

### 5.3 决策 D-2：bootimg 解包/重打包 = **引擎自研**（lead 已裁，替代 magiskboot/kptools）

> **lead 2026-11-08 决策**：修补引擎自研 bootimg 解析/重打包，替代 magiskboot；**SK 只 patch kernel 段、不改 ramdisk**（比 KSU 简单）。本节按此改写（v0.2 的「magiskboot 主选」已撤销）。

| 方案 | 结论 | 理由 |
|---|---|---|
| **引擎自研 bootimg（选定）** | ✅ 纳入 `engine/`（与 SK patch 同一二进制/JNI） | ①无 GPL-3.0 传染（消除 D-3 的许可证阻塞）；②无外部二进制依赖（消除「缺件即失败」的 RC P0 类故障）；③解包/回包/校验都在同一进程内，错误码与进度可结构化（C1 契约）；④可用 `--json` 报告每步，质测可断言 |
| magiskboot | ❌ 不引入 | GPL-3.0 + 外部 ELF + 不可审计；且其 cpio 能力仅在「需改 ramdisk」时才必要（见 D-14） |
| kptools | ⚠️ 仅作**参考实现/离线校验**（不随 APK） | GPL-2.0 源码在手（`upstream/KernelPatch/tools/bootimg.c`），可用于对照解析结果；运行期不依赖 |
| KSU `android-bootimg`（Rust crate） | ⚠️ 仅**参考设计**，不复制代码 | KSU §2.7/§5.1-③ 证明纯自研可行；其许可证需单独确认，本文不假定可直接 vendoring |

**自研 bootimg 的最小能力集（task-3 交付）**

| # | 能力 | 说明 | 依据 |
|---|---|---|---|
| P1 | boot header v0–v4 解析/序列化 | `magic="ANDROID!"`、kernel/ramdisk/second/dtb 各段 offset+size、页大小、os_version | AOSP bootimg 规范；`upstream/KernelPatch/tools/bootimg.c` 可对照 |
| P2 | kernel 段取出 + **解压归一** | gzip / lz4 / 未压缩；记录原压缩类型与参数以回填 | §5.4 步 3；引擎拒 `.gz` 直入（RC §2.3） |
| P3 | 写回 kernel 段（**等长**） | SK patch 不改长度 → 原地覆盖 + 重算校验（如有） | A1 §1、C1 §2.1 |
| P4 | repack | 段偏移按页对齐重排，输出 `ANDROID!` 合法的 boot.img | §5.4 步 8 |
| P5 | **AVB footer 处理** | 识别尾部 `AVBf` footer；输出两种模式：①保留 footer 但失效（需 vbmeta 处理）②剥离 footer | lead 2026-11-08：AVB 已确认存在 |
| P6 | （可选）ramdisk cpio 增删 | **仅当 D-14 判定需要 rc 注入时实现**（lead 明确「SK 不改 ramdisk」；但 AP 自启需要 rc，见 D-14） | E5、DOC-05 §5.1 |
| P7 | 分区/槽位探测 | `ro.boot.slot_suffix`、`by-name` 映射、`init_boot` vs `boot` | §5.4 步 2、R15 |

**注意（lead 表述与 AP 自启的张力 → D-14）**：lead 决策「SK 只 patch kernel 段、不改 ramdisk」在**内核 patch 层面完全成立**（SK 五 hook 全在 kernel Image 内，与 ramdisk 无关）。但**AP 守护的自启动依赖 ramdisk 内的 `init.sproot.rc` + `init.rc import`**（DOC-05 §5.1/E5）。因此存在两条互斥解读：
- **解读 A（推荐）**：引擎**只 patch kernel**；rc 注入由「引擎的 bootimg 模块」在同一流程内完成（P6 开启），即"SK 不改 ramdisk"指"SK 的内核补丁不依赖 ramdisk 改动"，而非"最终产物不改 ramdisk"。
- **解读 B**：最终 boot.img 的 ramdisk 一字不改 → **AP 无法开机自启** → 模块/授权不可用（root 仅在有 AP 进程被手动拉起时可用）。

→ 该点已列为 **§9 D-14**，需 lead 一句确认；本文实现按 A 预留、按 B 也能降级运行（UI 明示"模块能力需手动启动"）。

### 5.4 八步流程（可执行步骤 + 每步失败处理）

> **总原则（源自 RC §五 5.1）**：每一步都必须有**可断言的成功判据**；「打印 Successfully」不算成功。全程不经过 `sh -c`、不依赖 busybox、不做任何网络访问、不打包 shell 脚本。

| 步 | 动作 | 具体命令/断言 | 失败处理（用户可见） |
|---|---|---|---|
| **1 Preflight** | 环境 + 自身能力自检 | ①`Build.SUPPORTED_ABIS` 含 `arm64-v8a`；②`filesDir` 可用空间 ≥ 3×boot.img；③`binaries.json` 逐项 sha256 + ELF `e_machine=0xB7`（§5.2.1，**仅引擎 + AP 二进制**，无第三方工具）；④JNI `nativeVersion/nativeSelftest`；⑤创建 `logs/patch-<ts>.log` 并写入 cmd/env/cwd | 首屏红条 + 具名错误码（`E-PRE-01` ABI / `E-PRE-02` 空间 / `E-PRE-BIN` 二进制缺失或校验失败 / `E-PRE-03` JNI 加载失败），附「导出诊断」；**绝不进入下一步** |
| **2 AcquireInput** | 取镜像到 staging + 指纹 | SAF `openInputStream(uri)` → `work/boot.img`（分块拷贝 + 流式 sha256）；记录 `ro.boot.slot_suffix`、`ro.product.device`、`ro.build.version.sdk`；**扫描尾部 AVB footer（`AVBf`）并记录 vbmeta 关联信息**；判定来源是 `boot` 还是 `init_boot` | `[E-IO-04] 无法读取所选文件（%s）`；`[E-IO-05] 剩余空间不足：需 %d MB，实有 %d MB` |
| **3 Unpack** | 拆镜像 + **解压归一**（引擎 bootimg 模块） | JNI `nativeUnpack(work/boot.img, work/unpack/)` → `kernel`、`ramdisk.cpio`（若有）、`dtb*`、header 元数据 JSON；判断 `kernel` 是否已解压裸 Image：压缩魔数（`1f 8b` gzip / `04 22 4d 18` lz4）→ 解压为 `kernel.raw` 并**记录原压缩类型与参数** | `[E-IMG-06] 镜像格式无法解析（%s）`；`[E-IMG-07] 内核压缩格式不支持（%s）`；详细原因折叠块 |
| **4 Inspect** | 前置校验（APL §2.3-1 教训） | 引擎 `nativeInspect`：①必含 `CONFIG_KALLSYMS=y`；②`patched=false`（已修补则拒）；③内核版本落在引擎支持域（3.10~6.18，A1）；④记录 sha256 与大小 | 每项具名错误：`[E-IMG-08] 内核未开启 CONFIG_KALLSYMS`、`[E-IMG-09] 该镜像已被修补，请使用原厂 boot.img`、`[E-IMG-10] 内核版本 %s 不支持` |
| **5 Patch** | 调 SK 引擎（**JNI 进程内**） | `nativeRunPatch(in = work/kernel.raw, out = work/kernel.patched, key = K \| null(gen), json = work/p.ndjson)`；进度回调 + NDJSON 双写；引擎内部仍按 C1 落 `report.json` | 退出码→中文映射（§5.5）；**同时**校验 C1 §3 铁律：rc≠0 时 `kernel.patched` 必须不存在（若存在 → `E-ENG-11` 引擎违约并删除） |
| **6 Verify** | 产物断言（C1 §4 + RC §5.1-[5]） | ① `size(out)==size(in)`；② `sha256(out)!=sha256(in)`；③ out 内存在 48B 窗口 == `key[0:47]+NUL`；④ `report.hook_table` 含 5 个 hook 且地址非 0；⑤ `report.hidden_dir_prefix == key[0:16]`；⑥ `report.root_key == 传入 key`（防「刷了别人的 key」）；⑦ 引擎回读 `patched=true`（若引擎提供该能力，否则记 `skipped`） | 任一失败 → `[E-VER-12] 产物校验失败：<断言名>`，**删除产物、不导出、不回写**，保留 workdir |
| **7 RcInject**（**条件性**：仅 D-14 解读 A） | ramdisk rc 注入（E5 划归管理器/引擎 bootimg 模块） | JNI `nativeInjectRc(unpack/, key16Independent)`：写 `init.sproot.rc`（内容见 DOC-05 §5.1，不含密钥）+ `init.rc` 末尾追加 `import /init.sproot.rc`；断言：cpio 内可见 `/init.sproot.rc` 且 `init.rc` 尾部含该行；原 `init.rc` 备份进 manifest | `[E-RC-17] rc 注入失败`；`[E-RC-18] 该机型 init.rc 结构不支持` → 降级「跳过 rc 注入」（内核 patch 仍生效，模块需手动启动，UI 明示） |
| **8 Repack+Deliver** | 重打包与交付（成功判据=可验证事实） | JNI `nativeRepack(work/unpack/, out=new-boot.img)` → **AVB 策略二选一**（§5.4.1）；断言：存在、头部魔数 `ANDROID!`、size ≤ 目标分区 size、`sha256` 与 manifest 一致；交付按 **PatchMode 四模式**（§5.4.2） | `[E-PKG-13] 重打包失败/产物头非法`；`[E-DEL-14] 导出被取消`；`[E-FLS-15] 刷入失败（槽位 %s），已回滚`；`[E-FLS-16] 刷入后回读不一致，已尝试恢复备份` |

#### 5.4.1 AVB 处理（lead 2026-11-08：AVB 已确认存在）

已确认事实（lead）：目标设备 boot.img 尾部有 **`AVBf` footer**，固件中存在 **`vbmeta.img`**。因此修补后的 boot.img 与原厂 vbmeta 不再匹配，必须处理。三条路径：

| 路径 | 操作 | 适用 | 代价 |
|---|---|---|---|
| **A. vbmeta 解锁（主路径）** | 刷入前：`fastboot --disable-verity --disable-verification flash vbmeta vbmeta.img` → 再刷 patched boot | 首次安装 / 有 PC 的场合 | 关闭 verity/verification（设备状态可见）；**与「全程手机本地」目标存在张力，需人类确认（→ D-15）** |
| **B. 剥离 AVB footer（备选）** | repack 时去掉 `AVBf` footer 与相关对齐填充 → 产物无 AVB 签名 | 无 vbmeta 可刷 / 变砖救援 | bootloader 可能因「无 AVB 元数据」拒绝启动（机型相关，须真机验证） |
| **C. 官方全量回滚** | 用固件包内 `flash.bat`（官方刷机脚本）回原厂 | 任何失败后的兜底 | 清数据；需 PC |

**实现要求**：①步 2 必须记录「AVB 是否存在」；②步 8 产物命名区分 `-avb` / `-noavb`；③UI 在交付前必须明确展示三条路径与后果（禁止静默选 B）；④路径 A 的 fastboot 指令以文本形式给出（可复制），不在 App 内执行（App 无 fastboot 能力）。

#### 5.4.2 交付模式（沿用 APatch `PatchMode` 四模式，A2 借鉴）

| 模式 | 语义 | 产物/收尾 |
|---|---|---|
| `PATCH_ONLY`（**默认**） | 只出镜像，用户自行刷 | 导出到用户选择位置（SAF），命名 `sproot_patched_<ver>_<keyhash8>.img`；同时在 App 私有目录保留副本 |
| `PATCH_AND_INSTALL` | 直刷当前槽 | **经特权链**：`root-exec -- dd`/写分区 + 回读校验；需 `capability=FlashImage`；标记「需重启」 |
| `INSTALL_TO_NEXT_SLOT` | 写备槽 + 切槽 | `root-exec` 执行 `bootctl set-active-boot-slot` + 写 `post_ota.sh`（开机成功 `mark-boot-successful` 自删） |
| `UNPATCH` | 还原原厂 | 从内容寻址备份（sha1）写回 + 清理 AP（`apd undeploy --keep-modules`） |

> 直刷三条（2/3/4）**全部走特权链**（`root-exec`），修补链本身只产 `new-boot.img`——这就是 §3.1.1 「两链唯一交点」。

**rc 注入物的内容（固定模板，不含密钥）**：`init.sproot.rc` 只含 `on post-fs-data → exec /data/adb/apd post-fs-data --superkey-file /data/adb/sproot/key` 与 `service sproot_daemon`（`class late_start`、`user root`、不写 seclabel、`on boot start`），逐字模板与理由见 **DOC-05 §5.1**（本文不复制，避免双源漂移）。**约束**：①不内联密钥；②只引用 `/data` 下路径；③注入幂等（已存在则替换为同版本）；④失败可回滚（manifest 保存原 ramdisk 散列）。

**取消与超时**：`PatchOrchestrator` 持有 `Process` 句柄（取消 `destroyForcibly()`）与每步超时（unpack 60s / patch 300s / rc+repack 90s / flash 120s）；取消后清理 staging、UI 明示「已取消，未产生任何镜像、设备未改动」。

**日志落盘要求（RC F5）**：`filesDir/logs/patch-<yyyyMMdd-HHmmss>.log`，内容含：机型/内核/系统版本、每步 argv+cwd+env（脱敏：密钥替换为 `<KEY:len=48,sha256前8位>`）、stdout 与 stderr 分列、退出码、产物 sha256、耗时。日志保留最近 20 次，可一键导出 zip。

**shell 与 busybox 处置（新链明确不用）**：不走 `boot_patch.sh`，不打包 busybox；解包/重打包/刷入全部是直接 `ProcessBuilder(listOf(...))` 或 `root-exec`。RC §四 已论证「最小可判定失败面越短越好」，且 `echo; exit $?` 类吞码 bug（RC P2）在脚本层无法根治——干脆去掉脚本层。

### 5.5 引擎调用契约（与 C1 §1/§3 完全对齐，不新增协议）

```text
skroot-patch --in <kernel_image> --out <patched_kernel> (--gen-key | --key <48字符>)
             [--json <ndjson_path>] [--workdir <dir>]
```

退出码 → 用户文案（C1 §3 的 11+1 个码，管理器侧一律不外抛原始码给普通用户，只在诊断区保留）：

| code | C1 名称 | 用户文案 | 处置 |
|---|---|---|---|
| 0 | OK | 修补成功 | 进入校验/交付 |
| 101 | E_USAGE | 内部参数错误（缺陷，请上报） | 记日志 + 上报入口 |
| 102 | E_INPUT | 所选文件不是可修补的内核镜像 | 引导重选原厂 boot.img |
| 201 | E_KALLSYMS | 该内核不包含符号表，暂不支持 | 机型不支持提示 |
| 202 | E_SYMBOL_MISSING | 关键内核符号缺失，暂不支持 | 同上 + workdir 上报 |
| 210 | E_OFFSET_DERIVE | 内核结构调整推导失败 | 同上 |
| 211 | E_INIT_CRED_NOT_FOUND | 初始化凭证定位失败 | 同上 |
| 220 | E_NO_HOOK_SPACE | 内核可注入空间不足 | 机型不支持 |
| 230 | E_UNSUPPORTED_VERSION | 内核版本超出支持范围 | 提示支持区间 |
| 240 | E_WRITE | 写入失败（存储/权限） | 检查空间与权限 |
| 250 | E_INTERNAL | 未分类错误 | 上报 + 保留 workdir |

进度映射（C1 §2.3 的 `phase` 枚举 → UI 八步）：`read_input→②`、`parse_kallsyms|derive_offsets|locate_init_cred→⑤(子步)`、`patch_cfi_huawei|patch_hooks→⑤`、`embed_key→⑤`、`write_output→⑤/⑥`。UI 不显示裸 `pct`（会跳），显示子步文字 + 不确定进度条。步⑦rc 注入与步⑧重打包不在引擎进度流内，由编排层上报。

> 关键设计原则：**管理器绝不解析引擎的 stdout 人读文本**做判断，只认 `--json` 事件与退出码——文本格式可自由改，契约面稳定。

**管理器侧自有错误码（编排层，与引擎码不冲突；引擎码原样透传到诊断区）**

| code | 触发点 | 用户文案 | 是否可重试 |
|---|---|---|---|
| `E-PRE-01` | 步 1 ABI/设备检查 | 本机架构不支持（需 arm64-v8a） | 否 |
| `E-PRE-02` | 步 1 空间检查 | 存储空间不足（需 %d MB） | 是（清理后） |
| `E-PRE-BIN` | 步 1 二进制自检 | 修补组件缺失或损坏（%s） | 否（需更新 App） |
| `E-EXEC-03` | 步 1 可执行探针 | 本机禁止执行随包组件（已切换/未切换 JNI 模式） | 条件性 |
| `E-IO-04/05` | 步 2 | 无法读取文件 / 空间不足 | 是 |
| `E-IMG-06/07` | 步 3 | 镜像无法解析 / 内核压缩格式不支持 | 否（换镜像） |
| `E-IMG-08/09/10` | 步 4 | 无 kallsyms / 已修补 / 版本不支持 | 否 |
| `E-ENG-11` | 步 5 | 引擎违反契约（失败却产出文件） | 否（缺陷，上报） |
| `E-VER-12` | 步 6 | 产物校验失败（%s） | 条件性（重跑） |
| `E-RC-17/18` | 步 7 | ramdisk rc 注入失败 / 机型 rc 结构不支持 | 后者可「跳过 rc 注入」降级 |
| `E-PKG-13` | 步 8 | 重打包失败 | 条件性 |
| `E-DEL-14` | 步 8 | 导出被取消 | 是 |
| `E-FLS-15/16` | 步 8 | 刷入失败 / 回读不一致（已尝试恢复备份） | 否（须人工确认） |

### 5.6 失败回滚（三级）

**① 引擎级（不可绕过，已实现）**
- 非 0 退出绝不产生 `--out`：`engine/src/skroot_patch_main.cpp` 的统一失败出口 `fail()` 会 `fs::remove(opt.out_path)`（`:215-221`，注释即「失败不落盘铁律（C1 §3）」）；正常写入走 `write_file_atomic()`（临时文件 + `fs::rename`，`:103-116`）。
- `--out` 已存在同名文件时只在成功时覆盖（同一 `fail()` 路径保障）。

**② 编排级（staging + manifest）**
- 所有中间产物在 `filesDir/patch/work-<timestamp>/`；**绝不原地修改用户选的镜像**（旧链 `kptools` 就地改写工作副本，管理器侧必须复制）。
- 每步前后写 `manifest.json`（`step`、`startedAt`、`rc`、`inputSha256`、`outputSha256`）；失败时 workdir **保留**并暴露「导出诊断包」（zip：manifest + 引擎 report + 各步 stderr + 内核版本信息）。
- 成功时：产物移到 `filesDir/patch/out/`，workdir 清理（保留 manifest 摘要 30 天）。

**③ 刷机级（变砖防线，沿用旧链已验证机制——只复用「机制」，不复用其「判定」）**
- **原厂备份改为内容寻址（采纳 KSU 做法）**：刷入前 `sha1(stock boot) → /data/adb/sproot/backup/boot-<sha1>.img`，并把 sha1 记入 manifest 与（可选）ramdisk 内 `stock_image.sha1`；还原时先校验「备份确属本机当前 stock」再写回，避免多次修补互相覆盖（单份 `ori.img` 作为兼容保留）。
- A/B 设备走备槽：`bootctl set-active-boot-slot` 切备槽 + 写 `/data/adb/post-fs-data.d/post_ota.sh`，开机成功后 `bootctl mark-boot-successful` 并自删；失败重启自动回旧槽（APL §2.3-3 已验证的机制，直接复用）。
- ⚠️ **不复用旧链的成功判定**：旧链 `boot_patch.sh:103-117` 的 `-b || -c && -f` 优先级坑 + else 分支静默跳过仍打印 "Successfully Flashed!"（RC F7）。新链刷入判据 = **回读 sha256 一致**（§5.4 步 8）。
- 非 A/B 设备：**默认只导出镜像不直刷**，UI 明确提示「请联系 PC/fastboot 刷入或确认设备可回滚」，避免一次性变砖。
- **AVB 失败回滚（lead 确认 AVB 存在）**：刷 patched boot 前后若设备拒绝启动（bootloader 校验 vbmeta），回滚三选一：①`fastboot --disable-verity --disable-verification flash vbmeta vbmeta.img` 后重刷；②刷回 sha1 备份的原厂 boot（`UNPATCH`）；③官方固件包 `flash.bat` 全量回原厂（最彻底，清数据）。三条均写入 UI「救援指南」页（可复制指令）。
- 刷入前二次确认：显示「当前槽位 / 目标分区 / 镜像 sha256 前 16 位 / 备份路径（sha1 索引）」。

### 5.7 错误可见性铁律（对 RC 已确证根因的逐条反制）

**旧链失败机理（已确证，证据全部引自 RC；本文只做架构结论，不重复取证）**

| RC 编号 | 事实（已证实） | 直接后果 |
|---|---|---|
| **P0-a**（主因） | `boot_patch.sh` 调用 `./skroot-patch`，但该文件**全仓 0 命中**：`libs/arm64-v8a/.gitignore` 未忽略它（说明不是被忽略的构建产物，而是从未产出）、`jniLibs/` 目录不存在、`build.gradle.kts` 无任何拷贝 `engine/src/skroot-patch` 的任务 | `prepare()` 的 `listFiles{execs.contains}` **静默跳过**缺件（`:97-104`）；脚本 `:76-79` `if [ ! -f ./skroot-patch ]; then ... exit 1` → 秒退 |
| **P0-b** | `libkptools.so` 靠构建期 `downloadKptools` 从 GitHub Releases 下载；下载失败任务被 skip 而构建仍绿 | `boot_patch.sh:42` `command -v ./kptools || exit 1` → 输出更短（仅 3 行 banner） |
| **P0-c** | `SKKeyProvider.currentKey()` 在 `!BuildConfig.DEBUG` 时直接 `return null` | **release 包永远无法修补**（且无终端输出） |
| **P1**（"一闪"的机制） | 失败分支只写 `error=" Patch failed."`，真 stderr 被注释（`PatchesViewModel.kt:477-485`）；`patchdone` 不在失败分支置位 → UI 条件 `patching \|\| patchdone`（`Patches.kt:261`）为假 → **整块日志从 Compose 树移除**；`patchLog` 仅存内存、无落盘 | 用户只看到一闪 + 一句无信息量的错误 |
| **P2** | 脚本 `echo ...; exit $?`（`$?` 是 echo 的 0）、abort 用 `exit 0`、KALLSYMS_ALL 检查覆盖了 repack 的 rc | **失败可能被报成成功**；repack 失败测不到 |
| **P3** | `prepare()` 从 assets 读 `kpimg`（`PatchesViewModel.kt:107-112`），但 assets 无该文件（靠 `downloadKpimg`） | `FileNotFoundException` → `ensurePrepared()` 捕获后仅 `error="prepare failed: ..."`，终端无输出 |

> 结论：**根因是「交付物缺失 + 错误被吞」的组合，而不是 SK 引擎本身不可用**。因此新方案对引擎不做任何机制改动，只在「交付、自检、可见性」三处设硬约束。

**新方案六条铁律（每条对应一个已确证失败模式）**

| # | 铁律 | 对应 RC |
|---|---|---|
| L1 | **APK 二进制资产清单 + 构建期断言**：`binaries.json` 声明每个 native 资产的 `name/version/sha256/e_machine`；Gradle 任务在 `assemble` 前用 `readelf` 断言 `AArch64` 且文件存在，失败即 **build fail**（禁止 skip） | F2、F3、P0-a |
| L2 | **禁止运行期/构建期把网络下载当交付**：所有入包可执行件要么入库、要么由本仓 CI 构建；下载只允许作为「带 sha256 校验且失败即 fail build」的辅助 | F3、P0-b |
| L3 | **入口能力自检，缺件即阻断**：点击修补的第一件事是 §5.2.1 自检（存在/架构/摘要/可执行/JNI 加载），失败给出「缺什么、期望在哪、怎么办」，**不进流程、不落任何产物** | F1、P0-a、P0-b |
| L4 | **非必需资产解耦**：`kpimg` 这类「仅用于展示元信息」的资产不入包、不参与流程；SK 链不需要它（新 `boot_patch.sh` 已无 `-k kpimg`） | F4、P3 |
| L5 | **日志即产物 + 面板常驻**：日志写文件（非内存）、stdout/stderr 分列、失败后 UI 停留在错误行、可复制/可导出；错误面板由**状态机**驱动（`FAILED` 态常驻），**不依赖 `patching/patchdone` 这类瞬态布尔** | F5、P1、F11 |
| L6 | **成功必须可验证**：只认 §5.4 步 6/7 的断言（等长、密钥内嵌、hook 表、`ANDROID!` 魔数、刷入后回读 sha256），**不认**任何「脚本打印 Successfully」；同时全链不使用 shell 脚本（消除 `exit $?` 吞码） | F6、F7、P2 |

**补充硬约束（来自 lead 2026-11-08 全员通报，已并入本文）**

1. 架构必须规定「APK 内二进制资产清单 + 构建期断言」→ L1/L2（§5.2 实现要点 5）。
2. 修补执行层定为 aarch64-android `.so` + JNI **或**严格校验的 `nativeLibraryDir` 可执行 → §5.2 双形态。
3. 每步能力自检与错误可见性 → §5.2.1 + §5.4 步 1 + L3/L5。
4. 10 条规避项（RC §三 F1–F11）逐条落位见 §7-RC 对照表。

### 5.8 修补状态机（UI 与编排共用）

```
IDLE ──选镜像──> INPUT_READY ──开始──> PREFLIGHT ─> UNPACKING ─> INSPECTING
                                                              │
                          ┌───────────────────────────────────┘
                          ▼
                      PATCHING ──失败──> FAILED(保留workdir/可重试/可导出诊断)
                          │ 成功
                          ▼
                      VERIFYING ──失败──> FAILED
                          │ 成功
                          ▼
                      REPACKING ──> READY_TO_DELIVER ──导出──> EXPORTED
                                                    └─刷入──> FLASHED(需重启)
任意态 ──取消──> CANCELLED(清理workdir，无产物)
```

状态持久化到 `filesDir/patch/state.json`：进程被杀后重进 App 可恢复「上次失败点 + 日志」，满足「错误不断链」。

---

## 6. KSU / AP 对照：直接借鉴什么、必须改什么

### 6.1 借鉴清单（可直接抄的架构决策）

| # | 借鉴项 | 来源证据 | 在 SPRoot 的落地 |
|---|---|---|---|
| B1 | 管理器是**纯用户态 App**，修补在机内完成（选 boot.img → 拆 → patch → 重打 → 导出/刷入） | APL §2.1（AP 全链路）；KSU 同为机内修补形态（细节以 `docs/research/kernelsu-architecture.md` 为准） | §5.4 七步链 |
| B2 | **native 可执行文件藏进 jniLibs**（`lib*.so` 实为 ELF），运行期用 `nativeLibraryDir` 绝对路径 exec | APL §2.2 层 2；`PatchesViewModel.kt:86-113`；**KSU 同款**（`libksud.so`，KSU 调研 §5.1-①） | §5.2 落地步骤 |
| B9 | **全参数化修补命令 + 内容寻址备份 + type-safe 路由** | KSU 调研 §5.1-②④⑤（`--partition/--backup/--no-install`；`ksu_backup_<sha1>`；`NavKey` 路由） | §5.4（分区显式/自动备份）、§5.6-③（sha1 备份）、§4.4（miuix-nav 路由） |
| B3 | **修补产物与原厂备份分离**，卸载可还原 | APL §2.3-2（`ori.img`） | §5.6-③ |
| B4 | **A/B 备槽安装 + `mark-boot-successful` 自删** | APL §2.3-3（`post_ota.sh`+`bootctl`） | §5.6-③ 原样复用 |
| B5 | **模块目录模型**（`/data/adb/…` + `module.prop` + 启停标记文件） | APL §4.1（KPM 用 `<id>/disable`）；KSU/AP 社区通用模块模型 | 模块页保持该模型，不自创 |
| B6 | **密钥加密存储**（Keystore AES/GCM + SP） | APL §3.2（`APatchKeyHelper`） | `SukeyProvider` 同构，但**存储对象改为 SK 48 位密钥且删除 "su" 回退** |
| B7 | **UI 与业务逻辑分离**（Compose + ViewModel + StateFlow） | APL §2.2 层 1/层 2 | miuix + ViewModel 同构，状态机更严格（§5.8） |
| B8 | **授权列表 + scontext 选择**的 UX 形态 | APL §3.5 | 保留 UX，真值改到 apd 用户态文件（CALL §3.2） |

### 6.2 必须改（不能照搬，否则踩已踩过的坑）

| # | 上游做法 | 为什么不能照搬 | SPRoot 做法 |
|---|---|---|---|
| M1 | 修补链依赖 **KP 内核运行时**（supercall、kpimg、kpm、kstorage、user_event） | SK 内核**零常驻**（A1 §1、BOOT §2）：无 supercall、无事件总线、无 kstorage | 全部改用户态：文件 + apd + execve 提权（CALL §3） |
| M2 | `SuperKey` 可缺失/可回退 `"su"`（隐式授权） | 与 SK「密钥即唯一凭证」根本冲突；上游自己也在去 superkey（APL §2.4） | **删除一切 "su"/legacy 回退**；密钥缺失=未修补态 |
| M3 | 修补密钥由用户手输 8~63 位（APL §3.2） | SK 密钥是 48 位且**前 16 位即隐藏目录名**（A1 §4.2），手输 UX 灾难 | 默认 `--gen-key` 生成、可选「自定义」但强校验 `^[A-Za-z0-9]{48}$`；展示自动分组 + 复制 + 「前 16 位=隐藏目录」说明 |
| M4 | 构建期从 GitHub 下载 kptools/kpimg/kpatch/apd | 断网即缺件、缺件仍能构建成功（§5.7 证据） | 入库 + CI 构建 + sha256 预检（§5.7 铁律 1） |
| M5 | 修补用 shell 脚本（`boot_patch.sh`）当主链 | `set -x` 淹没关键错误；脚本失败对 UI 只呈现一个 rc | Kotlin 编排（每步类型化错误），脚本仅作参考实现 |
| M6 | 复用 20 个 JNI supercall（APL §3.4） | SK 无对应内核接口，且 JNI 让崩溃不可隔离 | 只保留「提权」一条通道（子进程/execve）；授权走 apd 文件 |
| M7 | KPM 嵌入（`-M/-E`）与 KPM 页 | SK 无内核运行时载体（A1 §1、B1 文档结论） | 删除 KPM 能力；模块页只保留 APM |
| M8 | 修补 UI 只展示一块滚动日志文本 | 旧路线"闪退感"的直接来源 | 七步状态灯 + 常驻错误面板 + 日志落盘（§5.7） |
| M9 | 修补写回原文件（上游引擎原地写） | 破坏原厂镜像、无法二次尝试 | 引擎 `--out` 显式输出（C1 §1）；作业副本隔离（§5.6-②） |
| M10 | **shell 脚本 + busybox 当主链**（`boot_patch.sh`） | RC P2：`echo; exit $?` 吞退出码、abort 用 `exit 0`、检查顺序覆盖 rc——脚本层是错误失真的放大器 | **彻底去脚本层**：Kotlin 直接调工具（§5.4 总原则），不打包 busybox（RC §四） |
| M11 | **能力被 `BuildConfig.DEBUG` 绑死**（`SKKeyProvider.kt:26` release 恒 null） | RC P0-c：release 包永不修补且无提示 | 密钥路径与构建类型解耦；CI 断言 release 包可完成一次修补 |
| M12 | **只靠「脚本打印」判定成功** | RC F7：未校验产物就打印 "Successfully"；块设备判断优先级坑 | 成功判据全部改为可验证事实（§5.4 步 6/8） |
| M13 | **无交互预授权模型**（KSU：未授权 su 直接拒绝，用户须手动翻列表开开关） | 体验问题真实存在，但 SK 零常驻内核**无法**提供 pending 队列 | 接受同款限制，并**补偿**：管理器「待授权应用」引导页（扫描近期 su 失败审计 `sulog` 等价物）+ 一键授权；文案明确「不会弹窗」 |
| M14 | **把 `nativeLibraryDir` 当守护进程路径**（KSU `getKsuDaemonPath()` 脆弱：APK 升级后路径失效、清数据后静默降级） | 与 B2 的 exec 形态同源，但**持久进程**不能这样 | 一次性工具（引擎/工具）才走 nativeLibraryDir；**持久守护固定 `/data/adb/apd`**（DOC-05 §3.2）；管理器官网建模 bundled/installed/stale 三态并在 UI 暴露 |

### 6.3 KSU 专项对照（已并入 `docs/research/kernelsu-architecture.md`，基线 `730f6446`）

| 维度 | KernelSU（实测） | AP（本地实证） | SPRoot 决策 |
|---|---|---|---|
| 管理器栈 | Kotlin 2.4.20 + Compose BOM `2026.09.00` + Material3 `1.5.0-alpha28` + **Miuix 0.9.4** + `miuix-nav`；compileSdk/targetSdk 37，**minSdk 31** | Kotlin + Compose Material3 | **Kotlin 2.4.20 + Compose BOM 2026.08.00 + miuix 0.9.4 + miuix-nav**，minSdk 26，material3 保持 1.4.0（exclude，§4.2/§4.6-U2） |
| 特权执行者 | `libksud.so`（Rust CLI）经 `nativeLibraryDir` 的绝对路径 **exec**，argv+stdout/stderr 通信 | `apd`（Rust）+ JNI supercall | **双形态**：引擎走 `libskrootpatch.so` exec（与 KSU 同构，§5.2-形态A）；持久特权走 `/data/adb/apd`（稳定路径，§3.5） |
| boot.img 解包/回包 | **纯 Rust 自研**（`android-bootimg` crate），APK 里**没有** magiskboot/kptools | 外挂 kptools（构建期下载） | **引擎自研（C++/JNI，lead 已裁）**，无第三方 ELF（§5.3） |
| 修补命令 | 单命令全参数化：`boot-patch -b/-o/--kmi/--partition/-m/-u/-f/--backup/--no-install` | shell 脚本 `boot_patch.sh` | 全参数化由 Kotlin 编排（§5.4）；**采纳 KSU 三个参数语义**：分区显式指定、强制备份、只改配置不重注入（→ §5.4/§8 D-13） |
| root 授权 | **无交互预授权**：`/data/adb/ksu/.allowlist`（内核持久化，二进制 magic）；未授权 su 直接拒绝、只记 sulog | 内核 kstorage + JNI 授权 | **用户态 allow_list（E4）+ socket SO_PEERCRED**；同样**无交互弹窗**（SK 零常驻内核，无 pending 队列）→ 记为已接受的 UX 限制（§6.3 注） |
| 管理器自身 root | `ksud debug su`（libsu `Shell.Builder.build(libksud.so,"debug","su")`） | `truncate` supercall shell | **`apd root-exec`**（唯一入口，§3.5.2） |
| 模块 | `/data/adb/modules` + module.prop + installer.sh（Magisk 语义） | `/data/adb/ap/` + apd | 沿用 apd（DOC-05），语义与 KSU/Magisk 同构 |
| 备份 | `do_backup` 按 sha1 存 `/data/adb/ksu/ksu_backup_<sha1>` + ramdisk 内 `stock_image.sha1` 指纹 | 单份 `ori.img` | **采纳内容寻址备份**（§5.6-③ 修订） |
| 二期能力 | `boot-patch-v2`（LKM 注入 kernel image，116KB，强绑 KMI，**管理器未接**） | — | **明确不做**（首版锁定 ramdisk/内核字节 patch 方案） |
| 对本项目可借鉴 | 薄 UI + 厚 CLI 进程边界、全参数化修补命令、自研 bootimg、内容寻址备份、type-safe 路由 + 双皮 | 修补链工程细节、备份/槽位机制 | §6.1（B1–B9） |
| 必须避开/改造 | 无交互预授权体验、把 `nativeLibraryDir` 当守护路径（APK 升级后路径失效） | — | §6.2（M13/M14） |

**KSU 对本文两个决策的直接影响**

1. **形态 A（子进程 exec）获得业界实证**：KSU 管理器同样把特权 CLI 打成 `libksud.so`、从 `nativeLibraryDir` 绝对路径 exec（RC F9 的「私有目录禁 exec」并不阻止该路径）。→ §5.2 保留 A 为默认，B 仍作为兜底。
2. **「自研 bootimg」可行性获得实证**：KSU 用纯 Rust `android-bootimg` 完成 parse→cpio→repack→写分区，并给出两个工程要点：`mmap` 分区直读；回包后 `drop` mmap 再写（规避部分内核拒绝写已映射块设备）。→ 写入 D-2 备选③；若 D-3 判定闭源，这是唯一合规路径。

> **关于授权 UX 的诚实说明**：SK 内核零常驻、无 pending 队列，因此**无法实现 Magisk 式「应用弹框请求 → 用户实时批准」**。SPRoot 的形态是「预授权名单 + 管理器为设备所有者」，应用首次 `su` 会被静默拒绝，用户需到管理器里打开开关。这与 KSU 的用户体验等价，属**已接受的设计限制**，需在 UI 文案与测试用例中明确（→ task-7）。

### 6.4 与旧契约 v1.0 的差异（E1–E5 勘误对照，供会签追溯）

> 依据：DOC-05 §4.6（sys-guard 起草）+ lead 2026-11-08 决议「五处勘误已采纳，契约升 v1.1」。本文按新口径书写，旧契约 v1.0 相关段落视为**待修订**（task-9 起草中）。

| 编号 | 契约 v1.0 原文口径 | 引擎/现实事实 | **本文采用口径（v1.1）** | 本文落位 | 对实现的影响 |
|---|---|---|---|---|---|
| **E1** | §3.1「密钥字符串本身是 SK 隐藏目录下的可执行路径（`/data/<key16>/su`）」 | `engine/src/patch_do_execve.cpp:76-98`：shellcode 对 filename **逐字节 strcmp 内嵌 47 字符密钥**；`skroot_patch_main.cpp:312-317` 落盘写 `key[0:47]+NUL` | **提权串 = 47 字符密钥本体**（`[A-Za-z0-9]`，无斜杠）；`/data/<key16>/…` 只是 su 部署位置 | §3.5.2、§5.2 | 若照旧原文实现**必然失配**（多一字符/多斜杠）→ 已按新口径写 |
| **E2** | §3.4 部署形态：隐藏目录下**固定名 `su`**，「用户/脚本执行即命中 argv[0] 分发」 | A1 §4.4：上游 kit 用**随机名**（`lib`+随机字母+`.so` 风格）；固定名可被 8 字符文件名扫描 | `/data/<key16>/<随机16字符>`（AP multicall 副本，0700）+ `/data/adb/ap/bin/su` 符号链接；**直连 su 仅限 root 上下文**，第三方授权走 socket | §3.5.1/§3.5.2、§3.3-B | 管理器**不能硬编码 su 路径**；启动后从 `apd version --json` / state 读取实际路径 |
| **E3** | §3.2 + BOOT §3.6：`SP_REQUEST_UID` 由 SK do_execve hook 在 cred 改写时注入环境变量，供 AP 还原请求方 uid | **不可实施**：hook 在 do_execve 入口、任何 exec 语义之前，无法构造新 envp；且提权后 `getuid()==0`，原 uid 不可恢复；引擎无相关代码 | 废弃 `SP_REQUEST_UID`；第三方身份用**抽象 socket `@sproot-<12hex>` + `SO_PEERCRED`**（内核填写，不可伪造）；管理器自身不查表（设备所有者等价） | §3.5.1、§3.5.2 | `apd.rs:196-205` 的 env 读取删除；管理器不需要「请求方 uid」概念 |
| **E4** | §3.2 授权表=`allow_list` csv（含 exclude 列）+ §3.6 另有独立 `exclude_list`（每行 uid） | 现实现 exclude 在 csv 行内（`PackageConfig.exclude`），`exclude_list` 未实现；两份存储=双写不一致 | **只保留单一 `/data/adb/ap/allow_list`**（csv 六列 `pkg,exclude,allow,uid,to_uid,sctx`），删除 `exclude_list` 独立文件口径 | §3.5.1、§3.2（AllowListRepository） | 管理器 UI 的「排除」= 同一文件内的 exclude 列，无第二个存储 |
| **E5** | BOOT §4：rc 注入 / `install_su` / key 文件由独立部署器 `skroot-deploy` 交付 | 新架构「手机本地修补」：管理器在修补期即可写 rc；无需独立部署器进程 | **取消独立部署器**：rc 注入 → 管理器修补流程步 7；`install_su`/key 文件/目录 → `apd deploy`（T2） | §3.1（组件图无部署器）、§3.5.3、§5.4 步 7 | 组件数 -1；管理器与 AP 之间多一条 `deploy` 接口；`skroot-deploy` 从交付物清单移除 |

**未偏差项（与契约 v1.0 保持一致，本文逐条遵循）**：密钥方案 B 无状态传参、失败静默/无内核痕迹、日志脱敏、错误码禁令（AP 用户态接口）、`kp` 别名删除、安全模式三路、busybox 首装归属、`argv[0]` basename 分发。

> **会签追溯**：本表与 DOC-05 §4.6 同源；契约 v1.1 定稿后，本文 §6.4 转为「历史差异记录」，并以 v1.1 为唯一基线。

---

## 7. 关键风险与缓解（按影响排序）

| # | 风险 | 触发条件 | 影响 | 缓解 | 归属 |
|---|---|---|---|---|---|
| R1 | **引擎 arm64 交叉编译失败/体积超标**（asmjit+capstone 依赖） | NDK 工具链不兼容、asmjit 在 arm64 主机异常 | 无引擎可用=项目阻塞 | task-3 先出可复现编译命令与产物校验；失败即回退「引擎用 x86 预构建只做离线验证 + 手机侧另寻实现」 | task-3 |
| R2 | **boot.img/AVB 格式覆盖不足**（boot header v4、vendor_boot、`init_boot`、异形 ramdisk、厂商校验） | 目标机型（魅族 21 等）分区/AVB 布局非标准 | 解包失败或刷入后不开机 | 自研 bootimg 按 AOSP 规范实现 + 首批机型真机矩阵；AVB 三路径（§5.4.1）明确展示；失败有 sha1 备份回滚 | 架构/task-3/qa |
| R3 | **自研 bootimg 的工程量与正确性**（替代成熟 magiskboot/kptools 的风险） | 自研解析/回包有 bug | 产物结构损坏 → 不开机 | ①以 `upstream/KernelPatch/tools/bootimg.c`（GPL-2.0 参考）做对照测试；②单元测试覆盖 header v0–v4 样本；③「解包→原样回包」应先字节一致（除对齐填充）作为回归基线；④真机小范围先验证 | task-3/qa |
| R4 | **首装鸡生蛋**：刷入需要 root，而 root 来自刷入 | 新用户首次安装 | 无法完成闭环 | 三条降级路径：①有 PC 时 fastboot 刷导出的 img（不作为主推但保留）②已有 Magisk/其他 root 时直接 dd ③A/B 备槽 + 回滚（§5.6-③）；UI 明确「本 App 不联网、不代刷，需你确认设备可回滚」 | 架构 |
| R5 | **miuix 实验性 API 变更** | 升级 miuix | UI 返工 | 版本钉死 + design system 门面（§4.6-R1） | task-4 |
| R6 | **工具链新版本兼容**（AGP 9.5.1 / Kotlin 2.4.21 / compileSdk 37） | CI 环境 | 构建红 | CI 冒烟先行（task-8）；Gradle/AGP/JDK 成对锁定 | task-8 |
| R7 | **密钥安全**：日志/备份/崩溃上报泄露 48 位密钥 | 任何字符串拼接失误 | 权限被他人获得 | 密钥只在 `SukeyProvider` 与引擎 argv 出现（argv 在 /proc 可见——**已知弱点**，见 D-4）；日志脱敏白名单；卸载清 Keystore | 架构/task-4 |
| R8 | **CMP 与 AndroidX Compose 混用** | 引入第三方 Compose 库 | 编译/运行时崩 | §4.2 约束 + CI 依赖审查 | task-4/task-8 |
| R9 | **内核支持域窄**（SK 依赖 kallsyms/CFI/华为 HKIP 绕过） | 冷门内核 | 大量机型不支持 | 引擎退出码→人话映射 + 「机型不支持」诚实提示 + 支持矩阵文档 | task-3/qa |
| R10 | **旧路线失败根因已确证**（RC）：引擎从未入包 + 错误被吞 | 已发生 | 项目一度误判「SK 机制不可行」 | 本文 §5.7 六条铁律逐条反制；RC §三 F1–F11 逐项落位见表 7.1 | ✅ 已闭环（本文） |
| R11 | **kernel 压缩格式导致重打包尺寸/偏移变化**（`Image.gz`/`Image.lz4`） | 解包产物 `kernel` 是压缩态，而引擎只吃**已解压裸 Image** | patch 后体积变化 → 重打包偏移/校验和变化 → 刷入后不开机 | §5.4 步 3 显式「解压归一 + 记录原压缩参数」；repack 后做「大小 ≤ 分区、头魔数、sha256 记录」三重断言；首批真机各机型验证（→ D-10） | task-3/qa |
| R12 | **子进程 exec 受限**（Android 10+ 私有目录禁 exec） | **已基本消解**：引擎走 JNI、无第三方工具；仅「诊断模式」需 exec | 诊断模式不可用（不影响主路径） | 诊断模式失败仅记录；主路径 JNI（§5.2） | task-3 |
| R13 | **能力被 `BuildConfig.DEBUG` 绑死**（RC P0-c：release 密钥恒 null） | 任何「debug 才启用」的开关 | release 包功能缺失且静默 | `SukeyProvider` 不读 `BuildConfig.DEBUG`；CI 增加 release 包端到端 case（RC §5.2 断言 6） | task-4/task-8 |
| R14 | **UI 日志随页面/状态卸载**（RC P1 的机制） | 日志状态用瞬态布尔驱动 | 用户看不到错误 | §5.7-L5：状态机 `FAILED` 常驻 + 日志落盘 + 独立 LogViewer | task-4 |
| R15 | **`init_boot` 与 `boot` 分区拆分**（Android 13+ ramdisk 迁到 init_boot） | 目标机型为 A13+ | 选错镜像 → rc 注入到没有 ramdisk 的 boot.img，开机不拉起 AP（但内核已 patch 成功，root 可用、模块不可用） | 步 2 记录分区来源与 `ro.build.version.sdk`；步 7 断言「unpack 出的确有 ramdisk.cpio」否则给 `E-RC-18` 并建议改用 init_boot；UI 双镜像引导（→ D-10/D-12） | task-3/task-5/qa |
| R16 | **rc 注入厂商兼容性**（`init.rc` 结构非标准、`import` 位置、second_stage 校验） | 厂商定制 ROM（魅族 21 等） | 开机不执行 AP → 模块能力全失效 | 注入点=文件尾部（最小侵入）+ 原文件入 manifest；失败可「跳过注入」；真机矩阵优先魅族 21（DOC-05 R1 同源风险） | task-5/qa |
| R17 | **AVB 与「无 PC」目标的张力**：主刷入路径需 `fastboot ... flash vbmeta`（PC） | 首次安装 | 「手机本地」只能覆盖修补，不能覆盖首次刷入 | 明确产品口径（D-15）：修补 100% 本地；首次刷入接受「一次性 PC/fastboot」；App 内提供可复制指令 + 校验（sha256/等长） | leader/人类 |
| R19 | **CI 唯一构建源带来的反馈延迟** | 本地禁止编译（人类硬规则） | 每个改动要等 CI 反馈，AI 无法本地验证 | §8：CI 分 job + 失败即早停；本地仅跑静态检查（`ktlint`/`cargo check` 不允许产出 APK）；每轮改动附「CI 预期结果」清单 | task-8 |
| R20 | **Keystore/密码入私仓的安全边界** | 方案 A（人类批准） | 仓库泄露即签名权泄露 | 仅 private 仓库；`keystore.properties` 不入公开镜像；CI secrets 仅 release job 使用；轮换预案 | 人类/task-8 |
| R18 | **管理器误当 root 提供者**（绕过 root-exec 自行提权） | 开发者图省事 | 密钥暴露面扩散、审计缺失 | 代码评审门禁：`grep -r "execve\|Runtime.exec(\"su\")\|ProcessBuilder(\"su\")"` 在 newapp 中必须 0 命中（除 root-exec 封装外） | 架构/task-4 |

### 7.1 RC §三 十项规避清单（F1–F11）落位对照

| RC | 规避要求 | 本文落位 |
|---|---|---|
| F1 | 缺件被静默跳过 → 需入口能力自检（存在/可执行/架构） | §5.2.1、§5.4 步 1、L1/L3 |
| F2 | 引擎交付形态与契约脱节 → 定为 JNI 主路径 + CI 断言架构 | §5.2（JNI 为主）、§5.2 实现要点 6、L1 |
| F9 | 私有目录 exec 被拦 → 引擎走 JNI、无第三方工具；仅诊断模式需 exec 目标在 nativeLibraryDir | §5.2 资产表、R12 |
| F3 | 构建期网络下载当交付 → 禁 skip-on-failure | §5.7-L2、R3 |
| F4 | 非必需资产（kpimg）硬依赖 → 降级为 optional | §5.7-L4 |
| F5 | 失败无日志落盘、面板卸载 → 日志即产物 | §5.4「日志落盘要求」、L5、R14 |
| F6 | `echo; exit $?` 吞码、abort 用 exit 0 → 去脚本层 | §5.4「shell 与 busybox 处置」、L6 |
| F7 | 未校验产物就宣布成功 → 成功判据=可验证事实 | §5.4 步 6/8、§5.6-③ 注、L6 |
| F8 | 能力被 DEBUG 绑死 → release 可用 | R13 |
| F9 | `filesDir` 内 exec 被拦 → exec 目标须在 `nativeLibraryDir` | §5.2 形态 A 实现要点 2、R12 |
| F9 | `filesDir` 内 exec 被拦 → exec 目标须在 `nativeLibraryDir` | §5.2 形态 A 实现要点 2、R12 |
| F10 | 壳层传参歧义（`sh -c` vs 直传）→ 单一调用路径 | §5.2 实现要点 3、§5.4 总原则 |
| F11 | 日志/环境不可复现 → 记录 cmdline/env/cwd、stderr 分列 | §5.4「日志落盘要求」 |

---

## 8. 待决项（需 leader/人类裁决或专项调研）

| ID | 事项 | 选项 | 建议 | 裁决人 | 阻塞谁 |
|---|---|---|---|---|---|
| D-1 | 新工程 `compileSdk` 下限 | 37 / 更低 | **37**（miuix `minCompileSdk=37` 硬要求）；**已随 app-client 定版落定**（AGP 9.4.1 / Kotlin 2.4.20 / compileSdk 37 / minSdk 26 / Gradle 9.7.1 / JDK 21） | ✅ 已定 | — |
| D-2 | magiskboot 引入与许可证 | magiskboot 主选（GPL-3.0）+ kptools 辅（GPL-2.0）/ 仅 kptools + 自研 cpio | **magiskboot 主选**（E5 要求 rc 注入，cpio 编辑不可绕过）；前提是 D-3 判为开源可分发 | leader + 架构 | task-3/task-5 |
| D-3 | 项目整体许可证策略 | 开源（GPL 兼容）/ 闭源分发 | 若闭源，则**禁止**打包 GPL 二进制 → 必须自研 bootimg/cpio（工量↑），且 kptools 亦受限；**建议开源托管** | 人类 | 全部打包工作 |
| D-4 | ~~密钥经 `argv` 传给引擎~~ **已解决** | — | 改为「临时 0600 文件 + 用后即删」（DOC-05 §4.2）；`root-exec --superkey-file` 同样走文件；**密钥不进 argv** | ✅ 架构已定 | — |
| D-5 | 首装刷入路径的产品口径 | 仅导出镜像 / 支持直刷（经 root-exec，需已有 root/AP）/ 引导 fastboot | **导出为默认，直刷为高级选项**（风险与体验平衡） | leader + 人类 | task-5 |
| D-6 | 目标机型首批清单 | 魅族 21（已知）+ 其他 | 需人类给首批 3~5 台；否则 R15/R16 无法收敛 | 人类 | qa/task-3 |
| D-7 | 模块页 v0.1 是否保留 | 保留（AP 子命令透视）/ 先只读状态 | **保留只读 + 安装/启停**，卸载与动作脚本下版 | leader | task-4/task-6 |
| D-8 | miuix 组件 API 细节 | 调研文件/自测 | 以 `docs/research/miuix-integration.md` + 首次真机截图回归为准 | research/task-4 | task-4 |
| D-9 | ~~引擎最终形态裁定~~ **已定（lead 2026-11-08）** | JNI 进程内为主，子进程为诊断/回退 | **落定**：`libskrootpatch.so` JNI 主路径；CLI `main()` 保留供诊断/PC/CI（§5.2） | ✅ lead 已裁 | task-3 按此实现 |
| D-10 | kernel 压缩态处理 + 镜像来源 | ①解压后 patch 再按原参数压回（默认）②要求用户提供已解压镜像 ③引擎扩展支持压缩态 | **①**；同时需确认 magiskboot unpack 是否已解压 kernel；首批机型验证重打包后尺寸 | 架构 + task-3 | task-3/task-5 |
| D-11 | 是否保留「自定义 48 位密钥」入口 | 只自动生成 / 允许用户输入（强校验） | **允许输入但默认生成**（密钥即隐藏目录，用户自定义可能撞弱口令 → 加熵提示） | leader | task-4 |
| D-12 | `init_boot` 场景的镜像引导 | 只接受 boot.img / 同时支持 boot.img 与 init_boot.img（A13+）/ 自动探测 | **自动探测 + 明确提示**：A13+ 机型 ramdisk 在 `init_boot`，需用户提供该分区镜像（否则 rc 注入失败、模块不可用）；内核 patch 仍在 boot.img | leader + task-5 | task-5/qa/task-7 |

---

## 9. 交付拆解与依赖关系（与任务板对齐）

| 任务 | 与本方案的关系 | 关键依赖 |
|---|---|---|
| task-1（本文） | 架构基线 | — |
| task-2 | 根因已确证并已并入本文（§5.7/§7.1/A-18~A-22）；剩余：给最小可验证链 | ✅ 已消费其结论 |
| task-3 | 落地 §5.2/§5.3 的二进制产物（引擎 + magiskboot + kptools）+ `--selftest` + `engine_run()` 双入口 | 依赖 D-2/D-3/D-9/D-10 |
| task-4 | 落地 §4（miuix）与 §3.1 L1/L2 骨架（含常驻错误面板/日志页/部署向导） | 依赖 D-1、§4.2 依赖矩阵、R13/R14/R18 |
| task-5 | 落地 §5.4 八步链（含 rc 注入）+ §3.5.3 首装三段式 | 依赖 task-3 产物 + D-5/D-12 |
| task-6 | 落地 §3.1 L4（本文已消费其 NEW-DOC-05 结论，含 E1–E5） | ✅ 已对齐（DOC-05 v1.0 草案） |
| task-7 | 把 §5.6/§5.7/§7.1/§5.8/§3.5 转成测试项 | 本文 + RC §5.2 断言表 + DOC-05 §9 |
| task-8 | CI：依赖解析冒烟 + 引擎/magiskboot 构建 + **架构/断网/入包断言** + APK 签名 | §4.6-R3、§5.7-L1/L2、A-22 |
| task-9（新） | 契约 v1.1 正文（E1–E5 修订） | 本文 §6.4 作为差异依据；与 DOC-05 §4.6 同源 |

**建议的里程碑顺序**：M1 = 工程能编译出 miuix 一屏（task-4+task-8）→ M2 = 引擎/magiskboot arm64 产物 + 静态校验（task-3）→ M3 = 八步链打通（task-5）→ M4 = 真机刷入/回滚（task-5+qa）→ M5 = 首次部署 T2 + 授权与模块（task-5+task-6）。

---

## 附录 A · 证据索引

| 编号 | 证据 | 获取方式/路径 |
|---|---|---|
| A-1 | miuix README：Maven 坐标、模块清单、`MiuixTheme`/`ThemeController` 用法、Kotlin 2.4.21 + CMP 1.12.2 badge、experimental 声明 | <https://raw.githubusercontent.com/compose-miuix-ui/miuix/main/README.md> |
| A-2 | Maven Central 版本元数据：`<release>0.9.4</release>`，`lastUpdated=20260920082405` | <https://repo1.maven.org/maven2/top/yukonga/miuix/kmp/miuix-ui/maven-metadata.xml> |
| A-3 | miuix 构建常量：`LIBRARY_VERSION="0.9.4"`、`MIN_SDK=24`、`COMPILE_SDK=37`、`TARGET_SDK=37`、`BUILD_TOOLS_VERSION="37.0.0"`、`JDK_VERSION=21` | `build-logic/src/main/kotlin/BuildConfig.kt`（raw.githubusercontent 同仓库）；`miuix-ui/build.gradle.kts` 使用 `BuildConfig.MIN_SDK` |
| A-4 | miuix-ui-android POM 依赖：`org.jetbrains.compose.foundation:foundation-android:1.12.0`、`kotlin-stdlib:2.4.20`、`material3-window-size-class:1.12.0-alpha03`、`material-color-utilities:5.0.1`、`navigationevent-compose-android:1.1.2`、`miuix-core/squircle-android:0.9.4` | <https://repo1.maven.org/maven2/top/yukonga/miuix/kmp/miuix-ui-android/0.9.4/miuix-ui-android-0.9.4.pom> |
| A-5 | `org.jetbrains.compose.foundation:foundation-android:1.12.0` POM 编译期依赖 `androidx.compose.foundation:foundation-android:1.12.0`（证明两套 Compose 族并存风险真实存在） | <https://repo1.maven.org/maven2/org/jetbrains/compose/foundation/foundation-android/1.12.0/foundation-android-1.12.0.pom> |
| A-6 | 引擎 CLI 已按 C1 实现：文件头 6 条差异声明 + `enum ExitCode`（0/101/102/201/202/210/211/220/230/240/250）+ `EngineOptions{in_path,out_path,report_path,workdir,json_sink,key}` | `engine/src/skroot_patch_main.cpp:1-48` |
| A-7 | 引擎已产出可运行产物（WSL x86-64）`engine/src/skroot-patch` 2,121,080 B，2026-11-06 22:15 | `Get-ChildItem engine/src/skroot-patch` |
| A-8 | kptools 具备 bootimg 重打包能力 | `upstream/KernelPatch/tools/bootimg.c`、`bootimg.h:108/111`（`repack_bootimg`、`repack_bootimg_mem`） |
| A-9 | 许可证：KernelPatch = GPL-2.0（`upstream/KernelPatch/LICENSE` 首 5 行）；Magisk = GPL-3.0（<https://raw.githubusercontent.com/topjohnwu/Magisk/master/LICENSE> 首 3 行） | 见左 |
| A-10 | 旧链二进制来自构建期下载 | `app/app/build.gradle.kts`：`registerDownloadTask(downloadKpimg/downloadKptools/downloadCompatKpatch)` + `buildApd`（cargo ndk → `libapd.so`）+ `preBuild.dependsOn(...)` |
| A-11 | 本地 libs 缺件实况 | `app/app/libs/arm64-v8a/` 仅 `libbootctl.so`(154,248) + `libbusybox.so`(1,710,600)，**无** `libkptools.so`/`libkpatch.so`/`libapd.so` |
| A-12 | 旧 prepare 的 symlink 清单含缺件 | `app/app/src/main/java/me/bmax/apatch/ui/viewmodel/PatchesViewModel.kt:86-113` |
| A-13 | 旧脚本依赖缺失即秒退 | `app/app/src/main/assets/boot_patch.sh`：`command -v ./kptools >/dev/null 2>&1 || { >&2 echo "- Command kptools not found!"; exit 1; }` |
| A-14 | 备份/备槽/回滚机制 | `docs/AP管理器与镜像修补链分析.md` §2.3（`ori.img`、`libbootctl.so`、`post_ota.sh`、`mark-boot-successful`） |
| A-15 | SK 特性与验收断言基线 | `docs/SPROOT-DOC-A1-*.md` §1/§4/§6/§7/§8；`docs/SPROOT-DOC-C1-*.md` §3/§4 |
| A-16 | 提权判据与授权表用户态化裁决 | `docs/phase2_skroot_ap_call_contract_draft.md` §2.3/§3.1/§3.2 |
| A-17 | rc 注入方案与首装窗口 | `docs/phase2_boot_sequence_analysis_draft.md` §3.2/§3.4/§3.5 |
| A-18 | **旧链失败根因（已确证）**：P0-a 引擎从未入包、P0-b kptools 构建期下载、P0-c release 密钥恒 null、P1 日志面板随失败卸载、P2 退出码被 echo 吞、P3 kpimg 硬依赖；含 F1–F11 规避清单与最小可验证链 | `docs/research/patch-failure-rootcause.md` §一/§二/§三/§四/§五 |
| A-19 | 旧链唯一产物是 x86-64 glibc ELF（`e_type=0x0003 ET_DYN`、`e_machine=0x003e`），且 `engine/src/Makefile` 无 `--target`/`-static`/NDK clang | `docs/research/patch-failure-rootcause.md` §2.3；`engine/src/Makefile:5-6,25,27` |
| A-20 | 引擎拒绝压缩/镜像后缀输入（`.img`/`.gz` → E102） | `docs/research/patch-failure-rootcause.md` §2.3 引 `engine/src/skroot_patch_main.cpp:194-195` |
| A-21 | 「一闪」的代码级机制：`Patches.kt:261` 可见性条件 `patching \|\| patchdone`；失败分支不置 `patchdone`、stderr 被注释 | `docs/research/patch-failure-rootcause.md` §2.4；`app/app/src/main/java/me/bmax/apatch/ui/viewmodel/PatchesViewModel.kt:477-485` |
| A-22 | 构建期 `readelf`/断网构建/删 kpimg/人为 rc=126 等 CI 断言清单 | `docs/research/patch-failure-rootcause.md` §5.2 |
| A-23 | **契约 v1.1 勘误 E1–E5**：提权串=47 字符、su 随机命名、废 `SP_REQUEST_UID` 改 socket+SO_PEERCRED、`exclude_list` 合表、取消独立部署器；含 `apd deploy/root-exec/auth/daemon` 子命令清单与部署清单（9 步）、首装三段式、回滚矩阵 | `docs/NEW-DOC-05-模块与守护方案.md` §3.1/§3.2/§4.1–§4.6/§5.1/§6.1–§6.4/§10（Q1–Q8） |
| A-24 | rc 注入物模板（`init.sproot.rc` 全文 + `import` 追加位置 + 不写 seclabel 的理由 + `class late_start` 选择） | `docs/NEW-DOC-05-模块与守护方案.md` §5.1 |
| A-25 | 引擎侧提权实现事实：do_execve filename 逐字节 strcmp、47+NUL 落盘、失败静默 | `engine/src/patch_do_execve.cpp:76-98`、`engine/src/skroot_patch_main.cpp:312-317`（经 DOC-05 §4.1/附录 A 索引，本文未另行反汇编复核） |
| A-26 | **KSU 架构实测**：管理器薄壳 + `libksud.so`（Rust）exec、无 magiskboot/kptools（自研 Rust `android-bootimg`）、`boot-patch` 全参数化、无交互预授权 allowlist、`ksu_backup_<sha1>` 内容寻址备份、`boot-patch-v2` 未接；含「5 借鉴 + 3 改造」清单 | `docs/research/kernelsu-architecture.md` §0/§2/§3/§5（基线 `730f6446`） |
| A-27 | **契约 v1.1 三方会签 + 两条边界**（修补链/特权链分离；`libskrootpatch.so` 与 `libapd.so` 职责不可互换）；lead 2026-11-08 通报 | lead 团队消息（2026-11-08）——**非文件证据，属决议记录**；契约正文由 task-9 落盘 |
| A-28 | **app-client 工具链定版**：AGP 9.4.1 / Kotlin 2.4.20 / compileSdk 37 / minSdk 26 / Gradle 9.7.1 / JDK 21 | lead 2026-11-08 通报（app-client 已按 miuix 调研结论定版） |

## 附录 B · 与其它任务的边界（避免越界写盘）

- 本文只写 `docs/NEW-DOC-01-*`；**不修改** `engine/`（task-3）、`newapp/`（task-4/5/8）、`docs/NEW-DOC-02/03/05/06`（task-2/3/6/7）。
- 本文对 task-6（apd/守护）只定义**接口边界**（§3.1 L4 + §3.3-B），不定义其实现细节。
- 本文对 task-7（质测）只提交**可测断言**（§5.6/§5.7/§5.8），不写测试用例正文。

---

*完 · 架构（architect） · 2026-11-08 · v0.1 草案（待合并 docs/research/* 三个调研文件后升 v1.0）*
