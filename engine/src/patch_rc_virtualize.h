// patch_rc_virtualize.h — rc 虚拟化（APatch 路线）的纯逻辑层 + shellcode 生成器
//
// 背景与设计依据：docs/NEW-DOC-02 §11。
//   机制：hook `__arm64_sys_openat`，把 init 对 rc 文件的打开重定向到 `/dev/user_init.rc`
//   （内容 = 原 rc 全文 + 追加模板）→ init 自己在 post-fs-data / boot_completed 时机 exec apd，
//   从而**彻底免 init_boot**（不再需要 ramdisk rc 注入）。
//
// 铁律（Lead 拍板，务必保持）：
//   1) **禁止硬编码代码洞地址**：必须 patch 时扫描「连续 0x00 ≥ 需求」的洞，
//      并断言洞在 [_stext,_etext) 内、区内逐字节为 0；洞清单与选中洞进 report。
//   2) 默认 off：`--launcher rc` 才织入；off 时产物与当前逐字节一致。
//   3) 失败降级：符号缺失 / 无洞 / 洞非法 → 整组不织入（其它 patch 照常）；
//      显式开启但前置不满足 → E202 且不产出 --out。
//
// clean-room：机制参考 KernelPatch/APatch（GPL-2.0-or-later）的理解，代码为本项目自研。
#pragma once
#include <cstddef>
#include <string>
#include <utility>
#include <vector>
#include "patch_base.h"

// ---------------------------------------------------------------- 常量（唯一来源）
// 最小可用洞大小：见 NEW-DOC-02 §11.4 体积预算（≈760–900 B，留 ≥2× 余量）
static constexpr size_t RC_VIRT_MIN_CAVE_BYTES = 2048;
// 单次读取上限：init.rc 实测 60–100 KB。读到上限即视为"可能被截断" → 放弃重定向
// （截断 rc 会破坏 init 解析 → 开机失败，属最高等级事故；见 patch_rc_virtualize.cpp 的安全闸）。
static constexpr size_t RC_VIRT_READ_MAX_BYTES = 512 * 1024;
// report 中最多列出多少个洞（真机排障用）
static constexpr size_t RC_VIRT_MAX_REPORT_CAVES = 16;

// 会被重定向的原 rc（顺序即匹配优先级）。
// 注意：`/init.rc` 只有 9 字节 < 替换串 19 字节 → **故意不处理**（GKI 机型无此文件；
// 要处理它就必须改用户栈指针，多一条链且真机风险高）。
extern const char* const RC_VIRT_ORIG_PATHS[2];
// 替换目标（同时用于 filp_open 写 与 覆写用户 path 串）
extern const char* const RC_VIRT_REPLACE_PATH;

// 追加到 init.rc 末尾的模板（唯一来源；语义等价于 apd/assets/init.sproot.rc）
std::string rc_virt_template();
size_t rc_virt_template_len();

// 自检用：本机制**必需**的符号名清单（顺序稳定；用于 A11 空断言防护：数量下限 + 逐个在场）
std::vector<const char*> rc_virt_required_symbols();

// ---------------------------------------------------------------- 纯逻辑（可离线自检）
struct RcVirtCave {
	size_t offset = 0;
	size_t size = 0;
};

// 在 [lo, hi) 中扫描连续 0x00 的洞（起点 4 字节对齐、大小 >= min_size），按出现顺序返回。
// 纯函数：无 IO、无全局状态 —— 供主流程与 --self-test-rcvirtual 共用同一份实现。
std::vector<RcVirtCave> rc_virt_scan_caves(const std::vector<char>& buf, size_t lo, size_t hi,
                                           size_t min_size, size_t max_out = RC_VIRT_MAX_REPORT_CAVES);

// 数据区布局（blob 相对偏移）。不变量：替换串必须严格短于每个被覆写的原路径串
// （因为我们是**原地覆写用户 path**，而不是改用户栈指针）。
struct RcVirtDataLayout {
	size_t off_orig[2] = { 0, 0 };
	size_t off_replace = 0;
	size_t off_template = 0;
	size_t data_len = 0;   // 已 4 字节对齐
	bool ok = false;
	std::string reason;
};
RcVirtDataLayout rc_virt_data_layout();

// 织入结果（report 的 launcher{} 直接由此构造）
struct RcVirtInfo {
	std::string mode = "off";     // off | rc | umh
	bool enabled = false;         // 是否真正织入
	std::string reason;           // 未织入原因（enabled=false 时必填）
	size_t cave_offset = 0;
	size_t cave_size = 0;
	size_t shellcode_bytes = 0;   // blob 总字节（含数据区）
	size_t template_bytes = 0;
	size_t hook_addr = 0;         // 被 hook 的 __arm64_sys_openat 入口（PAC/BTI 之后）
	std::vector<RcVirtCave> caves;                         // 扫描到的全部洞（最多 16）
	std::vector<std::pair<std::string, size_t>> symbols;   // 已解析符号 -> 地址
};

// ---------------------------------------------------------------- 生成器
class PatchRcVirtualize : public PatchBase {
public:
	PatchRcVirtualize(const PatchBase& patch_base, const KernelSymbolOffset& sym);
	~PatchRcVirtualize();

	// 检查前置条件（符号齐全 + kernel_read/write 新签名 + 数据布局安全 + 有合法洞）。
	// 不满足时返回 false，info.reason 说明原因（不抛异常、不写盘）。
	bool precheck(size_t cred_offset, RcVirtInfo& info);

	// 生成 blob 并放置到洞；同时把 openat 入口改成跳转。
	// 取 info.caves 中第一个能容纳 blob 的洞；都不行则返回 false（out_info.reason 说明）。
	bool weave(size_t cred_offset, size_t cred_uid_offset, const RcVirtInfo& info,
	           std::vector<patch_bytes_data>& vec_out, RcVirtInfo& out_info);

private:
	std::vector<uint8_t> build_blob(size_t blob_base, size_t hook_addr, size_t cred_offset,
	                                size_t cred_uid_offset, size_t& restore_insn_offset);

	KernelSymbolOffset m_sym;
};
