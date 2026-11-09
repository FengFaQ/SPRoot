# 团队交接记录 · 2026-11-09

> 背景：会话中模型提供商发生切换，导致既有子智能体（teammate）无法继续工作。
> 本文件在**清理旧会话之前**留档：记录五名成员的职责、已交付物、失败/中断情况，
> 以及重建团队后的启动任务。后续接任者请以本文件 + `docs/HANDOFF-2026-11-08.md` 为准。

---

## 1. 团队构成（旧会话，即将清理）

| 成员 | 职责 | 状态 |
|---|---|---|
| `architect` | SP 管理器架构与契约设计 | inactive（会话后期多次中断，且会话 token 只读无法 push） |
| `patcher` | 修补引擎本地化与失败根因 | inactive（多次 fail，最后改动由 Lead 代推） |
| `app-client` | 全新 SP 管理器 App 客户端 | inactive（多次 fail，交付均已落 v2） |
| `sys-guard` | 模块管理与守护进程（apd） | inactive（多次 fail） |
| `qa` | 测试策略与验收 | inactive（工具链已交付完整） |
| `lead` | 编排、审查、代推、裁决 | 运行中 |

**关键教训**：长会话成员反复因上下文过长/会话崩溃而中断。重建后应：
①每次只派**单一明确任务**，完成即收；
②成员无写权限时走 `git format-patch` + Lead 代推；
③关键改动由 Lead 直接在仓内落地，不让单点卡住整条链。

---

## 2. 各成员已交付物（全部已合入 `v2`）

### 2.1 architect
- `docs/NEW-DOC-01-技术方案v0.1.md` → **v1.1.3**（含 §3.7 客户端能力清单 + **D1~D7 设计/实现差异台账**、§11 许可合规、§3.5.4 密钥 ABI 注记、§1.4 刷机风险边界）
- `NOTICE` §8 / §8.2（apd Rust 依赖合规）
- 提交方式：无 push 权限，产出 patch 于 `dist/arch-*/`，由 Lead `git am` 代推（`6c1021e`、`ef365f3`、`91323f0`）

### 2.2 patcher
- `report.hooks[]` 逐 hook 落位（固定 schema，未解析组不省略，魅族21 实测 15 组）
- `report.hook_groups_count / hook_groups_resolved / hook_groups_expected`（引擎自算期望值：≥6.1 基 14 / <6.1 基 11 / +huawei 各 +1）
- `verify_artifact.sh`：**A7 T-12 强断言**、**A8 分支矩阵纯逻辑自检**（5.10→11、+huawei→12、6.1→14、+huawei→15、6.6→14）、**B7 密钥自证**（`embed_key.new_bytes` 以 `key[0:47]` hex 开头、`00` 结尾）
- `engine.yml`：NDK 交叉编译 + ELF AArch64/静态/PT_TLS 断言 + qemu 契约 101/102 + magiskboot v31.1 固定版本哈希
- `docs/NEW-DOC-02`：magiskboot 主路径 + **附录 C 手机端自检清单**（宿主 6.4s / qemu 226s / 真机 3–10s 基线）
- 修掉 Tier B 自身 bug：随机密钥仅 ~44 字符导致 rc=101 假失败

### 2.3 app-client
- 四页骨架（主页/修补/模块/设置）+ miuix 0.9.4
- 修补链 8+1 步：自检 → 备份 → magiskboot unpack → JNI patch → 覆盖 kernel → repack → **T-12 回读** → 导出
- **D6 AVB 硬断言**：AVB-1 footer 必留 / AVB-2 严格等长 / AVB-3 内嵌 vbmeta 逐字节比对（绝对偏移语义）
- **D1 RcInject**：注入 `init.sproot.rc` + 幂等 import，AVB 冲突时回滚 ramdisk → 重 repack → 重跑 T-12/AVB
- 模块页 list/install/uninstall/enable/disable + 错误码中文映射（`ModuleExitCode`/`ModuleFailureHints`）
- `app/native.lock.json` 可复现锁定（run_id + sha256 + size）
- 许可 asset 随包（`assets/NOTICE` + `assets/licenses/*`）

### 2.4 sys-guard
- apd P0/P1/P2：daemon（socket SO_PEERCRED + uid-listener + 单实例 flock + boot 轮询）、deploy/undeploy 九步幂等、root-exec/auth/version、`selftest --json`（stage/summary/checks）、bootloop 守卫、单文件 allow_list
- 密钥生命周期：`SecretKey` mlock + `write_volatile` 擦除 + execve 后擦
- `-Z` 真 setcon（直写 `/proc/self/attr/current`，drop uid 前切换）
- `deploy` 原子写 `/data/adb/sproot/key`（0600、幂等、各分支擦除内存副本）+ `undeploy` 删除
- `prop-rs`（UNKNOWN 许可）feature 隔离 + `--no-default-features` 干净构建路径
- `.github/workflows/release.yml` 发布门禁（LIC/ARCH/VER/manifest，engine 再许可默认阻止）
- `apd/THIRD_PARTY_LICENSES.md`（115 依赖）

### 2.5 qa
- 测试方案 `docs/NEW-DOC-06` v0.1r14；一页纸 `附录A`
- **APK1~APK14 门禁**（含 APK14 = LIC-1 许可强断言）；从 APK 解出的 magiskboot 真跑 MB-1~MB-6
- Tier B 真样本回归（魅族21 官方 boot.img）：rc=0 / 223s / 等长 / AVB RB1~RB6 6/6
- 7 个脚本：`verify_patch` / `check_apk_payload` / `check_error_codes` / `check_licenses` / `compare_ci_runs` / `magiskboot_regression` / `record_ci_artifact`
- `quality/records/runbooks/S0-S5R-runbook.md` 真机刷入执行手册

---

## 3. Lead 直接落地的产出（无法派给成员时）

- **许可体系**：`LICENSE`(AGPL-3.0) / `LICENSES.md`（SK 上游 15 文件逐字节清单 + sha256 指纹 + 「原作者保留所有权利」）/ `NOTICE` / `check_licenses.py` / `licenses.yml` 漂移门禁
- **`apd/assets/init.sproot.rc`** 自启模板（密钥不入 rc、`oneshot+disabled` 防 bootloop、clap 顶层参数序）
- **`.github/workflows/apk.yml`** 自动发布 job（滚动 release `v2-latest`，prerelease，保留最近 10 个历史副本）
- **`.github/workflows/ci-static.yml`** 存在性维护
- **`docs/HANDOFF-2026-11-08.md`** 完整交接（§6.5 技术债台账 D1~D7、§6.6 协作方式、§6.7 自启链、§6.8 三个静默错误）
- 代推：architect / patcher / (部分) sys-guard 的改动

---

## 4. 当前仓库状态（交接时）

- 分支 `v2`，HEAD `8a196f0`
- CI：apk / apd / engine / ci-static / licenses 五条工作流；**APK 构建成功后自动发布到 Releases**
- Release：https://github.com/FengFaQ/SPRoot/releases/tag/v2-latest

### 4.1 刚修但尚未进包（重要）
HEAD `8a196f0` 修复了**真机首跑修补失败的根因**：
> 引擎成功收尾时 `remove_all(opt.workdir)` 把客户端放在 workdir 内的 `--out`（kernel.patched）一起删掉，仍返回 rc=0 → 客户端报"引擎返回成功但没有产物"。

修法：只有默认 workdir（`out_path + ".work"`）才回收；`--out` 在 workdir 内时一律不删；新增 A9 防回退断言。

**⚠️ 该修复尚未进入发布 APK**，因为 native 由 `app/native.lock.json` 锁定。待办：
1. 等 `engine-arm64` CI 产出新 artifact
2. 更新 `app/native.lock.json` 的 `engine.{run_id,sha256,size}` 与 `extra_assets`
3. 推送 → APK 重建 → 自动 release 更新

---

## 5. 重建团队后的启动任务（按优先级）

| 优先级 | 任务 | 建议负责 |
|---|---|---|
| **P0** | 更新 `native.lock.json` 的 engine 到含 workdir 修复的新 artifact，触发 APK 重建并在 Release 验证 | Lead 或 app-client |
| **P0** | APK 内引擎/客户端联调确认：`--out` 与 `--workdir` 分离 | app-client |
| **P1** | **UI 重做：完全照抄 KernelSU 的 miuix 实现**（人类明确指令） | app-client |
| **P1** | 首启 `selftest` 返回 125 的文案改为「尚未部署 AP（需先刷入修补镜像）」 | app-client |
| **P1** | 修补进度条映射引擎 NDJSON 的 pct/phase（当前卡在 35% 不动） | app-client |
| **P2** | `key_material` 对 >47 字符输入改为严格失败（现在静默截断，会导致提权静默失败） | sys-guard |
| **P2** | 真 `init_boot` 优先闭环：修补页支持同时选 `boot.img` + `init_boot.img` | app-client |
| **P2** | Tier B 增加「CLI 下 `--out` 位于 `--workdir` 内」行为级回归用例 | qa |
| **P3** | 真机刷入窗口（S0~S5R 手册已备，需人类在场） | 人类 + qa |

---

## 6. UI 重做任务书（人类指令原文）

> 「拉取 KSU 的官方仓库，KSU 也使用了 miuix，你只用关心这个 UI，不用看另一个 UI 实现，**完全照抄它的 UI 实现**，在设置里面把它的设置项删掉，换成自己的。」

- 参考源：`https://github.com/tiann/KernelSU`（**GPL-3.0**，与项目 AGPL-3.0 兼容，可合法照抄，需在 NOTICE 标注来源）
- 只取 **miuix 版实现**，忽略 Material 版（不要 `UiMode` 双模式切换）
- 主题：`ui/theme/{MiuixTheme, Colors, Type, ThemeExt}.kt`
- 骨架：`ui/MainActivity.kt`(miuix 分支)、`ui/navigation3/`、`component/bottombar/{BottomBarMiuix, NavigationRailMiuix}.kt`
- 组件：`ui/component/miuix/*`（含 animation/effect/modifier 子目录）、`statustag/StatusTagMiuix`、`dialog/DialogMiuix`
- 屏幕：`HomeMiuix` / `InstallMiuix`+`FlashMiuix` / `ModuleMiuix` / `SettingsMiuix` / `AboutMiuix`
- 依赖：miuix **0.9.4** 五个模块 `miuix-ui / miuix-icons / miuix-nav / miuix-preference / miuix-blur`
- **设置页**：删掉 KSU 全部设置项，换成 SPRoot 自己的（主题、Root 密钥、日志、备份、AP 部署、引擎自检、工作目录清理、关于/许可）
- 业务逻辑类（PatchPipeline / ApdClient / InstallState / PatchEngine / Capabilities / SecretStore / BackupStore）**一行不改**

---

## 7. 协作纪律（沿用，务必遵守）

- **分支**：只推 `v2`；`main` 为旧 APatch 代码，不得作为测试对象
- **构建**：禁止本地编译交付物，一律 GitHub Actions
- **许可文件归属**：根 `LICENSE` / `LICENSES.md` / `NOTICE` 由 **Lead 独占维护**，成员不得修改
- **无写权限成员**：`git format-patch` 导出到 `dist/`，通知 Lead 代推
- **native 版本**：由 `app/native.lock.json` 锁定；升级必须改 lock 并单独 commit
- **artifact 下载**：先取 302 `redirect_url` 再 curl（`-L` 直连会 `curl(56)`）
- **体积口径**：区分「artifact zip 大小」与「未压缩 ELF 大小」
- **断言设计**：新增断言必须**先用负样本证明它会 FAIL**（T-12 是范例；AVB-3 曾因公式错误变成永远 PASS 的空断言）

---

*本文档由 Lead 于团队重建前整理，作为旧会话的对话/交付留档。*
