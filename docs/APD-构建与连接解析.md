# APD 构建与 AP 连接解析（逐代码）

> 目的：把 `apd` 这个 Rust 守护**怎么构建**、**怎么和 App 联系**、**稳定性风险在哪**，
> 逐代码讲清楚。基于 2026-11-11 的实际代码与实测，不是文档转述。
>
> 结论先行：**架构是健全的，稳定性问题集中在"失败被折叠成沉默"这一类**，
> 四项已修（post-fs-data 预算 / daemon 子系统恢复 / deploy 幂等 / 模块可靠性），
> 两项经验证「不是缺陷」而未改。

---

## 一、构建链（从源码到 APK 内的 libapd.so）

```
apd/src/*.rs  (26 个文件, ~450KB)
      │
      ├─ build.rs ──► 生成 VERSION_CODE / VERSION_NAME / VERSION_COMMIT
      │              （git rev-list --count HEAD + version.properties 的 epoch）
      │              写入 OUT_DIR，编译期 include!
      │
      ├─ Cargo.toml  [profile.release]
      │              opt-level="z" / lto="fat" / codegen-units=1
      │              panic="abort" / strip=true
      │              → 产物 ~2.1MB（arm64 动态链接）
      │
      ├─ gh workflow apd.yml
      │              cargo build --target aarch64-linux-android
      │              + cargo check --no-default-features（许可干净路径）
      │              + 15 项 capability marker 断言
      │              → artifact: apd-aarch64-latest / apd-aarch64-<sha>
      │
      ├─ app/native.lock.json 锁定 run_id + sha256 + size
      │              （可复现构建：CI 按 run_id 精确下载，不浮动 latest）
      │
      └─ 打进 APK：app/src/main/jniLibs/arm64-v8a/libapd.so
                    （保持 lib*.so 命名，才能被解压到 nativeLibraryDir 后 exec）
```

**关键事实（实测）**：`libapd-arm64.so` 是 **ET_DYN 动态链接**，
INTERP = `/system/bin/linker64`，NEEDED = `liblog.so / libdl.so / libm.so / libc.so`。
→ **不能在 qemu-user 下直接跑**（缺 Android linker）。本地验证只能：
- `cargo build` 出 **host(x86-64) 版本**做连接性测试（不是交付物）
- 或 `cargo test` 跑单测

---

## 二、AP 与 App 的联系（三层，逐代码）

### 2.1 SELinux 域约束决定了"引导器"只能是一个路径

```
untrusted_app 域：可以 exec  nativeLibraryDir/libapd.so   (apk_data_file)
                 不能 exec  /data/adb/apd                  (adb_data_file)
```

所以 `ApdClient.kt` 里刻意分成两个概念（这是**整个连接层最关键的设计**）：

| 概念 | 路径 | 用途 |
|---|---|---|
| `apdBootstrap()` | **永远** `nativeLibraryDir/libapd.so` | 唯一可作为"被执行文件"的引导器 |
| `deployedApd()` | `/data/adb/apd` | **只作 root-exec 的目标命令参数**，app 域绝不 exec |

`ApdClient.kt:72-76` 的注释把理由写死了：若把 `/data/adb/apd` 当引导器 →
部署完成后 app 反而起不来。

### 2.2 提权链（唯一 root 入口）

```
App (untrusted_app)
  │ 写 0600 临时密钥文件到自身私有目录
  │   argv = [libapd.so, --superkey-file <tmp>, root-exec, [opts], --, <cmd>]
  ▼
libapd.so 以 app uid 启动 → 读 --superkey-file → 拿到 47 字符 token
  │  execve(token47, NULL, NULL)
  ▼
内核 do_execve hook：filename 与内嵌 47+NUL 逐字节比对
  │ 命中 → 就地改 cred/caps/清 seccomp → 返回原 exec 路径 → -ENOENT
  ▼
进程已是 uid 0（**唯一的成功判据是 getuid()==0**，不是 execve 返回值）
```

**三个必须记住的细节**：
1. `--superkey-file` 是 **clap 顶层参数**，必须放在子命令 `root-exec` **之前**
   （写成 `apd root-exec --superkey-file ...` 会 rc=2 unexpected argument）
2. 密钥临时文件在 `finally` 里删（`ApdClient.kt:197-201`），删不掉时 warn 并留待下次启动清理
3. 内核侧失败是**静默的**（纯 ENOENT，无计数、无审计）→ AP 侧日志绝不打印密钥或任何前缀

### 2.3 数据面（schema 契约）

用 **host 构建的 apd 实测**，App 侧解析字段与实际输出完全对齐：

| 命令 | 顶层字段（实测） | App 侧读取 |
|---|---|---|
| `version --json` | `boot_stage_done / commit / daemon_pid / deployed / deployed_at / hidden_file / key_prefix_sha256 / version / version_code` | `ApdClient.deployedApdVersion()` |
| `selftest --json` | `checks[] / ok / stage / summary`，每条 check 含 `critical/detail/name/ok/status` | `ApdSelfTest` 读 `stage`/`summary`/`ok` |
| `auth list --json` | `[]` 或三态条目 | `AuthRepository` |
| `module list --json` | `[]` 或模块列表 | `Modules.kt` |

### 2.4 兼容门面（**故意保留，不能改**）

```
apd -V  →  "apd 10762"
```

这是给 **NeoZygisk** 看的：它执行 `apd -V`，取第 2 个 token 转 i32，要求 **≥10762**
才认定"这台机器有可用的 root 方案"。删掉或改小 = Zygisk 链静默失效。

> 这也是为什么 **APD 里 60 处 `APatch`/`APATCH` 字样不能全删**：
> 一部分是兼容面（`defs::APATCH_COMPAT_VERSION`、`APATCH_PACKAGE_CONFIG` 路径），
> 一部分只是注释/日志措辞。**本轮按用户要求全部保留，只修稳定性。**

---

## 三、稳定性风险与处置（本轮四项）

### APD-1 · post-fs-data 阶段硬预算

**风险机理**：`init.sproot.rc` 用 `exec -- /data/adb/apd post-fs-data`，
`exec` 是 **init 的阻塞原语** —— apd 不返回，init 就不继续。所以这一阶段任何一步卡住 = **开机卡住**。

| 阻塞点 | 原状 | 处置 |
|---|---|---|
| `sepol` 写 `/sys/fs/selinux/load` | 无超时 | **不可中断**（内核路径），改为**计量 + 记录** |
| log rotation `sh -c` | **裸 `.wait()`** | `wait_bounded(10s)` |
| 模块脚本 | 单脚本 60s，**模块数不限** | 阶段总预算 90s + 单脚本取 `min(60, 剩余)` |
| 多个 `?` 传播 | 返回 Err → init 认为服务失败 | 全部降级 warn，阶段**总是 Ok** |

**最坏耗时上界**：改前 `N×60s`（10 模块 = 600s ≈ 10 分钟）→ 改后 **100s**，且**与模块数无关**。

一个刻意的设计：预算为 0 时返回 `None`（跳过），**不是** `Some(0)`。
因为 `wait_timeout(0)` 会立刻 SIGKILL 子进程，表现成"脚本失败"而非"预算耗尽"，会掩盖真因。

### APD-2 · daemon 子系统崩溃恢复

**风险机理**：`thread::spawn` 的线程退出/panic 是**静默的**。uid-listener 死了 →
授权不同步；auth socket 死了 → 所有 su 请求失败。但 daemon 还活着、健康检查看着正常。

处置：`restart_delay()` 纯函数 + `supervise()` 监管循环。
**上限 3 次 / 退避 1s→2s→4s / 超限 `gave_up` 永久停止 + error 级日志**，状态写入
`daemon_subsystems.json`，selftest 三态：`running→ok` / `gave_up→fail(critical)` /
**无快照 → `unknown`（不是 pass）**。

**顺带发现的真缺陷**：`acquire_instance_lock()` 原返回 `Option<File>`，把所有失败折叠成
`None` → 锁文件打不开（目录不存在/权限坏）也打"another instance is already running"，
**把部署故障伪装成正常状态**。改为 `LockOutcome{Acquired, AlreadyRunning, Unavailable}` 三态。

### APD-3 · deploy/undeploy 幂等与回滚

| 核查点 | 结论 | 处置 |
|---|---|---|
| 重复 deploy 残留隐藏 su | **缺陷** | `install_su()` 每次生成随机 16 字符名，旧的从不删 → 实测 3 次留下 3 个。已加清理 |
| 重复 deploy 丢用户授权 | **不是缺陷** | `prepare_storage()` 只在 `migrated \|\| !store_exists()` 时写；实测 uid=10086 完好保留 |
| legacy `allow_list` 被覆盖 | **不是缺陷** | `if !path.exists()` 守卫 |
| 半部署"文件在但状态说没有" | **缺陷** | `state.json` 在 step 9 才写。已加 `deploy_incomplete` marker + `deploy_status` 四态 |
| rename 跨文件系统 | **不是缺陷** | 实测 `key` → `key.tmp` **同目录**，不会 EXDEV |
| undeploy 清理顺序 | **不是缺陷** | symlink 先删 → 隐藏目录 → 密钥 → rc → 目录 → **二进制最后** |

### APD-4 · 模块可靠性
（进行中）

---

## 四、连接性实测（我做的，不是文档转述）

用 host 构建的 apd，测 App 会发起的每条命令：

```
version --json        exit=0   10 字段齐全
selftest --json       exit=0   checks[]=22, stage=undeployed, summary 齐
auth list --json      exit=0   []
module list --json    exit=0   []
-V                    exit=0   "apd 10762"  ← 满足 NeoZygisk ≥10762
deploy（无 key）      exit=0   "no key available (use --superkey-file)" 干净失败
```

非 root 身份（`runuser -u nobody`）：

```
root-exec -- id       exit=125    stdout= stderr=   ← 契约要求，且不泄露
auth list --json      exit=125    stdout= stderr=
```

**结论：连接层 schema 与退出码完全符合契约。**

---

## 五、未验证 / 只能说"未证实"的部分

1. **真机 `/data/adb` 上的 SELinux 上下文行为** —— 本环境用实路径测过权限位，
   但没有真机的 `adb_data_file` 标签约束。
2. **`flock` / `rename` 在真机 ext4/f2fs 上的断电一致性** —— fsync 顺序按标准实现，未验证断电。
3. **`sepol.to_file` 在特定机型上的最坏阻塞时长** —— 不可中断，只能靠日志举证。
4. **子系统真实崩溃后的恢复** —— 单测覆盖了策略，但"让 inotify/accept 真的失败"需要设备。
5. **`apd` 动态链接产物无法在 qemu-user 下运行** —— 连接性验证用的是 host 构建，
   两者在**业务逻辑**上同源，但 **Android 特有路径（liblog/SELinux）未经真实环境**。

**真机排查用这几条**：

```bash
adb logcat -s apd | grep -E 'restarting in|GAVE UP|PANICKED|budget exhausted|post-fs-data'
#   restarting in   → 子系统正在恢复（正常）
#   GAVE UP         → 本 boot 该子系统已死，必须处理
#   PANICKED (caught) → 代码 bug，daemon 活着但子系统没了
#   budget exhausted → post-fs-data 预算耗尽，部分模块本 boot 未运行
adb shell 'apd selftest --json' | grep -E 'stage|deploy_status|daemon_subsystems'
#   status=unknown  → daemon 没跑或没发布快照，**不是"健康"**
#   state=gave_up   → 同 GAVE UP
```

---

*本文基于 v2 分支 2026-11-11 的代码与实测。凡标注"实测"的均有真实命令输出支撑；
凡标注"未证实"的一律未做断言。*
