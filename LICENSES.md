# 许可与来源标注（LICENSES / ATTRIBUTION）

> 本文件是本仓库**唯一的许可与来源权威说明**。任何发布物（APK、引擎二进制、守护二进制、源码归档）
> 都必须随附本文件。最后更新：2026-11-08。
>
> 本文件 SPDX：`AGPL-3.0-only`。第三方组件清单另见仓库根 `NOTICE`；二者互为补充，不得只发布其一。

---

## 0. 一句话结论

| 组成 | 许可状态 | 覆盖范围 |
|---|---|---|
| **本项目自有代码**（App / 守护 apd / 自研 bootimg / 流水线 / CI / 文档） | **AGPL-3.0**（见仓库根 `LICENSE`） | 仅自有部分 |
| **源自 SKRoot 上游的内容**（`engine/src` 下 15 个 patch 文件 + 1 个改造文件） | **原作者保留所有权利（All Rights Reserved，无 SPDX 声明）** | 见 §2 逐文件清单 |
| **第三方开源组件** | 各自原许可（见 §3） | 各自范围 |
| **magiskboot（Magisk）** | **GPL-3.0** | 独立可执行组件，见 §4 |

**关键原则（人类明确指示）**：
1. 来自 SKRoot 上游的内容，**不改动、不替换、不重新授权其许可证**；
2. 在文档中**逐项标注**哪些内容来自上游；
3. 明确写明 **SKRoot 原作者保留其所有权利**；
4. 本项目的 AGPL-3.0 **不覆盖**上游内容——上游内容的授权条件以原作者为准。

---

## 1. 上游来源

| 项 | 值 |
|---|---|
| 上游项目 | SKRoot - Linux 内核级完美隐藏 Root 方案（`abcz316/SKRoot-linuxKernelRoot`） |
| 本仓库使用的上游目录 | `upstream/SKRoot-linuxKernelRoot/Lite_version/src/patch_kernel_root/`（仅作对照基线，**不入发布物**） |
| 获取时间 | 2026-11-06 |
| **上游是否提供 LICENSE** | **否**。经检索：顶层与 `Lite_version/src/patch_kernel_root/` 下**不存在** `LICENSE` / `COPYING` / `NOTICE` 文件，源码文件内亦**无版权头或 SPDX 标识**。 |
| **据此认定的默认法律状态** | **保留所有权利（All Rights Reserved）**。未授予任何明示许可，不得复制、修改、分发。 |
| 本项目使用依据 | 已取得**原作者授权**（授权记录由项目所有者持有，出于保密不入仓库）。**该授权仅约束"使用"，不改变上游代码本身的许可状态。** |

> ⚠️ **重要**：仓库根的 `LICENSE`（AGPL-3.0）**不适用于** §2 所列上游内容。
> 若上游授权范围不足以支撑对外分发，则发布物必须**移除** §2 所列文件或改为不含该部分的构建
> （见 §5 发布前检查清单）。

---

## 2. 来自 SKRoot 上游的内容（逐文件清单）

### 2.1 与上游**逐字节完全相同**的文件（13 个）

以下文件与上游 `Lite_version/src/patch_kernel_root/` 同名文件 **`diff` 结果完全一致（0 差异）**，
可视为上游原始文件的副本。**许可状态：原作者保留所有权利。**

| # | 文件 | 大小（字节） | sha256（固定指纹） | 与上游比对 |
|---|---|---|---|---|
| 1 | `engine/src/patch_base.h` | 1,706 | `85ed4a29dc97829cf42312fa3286c71d9fb2afc46e4ec54b360b1ea343f7b4cb` | 完全相同 |
| 2 | `engine/src/patch_do_execve.cpp` | 6,067 | `92828a418f3c0db75f298f98551be6604b58bc394274b6cba8871e731174626e` | 完全相同 |
| 4 | `engine/src/patch_do_execve.h` | 735 | `4061fd4af6c9276c23b984483e67a9f32f512d4d47044bed1e80844f0b3e5908` | 完全相同 |
| 5 | `engine/src/patch_avc_denied.cpp` | 2,267 | `c812a30ca6d234eab3f6f8bc89a146d4718acef936caac3d22460a2cbd211e73` | 完全相同 |
| 6 | `engine/src/patch_avc_denied.h` | 433 | `503fe20399b33593f31fcf9bed5c70583759bcdc7d1824ce42191b2a0d510171` | 完全相同 |
| 7 | `engine/src/patch_audit_log_start.cpp` | 2,471 | `469439f739051d0d0c5afc37905a20a69091590e4cdf14e68a583482a8369192` | 完全相同 |
| 8 | `engine/src/patch_audit_log_start.h` | 502 | `d16c07379522ddf1cb6b8e80c039e6f842e9ebbe8fe2f467393225e865c4d667` | 完全相同 |
| 9 | `engine/src/patch_current_avc_check.cpp` | 2,436 | `0e3a96563f08cf0ef14d928697fc4d696f95fcfc93cfbdb2912497ed409f34d8` | 完全相同 |
| 10 | `engine/src/patch_current_avc_check.h` | 389 | `97509f8f490428f3feb67b405cc0e7dd41237087d25b856d6a8afa514e0a0e5f` | 完全相同 |
| 11 | `engine/src/patch_filldir64.cpp` | 3,796 | `9981c6d63484c07920996b9445ca0915094b555421441f407b2b94a2be590426` | 完全相同 |
| 12 | `engine/src/patch_filldir64.h` | 605 | `3c88aea9c0d9fd6816943a24d24ad1ce2febb3456527269edb1c8fd2dc631d01` | 完全相同 |
| 13 | `engine/src/patch_compat_filldir.cpp` | 3,905 | `30963060865f9eea04735a3a80a3a76d8d7d6b6734d5dd7b310c8fd1f67b0929` | 完全相同 |
| 14 | `engine/src/patch_compat_filldir.h` | 642 | `48ec8c1080d2a7f98a6132a6c942010d4fad083bf95d5afffe8d08a6f1efed93` | 完全相同 |

### 2.2 基于上游**改造**的文件（3 个）

| 文件 | 大小 | sha256（当前指纹） | 上游对应文件 | 关系 |
|---|---|---|---|---|
| `engine/src/patch_base.cpp` | 9,489 | `f19531b831643d099171259ed99dc93d618b94d56c4887cce6f60ef973bf6a1c` | `patch_base.cpp`（7,485） | **改造衍生**：保留上游 emit/patch 原语，新增 ①拷贝构造补拷 `m_huawei_kti_calc_base`（修复华为内核 rc 路径 adrp 基址为 0）、②`emit_safe_bl`/`patch_jump` 返回值可检（汇编失败不再静默产出空字节）、③`_exit(EXIT_FAILURE)` 猝死出口改为 `handleError` 统一失败路径（保 C1 契约）、④`skip_pac_bti_at_func_start`/`find_all_aarch64_ret_offsets` 边界钳制。**上游部分仍属原作者保留所有权利**，本项目新增部分属 AGPL-3.0。 |
| `engine/src/skroot_patch_main.cpp` | 117,526 | `1a2c4a87e48e22ad3f4b37ed568450efc0abdb77a60b6a5f3d19b2a7762bbfa3` | `patch_kernel_root.cpp`（12,492） | **改造衍生**：保留上游核心修补与提权逻辑，新增命令行契约（`--in/--out/--key/--gen-key/--json/--workdir/--quiet`）、NDJSON 进度上报、退出码体系、确定性输出。**上游部分仍属原作者保留所有权利**，本项目新增部分属 AGPL-3.0。 |
| `engine/src/patch_kernel_root.h` | 3,028 | `e1ac978236e4c48505ad1d2d66b20402818dedae719c3eec734a7414a943ada7` | `patch_kernel_root.h`（2,630） | **改造衍生**：保留上游宏与数据结构定义，新增缓冲区边界守卫（`patch_ret_*` 读首指令前的 `start+4 <= size` 检查、`patch_data` 的 `start+buf_size <= size` 检查——修复畸形 kallsyms 输入下的越界读）。**上游部分仍属原作者保留所有权利**，本项目新增守卫部分属 AGPL-3.0。 |

### 2.3 上述内容在本仓库中的位置与影响面

- **源码**：`engine/src/` 下 §2.1、§2.2 所列文件。
- **二进制**：编译产物 `skroot-patch`（exec）、`libskrootpatch.so`（JNI）**包含上游代码的编译结果**。
  发布二进制时，视为"包含上游代码的衍生作品"，需同时满足上游授权与本文件 §5 的检查清单。
- **文档**：`docs/SPROOT-DOC-A1-SKRoot机制深度分析.md`、`docs/NEW-DOC-02-修补引擎本地化方案.md`、
  `docs/NEW-DOC-03-修补失败根因.md` 等对上游机制的**分析性描述**属本项目原创文档，AGPL-3.0；
  其中引用的上游代码片段仍属原作者。

---

## 3. 本项目自研内容（AGPL-3.0）

以下为**完全自研**、不含上游代码的内容，按仓库根 `LICENSE`（AGPL-3.0）授权：

| 领域 | 内容 |
|---|---|
| 引擎自研部分 | `engine/src/bootimg.{cpp,h}`（自研 boot.img v3/v4 解析与等长重打包）、`engine/src/patch_pipeline.{cpp,h}`（流水线编排）、`engine/src/spr_json.hpp`、`engine/src/spr_sha256.h` |
| 引擎构建/集成 | `engine/android/**`（NDK 交叉编译、JNI 桥、验证脚本）、`.github/workflows/engine.yml` |
| 管理器 App | `app/**`（Kotlin + Jetpack Compose + miuix UI）、`app/src/main/java/me/sproot/patch/**`（JNI 门面） |
| 守护进程 apd | `apd/**`（Rust：daemon / deploy / root-exec / auth / selftest / secret / selinux / bootstate） |
| 测试与质量 | `quality/**`（verify_patch.py、check_apk_payload.py、record_ci_artifact.py、selftest 等） |
| 文档与方案 | `docs/**`、`LICENSES.md`、`NOTICE`、`README.md` |
| CI 工作流 | `.github/workflows/**` |

> 注：`app/` 与 `apd/` 中若存在**历史遗留**的 APatch（GPL-3.0）派生代码，将按 §5 检查清单单独标注；
> 当前 `v2` 分支已按"从零实现"重写，仅保留 API 形状参考，不复用其代码。

---

## 4. 第三方组件

### 4.1 随源码/二进制分发的组件

| 组件 | 版本 | 许可 | SPDX | 用途 | 分发形式 |
|---|---|---|---|---|---|
| **asmjit** | 上游随附（asmjit2-src） | Zlib | `Zlib` | 运行时生成 ARM64 shellcode | 静态编译进引擎 |
| **capstone** | 5.0.7 | BSD-3-Clause | `BSD-3-Clause` | 指令反汇编/校验（分析路径） | 静态编译进引擎 |
| **cJSON** | 随附 | MIT | `MIT` | JSON 输出（NDJSON 进度/报告） | 静态编译进引擎 |
| **magiskboot** | Magisk **v31.1** | **GPL-3.0** | `GPL-3.0-only` | boot.img 解包/重打包（主打包工具） | APK 内 `lib/arm64-v8a/libmagiskboot.so` |
| AndroidX / Jetpack Compose | BOM 2026.08.00 | Apache-2.0 | `Apache-2.0` | App UI 基础设施 | APK |
| Kotlin 标准库 / kotlinx | 2.4.20 | Apache-2.0 | `Apache-2.0` | App 语言运行时 | APK |
| **miuix** | 0.9.4 | Apache-2.0 | `Apache-2.0` | UI 组件库 | APK |
| Rust crates（apd） | 见 `apd/Cargo.lock` | 多为 MIT / Apache-2.0 | 各自 | 守护进程依赖 | 二进制 |

各组件原始许可文本随附于：
- asmjit / capstone / cJSON：`engine/src/3rdparty/`（含各自 LICENSE 文件）
- magiskboot：**APK 内 `NOTICE`**（含来源 URL、版本、二进制 sha256、GPL-3.0 全文获取方式与源码链接）

### 4.2 仅作参考、未复制代码的项目

| 项目 | 许可 | 与本项目关系 |
|---|---|---|
| KernelSU（`tiann/KernelSU`） | **GPL-3.0** | **已复制 UI 代码**：miuix 主题/底栏/组件/六个屏幕版式（详见 `NOTICE §6.1` 文件清单）。GPL-3.0 与 AGPL-3.0 兼容，整体按 AGPL-3.0 分发；业务逻辑未取。架构层面（薄 UI + 厚 CLI、自研 bootimg、root-exec 模型）亦参考 |
| APatch / KernelPatch（`bmax121`） | GPL-2.0 / GPL-3.0 | **仅接口形状与机制参考**，未 vendor、不调用 kptools/kpimg |
| Skroot-Next（`zh-sc/Skroot-Next`） | GPL-3.0 | **仅调研参考**（密钥 ABI 差异、asm oracle 思路），未复制代码 |
| Magisk（`topjohnwu/Magisk`） | GPL-3.0 | 仅取用其发布的可执行组件 magiskboot（见 §4.1） |

---

## 5. 发布前检查清单（发布门禁）

发布任何构建物之前，逐项确认：

- [ ] **LIC-1** 发布物含本文件 `LICENSES.md` 与仓库根 `LICENSE`（AGPL-3.0 全文）。
- [ ] **LIC-2** APK 内含 `NOTICE`，列明 magiskboot（Magisk v31.1，GPL-3.0，来源 URL、sha256、源码获取方式）。
- [ ] **LIC-3** §2 所列上游文件**未授予 AGPL**；发布物中不得出现"本仓库整体以 AGPL 授权"之类将上游内容纳入 AGPL 的表述。
- [ ] **LIC-4** 若上游授权文本**未明确覆盖对外分发/再许可**：则公开发布物必须**移除** §2 内容（改为不含引擎的构建，或仅内部使用），或经项目所有者与上游作者书面确认后再发布。
- [ ] **LIC-5** `engine/src/3rdparty/**` 各 LICENSE 文件随源码归档分发；二进制分发时在 `NOTICE` 中列明。
- [ ] **LIC-6** `apd/Cargo.lock` 中的 GPL/AGPL 类依赖（如有）在 `NOTICE` 中单独列出并评估传染性。
- [ ] **LIC-7** 不得移除或修改上游文件中的任何版权声明（当前上游文件内**无**版权头，因此**不得添加**可能误导归属的头，反之亦然）。

---

## 6. 变更记录

| 日期 | 变更 |
|---|---|
| 2026-11-08 | 首次建立。人类裁定：本项目自有代码采用 AGPL-3.0；SK 上游内容逐项标注、原作者保留所有权利、不改动其许可。经 `diff` 核实上游 15 个文件逐字节相同、1 个文件为改造衍生。 |
