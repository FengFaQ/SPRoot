// patch_pipeline.h — SPRoot 修补管线接口
// 从上游 patch_kernel_root.cpp 抽出的管线层（去 main 化重构），逻辑与上游 master@943382cd 逐行等价
#pragma once
#include <cstdint>
#include <cstddef>
#include <vector>
#include "patch_kernel_root.h"
#include "analyze/symbol_analyze.h"

struct PatchKernelOffset {
	size_t cred_offset = 0;
	size_t cred_uid_offset = 0;
	size_t seccomp_offset = 0;
	uint64_t huawei_kti_addr = 0;
};

struct PatchKernelResult {
	bool patched = false;
	size_t root_key_start = 0;
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
PatchKernelResult patch_kernel_handler(const std::vector<char>& file_buf, const PatchKernelOffset& off, KernelSymbolOffset& sym, std::vector<patch_bytes_data>& vec_patch_bytes_data, std::vector<PatchGroupSpan>* groups = nullptr);
