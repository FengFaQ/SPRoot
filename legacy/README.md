# legacy/ —— 旧管理器与历史文档归档

> 本目录是 **main 分支时代（2026-11 上旬）的历史快照**，已于 2026-11-13 归档。
> 内容**不再维护**，仅作历史参考。当前活跃代码在仓库根目录。

## 来源

- 源分支：`main` @ `1999501`（feat(newapp): SPRoot Manager 骨架 + CI）
- 归档日期：2026-11-13
- 分支已删除；完整 git 历史备份于仓库外 `SPRoot-branch-backups/`（含 `sproot-full-history.bundle`，可完全恢复任意历史版本）

## 内容

| 路径 | 说明 |
|---|---|
| `newapp/` | 旧管理器骨架（Kotlin + Compose + Miuix 0.9.4，37 文件）。已被仓库根 `app/`（v2 全面重写版）取代 |
| `docs-main/` | main 时代的 20+ 份设计/分析文档（NEW-DOC-01~06、SPROOT-DOC-A1/B1/C1、phase1/2 分析、research/）。当前文档在仓库根 `docs/` |
| `workflows/newapp.yml` | 旧管理器的 CI 工作流（已停用） |
| `legacy-snapshot-main-1999501.zip` | 上述内容的压缩快照（360 KB） |

## 为什么归档

- 旧管理器是骨架原型，v2 的 `app/` 从零重写（含 SPRoot 契约 v1.1 的完整实现、
  compose 期 IO 修复、EXPORT 失败语义等，见 git log）。
- 旧文档中 SKRoot 机制分析（SPROOT-DOC-A1）与 C1 契约仍有参考价值，
  但其中若干方案（SuperKey 移除、KP 机制）已被 v2 实现推翻，**以当前 docs/ 为准**。

## 恢复方法

```bash
# 恢复完整历史（任意 commit / 分支）
git clone SPRoot-branch-backups/sproot-full-history.bundle restored-repo
# 或仅解压旧管理器快照
unzip legacy-snapshot-main-1999501.zip -d restored-newapp/
```
