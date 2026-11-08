# SPRoot（新管理器，v2 重构）

SKRoot-Lite × 全新管理器：放弃改造 APatch 管理器，从零实现，仅参考 KernelSU/APatch 架构；
支持**手机端本地修补**（App 进程内 JNI 引擎），UI 使用 [miuix](https://github.com/compose-miuix-ui/miuix)。

## 构建（全部经 GitHub Actions，不在本地编译交付物）
- `.github/workflows/apk.yml` → 管理器 APK（release，仓库内 keystore 签名，验证 CN=FengFa）
- `engine/` → SK 修补引擎（arm64 交叉编译，见 engine.yml）
- `apd/` → 模块守护（Rust arm64 交叉编译，见 apd.yml）

## 关键约束
- 契约 v1.1-FROZEN：docs/phase2_skroot_ap_call_contract_v1.1.md
- 修补链（App 内 JNI，无需 root）≠ 特权链（root-exec 走 AP）
- 错误必须完整可见、失败不得假成功（旧版"一闪即退"的根因见 docs/research/patch-failure-rootcause.md）
