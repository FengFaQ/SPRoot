#!/usr/bin/env python3
"""内置模块物料校验（推前自检 #4）：app/builtin-components.json ↔ assets 实际文件。

背景：内置 NeoZygisk / Mountify 的官方 release zip 直接打进 APK。为保证**可复现**与
"没有被悄悄替换"，清单里记录 size + sha256 + 来源 + 许可证，本脚本校验：
  1. 清单 JSON 合法、每个组件字段齐全（name/version/file/sha256/size/source_url/license/license_url）；
  2. 文件存在、size 一致、sha256 一致（漂移 → 失败）；
  3. assets 目录里没有"未登记"的 zip（防止漏登记/私货）。

用法：python3 quality/static/check_builtin_assets.py [--root .]
退出码：0 通过；1 发现问题。
"""
import argparse
import hashlib
import json
import os
import sys

DEFAULT_ROOT = "."
REQUIRED_FIELDS = (
    "id", "name", "version", "file", "size", "sha256",
    "source_url", "license", "license_url",
)
ASSET_DIR = "app/src/main/assets/builtin"


def sha256_of(path):
    digest = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def main():
    ap = argparse.ArgumentParser(description="内置模块物料校验")
    ap.add_argument("--root", default=DEFAULT_ROOT)
    args = ap.parse_args()
    root = os.path.abspath(args.root)
    manifest = os.path.join(root, "app", "builtin-components.json")
    if not os.path.isfile(manifest):
        print("::error::缺少清单 app/builtin-components.json")
        return 1
    try:
        data = json.load(open(manifest, encoding="utf-8"))
    except json.JSONDecodeError as exc:
        print("::error::清单 JSON 解析失败：%s" % exc)
        return 1
    components = data.get("components") or []
    if not components:
        print("::error::清单 components 为空")
        return 1

    bad = 0
    registered_files = set()
    for comp in components:
        missing = [f for f in REQUIRED_FIELDS if f not in comp]
        if missing:
            bad += 1
            print("::error::组件 %r 缺字段：%s" % (comp.get("id"), missing))
            continue
        rel = comp["file"]
        registered_files.add(os.path.normpath(rel))
        path = os.path.join(root, rel)
        if not os.path.isfile(path):
            bad += 1
            print("::error::组件 %s 文件不存在：%s" % (comp["id"], rel))
            continue
        actual_size = os.path.getsize(path)
        if actual_size != comp["size"]:
            bad += 1
            print("::error::组件 %s size 漂移：清单=%d 实际=%d（%s）"
                  % (comp["id"], comp["size"], actual_size, rel))
        actual_sha = sha256_of(path)
        if actual_sha != comp["sha256"].lower():
            bad += 1
            print("::error::组件 %s sha256 漂移：清单=%s 实际=%s"
                  % (comp["id"], comp["sha256"], actual_sha))
        print("[ok] %-10s %-14s %9d B  sha256=%s…" % (
            comp["id"], comp["version"], actual_size, actual_sha[:12]))

    asset_dir = os.path.join(root, ASSET_DIR)
    if os.path.isdir(asset_dir):
        for name in sorted(os.listdir(asset_dir)):
            rel = os.path.normpath(os.path.join(ASSET_DIR, name))
            if name.endswith(".zip") and rel not in registered_files:
                bad += 1
                print("::error::%s 未登记到 app/builtin-components.json" % rel)
    else:
        bad += 1
        print("::error::assets 目录不存在：%s" % ASSET_DIR)

    print("check_builtin_assets: %d component(s), %d problem(s)" % (len(components), bad))
    return 0 if bad == 0 else 1


if __name__ == "__main__":
    sys.exit(main())
