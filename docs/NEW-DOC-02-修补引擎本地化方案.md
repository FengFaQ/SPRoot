# NEW-DOC-02 SPRoot 修补引擎本地化方案（SK 引擎 → Android arm64）

- 文档编号：NEW-DOC-02（任务 task-3）
- 作者：patcher（修补组成员）
- 状态：**v1.0 已实测**（本机交叉编译 + qemu 用户态验证通过；CI 运行证据待 GitHub 侧触发）
- 上游契约：`docs/SPROOT-DOC-C1-修补引擎IO契约.md` v1.0 §1–§4；`docs/phase2_skroot_ap_call_contract_v1.1.md` v1.1（E1 47 字符提权串口径）
- 机制依据：`docs/SPROOT-DOC-A1-SKRoot机制深度分析.md`、`docs/research/patch-failure-rootcause.md`（旧链路失败根因）
- 代码位置：`engine/src/`（引擎，未改动机制）、`engine/android/`（本次新增的交叉编译与验证工具链）
- 产物：`engine/dist/`（含 `SHA256SUMS`、`build-info.txt`）

---

## 0. 结论摘要（TL;DR）

| 项 | 结论 |
|---|---|
| 旧病根 | 唯一产物 `engine/src/skroot-patch` 是**宿主 x86-64 glibc 动态 ELF**（`e_machine=0x3E`、带 `PT_INTERP=/lib64/ld-linux-x86-64.so.2`），并且从未进 APK。详见 NEW-DOC-03。 |
| 本方案 | `engine/android/build_arm64.sh` 用 **NDK r27c clang 18.0.3** 交叉编译同一份 C++ 源码（含新增内置 bootimg 处理），产出 **AArch64** 两种形态：①静态可执行 `libskrootpatch.so`（2,909,504 B）②JNI 共享库 `libskpatchjni.so`（2,975,528 B）。两者均通过 ELF/契约断言。 |
| 关键坑（已实测） | NDK 静态可执行文件必须用 **`ANDROID_API>=29`** 编译，否则 `PT_TLS p_align=8`，bionic 启动即 abort；API≥29 才输出 `p_align=0x40`。 |
| 无真机验证 | `verify_artifact.sh`：Tier A（ELF/契约，qemu-user 可跑，**ALL PASS**）+ Tier B（真内核字节级断言）。**已用魅族 21 官方 boot.img 全链实测**：抽 kernel sha256 与 qa ground truth 一致、补丁 rc=0、等长、可确定性复现、重打包等长。 |
| 推荐集成 | **主形态 JNI（`libskpatchjni.so` + `System.loadLibrary`）**，次形态静态 exec（从 `nativeLibraryDir` 真实路径 exec，**禁止** symlink 到 `filesDir`）。 |
| 实测编译耗时 | 全量 100+ TU，12 核并行 **18 秒**（`make objs`，见 §3.4）。 |
| 未完成 | ①`engine.yml` 已在工作区就绪并待推 v2 触发 run（本机不产出交付物，交付物只认 CI artifacts）②AVB/vbmeta 刷入策略待独立子智能体结论后落实现（§7.3）。 |

---

## 1. 现状与缺口

### 1.1 引擎源码树（已确认可编）

```
engine/src/
├── skroot_patch_main.cpp       CLI/参数/NDJSON/report（DOC-C1 实现）
├── patch_pipeline.cpp          五组 hook 编排（A1 §7）
├── analyze/                    kallsyms 解析（3.10~6.18 七套实现）
├── patch_*.cpp                 do_execve/filldir64/compat_filldir/avc_denied/audit_log_start
├── 3rdparty/asmjit2-src/       A64 汇编器（源码内编，4.9 MB）
├── 3rdparty/capstone-5.0.7/    A64 反汇编（源码内编，13 MB）
├── 3rdparty/cJSON/             report JSON
└── Makefile                    宿主 g++ 构建（本次新增 OBJDIR/objs 目标，见 §3.1）
```

### 1.2 缺口（旧链路的直接病灶）

| 缺口 | 证据 |
|---|---|
| 构建脚本无 `--target=aarch64`、无 NDK clang、无 `-static` | 原 `engine/src/Makefile:5-6,25,27`（`CXX := g++` / `-std=c++20 -O2 -Wall` / `LIBS := -ldl -pthread`） |
| 唯一产物是 x86-64 | `engine/src/skroot-patch`：`ELF 64-bit LSB pie executable, x86-64`、`interpreter /lib64/ld-linux-x86-64.so.2` |
| 产物不进 APK | `app/app/src/main/jniLibs` 目录不存在；`app/app/libs/arm64-v8a/` 只有 `libbusybox.so`、`libbootctl.so`；`app/app/build.gradle.kts` 无拷贝/断言任务 |
| 客户端期望名与脚本检查名不一致 | `PatchesViewModel.kt:88-104` symlink 名为 `libskrootpatch.so`.substring(3,len-3) = **`skrootpatch`**，而 `boot_patch.sh:76` 检查 `./skroot-patch` |

→ 修复策略：**产物由本仓构建、进 APK、并在构建期断言架构**（§4/§5），不再依赖「构建期从 GitHub 下载」。

---

## 2. 构建环境探测（本机 WSL，实测记录）

命令：`uname -a`、`dpkg -l | grep gcc-aarch64`、`ls /opt/android*`、`command -v clang cmake ndk-build`。

| 项 | 探测结果 | 影响 |
|---|---|---|
| OS | Ubuntu 24.04.3 LTS（WSL2，kernel 6.18.40.1） | 可作为构建宿主 |
| CPU/内存 | 12 vCPU / 7 GB 可用 | 并行 `-j12` 足够 |
| 主机编译器 | gcc/g++ 13.3（`/usr/bin/gcc`） | 可编宿主版，**不可**直接编 Android |
| clang / cmake / ndk-build / magiskboot | **均 MISSING** | 需自装 NDK（§2.1） |
| Android SDK / NDK / Android Studio | `$ANDROID_HOME`、`$LOCALAPPDATA\Android\Sdk`、`/opt/ndk*` **均不存在** | 无法走 Android Studio/Gradle 原生构建 |
| JDK | `/usr/bin/java`、`javac` 存在（NDK 自带 jni.h，本例未用 JDK） | — |
| 网络 | `github.com` 可达（间歇性超时，21:20 恢复）；`dl.google.com` 200；`android.googlesource.com` 200 | 可下 NDK / 工具链 |
| qemu | `apt install qemu-user-static`（8.2.2）成功 | **无真机也能执行 arm64 产物**做契约自检 |

### 2.1 NDK 获取（本机与 CI 同一步骤）

```bash
# 664 MB，实测约 40 秒（本机带宽）
curl -sSL -o /tmp/ndk-r27c.zip \
  https://dl.google.com/android/repository/android-ndk-r27c-linux.zip
mkdir -p /opt/android-ndk && cd /opt/android-ndk && unzip -q /tmp/ndk-r27c.zip
export ANDROID_NDK_HOME=/opt/android-ndk/android-ndk-r27c
# 校验：clang 18.0.3，target aarch64-linux-android29-clang++
$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin/clang++ --version
```

实测版本串：`Android (12470979, +pgo, +bolt, +lto, +mlgo, based on r522817c) clang version 18.0.3`。

---

## 3. 交叉编译实施（已实测）

### 3.1 engine 源码的三处改动（不触碰修补机制）

| 文件 | 改动 | 目的 |
|---|---|---|
| `engine/src/Makefile` | `CXX/CC/TARGET/OBJDIR` 改 `?=`；`OBJS` 加 `$(OBJDIR)/` 前缀；新增 `objs` 目标 | 支持**树外 obj 目录**，x86-64 与 arm64 的 `.o` 不互相污染；`objs` 供 JNI 形态自定义链接 |
| `engine/src/skroot_patch_main.cpp` | `int main(...)` → **`extern "C" int spr_engine_run(...)`**，文末加 `#ifndef SPROOT_NO_CLI_MAIN` 的 `main()` 转发 | CLI 与 JNI 共用**同一实现**（避免 `-Dmain=...` 造成的 C++ 名字修饰不匹配，JNI 侧可直接链接 `spr_engine_run`） |
| `engine/android/`（新增） | `build_arm64.sh`、`verify_artifact.sh`、`jni/skroot_patch_jni.cpp` | 交叉编译 + 静态验证 + JNI 包装 |

> 宿主构建未回归：`make -C engine/src -j8` 仍产出 `engine/src/skroot-patch`（x86-64，2,133,408 B，2026-11-08 21:18 复测通过）。

### 3.2 编译命令（可复现，实测通过）

```bash
cd engine/android
export ANDROID_NDK_HOME=/opt/android-ndk/android-ndk-r27c
./build_arm64.sh both        # exec + jni 两形态；API 默认 29
```

`build_arm64.sh` 内部（等价命令）：

```bash
NDKB=$ANDROID_NDK_HOME/toolchains/llvm/prebuilt/linux-x86_64/bin
# exec 形态（静态可执行文件，文件名按 Android 打包规范用 lib*.so）
make -C engine/src -j12 \
  OBJDIR=<build>/exec/obj-a29 TARGET=<build>/exec/libskrootpatch.so \
  CXX=$NDKB/aarch64-linux-android29-clang++ CC=$NDKB/aarch64-linux-android29-clang \
  CXXFLAGS="-std=c++2a -O2 -Wall -static -fPIE -DNDEBUG -DCAPSTONE_HAS_ARM64 \
            -DCAPSTONE_USE_SYS_DYN_MEM -I. -I../upstream/SKRoot-linuxKernelRoot/Lite_version/src/patch_kernel_root \
            -I3rdparty/capstone-5.0.7/include -I3rdparty/asmjit2-src/src -I3rdparty -I3rdparty/cJSON" \
  CFLAGS="<同上，C>" LDFLAGS="-static" LIBS=""
llvm-strip --strip-all <build>/exec/libskrootpatch.so

# jni 形态（共享库，静态 libc++，仅依赖系统 libc/libm/libdl）
make -C engine/src objs -j12 OBJDIR=<build>/jni/obj-a29 \
  CXX=$NDKB/aarch64-linux-android29-clang++ CC=$NDKB/aarch64-linux-android29-clang \
  CXXFLAGS="-std=c++2a -O2 -Wall -fPIC -fexceptions -frtti -DSPROOT_NO_CLI_MAIN=1 <同 INCLUDES/DEFINES>"
$NDKB/aarch64-linux-android29-clang++ -std=c++2a -O2 -fPIC -c jni/skroot_patch_jni.cpp -o <obj>/jni_wrapper.o
$NDKB/aarch64-linux-android29-clang++ -shared -static-libstdc++ -o <build>/jni/libskpatchjni.so \
  <obj>/jni_wrapper.o $(find <obj> -name '*.o' ! -name jni_wrapper.o)
llvm-strip --strip-all <build>/jni/libskpatchjni.so
```

**asmjit / capstone 如何编**：两者都是**源码内编**（非预编译库），asmjit 只编 `asmjit/arm/*` + `asmjit/core/*`（**裁掉 x86 后端可省 ~200 KB**，当前 Makefile 仍包含 x86 源，见 §10-R3），capstone 只编 `arch/AArch64` + 公共 `cs.c/MC*.c/SStream.c/utils.c/Mapping.c`。两者与目标架构无关（asmjit 是「生成 arm64 机器码」，capstone 是「反汇编 arm64」），可在任意宿主上交叉编译，实测 0 报错（仅 263 处 `-Wunused-function` 类告警）。

### 3.3 产物与校验（实测）

| 产物 | 路径 | 大小 | sha256（前 32 位） | 类型 |
|---|---|---|---|---|
| 静态可执行 | `engine/dist/libskrootpatch-arm64-exec.so` | **2,909,504 B** | `e94a8186163b77180d7eb8f80ee74ff9` | `ELF 64-bit LSB executable, ARM aarch64, statically linked, stripped` |
| JNI 共享库 | `engine/dist/libskpatchjni-arm64.so` | **2,975,528 B** | `a55c6ed08c37cb25f27e532dd2e16dc9` | `ELF 64-bit LSB shared object, ARM aarch64, dynamically linked`（NEEDED 仅 `libm.so/libdl.so/libc.so`） |

> 注：以上为**本地参考构建**（同一脚本、同一 NDK）。按人类规则「禁止本地编译产物作为交付」，正式交付物只取 CI artifacts（§5）。含 bootimg 功能后体积较首版 +15.5 KB。

完整摘要见 `engine/dist/SHA256SUMS`：

```
e94a8186163b77180d7eb8f80ee74ff9b5464492b8c35bbc1012b3a6c0ce3005  libskrootpatch-arm64-exec.so
a55c6ed08c37cb25f27e532dd2e16dc9f6729592efc8c4876fafc8244310831a  libskpatchjni-arm64.so
```

ELF 证据链（`readelf`）：

```
$ file libskrootpatch-arm64-exec.so
ELF 64-bit LSB executable, ARM aarch64, version 1 (SYSV), statically linked, stripped
$ readelf -h …  Type: EXEC   Machine: AArch64   Entry point: 0x34e480
$ readelf -lW … | grep TLS        →  TLS … R  0x40        # p_align=64（bionic 硬要求）
$ readelf -lW … | grep INTERP     →  (无)                   # 无宿主动态解释器
$ readelf -dW … | grep NEEDED     →  (无)                   # 静态，零动态依赖
```

**TLS 对齐坑（实测数据，务必写进 CI 断言）**：同一份源码，`ANDROID_API` 取 21/24/28 编出的静态可执行文件 `PT_TLS p_align=0x8`，qemu/bionic 启动即 `Aborted`：

```
error: executable's TLS segment is underaligned: alignment is 8 (skew 0),
       needs to be at least 64 for ARM64 Bionic
```

| ANDROID_API | 21 | 24 | 28 | **29** | 30 | 33 | 34 |
|---|---|---|---|---|---|---|---|
| PT_TLS p_align | 0x8 | 0x8 | 0x8 | **0x40** | 0x40 | 0x40 | 0x40 |
| qemu 启动 | abort | abort | abort | **OK** | OK | OK | OK |

→ 结论：**构建脚本默认 `ANDROID_API=29`**，`verify_artifact.sh` 的 A3 断言 `p_align=0x40`。

### 3.4 编译耗时（实测）

全量 100+ TU（引擎 20 + asmjit 57 + capstone 12 + cJSON 1）×12 核并行：**18 秒**（`SECONDS` 计时，树外 obj 目录冷启动）。CI 单次构建成本可忽略。

---

## 4. APK 集成（两种形态 + 推荐）

### 4.1 形态 A（推荐主形态）：JNI 共享库

- 产物：`libskpatchjni-arm64.so` → 放 `newapp/src/main/jniLibs/arm64-v8a/libskpatchjni.so`
- Java 侧：

```kotlin
object NativeEngine {
    init { System.loadLibrary("skpatchjni") }
    // 返回 DOC-C1 §3 退出码；进度从 jsonSink（NDJSON）轮询，密钥从 <out>.report.json 读
    external fun patch(inPath: String, outPath: String, key: String?, jsonSink: String?, workdir: String?): Int
    external fun engineVersion(): String
}
```

- JNI 签名（已实现，见 `engine/android/jni/skroot_patch_jni.cpp`）：
  `Java_me_sproot_patch_NativeEngine_patch(JNIEnv*, jclass, jstring in, jstring out, jstring key?, jstring jsonSink?, jstring workdir?) → jint`
- 优点：**完全绕过 exec 限制**（Android 10+ `untrusted_app` 对 app 私有目录 executables 的 W^X/app_data_file 约束不适用）；一次 `System.loadLibrary` 即可调用；无 shell/busybox 依赖。
- 代价：引擎崩溃（asmjit/capstone 断错）会连带 App 进程；建议在 `Thread` + `try/catch` 中调用，并把「引擎进程级崩溃」纳入验收（见 §6 Tier C 建议）。

### 4.2 形态 B（备选/排障）：静态可执行文件

- 产物：`libskrootpatch-arm64-exec.so` → 放 `jniLibs/arm64-v8a/libskrootpatch.so`
- 必须满足三条，否则 ENOEXEC/EACCES：
  1. `android { packaging { jniLibs { useLegacyPackaging = true } } }`（或 `android:extractNativeLibs="true"`），让文件被解压到 `nativeLibraryDir` 且带可执行位（旧工程已有此配置 `app/app/build.gradle.kts:129-131`）；
  2. **直接 exec `nativeLibraryDir` 下的真实路径**：`ProcessBuilder("$nativeLibraryDir/libskrootpatch.so", "--in", …)`；
  3. **禁止** symlink 到 `filesDir` 后再 exec（旧链路 `PatchesViewModel.kt:75,103` 的做法；symlink 目标虽在 nativeLibraryDir，但 cwd/argv[0] 在 app 私有目录，且 SELinux 判定按路径与域，实测风险高 —— 见 NEW-DOC-03 R2）。
- 优点：崩溃隔离（进程崩了 App 还在）、便于抓 stderr、便于外部/root 复用；缺点：多一层 exec 约束与路径规范。

### 4.3 Gradle 打包任务（必须带断言，禁止再次「静默缺件」）

```kotlin
val copyEngine by tasks.registering(Copy::class) {
    from(rootProject.file("engine/dist/libskpatchjni-arm64.so"))
    into(layout.projectDirectory.dir("src/main/jniLibs/arm64-v8a"))
    rename { "libskpatchjni.so" }
}
val assertEngine by tasks.registering {
    val f = file("src/main/jniLibs/arm64-v8a/libskpatchjni.so")
    doLast {
        require(f.isFile) { "引擎缺失：${f.absolutePath}（先跑 engine/android/build_arm64.sh）" }
        val bytes = f.readBytes()
        // ELF e_machine 在 0x12，AArch64 = 0xB7；0x3E 是 x86-64（旧病）
        val machine = (bytes[0x12].toInt() and 0xff) or ((bytes[0x13].toInt() and 0xff) shl 8)
        require(machine == 0xB7) { "引擎架构错误：e_machine=0x${machine.toString(16)}，必须 0xB7(AArch64)" }
        require(bytes.size in 1_500_000..6_000_000) { "引擎体积异常：${bytes.size} B" }
    }
}
tasks.named("preBuild") { dependsOn(copyEngine, assertEngine) }
```

> 与旧工程的区别：`assertEngine` **硬失败**（`require` 抛异常 → 构建中断），不再有「下载失败静默跳过 → 半残 APK」的路径（老 `app/app/build.gradle.kts:243-301` 的 `registerDownloadTask` 就是这种模式）。

---

## 5. CI 交叉编译（GitHub Actions，工作流已就绪）

### 5.1 工作流文件（已就绪）

工作流：**`.github/workflows/engine.yml`**（`name: engine-arm64`），步骤：

| 步骤 | 内容 |
|---|---|
| Install NDK r27c | 从 `dl.google.com` 取 `android-ndk-r27c-linux.zip` → `$HOME/ndk/android-ndk-r27c`，导出 `ANDROID_NDK_HOME` |
| Install qemu | `apt-get install qemu-user-static`（契约运行时自检） |
| Build | `bash engine/android/build_arm64.sh both`（exec + JNI 两形态） |
| ELF 断言（防宿主 x86-64 回归） | `file` 含 `ARM aarch64`、`readelf -h` 含 `AArch64`、可执行位；exec 无 `PT_INTERP`/无 `NEEDED`；`PT_TLS=0x40`；JNI 仅系统库 |
| 契约断言 | qemu 直跑 arm64 产物：`--help` rc=0、缺参 rc=101、输入不存在 rc=102、**rc≠0 时不得产出 `--out`**（C1 §3） |
| Bootimg 回归 | 若仓内有 `samples/meizu21/boot.img`：`--bootimg info/extract-kernel/repack` 全链 + kernel sha256 核对 |
| 归档 | `actions/upload-artifact@v4`（含 `SHA256SUMS`），产物名 `skroot-engine-arm64` |

### 5.2 证据状态（如实）

| 证据 | 状态 |
|---|---|
| 本地等价 CI 构建（同脚本/同 NDK/同断言） | ✅ 通过（§6.1 ALL PASS） |
| 本地 qemu 跑**真实内核**（arm64 产物） | ✅ 通过：33,892,864 B 真内核 `rc=0`，耗时 226 s，产物 sha256 与宿主构建**完全一致**（跨架构确定性，§6.2 B9） |
| GitHub Actions 运行链接 | ⏳ 工作流待推 `v2` 分支后产生 run 链接（人类规则：交付物只认 CI artifacts，本地不产出交付件） |

---

## 6. 无真机静态验证方法（已实现为脚本）

脚本：`engine/android/verify_artifact.sh [build/arm64]`；设 `KERNEL_IMAGE=<arm64 Image>` 追加 Tier B。逐条 PASS/FAIL，退出码 0/1，CI 可直接判定。

### 6.1 Tier A：ELF 形态 + 契约行为（**当前 ALL PASS**）

| 断言 | 判据 | 参照 |
|---|---|---|
| A1 架构 | `readelf -h` → `Machine: AArch64`（0xB7） | 根治旧病「x86-64 0x3E」 |
| A2 静态化 | 无 `PT_INTERP`、无 `NEEDED` | 不依赖设备 libc 版本 |
| A3 TLS 对齐 | `PT_TLS p_align=0x40` | §3.2 实测坑 |
| A4 体积/摘要/可执行位 | 记录 sha256、`chmod 0755` | 包体预算与追溯 |
| A5 JNI 形态 | `DYN`、导出 `Java_..._patch`、`spr_engine_run` 已解析、仅系统库依赖 | §4.1 |
| A6 契约退出码 | `--help` rc=0；缺参 rc=101；输入不存在 rc=102；`--key short` rc=101；`.img` 输入 rc=102 | C1 §1/§3 |
| A6d 铁律 | rc≠0 时 `--out` **不存在** | C1 §3 行为铁律 |

实测日志摘录（qemu-user，2026-11-08）：

```
PASS  A1 引擎 e_machine=AArch64（AArch64）
PASS  A2 引擎无 PT_INTERP（静态）      PASS  A2b 引擎无 NEEDED 依赖
PASS  A3 PT_TLS p_align=0x40（bionic arm64 要求 >=64）
PASS  A4 大小 2893952 B sha256=ddc04731be02c9c7…
PASS  A6 --help rc=0 且打印版本 banner
PASS  A6b 缺参数 rc=101 (E_USAGE)      PASS  A6c 输入不存在 rc=102 (E_INPUT)
PASS  A6d 失败不落盘铁律：rc!=0 时 --out 不存在
PASS  A6e 密钥格式非法 rc=101          PASS  A6f 拒绝 .img 明文输入 rc=102
RESULT: ALL PASS
```

`--help` 原始输出（qemu 直跑 arm64 静态产物）：

```
SPRoot patch engine v1.0.0 (contract SPROOT-DOC-C1 v1.0)
usage: skroot-patch --in <kernel_image> --out <patched_kernel> (--gen-key|--key <48ch>)
       [--json <ndjson_sink>] [--workdir <dir>] [--quiet]
```

### 6.2 Tier B：真修补产物字节级断言（**已用魅族 21 官方镜像实测通过**）

样本：`samples/meizu21/boot.img`（100,663,296 B，sha256 `8c899eb3…`，来源 `MEIZU21_12.7` 官方固件）。

```bash
E=engine/src/skroot-patch     # arm64 形态：qemu-aarch64-static engine/android/build/arm64/exec/libskrootpatch.so
$E --bootimg info           --in samples/meizu21/boot.img
$E --bootimg extract-kernel --in samples/meizu21/boot.img --out kernel.raw
$E --in kernel.raw --out kernel.patched --key <48字符> --json ev.ndjson --workdir w
$E --bootimg repack         --in samples/meizu21/boot.img --kernel kernel.patched --out new-boot.img
```

| 断言 | 实测结果 |
|---|---|
| B0 解包字段 | `header_version=4`、`header_size=1584(0x630)`、`page_size=4096`、`kernel offset=4096(0x1000)`、`kernel_size=33,892,864`、`compression=raw` |
| B0b **交叉验证** | 抽出 kernel sha256 = `4d445384c046f2ba2de856b3215de8d2be83a9ccad115948a34471cde3098481`，与 qa ground truth **逐字节一致** |
| B1 | 补丁 rc=0（`hook_count=19`），`kernel_version=6.1.25`（真实解析，非占位） |
| B2 | 等长：`33,892,864 B` |
| B3 | 输出 sha256 = `399ac4797e6ab2ae5700cc21350676daf7b89fe397e473ef24ae32cb9cce3f2f` ≠ 输入 |
| B4 | 产物含 `key[0:47]+NUL` 窗口（47 字符提权串，CALL v1.1 **E1**） |
| B5 | `report.json`：`input/output.sha256+size` 齐全且等长、`hidden_dir_prefix==key[0:16]`、`hook_table` 为数组（含 `blob_count/root_key_region`）、`kernel_version=6.1.25`、`errors=[]` |
| B6 | NDJSON 终态 `result` 唯一且为最后一行 |
| B7 重打包 | `new-boot.img` **等长**（100,663,296 B）；kernel 段 == `kernel.patched`；差异只落在 kernel 段 |
| B8 确定性 | 同输入同密钥两次运行产物 sha256 相同 |
| B9 跨架构 | arm64 产物（qemu）与宿主 x86-64 产物输出**同一 sha256** |

> 附带修复（C1 §2.3 契约偏差，qa 会判）：①`report.json` 原缺 `output.size/output.sha256`；②`hook_table` 被 `cJSON*`→`bool` 重载误选写成 `true`；③`kernel_version` 恒 `unknown`。已分别补 `engine/src/spr_sha256.h`（自实现 SHA-256，无第三方依赖）、修正 JSON 序列化、接 `KernelVersionParser`。

### 6.3 样本与解包工具链（来源：官方固件，非公开 GKI）

| 项 | 结论 |
|---|---|
| 样本来源 | **魅族 21 官方固件 `/mnt/c/MEIZU21_12.7` 解包**（人类提供）；清单 `samples/meizu21/SAMPLES.csv`：`boot.img`、`init_boot.img`（ramdisk 形态：kernel_size=0、ramdisk=2,131,370）、`vbmeta.img`（12,288 B，`AVB0`）。**不使用公开 GKI 样本**；本地已按人类指示删除 GKI 下载（1.5 GB 已清理） |
| boot.img 形态 | header **v4**，kernel **未压缩 raw arm64 Image**（首字节 `MZ`），`ramdisk_size=0` → 无解压/重压缩风险，SK 补丁天然等长 |
| 解包/重打包 | **引擎内置**（`engine/src/bootimg.cpp` + `--bootimg` 子命令），不依赖 magiskboot/kptools → §7 |
| magiskboot 备选 | Magisk APK 内 `lib/arm64-v8a/libmagiskboot.so` 为静态 arm64 可执行（770,632 B），实测 qemu 可运行；仅在遇到 v2 header / vendor_boot / 压缩内核时启用，不默认引入 |

### 6.4 建议补充 Tier C（真机前，qa task-7 可采纳）

静态断言无法覆盖「引擎在真 bionic 环境跑通」，故建议真机前加：①`adb push` 引擎到 `/data/local/tmp` 直接跑 `--help`（验证非 qemu 的真实 bionic 加载）；②用 `/dev/block/by-name/boot` 拷贝出的真 boot.img 走 Tier B。这两项即 G-17 的落地形式。

---

## 7. bootimg 解包/重打包与刷入链（AVB）

### 7.1 为什么自己做（不引 magiskboot/kptools）

旧链路三个阻断项里有两条是"外部件"：`libkptools.so` 构建期下载、`kpimg` 资产硬依赖（NEW-DOC-03 R3/R4）。KernelSU 的 `ksud` 用自研 bootimg 解析证明了"完全不依赖 magiskboot 也可行"。SPRoot 更简单：**SK 只 patch kernel 段，不改 ramdisk**（无需 KSU 式 init→init.real 替换），所以自研成本很低。

### 7.2 已实现（`engine/src/bootimg.{h,cpp}`，随引擎编进 arm64 产物）

| 子命令 | 作用 | 实测（魅族 21 boot.img） |
|---|---|---|
| `--bootimg info --in boot.img` | 打印 header/kernel/ramdisk/压缩/AVB 全字段 | ✅ v4 / 1584 / 4096 / kernel@4096 33,892,864 raw |
| `--bootimg extract-kernel --in boot.img --out kernel` | 抽出 kernel Image（拒绝压缩内核并说明如何解压） | ✅ sha256 与 qa ground truth 一致 |
| `--bootimg unpack --in boot.img --outdir DIR` | 产出 `DIR/kernel` + `DIR/bootimg.info.txt` | ✅ |
| `--bootimg repack --in boot.img --kernel patched --out new-boot.img [--strip-avb]` | 等长回填 kernel 段；`--strip-avb` 截断 AVB 尾部区 | ✅ 等长 100,663,296 B |

约束与安全边界：
- **只接受等长 kernel**（SK 补丁等长；不等长直接 E240 拒绝落盘，防"半成品写进 boot"）。
- 压缩内核（gzip/lz4）**明确报错**而非猜测处理；样本机为 raw，故 MVP 不受影响。后续若目标机为压缩内核，vendor 一个 inflate/lz4 单文件实现即可（CI 内可测）。
- 不处理 ramdisk/dtb/vendor_boot/v4 boot signature（SK 路线不需要）。

### 7.3 AVB 与刷入链（**MVP 开机关键路径**）

实测解析（`bootimg info` + 手工核对）：

```
boot.img:      header v4, kernel@0x1000 (33,892,864 raw), ramdisk_size=0
尾部:          AVBf footer @ 100,663,232（镜像末 64B，大端）
               original_image_size = 33,898,496   ← 有效镜像长度（header 页 + kernel pages）
               vbmeta_offset      = 33,898,496   ← 内嵌 vbmeta 紧随其后
               vbmeta_size        = 2,496
文件长度 100,663,296 = fastboot 分区尺寸（其余为 0 填充）
另有独立 vbmeta.img 12,288 B（magic AVB0，alg=SHA256_RSA2048）→ 顶层签名链
```

结论（**先验推断，待独立子智能体源码核实与真机验证**）：
1. **修补必然破坏 AVB**：kernel 字节变化 → boot 内嵌 vbmeta 的 hash 失配 → 顶层 `vbmeta.img` 链式校验失败。**直接 `fastboot flash boot` 会触发验证失败/回滚或卡开机**。
2. **主路径（推荐）**：关闭校验后再刷：
   `fastboot --disable-verity --disable-verification flash vbmeta vbmeta.img`（官方 vbmeta 只改 flags：`AVB_VBMETA_IMAGE_FLAGS_HASHTREE_DISABLED=1 | VERIFICATION_DISABLED=2`），随后 `fastboot flash --slot=all boot new-boot.img`（A/B 机型必须 `--slot=all`，与官方 `flash.bat` 一致）。
3. **备选路径**：`--bootimg repack --strip-avb` 去掉 boot 的 AVBf footer/内嵌 vbmeta（输出 33,898,496 B）后刷入；适用于 boot 走"独立校验被跳过"的机型。两条路径**都必须真机实测能否开机**，不能开机则记录阻断并用官方全量包回滚（`flash.bat.reference` 可全量回滚）。
4. **回滚保障**：刷入前把原 boot.img 落盘并按 **sha1 内容寻址**保存（学 KSU），失败可秒回官方原镜像。
5. 风险提示：`--disable-verification` 属降级安全态（设备可能显示警告/影响 OTA），需产品侧明示用户。

> ⏳ 待办：AVB 专项结论（独立子智能体）同步后，把"是否必须 vbmeta 关闭/可否只刷 boot"的最终判定回写本节，并在 `bootimg` 里补 `--patch-vbmeta-flags` 子命令（直接产出 flags 已改的 vbmeta.img，去掉对 `fastboot --disable-*` 的依赖）。

## 8. 与契约对齐（DOC-C1 v1.0 / CALL v1.1）

| 契约项 | 实现/证据 | 状态 |
|---|---|---|
| §1 CLI 全参数、零交互 | `skroot_patch_main.cpp:154-190`（`--in/--out/--key/--gen-key/--json/--workdir/--quiet`）；A6 实测 | ✅ |
| §1 拒绝 `.img/.gz` | `main.cpp` 扩展名检查 → rc=102；A6f 实测 | ✅ |
| §2.1 等长产物 | 写回逻辑基于输入 `file_buf` 拷贝；Tier B2 断言待样本 | ⏳ |
| §2.2 密钥三处明文（stdout 末行 / NDJSON result / report.json） | `main.cpp` `ROOT_KEY=` 兜底行 + `emit_result` + `build_report`；A6c 实测输出含 `ROOT_KEY=` | ✅ |
| §2.3 report schema | `build_report`（engine_version/input/output/hook_table/hidden_dir_prefix…） | ✅（Tier B5 待样本验证） |
| §3 退出码 11 枚举 | `ExitCode` enum；A6b/A6c/A6e 实测 101/102 | ✅ |
| §3 失败不落盘铁律 | `fail()` 内 `fs::remove(opt.out_path)`；A6d 实测 | ✅ |
| §4-2 密钥内嵌（47+NUL） | `main.cpp` 写入 `key[0..46] + NUL`；跳板逐字节 cmp（`patch_do_execve.cpp:89-102`）；Tier B4 | ✅ 实现 / ⏳ 样本断言 |
| **CALL v1.1 E1 提权串=47 字符** | 内核侧第 48 字节恒为 NUL → 用户态必须传 47 字符 NUL 串（48 字符必失配） | ✅ 已会签 |
| CALL v1.1 E5 取消独立部署器 | 本方案只交付引擎两形态，**无** `skroot-deploy` | ✅ |
| CALL v1.1 §8 root-exec 依赖 `--superkey-file` | 与引擎无关（属 apd/守护面，task-6） | n/a |

---

## 9. 包体与体积

| 组合 | 增量 | 评价 |
|---|---|---|
| 仅 JNI 形态 | **2.82 MB** | 推荐（单产物、单调用路径） |
| 仅 exec 形态 | 2.76 MB | 需 legacy packaging + 路径规范 |
| 两形态都带 | 5.58 MB | 仅调试期（排障时 exec 形态可直接抓 stderr） |

优化空间（未实施，列入 §10-R3）：裁掉 asmjit x86 后端（~200 KB）、`-ffunction-sections -Wl,--gc-sections`、`-flto`（预估可降到 ~2.0–2.3 MB）；`capstone` 仅 A64 已裁，`cJSON` 不可省（report）。

---

## 10. 风险与待决

| # | 风险/待决 | 影响 | 建议 |
|---|---|---|---|
| R1 | CI 运行证据缺失（本机无推送凭据） | G-02 仅「本机实测」级 | task-8 接入后跑 `engine-arm64.yml`，把 run 链接补进本文 §5.2 |
| R2 | 样本镜像未就位 | Tier B（等长/密钥/hook 落位）无法断言 | 下载 GKI/原厂 boot.img → `samples/` + sha256（进行中）；魅族 21 原厂镜像由 lead 向人类取 |
| R3 | 体积未优化、asmjit 含 x86 后端 | APK 多 ~0.6–0.8 MB | 后续单独 PR 做 `--gc-sections`/裁剪，需回归 Tier A/B |
| R4 | JNI 形态崩溃会拖垮 App 进程 | 稳定性 | 独立线程 + 崩溃上报；或默认走 exec 形态 |
| R5 | 引擎当前在 arm64 上**只做过 qemu 验证**，未在真 bionic 运行 | 真机首跑可能踩 bionic 差异 | 真机前置步骤见 §6.4 Tier C；建议 qa 把「`/data/local/tmp` 直跑 `--help`」列为门禁 |
| R6 | `kptools unpack` 是否输出**已解压** Image 未确认 | 引擎拒绝 `.gz`（rc=102） | 真机/样本阶段确认；必要时在 App 侧加 `gzip -d`/lz4 解压步骤 |
| R7 | NDK 版本漂移 | 产物不一致 | 固定 r27c + `SHA256SUMS` + CI 断言 |

---

## 11. 验收门禁映射（lead 的 10 条 → 本文证据）

| 门禁 | 本文落点 |
|---|---|
| ① 入口能力自检（存在+可执行+ELF AArch64） | §4.3 `assertEngine`；§6.1 A1/A4b |
| ② 引擎 aarch64 .so+JNI、打包任务+CI 断言 | §3.3、§4.1/§4.3、§5.1 |
| ③ 禁止构建期网络下载当交付 | §4.3（构建期断言，不再 `registerDownloadTask`）；magiskboot 下载带 sha256 且失败即 fail |
| ④ kpimg 降级可选 | 引擎不依赖 kpimg（`boot_patch.sh` 新版已无 `-k kpimg`） |
| ⑤ 日志落盘、失败后 UI 常驻 | App 侧（task-4/5）；引擎提供 `--json` NDJSON + 退出码 + report |
| ⑥ 脚本禁 `echo; exit $?` | 新链取消 shell 脚本层（引擎进程内/直 exec） |
| ⑦ 成功须校验产物存在+回读 patched | §6.2 B2/B3/B5；App 侧再断言「out 存在且 `report.patched`」 |
| ⑧ 密钥不绑 DEBUG | 引擎 `--gen-key` 产出（与构建类型无关） |
| ⑨ 不在 filesDir exec ELF | §4.1（JNI 主形态）、§4.2（exec 必须 nativeLibraryDir 真实路径） |
| ⑩ 单一调用路径、不 `sh -c` 拼串 | §4.1 JNI 直接调用；exec 形态 `ProcessBuilder(listOf(...))` |

---

## 附录 A：本次新增/修改文件清单

| 文件 | 说明 |
|---|---|
| `engine/src/Makefile` | `?=` 化 + `OBJDIR` 树外目标文件 + `objs` 目标（宿主构建行为不变） |
| `engine/src/skroot_patch_main.cpp` | 入口改 `extern "C" spr_engine_run()`，`main()` 转发（`SPROOT_NO_CLI_MAIN` 可裁） |
| `engine/android/build_arm64.sh` | arm64 交叉编译（exec / jni / both） |
| `engine/android/verify_artifact.sh` | Tier A/B 静态验证（qemu 契约自检） |
| `engine/android/jni/skroot_patch_jni.cpp` | JNI 入口 `NativeEngine.patch/engineVersion` |
| `engine/android/ci/engine-arm64.yml` | GitHub Actions 工作流（待 task-8 接入 `.github/workflows/`） |
| `engine/dist/` | 产物 + `SHA256SUMS` + `build-info.txt` |
| `samples/` | 样本内核镜像 + 来源/sha256 说明（进行中） |

## 附录 B：复现清单（从零到证据）

```bash
# 1) 工具链
curl -sSL -o /tmp/ndk.zip https://dl.google.com/android/repository/android-ndk-r27c-linux.zip
mkdir -p /opt/android-ndk && unzip -q /tmp/ndk.zip -d /opt/android-ndk
sudo apt-get install -y qemu-user-static

# 2) 构建
export ANDROID_NDK_HOME=/opt/android-ndk/android-ndk-r27c
bash engine/android/build_arm64.sh both          # ≈18s / 12 核

# 3) 断言（ALL PASS 才算过）
bash engine/android/verify_artifact.sh

# 4) 真镜像断言（有样本后）
KERNEL_IMAGE=samples/gki-kernel-arm64.Image bash engine/android/verify_artifact.sh
```

## 12. 许可证与合规（D-3）

| 组件 | 位置 | 许可证 | 结论 |
|---|---|---|---|
| **SKRoot（引擎核心来源）** | `upstream/SKRoot-linuxKernelRoot/`（`Lite_version/src/patch_kernel_root/*` 被 vendor 进 `engine/src/analyze|patch_*.cpp`） | **未声明：仓库无 LICENSE/COPYING，源文件无版权头** → 默认 **All rights reserved（proprietary）**，SPDX 无对应标识 | 🔴 **风险**：无授权即不可再分发/再许可。`engine/` 作为其衍生作品，**发布任何二进制（APK/引擎）前必须取得上游书面许可**，或对 core 做 clean-room 重写，或本项目仅内部使用不发布 |
| KernelPatch | `upstream/KernelPatch/LICENSE` | **GPL-2.0** | 我们已不 vendor 其源码、也不再调用 kptools（bootimg 自研），仅历史参考 → **不引入传染** |
| asmjit | `engine/src/3rdparty/asmjit2-src/LICENSE.md` | **Zlib**（改写自 MIT 风格，宽松） | ✅ 兼容闭源/开源分发，保留版权声明即可 |
| capstone | `engine/src/3rdparty/capstone-5.0.7/`（仅 include/ + src/，未附 LICENSE） | 上游 **BSD-3-Clause** | ✅ 宽松兼容；**行动项**：把上游 LICENSE.TXT/copyright 一并入库（避免"未附声明"的合规瑕疵） |
| cJSON | `engine/src/3rdparty/cJSON/`（仅 .cpp/.h，未附 LICENSE） | 上游 **MIT** | ✅ 兼容；**行动项**：补 LICENSE 文本入库 |
| 旧管理器 App（APatch 衍生） | `app/LICENSE` | **GPL-3.0** | 仅作历史参考/根因分析对象；新 App **不继承**其代码则不受 GPL-3 约束（若复用其 UI/代码则需整体 GPL-3 开源，需 architect 拍板） |

**结论（可执行）**：
1. asmjit(Zlib)/capstone(BSD-3)/cJSON(MIT) 与我们自研代码兼容，可分发；只需补齐第三方位 LICENSE 文本。
2. **真正的合规风险只有 SKRoot 核心算法（无许可证）**：这是 D-3 的阻塞点，需人类与上游作者确认授权，或走 clean-room。
3. 新 App 若从 APatch(GPL-3) 复制代码 → 必须整体 GPL-3 开源；纯参考设计不受限。
