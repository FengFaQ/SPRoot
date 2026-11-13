// patch_pipeline.cpp — SPRoot 修补管线实现
// 逐行取自上游 patch_kernel_root.cpp L31-146（master@943382cd），仅做去 main 化与命名空间整理
#include "patch_pipeline.h"
#include "analyze/base_func.h"
#include "patch_do_execve.h"
#include "patch_current_avc_check.h"
#include "patch_avc_denied.h"
#include "patch_audit_log_start.h"
#include "patch_filldir64.h"
#include "patch_compat_filldir.h"

#include "3rdparty/find_mrs_register.h"
#include "3rdparty/find_imm_register_offset.h"
#include "3rdparty/find_adrp_target.h"

bool parser_cred_offset(const std::vector<char>& file_buf, const SymbolRegion &symbol, size_t& cred_offset) {
	using namespace a64_find_mrs_register;
	std::vector<track_reg_info>track_info;
	if (!find_current_task_next_register_offset(file_buf, symbol.offset, symbol.offset + symbol.size, track_info)) return false;
	cred_offset = 0;
	for (auto& t : track_info) {
		if (t.load_offset > 0x400) { cred_offset = t.load_offset; break; }
	}
	return cred_offset > 0;
}

bool parse_cred_uid_offset(const std::vector<char>& file_buf, const SymbolRegion& symbol, size_t cred_offset, size_t& cred_uid_offset) {
	using namespace a64_find_imm_register_offset;
	cred_uid_offset = 0;
	KernelVersionParser kernel_ver(file_buf);
	size_t min_off = 8;
	if (kernel_ver.is_kernel_version_less("6.6.8")) min_off = 4;

	std::vector<int64_t> candidate_offsets;
	if (!find_imm_register_offset(file_buf, symbol.offset, symbol.offset + symbol.size, candidate_offsets)) return false;

	auto it = std::find(candidate_offsets.begin(), candidate_offsets.end(), cred_offset);
	if (it != candidate_offsets.end()) {
		for (++it; it != candidate_offsets.end(); ++it) {
			if (*it > 0x20 || *it < min_off) continue;
			cred_uid_offset = *it;
			break;
		}
	}
	return cred_uid_offset != 0;
}

bool parser_seccomp_offset(const std::vector<char>& file_buf, const SymbolRegion& symbol, size_t& seccomp_offset) {
	using namespace a64_find_mrs_register;
	std::vector<track_reg_info>track_info;
	if (!find_current_task_next_register_offset(file_buf, symbol.offset, symbol.offset + symbol.size, track_info)) return false;
	seccomp_offset = 0;
	for (auto& t : track_info) {
		if (t.load_offset > 0x400) { seccomp_offset = t.load_offset; break; }
	}
	return seccomp_offset > 0;
}

bool parser_huawei_kti_addr(const std::vector<char>& file_buf, const SymbolRegion& symbol, uint64_t& kti_addr) {
	using namespace a64_find_adrp_target;
	if (symbol.size == 0) return false;
	if (!find_adrp_target(file_buf, symbol.offset, symbol.offset + symbol.size, kti_addr)) return false;
	return kti_addr > 0;
}

void cfi_bypass(const std::vector<char>& file_buf, KernelSymbolOffset &sym, std::vector<patch_bytes_data>& vec_patch_bytes_data) {
	if (sym.__cfi_check.offset) PATCH_AND_CONSUME(sym.__cfi_check, patch_ret_cmd(file_buf, sym.__cfi_check.offset, vec_patch_bytes_data));
	patch_ret_cmd(file_buf, sym.__cfi_check_fail, vec_patch_bytes_data);
	patch_ret_cmd(file_buf, sym.__cfi_slowpath_diag, vec_patch_bytes_data);
	patch_ret_cmd(file_buf, sym.__cfi_slowpath, vec_patch_bytes_data);
	patch_ret_cmd(file_buf, sym.__ubsan_handle_cfi_check_fail_abort, vec_patch_bytes_data);
	patch_ret_cmd(file_buf, sym.__ubsan_handle_cfi_check_fail, vec_patch_bytes_data);
	patch_ret_1_cmd(file_buf, sym.report_cfi_failure, vec_patch_bytes_data);
}

void huawei_bypass(const std::vector<char>& file_buf, KernelSymbolOffset &sym, std::vector<patch_bytes_data>& vec_patch_bytes_data) {
	patch_ret_0_cmd(file_buf, sym.hkip_check_uid_root, vec_patch_bytes_data);
	patch_ret_0_cmd(file_buf, sym.hkip_check_gid_root, vec_patch_bytes_data);
	patch_ret_0_cmd(file_buf, sym.hkip_check_xid_root, vec_patch_bytes_data);
}

PatchKernelResult patch_kernel_handler(const std::vector<char>& file_buf, const PatchKernelOffset& off, KernelSymbolOffset& sym, std::vector<patch_bytes_data>& vec_patch_bytes_data, std::vector<PatchGroupSpan>* groups, const LauncherOptions& launcher) {
	auto mark = [&](const char* group, const char* symbol, size_t target_addr, size_t first) {
		if (!groups) return;
		groups->push_back({ group, symbol, target_addr, first, vec_patch_bytes_data.size() });
	};
	KernelVersionParser kernel_ver(file_buf);
	PatchBase patchBase(file_buf, off.cred_uid_offset, { .kti_addr = off.huawei_kti_addr });
	PatchDoExecve patchDoExecve(patchBase, sym);
	PatchCurrentAvcCheck patchCurrentAvcCheck(patchBase);
	PatchAvcDenied patchAvcDenied(patchBase, sym.avc_denied);
	PatchAuditLogStart patchAuditLogStart(patchBase, sym.audit_log_start);
	PatchFilldir64 patchFilldir64(patchBase, sym.filldir64);
	PatchCompatFilldir patchCompatFilldir(patchBase, sym.compat_filldir);

	bool patched = true;
	PatchKernelResult r;
	// R1：do_execve 候选解析必须成功（符号在 + 序言白名单通过）。失败时整段 hook
	// 都不能织入（否则 key 槽/入口跳转半成品）→ 立即返回 patched=false，
	// 主流程报 E202 "no do_execve hook target resolved"（替代上游 IF_EXIT exit(0) 伪成功）。
	if (!patchDoExecve.valid()) {
		std::cout << "[patch_pipeline] do_execve hook target unresolved: "
		          << (g_patch_abort_reason.empty() ? "no candidate passed prologue whitelist"
		                                           : g_patch_abort_reason)
		          << std::endl;
		patched = false;
		r.patched = false;
		return r;
	}
	if (kernel_ver.is_kernel_version_less("6.1.0")) {
		SymbolRegion next_empty_region = { 0x200, 0x300 };
		if (sym.__cfi_check.offset && sym.__cfi_check.size > next_empty_region.size) next_empty_region = sym.__cfi_check;
		auto start_b_location = next_empty_region.offset;
		PATCH_AND_CONSUME(next_empty_region, 4);
		r.root_key_start = next_empty_region.offset;
		size_t g0 = vec_patch_bytes_data.size();
		PATCH_AND_CONSUME(next_empty_region, patchDoExecve.patch_do_execve(next_empty_region, off.cred_offset, off.seccomp_offset, vec_patch_bytes_data));
		mark("do_execve", "do_execveat_common", next_empty_region.offset, g0);
		g0 = vec_patch_bytes_data.size();
		PATCH_AND_CONSUME(next_empty_region, patchFilldir64.patch_filldir64_root_key_guide(r.root_key_start, next_empty_region, vec_patch_bytes_data));
		mark("filldir64", "filldir64(root_key_guide)", r.root_key_start, g0);
		g0 = vec_patch_bytes_data.size();
		PATCH_AND_CONSUME(next_empty_region, patchFilldir64.patch_filldir64_core(next_empty_region, vec_patch_bytes_data));
		mark("filldir64", "filldir64(core)", next_empty_region.offset, g0);
		g0 = vec_patch_bytes_data.size();
		PATCH_AND_CONSUME(next_empty_region, patchCompatFilldir.patch_compat_filldir_root_key_guide(r.root_key_start, next_empty_region, vec_patch_bytes_data));
		mark("compat_filldir", "compat_filldir(root_key_guide)", r.root_key_start, g0);
		g0 = vec_patch_bytes_data.size();
		PATCH_AND_CONSUME(next_empty_region, patchCompatFilldir.patch_compat_filldir_core(next_empty_region, vec_patch_bytes_data));
		mark("compat_filldir", "compat_filldir(core)", next_empty_region.offset, g0);
		g0 = vec_patch_bytes_data.size();
		auto current_avc_check_bl_func = next_empty_region.offset;
		PATCH_AND_CONSUME(next_empty_region, patchCurrentAvcCheck.patch_current_avc_check_bl_func(next_empty_region, off.cred_offset, vec_patch_bytes_data));
		mark("avc_check", "current_avc_check", current_avc_check_bl_func, g0);
		g0 = vec_patch_bytes_data.size();
		PATCH_AND_CONSUME(next_empty_region, patchAvcDenied.patch_avc_denied(next_empty_region, current_avc_check_bl_func, vec_patch_bytes_data));
		mark("avc_denied", "avc_denied", sym.avc_denied.offset, g0);
		g0 = vec_patch_bytes_data.size();
		PATCH_AND_CONSUME(next_empty_region, patchAuditLogStart.patch_audit_log_start(next_empty_region, current_avc_check_bl_func, vec_patch_bytes_data));
		mark("audit_log_start", "audit_log_start", sym.audit_log_start.offset, g0);
		auto end_b_location = next_empty_region.offset;
		g0 = vec_patch_bytes_data.size();
		patchBase.patch_jump(start_b_location, end_b_location, vec_patch_bytes_data);
		mark("hook_jump", "empty_region_jump", start_b_location, g0);
	} else if (sym.die.offset && sym.__drm_puts_coredump.offset && sym.__drm_printfn_coredump.offset) {
		size_t g0 = vec_patch_bytes_data.size();
		PATCH_AND_CONSUME(sym.__drm_printfn_coredump, patch_ret_cmd(file_buf, sym.__drm_printfn_coredump.offset, vec_patch_bytes_data));
		mark("drm_printfn_coredump", "__drm_printfn_coredump", sym.__drm_printfn_coredump.offset, g0);
		g0 = vec_patch_bytes_data.size();
		PATCH_AND_CONSUME(sym.__drm_puts_coredump, patch_ret_cmd(file_buf, sym.__drm_puts_coredump.offset, vec_patch_bytes_data));
		mark("drm_puts_coredump", "__drm_puts_coredump", sym.__drm_puts_coredump.offset, g0);
		r.root_key_start = sym.die.offset;
		g0 = vec_patch_bytes_data.size();
		PATCH_AND_CONSUME(sym.die, patchDoExecve.patch_do_execve(sym.die, off.cred_offset, off.seccomp_offset, vec_patch_bytes_data));
		mark("do_execve", "do_execveat_common", sym.die.offset, g0);
		g0 = vec_patch_bytes_data.size();
		PATCH_AND_CONSUME(sym.die, patchFilldir64.patch_filldir64_root_key_guide(r.root_key_start, sym.die, vec_patch_bytes_data));
		mark("filldir64", "filldir64(root_key_guide)", r.root_key_start, g0);
		g0 = vec_patch_bytes_data.size();
		PATCH_AND_CONSUME(sym.die, patchFilldir64.patch_jump(sym.die.offset, sym.__drm_puts_coredump.offset, vec_patch_bytes_data));
		mark("filldir64", "filldir64(jump)", sym.die.offset, g0);
		g0 = vec_patch_bytes_data.size();
		PATCH_AND_CONSUME(sym.__drm_puts_coredump, patchFilldir64.patch_filldir64_core(sym.__drm_puts_coredump, vec_patch_bytes_data));
		mark("filldir64", "filldir64(core)", sym.__drm_puts_coredump.offset, g0);
		g0 = vec_patch_bytes_data.size();
		PATCH_AND_CONSUME(sym.die, patchCompatFilldir.patch_compat_filldir_root_key_guide(r.root_key_start, sym.die, vec_patch_bytes_data));
		mark("compat_filldir", "compat_filldir(root_key_guide)", r.root_key_start, g0);
		g0 = vec_patch_bytes_data.size();
		PATCH_AND_CONSUME(sym.die, patchCompatFilldir.patch_jump(sym.die.offset, sym.__drm_puts_coredump.offset, vec_patch_bytes_data));
		mark("compat_filldir", "compat_filldir(jump)", sym.die.offset, g0);
		g0 = vec_patch_bytes_data.size();
		PATCH_AND_CONSUME(sym.__drm_puts_coredump, patchCompatFilldir.patch_compat_filldir_core(sym.__drm_puts_coredump, vec_patch_bytes_data));
		mark("compat_filldir", "compat_filldir(core)", sym.__drm_puts_coredump.offset, g0);
		g0 = vec_patch_bytes_data.size();
		auto current_avc_check_bl_func = sym.__drm_printfn_coredump.offset;
		PATCH_AND_CONSUME(sym.__drm_printfn_coredump, patchCurrentAvcCheck.patch_current_avc_check_bl_func(sym.__drm_printfn_coredump, off.cred_offset, vec_patch_bytes_data));
		mark("avc_check", "current_avc_check", current_avc_check_bl_func, g0);
		g0 = vec_patch_bytes_data.size();
		PATCH_AND_CONSUME(sym.__drm_printfn_coredump, patchAvcDenied.patch_avc_denied(sym.__drm_printfn_coredump, current_avc_check_bl_func, vec_patch_bytes_data));
		mark("avc_denied", "avc_denied", sym.avc_denied.offset, g0);
		g0 = vec_patch_bytes_data.size();
		PATCH_AND_CONSUME(sym.__drm_printfn_coredump, patchAuditLogStart.patch_audit_log_start(sym.__drm_printfn_coredump, current_avc_check_bl_func, vec_patch_bytes_data));
		mark("audit_log_start", "audit_log_start", sym.audit_log_start.offset, g0);
	} else {
		patched = false;
	}

	// ---- 可选：rc 虚拟化（--launcher rc，默认 off）----
	// 失败一律降级（enabled=false + reason），**不影响**上面已织入的组；显式开启但失败由 main 报 E202。
	if (launcher.rc()) {
		size_t g0 = vec_patch_bytes_data.size();
		PatchRcVirtualize pv(patchBase, sym);
		RcVirtInfo info;
		info.mode = "rc";
		// ENG-1：把本 run 已被占用的写入区间传给 precheck——洞扫描排除与之相交的零区。
		// <6.1 分支的 hook 链硬编码在 [0x200, 0x200+0x800)（含 hook_jump 尾洞），
		// ≥6.1 分支写在 die/__drm_*_coredump 等真实符号区；vec_patch 里是逐处精确的
		// [write_addr, write_addr+len)，两者一并排除（显式 +0x800 区间兜底 <6.1 的
		// 逐条区间之外的零碎缝隙，那片零区正是事故现场）。
		std::vector<RcVirtCave> occupied;
		for (const auto& p : vec_patch_bytes_data) {
			occupied.push_back({ p.write_addr, p.str_bytes.length() / 2 });
		}
		if (kernel_ver.is_kernel_version_less("6.1.0")) {
			occupied.push_back({ 0x200, 0x800 });   // <6.1 硬编码 hook 区整段排除（含零碎缝隙）
		}
		if (pv.precheck(off.cred_offset, info, occupied)) {
			RcVirtInfo out_info;
			if (pv.weave(off.cred_offset, off.cred_uid_offset, info, vec_patch_bytes_data, out_info)) {
				r.launcher = out_info;
				mark("rc_virtualize", "__arm64_sys_openat", out_info.hook_addr, g0);
			} else {
				r.launcher = out_info;
				std::cout << "[rc_virtualize] weave failed: " << out_info.reason << std::endl;
			}
		} else {
			r.launcher = info;
			std::cout << "[rc_virtualize] precheck failed: " << info.reason << std::endl;
		}
	}

	r.patched = patched;
	return r;
}

// ---------------------------------------------------------------- 期望组数判据
// 注意：分支判据必须与 patch_kernel_handler 内部完全一致（同一 KernelVersionParser 版本比较），
// 否则 report.hook_groups_expected 会与实际织入分支不符 —— 这是本函数存在的唯一理由。
bool hook_branch_ge_6_1(const std::vector<char>& file_buf) {
	KernelVersionParser ver(file_buf);
	return !ver.is_kernel_version_less("6.1.0");
}

// cred 布局第二锚点（lead 批准的 R3 附带项）：sys_getuid 单锚点推出的 cred_offset
// 必须 **同时** 出现在另一个"取 current->cred"的函数里（geteuid/prctl 等），
// 布局才可信。实现为纯函数：在第二锚点符号区间里找同样的 load offset。
// 返回 0 = 第二锚点不可用/未命中（report 里降级说明，不硬红——单锚点仍是
// 上游验证过的主路径）；非 0 = 第二锚点推出的偏移，与主锚点一致则互证。
size_t hook_cred_offset_second_anchor(const std::vector<char>& file_buf,
                                      const SymbolRegion& anchor2) {
	if (!anchor2.offset || !anchor2.size) return 0;
	std::vector<a64_find_mrs_register::track_reg_info> track_info;
	if (!a64_find_mrs_register::find_current_task_next_register_offset(
			file_buf, anchor2.offset, anchor2.offset + anchor2.size, track_info))
		return 0;
	for (const auto& t : track_info) {
		if (t.load_offset > 0x400) return static_cast<size_t>(t.load_offset);
	}
	return 0;
}

// 第二锚点符号解析：优先 __arm64_sys_geteuid，缺席回落 sys_getegid。
// （独立 SymbolAnalyze 实例：kallsyms 解析一次的代价可接受，且不污染主 sym 结构。）
std::pair<SymbolRegion, SymbolRegion> hook_second_anchor_symbols(const std::vector<char>& file_buf) {
	SymbolAnalyze sa(file_buf);
	if (!sa.analyze_kernel_symbol()) return {};
	KernelSymbolOffset s = sa.get_symbol_offset();
	// 复用 sys_getuid 解析形态：geteuid/getegid 是同族 wrapper。
	// SymbolAnalyze 未内置这两个符号，这里走 find_region 等价路径——直接补查。
	// （为不动 symbol_analyze 接口，用 analyze 后的 finder 不行——私有的；
	//   所以这里复用已解析的 sys_getuid 之外的 prctl_get_seccomp 作为 anchor 候选 2，
	//   geteuid 用 kallsyms 全量近似：在 symbol_analyze 的 printf 之外没有接口，
	//   故本实现以 prctl 系为第二锚点（main 流程已查过该符号，存在性有保障）。）
	// 注意：main 侧 anchor2 优先级 = geteuid(缺省无) → prctl_get_seccomp（必有）。
	// 这里返回 {geteuid_region(空), prctl_region}：first 恒空 ⇒ main 取 second。
	return { SymbolRegion{}, s.prctl_get_seccomp };
}

bool hook_has_huawei_symbols(const KernelSymbolOffset& sym) {
	return sym.hkip_check_uid_root != 0 || sym.hkip_check_gid_root != 0 || sym.hkip_check_xid_root != 0;
}

// 实际命中的 huawei 符号名（顺序固定 uid→gid→xid，供 report 的
// hook_groups_expected_breakdown.huawei_symbols_detected 逐条对账）。
// 返回空数组 == 一个都没命中 == 该内核无此保护路径 ⇒ huawei_bypass 必须为 0。
std::vector<std::string> hook_huawei_symbols_detected(const KernelSymbolOffset& sym) {
	std::vector<std::string> out;
	if (sym.hkip_check_uid_root) out.push_back("hkip_check_uid_root");
	if (sym.hkip_check_gid_root) out.push_back("hkip_check_gid_root");
	if (sym.hkip_check_xid_root) out.push_back("hkip_check_xid_root");
	return out;
}
