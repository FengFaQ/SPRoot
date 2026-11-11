# quality/crossverify —— 内核补丁独立交叉验证（第二套实现）

> 与 `quality/static/`（断言式静态检查）和 `quality/bootability/`（task-18 行为级验证）
> **完全独立**：本目录不 import 它们、不共享辅助函数、连解码方式都刻意不同。
>
> **为什么要第二套**：一套检查系统对自己说谎时，它自己发现不了。
> 本项目已有两次前车之鉴——
> - AVB-3 断言写成永远 PASS，全绿了一整代
> - `patch_audit` 首跑 `attributed_regions=0`（没传 `--report`），仍然 `rc=0 / ok:true`
>
> 两套独立实现互相校验，比单套更难自欺。

---

## 与另两套的分工

| 体系 | 问的问题 | 手段 |
|---|---|---|
| `quality/static/` | 产物字段对不对？hash 变了没？ | 比字符串 / 比字段 |
| `quality/bootability/` | 镜像能不能跑起来？ | 自研解码 + 启发式归属 + qemu |
| **`quality/crossverify/`（本目录）** | **上面两家得出的结论，换个实现还成立吗？** | **objdump 真实反汇编 + 归一化日志 diff** |

---

## 检查项

| ID | 判据 | 失败含义 |
|---|---|---|
| **XV-1** | 逐字节 diff：区段数 / 总字节 / 等长 | 不等长 = 破坏镜像布局 |
| **XV-2** | 每处改动能否归因到引擎 `report` 声明的地址（容差 0x400） | 有改动**未被 report 记录** → 审计者以为"没记就是没改" |
| **XV-3** | 每个 code cave 是否有**控制流出口**（`b` / `b.cond` / `br` / `ret`），且目标落在 cave 外 | 无出口 = 执行到此处**内核必挂死** |
| **XV-4** | 改动是否越出 Image 头声明的 `image_size` | 越界 = 改到了分区填充区或别的东西 |
| **XV-5** | 启动日志**时间戳归一化后**逐行 diff（只比首次 panic 之前） | 出现差异 = patch 改变了启动行为 |
| **XV-6** | kallsyms 可用性探测（决定"按符号名查驱动"能否做） | 不可用 = 驱动检查只能靠地址区间，**这是能力边界不是通过** |

---

## 用法

```bash
# 自检（负样本铁律：每条判据都要在坏输入上判红）
python3 quality/crossverify/xv.py --self-test

# 完整校验
python3 quality/crossverify/xv.py \
  --orig    kernel.orig \
  --patched kernel.patched \
  --report  kernel.patched.report.json \
  --log-orig    serial-orig.log \
  --log-patched serial-patched.log \
  --json    quality/crossverify/records/xv-real.json
```

退出码：`0` = 全部 PASS；`1` = 有 FAIL；`2` = 参数错误。

---

## 三个必须知道的设计细节

### 1. cave 边界用 **report 声明地址**，不能用 diff 碎片

diff 出来的"改动区段"会被**原地未变的字节**切成碎片。实测：一个 176 字节的
shellcode cave，因为中间有若干未变的 zero 字节，被切成 `110 + 63 + 5` 三片。
拿中间那片去判"有无出口"**必然误报**。

正确做法：从 `report` 取声明地址，把相邻地址之间的区域合并成候选 cave。

### 2. 跳转必须包含**条件跳转**

shellcode 里的错误分支常写成 `b.cs 0xa0` / `b.ne 0xa0`（无逗号）。
早期版本只匹配无条件 `b` 且按逗号分割，导致真实 cave 被误判"无出口"。
现在按「行内最后一个 `0x....` 是目标」抓目标，兼容 `b.cs` / `b.ne` / `cbz` / `cbnz` / `tbz` / `tbnz`。

### 3. 启动日志必须**归一化 + 截到首次 panic**

- 行首 `[ 0.123456][0:0]` 时间戳每次运行都不同 → 直接 diff 会全不等
- panic 后 init 反复重试挂载，日志长度随重试次数变化（实测 7923 vs 9476 行）
  → 只比**首次 panic 之前**的序列，否则是噪声不是功能差异

---

## 当前实测结论（2026-11-11）

真实修补（CI 引擎 `skroot-engine-arm64`，随机 48 字符 key，`rc=0`）：

```
[XV-1] PASS  改动 20 区段 / 466 字节，等长=True
[XV-2] FAIL  声明地址 54 个；可归属 17 处，未解释 3 处
            未解释: 0x0038fcd4 / 0x003a1d88 / 0x003a276c
[XV-3] PASS  3 个 cave，共 12 个控制流出口
[XV-4] PASS  image_size=0x2100000；越界改动 0 处
[XV-5] PASS  归一化后 +0 / -0 行（启动路径与原厂逐行一致）
[XV-6] INFO  kallsyms 特征串全部不存在
```

**XV-2 的 3 处未解释是真缺陷**：那三处是 `do_execve` / `filldir64` /
`compat_filldir` 的**真实函数入口**被换成 `b` 跳转，但引擎 `report.hooks[]`
只记了跳板侧、没记被改写的原函数入口。已上报 engine 线修 report 完整性。

**XV-5 的 `+0/-0` 是"能开机"的最强证据**：修补后内核在 qemu 上走完与原厂
**逐行相同**的启动路径（含全部驱动 probe），只停在 `Unable to mount root fs`
——那是该 boot.img `ramdisk_size=0` 的固有性质，与 patch 无关。

---

## 诚实边界

1. **qemu ≠ 真机**。验证的是"内核代码本身没被 patch 破坏"，不是"魅族21 能开机"。
   真机有厂商 dtb、专有驱动、HAL 层，这些 qemu 里没有。
2. **`ramdisk_size=0`** 使两个内核都到不了用户态；`init_boot.img` 的 ramdisk
   需要合并后才能验证 init 阶段。
3. **kallsyms 不可用** → "驱动符号是否被 patch 覆盖"**无法按符号名验证**。
   只能退化为"改动地址是否落在已知驱动地址范围外"，那是弱判据。
   **不要因为 XV-6 没报错就以为驱动没问题。**
4. **`XV-3` 只证明"cave 里有出口"，不证明"每条执行路径都能正确返回"**。
   完整的数据流/语义验证需要真机跑。
