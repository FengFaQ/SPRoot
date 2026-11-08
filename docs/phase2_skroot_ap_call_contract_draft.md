# SKRoot ↔ AP 调用契约（v0.2 · 合并内核修补会签裁决）

> 撰写/合并：系统守护（AP 消费方） · 会签：内核修补（SK 提供方，4 项 SK-TBD 已裁决，见各节标注） · 状态：**待双签确认 → leader 终审**
> 上游基线：APatch @ `e8e4651c`；对齐文档：a_088357a042463ba8（apd 深度分析 v4）、a_7103b902143367a9（内核修补 DOC-C1）
> v0.2 相对 v0.1 的重大变更：授权表内核接口全部取消（用户态化）；SK-ESC 改为 execve 形态；SK-SETPATH 删除（su 走部署形态）；「kp」别名删除。

---

## 1. 范围

定义 SKRoot 运行侧与 AP 二进制（改造自 apd）之间的全部对接接口。**不含**：APK ↔ AP 调用（沿用 APatch shell 形态）；修补引擎 I/O 契约（DOC-C1）。

## 2. 总则

### 2.1 错误码（v0.2 收窄适用范围）
errno 约定（成功 0、失败 -Exxx，禁自定义正数错误码）适用于 **AP 用户态接口**（子命令、文件 helper、授权表操作）。**特例**：SK-ESC 走 §3.1 的 execve 判据（成功时返回 -ENOENT + `getuid()==0`），不适用本条。

### 2.2 调用身份
- AP 发起提权（execve key）的进程：uid 任意（提权前），提权后 uid 0。
- SELinux context：AP 侧沿用 magisk_rules() 注入 + `u:r:magisk:s0`（模块生态兼容，与 SK 零触碰不冲突——注入发生在 AP 的 post-fs-data 路径，非 SK 内核行为）。
- SK 内核侧不校验调用方 SELinux context（A1 隐蔽特性）。

### 2.3 密钥传递（终选·方案 B 无状态传参，裁决 4）
- AP 不缓存密钥、不建会话；每次进程启动经 `-s/--superkey <KEY>` 获得密钥（48 位字符串），提权动作 = `execve(key_str, NULL, NULL)` 现取现用。
- **内核侧行为（裁决 4）**：密钥校验失败=静默拒绝，execve 表现为普通 ENOENT，无计数、无告警、无内核态痕迹（任何常驻计数器都是检测面，违反 A1 特性 4/5）。
- AP 侧日志：提权失败记一行（tag APatchD），**日志内容不得包含密钥原文或前缀**（脱敏）。
- 理由（定稿依据）：内核每次提权内联验 key，无会话可缓存——方案 B 是唯一可行而非偏好。

### 2.4 幂等性
- SK-ESC（execve key）：幂等（重复执行再次验 key 并重写 cred，无副作用）。
- 授权表文件写：以单写者模型保证（§3.2）。

## 3. 接口规格

### 3.1 SK-ESC —— 提权（v0.2 重写，裁决 1）
- **形态**：`execve(key_str, NULL, NULL)` —— 密钥字符串本身是 SK 隐藏目录下的可执行路径（`/data/<密钥前16字符>/su`，见 §3.4），execve 触发 SK do_execve hook，就地改 cred 为 uid 0。**不是**「调用返回码」式接口。
- **成功判据**：execve 返回后 `getuid()==0`。execve 本身的返回值无意义（成功路径返回 **-ENOENT**——密钥路径对普通 stat/access 不可见，这是隐蔽性设计的一部分），AP 封装不得以 execve 返回值判成败。
- AP 侧落地：`privilege_apd_profile()` 与 `root_shell()` 改写为 escalate 封装（参考 SKRoot 上游 testRoot get_root 写法）：execve 失败返回后检查 `getuid()==0`，为 0 即提权成功继续执行；非 0 且 execve 报 ENOENT → 无效密钥（静默失败，记脱敏日志）。
- 失败行为：AP 记日志继续运行，不得 panic。
- 原 v0.1 的「一次提权会话内免验」表述废弃：SK 无会话概念，提权后进程**就是** uid 0，不存在后续内核验 key。

### 3.2 授权表 —— 用户态强制（v0.2 重写，裁决 2：内核接口取消）
- **原 SK-GRANT/REVOKE（0x1100/0x1101）删除**：授权表不在内核，为 **AP 自管用户态文件**。
- 存储：`/data/adb/ap/allow_list`（csv：pkg,uid,to_uid,allow,exclude,sctx —— 沿用现 package_config 格式，零迁移）+ `/data/adb/ap/exclude_list`（§3.6）。
- 一致性模型【裁决 2】：**单写者**（uid-listener 唯一写）+ 文件锁（flock）；读侧（su 判定）只读。
- `refresh_ap_package_list()` 的安全语义原样迁移：torn-read 熔断（配置不可读且存在非关键授权时不清表）、fail-open、uid 0/2000 跳过——这些逻辑从「内核撤销+重授」改为「文件重写」，防护目标不变（防误清全表）。
- SuProfile `{uid,to_uid,scontext[96]}` 布局保留，仅作为 AP 内部约定。
- 授权执行点：进程 exec su（§3.4 路径）→ AP root_shell() → 查 allow_list 判定该 uid 是否已授权（AP 用户态强制）。

### 3.3 授权表读 —— 用户态（v0.2 重写，裁决 2）
- **原 SK-COUNT/LIST（0x1102/0x1103）删除**：授权表读=AP 读自己的文件，COUNT-then-LIST 同进程同文件天然一致，无需内核快照语义。
- AP 侧 helper（供 APK 查询授权列表）保留 `-ENOSPC` 缓冲区语义（读缓冲不足报错而非截断）。
- 与 DOC-C1 §5.1 的统一口径【双签确认】：**授权表 = AP 用户态强制 + UI 审计，内核不参与**（非目标 N1）。

### 3.4 su 部署契约（v0.2 重写，裁决 3：SK-SETPATH 删除）
- **SK-SETPATH（0x1111）删除**：无内核 su 重映射。
- 替代形态（裁决 3）：SPRoot install_su 等价实现把 **AP multicall 二进制本体**部署为隐藏目录 `/data/<密钥前16字符>/su` 的「su」文件。用户/脚本执行 su 时 argv[0] basename = "su"，AP 的 argv[0] 分发直接命中（分发只看 basename，隐藏目录名含密钥随机段不影响）。
- AP 侧要求：
  1. argv[0] 分发表**只保留 su 分支**；「kp」别名**删除**（kpatch CLI 不存在）。
  2. `resetprop`、`magiskpolicy` 符号链接分发保留（/data/adb/ap/bin/ 下，指向 AP 本体）。
  3. `root_shell()` 内部逻辑不变（交互 shell + allow_list 授权判定）。
- `/data/adb/ap/su_path` 文件废弃（随 0x1111 删除），init_load_su_path() 一并删除。

### 3.5 安全模式（v0.2 修订，裁决备注）
- 原 SK-SAFEMODE（0x1112）删除（内核不参与）。
- AP 侧 `is_safe_mode()` 两路保留：`persist.sys.safemode==1` / `ro.sys.safemode==1` 属性检测；
- 叠加文件标志路【裁决备注】：`/data/adb/sproot/safemode` 文件存在即 true（对齐 DOC-C1 §5.3，给用户手动触发口）。
- 三路任一命中即 safe mode：禁用全部模块、跳过 stage 脚本（行为不变）。

### 3.6 排除列表存储（不变）
`/data/adb/ap/exclude_list`（每行一个 uid，0600，AP 读写）。原 kstorage（0x1041）内核路径删除。

## 4. 密钥运行时形态（终选定稿，裁决 4）

方案 B（无状态传参）**终选**，非偏好而是唯一可行：内核每次提权内联验 key，无会话可缓存。内核失败行为=静默拒绝（普通 ENOENT，无计数无告警）。AP 侧日志脱敏要求见 §2.3。方案 A（常驻缓存）废止。

## 5. busybox 首装归属（定稿，不变）

APK 部署（打包 libbusybox.so，APatchApp.kt:148 模式 `cp -f ${nativeDir}/libbusybox.so /data/adb/ap/bin/busybox`）+ APM 安装脚本幂等兜底（`[ -x ... ] || cp`）。AP 本体不携带不下载 busybox。首装链：APK（root 可用）→ busybox → APM 模块包经旧机制安装 → AP 就位。

## 6. 对 AP 二进制实现（pl_55b89e1161f02d61）的影响清单

| v0.1 假设 | v0.2 定稿 | 实现影响 |
|---|---|---|
| 6 类 supercall 替换为 SK 内核接口 | 仅提权保留（execve 形态）；授权表/安全模式/kstorage/su-path 全部用户态化 | supercall.rs 大幅缩减：删 sc_su_grant/revoke/nums/allow_uids/reset_path/get_safemode/set_ap_mod_exclude，留 execve escalate 封装 |
| refresh 逻辑调内核授权表 | refresh 重写用户态文件（flock 单写者） | uid-listener 改文件写，防护语义迁移 |
| argv[0] 分发 su/kp/resetprop/magiskpolicy | 只留 su + 两个符号链接 | cli.rs 删 kp 分支 |
| su_path 注册 | 删除 | defs.rs/event.rs 相应清理 |
| is_safe_mode 三路 | 属性两路 + 文件标志 | utils.rs 小改 |

## 7. 会签栏

| 方 | 状态 | 说明 |
|---|---|---|
| 系统守护（AP 消费方） | ✅ v0.2 合并完成 | 4 项裁决全部合并；§3.1/3.2/3.3/3.4 重写，§6 影响清单新增 |
| 内核修补（SK 提供方） | ✅ 双签确认（2026-11-06） | 逐条核验无偏差：「execve 返回值不得判成败」「方案 B 唯一可行」等关键约束均确认 |
| leader | ✅ 终审通过（2026-11-06） | N1 口径确认无异议；本契约定稿为 pl_04906c2c2baaa280 交付物；遗留项：DOC-C1 §5.1 密钥形态表述待内核修补加终局标注（指向本契约 §2.3） |

**定稿状态：v1.0（终审通过，契约锁定）**。安全附加要求（leader 终审补充）：普通应用进程不可读取密钥（密钥只在 AP/APK 侧），此项列入质测安全测试项。

*版本历史：v0.1 草案（2026-11-06，系统守护）→ v0.2 合并内核修补 4 项裁决（2026-11-06）：ESC 改 execve 判据、授权表用户态化（GRANT/REVOKE/COUNT/LIST 内核接口取消）、SETPATH 删除改部署形态、kp 别名删除、密钥方案 B 终选、SAFEMODE 叠加文件标志。*
