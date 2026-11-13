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
| 本方案 | `engine/android/build_arm64.sh` 用 **NDK r27c clang 18.0.3** 交叉编译同一份 C++ 源码，产出 **AArch64** 两种形态：①静态可执行 `libskrootpatch.so`（2,893,952 B）②JNI 共享库 `libskpatchjni.so`（2,958,000 B）。两者均已实测通过 ELF/契约断言。 |
| 关键坑（已实测） | NDK 静态可执行文件必须用 **`ANDROID_API>=29`** 编译，否则 `PT_TLS p_align=8`，bionic 启动即 abort；API≥29 才输出 `p_align=0x40`。 |
| 无真机验证 | 已建立 `verify_artifact.sh`：Tier A（ELF/契约，qemu-user 可跑，**当前 ALL PASS**）+ Tier B（真内核 Image 的等长/密钥内嵌/hook 落位断言，待样本镜像）。 |
| 推荐集成 | **主形态 JNI（`libskpatchjni.so` + `System.loadLibrary`）**，次形态静态 exec（从 `nativeLibraryDir` 真实路径 exec，**禁止** symlink 到 `filesDir`）。 |
| 实测编译耗时 | 全量 100+ TU，12 核并行 **18 秒**（`make objs`，见 §3.4）。 |
| 未完成 | ①GitHub Actions 实际运行链接（本机无仓库推送凭据，工作流文件已就绪）②样本内核 Image 字节级断言（下载进行中，见 §6.3）。 |

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

**asmjit / capstone 如何编**：两者都是**源码内编**（非预编译库），asmjit 只编 `asmjit/arm/*` + `asmjit/core/*`（**裁掉 x86 后端可省 ~200 KB**，当前 Makefile 仍包含 x86 源，见 §9-R3），capstone 只编 `arch/AArch64` + 公共 `cs.c/MC*.c/SStream.c/utils.c/Mapping.c`。两者与目标架构无关（asmjit 是「生成 arm64 机器码」，capstone 是「反汇编 arm64」），可在任意宿主上交叉编译，实测 0 报错（仅 263 处 `-Wunused-function` 类告警）。

### 3.3 产物与校验（实测）

| 产物 | 路径 | 大小 | sha256（前 32 位） | 类型 |
|---|---|---|---|---|
| 静态可执行 | `engine/dist/libskrootpatch-arm64-exec.so` | **2,893,952 B** | `ddc04731be02c9c79fd50f5ee7e0922d` | `ELF 64-bit LSB executable, ARM aarch64, statically linked, stripped` |
| JNI 共享库 | `engine/dist/libskpatchjni-arm64.so` | **2,958,000 B** | `efc66fede84b8f0634568612cae94da27` | `ELF 64-bit LSB shared object, ARM aarch64, dynamically linked`（NEEDED 仅 `libm.so/libdl.so/libc.so`） |

完整摘要见 `engine/dist/SHA256SUMS`：

```
ddc04731be02c9c79fd50f5ee7e0922d670c64c6a8a8d04714e97469b825e986  libskrootpatch-arm64-exec.so
efc66fede84b8f0634568612cae94da276ea2601646704714c91cdb5ddfd518a  libskpatchjni-arm64.so
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

### 5.1 工作流文件

已放 `engine/android/ci/engine-arm64.yml`（**未**直接放 `.github/workflows/` 以免与 task-8 的 CI 目录撞车；task-8 负责人可原样移动）。核心步骤：

```yaml
name: engine-arm64
on: [push, pull_request, workflow_dispatch]
jobs:
  build-arm64:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: Install NDK r27c
        run: |
          curl -sSL -o /tmp/ndk.zip https://dl.google.com/android/repository/android-ndk-r27c-linux.zip
          mkdir -p "$HOME/ndk" && unzip -q /tmp/ndk.zip -d "$HOME/ndk"
      - name: Install qemu (for contract self-check)
        run: sudo apt-get update -qq && sudo apt-get install -y -qq qemu-user-static
      - name: Build both artifacts
        env: { ANDROID_NDK_HOME: ${{ github.workspace }}/../ndk/android-ndk-r27c }
        run: |
          ANDROID_NDK_HOME="$HOME/ndk/android-ndk-r27c" bash engine/android/build_arm64.sh both
      - name: ELF + contract assertions（防宿主 x86-64 回归）
        run: |
          set -euo pipefail
          E=engine/android/build/arm64/exec/libskrootpatch.so
          J=engine/android/build/arm64/jni/libskpatchjni.so
          file "$E" | grep -q 'ARM aarch64'
          file "$J" | grep -q 'ARM aarch64'
          readelf -h "$E" | grep -q 'AArch64'
          test "$(readelf -lW "$E" | awk '/TLS/{print $NF}')" = "0x40"
          ! readelf -lW "$E" | grep -q INTERP          # 无宿主 glibc 解释器
          ! readelf -dW "$E" | grep -q NEEDED          # 无动态依赖
          test -x "$E"
          bash engine/android/verify_artifact.sh engine/android/build/arm64
      - uses: actions/upload-artifact@v4
        with: { name: skroot-engine-arm64, path: engine/android/build/arm64/**/*.so }
```

### 5.2 证据状态（如实）

- 本机（WSL）**已实测**：编译成功 + `verify_artifact.sh` **ALL PASS**（§3.3/§6.1 日志摘录）。
- GitHub Actions **尚未实际运行**：本机无该仓库的推送凭据/凭证，无法产出 run 链接。工作流文件与断言已就绪，task-8 负责人接入仓库后即可产出运行链接（这属于 task-8 交付面）。
- 因此 G-02 当前可用的证据是「本机交叉编译 + ELF/契约断言 + qemu 执行日志」；CI 链接由 task-8 补齐。

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

### 6.2 Tier B：真修补产物字节级断言（待样本镜像）

设 `KERNEL_IMAGE=<boot.img unpack 出的 arm64 Image>` 后脚本自动断言：

| 断言 | 判据（契约依据） |
|---|---|
| B1 | 引擎 rc=0 |
| B2 | `size(out) == size(in)`（C1 §2.1 字节级 patch 不改大小） |
| B3 | `sha256(out) != sha256(in)`（确有改动） |
| B4 | out 中存在 `key[0:47] + NUL` 窗口（C1 §4-2；与 v1.1 **E1**「47 字符提权串」一致） |
| B5 | `report.json`：`hidden_dir_prefix == key[0:16]`、`hook_table` 非空、in/out size 相等 |
| B6 | NDJSON 终态 `result` 事件**唯一且为最后一行**（C1 §2.3） |

### 6.3 样本内核 Image 获取（进行中）

- **boot.img 解包/重打包工具链**：Magisk 官方 APK 内的 `lib/arm64-v8a/libmagiskboot.so`（**静态链接 arm64，770,632 B**，已实测在 qemu 下可直接运行 `MagiskBoot - Boot Image Modification Tool`）。来源：`https://github.com/topjohnwu/Magisk/releases/download/v28.1/Magisk-v28.1.apk`。
  - 用法（宿主侧验证）：`unzip -j Magisk-v28.1.apk 'lib/arm64-v8a/libmagiskboot.so' -d tools/` → `qemu-aarch64-static tools/libmagiskboot.so unpack boot.img`（产出 `kernel`/`ramdisk.cpio`/`dtb`）。
  - 入库策略：**不**把 APK 入仓；CI 内 `curl` 下载 + sha256 固定 + `unzip`（失败即 fail build），或由 task-8 决定是否直接入库 770 KB 的 magiskboot 二进制。
  - 自建路径（如需完全可控）：`git clone https://github.com/topjohnwu/Magisk` → `native/src/boot` 目录用本仓 NDK 工具链编译（同 NDK r27c + `-static`；约 20 个 C++ TU），CI 内可复现。
- **kptools-android**（备选，KernelPatch 官方 release，294,032 B，**动态 bionic**，需 `/system/bin/linker64`）：已下载可用；在 qemu 下需补 bionic 运行库，故**优先 magiskboot**（静态）。
- **样本镜像**：`samples/` 目录 + `samples/README.md` 记录来源/版本/sha256（见 §9-T2，下载进行中）。

### 6.4 建议补充 Tier C（真机前，qa task-7 可采纳）

静态断言无法覆盖「引擎在真 bionic 环境跑通」，故建议真机前加：①`adb push` 引擎到 `/data/local/tmp` 直接跑 `--help`（验证非 qemu 的真实 bionic 加载）；②用 `/dev/block/by-name/boot` 拷贝出的真 boot.img 走 Tier B。这两项即 G-17 的落地形式。

---

## 7. 与契约对齐（DOC-C1 v1.0 / CALL v1.1）

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

## 8. 包体与体积

| 组合 | 增量 | 评价 |
|---|---|---|
| 仅 JNI 形态 | **2.82 MB** | 推荐（单产物、单调用路径） |
| 仅 exec 形态 | 2.76 MB | 需 legacy packaging + 路径规范 |
| 两形态都带 | 5.58 MB | 仅调试期（排障时 exec 形态可直接抓 stderr） |

优化空间（未实施，列入 §9-R3）：裁掉 asmjit x86 后端（~200 KB）、`-ffunction-sections -Wl,--gc-sections`、`-flto`（预估可降到 ~2.0–2.3 MB）；`capstone` 仅 A64 已裁，`cJSON` 不可省（report）。

---

## 9. 风险与待决

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

## 10. 验收门禁映射（lead 的 10 条 → 本文证据）

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
