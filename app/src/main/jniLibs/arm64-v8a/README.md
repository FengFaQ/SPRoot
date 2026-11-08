# jniLibs（arm64-v8a）—— 随 APK 分发的原生二进制

本目录必须存在以下三个文件才具备完整能力（本骨架阶段可以为空，
App 的入口能力自检会把缺失项作为**阻断项**明确展示，而不是静默跳过）：

| 文件 | 来源 | 作用 | 自检要求 |
|---|---|---|---|
| `libskrootpatch.so` | 修补引擎组（task-3，engine/ 交叉编译产物） | SK 内核修补 + 48 位密钥注入，经 **JNI** 进程内调用 | AArch64 ELF，且导出 `PatchEngine` 契约 v1 的 4 个 JNI 函数（见 `core/PatchEngine.kt` 注释） |
| `libmagiskboot.so` | Magisk 的 magiskboot（aarch64 静态/共享库形态，重命名以便随 APK 解压） | boot.img unpack/repack、kernel decompress/compress | AArch64 ELF + 可执行（`nativeLibraryDir` 下可直接 exec） |
| `libapd.so` | AP 守护（sys-guard，task-6） | 首装部署后复制为 `/data/adb/apd`；`root-exec` 等特权子命令 | AArch64 ELF |

## 硬性约束（来自旧版失败根因）

1. **不得**在构建期从网络下载上述二进制（旧版 `preBuild` 下载 libkptools/kpimg，失败即 exit 1）。
   产物必须随仓库/CI 产出，缺失时构建照常通过、运行时以阻断项报出。
2. 引擎**不做**可执行文件形态（不再出现 `filesDir/xxx` 再 chmod exec 的路径）。
   `libskrootpatch.so` 只走 `System.loadLibrary("skrootpatch")`。
3. `libmagiskboot.so` 必须解压到 `nativeLibraryDir`（`packaging.jniLibs.useLegacyPackaging = true`），
   因为要用 `ProcessBuilder` 直接 exec。
4. 只交付 `arm64-v8a`（`ndk.abiFilters`），其它 ABI 一律不打包。
