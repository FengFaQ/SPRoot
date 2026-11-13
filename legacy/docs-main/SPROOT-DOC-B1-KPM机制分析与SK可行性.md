# KPM 安装/加载机制分析与 SK 可行性结论（SPRoot 第一阶段文档 B）

- 文档编号：SPROOT-DOC-B1
- 作者：内核修补（内核/修补方向）
- 状态：正式稿 v1.0
- 范围限定（按人类指令）：**只分析 KPM 的安装（Load 运行时加载）与加载路径，不讨论嵌入方式。**
- 面向读者：总设计、系统守护（AP 二进制改造）、客户端（APM/KPM 界面）。读完即可做设计决策，无需再读上游源码。

基线：
| 仓库 | 锁定 commit |
|---|---|
| bmax121/KernelPatch | `d4c88d6672687456b299e4b5153c24dc979d789b`（2026-10-29） |
| bmax121/APatch | `e8e4651c16fb5075882a25813c8732285fdb3505`（2026-10-13） |

---

## 1. KPM 是什么

KPM（KernelPatch Module）是**裸机 ARM64 ELF 可执行文件**（非 Linux .ko），由 KernelPatch（KP）运行时在内核态加载执行，用于动态扩展内核行为。上游官方文档位置：`KernelPatch/doc/zh-CN/module.md`。每个 KPM 必须声明元信息宏（`KPM_NAME/KPM_VERSION/KPM_LICENSE/KPM_AUTHOR/KPM_DESCRIPTION`）与生命周期回调（`KPM_INIT/KPM_CTL0/KPM_CTL1/KPM_EXIT`）。

关键属性：
- **加载器是 KP 自带的 ELF 加载器**（`kernel/patch/module/module.c`），不复用 Linux 内核的 module loader。
- **代码内存来自 KP 启动时预留的 ROX 内存池**（`kp_rox_mem`，tlsf 分配器，见 `kernel/include/kpmalloc.h` 的 `kp_malloc_exec`）。
- **KPM 的未定义符号只解析到「KP 运行时导出表」**（`kernel/base/symbol.c: symbol_lookup_name` 遍历 KP 修补时建立的哈希符号表），共约 **183 个 `KP_EXPORT_SYMBOL`**：hook 框架（`hook/hook_wrap/fp_hook_*`）、syscall hook（`hook_syscalln` 系列）、`kallsyms_lookup_name`、`printk`、字符串/内存函数、`compat_copy_to_user`、kstorage、task_ext、`commit_su/su_add_allow_uid` 等。

## 2. 安装路径（用户态 → 内核）

### 2.1 运行时加载（Load）

唯一内核入口是 supercall `sc_kpm_load(superkey, path, args, reserved)`（`kernel/patch/common/supercall.c: call_kpm_load`）：

```
用户态路径 A（命令行）: kpatch supercall "kpm load /path/x.kpm [args]"
    └─ kernel/patch/common/supercmd.c: load_module_path(path, args, 0)
用户态路径 B（APK）:   Natives.loadKernelPatchModule()
    └─ APatch/app/src/main/cpp/apjni.cpp:129: sc_kpm_load(super_key, module_path, args, nullptr)
内核态执行:            load_module_path → 读文件 → load_module_ex()
    ├─ elf_header_check（EM_AARCH64）
    ├─ setup_load_info / get_modinfo（name/version/license…）
    ├─ layout_sections → move_module: mod->start = kp_malloc_exec(size)（ROX 池）
    ├─ simplify_symbols: SHN_UNDEF → symbol_lookup_name()（只查 KP 导出表；
    │   查不到即失败："unknown symbol: %s" → -ENOENT）
    ├─ apply_relocations（REL/RELA 重定位）
    └─ 调用 KPM_INIT(args, event="load", NULL)，返回 0 即加载成功
```

鉴权：所有 supercall（含 kpm_load）都要求**正确的 superkey**（KP 的 48~64 位超级密钥，修补时以 SHA256 哈希形式写入 kpimg setup 区；`kernel/patch/common/supercall.c:425 has_preset_superkey/auth_superkey`）。**没有 superkey 就没有任何 KPM 安装入口。**

加载后管理：`sc_kpm_control`（ctl0）/`sc_kpm_unload`/`sc_kpm_nums`/`sc_kpm_list`/`sc_kpm_info`，对应 `kpatch` 子命令 `kpm ctl0/unload/num/list/info` 与 APK 的 `Natives.*` JNI 封装。

### 2.2 持久化安装（开机自动加载）

1. **安装动作**（APK 侧，`KPM.kt`）：选中 `.kpm` → `kptools -l -M <file>` 解析元信息校验 → 复制到 `/data/adb/ap/kpm/<id>/<id>.kpm`（`APApplication.KPMS_DIR = /data/adb/ap/kpm/`）。**安装后不立即加载，提示重启生效**（UI 注释原文："Installed KPMs are loaded by the boot-time loader. Do not load them in the current session"）。
2. **开机加载**（内核侧，`kernel/patch/android/userd.c: load_ap_kpm_modules()`）：在 `post-fs-data` **before** 事件由 `kernel/patch/common/user_event.c:30` 触发：
   - 遍历 `/data/adb/ap/kpm/`（内核直接 filp_open + iterate_dir，自带临时提权 `set_priv_sel_allow`）；
   - `<id>/disable` 标记文件存在 → 跳过；安全模式（android_is_safe_mode）→ 全部跳过；
   - 逐个 `load_module_path_event(path, 0, EXTRA_EVENT_POST_FS_DATA, 0)`（KPM 收到 event="post-fs-data"）。
3. 卸载持久化 KPM（APK）：先 `sc_kpm_unload` 卸载运行实例，再 `rm -rf /data/adb/ap/kpm/<id>`。

### 2.3 旁路机制（与本任务相关性低，仅记录）

- APatch `apd late-load`：jailbreak 模式在**原厂内核**上 late-load `kernelpatch.ko`（LKM，`insmod.rs` 绕过 vermagic/CRC 校验）——这是 LKM 路线，不是 KPM，但它说明 KP 运行时可以不经 boot 修补进入内核。
- 嵌入式 KPM（kptools `-M` embed extra）按人类指令**不在本文范围**。

## 3. KPM 运行的全部前置条件（判定依据）

| # | 前置条件 | 提供者 | 证据位置 |
|---|---|---|---|
| P1 | KP 运行时已在内核中（引导早期接管） | KP 修补流程（kpimg 静态嵌入 + 内核入口改写） | tools/patch.c、kpimg.lds |
| P2 | KP 导出符号表（183 符号：hook/hotpatch/printk/kallsyms_lookup_name/…） | KP 运行时启动时构建 | kernel/base/symbol.c |
| P3 | ROX 可执行内存池（kp_rox_mem） | KP 运行时启动时预留 | kernel/include/kpmalloc.h |
| P4 | ELF 加载器/重定位器（KP 自带，非 Linux module loader） | KP 运行时 | kernel/patch/module/module.c |
| P5 | supercall 分发与 superkey 鉴权 | KP 运行时 | kernel/patch/common/supercall.c |
| P6 | 事件总线（post-fs-data 等，驱动持久化 KPM 自动加载） | KP 运行时 | kernel/patch/common/user_event.c |
| P7 | 内核 `CONFIG_KALLSYMS=y`（KP 修补时静态解析，运行期 `kallsyms_lookup_name` 由 KP 自己实现/代理） | 原厂内核 | KP README |

## 4. SK 修补内核上能否运行 KPM？

**结论：原样不可行。KPM 硬依赖 KP 运行时（P1~P6 全部缺失），SK 修补内核一个都没有。**

论证：
1. SK 的修补产物是**纯字节 patch 的原厂内核**：没有 kpimg、没有引导接管、没有 supercall 分发、没有符号导出表、没有预留内存池（对照 SKRoot 文档 A1 §8 特性 3——这正是 SK 隐蔽性的来源）。
2. KPM 的 ELF 加载第一步 `simplify_symbols` 就要查 KP 导出表；即使把 ELF 灌进内存，`hook()`/`printk()` 等一切未定义符号都无从解析。
3. KPM 无自举能力：它不是 .ko，不能走 `init_module(2)`；它依赖的 `hook` 内联跳板框架（字节级 trampoline 修补）本身是 KP 运行时提供的 RW/ROX 内存与指令修复引擎。
4. 鉴权模型不同：KPM 入口收口于 superkey 鉴权的 supercall；SK 的凭证模型是 48 位密钥 + execve 路径比对（文档 A1 §4.3），二者没有互译层。

## 5. 三条适配路线与取舍（供第二阶段总设计裁决）

| 路线 | 内容 | 工作量 | 对 SK 隐蔽性的影响 | 结论建议 |
|---|---|---|---|---|
| R1 直接兼容 | 在 SK 内核上跑现成 KPM | 不可行（§4） | — | ❌ 排除 |
| R2 移植 KP 运行时为「SK-KPM 宿主」 | 把 KP 的符号表/ROX 池/ELF 加载器/hook 框架做成 SK 修补时可选注入的组件 | 极大（≈把 KP 重新实现一遍）且相互矛盾：KP 是常驻运行时（引导接管+常驻数据结构），SK 卖点是零常驻、零载荷 | 严重破坏（特性 1/3/5 全部失守），且与人类已定方向（SK 驱动 Su/Root、AP 提供模块与管理器）冲突 | ❌ 不建议，除非产品定位改变 |
| R3 SK 原生化重写 | 不跑 KPM；把目标 KPM 的功能用 SK 方式实现——修补期静态织入 shellcode（同文档 A1 的 hook 范式），管理面经 AP 管理器/模块体系 | 每个 KPM 一份移植工作；需要为「模块」定义 SK 化形态（如：修补期模块清单、或 AP 模块携带预编译 SK shellcode 片段由引擎统一织入） | 无损（仍然零常驻、零运行时） | ✅ 与 Q2 决策吻合：KPM 能力保留为架构接口位，后期讨论 |

**按人类既定决策（Q2：KPM 作为能力保留但先搁置）**：SPRoot 架构文档中应预留「SK 模块接口位」，其形态即 R3 的落点——具体提案（SK 模块清单格式、织入时机、与 APM 的对应关系）留给第二阶段总体设计，本文不展开。

### 5.1 R3 评估触发条件（防止「永远搁置」）

搁置 ≠ 关闭。满足以下任一条件时，**必须**重开 R3 评估（以 §6 八项清单为验收基线，产出新的可行性结论文档）：

- **T1 功能缺口**：第二阶段总体设计完成后，确认存在 APM 用户态模块无法覆盖、必须内核态实现的功能诉求（任一具体需求被提出即算）。
- **T2 上游信号**：SKRoot 上游（abcz316）公开 Pro 版内核模块 SDK 的内核态部分或官方扩展机制——届时优先评估「直接对接上游」而非 R3 自研。
- **T3 生态需求**：有明确用户/生态伙伴要求在 SPRoot 上运行既有 KPM 的等效能力（列出具体 KPM 名单）。
- **T4 定期复核**：第三阶段（迁移实现落地）结束时的集成评审节点，无论是否有前述触发，强制复核一次本节结论的有效性。

## 6. 适配项清单（若未来启用 R3，需逐项解决）

1. **hook 框架**：KPM 依赖的 `hook/hook_wrap`（inline hook + 修复指令引擎）→ 需要等价的「修补期静态 hook 描述格式」（目标符号 + 跳板模板），或最小运行期 trampoline 分配器（但这会引入常驻内存，需权衡）。
2. **符号解析**：KPM 用 `kallsyms_lookup_name` 动态找符号 → SK 修补期已有 kallsyms 静态解析器（文档 A1 §3），可改为修补期绑定 + 版本兼容表。
3. **内核 API 面**：KPM 大量调用 `printk/strlen/copy_to_user` 等 kfunc → SK shellcode 无法携带这些；需在织入期解析为具体地址（同上，修补期完成）。
4. **内存分配**：`kp_malloc/kp_malloc_exec` → SK 无池；静态织入模型下不需要分配器，但需要为每模块预留「空隙空间」（复用 SK 的死代码空间策略，文档 A1 §7）。
5. **加载/卸载语义**：KPM 可动态 load/unload → SK 静态模型没有卸载；产品语义改为「重新修补 boot 即更新模块」（对用户透明度下降，管理器需提示）。
6. **鉴权与管理面**：sc_kpm_control 的 ctl0 交互 → 需设计 SK 版控制通道（注意：任何常驻通道都会破坏隐蔽性，建议用「密钥路径 + 文件系统」这类无痕通道）。
7. **事件模型**：`EXTRA_EVENT_POST_FS_DATA` 等启动事件回调 → SK shellcode 常驻点只有五个函数，事件回调需映射到 hook 点内的时机判定。
8. **KPM 生态兼容**：现有 KPM 源码需按新接口改写才能发布为「SK 模块」——本质是新的模块生态，不是兼容层。

## 7. 一页纸结论

- KPM 的安装（Load）= superkey 鉴权下的 `sc_kpm_load` → KP 自带 ELF 加载器 → KP ROX 池执行 → 符号只解析到 KP 导出表（183 符号）；持久化安装 = `/data/adb/ap/kpm/<id>/<id>.kpm` + 重启后 KP 在 post-fs-data before 事件自动加载（disable 标记可禁用，安全模式全跳过）。
- **KPM 100% 依赖 KernelPatch 运行时**（引导接管、符号表、内存池、加载器、鉴权、事件总线六位一体）。
- **在 SK 修补内核上原样运行 KPM：不可行。** 硬塞 KP 运行时进去会摧毁 SK 的全部隐蔽性设计。
- **建议结论（与 Q2 决策一致）：KPM 能力以「SK 原生模块接口位」形式保留在架构中，先搁置；若后期启用，走 SK 原生化重写路线（R3），适配项见 §6，重开评估的触发条件见 §5.1（T1 功能缺口 / T2 上游信号 / T3 生态需求 / T4 三阶段末强制复核）。**
