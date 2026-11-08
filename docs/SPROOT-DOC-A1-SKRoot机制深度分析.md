# SK Root 实现机制深度分析（SPRoot 第一阶段文档 A）

- 文档编号：SPROOT-DOC-A1
- 作者：内核修补（内核/修补方向）
- 状态：正式稿 v1.0
- 面向读者：总设计、客户端（APK 改造）、系统守护（AP 二进制）、质测。**读完本文即可设计，无需再读上游源码。**

---

## 0. 上游基线（已锁定）

| 项 | 值 |
|---|---|
| 上游仓库 | https://github.com/abcz316/SKRoot-linuxKernelRoot |
| 锁定分支 / commit | `master` @ `943382cd1158143179c6fc3bc9d671098b10c2ca`（2026-11-04，"Update README.md"） |
| 作者最后推送 | 2026-11-05（仓库级），master 分支即最新 |
| 分析范围 | `Lite_version/`（Pro 目录仅 PDF/SDK 骨架，闭源部分不在分析范围） |
| 工作区副本 | `SPRoot/upstream/SKRoot-linuxKernelRoot/` |

**获取途径说明（按人类要求记录）**：本工作区克隆经第三方镜像 `gh-proxy.com` 完成（当时 WSL 直连 github.com 被环境阻断）。此后已用 **Windows 侧原生 git 对官方地址 `ls-remote` 复核**：远程 HEAD = `943382cd...`，与本地克隆逐位一致，基线可信。SPRoot 所有引用统一以该 commit 为准。

上游关键目录结构：

```
Lite_version/
├── patch_kernel_root(2026-9-14).exe   # 官方预编译修补器（Windows）
├── SKRoot_Lite(2026-9-14).apk         # 官方演示 APK
└── src/
    ├── patch_kernel_root/             # ★ 内核修补引擎（C++/asmjit，本文重点）
    │   ├── patch_kernel_root.cpp      # 主流程
    │   ├── patch_base.{cpp,h}         # 跳板/汇编基础设施
    │   ├── patch_do_execve.*          # ★ do_execve hook（提权核心）
    │   ├── patch_filldir64.*          # 目录隐藏（64 位 getdents）
    │   ├── patch_compat_filldir.*     # 目录隐藏（32 位 compat）
    │   ├── patch_avc_denied.*         # SELinux denial 静默放行
    │   ├── patch_audit_log_start.*    # 审计日志静默
    │   ├── patch_current_avc_check.*  # 「密钥进程」判定辅助
    │   ├── analyze/                   # kallsyms 解析、cred/init_cred 搜索
    │   └── 3rdparty/                  # asmjit、capstone、aarch64 辅助
    ├── PermissionManager/             # 官方 Root 管理 APK（Gradle 工程）
    └── testRoot/                      # 用户态 RootKit kit（NDK，含闭源 .a）
        └── kernel_root_kit/           # ★ 用户态 API（get_root/install_su…）
```

---

## 1. 修补对象与总体结论

**SK 修补的对象不是 boot.img，而是从 boot.img 中解包出来的 Linux 内核 Image（Image.gz 解压后的裸 arm64 内核文件）。**

- 上游工具明确拒绝 `.img` 后缀输入（`check_file_path`），README 指引：用 `magiskboot unpack boot.img` 取出 kernel → patch → `magiskboot repack`。
- 补丁是**纯字节就地改写**：不新增 section、不追加数据、不改变内核内存布局，内核加载地址/大小不变。
- 没有任何「驱动模块」「kpimg」之类的运行时载荷被嵌入——**修补完成后内核里只有原厂代码 + 若十字节的内联跳板与 shellcode**。

一句话结论：**SKRoot = 「把一段自包含的 ARM64 shellcode 织进原厂内核五个函数的执行流」的静态修补术 + 一个以 48 字符密钥为唯一凭证的内核态提权机制。**

## 2. 修补引擎主流程（patch_kernel_root.cpp）

```
输入: 内核 Image 文件路径（非 .img）
 1. 读入整个文件到内存 file_buf
 2. SymbolAnalyze::analyze_kernel_symbol()
    └─ 解析 kallsyms（6 套版本适配器: 通用/4.6.0/6.1.0/6.1.42/6.1.60/6.4.0/6.12.0）
    └─ 得到 KernelSymbolOffset（约 25 个符号的偏移+大小，见 §3）
 3. 结构体偏移动态推导（不依赖源码/配置）:
    - cred_offset          : 反汇编 sys_getuid，追踪 current->cred 装载偏移（取 >0x400 者）
    - cred_uid_offset      : 反汇编 sys_getuid 中 imm 偏移（4 或 8，按内核版本 <6.6.8 分界）
    - seccomp_offset       : 反汇编 prctl_get_seccomp，同法推 task->seccomp 偏移
    - huawei_kti_addr      : 反汇编 kti_randomize_init 的 ADRP 目标（华为 KTI 缓解）
 4. InitCredSearcher: 用字节模式搜索 init_cred（见 §5）
 5. cfi_bypass / huawei_bypass（见 §6）
 6. patch_kernel_handler: 依据内核版本选 hook 空间并织入五组 hook（见 §7）
 7. 交互式生成/输入 48 位 Root 密钥 → 写入 shellcode 头部 48 字节（见 §4）
 8. 把全部补丁逐条写回文件（write_all_patch），打印密钥
输出: 修改后的内核 Image + 打印在控制台的 48 位密钥
```

注意上游是**交互式 CLI**（cin 询问是否随机生成密钥、是否写盘）。SPRoot 修补引擎需要改为**非交互式**（参数化），这是 I/O 契约改造点，见文档末尾 §9 对 SPRoot 的直接推论。

## 3. 引擎依赖的内核符号全集

| 符号 | 用途 | 缺失后果 |
|---|---|---|
| `die` / `__drm_puts_coredump` / `__drm_printfn_coredump` | ≥6.1 内核的 **shellcode 空间**（三个不死于正常路径的死代码函数） | 无空间 → patch 失败 |
| `do_execve` / `do_execveat` / `do_execveat_common` / `do_execve_common` / `__do_execve_file` | 提权 hook 挂载点（择最大者） | 无法提权 |
| `filldir64` / `compat_filldir` | 目录项隐藏挂载点 | 隐藏目录可见 |
| `avc_denied` | SELinux denial 路径 | 隐蔽性退化 |
| `audit_log_start` | 内核审计日志入口 | 审计残留 |
| `sys_getuid` | cred/uid 偏移推导锚点 | 偏移推导失败 |
| `prctl_get_seccomp` | seccomp 偏移推导锚点 | 偏移推导失败 |
| `__cfi_check`(可空) / `__cfi_check_fail` / `__cfi_slowpath(_diag)` / `__ubsan_handle_cfi_check_fail(_abort)` / `report_cfi_failure` | CFI 全家桶 RET 化；`__cfi_check` 兼作 <6.1 的 shellcode 空间 | 有 CFI 内核会 panic |
| `hkip_check_uid_root` / `hkip_check_gid_root` / `hkip_check_xid_root` / `kti_randomize_init` | 华为 HKIP 绕过（可空，非华为设备没有） | 华为设备被拦 |

kallsyms 解析要求内核 `CONFIG_KALLSYMS=y`（原厂内核普遍满足；这与 KernelPatch 的要求相同，但 SK 只读 kallsyms 做静态分析，**运行期不用它**）。

## 4. 48 位 Root 密钥机制（生成/存储/验证/提权全链路）

### 4.1 生成

- `ROOT_KEY_LEN = 48`；`generate_random_str(48)` 产出 48 字符随机串（大小写字母+数字）；也接受用户手工输入 48 字符。
- 修补器落盘时做了一次**去尾**：`write_key = str_root_key` 去掉最后一个字符 → 实际写入内核的是 **47 字符 + `'\0'`（共 48 字节）**。用户态使用时同样去尾（`internel_key.erase(size()-1)`），两端一致。**SPRoot 重新实现时建议规范化为「密钥恒 47+NUL」，消除这个易错点**（对外仍宣传 48 位密钥）。

### 4.2 存储

- 48 字节密钥**直接内嵌在补丁 shellcode 的开头**（`root_key_start` 处 `embed 48B`），位于内核 Image 内。
- **前 16 字符**（`FOLDER_HEAD_ROOT_KEY_LEN=16`）同时充当隐藏目录名：`/data/<key[0..15]>/`，目录内放 `su` 等。内核 filldir hook 与用户态用同一前缀做比对/寻址——**密钥即路径，路径即凭证**，无任何配置文件。
- 密钥不出现在 boot.img 头部、无 superkey 哈希、无配置块（对比 KP：superkey 以 SHA256 存在 kpimg setup 区）。

### 4.3 验证与提权（一次 execve 完成）

用户态唯一提权入口（`kernel_root::get_root`）：

```c
syscall(__NR_execve, internel_key /* 47字符密钥 */, NULL, NULL);
if (getuid() != 0) return ERR_NO_ROOT;
// 成功后: setsid、回到 init mount ns、cgroup 迁根、
// selinux_procattr setcon("u:r:shell:s0")
```

内核侧（patch_do_execve 织入 do_execve 系入口，在原有第一条指令前跳板）：

1. `do_execve(filename=..., ...)` 进入 → 跳板先拿到控制权，shellcode 头部就是 48 字节密钥本体。
2. 若 `filename` 是错误码（≥-4095）直接放行。
3. 逐字节比对 `filename 字符串` 与内嵌密钥（到 NUL 为止）。不匹配 → 执行被保存的原第一条指令 → 跳回 `do_execve+4`，正常执行。**普通进程的 execve 只多付出一次 48 字节 strcmp + 2 条跳转。**
4. 匹配 → 就地把 `current` 的 cred 改写为 init_cred 同款：
   - `atomic_long_t usage` 原值保留后整块清零重写（4~5 个 64 位清零覆盖 uid/gid/suid/sgid/euid/egid/fsuid/fsgid 全 0）；
   - `securebits = 0`；
   - capabilities 五元组（inheritable/permitted/effective/bset/ambient+`cap_bset` 后位）全部写满 `cap_ability_max`（按内核版本取 `0x1FFFFFFFFF` 等五种掩码之一，`cap_cnt` 4~5 个 64 位字）。
5. 清 `thread_info.flags` 的 `TIF_SECCOMP`（三种取 current 方式自适应：`CONFIG_THREAD_INFO_IN_TASK` / `SP_EL0` 寄存器 / 栈底掩码；华为设备走 KTI 换算），`task->seccomp = 0`。
6. 跳回 `do_execve+4` 正常走 execve → 密钥不是合法路径，execve 返回 `ENOENT` → 用户态发现 `getuid()==0`，提权完成。

**全程无进程创建、无文件读写、无系统调用表修改、无内核数据段常驻结构**——凭证判定只存在于 do_execve 入口的十几条指令里。

### 4.4 su 部署与注入（用户态 kit，密钥持有人可调用）

- `install_su(key)` → 建隐藏目录 `/data/<key16>/`（SELinux label 伪装成 system_file）→ 生成随机名 su 可执行文件（`lib`+随机字母+`.so` 风格命名）→ 返回绝对路径。
- `find_all_cmdline_process / kill_process / wait_and_find_cmdline_process / inject_process_env64_PATH_wrapper`：向 64 位目标进程注入 `PATH=su目录` 环境变量（ptrace 实现）；32 位 App 明确不支持（`ERR_EXIST_32BIT`）。
- `parasite_precheck_app / parasite_app`：寄生到目标 APP 已加载的 JNI 库。
- 完整错误码体系 `KRootErr`（OK=0，ERR_PARAM=-10000，其余自动递减；带 `to_string`/`is_ok` 辅助）——SPRoot 管理器可直接复用该枚举语义。

## 5. init_cred 的定位方式（无需源码的秘密）

引擎**不读内核配置、不依赖调试符号**，用「模式字节」在内核数据区搜索 init_cred：

- 模式 = `atomic_usage(=4,4/8字节) + uid..fsgid 8×4B 全 0 + securebits(=0) + cap 全满掩码`；
- 5 种 cap 掩码候选（`0x1FFFFFFFFF`/`0xFFFFFFFFFF`/`0x3FFFFFFFFF`/`0x7FFFFFFFFF`/`0x1FFFFFFFFFF`）× usage/securebits 宽度组合，4 字节对齐扫描，命中即得 `init_cred` 偏移、`atomic_usage_size`、`securebits_size`、`cap_cnt`、`cap_ability_max`。

这套结构模型（`cred_model` / `InitCredResult`）是 SPRoot 移植时必须原样保留的部分。

## 6. 稳定性三件套（可开机性保障）

1. **CFI/金丝雀绕过**：`__cfi_check_fail`、`__cfi_slowpath(_diag)`、`__ubsan_handle_cfi_check_fail(_abort)` 全部 patch 成 `RET`（`report_cfi_failure` patch 成 `MOV w0,#1; RET`）；shellcode 内调用内核函数时用 `CanaryBypassGuard` 处理栈金丝雀。CFI 内核（如 5.x+ CONFIG_CFI_CLANG）不绕过必 panic。
2. **PAC/BTI 感知**：改写任何函数头时检查首条指令是否 `paciasp/pacibsp/paciaz/pacibz/bti`——是则跳板序言补 `paciasp`（`DF2403D5`），RET 处补对应 `autiasp`，函数体 RET 前保留签权；`skip_pac_bti_at_func_start` 保证跳板落在真实第一条指令。
3. **跳板范式**：所有 hook 都是「入口改 B 跳板 → 跳板内先做判断，不命中则执行被保存的原指令 → B 回原函数+4」的内联模式；shellcode 全部用 asmjit 现场汇编，位置无关（ADR 相对寻址），目标空间不足（`shellcode_size > region.size`）即整体失败**且不写盘**——半成品内核不可能被刷出。

另：`patch_ret_cmd` 系列对「函数首条指令是 PAC/BTI」的差异化编码、华为 `hkip_check_*_root` 三函数 RET 0、`is_huawei()` 走 KTI 地址换算取 current，都是国产设备兼容性的关键，SPRoot 需原样继承。

## 7. 五组 hook 的落位布局

**内核 < 6.1**：shellcode 空间 = `__cfi_check`（若够大）否则固定空隙 `0x200~0x300`，顺序紧密排布：
```
[48B 密钥][do_execve trampoline+提权体]
[filldir64 guide(adr x11=密钥)][filldir64 core]
[compat_filldir guide][compat core]
[current_avc_check 辅助函数]
[avc_denied 体][audit_log_start 体]
```
**内核 ≥ 6.1**：空间 = `die`（主空间）+ `__drm_puts_coredump` + `__drm_printfn_coredump`（辅助），逻辑同上按三函数分片。

每组 hook 的「guide」仅 4 字节（一条 ADR 把密钥地址装进 x11），core 才是比对/隐藏逻辑——guide/core 分离是为了在多空间碎片里拼装。

## 8. SK Root 特点清单（SPRoot 必须保留项）

| # | 特性 | 必须保留 | 说明 |
|---|---|---|---|
| 1 | **SELinux 零触碰** | ★★★ | 不改 policy、不 setenforce 0、不加载 sepolicy。隐蔽来自 `avc_denied`/`audit_log_start` 对「密钥进程」静默放行（判定条件：euid==0 且 securebits==0 且 caps 全满）——普通进程的 denial 行为分毫未动 |
| 2 | **无挂载** | ★★★ | 无 bind mount、无 magic mount、无 overlay，`/proc/mounts` 与原厂一致 |
| 3 | **纯字节 patch、零新增载荷** | ★★★ | 无 kpimg/模块/段追加；内核大小、入口、布局不变；关闭与原厂内核逐位 diff 仅十几个指令槽 |
| 4 | **密钥即唯一凭证** | ★★★ | 48 位（实际 47+NUL）密钥；无 daemon、无 socket、无设备节点、无 sysfs 接口 |
| 5 | **运行期零 kallsyms 依赖** | ★★ | kallsyms 只在修补时静态解析；修补后内核运行行为与原厂完全一致（除 hook 点） |
| 6 | **密钥前缀即隐藏目录** | ★★ | `/data/<16字符>/` 由 filldir 双路（64/32 位 getdents）隐藏；目录 label 伪装 system_file |
| 7 | **seccomp 感知清除** | ★★ | 提权同时清 TIF_SECCOMP 与 task->seccomp，避免后续 execve 被过滤器拦截 |
| 8 | **华为/CFI/PAC 兼容层** | ★★ | HKIP bypass、CFI RET 化、PAC/BTI 感知、金丝雀绕过——缺一则相当一部分真机不可用 |
| 9 | **失败不落盘** | ★★ | 任何步骤失败即中止且不写文件，杜绝刷入半成品内核变砖 |
| 10 | **用户态 kit API 面** | ★ | get_root/run_root_cmd/root_exec_process/install_su/uninstall_su/进程查找/64 位注入/寄生；KRootErr 错误码体系 |

## 9. 对 SPRoot 修补引擎的直接推论（I/O 契约输入）

1. 引擎输入必须是**解包后的内核 Image**（或由引擎内部先调 magiskboot/kptools 的 bootimg 组件解包——需在契约中二选一定死）。
2. 上游是交互式 CLI（cin 两次询问），SPRoot 必须参数化：`--key <48字符>` / `--gen-key`、`--in kernel`、`--out kernel`、`--json`（进度/结果机读输出，供 APK 调用）。
3. 密钥去尾行为（48→47+NUL）建议在 SPRoot 中规范化；APK 与引擎对「48 位密钥」的字符串约定必须一致。
4. 引擎必须输出：密钥、每 hook 点落位地址、总补丁条数、各分析步骤成败——供 `--json` 与日志双通道。
5. 失败即中止不写盘的原子性必须保留，且错误码需映射到可读原因（kallsyms 解析失败 / cred 偏移推导失败 / 空间不足 / 不支持的内核版本…）。

> 独立 I/O 契约文档（SPROOT-DOC-C1）将按上述推论单独成文，供客户端与质测引用。
