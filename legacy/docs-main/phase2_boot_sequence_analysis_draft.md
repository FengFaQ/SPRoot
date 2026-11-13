# SPRoot 启动时序分析：SK 修补内核下三阶段事件的触发与 AP 拉起（联合稿 v0.5）

> 撰写：系统守护（AP 侧） · 联合：内核修补（§4 三项已回复并入） · 状态：**联合稿，待并入总体架构文档（pl_5303149fe0d47573）**
> 输入：APatch @ `e8e4651c` 源码证据；《SKRoot↔AP 调用契约》v1.0（a_43f9e853a06b5109）；内核修补侧结论（2026-11-06 两轮团队消息）。

---

## 1. APatch 现状：三阶段事件是谁拉起的（源码证据）

| 环节 | 机制 | 证据 |
|---|---|---|
| 事件触发 | **KP 内核常驻运行时**在 init 各阶段主动拉起用户态：内核态 exec `/data/adb/apd post-fs-data`（带 superkey 环境注入） | APatch 仓库无 user_event 代码（纯 KernelPatch 内核实现）；KP uapi 仅有 supercall 命令字定义（app/src/main/cpp/uapi/scdefs.h 全文核过，无 EVENT 常量） |
| 事件回执 | apd 各 stage 完成后经 `truncate <key> event <stage> <state>` 把状态报回内核——`truncate` 是 KP 的 su_path 重映射目标（SUPERCMD），被内核劫持为 supercall | apd/src/event.rs:26-41（report_kernel）；APatchApp.kt:52 `SUPERCMD=/system/bin/truncate`；scdefs.h:98 |
| su shell 的 rc | `.aprc` 只是 **root shell 的 ash rc**（ENV 变量注入），与 init 拉起无关 | apd/src/apd.rs:258-260（root_shell 内 `ENV=AP_RC_PATH`）；defs.rs AP_RC_PATH |
| apd 部署 | APK 复制 libapd.so → /data/adb/apd + bin/ 符号链接； su_path 经 JNI `Natives.resetSuPath` 注册 | APatchApp.kt:132-146, 237；Natives.kt:152 |

**结论**：APatch 的三阶段事件驱动权在 KP 内核（常驻运行时 + 内核态 execve 拉起 + truncate 回执通道）。

## 2. SK 修补内核的本质差异（内核修补侧结论，全文采纳）

SK 内核**无任何常驻代码**：五个 hook 点只在被调用时执行，内核不会主动通知/拉起任何用户态进程，也没有 user_event 总线。因此：

> **SK 修补内核零事件能力。三阶段事件驱动权 100% 回到用户态。**

内核侧补充结论（直接约束本设计）：
- 第一个可用提权点 = `/data` 可读之后 + 进程持有密钥（开机早期只读 fs、无密钥文件；内核 Image 内嵌密钥不可读，kcore/dev/mem 早期被 SELinux 拦）；
- 密钥物理载体只能是用户态文件（APK 首装写入的 0600 受保护配置），或 rc 内联（**否决**：明文密钥进 system 分区=公开）。

## 3. SPRoot 设计：AP 作为「用户态事件引擎」

### 3.1 总体链路（定稿方向，内核修补已认同）

```
SK 修补内核开机（hook 活跃、零常驻）
  └→ init（原厂，未修改）按 rc 拉起 AP
       └→ AP post-fs-data：读密钥文件 → execve(key) 自提权（契约 §3.1）
            ├→ 阶段编排① post-fs-data（阻塞）：脚本/modules_update/挂载/sepolicy.rule
            ├→ 阶段② service（阻塞或子进程等待）：service.sh + common service.d
            └→ 阶段③ boot-completed：轮询/等待 sys.boot_completed=1 → boot-completed.sh → fork uid-listener 常驻
```

职责平移：**KP 内核事件总线的职责全部平移给 AP 常驻进程**。uid-listener 已是常驻线程宿主，扩展为事件编排器（或独立编排线程）。零内核改动，保持 SELinux 零触碰 + 无内核常驻。

### 3.2 rc 注入方案（AP 侧立场，需部署器落地）

- **rc 来源**：随 SK 修补 boot 镜像注入（修补引擎/部署器把 `service post-fs-data` 段的脚本写进 ramdisk 的 rc——等价于 KP 的 ramdisk patch 位置，但内容只是拉起 AP，不含密钥）。归属：内核修补/部署器（其消息已表明 install_su 部署器归他们，rc 注入同属部署器职责，本稿交接口径见 §5）。
- **rc 内容约束**：只引用 `/data` 下的路径与二进制，**不内联密钥**；`seclabel` 不写死（沿用 init 默认 context，AP 自提权后不受限）。
- **首次安装闭环**：APK 首装（root 可用后）负责 ① 部署 AP 本体与 bin/ 符号链接（沿用 APatchApp.kt:132-146 模式）；② 写密钥文件 `/data/adb/sproot/key`（0600，root-owned）；③ 写 `/data/adb/sproot/` 下 stage 标志。**修补镜像刷入发生在前、APK 首装在后**——中间存在「rc 已在拉 AP 但 AP/key 未就绪」窗口：AP 必须容忍「本体在但密钥文件缺失」状态=静默退出（等下次 rc 重试），不得报错刷日志（隐蔽性）。

### 3.4 rc 注入方案（v0.5 定稿，内核修补 §4-1）

- **落点**：boot.img ramdisk cpio 内两个文件——① 新建 `init.sproot.rc`（定义 `service sproot_ap /data/adb/apd post-fs-data --superkey-file /data/adb/sproot/key` + `on property:sys.boot_completed` 触发 boot-completed 编排）；② 既有 `init.rc` 末尾**文本级追加**一行 `import /init.sproot.rc`（字节级安全操作；cpio 重打包由 magiskboot cpio add 完成）。不走 kptools embed-extra——那依赖 KP 内核运行时开机解 extra，SK 无运行时，init 直接消费 ramdisk 文件是零内核改动的唯一路径。
- **实测门禁项**：部分厂商 init.rc 结构非标准（import 追加位置）、个别机型 ramdisk second_stage 校验；列真机验证矩阵。
- **rc 内容约束**：不内联密钥；`seclabel u:r:magisk:s0` 建议写入（少一次 setcon 依赖），前提见 §3.5。
- **首次安装闭环**：APK 首装（root 可用后）负责 ① 部署 AP 本体与 bin/ 符号链接（沿用 APatchApp.kt:132-146 模式）；② 写密钥文件 `/data/adb/sproot/key`（0600，root-owned）；③ 写 `/data/adb/sproot/` 下 stage 标志。**修补镜像刷入在前、APK 首装在后**——中间「rc 已在拉 AP 但 AP/key 未就绪」窗口：AP 必须容忍「本体在但密钥文件缺失」=静默退出（等 rc 重试），不得报错刷日志（隐蔽性）。

### 3.5 SELinux context（v0.5 定稿，内核修补 §4-2：avc_denied 吞 denial 机制）

静态推导链（SK「SELinux 零触碰」的实现真意）：
- service 不写 seclabel 时默认跑在 `u:r:init:s0`；提权前 execve(密钥) 的 hook 在 do_execve 入口、任何 LSM 检查之前改写 cred——路径查找返回 ENOENT，全程无有效 AVC。
- 提权后 euid==0 且 caps 全满 → **avc_denied hook 对该进程所有 denial 静默放行（返回 0）**：policy 一字不改，但 root 进程的 denial 结果被吞，功能等价 permissive（仅该进程）。
- 推论：AP 提权后可自由 setcon（`u:r:magisk:s0` 可达），无需 policy 注入——模块 sepolicy.rule 注入（Q4）照常走 magisk_rules 路径，与该机制不冲突（那是往 /sys/fs/selinux/load 写规则，不是依赖 permissive）。
- **实测门禁项**：目标机型 policy 是否含 `magisk` 域（APatch 装机量说明主流有，列入质测兼容矩阵）；无该域回退 = rc 不写 seclabel（init 域起跑 + 提权后 setcon）。

### 3.6 与既有实现的衔接（已完成的 §6 改造如何服务本设计）

- `report_kernel` 已退化为日志（event.rs）——SK 下无 truncate 回执通道，事件状态改由 AP 日志 +（可选）`/data/adb/ap/` 下 stage 完成标志文件承载，供 APK/质测探测。
- `privilege_apd_profile` 已是 execve(key) escalate——启动链直接复用；`--superkey-file` 已实现（cli.rs，读 0600 文件 + 失败静默降级）。
- uid-listener 常驻线程已就位——扩展事件编排器时新增编排线程而非复用（避免 inotify 阻塞影响 stage 时序）。
- root_shell 授权执行点已接线（apd.rs）：请求方 uid 经 SK hook 传递的 `SP_REQUEST_UID` 环境变量还原（hook 侧需在 cred 改写时设置该 env——**交内核修补确认可行性与实现点**；缺省回退 0 仅供 rc 已提权链路），is_authorized 拒绝时静默 exit(1)。

## 4. 部署器交付物清单（内核修补 §4-3，暂名 skroot-deploy）

1. `install_su(key)`：建 `/data/<key前16字符>/` 隐藏目录（label 伪装 system_file）→ 部署 AP multicall 本体副本为 `su` 文件（契约 §3.4 argv[0] 分发）→ 返回 su 路径。
2. `write_key_file(key)`：写 `/data/adb/sproot/key`（0600；mlock 由 AP 侧读取时做）。
3. `inject_rc(bootimg_dir)`：产出 init.sproot.rc + init.rc import 追加 diff 指令；magiskboot cpio 序列由 APK 现有修补链执行（部署器不自带 magiskboot），在 repack 前调用。
4. `uninstall_all(key)`：隐藏目录/key 文件/rc 注入物的反向清单（供 APK 卸载流程）。
5. 交付形态：倾向并入 skroot-patch 引擎同库导出（一个 so 提供 patch + deploy 两族 API，APK 一次加载）——待与客户端会签确认。
6. **补充需求（AP 侧新增）**：SK do_execve hook 在 cred 改写时向新进程注入 `SP_REQUEST_UID=<原uid>` 环境变量（见 §3.6 授权执行点），供 AP root_shell 还原请求方身份。

**时序闭环**（内核修补 §4-3 原文）：APK 修补 boot → 刷入 → 首启 rc 拉起 AP → AP 自提权 → APK 拿到 root 后调 install_su/write_key_file——循环依赖在此解开。

## 5. 交接口径（已对齐，并入总体架构文档 pl_5303149fe0d47573）

- AP 侧承诺（pl_55b89e1161f02d61）：`--superkey-file`（✅ 已实现）、静默退出窗口逻辑、root_shell 授权执行点（✅ 已接线，待 SP_REQUEST_UID 对端）、stage 标志文件（可选）、编排线程扩展。
- 内核修补承诺（skroot-deploy）：rc 注入（✅ 方案已定 §3.4）+ 密钥文件规范 + install_su 部署器 + SP_REQUEST_UID env 注入。
- 双方接口：`/data/adb/sproot/key`（0600，内容=48 位密钥原文）；rc 调用行 `exec /data/adb/apd post-fs-data --superkey-file /data/adb/sproot/key`；`SP_REQUEST_UID` env（hook→AP）。
- 联合实测门禁：init.rc import 追加位置（厂商差异）、机型 policy 的 magisk 域存在性、ramdisk second_stage 校验——三项进质测兼容矩阵。

*版本：v0.1 AP 侧初稿 → v0.5 联合稿（2026-11-06，系统守护 + 内核修补 §4 三项并入：rc 落点/avc_denied 机制/部署器清单）。待双方确认后并入总体架构文档。*
