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
EXEC="$ROOT/exec/skroot-patch"
JNI="$ROOT/jni/libskrootpatch.so"
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
  if "$READELF" -h "$JNI" | grep -q 'DYN'; then pass "A5 libskrootpatch.so 为共享库（DYN，可 System.loadLibrary）"; else fail "A5 JNI 产物不是 ET_DYN（不能 loadLibrary）"; fi
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
  # A6g 密钥长度严格 48：47 字符（不足）与 49 字符（超长）都必须 E101，
  # 不许"静默截断/放行"——否则会出现 App 47 字符 vs 内核 48 字符的静默失配（提权 exit 125）
  K47="$(printf 'A%.0s' $(seq 1 47))"; K49="${K47}AA"
  timeout 180 "$QEMU" "$EXEC" --in /x --out /y --key "$K47" >/dev/null 2>&1; rc=$?
  [[ $rc -eq 101 ]] && pass "A6g 47 字符密钥 rc=101（不足 48，严格拒绝而非截断）" \
                    || fail "A6g 47 字符密钥 rc=$rc（期望 101）"
  timeout 180 "$QEMU" "$EXEC" --in /x --out /y --key "$K49" >/dev/null 2>&1; rc=$?
  [[ $rc -eq 101 ]] && pass "A6h 49 字符密钥 rc=101（超长，严格拒绝而非放行）" \
                    || fail "A6h 49 字符密钥 rc=$rc（期望 101）"
else
  echo "SKIP  A6 契约自检（无 qemu-aarch64，可 apt install qemu-user-static）"
fi

# ---------------- A7：T-12 强断言（合成 boot.img，无需真机样本）-----------------
# 防止「repack 静默用原始 kernel / 只比长度不比内容」类回归。
# 强判据：repack 后**回读 kernel 段 sha256 必须等于 patch 输出 sha256**（不是只比长度）。
if [[ -n "$QEMU" && -f "$EXEC" ]] && command -v python3 >/dev/null; then
  echo "--- A7 T-12 强断言（合成 boot.img：extract → mutate → repack → 回读 sha256）---"
  T="$(mktemp -d)"
  python3 - "$T" <<'SYNTH'
import struct, sys, os
d = sys.argv[1]
PS = 4096
kernel = bytes((i * 37 + 11) & 0xFF for i in range(65536))   # 确定性伪随机 kernel payload
hdr = bytearray(PS)
hdr[0:8] = b'ANDROID!'
struct.pack_into('<I', hdr, 8, len(kernel))
struct.pack_into('<I', hdr, 12, 0)
struct.pack_into('<I', hdr, 16, 0x1c00017a)
struct.pack_into('<I', hdr, 20, 1584)
struct.pack_into('<I', hdr, 40, 4)
struct.pack_into('<I', hdr, 1580, 0)
img = bytes(hdr) + kernel
orig_size = len(img)
vbmeta = bytes(range(256))
footer = bytearray(64)
footer[0:4] = b'AVBf'
struct.pack_into('>II', footer, 4, 1, 0)
struct.pack_into('>Q', footer, 12, orig_size)
struct.pack_into('>Q', footer, 20, orig_size)
struct.pack_into('>Q', footer, 28, len(vbmeta))
part = 98304
img = img + vbmeta
img = img + b'\x00' * (part - 64 - len(img)) + bytes(footer)
open(os.path.join(d, 'boot.img'), 'wb').write(img)
open(os.path.join(d, 'kernel.expect'), 'wb').write(kernel)
mut = bytearray(kernel); mut[1234] ^= 0xFF; mut[60000] ^= 0x55
open(os.path.join(d, 'kernel.mutated'), 'wb').write(bytes(mut))
print('synthetic boot.img', len(img), 'kernel', len(kernel))
SYNTH
  rc=0
  "$QEMU" "$EXEC" --bootimg extract-kernel --in "$T/boot.img" --out "$T/kernel.out" --quiet || rc=1
  if [[ $rc -eq 0 ]] && [[ "$(sha256sum < "$T/kernel.out")" == "$(sha256sum < "$T/kernel.expect")" ]]; then
    pass "A7a extract-kernel 解出的 kernel 与内嵌 payload 逐字节一致"
  else
    fail "A7a extract-kernel 结果与 payload 不一致"
  fi
  "$QEMU" "$EXEC" --bootimg repack --in "$T/boot.img" --kernel "$T/kernel.mutated" --out "$T/new-boot.img" --quiet || rc=1
  "$QEMU" "$EXEC" --bootimg extract-kernel --in "$T/new-boot.img" --out "$T/kernel.readback" --quiet || rc=1
  if [[ "$(sha256sum < "$T/kernel.readback")" == "$(sha256sum < "$T/kernel.mutated")" ]]; then
    pass "A7b **T-12 强断言**：repack 后回读 kernel sha256 == patch 输出 sha256"
  else
    fail "A7b T-12 失败：repack 后回读 kernel != patch 输出（补丁被静默丢弃）"
  fi
  if python3 - "$T" <<'CHK'
import sys, os
d = sys.argv[1]
a = open(os.path.join(d, 'boot.img'), 'rb').read()
b = open(os.path.join(d, 'new-boot.img'), 'rb').read()
ok = (len(a) == len(b)
      and a[-64:-60] == b'AVBf' and b[-64:-60] == b'AVBf'
      and a[-64:] == b[-64:]
      and a[4096:4096 + 65536] != b[4096:4096 + 65536])
sys.exit(0 if ok else 1)
CHK
  then
    pass "A7c repack 产物等长 + AVBf footer 逐字节保留 + kernel 段确有改动"
  else
    fail "A7c AVB/长度断言失败"
  fi
  python3 -c "import shutil,sys; shutil.rmtree(sys.argv[1], ignore_errors=True)" "$T"
else
  echo "SKIP  A7 T-12 强断言（需 qemu + python3）"
fi

# ---------------- A8：hook_groups_expected 分支逻辑自检（纯逻辑，无需镜像，CI 必跑）-----
# 目的：5.10/5.15 机型我们没有真机镜像，但必须保证"选 11 组分支"的逻辑正确，
# 否则将来用户在 <6.1 内核上会被误报成"引擎 bug"。
#   期望：<6.1 → 11（无 huawei）/ 12（有 huawei）；>=6.1 → 14（无 huawei）/ 15（有 huawei）
if [[ -n "$QEMU" && -f "$EXEC" ]]; then
  echo "--- A8 hook_groups_expected 分支自检（<6.1 → 11/12；>=6.1 → 14/15）---"
  a8_expect() {   # $1=version $2=extra-arg(or empty) $3=expected
    local got
    if [[ -n "$2" ]]; then
      got="$("$QEMU" "$EXEC" --self-test-groups --assume-kernel-version "$1" "$2" 2>/dev/null \
             | sed -n 's/.*hook_groups_expected=\([0-9][0-9]*\).*/\1/p' | head -1)"
    else
      got="$("$QEMU" "$EXEC" --self-test-groups --assume-kernel-version "$1" 2>/dev/null \
             | sed -n 's/.*hook_groups_expected=\([0-9][0-9]*\).*/\1/p' | head -1)"
    fi
    if [[ "$got" == "$3" ]]; then
      pass "A8 ${1}${2:+ +$2} → hook_groups_expected=$3"
    else
      fail "A8 ${1}${2:+ +$2} → 期望 $3 实得 '${got}'（分支选路/基数字错误）"
    fi
  }
  a8_expect 5.10.0 "" 11
  a8_expect 5.10.0 "--assume-huawei-symbols" 12
  a8_expect 5.15.104 "" 11
  a8_expect 6.1.0 "" 14
  a8_expect 6.1.0 "--assume-huawei-symbols" 15
  a8_expect 6.6.30 "" 14
else
  echo "SKIP  A8 期望组数分支自检（需 qemu）"
fi

# ---------------- A9：workdir 所有权契约（防真机事故回退）----------------
# 真机事故（2026-11-09，魅族21 首跑）：客户端把 --out=kernel.patched 放在显式传入的
# --workdir 内；引擎写完产物后无条件 remove_all(workdir)，连产物一起删掉，却仍返回
# rc=0 → 客户端报"引擎返回成功但没有产物"，修补在最后一步失败。
# 修复语义（最终版，NEW-DOC-02 §8.1）：
#   ① 显式传入 --workdir ⇒ 目录及其内容生命周期**归调用方**，引擎不得删除其中任何内容；
#   ② 只有默认 workdir（<out>.work，调用方没传时才存在）由引擎成功收尾时回收；
#   ③ 兜底：--out / <out>.report.json / --json 任一落在 workdir 内 ⇒ 不删。
# 判定收进纯函数 should_reclaim_workdir()，由 --self-test-workdir 行为矩阵覆盖（A9d）。
# A9a~A9c 只是静态防回退 grep；**真正的行为证据是 A9d**，且 Tier B 用真内核跑事故现场
# （B9：--out 落在显式 --workdir 内仍必须 rc=0 且产物/报告/目录全在）。
# 负样本已验：把判定回退成"无条件删"→ A9d 5 例 FAIL；回退成 8a196f0 原版（纯字符串相等）
# → A9d 的 explicit_workdir_equal_default_string / default_workdir_json_sink_inside 2 例 FAIL。
# 注意：$ROOT 是构建产物目录（engine/android/build/arm64），源码在 engine/src。
# 之前误写 "$ROOT/src/..." 导致 CI 假失败（grep 找不到文件 → 三条断言全红）。
A9_SRC="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/src/skroot_patch_main.cpp"
if [ ! -f "$A9_SRC" ]; then
  echo "FATAL: 找不到引擎源码 $A9_SRC（A9 断言无法执行，说明脚本路径推导有误）"
  exit 1
fi
if grep -q "workdir preserved (owned by caller)" "$A9_SRC" 2>/dev/null; then
  pass "A9a workdir 所有权保护存在"
else
  fail "A9a 缺少 workdir 所有权保护（真机事故会复发：产物被引擎删除）"
fi
A9_FN="$(grep -c 'should_reclaim_workdir' "$A9_SRC" 2>/dev/null || true)"
A9_FLAG="$(grep -c 'workdir_from_user' "$A9_SRC" 2>/dev/null || true)"
if [[ "${A9_FN:-0}" -ge 2 && "${A9_FLAG:-0}" -ge 2 ]]; then
  pass "A9b 判定集中在纯函数 should_reclaim_workdir + 显式 --workdir 标记（不看字符串猜所有权）"
else
  fail "A9b 缺少 should_reclaim_workdir 纯函数或 workdir_from_user 显式标记（found fn=$A9_FN flag=$A9_FLAG）"
fi
A9_GUARD="$(grep -A3 'if (reclaim_workdir)' "$A9_SRC" 2>/dev/null || true)"
if [[ "$A9_GUARD" == *"remove_all"* ]]; then
  pass "A9c remove_all(workdir) 已受判定保护（不再无条件删除）"
else
  fail "A9c remove_all(workdir) 未受 reclaim_workdir 保护（无条件删除仍存在）"
fi
# A9f：保留诊断必须走 stderr —— 否则会挤掉 stdout 末行 ROOT_KEY=（C1 §2.2 会签口径）
if grep -qF 'std::cerr << "[engine] workdir preserved' "$A9_SRC" 2>/dev/null; then
  pass "A9f preserved 诊断走 stderr（不挤占 stdout 末行 ROOT_KEY=）"
else
  fail "A9f preserved 诊断未走 stderr（可能挤掉 stdout 末行 ROOT_KEY=）"
fi
# A9d 行为级：--self-test-workdir 跑判定矩阵（编进同一份 arm64 产物，qemu 直跑）
if [[ -n "$QEMU" && -f "$EXEC" ]]; then
  A9LOG="$("$QEMU" "$EXEC" --self-test-workdir 2>&1)"; a9rc=$?
  A9CNT="$(printf '%s\n' "$A9LOG" | grep -c -E '^(WDCASE|WDDIR) ' || true)"
  if [[ $a9rc -eq 0 && "$A9LOG" == *"WDRESULT PASS"* ]]; then
    pass "A9d workdir 所有权判定矩阵全绿（$A9CNT 例，--self-test-workdir）"
  else
    fail "A9d workdir 所有权判定矩阵有失败项（rc=$a9rc）"
    printf '%s\n' "$A9LOG" | grep -E 'FAIL$|^WDRESULT' | sed 's/^/       /'
  fi
  # 防"把关键用例删掉 → 空断言"：事故现场与本轮加固的两个用例必须在场且 PASS
  for a9case in explicit_workdir_containing_out explicit_workdir_equal_default_string default_workdir_json_sink_inside; do
    if [[ "$A9LOG" == *"WDCASE $a9case"*"PASS"* ]]; then
      pass "A9d-[$a9case] 行为用例在场且 PASS"
    else
      fail "A9d-[$a9case] 用例缺失或未 PASS（断言被削弱成空断言）"
    fi
  done
  [[ "${A9CNT:-0}" -ge 12 ]] && pass "A9e 矩阵规模 ${A9CNT} 例（>=12，防用例被删空）" \
                             || fail "A9e 矩阵仅 ${A9CNT} 例（<12，疑似用例被删）"
else
  echo "SKIP  A9d/A9e 行为级判定矩阵（需 qemu + exec 产物）"
fi


# ---------------- A10：密钥 ABI「47 字符 + NUL」闭环（P0，提权链成败）----------------
# 内核侧存的是 key[0:47] + NUL（共 48 字节）；用户态提权串必须是 **47 字符**。
# 若把 48 个可见字符写进内核（49 字节）或写成 48 字节无 NUL，提权会**静默失败**
# （普通 ENOENT，无日志无计数，客户端只看到 exit 125）——与 Skroot-Next 的
# "48 字节无 NUL" ABI 混用属同一类坑。
# 行为证据：--self-test-keyabi（编进同一份 arm64 产物，不需内核镜像 ⇒ CI 每次必跑）。
# 负样本已验（本地，2026-11-09）：写"48 可见字符+NUL"(49B) → 3 例 FAIL；写"48 可见字符
# 无 NUL" → 3 例 FAIL；把 abi 改回 "47+NUL" / last_byte_is_nul 改 false → 2 例 FAIL。
if [[ -n "$QEMU" && -f "$EXEC" ]]; then
  KEY48="ABCdef0123456789ABCdef0123456789ABCdef0123456789"
  KLOG="$("$QEMU" "$EXEC" --self-test-keyabi --key "$KEY48" 2>&1)"; krc=$?
  if [[ $krc -eq 0 && "$KLOG" == *"KEYRESULT PASS"* ]]; then
    pass "A10 密钥 ABI 自检全绿（48 字符输入 → 内核 47+NUL / 48 字节）"
  else
    fail "A10 密钥 ABI 自检失败（rc=$krc）"
    printf '%s\n' "$KLOG" | grep -E 'FAIL$|^KEYRESULT|^KEYSPEC' | sed 's/^/       /'
  fi
  for kcase in embed_is_48_bytes last_byte_nul prefix_47_matches escalation_token_is_47 \
               reject_47_chars reject_49_chars spec_abi_47nul spec_bytes_48 \
               spec_last_byte_is_nul spec_input_len_48; do
    if [[ "$KLOG" == *"KEYCASE $kcase"*"PASS"* ]]; then
      pass "A10-[$kcase]"
    else
      fail "A10-[$kcase] 用例缺失或未 PASS（密钥 ABI 断言被削弱成空断言）"
    fi
  done
  if [[ "$KLOG" == *"abi=47+nul"* && "$KLOG" == *"bytes=48"* \
        && "$KLOG" == *"last_byte_is_nul=true"* && "$KLOG" == *"input_len=48"* \
        && "$KLOG" == *"embed_hex_len=96"* && "$KLOG" == *"embed_hex_tail=00"* ]]; then
    pass "A10s KEYSPEC 摘要自证（abi=47+nul bytes=48 input_len=48 hex_len=96 tail=00）"
  else
    fail "A10s KEYSPEC 摘要不符：$(printf '%s\n' "$KLOG" | grep '^KEYSPEC')"
  fi
  # 随机密钥（--gen-key 路径）再跑一次：保证不是只对固定样例成立
  KLOG2="$("$QEMU" "$EXEC" --self-test-keyabi 2>&1)"; krc2=$?
  if [[ $krc2 -eq 0 && "$KLOG2" == *"KEYRESULT PASS"* ]]; then
    pass "A10r 随机密钥（--gen-key 路径）ABI 自检同样全绿"
  else
    fail "A10r 随机密钥 ABI 自检失败（rc=$krc2）"
  fi
else
  echo "SKIP  A10 密钥 ABI 行为自检（需 qemu + exec 产物）"
fi

# ---------------- Tier B：真修补产物字节级断言（需一份真 arm64 内核 Image）-----
if [[ -n "${KERNEL_IMAGE:-}" ]]; then
  echo "=== Tier B: 真修补产物断言 (KERNEL_IMAGE=$KERNEL_IMAGE) ==="
  B="$(mktemp -d)"
  # 48 字符 [A-Za-z0-9] 密钥：base64 需取足够熵（32B→44 字符，过滤非字母数字后可能不足 48）
  KEY="$(head -c 128 /dev/urandom | base64 | tr -dc 'A-Za-z0-9' | head -c 48)"
  [[ ${#KEY} -eq 48 ]] || { echo "FAIL  B0 随机密钥长度 ${#KEY} != 48"; exit 1; }
  # ★ 事故现场重现条件（真机 2026-11-09）：--out 落在**显式传入**的 --workdir 内。
  #   Tier B 主用例刻意按这个形态跑（不是"产物在 workdir 外面"的理想形态），
  #   因此 B1~B9 全部在事故条件下断言；B10 再用默认 workdir 做正控。
  mkdir -p "$B/work"
  OUT="$B/work/kernel.patched"
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
    # B7（lead 要求）：用产物字节反证"密钥确实写进内核"——embed_key.new_bytes 必须等于
    # key[0:47] 的 hex + "00"（47 字符 + NUL，CALL v1.1 E1）；同时断言 hook 组数不小于下界，
    # 便于跨机型对比"是否少组"。
    # B8（lead 批准）：组数**严格等值**——hook_groups_count == hook_groups_expected，
    # 且 resolved >= expected - (has_huawei?1:0)；比下界敏感（少一组即红）
    if [[ -f "$OUT.report.json" ]] && command -v python3 >/dev/null; then
      python3 - "$OUT.report.json" <<'GRPCHK' && pass "B8 组数严格等值：count == expected，且 resolved >= expected - has_huawei" || fail "B8 组数与期望不符（可能分支选路错/少织入一组）"
import json, sys
r = json.load(open(sys.argv[1]))
cnt = int(r.get("hook_groups_count", 0))
exp = int(r.get("hook_groups_expected", -1))
res = int(r.get("hook_groups_resolved", 0))
hua = bool(r.get("hook_has_huawei_symbols", False))
ok = (exp > 0 and cnt == exp and res >= exp - (1 if hua else 0))
sys.exit(0 if ok else 1)
GRPCHK
    fi
    if [[ -f "$OUT.report.json" ]] && command -v python3 >/dev/null; then
      python3 - "$OUT.report.json" "$KEY" <<'KEYCHK' && pass "B7 密钥自证：embed_key.new_bytes = 48 字节(key[:47]hex+\"00\")，key_spec 四字段自证" || fail "B7 密钥自证/key_spec 断言失败（47+NUL ABI 被破坏 → 提权会静默失败）"
import json, sys
r = json.load(open(sys.argv[1])); key = sys.argv[2]
ek = [g for g in r.get("hooks", []) if g.get("group") == "embed_key"]
nb = ek[0].get("new_bytes", "") if ek else ""
ks = r.get("key_spec") or {}
ok = (bool(ek)
      # 内嵌字节必须恰好 48 字节（96 hex）、偶数长度、前 47 字符逐字节相同、末字节 NUL
      and len(nb) == 96
      and len(nb) % 2 == 0
      and nb.startswith(key[:47].encode().hex())
      and nb.endswith("00")
      # report.key_spec 必须能机器断言（客户端/质测直接读）
      and ks.get("abi") == "47+nul"
      and ks.get("bytes") == 48
      and ks.get("last_byte_is_nul") is True
      and ks.get("input_len") == 48
      and ks.get("stored_len") == 47
      and ks.get("length") == 48
      and int(r.get("hook_groups_count", 0)) >= 13
      and int(r.get("hook_groups_resolved", 0)) >= 10)
sys.exit(0 if ok else 1)
KEYCHK
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
  else
    fail "B2-B6 未产出 --out（rc=$rc），无法做字节级断言"
  fi
  if [[ -f "$B/events.ndjson" ]]; then
    last="$(grep '"event":"result"' "$B/events.ndjson" | tail -1)"
    n="$(grep -c '"event":"result"' "$B/events.ndjson")"
    [[ "$n" == "1" && "$last" == "$(tail -1 "$B/events.ndjson")" ]] \
      && pass "B6 NDJSON 终态 result 唯一且为最后一行" || fail "B6 NDJSON 终态事件异常（count=$n）"
  else
    fail "B6 缺少 NDJSON sink（--json 未落盘）"
  fi

  # ---- B9 事故现场断言（本轮核心回归）------------------------------------
  # 条件与真机事故完全一致：显式 --workdir + --out 在其内部。旧代码在这里 rc=0 却把
  # 产物删掉，所以必须断言"文件真的还在"，而不是只看 rc。
  [[ -d "$B/work" ]] && pass "B9a 显式 --workdir 目录未被整体删除" \
                     || fail "B9a 显式 --workdir 被引擎删除（真机事故复发）"
  [[ -s "$OUT" ]] && pass "B9b --out 存在且非空（$(stat -c%s "$OUT") B）" \
                  || fail "B9b --out 不存在/为空（真机事故复发：rc=0 但无产物）"
  [[ -f "$OUT.report.json" ]] && pass "B9c report 存在（workdir 内产物未被连带清理）" \
                              || fail "B9c report 缺失（workdir 内产物被连带删除）"
  [[ "$(tail -1 "$B/stdout.txt")" == ROOT_KEY=* ]] \
    && pass "B9d stdout 末行仍为 ROOT_KEY=（C1 §2.2 会签口径未被诊断输出挤掉）" \
    || fail "B9d stdout 末行不是 ROOT_KEY=（实际: $(tail -1 "$B/stdout.txt")）"
  if grep -qF 'workdir preserved (owned by caller)' "$B/stderr.txt"; then
    pass "B9e stderr 输出 workdir 保留诊断（可排查）"
  else
    fail "B9e 缺少 preserved 诊断（stderr）"
  fi

  # ---- B10 正控：默认 workdir（调用方没传 --workdir）必须被引擎回收 ----------
  # 防"干脆永不删除"把 ① 显式=保留 演成 ② 永远保留（契约另一半失效）。
  # 代价：多一次真内核修补（qemu 下约 200s+），仅在有 KERNEL_IMAGE 时执行。
  mkdir -p "$B/d2" "$B/d2/kernel.patched.work"
  : > "$B/d2/kernel.patched.work/engine-scratch-marker"
  OUT2="$B/d2/kernel.patched"
  $RUN "$EXEC" --in "$KERNEL_IMAGE" --out "$OUT2" --key "$KEY" >/dev/null 2>"$B/stderr2.txt"; rc2=$?
  [[ $rc2 -eq 0 ]] && pass "B10a 默认 workdir 形态 rc=0" || fail "B10a 默认 workdir 形态 rc=$rc2"
  [[ -s "$OUT2" ]] && pass "B10b 默认 workdir 形态产物存在" || fail "B10b 默认 workdir 形态产物缺失"
  [[ ! -e "$B/d2/kernel.patched.work" ]] \
    && pass "B10c 默认 workdir（<out>.work）已由引擎回收（C1 §3 / NEW-DOC-02 §8.1）" \
    || fail "B10c 默认 workdir 未回收（契约另一半失效：应回收却没回收）"
  echo "诊断目录保留：$B"
else
  echo "SKIP  Tier B 字节级断言（设置 KERNEL_IMAGE=<boot.img unpack 出的 arm64 Image> 后重跑）"
fi

echo
if [[ $FAIL -eq 0 ]]; then echo "RESULT: ALL PASS"; exit 0; else echo "RESULT: $FAIL 项失败"; exit 1; fi
