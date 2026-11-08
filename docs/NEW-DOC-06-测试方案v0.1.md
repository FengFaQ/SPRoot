# 新 SP 管理器 测试方案 v0.1

- 文档编号：NEW-DOC-06
- 作者：质测（qa）
- 状态：v0.1 草案（待 leader 评审；已同步 NEW-DOC-05 v1.0 草案与契约 v1.1 起草口径；受 v1.1 影响的用例标 `acceptance_ready=false`，见 §0.4）
- 日期：2026-11-08
- 项目定位：**从零实现全新 SP 管理器**，仅在安卓手机本地完成 boot 镜像修补，UI 用 miuix，参考 KSU/APatch 交互范式。**旧路线（改造 APatch 管理器 + 手机内调 kptools）已放弃**，旧交付物不再作为被测对象。
- 读者：leader、架构（architect）、修补（patcher）、客户端（app-client）、系统（sys-guard）
- 输入依据：
  - `docs/SPROOT-DOC-A1-SKRoot机制深度分析.md`（SK 机制、五组 hook、48 位密钥、失败不落盘）
  - `docs/SPROOT-DOC-C1-修补引擎IO契约.md`（引擎 CLI/退出码/NDJSON/report/验收必查 7 项）
  - `docs/phase2_skroot_ap_call_contract_draft.md`（已定稿 v1.0：execve 提权判据、方案 B 无状态密钥、日志脱敏）
  - `docs/phase2_boot_sequence_analysis_draft.md`（SK 内核零事件能力 → AP 用户态事件引擎、rc 注入、`/data/adb/sproot/key`）
  - `docs/SPROOT-DOC-B1-KPM机制分析与SK可行性.md`（KPM 在 SK 上不可行 → 新项目不继承 KPM）
  - 旧项目测试方案 v0.3（资产 `a_4ebb6d652bc90921`）与 `quality/device-scripts/` 三脚本（复用见 §0.2）
  - `docs/research/patch-failure-rootcause.md`（旧链路「一闪即退」根因确证，2026-11-08 → §3A T1~T7）

---

## 0. 定位、边界与复用

### 0.1 本方案做什么 / 不做什么

**做**：把新项目的五条主线（本地修补链、Root 授权、模块生命周期、隐藏性、失败可见性）转成**可判定、可脚本化、可门禁**的测试项；定义无真机阶段的静态验证清单；定义魅族 21 真机序列、回滚与救砖预案；定义研发进入系统测试前必须交付的契约/产物门禁。

**不做**：
1. 不替研发做设计决策。凡本方案标注 **【TBD-n】** 的，是研发尚未定稿、质测不臆造的点，缺失时对应测试项 `acceptance_ready=false`。
2. 不继承旧 APatch 管理器的任何实现假设（旧 B6「SuperKey 移除反向验证」、K 组「KPM UI 隐显」**废止**）。
3. 不测 PC 端刷机工具链本身（fastboot 属环境前提，仅在回滚预案中出现）。
4. 不对抗任意第三方 Root 检测工具（隐藏性只验证「SK 设计承诺成立 + 与基线无差集」，检测样本仅记录事实，不判 Pass/Fail——沿用旧方案 Q4=B 口径）。

### 0.2 旧资产复用与废止清单（先读再用，不重复造）

| 旧资产 | 处置 | 新编号/位置 |
|---|---|---|
| `quality/device-scripts/s0_baseline_capture.sh` | **直接复用**（修补前基线快照） | S0 |
| `quality/device-scripts/sec1_unreadable_probe.sh` | **直接复用**（普通 uid 不可读密钥面） | SEC-1 / H-7 |
| `quality/device-scripts/h_group_probe.sh` | **直接复用**（运行态安全面导出，与基线 diff） | H-3/5/6/7 |
| 旧方案 v0.3 §3A 断言组 A-1~A-9 | 改造为 **V 组静态断言**（分工：交付脚本 `quality/static/verify_patch.py` 覆盖内部断言 V1~V7，其余手工/半自动） | §4 |
| 旧方案 v0.3 §4A H-0~H-11 | **沿用**（H-11 华为路径维持部分覆盖，进发布声明） | §8 |
| 旧方案 v0.3 §4A-SEC SEC-1~SEC-6 | **沿用**，SEC-3 按方案 B（密钥不常驻）口径调整 | §8 |
| 旧方案 v0.3 §6 E1~E9 | 保留并扩充 | §7 |
| 旧方案 v0.3 B1~B5 | 拆解改造：B1 → B-01（不刷入）/B-02（刷入开机）；B2→B-03；B3/B4→B-04 骨架 | §2 |
| 旧方案 v0.3 B6（SuperKey 反向验证）、K 组（KPM） | **废止**（新 App 从零、无 SuperKey；KPM 不继承） | — |

### 0.3 被测对象与交付形态（研发待确认项已标注）

| 组件 | 形态 | 状态 |
|---|---|---|
| 修补引擎 | `skroot-patch`：x86_64 宿主版已存在（`engine/src/skroot-patch`，2026-11-06 构建）；**arm64 版未交付**【TBD-3】 | 阻断 L0/L1 |
| 管理器 App | 全新工程（Kotlin + Compose + miuix），debug APK 由 CI 产出 | 待 task-4/5 |
| 本地修补链 | APK 内携带引擎 + boot 解包/重打包能力，五阶段：选镜像→解包→patch→重打包→导出/刷入 | 待 task-5 |
| Root 授权 / 守护 | **AP 二进制 = `app/apd`（Rust）改造**：`daemon`/`deploy`/`undeploy`/`root-exec`/`auth`/`version --json` 子命令；管理器直连 + 第三方 **SO_PEERCRED 授权 socket**；授权表单文件用户态强制 | NEW-DOC-05 v1.0 草案（待终审） |
| 模块系统 | APM 兼容模块（安装/启停/卸载/undo/action/lua、sepolicy.rule、安全模式）；bootloop 守卫自动 safemode | NEW-DOC-05 v1.0 草案 §3/§5.5/§9 |
| 刷入通道 | **MVP（lead 裁决 2026-11-08）**：本地修补 + **导出镜像**；首次刷入走**主机 fastboot / Recovery**（人类可自行救砖）。应用内直写 boot 分区（`root-exec`/dd）需已 root/Shizuku ⇒ **P2 增量**，不作为 MVP 门禁 | ✅ 已裁决（TBD-1 关闭） |

### 0.4 契约 v1.1 勘误映射（依据 v1.1-FROZEN，2026-11-08 21:23 冻结）

> 来源：《SKRoot↔AP 调用契约 **v1.1-FROZEN**》（`docs/phase2_skroot_ap_call_contract_v1.1.md`，**冻结时间 2026-11-08 21:23 +08:00**，签署：系统守护 ✅ / patcher ✅ / app-client ✅ / leader 终审 ✅）+ NEW-DOC-05 §4.6 + lead 决议（Q1–Q8 全部裁决）。**本表受影响用例已于冻结当日翻转 `acceptance_ready=true`**（见 §0.5）；此后契约变更须重新会签并复核期望值。
> E1 实现注记（测试口径）：`k47` 为相对路径 token，hook 在路径查找前改 cred，cwd 不影响提权；但若 cwd 恰有同名文件，execve 会**真的以 root 加载该文件** → 已列 **B-03-10 / SEC-8** 专项验证。

| 勘误 | v1.0 口径 | v1.1 口径（本方案用例按此写） | 影响测试项 |
|---|---|---|---|
| **E1** | 提权串 = 隐藏目录下 su 路径 | **提权串 = 47 字符密钥本体**（`key[0:47]`，无斜杠）；成功判据仍 `getuid()==0`，execve 返回必为 -ENOENT | B-03-3、B-03-4、SEC-4 |
| **E2** | 隐藏目录内固定名 `su` | 隐藏目录内 **随机 16 字符名**（0700 root）+ `/data/adb/ap/bin/su` 符号链接；直连 su 仅限 root 上下文 | B-03-5、H-7、H-15 |
| **E3** | `SP_REQUEST_UID` 内核环境注入 | **废弃**；第三方 App 身份 = **抽象 socket `SO_PEERCRED`**；socket 暴露由 lead 裁决**接受**但列入隐藏性检测面 | B-03-6、H-15、SEC-7 |
| **E4** | `allow_list` + 独立 `exclude_list` 两份存储 | **只保留单一 `allow_list`（csv 六列含 exclude）**，删除 `exclude_list` 口径；单写者 + flock + tmp/rename | B-03-5、B-04-2、SEC-5 |
| **E5** | rc 注入/install_su/key 文件由独立部署器 `skroot-deploy` | 部署并入**管理器（rc 注入）+ AP `deploy` 子命令**；新增 `undeploy` 逆序清理 | H-12、S2/S4/S10、B-07 |

---

### 0.5 v1.1 冻结翻转清单（lead TBD-10 要求：case id + 依赖条款）

> 触发：patcher + app-client 会签完成 → lead 确认冻结 → **质测 1 个工作日内**按下表批量翻转 `acceptance_ready=false → true`，并在 §10 记录表登记「翻转日期 + 冻结版契约文件名/sha256」。
> **冻结时间：2026-11-08 21:23 +08:00（已登记）**；冻结版文件：`docs/phase2_skroot_ap_call_contract_v1.1.md`（**v1.1-FROZEN**，签署：系统守护/patcher/app-client/leader 四方齐）。翻转执行：qa，2026-11-08 冻结通知当轮完成，**无遗留 false 项**。

| 用例 | 依赖条款（v1.1） | 勘误 | 当前 | 翻转后判定要点（需复核项） |
|---|---|---|---|---|
| B-03-3 | §3.1 / §2.3 | E1 | ✅ true（v1.1-FROZEN） | 传 `k47` 成功（getuid==0）、传 48 字符全文失配、execve 返回 -ENOENT |
| B-03-4 | §2.3 / §3.1 | E1 | ✅ true（v1.1-FROZEN） | 错误密钥静默（无告警/无计数）口径不变，确认不因 E1 改动 |
| B-03-5 | §3.2 / §3.6 | E4 | ✅ true（v1.1-FROZEN） | 仅 `allow_list` 单文件；`exclude` 为行内列；无 `exclude_list` |
| B-03-6 | §3.3 / §4.3 | E3 | ✅ true（v1.1-FROZEN） | socket `SO_PEERCRED` 判定；未授权静默 exit 1；`root-exec` 不查表写审计 |
| B-03-7 | §4.2 | E2 | ✅ true（v1.1-FROZEN） | 隐藏目录内随机 16 字符名；`bin/su` 链接；非 root exec 得 EACCES |
| B-03-8 | §4.5 | E1 | ✅ true（v1.1-FROZEN） | 密钥文件 48 字符原文、读取后截 47；恢复路径为 undeploy+重修补 |
| B-03-10 | §0.2 实现注记 | E1 | ✅ true（v1.1-FROZEN） | 安全 cwd 内提权；cwd 同名文件不得被 root 加载 |
| B-04-2 / B-04-6 | §3.2 / §7 | E4/E5 | ✅ true（v1.1-FROZEN） | 排除开关读写同一 csv；deploy/undeploy 幂等与逆序清理 |
| B-09 | NEW-DOC-05 §5.5/§9-5 | —（非 v1.1） | ✅ 可执行 | bootloop 第 2 次启动自动 safemode；不受 v1.1 影响 |
| H-7 | §4.2 | E2 | ✅ true（v1.1-FROZEN） | 隐藏目录内文件名非 `su`（8 字符扫描特征消失） |
| H-12 | §7 | E5 | ✅ true（v1.1-FROZEN） | rc 注入归管理器（步 7，repack 前）；AP 不再交付该职责 |
| H-15 | §4.3 socket 规格 | E3 | ✅ true（v1.1-FROZEN） | socket 检测面 5 项结论归档（含 `/proc/net/unix` 接受项） |
| H-16 | §3.2/§3.3 | E3/E4 | ✅ true（v1.1-FROZEN） | 授权表 / `auth` / socket / `root-exec` 四路对同一 uid 结论一致 |
| SEC-5 | §3.2 | E4 | ✅ true（v1.1-FROZEN） | 单文件 torn-read 熔断/fail-open/uid 0、2000 保护 |
| SEC-7 | §4.3 | E3 | ✅ true（v1.1-FROZEN） | socket 静默拒绝、peer cred 不可伪造、消息不含密钥 |
| SEC-8 | §0.2 实现注记 | E1 | ✅ true（v1.1-FROZEN） | cwd 劫持防护（发布否决项） |
| S7 | §7 / NEW-DOC-05 §6 | E5 | ✅ true（v1.1-FROZEN） | deploy 幂等 + undeploy 后无残留（S7 序列内执行） |
| G-16 | §8 会签矩阵 | E1–E5 | ✅ 已冻结 | 会签记录齐全 + 冻结版文件 sha256 归档 |

> 翻转后若发现实现与 v1.1 文本不一致：按「契约偏差」缺陷处理（P0），**不得**以测试侧放宽判定来消化。

---
### 0.6 v1.1-FROZEN 七条契约点 → 测试项映射（sys-guard 冻结通知点名覆盖）

| # | 契约点（v1.1-FROZEN） | 覆盖用例 | 判定要点 |
|---|---|---|---|
| 1 | 提权串 = **47 字符密钥本体**（传 48 字符必失败） | B-03-3 / SEC-4 | `execve(k47)`→getuid()==0；48 字符全文调用→uid 不变 |
| 2 | su 隐藏目录内**随机 16 字符命名** + `bin/su` 链接，仅 root 上下文可用 | B-03-7 / H-7 | 目录内名字非 `su`；非 root exec 得 EACCES |
| 3 | 授权 = daemon 抽象 socket + **SO_PEERCRED**，`SP_REQUEST_UID` 已废弃 | B-03-6 / SEC-7 / H-15 / H-16 | 未授权 uid 静默拒绝；peer cred 不可伪造 |
| 4 | 授权表**单文件** `/data/adb/ap/allow_list`（exclude 行内列） | B-03-5 / SEC-5 | 无 `exclude_list` 文件；单写者+flock+tmp/rename |
| 5 | 部署 = 管理器（rc 注入）+ AP（deploy/undeploy），**无 skroot-deploy** | B-04-6 / H-12 / S10 | rc 注在修补流程内；undeploy 无残留 |
| 6 | `root-exec` 为管理器**唯一** root 入口；exit code 0/1/2/124/125/126/127；密钥不入 argv/日志 | **B-10**（新增）/ SEC-9 | 静态仅一条调用链；rc 矩阵逐码复现；`ps/cmdline/log` 零密钥 |
| 7 | **修补链 ≠ 特权链**：修补在 App 内无需 root，特权操作才走 root-exec | B-01 前置 / B-03 前置 / §1 L1 vs L3 | 无 root 状态下 B-01 全链必须成功；**禁止**「无 root 修补应失败」类前提 |

### 0.7 产物来源纪律（人类硬规则：禁止本地编译）

1. **测试对象 = CI artifacts**：APK / 引擎（exec 与 JNI）/ apd 一律从 CI run 下载，本地不构建；每条证据须记录 **run id + artifact 名 + sha256**。
2. 门禁基线（G-02/G-03/G-17 证据列）以 **CI 产物 sha256 + run id** 为准，本地二进制仅可用于工具自检（`selftest.sh`）不得作为门禁证据。
3. 样本与固件不属构建产物，按 §4.2 / `samples/` 清单管理（sha256 已记录）。

---

## 1. 测试分层

| 层 | 名称 | 环境 | 触发时机 | 覆盖 | 门禁作用 |
|---|---|---|---|---|---|
| **L0** | 静态/产物断言 | 宿主 WSL（x86_64）+ **CI artifacts** + 样本镜像集（本地不构建） | 每次 CI run 后 | 产物正确性、契约一致性、失败不落盘 | 合入门禁（硬） |
| **L1** | 单机集成（不刷入，**修补链无需 root**：App 进程内 JNI） | 魅族 21 + CI 产 debug APK | 每迭代 | APK↔引擎全链（PATCH_ONLY）、UI↔日志↔错误面 | RC 门禁 |
| **L2** | 契约对接 | 宿主 + 设备 | 契约发布时 | 引擎 NDJSON/退出码↔APK；守护子命令/授权表↔模块 | 契约验收 |
| **L3** | 真机系统 | 魅族 21（刷入态）；**MVP 刷入通道 = 主机 fastboot/Recovery**，应用内直刷属 P2 | 里程碑 | 开机存活、授权、模块、隐藏性 | RC/发布门禁 |
| **L4** | 异常与回滚 | 魅族 21 | 每次真机测试前必做 | 故障注入、回滚、救砖、幂等重跑 | 准入前置（硬） |
| **R** | 回归 | 宿主 + 设备 | 缺陷修复后 | 修复项 + 邻接项 | 缺陷关闭条件 |

**分层间硬约束**：L0 全绿才允许进 L1；L1 全绿且 L4 回滚演练完成才允许**首次刷入**（L3）；L3 任一阻断项 FAIL 即停止后续 L3 步骤并触发回滚（§6）。

---

## 2. 阻断级必测项与判定标准

> 通用约定：每项测试必须产出**证据四件套**——① logcat 片段；② 落盘日志/报告文件；③ 关键产物 sha256；④ 截图/录屏（UI 相关）。缺证据视为未执行。
> 判定原则：**只认实测证据，不认「应该没问题」**；阻断项 FAIL 一律不得带缺陷进入下一里程碑。

### B-01 本地修补链（不刷入）—— 阻断
- **依据**：NEW-DOC-01（架构）、NEW-DOC-02（引擎本地化）、C1 §1/§2/§4。
- **两级判定（lead TBD-1 裁决要求，二者分开记，不可互替）**：
  - **级 1「导出成功」**（MVP 门禁，本项判定范围）：产物生成、通过 V 组断言、可被用户取出（SAF/文件管理器可见）、sha256 可记录；
  - **级 2「刷入成功」**（B-02 判定范围，MVP 走主机 fastboot/Recovery）：镜像写入目标分区且回读 sha256 一致 → 见 B-02。**级 1 通过 ≠ 级 2 可用**，报告必须分别给出结论。
- **步骤**：APK 选原厂 boot.img → 解包 → 引擎 patch（`--gen-key`）→ 重打包 → 导出到用户可见目录；重复 3 次。
- **判定（全部满足）**：
  1. 五个阶段在 UI 上**逐一可见**（不做「一次性转圈」），每阶段有开始/结束状态与耗时；
  2. 引擎退出码 0；`--json` 终态 `result.code=0` 且为最后一条事件；`<out>.report.json` 生成；
  3. 导出产物通过 §4 V 组全部断言（L0 脚本可复跑）；
  4. 导出产物文件名含时间戳与密钥**掩码**（不得把明文密钥放文件名）；
  5. 3 次运行均成功，每次密钥不同（`--gen-key` 语义）；
  6. 全链耗时记录（用于后续性能回归基线）。
- **证据**：logcat、progress.ndjson、report.json、导出镜像 sha256、5 阶段截图。

### B-02 刷入后开机存活 —— 阻断（首次真机硬门禁）
- **前置**：B-01 级 1 通过、S0 基线完成（§5）、L4 回滚演练完成（§6）、产物通过 V 组；**刷入通道按 MVP 裁决：主机 fastboot / Recovery**（`fastboot flash boot` 或 Recovery 刷入包）；应用内直刷（P2）不参与本项判定。
- **步骤（级 2）**：主机侧刷入补丁镜像 → 校验刷写返回与回读 sha256 → 重启 ×3（每次间隔最少 2 分钟稳定运行）。
- **判定（全部满足）**：
  1. 每次冷启动 ≤ **3 分钟**进桌面（记录实测值，超时即 FAIL，阈值可随基线校准但须留档）；
  2. `getprop sys.boot_completed` = 1；无启动循环（`/proc/uptime` 递增、无重复 boot reason）；
  3. `dmesg` 无 `Kernel panic`/`Unable to handle kernel`；`logcat` 无连续 Native crash 风暴；
  4. `uname -r` 与 S0 基线一致（SK 产物严格等长、不改版本）；
  5. 管理器 App 自检显示「已修补」且版本号与 report.engine_version 一致；
  6. 基础功能冒烟：Wi-Fi、移动数据、相机、指纹解锁、充电、通话各 1 次正常（清单见 §5 冒烟表）。
- **证据**：三次启动的时间戳与耗时、dmesg 快照、开机前后截图、冒烟记录。

### B-03 Root 授权（48 位密钥）—— 阻断
> **依据口径：契约 v1.1（起草中，会签后冻结）**——提权串/授权面/su 命名/授权表存储按 §0.4 的 E1/E2/E3/E4 新口径判定；v1.1 冻结前本项标 `acceptance_ready=false`，冻结后复核。
- **依据**：A1 §4、C1 §0/§2.2、契约 v1.1 §3.1/§3.2/§3.4/§2.3、NEW-DOC-05 §4.1/§4.3/§4.4。
- **判定（逐条，全部满足）**：
  1. 密钥格式 `^[A-Za-z0-9]{48}$`；`report.key_spec={charset:"A-Za-z0-9",length:48,stored_len:47}`；
  2. 内核内嵌窗口 = `key[0..46]+NUL`（48 字节），且落在某 hook 跳板头部（见 V-04/V-05）；
  3. **提权串 = `key[0:47]` 47 字符本体（无斜杠）**：`execve(key47)` 返回 **-ENOENT 且 `getuid()==0`**——**不得以 execve 返回值判成败**；传 48 字符全文必须失配（E1）；
  4. 错误密钥 → 静默拒绝：uid 不变、`dmesg` 无新增告警、无计数文件；
  5. **授权表（单一 `/data/adb/ap/allow_list`，csv 六列含 exclude；无独立 `exclude_list` 文件）**生效：管理器 `auth grant/revoke` 后 socket 请求结果随之变化、重启后保留；写侧单写者 + flock + tmp/rename（E4）；日志**不含密钥原文或前缀**（用密钥值对全量日志反向 grep，零命中）；
  6. **第三方授权面（E3）**：抽象 socket `@sproot-<12hex>` 上 `SO_PEERCRED` 取到的 uid 即判定依据——授权 uid 连接放行、未授权 uid 静默失败（exit 1 无输出）、伪造 uid 的尝试无效（peer cred 由内核填写）；管理器路径（`root-exec`）不查表但写审计日志；
  7. **su 部署（E2）**：`/data/<key16>/<随机16字符>`（0700 root）存在且可执行；`/data/adb/ap/bin/su` 符号链接指向它；非 root 上下文 exec 该副本必须 `EACCES`（无授权面）；
  8. 密钥丢失（清 App 数据）→ 有**文档化恢复路径**且实测可执行（重修补或密钥轮换；current 方案为 `undeploy` + 重新修补，须实测）；
  9. 提权后 seccomp 已清（`/proc/self/status` Seccomp=0）、后续 execve 不被旧过滤器拦截；
  10. **cwd 碰撞防护（E1 实现注记）**：`k47` 是无 `/` 的相对 token——在提权进程 cwd 放置一个**与 k47 同名**的可执行文件，验证 AP 在已知安全 cwd（如 `/data/adb/sproot/`）执行提权、**不会被该文件劫持为 root 加载**；如发生加载即判 FAIL（安全缺陷）。
- **证据**：提权前后 `id`/`getuid` 输出、execve 返回与 errno 记录、allow_list 前后 diff + `auth` 前后 socket 结果、`ls -l` 隐藏目录与 su 链接、非 root exec 的 EACCES 记录、日志脱敏 grep 结果。

### B-04 模块安装与生命周期 —— 阻断
> **依据 NEW-DOC-05 v1.0 草案（已交付，待终审）**。判定直接吸收其 **§9 验收 7 项**中的模块相关 3 项 + §3/§5 语义；终审若修订，1 个工作日内回填。

- **判定（逐条，全部满足）**：
  1. 安装落盘 → **重启后生效**（`modules_update` → 下次 post-fs-data 搬运；安装不即时生效，UI 必须明确提示重启，重启前安装按钮状态与提示一致）；
  2. 启用/禁用/卸载/undo-uninstall/action/list/config 行为与模块文件状态一致、可逆；`module list` 输出 JSON 可解析；含 `sepolicy.rule` 模块可安装且规则经用户态路径注入生效，UI **必须出现「隐藏性不作保证」警示**；
  3. 模块 id 拒绝路径穿越（负样本 `../`、绝对路径、空 id、超长 id）——不得创建越界目录；
  4. **三阶段脚本按序执行**（NEW-DOC-05 §9-2）：样例模块 `post-fs-data.sh` / `service.sh` / `boot-completed.sh` 各写一行日志 → 三阶段日志均出现且顺序正确（`state.json` 有阶段完成时间）；metamodule、`*.d`、lua 路径按 NEW-DOC-05 §5.2 顺序抽测；
  5. **安全模式三路**（`persist.sys.safemode` / `ro.sys.safemode` / `/data/adb/sproot/safemode`）任一命中：全部模块禁用、stage 脚本跳过，开机仍成功；
  6. **部署/逆序清理**（E5）：`deploy` 幂等（重复执行 sha256 校验通过）；`undeploy` 后 `/data/<key16>/`、`/data/adb/sproot/`、`/data/adb/apd` 均不存在且开机正常；
  7. 卸载后无残留目录语义（对照 NEW-DOC-05 §3.2 布局清单）。
- **证据**：模块目录树前后快照、三阶段日志、`state.json`、重启前后状态、负样本创建结果、安全模式/deploy/undeploy 截图与日志。

### B-09 模块脚本超时与 bootloop 自动安全模式 —— 阻断（新增）
> 依据 NEW-DOC-05 §5.5（bootloop 守卫）+ §9-5（验收第 5 项）+ NEW-DOC-05 §8 R4 风险。SK 内核零监管，这是唯一的开机自愈防线，必须实测验证；**任一子项不满足即阻断**（对应主线「模块安装与生命周期」的故障态）。

- **判定（逐条，全部满足）**：
  1. **脚本超时**：构造故意睡死（`sleep 600`）的 `post-fs-data.sh` 模块 → 脚本在超时阈值（默认 60s，可配）内被 kill（进程组一并清理），`post-fs-data` 阶段继续处理后续模块（不整体中断），开机成功；
  2. **bootloop 自动 safemode**：放置一个持续挂死/导致重启的模块 → **第 2 次启动**自动写入 `/data/adb/sproot/safemode` 并禁用全部模块，**第 2 次启动必须能进桌面**（不允许第 3 次循环）；
  3. 管理器 UI 在自动 safemode 后显示「上次启动未完成，已进入安全模式」，且提供清除 safemode 的入口（删除文件后再次重启模块恢复）；
  4. `boot_count` 语义正确：`post-fs-data` 入口 +1、`boot-completed` 完成清零；正常开机连续 5 次 `boot_count` 始终为 0（不得误触发 safemode）；
  5. 超时/守卫全过程不产生内核 panic、不破坏数据分区，日志（`/data/adb/ap/log/`）留痕（脱敏）。
- **证据**：睡死模块用例的时序日志与 `dmesg`、两次启动耗时、`safemode` 文件与模块禁用状态、`boot_count` 录值、UI 截图。

### B-10 管理器唯一 root 入口（root-exec）契约 —— 阻断
> 依据：契约 v1.1-FROZEN §8（root-exec 为唯一 root 入口、exit code 0/1/2/124/125/126/127、密钥不入 argv/日志）+ §0.6 第 6 条。
- **判定（逐条，全部满足）**：
  1. **静态唯一性**：管理器全部特权动作（deploy / auth / module / P2 刷入）只经 `root-exec`；工程内无第二条 `execve(key)` 提权路径、无绕过 `root-exec` 直写块设备/直调 AP 私有接口的实现；参数形态为 `root-exec --superkey-file <0600文件> [--uid N] -- <cmd...>`；
  2. **exit code 矩阵**：构造 7 类结果逐码复现 **0/1/2/124/125/126/127**（成功/命令失败/用法错误/超时/内部错误/不可执行/命令不存在），管理器对 124/125/126/127 必须给出可读文案与下一步指引（不得统一「执行失败」）；
  3. **stdio 透传**：stdout 与 stderr **不混流**（诊断需要原始 stderr）；退出码原样回传 UI 且写入日志；
  4. **密钥面**：`ps -ef`、`/proc/<pid>/cmdline`、`/proc/<pid>/environ`、logcat、管理器日志中**零密钥命中**（用已知密钥反向 grep）；`--superkey-file` 文件 0600 且用后删除；
  5. **不可绕过授权**：第三方 socket 请求按 allow_list 拒绝（静默 exit 1），`root-exec` 不得成为第三方绕过授权的通道（只能由管理器调用）。
- **证据**：静态 grep 结果、7 类 rc 复现记录、`ps`/cmdline/environ/logcat 抓取、临时 key 文件权限与删除记录。

### B-05 隐藏性基线一致 —— 阻断（发布门禁）
- **方法**：S0 基线快照（`s0_baseline_capture.sh`）vs 修补态实测（`h_group_probe.sh`）**差集比对**，H 组见 §8。
- **判定**：H-1/H-2（宿主静态）/H-3（SELinux 零触碰）/H-5（无挂载差集）/H-6（无新增常驻面）/H-7（密钥前缀目录双路不可见）/H-15（授权 socket 检测面，含 lead Q7 接受项的评估结论）全部通过；H-4/H-8/H-9/H-10/H-12~H-14/H-16 按 §8 逐项判定；H-11 部分通过并进发布声明。
- **证据**：两套导出目录、diff 报告（只导出事实，不写结论的地方按 §8 口径判定）。

### B-06 修补失败必须给出完整错误 —— **独立阻断级验收项**
> **独立验收项编号 B-06**：与功能成败解耦——即使修补成功，本项不达标仍判 FAIL。直接针对旧路线「点击修补后终端输出一闪即报错终止、无任何可读错误」的问题。详细判据见 **§3**。

- **判定（全部满足）**：§3 的 EC-1~EC-8 全通过，且 8 类故障注入（§3.2 矩阵）逐类满足「错误码正确 + UI 可读原因 + 日志落盘 + 进程存活 + 可导出」。
- **证据**：每类注入的录屏（失败瞬间起 ≥5 秒）+ logcat + 落盘日志文件 + UI 截图。

### B-07 回滚可用 —— 阻断（真机）
- **判定**：按 §6 回滚预案**实测**刷回原厂 boot 并成功开机；回滚后设备与 S0 基线一致（开机正常、管理器显示未修补、无补丁残留）；一次回滚全流程 ≤ **10 分钟**（记录实测耗时）。
- **证据**：回滚命令记录、回滚后开机截图、基线比对结果。

### B-08 密钥安全最小面（SEC-1）—— 阻断（发布否决）
- **判定**：普通应用进程（untrusted_app 视角）无法读取密钥存储、无法读 `allow_list`、`/data/<key16>/` 不可见、`execve(key)` 越权提权必须静默失败；密钥不出现在 logcat / `/sdcard` / 系统属性 / 环境变量（AP 进程外）/ boot 镜像头部配置块。
- **证据**：`sec1_unreadable_probe.sh` 输出、全盘 grep 结果、`dumpsys`/`getprop` 抓取。

---

## 3. B-06 专项：错误可见性验收（针对「一闪即退」）

### 3.1 判据 EC-1~EC-8（错误可见性判据；与 §7 异常场景 E1~E12 区分）

| # | 判据 | 判定方法 | 失败含义 |
|---|---|---|---|
| EC-1 | 失败后 **≤3 秒** UI 出现错误区域（非 toast 即消） | 录屏逐帧 + 截图 | 用户来不及看 |
| EC-2 | 错误必须带**错误码**（取自 C1 §3 稳定枚举）+ **人类可读原因** + **下一步指引** | UI dump 文本断言；禁止「未知错误」「操作失败」单独出现 | 不可诊断 |
| EC-3 | 错误详情**落盘**为日志文件，路径可在 UI 复制/查看；文件非空且含错误码与阶段名 | 文件存在 + `grep -F E2xx` + 内容长度>0 | 事后无法复盘 |
| EC-4 | 失败**不得**自动关闭页面/清空日志/自动重试；返回再进可回看历史 | 操作后仍停留在修补页并有历史列表 | 信息被销毁 |
| EC-5 | 每个失败阶段有独立标识（解包失败 ≠ 引擎失败 ≠ 重打包失败），不得笼统归为「修补失败」 | stage 字段/文案与 5 阶段映射表一致 | 定位成本 |
| EC-6 | 引擎被外部杀死/超时必须报错（不是无限转圈） | 注入 kill / 超时 | 卡死 |
| EC-7 | 失败后 App 进程存活、可重新发起（无闪退、无 ANR） | `pidof` 前后一致；3 次重复不崩 | 直接复现旧问题 |
| EC-8 | 错误详情可导出（分享/复制），且导出内容**脱敏**（无明文密钥，仅掩码） | 导出文件 grep 密钥零命中 | 泄漏 or 不可上报 |

**判 PASS 的必要条件**：8 类注入 × 8 条判据 = 全通过；任一组合不通过即 B-06 FAIL。

### 3.2 故障注入矩阵（不依赖根因，先按症状设计）

| # | 注入方式 | 期望退出码/阶段 | 期望 UI 要点 | 自动化断言 |
|---|---|---|---|---|
| F-1 | 选 `.img` 直接喂引擎（或选非 boot 的 img） | E102 / 输入校验 | 「镜像格式/非内核镜像」+ 指引重选 | logcat 出现 E102；UI dump 含关键词 |
| F-2 | 选截断/损坏 boot.img | 解包阶段失败（自定义错误码需在映射表） | 「解包失败」+ 文件可疑提示 | stage=unpack；错误码在映射表 |
| F-3 | 非 arm64 内核镜像 | E102（非 arm64 Image） | 机型不支持提示 | E102 |
| F-4 | 密钥格式错（`--key` 传 47 位/含符号） | E101 | 「密钥需 48 位字母数字」 | E101 |
| F-5 | 输出目录只读/空间不足 | E240 | 「存储不可写/空间不足」 | E240 |
| F-6 | 空间不足类内核（构造 E220 样本内核或桩） | E220 / 引擎织入阶段 | 「该内核无可用跳板空间，暂不支持」 | E220 |
| F-7 | 重打包失败（工具缺失/ramdisk 异常） | 重打包阶段失败 | 「重打包失败」+ 保留工作目录 | stage=repack |
| F-8 | 引擎进程中途被 kill（模拟中断） | 中断（非 0 退出） | 「修补被中断」+ 可重试 | 进程存活；日志含中断记录 |

> 【TBD-9】F-2/F-7 的具体错误码取决于 APK 侧解包/重打包组件（magiskboot 或自研）的错误码设计，待 app-client 在「错误码→文案映射表」（G-04）中定稿后回填；本表当前以阶段名 + 语义判定，不臆造码值。

### 3.3 自动化落地方式（可脚本化部分）

`quality/static/verify_error_visibility.sh`（**待研发产物到位后编写**，本方案先固化断言规则）：
1. `adb logcat -d` 抓取 → 断言出现期望错误码字符串与 stage 名；
2. `adb shell pidof <pkg>` 断言 App 存活；
3. `adb pull` 日志目录 → 断言文件非空、含错误码、含 5 阶段之一；
4. `adb shell uiautomator dump` → 断言错误面板文本含「错误码 + 原因 + 指引」三段；
5. 录屏文件归档（人工复核 EC-1 的 3 秒时限）。
> UI 侧 Compose 对 uiautomator 的可读性有限【TBD-6】：若 dump 不可用，退化为「截图 + 日志 + 人工核对清单」，但**不得因此取消 B-06**。

---

## 3A. 根因回归组 T1~T7（阻断级，针对已确证的旧链路缺陷）

> 来源：`docs/research/patch-failure-rootcause.md`（lead 2026-11-08 通报，已确证）。旧链路根因 = 引擎未入包（P0-a）+ 构建期网络下载（P0-b）+ release 无密钥（P0-c）+ 失败后日志面板卸载（P1）+ 退出码自吞（P2）+ 未校验产物即宣布成功（P3/F7）。
> **本组与 B 组同为阻断级**：T1~T7 任一 FAIL，该版本不得进入真机刷入序列，也不得签发 RC。T 组是「新管理器不得重犯旧错」的**负向回归证据链**，每项都必须能指回旧实现的具体证据行。

| # | 名称 | 对应旧缺陷 | 执行方法 | 判定（阻断） | 自动化 |
|---|---|---|---|---|---|
| **T1** | 二进制能力自检 | P0-a / P0-b / F1 / F2 | ①宿主解包 APK：`unzip -l` 断言 `lib/arm64-v8a/` 内引擎与解包器存在；`readelf -h` 断言 `e_machine=0xAARCH64(0xB7)`、无 x86-64 混入；②设备上打开管理器「能力自检」：逐项显示引擎可加载、解包器存在且可执行、ELF 头正确、版本号一致；③**负样本**：从测试包移除引擎/解包器后点修补 | ①APK 内依赖齐全且架构正确；②缺项时 UI **在进入修补前**阻断，文案点名「缺什么、期望放哪、如何安装」（禁止旧版 `prepare()` 静默跳过 symlink）；③负样本错误必须是「缺引擎」而非「Patch failed.」 | `quality/static/check_apk_payload.sh`（待写，T1/T6 静态部分，进 CI） |
| **T2** | 无网络依赖 | P0-b / P3 / F3 | ①宿主断网（或 Gradle `--offline` + 空缓存）clean 构建；②断言 APK 内全部可执行依赖均来自仓库或本仓构建产物（清单 + sha256 与入库记录一致，构建日志零 http 下载）；③设备飞行模式 + 清 App 数据后完成一次完整修补（B-01 流程） | 构建期零网络依赖（下载仅允许辅助且失败必须 fail build）；设备断网修补成功。任一不满足即 FAIL | 构建日志断言 + `check_apk_payload.sh` |
| **T3** | 错误可见性（专项强化） | P1（日志面板卸载、stderr 被注释） | 在 §3 EC-1~EC-8 基础上追加旧缺陷专项：①失败后日志容器**仍存在于 UI 树**（uiautomator dump 断言，不得随 `patching/patchdone` 卸载）；②真实 stderr 内容出现在 UI（禁止只显示 "Patch failed." 之类无信息文案）；③日志**先落盘后更新 UI**（杀进程后文件仍在）；④5 场景：缺引擎 / 引擎 rc=126 / rc=1 / 引擎崩溃 / 超时 | §3 EC-1~EC-8 全通过 + 上述 4 条专项全通过；缺一即 FAIL | `error_visibility.sh`（待写，§3.3） |
| **T4** | 失败不假成功 | P2 / F6 / F7（`echo; exit $?`、abort 用 `exit 0`、rc 被覆盖、未校验产物即打印 Successfully） | ①**壳层静态审查**：修补执行层（脚本或等价逻辑）无 `exit $?`、无 `; exit 0` 作 abort、存在 `set -eu`、每条外部命令 rc 立即捕获；②**动态注入**：引擎分别返回 {1,126,127,139,255, 超时} → App 必须显示失败、状态为失败、无产物、日志含真实 rc；③成功文案与产物一致性：出现「成功」必须有产物 | 所有注入 rc 如实传播、UI 状态与退出码一致；失败时 `--out`/new-boot.img **不存在**（C1 §3 铁律）；出现「成功文案 + 无产物」即判 **P0 缺陷**（比崩溃更危险） | `rc_fidelity.sh`（待写，壳层 rc 矩阵） |
| **T5** | 成功校验 | F7（脚本打印 Successfully 与真实产出无关） | 成功路径逐项断言：①`new-boot.img` 存在；②头部魔数 `ANDROID!`；③size ≤ boot 分区 size；④产物 `patched=true` 回读一致（`kptools -l -i` 或引擎 report）；⑤`report.root_key == 传入 key`（防刷入他人密钥）；⑥产物 sha256 写入日志与 UI | 6 项全满足才允许显示「成功」；任一不满足必须转失败态并给原因（防「未刷入当已刷入」） | B-01 断言 + `patched` 回读脚本 |
| **T6** | release 构建可用 | P0-c / F8（`if (!BuildConfig.DEBUG) return null`） | CI 产**签名 release 包** → 安装 → 完成一次完整修补（PATCH_ONLY）；比对 release 与 debug 包内引擎/解包器 sha256 一致；静态检查密钥来源路径无 `BuildConfig.DEBUG` 门控（仅允许用于日志开关） | release 包可完成 B-01 全链且密钥来源可用；密钥/修补路径的 DEBUG 分支零命中（或经 leader 明示豁免并留档） | CI（task-8）+ grep 门禁 |
| **T7** | 路径约束与单一调用路径 | F9 / F10（filesDir 内 exec 受限；`sh -c` 拼串、位置参数透传歧义） | ①**静态**：全工程无「ELF 放 filesDir 后直接 exec」；若 exec，目标必须在 nativeLibraryDir；首选 JNI `.so` 加载；②**调用路径唯一**：grep 证明只有一条调用链（`ProcessBuilder(listOf(...))` 或 JNI），无 `sh -c` 拼串、不向解包器透传多余位置参数；③**参数矩阵**：正常镜像 / 含空格路径 / 特殊字符路径 / 缺参数 / 超长 key 五类输入行为一致且错误可读；④真机：exec 失败必须区分 `ENOEXEC` / `EACCES` / 解释器缺失三类错误码 | 静态 0 违规 + 参数矩阵全通过 + 三类错误可区分（真机实测） | `path_binding_static.sh`（待写）+ 真机 |

| **T8** | CI 产物一致性（可复现构建） | 新增（人类硬规则：只认 CI 产物） | 同一 commit + 同一 CI 配置触发两次 run → 下载同名 artifact，比对 sha256；不一致时做内容级比对（文件清单/size/ELF 段/时间戳段） | 首选 bit-identical；若不可复现，差异必须可解释（签名/时间戳字段）且功能一致，并在证据列记录差异说明；**禁止**「只测本地构建产物」 | CI（task-8）+ `sha256sum` |
| **T9** | APK 签名校验（V3） | 新增（发布安全门） | `apksigner verify --verbose --print-certs` 对 CI 产 release APK | V3 scheme = true（含 V2/V1 视兼容需要）；证书 sha256 与发布记录一致；**非 debug 证书**；签名校验失败即阻断 | CI（task-8） |

**T 组与 B 组映射**：T3 ⊂ B-06（B-06 是产品级验收项，T3 是其旧缺陷专项证据）；T4/T5 ⊂ B-01 失败/成功分支；T1/T2/T6/T7 为独立阻断项，无对应 B 项时单独计门禁。

**T 组脚本计划**（待研发产物到位后补齐，接口先定）：
| 脚本 | 覆盖 | 输入 | 预计依赖 |
|---|---|---|---|
| `quality/static/check_apk_payload.sh` | T1/T2/T6 静态 | APK 文件 + 依赖清单 | `unzip`、`readelf`/`file`、`sha256sum` |
| `quality/static/rc_fidelity.sh` | T4 | 修补执行层 + 注入桩 | `bash`、引擎桩（返回指定 rc） |
| `quality/static/error_visibility.sh` | T3/B-06 | 设备 + APK | `adb`、`uiautomator` |
| `quality/static/path_binding_static.sh` | T7 静态 | 客户端源码 | `grep`/`rg` |

---

## 4. 无真机阶段的静态验证清单（L0，可脚本化）

**交付脚本**：`quality/static/verify_patch.py`（本轮已交付，用法见 `quality/static/README.md`）。
**输入**：引擎产物三元组——`--in` 原厂内核 Image、`--out` 修补后内核、`--report` report.json（可选 `--key` / `--ndjson`）。
**退出码**：0=全 PASS；1=有 FAIL；2=用法/环境错误。

### 4.1 断言表

| # | 断言 | 方法 | 判定 | 脚本 |
|---|---|---|---|---|
| V-01 | 拒绝非内核输入 | 对 `.img`/`.gz`/截断文件跑引擎 | 退出码 E102，且 `--out` 不存在 | 脚本 V2（负样本模式） |
| V-02 | 纯字节 patch **等长** | `size(out)==size(in)` | 成立 | V1 |
| V-03 | 内容确有改动 | `sha256(out)!=sha256(in)` | 成立 | V1 |
| V-04 | 密钥内嵌 | out 中存在 48 字节窗口 == `key[0..46]+NUL` | 成立 | V2 |
| V-05 | 密钥位于跳板头部 | 上述窗口起点与 `report.hook_table` 中任一 `trampoline`/`hook_addr` 距离 ≤ 512B | 成立（弱断言，需人工复核一次） | V2/V4 |
| V-06 | 隐藏目录前缀一致 | `report.hidden_dir_prefix == key[0:16]` | 成立 | V3 |
| V-07 | 产物可解析（arm64 Image 头） | 偏移 0x38 魔数 == `0x644d5241`（"ARM\x64"）；0x10 处 `image_size` 字段 == 文件长度 | 成立 | V5 |
| V-08 | hook 落位齐全 | report 五组（do_execve/filldir64/compat_filldir/avc_denied/audit_log_start）地址均非 0；cfi/huawei 计数有值 | 成立 | V4 |
| V-09 | 报告自洽 | `report.input.sha256/size` == 实际 in；`report.output.*` == 实际 out；`engine_version` 非空 | 成立 | V3 |
| V-10 | NDJSON 合法 | 每行合法 JSON；`phase` ∈ 固定枚举；终态 `result` 唯一且为最后一条 | 成立 | V6 |
| V-11 | 失败不落盘 | 构造 E210/E220/E102 负样本 → 退出码对应且 `--out` **不存在**；workdir 保留 | 成立 | 手工 + V2 负样本 |
| V-12 | 退出码枚举稳定 | 101/102/201/202/210/211/220/230/240 逐类可复现且语义正确 | 成立 | 手工矩阵（附记录表） |
| V-13 | 改动区域受控 | out 与 in 的差异区段数 ≥5（五个 hook）、单区段 ≤ 512B、总改动 ≤ 256KB；差异区段与 report 地址可对应 | 成立（弱断言，防「大段重写」） | V4 |
| V-14 | SELinux policy 零 diff | policy 数据区定位困难 → 降级为「除 hook 区段外无 diff」（由 V-13 保证），另用 `avc_denied`/`audit_log_start` 符号区仅被跳板重定向做人工复核 | 成立 | V-13 + 人工 |
| V-15 | 重打包链路无损 | 原厂 boot.img：`magiskboot unpack` → `repack` → 再 `unpack`，vmlinux/ramdisk sha256 与首次一致；产物可被再次解包 | 成立 | 手工脚本（§4.3） |
| V-16 | APK 内 arm64 产物合规 | APK 解包后 `lib/arm64-v8a/` 含引擎（ELF aarch64）、无 x86_64 残留；`extractNativeLibs` 策略与「可执行化」方案一致；文件非 0 字节 | 成立 | 手工/CI 脚本 |
| V-17 | KP 残留检测 | out 与 APK 中无 kpimg/preset magic、无 supercall 号段（0x1158/0x1000~0x1146）特征、无 `kp` 别名 | 成立 | grep 脚本 |
| V-18 | 密钥不入日志/产物 | 全量日志、导出目录、`/sdcard`、report 之外的产物：明文密钥与 key[:16] 零命中（report 允许 `root_key` 字段，但需标注为"含密钥，禁止外发"） | 成立 | grep 脚本 |
| V-19 | 失败可见性静态部分 | APK 源码/资源中存在 5 阶段 × 错误码映射表；无「catch 后静默 return」路径（代码审查 + 静态扫描 `catch (e) {}` 空块） | 成立 | 代码审查 |
| V-20 | 引擎版本一致 | APK 内嵌引擎 `--version`/report `engine_version` 与 CI 构建记录一致（防「装的不是打的」） | 成立 | 手工 |
| V-21 | 构建期无网络依赖（T2 静态部分） | 断网/`--offline` clean 构建；APK 内可执行依赖清单每条均有入库或本仓构建来源 + sha256 | 成立 | `check_apk_payload.sh` |
| V-22 | release 包可用（T6 静态部分） | release APK 内引擎/解包器与 debug 包 sha256 一致；密钥/修补路径无 `BuildConfig.DEBUG` 门控 | 成立 | CI + grep |
| V-23 | 单一调用路径与 exec 位置（T7 静态部分） | 全工程仅一条修补调用链；无 `sh -c` 拼串、无位置参数透传；ELF 不在 filesDir 直接 exec | 成立 | `path_binding_static.sh` |

### 4.2 样本镜像集（L0 前置）与基线（实测，2026-11-08）

> 人类提供魅族 21 官方固件，样本已就位；**按人类指令不再下载 GKI 样本**（patcher 已收删除指令）。样本基线记录：`quality/records/S0-2026-11-08/bootimg-baseline.md`。

| 样本 | 文件 | 字节 | sha256 | 基线结论（`--bootimg` BI1~BI6，10/10 PASS） |
|---|---|---|---|---|
| 魅族 21 boot.img | `samples/meizu21/boot.img` | 100,663,296 | `8c899eb3…c4619f5` | header **v4** / header_size 0x630 / page_size 4096 / 页对齐；**kernel 段 off=0x1000, size=33,892,864, sha256 `4d445384…3098481`**；kernel 为**未压缩 arm64 Image**；ramdisk_size=0（在 init_boot） |
| 魅族 21 init_boot.img | `samples/meizu21/init_boot.img` | 8,388,608 | `c0a74489…59534bb` | header v4 / page 4096；kernel_size=0、ramdisk_size=2,131,370 → **init_boot 形态**（不参与 Kernel Image 断言） |
| 魅族 21 vbmeta.img | `samples/meizu21/vbmeta.img` | 12,288 | `873920f4…54fe76` | AVB 元数据（归档，供回滚/校验用） |
| Kernel Image（in/out） | `samples/meizu21/kernel`(待 patcher 解包) | 33,892,864（预期） | 预期 `4d445384…3098481` | **TBD-8 关闭**：本样本 kernel 已是未压缩 Image，可直接喂引擎 |
| 负样本集 | 派生 | — | — | `.img`/`.gz`/截断/非 arm64：由 boot.img/kernel 派生，脚本生成 |

**样本基线关键观察（进 B-01/V 组判定口径）**：
1. **解包目标唯一确定**：Kernel Image = `boot.img[0x1000 : 0x1000+33,892,864]`，sha256 `4d445384c046f2ba2de856b3215de8d2be83a9ccad115948a34471cde3098481` → 作为 `--sample meizu21-boot-kernel` 的 `in`，patcher 解包产物必须与之逐位一致；
2. **image_size 字段（34,603,008）> kernel_size（33,892,864，+2.05%）**：V-07 断言以**实际字节数/kernel_size**为准，`image_size` 仅作参考（工具已按 ≤5% 容差 WARN 而非 FAIL）；重打包后 kernel 段长度必须仍为 33,892,864（等长铁律）；
3. **A/B 双槽已证实**：官方 `samples/meizu21/flash.bat.reference` 用 `fastboot flash --slot=all boot` 并切槽再刷 → S4/§6 按 A/B 双槽准备（S0 仍需 `fastboot getvar current-slot` 实测确认）。

### 4.3 关联手工脚本（待补）

1. `magiskboot` round-trip（V-15）：`unpack → repack → unpack` 三段 sha 比对；
2. 反汇编复核（V-05/V-14）：用 `llvm-objdump`/capstone 检查目标地址首指令为 `B`（跳板）且跳板序言含 `paciasp`（若原函数含）；
3. 差异区段报告（V-13）：脚本已输出区段摘要，反汇编复核为增强项。

---

## 5. 真机测试序列（魅族 21：内核 6.1.25 / Android 16 / ARM64）

### 5.1 S0 建档卡（唯一设备，必须先建档再动手）

> 表内标 **待实测** 的项由 S0 现场采集填写，**质测不预填、不臆造**。

| 项 | 采集方式 | 值 |
|---|---|---|
| 机型/固件版本 | `getprop ro.product.model` / `ro.build.fingerprint` | 待实测 |
| 内核版本 | `uname -r` | 预期 6.1.25（以实测为准） |
| Android 版本/安全补丁 | `getprop ro.build.version.release` / `ro.build.version.security_patch` | 待实测 |
| boot 分区路径 | `ls -l /dev/block/by-name/boot*` | 待实测 |
| **槽位形态（A/B 或单槽）** | `getprop ro.boot.slot_suffix`；`ls /dev/block/by-name/ \| grep boot` | **待实测（决定回滚路径 R1/R2）** |
| AVB/vbmeta 状态 | `getprop ro.boot.veritymode`；`fastboot getvar` 相关项 | 待实测 |
| Bootloader 状态 | `fastboot oem device-info` / 屏幕显示 | 待实测（**必须已解锁，否则真机序列不可执行**） |
| fastboot/adb 可用性 | 主机侧连通性；`fastboot devices` / `adb devices` | 待实测（MVP 刷入通道前置） |
| Recovery 可用性 | 侧载/卡刷是否可用（`adb sideload` 或 Recovery 刷入包） | 待实测（fastboot 不可用时的备份通道） |
| 原厂 boot.img 备份 | 从设备分区 dump 或官方固件提取；**双介质**（主机 + 设备） | 待实测 + sha256 |
| 救砖手段 | 人类自备（官方固件/fastboot 刷回） | 人类确认 |
| 基线快照 | `s0_baseline_capture.sh` 输出目录 | 待实测 |

### 5.2 序列

| 步 | 名称 | 前提 | 步骤要点 | 判定 | 失败处置 |
|---|---|---|---|---|---|
| S0 | 建档 + 基线 + 备份 | 设备就绪 | §5.1 建档卡全填；跑 `s0_baseline_capture.sh`；备份原厂 boot.img 双介质 + sha256；确认回滚工具 | 建档卡无空项；备份可读且 sha256 记录 | 停止，不进入 S1 |
| S1 | 静态验证（L0） | 样本镜像集到位、arm64 产物到位 | 跑 `verify_patch.py` + 负样本矩阵 | V 组全 PASS | 停止，退回研发 |
| S2 | 手机端修补（不刷入） | S1 绿；debug APK 安装 | B-01 全流程 ×3 | B-01 判定全满足 | 停止，报 B-06/B-01 缺陷 |
| S3 | 产物核对 + 重打包 | S2 产物导出 | 核对 sha256；做 V-15 round-trip；再核对 | 与 S2 记录一致 | 停止 |
| S4 | 首次刷入（**级 2**） | S3 绿 + B-01 级 1 通过；**S5R 回滚演练已排程**；备份确认 | **MVP 通道：主机 fastboot**（`fastboot flash boot <new-boot.img>`；A/B 刷非活动槽）或 **Recovery 刷入**；刷后回读 sha256；**应用内直刷（P2，需已 root/Shizuku）不在本序列** | 命令返回 OK + 回读 sha256 == 产物 sha256 | 立即按 §6 回滚 |
| S5 | 首启观察 | S4 完成 | 计时冷启动；`sys.boot_completed`；dmesg/logcat；冒烟表 | B-02 判定全满足 | 10 分钟未进系统 → 回滚 |
| **S5R** | **回滚演练（强制）** | S5 通过 | 按 §6 R1/R2 刷回原厂 boot → 验证开机 → 再刷回补丁镜像 | 回滚 ≤10 分钟且回滚后正常开机 | 回滚失败 → 全项目停机，人类介入 |
| S6 | Root 授权 | S5R 完成 | B-03 全项；SEC-1 探针；日志脱敏 grep | B-03/B-08 判定全满足 | 阻断项 FAIL → 回滚并停机 |
| S7 | 模块生命周期 + bootloop 守卫 | S6 绿；NEW-DOC-05 终审版；G-17 产物就位 | B-04 全项（三阶段脚本/启停卸载/sepolicy.rule/安全模式三路/部署幂等）+ **B-09**（睡死模块 → 超时 kill → 第 2 次启动自动 safemode 且可进桌面 → 清 safemode 恢复） | B-04 与 B-09 判定全满足 | 停机，保留模块现场；若 bootloop 第 2 次未自愈 → 按 §6 回滚 |
| S8 | 隐藏性对照 | S7 绿 | `h_group_probe.sh` + `sec1_unreadable_probe.sh`，与 S0 基线 diff | B-05 判定 | 发布阻止（非回滚项） |
| S9 | 稳定性 | S8 完成 | 重启 ×5 + 待机 2h + 充电/通话/相机各 3 次；无重启/无 ANR | 无异常、无 panic | 停机取证 |
| S10 | 卸载与恢复原厂 | S9 完成 | `apd undeploy --keep-modules`（E5：删除 su 链接/隐藏目录/sproot/key/state）→ 管理器删除 rc 注入物 → 回滚原厂 boot → 与 S0 比对 | `/data/<key16>/`、`/data/adb/sproot/`、`/data/adb/apd` 均不存在；设备回到基线且无残留 | 停机，人类介入 |

### 5.3 冒烟表（每次开机后执行，1 次/项）

Wi-Fi 连接、移动数据、相机前后摄、指纹解锁、通话拨出、充电识别、蓝牙、音量/亮度、应用市场启动、`pm list packages` 计数与基线差异 ≤ 基线容忍。

### 5.4 停机规则（STOP）

- STOP-1 任何阻断项（B-01~B-08）FAIL；
- STOP-2 刷入后 10 分钟未进桌面 或 连续 2 次启动失败；
- STOP-3 出现 kernel panic / 数据分区异常 / 无法进入 fastboot；
- STOP-4 密钥明文出现在日志/导出物；
- STOP-5 回滚失败或回滚后仍不可用。
> 触发 STOP 后：保留现场（logcat/dmesg/截图/产物/工作目录）→ 执行回滚 → 通知 leader 与人类 → **在根因闭环前不再刷机**。

---

## 6. 回滚与救砖预案

### 6.1 备份规范（S0 必做，硬前置）

1. 原厂 boot.img（若 A/B 则两槽都备）：提取后计算 sha256，**双介质**存档（主机工作区 + 设备端 `/data/adb/sproot/backup/` 或 `/sdcard`）；
2. `fastboot getvar all` 全量输出存档（含槽位、解锁状态）；
3. 分区布局快照（`ls -l /dev/block/by-name/`）；
4. S0 基线快照目录（`s0_baseline_capture.sh` 输出）；
5. 上述 4 项在 `quality/records/S0-<日期>/` 归档（本方案建议路径，研发/人类可调整）。

### 6.2 回滚路径（按优先级）

| 路径 | 适用 | 命令要点 | 判定 |
|---|---|---|---|
| **R1 主机 fastboot 刷回原厂 boot** | 默认路径（单槽/任意） | `fastboot flash boot <原厂boot.img>`（A/B：`fastboot flash boot_a/b` + `fastboot set_active <原槽>`） | 刷写 OK + 重启进桌面 |
| **R2 A/B 槽位回退** | A/B 且补丁刷在备槽 | `fastboot --set-active=<原活动槽>` | 切槽后正常开机 |
| **R3 设备端 dd 回写** | 设备仍可 root/adb shell | `dd if=<原厂boot.img> of=/dev/block/by-name/boot`（路径以 S0 建档为准） | 回写成功 + 重启正常 |
| **R4 救砖（人类主导）** | 无法进系统/无法进 fastboot | 人类自备：官方固件 fastboot 刷机 / 恢复模式 / 厂商工具 | 人类确认设备恢复 |

> 补充：AVB/vbmeta 相关处理（若刷 boot 后校验失败）以 S0 实测的 `veritymode` 为准，处理方式由架构/人类定【TBD-5】，本方案不预设命令。

### 6.3 触发与时限

- 触发：STOP-1~STOP-5 任一命中；
- 时限：判定失败后 **5 分钟内**开始回滚；回滚全流程目标 ≤ **10 分钟**；
- 演练：**S5R 强制演练**（首次刷入成功后立即做一次），演练记录进 `quality/records/`；
- 回滚后回归：设备回到 S0 基线（开机正常 + 管理器显示未修补 + 无隐藏目录/rc 残留），并记录「回滚后基线比对表」。

---

## 7. 异常/故障注入集（E 组，L4）

| # | 场景 | 步骤要点 | 判定 |
|---|---|---|---|
| E1 | 镜像损坏/截断 | 用截断后的 boot.img 走全链 | 明确错误（§3 F-2），不崩 |
| E2 | 不支持的镜像（.img/.gz/非 arm64） | 直接选择 | E102 + 指引（F-1/F-3） |
| E3 | 存储空间不足 | 填充设备存储至 <2GB 后修补 | E240 类错误 + 不产生半成品 |
| E4 | 解包工具缺失/失败 | 移除/破坏内置工具（测试版可注入开关） | 阶段错误可见（EC-5/§3） |
| E5 | 重打包失败 | 构造异常 ramdisk | 阶段错误可见 + 工作目录保留 |
| E6 | 引擎中途被杀 | 修补过程中 `kill -9` 引擎进程 | 无半成品可刷（V-11 铁律）；UI 报中断（EC-6） |
| E7 | 密钥丢失 | 清 App 数据 → 再提权 | 无法提权且**静默**（无告警刷屏）；有恢复路径（B-03-6） |
| E8 | 重复修补 | 对已修补内核再跑引擎 | 明确拒绝或幂等（二选一，须有明确行为且写进契约）；不得产出可疑镜像 |
| E9 | 断电/重启中断修补 | 修补中强制重启 | 恢复后无半成品残留；可重跑成功 |
| E10 | 权限/SELinux 拒绝 | 严格模式或受限 context 下运行 | 错误可见且不崩溃；提示可操作 |
| E11 | 退出码被吞（假成功） | 注入引擎 rc=126/1/超时，观察 App 终态（T4） | 必须判失败、无产物、无「成功」文案；出现假成功即 P0 |
| E12 | 依赖被移除 | 删除/改名引擎或解包器后点修补（T1 负样本） | 进修补前即阻断，文案点名缺失项；不得进脚本后 `exit 1` |
| E13 | 模块脚本睡死/挂死 | 放 `sleep 600` 的 post-fs-data.sh 模块开机（B-09） | 超时 kill、后续模块继续、开机成功；第 2 次启动自动 safemode 且可进桌面 |
| E14 | daemon 被杀/冻结 | `kill -9` daemon / SIGSTOP，观察授权与阶段 | 已建会话不受影响；init 自动重启 daemon（带退避）；阶段完成状态可补跑 |

---

## 8. 隐藏性 H 组与安全 SEC 组（沿用旧方案，改造点标出）

### 8.1 H 组（来源：A1 §8 十项特性；方法=基线差集）

H-0 基线快照；H-1 纯字节 patch（=V 组）；H-2 失败不落盘（=V-11）；H-3 SELinux 零触碰（`getenforce`、`/sys/fs/selinux/load` 无写入、普通进程 denial 行为不变）；H-4 提权进程 denial 静默；H-5 无挂载（`/proc/mounts` 与基线一致，排除模块用户态行为）；H-6 密钥唯一凭证、无 daemon/socket/设备节点/新增 sysfs；H-7 `/data/<key16>/` 双路不可见（64/32 位 getdents）且 label 伪装；H-8 seccomp 清除；H-9 零 kallsyms 运行时依赖；H-10 kit API 错误码一致；H-11 华为/CFI/PAC（华为路径不可达，进发布声明）。

**新项目改造点**：
- **H-12（新增）**：rc 注入的可见性——`init.sproot.rc` 与 `init.rc` 的 `import` 追加行**不得**引入新挂载点、新属性、新 socket；`getprop` 增量与基线比对；二次解析随后重启保持稳定。
- **H-13（新增）**：`/data/adb/sproot/` 目录与 `key` 文件（0600）的 SELinux label 与可读性——普通 uid 不可读，`ls` 可见性按方案约定（若约定为可见则记录，不作隐藏承诺）。
- **H-14（新增）**：守护进程形态核对——只允许「开机由 init 拉起 + 提权后常驻」这一种可见形态；不得新增内核常驻结构、不得新增可被 `ps` 轻易识别的固定进程名（命名规范由 sys-guard 定稿后回填判定）。
- **H-15（新增，lead Q7 裁决项）**：**第三方授权 socket 的检测面**——lead 已裁决「socket 暴露接受」，但必须逐项评估并留结论：
  1. socket 形态与名字来源（抽象 socket `@sproot-<12hex>`，名字 deploy 时随机、不含密钥任何片段）；
  2. 权限/可达性：非 root 进程能否 connect（预期可否？以 NEW-DOC-05 §4.4「按 uid 判定」为准，connect 本身可能开放但必须被 SO_PEERCRED 授权拦下）；
  3. `SO_PEERCRED` 是否可被伪造/绕过（结论：由内核填写、用户态不可伪造；需实测并记录取证方法）；
  4. **`/proc/net/unix` 枚举面**：socket 名是否可见、可见时的检出风险与结论（这是**接受项**，必须记录「已知暴露 + 缓解：名字不含密钥」）；
  5. socket 文件（`/data/adb/ap/bin/.su.sock` 0644）的存在性与可读性是否泄漏授权面信息。
  > 判定：以上 5 点均有实测记录 + 结论；任一「未记录」即 H-15 FAIL（不是要求零暴露——暴露已被接受，但**不得存在未记录的暴露面**）。

- **H-16（新增）**：授权面一致性——`/data/adb/ap/allow_list` 与 `auth` 子命令、socket 判定、管理器 `root-exec` 三者对同一 uid 的结论必须一致（抽 5 个 uid 交叉验证）；授权表在 tmp+rename 期间并发读不出现半表（SEC-5 覆盖，此处做端到端核对）。

### 8.2 SEC 组（沿用 SEC-1~SEC-6，SEC-3 按方案 B 调整）

- SEC-1 普通应用进程不可读取密钥（**发布否决项**）；
- SEC-2 密钥不出现在共享面（logcat/`/sdcard`/属性/env/boot 头部）；
- SEC-3 密钥内存面最小暴露（方案 B：守护每次经 `--superkey`/`--superkey-file` 取得，不常驻缓存；`/proc/<pid>/cmdline|environ` 可见性按 context 实测记录，属已知残余风险）；
- SEC-4 内核侧无密钥痕迹（错误密钥多次尝试，dmesg 零增量）；
- SEC-5 授权表完整性（0600 + 单写者 + flock；torn-read 熔断 / fail-open / uid 0、2000 跳过语义审计）；
- SEC-6 sepolicy.rule 模块的安全边界与 UI 警示。
- **SEC-7（新增，E3/Q7 配套）**：**授权 socket 安全**——①未经授权的 uid 连接后必须静默失败（无输出、exit 1、不提供任何「是否已授权」的侧信道差异）；②`SO_PEERCRED` 不可被客户端伪造（构造 setuid/降权请求验证判定仍按真实 uid）；③socket 消息不得回显密钥或路径前缀；④socket 可枚举性按 H-15 记录结论，不在本方按「必须不可见」判定。
- **SEC-8（新增，E1 实现注记）**：**k47 相对 token 的 cwd 劫持防护**——在提权进程 cwd 放置与 `k47` 同名的可执行文件，验证提权不被劫持（AP 应在 `/data/adb/sproot/` 等安全 cwd 执行）；若 root 加载了该同名文件即 **发布否决**。
- **SEC-9（新增，契约 §8 配套）**：**密钥不入 argv/日志**——全量扫描 `ps`、`/proc/*/cmdline`、`/proc/*/environ`、logcat、管理器与 AP 日志，已知密钥及其前 16 字符**零命中**；`--superkey-file` 临时文件用后必须删除（发布否决项）。
  > 说明：SEC-1~SEC-6 沿用旧口径；SEC-7/SEC-8 为 v1.1（E1/E3）新增接口的专属安全项，**发布门禁同列**。

---

## 9. 测试门禁

### 9.1 进入系统测试（首次刷入）前，研发必须交付（G 组）

> 任一缺失 → 质测不启动 L3 真机序列（可执行 L0/L1，不计验收）。**当前缺口**已标注。

| # | 交付物 | 验收形式 | 责任方 | 当前状态 |
|---|---|---|---|---|
| G-01 | 修补引擎 CLI 契约（C1 或其修订版）冻结：参数、退出码、NDJSON、report schema | 文档版本 + 质测会签 | patcher | C1 已有 v1.0；新项目修订待确认 |
| G-02 | **AArch64 引擎产物**（exec 独立 ELF 或 JNI `.so`）+ 可复现构建命令 + sha256 + 版本号 | `quality/static/verify_patch.py --elf`（架构/静态性/可执行位/TLS）+ 构建日志 + T1 静态检查 | patcher（task-3） | **✅ 证据已达成（2026-11-08 实测，见下）** |
| G-02-E1 | exec 形态：`engine/android/build/arm64/exec/libskrootpatch.so` — 2,893,952 B, mode 755, ELF64-LSB **AArch64(0xB7)**, ET_EXEC, **无 PT_INTERP / 无 PT_DYNAMIC / NEEDED=[]（全静态）**, PT_TLS p_align=0x40；sha256=`ddc04731be02c9c79fd50f5ee7e0922d670c64c6a8a8d04714e97469b825e986` | `--elf` 4/4 PASS | patcher | ✅ 2026-11-08 |
| G-02-E2 | JNI 形态：`engine/android/build/arm64/jni/libskpatchjni.so` — 2,958,000 B, mode 755, AArch64, ET_DYN, 无 PT_INTERP, NEEDED=[libm,libdl,libc]（进程内加载可接受）；sha256=`efc66fede84b8f0634568612cae94da276ea2601646704714c91cdb5ddfd518a` | `--elf` 4/4 PASS | patcher | ✅ 2026-11-08 |
| G-02-E3 | 运行时证据：qemu-user 下契约退出码 101/102 实测通过 | patcher 报告 + 日志（**待 patcher 补交命令与日志归档**） | patcher | ⏳ 待补交 |
| G-03 | 管理器 debug APK（CI artifact 可下载） | 安装成功 + 启动不崩 | app-client | 待 task-4 |
| G-04 | 五阶段 × 错误码 → UI 文案/指引**映射表** | 表格评审（B-06 判定依据） | app-client | 待 |
| G-05 | 日志落盘路径与脱敏规范 | 文档 + 样例日志 | app-client | 待 |
| G-06 | 模块与守护方案定稿（NEW-DOC-05） | 文档 + 终审记录 | sys-guard | ✅ 已交付 v1.0 草案（465 行），待 lead 终审 |
| G-07 | 旧链路失败根因报告（NEW-DOC-03）+ 规避项→测试项清单 | 文档 | patcher | 待 task-2 |
| G-08 | 样本镜像集（§4.2） | 文件 + sha256 + `--bootimg` 基线记录 | 人类（已提供）/patcher（解包产物）/质测（基线） | **✅ 样本已就位（boot/init_boot/vbmeta+SAMPLES.csv）**；基线 10/10 PASS；**Kernel Image 解包产物待 patcher 提供**（sha256 必须 == `4d445384…`） |
| G-09 | 首次刷入通道定稿 | 文档（含每步命令与失败回滚，task-10 交付） | architect/人类 | ✅ **已裁决**（MVP=本地修补+导出，主机 fastboot/Recovery；应用内直刷=P2） |
| G-09b | 刷入通道可复现验证（按定稿命令走通一次，含回读校验） | 记录 + 回读 sha256 | 质测 + 人类 | 待（S4 证据即验收） |
| G-10 | 密钥存储方案（Keystore/GCM 或等效）+ 守护日志脱敏自查 | 文档 + 自查记录 | app-client/sys-guard | 待 |
| G-11 | 设备建档卡（§5.1）+ 双介质备份 + 救砖确认 | 记录归档 | 人类/质测 | 待 |
| G-12 | 缺陷库/回归清单就绪 | 记录 | 质测 | 本方案落地后建 |
| G-13 | **离线/无网络构建证明**（T2）：断网 clean 构建日志 + 依赖 sha256 清单 | 构建日志 + 清单 | app-client/patcher | 待 |
| G-14 | **签名 release 包**可用（T6）：与 debug 同依赖、密钥路径无 DEBUG 门控 | APK + sha256 对照 + grep 记录 | app-client | 待 task-8 |
| G-15 | **单一调用路径与 exec 位置声明**（T7）：调用链说明 + 静态检查记录 | 文档 + 检查结果 | app-client | 待 |
| G-16 | **契约 v1.1 会签冻结**（E1–E5 修订落地，含 APK↔AP 补章：root-exec/deploy/auth/socket） | 冻结版契约 + 会签记录 | sys-guard/lead | 起草中（task-9） |
| G-17 | **AP 二进制 aarch64 产物**（`app/apd` Rust 改造）+ 交叉编译链固化（rustup target + NDK，`--locked`） | CI 产物 `libapd.so` + sha256 + 体积记录 | sys-guard/app-client（task-8） | **缺口，保持阻断**：本机无 NDK/目标工具链（NEW-DOC-05 §1.4）；CI 证据到位后翻转 |

### 9.2 合入门禁（每次提交/PR）

1. CI 构建绿（含 arm64 引擎与 APK）；
2. V 组静态断言全 PASS（脚本进 CI），其中 V-21/V-22/V-23 为 T2/T6/T7 的静态门（T 组进 CI 前置）；
3. T1（能力自检）与 T4（失败不假成功）的静态部分必须随每次修补链改动重跑；
4. B-06 相关：UI 错误展示改动必须附失败场景截图/logcat；
5. 无未关闭的阻断缺陷；契约文档变更须同步知会质测。

### 9.3 RC 门禁

1. B-01/B-02/B-03/B-05/B-06/B-07 全通过；
2. **T1~T7 全通过**（旧链路根因回归组，阻断级，证据须指回旧缺陷行）；
3. H 组（H-1~H-10、H-12~H-14、H-16 按定稿）通过；H-11 部分通过 + 发布声明，H-15 见第 6 条；
4. SEC-1~SEC-6 全部通过（SEC-1 为发布否决项）；
5. **B-04（模块生命周期）与 B-09（脚本超时 + bootloop 自动 safemode）全通过**（依据 NEW-DOC-05 终审版）；
6. **B-10（root-exec 唯一入口 + rc 矩阵 + 密钥不入 argv/日志）全通过**；
7. H-15（socket 检测面）结论已归档；SEC-7 / SEC-9 通过（与 SEC-1 同为发布否决项）；
8. **T-8（CI 产物一致性）与 T-9（APK 签名 V3）通过**（发布前必查，见 §9.4）。

### 9.4 发布门禁

1. RC 门禁全部通过；回滚演练记录归档；
2. 兼容性声明：仅验证魅族 21（6.1.25/Android 16/ARM64）单档，引擎支持域 3.10~6.18 内其余档位逐条列「未覆盖」；
3. 已知风险清单（H-11 未覆盖、SEC-3 cmdline 可见性、TBD 项）随版本归档；
4. 最终由质测签署/否决（否决权：阻断项 FAIL、回滚失败、门禁材料缺失、单机序列未按序）。

---

## 10. 用例追溯与记录规范

- 追溯：每项测试标注**来源文档 §**（已在各条「依据」中给出）；契约变更 → 对应测试项期望值复核并记录翻转日期。
- 记录：`quality/records/<阶段>-<日期>/` 下按测试项编号存放：`logcat.txt` / `dmesg.txt` / `report.json` / `sha256.txt` / `screenshot-*.png` / `verdict.md`（PASS/FAIL + 判定依据一句话）。
- 缺陷：编号 `Q-<序号>`，含复现步骤、期望/实际、证据路径、关联测试项编号；阻断级缺陷默认 P0。
- 密钥处置：任何归档材料不得含明文密钥（report.json 含 `root_key`，归档时必须脱敏或标注并单独加密封存）。

---

## 11. 风险与待决项

| # | 风险/待决 | 影响 | 建议/责任 |
|---|---|---|---|
| TBD-1 | ~~刷入通道未定~~ | ✅ **已裁决（2026-11-08）**：MVP = 本地修补 + 导出镜像，首次刷入走**主机 fastboot/Recovery**；应用内直写 boot 分区（`root-exec`/dd）需已 root/Shizuku ⇒ **P2 增量**，不作 MVP 门禁。已按此改 §1 L3、§2 B-01/B-02 两级判定、§5.1/§5.2 S4、§9 G-09。**落地文档与命令复现归 task-10** | lead（已决）/task-10 |
| TBD-11 | **P2 应用内直刷**（`root-exec`+dd/回读校验）未设计 | 已 root 用户的 OTA 后重刷体验 | P2：待 MVP 真机通过后再排期（前置已有 root 环境） |
| TBD-2 | ~~模块/守护方案未定稿~~ | ✅ **已交付（2026-11-08）**：`docs/NEW-DOC-05-模块与守护方案.md` v1.0 草案（含 §8 验收 7 项、§5.5 bootloop 守卫、§7 backlog），B-04 判定已按其 §9 写定；待 lead 终审后如需修订 1 日内回填 | sys-guard/lead |
| TBD-3 | arm64 引擎未交付（现有 x86_64） | L0 只能在宿主跑，L1 无法进行 | patcher（task-3） |
| TBD-4 | ~~无样本内核镜像集~~ | ✅ **已解决（2026-11-08）**：`samples/meizu21/{boot,init_boot,vbmeta}.img` + `SAMPLES.csv` 就位；bootimg 基线 10/10 PASS（`quality/records/S0-2026-11-08/bootimg-baseline.md`）。遗留：**Kernel Image 解包产物**待 patcher 交付（用于 `--sample meizu21-boot-kernel`） | patcher |
| TBD-12 | **CI 产物获取与归档流程**未固化：每个 run 的 artifact 下载/命名/sha256 记录由谁执行 | 门禁证据链（禁止本地构建） | 质测按 run id 手工下载先跑，CI（task-8）固化 artifact 命名与保留期 |
| TBD-5 | 槽位/AVB/BL 状态**部分确认**：官方 flash.bat 证明 **A/B 双槽 + `--slot=all`**；AVB/vbmeta 处理与 BL 解锁状态仍待 S0 实测 | 回滚路径 R1/R2 已可按 A/B 准备，AVB 细节待实测 | S0 建档实测（`fastboot getvar current-slot` / `ro.boot.veritymode`） |
| TBD-6 | miuix/Compose UI 自动化能力有限 | B-06 的 EC-1/EC-2/EC-4 需人工复核 | 采用截图+dump 混合，保留人工判定 |
| TBD-7 | ~~旧链路「一闪即退」根因未闭环~~ | ✅ **已闭环（2026-11-08）**：根因见 `docs/research/patch-failure-rootcause.md`（引擎未入包 P0-a / 构建期下载 P0-b / release 无密钥 P0-c / 日志面板卸载 P1 / 退出码自吞 P2 / 未校验产物 P3），已转为 §3A T1~T7 阻断项。残留项转入 TBD-9 | patcher |
| TBD-8 | ~~解包产物是否已解压~~ | ✅ **已闭环（2026-11-08）**：魅族 21 boot.img 的 kernel 段实测为**未压缩 arm64 Image**（0x38 魔数成立），引擎可直接接收；`Image.gz/lz4` 分支保留给其它样本（工具 BI4 会 WARN 提示先解压） | 质测（已实测）/patcher 复核解包产物 sha256 |
| TBD-9 | 解包/重打包组件的错误码未定义（F-2/F-7 期望码待回填） | B-06 的故障注入矩阵部分项只能按阶段名判定 | app-client 在 G-04 映射表中定稿后 1 日内回填 |
| TBD-10 | ~~契约 v1.1 尚未冻结~~ | ✅ **已关闭（2026-11-08 21:23 v1.1-FROZEN）**：§0.5 翻转清单 17 项全部 `acceptance_ready=true`，冻结时间与签署方已登记（§0.4/§0.5）；后续契约变更须重新会签并复核 | qa（已完成） |
| R-1 | 单设备资源风险（设备变砖期间测试停摆） | 序列拉长 | 人类可自行救砖（已确认）；每次刷机前备份+演练 |
| R-2 | 契约漂移（C1 修订后测试期望失配） | 返工 | 契约变更必须知会质测并复核期望值 |

---

## 12. 里程碑

| 节点 | 内容 | 依赖 | 状态 |
|---|---|---|---|
| M0 | 本方案 v0.1 评审 | — | 本文 |
| M1 | L0 可执行（脚本 + 样本镜像集 + arm64 产物） | G-02/G-08 | 待 |
| M2 | L1 可执行（APK 修补链 + 错误映射表） | G-03~G-05 | 待 |
| M3 | 首次真机刷入（S0~S5R，**主机 fastboot/Recovery**） | G-09/G-09b/G-11 + L4 演练就绪 + B-01 级 1 | 待 |
| M4 | 全量真机序列（S6~S10） | M3 + NEW-DOC-05 终审 + 契约 v1.1 冻结 + G-17 | 待 |
| M5 | RC/发布门禁签署 | M4 + 缺陷清零 | 待 |

---

## 附：变更记录

- v0.1（2026-11-08）：首版。新项目转向后的测试方案：L0~L4+R 分层；B-01~B-08 阻断项（含 **B-06 修补失败必须完整报错**为独立验收项，判据 EC-1~EC-8 + 8 类故障注入）；V-01~V-20 静态验证清单（交付 `quality/static/verify_patch.py` 覆盖内部断言 V1~V7）；魅族 21 建档卡 + S0~S10 序列 + S5R 强制回滚演练；回滚/救砖 R1~R4；E1~E10 故障注入；H 组新增 H-12~H-14；G-01~G-12 门禁；TBD-1~TBD-7 待决项。
- v0.1r3（2026-11-08，lead 四项缺口裁决 + v1.1 冻结准备）：①**TBD-1 关闭**——MVP=本地修补+导出镜像、首次刷入走主机 fastboot/Recovery，应用内直刷列 P2；新增 B-01/B-02 **「导出成功/刷入成功」两级判定**，L3、S0 建档（Recovery 可用性）、S4、G-09/G-09b、M3 同步改写；②G-02/G-17 保持阻断，记录「等 CI 证据（readelf/可执行位/静态链接或随包 libc++）后翻转」；③样本加载参数化：`verify_patch.py` 新增 `--sample/--manifest/--list-samples`（samples/manifest.json 契约，task-10 产出即可跑）；④v1.1 依据更新为 `phase2_skroot_ap_call_contract_v1.1.md`（v1.1-D1，lead 已签署、待 patcher/app-client 会签），按 E1 实现注记新增 **B-03-10 / SEC-8（k47 cwd 劫持防护，发布否决）**；⑤新增 §0.5「v1.1 冻结翻转清单」；新增 TBD-11（P2 应用内直刷）。
- v0.1r2（2026-11-08，契约 v1.1 + 模块方案同步）：①新增 §0.4「契约 v1.1 勘误映射」（E1 提权串=47 字符本体 / E2 su 随机命名 / E3 废弃 SP_REQUEST_UID 改 socket SO_PEERCRED / E4 授权表单一文件 / E5 部署并入管理器+AP），受影响用例标注「依据 v1.1（起草中，会签后冻结）」；②B-03 按 v1.1 重写判定（提权串 47 字符、socket 授权面、随机名 su、单文件授权表）；③B-04 吸收 NEW-DOC-05 §9 验收（三阶段脚本/部署幂等/undeploy）并新增 **B-09 模块脚本超时与 bootloop 自动安全模式**（阻断级，验证第 2 次启动自动禁用模块且可进桌面）；④H 组新增 H-15（Q7 socket 检测面：权限/`SO_PEERCRED` 不可伪造/未授权连接/`/proc/net/unix` 枚举，暴露接受但必须留结论）、H-16（授权面一致性）；SEC 组新增 SEC-7（socket 授权安全，发布门禁）；⑤E 组新增 E13/E14；⑥门禁新增 G-16（v1.1 冻结）、G-17（AP 二进制 aarch64 交叉编译产物）；RC 门禁并入 B-04/B-09/H-15/SEC-7；⑦TBD-2 关闭（NEW-DOC-05 已交付）、新增 TBD-10（v1.1 未冻结）。
- v0.1r（2026-11-08，根因回填）：lead 通报旧链路根因确证（`docs/research/patch-failure-rootcause.md`）→ 新增 **§3A 根因回归组 T1~T7（阻断级）**：T1 二进制能力自检、T2 无网络依赖、T3 错误可见性专项、T4 失败不假成功、T5 成功校验、T6 release 构建可用、T7 路径约束与单一调用路径；同步新增 V-21~V-23、E11~E12、G-13~G-15，门禁并入合入/RC 条件；TBD-7 关闭（根因已闭环），新增 TBD-8（解包产物是否已解压待真机验证）、TBD-9（解包/重打包错误码待回填）。
