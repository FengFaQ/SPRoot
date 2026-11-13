// patch_pipeline.h — SPRoot 修补管线接口
// 从上游 patch_kernel_root.cpp 抽出的管线层（去 main 化重构），逻辑与上游 master@943382cd 逐行等价
#pragma once
#include <cstdint>
#include <cstddef>
#include <string>
#include <vector>
#include "patch_kernel_root.h"
#include "analyze/symbol_analyze.h"
#include "patch_rc_virtualize.h"

struct PatchKernelOffset {
	size_t cred_offset = 0;
	size_t cred_uid_offset = 0;
	size_t seccomp_offset = 0;
	uint64_t huawei_kti_addr = 0;
};

// 可选启动器（默认 off；--launcher off|rc|umh，见 NEW-DOC-02 §9.4 / §11）
struct LauncherOptions {
	std::string mode = "off";
	bool rc() const { return mode == "rc"; }
};

struct PatchKernelResult {
	bool patched = false;
	size_t root_key_start = 0;
	RcVirtInfo launcher;   // rc 虚拟化的结果（mode/enabled/reason/cave/符号），供 report 使用
};

// 结构体偏移动态推导（A1 §2 步骤 3）
bool parser_cred_offset(const std::vector<char>& file_buf, const SymbolRegion& symbol, size_t& cred_offset);
bool parse_cred_uid_offset(const std::vector<char>& file_buf, const SymbolRegion& symbol, size_t cred_offset, size_t& cred_uid_offset);
bool parser_seccomp_offset(const std::vector<char>& file_buf, const SymbolRegion& symbol, size_t& seccomp_offset);
bool parser_huawei_kti_addr(const std::vector<char>& file_buf, const SymbolRegion& symbol, uint64_t& kti_addr);

// 稳定性 bypass（A1 §6）
void cfi_bypass(const std::vector<char>& file_buf, KernelSymbolOffset& sym, std::vector<patch_bytes_data>& vec_patch_bytes_data);
void huawei_bypass(const std::vector<char>& file_buf, KernelSymbolOffset& sym, std::vector<patch_bytes_data>& vec_patch_bytes_data);

// 五组 hook 织入（A1 §7）
// groups 非空时，按"组"回填每组的 patch blob 下标区间（供 report.json 的 hooks[] 逐 hook 落位，
// C1 §2.3；非破坏性新增，不影响原有 hook_table 聚合字段）
struct PatchGroupSpan {
	const char* group;    // do_execve / filldir64 / compat_filldir / avc_denied / audit_log_start / ...
	const char* symbol;   // 目标符号（人类可读）
	size_t target_addr;   // 补丁作用地址（0 = 动态解析/未定）
	size_t first;         // vec_patch 起始下标
	size_t last;          // vec_patch 结束下标（不含）
};
PatchKernelResult patch_kernel_handler(const std::vector<char>& file_buf, const PatchKernelOffset& off, KernelSymbolOffset& sym, std::vector<patch_bytes_data>& vec_patch_bytes_data, std::vector<PatchGroupSpan>* groups = nullptr, const LauncherOptions& launcher = LauncherOptions{});

// 期望组数（**唯一来源**，供 report.hook_groups_expected 与自检共用；不要在各处硬编码）：
//   ≥6.1 分支：cfi + drm_printfn + drm_puts + do_execve + filldir64×3 + compat_filldir×3
//              + avc_check + avc_denied + audit_log_start + embed_key = 14
//   <6.1 分支：cfi + do_execve + filldir64×2 + compat_filldir×2 + avc_check + avc_denied
//              + audit_log_start + hook_jump + embed_key               = 11
//   + huawei_bypass（内核存在 hkip_check_*_root 符号时 +1）
//   + rc_virtualize（--launcher rc 显式开启时 +1；默认 off 时不变）
//
// ★ 自解释（task-16）：只返回一个数字无法区分「≥6.1 且无 huawei 的 14」与「<6.1 且
//   huawei+rc 的 14」，也无法解释魅族21 实测 15 组的成因。故把推导过程显式化为
//   HookGroupsExpectedBreakdown，report.hook_groups_expected_breakdown 直接由它序列化。
//   total 由各分项求和得出，hook_groups_expected_count() 也只走这一条路径
//   （见下方 .total 的使用），**不存在第二处算数**。
//
// 【如何用 breakdown 对账「魅族21 实测 15 组 vs 基线 14」（真机复核 SOP）】
//   拿到报告先看 hook_groups_expected_breakdown，按顺序回答三个问题：
//   ① kernel_ge_6_1 / branch_base：是否 >=6.1 的 14 基线？若 branch_base==11 说明版本判到
//      <6.1 分支（镜像与机型不符，或版本串解析错了），15 就与 huawei 无关。
//   ② huawei_symbols_detected：**空数组 = 该内核没有 hkip_check_*_root 符号族**，
//      此时 huawei_bypass 必须是 0；若数组非空则逐项列出命中的符号名（uid/gid/xid 三个
//      各自独立），huawei_bypass 必须是 1（该族存在就多织一组，不按符号个数叠加）。
//      魅族21 若真是 15 组：预期 huawei_symbols_detected 非空 → huawei_bypass=1。
//      若该数组为空却仍然 15：说明 +1 来自 launcher_rc，不是华为符号，需查 --launcher 入参。
//   ③ launcher_rc：0 = 默认 off；1 = 显式 --launcher rc（rc_virtualize 多一组）。
//   最后核 total == branch_base + huawei_bypass + launcher_rc，
//   且 total == report.hook_groups_expected（两者不一致即是本引擎的 bug，自检会红）。
//   这样 15 到底差在 huawei 还是 rc，不必读引擎源码即可判定。
struct HookGroupsExpectedBreakdown {
	bool        kernel_ge_6_1 = true;    // 分支判据（与 patch_kernel_handler 同一判据）
	unsigned    branch_base = 14;        // >=6.1 → 14；<6.1 → 11
	unsigned    huawei_bypass = 0;       // 存在 hkip_check_*_root 符号族时 +1（不按符号数叠加）
	unsigned    launcher_rc = 0;         // --launcher rc 显式开启时 +1
	// 实际命中的符号名（不是布尔）。空数组 = 一个都没命中，即该内核无此保护路径。
	// 顺序固定 uid→gid→xid，便于与 kallsyms dump / 上游源码逐条对账。
	std::vector<std::string> huawei_symbols_detected;
	// huawei_bypass 这个 +1 的依据，三选一：
	//   "detected" 真的从 kallsyms 命中（此时 huawei_symbols_detected 必非空）
	//   "assumed"  调用方用 --assume-huawei-symbols 强制假定（真机没有符号也当有）
	//   "none"     未命中，也不假定
	// 单列此字段是因为「假定」与「实测」在真机复核时结论完全不同：前者不能用来证明
	// 魅族21 真有华为符号，后者才是证据。
	std::string huawei_bypass_source = "none";

	// 唯一算数入口：分项之和。report.hook_groups_expected 与自检都取这个值。
	unsigned total() const {
		return branch_base + huawei_bypass + launcher_rc;
	}
	// 自洽性：分支基数对、且 huawei_bypass 与"依据"严格一致。
	// 防的正是「命中却写 0」「没命中却写 1」这类会让 14/15 对不上的静默漂移。
	bool consistent() const {
		if (branch_base != (kernel_ge_6_1 ? 14u : 11u)) return false;
		if (huawei_bypass_source == "detected")
			return huawei_bypass == 1u && !huawei_symbols_detected.empty();
		if (huawei_bypass_source == "assumed")
			return huawei_bypass == 1u;              // 假定：不要求有实测符号名
		if (huawei_bypass_source == "none")
			return huawei_bypass == 0u && huawei_symbols_detected.empty();
		return false;                                 // 未知来源 ⇒ 判不自洽（防漏判）
	}
};

// 由判据构造 breakdown（**唯一来源**）：给出分支、命中符号名、是否 rc，其余字段推导。
// huawei_symbols_detected 由调用方填入实际命中项；huawei_bypass 恒 = (数组非空 或 assumed)。
inline HookGroupsExpectedBreakdown hook_groups_expected_breakdown(
		bool kernel_ge_6_1, const std::vector<std::string>& huawei_symbols_detected,
		bool launcher_rc, bool huawei_assumed = false) {
	HookGroupsExpectedBreakdown b;
	b.kernel_ge_6_1 = kernel_ge_6_1;
	b.branch_base = kernel_ge_6_1 ? 14u : 11u;
	b.huawei_symbols_detected = huawei_symbols_detected;
	if (!huawei_symbols_detected.empty()) {
		b.huawei_bypass = 1u;
		b.huawei_bypass_source = "detected";
	} else if (huawei_assumed) {
		b.huawei_bypass = 1u;
		b.huawei_bypass_source = "assumed";
	} else {
		b.huawei_bypass = 0u;
		b.huawei_bypass_source = "none";
	}
	b.launcher_rc = launcher_rc ? 1u : 0u;
	return b;
}

inline size_t hook_groups_expected_count(bool kernel_ge_6_1, bool has_huawei_symbols,
                                         bool launcher_rc = false) {
	// 单一来源：只走 breakdown.total()，不在此处另写一份算式。
	// 本重载只关心"有没有 huawei"这一个布尔（调用方通常还不知道具体符号名），
	// 故用 huawei_assumed 表达"按假定有"，其内容不对外传递
	// （report 走 hook_huawei_symbols_detected() 拿真实符号名）。
	return hook_groups_expected_breakdown(kernel_ge_6_1, /*huawei_symbols_detected*/{},
	                                     launcher_rc, /*huawei_assumed*/has_huawei_symbols).total();
}

// 内核是否 >= 6.1（与 patch_kernel_handler 内部选路判据保持一致）
bool hook_branch_ge_6_1(const std::vector<char>& file_buf);
// cred 布局第二锚点：在 anchor2（如 prctl_get_seccomp）区间里独立推导
// task_struct.cred load offset；返回 0 = 锚点不可用/未命中（report 降级说明）。
size_t hook_cred_offset_second_anchor(const std::vector<char>& file_buf,
                                      const SymbolRegion& anchor2);
// 第二锚点符号解析：first=__arm64_sys_geteuid（当前恒空，kallsyms 接口未含），
// second=prctl_get_seccomp（主流程必查符号，存在性有保障）。main 取非空者。
std::pair<SymbolRegion, SymbolRegion> hook_second_anchor_symbols(const std::vector<char>& file_buf);
// 内核是否存在华为 hkip_check_*_root 符号族（决定是否多一组 huawei_bypass）
bool hook_has_huawei_symbols(const KernelSymbolOffset& sym);
// 实际命中的 huawei 符号名（空 = 一个都没命中）。供 report 的 breakdown 列出具名证据。
std::vector<std::string> hook_huawei_symbols_detected(const KernelSymbolOffset& sym);
