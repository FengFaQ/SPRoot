// patch_pipeline.h — SPRoot 修补管线接口
// 从上游 patch_kernel_root.cpp 抽出的管线层（去 main 化重构），逻辑与上游 master@943382cd 逐行等价
#pragma once
#include <cstdint>
#include <cstddef>
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
inline size_t hook_groups_expected_count(bool kernel_ge_6_1, bool has_huawei_symbols,
                                         bool launcher_rc = false) {
	return (kernel_ge_6_1 ? 14u : 11u) + (has_huawei_symbols ? 1u : 0u) + (launcher_rc ? 1u : 0u);
}

// 内核是否 >= 6.1（与 patch_kernel_handler 内部选路判据保持一致）
bool hook_branch_ge_6_1(const std::vector<char>& file_buf);
// 内核是否存在华为 hkip_check_*_root 符号族（决定是否多一组 huawei_bypass）
bool hook_has_huawei_symbols(const KernelSymbolOffset& sym);
