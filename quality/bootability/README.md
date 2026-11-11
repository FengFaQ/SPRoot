# quality/bootability —— 开机能力验证体系（B0）

> 与 `quality/static/` **完全独立**的新体系。旧体系是"断言式"（比 hash / 比字段 / 比字符串）；
> 本体系是**行为级/逐代码**验证：让修补后的内核真的跑，看它会不会崩、驱动是否还活着。
>
> 建于 2026-11-11（task-18）。用户诉求原话：
> 「不要依赖现有的检查方式，重新开一个检查方式，逐代码检查修补，
>   检查修补过的镜像是否能正常开机、驱动是否正常。」

## 为什么需要另一套

`quality/static/*.py` 能证明「文件等长」和「sha256 变了」，但**证明不了**：

- 那 ~465 个被改的字节落在**哪里**（是 code cave，还是正在运行的驱动 probe？）
- 跳转目标是否真的指回原函数，还是跳进了数据段
- 开机路径上的代码有没有被踩坏

「补丁写进去了」和「设备还能开机、摄像头还能用」是两件事。本体系只回答后者。

## 三层结构

| 层 | 目录/脚本 | 依赖 | 回答的问题 |
|---|---|---|---|
| A. 逐代码补丁审计 | `patch_audit.py` | `--report`（**硬要求**） | 每处改动落在哪个 hook 组/函数、跳转对不对、有没有踩非 .text |
| B. 驱动完整性 | `driver_integrity.py` | 可选 `--engine-log`（地址映射） | 有没有驱动符号被 patch 覆盖 |
| C. 开机行为 | `boot_qemu.py` | qemu-system-aarch64 | 原厂 vs patched 谁能跑到哪一层、串口日志是否一致 |

三层都可以独立跑；A/B 不依赖 qemu，C 缺 qemu 时如实报"不可行"而不是跳过。

## 反"假绿"机制（本体系最重要的部分）

本体系专门防一类缺陷：**门禁跑了、变绿了，但什么都没验到**。
这在本项目已发生过三次（AVB-3 空断言、`apd_in_path` 初版、本体系 A 层的 0% 归属）。

| 机制 | 防的是什么 |
|---|---|
| **PA-0** | 有字节改动但 0 处可归属 → **BLOCKING**。无法回答"改的是哪个函数"就不算做了逐代码审计 |
| `--report` 硬要求 | `run_all.py` 缺失 `--report` 直接 exit 2，而不是产出 `attributed_regions: 0` 的"绿"报告 |
| `stage_status()` | 每层从自己的 JSON 读真实判据（`blocking[]` / `comparison.regression`），**形状不符即 FAIL**，绝不默认 True |
| `unknown ≠ pass` | 无地址映射、无 qemu 时明确记 gap/infeasible，不用 PASS 掩盖 |
| 负样本自检 | `self_test.py` 25 条，每条断言都必须能被证明会 FAIL；连自检自己也用 `--self-test-of-self-test` 证明会红 |

## 用法

```bash
# 一条命令跑完（推荐）
bash quality/bootability/run_sample.sh samples/meizu21/boot.img <engine> /tmp/b0

# ARM64 引擎在 x86 宿主上：
ENGINE_RUNNER=qemu-aarch64-static bash quality/bootability/run_sample.sh ...

# 分层
python3 quality/bootability/patch_audit.py      --orig k.orig --patched k.patched \
        --report k.patched.report.json --json out/a.json      # --report 必填
python3 quality/bootability/driver_integrity.py --orig k.orig --patched k.patched \
        --engine-log engine.log --json out/b.json
python3 quality/bootability/boot_qemu.py        --orig k.orig --patched k.patched \
        --record-dir quality/bootability/records/

# 自检（无需样本/qemu，秒级，适合进 CI）
python3 quality/bootability/self_test.py
python3 quality/bootability/self_test.py --self-test-of-self-test
```

## 铁律（沿用项目历史教训）

**每条断言都必须有负样本证明它会 FAIL。** 没有负样本的断言 = 未证明。

本体系的 `self_test.py` 会主动构造违规输入（伪 code cave、未对齐跳转、越界 `b`、
驱动区被踩），要求对应断言**必须变红**；任何一条负样本没能变红，自检本身判失败。

这条铁律的由来：AVB-3 曾因公式错误写成"永远 PASS"的空断言，比没有断言更危险
（`docs/HANDOFF-2026-11-08.md:153-155`）。2026-11-11 的 `apd_in_path` 初版也栽在同一
模式上——实现看起来完全正确，靠跑负样本才发现删掉符号链接仍然 PASS。

## 诚实边界（务必读）

- **qemu 通过 ≠ 真机可开机。** Android 内核在 qemu 上启动是非常规用法：缺 devicetree、
  缺 HAL、缺 vendor 分区都会让它失败。这类失败**不能**判为"镜像有问题"。
- 因此 C 层的判据**只有相对差异**：原厂 vs patched 在相同 qemu 配置下表现一致 = 证据；
  patched 比原厂更早失败 = 回归。
- 本样本 `boot.img` 的 `RAMDISK_SZ = 0`（无 ramdisk）→ Android 内核大概率到不了用户态。
  **这本身是正确结论，不是本体系的失败。**
- "在 qemu 上验证过"与"在真机上验证过"必须分开陈述，报告里逐条标注。
