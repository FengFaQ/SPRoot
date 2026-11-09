# SKRoot ↔ AP 调用契约（v1.1 · 勘误修订版）

> 修订：系统守护（sys-guard，AP 消费方） · 依据：《NEW-DOC-05 模块与守护方案》§4.6 + 修补引擎源码实证 · 日期：2026-11-08
> 状态：**v1.1-FROZEN（冻结：2026-11-08 21:23 +08:00）** —— 三方会签完成：系统守护 ✅ / 内核修补 patcher ✅ / 客户端 app-client ✅ / leader 终审 ✅；**TBD-10 关闭**。冻结后任何条款变更须走 v1.2 修订流程，不得就地修改。
> 基线：v1.0（`docs/phase2_skroot_ap_call_contract_draft.md`，2026-11-06 三方签署：系统守护 ✅ / 内核修补 ✅ / leader ✅ 终审通过）
> 上游依据：APatch @ `e8e4651c`、SKRoot @ `943382cd`（A1）、引擎实现 `engine/src/`
> 本文档**取代** v1.0 作为实现验收依据；v1.0 中未列入 §0 变更明细的条款**逐字继承、不做静默改动**。

---

## 0. 修订明细（全部差异都在这一节）

> **评审方式**：本节 5 条即全部差异；§1–§8 是已落文全文；§9 为签署矩阵（v1.1-FROZEN，全部 ✅）。
> **编号约定**：**§2/§3/§4/§5/§6 与 v1.0 原编号严格对齐**（v1.0 §2 总则→本文 §2、§3 接口规格→本文 §3、……），因此其他文档里「CALL §3.1 / §3.2 / §3.4」的引用在 v1.1 下**继续有效**；新增内容只占 §7（部署职责）、§8（APK↔AP）。
> 标记：`【E#】`=本次勘误；`【P】`=需 patcher 会签；`【C】`=需 app-client 会签；`【L】`=需 leader 终审。

### 0.1 一览

| # | 勘误点 | v1.0 口径 | v1.1 口径 | 级别 | 签署方 |
|---|---|---|---|---|---|
| **E1** | 提权串形态 | `execve(key_str)`，key_str **是隐藏目录路径** `/data/<key16>/su` | `execve(k47)`，`k47 = key[0:47]`，**裸密钥串、无路径分隔符** | 🔴 阻断（照旧实现必失败） | 【P】【C】 |
| **E2** | su 部署文件名 | 隐藏目录内固定名 `su` | 隐藏目录内**随机 16 字符名** + `bin/su` 符号链接 | 🟡 隐藏性 | 【P】【C】 |
| **E3** | 授权执行点 | `SP_REQUEST_UID` 由内核 hook 注入 env，AP 据此还原请求方 uid | **删除该需求**；第三方走抽象 socket + `SO_PEERCRED`，管理器走 root-exec，直连 su 仅 root 上下文 | 🔴 阻断（内核侧不可实施） | 【P】【C】 |
| **E4** | 授权表存储 | `allow_list` csv + 独立 `exclude_list` 双文件 | **单文件** `allow_list`（exclude 为行内列），删除 `exclude_list` | 🟢 简化 | 【C】 |
| **E5** | 部署职责 | 独立部署器 `skroot-deploy` 交付 install_su / write_key_file / inject_rc / uninstall_all | **取消该组件**：rc 注入归管理器修补流程，install_su/key/目录/卸载归 AP `deploy`/`undeploy` | 🟡 分工 | 【P】【C】【L】 |

### 0.2 E1 · 提权串 = 47 字符密钥本体，不是隐藏目录路径 【P】【C】🔴

| 项 | 内容 |
|---|---|
| **原文**（v1.0 §3.1） | 「形态：`execve(key_str, NULL, NULL)` —— **密钥字符串本身是 SK 隐藏目录下的可执行路径**（`/data/<密钥前16字符>/su`，见 §3.4），execve 触发 SK do_execve hook，就地改 cred 为 uid 0。」 |
| **修订文**（v1.1 §3.1） | 提权调用 `execve(k47, NULL, NULL)`，其中 **`k47` = 48 字符密钥的前 47 个字符**（`[A-Za-z0-9]{47}`，**不含任何路径分隔符**）。内核 do_execve hook 将 filename 参数与内嵌密钥**逐字节比对至 NUL**，命中即就地改写 cred；随后内核按正常 execve 流程查找路径（必然 `ENOENT`）→ 调用返回 `-ENOENT`，**但进程已是 uid 0**。成功判据**只有** `getuid()==0`。`/data/<key16>/<随机名>`（E2）是 **su 部署位置**，与提权串无关。 |
| **证据** | `engine/src/patch_do_execve.cpp:76-98`：shellcode 头部 embed 48B 密钥缓冲；`ldrb w14,[x11],#1 / ldrb w15,[x12],#1 / cmp / b.ne end / cbnz w15, loop` = filename 与密钥逐字节 strcmp 至 NUL；命中后 `:99-135` 改 cred/caps/清 seccomp，再跳回 `do_execve+4`。`engine/src/skroot_patch_main.cpp:312-317`：落盘时 `write_key = root_key 去掉最后 1 字符`，写 `len+1` 字节 = **47 字符 + NUL**。A1 §4.1/§4.3 同口径（`internel_key.erase(size()-1)` 后 `execve(internel_key, NULL, NULL)`）。<br>**patcher 独立复核**：`patch_do_execve.cpp:70,89-102`、`skroot_patch_main.cpp:325-330`（与本文引用同一语义，行号随引擎版本微差）；结论一致——**传 48 字符会在第 48 位失配**。 |
| **为什么 v1.0 会写错** | 把「密钥前缀即隐藏目录名」（A1 §4.2 的正确结论）与「提权参数」混成一句；两者共用密钥前缀，但**传进 execve 的是密钥本体、不是目录路径**。 |
| **影响面** | ① AP：`app/apd/src/supercall.rs:42-62` **已按正确语义实现，无需改**；② 客户端：密钥「48 存、47 用」截断在 `SukeyProvider`/调用层固化，**禁止**把 48 字符全文或任何路径交给 execve；③ 质测：新增单测「传 48 字符 → 提权失败（预期）；传 k47 → 成功」；④ 契约文字勘误，机制不变。 |
| **实现注记（非强制）** | `k47` 是无 `/` 的相对路径 token。hook 在路径查找**之前**改 cred，cwd 不影响提权；但若 cwd 恰有同名文件，execve 会**真的成功**（root 身份加载该文件）。故 `escalate()` 建议在已知安全 cwd（如 `/data/adb/sproot/`，且该目录无此 token 同名文件）执行；「随机 47 字符碰撞」概率可忽略。 |

### 0.3 E2 · su 部署：隐藏目录内随机命名 【P】【C】

| 项 | 内容 |
|---|---|
| **原文**（v1.0 §3.4） | 「SPRoot install_su 等价实现把 **AP multicall 二进制本体**部署为隐藏目录 `/data/<密钥前16字符>/su` 的「su」文件。用户/脚本执行 su 时 argv[0] basename = "su"，AP 的 argv[0] 分发直接命中……」 |
| **修订文**（v1.1 §3.4） | `mkdir /data/<key[0:16]>/`（0700 root，label 伪装 system_file）→ AP multicall 副本部署为**目录内随机 16 字符文件名**（`[A-Za-z0-9]{16}`，0700 root）→ 建立 `→ /data/adb/ap/bin/su` 符号链接。argv[0] 分发只依赖**调用者传入的 basename**（经 `bin/su` 调用时 argv[0]=`su`），与隐藏文件真名无关。**该入口仅对 root 上下文可用**（0700），不构成对普通 App 的授权面（授权见 E3）。 |
| **证据** | A1 §4.4（上游 kit 用随机名：`lib`+随机字母+`.so` 风格）；`app/apd/src/cli.rs:220-231`（分发只看 argv[0]）。 |
| **理由** | 固定名 `su` + 随机目录名 = 「目录不可枚举、内部名恒定」的组合指纹，按 8 字符文件名特征扫目录即可命中；随机名把可扫描面降到与上游一致。 |
| **影响面** | ① 客户端 deploy 按随机名实现；② AP 无需改；③ 契约 v1.0 §3.4「用户/脚本执行 su」收紧为「root 上下文入口」；④ 质测隐藏性检测面增加「隐藏目录内文件名非 `su`」。 |

### 0.4 E3 · 授权执行点：废弃 `SP_REQUEST_UID`，改 SO_PEERCRED 【P】【C】🔴

| 项 | 内容 |
|---|---|
| **原文**（v1.0 §3.2 执行点 + 时序稿 §3.6/§4-6） | 「授权执行点：进程 exec su（§3.4 路径）→ AP `root_shell()` → 查 allow_list 判定该 uid 是否已授权」；配套「SK do_execve hook 在 cred 改写时向新进程注入 `SP_REQUEST_UID=<原uid>` 环境变量，供 AP 还原请求方身份」。 |
| **修订文**（v1.1 §3.2、§3.3） | **删除 `SP_REQUEST_UID` 需求（内核侧该项交付取消）**。授权执行点按来源分三类：<br>**(a) 第三方 App**：经 AP 守护**抽象 socket** 发起 root 请求，AP 用 `SO_PEERCRED` 取内核填写的真实对端 uid（不可伪造）→ 查 allow_list → 授权则 fork 会话（按 `to_uid`/`sctx` 降权或切 context）→ 回传 stdio/exit code；拒绝静默（exit 1）。<br>**(b) 管理器**：持有密钥，经 `root-exec`（§8.2）直接以 root 运行 → 视为设备所有者，不查表，写审计日志。<br>**(c) `/data/<key16>/<随机名>` 直连**：调用者已是 root → 不查表，文档化为「root 上下文入口」。 |
| **证据（不可实施性）** | ① `engine/src/patch_do_execve.cpp` 全部 hook 只做「改 cred + 清 seccomp + 跳回」，**无任何 envp 构造**；② do_execve 入口处构造新 envp 需要分配/写用户内存并重排指针数组，发生在 LSM/路径解析之前，不可行；③ 即便注入 env，提权后 `getuid()==0` 已使「原 uid」不可信，仍需独立凭据；④ 时序稿 §3.6 原文即标「**交内核修补确认可行性**」= 从未确认。 |
| **影响面** | ① patcher：**撤出**「SP_REQUEST_UID env 注入」交付（净省一项内核改动）；② AP：`app/apd/src/apd.rs:196-205` env 读取删除，改 socket peer cred / `--request-uid`（仅 root 上下文可信）；③ 客户端：需 su 客户端（AAR）连抽象 socket；④ 契约 §3.2/§3.3 重写。 |
| **socket 暴露（已知风险，显式接受）【L】** | 抽象 socket `@sproot-<12hex>`（名随机，deploy 时生成，写 `/data/adb/ap/bin/.su.sock` 0644 供客户端读取）；`/proc/net/unix` 可枚举该名。名字不含密钥任何片段；列入质测隐藏性检测面「已接受项」。 |

### 0.5 E4 · 授权表：单文件 `allow_list`，删除独立 `exclude_list` 【C】

| 项 | 内容 |
|---|---|
| **原文** | §3.2：「存储：`/data/adb/ap/allow_list`（csv：pkg,uid,to_uid,allow,exclude,sctx —— 沿用现 package_config 格式，零迁移）+ `/data/adb/ap/exclude_list`（§3.6）」；§3.6：「`/data/adb/ap/exclude_list`（每行一个 uid，0600，AP 读写）」。 |
| **修订文**（v1.1 §3.2、§3.6） | **只保留单一文件** `/data/adb/ap/allow_list`（csv 六列 `pkg,exclude,allow,uid,to_uid,sctx` + 表头，0600 root），`exclude` 为**行内字段**；**删除** `exclude_list` 独立文件口径与 §3.6 对应内容；per-uid 模块排除判定 = 读同一文件 `exclude==1`。 |
| **证据** | `app/apd/src/package.rs:13-21`（`PackageConfig{pkg,exclude,allow,uid,to_uid,sctx}` 行内字段）；`app/apd/src/auth_list.rs:153-158`（`is_excluded()` 读同一文件）；v1.0 §3.2 自身已写「沿用现 package_config 格式」——该格式即行内列；`exclude_list` 在实现中从未存在。 |
| **理由** | 两份存储 = 双写不一致风险（kstorage gid=1 已随 v0.2 删除，无内核侧第二来源可对齐）；单文件 + flock + tmp/rename 的一致性模型更简单可验。 |
| **影响面** | 客户端 UI：排除开关读写同一 csv（经 AP `auth` 子命令）；契约 §3.6 删除；质测删除「两表一致性」用例。 |

### 0.6 E5 · 部署职责：并入管理器 + AP，取消 `skroot-deploy` 【P】【C】【L】

| 项 | 内容 |
|---|---|
| **原文**（时序稿 §4） | 「部署器交付物清单（暂名 `skroot-deploy`）：install_su(key) / write_key_file(key) / inject_rc(bootimg_dir) / uninstall_all(key) …交付形态倾向并入 skroot-patch 引擎同库导出」。 |
| **修订文**（v1.1 §7） | **取消独立部署器组件**，职责拆分且全部落在用户态：① **rc 注入** = 管理器修补流程第 7 步（repack 前）写 `init.sproot.rc` + `init.rc` 尾部追加 `import`；② **install_su / key 文件 / 目录初始化 / busybox / 符号链接** = AP `deploy` 子命令；③ **卸载** = AP `undeploy`；④ **校验** = 管理器 ArtifactVerifier（C1 §4）+ deploy 冒烟。 |
| **证据** | 新架构「手机本地修补」：rc 由管理器写 ramdisk 最自然，无需另建组件；NEW-DOC-05 §6.1/§6.2；NEW-DOC-01 §3.3-A 七步修补链（第 7 步 = repack）。 |
| **理由** | 组件越少失败面越小（leader 裁决 Q4）；`skroot-deploy` 读写 `/data` 与 boot 镜像最终仍需 root + 管理器配合，独立组件无收益。 |
| **影响面** | patcher：撤出该交付项，**只保留修补引擎本体（C1）**；app-client：承担 rc 注入与 deploy 调用；契约 §1 范围调整。 |

---

## 1. 范围（v1.1 修订）

定义 **SK 修补内核 ↔ AP 二进制**之间的全部对接接口，以及**新管理器 App ↔ AP 二进制**的最小调用契约（§8，v1.1 新增——v1.0 §1 明确「不含 APK↔AP」，但新架构管理器从零重写，shell 形态不再自动成立，必须定死）。**不含**：修补引擎 I/O（C1）、APK UI 实现、AP 内部模块语义（NEW-DOC-05）。

| 项 | v1.0 | v1.1 |
|---|---|---|
| 接口集合 | SK-ESC、授权表（用户态）、su 部署、安全模式 | 同左（口径按 E1–E4 修订） |
| 部署职责 | 不含（在时序稿 §4 定义 skroot-deploy） | **新增 §7**（E5） |
| APK↔AP | 明确不含 | **新增 §8**（root-exec/deploy/auth/version） |

---

## 2. 总则（v1.0 §2 继承）

### 2.1 错误码

errno 约定（成功 0、失败 `-Exxx`，禁自定义正数错误码）适用于 **AP 用户态接口**（子命令、文件 helper、授权表操作）。**特例**：SK-ESC 走 §3.1 的 execve 判据（成功路径返回 `-ENOENT` + `getuid()==0`），不适用本条。CLI 进程退出码见 §8.5。

### 2.2 调用身份

- AP 发起提权（execve key）的进程：uid 任意（提权前），提权后 uid 0。
- SELinux context：提权前沿用进程原有 context（rc 拉起 = init 域，时序稿 §3.5）；提权后 AP 可 `setcon("u:r:magisk:s0")`（A1 §4.3），失败保留原 context，不得 panic。
- SK 内核侧不校验调用方 SELinux context（A1 隐蔽特性）。模块 `sepolicy.rule` 注入沿用 `magisk_rules()` 路径（写 `/sys/fs/selinux/load`，非内核 hook 行为）。

### 2.3 密钥传递（方案 B 无状态传参，v1.0 裁决 4 继承）

- AP 不缓存密钥、不建会话；每次进程启动经 `-s/--superkey <KEY>` 或 `--superkey-file <0600 文件>` 获得密钥。
- **密钥规范化【E1】**：对外「48 字符」；内核内嵌与 execve 提权用 **`k47` = 前 47 字符**；第 48 字符不参与内核比对（保留为展示长度，A1 §4.1）。
- 提权动作 = `execve(k47, NULL, NULL)` 现取现用（§3.1）。
- **内核侧行为**：密钥校验失败 = 静默拒绝（普通 `ENOENT`，无计数、无告警、无内核态痕迹）。
- **AP 侧日志**：失败记一行（tag `APatchD`），**内容不得包含密钥原文或任何前缀**（脱敏）。

### 2.4 幂等性

- SK-ESC（execve k47）：幂等（重复执行再次验 key 并重写 cred，无副作用）。
- 授权表文件写：以单写者模型保证（§3.2）。

---

## 3. 接口规格

### 3.1 SK-ESC · 提权（【E1】重写）

- **形态**：`execve(k47, NULL, NULL)`，`k47 = key[0:47]`（`[A-Za-z0-9]{47}`，**无路径分隔符**）。
- **成功判据**：execve 返回后 `getuid()==0`。execve 自身返回值无意义（成功路径必为 `-ENOENT`，因 `k47` 不是真实路径）。
- **AP 侧落地**：`app/apd/src/supercall.rs::escalate()`（**已按此实现，保持**）；`privilege_apd_profile()` 与 `root_shell()` 复用该封装。
- **失败行为**：静默（记脱敏日志后继续/退出），**不得 panic**。
- **实现注记**：`k47` 为相对路径 token，建议在已知安全 cwd 执行（见 §0.2 实现注记）。
- 【P】内核侧确认：hook 语义即「filename 逐字节 strcmp 内嵌 47 字符 + NUL → 改 cred → 返回执行流」，不要求路径存在。

### 3.2 授权表 · 用户态强制（【E3】【E4】重写）

**存储**：`/data/adb/ap/allow_list`，csv 六列 `pkg,exclude,allow,uid,to_uid,sctx` + 表头，0600 root。**单一文件**，无 `exclude_list`（【E4】）。

**一致性模型**：
- **单写者** = AP 守护（`daemon`：uid-listener 线程 + `auth` 子命令 + socket 会话记录），全部经 `flock(LOCK_EX)`（`auth_list.rs:41-52`）；
- **写** = tmp + rename（`package.rs:127-185`）；读侧无锁（永远看到完整一代）；
- **安全语义原样保留**：torn-read 熔断（不可读且存在非关键授权 → 拒绝重写）、fail-open 边界、uid 0/2000 保护、空 `/data/system/packages.list` 视为撕裂拒绝同步、uid 重算（`package.rs:241-330`）。

**授权执行点（三类来源，【E3】替代 SP_REQUEST_UID）**：

| 来源 | 身份来源 | 判定 | 失败行为 |
|---|---|---|---|
| (a) 第三方 App | **socket `SO_PEERCRED`** 对端 uid（内核填写，不可伪造） | `allow==1 && exclude==0`，或受保护 uid（0/2000） | 静默拒绝，exit 1，无输出 |
| (b) 管理器 | 持有密钥 + `root-exec`（§8.2） | 设备所有者，不查表；写审计日志 | 提权失败 exit 125 |
| (c) `/data/<key16>/<随机名>` 直连 | 调用者已是 root | 不查表（root 上下文入口） | — |

**socket 规格（新增）**：Linux 抽象 socket `@sproot-<12 hex>`（deploy 时随机生成，名写入 `/data/adb/ap/bin/.su.sock` 0644）；请求帧 `{ver, cmd[], scontext?, to_uid?}`；响应回传 stdio 流 + exit code。daemon 按 `to_uid`/`sctx` 对会话降权/切 context 后 exec。**已知暴露**：`/proc/net/unix` 可枚举抽象名（已接受，见 §0.4）。

**其余口径**：读=AP 读自己的文件（`-ENOSPC` 缓冲语义保留）；`SuProfile{uid,to_uid,scontext[96]}` 布局保留为 AP 内部约定；授权执行点**内核不参与**（非目标 N1 不变）。

### 3.3 授权表读 · 用户态（v1.0 继承）

AP 读自己的文件；COUNT-then-LIST 同进程同文件天然一致，无需内核快照语义。AP 侧 helper（供管理器查询授权列表）保留 `-ENOSPC` 缓冲区语义（读缓冲不足报错而非截断）。与 C1 §5.1 统一口径：**授权表 = AP 用户态强制 + UI 审计，内核不参与**（非目标 N1）。

### 3.4 su 部署契约（【E2】重写）

1. 隐藏目录：`/data/<key[0:16]>/`，0700 root，label 伪装 `system_file`（A1 §4.4）。
2. 目录内：AP multicall 副本，文件名**随机 16 字符**（`[A-Za-z0-9]`），0700 root。
3. 便捷链接：`/data/adb/ap/bin/su` → 上述文件（符号链接不改变权限语义）。
4. AP 侧要求：
   - argv[0] 分发**只保留 `su` 分支**（basename 精确匹配，避免 `ends_with` 过宽）；
   - `resetprop`、`magiskpolicy` 符号链接分发保留（`/data/adb/ap/bin/` 下指向 AP 本体）；
   - `root_shell()` 内部逻辑不变（交互 shell + allow_list 判定，判定来源见 §3.2）。
5. **可用范围**：仅 root 上下文（daemon / root-exec 会话 / 已提权 shell）；普通 App 不由此获得 root，其请求走 §3.2(a) socket。
6. `/data/adb/ap/su_path` 文件与 `init_load_su_path()` 保持删除（v1.0 已定）。

### 3.5 安全模式（v1.0 继承 + 强化）

三路任一命中即 safe mode（禁用全部模块、跳过 stage 脚本）：
1. `persist.sys.safemode==1`；2. `ro.sys.safemode==1`；3. 文件 `/data/adb/sproot/safemode` 存在。
**v1.1 新增（NEW-DOC-05 §5.5）**：`boot_count` 自动升级——`post-fs-data` 入口 +1、`boot-completed` 清零；连续 2 次未完成启动 → 自动写 safemode 文件并禁用全部模块（SK 无内核监管下的 bootloop 铁闸）。属 AP 内部行为，不改接口面。

### 3.6 排除列表（【E4】删除独立文件）

原「`/data/adb/ap/exclude_list` 每行一个 uid」**删除**；per-uid 模块排除 = `/data/adb/ap/allow_list` 行内 `exclude==1`（§3.2）。**替代内容**：`auth exclude/unexclude <uid>`（§8.3）。

---

## 4. 密钥运行时形态（v1.0 继承 + E1 规范化）

方案 B（无状态传参）**终选**，非偏好而是唯一可行：内核每次提权内联验 key，无会话可缓存。内核失败行为 = 静默拒绝（普通 ENOENT，无计数无告警）。AP 侧日志脱敏见 §2.3。方案 A（常驻缓存）废止。

**密钥文件（v1.1 明确）**：`/data/adb/sproot/key`（0600 root），内容 48 字符原文、无换行；由管理器 `deploy` 写入、rc 以 `--superkey-file` 读取；普通 App 不可读（v1.0 签署栏附加要求，安全测试项）。

---

## 5. busybox 首装归属（v1.0 继承，不变）

APK 部署（打包 `libbusybox.so`，APatchApp.kt:148 模式 `cp -f ${nativeDir}/libbusybox.so /data/adb/ap/bin/busybox`）+ deploy 幂等兜底（`[ -x ... ] || cp`）。AP 本体不携带不下载 busybox。首装链：管理器 deploy → busybox → AP 就位。

---

## 6. 对 AP 二进制实现的影响清单（更新 v1.0 §6）

| v1.0 假设 | v1.1 定稿 | 实现影响 |
|---|---|---|
| 6 类 supercall 内核接口 | 仅提权保留（execve k47） | `supercall.rs` 仅剩 escalate 封装 ✅ 已实现 |
| `SP_REQUEST_UID` 由内核注入 env | **删除**（【E3】） | `apd.rs:196-205` 删 env 读取；改 socket/`--request-uid`（仅 root 上下文可信） |
| su 部署 = 固定名 `su` | 随机名 + `bin/su` 链接（【E2】） | `deploy` 新实现 |
| `allow_list` + `exclude_list` 双文件 | 单文件 `allow_list`（【E4】） | `supercall.rs:98-103` 路径统一；不实现 `exclude_list` |
| 授权执行点 = `root_shell()` 查表 | 三分来源（socket/管理器/root 上下文） | 新增 `daemon`（socket + uid-listener）、`auth` 子命令 |
| APK↔AP 不定义 | **新增 §8**（root-exec/deploy/version） | 新增子命令；`cli.rs` 扩展 |
| 部署器 `skroot-deploy` 交付 | 并入管理器 + AP（【E5】） | 无独立组件 |
| `report_kernel` 事件上报 | 本地日志 + `state.json`（N2 非目标不变） | `event.rs:26-31` ✅ 已退化 |
| KP 专属子命令（insmod/late-load/magica/soft-reboot） | **删除**（NEW-DOC-05 D9，leader 已裁 Q5） | 删模块与 Cargo 依赖；**源码可留注释，UI 入口删除** |

---

## 7. 部署职责与首启闭环（【E5】重写时序稿 §4）

| 原交付项（skroot-deploy） | v1.1 归属 | 落地方式 |
|---|---|---|
| `install_su(key)` | **AP `deploy` 子命令** | 建隐藏目录 + 随机名副本 + `bin/su` 链接（§3.4） |
| `write_key_file(key)` | **AP `deploy` 子命令** | 写 `/data/adb/sproot/key` 0600（§4） |
| `inject_rc(bootimg_dir)` | **管理器修补流程** | 写 `init.sproot.rc` + `init.rc` 尾部追加 `import`（repack 前） |
| `uninstall_all(key)` | **AP `undeploy` 子命令** | 逆序清理（链接→隐藏目录→sproot→ap→apd） |
| （新增）目录/符号链接/busybox/空授权表 | AP `deploy` | 幂等、sha256 校验、失败不留半成品（NEW-DOC-05 §6.2 九步清单） |

**rc 内容约束**（沿用时序稿 §3.4）：不内联密钥；只引用 `/data` 路径；`seclabel` 不写死。**rc 由管理器注入而非修补引擎注入**——引擎（C1）的输入只有内核 Image、不触碰 ramdisk，职责边界不变。

**首启闭环（与 NEW-DOC-05 §6.1 一致）**：修补期（无 root，不动 `/data`）→ 首启管理器 exec AP 自身完成 `deploy`（AP 自提权，无需预置 root）→ **第二次开机**起 rc 链完整生效。rc 在 AP/部署物不存在时静默失败（不阻断开机）。

---

## 8. APK ↔ AP 最小调用契约（v1.1 新增）

> v1.0 §1 明确「不含 APK↔AP（沿用 APatch shell 形态）」；新架构管理器从零重写，且无 KP 的 `SUPERCMD` 重映射，shell 形态不再自动成立。本章定死最小面，供 app-client 与 AP 双向实现。

### 8.1 调用模型

> **边界铁律（v1.1-FROZEN 补充，2026-11-08）**：**修补链 ≠ 特权链**。修补（选镜像 / 解包 / patch / 重打包 / 导出）在管理器 App 进程内完成（JNI 引擎或引擎子进程），**全程无需 root**，**不得**经 `root-exec`；特权操作（`deploy` / 模块安装 / 授权 / root 会话）才经 §8.2 `root-exec` 走 AP。两条调用路径不得混用，避免把「无需 root 的修补」污染成特权路径（也避免 root 失败阻断修补）。

1. **进程边界**：管理器 `exec` AP 二进制（`/data/adb/apd`，或首装时 APK `nativeLibraryDir` 内副本），参数经 argv，输出经 stdout/stderr（UTF-8 行文本），结果经 **exit code**。**不共享内存、不 dlopen。**
2. **身份**：管理器以**普通 App uid** exec AP；AP 启动后读 `--superkey-file` 自行提权（§3.1）。**无需 JNI、无需 libsu。**
3. **密钥文件**：管理器把密钥写入**自身私有目录**临时文件（0600，同 uid 可读）→ 传 `--superkey-file` → 调用返回后立即删除。
4. **并发**：管理器侧串行化 root 操作（单队列）；AP 不保证多进程并发安全（授权表除外，flock 保护）。

### 8.2 root-exec（管理器唯一 root 入口，Q8 裁决）

```
apd root-exec --superkey-file <PATH> [--uid <N>] [--scontext <CTX>] [--timeout <SEC>] -- <CMD> [ARGS...]
```

| 项 | 规格 |
|---|---|
| 语义 | 读取 key → `execve(k47)` 提权 → 校验 `getuid()==0` → 按 `--uid`/`--scontext`（可选）设置身份/context → `exec CMD`，**stdio 全透传**，AP 不缓冲不改写 |
| 退出码 | `CMD` 退出码原样返回；AP 自身失败见 §8.5 |
| 密钥泄露面 | `--superkey-file` 只传路径不传内容；AP 日志脱敏；`/proc/<pid>/cmdline` 不含密钥 |
| 超时 | `--timeout` 秒级；超时 AP 杀进程组并 exit 124（管理器另有外层超时兜底） |
| 授权 | 视为管理器（设备所有者），不查 allow_list（§3.2(b)）；写审计日志 |
| 失败 | 提权失败 → **exit 125，无 stdout/stderr**（与内核静默拒绝一致） |

**与 NEW-DOC-01 L3 `Escalation` 的关系（已裁决，v1.1-FROZEN）**：L3 的 execve 实现体**收敛到 AP 进程内**（`root-exec`）。Kotlin 侧不引入 JNI：只负责「写 0600 临时密钥文件 → 起 root-exec 进程 → 按 exit code 判定」，不 exec 自身、不 native fork。`root-exec` 自此为管理器**唯一** root 入口。

### 8.3 管理器使用的其他子命令（最小集）

| 子命令 | 用途 | 身份要求 | 备注 |
|---|---|---|---|
| `apd module install/uninstall/undo-uninstall/enable/disable/action/list/config` | 模块管理 | root（经 root-exec 包裹） | 行为同 APatch（NEW-DOC-05 §3.1） |
| `apd auth list/grant/revoke/exclude/unexclude [--json]` | 授权表 UI 读写 | root | 单写者 + flock；`--json` 供 UI 解析 |
| `apd deploy --source <nativeLibDir> --superkey-file <P>` | 首装部署（§7） | 自提权 | 幂等；九步清单见 NEW-DOC-05 §6.2 |
| `apd undeploy [--keep-modules]` | 卸载清理 | root | 逆序 |
| `apd version --json` | 状态/版本探测 | 任意 | 输出 `{version, deployed, key_prefix_sha256, daemon_pid?}`，**不含密钥明文** |
| `apd daemon --superkey-file <P>` | 常驻守护（socket/uid-listener/阶段②③） | init（rc） | **仅 rc 调用**，管理器不得拉起 |

### 8.4 输出约定

- 机器可读输出（`list`/`auth --json`/`version`）统一 JSON，**不得混入日志行**；日志走 stderr 或 `logcat`（tag `APatchD`）。
- 任何输出（stdout/stderr/logcat）**不得含密钥原文或前缀**。
- 管理器只用 exit code 判成败，不解析人类可读文本。

### 8.5 退出码（CLI 稳定枚举）

| 码 | 含义 | 管理器动作 |
|---|---|---|
| 0 | 成功 | 继续 |
| 1 | 一般失败（业务/IO） | 展示 stderr 摘要 + 日志落盘 |
| 2 | 用法错误（参数） | 视为 bug，展示原始 argv |
| 124 | 超时（`--timeout`） | 提示用户，保留日志 |
| 125 | 提权失败 / 未授权（静默） | 提示「密钥无效或需重新修补」，不重试 |
| 126 | 命令不可执行（权限/格式） | 提示「需重新部署」 |
| 127 | 命令不存在 | 同上 |

---

## 9. 会签矩阵（v1.1-FROZEN，全部签署完成）

> **冻结时间：2026-11-08 21:23 +08:00（Asia/Shanghai）**。下表「条款」列按 **v1.0 原文编号**（便于对照旧稿评审）；v1.1 自身编号与 v1.0 对齐（§2–§6），新增 §7/§8。

| 条款 | v1.0 | v1.1 变更 | 签署方 | 状态 |
|---|---|---|---|---|
| §2.1/§2.2 错误码/身份 | ✅ 三方 | 未变（仅补 setcon 说明） | 继承 | ✅ 无需重签 |
| §2.3 密钥方案 B、脱敏 | ✅ 三方 | +E1 规范化（48 存/47 用） | app-client | ✅ |
| §3.1 SK-ESC | ✅ 三方 | **E1 重写**（🔴 阻断） | patcher / app-client | ✅ |
| §3.2 授权表 | ✅ 三方 | **E3/E4 重写**（🔴 阻断） | patcher / app-client | ✅ |
| §3.3 授权表读 | ✅ 三方 | 未变（路径随 E4 统一 `allow_list`） | app-client | ✅ |
| §3.4 su 部署 | ✅ 三方 | **E2 重写** | patcher / app-client | ✅ |
| §3.5 安全模式 | ✅ 三方 | +boot_count 强化（AP 内部） | 继承 | ✅ 无需重签 |
| §3.6 排除列表 | ✅ 三方 | **E4 删除独立文件** | app-client | ✅ |
| §4 密钥运行时形态 | ✅ 三方 | +E1 规范化、密钥文件规格 | app-client | ✅ |
| §5 busybox 首装 | ✅ 三方 | 未变（并入 deploy 第 3 步） | 继承 | ✅ 无需重签 |
| §6 影响清单 | ✅ 三方 | 全面更新 | app-client | ✅ |
| **§7 部署职责（原时序稿 §4）** | ✅ 三方 | **E5 重写**（撤 skroot-deploy） | patcher / app-client / leader | ✅ |
| **§8 APK↔AP（新增）** | — | **新增**：root-exec 为唯一 root 入口；L3 收敛裁决 | app-client / leader | ✅ |
| socket 暴露接受 | — | 新增（§0.4 已知风险） | leader | ✅ |
| KP 子命令删除 | — | 新增（NEW-DOC-05 D9） | leader（Q5 裁决） | ✅ |

**会签栏（v1.1-FROZEN）**

| 方 | 角色 | 状态 | 签署时间 | 签署范围与说明 |
|---|---|---|---|---|
| 系统守护（sys-guard） | AP 消费方 | ✅ | 2026-11-08 21:0x +08 | 提交 v1.1-D1，E1–E5 全部证据化 |
| 内核修补（patcher） | SK 提供方 | ✅ | 2026-11-08 +08 | E1（独立复核 `patch_do_execve.cpp:70,89-102`、`skroot_patch_main.cpp:325-330`：48 字符在第 48 位失配）/ E2 / E3（do_execve 跳板只拿 filename 指针，无 envp 可用）/ E5 / §8 全部无异议；明确 task-3 **不产出**任何独立部署器 |
| 客户端（app-client） | APK 消费方 | ✅ | 2026-11-08 +08 | E1–E5 + §8 确认；同意 L3 收敛到 `root-exec`（写 0600 临时密钥 → root-exec → 仅按 exit code 判成败；125 不重试 / 124 超时 / 126·127 需重新部署 / 2 视为 bug 并展示原始 argv；串行单队列；无 `sh -c`；密钥不入 argv/日志） |
| leader | 终审 | ✅ | 2026-11-08 21:23 +08 | 批准冻结；TBD-10 关闭 |

> **冻结声明**：本契约自 **2026-11-08 21:23 +08:00** 起状态为 **v1.1-FROZEN**，作为 AP 二进制、管理器 App、质测门禁的共同验收依据。任何条款变更（含文字性修订）须开 v1.2：在 §0 增补修订明细、重新走 patcher + app-client + leader 会签，不得就地修改本文件。

---

## 10. 变更记录

| 版本 | 日期 | 变更 |
|---|---|---|
| v0.1 | 2026-11-06 | 草案（系统守护） |
| v0.2 | 2026-11-06 | 合并内核修补 4 项裁决（ESC 改 execve 判据、授权表用户态化、SETPATH 删除改部署形态、kp 别名删除、密钥方案 B 终选、SAFEMODE 文件标志） |
| **v1.0** | 2026-11-06 | 三方签署定稿（系统守护 / 内核修补 / leader 终审） |
| **v1.1-D1** | **2026-11-08** | **勘误修订**：E1 提权串 = 47 字符密钥本体（非隐藏目录路径，🔴 照旧必失败）；E2 su 随机命名；E3 废弃 SP_REQUEST_UID 改 socket SO_PEERCRED；E4 exclude_list 合入 allow_list 单文件；E5 取消 skroot-deploy、部署并入管理器+AP；新增 §7 部署职责、§8 APK↔AP 最小契约（root-exec 为唯一 root 入口）、§9 会签矩阵。**章节编号与 v1.0 对齐**（§2–§6 不动，新增仅 §7/§8），跨文档引用 CALL §3.1/§3.2/§3.4 继续有效。依据：`engine/src/patch_do_execve.cpp:76-135`、`engine/src/skroot_patch_main.cpp:312-317`、`app/apd/src/{supercall,auth_list,package,apd}.rs`、NEW-DOC-05 §4.6、NEW-DOC-01 §3.1/§3.3 |
| **v1.1-FROZEN** | **2026-11-08 21:23 +08** | **冻结**：三方会签完成（系统守护 ✅ / patcher ✅ / app-client ✅ / leader 终审 ✅），TBD-10 关闭；补充 §8.1「修补链 ≠ 特权链」边界铁律，§8.2 L3 收敛裁决落文。后续变更须开 v1.2。 |

---

*附：每条修订的「原文→修订文→证据→影响面」均见 §0；patcher/app-client 若有异议，请对 §0 逐条批注，避免在 §2–§8 全文里改散。*

---

## 附件 A：勘误 E6（errata，2026-11-10 追加）

> **性质**：勘误（errata）。**本附录为追加内容，不修改本契约正文（§1–§10）任何条文**；正文保持 **v1.1-FROZEN（2026-11-08 21:23 +08）** 原样。
> **触发**：人类需求「增加 root 授权功能页面」→ 每应用三态（`allow | deny | ask`）+ 授权日志 + 每应用元数据（`last_grant_at` / `request_count` / `sctx` / `exclude`），v1.1 §3.2 / E4 的 csv 六列无法承载。
> **关联**：`docs/NEW-DOC-05-模块与守护方案.md` §13（授权模型：三态 + 日志）与 §4.6 勘误表 E6 行；实现：`apd/src/auth_list.rs`、`apd/src/cli.rs`、`apd/src/daemon.rs`。

### A.1 变更内容（E6）

| 项 | v1.1 正文口径（§3.2 / E4） | E6 后实现口径 |
|---|---|---|
| 授权决策源 | `/data/adb/ap/allow_list`（csv：`pkg,exclude,allow,uid,to_uid,sctx`） | `/data/adb/sproot/auth_list.json`（JSON 文档 `{"version":1,"entries":[...]}`；每 uid：`state ∈ allow\|deny\|ask` + `package/label/app_icon_path/last_grant_at/last_deny_at/request_count/to_uid/sctx/exclude`） |
| 表外 uid | 拒绝 | **`ask`**（默认态）。Phase 1 中 `ask` 显式拒绝（`decision=ask_pending`），Phase 2 接交互式弹窗 |
| 旧 csv | 唯一决策源 | **只读**：升级后首次加载**一次性迁移**（`allow==1 && exclude==0` → `allow`；其它已持久化行 → `deny`；`exclude/to_uid/sctx` 原样保留），此后不再写入，仅留作审计/回滚留档。**升级不丢授权** |
| 写一致性 | 单写者 + tmp/rename | 不变：`flock` 单写者、tmp+rename 原子写、torn-read 熔断、uid 0/2000 保护逐条保留 |
| 审计 | —（未定义） | 新增 `/data/adb/sproot/auth_log.jsonl`（0600、JSONL、上限 256 KiB 时重写为最新 500 条）。每行仅 `cmd_name`（可执行名）+ `cmd_args`（参数个数）+ `cmd_sha256`（SHA-256 前 16 hex），**不含完整命令、参数或任何密钥** |

### A.2 调用契约兼容性（正文 §8 不变）

- App ↔ apd 的**调用**契约未变：仍是 `root-exec --uid <n> --scontext <ctx> -- <cmd...>`；`auth list/set/log/revoke --json` 属新增**管理**命令（root-only，经 `root-exec` 包裹调用）。
- daemon socket 响应仅新增**加法**字段 `"decision": "allow" | "deny" | "ask_pending"`（§8 未定义该响应结构，故不构成破坏性变更；老客户端忽略该字段即可）。
- 密钥/提权链（§4、E1）与 rc 自启链（§7）不受影响。

### A.3 行为后果（必须让使用者知道）

**默认态为 `ask`，而 Phase 1 中 `ask` 等同拒绝** → **新装应用第一次请求 root 会被拒绝**（`decision=ask_pending`），用户必须到授权页把该应用手动设为 `allow`。这是 Phase 1 的既定取舍：**安全（绝不隐式放行）但需要一次手动放行**。

### A.4 会签需求

| 方 | 状态 |
|---|---|
| 系统守护（apd 实现方） | ✅ 2026-11-10 提交 |
| 客户端（app-client / ui-client） | ⏳ 待确认（按 NEW-DOC-05 §13 的 JSON 契约实现授权页） |
| leader | ✅ 2026-11-10 裁决：以勘误形式追加，不改冻结正文 |

> 若后续开 v1.2：应把 E6 合并进 §0 修订明细，并按 §9 重新会签；届时删除本附件。
