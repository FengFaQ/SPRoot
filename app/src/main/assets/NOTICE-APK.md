# SPRoot Manager APK — 随包第三方声明（追加页）

> 本页与仓库根 `NOTICE` 一起组装为 APK 内 `assets/NOTICE`（构建期由 CI 合并，单一事实源）。
> 同时提供 `assets/licenses/NOTICE`（同内容）与 `assets/licenses/LICENSE`（本项目 AGPL-3.0 全文）。
> 仓库根 `LICENSE`（AGPL-3.0）也会一并打入 `assets/licenses/AGPL-3.0.txt`。

## 一、本项目许可

- **SPRoot Manager（本 APK 自有代码）** 以 **GNU Affero General Public License v3.0（AGPL-3.0）** 发布。
- 全文随包：`assets/licenses/AGPL-3.0.txt`（= 仓库根 `LICENSE`）；在线地址：
  https://www.gnu.org/licenses/agpl-3.0.txt
- 对应源码：本仓库（v2 分支），构建 commit 见构建日志。

## 二、SKRoot 上游内容 —— 原作者保留所有权利

- **本 App 内嵌的 SKRoot 内核修补引擎**（`lib/arm64-v8a/libskrootpatch.so`）源自
  **SKRoot — https://github.com/abcz316/SKRoot-linuxKernelRoot**（基线 commit `943382cd`）。
- 该上游**未提供 LICENSE/COPYING，源文件无版权头**，因此**版权归 SKRoot 原作者所有，
  保留所有权利（All Rights Reserved）**。
- 本项目**未改动其许可状态**：**AGPL-3.0 不适用于该部分**（`libskrootpatch.so` 中的 SKRoot 代码）。
- **使用授权**：项目所有人已确认**已获原作者授权**可用于本项目；对外分发 APK/二进制前
  须保存书面授权凭证（详见仓库根 `NOTICE` §1 与 `LICENSES.md` §2 的逐文件 sha256 指纹）。
- 该项目自有代码（Kotlin/Compose UI、管线、AP 客户端等）与 SKRoot 部分可分割，后者不传染前者。

## 三、magiskboot（Magisk v31.1，GPL-3.0）

- 来源：Magisk 官方 release v31.1
  `https://github.com/topjohnwu/Magisk/releases/download/v31.1/Magisk-v31.1.apk`
  （APK sha256 `89cefac0a318afa39fb3880c45c87b036c05bc1ebf02a7a715b7f8d7c26ac6e7`）
- 随包二进制：`lib/arm64-v8a/libmagiskboot.so`（988,984 B，
  sha256 `97e9ec75731103d31c9e1effca7a50c6444309ce8f78c2ec27e9fe721da4bb54`），
  保持 `lib*.so` 命名以便 Android 安装时解压到 `nativeLibraryDir` 后以子进程执行。
- 许可：**GPL-3.0**；全文随包：`assets/licenses/GPL-3.0.txt`。
- 源码：https://github.com/topjohnwu/Magisk （tag `v31.1`）。
- 分发方式：magiskboot 作为**独立可执行程序**被 App 以子进程调用，属于聚合分发，
  **不传染本项目自有代码**；本项目未修改其源码（若未来修改，将公开修改后源码）。
- 机器可读元数据：`assets/magiskboot-meta.txt`（版本/来源/sha256）。

## 四、其他第三方组件

| 组件 | 位置/形态 | 许可 |
|---|---|---|
| AndroidX（activity、graphics.path 等） | APK 内 jar/so | Apache-2.0 |
| Jetpack Compose（runtime/foundation/ui） | APK 内 jar | Apache-2.0 |
| Kotlin stdlib / kotlinx.coroutines | APK 内 jar | Apache-2.0 |
| Miuix 0.9.4（`top.yukonga.miuix.kmp`） | APK 内 jar | Apache-2.0 |
| material-color-utilities / NavigationEvent（miuix 间接依赖） | APK 内 jar | Apache-2.0 |
| **asmjit** | 编译进 `libskrootpatch.so`（AArch64 汇编器） | **Zlib** |
| **capstone 5.0.7** | 编译进 `libskrootpatch.so`（反汇编） | **BSD-3-Clause** |
| **cJSON** | 编译进 `libskrootpatch.so`（JSON 序列化） | **MIT** |
| AP 守护 `libapd.so`（SPRoot 自有，Rust） | APK 内 so | AGPL-3.0（同本项目） |
| 构建期材料 `ENGINE-META.txt` / `SYMBOLS-libskrootpatch.txt` / `SHA256SUMS` / `apd-version.txt` | `assets/` | 随各组件同许可 |

完整依赖许可可由构建产物 `app/build/reports/licenses/` 或仓库根 `LICENSES.md` 追溯。

## 五、免责声明

本 App 会修改 boot 镜像，可能导致设备无法开机、数据丢失、保修失效。使用风险自担；
刷机前请备份原厂镜像并确认可回滚（见 `docs/NEW-DOC-02` §7.4）。
