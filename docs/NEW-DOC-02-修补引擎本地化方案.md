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
| 本方案 | `engine/android/build_arm64.sh` 用 **NDK r27c clang 18.0.3** 交叉编译同一份 C++ 源码（含新增内置 bootimg 处理），产出 **AArch64** 两种形态：①静态可执行 `skroot-patch`（**2,950,880 B**，工具/排障用，**不进 APK**）②**JNI 共享库 `libskrootpatch.so`**（**3,018,552 B**，`System.loadLibrary("skrootpatch")`，进 `jniLibs`）。两者均通过 ELF/契约断言。 |
| 关键坑（已实测） | NDK 静态可执行文件必须用 **`ANDROID_API>=29`** 编译，否则 `PT_TLS p_align=8`，bionic 启动即 abort；API≥29 才输出 `p_align=0x40`。 |
| 无真机验证 | `verify_artifact.sh`：Tier A（ELF/契约，qemu-user 可跑，**ALL PASS**）+ Tier B（真内核字节级断言）。**已用魅族 21 官方 boot.img 全链实测**：抽 kernel sha256 与 qa ground truth 一致、补丁 rc=0、等长、可确定性复现、重打包等长。 |
| **`--workdir` 所有权契约（2026-11-09 真机事故后）** | **显式传入 `--workdir` ⇒ 目录及内容生命周期归调用方，引擎不得删除其中任何内容；只有默认 workdir（`<out>.work`）由引擎成功收尾时回收**（§8.1）。行为证据：A9d `--self-test-workdir` 13 例矩阵（CI 必跑）+ Tier B B9 事故现场回归 / B10 默认回收正控。 |
| **密钥 ABI 契约（P0，提权链）** | CLI `--key` **严格 48 字符**（47/49/非 alnum → E101）；内核只写 **`key[0:47]` + NUL = 48 字节**（`embed_key_bytes()`，内联断言）；`report.key_spec` 输出 `abi="47+nul"/bytes=48/last_byte_is_nul=true/input_len=48`（§7.5）。行为证据：A10 `--self-test-keyabi` 17 例（CI 必跑）+ Tier B B7。**用户态提权串必须 47 字符**。 |
| **用户态 daemon 启动方式（2026-11-10 评估）** | SK 引擎**无任何内核态拉起用户态的能力**（`call_usermodehelper`/`kernel_thread`/`kthread_run`/`init_task` 全零命中）；KSU 亦需改 ramdisk/init_boot，APatch 靠常驻内核运行时做 init.rc 虚拟化才不需要。**选型：A（App 自引导）先落地 → B（提权时内核顺带拉起）作兜底 → C（init hook + umh）不建议**。详见 §9。 |
| 推荐集成 | **主形态 JNI（`libskrootpatch.so` + `System.loadLibrary("skrootpatch")`）**，次形态静态 exec（`skroot-patch` 从 `nativeLibraryDir` 真实路径 exec，**禁止** symlink 到 `filesDir`；该可执行件默认不进 APK）。 |
| 实测编译耗时 | 全量 100+ TU，12 核并行 **18 秒**（`make objs`，见 §3.4）。 |
| 打包工具 | **magiskboot v31.1 ARM64**（SKRoot 官方路径；988,984 B，静态 AArch64，sha256 `97e9ec75…bb54`），CI 内固定版本+哈希校验并随 artifact 分发；自研 bootimg 保留为交叉校验/回退（§7）。**禁止 AIK**。 |
| CI 证据 | ✅ **最新 run 42635720094**（engine-arm64, v2, 907f5ad）**success**，artifact `skroot-engine-arm64`（2,421,354 B）：`libskrootpatch.so` sha256 `62955d8f…82fd1` / 3,018,552 B，`skroot-patch` sha256 `29908b1c…a8052` / 2,950,880 B；Tier A 全绿含 **A6g/A6h/A9d/A10**（workdir 所有权 + 密钥 ABI 行为断言，CI 必跑）。历史 run 42486757913 / 42487185268 见 §5.2。 |
| AVB/刷入 | 主路径 = **只刷 boot**（`--slot=all`），**禁止**动 vbmeta / `--disable-verification`；产物须严格等于分区大小且逐字节保留内嵌 vbmeta+footer（§7.4） |

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
$NDKB/aarch64-linux-android29-clang++ -shared -static-libstdc++ -Wl,-soname,libskrootpatch.so -o <build>/jni/libskrootpatch.so \
  <obj>/jni_wrapper.o $(find <obj> -name '*.o' ! -name jni_wrapper.o)
llvm-strip --strip-all <build>/jni/libskrootpatch.so
```

**asmjit / capstone 如何编**：两者都是**源码内编**（非预编译库），asmjit 只编 `asmjit/arm/*` + `asmjit/core/*`（**裁掉 x86 后端可省 ~200 KB**，当前 Makefile 仍包含 x86 源，见 §11-R3），capstone 只编 `arch/AArch64` + 公共 `cs.c/MC*.c/SStream.c/utils.c/Mapping.c`。两者与目标架构无关（asmjit 是「生成 arm64 机器码」，capstone 是「反汇编 arm64」），可在任意宿主上交叉编译，实测 0 报错（仅 263 处 `-Wunused-function` 类告警）。

### 3.3 产物与校验（实测）

| 产物 | 路径 | 大小 | sha256（前 32 位） | 类型 |
|---|---|---|---|---|
| 静态可执行（工具/排障，**不进 APK**） | `engine/dist/skroot-patch-arm64`（源码产物 `build/arm64/exec/skroot-patch`） | **2,950,880 B** | `29908b1c5aeef254…` | `ELF 64-bit LSB executable, ARM aarch64, statically linked, stripped` |
| **JNI 共享库（APK 内 `libskrootpatch.so`）** | `engine/dist/libskrootpatch-arm64-jni.so`（源码产物 `build/arm64/jni/libskrootpatch.so`） | **3,018,552 B** | `62955d8fedab5c24…` | `ELF 64-bit LSB shared object, ARM aarch64, dynamically linked`，`SONAME=libskrootpatch.so`，NEEDED 仅 `libm.so/libdl.so/libc.so`，导出 `Java_me_sproot_patch_NativeEngine_patch/engineVersion` |

> **命名约定（与 app-client 对齐，重要）**：APK `jniLibs/arm64-v8a/libskrootpatch.so` 必须是**JNI 共享库**（ET_DYN，`System.loadLibrary("skrootpatch")`）；独立可执行文件改名 `skroot-patch`（裸名），**绝不放进 jniLibs**，避免"名字像库实为 ET_EXEC、loadLibrary 必崩"的混淆。

> 注：以上为**本地参考构建**（同一脚本、同一 NDK）。按人类规则「禁止本地编译产物作为交付」，正式交付物只取 CI artifacts（§5）。含 bootimg 功能后体积较首版 +15.5 KB。

完整摘要（**以 CI artifact 为准**，run 42635720094 / 907f5ad；本地参考构建与 CI 逐字节一致）：

```
29908b1c5aeef2544da15296fdd72a6e9e2cf8d3b524eb278717e9beff0a8052  skroot-patch            (2,950,880 B)
62955d8fedab5c24ae87b9472c6c5506aab55efe7d57964b03da4c54a1382fd1  libskrootpatch.so       (3,018,552 B)
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

- 产物：`libskrootpatch.so` → 放 `newapp/src/main/jniLibs/arm64-v8a/libskrootpatch.so`
- Java 侧：

```kotlin
object NativeEngine {
    init { System.loadLibrary("skrootpatch") }
    // 返回 DOC-C1 §3 退出码；进度从 jsonSink（NDJSON）轮询，密钥从 <out>.report.json 读
    external fun patch(inPath: String, outPath: String, key: String?, jsonSink: String?, workdir: String?): Int
    external fun engineVersion(): String
}
```

- JNI 签名（已实现，见 `engine/android/jni/skroot_patch_jni.cpp`）：
  `Java_me_sproot_patch_NativeEngine_patch(JNIEnv*, jclass, jstring in, jstring out, jstring key?, jstring jsonSink?, jstring workdir?) → jint`
- 优点：**完全绕过 exec 限制**（Android 10+ `untrusted_app` 对 app 私有目录 executables 的 W^X/app_data_file 约束不适用）；一次 `System.loadLibrary` 即可调用；无 shell/busybox 依赖。
- 代价：引擎崩溃（asmjit/capstone 断错）会连带 App 进程；建议在 `Thread` + `try/catch` 中调用，并把「引擎进程级崩溃」纳入验收（见 §6 Tier C 建议）。

#### 4.1.1 JNI ABI 契约（与客户端逐字对齐，防 UnsatisfiedLinkError）

| 项 | 值（**权威**） |
|---|---|
| 库文件名（进 `jniLibs/arm64-v8a/`） | **`libskrootpatch.so`** |
| `System.loadLibrary` 实参 | **`"skrootpatch"`** |
| ELF SONAME | `libskrootpatch.so`（与文件名一致；loadLibrary 按**文件名**查找，SONAME 仅用于依赖解析，二者保持一致最不易出错） |
| Java 类（FQCN） | `me.sproot.patch.NativeEngine`（Kotlin `object` + `@JvmStatic external fun`） |
| 方法 1 | `patch(inPath: String, outPath: String, key: String?, jsonSink: String?, workdir: String?): Int` |
| 符号 1 | `Java_me_sproot_patch_NativeEngine_patch` |
| 方法 2 | `engineVersion(): String` |
| 符号 2 | `Java_me_sproot_patch_NativeEngine_engineVersion` |
| C 侧签名 | `(JNIEnv*, jclass, jstring, jstring, jstring, jstring, jstring) -> jint` / `(JNIEnv*, jclass) -> jstring` |

- `@JvmStatic` 只是把 Kotlin object 的方法变成静态桥接；JNI **按符号名查找**，静态/实例两种形态下符号名相同，故当前 C 侧第二参用 `jclass` 均安全（现有客户端用 `@JvmStatic`）。
- **CI 强断言**（`engine.yml`「JNI 符号与客户端包名对齐断言」）：`llvm-nm -D` 必须精确出现上述两个符号，且**不得残留**旧包名符号（如 `Java_com_sproot_manager_*`），否则 CI 红。
- `ENGINE-META.txt` 已固化：`jni_class=me.sproot.patch.NativeEngine`、`jni_load_library=skrootpatch`、`jni_soname=libskrootpatch.so`、`jni_symbols=…`、`jni_binding_version=2`，客户端可机器校验。
- ⚠️ **旧名作废**：`libskpatchjni.so` / `System.loadLibrary("skpatchjni")` 为过渡期旧命名，客户端 `PatchEngine.kt` 的 `LIB_NAME` 必须改为 `"skrootpatch"`、`LIB_FILE_NAME` 改为 `"libskrootpatch.so"`（lead 已通知客户端，方案 A：改客户端、不加别名副本，保持单一事实源）。

### 4.2 形态 B（备选/排障）：静态可执行文件

- 产物：`skroot-patch`（**裸名，不进 jniLibs**）；如需随 APK 分发再额外命名 `libskrootpatchexec.so`（仅 exec、永不 loadLibrary），二选一不得同名
- 必须满足三条，否则 ENOEXEC/EACCES：
  1. `android { packaging { jniLibs { useLegacyPackaging = true } } }`（或 `android:extractNativeLibs="true"`），让文件被解压到 `nativeLibraryDir` 且带可执行位（旧工程已有此配置 `app/app/build.gradle.kts:129-131`）；
  2. **直接 exec `nativeLibraryDir` 下的真实路径**：`ProcessBuilder("$nativeLibraryDir/libskrootpatch.so", "--in", …)`；
  3. **禁止** symlink 到 `filesDir` 后再 exec（旧链路 `PatchesViewModel.kt:75,103` 的做法；symlink 目标虽在 nativeLibraryDir，但 cwd/argv[0] 在 app 私有目录，且 SELinux 判定按路径与域，实测风险高 —— 见 NEW-DOC-03 R2）。
- 优点：崩溃隔离（进程崩了 App 还在）、便于抓 stderr、便于外部/root 复用；缺点：多一层 exec 约束与路径规范。

### 4.3 Gradle 打包任务（必须带断言，禁止再次「静默缺件」）

```kotlin
val copyEngine by tasks.registering(Copy::class) {
    from(rootProject.file("engine/dist/libskrootpatch-arm64-jni.so"))
    into(layout.projectDirectory.dir("src/main/jniLibs/arm64-v8a"))
    rename { "libskrootpatch.so" }
}
val assertEngine by tasks.registering {
    val f = file("src/main/jniLibs/arm64-v8a/libskrootpatch.so")
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
| 本地 qemu 跑**真实内核**（arm64 产物） | ✅ 通过：33,892,864 B 真内核 `rc=0`，耗时 226 s，产物 sha256 与宿主构建**完全一致**（跨架构确定性，§6.2 B9b） |
| GitHub Actions 运行 | 🔴 **第 1 次 run 失败**（[run 42486267958](https://github.com/FengFaQ/SPRoot/actions/runs/42486267958)，commit b53c58e）：**失败点为 `Install qemu-user-static` 步骤**（ubuntu-latest 的 `apt-get update` 因第三方源 403 失败），构建/断言步骤全部 skipped。已修复：`update` 容错 + apt 安装失败回落下载静态 `qemu-aarch64-static`（multiarch release），并重推 v2 触发新 run |
| **正式绿 run（含 magiskboot）** | ✅ **run 42487185268**（engine-arm64, v2, 5749ef48）**success**：11/11 步骤全绿，artifacts **2 个** → `skroot-engine-arm64`（2,337,542 B，含两形态 + SHA256SUMS + magiskboot）与 `magiskboot-arm64`（491,115 B，含 `magiskboot` + `magiskboot-meta.txt`）。链接：https://github.com/FengFaQ/SPRoot/actions/runs/42487185268 |
| 历史绿 run（无 magiskboot） | run 42486757913（aee97046）success，1 artifact（1,846,449 B） |
| **2026-11-09 追加：workdir 所有权 + 密钥 ABI 加固后的本地全量验证** | ✅ **Tier A + Tier B 均 ALL PASS**（真内核 `samples/meizu21` 抽出的 33,892,864 B Image，qemu 两次真修补）：A9d 13 例矩阵、A10 17 例 keyabi、B7（`new_bytes` 96 hex + `key_spec` 六字段）、B9 事故现场（`--out` 在显式 `--workdir` 内：rc=0 且产物/报告/目录全在）、B9d（stdout 末行 = `ROOT_KEY=`）、B10（默认 `<out>.work` 已回收）。**负样本全部打红**：workdir 回退「无条件删」→5 例 FAIL、回退 8a196f0 原版→2 例 FAIL；key 写 49 字节→3 例 FAIL、写 48 可见字符无 NUL→3 例 FAIL、`key_spec` 字段改错→2 例 FAIL |
| **本次 CI run（2026-11-09 加固后）** | ✅ **run 42635720094**（engine-arm64, v2, 907f5ad）**success**，15/15 步骤绿（trigger 步骤 skipped）。artifacts：`skroot-engine-arm64`（2,421,354 B）、`magiskboot-arm64`（491,115 B）。**`libskrootpatch.so`：sha256 `62955d8fedab5c24ae87b9472c6c5506aab55efe7d57964b03da4c54a1382fd1`，size 3,018,552 B**；`skroot-patch`（exec）：sha256 `29908b1c5aeef2544da15296fdd72a6e9e2cf8d3b524eb278717e9beff0a8052`，size 2,950,880 B。CI 日志实测：A6g/A6h、A9a~A9f、A9d-3 用例、A10 17 例、A10s、A10r 全 PASS，`RESULT: ALL PASS`。链接：https://github.com/FengFaQ/SPRoot/actions/runs/42635720094 |

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
| **A7 T-12 强断言** | 合成 boot.img：extract→mutate→repack→**回读 kernel 段 sha256 == patch 输出 sha256**（不是只比长度）+ 产物等长 + `AVBf` footer 逐字节保留 | 防「repack 静默用原始 kernel」；**CI 每次必跑**（不依赖真机样本） |
| A7b 反例保护 | 若回读 sha256 ≠ patch 输出 → 直接 FAIL | 阻断级测试项 T-12 |
| **A9 workdir 所有权（静态）** | A9a 保留提示存在；A9b 判定收在纯函数 `should_reclaim_workdir()` + `workdir_from_user` 显式标记；A9c `remove_all` 受 `reclaim_workdir` 保护 | 防真机事故（产物被引擎自删）回退；见 §8.1 |
| **A9d workdir 所有权（行为，CI 必跑）** | `--self-test-workdir`：**13 例判定矩阵**（7 例 `should_reclaim_workdir` + 6 例 `path_within_dir` 边界），逐例 `expect=reclaim\|keep` vs `got`；rc=0 且 `WDRESULT PASS` 才算过；并单独校验 3 个关键用例（事故现场 / 显式传默认值 / NDJSON 在默认 workdir 内）在场且 PASS，防「删用例 → 空断言」 | **静态 grep 证明不了行为**，A9d 是这条契约的 CI 门槛 |
| A9e 矩阵规模 | `WDCASE/WDDIR` 行数 ≥12（防用例被删空） | T-12 铁律（负样本可打红） |
| **A6g/A6h 密钥长度严格 48** | 47 字符（不足）→ rc=101；49 字符（超长）→ rc=101 | 防"静默截断/放行"导致 ABI 漂移 |
| **A10 密钥 ABI 47+NUL（行为，CI 必跑）** | `--self-test-keyabi`：**17 例**（内嵌 48 字节 / 末字节 NUL / 前 47 字符逐字节相同 / 提权串 47 字符 / 47·49·非 alnum·空 全拒 / `key_spec` 六字段自证）；`KEYRESULT PASS` + `KEYSPEC abi=47+nul bytes=48 last_byte_is_nul=true input_len=48 embed_hex_len=96 embed_hex_tail=00` 才算过；另用随机密钥（`--gen-key` 路径）复跑一次 | 提权链成败所系（错 ABI = 静默 exit 125）；见 §7.5 |

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
# ★ 事故现场形态（2026-11-09 真机）：--out 位于**显式传入**的 --workdir 内
mkdir -p w && $E --in kernel.raw --out w/kernel.patched --key <48字符> --json ev.ndjson --workdir w
$E --bootimg repack         --in samples/meizu21/boot.img --kernel w/kernel.patched --out new-boot.img
```

> **Tier B 主用例自 2026-11-09 起改为「事故现场形态」**（`--out` 在显式 `--workdir` 内），
> 因为事故正是发生在这一形态下；此前"产物在 workdir 外面"的理想形态刚好躲过了 bug。

| 断言 | 实测结果 |
|---|---|
| B0 解包字段 | `header_version=4`、`header_size=1584(0x630)`、`page_size=4096`、`kernel offset=4096(0x1000)`、`kernel_size=33,892,864`、`compression=raw` |
| B0b **交叉验证** | 抽出 kernel sha256 = `4d445384c046f2ba2de856b3215de8d2be83a9ccad115948a34471cde3098481`，与 qa ground truth **逐字节一致** |
| B1 | 补丁 rc=0（`hook_count=19`），`kernel_version=6.1.25`（真实解析，非占位） |
| B2 | 等长：`33,892,864 B` |
| B3 | 输出 sha256 = `399ac4797e6ab2ae5700cc21350676daf7b89fe397e473ef24ae32cb9cce3f2f` ≠ 输入 |
| B4 | 产物含 `key[0:47]+NUL` 窗口（47 字符提权串，CALL v1.1 **E1**） |
| **B7 密钥自证（P0，脚本 Tier B B7）** | `hooks[embed_key].new_bytes` **长度恰为 96 hex（48 字节、偶数）**、以 `key[0:47]` hex **开头**、以 `00` **结尾**；且 `report.key_spec` 自证 `abi=47+nul / bytes=48 / last_byte_is_nul=true / input_len=48 / stored_len=47 / length=48`（客户端与质测可直接断言） |
| B5 | `report.json`：`input/output.sha256+size` 齐全且等长、`hidden_dir_prefix==key[0:16]`、`hook_table` 为数组（含 `blob_count/root_key_region`）、`kernel_version=6.1.25`、`errors=[]` |
| B6 | NDJSON 终态 `result` 唯一且为最后一行 |
| B7 重打包（magiskboot 主路径，手动链） | `magiskboot unpack` → patch → **`cp kernel.patched kernel`** → `magiskboot repack boot.img` → `new-boot.img` **等长 100,663,296 B**；回读 kernel 段 == `kernel.patched`；内嵌 vbmeta 2,496 B 与 `AVBf` footer **逐字节保留**。⚠️ 实测坑：不覆盖 `kernel` 时 repack 会静默用原始 kernel（补丁丢失） |
| B7b 双路交叉校验 | `magiskboot unpack` 的 `kernel` 与自研 `--bootimg extract-kernel` 的 sha256 均为 `4d445384…4841`（独立实现互证） |
| B8 确定性 | 同输入同密钥两次运行产物 sha256 相同 |
| **B9 事故现场回归（核心）** | 在 `--out` 位于显式 `--workdir` 内的形态下：**B9a** workdir 目录仍存在（未被整体删除）；**B9b** `--out` 存在且非空；**B9c** `--out.report.json` 存在（workdir 内产物未被连带清理）；**B9d** stdout 末行仍为 `ROOT_KEY=`（C1 §2.2 会签口径）；**B9e** stderr 有 `workdir preserved (owned by caller)` 诊断 | 真机事故的真实复现条件；旧代码此处 rc=0 但 B9b 必红 |
| **B10 正控：默认 workdir 回收** | 不传 `--workdir`（默认 `<out>.work` 由测试预建并放 marker）→ rc=0、产物存在、**`<out>.work` 已被引擎回收** | 防「干脆永不删除」把契约另一半演没 |
| B9b 跨架构 | arm64 产物（qemu）与宿主 x86-64 产物输出**同一 sha256**（原编号 B9，因新增 B9 事故回归而改号，§5.2 引用同步） |

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

## 7. bootimg 解包/重打包与刷入链（magiskboot 主路径 + AVB 策略）

### 7.1 主打包工具：magiskboot（ARM64，SKRoot 官方路径）

依据：SKRoot 官方提示使用 **magiskboot**；**AIK（Android.Image.Kitchen）不支持 Linux 6.0+ 内核，禁止任何形式使用**；禁止非官方 Windows 版 magiskboot。

| 项 | 值（实测） |
|---|---|
| 来源 | Magisk 官方 release **v31.1**：`https://github.com/topjohnwu/Magisk/releases/download/v31.1/Magisk-v31.1.apk` |
| APK | 10,679,283 B，sha256 `89cefac0a318afa39fb3880c45c87b036c05bc1ebf02a7a715b7f8d7c26ac6e7` |
| 取件 | `unzip -j Magisk-v31.1.apk 'lib/arm64-v8a/libmagiskboot.so'` → 重命名 `magiskboot`（**它是可执行文件，只是名字带 .so**） |
| 二进制 | **988,984 B**，sha256 `97e9ec75731103d31c9e1effca7a50c6444309ce8f78c2ec27e9fe721da4bb54` |
| 形态 | `ELF 64-bit LSB executable, ARM aarch64, version 1 (SYSV), statically linked, BuildID[sha1]=72c3089b…, stripped` → **全静态，无运行时依赖** |
| 运行验证 | `qemu-aarch64-static ./magiskboot` → 打印 `MagiskBoot - Boot Image Modification Tool` + 动作列表 ✅ |

CI 集成（`.github/workflows/engine.yml` 已加步骤）：下载固定版本 APK → `sha256sum -c` 校验 APK → 解出 arm64 `libmagiskboot.so` → 重命名 + `chmod 0755` → `file`/`readelf` 断言 AArch64 → 校验二进制 sha256 → 生成 `magiskboot-meta.txt`（版本/APK sha256/二进制 sha256/来源 URL/GPL-3 声明）→ 作为独立 artifact **`magiskboot-arm64`** 上传，并同时拷入 `skroot-engine-arm64` 包内。

### 7.2 打包链（实测跑通，魅族 21 官方 boot.img）

```bash
# 0) 取件与校验（CI 内固定版本 + sha256，禁止"最新版"漂移）
./magiskboot unpack boot.img          # 产出 kernel（本例 ramdisk_size=0，无 ramdisk.cpio）
# 1) SK 引擎 patch kernel（二进制等长 patch）
skroot-patch --in kernel --out kernel.patched --key <48字符> --json ev.ndjson
# 2) ⚠️ 关键：magiskboot repack 只认文件名 `kernel`
#    实测踩坑：若 cwd 里同时存在 kernel 与 kernel.patched，repack 会用 **kernel（原始的）**，
#    补丁静默丢失。必须先覆盖：cp kernel.patched kernel（或 mv）
cp kernel.patched kernel
# 3) 以原镜像为模板重打包
./magiskboot repack boot.img          # → new-boot.img
# 4) 校验（等长 + AVB 区逐字节保留 + kernel 段 == patched）
```

实测结果：

| 断言 | 结果 |
|---|---|
| `magiskboot unpack` 字段 | `HEADER_VER[4] KERNEL_SZ[33892864] RAMDISK_SZ[0] PAGESIZE[4096] KERNEL_FMT[raw] VBMETA` |
| magiskboot 解出的 `kernel` sha256 | `4d445384c046f2ba2de856b3215de8d2be83a9ccad115948a34471cde3098481`（与 qa ground truth、自研解析**三方一致**） |
| patch rc | 0（`hook_count=19`，`kernel_version=6.1.25`） |
| `magiskboot repack` 产物 | `new-boot.img` = **100,663,296 B = 分区大小**（与官方等长） |
| kernel 段回读 | 自研 `--bootimg extract-kernel` 抽出 sha256 = `399ac479…3f2f` == `kernel.patched`（补丁确实进镜像） |
| AVB 区 | `AVBf` footer 保留 ✅；内嵌 vbmeta（2,496 B @33,898,496）**逐字节保留** ✅ |
| 反例记录 | 未 `cp` 覆盖 `kernel` 时 repack 产物 kernel 仍为原始 `4d445384…` → **静默丢补丁**（已写入测试项 T-12：断言产物 kernel==patched） |

### 7.3 自研 bootimg 的定位：交叉校验 / 回退 / 结构化信息

`engine/src/bootimg.{h,cpp}`（`--bootimg info|extract-kernel|unpack|repack`）**保留**，但不作为主打包器：

1. **交叉校验**：`magiskboot unpack` 与自研 `extract-kernel` 双路取 kernel，比对 sha256（本例一致 → 证明两条独立实现解析一致，qa 的 TBD-8 复验项）。
2. **回退**：magiskboot 缺件/不被设备接受时，可用自研 repack（等长原地回填，保留全部 AVB 区字节）完成打包。
3. **结构化信息**：`--bootimg info` 输出 header/压缩/AVB 字段，供 UI 展示与门禁断言（比 magiskboot 的文本输出更易机器判定）。
4. 自研实现同样遵守「失败不落盘」「等长才写」铁律（`repack` 长度不等直接 E240）。

### 7.4 AVB 与刷入链（**结论已按专项调查修正**）

事实层（实测，**含魔数坑**）：
```
boot.img: header v4, kernel@0x1000 raw 33,892,864 B, ramdisk_size=0
尾部:     AVBf footer @100,663,232（末 64B, 大端）
          original_image_size=33,898,496  vbmeta_offset=33,898,496  vbmeta_size=2,496
文件长:   100,663,296 = fastboot 分区大小（余下为 0 填充）
⚠️ **魔数坑**：AVB footer 的魔数是 **`AVBf`**（小写 f），不是 `AVB0`；`AVB0` 是 vbmeta 镜像（独立 `vbmeta.img`）的魔数。
   判断"boot.img 是否带 AVB"必须用 `tail[-64:-60] == b"AVBf"`；用 `AVB0` 判断会把**有签名的原厂镜像误报为无 AVB**（本组已踩）。
独立 vbmeta.img: 12,288 B, magic AVB0, 算法 SHA256_RSA4096
```

专项调查结论（MEIZU21GKI / KernelSU / KernelPatch 三路源码 + 人类实测反馈）：
- **三家都不碰 AVB 密码学**：不重签、不改 vbmeta flags、不用 `--disable-verification`，只保证镜像结构合法（footer/vbmeta blob 在、总长 == 分区大小），**依靠已解锁 bootloader 跳过验证**开机。
- 该机 `vbmeta.img` 对 boot 只有 **Chain Partition descriptor（RSA4096）**，boot 的 hash descriptor 在 **boot.img 内嵌 vbmeta** 里；没有厂商私钥，改动后的 boot 在**锁态**必然验证失败。
- `--disable-verification` 既治不了病，又正是**清数据**的来源 → **禁止**。

**主路径（MVP 采用）**：
```bash
fastboot flash --slot=all boot sproot-<device>-boot.img
fastboot reboot
```
- **明确禁止**：`flash/erase vbmeta`、`--disable-verity`、`--disable-verification`、重签/改 vbmeta flags。
- 结构要求（实现已满足）：产物体积 **== 分区大小**（boot=100,663,296；init_boot=8,388,608）+ 内嵌 vbmeta 与 `AVBf` footer **逐字节保留**。
- 备选（**仅结构异常时**，有风险）：`--bootimg repack --strip-avb` 去尾部 AVB 区；锁态/严格机型可能拒启，需真机验证。
- 回滚：刷前按 **sha1 内容寻址**保存原 boot.img（学 KSU），失败可用官方全量包回滚（`flash.bat.reference`）。
- 待验：真机刷入后能否开机（`fastboot` 在 A/B 双槽的 `--slot=all` 语义已与官方脚本一致）。

### 7.5 ⚠️ P0 防错：密钥 ABI 不与 Skroot-Next 兼容（必须写死）

调研 `docs/research/skroot-next-analysis.md` 结论：**两套 patcher 的密钥 ABI 不兼容，产物不可混用**。

| 维度 | **SPRoot / 现行上游（本引擎）** | Skroot-Next（第三方） |
|---|---|---|
| 密钥长度 | 输入 48 字符 `[A-Za-z0-9]` | 48 字节 |
| 落盘写入内核 | **`key[0..46]` + NUL（共 48 字节，第 48 字节恒为 0）** | **48 字节无 NUL**，按输入实际长度比较 |
| 内核侧比较 | 跳板逐字节 `ldrb/cmp/cbnz`，遇 NUL 终止（`patch_do_execve.cpp:89-102`） | 按长度比较（`base_func.h:34-40`） |
| 用户态提权串 | **47 字符 + NUL**（CALL v1.1 **E1**） | 48 字节按实际串 |
| 交叉套用后果 | **提权失败（静默）**；两套补丁的隐藏目录前缀/密钥语义不同 | 同左 |

**引擎已实现的强制自检（可测）**：
- `--key-abi <值>`：缺省 `47nul`；**任何其它取值（如 `48nul`/`48`）→ `E101` 拒绝**，stderr 明确写出
  "SPRoot engine implements only 47+NUL … Skroot-Next uses 48-byte no-NUL ABI and is INCOMPATIBLE"。
- **`--key` 长度严格 48**（`^[A-Za-z0-9]{48}$`）：47 字符/49 字符/非 alnum **一律 `E101`**，绝不静默截断或放行
  （CI：A6g/A6h 实测 rc=101）。这防的是"App 用 47、引擎收 48、内核存 47"之间任一环被静默改写。
- **内核内嵌字节的唯一来源**：`engine/src/skroot_patch_main.cpp` 的 `embed_key_bytes()`
  —— `out = cli_key[0:47] + '\0'`，**恰好 48 字节，第 48 字节恒为 0**（第 48 个可见字符不写入内核）。
  函数内联断言 `size()==48 && out[47]==0`；`embed_key` 阶段若派生失败直接 `E250` 失败（不写产物）。
- **`report.json → key_spec`：机器可断言的四字段**（客户端/质测直接读，不必猜）：

  ```json
  "key_spec": {"charset":"A-Za-z0-9","length":48,"stored_len":47,
               "bytes":48,"last_byte_is_nul":true,"input_len":48,
               "abi":"47+nul","source":"user|gen"}
  ```

  | 字段 | 语义 |
  |---|---|
  | `abi` | `"47+nul"`（**规范小写**；非 Next 的 48-no-NUL） |
  | `bytes` | 内核内嵌总字节数 = **48**（47 字符 + NUL） |
  | `last_byte_is_nul` | **true**（第 48 字节恒为 0） |
  | `input_len` | CLI 输入长度 = **48**（48 字符只取前 47 落盘） |
  | `stored_len` / `length` / `charset` | 47 / 48 / `A-Za-z0-9`（旧字段保留，向后兼容） |

- 文档写死：**禁止**用 Skroot-Next 打过的内核再用本引擎/密钥提权；反之亦然。

**全链一致性（CALL v1.1 E1，缺一环即静默失配）**：

```
App 持有 48 字符密钥
  → 引擎 embed_key_bytes(): 写 key[0:47] + NUL（48 字节）     ← A10 / Tier B B7 断言
  → 内核跳板存 47 字符 + NUL（逐字节 ldrb/cmp/cbnz，遇 NUL 终止）
  → daemon 读 /data/adb/sproot/key（47 字符纯文本、无 NUL）
  → escalation_token() 47 字符 → execve 47 字符 → 内核 strcmp 命中
```
⚠️ 客户端/daemon 侧**必须用 47 字符**（`ROOT_KEY[:47]` / 文件内容 47 字符）；若 App 拿 48 字符去提权，
内核第 48 字节是 NUL 而比较串第 48 字节是可见字符 → **静默失败（普通 ENOENT，无日志）**。

**验证命令**（CI 已覆盖）：
```bash
skroot-patch --in kernel --out out --key <48字符> --key-abi 48nul   # → rc=101 且无 out 文件
skroot-patch --in kernel --out out --key <48字符>                   # → rc=0，report.key_spec.abi=="47+nul"
skroot-patch --self-test-keyabi --key <48字符>                      # → KEYRESULT PASS 17/17，KEYSPEC abi=47+nul …
skroot-patch --self-test-keyabi                                     # 随机密钥（--gen-key 路径）同样 17/17
```
**负样本已验**（T-12 铁律）：写「48 可见字符 + NUL」(49 B) → 3 例 FAIL；写「48 可见字符无 NUL」→ 3 例 FAIL；
把 `abi` 改回 `"47+NUL"` 或 `last_byte_is_nul` 改 false → 2 例 FAIL。

### 7.6 诊断能力：`--dump-symbols` 与前置拒绝提示

借鉴 Skroot-Next 的可用做法（其余代码不可复用：该仓 2025-01 后无更新、MSVC 构建、无 release/CI）：

| 能力 | 本引擎现状 |
|---|---|
| 全量符号 dump | **已加 `--dump-symbols`**：解析 kallsyms 后按地址排序打印 `0x<offset> <symbol>`（魅族 21 实测 93,240 行），NDJSON 输出 `result.symbols` 计数；仅读不写盘，用于"机型号/符号缺失"快速定位 |
| 拒绝错误输入并给可操作提示 | **已强化**：`--in *.img/*.gz` → `E102`，stderr 给出具体命令：`./magiskboot unpack boot.img` / `gzip -dc` / `lz4 -d` |
| 能力边界与危险警告 | 见本节 + `NOTICE` 免责声明 + `NEW-DOC-03` 失败模式清单 |

### 7.7 asm oracle：asmjit 字节的独立对照器

风险：引擎五组跳板全部由 **asmjit** 生成；asmjit 版本/编译参数变化可能让字节**静默**改变 → 修补"成功"但内核跑飞。
做法（`engine/android/tools/`）：

- `asmjit_probe.cpp`：对一组关键指令（`nop / mov x0,x0 / add / sub sp,sp / cmp / ldrb post-index / str xzr post-index / cbnz / movk / ret`）用 asmjit 生成字节；
- `asm_oracle.sh`：用 **NDK 自带 LLVM 汇编器**（`clang -x assembler` + `llvm-objdump -d`）汇编同一批助记符，取机器码与 asmjit 输出**逐字节比对**；不一致即 FAIL。
- 实测：**10/10 PASS**；已进 CI（`engine.yml` 步骤「asm oracle」）。probe 只作测试，不进引擎产物。

### 7.8 许可证（Magisk GPL-3.0 的聚合声明 + 根 NOTICE）

magiskboot 来自 Magisk（**GPL-3.0**），我们以**独立可执行程序**随 APK/工具链分发 → 属 GPL 允许的「聚合（aggregate）」，**不传染**我们自研代码；义务是：①随包提供 GPL-3.0 文本；②在 NOTICE/许可清单标注来源（URL + 版本 + 二进制 sha256）；③若修改 magiskboot 源码则需公开修改。已生成 `magiskboot-meta.txt` 随 artifact 分发。
**仓根 `NOTICE` 已建立**（本组维护）：逐项列出 SKRoot(proprietary·已获授权)/asmjit(Zlib)/capstone(BSD-3)/cJSON(MIT)/Magisk(GPL-3.0)/旧 APatch(GPL-3.0) 的来源、SPDX、义务与待补 LICENSE 文本清单；`LICENSE`（自有代码许可）待人类裁定。

> SKRoot 核心授权：人类已确认「有作者授权，可以调用」→ D-3 合规阻塞解除（不再需要 clean-room）；仍建议在 NOTICE 注明授权来源与范围。

## 8. 与契约对齐（DOC-C1 v1.0 / CALL v1.1）

| 契约项 | 实现/证据 | 状态 |
|---|---|---|
| §1 CLI 全参数、零交互 | `skroot_patch_main.cpp:154-190`（`--in/--out/--key/--gen-key/--json/--workdir/--quiet`）；A6 实测 | ✅ |
| §1 拒绝 `.img/.gz` | `main.cpp` 扩展名检查 → rc=102；A6f 实测 | ✅ |
| §2.1 等长产物 | 写回逻辑基于输入 `file_buf` 拷贝；Tier B2 断言待样本 | ⏳ |
| §2.2 密钥三处明文（stdout 末行 / NDJSON result / report.json） | `main.cpp` `ROOT_KEY=` 兜底行 + `emit_result` + `build_report`；A6c 实测输出含 `ROOT_KEY=` | ✅ |
| **§2.3 `hooks[]` 逐 hook 落位（P1 新增，非破坏性）** | 引擎已输出顶层 `hooks[]`：每项 `{group,name,resolved,reason?,hook_addr,trampoline,bytes,patch_offset,orig_bytes,new_bytes,len,target_addr,target_resolved,patches[]}`；无法解析的组输出 `resolved:false + reason`（不省略）；`huawei_bypass` 仅在**内核存在 hkip_check_*_root 符号**时才产出（不适用 ≠ 失败）。魅族 21(6.1.25) 实测 **14 组**（cfi_bypass/drm×2/do_execve/filldir64×3/compat_filldir×3/avc_check/avc_denied/audit_log_start/embed_key）。旧 `hook_table` 聚合字段**原样保留** | ✅ |
| **组数自校验 `hook_groups_expected`（P1 二期）** | 引擎自算期望组数（**唯一来源** `hook_groups_expected_count(ge61, huawei)`）：≥6.1 基 14、<6.1 基 11，`+1` when huawei 符号存在；report 并列输出 `hook_branch`(>=6.1/<6.1)、`hook_has_huawei_symbols`、`hook_groups_expected_source`(kernel/assume)。断言口径：**`hook_groups_count == hook_groups_expected` 严格等值** + `hook_groups_resolved >= expected - huawei` —— 比下界敏感，少织一组即红 | ✅ |
| 组数分支自检（无真镜像覆盖 <6.1） | `--self-test-groups [--assume-kernel-version V] [--assume-huawei-symbols]`（隐藏测试参数，不面向用户）：输出 `branch/base/huawei/hook_groups_expected`，CI/A8 断言矩阵 `<6.1: 5.10.0→11, +huawei→12, 5.15.104→11`；`>=6.1: 6.1.0→14, +huawei→15, 6.6.30→14` | ✅ |
| §2.3 report schema | `build_report`（engine_version/input/output/hook_table/hidden_dir_prefix…） | ✅（Tier B5 待样本验证） |
| §3 退出码 11 枚举 | `ExitCode` enum；A6b/A6c/A6e 实测 101/102 | ✅ |
| §3 失败不落盘铁律 | `fail()` 内 `fs::remove(opt.out_path)`；A6d 实测 | ✅ |
| **§3 `--workdir` 所有权（事故后补足契约）** | 判定收进纯函数 `should_reclaim_workdir()` + 解析期 `workdir_from_user` 标记；行为证据 A9d（13 例矩阵，CI 必跑）+ Tier B B9/B10（真内核） | ✅ 见 §8.1 |
| §4-2 密钥内嵌（47+NUL） | `embed_key_bytes()` 取 CLI 密钥前 47 字符 + NUL（48 字节，内联断言）；跳板逐字节 cmp（`patch_do_execve.cpp:89-102`）；A10 17 例 + Tier B B7 | ✅ |
| **§4-2 `report.key_spec` 机器可断言（P0）** | `abi="47+nul"` / `bytes=48` / `last_byte_is_nul=true` / `input_len=48`（+ `stored_len=47`/`length=48`/`charset`）；A10s 摘要自证 + Tier B B7 断言 | ✅ 见 §7.5 |
| **CALL v1.1 E1 提权串=47 字符** | 内核侧第 48 字节恒为 NUL → 用户态必须传 47 字符 NUL 串（48 字符必失配） | ✅ 已会签 |
| CALL v1.1 E5 取消独立部署器 | 本方案只交付引擎两形态，**无** `skroot-deploy` | ✅ |
| CALL v1.1 §8 root-exec 依赖 `--superkey-file` | 与引擎无关（属 apd/守护面，task-6） | n/a |

---

### 8.1 CLI 契约补充：`--workdir` 所有权（真机事故后追加，2026-11-09）

> **契约原文（本仓权威口径，客户端与引擎共同遵守）**
>
> **显式传入 `--workdir` 时，该目录及其内容的生命周期归调用方，引擎不得删除其中任何内容；只有默认 workdir（`<out>.work`）由引擎在成功收尾时回收。**

**事故背景**（真机首跑，魅族 21）：客户端把 `--out=kernel.patched` 放在显式传入的 `--workdir` 内，
引擎成功收尾时 `remove_all(workdir)` 把产物一并删掉却仍返回 `rc=0` → 客户端只看到
「引擎返回成功但没有产物」，修补在最后一步静默失败（`kernel.patched` 不存在）。
旧实现的问题不在"清理"本身，而在**用字符串是否等于默认值来推断目录所有权**，且 `--out` 的保护
没有覆盖 `--json` 事件流。

判定规则（实现：`engine/src/skroot_patch_main.cpp` 的 `should_reclaim_workdir()`，纯函数、可被自检直接调用）：

| 场景 | 成功收尾行为 |
|---|---|
| 调用方**显式**传 `--workdir`（无论 `--out` 在不在其中） | **不删除**该目录及其中任何内容 |
| 调用方显式传入的 `--workdir` 恰好等于默认值字符串（`<out>.work`） | **不删除** —— 所有权按「命令行是否出现 `--workdir`」判定，**禁止**用字符串比对推断 |
| 未传 `--workdir`（引擎自拼 `<out>.work`） | 回收该目录 |
| 未传 `--workdir`，但 `--out` / `<out>.report.json` / `--json` 任一落在默认 workdir 内 | **不删除**（兜底，防丢产物 / 事件流） |
| 任意失败（rc≠0） | **不删除** workdir（保留诊断）；只删 `--out`（C1 §3 铁律） |

补充约定：

1. **stdout 末行必须是 `ROOT_KEY=<48字符>`**（C1 §2.2 客户端会签兜底口径）。`workdir preserved` 之类诊断一律走 **stderr**，并受 `--quiet` 抑制 —— 否则会把末行挤掉，变成"密钥读不到"的新事故。
2. 引擎当前**不向 workdir 写任何中间产物**（尚无检查点/转储），故上述规则的实际风险面只有"成功收尾的 `remove_all`"。将来若在 workdir 内落盘任何文件，必须先更新本节与 A9d 矩阵。
3. **证据链**：
   - 静态 A9a~A9c（防 grep 级回退）；
   - **行为（CI 必跑）** A9d：`--self-test-workdir` 13 例矩阵（7 例 `should_reclaim_workdir` + 6 例 `path_within_dir` 边界），逐例 `expect=reclaim|keep` vs `got`；A9e 规模下限 ≥12 防"删用例成空断言"；
   - **行为（真内核）** Tier B B9（事故现场：rc=0 且产物/报告/目录全在、stdout 末行仍为 `ROOT_KEY=`）、B10（默认 workdir 回收正控）。
4. **负样本（T-12 铁律：断言必须先被证明能红，2026-11-09 实测）**：
   - 判定回退成「无条件删除」（8a196f0 之前）→ A9d **5 例 FAIL**（`WDRESULT FAIL 8/13 cases`，rc=1）；
   - 判定回退成 8a196f0 原版（纯字符串相等、只护 `--out`）→ A9d 恰好在 `explicit_workdir_equal_default_string` 与 `default_workdir_json_sink_inside` **两例 FAIL**（`WDRESULT FAIL 11/13 cases`，rc=1）。
   - 后者证明本轮加固的两个缺口（显式传默认值仍被删、NDJSON sink 被连带删除）是**真实存在**的行为缺口，而非防御性臆想。
5. 与上游契约的关系：`SPROOT-DOC-C1` §3 只写了「默认 `<out>.work/`，成功后自动清理，失败时保留」，**未规定显式传入时的所有权** —— 本节即该空白的补足。建议 C1 下一版由 Lead 合并本节文字，避免两处口径漂移。

### 8.2 密钥 ABI 契约（P0，提权链成败）

> **契约一句话**：CLI `--key` 收 **48 字符**；引擎只把 **`key[0:47]` + NUL（48 字节）** 写进内核；
> **用户态提权串必须是这 47 个字符**（不带 NUL、不带第 48 字符）。

细节、全链图与负样本见 **§7.5**。机器可断言面（客户端/质测不必猜）：

| 断言对象 | 期望值 |
|---|---|
| `report.key_spec.abi` | `"47+nul"` |
| `report.key_spec.bytes` | `48` |
| `report.key_spec.last_byte_is_nul` | `true` |
| `report.key_spec.input_len` | `48` |
| `report.key_spec.stored_len` / `length` | `47` / `48` |
| `hooks[] group=embed_key` 的 `new_bytes` | 96 个 hex 字符（48 字节、偶数），`startswith(key[:47].hex())`、`endswith("00")` |
| 失败语义 | `--key` 47 字符 / 49 字符 / 非 alnum → `rc=101`（E101），**绝不静默截断或放行** |

证据：A6g/A6h（长度守卫）、A10（`--self-test-keyabi` 17 例，CI 必跑）、Tier B B4/B7（真内核产物字节）。

## 9. 用户态守护进程的启动方式：三种方案对比与选型（2026-11-10 评估）

> **本节结论先行**：
> 1. **“KSU/APatch 都不需要修补 init_boot”这个前提只对 APatch 成立，对 KSU 不成立** —— KSU 同样要改 ramdisk（`/init` → ksuinit → 加载 ko → exec `init.real`），在 GKI+init_boot 机型上改的就是 **init_boot**（见 §9.2 证据）。
> 2. 我们**确实没有**任何“从内核态拉起用户态进程”的能力（§9.1 代码级核对），这是 SK 上游「零常驻内核运行时」模型的直接推论，不是实现遗漏。
> 3. **建议**：**A（App 自引导）先落地验证**；**B（提权时顺带拉起）作为 A 的兜底，待 A 真机结果再决定**；**C（init hook + `call_usermodehelper`）不建议做**，C′（init.rc 虚拟化，APatch 路线）超出 SK 静态 patch 模型，且只能 clean-room。**本轮不动代码。**

### 9.1 现状核对：SK 引擎有没有“拉起用户态”的能力？

**结论：完全没有。** 代码级证据（`engine/src/`，排除 `3rdparty/`）：

| 检索项 | 命中数 | 说明 |
|---|---|---|
| `call_usermodehelper` / `usermodehelper` / `umh` | **0** | 内核态拉起用户态进程的标准原语，零引用 |
| `kernel_thread` / `kthread_run` / `kthread_create` | **0** | 零引用 |
| `init_task` / `init_task_struct` | **0** | 未做任务链表注入（引擎里的 `init_cred` 是**凭据**搜索器，只用于提权，不创建进程） |
| `userd` / `init.rc` 虚拟化 | **0** | 无 APatch 式 rc 替换机制 |

上游同源核对（`upstream/SKRoot-linuxKernelRoot/`，Lite + Pro 全仓）：`call_usermodehelper` / `kernel_thread` / `kthread_run` 亦为 **0 命中**（`userd` 命中均为 `userdata` 误匹配）。即：**上游 SKRoot 本身就没有启动器**，A1 文档「无 daemon、无 socket、无设备节点」的结论在代码上成立。

引擎的全部 hook 组（`patch_pipeline.cpp:98-171`）都是**拦截类**，无一组创建/唤醒进程：

| 组 | 作用 | 是否创建进程 |
|---|---|---|
| `do_execve`（`patch_do_execve.cpp`） | 比较 execve 路径串 == 47 字符密钥；命中则就地把 `current->cred` 改成 uid/gid=0 + 满 cap、清 `TIF_SECCOMP` 与 seccomp 字段 | ❌ 只改**当前**进程凭据 |
| `filldir64` / `compat_filldir` | 按密钥前缀隐藏目录项 | ❌ |
| `avc_check` / `avc_denied` | 仅当“uid=0 且满 cap 且 securebits=0”（即被本引擎提权的进程）时把 AVC 拒绝改为放行（`patch_current_avc_check.cpp:20-50`） | ❌ |
| `audit_log_start` | 同类进程的审计静默 | ❌ |
| `cfi_bypass` / `huawei_bypass` / `drm_*_coredump` | 稳定性/厂商绕过 | ❌ |
| `embed_key` | 写入 47+NUL 密钥 | ❌ |

→ **现状**：内核只负责「让某个进程变成 root」，**没有任何机制让内核主动把 `/data/adb/apd` 拉起来**。因此“AP 守护开机自启”只能借 init：把 `init.sproot.rc` 塞进 **ramdisk**，由 init 解析 service 并启动（§3.1.3 自启链）。

### 9.2 为什么“我们要 init_boot”而 APatch 不要（同业对照，含一处前提纠正）

先修正前提：**KSU 也要改 ramdisk**。仓库内已做的源码级调研（`docs/research/kernelsu-architecture.md`）记录：

- KSU `boot-patch` 第 11 步：`cpio.mv("init","init.real")` + `cpio.add("init", 0o755, ksu_init)` + `cpio.add("kernelsu.ko", ...)`（同文档 L251）；
- `ksuinit` 的定位是「ramdisk 里的 `/init` 第一阶段：加载 ko 后 exec 真正的 init」（同文档 L33）；
- 目标分区参数含 `--partition <boot|init_boot|vendor_boot>`（同文档 L696）。

即 **KSU 的引导件必须落在 ramdisk**；GKI 机型的 ramdisk 在 `init_boot`，所以 **KSU 在这些机型上同样修 init_boot**。我们的方案与 KSU 属**同一层级**的手段（都是往 ramdisk 塞引导件），区别只是我们塞 `rc` 交给 init 解析，KSU 塞 `/init` 外壳程序。

**APatch 确实不需要 init_boot**，原因不是“它更聪明地避开了”，而是它有 SK 所没有的**常驻内核运行时**（KernelPatch）：

| 维度 | SK/SKRoot（我们） | KernelSU | APatch / KernelPatch |
|---|---|---|---|
| 内核侧形态 | **纯静态 shellcode**，无分配器/无 hook 框架/无运行时符号解析 | 常驻 ko 模块（LKM 模式） | 常驻补丁运行时（hook 框架 + `vmalloc` + 运行时 kallsyms 解析） |
| 拉起用户态的方式 | **无** | 改 ramdisk `/init`（ksuinit 加载 ko 后 exec 真 init）；核心 root 不依赖常驻 daemon | **init.rc 虚拟化**：syscall hook 拦截 init 对 `/system/etc/init/hw/init.rc` 等的 open，把内容替换为 `原 rc + exec su -Z u:r:magisk:s0 exec /data/adb/apd`（`upstream/KernelPatch/kernel/patch/android/userd.c:46,106-130,1743-1797`） |
| 是否需要动 ramdisk | **需要**（rc 注入） | **需要**（`/init`） | **不需要**（rc 在 `/system`，内核侧替换） |

**为什么 Meizu21 上是 init_boot 而不是 boot**（实测证据，`--bootimg info samples/meizu21/boot.img`）：

```
header_version=4  kernel: offset=4096 size=33892864 compression=raw
ramdisk: offset=33898496 size=0        ← boot.img 里没有 ramdisk（GKI 布局）
init_boot.img: ramdisk=2,131,370 B     ← rc 只能注进这里
```

→ **“要修 init_boot”不是架构缺陷，而是两件事的交集**：①SK 内核无启动器（§9.1）＋②本机 ramdisk 在 init_boot（实测）。非 GKI 机型（ramdisk 在 boot）只需修 boot.img，与 KSU 的“只修 boot”表现一致。

### 9.3 三方案对比

| | **A：App 自引导** | **B：提权时顺带拉起** | **C：init hook + `call_usermodehelper`** |
|---|---|---|---|
| 机制 | App 先用密钥 exec 一次（内核 hook 授 root）→ 该进程再 `exec /data/adb/apd daemon` | 在 `do_execve` 密钥命中分支后，**内核**调 `call_usermodehelper("/data/adb/apd", argv, envp, UMH_WAIT_EXEC)` | 新增 hook（如 init 首次 exec 或某低频繁 exec 点）→ 一次性标志 → `call_usermodehelper` |
| 引擎改动 | **0** | 小（见 §9.4） | 大（时机/标志/符号/风险，见 §9.5） |
| **能否实现“开机自启”** | ❌ 需用户打开一次 App | ❌ 需先有人提权一次 | ⚠️ 理论可以，但**触发时机在 `/data` 挂载之前**（见 §9.5） |
| 真机风险 | 低（纯用户态链路；依赖现有 avc 绕过） | 中（改 exec 热路径；daemon 以 kernel cred/SID 运行） | **高**（早期启动路径，失败=无法开机/bootloop） |
| 何时可交付 | **立即可做**（ui-client 已在做） | A 之后按需 | 不建议排期 |

**A 的真实定位（必须说清，避免误判）**：A **不是 rc 注入的替代方案**，而是**首版降低刷机面/验证成本的路径**：
- 现状链路本来就存在“首次部署必须有一次用户态 root”的鸡生蛋（`apd deploy` 要写 `/data/adb/sproot/key`，`/data/adb` 仅 root 可写）——这一步**任何方案都躲不掉**，rc 注入解决的是**之后每次开机**的自动拉起；
- A 能让“首版不注入 rc 也能跑通全链”（App 打开即可用），代价是**第三方 App 的 root 依赖用户先打开 SPRoot Manager**（daemon 未起时无 socket 授权），且模块开机挂载/`bootloop` 计数时机会后移；
- 因此 A/B 都是“**免 init_boot 但放弃开机自启**”；要在免 init_boot 的同时保留开机自启，只有 C 或 C′（APatch 路线），二者都要求常驻内核运行时。

**我们的 avc 绕过使 A 在技术上可行**（关键前提）：`avc_check` 的放行判据是「uid=0 + 满 cap + securebits=0」，而提权后进程恰好满足 → 其后续 `exec /data/adb/apd`、写 `/data/adb` 的 AVC 拒绝会被改为放行（`patch_current_avc_check.cpp:20-50` + `patch_avc_denied.cpp:25-41`）。**残留风险**：进程 SELinux **域**仍是 `untrusted_app`（改的是 cred 不是 sid），apd 在该域下的 socket/模块/setcon 行为与 init 拉起时不同，**必须真机实测**。

### 9.4 若做 B：最小改动清单（含断言与负样本）

**空间与符号预算（在真机镜像 `samples/meizu21` 抽出的 33,892,864 B Image 上实测）**：

| 项 | 实测值 | 说明 |
|---|---|---|
| shellcode 可用区 `die` | 0x24bd4 起、到下一符号 **696 B** | ≥6.1 分支的 shellcode 宿主（`root_key_region=0x24bd4`） |
| `die` 已用 | **180 B**（do_execve 164 + 4 个 4 B 跳转） | 含 48 B 内联密钥缓冲 |
| `die` 余量 | **≈516 B** | 加 launcher 可用 |
| `__drm_printfn_coredump` / `__drm_puts_coredump` | 252 B（已用 140）/ 212 B（已用 56） | 另两处宿主 |
| 全量已打补丁字节 | 508 B | 来自真实 report.json |
| `call_usermodehelper` | **在 kallsyms 中，0x000d5878**（下一符号间距 164 B） | patch 时可解析，无需运行时 kallsyms |
| `kallsyms_lookup_name` | 在 kallsyms 中，0x001a4354 | 备用（不推荐运行时解析） |

**改动清单（约 1–2 人日 + 真机窗口）**：

1. **新增 `engine/src/patch_daemon_launcher.{h,cpp}`（自有代码，非上游衍生）**：asmjit 生成 launcher shellcode——
   - 栈上构造 argv（**注意 clap 顶层参数序**，`apd` 的 `--superkey-file` 必须在子命令前）：
     `["/data/adb/apd", "--superkey-file", "/data/adb/sproot/key", "daemon", NULL]` → 4 个内联字符串（13+15+20+7 ≈ 55 B）+ 40 B 指针表 + 调用序列，**估 250–320 B**（在 516 B 余量内，但缓冲很薄）；
   - 字符串地址用 `adr` **PC 相对**取得（**禁止嵌入绝对内核地址**：当前 patch 模型不含 KASLR 基址，硬编码绝对地址在启用 KASLR 的机型上必崩）；
   - `mov x3, #1`（`UMH_WAIT_EXEC`）+ 间接调用；返回失败**静默**（不 panic、不改 exec 结果）。
2. **`analyze/symbol_analyze.{h,cpp}` 新增 `call_usermodehelper` 解析**：找不到时**降级不织入**并在 report 标 `launcher.resolved=false`（不阻断修补）。
3. **上游衍生文件的小幅插入**：`patch_do_execve.cpp` 在密钥命中分支后加一条 `bl launcher`（约 +20 B）。**这是 B 唯一必须动上游文件的地方**；备选“另挂一个 hook 点”不可行（同一函数只能有一个入口 hook）。
4. **CLI 契约**：新增 `--launcher <off|on>`（**默认 off**，保持现有契约与 CI 不变）+ report 新增 `launcher {enabled, symbol, shellcode_bytes, trigger}`。
5. **组数契约**：launcher **并入 `do_execve` 组**（不新增 group）→ `hook_groups_expected` 仍为 14/15（11/12），**不破坏“组数严格等值”断言**；若作为独立组则必须同步 `hook_groups_expected_count()` + A8 矩阵 + qa 断言（不推荐）。

**必须新增的断言（按铁律，先证伪再入库）**：

| 断言 | 判据 | 负样本（必须先打红） |
|---|---|---|
| `--self-test-launcher`（CI 必跑） | launcher blob 中：内联字符串逐字节 == 上述 argv 四条；指针表 5 项且末项为 0；`UMH_WAIT_EXEC` 立即数 == 1；调用点为 `blr` 前有**位置无关**的目标装载（`adr/adrp+add` 或 literal load），**不得**出现裸绝对地址 | 改字符串 1 字节 / 指针表少 NULL / 立即数改 2 / 换成硬编码绝对地址 → 断言必须红 |
| Tier B（真内核） | report `launcher.symbol_addr` == kallsyms 中 `call_usermodehelper` 偏移；`hooks[do_execve].new_bytes` 含该调用序列 | 把符号名拼错 → 期望 `launcher.resolved=false` 或 E202，**且产物仍正常生成**（不得因 launcher 失败而丢修补） |
| 真机 S3 | apd `selftest --json` **必须新增 `startup_source`**（`init-rc` / `app-exec` / `kernel-umh`）+ `parent_pid` | 不实现该字段 → **B 无法验收**（否则又是“永远 PASS 的空断言”） |

**B 的风险清单**：①kCFI/kCFI 检查（间接调用）依赖现有 `cfi_bypass` 组生效——**必须真机确认不会 panic**；②daemon 以 kernel cred（uid 0 + 满 cap）与 **kernel/init 域 SID** 运行，与 `service ... user root` 拉起的语义不同，apd 的 socket/模块/setcon 行为需实测；③每次密钥提权都会尝试拉起（apd 有单实例 `flock`，重复启动无害但有多余进程抖动；若要“只拉一次”需**可写标志**，而当前 patch 模型只写只读 text → 不建议为此引入 data 段 patch）。

### 9.5 为什么不建议 C（init hook + `call_usermodehelper`）

1. **触发时机不成立（硬事实）**：init 由 `kernel_init` exec，`/data` 由 init 自己在其后的阶段挂载 → “hook init 首次 exec 就拉 daemon”时 `/data/adb/apd` **还不存在**，UMH 必然失败。要可用必须“等 /data 就绪” → 需要轮询/延迟/重试逻辑。
2. **“只拉一次”需要可写内存**：一次性标志必须放在可写段（`.data`/`.bss`），而 shellcode 取该地址同样需要运行期基址（KASLR）推导；当前引擎刻意只用 PC 相对寻址（零 KASLR 依赖），引入绝对地址即新增一类机型风险。
3. **SID 不对（APatch 用实证避开了这一点）**：UMH 的 helper 继承 kernel 上下文（`kthreadd` → `init_cred`），exec 后**不是** `u:r:magisk:s0` / `init` 域。APatch 特意走“由 init 自己 exec（`su -Z u:r:magisk:s0 exec /data/adb/apd`）”，就是为了拿对域。我们走 UMH 会得到一个“root 但域不对”的 daemon，apd 的后续能力（模块挂载、域转换、socket 授权）都可能静默失效。
4. **风险-收益倒挂**：C 的唯一收益是“省掉 init_boot 修补”，但 §9.2 表明 KSU 也修 ramdisk/init_boot → 该收益并不构成架构优势；而 C 失败即“无法开机”，代价是最高等级。
5. **C′（APatch 的 init.rc 虚拟化）为什么也不现实**：它需要**常驻运行时**（syscall hook 框架、`vmalloc`、运行时 kallsyms、`/dev` 写入、`copy_to_user` 路径改写），本质是 KernelPatch 级子系统；SK 引擎是“一次性静态 patch”，无分配器、无运行时符号解析 → 要做等于重写一套 KernelPatch，且 KernelPatch 为 GPL-2.0-or-later，**不能混入 proprietary 的 SK 引擎**（只能 clean-room，工期以周计）。

### 9.6 上游与许可约束（对本议题的直接结论）

| 约束 | 事实 | 对 A/B/C 的影响 |
|---|---|---|
| SKRoot 上游 | proprietary（作者授权可用），**无启动器**（§9.1） | B 的 launcher 是我们的**全新自有代码**；只需在 `patch_do_execve.cpp` 做 ≤5 行插入，属“小幅插入”而非重写核心逻辑 → **工程上可行** |
| 许可混合现状 | 上游部分“原作者保留所有权利” + 本项目新增部分 AGPL-3.0，**同二进制混合的现状已存在并已评估** | 新增自有文件（AGPL-3.0）不改变该结构；但**若引入 KernelPatch（GPL-2.0-or-later）代码**会污染整包 → 禁止复制 C′ |
| 无真机 → 断言门槛 | 新增断言必须先被负样本打红（T-12 铁律） | B 的 `--self-test-launcher` 与 Tier B 断言必须先各造一条负样本；C 无法在无真机条件下验证“开机不崩” → 不可测即不应排期 |

### 9.7 建议（供 Lead 拍板）

1. **先落地 A**（ui-client 已在做；零引擎改动、零新 CI 周期）：它能验证“免 init_boot 也能跑通全链”，并暴露 apd 在 `untrusted_app` 域下的真实行为（这是所有后续方案的共同前置知识）。
2. **B 暂不排期，等 A 的真机结论**：若 A 的失败点正是“提权进程 exec/写 `/data/adb` 被拒”，则 B 的价值成立（由内核代为拉起），再按 §9.4 实施；若 A 顺利，**B 无必要**（A 已能拉起 daemon）。
3. **C 不做**；若未来确实要实现“免 ramdisk 且开机自启”，应单独立项评估 **C′（rc 虚拟化）**——需明确“引入常驻内核运行时”这一架构变更，并做许可与真机风险双重评审，**不要**以“加个 hook”的名义低估工程量。
4. **对外口径**：「我们要修 init_boot」的准确表述是——**本机 ramdisk 在 init_boot（GKI 布局），而我们的守护自启走 ramdisk rc（KSU 走 ramdisk `/init`，同一层级）**；APatch 不需要 ramdisk 是因为它有常驻内核运行时做 rc 虚拟化，不是因为我们漏了什么。

## 10. 包体与体积

| 组合 | 增量 | 评价 |
|---|---|---|
| 仅 JNI 形态 | **2.88 MB**（3,018,552 B） | 推荐（单产物、单调用路径） |
| 仅 exec 形态 | 2.81 MB（2,950,880 B） | 需 legacy packaging + 路径规范 |
| 两形态都带 | 5.69 MB | 仅调试期（排障时 exec 形态可直接抓 stderr） |

优化空间（未实施，列入 §11-R3）：裁掉 asmjit x86 后端（~200 KB）、`-ffunction-sections -Wl,--gc-sections`、`-flto`（预估可降到 ~2.0–2.3 MB）；`capstone` 仅 A64 已裁，`cJSON` 不可省（report）。

---

## 11. 风险与待决

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

## 12. 验收门禁映射（lead 的 10 条 → 本文证据）

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
| `engine/src/skroot_patch_main.cpp` | 入口改 `extern "C" spr_engine_run()`，`main()` 转发（`SPROOT_NO_CLI_MAIN` 可裁）；**workdir 所有权判定收进纯函数 `should_reclaim_workdir()` + `path_within_dir()`，新增隐藏自检 `--self-test-workdir`（§8.1）**；**密钥内嵌统一走 `embed_key_bytes()`（47+NUL/48 字节，内联断言）+ `--self-test-keyabi`（§7.5）** |
| `engine/android/build_arm64.sh` | arm64 交叉编译（exec / jni / both） |
| `engine/android/verify_artifact.sh` | Tier A/B 验证（qemu 契约自检）；**A9d `--self-test-workdir` 13 例行为矩阵（CI 必跑）**、**Tier B B9 事故现场回归 / B10 默认 workdir 回收正控**；**A6g/A6h 密钥长度严格 48 + A10 `--self-test-keyabi` 17 例（CI 必跑）+ Tier B B7 强化（new_bytes=96 hex + key_spec 六字段）** |
| `engine/android/jni/skroot_patch_jni.cpp` | JNI 入口 `NativeEngine.patch/engineVersion` |
| `engine/android/tools/asmjit_probe.cpp` | asmjit 指令字节生成器（oracle 的 A 侧） |
| `engine/android/tools/asm_oracle.sh` | asm oracle：asmjit vs NDK LLVM 汇编器逐字节比对（10/10 PASS，已进 CI） |
| `.github/workflows/engine.yml` | 引擎 CI（NDK 交叉编译 + ELF/JNI 符号/契约/T-12/asm oracle/magiskboot + artifacts） |
| `NOTICE` | 第三方组件与许可清单（由 lead 维护，本组不再改动） |
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

## 13. 许可证与合规（D-3）

| 组件 | 位置 | 许可证 | 结论 |
|---|---|---|---|
| **SKRoot（引擎核心来源）** | `upstream/SKRoot-linuxKernelRoot/`（`Lite_version/src/patch_kernel_root/*` 被 vendor 进 `engine/src/analyze|patch_*.cpp`） | **未声明（proprietary，无 SPDX）** | ✅ **已解除**（2026-11-08 人类确认「有作者授权，可以调用」）→ 不阻塞发布；建议 NOTICE 记录授权来源与范围。若后续对外分发仍需作者书面许可存档 |
| **magiskboot（Magisk v31.1）** | CI 产物 `magiskboot-arm64`，随 APK/工具链分发 | **GPL-3.0** | ✅ 独立可执行「聚合」不传染：需随包 GPL-3.0 文本 + NOTICE 标注来源/版本/sha256（已生成 `magiskboot-meta.txt`） |
| KernelPatch | `upstream/KernelPatch/LICENSE` | **GPL-2.0** | 我们已不 vendor 其源码、也不再调用 kptools（bootimg 自研），仅历史参考 → **不引入传染** |
| asmjit | `engine/src/3rdparty/asmjit2-src/LICENSE.md` | **Zlib**（改写自 MIT 风格，宽松） | ✅ 兼容闭源/开源分发，保留版权声明即可 |
| capstone | `engine/src/3rdparty/capstone-5.0.7/`（仅 include/ + src/，未附 LICENSE） | 上游 **BSD-3-Clause** | ✅ 宽松兼容；**行动项**：把上游 LICENSE.TXT/copyright 一并入库（避免"未附声明"的合规瑕疵） |
| cJSON | `engine/src/3rdparty/cJSON/`（仅 .cpp/.h，未附 LICENSE） | 上游 **MIT** | ✅ 兼容；**行动项**：补 LICENSE 文本入库 |
| 旧管理器 App（APatch 衍生） | `app/LICENSE` | **GPL-3.0** | 仅作历史参考/根因分析对象；新 App **不继承**其代码则不受 GPL-3 约束（若复用其 UI/代码则需整体 GPL-3 开源，需 architect 拍板） |

**结论（可执行）**：
1. asmjit(Zlib)/capstone(BSD-3)/cJSON(MIT) 与我们自研代码兼容，可分发；只需补齐第三方位 LICENSE 文本。
2. **真正的合规风险只有 SKRoot 核心算法（无许可证）**：这是 D-3 的阻塞点，需人类与上游作者确认授权，或走 clean-room。
3. 新 App 若从 APatch(GPL-3) 复制代码 → 必须整体 GPL-3 开源；纯参考设计不受限。


## 附录 C：手机端「引擎自检」清单（S2 真机首跑用）

> 目的：在**不动 boot 分区**的前提下，先在手机上确认"引擎能跑、能读真内核、能生成密钥、能产出等长镜像"。
> 全部步骤只操作 `/data/local/tmp` 与 `/sdcard/Download`，不写分区、不刷机。

### C.0 前置（一次性）

| 项 | 要求 | 校验命令 |
|---|---|---|
| 设备解锁 | bootloader 已解锁（AVB 跳过验证依赖它，见 §7.4） | `adb shell getprop ro.boot.verifiedbootstate` → `orange` |
| 架构 | arm64-v8a | `adb shell getprop ro.product.cpu.abi` → `arm64-v8a` |
| 原厂 boot 备份 | 从设备读原始 boot | `adb shell su -c 'dd if=/dev/block/by-name/boot iflag=direct of=/data/local/tmp/boot.img'`（或用户提供的 `samples/meizu21/boot.img`） |
| 工具到位 | `skroot-patch`(arm64 静态) + `libskrootpatch.so`(JNI) + `magiskboot`(arm64) | 从 CI artifact `skroot-engine-arm64` / `magiskboot-arm64` 取 |

### C.1 引擎可执行（exec 形态，最快凸轮）

```bash
adb push skroot-patch /data/local/tmp/ && adb shell chmod 755 /data/local/tmp/skroot-patch
adb shell /data/local/tmp/skroot-patch --help
```
- **期望**：`SPRoot patch engine v1.0.0 (contract SPROOT-DOC-C1 v1.0)` + usage，**rc=0**
- **失败判读**：`Permission denied` → 未 chmod；`not executable: 64-bit ELF` → 架构错（拿到 x86 版）；
  `No such file or directory` 但文件在 → 静态链接缺失（应为 static，无 PT_INTERP）

### C.2 解包真内核（magiskboot）

```bash
mkdir -p /data/local/tmp/patch && cd /data/local/tmp/patch
adb push magiskboot . && adb shell chmod 755 magiskboot
adb shell 'cd /data/local/tmp/patch && ./magiskboot unpack /data/local/tmp/boot.img'
adb shell 'cd /data/local/tmp/patch && ls -l kernel && sha256sum kernel'
```
- **期望**：`HEADER_VER [4] KERNEL_SZ [33892864] PAGESIZE [4096] KERNEL_FMT [raw]`；
  `kernel` sha256 = `4d445384…4841`（与 §6.2 交叉校验一致）
- 若 `KERNEL_FMT` 不是 raw（gzip/lz4）：先解压再进下一步，**不要**把压缩 kernel 交给引擎（引擎会 rc=102/E230）

### C.3 修补（只写 /data/local/tmp）

```bash
adb shell 'cd /data/local/tmp/patch && ./skroot-patch --in kernel --out kernel.patched \
  --gen-key --json ev.ndjson --workdir w --key-abi 47nul'
```
- **期望**：
  - rc=0，stdout 末行 `ROOT_KEY=<48字符>`；`ev.ndjson` 含 progress 阶段序列
    `read_input → parse_kallsyms → derive_offsets → locate_init_cred → patch_cfi_huawei → patch_hooks → embed_key → write_output` 与唯一 `result` 终态
  - `size(kernel.patched) == size(kernel)`（等长，C1 §2.1）
  - `kernel.patched.report.json`：`hook_groups_count >= 13`、`hook_groups_resolved >= 10`、
    `hooks[group=embed_key].new_bytes` = `key[0:47]` hex + `00`（**96 hex = 48 字节**）、`key_spec.abi == "47+nul"`（另见 `bytes=48 / last_byte_is_nul=true / input_len=48`）
- **耗时预期**：宿主机 33.9 MB 内核 **6.4 s**；qemu 模拟 **226 s**；真机（8 核 A 系）预计 **3–10 s**。
  若 >60 s 仍未结束：检查是否 `--workdir` 落在 sdcard（FUSE 慢），应放 `/data/local/tmp`

### C.3b 组数自校验（纯逻辑，10 秒内完成，不需要内核镜像）

```bash
adb shell '/data/local/tmp/skroot-patch --self-test-groups --assume-kernel-version 5.10.0'
# 期望: branch=<6.1 base=11 huawei=no hook_groups_expected=11
adb shell '/data/local/tmp/skroot-patch --self-test-groups --assume-kernel-version 6.1.0 --assume-huawei-symbols'
# 期望: branch=>=6.1 base=14 huawei=yes hook_groups_expected=15
```
- 用途：**验证分支选路逻辑**（<6.1 内核机型上若报"组数不符"，先用本命令确认引擎自身分支判断是否正确，再怀疑镜像）
- 真机正常修补后，`report.hook_groups_expected` 必须 **== `hook_groups_count`**（严格等值）；`hook_groups_resolved >= expected - (hook_has_huawei_symbols?1:0)`

### C.4 打包回镜像（仍不刷机）

```bash
adb shell 'cd /data/local/tmp/patch && cp kernel.patched kernel && ./magiskboot repack /data/local/tmp/boot.img'
adb shell 'cd /data/local/tmp/patch && ls -l new-boot.img && sha256sum new-boot.img'
```
- **期望**：`new-boot.img` 大小 **== 原 boot.img（100,663,296 B）**；`AVBf` footer 与内嵌 vbmeta 逐字节保留（`--bootimg info` 或 §7.4 脚本核对）
- ⚠️ 必须 `cp kernel.patched kernel`（T-12：magiskboot repack 只认 `kernel` 文件名）

### C.5 JNI 形态自检（供 App 集成前确认）

```kotlin
// 在真机 shell 里无法直接 loadLibrary，用 App/Instrumentation 或 adb 触发 PatchEngine.ensureLoaded()
```
- **期望**：`System.loadLibrary("skrootpatch")` 成功；`NativeEngine.engineVersion()` 返回 `1.0.0`；
  传 `key=null` 调 `NativeEngine.patch(...)` 能拿到 rc∈{0,101..}，不抛 `UnsatisfiedLinkError`
- **失败判读**：`UnsatisfiedLinkError: dlopen failed: library "libskrootpatch.so" not found` → 文件名/ABI 目录错；
  `... has text relocations` / `cannot locate symbol` → 依赖非系统库（应为 libm/libdl/libc only）

### C.6 需要回传的证据（S2 归档）

`adb pull /data/local/tmp/patch/{ev.ndjson,kernel.patched.report.json,new-boot.img}`
+ `skroot-patch --help` 输出 + `sha256sum` 三件（kernel / kernel.patched / new-boot.img）+ 耗时。
对照本清单逐项打勾；任一项不符即为**引擎侧 bug**，附 `w/` 目录（失败时保留）与机型内核版本（`uname -r`）。

