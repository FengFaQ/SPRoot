#!/usr/bin/env bash
# engine/android/build_arm64.sh — SK 修补引擎（skroot-patch）arm64 交叉编译包装脚本
#
# 用法:
#   ./build_arm64.sh [exec|jni|both]         # 默认 exec
#   ANDROID_NDK_HOME=/path/to/ndk ./build_arm64.sh both
#   ANDROID_API=29 ./build_arm64.sh exec     # 必须 >=29，原因见下
#
# 两种产物（对应 docs/NEW-DOC-02 §4 的两条集成形态）:
#   exec → build/arm64/exec/skroot-patch        静态链接可执行文件（ELF ET_EXEC；**不进 APK**，工具/排障用）
#          直接以 nativeLibraryDir 下的**真实路径** exec，绝不 symlink 进 app 私有目录
#          （旧链路 symlink + filesDir exec 命中 app_data_file 禁止执行，NEW-DOC-03 R2）
#   jni  → build/arm64/jni/libskrootpatch.so    **JNI 可加载形态**（ET_DYN，System.loadLibrary
#          ("skrootpatch")），入口 engine/android/jni/skroot_patch_jni.cpp
#          命名约定（与 app-client 对齐）：APK jniLibs 里的 `libskrootpatch.so` 必须是 JNI
#          共享库；独立可执行文件不叫这个名，避免"以为能 loadLibrary 实为 ET_EXEC"的混淆。
#
# 依赖: NDK r27+。本机（WSL Ubuntu 24.04）**无预装 NDK**，已验证做法是把
#       android-ndk-r27c-linux.zip 解包到 /opt/android-ndk 后即可编译；
#       CI（GitHub Actions ubuntu-latest）用同一步骤，见 NEW-DOC-02 §5。
set -euo pipefail

MODE="${1:-exec}"
# API 必须 >=29：NDK r27 的 bionic 静态可执行文件在 API<29 的 sysroot 下生成
# PT_TLS p_align=8，bionic 一启动就 abort：
#   "executable's TLS segment is underaligned ... needs to be at least 64 for ARM64 Bionic"
# API>=29 才输出 p_align=0x40（已实测，见 NEW-DOC-02 §3.2）。
API="${ANDROID_API:-29}"
JOBS="${JOBS:-$(nproc)}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC="$(cd "$HERE/../src" && pwd)"
OUTROOT="$HERE/build/arm64"

NDK="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-/opt/android-ndk/android-ndk-r27c}}"
HOSTTAG="linux-x86_64"
BIN="$NDK/toolchains/llvm/prebuilt/$HOSTTAG/bin"
if [[ ! -x "$BIN/aarch64-linux-android${API}-clang++" ]]; then
  echo "ERROR: NDK clang not found: $BIN/aarch64-linux-android${API}-clang++" >&2
  echo "       请设置 ANDROID_NDK_HOME 指向 NDK r27+ 根目录（未安装见 NEW-DOC-02 §5.1）" >&2
  exit 1
fi
CC_NDK="$BIN/aarch64-linux-android${API}-clang"
CXX_NDK="$BIN/aarch64-linux-android${API}-clang++"
STRIP_NDK="$BIN/llvm-strip"

# 公共宏与头文件路径：与 engine/src/Makefile 一致（asmjit / capstone / cJSON 全部源码内编）
COMMON_DEFS=(-DNDEBUG -DCAPSTONE_HAS_ARM64 -DCAPSTONE_USE_SYS_DYN_MEM)
INCLUDES=(-I"$SRC" -I"$SRC/../upstream/SKRoot-linuxKernelRoot/Lite_version/src/patch_kernel_root"
          -I"$SRC/3rdparty/capstone-5.0.7/include" -I"$SRC/3rdparty/asmjit2-src/src"
          -I"$SRC/3rdparty" -I"$SRC/3rdparty/cJSON")

build_exec() {
  local out="$OUTROOT/exec" obj="$OUTROOT/exec/obj-a$API"
  mkdir -p "$out"
  echo "[exec] $(basename "$CXX_NDK") → aarch64-linux-android$API，静态链接可执行文件"
  make -C "$SRC" -j"$JOBS" \
    OBJDIR="$obj" \
    TARGET="$out/skroot-patch" \
    CXX="$CXX_NDK" CC="$CC_NDK" \
    CXXFLAGS="-std=c++2a -O2 -Wall -static -fPIE ${COMMON_DEFS[*]} ${INCLUDES[*]}" \
    CFLAGS="-O2 -Wall -static -fPIE ${COMMON_DEFS[*]} ${INCLUDES[*]}" \
    LDFLAGS="-static" \
    LIBS=""
  "$STRIP_NDK" --strip-all "$out/skroot-patch" -o "$out/.stripped"
  mv "$out/.stripped" "$out/skroot-patch"
  chmod 0755 "$out/skroot-patch"
  echo "[exec] $(stat -c%s "$out/skroot-patch") bytes → $out/skroot-patch"
}

build_jni() {
  local out="$OUTROOT/jni" obj="$OUTROOT/jni/obj-a$API"
  mkdir -p "$out"
  echo "[jni] libskrootpatch.so（JNI 可加载；引擎全量 TU + JNI 包装，静态 libc++，仅依赖系统 libc/libm/libdl）"
  # 引擎 TU 编成 PIC；-DSPROOT_NO_CLI_MAIN 去掉 main（共享库不导出 main），
  # 逻辑入口统一是 extern "C" spr_engine_run()。
  make -C "$SRC" objs -j"$JOBS" \
    OBJDIR="$obj" \
    CXX="$CXX_NDK" CC="$CC_NDK" \
    CXXFLAGS="-std=c++2a -O2 -Wall -fPIC -fexceptions -frtti -DSPROOT_NO_CLI_MAIN=1 ${COMMON_DEFS[*]} ${INCLUDES[*]}" \
    CFLAGS="-O2 -Wall -fPIC ${COMMON_DEFS[*]} ${INCLUDES[*]}"
  # JNI 包装（jni.h 用 NDK sysroot 自带，不需要 JDK）
  "$CXX_NDK" -std=c++2a -O2 -fPIC -c "$HERE/jni/skroot_patch_jni.cpp" -o "$obj/jni_wrapper.o"
  local objs
  mapfile -t objs < <(find "$obj" -name '*.o' ! -name 'jni_wrapper.o' | sort)
  "$CXX_NDK" -shared -static-libstdc++ -Wl,-soname,libskrootpatch.so \
    -o "$out/libskrootpatch.so" "$obj/jni_wrapper.o" "${objs[@]}"
  "$STRIP_NDK" --strip-all "$out/libskrootpatch.so" -o "$out/.stripped"
  mv "$out/.stripped" "$out/libskrootpatch.so"
  echo "[jni] $(stat -c%s "$out/libskrootpatch.so") bytes → $out/libskrootpatch.so"
}

# ---------------------------------------------------------------- 元数据与符号清单
# 目的：让客户端/qa 能机器校验「ABI + NDK 版本 + 依赖 + 导出符号」，而不是靠口头约定。
write_meta() {
  local exe="$OUTROOT/exec/skroot-patch"
  local so="$OUTROOT/jni/libskrootpatch.so"
  local nm="$BIN/llvm-nm" readelf="$BIN/llvm-readelf"
  {
    echo "engine_version=1.0.0"
    echo "contract=SPROOT-DOC-C1 v1.0; CALL v1.1 (FROZEN)"
    echo "abi=arm64-v8a (AArch64, LP64)"
    echo "android_api=$API"
    echo "ndk=$(basename "$NDK")"
    echo "clang=$("$CXX_NDK" --version | head -1)"
    echo "build_script=engine/android/build_arm64.sh"
    echo "sources=engine/src (SPRoot layer + SKRoot upstream master@943382cd vendored)"
    echo "--- exec: skroot-patch (静态可执行，工具/排障用，不进 APK) ---"
    [[ -f "$exe" ]] && echo "exec_sha256=$(sha256sum "$exe" | cut -d' ' -f1)" && echo "exec_size=$(stat -c%s "$exe")"
    echo "--- jni: libskrootpatch.so (System.loadLibrary(\"skrootpatch\")) ---"
    if [[ -f "$so" ]]; then
      echo "jni_sha256=$(sha256sum "$so" | cut -d' ' -f1)"
      echo "jni_size=$(stat -c%s "$so")"
      echo "jni_soname=$("$readelf" -dW "$so" | awk '/SONAME/{gsub(/[][]/,"",$NF); print $NF}')"
      echo "jni_needed=$("$readelf" -dW "$so" | awk '/NEEDED/{print $NF}' | tr '\n' ',' | sed 's/,$//')"
      echo "jni_layout=$("$readelf" -hW "$so" | awk -F: '/Type/{gsub(/ /,"",$2); print $2}')"
    fi
  } > "$OUTROOT/ENGINE-META.txt"
  if [[ -f "$so" ]]; then
    { echo "# libskrootpatch.so 导出符号（System.loadLibrary 可见）"; "$nm" -D --defined-only "$so" | sort -k3; } \
      > "$OUTROOT/SYMBOLS-libskrootpatch.txt"
  fi
  echo "[meta] → $OUTROOT/ENGINE-META.txt"
  cat "$OUTROOT/ENGINE-META.txt"
}

case "$MODE" in
  exec) build_exec ;;
  jni)  build_jni ;;
  both) build_exec; build_jni ;;
  *) echo "usage: $0 [exec|jni|both]" >&2; exit 2 ;;
esac

write_meta
echo "OK. 静态自检: bash $HERE/verify_artifact.sh $OUTROOT"
