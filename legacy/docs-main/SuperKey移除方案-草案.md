# SPRoot 管理器 SuperKey 移除方案（v2 定稿）

> 所属：pl_56e797e81aac672c「SPRoot 管理器 APK 改造」· 客户端
> 依据：《AP 管理器与镜像修补链分析》（a_9d8c9a952e5d63fc v3 已复核）§3.6 引用点清单
> 已定决策：Q1=拉上游锁 e8e4651c · Q2=KPM 保留暂搁置 · **Q3=A SuperKey 逻辑彻底移除** · Q4=B sepolicy.rule 注入保留+安装警示
> 四项开放问题裁决（leader）：① InstallAP.sh **保留**并列入 P1；② 密钥字符集 **[A-Za-z0-9]{48}**（已写入 DOC-C1）；③ 旧 key 迁移**人类终裁 A：需重新修补，不提供迁移向导**；④ magica 支线**保留另立条目**
> 状态：**v2 定稿**——P1（a1-a8）与 P2 核心接线均已落地（动态 f_e3b09b57c210ea47 / f_6744c7c845046767），P3 等《SKRoot↔AP 调用契约》v1.0

---

## 1. 目标与非目标

**目标**
1. 移除 APK 内全部 SuperKey 生成/存储/校验/回退逻辑（Q3=A，不留 "su" 隐式授权路径）。
2. Root 授权链改为 SKRoot 48 位密钥：修补完成后由修补引擎产出密钥 → 管理器安全持有 → shell/JNI 全链使用。
3. 修补 UI 对接 SK 修补引擎（接口位预留），模块管理 UI 对接 AP 二进制（接口位预留）。
4. 模块安装含 `sepolicy.rule` 时展示「隐藏性不作保证」警示（Q4=B）。

**非目标（本期不做）**
- KPM 入口拆除（Q2 搁置；仅要求修补引擎契约给出 KPM 能力位）。
- 内核侧 scdefs/supercall 协议改动实现（内核修补组职责，本文只声明依赖）。
- apd → AP 二进制的改造（系统守护组职责）。

---

## 2. 移除/替换矩阵（按文件）

> 图例：🗑 删除　♻️ 改造　🆕 新增　⏸ 接口位预留，契约到了再实现

| # | 文件 | 动作 | 具体内容 |
|---|---|---|---|
| 1 | `APatchApp.kt` | ♻️ | 删 `resolveSuperKey()`（su 优先 + legacy 回退整段）；`superKey` 属性改为 `superKey48`，setter 不再驱动 kp/ap 双状态判定，状态初始化改走「SK 密钥就绪检查」（⏸ 依赖 SKRoot 契约的就绪探测命令） |
| 2 | `APatchKeyHelper.java` | ♻️ | 保留 Keystore/GCM 加密壳；SP 键改 `sk_key48_enc`；删 `readSPSuperKey` 的明文迁移与 `SUPER_KEY_IV` legacy 分支；新增「一次性写入后锁定」（写入后 `skip_store=1`，防覆盖） |
| 3 | `PatchesViewModel.kt` | ♻️ | 删 `superkey` 手输状态、`checkSuperKeyValidation`（8-63 位规则）；`doPatch(mode, useKey)` 改 `doPatch(mode)`：密钥参数改由 SK 修补引擎返回（⏸ 依赖修补引擎 I/O 契约：引擎调用方式、密钥回传格式）；删 `"su"` 默认值兜底 |
| 4 | `Patches.kt` | ♻️ | 删「自定义 SuperKey」开关 + `SetSuperKeyView`（222-231、488-560 行）；🆕 修补完成卡片展示 48 位密钥（可复制、仅首次明文显示，之后以掩码回显）；校验提示文案换 SK 格式说明 |
| 5 | `APatchCli.kt` | ♻️ | `createRootShell/tryGetRootShell/createMainRootShell` 命令模板保留，key 实参换 `superKey48`；删 kpatch ELF 回退分支（0.11+ 已弃用）；`refresh()` 保留 |
| 6 | `Natives.kt` + `apjni.cpp` | ♻️ | 方法签名不动（key 仍是首参字符串）；空 key 兜底 `ensureSuperKeyNonNull` 从「兜底 su」改为「直接抛异常」——密钥缺失必须显式失败，不再静默降级 |
| 7 | `KPModel.kt` | ♻️ | `KPImgInfo.superKey` 字段改 `skKeyInfo`（⏸ 依赖新 kptools ` -l` 输出格式契约：SK 密钥元信息展示字段） |
| 8 | `assets/boot_patch.sh` | ♻️ | 参数名改 `SKKEY`；删 `[ "$SUPERKEY" != "su" ]` 分支——**始终传 `-S "$SKKEY"`**（SK 分支无隐式授权）；其余逻辑（KALLSYMS 校验/ori.img/repack）不动 |
| 9 | `assets/InstallAP.sh`（Recovery 安装路径） | ♻️ | 同步删 su 分支判断（若保留该安装路径；待定，见 §5 开放问题） |
| 10 | `Home.kt` / `MainActivity.kt` | ♻️ | 状态呈现随 §2#1 状态判定改造联动；「未修补→去修补」引导不变 |
| 11 | `SuperUserViewModel.kt` / `SuperUser.kt` | ⏸ | grantSu/revokeSu 调用面保留；底层是否改走 SKRoot 48 位密钥授权（替代 kstorage su 列表）依赖 SKRoot 调用契约——**本文件只换 key 来源，不重写授权模型** |
| 12 | `Install.kt` | 🆕 | 安装确认对话框（11282 新增）加检测：zip 内含 `sepolicy.rule` → 追加警示「该模块包含 SELinux 规则注入，SPRoot 对其隐藏性不作保证」；实现方式：`ZipFile.entries()` 扫描，无需解压 |
| 13 | `APatchCli.kt`（模块链） | ⏸ | `execApd` → AP 二进制改造后命令面变化（`/data/adb/apd` → AP multicall？）依赖系统守护组契约；本期不动，仅确认调用集中在 `execApd/listModules/installModule/runAPModuleAction/softReboot` 七个函数，改造时单点替换 |

**删除后不得残留的符号（CI grep 门禁）**：`resolveSuperKey`、`readSPSuperKey`、`writeSPSuperKey`（旧名）、`SetSuperKeyView`、`patch_custom_superkey`、`checkSuperKeyValidation`、`"su"` 作为 key 默认值（`Natives.su(0,null)` 调用需随 #1 消失）。

---

## 3. SK 48 位密钥流（目标态）

```
[修补] PatchesScreen → SK 修补引擎(⏸契约) ──产出 48 位密钥──┐
                                                          ▼
                            kptools -S <key> 绑定内核   +   密钥回传 APK
                                                          ▼
[持有] APatchKeyHelper: Keystore-GCM 加密 → SP(sk_key48_enc)，一次性写入后锁定
                                                          ▼
[使用] APApplication.superKey48 ─┬→ createRootShell: SUPERCMD + key + -Z magisk:s0
                                 └→ Natives.* → supercall(key, …)
                                                          ▼
[展示] 修补完成卡片：首次明文 + 复制按钮；此后掩码；设置页提供「重新生成密钥」
       入口（⏸ 依赖修补引擎重签能力，需 SUPERCALL_SKEY_SET 0x100b 或 SK 契约等价命令）
```

密钥格式约束（待内核契约确认）：48 字符、`SUPERCALL_KEY_MAX_LEN=0x40` 内、建议含大小写+数字（避开 shell 元字符，因为要进 `boot_patch.sh $1` 与 shell 命令行）。

## 4. 实施顺序

1. **P1（可立即做，无契约依赖）**：#3/#4 删手输 UI 与校验、#8 脚本删 su 分支、#6 兜底改硬失败、#12 sepolicy 警示、#2 存储壳改造——先让「移除」落地，密钥来源临时由「修补引擎 stub」提供（debug 构建可注入测试密钥）。
2. **P2（等修补引擎契约）**：#1 状态判定、SK 引擎调用与密钥回传、#7 元信息解析、密钥再生成入口。
3. **P3（等 SKRoot 调用契约）**：#5/#11 shell 与授权链最终形态、#13 模块链换 AP 二进制。
4. 每阶段过 CI grep 门禁（§2 清单）+ 编译 + 单元测试（密钥加解密、zip 扫描警示）。

## 5. 开放问题（需 leader/契约方裁决）

1. Recovery 路径 `InstallAP.sh` 是否保留？（上游 0.11 已偏 soft 路径，若弃用可直接删）
2. 48 位密钥字符集是否允许 shell 特殊字符？建议契约直接限定 `[A-Za-z0-9]{48}`。
3. ~~旧版本升级兼容：是否提供「旧 key 迁移向导」~~ **已终裁（人类，A 方案）：需重新修补，不提供迁移向导**——P3 定稿：不做旧 key 探测、不留 nativeReady legacy 分支，升级用户引导文案为「需重新修补」。
4. magica 越狱支线（jailbreak）不含 superKey，是否随 Q3=A 一并裁剪出 SPRoot 范围？（建议保留，另立条目）

---

*草案 v1 · 客户端 · 2026-11-06 · 等契约后升级 v2 定稿*
