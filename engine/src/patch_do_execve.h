#pragma once
#include <iostream>
#include <vector>
#include "patch_base.h"

#pragma pack(push, 1)
struct ExecveParam {
	uint32_t do_execve_addr = 0;
	uint8_t do_execve_filename_reg = 0;
	bool     is_single_char_ptr = false;
};
#pragma pack(pop)

class PatchDoExecve : public PatchBase
{
public:
	PatchDoExecve(const PatchBase& patch_base, const KernelSymbolOffset &sym);
	~PatchDoExecve();

	size_t patch_do_execve(const SymbolRegion& hook_func_start_region, size_t task_struct_cred_offset, size_t task_struct_seccomp_offset,
		std::vector<patch_bytes_data>& vec_out_patch_bytes_data);

	// R1：候选解析是否成功（符号在且通过序言白名单）。false 时 patch_do_execve
	// 前置短路返回 0，管线报 E202（替代上游 IF_EXIT exit(0) 伪成功）。
	bool valid() const { return m_do_execve_valid; }

private:
	void init_do_execve_param(const KernelSymbolOffset& sym);
	int get_need_write_cap_cnt();

	ExecveParam m_doexecve_reg_param = { 0 };
	bool m_do_execve_valid = false;
	bool m_best_preferred = false;   // R2①：带选候选是否已锁定（锁定后 size-max 兜底不参与）
};