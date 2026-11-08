# SPRoot 修补引擎 I/O 契约（SPROOT-DOC-C1）

- 文档编号：SPROOT-DOC-C1
- 作者：内核修补（内核/修补方向）
- 状态：v1.0 草案（待 leader 仲裁 + 客户端/系统守护会签）
- 依赖：SPROOT-DOC-A1（SK 机制深度分析）、系统守护《AP 系统模块与 apd 深度分析及 SK 适配设计》v4（a_088357a042463ba8）
- 读者：客户端（APK 修补 UI 与密钥流程）、系统守护（AP 二进制）、质测（门禁用例）、总设计

---

## 0. 范围与不做什么

本契约定义 **SPRoot 修补引擎（组件名暂定 `skroot-patch`）** 的输入/输出/退出码/进度/错误面。它只负责「内核 Image → SK 修补内核 + 48 位 Root 密钥」；**不负责** boot.img 的解包/重打包（由调用方用 magiskboot 或 kptools 的 bootimg 组件完成，理由见 A1 §9-1；客户端现有修补链已具备该能力，见客户端《AP 管理器与镜像修补链分析》）。

术语约定：
- **内核 Image**：boot.img 解包后的、已解压的 arm64 内核二进制（即上游 patch_kernel_root 的直接输入对象）。
- **SK 修补内核**：本引擎输出的、已织入 SK shellcode 的内核 Image。
- **Root 密钥**：48 字符 `[A-Za-z0-9]{48}` 随机串（字符集按 leader 裁决定死）。引擎落盘时写入内核的是密钥前 47 字符 + NUL（继承上游 A1 §4.1 语义，密钥第 48 字符不参与内核内比对，仅作为展示长度）。

## 1. 命令行契约（引擎唯一入口）

```text
skroot-patch --in <kernel_image> --out <patched_kernel> (--gen-key | --key <48字符>)
             [--json <progress_sink_path>] [--workdir <dir>]
             [--verbose | --quiet]
```

| 参数 | 必选 | 语义 | 约束/默认 |
|---|---|---|---|
| `--in <path>` | ✅ | 输入内核 Image 路径 | 必须可读；拒绝 `.img`/`.gz`（引导调用方先解包解压，与上游 check_file_path 语义一致） |
| `--out <path>` | ✅ | 输出修补内核路径 | 引擎**只写该文件**，绝不回写输入（上游是原地写，SPRoot 改为显式 out，见 A1 §9-2 原子性要求） |
| `--gen-key` / `--key <KEY>` | 二选一 | 自动生成 48 位随机密钥 / 用户指定密钥 | `--key` 必须整体匹配 `^[A-Za-z0-9]{48}$`，否则 E101 |
| `--json <path>` | ❌ | 结构化进度/结果流（NDJSON）落盘路径 | 不给则不落盘，仅 stdout 人读 |
| `--workdir <dir>` | ❌ | 临时分析目录 | 默认 `<out>.work/`，成功后自动清理，失败时保留供诊断 |
| `--verbose/--quiet` | ❌ | 日志级别 | 默认 verbose |

**与上游差异（有意为之）**：上游为交互式 CLI（cin 密钥选择、cin 是否写盘）。SPRoot 引擎必须全参数化、零交互、单次调用完成——它是被 APK 经 JNI/进程调用的后端，不是人用的工具（A1 §9-2）。

## 2. 产物与输出契约

### 2.1 文件产物

| 产物 | 路径 | 内容 |
|---|---|---|
| SK 修补内核 | `--out` 指定 | 与输入同长度的内核 Image，字节级 patch（不改变大小/入口/布局，A1 §1） |
| 结果报告（可选） | `<out>.report.json` | 机器可读完整结果（见 2.3），成功/失败都写 |
| 工作目录 | `<out>.work/` | 失败时保留：含符号解析 dump、偏移推导中间值，供质测与开发诊断 |

### 2.2 Root 密钥交付

- 密钥以**明文 48 字符**出现在三处：stdout 末行 `ROOT_KEY=<48字符>`、`--json` 流的 `result.root_key` 字段、`<out>.report.json` 同字段。
- **不做**密钥文件落盘（密钥管理是 APK 的职责：客户端生成→调用引擎 `--key`→密钥由 APK 加密存储进管理器数据库；引擎进程退出后密钥只存在于调用方）。
- 语义提醒（继承 A1 §4.2）：密钥**前 16 字符即 `/data` 隐藏目录名**——客户端向用户展示密钥时注意：目录名由密钥自derive，无需单独配置。

### 2.3 `--json` NDJSON 事件流与 report 结构

每行一个 JSON 事件（`ts` 为 epoch ms）：

```jsonc
// 进度事件（phase 取值固定枚举）
{"ts":0,"event":"progress","phase":"read_input","pct":0,"detail":"size=12345678"}
{"ts":1,"event":"progress","phase":"parse_kallsyms","pct":5,"detail":"version=5.10.157-android13"}
{"ts":2,"event":"progress","phase":"derive_offsets","pct":20,"detail":"cred=0x2a0,uid=0x4,seccomp=0x9a8"}
{"ts":3,"event":"progress","phase":"locate_init_cred","pct":30,"detail":"init_cred=0x1a2b3c"}
{"ts":4,"event":"progress","phase":"patch_cfi_huawei","pct":40,"detail":"cfi=6 hw=0"}
{"ts":5,"event":"progress","phase":"patch_hooks","pct":60,"detail":"space=die@0x2c1d40"}
{"ts":6,"event":"progress","phase":"embed_key","pct":80,"detail":"len=48"}
{"ts":7,"event":"progress","phase":"write_output","pct":95,"detail":"bytes=12345678"}
// 终态事件（二选一，恒为最后一个事件）
{"ts":8,"event":"result","code":0,"root_key":"<48字符>","hook_table":[...],"report_ref":"<out>.report.json"}
{"ts":9,"event":"result","code":E2xx,"error":"E210: cred offset derivation failed at sys_getuid","detail":"..."}
```

`report.json` 顶层结构：

```jsonc
{
  "engine_version": "1.0.0",           // SPRoot 修补引擎版本
  "upstream_baseline": "943382cd",     // SKRoot 基线（一致性追溯）
  "input": {"path":"...","sha256":"...","size":0},
  "output": {"path":"...","sha256":"...","size":0},
  "kernel_version": "5.10.157-android13",
  "key_spec": {"charset":"A-Za-z0-9","length":48,"stored_len":47,"source":"gen|user"},
  "hook_table": [                       // 每个 hook 的落位（质测核对/审计用）
    {"name":"do_execve","symbol":"do_execveat_common","hook_addr":"0x...","trampoline":"0x...","bytes":220},
    {"name":"filldir64","symbol":"filldir64","hook_addr":"0x...","trampoline":"0x...","bytes":96},
    {"name":"compat_filldir","symbol":"compat_filldir","hook_addr":"0x...","trampoline":"0x...","bytes":96},
    {"name":"avc_denied","symbol":"avc_denied","hook_addr":"0x...","trampoline":"0x...","bytes":80},
    {"name":"audit_log_start","symbol":"audit_log_start","hook_addr":"0x...","trampoline":"0x...","bytes":80},
    {"name":"cfi_bypass","symbol":"__cfi_check","count":6},
    {"name":"huawei_bypass","symbol":"hkip_check_uid_root","count":0}
  ],
  "hidden_dir_prefix": "<密钥前16字符>",   // /data/<prefix>/ 隐藏目录名，A1 §4.2
  "errors": []
}
```

## 3. 退出码契约（稳定枚举，供 APK/质测/CI 判定）

| 码 | 名称 | 含义 | 调用方可做的动作 |
|---|---|---|---|
| 0 | OK | 成功，产物就绪 | 读取 root_key，走刷入链路 |
| 101 | E_USAGE | 参数缺失/互斥冲突/`--key` 格式错 | 修正调用参数（bug，不该出现在生产） |
| 102 | E_INPUT | 输入不可读/是 .img/.gz/非 arm64 Image | 提示用户重选 boot.img 或先解包 |
| 201 | E_KALLSYMS | kallsyms 解析失败 | 不支持该内核 → 提示「机型暂不支持」 |
| 202 | E_SYMBOL_MISSING | 关键符号缺失（A1 §3 表必选符号缺一） | 同上 |
| 210 | E_OFFSET_DERIVE | cred/uid/seccomp 偏移推导失败 | 同上，附 workdir 供上报 |
| 211 | E_INIT_CRED_NOT_FOUND | init_cred 模式搜索失败 | 同上 |
| 220 | E_NO_HOOK_SPACE | shellcode 空间不足（die/cfi 区域过小） | 同上 |
| 230 | E_UNSUPPORTED_VERSION | 内核版本超出 3.10~6.18 支持域 | 提示不支持 |
| 240 | E_WRITE | 输出写失败（磁盘/权限） | 检查存储空间 |
| 250 | E_INTERNAL | 未分类内部错误 | 上报日志 |

**行为铁律（继承 A1 §8-9）**：任何非 0 退出码下，引擎**不得**写出任何 `--out` 文件（哪怕部分写完也必须删除）；`--out` 若已存在同名文件，仅在成功时覆盖。这保证 APK 永远不会把半成品刷进 boot.img。

## 4. SK 特性继承清单（实现验收基线）

引擎实现必须逐项满足 A1 §8 的 10 项特性，其中验收必查：

1. 纯字节 patch：`out.sha256 != in.sha256` 但 `out.size == in.size`；out 反汇编中不出现新 section/新段。
2. 密钥内嵌位置：out 中存在 48 字节窗口等于 `[key[0..46] + NUL]`，且该窗口位于某 hook 跳板头部。
3. 隐藏目录前缀一致性：`report.hidden_dir_prefix == root_key[0:16]`。
4. SELinux 零触碰：out 中 avc_denied/audit_log_start 仅被跳板重定向，policy 数据区无 diff。
5. seccomp 清除逻辑存在：do_execve shellcode 含 TIF_SECCOMP bic + task->seccomp 清零（A1 §4.3-5）。
6. 失败不落盘：人为构造 E210/E220 用例验证 out 未产生。
7. PAC/BTI 感知：对含 paciasp 序言的符号，跳板序言含 `paciasp`（A1 §6-2）。

质测可直接把 1/2/3/6/7 做成自动化断言（对固定测试内核 Image 跑引擎后做字节级校验）。

## 5. 与消费方的对接决定（回应系统守护 §4/§5 开放问题）

### 5.1 apd 侧提权（回应守护 §4.1 / §5.3「密钥运行时形态」）

SK 提权对 apd/AP 的可见形态**只有一条**（A1 §4.3）：

```c
// 任何进程获得 root 的唯一方式：
execve(key_str /* 47字符 */, NULL, NULL);   // 返回 ENOENT 但 uid 已变 0
if (getuid() == 0) { /* 已提权 */ }
```

- **密钥运行时形态【终局，已随调用契约 v0.2 双签定稿】**：**方案 B 无状态传参**——apd/AP 不缓存密钥，每次进程启动经 `--superkey` 参数获得，所有提权动作现取现用；内核侧每次提权内联验 key（无会话可缓存，方案 B 是唯一可行而非偏好）；密钥校验失败=**静默拒绝**（普通 ENOENT，无计数无告警，任何内核态痕迹都违反隐蔽性）；AP 侧日志脱敏（不含密钥原文/前缀）。完整规格以《SKRoot↔AP 调用契约》v1.0 终稿（pl_04906c2c2baaa280，资产 a_43f9e853a06b5109 v4）§2.3/§4 为准，本文不再单列。（本节初稿曾写「常驻进程内存缓存 SPROOT_KEY env」，已废止。）
- **授权表四操作（grant/revoke/list/count）**：SK 内核无 supertable。SPRoot 落地方案：授权表由 apd 用户态自管（复用现有 package_config csv + 文件锁），提权面只认「execve 密钥」这一个动作——任何持有密钥的进程天然拥有完整 root，**授权表降级为「APK UI 层的授权展示与审计」而非内核强制项**。若第二阶段总设计要求内核级强制（每 uid 侧门），需扩展 SK shellcode（成本高、隐蔽性风险），本契约默认不提供，标记为**非目标 N1**。
- **scontext**：SK 提权后进程 context 保持原样（SELinux 零触碰设计的推论）。守护 §4.4 要求的 `u:r:magisk:s0` 由 apd 自行 `setcon`（上游 testRoot 已示范 `selinux_procattr::setcon("u:r:shell:s0")` 可行，A1 §4.3；APK 传 `-Z` 的机制继续有效）。**无内核接口参与 context 切换。**

### 5.2 su_path 重映射（回应守护 §5.2）

- KP 的 `SUPERCMD=/system/bin/truncate` 重映射在 SK 中**无等价物**（A1 §8-4：无 daemon、无 syscall 表修改）。
- SPRoot su 部署沿用上游语义：`/data/<密钥前16字符>/su` 隐藏目录 + filldir 双路隐藏（A1 §4.4）。apd 侧 `su_path` 注册接口（0x1111 对端）**删除**；守护侧把「注册内核 su 路径」改为「APK 经 SK kit API `install_su(key)` 获取路径后写入本地配置」。安装时机：管理器首启时由 APK 调 SK kit（借道已提权 apd 或 APK 自持密钥直接提权）执行。

### 5.3 安全模式查询（回应守护 §4.3）

- SK 无内核 safemode 位。`SUPERCALL_SU_GET_SAFEMODE(0x1112)` 对端删除；`is_safe_mode()` 收窄为属性两路（守护已论证安全侧不放宽）。SPRoot 追加**文件标志路**：`/data/adb/sproot/safemode` 存在即 true（APK 出问题时可被用户/adb 创建），纳入守护改造。

### 5.4 事件上报 / kstorage（守护 §6 第 5/6 类接口）

- 事件上报：**非目标 N2**（SK 无常驻内核通道；APM 事件审计走 apd 本地日志）。
- kstorage 替代存储：**非目标 N3**；需要持久小状态用 `/data/adb/sproot/` 文件区。

## 6. 与 KP 风格的产物区分（供质测判别，A1 §9-3）

| 维度 | KernelPatch 修补产物 | SPRoot(SK) 修补产物 |
|---|---|---|
| 镜像大小 | 内核尾部追加 kpimg，**变大**（4K 对齐 + preset 区） | 与原内核**严格等长** |
| 入口指令 | `_stext` 首条 B 被改写到 kpimg | execve 入口（do_execve 系）首条指令改为 B 跳板 |
| 数据结构 | setup 区（含 superkey SHA256、kallsyms 偏移表） | 无任何常驻结构；48B 密钥在跳板体内 |
| 可见特征 | `kpatch` supercall 号段、preset magic 可扫描 | 无 magic、无 supercall；唯一指纹是五处跳板字节 |
| 鉴权模型 | superkey（SHA256 存 setup） | 48 字符明文密钥（密钥即路径前缀） |

质测门禁可用上表做「KP 残留检测」（修补产物中不得出现 preset magic / kpimg 特征节名）。

## 7. 待仲裁/会签项

| # | 事项 | 我的建议 | 待谁 |
|---|---|---|---|
| A | §5.1 授权表降级为 UI 层语义（无内核强制） | 采纳（隐蔽性优先） | leader 仲裁 + 守护会签 |
| B | `-s/--superkey` 参数名保留还是更名 `--key` | 保留 `--superkey`（APK 侧改动最小），语义重定义 | 守护会签 |
| C | 密钥环境变量名 `SPROOT_KEY` | 采纳 | 守护会签 |
| D | 引擎组件名 `skroot-patch` 与二进制交付形态（独立 ELF，APK jniLibs 内置，A1 §9-2 参数化） | 采纳 | 客户端会签（JNI 调用形态） |
| E | `--json` NDJSON 事件枚举与 report schema 定稿 | 本文即基线 | 客户端/质测会签 |
| F | 非目标 N1/N2/N3 是否接受 | 采纳 | leader 仲裁 |

---
*本契约成文后即为 SPRoot 修补引擎实现（pl_c86514ad3488e53f）的验收依据；实现偏差须回改本文并重新会签。*
