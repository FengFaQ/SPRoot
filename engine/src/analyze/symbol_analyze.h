#pragma once
#include "kernel_symbol_finder.h"
#include <iostream>
#include <vector>

struct KernelSymbolOffset {
	size_t _text = 0;
	size_t _stext = 0;
	SymbolRegion die = { 0 };
	SymbolRegion __drm_puts_coredump = { 0 };
	SymbolRegion __drm_printfn_coredump = { 0 };

	SymbolRegion __do_execve_file = { 0 };
	SymbolRegion do_execveat_common = { 0 };
	SymbolRegion do_execve_common = { 0 };
	SymbolRegion do_execveat = { 0 };
	SymbolRegion do_execve = { 0 };

	SymbolRegion avc_denied = { 0 };
	SymbolRegion audit_log_start = { 0 };
	SymbolRegion filldir64 = { 0 };
	SymbolRegion compat_filldir = { 0 };

	SymbolRegion sys_getuid = { 0 };
	SymbolRegion prctl_get_seccomp = { 0 };

	// rc 虚拟化（--launcher rc，见 patch_rc_virtualize.h / NEW-DOC-02 §11）
	size_t _etext = 0;
	SymbolRegion openat_sys = { 0 };
	SymbolRegion str_cmp = { 0 };
	SymbolRegion strncpy_from_user_fn = { 0 };
	SymbolRegion filp_open_fn = { 0 };
	SymbolRegion kernel_read_fn = { 0 };
	SymbolRegion kernel_write_fn = { 0 };
	SymbolRegion filp_close_fn = { 0 };
	SymbolRegion vmalloc_fn = { 0 };
	SymbolRegion vfree_fn = { 0 };
	SymbolRegion copy_to_user_fn = { 0 };

	SymbolRegion __cfi_check = { 0 };
	size_t __cfi_check_fail = 0;
	size_t __cfi_slowpath_diag = 0;
	size_t __cfi_slowpath = 0;
	size_t __ubsan_handle_cfi_check_fail_abort = 0;
	size_t __ubsan_handle_cfi_check_fail = 0;
	size_t report_cfi_failure = 0;

	//huawei
	size_t hkip_check_uid_root = 0;
	size_t hkip_check_gid_root = 0;
	size_t hkip_check_xid_root = 0;
	SymbolRegion kti_randomize_init = { 0 };
};

class SymbolAnalyze
{
public:
	SymbolAnalyze(const std::vector<char> & file_buf);
	~SymbolAnalyze();
	bool analyze_kernel_symbol();
	KernelSymbolOffset get_symbol_offset();
	std::unordered_map<std::string, uint64_t> get_all_symbols();
private:
	bool find_symbol_offset();
	void printf_symbol_offset();

	const std::vector<char>& m_file_buf;
	KernelSymbolFinder m_sym_finder;
	KernelSymbolOffset m_sym_offset;
};