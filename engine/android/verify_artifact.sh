#!/usr/bin/env bash
# engine/android/verify_artifact.sh — SK 引擎 arm64 产物静态验证（无真机可跑）
#
# 用法:
#   bash verify_artifact.sh [build/arm64 目录]              # Tier A：ELF/契约自检（qemu 可选）
#   KERNEL_IMAGE=/path/to/kernel bash verify_artifact.sh    # Tier A + Tier B：真修补字节级断言
#
# 依据：docs/NEW-DOC-02 §6、SPROOT-DOC-C1 §2/§3/§4。
# 退出码：0 全通过；1 有断言失败。逐条 PASS/FAIL，CI 可直接判定。
set -uo pipefail

ROOT="${1:-$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/build/arm64}"
EXEC="$ROOT/exec/libskrootpatch.so"
JNI="$ROOT/jni/libskpatchjni.so"
QEMU="$(command -v qemu-aarch64-static || command -v qemu-aarch64 || true)"
NDK="${ANDROID_NDK_HOME:-${ANDROID_NDK_ROOT:-/opt/android-ndk/android-ndk-r27c}}"
READELF="$(command -v readelf || echo "$NDK/toolchains/llvm/prebuilt/linux-x86_64/bin/llvm-readelf")"
FAIL=0
pass() { echo "PASS  $*"; }
fail() { echo "FAIL  $*"; FAIL=$((FAIL+1)); }

echo "=== Tier A: ELF 形态与契约自检 ($ROOT) ==="

# A1 架构必须是 AArch64（EM_AARCH64=0xB7），而不是旧链路那种宿主 x86-64（0x3E）
if [[ -f "$EXEC" ]]; then
  mach="$("$READELF" -h "$EXEC" | awk -F: '/Machine/{gsub(/^ +/,"",$2); print $2}')"
  [[ "$mach" == *AArch64* ]] && pass "A1 引擎 e_machine=AArch64（$mach）" \
                             || fail "A1 引擎架构错误：$mach（必须 AArch64）"
  # A2 静态链接：无 PT_INTERP / 无 NEEDED，不依赖设备 libc 版本
  if "$READELF" -lW "$EXEC" | grep -q INTERP; then fail "A2 引擎含 PT_INTERP（非静态）"; else pass "A2 引擎无 PT_INTERP（静态）"; fi
  if "$READELF" -dW "$EXEC" 2>/dev/null | grep -q NEEDED; then fail "A2b 引擎有 NEEDED 依赖"; else pass "A2b 引擎无 NEEDED 依赖"; fi
  # A3 TLS 段对齐：API<29 sysroot 下为 0x8，bionic 启动即 abort（NEW-DOC-02 §3.2 实测）
  tls="$("$READELF" -lW "$EXEC" | awk '/TLS/{print $NF}')"
  [[ "$tls" == "0x40" ]] && pass "A3 PT_TLS p_align=0x40（bionic arm64 要求 >=64）" \
                         || fail "A3 PT_TLS p_align=$tls（必须 0x40，请用 ANDROID_API>=29 重建）"
  sz="$(stat -c%s "$EXEC")"
  pass "A4 大小 $sz B sha256=$(sha256sum "$EXEC" | cut -c1-16)…（登记于 NEW-DOC-02 §3.3）"
  [[ -x "$EXEC" ]] && pass "A4b 可执行位已置" || fail "A4b 缺少可执行位（APK 内必须 0755/zip 未压缩）"
else
  fail "A0 缺少 $EXEC（先跑 build_arm64.sh exec）"
fi

# A5 JNI 形态：导出入口符号 + 只依赖系统库
if [[ -f "$JNI" ]]; then
  if "$READELF" -h "$JNI" | grep -q 'DYN'; then pass "A5 引擎 .so 为共享库（DYN）"; else fail "A5 .so ELF 类型错误"; fi
  syms="$("$READELF" -sW --dyn-syms "$JNI" 2>/dev/null)"
  # 注意：不能写 `echo "$syms" | grep -q ...` —— pipefail 下 grep -q 提前退出会让
  # 写端拿到 SIGPIPE(141)，把「找到」误判成失败（本脚本已踩过此坑）
  [[ "$syms" == *"Java_me_sproot_patch_NativeEngine_patch"* ]] \
    && pass "A5b 导出 Java_me_sproot_patch_NativeEngine_patch" \
    || fail "A5b 未导出 JNI 入口 patch()"
  # spr_engine_run 是引擎共享实现入口：只要它没留在未定义符号里，就说明 JNI 包装
  # 已与引擎逻辑真实链接（strip 后它是本地符号，不导出；用 UND 检查比导出更准）
  undef="$("$READELF" -sW --dyn-syms "$JNI" 2>/dev/null | awk '$7=="UND" && $8 ~ /spr_engine_run/')"
  [[ -z "$undef" ]] && pass "A5c spr_engine_run 已在库内解析（JNI→引擎逻辑链路成立）" \
                    || fail "A5c spr_engine_run 未解析：$undef"
  bad="$("$READELF" -dW "$JNI" | grep NEEDED | grep -vE 'libc\.so|libm\.so|libdl\.so|liblog\.so' || true)"
  [[ -z "$bad" ]] && pass "A5d 仅依赖系统库（无 libc++_shared 等私有依赖）" || fail "A5d 存在非系统依赖: $bad"
else
  echo "SKIP  A5 JNI 产物未构建（可只交付 exec 形态）"
fi

# A6 契约行为：qemu 用户态执行 arm64 产物（无 qemu 则 SKIP）
if [[ -n "$QEMU" && -f "$EXEC" ]]; then
  echo "--- A6 契约自检（$QEMU）---"
  timeout 180 "$QEMU" "$EXEC" --help >/tmp/spr_help.txt 2>&1; rc=$?
  [[ $rc -eq 0 ]] && grep -q 'SPRoot patch engine' /tmp/spr_help.txt \
    && pass "A6 --help rc=0 且打印版本 banner" || fail "A6 --help 异常 rc=$rc"
  timeout 180 "$QEMU" "$EXEC" >/dev/null 2>&1; rc=$?
  [[ $rc -eq 101 ]] && pass "A6b 缺参数 rc=101 (E_USAGE)" || fail "A6b 缺参数 rc=$rc（期望 101）"
  rm -f /tmp/spr_noout.img
  timeout 180 "$QEMU" "$EXEC" --in /tmp/__no_such_kernel__ --out /tmp/spr_noout.img --gen-key >/tmp/spr_o.txt 2>&1; rc=$?
  [[ $rc -eq 102 ]] && pass "A6c 输入不存在 rc=102 (E_INPUT)" || fail "A6c 输入不存在 rc=$rc（期望 102）"
  [[ ! -e /tmp/spr_noout.img ]] && pass "A6d 失败不落盘铁律：rc!=0 时 --out 不存在" || fail "A6d 违反铁律：失败仍产出 --out"
  timeout 180 "$QEMU" "$EXEC" --in /x --out /y --key short >/dev/null 2>&1; rc=$?
  [[ $rc -eq 101 ]] && pass "A6e 密钥格式非法 rc=101" || fail "A6e 密钥校验 rc=$rc（期望 101）"
  timeout 180 "$QEMU" "$EXEC" --in /tmp/__x.img --out /tmp/__y --gen-key >/dev/null 2>&1; rc=$?
  [[ $rc -eq 102 ]] && pass "A6f 拒绝 .img 明文输入 rc=102（调用方须先 unpack 解压）" || fail "A6f .img 拒绝逻辑 rc=$rc"
else
  echo "SKIP  A6 契约自检（无 qemu-aarch64，可 apt install qemu-user-static）"
fi

# ---------------- Tier B：真修补产物字节级断言（需一份真 arm64 内核 Image）-----
if [[ -n "${KERNEL_IMAGE:-}" ]]; then
  echo "=== Tier B: 真修补产物断言 (KERNEL_IMAGE=$KERNEL_IMAGE) ==="
  B="$(mktemp -d)"
  KEY="$(head -c 32 /dev/urandom | base64 | tr -dc 'A-Za-z0-9' | head -c 48)"
  OUT="$B/kernel.patched"
  RUN="${QEMU:-}"
  $RUN "$EXEC" --in "$KERNEL_IMAGE" --out "$OUT" --key "$KEY" \
      --json "$B/events.ndjson" --workdir "$B/work" >"$B/stdout.txt" 2>"$B/stderr.txt"; rc=$?
  [[ $rc -eq 0 ]] && pass "B1 引擎 rc=0" || fail "B1 引擎 rc=$rc（stderr 末尾: $(tail -1 "$B/stderr.txt")）"
  if [[ -f "$OUT" ]]; then
    s_in="$(stat -c%s "$KERNEL_IMAGE")"; s_out="$(stat -c%s "$OUT")"
    [[ "$s_in" == "$s_out" ]] && pass "B2 产物等长（$s_in B）" || fail "B2 长度不等 in=$s_in out=$s_out"
    [[ "$(sha256sum < "$KERNEL_IMAGE")" != "$(sha256sum < "$OUT")" ]] && pass "B3 字节确有改动（sha256 不同）" \
      || fail "B3 产物与输入完全相同（未打补丁）"
    if command -v python3 >/dev/null; then
      python3 - "$OUT" "$KEY" <<'PY' && pass "B4 密钥已内嵌（47 字符 + NUL 窗口，C1 §4-2）" || fail "B4 未找到密钥内嵌窗口"
import sys
data = open(sys.argv[1], 'rb').read(); key = sys.argv[2]
sys.exit(0 if data.find(key[:47].encode() + b'\x00') >= 0 else 1)
PY
    fi
    if [[ -f "$OUT.report.json" ]] && command -v python3 >/dev/null; then
      python3 - "$OUT.report.json" "$KEY" <<'PY' && pass "B5 report.json 契约（hidden_dir_prefix/hook_table/等长）" || fail "B5 report.json 字段不符"
import json, sys
r = json.load(open(sys.argv[1])); k = sys.argv[2]
ok = (r.get("hidden_dir_prefix") == k[:16]
      and isinstance(r.get("hook_table"), list) and len(r["hook_table"]) > 0
      and r.get("input", {}).get("size") == r.get("output", {}).get("size"))
sys.exit(0 if ok else 1)
PY
    else
      fail "B5 缺少 report.json"
    fi
    if [[ -f "$B/events.ndjson" ]]; then
      last="$(grep '\"event\":\"result\"' "$B/events.ndjson" | tail -1)"
      n="$(grep -c '\"event\":\"result\"' "$B/events.ndjson")"
      [[ "$n" == "1" && "$last" == "$(tail -1 "$B/events.ndjson")" ]] \
        && pass "B6 NDJSON 终态 result 唯一且为最后一行" || fail "B6 NDJSON 终态事件异常（count=$n）"
    fi
  else
    fail "B2-B6 未产出 --out（rc=$rc），无法做字节级断言"
  fi
  echo "诊断目录保留：$B"
else
  echo "SKIP  Tier B 字节级断言（设置 KERNEL_IMAGE=<boot.img unpack 出的 arm64 Image> 后重跑）"
fi

echo
if [[ $FAIL -eq 0 ]]; then echo "RESULT: ALL PASS"; exit 0; else echo "RESULT: $FAIL 项失败"; exit 1; fi
