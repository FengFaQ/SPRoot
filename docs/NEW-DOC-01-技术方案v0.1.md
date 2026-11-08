# 全新 SP 管理器技术方案 v0.1（NEW-DOC-01）

- 文档编号：NEW-DOC-01
- 版本：**v1.1.3（定稿）**（新增 §3.7「客户端已实现能力清单（v2 实况）」+ §8.2 按实况修正为 5 个工作流 + 7 项设计/实现差异）
- 作者：架构（architect）
- 状态：**定稿 v1.1.3｜供实现与质测引用**
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

> **证据约定**：本文所有技术断言均给出「本地文件路径 + 行号」或「URL + 版本」两类证据之一。凡未取得证据者，一律写入 §9 待决项，不作为设计前提。文末附录 A 汇总。

---

## 0. 摘要与结论（先看这 13 条）

| # | 结论 | 依据 |
|---|---|---|
| 1 | **四层架构**成立：UI 层（miuix/Compose）→ 修补执行层（Kotlin 编排 + **JNI 引擎**）→ Root 授权层（47 字符密钥 execve 提权 + 用户态 allow_list + **root-exec 为唯一 root 入口**）→ 模块管理层（AP 守护改造）。层间只用三类接口：引擎 JNI/CLI（C1）、APK↔AP 接口（§3.5）、AP 子命令。**组件总数比旧方案少 1 个：不再有独立部署器**（E5）。 | §3、§3.5 |
| 2 | **miuix 可用且已实证到 Maven 产物级**：坐标 `top.yukonga.miuix.kmp:miuix-ui-android:0.9.4`（仅需 `mavenCentral()`），AAR 元数据 `minCompileSdk=37` / 库 `minSdk=24`；现有 BOM `2026.08.00`（foundation 1.12.0）与 miuix 要求**天然对齐**。 | §4.1、§4.7（E1–E6） |
| 3 | **工程形态：Android 单平台**（非 KMP），动作项只有两个：① `exclude("org.jetbrains.compose.material3:material3-window-size-class")`（否则 material3 在 runtime 被抬到 `1.5.0-alpha22`，编译 1.4.0/运行 alpha）；② Kotlin 升 `2.4.20`。**能力边界**：blur 需 minSdk 33、glass 在 0.9.4 未发布（404）。 | §4.2、§4.6（U1–U3）、E3/E5/E7/E8/E9/E10 |
| 4 | **引擎交付 = JNI 进程内（默认）**：`libskrootpatch.so` 以 JNI 加载进 App 进程执行修补（同库保留 CLI `main()` 作诊断/PC 复用）。**修补链不需要 root**。子进程 exec 形态降为诊断/回退模式。 | §5.2、§5.2.1、A-27 |
| 5 | **修补链与特权链两条独立通道**（lead 决议，契约 v1.1）：**修补链**＝App 进程内（选镜像→解包→JNI 引擎→rc 注入→重打包→导出/写分区），权限=普通 App；**特权链**＝`apd root-exec`（deploy/模块/授权/刷入）→ AP 自提权。**两链不合并、不互代**（§3.1）。 | §3.1、§3.5、§3.6 |
| 6 | **两个 native 资产、职责不可互换**：`libskrootpatch.so`（JNI，修补，无 root）vs `libapd.so`→`/data/adb/apd`（守护/特权，root）。 | §5.2 资产表、§3.5.2 |
| 7 | **打包链 = magiskboot（人类指令）+ 引擎 patch kernel**：`magiskboot unpack` → 引擎 patch kernel 段 → `magiskboot repack`（**原厂镜像作模板**）；自研 bootimg 保留为**交叉校验/回退**实现。magiskboot 取 **Magisk v31.1 ARM64 官方构建**，按「独立程序聚合」处理（APK NOTICE 标注来源 + GPL-3.0 声明，不传染自研代码）；**禁用 AIK（Android.Image.Kitchen）与非官方 Windows 版 magiskboot**。 | §5.3、§5.3.1、A-8、A-36 |
| 8 | **旧路线失败根因已确证（RC）**：`boot_patch.sh` 调 `./skroot-patch`，但该文件**全仓 0 命中、从未被打包**（`lib/arm64-v8a/` 只有 `libbootctl.so`/`libbusybox.so`；jniLibs 目录不存在；`engine/src/skroot-patch` 是 **x86-64 glibc** ELF，`e_machine=0x3e`）；脚本缺失即 `exit 1`；同时 `Patches.kt:261` 的瞬态条件让失败瞬间卸载日志面板、真实 stderr 被注释 → 用户只见「一闪即退」。新方案落实 **二进制资产清单 + 构建期断言 + 运行期自检 + 日志落盘** 四件套。 | §5.7、RC §一/§二、A-10~A-13 |
| 9 | **失败回滚做三级**：①引擎级（非 0 退出绝不产出 `--out`，C1 §3 铁律）；②编排级（staging 工作目录 + manifest 校验，绝不原地覆盖原厂镜像）；③刷机级（内容寻址原厂备份 + A/B 备槽切换 + 开机成功才 `mark-boot-successful`）。 | §5.6、A-14 |
| 10 | **提权/授权/部署按契约 v1.1 五处勘误口径落定（E1–E5）**：提权串=**47 字符密钥本体**（非路径）；su 部署=隐藏目录内**随机 16 字符名**+`/data/adb/ap/bin/su` 符号链接；废弃 `SP_REQUEST_UID`，第三方身份改 **抽象 socket + `SO_PEERCRED`**；`exclude_list` 并入单一 `allow_list`；**取消独立部署器**，部署职责拆分给「管理器（修补期写 rc）+ `apd deploy`（首启 T2）」。 | §3.5、§5.4、§6.4（E1–E5 差异表）、A-23 |
| 11 | **root 入口唯一化**：管理器**所有**需要 root 的动作（部署、模块、授权、刷入镜像）统一走 `apd root-exec --superkey-file <tmp> -- <cmd>`；管理器自身不实现提权逻辑、不长期持有 root shell。 | §3.5.2 |
| 12 | **最大未知不在 UI、在分区与 AVB**：Android 13+ `init_boot`（8MB，ramdisk 形态 → RcInject 优先目标）与 `boot`（96MB）拆分、kernel 压缩与重打包尺寸、**AVB 只保留语义不碰密码学（刷入仅 `fastboot flash boot --slot=all`，禁止 vbmeta/disable-verity/disable-verification）**、引擎 arm64/JNI 交叉编译（task-3）。 | §5.4.1、§7、§9 |
| 13 | **许可与合规**：自有代码定案 **AGPL-3.0-only**（仓根 `LICENSE` 全文 + SPDX 头）；`NOTICE` 覆盖 magiskboot(GPL-3.0)/Apache-2.0/Zlib/BSD-3/MIT；**⚠️ `engine/` 为 SKRoot proprietary 移植，AGPL 整仓发布等于对其再许可 → 授权未明确覆盖 AGPL 再分发时，公开发布前必须剥离 `engine/`**（§11，需人类/律师确认）。 | §11、§5.3.3、A-38~A-41 |

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
| K3 | **自研/内嵌 bootimg 解析而非依赖外部工具** | KSU §2.7、§5.1-③ | **人类指令改为：打包用 magiskboot（官方 ARM64）+ 自研 bootimg 作交叉校验/回退**（§5.3）；禁用 AIK |
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
| B2 | 预编译二进制 + shell 脚本当**引擎**（APM §5.2-②） | 不可审计、文本判定、无结构化错误码、构建期下载 | **引擎 = JNI `libskrootpatch.so` + C1 退出码**；打包工具允许官方 magiskboot（独立程序聚合、sha256 锁定、退出码直传），**禁止 shell 脚本编排** |
| B3 | ProcessBuilder 与 libsu **双路径 + 字符串拼命令**（APM §5.2-③） | 参数转义 bug、语义不一致、排查翻倍 | **单一路径**：JNI（修补）/`root-exec`（特权），不拼 shell 串 |
| B4 | 无交互预授权（KSU §5.2-①） | SK 零常驻无法做 pending 队列 | 接受限制 + 「待授权应用」引导页补偿（§6.3 注） |
| B5 | 把 `nativeLibraryDir` 当**守护进程**固定路径（KSU §5.2-②） | APK 升级即失效 | 只有一次性工具/引擎走该路径；**守护固定 `/data/adb/apd`**（§6.2-M14） |

#### C. 明确禁止（写入纪律）

| # | 禁止项 | 理由/替代 |
|---|---|---|
| X1 | 禁止 superkey/密钥经 argv 或 `truncate` 等通用二进制传递 | 用 `--superkey-file`（临时 0600）+ AP 内部 `execve(key47)` |
| X2 | 禁止「预编译二进制 + **shell 脚本**」作为修补**引擎** | 引擎用 JNI `libskrootpatch.so` + 结构化退出码；**打包工具**允许使用官方 magiskboot 二进制（独立程序聚合，§5.3.3），但禁止用 shell 脚本做编排 |
| X3 | 禁止把 nativeLibraryDir 当守护进程长期路径 | 守护固定 `/data/adb/apd`，管理器建模 bundled/installed/stale |
| X4 | 禁止运行期/构建期网络下载二进制当交付物 | 入库 + CI 构建 + sha256 校验（§5.7-L1/L2） |
| X5 | **禁止本地编译 APK / 镜像**（人类硬规则） | 唯一可信构建源 = GitHub Actions（§8） |
| X6 | 禁止「脚本打印 Successfully」当成功判据 | 只认可验证事实（等长/魔数/回读 sha256，§5.4 步 6/8） |

### 1.4 刷机风险与能力边界（面向用户 / 发布者）

> 写法参考 `Skroot-Next` README 的「能力边界 + 危险警告」体例（简洁、不承诺、把选择权交用户）。本节应**原样出现在 App「关于/警告」页与 Release Notes 顶部**。

#### 1.4.1 能力边界（能做什么 / 不能做什么）

| ✅ 本项目管理器能做 | ❌ 本项目不能做 / 不承诺 |
|---|---|
| 在**手机本地**把用户提供的原厂 `boot.img`/`init_boot.img` 修成 SK 修补镜像 | 不提供「一键 root」：**首次刷入必须由用户在 PC 上 fastboot 完成**（§5.4.1） |
| 生成 48 位（实为 47+NUL）Root 密钥并加密保管 | 不破解 bootloader、不绕过厂商锁、不提供解锁服务 |
| 管理 AP 二进制部署、模块、root 授权（经 `apd root-exec`） | **不碰 AVB 密码学**：不重签 vbmeta、不提供 `--disable-verity/--disable-verification` 方案 |
| 提供 sha1 内容寻址原厂备份与回滚指令（可复制） | 不保证所有机型/内核可修补（SK 仅支持 3.10~6.18 且需 kallsyms/CFI 兼容） |
| 全程离线（运行期零网络；无遥测、无后台上传） | 不做 OTA 保留（系统 OTA 会覆盖 boot，需重新修补） |

#### 1.4.2 危险警告（必须展示，用户确认后才可继续）

1. **解锁 bootloader 会清空设备数据**，且可能失去官方保修、影响部分支付/银行类 App。
2. **刷入非原厂 boot 有变砖风险**。是否能回滚取决于：是否已备份原厂镜像（本 App 自动做 sha1 备份）、是否 A/B 设备、是否具备 PC + 官方固件包。
3. **AVB 状态会改变**：解锁设备后 bootloader 跳过验证；**不要**尝试用 `--disable-verification` 等命令（会清数据），本项目所有刷入命令**只允许** `fastboot flash boot --slot=all` / `flash init_boot --slot=all` + `reboot`。
4. **密钥丢了就没了**：密钥内嵌在内核镜像中，无法热轮换；换钥=重新修补并刷入。密钥泄露=设备 root 失守。
5. **模块与脚本以 root 运行**，第三方模块可能损坏系统；请只安装可信来源。
6. **不要混用不同体系的镜像/密钥**：本项目密钥 ABI 为 **47 字符 + NUL**，与 `Skroot-Next` 风格（48 字节无 NUL）**不兼容**（§3.5.4）。
7. 本软件**按「现状」提供，不附带任何担保**；因使用导致的设备损坏、数据丢失、保修失效由用户自行承担。

#### 1.4.3 发布者义务（Release 门禁）

| 项 | 要求 |
|---|---|
| 命令 | Release Notes 必须给出**准确**的 `fastboot` 命令（含 `--slot=all`），并显式写「禁止 vbmeta/disable-verification」 |
| 回滚 | 必须给出「如何刷回原厂」的完整步骤（原厂镜像获取途径 + `flash.bat`） |
| 机型 | 必须列出**已验证机型**与**已报告不兼容机型**；不得宣称「全机型支持」 |
| 合规 | 仓根 `LICENSE`/`NOTICE` 齐全（**AGPL-3.0-only**，§5.3.4/§11）；APK 内 NOTICE 与仓根一致（CI 断言）；**发布前完成 §11.3 检查清单** |
| 密钥 | 不得在 Release/日志/示例中展示任何**真实用户密钥**；文档示例一律用占位符 |

---

## 2. 现状基线与资产清点

### 2.1 已有资产（本次直接复用，不重造）

| 资产 | 路径 | 状态 | 本方案用法 |
|---|---|---|---|
| SK 修补引擎源码 | `engine/src/`（`skroot_patch_main.cpp`、`patch_pipeline.*`、`analyze/*`、五组 `patch_*.cpp`） | 已在 WSL 编译出 x86-64 产物 `engine/src/skroot-patch`（2,121,080 B，2026-11-06）；**需 task-3 补 JNI 入口 + arm64 交叉编译** | 引擎以 `libskrootpatch.so`（JNI）随 APK 分发（§5.2） |
| **magiskboot**（新增，人类指令） | **外部获取**：Magisk **v31.1** 官方 ARM64 release 产物（不入 git，由 CI 按 sha256 拉取/校验） | **需 CI 固化版本 + sha256** | 打包主路径：`unpack` / `repack` / `cpio`（§5.3.1） |
| 引擎 I/O 契约 | `docs/SPROOT-DOC-C1-修补引擎IO契约.md` | v1.0 草案（12 个退出码、NDJSON 事件、report schema） | 编排层唯一调用面，不再自定义协议 |
| SK 机制分析 | `docs/SPROOT-DOC-A1-*.md` | v1.0 正式稿 | 决定 hack 点、稳定性要求、验收断言 |
| 旧修补链 | `app/app/src/main/assets/boot_patch.sh` + `PatchesViewModel.kt` | 已含 SPRoot 改造注记（P1/P2 标记） | 保留其「模式/备份/槽位」思路，废弃其脚本主链 |
| bootimg 工具源码（参考） | `upstream/KernelPatch/tools/bootimg.{c,h}` | GPL-2.0，含 `repack_bootimg()` | **仅离线参考/对照**；打包主路径改用 magiskboot（§5.3） |
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
│  BootImageRepository  → magiskboot 解包/回包（exec）+ 自研 bootimg 交叉校验 + AVB │
│  RcInjector           → 写 init.sproot.rc + init.rc 追加 import（cpio）│
│  PatchOrchestrator    → 八步状态机 + 进度聚合 + 失败恢复             │
│  EngineRunner         → JNI 引擎调用 + 进度回调/NDJSON 双写 + 退出码映射   │
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
│  ③ magiskboot（官方 ARM64，子进程，无 root）│         │  ③ apd root-exec -- <cmd>               │
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
| `BootImageRepository` | L2 | magiskboot（exec）解包/回包 + 自研 bootimg 交叉校验、AVB 区段保留、分区与槽位探测 | boot.img | 工作目录布局 + header/AVB 元数据 | 格式不支持 / magiskboot 失败→回退 |
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
     ├ 3 Unpack       ：magiskboot unpack boot.img → kernel / ramdisk.cpio / dtb（自研 bootimg 交叉校验产物）
     ├ 4 Inspect      ：引擎 nativeInspect（CONFIG_KALLSYMS / patched? / 版本域）
     ├ 5 Patch        ：JNI nativeRunPatch(in=kernel.raw, out=kernel.patched, key|gen-key, json=双写)
     ├ 6 Verify       ：ArtifactVerifier（等长、sha256≠、密钥内嵌、hook_table 5 项、prefix==key[0:16]）
     ├ 7 RcInject     ：magiskboot cpio 写 init.sproot.rc + init.rc import（**优先 init_boot 的 ramdisk**）
     ├ 8 Repack+Deliver：magiskboot repack（原图作模板）→ new-boot.img（AVB 等长+区段断言）→ 导出/刷入（**通道②**）
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
| **T1 修补期** | 用户点修补 | 管理器（**无 root**） | 选镜像 → 引擎 JNI 解包 → 引擎 patch kernel → 取 48 字符 key（Keystore 存）→ **RcInject 步骤写 ramdisk（优先 init_boot）** → 引擎 repack（AVB 策略）→ 导出/刷入 | **完全不动**（只有 App 私有 workdir） |
| **T2 首启** | 刷入后第一次开机、打开管理器 | 管理器 → AP | 写临时 key 文件 → `apd deploy` → AP 自提权后落盘 `/data/adb/*`、`sproot/key`、隐藏 su → 删临时文件 | 就绪；**模块能力尚未生效** |
| **T3 二次开机起** | 之后每次开机 | init + AP daemon | rc → post-fs-data → daemon → services/boot-completed | 稳态 |

> **UI 必做**：T2 完成后明确提示「需重启一次，模块功能才会生效」；重启前模块页禁用安装按钮（DOC-05 §6.1 末段）。

### 3.5.4 ⚠️ 密钥 ABI 注记（禁止跨体系套用）

> **本项目的密钥 ABI 固定为「47 字符 + NUL」（对外展示 48 位）**：内核 shellcode 头部 48 字节 = `key[0:47] + '\0'`；`execve` 的 filename 与这 47 字符逐字节比对至 NUL（引擎 `engine/src/patch_do_execve.cpp`、`skroot_patch_main.cpp:312-317`；A1 §4.1）。
>
> **存在另一套不兼容 ABI**：`Skroot-Next` 风格 patcher 采用 **48 字节无 NUL、按实际长度比较**。二者**不可混用**：
> - 用 Skroot-Next 风格 patched 镜像 + 本项目 47 字符密钥 → **提权静默失败**（多/少一字符即失配；内核无日志，现象为「su 静默返回、uid 仍非 0」）；
> - 反之（本项目镜像 + Skroot-Next 的 48 字节密钥）同样失配。
>
> **纪律**：①**密钥不可跨体系套用**（本项目密钥只对本项目镜像有效）；②`SukeyProvider` 只生成/保存 47 字符语义密钥，对外按 48 位展示；③诊断页显示 `key ABI = 47+NUL`；④质测必须有「错体系密钥 → 提权失败且不崩溃、不刷错误日志」用例（task-7）。

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

### 3.7 客户端已实现能力清单（**v2 实况**，2026-11-08）

> 目的：技术方案的客户端章节此前停留在设计态；本节把 `app/`（Kotlin + Compose + miuix 客户端）的**实际实现**逐项登记，并列出与设计态的差异（§3.7.2），供 task-5/task-7 对齐。证据均为 v2 仓库内文件路径，可直接核对。

#### 3.7.1 已实现（可用）

| # | 能力 | 实现入口（v2 路径） | 关键点 |
|---|---|---|---|
| C1 | 四页导航（主页/修补/模块/设置） | `app/src/main/java/com/sproot/manager/ui/AppRoot.kt:35-69` | miuix `NavigationBar`，4 个 `AppTab`；与 §4.4 页面映射一致 |
| C2 | **修补流水线（8+1 步）** | `app/.../core/PatchPipeline.kt:18-45` | ①环境自检→②复制+**sha1 内容寻址备份**→③`magiskboot unpack`→④压缩则 `decompress`→⑤**JNI patch（密钥注入）**→⑥原压缩格式 `compress`→⑦`magiskboot repack`→⑧校验（存在/内核等长/sha256/与输入不同）→⑨状态机置「已修补·待刷入」；**全程无 root、无 shell、无脚本** |
| C3 | PatchMode 模式（改造版） | `app/.../core/Models.kt:30-49` | 可用：`EXPORT_ONLY`、`EXPORT_AND_GUIDE`（含 fastboot 模板+AVB 风险提示）；预留 `RESTORE_BACKUP`、`UNINSTALL_RESTORE`（`enabled=false`） |
| C4 | **入口能力自检（阻断式）** | `app/.../core/Capabilities.kt:22-60+` | ABI / 引擎 / magiskboot / 存储（≥3 GiB）/ 密钥 / 输入魔数；每条含**绝对路径+实际值**；blocking 项不过**不进修补** |
| C5 | **引擎自检（JNI）** | `app/.../core/PatchEngine.kt:55-110`、`core/Elf.kt:10-52` | `present/executable/aarch64/loaded/version/detail` 五元组；`System.loadLibrary` 失败给「缺件/ABI/符号」排查建议；`EM_AARCH64=183` 断言 |
| C6 | 修补引擎 JNI 桥 | `app/src/main/java/me/sproot/patch/NativeEngine.kt:1-40` | 两个 JNI 符号 `patch` / `engineVersion`；参数语义对齐 C1（in/out/key/json/workdir；`key=null`→`--gen-key`；固定 `--quiet`，结果走 json/report） |
| C7 | **原厂备份（sha1 内容寻址）** | `app/.../core/BackupStore.kt:11-63` | `filesDir/backups/<sha1>.img` + `backups/index.tsv`（时间/sha1/大小/原名）；同 sha1 **复用**；对应 §5.6-③ |
| C8 | 产物导出 | `app/.../core/Exporter.kt:11-40` | API≥29 走 MediaStore → `Downloads/SPRoot/`；API≤28 回退公共 Downloads 目录（无需存储权限/权限回退） |
| C9 | **模块页（列表/启停/安装/卸载）** | `ui/pages/ModulesPage.kt:51-190`、`core/Modules.kt:26-178` | `module list --json` 解析；`enable/disable <id>`；`install <zip> --json`（SAF→私有 cache→PK 头/大小校验→两阶段语义）；`uninstall <id>` 二次确认；`install_status` 未知字段占位兼容 |
| C10 | **AP 首启自检（selftest）** | `core/ApdSelfTest.kt:6-40,81-85` | `apd selftest --json`；stage 语义：`undeployed`(exit 0,正常)/`deployed`/`daemon_ready`/`needs_repair`/`no_ap_binary`/`escalation_failed`/`timeout`/`parse_failed`；结果缓存 SharedPreferences，主页/模块/设置页**只读展示**（不每次起进程） |
| C11 | 版本探测（非 root） | `core/ApdClient.kt:209,283-294` | `apd version --json` 任意身份可调；用于部署状态/版本偏差检测 |
| C12 | **许可资产随包** | `app/src/main/assets/{NOTICE-APK.md,NOTICE-magiskboot.txt,licenses/GPL-3.0.txt}`；`apk.yml:105` | CI 由仓库根 `NOTICE`/`LICENSE` 装配（单一可信源），与 §5.3.4/§11 一致 |
| C13 | **原生二进制锁定（可复现构建）** | `app/native.lock.json`；`apk.yml:35-68` | engine/apd/magiskboot 的 artifact、`run_id`、`sha256`、`size` **全固定**；CI 按 lock 精确下载+校验，**不浮动 latest**；升级须显式改 lock 并单独 commit |
| C14 | 日志与状态持久化 | `core/AppLog.kt`、`core/InstallState.kt:56-72` | 结构化阶段日志 + 安装状态快照（含 `artifact_sha256`）；对应 §5.7-L5 |
| C15 | 密钥安全存储 | `core/SecretStore.kt` | Android Keystore 加密（对应 §3.5.2「临时 0600 文件、用后即删」） |
| C16 | CI 五工作流 | `.github/workflows/{apk,engine,apd,licenses,release}.yml` | 见 §8.2（已按实况修正）；`licenses.yml` 校验许可文件 + 上游 15 文件 sha256 指纹不漂移 |

#### 3.7.2 与设计态的差异（**必须对齐**）

| # | 设计条目 | 实现态（v2） | 处置建议 |
|---|---|---|---|
| D1 | §5.4 步⑦ **RcInject（ramdisk rc 注入）** | **客户端未实现**（`app/` 内无 `init.sproot`/`cpio` 相关代码） | 归入 `apd deploy`（T2）或 task-5 迭代；模板见 DOC-05 §5.1；**注意 §3.5.4 key ABI 与 rc 不含密钥约束** |
| D2 | §5.4.2 PatchMode **四模式全开** | 实为 **2 可用 + 2 预留**（RESTORE/UNINSTALL `enabled=false`） | 与「导出与刷入分离」原则一致；直刷模式待 AP 部署链打通后开放（task-5） |
| D3 | §3.6 **12 态双轴状态机** | 客户端用 `PatchUiState` + `InstallState` + `ApdSelfTest.stage` 组合表达，**未落显式 12 态枚举** | task-4 后续收敛为单一 `deriveCapabilities(state)`（避免散落条件判断） |
| D4 | §5.2.1 `nativeSelftest()` | 引擎侧仅实现 `engineVersion()`；自检以 `Capabilities`+`Elf` 断言替代 | 可选：引擎补 `nativeSelftest`；当前替代方案已满足「缺件即阻断」 |
| D5 | §8.2 设计态 **6 工作流** | 实际 **5 个**（`apk/engine/apd/licenses/release`）；无 `ci-static`、无独立 `magiskboot.yml` | 文档已改（§8.2）；静态检查是缺口（D7），magiskboot 并入 `native.lock` |
| D6 | §5.4.1 **AVB 硬断言（等长+区段 diff）** | 客户端目前仅做「AVB footer 存在性前后对比 + 不修改」日志（`PatchPipeline.kt:465-469`）+ 等长/sha256 | task-5 补「AVB 区段逐字节 diff」硬门禁（PKG-2/AVB-2） |
| D7 | §8.1 G2「本地只做静态检查」 | **无 `ci-static` 工作流**（ktlint/detekt/clippy 未在 CI 强制） | task-8 补最小静态检查 job（低成本、高收益） |

#### 3.7.3 验证方式（可直接复用）

| 层 | 方法 |
|---|---|
| 本地静态 | `cargo check`（apd，默认 + `--no-default-features` 两条路径）；Gradle 依赖解析 |
| CI | `apk.yml` 断言 AArch64 natives + 签名 + NOTICE assets；`licenses.yml` 断言许可与上游指纹；`engine.yml`/`apd.yml` 断言 ELF 架构与能力标记 |
| 质测建议（task-7） | 对 C4/C5/C9/C10 各做一条「人为破坏（删 lib/改 lock/断网/错密钥）→ UI 必须给具名错误且不静默」用例 |

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
| **magiskboot**（Magisk v31.1 官方 ARM64） | 独立 ELF（`jniLibs`/`assets` 落地后可执行） | App 进程 spawn 的子进程（**无 root**） | `unpack` / `repack` / `cpio`（打包主路径，§5.3.1） | 普通 App 权限（仅文件操作） |
| ~~kptools~~ | **不引入**（仅离线参考源码） | — | — | — |

> **反模式警戒**：不要试图用 `libskrootpatch.so` 承担特权行为（它不是特权执行者）；也不要用 AP 二进制做内核 patch（AP 无修补能力）。二者短信道、不同权限域。

#### 5.2.1 引擎能力自检（每次修补前执行，失败即阻断）

```kotlin
// EngineCapabilityProbe —— 结果落 logs/preflight-<ts>.log
1. 资产存在：`nativeLibraryDir/libskrootpatch.so`（引擎）与 `libmagiskboot.so`（打包工具）存在，size == binaries.json 记录值
2. 架构正确：读 ELF header，两者均 `e_machine == 0xB7(AArch64)`，ELFCLASS64
3. 摘要正确：sha256(file) == binaries.json（含 magiskboot 的官方来源版本号 Magisk v31.1）
4. JNI 加载探针：System.loadLibrary("skrootpatch") 成功；nativeVersion() 与 binaries.json 一致
5. 自检干跑：nativeSelftest() 返回 0
6. **magiskboot 可执行探针**：exec `libmagiskboot.so`（无参数打印 usage）→ 期望 rc==0 或已知 usage rc；失败分类 ENOEXEC/EACCES/ENOENT/超时（此时按 §5.3 回退自研 bootimg，并置 fallback_used=true）
7. 【仅诊断模式】exec `libskrootpatch.so --selftest`
```

> **对 task-3 的明确要求**：①同库导出 JNI 入口（`System.loadLibrary("skrootpatch")` 可加载）+ `nativeVersion/nativeSelftest/nativeRunPatch`；②**同时**保留 CLI `main()`（诊断模式 + PC/CI 复用）；③NDK clang `aarch64-linux-android` 构建，CI 用 `readelf` 断言架构（RC §5.2 断言 1）；④JNI 层禁止抛 C++ 异常跨边界（`-fno-exceptions` + 错误码返回）。

**实现要点**

1. 打包：`libskrootpatch.so` 放 `app/src/main/jniLibs/arm64-v8a/`；`binaries.json` 由 CI 生成（`name/version/sha256/source_commit/abi`）。
2. JNI 调用：`external fun nativeRunPatch(inPath: String, outPath: String, keyOrNull: String, jsonPath: String?, workDir: String?): Int`，返回值即 C1 退出码；进度经 `onProgress(phase, pct, detail)` 回传 + NDJSON 文件双写。
3. **线程**：`nativeRunPatch` 在 `Dispatchers.IO` 专用线程执行（引擎内部有阻塞 IO 与大量内存分配）；**并发保护**：进程级 `Mutex`，同一时刻只允许一次修补（引擎不是线程安全的）。
4. **崩溃可观测**：JNI 入口外层记录 `state.json`（步骤、输入 sha256、时间戳）；App 启动时若发现 `state.json` 为「进行中」，展示「上次修补异常中断 + 日志/诊断包」，不静默。
5. 完整性：随包 `binaries.json` 参与 §5.2.1 自检；缺件/摘要不符 → `E-PRE-BIN`。
6. **构建期断言**（Gradle task，失败即 build fail）：`lib/arm64-v8a/libskrootpatch.so` 与 `lib/arm64-v8a/libmagiskboot.so` 均存在且 `readelf -h` 显示 `Machine: AArch64`；magiskboot 的 sha256 与「Magisk v31.1 官方 ARM64 release」记录一致；`assets/` 内不得出现任何 `.sh` 修补脚本；**不得出现 AIK 产物或非官方 Windows 版 magiskboot**（RC F2/F3 + 人类指令）。

### 5.3 决策 D-2（v1.0.1 修订）：打包工具链 = **magiskboot（人类指令）+ 引擎 patch kernel**，自研 bootimg 转为交叉校验/回退

> **人类新指令（2026-11-08，经 lead 转达）**：打包**用 magiskboot（ARM64）**，**禁用 AIK（Android.Image.Kitchen，不支持 Linux 6.0+ 内核）与非官方 Windows 版 magiskboot**。本节据此修订；v1.0「自研 bootimg 为唯一打包路径」作废（自研实现保留为交叉校验与回退）。

| 方案 | 结论 | 说明 |
|---|---|---|
| **magiskboot（打包主路径）** | ✅ **引入**：来自 **Magisk v31.1 官方 ARM64 构建**，作为独立可执行组件随 APK 分发 | `unpack` / `repack` / `cpio` 三条命令覆盖解包、回包、ramdisk 编辑；**原厂镜像作模板**（repack 保留原布局/AVB 区段）；许可证 GPL-3.0，按「独立程序聚合」处理（§5.3.2） |
| **引擎（`libskrootpatch.so`）** | ✅ 保留：只负责 **kernel 段字节 patch**（SK shellcode）+ 密钥 | 不做 bootimg 结构解析（可由 magiskboot 承担），但保留最小 header 解析用于校验 |
| **自研 bootimg** | ⚠️ **保留为交叉校验 / 回退** | ①交叉校验：独立解析 magiskboot 产物，断言结构/段偏移/AVB 区段；②回退：magiskboot 在特定机型失败时启用，并记录 `fallback_used=true` 供质测追踪 |
| kptools | ❌ 不引入（仅离线参考 `upstream/KernelPatch/tools/bootimg.c`） | 避免第三套格式实现 |
| **AIK（Android.Image.Kitchen）** | 🚫 **明令禁止** | 不支持 Linux 6.0+ 内核；且非 ARM64 机内工具 |
| 非官方 Windows 版 magiskboot | 🚫 **明令禁止** | 来路不明、无校验、可能被植入；只用 Magisk 官方 release 产物 |

#### 5.3.1 打包链（命令级，**唯一主路径**）

```bash
# 工作目录 work/，boot.img 为用户提供的原厂镜像（repack 的模板）
# 1) 解包（magiskboot 在 APK 内以 libmagiskboot.so 形式落 nativeLibraryDir 后 exec）
magiskboot unpack work/boot.img
#   产物：kernel / ramdisk.cpio / dtb / header 等（按镜像实际内容）
# 2) 引擎 patch kernel 段（JNI）：kernel → kernel.patched（等长，SK shellcode）
#    同时输出 report.json（密钥/hook_table/hidden_dir_prefix）
# 3) RcInject（D-14）：magiskboot cpio 向 ramdisk 加 2 个文件
magiskboot cpio ramdisk.cpio "add 0644 init.sproot.rc work/init.sproot.rc"
magiskboot cpio ramdisk.cpio "extract init.rc"        # 取原 init.rc
#    末尾追加一行 import /init.sproot.rc 后回写：
magiskboot cpio ramdisk.cpio "add 0644 init.rc work/init.rc.patched"
# 4) 回包（以原厂 boot.img 为模板，保留段布局与 AVB 区段）
magiskboot repack work/boot.img work/new-boot.img
```

**打包链验收断言（与 §5.4.1 AVB-1~AVB-4、§5.4 步⑧ 一致，硬门禁）**

| # | 断言 | 判定 |
|---|---|---|
| PKG-1 | 产物 `new-boot.img` **长度严格等于目标分区大小**：`boot=100663296 B`、`init_boot=8388608 B`（**不是 ≤**） | 不等即失败（`E-AVB-20`） |
| PKG-2 | 原厂镜像与产物的 **AVB footer + 内嵌 vbmeta 区逐字节保留**（对应区段 diff 为空） | diff 非空即失败（`E-AVB-19`） |
| PKG-3 | 自研 bootimg **交叉解析**产物成功：`ANDROID!` 魔数、各段 offset+size 与 magiskboot 一致、`kernel` 段可再次 patch/回读 | 不一致即失败（`E-PKG-21`） |
| PKG-4 | `magiskboot unpack new-boot.img` 可再次解包，`kernel` sha256 == `kernel.patched` | 失败即判「产物不可用」 |
| PKG-5 | 命令文本中**不含** `vbmeta` / `disable-verity` / `disable-verification`（AVB-4 同源） | 静态检查 |

> **回退规则**：若 magiskboot unpack/repack 在目标机型失败（rc≠0 或 PKG-3 不过），自动切**自研 bootimg 回退**路径并置 `fallback_used=true`、记录完整日志；回退产物**同样**必须通过 PKG-1/PKG-2/AVB 断言，否则不导出。

#### 5.3.2 自研 bootimg 的最小能力集（交叉校验/回退用）

| # | 能力 | 说明 |
|---|---|---|
| P1 | boot header v0–v4 解析/序列化 | `magic="ANDROID!"`、段 offset+size、页大小、os_version |
| P2 | kernel 段取出 + 解压归一 | gzip / lz4 / 未压缩；记录原压缩类型以回填（引擎拒 `.gz` 直入，RC §2.3） |
| P3 | 写回 kernel 段（等长） | SK patch 不改长度 |
| P4 | repack（回退模式） | 段偏移按页对齐重排，输出合法 boot.img；受 PKG-1/PKG-2 硬约束 |
| P5 | AVB 区段识别与**逐字节保留** | 定位 `AVBf` footer 与内嵌 vbmeta 区段，repack 时不改写 |
| P6 | ramdisk cpio 增删（回退时） | RcInject 等价实现：写 `init.sproot.rc` + `init.rc` import（优先 init_boot） |
| P7 | 分区/槽位探测 | `ro.boot.slot_suffix`、by-name、`init_boot` vs `boot` |

**D-14 裁决落定（lead 2026-11-08：采纳解读 A，固定表述如下）**

> 「引擎 patch kernel 段（SK shellcode）；**同一流水线的 RcInject 步骤**向 ramdisk 写入 `init.sproot.rc` + `init.rc` import 行。二者是流水线两个独立步骤，SK 内核运行时零 ramdisk 依赖。」

落地约束（lead 明确，写死）：
1. **SK 内核运行时**只依赖 kernel Image 内的五 hook/密钥，**不依赖 ramdisk 任何内容**——这点比 KSU 简单（KSU 需改 `init`→`init.real`）。
2. 我们是**安装器**：在修补流水线内改 ramdisk 是允许且必要的（KSU 同样改 init）。
3. **隐藏性约束**：ramdisk **只加 2 个文件**（`init.sproot.rc` + 一行 `import` 追加），**不碰 SELinux policy、不改 AVB 逻辑**；AVB 只按 §5.4.1 的显式策略处理。
4. **RcInject 目标（D-12 裁决）**：本机存在 `init_boot.img`（8MB，ramdisk 形态）→ **优先注入 init_boot 的 ramdisk**（Android 13+ 通用 ramdisk 在 init_boot）；`boot.img` 的 ramdisk 作为回退；**实现中两者都探测并在 manifest 记录实际目标**。

#### 5.3.3 许可证标注（**人类已定：自有代码 AGPL-3.0-only**；D-3 已裁）

| 组件 | 许可证状态 | 处置 |
|---|---|---|
| **本项目自有代码**（管理器 App / 自研 bootimg / JNI 胶水 / 构建脚本 / 文档） | **AGPL-3.0-only**（SPDX） | `LICENSE` = AGPL-3.0 **全文**（含 SPDX 头）；`README` 许可段标注 `SPDX-License-Identifier: AGPL-3.0-only`；若人类后续允许升级版本，可改 `AGPL-3.0-or-later`（**默认 only**） |
| **SKRoot 上游源码**（`abcz316/SKRoot-linuxKernelRoot`，vendored 到 `engine/`） | **proprietary（无 SPDX 声明）** | 本项目使用已获上游作者授权；**再许可风险见 §11**（授权未明确覆盖 AGPL 再分发时须剥离 `engine/`）；NOTICE 不声称其开源、不附其源码 |
| **magiskboot**（Magisk v31.1 官方 ARM64 release 产物） | **GPL-3.0** | 独立可执行组件，「独立程序聚合」处理：APK `NOTICE` 标注来源（`topjohnwu/Magisk` v31.1）、GPL-3.0 声明与**可获取源码**链接；不改写/不链接其源码；保留原始二进制 + sha256。**GPL-3.0 与 AGPL-3.0 兼容**（AGPL-3.0 §13 第 2 段允许与 GPL-3.0 组合/链接），本项目按聚合处理 |
| Kotlin / Compose / AndroidX / miuix | Apache-2.0 | Apache-2.0 与 AGPL-3.0 兼容；随 NOTICE 声明 |
| asmjit（`engine/3rdparty/asmjit2-src`） | Zlib | 保留版权与许可声明（本地 `LICENSE.md` 已核实） |
| capstone（`engine/3rdparty/capstone-5.0.7`） | BSD-3-Clause | 解包后核对包内 LICENSE.TXT 并随发布物附带声明 |
| cJSON（`engine/3rdparty/cJSON`） | MIT | 保留版权与许可声明 |
| **AP 二进制 `apd` 的 115 个 Rust 依赖** | 全部 permissive（MIT/Apache-2.0/Zlib/Unlicense/ISC/BSD-3/BSL-1.0/CC0/Unicode-3.0）**除 2 项 UNKNOWN** | 明细 `apd/THIRD_PARTY_LICENSES.md`；根 `NOTICE` §8；**`prop-rs`/`prop-rs-android` = UNKNOWN 阻断发布**（§11.4）；`extattr`(GPL-2.0-only) 已移除 |
| KernelPatch `bootimg.c` / APatch / KernelSU | 各为 GPL-2.0 / GPL 系 | **仅参考、未复制代码**，不随包分发；如未来复制须按其 GPL 处理并做 AGPL 兼容性评估 |
| AIK / 非官方 Windows magiskboot | — | **禁止使用**（人类指令） |

> **结论**：自有代码定案 **AGPL-3.0-only**；第三方义务通过 `NOTICE` 履行；唯一未闭环项是 **SKRoot proprietary 部分的再许可**（§11，需人类/律师确认授权范围）。

#### 5.3.4 仓根 `LICENSE` / `NOTICE`（**人类已定：AGPL-3.0-only，已落盘**）

> 现状（v1.1）：仓库根已新增 **`LICENSE`（AGPL-3.0 全文 + SPDX 头）**、**`NOTICE`（第三方声明）**、**`README.md`（含许可段）**。这是发布门禁的硬要求。

**① `LICENSE`（已完成）**

| 项 | 内容 |
|---|---|
| 文件名 | 仓库根 `LICENSE` |
| 内容 | 首行 `SPDX-License-Identifier: AGPL-3.0-only` → 版权行 → AGPL 权利/免责说明 → **AGPL-3.0 全文**（约 34 KB，自 SPDX 官方文本 `AGPL-3.0-only.txt` 逐字拷贝） |
| 附加说明 | 文件头部含两段说明：①第三方组件不在本许可范围（见 `NOTICE`）；②`engine/` 的 SKRoot 派生部分再许可风险（见 §11） |
| 版本选择 | **默认 `AGPL-3.0-only`**；若人类希望允许后续版本，改为 `AGPL-3.0-or-later` 并同步 `README`/`NOTICE`/SPDX 头 |

**② `NOTICE`（已完成，必须与 APK 内 NOTICE 同内容）**

| # | 组件 | 许可证 | 声明要点 |
|---|---|---|---|
| N1 | **magiskboot**（Magisk v31.1 官方 ARM64） | **GPL-3.0** | 来源 URL + v31.1 + **可获取源码**链接（GPL §6 义务）；以**独立可执行程序**分发（聚合）；说明 **GPL-3.0 与 AGPL-3.0 兼容**（AGPL §13 第 2 段） |
| N2 | Kotlin / Compose / AndroidX | Apache-2.0 | 版权归 JetBrains/AOSP 及贡献者；保留声明 |
| N3 | miuix 0.9.4 | Apache-2.0 | 来源 URL |
| N4 | asmjit | **Zlib** | 完整 zlib 声明三项限制（本地 `LICENSE.md` 已核实） |
| N5 | capstone 5.0.7 | **BSD-3-Clause** | 保留版权/条件/免责；包内 LICENSE.TXT 解包后核对 |
| N6 | cJSON | **MIT** | 版权行 + 许可声明 |
| N7 | **SKRoot 上游源码（vendored）** | **proprietary（无 SPDX）** | 说明「已获作者授权、授权凭证人类持有、不声称开源」+ **⚠️ 再许可风险提示（指向 §11）** |
| N8 | KernelPatch / APatch / KernelSU | GPL-2.0 / GPL 系 | **仅参考、未复制代码**；若未来复制须按其 GPL 处理并评估 AGPL 兼容性 |
| N9 | AIK / 非官方 Windows magiskboot | — | **禁止使用** |

**③ `README.md` 许可段（已完成）**

- 明确 `SPDX-License-Identifier: AGPL-3.0-only`；
- 指向 `LICENSE`（自有代码）与 `NOTICE`（第三方）；
- 单列「⚠️ engine/ 再许可风险」一句 + 指向 §11；
- 单列「刷机风险与能力边界」摘要（对应 §1.4）。

**④ CI 断言（task-8 落地）**

| 项 | 断言 |
|---|---|
| `newapp.yml` | APK 内 `NOTICE` 含 `magiskboot` + `GPL-3.0` + `v31.1`；含 SKRoot proprietary 段落 |
| `release.yml` | 仓根 `LICENSE`/`NOTICE` 存在且非空；`LICENSE` 含 `SPDX-License-Identifier: AGPL-3.0-only` 且含「GNU AFFERO GENERAL PUBLIC LICENSE」正文 |
| 变更纪律 | 新增第三方依赖 → 同步更新 `NOTICE`（PR 检查项）；**禁止**在 CI 中自动改写 `LICENSE` 正文 |

### 5.4 八步流程（可执行步骤 + 每步失败处理）

> **总原则（源自 RC §五 5.1）**：每一步都必须有**可断言的成功判据**；「打印 Successfully」不算成功。全程不经过 `sh -c`、不依赖 busybox、不做任何网络访问、不打包 shell 脚本。

| 步 | 动作 | 具体命令/断言 | 失败处理（用户可见） |
|---|---|---|---|
| **1 Preflight** | 环境 + 自身能力自检 | ①`Build.SUPPORTED_ABIS` 含 `arm64-v8a`；②`filesDir` 可用空间 ≥ 3×boot.img；③`binaries.json` 逐项 sha256 + ELF `e_machine=0xB7`（§5.2.1：**引擎 + magiskboot + AP 二进制**）；④JNI `nativeVersion/nativeSelftest` + magiskboot 可执行探针；⑤创建 `logs/patch-<ts>.log` 并写入 cmd/env/cwd | 首屏红条 + 具名错误码（`E-PRE-01` ABI / `E-PRE-02` 空间 / `E-PRE-BIN` 二进制缺失或校验失败 / `E-PRE-03` JNI/工具加载失败），附「导出诊断」；**绝不进入下一步** |
| **2 AcquireInput** | 取镜像到 staging + 指纹 | SAF `openInputStream(uri)` → `work/boot.img`（分块拷贝 + 流式 sha256）；记录 `ro.boot.slot_suffix`、`ro.product.device`、`ro.build.version.sdk`、**分区精确字节数（boot=100663296 / init_boot=8388608）与 AVB 区段偏移**；**探测 `boot` 与 `init_boot` 两个分区**（A13+ 优先 init_boot，判据：init_boot 存在且含 ramdisk；决定 RcInject 目标并记入 manifest） | `[E-IO-04] 无法读取所选文件（%s）`；`[E-IO-05] 剩余空间不足：需 %d MB，实有 %d MB` |
| **3 Unpack** | 拆镜像 + **解压归一**（magiskboot 主路径，自研 bootimg 交叉校验） | `magiskboot unpack work/boot.img` → `kernel`、`ramdisk.cpio`（若有）、`dtb*`；自研 bootimg 独立解析同一镜像做交叉校验（段 offset+size 必须一致）；判断 `kernel` 是否已解压裸 Image：压缩魔数（`1f 8b` gzip / `04 22 4d 18` lz4）→ 解压为 `kernel.raw` 并**记录原压缩类型与参数** | `[E-IMG-06] 镜像格式无法解析（%s）`；`[E-IMG-07] 内核压缩格式不支持（%s）`；magiskboot 失败 → 自动回退自研 bootimg（置 `fallback_used=true`） |
| **4 Inspect** | 前置校验（APL §2.3-1 教训） | 引擎 `nativeInspect`：①必含 `CONFIG_KALLSYMS=y`；②`patched=false`（已修补则拒）；③内核版本落在引擎支持域（3.10~6.18，A1）；④记录 sha256 与大小 | 每项具名错误：`[E-IMG-08] 内核未开启 CONFIG_KALLSYMS`、`[E-IMG-09] 该镜像已被修补，请使用原厂 boot.img`、`[E-IMG-10] 内核版本 %s 不支持` |
| **5 Patch** | 调 SK 引擎（**JNI 进程内**） | `nativeRunPatch(in = work/kernel.raw, out = work/kernel.patched, key = K \| null(gen), json = work/p.ndjson)`；进度回调 + NDJSON 双写；引擎内部仍按 C1 落 `report.json` | 退出码→中文映射（§5.5）；**同时**校验 C1 §3 铁律：rc≠0 时 `kernel.patched` 必须不存在（若存在 → `E-ENG-11` 引擎违约并删除） |
| **6 Verify** | 产物断言（C1 §4 + RC §5.1-[5]） | ① `size(out)==size(in)`；② `sha256(out)!=sha256(in)`；③ out 内存在 48B 窗口 == `key[0:47]+NUL`；④ `report.hook_table` 含 5 个 hook 且地址非 0；⑤ `report.hidden_dir_prefix == key[0:16]`；⑥ `report.root_key == 传入 key`（防「刷了别人的 key」）；⑦ 引擎回读 `patched=true`（若引擎提供该能力，否则记 `skipped`） | 任一失败 → `[E-VER-12] 产物校验失败：<断言名>`，**删除产物、不导出、不回写**，保留 workdir |
| **7 RcInject**（**必做**，D-14 解读 A） | ramdisk rc 注入（magiskboot cpio）；**目标优先 `init_boot` 的 ramdisk，`boot.img` ramdisk 回退**（D-12） | `magiskboot cpio ramdisk.cpio "add 0644 init.sproot.rc …"` + extract/追加 `import /init.sproot.rc`/add 回写；断言：cpio 内可见 `/init.sproot.rc` 且 `init.rc` 尾部含该行；原 `init.rc` 备份进 manifest；**只动这 2 个文件，不碰 policy/AVB** | `[E-RC-17] rc 注入失败`；`[E-RC-18] 目标分区无 ramdisk 或 init.rc 结构不支持` → 按 D-12 回退另一分区；仍失败则报「该机型暂不支持 AP 自启」，内核 patch 仍可导出（UI 明示模块需手动启动） |
| **8 Repack+Deliver** | 重打包与交付（成功判据=可验证事实） | `magiskboot repack work/boot.img work/new-boot.img`（**原图作模板**）+ 自研 bootimg 交叉校验（§5.3.1 PKG-1~PKG-5）；断言（**全部必须通过**）：① 存在且头部魔数 `ANDROID!`；② **产物长度严格等于目标分区大小**（`boot`=100663296 B / `init_boot`=8388608 B，**不是「≤」**）；③ **AVB footer + 内嵌 vbmeta 区逐字节保留**（与原厂镜像对应区段 diff 为空）；④ `sha256` 与 manifest 一致；交付按 **PatchMode 四模式**（§5.4.2） | `[E-PKG-13] 重打包失败/产物头非法`；`[E-PKG-21] 交叉校验不一致`；`[E-AVB-19] AVB 区段被改动`；`[E-AVB-20] 产物长度与分区不符（期望 %d，实际 %d）`；`[E-DEL-14] 导出被取消`；`[E-FLS-15] 刷入失败（槽位 %s），已回滚`；`[E-FLS-16] 刷入后回读不一致，已尝试恢复备份` |

#### 5.4.1 AVB 处理（**lead 2026-11-08 结论，已推翻 v0.5 旧路径**）

**结论（写死）：AVB 不碰密码学，只保留语义。**

| 事实 | 说明 |
|---|---|
| 三家实现（Magisk/KSU/APatch）都不重签 AVB | 靠**已解锁的 bootloader 跳过验证**，不做签名 |
| vbmeta 对 boot 仅 **Chain descriptor（RSA4096）** | 真正的 **boot hash descriptor 在内嵌 vbmeta 里**（boot.img 尾部）；无魅族私钥不可能重签 |
| 算法实为 **SHA256_RSA4096** | 我方无该私钥 → **任何「重签 AVB」的实现路径都不可行** |
| `--disable-verification` 是**清数据来源** | 必须**禁止**（会导致用户数据被清） |

**刷入链（唯一主路径）**

```bash
# 仅刷 boot（双槽一次覆盖），然后重启；不触碰 vbmeta
fastboot flash boot --slot=all sproot_patched_boot.img
fastboot reboot
```

**明令禁止**（写进 UI 提示与质测用例，出现即判失败）：
- `fastboot flash vbmeta ...`
- `fastboot --disable-verity ...` / `fastboot --disable-verification ...`
- 任何对 vbmeta / AVB 签名区的写入或重算

**AVB 两条路径（收敛为 2）**

| 路径 | 操作 | 结论 |
|---|---|---|
| **① 保留语义（唯一主路径）** | 产物与原厂镜像**等长**，AVB footer + 内嵌 vbmeta 区**逐字节原样保留**；bootloader 因已解锁而跳过验证 | ✅ 默认且唯一推荐；断言见步骤⑧ |
| **② 剥离 footer（备选，仅救援）** | 去掉 `AVBf` footer 与对齐填充（产物变短）；适用于原厂镜像本身异常、或需要对比排障 | ⚠️ **风险**：产物不再与分区等长，可能被 bootloader 拒绝（机型相关），且**丢失去回滚校验依据**；默认隐藏，需用户显式选择并阅读警告 |

**验收断言（质测可直接脚本化，纳入 §5.4 步⑧）**

| # | 断言 | 判定 |
|---|---|---|
| AVB-1 | `stat -c%s new boot.img == 100663296`（boot）/ `== 8388608`（init_boot） | 硬断言，不等即失败 |
| AVB-2 | 原厂镜像与产物的 `[size-fixed_avb_tail, size)` 区段逐字节相等（footer + 内嵌 vbmeta） | 硬断言，diff 非空即失败 |
| AVB-3 | 产物头部 `ANDROID!` 魔数正确、kernel/ramdisk 段可被引擎重新解包 | 硬断言 |
| AVB-4 | 刷入命令文本中**不含** `vbmeta` / `disable-verification` / `disable-verity` | 静态检查（UI/文档/脚本） |

**实现要求**：① 步 2 记录原厂 boot/init_boot 的**精确字节长度与 AVB 区段偏移**；② 步 8 以「等长 + AVB 区段清零 diff」为硬门禁；③ UI 交付前展示「仅刷 boot、禁止 vbmeta 操作」的一键复制指令；④ 回滚路径见 §5.6-③。

#### 5.4.2 交付模式（沿用 APatch `PatchMode` 四模式，A2 借鉴）

| 模式 | 语义 | 产物/收尾 |
|---|---|---|
| `PATCH_ONLY`（**默认**） | 只出镜像，用户自行刷 | 导出到用户选择位置（SAF），命名 `sproot_patched_<ver>_<keyhash8>.img`；同时在 App 私有目录保留副本 |
| `PATCH_AND_INSTALL` | 直刷当前槽 | **经特权链**：`root-exec -- dd`/写分区 + 回读校验；需 `capability=FlashImage`；标记「需重启」 |
| `INSTALL_TO_NEXT_SLOT` | 写备槽 + 切槽 | `root-exec` 执行 `bootctl set-active-boot-slot` + 写 `post_ota.sh`（开机成功 `mark-boot-successful` 自删） |
| `UNPATCH` | 还原原厂 | 从内容寻址备份（sha1）写回 + 清理 AP（`apd undeploy --keep-modules`） |

> 直刷三条（2/3/4）**全部走特权链**（`root-exec`），修补链本身只产 `new-boot.img`——这就是 §3.1.1 「两链唯一交点」。**所有直刷路径同样禁止触碰 vbmeta**（§5.4.1 禁令对 fastboot 与 dd 两条路同等生效）；PC 侧 fastboot 命令固定为 `fastboot flash boot --slot=all` / `fastboot flash init_boot --slot=all` + `reboot`。

**rc 注入物的内容（固定模板，不含密钥）**：`init.sproot.rc` 只含 `on post-fs-data → exec /data/adb/apd post-fs-data --superkey-file /data/adb/sproot/key` 与 `service sproot_daemon`（`class late_start`、`user root`、不写 seclabel、`on boot start`），逐字模板与理由见 **DOC-05 §5.1**（本文不复制，避免双源漂移）。**约束**：①不内联密钥；②只引用 `/data` 下路径；③注入幂等（已存在则替换为同版本）；④失败可回滚（manifest 保存原 ramdisk 散列）。

**取消与超时**：编排层持**取消令牌**（`AtomicBoolean`）；magiskboot 子进程可 `destroyForcibly()`，JNI 引擎在每个阶段边界检查令牌并返回 `E_CANCELLED`（内部码）；每步超时（unpack 60s / patch 300s / repack 90s）由编排层看门狗执行——JNI 内阻塞无法强杀，超时后置「修补组件可能无响应」并提示重启 App（native 崩溃风险已在 §5.2 说明）。取消后清理 staging、UI 明示「已取消，未产生任何镜像、设备未改动」。

**日志落盘要求（RC F5）**：`filesDir/logs/patch-<yyyyMMdd-HHmmss>.log`，内容含：机型/内核/系统版本、每步参数与环境（脱敏：密钥替换为 `<KEY:len=48,sha256前8位>`）、引擎进度事件、退出码、产物 sha256、耗时。日志保留最近 20 次，可一键导出 zip。

**shell 与 busybox 处置（新链明确不用）**：不走 `boot_patch.sh`，不打包 busybox；打包/rc 注入用 **magiskboot 官方二进制**（exec），kernel patch 在 JNI 引擎内，刷入经 `root-exec`。RC §四 已论证「最小可判定失败面越短越好」，且 `echo; exit $?` 类吞码 bug（RC P2）在脚本层无法根治——干脆去掉脚本层（magiskboot 是程序而非脚本，其退出码直接透传）。

### 5.5 引擎契约（与 C1 §1/§3 语义完全对齐；JNI 主路径，CLI 仅诊断）

**JNI 参数 ↔ C1 CLI 参数映射（同一份引擎实现，语义等价）**

| C1 CLI（诊断/PC） | JNI（App 主路径） | 说明 |
|---|---|---|
| `--in <kernel>` | `nativeRunPatch(inPath, …)` 第 1 参 | 已解压裸 Image（非 `.img`/`.gz`） |
| `--out <patched>` | 第 2 参 | 失败绝不产出（C1 §3） |
| `--key <48字符>` / `--gen-key` | 第 3 参 `keyOrNull`（null = 生成） | 生成后由引擎写 `report.root_key` 并在返回值旁回传 |
| `--json <ndjson>` | 第 4 参 `jsonPath`（可空）+ `onProgress` 回调 | **双通道**：回调实时、文件可回溯 |
| `--workdir <dir>` | 第 5 参 `workDir`（可空） | 失败保留供诊断 |
| 退出码 0/101/…/250 | **返回值 int**（同一枚举） | 不抛异常跨边界 |

CLI 形态（**仅诊断/PC/CI 复用**，不参与 App 主路径）：

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
| `E-PRE-03` | 步 1 JNI 加载/自检 | 修补组件不可用（架构/依赖不匹配） | 否（需更新 App） |
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
- **AVB 失败回滚（lead 2026-11-08 结论：不碰 AVB 密码学）**：若刷入后设备拒绝启动，**不要**执行任何 vbmeta/disable-verification 操作（会清数据），按顺序：①`fastboot flash boot --slot=all <sha1 备份的原厂 boot.img>`（`UNPATCH`，最直接）；②若涉及 init_boot（A13+）同样 `fastboot flash init_boot --slot=all <原厂 init_boot.img>`；③官方固件包 `flash.bat` 全量回原厂（最彻底，清数据）。三条写入 UI「救援指南」页（可复制指令 + 明确「禁止 vbmeta 操作」警示）。
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
| L1 | **APK 二进制资产清单 + 构建期断言**：`binaries.json` 声明每个 native 资产的 `name/version/sha256/e_machine`（含 **magiskboot = Magisk v31.1 官方 ARM64**）；Gradle 任务在 `assemble` 前用 `readelf` 断言 `AArch64` 且文件存在，失败即 **build fail**（禁止 skip） | F2、F3、P0-a |
| L2 | **禁止把「运行期网络下载」当交付**：入包可执行件（引擎/magiskboot/AP）要么由本仓 CI 构建，要么由 CI 从**官方固定版本**拉取并做 sha256 校验（失败即 fail build）；**运行期零下载** | F3、P0-b、R21 |
| L3 | **入口能力自检，缺件即阻断**：点击修补的第一件事是 §5.2.1 自检（存在/架构/摘要/可执行/JNI 加载），失败给出「缺什么、期望在哪、怎么办」，**不进流程、不落任何产物** | F1、P0-a、P0-b |
| L4 | **非必需资产解耦**：`kpimg` 这类「仅用于展示元信息」的资产不入包、不参与流程；SK 链不需要它（新 `boot_patch.sh` 已无 `-k kpimg`） | F4、P3 |
| L5 | **日志即产物 + 面板常驻**：日志写文件（非内存）、stdout/stderr 分列、失败后 UI 停留在错误行、可复制/可导出；错误面板由**状态机**驱动（`FAILED` 态常驻），**不依赖 `patching/patchdone` 这类瞬态布尔** | F5、P1、F11 |
| L6 | **成功必须可验证**：只认 §5.4 步 6/7 的断言（等长、密钥内嵌、hook 表、`ANDROID!` 魔数、刷入后回读 sha256），**不认**任何「脚本打印 Successfully」；同时全链不使用 shell 脚本（消除 `exit $?` 吞码） | F6、F7、P2 |

**补充硬约束（来自 lead 2026-11-08 全员通报，已并入本文）**

1. 架构必须规定「APK 内二进制资产清单 + 构建期断言」→ L1/L2（§5.2 实现要点 5）。
2. 修补执行层定为 **aarch64-android JNI `.so`（主路径）**；子进程可执行仅作诊断/回退 → §5.2。
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
| B1 | 管理器是**纯用户态 App**，修补在机内完成（选 boot.img → 拆 → patch → 重打 → 导出/刷入） | APL §2.1（AP 全链路）；KSU §0-1 同为机内修补 | §5.4 八步链 |
| B2 | **native 资产随 APK 分发**：KSU 用 `libksud.so`（一次性工具 exec）；SPRoot 用 `libskrootpatch.so`（JNI 进程内）+ `libmagiskboot.so`（打包工具 exec）+ `libapd.so`→稳定路径 | APL §2.2 层 2；KSU §5.1-①；`PatchesViewModel.kt:86-113`（旧链 symlink 反例） | §5.2 资产表 |
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
| M8 | 修补 UI 只展示一块滚动日志文本 | 旧路线"闪退感"的直接来源 | 八步状态灯 + 常驻错误面板 + 日志落盘（§5.7） |
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
| 特权执行者 | `libksud.so`（Rust CLI）经 `nativeLibraryDir` 的绝对路径 **exec**，argv+stdout/stderr 通信 | `apd`（Rust）+ JNI supercall | **JNI 引擎（修补，进程内）+ `/data/adb/apd`（特权，稳定路径）**；引擎不承担特权（§5.2） |
| boot.img 解包/回包 | **纯 Rust 自研**（`android-bootimg` crate），APK 里**没有** magiskboot/kptools | 外挂 kptools（构建期下载） | **magiskboot（Magisk v31.1 官方 ARM64）+ 自研 bootimg 交叉校验/回退**（人类指令，§5.3） |
| 修补命令 | 单命令全参数化：`boot-patch -b/-o/--kmi/--partition/-m/-u/-f/--backup/--no-install` | shell 脚本 `boot_patch.sh` | 全参数化由 Kotlin 编排 + JNI（§5.4）；**采纳 KSU 三个参数语义**：分区显式指定、强制备份、只改配置不重注入（→ §5.4/§9 D-13） |
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
| R2 | **boot.img/AVB 格式覆盖不足**（boot header v4、`init_boot`、异形 ramdisk、厂商校验） | 目标机型（魅族 21 等）分区布局非标准 | 解包失败或刷入后不开机 | **主路径用 magiskboot v31.1（格式覆盖最广）+ 自研 bootimg 交叉校验**；首批机型真机矩阵；**AVB 只保留语义（§5.4.1），等长 + 区段 diff 双硬门禁**；失败有 sha1 备份回滚 | 架构/task-3/qa |
| R3 | **打包链正确性**（magiskboot 与自研 bootimg 双实现一致性） | 两实现解析/回包结果不一致 | 产物结构损坏 → 不开机 | ①magiskboot 用 Magisk v31.1 官方 ARM64 + sha256 锁定（构建期断言）；②**交叉校验硬门禁**（§5.3.1 PKG-1~PKG-5）；③「解包→原样回包」必须字节一致（含 AVB 区段）作为回归基线；④真机小范围先验证；⑤SK 引擎许可证已解决（proprietary + 作者授权，§5.3.3） | task-3/qa |
| R21 | **magiskboot GPL-3.0 合规** | 随 APK 分发第三方 GPL 程序 | 合规风险 | 按「独立程序聚合」：APK `NOTICE` 标注来源/v31.1/GPL-3.0/源码链接；不改写其源码、不与其链接；保留原始二进制 + sha256；**禁用 AIK 与非官方 Windows 版**（§5.3.3） | task-8/人类 |
| R4 | **首装鸡生蛋**：刷入需要 root，而 root 来自刷入 | 新用户首次安装 | 无法完成闭环 | 三条降级路径：①有 PC 时 fastboot 刷导出的 img（不作为主推但保留）②已有 Magisk/其他 root 时直接 dd ③A/B 备槽 + 回滚（§5.6-③）；UI 明确「本 App 不联网、不代刷，需你确认设备可回滚」 | 架构 |
| R5 | **miuix 实验性 API 变更** | 升级 miuix | UI 返工 | 版本钉死 + design system 门面（§4.6-R1） | task-4 |
| R6 | **工具链新版本兼容**（AGP 9.4.1 / Kotlin 2.4.20 / compileSdk 37） | CI 环境 | 构建红 | CI 冒烟先行（task-8）；Gradle/AGP/JDK 成对锁定（与 app-client 定版一致） | task-8 |
| R7 | **密钥安全**：日志/备份/崩溃上报泄露 48 位密钥 | 任何字符串拼接失误 | 权限被他人获得 | 密钥只在 `SukeyProvider` 与引擎 argv 出现（argv 在 /proc 可见——**已知弱点**，见 D-4）；日志脱敏白名单；卸载清 Keystore | 架构/task-4 |
| R8 | **CMP 与 AndroidX Compose 混用** | 引入第三方 Compose 库 | 编译/运行时崩 | §4.2 约束 + CI 依赖审查 | task-4/task-8 |
| R9 | **内核支持域窄**（SK 依赖 kallsyms/CFI/华为 HKIP 绕过） | 冷门内核 | 大量机型不支持 | 引擎退出码→人话映射 + 「机型不支持」诚实提示 + 支持矩阵文档 | task-3/qa |
| R10 | **旧路线失败根因已确证**（RC）：引擎从未入包 + 错误被吞 | 已发生 | 项目一度误判「SK 机制不可行」 | 本文 §5.7 六条铁律逐条反制；RC §三 F1–F11 逐项落位见表 7.1 | ✅ 已闭环（本文） |
| R11 | **kernel 压缩格式导致重打包尺寸/偏移变化**（`Image.gz`/`Image.lz4`） | 解包产物 `kernel` 是压缩态，而引擎只吃**已解压裸 Image** | patch 后体积变化 → 重打包偏移/校验和变化 → 刷入后不开机 | §5.4 步 3 显式「解压归一 + 记录原压缩参数」；repack 后做「大小 ≤ 分区、头魔数、sha256 记录」三重断言；首批真机各机型验证（→ D-10） | task-3/qa |
| R12 | **子进程 exec 受限**（Android 10+ 私有目录禁 exec） | 影响 magiskboot（主路径工具）与「诊断模式」引擎 | magiskboot 无法执行 → 打包不可用（可回退自研 bootimg） | magiskboot 从 `nativeLibraryDir` 绝对路径 exec（KSU 同款已证）；失败自动回退自研 bootimg 并告警（§5.3.1 回退规则） | task-3/task-5 |
| R13 | **能力被 `BuildConfig.DEBUG` 绑死**（RC P0-c：release 密钥恒 null） | 任何「debug 才启用」的开关 | release 包功能缺失且静默 | `SukeyProvider` 不读 `BuildConfig.DEBUG`；CI 增加 release 包端到端 case（RC §5.2 断言 6） | task-4/task-8 |
| R14 | **UI 日志随页面/状态卸载**（RC P1 的机制） | 日志状态用瞬态布尔驱动 | 用户看不到错误 | §5.7-L5：状态机 `FAILED` 常驻 + 日志落盘 + 独立 LogViewer | task-4 |
| R15 | **`init_boot` 与 `boot` 分区拆分**（Android 13+ ramdisk 迁到 init_boot） | 目标机型为 A13+ | 选错目标 → rc 注入到无 ramdisk 的分区，开机不拉起 AP（内核 patch 成功、root 可用、模块不可用） | **已裁（D-12）**：步 2 同时探测两者；步 7 **优先 init_boot 的 ramdisk、boot 回退**，实际目标记入 manifest；两者皆无则报 E-RC-18 并明示 | task-3/task-5/qa |
| R16 | **rc 注入厂商兼容性**（`init.rc` 结构非标准、`import` 位置、second_stage 校验） | 厂商定制 ROM（魅族 21 等） | 开机不执行 AP → 模块能力全失效 | 注入点=文件尾部（最小侵入）+ 原文件入 manifest；失败可「跳过注入」；真机矩阵优先魅族 21（DOC-05 R1 同源风险） | task-5/qa |
| R17 | **AVB 刷入路径被误用**（历史上曾有 vbmeta/disable-verification 方案，已推翻） | 任何 vbmeta 写入或 `--disable-verification` 都会**清数据/触发 bootloader 状态变更** | 用户数据丢失、设备状态异常 | **已裁（lead 2026-11-08）**：唯一主路径 `fastboot flash boot --slot=all`；**禁止**任何 vbmeta/disable-verity/disable-verification；产物必须等长 + AVB 区段逐字节保留（§5.4.1 AVB-1~AVB-4 硬断言）；UI/文档/脚本静态检查禁词 | ✅ 已裁 |
| R19 | **CI 唯一构建源带来的反馈延迟** | 本地禁止编译（人类硬规则） | 每个改动要等 CI 反馈，AI 无法本地验证 | §8：CI 分 job + 失败即早停；本地仅跑静态检查（`ktlint`/`cargo check` 不允许产出 APK）；每轮改动附「CI 预期结果」清单 | task-8 |
| R20 | **Keystore/密码入私仓的安全边界** | 方案 A（人类批准） | 仓库泄露即签名权泄露 | 仅 private 仓库；`keystore.properties` 不入公开镜像；CI secrets 仅 release job 使用；轮换预案 | 人类/task-8 |
| R18 | **管理器误当 root 提供者**（绕过 root-exec 自行提权） | 开发者图省事 | 密钥暴露面扩散、审计缺失 | 代码评审门禁：`grep -r "execve\|Runtime.exec(\"su\")\|ProcessBuilder(\"su\")"` 在 newapp 中必须 0 命中（除 root-exec 封装外） | 架构/task-4 |
| R22 | **密钥 ABI 混用**（本项目 47+NUL vs `Skroot-Next` 48 字节无 NUL） | 用户拿另一分支 patched 镜像套本项目密钥（或反之） | **提权静默失败**（无崩溃、无内核日志，极难排查） | §3.5.4 醒目注记 + `SukeyProvider` 只存 47 字符语义密钥 + 诊断页显示 `key ABI = 47+NUL`；质测强制用例「错体系密钥 → 提权失败且静默」；Release Notes/FAQ 写明「密钥不可跨体系套用」 | 架构/task-4/task-7 |
| R23 | **`engine/` 再许可合规风险**（SKRoot proprietary 移植 + 整仓 AGPL-3.0 发布） | 人类授权未明确覆盖「可按 AGPL 再分发」 | 公开发布即构成对 proprietary 部分的越权许可 | §11.2 三种处置；**未确认前公开发布物必须剥离 `engine/`**；`release.yml` 加发布物清单门禁；`engine/` 派生文件不加 SPDX/AGPL 头 | 人类/律师 + task-8 |
| R24 | **apd 依赖 UNKNOWN 许可**（`prop-rs` / `prop-rs-android` 0.2.0，git 依赖，无 LICENSE） | 上游不补许可 | `resetprop` 相关发布物法律状态不明 | **已缓解（lead 裁决）**：做成可选 feature `resetprop`（默认开）；`--no-default-features` 产出干净构建；`apd.yml` 每次编译校验该路径；**公开版走 --no-default-features**；人类待办：授权或自研替代（§11.4.4） | 人类 + task-6 |

### 7.1 RC §三 十项规避清单（F1–F11）落位对照

| RC | 规避要求 | 本文落位 |
|---|---|---|
| F1 | 缺件被静默跳过 → 需入口能力自检（存在/可执行/架构） | §5.2.1、§5.4 步 1、L1/L3 |
| F2 | 引擎交付形态与契约脱节 → 定为 JNI 主路径 + CI 断言架构 | §5.2（JNI 为主）、§5.2 实现要点 6、L1 |
| F3 | 构建期网络下载当交付 → 禁 skip-on-failure（magiskboot 由 CI 按 sha256 拉官方 release，失败即 fail build） | §5.7-L2、R21 |
| F4 | 非必需资产（kpimg）硬依赖 → 降级为 optional | §5.7-L4 |
| F5 | 失败无日志落盘、面板卸载 → 日志即产物 | §5.4「日志落盘要求」、L5、R14 |
| F6 | `echo; exit $?` 吞码、abort 用 exit 0 → 去脚本层 | §5.4「shell 与 busybox 处置」、L6 |
| F7 | 未校验产物就宣布成功 → 成功判据=可验证事实 | §5.4 步 6/8、§5.6-③ 注、L6 |
| F8 | 能力被 DEBUG 绑死 → release 可用 | R13 |
| F9 | `filesDir` 内 exec 被拦 → **magiskboot**（打包主路径工具）与诊断模式引擎的 exec 目标须在 `nativeLibraryDir`；补丁引擎走 JNI | §5.2 资产表、R12 |
| F10 | 壳层传参歧义（`sh -c` vs 直传）→ 单一调用路径 | §5.2 实现要点 3、§5.4 总原则 |
| F11 | 日志/环境不可复现 → 记录 cmdline/env/cwd、stderr 分列 | §5.4「日志落盘要求」 |

---

## 8. 构建与产物策略（**CI 唯一可信构建源**）

> **人类硬规则（2026-11-08）**：**禁止本地编译 APK / 镜像**，所有构建统一走 GitHub Actions。本节为实施基线。

### 8.1 原则

| # | 原则 | 落地 |
|---|---|---|
| G1 | **单一可信构建源 = CI**：任何交付物（APK / 引擎 .so / AP 二进制 / 镜像）只能由 CI 产出，本地不得作为交付来源 | 仓库内不放 `app-debug.apk`/`*.so` 二进制产物；`.gitignore` 显式忽略构建输出 |
| G2 | **本地只做静态检查** | 允许：`ktlint`/`detekt`/`cargo check`/`clang-tidy` 等**不产出交付物**的检查；禁止：`assembleDebug`/`cargo build --release` 产物入库或分发 |
| G3 | **产物可追溯** | 每个 artifact 名含 `版本号 + git commit short + run id`；同时产出 `SHA256SUMS` |
| G4 | **失败即早停** | 构建矩阵按「静态检查 → 引擎 → AP → APK → 签名」顺序，前面失败不跑后面（省时间、错误清晰） |
| G5 | **不可信输入不参与构建** | 所有第三方二进制（若有）必须 sha256 固定；网络下载仅作辅助且失败即 fail（RC F3） |
| G6 | **分支策略：`v2` 为唯一开发分支**（lead 2026-11-08 决定） | `main` 是旧代码 + app-client 早期推送，**不作为测试/构建对象**；所有 workflow 的 push/PR 触发只监听 `v2`（及 tag） |

### 8.2 CI 工作流清单（GitHub Actions）——**按 v2 实况修正（5 个，非设计态 6 个）**

> 设计态曾列 6 个工作流（`ci-static/engine/magiskboot/apd/newapp/release`）。v2 实际为 **5 个**：`apk / engine / apd / licenses / release`。`ci-static` 未落地（静态检查是缺口，见 §3.7.2 D7）；`magiskboot` 无独立 workflow，改为在 `apk.yml` 内按 `app/native.lock.json` 精确拉取并 sha256 校验。

| 工作流（v2 实际文件名） | 触发 | 产出 | 关键断言 | 归属 |
|---|---|---|---|---|
| `apk.yml` | `app/**`、`app/native.lock.json`、根 LICENSE/NOTICE 变更 | `SPRoot-APK`（release APK，签名） | 按 `native.lock.json` 精确拉取 engine/apd/magiskboot + **sha256 逐一校验**；装配 NOTICE assets（单一可信源）；`apksigner verify`；APK 内含所需 **AArch64 natives** 断言 | task-4/8 |
| `engine.yml` | `engine/**` 变更 | `libskrootpatch.so`（arm64）+ `skroot-patch`（x86-64 诊断） | `readelf` 断言 AArch64；`--selftest` 返回 0；符号清单 + sha256；**依赖 apt `qemu-user`（提供 qemu-aarch64）做契约自检**——⚠️ 不是 `qemu-user-static`（无安装候选，首次 CI 失败原因） | task-3/8 |
| `apd.yml` | `apd/**` 变更 | `libapd.so`（arm64） | ELF 断言（AARCH64+ET_DYN）；能力标记 grep；体积归因；`--no-default-features` **licence-clean 编译校验**（§11.4.4）；sha256 | task-6/8 |
| `licenses.yml` | LICENSE/LICENSES.md/NOTICE/`engine/src/**`/检查脚本变更 | 无产物（只报告） | LICENSE 存在且为 AGPL-3.0；LICENSES.md 含「保留所有权利」+ SKRoot 来源；NOTICE 含 AGPL-3.0 + magiskboot；`check_licenses.py` 上游 **15 文件 sha256 指纹未漂移** | task-8 |
| `release.yml` | tag `v*` | Release assets + SHA256SUMS | 上述汇总 + 版本号与 tag 一致 | task-8 |

> **产物依赖顺序**：`engine.yml` / `apd.yml` 的 artifact 由 `apk.yml` 按 `native.lock.json`（含 `run_id` + sha256）消费；**禁止**在 `apk.yml` 里现场编译 Rust/C++。`magiskboot` 同样走 `native.lock.json` 锁定（Magisk v31.1 官方 ARM64，sha256 记录）。

### 8.3 产物管理与发布

| 项 | 规格 |
|---|---|
| Artifacts | CI 保留 30 天；命名 `sproot-<component>-<version>-<commit8>-<runid>` |
| 校验 | 每次构建产出 `SHA256SUMS`；APK 另附 `apksigner verify --print-certs` 输出 |
| 发布 | 仅 tag 触发 `release.yml`；Release Notes 含：版本、commit、引擎/AP 版本、支持的 kernel 区间、已知机型限制 |
| 版本绑定 | `version.properties` 里的 `versionCode/versionName` 在 CI 中由 tag 校验（不一致即 fail） |

### 8.4 密钥与签名（方案 A：keystore 入 private 仓库，人类已批准）

| 项 | 做法 | 风险控制 |
|---|---|---|
| keystore | `sproot-release.jks`（人类已生成于 `~/.ssh/`）提交进 **private 仓库** | 仅 private 仓库；公开镜像/分叉不得包含 |
| 密码 | `keystore.properties`（不入库？→ 方案 A 明确允许入私仓）由 CI 从仓库读取 | 建议仍用 Actions Secrets 覆盖仓库内值（双保险）；`SPROOT_SIGNING.md` 记录轮换步骤 |
| 签名校验 | CI 内 `apksigner verify --strict` + 打印证书指纹 | 指纹与人类登记值比对 |
| 泄露预案 | 立即轮换 keystore + 重新发布；旧版本标注撤销 | 人类决策 |

### 8.5 风险（本地不可验证）

| # | 风险 | 缓解 |
|---|---|---|
| C1 | AI/开发者**无法本地验证编译结果**，编译错误只能等 CI 反馈 | 每轮变更前先跑「静态检查 + 依赖解析」最小 job；PR 描述里写「CI 预期通过项」；失败日志贴回上下文 |
| C2 | CI 环境与真机差异（NDK/SDK 版本） | 工具链版本在仓库锁定（Gradle wrapper、`ndkVersion`、rust-toolchain.toml）；CI 镜像版本随 PR 显式升级 |
| C5 | **AGP 9 新 DSL（`newDsl`）与 KGP 的已知冲突**（首次 CI 构建失败原因，lead 2026-11-08 已修 `gradle.properties` 并推 v2 重跑） | `gradle.properties` 中的开关配置入库并附注释；CI 失败首查 `gradle.properties` 与 AGP/KGP 版本对；把「最小 Android 工程能 assembleDebug」作为 M1 门禁（避免后续所有 job 被同一问题反复阻塞） |
| C3 | 首次 CI 全绿前的「盲改」成本 | 先建最小骨架（仅编译一个空 APK）打通流水线，再逐步加断言（里程碑 M1） |
| C4 | Release 构建无人本地试装 | Release 也必须由 CI 出；真机安装验证列入 qa 门禁（task-7） |

---

## 9. 待决项（需 leader/人类裁决或专项调研）

| ID | 事项 | 选项 | 建议 | 裁决人 | 阻塞谁 |
|---|---|---|---|---|---|
| D-1 | 新工程 `compileSdk` 下限 | 37 / 更低 | **37**（miuix `minCompileSdk=37` 硬要求）；**已随 app-client 定版落定**（AGP 9.4.1 / Kotlin 2.4.20 / compileSdk 37 / minSdk 26 / Gradle 9.7.1 / JDK 21） | ✅ 已定 | — |
| D-2 | **打包工具链**（人类新指令 2026-11-08） | 自研 bootimg / magiskboot / 两者结合 | **✅ 已定**：**引入 magiskboot（Magisk v31.1 官方 ARM64）**作打包主路径（unpack/repack/cpio），**原图作模板**；自研 bootimg 保留为交叉校验/回退；**禁用 AIK 与非官方 Windows 版**（§5.3） | ✅ 人类已定 | task-3/task-5 |
| D-3 | **许可证策略** | 开源 / 闭源 | **✅ 已定（人类 2026-11-08）**：自有代码 = **AGPL-3.0-only**（SPDX）；仓根 `LICENSE` 全文 + `NOTICE` + `README` 许可段已落盘；**遗留：`engine/` 为 SKRoot proprietary 移植，再许可范围需人类/律师确认**（§11.2：未覆盖则发布前剥离） | ✅ 人类已定 + ⚠️ 再许可待确认 | 发布门禁 |
| D-4 | ~~密钥经 `argv` 传给引擎~~ **已解决** | — | 改为「临时 0600 文件 + 用后即删」（DOC-05 §4.2）；`root-exec --superkey-file` 同样走文件；**密钥不进 argv** | ✅ 架构已定 | — |
| D-5 | 首装刷入路径的产品口径 | 仅导出镜像 / 支持直刷（经 root-exec，需已有 root/AP）/ 引导 fastboot | **导出为默认，直刷为高级选项**（风险与体验平衡） | leader + 人类 | task-5 |
| D-6 | 目标机型首批清单 | 魅族 21（已知）+ 其他 | 需人类给首批 3~5 台；否则 R15/R16 无法收敛 | 人类 | qa/task-3 |
| D-7 | 模块页 v0.1 是否保留 | 保留（AP 子命令透视）/ 先只读状态 | **保留只读 + 安装/启停**，卸载与动作脚本下版 | leader | task-4/task-6 |
| D-8 | miuix 组件 API 细节 | 调研文件/自测 | 以 `docs/research/miuix-integration.md` + 首次真机截图回归为准 | research/task-4 | task-4 |
| D-9 | ~~引擎最终形态裁定~~ **已定（lead 2026-11-08）** | JNI 进程内为主，子进程为诊断/回退 | **落定**：`libskrootpatch.so` JNI 主路径；CLI `main()` 保留供诊断/PC/CI（§5.2） | ✅ lead 已裁 | task-3 按此实现 |
| D-10 | kernel 压缩态处理 + 镜像来源 | ①打包链内解压→patch→按原参数压回（默认）②要求用户提供已解压镜像 ③扩展支持压缩态 | **①**（magiskboot 主路径内置解压/回压；自研 bootimg 回退时同样实现）；首批机型验证重打包后尺寸 | 架构 + task-3 | task-3/task-5 |
| D-11 | 是否保留「自定义 48 位密钥」入口 | 只自动生成 / 允许用户输入（强校验） | **允许输入但默认生成**（密钥即隐藏目录，用户自定义可能撞弱口令 → 加熵提示） | leader | task-4 |
| D-12 | `init_boot` 场景的镜像引导 | 只接受 boot.img / 同时支持 boot.img 与 init_boot.img（A13+）/ 自动探测 | **已裁（lead 2026-11-08）**：本机 `init_boot.img`（8MB，ramdisk 形态）存在 → **RcInject 优先 init_boot 的 ramdisk**（A13+ 通用 ramdisk 在 init_boot），`boot.img` ramdisk 回退；**实现中两者都探测并在 manifest 记录实际目标** | ✅ lead 已裁 | task-3/task-5/task-7 |
| **D-13** | 采纳 KSU 三个修补参数语义 | 采纳 / 不采纳 | **采纳**：①`--partition` 显式指定目标分区（不猜）②`--backup` 强制备份 ③`--no-install` 只改配置不重注入；在 Kotlin 编排层实现（引擎 CLI 可增可选参数） | 架构（已建议） | task-3/task-5 |
| **D-14** | **rc 注入是否做**（lead「SK 不改 ramdisk」与 AP 自启的张力） | A：流水线内 RcInject（magiskboot cpio）/ B：最终镜像 ramdisk 一字不改，AP 不自启 | **✅ 已裁（lead 2026-11-08：采纳 A）**：固定表述「引擎 patch kernel 段；**同一流水线的 RcInject 步骤**写 ramdisk（`init.sproot.rc` + `init.rc` import）；两步骤独立，SK 内核运行时零 ramdisk 依赖」。隐藏性：ramdisk 只加 2 文件、不碰 policy、不改 AVB 逻辑 | ✅ lead 已裁 | task-3/task-5 |
| **D-15** | **AVB/首次刷入与「无 PC」目标** | ①接受首次刷入需 PC ②尝试纯手机路径 ③卡刷包兜底 | **✅ 已裁（lead 2026-11-08，AVB 结论更新）**：MVP 首次刷入必须 PC，命令**仅** `fastboot flash boot --slot=all` + `reboot`；**禁止任何 vbmeta / disable-verity / disable-verification**（清数据来源）；「全程无 PC」列 **P2 增量**（应用内 dd 需已 root/Shizuku）；修补本身 100% 本地 | ✅ lead 已裁 | task-5/qa |
| **D-16** | 卡刷包（APK 兼作 recovery zip）是否本期做 | 做 / 不做 | **做**（成本低、救援价值高）；依赖 CI 产出 update-binary（§8.2 可加 job） | leader | task-8 |

---

## 10. 交付拆解与依赖关系（与任务板对齐）

| 任务 | 与本方案的关系 | 关键依赖 |
|---|---|---|
| task-1（本文） | 架构基线 | — |
| task-2 | 根因已确证并已并入本文（§5.7/§7.1/A-18~A-22）；剩余：给最小可验证链 | ✅ 已消费其结论 |
| task-3 | 落地 §5.2/§5.3：引擎 = **JNI 主路径 + CLI 诊断**；**magiskboot v31.1 集成（exec）** + **自研 bootimg 交叉校验/回退**（等长保留/压缩归一/RcInject）+ 12 退出码 | 依赖 D-2/D-9/D-10（D-2/D-3 已裁） |
| task-4 | 落地 §4（miuix）与 §3.1 L1/L2 骨架 + §3.6 状态机/能力门禁 + 常驻错误面板/日志页 | 依赖 D-1、§4.2 依赖矩阵、R13/R14/R18 |
| task-5 | 落地 §5.4 八步链（RcInject 必做，优先 init_boot）+ §5.4.1 AVB + §3.5.3 首装三段式 | 依赖 task-3 产物 + D-5/D-12/D-15（D-12/D-15 已裁） |
| task-6 | 落地 §3.1 L4（本文已消费其 NEW-DOC-05 结论，含 E1–E5） | ✅ 已对齐（DOC-05 v1.0 草案） |
| task-7 | 把 §5.6/§5.7/§7.1/§5.8/§3.5/§3.6 转成测试项；**新增密钥 ABI 错配用例（§3.5.4/R22）** | 本文 + RC §5.2 断言表 + DOC-05 §9 |
| task-8 | **CI 唯一构建源**（§8）：`ci-static/engine/magiskboot/apd/newapp/release` 六工作流 + 架构/断网/入包断言 + 签名 + **LICENSE/NOTICE 断言（§5.3.4④/§11.3）** | §4.6-R3、§5.7-L1/L2、A-22、A-30 |
| task-9（新） | 契约 v1.1 正文（已 FROZEN；本文 §6.4 为差异依据） | ✅ 已冻结（2026-11-08 21:23） |

**建议的里程碑顺序**：M1 = 最小 CI 流水线跑通（空 APK 可编译，task-4+task-8）→ M2 = 引擎 arm64/JNI 产物 + 自研 bootimg 单测（task-3）→ M3 = 八步链打通（task-5）→ M4 = 真机刷入/AVB/回滚（task-5+qa）→ M5 = 首次部署 T2 + 授权与模块（task-5+task-6）。

---

## 11. 许可与合规（**AGPL-3.0 定案 + SKRoot 再许可风险**）

> 依据：人类 2026-11-08 决定「自有代码用 **AGPL-3.0**」；lead 要求同时标注 `engine/` 的再许可风险。本节是**发布前必读**。

### 11.1 现状（已落盘）

| 文件 | 内容 | 状态 |
|---|---|---|
| 仓库根 `LICENSE` | SPDX 头 `AGPL-3.0-only` + 说明 + **AGPL-3.0 全文**（≈34 KB，自 SPDX 官方文本拷贝） | ✅ 已落盘 |
| 仓库根 `NOTICE` | 第三方声明：magiskboot（GPL-3.0，源码链接）、Kotlin/Compose/AndroidX/miuix（Apache-2.0）、asmjit（Zlib）、capstone（BSD-3）、cJSON（MIT）、**SKRoot（proprietary + 风险提示）**、KernelPatch/APatch/KernelSU（仅参考未复制）、禁用项（AIK/非官方 magiskboot） | ✅ 已落盘 |
| 仓库根 `README.md` | 项目简介 + **许可段（SPDX `AGPL-3.0-only`）** + 再许可风险提示 + 风险与能力边界摘要 + 目录说明 | ✅ 已落盘 |

**许可兼容性结论**：
- **AGPL-3.0-only 与 GPL-3.0 兼容**：AGPL-3.0 §13 第 2 段明确允许与 GPL-3.0 作品链接/组合；magiskboot 仍按「独立程序聚合」分发（不链接、不改写），双保险。
- Apache-2.0 / MIT / BSD-3 / Zlib 均可与本项目 AGPL-3.0 组合分发，但**必须保留其声明**（`NOTICE`）。
- **GPL-2.0-only 与 AGPL/GPL-3.0 不兼容**（例如若未来复制 KernelPatch 的 `bootimg.c`，会引入冲突）→ 因此本项目只「参考」不「复制」；若必须复制，需改用 `GPL-2.0-or-later` 或放弃该代码。

### 11.2 ⚠️ 核心风险：`engine/` 的再许可（必须由人类/律师确认）

**事实**：
- `engine/` 是**移植自 SKRoot 上游源码**（`abcz316/SKRoot-linuxKernelRoot`，**proprietary，无 LICENSE/SPDX**）。
- 本仓库整体声明 **AGPL-3.0-only** = 等于把 `engine/` 这部分也按 AGPL 对外授权。
- 人类答复是「我有作者授权，可以调用它」——**「可调用」≠「可再许可/可按 AGPL 对外分发」**。

**三种处置（按授权范围选择）**：

| 情形 | 判定 | 处置 |
|---|---|---|
| **A. 授权文本明确覆盖「可再许可 / 可按 AGPL（或任意 OSI 许可）分发」** | 合规 | 保持现状：整仓 AGPL-3.0；`NOTICE` 第 7 条保留「已获授权」陈述（不附授权原文） |
| **B. 授权只覆盖「调用/修改/自用」，未覆盖再分发再许可** | **不完全合规** | **公开发布前必须把 `engine/` 从发布物中剥离**：①仓库转为 private，或 ②公开仓用 `engine/` 的**二进制/接口占位**（`libskrootpatch.so` 由 CI 从私有源构建），源码不公开；自研部分（管理器/打包链/文档）仍可按 AGPL 公开 |
| **C. 仅以二进制形式分发 engine，主张不构成衍生作品** | 法律定性不确定 | 需律师意见；在结论出来前按 B 处理（更保守） |

**工程侧强制约束（立项即生效，不依赖法律结论）**：
1. `engine/` 目录**不得**出现在对外发布物（Release source tarball / 公开镜像）中，除非 A 条件成立。
2. CI 的 `release.yml` 增加「发布物清单检查」：若 `engine/` 源码被打包进发布物，**必须**同时存在人类签署的授权确认记录（否则 fail）。
3. 任何 `engine/` 派生代码的文件头**不得**自行加 `SPDX-License-Identifier: AGPL-3.0` 声明（会构成对外授权声明）——保持无 SPDX，由根 `LICENSE` 的例外说明（§11.2）承接。
4. `NOTICE` 第 7 条与 `LICENSE` 头部说明必须**同步更新**（CI 断言：两处均含 `proprietary` 与 `SKRoot`）。

### 11.3 发布前检查清单（Release 门禁，逐项打勾）

- [ ] 仓库根 `LICENSE` 为 AGPL-3.0 全文且含 SPDX 头；`NOTICE` 与 APK 内一致
- [ ] `README` 许可段 SPDX 标识正确（`AGPL-3.0-only`）
- [ ] **SKRoot 再许可授权已确认**（A 情形）或 **`engine/` 已从发布物剥离**（B/C 情形）
- [ ] magiskboot：来源 v31.1 / GPL-3.0 / 源码链接 / sha256 齐全且未修改
- [ ] capstone 包内 LICENSE.TXT 已核对并附带
- [ ] 无 GPL-2.0-only 代码被复制进仓库（KernelPatch/APatch/KernelSU 仅参考）
- [ ] 发布物不含真实用户密钥、不含 keystore/密码
- [ ] Release Notes 含「刷机风险与能力边界」摘要（§1.4）与准确 fastboot 命令
- [ ] **AP 二进制 `apd` 的 Rust 依赖已闭环**：`apd/THIRD_PARTY_LICENSES.md` 已更新；`extattr`（GPL-2.0-only）确认已移除；**`prop-rs` / `prop-rs-android`（UNKNOWN）已解决或含 resetprop 的构建不发布**（§11.4）

### 11.4 apd（AP 二进制）Rust 依赖合规（115 个，2 项阻断）

> 来源：`apd/THIRD_PARTY_LICENSES.md`（sys-guard 生成，`Cargo.lock` + `cargo metadata --filter-platform aarch64-linux-android`，v2 commit `8b28e87`）。根 `NOTICE` §8 已汇总。

**11.4.1 汇总（按项计次）**：MIT 100 · Apache-2.0 88 · Zlib 5 · Unlicense 5 · ISC 2 · LLVM-exception 2 · BSD-3-Clause 1 · BSL-1.0 1 · CC0-1.0 1 · Unicode-3.0 1 · **UNKNOWN 2**。合计 115 包；**除标记项外全部宽松许可，与 AGPL-3.0 兼容**。

**11.4.2 已消除不兼容**：`extattr 1.0.0`（crates.io，**GPL-2.0-only**，与 AGPL-3.0 不兼容）→ 已用 raw `libc` 的 `lsetxattr/lgetxattr` 等价替换（`apd/src/restorecon.rs`，wire 行为不变），并从 `Cargo.toml`/`Cargo.lock` 移除。**该项已闭环，保留记录供审计**。

**11.4.3 ⚠️ 阻断项：2 个 UNKNOWN（`prop-rs` / `prop-rs-android` 0.2.0）**

| 项 | 事实 | 影响 |
|---|---|---|
| 来源 | git：`AndroidPatch/restprops` rev `6f57231…` | — |
| 许可 | **UNKNOWN**：上游仓库无 LICENSE 文件、`Cargo.toml` 无 `license` 字段 | 法律状态不明 = 不得对外分发 |
| 用途 | `resetprop` 功能（bionic property area 操作） | 不易直接替换；移除会破坏模块生态脚本兼容 |
| 检测盲区 | `quality/static/check_licenses.py` 只校验 `LICENSES.md` 指纹，**查不到 UNKNOWN** | 必须人工清单勾选 |

**处置（三选一，需人类/architect 决策）**：
1. **向上游作者要求补 LICENSE 或书面授权**（推荐；与 SKRoot 授权凭证一并归档）；
2. **自研最小替代**（仅需 bionic property area 读写；task-6 评估工期）；
3. 移除 `resetprop` 功能（**不建议**，破坏模块兼容）。

**门禁**：闭环前**含 `resetprop` 的发布物不得公开发布**；`apd/Cargo.toml` 已标注 `license = "AGPL-3.0-only"`（与根 LICENSE 一致）。

**11.4.4 工程处置（lead 2026-11-08 夜间裁决，✅ 已落地于 v2）**

裁决：**不删 resetprop、不阻塞开发**；把 `prop-rs`/`prop-rs-android` 做成**可选 feature**，为发布侧留干净路径。

| 项 | 实现（v2 HEAD 实况） | 验收 |
|---|---|---|
| `apd/Cargo.toml` | `[features] default = ["resetprop"]`、`resetprop = ["dep:prop-rs", "dep:prop-rs-android"]`；两个 crate 均声明为直接依赖且 `optional = true`（同一 git rev，Cargo 去重为单实例） | `cargo metadata` 显示 features 正确；`cargo tree --no-default-features` 下 `prop-rs` 0 命中 |
| 代码条件编译 | `apd/src/resetprop.rs` 改为 **façade**：`#[cfg(feature="resetprop")] #[path="resetprop_prop_rs.rs"]` 编译真实实现；`#[cfg(not(...))]` 编译 **stub**，调用该子命令时**显式报「feature 未启用」**（不静默降级）。`cli.rs` 的 argv0 分派与 `Commands::Resetprop` 无需改动（façade 保持同一 API） | `cargo check` 默认与 `--no-default-features` **均通过**（本地 WSL 在 `05e626a` 上实测两次 Finished） |
| CI 防腐化 | `apd.yml` 每次执行 `cargo check --locked --no-default-features --target aarch64-linux-android`（**仅编译校验，不替换发布产物**） | CI 每次跑该步；失败即红 |
| 发布策略 | **对外公开发布必须用 `--no-default-features` 构建**（依赖树不含 UNKNOWN）；默认构建（含 resetprop）仅限内部/自用，**不作为 release candidate** | Release Notes 标注构建 feature 集；`release.yml` 校验产物来源 |

> 说明：`prop-rs` 与 `prop-rs-android` 是同一 `AndroidPatch/restprops` 仓库内的两个 crate（同 rev）。v2 当前把两者都声明为直接 optional 依赖并在 feature 中逐一声明——与 lead 的 `dep:prop-rs` 写法一致，比「只 gate 直接依赖、靠传递消失」更显式，二者行为等价。

**仍未闭环（人类 2026-11-09 待办）**：`prop-rs` **授权 / 自研替代 二选一**，与 SKRoot 授权一并归档；在此之前的对外发布一律走 `--no-default-features`。

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
| A-27 | **契约 v1.1 三方会签 + 两条边界**（修补链/特权链分离；`libskrootpatch.so` 与 `libapd.so` 职责不可互换）；lead 2026-11-08 通报 | lead 团队消息（2026-11-08）——**非文件证据，属决议记录**；契约正文由 task-9 落盘（**已 FROZEN 2026-11-08 21:23**） |
| A-28 | **app-client 工具链定版**：AGP 9.4.1 / Kotlin 2.4.20 / compileSdk 37 / minSdk 26 / Gradle 9.7.1 / JDK 21 | lead 2026-11-08 通报（newapp 骨架已落盘） |
| A-29 | **APatch 管理器实测**：10 态状态机 + 页面能力门禁、`PatchMode` 四模式、`ori.img` 闭环、superkey-as-argv 反模式、双路径拼串反模式、KPM 双视图、APK 兼卡刷包 | `docs/research/apatch-manager-architecture.md` §1.3/§2.2/§3.2/§5.1/§5.2（基线 `e8e4651c`） |
| A-30 | **人类硬规则**：禁止本地编译 APK/镜像，构建统一走 GitHub Actions；keystore 入 private 仓库（方案 A） | 人类指令（lead 2026-11-08 转达）——决议记录，无文件证据 |
| A-31 | **AVB 结论（2026-11-08 更新，推翻 v0.5 旧路径）**：三家实现均不重签 AVB，靠解锁 bootloader 跳过验证；vbmeta 对 boot 仅 **Chain descriptor（RSA4096）**，boot hash descriptor 在**内嵌 vbmeta**，算法 **SHA256_RSA4096**，无魅族私钥不可重签；**`--disable-verification` 是清数据来源，禁用**。刷入仅 `fastboot flash boot --slot=all` + reboot；禁止任何 vbmeta/disable-verity/disable-verification | lead 2026-11-08 通报（**结论记录**；真机复核 footer 偏移与分区尺寸 100663296/8388608） |
| A-32 | **lead 四项裁决（2026-11-08）**：D-14 采纳解读 A（引擎 patch kernel + 同流水线 RcInject 写 ramdisk，两独立步骤，SK 运行时零 ramdisk 依赖；ramdisk 只加 2 文件、不碰 policy、不改 AVB 逻辑）；D-15 MVP 首次刷入必须 PC；D-12 `init_boot.img`（8MB）存在→RcInject 优先 init_boot、boot 回退；D-3 改为「人类已获作者授权」（见 A-34） | lead 团队消息（**决议记录**，无文件证据） |
| A-34 | **D-3 许可证定论（人类 2026-11-08）**：「我有作者授权，可以调用它」→ SKRoot 源码 proprietary（无 SPDX），本项目使用已获上游作者授权（授权记录由人类持有，不入库），无开源阻塞 | 人类答复（经 lead 转达，**决议记录**） |
| A-35 | **engine CI 依赖更正**：`engine.yml` 需 apt 安装 **`qemu-user`**（提供 `qemu-aarch64`，可运行静态 arm64 产物）；**不是 `qemu-user-static`**（ubuntu-latest 无安装候选，首次 CI 失败原因） | lead 2026-11-08 通报（**更正记录**） |
| A-36 | **人类新指令（2026-11-08）**：打包**用 magiskboot（ARM64）**，**禁用 AIK（Android.Image.Kitchen，不支持 Linux 6.0+ 内核）与非官方 Windows 版 magiskboot**；magiskboot 作独立可执行组件（「独立程序聚合」+ NOTICE，GPL-3.0 不传染） | 人类指令（经 lead 转达，**决议记录**） |
| A-37 | **magiskboot 来源锁定**：Magisk **v31.1** 官方 ARM64 release 产物；sha256 记录于 `binaries.json`；构建期 `readelf` 断言 AArch64 | 人类指令 + CI 规格（版本号由人类指定） |
| A-38 | **人类许可决定（2026-11-08）**：自有代码 = **AGPL-3.0**（默认 `AGPL-3.0-only`）；要求 LICENSE 落全文而非仅名字；README 加许可段 | 人类指令（经 lead 转达，**决议记录**） |
| A-39 | **⚠️ `engine/` 再许可风险**：`engine/` 移植自 SKRoot 上游（proprietary、无 LICENSE）；整仓 AGPL 发布 = 对其再许可；人类「可调用」授权未必然覆盖「可改许可对外分发」→ 未覆盖时须剥离 `engine/` 或改二进制分发（法律定性待确认） | lead 2026-11-08 风险提示（**决议记录**）；§11.2 三种处置 |
| A-40 | **密钥 ABI 双体系**：本项目 = **47 字符 + NUL**（对外展示 48 位）；`Skroot-Next` 风格 = **48 字节无 NUL、按实际长度比较** → **不可混用**，错配表现为提权静默失败 | lead 2026-11-08 调研通报（**结论记录**）；本文 §3.5.4 + R22 |
| A-41 | **仓根合规文件已落盘**：`LICENSE`（SPDX `AGPL-3.0-only` + AGPL-3.0 全文，34 KB 自 SPDX 官方文本）、`NOTICE`（第三方清单 + SKRoot 风险 + 禁用项 + 发布检查清单）、`README.md`（许可段 + 风险摘要） | 本工作区文件（2026-11-08 创建） |
| A-42 | **apd Rust 依赖清单**：`apd/THIRD_PARTY_LICENSES.md`（115 个 Android 目标传递依赖，`Cargo.lock` + `cargo metadata --filter-platform aarch64-linux-android`，commit `8b28e87`）；全部 permissive 除：`extattr`（GPL-2.0-only，已移除并等价替换）、`prop-rs`/`prop-rs-android`（UNKNOWN，**阻断发布**） | sys-guard 2026-11-08 交付；根 `NOTICE` §8 + 本文 §11.4 |
| A-43 | **v2 apd feature 隔离实况**（commit `d45822b` / `05e626a`）：`Cargo.toml` features `default=["resetprop"]` + 两 crate optional；`resetprop.rs` façade + `resetprop_prop_rs.rs`；`apd.yml` 有 `cargo check --no-default-features --target aarch64-linux-android`；本地 `cargo check` 默认与 `--no-default-features` 均 Finished | 本工作区 WSL 实测（2026-11-08）+ `origin/v2` |
| A-44 | **客户端 v2 实现清点**（本文件 §3.7）：四页导航、8+1 步修补流水线（magiskboot + JNI patch + sha1 备份 + 等长/sha256 校验）、入口能力自检、引擎自检五元组、模块页 list/enable/disable/install/uninstall、AP selftest 首启、许可 assets 随包、`native.lock.json` 可复现锁定、CI 五工作流 | `app/` 源码 + `app/native.lock.json` + `.github/workflows/`（v2 `b93dd39` 实况核对） |
| A-33 | **构建与分支**：人类硬规则「禁止本地编译，统一 GitHub Actions」；`v2` 为唯一开发分支（`main` 为旧代码，不作测试对象）；首次 CI 失败根因 = AGP9 `newDsl` × KGP 冲突，lead 已修 `gradle.properties` | lead 团队消息（**决议记录**）；workflow YAML 由 task-8 落盘 |

## 附录 B · 与其它任务的边界（避免越界写盘）

- 本文只写 `docs/NEW-DOC-01-*`；**不修改** `engine/`（task-3）、`newapp/`（task-4/5/8）、`docs/NEW-DOC-02/03/05/06`（task-2/3/6/7）。
- 本文对 task-6（apd/守护）只定义**接口边界**（§3.1 L4 + §3.5），其实现细节与部署清单以 `docs/NEW-DOC-05-模块与守护方案.md` 为准。
- 本文对 task-7（质测）只提交**可测断言**（§3.6/§5.6/§5.7/§5.8/§8.2），不写测试用例正文。
- 本文对 task-8（CI）给出**目标流水线规格**（§8），具体 workflow YAML 由 task-8 编写。
- 调研输入（只读引用，不改写）：`docs/research/{patch-failure-rootcause,miuix-integration,kernelsu-architecture,apatch-manager-architecture}.md`。

---

*完 · 架构（architect） · 2026-11-08 · **v1.1.3 定稿**（v1.1.2 基础上新增 §3.7 客户端已实现能力清单 + §8.2 CI 实况修正 + D1–D7 设计/实现差异；v1.1.2 已含 §3.5.4 密钥 ABI 注记/R22、§1.4 刷机风险与能力边界、§11 许可与合规、§11.4 apd 依赖合规/R24）*
