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
PatchKernelResult patch_kernel_handler(const std::vector<char>& file_buf, const PatchKernelOffset& off, KernelSymbolOffset& sym, std::vector<patch_bytes_data>& vec_patch_bytes_data);
