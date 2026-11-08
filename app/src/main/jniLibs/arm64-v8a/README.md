# jniLibs（arm64-v8a）—— 随 APK 分发的原生二进制

本目录由 CI 在构建前自动填充（`apk.yml` 的 "Fetch native libs from CI artifacts" 步骤）；
缺失时构建**照常通过**，但 App 的入口能力自检会把这些项作为**阻断项**明确展示（绝不静默跳过）。

| 文件 | 来源 | 作用 | 自检要求 |
|---|---|---|---|
| `libskrootpatch.so` | 修补引擎组（engine CI，artifact 名待定稿） | SK 内核修补 + 48 位密钥注入，经 **JNI** 进程内调用 | AArch64 ELF，且导出 `PatchEngine` 契约 v1 的 4 个 JNI 函数（见 `core/PatchEngine.kt` 注释） |
| `libapd.so` | 系统守护组（artifact **`apd-aarch64-latest`**，内含 `libapd-arm64.so`） | 部署后复制为 `/data/adb/apd`；提供 `root-exec` 等特权子命令 | AArch64 ELF（PIE）；版本用 `apd-version.txt.commit` 与 `apd version --json` 比对 |
| `libmagiskboot.so` | Magisk 的 magiskboot（aarch64，重命名以便随 APK 解压） | boot.img unpack/repack、kernel decompress/compress | AArch64 ELF + 可执行（`nativeLibraryDir` 下可直接 exec） |

## 硬性约束（来自旧版失败根因）

1. **不得**在构建期从第三方站点下载上述二进制（旧版 `preBuild` 下载 libkptools/kpimg，失败即 exit 1）。
   只允许从**本仓库自己的 CI artifact** 拉取，且失败必须是 SKIP 不是构建失败。
2. 引擎**不做**可执行文件形态（不再出现 `filesDir/xxx` 再 chmod exec 的路径）。
   `libskrootpatch.so` 只走 `System.loadLibrary("skrootpatch")`。
3. `libmagiskboot.so` / `libapd.so` 必须解压到 `nativeLibraryDir`
   （`packaging.jniLibs.useLegacyPackaging = true`），因为要用 `ProcessBuilder` 直接 exec。
4. 只交付 `arm64-v8a`（`ndk.abiFilters`），其它 ABI 一律不打包。
5. `apd-version.txt` 需一并打入 `app/src/main/assets/`，用于运行时版本一致性校验。
