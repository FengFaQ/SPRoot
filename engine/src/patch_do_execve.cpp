#pragma once
#include "patch_do_execve.h"
#include "analyze/base_func.h"
#include "3rdparty/aarch64_asm_helper.h"
using namespace asmjit;
using namespace asmjit::a64;
using namespace asmjit::a64::Predicate;

// R1：IF_EXIT 异常原因的全局落点（定义在引擎侧，patch_kernel_root.h 只 extern）。
// 旧实现条件为真时直接 exit(0)（伪成功）；现在只记录，由管线/主流程判 E-code。
std::string g_patch_abort_reason;

// R2②：函数序言白名单 —— 选中候选的首指令必须属于这些形态之一。
// 目的：①挡住 PC-relative 首指令（adrp/adr/b/cb 类搬到 blob 槽位后语义错误）；
// ②确认这是"函数入口"而非中途落点（AArch64 序言必为 PAC/BTI/帧建立之一）。
static bool insn_is_valid_prologue(uint32_t insn) {
	return aarch64_insn_is_paciasp(insn) || aarch64_insn_is_pacibsp(insn)
	    || aarch64_insn_is_paciaz(insn)  || aarch64_insn_is_pacibz(insn)
	    || aarch64_insn_is_bti(insn)
	    || aarch64_insn_is_stp_pre(insn);   // stp x29, x30, [sp, #-N]!（帧建立，非 PAC 内核）
}

#define MAX_ERRNO	4095
#define TIF_SECCOMP 11

PatchDoExecve::PatchDoExecve(const PatchBase& patch_base, const KernelSymbolOffset& sym) : PatchBase(patch_base) {
	init_do_execve_param(sym);
}
PatchDoExecve::~PatchDoExecve() {}

void PatchDoExecve::init_do_execve_param(const KernelSymbolOffset& sym) {
	struct ExecveCandidate {
		SymbolRegion execve_sym;
		uint8_t filename_reg = 0;
		bool is_single_char_ptr = false;
	};
	auto set_execve_param = [this](const ExecveCandidate& c) {
		if (!c.execve_sym || c.execve_sym.size == 0) return false;
		m_doexecve_reg_param.do_execve_addr = static_cast<uint32_t>(c.execve_sym.offset);
		m_doexecve_reg_param.do_execve_filename_reg = c.filename_reg;
		m_doexecve_reg_param.is_single_char_ptr = c.is_single_char_ptr;
		return true;
	};
	ExecveCandidate best{};
	// R2①：版本带选（5.4- __do_execve_file 是唯一真身；6.1+ do_execveat_common 是
	// 唯一汇聚点，已核对 v6.1.25/android14-6.1 调用链），size-max 仅作未知 vendor 兜底。
	// R2②：序言白名单——不在集合的候选视为"非函数入口/不可搬移首指令"，跳过降级。
	const bool ver_ge_5_5 = !m_kernel_ver_parser.is_kernel_version_less("5.5.0");
	auto consider = [&](SymbolRegion execve_sym, uint8_t filename_reg, bool is_single_char_ptr,
	                    bool preferred_5_5, bool preferred_pre_5_5) {
		if (!execve_sym || execve_sym.size == 0) return;
		// 序言白名单（R2②）：读首指令校验；越界/不在集合 → 跳过该候选。
		const std::vector<char>& fbuf = m_file_buf;
		if (execve_sym.offset + sizeof(uint32_t) > fbuf.size()) {
			if (g_patch_abort_reason.empty()) g_patch_abort_reason = "execve candidate out of range";
			return;
		}
		const uint32_t insn0 = *reinterpret_cast<const uint32_t*>(&fbuf[execve_sym.offset]);
		if (!insn_is_valid_prologue(insn0)) {
			if (g_patch_abort_reason.empty())
				g_patch_abort_reason = "execve candidate prologue not in whitelist";
			return;   // 降级下一候选（R2②）
		}
		// 带选优先：当前版本段的 preferred 候选直接锁定（不受 size 竞争干扰）。
		if ((ver_ge_5_5 && preferred_5_5) || (!ver_ge_5_5 && preferred_pre_5_5)) {
			best.execve_sym = execve_sym;
			best.filename_reg = filename_reg;
			best.is_single_char_ptr = is_single_char_ptr;
			m_best_preferred = true;
			return;
		}
		// 非 preferred：preferred 已锁定时不再参与 size-max；否则按 size-max 兜底。
		if (m_best_preferred) return;
		if (!best.execve_sym || execve_sym.size > best.execve_sym.size) {
			best.execve_sym = execve_sym;
			best.filename_reg = filename_reg;
			best.is_single_char_ptr = is_single_char_ptr;
		}
	};
	if (m_kernel_ver_parser.is_kernel_version_less("3.14.0")) {
		consider(sym.do_execve_common, 0, true, false, false);
		consider(sym.do_execve, 0, true, false, false);
	} else {
		// 顺序即带选优先级：preferred 段内首个命中即锁定（R2①）。
		consider(sym.__do_execve_file, 1, false, /*preferred_5_5*/false, /*preferred_pre_5_5*/true);
		consider(sym.do_execveat_common, 1, false, /*preferred_5_5*/true, false);
		consider(sym.do_execve, 0, false, false, false);
		consider(sym.do_execveat, 1, false, false, false);
		consider(sym.do_execve_common, 0, false, false, false);
	}
	set_execve_param(best);
	m_doexecve_reg_param.do_execve_addr = skip_pac_bti_at_func_start(m_doexecve_reg_param.do_execve_addr);
	// R1：候选全空（或全部被序言白名单拒绝）→ valid=false，patch_do_execve 前置短路，
	// 管线报 E202 而不是 IF_EXIT exit(0) 伪成功。
	m_do_execve_valid = set_execve_param(best)
		&& m_doexecve_reg_param.do_execve_addr != 0;
}

int PatchDoExecve::get_need_write_cap_cnt() {
	InitCredResult cred_result = m_init_cred_searcher.get_init_cred_result();
	return cred_result.cap_cnt;
}

size_t PatchDoExecve::patch_do_execve(const SymbolRegion& hook_func_start_region, size_t task_struct_cred_offset, size_t task_struct_seccomp_offset,
	std::vector<patch_bytes_data>& vec_out_patch_bytes_data) {

	// R1：候选全空（或全被序言白名单拒绝）→ 前置短路返回 0；管线据 valid()==false
	// 报 E202（替代上游 IF_EXIT exit(0) 的 rc-0 伪成功）。
	if (!m_do_execve_valid) return 0;
	size_t hook_func_start_addr = hook_func_start_region.offset;
	if (hook_func_start_addr == 0) return 0;
	std::cout << "Start hooking addr:  " << std::hex << hook_func_start_addr << std::endl << std::endl;
	if (is_huawei()) update_huawei_kti_calc_base(hook_func_start_addr);

	InitCredResult cred_result = m_init_cred_searcher.get_init_cred_result();
	int cap_cnt = get_need_write_cap_cnt();

	size_t hook_jump_back_addr = m_doexecve_reg_param.do_execve_addr + 4;
	char empty_root_key_buf[ROOT_KEY_LEN] = { 0 };

	size_t restore_insn_offset = 0;
	std::vector<uint8_t> bytes = assemble_aarch64([=, &restore_insn_offset](asmjit::a64::Assembler* a) {
		Label label_end = a->newLabel();
		Label label_cycle_name = a->newLabel();
		int key_start = a->offset();
		a->embed((const uint8_t*)empty_root_key_buf, sizeof(empty_root_key_buf));
		restore_insn_offset = a->offset();
		a->mov(x0, x0);
		a->mov(x11, Imm(uint64_t(-MAX_ERRNO)));
		a->cmp(a64::x(m_doexecve_reg_param.do_execve_filename_reg), x11);
		a->b(CondCode::kCS, label_end);

		if (m_doexecve_reg_param.is_single_char_ptr) {
			a->mov(x11, a64::x(m_doexecve_reg_param.do_execve_filename_reg));
		} else {
			a->ldr(x11, ptr(a64::x(m_doexecve_reg_param.do_execve_filename_reg)));
		}

		int key_offset = key_start - a->offset();
		aarch64_asm_adr_x(a, x12, key_offset);

		a->bind(label_cycle_name);
		a->ldrb(w14, ptr(x11).post(1));
		a->ldrb(w15, ptr(x12).post(1));
		a->cmp(w14, w15);
		a->b(CondCode::kNE, label_end);
		a->cbnz(w15, label_cycle_name);
		emit_get_current(a, x12);
		a->ldr(x14, ptr(x12, task_struct_cred_offset));
		a->add(x14, x14, Imm(cred_result.atomic_usage_size));
		a->str(xzr, ptr(x14).post(8));
		a->str(xzr, ptr(x14).post(8));
		a->str(xzr, ptr(x14).post(8));
		a->str(xzr, ptr(x14).post(8));
		a->str(wzr, ptr(x14).post(cred_result.securebits_size));
		a->mov(x13, Imm(cred_result.cap_ability_max));
		a->stp(x13, x13, ptr(x14).post(16));
		a->stp(x13, x13, ptr(x14).post(16));
		if (cap_cnt >= 5) {
			a->str(x13, ptr(x14).post(8));
		}
		a->mov(x15, Imm((uint64_t)1ULL << TIF_SECCOMP));
		if (is_CONFIG_THREAD_INFO_IN_TASK()) {
			a->ldaxr(x14, ptr(x12, offsetof(thread_info, flags)));
			a->bic(x14, x14, x15);
			a->stlxr(x15, x14, ptr(x12, offsetof(thread_info, flags)));
		} else if (is_CURRENT_FROM_SP_EL0_THREAD_INFO()) {
			uint32_t sp_el0_id = SysReg::encode(3, 0, 4, 1, 0);
			a->mrs(x13, sp_el0_id);
			emit_huawei_kti_add(a, x13);
			a->ldaxr(x14, ptr(x13, offsetof(thread_info, flags)));
			a->bic(x14, x14, x15);
			a->stlxr(x15, x14, ptr(x13, offsetof(thread_info, flags)));
		} else {
			a->mov(x13, sp);
			a->and_(x13, x13, Imm((uint64_t)~(THREAD_SIZE - 1)));
			a->ldaxr(x14, ptr(x13, offsetof(thread_info, flags)));
			a->bic(x14, x14, x15);
			a->stlxr(x15, x14, ptr(x13, offsetof(thread_info, flags)));
		}

		a->str(wzr, ptr(x12, task_struct_seccomp_offset));
		a->bind(label_end);
		aarch64_asm_b(a, (int32_t)(hook_jump_back_addr - (hook_func_start_addr + a->offset())));
		std::cout << print_aarch64_asm(a) << std::endl;

	});
	if (bytes.size() == 0) return 0;
	uint32_t orig_insn = 0;
	std::memcpy(&orig_insn, m_file_buf.data() + m_doexecve_reg_param.do_execve_addr, sizeof(orig_insn));
	std::memcpy(bytes.data() + restore_insn_offset, &orig_insn, sizeof(orig_insn));
	std::string str_bytes = bytes2hex((const unsigned char*)bytes.data(), bytes.size());
	size_t shellcode_size = bytes.size();
	if (shellcode_size > hook_func_start_region.size) {
		std::cout << "[发生错误] patch_do_execve failed: not enough kernel space." << std::endl;
		return 0;
	}
	
	vec_out_patch_bytes_data.push_back({ str_bytes, hook_func_start_addr });
	patch_jump(m_doexecve_reg_param.do_execve_addr, hook_func_start_addr + sizeof(empty_root_key_buf), vec_out_patch_bytes_data);
	return shellcode_size;
}
