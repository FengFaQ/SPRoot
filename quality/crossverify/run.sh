#!/usr/bin/env bash
# SPRoot 开机能力验证 · 一键复跑
#
# 用法：
#   ./run.sh <boot.img> [engine-dir] [out-dir]
#
#   boot.img    : 原厂 boot 镜像（如 samples/meizu21/boot.img）
#   engine-dir  : 含 skroot-patch 的 arm64 引擎目录（CI artifact 解压结果）
#                 省略则尝试从 GitHub Actions 下载最新 skroot-engine-arm64
#   out-dir     : 工作目录，默认 ./_boot_test
#
# 依赖（缺失会自动提示，不会静默跳过）：
#   qemu-aarch64-static / qemu-system-aarch64 / aarch64-linux-gnu-objdump / python3
#
# 产出：
#   <out>/kernel.orig / kernel.patched / kernel.patched.report.json
#   <out>/serial-orig.log / serial-patched.log
#   <out>/xv.json      第二套独立验证结果
#   <out>/bootability/ 第一套验证结果（若 quality/bootability 可用）
set -uo pipefail

BOOT_IMG="${1:-}"
ENGINE_DIR="${2:-}"
OUT="${3:-./_boot_test}"
REPO="$(cd "$(dirname "$0")/../.." && pwd)"

die() { echo "ERROR: $*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "缺少依赖：$1（$2）"; }

[ -n "$BOOT_IMG" ] || die "用法: $0 <boot.img> [engine-dir] [out-dir]"
[ -f "$BOOT_IMG" ] || die "找不到 boot.img: $BOOT_IMG"

need python3        "解析镜像头"
need qemu-aarch64-static "跑 arm64 引擎"
need qemu-system-aarch64 "启动内核"
need aarch64-linux-gnu-objdump "反汇编 cave"

mkdir -p "$OUT"
echo "=== 1. 解析 boot.img，提取 kernel ==="
python3 - "$BOOT_IMG" > "$OUT/kernel.orig" <<'PY'
import struct, sys
d = open(sys.argv[1], 'rb').read()
assert d[:8] == b'ANDROID!', 'not an Android boot image'
ks = struct.unpack_from('<I', d, 8)[0]      # kernel_size
rs = struct.unpack_from('<I', d, 12)[0]     # ramdisk_size (v3/v4)
hv = struct.unpack_from('<I', d, 40)[0]     # header_version
page = 4096
sys.stderr.write("kernel_size=%d ramdisk_size=%d header_version=%d\n" % (ks, rs, hv))
sys.stdout.buffer.write(d[page:page + ks])
PY
[ -s "$OUT/kernel.orig" ] || die "kernel 提取失败"
echo "   -> $OUT/kernel.orig ($(stat -c%s "$OUT/kernel.orig") B)"

echo "=== 2. 取得 arm64 引擎 ==="
if [ -z "$ENGINE_DIR" ]; then
  ENGINE_DIR="$OUT/engine"
  mkdir -p "$ENGINE_DIR"
  TOKEN=$(grep -o 'github_pat_[A-Za-z0-9_]*' "$HOME/.ssh/GITHUB_CREDENTIALS.md" 2>/dev/null \
          </dev/null || true)
  if [ -z "${TOKEN:-}" ]; then
    die "未指定 engine-dir 且找不到 GitHub token；请手工解压 CI artifact 后作为第 2 参传入"
  fi
  echo "   下载最新 skroot-engine-arm64 ..."
  AID=$(curl -s -H "Authorization: Bearer $TOKEN" \
        "https://api.github.com/repos/FengFaQ/SPRoot/actions/artifacts?per_page=30" \
        | python3 -c "
import json,sys
for a in json.load(sys.stdin).get('artifacts',[]):
    if a['name']=='skroot-engine-arm64' and not a['expired']:
        print(a['id']); break")
  [ -n "$AID" ] || die "未找到可用的 skroot-engine-arm64 artifact"
  curl -sL -H "Authorization: Bearer $TOKEN" \
       "https://api.github.com/repos/FengFaQ/SPRoot/actions/artifacts/$AID/zip" \
       -o "$OUT/engine.zip" || die "下载失败"
  unzip -oq "$OUT/engine.zip" -d "$ENGINE_DIR" || die "解压失败"
fi
chmod +x "$ENGINE_DIR/skroot-patch" 2>/dev/null
[ -x "$ENGINE_DIR/skroot-patch" ] || die "engine-dir 里没有可执行的 skroot-patch"

echo "=== 3. 修补内核（真跑引擎，qemu-aarch64 用户态）==="
KEY=$(python3 -c "import secrets,string;print(''.join(secrets.choice(string.ascii_letters+string.digits) for _ in range(48)))")
qemu-aarch64-static "$ENGINE_DIR/skroot-patch" \
  --in "$OUT/kernel.orig" --out "$OUT/kernel.patched" --key "$KEY" \
  --json "$OUT/progress.ndjson" --workdir "$OUT/work" > "$OUT/engine.log" 2>&1
RC=$?
echo "   engine rc=$RC"
[ "$RC" -eq 0 ] || { tail -20 "$OUT/engine.log"; die "引擎修补失败"; }
[ -s "$OUT/kernel.patched" ] || die "引擎返回成功但没有产物"
echo "   -> $(stat -c%s "$OUT/kernel.patched") B"

echo "=== 4. 启动对照（qemu-system-aarch64）==="
boot_one() {
  local kern="$1" log="$2"
  timeout 60 qemu-system-aarch64 \
    -M virt -cpu cortex-a57 -m 2048 -smp 2 -nographic \
    -kernel "$kern" \
    -append "console=ttyAMA0 earlycon=pl011,0x9000000 ignore_loglevel" \
    > "$log" 2>&1
  local rc=$?
  # 124 = timeout（内核 panic 后停在重试循环，属正常）
  if [ "$rc" -ne 0 ] && [ "$rc" -ne 124 ]; then
    echo "   WARN: $kern 退出码 $rc"
  fi
  echo "   $(basename "$log"): $(wc -l < "$log") 行"
}
boot_one "$OUT/kernel.orig"    "$OUT/serial-orig.log"
boot_one "$OUT/kernel.patched" "$OUT/serial-patched.log"

echo "=== 5. 第二套独立验证（crossverify）==="
python3 "$REPO/quality/crossverify/xv.py" \
  --orig "$OUT/kernel.orig" --patched "$OUT/kernel.patched" \
  --report "$OUT/kernel.patched.report.json" \
  --log-orig "$OUT/serial-orig.log" --log-patched "$OUT/serial-patched.log" \
  --json "$OUT/xv.json"
XV_RC=$?

echo "=== 6. 第一套验证（bootability，若存在）==="
if [ -f "$REPO/quality/bootability/patch_audit.py" ]; then
  mkdir -p "$OUT/bootability"
  python3 "$REPO/quality/bootability/patch_audit.py" \
    --orig "$OUT/kernel.orig" --patched "$OUT/kernel.patched" \
    --report "$OUT/kernel.patched.report.json" \
    --json "$OUT/bootability/patch-audit.json" 2>&1 | tail -5
  python3 "$REPO/quality/bootability/driver_integrity.py" \
    --orig "$OUT/kernel.orig" --patched "$OUT/kernel.patched" \
    --json "$OUT/bootability/driver-integrity.json" 2>&1 | tail -5
else
  echo "   SKIP: quality/bootability 不存在"
fi

echo ""
echo "=============================================================="
if [ "$XV_RC" -eq 0 ]; then
  echo "结论：crossverify 全部 PASS"
else
  echo "结论：crossverify 有 FAIL —— 见上方明细与 $OUT/xv.json"
fi
echo "工作目录：$OUT"
echo "=============================================================="
exit "$XV_RC"
