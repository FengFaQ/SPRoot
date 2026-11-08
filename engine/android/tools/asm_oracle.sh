#!/usr/bin/env bash
# engine/android/tools/asm_oracle.sh — asmjit 输出字节的独立 oracle
#
# 思路（lead 2026-11 指示，借鉴 Skroot-Next 的文本汇编路线）：
#   同一批 AArch64 助记符，两条互相独立的实现分别产出机器码：
#     A) asmjit（引擎实际使用的汇编器，编译 asmjit_probe.cpp 后运行）
#     B) NDK 自带 LLVM 汇编器（aarch64-linux-android$API-clang -x assembler + llvm-objdump -d）
#   逐字节比对；不一致即 FAIL。用于捕捉「asmjit 版本/参数变化导致跳板字节悄悄改变」
#   这类会直接让修补后内核跑飞的风险。
#
# 用法: ANDROID_NDK_HOME=... bash engine/android/tools/asm_oracle.sh [ANDROID_API]
set -uo pipefail
API="${1:-${ANDROID_API:-29}}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SRC="$(cd "$HERE/../../src" && pwd)"
OUT="${ORACLE_OUT:-$(mktemp -d)}"
NDK="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-/opt/android-ndk/android-ndk-r27c}}"
BIN="$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin"
CXX="$BIN/aarch64-linux-android${API}-clang++"
CC="$BIN/aarch64-linux-android${API}-clang"
OBJDUMP="$BIN/llvm-objdump"
[ -x "$CXX" ] || { echo "ERROR: NDK clang 不存在: $CXX" >&2; exit 2; }
QEMU="$(command -v qemu-aarch64-static || command -v qemu-aarch64 || true)"
[ -n "$QEMU" ] || { echo "SKIP: 无 qemu-aarch64，无法运行 arm64 probe" >&2; exit 0; }

mkdir -p "$OUT"
PASS=0; FAIL=0

# ---- A) 编译 asmjit_probe（aarch64，静态，qemu 可跑）----
ASMJIT_SRCS=$(find "$SRC/3rdparty/asmjit2-src/src/asmjit" -name '*.cpp' | sort)
# shellcheck disable=SC2086
"$CXX" -std=c++20 -O1 -static -fno-exceptions -fno-rtti \
  -I"$SRC/3rdparty/asmjit2-src/src" \
  "$HERE/asmjit_probe.cpp" $ASMJIT_SRCS \
  -o "$OUT/asmjit_probe" 2>"$OUT/build.log" || {
    echo "FAIL: asmjit_probe 编译失败（见 $OUT/build.log）"; tail -5 "$OUT/build.log"; exit 1; }

# ---- 逐条比对 ----
while IFS=$'\t' read -r name asmtext; do
  got="$("$QEMU" "$OUT/asmjit_probe" "$name" 2>/dev/null)"
  # B) LLVM 汇编器
  printf '\t.text\n\t%s\n' "$asmtext" > "$OUT/one.s"
  "$CC" -x assembler -c "$OUT/one.s" -o "$OUT/one.o" 2>"$OUT/as.log"
  if [ $? -ne 0 ]; then
    echo "FAIL  $name  汇编器报错: $(head -1 "$OUT/as.log")"; FAIL=$((FAIL+1)); continue
  fi
  want="$("$OBJDUMP" -d --no-show-raw-insn=0 "$OUT/one.o" 2>/dev/null | awk '/[0-9a-f]{8}:/{for(i=2;i<=5;i++){printf "%s", $i}; exit}' | tr -d ' ')"
  # llvm-objdump 可能把 4 字节拆成 "1f 20 03 d5" 形式，统一去掉分隔符
  want="$(echo "$want" | tr -d ' ')"
  if [ "$got" = "$want" ]; then
    echo "PASS  $name  $got  ($asmtext)"; PASS=$((PASS+1))
  else
    echo "FAIL  $name  asmjit=$got  llvm=$want  ($asmtext)"; FAIL=$((FAIL+1))
  fi
done < <("$QEMU" "$OUT/asmjit_probe" --list)

echo
echo "asm oracle: PASS=$PASS FAIL=$FAIL  (out=$OUT)"
[ "$FAIL" -eq 0 ] || exit 1
