# SPRoot Manager APK — 随包第三方声明（追加页）

> 本文件由 `app/src/main/assets/NOTICE` 组装：仓库根 `NOTICE`（同源事实）+ 本追加页。
> 构建时 CI 会把根 `NOTICE` 与 `LICENSE` 一并复制进 APK 的 `assets/`，保证分发物自带完整声明。

## A. APK 内实际分发的二进制

| 文件 | 来源 | 许可 | 全文位置 |
|---|---|---|---|
| `lib/arm64-v8a/libmagiskboot.so` | Magisk v31.1 官方 APK（`lib/arm64-v8a/libmagiskboot.so`，988,984 B，sha256 `97e9ec75…4bb54`） | **GPL-3.0** | `assets/licenses/GPL-3.0.txt` |
| `lib/arm64-v8a/libapd.so` | SPRoot 自有 AP 守护（Rust 交叉编译，commit 见 `assets/apd-version.txt`） | **AGPL-3.0**（SPRoot 主体许可） | `assets/licenses/AGPL-3.0.txt` |
| `lib/arm64-v8a/libskrootpatch.so` | SKRoot 内核修补引擎（aarch64 JNI） | **proprietary — All Rights Reserved**，项目所有人已取得作者授权内部使用；**对外分发前需保存书面授权凭证** | 见仓库根 `NOTICE` §1 与 `LICENSES.md` §2 |
| `assets/apd-version.txt` | apd CI 生成的版本/commit/sha256 元数据 | 随 AP 同许可 | — |
| `assets/ENGINE-META.txt` / `SYMBOLS-libskrootpatch.txt` / `SHA256SUMS` / `magiskboot-meta.txt` | 构建期校验材料（来自 CI artifact） | 随各组件同许可 | — |

## B. APK 内的 Kotlin/Compose 依赖（Gradle 依赖树解析）

| 组件 | 许可 |
|---|---|
| Kotlin stdlib / kotlinx.coroutines | Apache-2.0 |
| Jetpack Compose（runtime/foundation/ui）、androidx.activity、androidx.graphics.path | Apache-2.0 |
| Miuix（`top.yukonga.miuix.kmp:miuix-ui-android` / `miuix-icons-android` 0.9.4） | Apache-2.0 |
| material-color-utilities（miuix 间接依赖） | Apache-2.0 |
| AndroidX NavigationEvent（miuix 间接依赖） | Apache-2.0 |

> 完整依赖许可可在构建输出 `app/build/reports/licenses/` 查看（Gradle 依赖树生成）。

## C. magiskboot 的 GPL-3.0 义务（本项目履行方式）

1. **提供 GPL-3.0 全文**：`assets/licenses/GPL-3.0.txt`（随 APK 分发）。
2. **标注来源与二进制指纹**：Magisk v31.1 官方 release
   `https://github.com/topjohnwu/Magisk/releases/download/v31.1/Magisk-v31.1.apk`
   （sha256 `89cefac0a318afa39fb3880c45c87b036c05bc1ebf02a7a715b7f8d7c26ac6e7`），
   解出的 magiskboot sha256 `97e9ec75731103d31c9e1effca7a50c6444309ce8f78c2ec27e9fe721da4bb54`。
3. magiskboot 以**独立可执行程序**形式被 App 以子进程调用，与本项目自有代码聚合分发，
   不改动其源码；若未来修改源码，须公开修改后源码。
4. 需要 magiskboot 源码：见 Magisk 上游仓库（GPL-3.0）。

## D. SKRoot 引擎的 proprietary 声明

`libskrootpatch.so` 中包含源自 `upstream/SKRoot-linuxKernelRoot` 的代码（上游未附许可证，
默认 All Rights Reserved）。项目所有人已确认取得作者授权可调用；**对外分发 APK/二进制前，
必须保存书面授权凭证**。AGPL-3.0 **不覆盖**该部分（详见仓库根 `NOTICE` §1、`LICENSES.md` §2）。

## E. 免责声明

本 App 会修改 boot 镜像，可能导致设备无法开机、数据丢失、保修失效。
使用风险自担；刷机前请备份原厂镜像并确认可回滚。
