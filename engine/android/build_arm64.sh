#!/usr/bin/env bash
# engine/android/build_arm64.sh — SK 修补引擎（skroot-patch）arm64 交叉编译包装脚本
#
# 用法:
#   ./build_arm64.sh [exec|jni|both]         # 默认 exec
#   ANDROID_NDK_HOME=/path/to/ndk ./build_arm64.sh both
#   ANDROID_API=29 ./build_arm64.sh exec     # 必须 >=29，原因见下
#
# 两种产物（对应 docs/NEW-DOC-02 §4 的两条集成形态）:
#   exec → build/arm64/exec/libskrootpatch.so   静态链接可执行文件（ELF ET_EXEC）
#          直接以 nativeLibraryDir 下的**真实路径** exec，绝不 symlink 进 app 私有目录
#          （旧链路 symlink + filesDir exec 命中 app_data_file 禁止执行，NEW-DOC-03 R2）
#   jni  → build/arm64/jni/libskpatchjni.so     共享库 + JNI（进程内调用，完全绕过
#          exec 限制；入口 engine/android/jni/skroot_patch_jni.cpp）
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
    TARGET="$out/libskrootpatch.so" \
    CXX="$CXX_NDK" CC="$CC_NDK" \
    CXXFLAGS="-std=c++2a -O2 -Wall -static -fPIE ${COMMON_DEFS[*]} ${INCLUDES[*]}" \
    CFLAGS="-O2 -Wall -static -fPIE ${COMMON_DEFS[*]} ${INCLUDES[*]}" \
    LDFLAGS="-static" \
    LIBS=""
  "$STRIP_NDK" --strip-all "$out/libskrootpatch.so" -o "$out/.stripped"
  mv "$out/.stripped" "$out/libskrootpatch.so"
  chmod 0755 "$out/libskrootpatch.so"
  echo "[exec] $(stat -c%s "$out/libskrootpatch.so") bytes → $out/libskrootpatch.so"
}

build_jni() {
  local out="$OUTROOT/jni" obj="$OUTROOT/jni/obj-a$API"
  mkdir -p "$out"
  echo "[jni] libskpatchjni.so（引擎全量 TU + JNI 包装，静态 libc++，仅依赖系统 libc/libm/libdl）"
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
  "$CXX_NDK" -shared -static-libstdc++ -o "$out/libskpatchjni.so" \
    "$obj/jni_wrapper.o" "${objs[@]}"
  "$STRIP_NDK" --strip-all "$out/libskpatchjni.so" -o "$out/.stripped"
  mv "$out/.stripped" "$out/libskpatchjni.so"
  echo "[jni] $(stat -c%s "$out/libskpatchjni.so") bytes → $out/libskpatchjni.so"
}

case "$MODE" in
  exec) build_exec ;;
  jni)  build_jni ;;
  both) build_exec; build_jni ;;
  *) echo "usage: $0 [exec|jni|both]" >&2; exit 2 ;;
esac

echo "OK. 静态自检: bash $HERE/verify_artifact.sh $OUTROOT"
