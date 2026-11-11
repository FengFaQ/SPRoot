#!/usr/bin/env bash
# B0 开机能力验证 —— 一条命令从「原厂 boot.img」跑到「三层结论」。
#
# 需要：python3、aarch64 工具（可选）、qemu-system-aarch64（C 层必需）。
# 不需要：root、真机、CI 产物（引擎可执行文件由调用方提供或已存在）。
#
# 用法：
#   bash quality/bootability/run_sample.sh \
#       /path/to/samples/meizu21/boot.img \
#       /path/to/engine/skroot-patch \
#       [输出目录]
#
# 纪律：
#   * 只读取 boot.img，绝不修改样本
#   * 中间产物全部落在输出目录（默认 /tmp/sproot-b0），不进仓库

set -uo pipefail

BOOT_IMG="${1:?usage: run_sample.sh <boot.img> <skroot-patch> [outdir]}"
ENGINE="${2:?usage: run_sample.sh <boot.img> <skroot-patch> [outdir]}"
OUT="${3:-/tmp/sproot-b0}"
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

mkdir -p "$OUT"
echo "[b0] workdir: $OUT"

# ---- 1. 从 boot.img 解出 kernel（v3/v4 头布局）---------------------------
echo "[b0] unpacking kernel from $BOOT_IMG"
python3 - "$BOOT_IMG" "$OUT/kernel.orig" <<'PY'
import struct, sys, hashlib
src, dst = sys.argv[1], sys.argv[2]
d = open(src, 'rb').read()
if d[:8] != b'ANDROID!':
    sys.exit("not an Android boot image (magic != ANDROID!)")
ks = struct.unpack_from('<I', d, 8)[0]     # kernel_size
hv = struct.unpack_from('<I', d, 40)[0]    # header_version
rs = struct.unpack_from('<I', d, 12)[0]    # ramdisk_size (v3/v4 layout)
ps = 4096
k = d[ps:ps + ks]
open(dst, 'wb').write(k)
print("  header_version=%d kernel_size=%d ramdisk_size=%d page=4096" % (hv, ks, rs))
print("  kernel sha256=%s" % hashlib.sha256(k).hexdigest())
if rs == 0:
    print("  NOTE: RAMDISK_SZ=0 → 无 ramdisk，内核到不了用户态（预期结论，不是缺陷）")
PY

# ---- 2. 打补丁 ----------------------------------------------------------
echo "[b0] patching"
KEY=ABCdef0123456789ABCdef0123456789ABCdef0123456789
${ENGINE_RUNNER:-} "$ENGINE" --in "$OUT/kernel.orig" --out "$OUT/kernel.patched" --key "$KEY" \
    > "$OUT/engine.log" 2>&1
rc=$?
if [ $rc -ne 0 ]; then
    echo "[b0] engine FAILED rc=$rc — 见 $OUT/engine.log" >&2
    tail -20 "$OUT/engine.log" >&2
    exit 1
fi
echo "  patched: $(sha256sum "$OUT/kernel.patched" | cut -d' ' -f1)"

# ---- 3. A/B/C 三层 + 自检 ----------------------------------------------
python3 "$HERE/run_all.py" \
    --orig "$OUT/kernel.orig" \
    --patched "$OUT/kernel.patched" \
    --engine-log "$OUT/engine.log" \
    --report "$OUT/kernel.patched.report.json" \
    --out "$OUT/records"
rc=$?

echo
echo "[b0] 记录目录: $OUT/records"
ls -1 "$OUT/records" 2>/dev/null | sed 's/^/  /'
exit $rc
