#include "patch_base.h"
#include "3rdparty/find_mrs_register.h"

#include <cstdlib>

using namespace asmjit;
using namespace asmjit::a64;
using namespace asmjit::a64::Predicate;

PatchBase::PatchBase(const std::vector<char>& file_buf, size_t cred_uid_offset, const huawei_extra& huawei) :
	m_file_buf(file_buf), 
	m_kernel_ver_parser(file_buf), 
	m_init_cred_searcher(file_buf, cred_uid_offset), 
	m_huawei_extra(huawei) {
	IF_EXIT(!m_init_cred_searcher.init());
}

PatchBase::PatchBase(const PatchBase& other)
	: m_file_buf(other.m_file_buf)
	, m_kernel_ver_parser(other.m_file_buf)
	, m_init_cred_searcher(other.m_init_cred_searcher)
	, m_huawei_extra(other.m_huawei_extra)
	// ENG-2：必须携带 kti 计算基址——否则华为内核的拷贝副本（PatchDoExecve 等）在
	// 自己的 update_huawei_kti_calc_base 之前就带着 0 基址生成 ADRP，page_delta
	// 超界 → emit_huawei_kti_add _exit 猝死（绕过统一失败出口，无 report/NDJSON/ROOT_KEY 行）。
	, m_huawei_kti_calc_base(other.m_huawei_kti_calc_base) {}

PatchBase::~PatchBase() {}

uint32_t PatchBase::skip_pac_bti_at_func_start(uint32_t addr) {
	if ((uint64_t)addr + sizeof(uint32_t) > m_file_buf.size()) return addr;   // ENG-7
	uint32_t instr = *reinterpret_cast<const uint32_t*>(&m_file_buf[addr]);
	if (aarch64_insn_is_pac_or_bti(instr)) return addr + 4;
	return addr;
}

SymbolRegion PatchBase::skip_pac_bti_at_func_start(const SymbolRegion& sym) {
	auto new_sym = sym;
	new_sym.consume(skip_pac_bti_at_func_start(sym.offset) - sym.offset);
	return new_sym;
}

size_t PatchBase::patch_jump(size_t patch_addr, size_t jump_addr, std::vector<patch_bytes_data>& vec_out_patch_bytes_data) {
	aarch64_asm_ctx asm_ctx = init_aarch64_asm();
	auto a = asm_ctx.assembler();
	// ENG-4：检查 aarch64_asm_b 的返回值（±128MB/4 对齐守卫失败时它不 embed 任何字节）。
	if (!aarch64_asm_b(a, (int32_t)(jump_addr - patch_addr))) return 0;
	std::vector<uint8_t> bytes = aarch64_asm_to_bytes(a);
	if (bytes.size() == 0) return 0;
	std::string str_bytes = bytes2hex((const unsigned char*)bytes.data(), bytes.size());
	vec_out_patch_bytes_data.push_back({ str_bytes, patch_addr });
	return bytes.size();
}

bool PatchBase::is_CONFIG_THREAD_INFO_IN_TASK() {
	using namespace a64_find_mrs_register;
	if (count_mrs_sp_el0() <= 5000) return false;
	return !is_CURRENT_FROM_SP_EL0_THREAD_INFO();
}

bool PatchBase::is_CURRENT_FROM_SP_EL0_THREAD_INFO() {
	using namespace a64_find_mrs_register;
	if (count_mrs_sp_el0() <= 5000) return false;

	static bool last_result = false;
	static bool inited = false;
	if (inited) return last_result;

	int thread_info_size_cnt = 0;
	size_t max_size = m_file_buf.size() - 4;
	for (auto x = 0; x < max_size; x += 4) {
		uint32_t insn = *(uint32_t*)&m_file_buf[x];
		if (!aarch64_insn_is_mrs_sp_el0(insn)) continue;
		auto y = x + 4;
		if (y >= max_size) break;
		int end_off = 0;
		for (; y < max_size; y += 4) {
			uint32_t end_insn = *(uint32_t*)&m_file_buf[y];
			if (aarch64_insn_is_ret(end_insn) || aarch64_insn_is_retaa(end_insn) || aarch64_insn_is_retab(end_insn)) { end_off = y; break; }
		}
		if (end_off == 0) continue;
		std::vector<track_reg_info>track_info;
		if (!find_current_task_next_register_offset(m_file_buf, x, end_off, track_info)) continue;
		if (track_info.size() >= 2 && track_info[0].load_offset == offsetof(thread_info, task)) {
			for (auto& t : track_info) {
				if (t.load_offset > 0x400) {
					thread_info_size_cnt++; break; }
			}
		}
	}
	float radio = (float)thread_info_size_cnt / (float)count_mrs_sp_el0();
	last_result = radio >= 0.1f;
	inited = true;
	return last_result;
}

void PatchBase::emit_get_current(Assembler* a, GpX x) {
	uint32_t sp_el0_id = SysReg::encode(3, 0, 4, 1, 0);

	if (is_CONFIG_THREAD_INFO_IN_TASK()) {
		a->mrs(x, sp_el0_id);
		emit_huawei_kti_add(a, x);
		return;
	}

	if (is_CURRENT_FROM_SP_EL0_THREAD_INFO()) {
		a->mrs(x, sp_el0_id);
		emit_huawei_kti_add(a, x);
		a->ldr(x, ptr(x, offsetof(thread_info, task)));
		return;
	}

	a->mov(x, sp);
	a->and_(x, x, Imm((uint64_t)~(THREAD_SIZE - 1)));
	a->ldr(x, ptr(x, offsetof(thread_info, task)));
}

void PatchBase::emit_safe_bl(Assembler* a, size_t func_base_addr, size_t target) {
	RegProtectGuard g1(a, x29, x30);
	size_t bl_addr = func_base_addr + a->offset();
	int64_t diff = (int64_t)target - (int64_t)bl_addr;
	// ENG-4：检查返回值——守卫失败时 aarch64_asm_bl_raw 不 embed 任何字节，继续
	// 组装会生成"调用了不存在调用"的残缺 blob。失败时记入 errorHandler（
	// assemble_aarch64 会因此返回空 → 上层统一失败路径），并 emit nop 占位维持形状。
	if (!aarch64_asm_bl_raw(a, (int32_t)diff)) {
		if (auto* eh = a->errorHandler()) eh->handleError(kErrorInvalidState,
		                                                  "bl_raw failed (range guard)",
		                                                  a);
		a->nop();
		a->nop();
	}
}

std::vector<uint8_t> PatchBase::assemble_aarch64(std::function<void(asmjit::a64::Assembler* a)> fn) {
	aarch64_asm_ctx asm_ctx = init_aarch64_asm();
	auto a = asm_ctx.assembler();
	fn(a);
	return aarch64_asm_to_bytes(a);
}

int PatchBase::count_mrs_sp_el0() {
	static int cnt = 0;
	static bool inited = false;
	if (inited) return cnt;
	for (auto x = 0; x < m_file_buf.size() - 4; x += 4) {
		uint32_t insn = *(uint32_t*)&m_file_buf[x];
		if (aarch64_insn_is_mrs_sp_el0(insn)) cnt++;
	}
	inited = true;
	return cnt;
}

std::vector<size_t> PatchBase::find_all_aarch64_ret_offsets(size_t offset, size_t size) {
	std::vector<size_t> v_ret_addr;
	std::string mode;
	// ENG-7：range 尾部可能越界（size 来自符号表），逐条钳制到缓冲内。
	for (size_t i = offset; i + sizeof(uint32_t) <= m_file_buf.size() && i < offset + size; i += 4) {
		uint32_t instr = *reinterpret_cast<const uint32_t*>(&m_file_buf[i]);
		std::string cur;
		if (aarch64_insn_is_ret(instr)) cur = "ret";
		else if (aarch64_insn_is_retaa(instr)) cur = "retaa";
		else if (aarch64_insn_is_retab(instr)) cur = "retab";
		if (cur.empty()) continue;
		if (mode.empty()) mode = cur;
		else if (mode != cur) {
			printf("Error: RET / RETAA / RETAB cannot appear together in the same range.\n");
			_exit(EXIT_FAILURE);
		}
		v_ret_addr.push_back(i);
	}
	return v_ret_addr;
}

uint32_t PatchBase::find_func_epilogue_offset(uint32_t func_start_off, uint32_t func_size) {
	constexpr uint32_t kInsnSize = sizeof(uint32_t);
	uint32_t entry_insn = 0;
	std::memcpy(&entry_insn, m_file_buf.data() + func_start_off, sizeof(entry_insn));
	std::vector<std::vector<uint8_t>> vec_epilogue;
	if (aarch64_insn_is_paciaz(entry_insn)) {
		vec_epilogue.emplace_back(assemble_aarch64([](asmjit::a64::Assembler* a) { aarch64_asm_autiaz(a); a->ret(x30); }));
	} else if (aarch64_insn_is_paciasp(entry_insn)) {
		vec_epilogue.emplace_back(assemble_aarch64([](asmjit::a64::Assembler* a) { aarch64_asm_autiasp(a); a->ret(x30); }));
		vec_epilogue.emplace_back(assemble_aarch64([](asmjit::a64::Assembler* a) { aarch64_asm_retaa(a); }));
	} else if (aarch64_insn_is_pacibz(entry_insn)) {
		vec_epilogue.emplace_back(assemble_aarch64([](asmjit::a64::Assembler* a) { aarch64_asm_autibz(a); a->ret(x30); }));
	} else if (aarch64_insn_is_pacibsp(entry_insn)) {
		vec_epilogue.emplace_back(assemble_aarch64([](asmjit::a64::Assembler* a) { aarch64_asm_autibsp(a); a->ret(x30); }));
		vec_epilogue.emplace_back(assemble_aarch64([](asmjit::a64::Assembler* a) { aarch64_asm_retab(a); }));
	} else {
		vec_epilogue.emplace_back(assemble_aarch64([](asmjit::a64::Assembler* a) { a->ret(x30); }));
	}
	const uint32_t func_end_off = func_start_off + func_size;
	for (const auto& epilogue : vec_epilogue) {
		if (epilogue.empty() || epilogue.size() > func_size || epilogue.size() % kInsnSize != 0) continue;
		uint32_t pos = func_end_off - static_cast<uint32_t>(epilogue.size());
		pos -= (pos - func_start_off) % kInsnSize;
		while (true) {
			if (std::memcmp(m_file_buf.data() + pos, epilogue.data(), epilogue.size()) == 0) return pos;
			if (pos < func_start_off + kInsnSize) break;
			pos -= kInsnSize;
		}
	}
	return 0;
}

bool PatchBase::is_huawei() {
	return !!m_huawei_extra.kti_addr;
}

void PatchBase::update_huawei_kti_calc_base(size_t base) {
	m_huawei_kti_calc_base = base;
}

void PatchBase::emit_huawei_kti_add(Assembler* a, GpX x) {
	if (!is_huawei()) return;
	RegProtectGuard g1(a, x1);
	uint64_t cur_abs_addr = (uint64_t)m_huawei_kti_calc_base + (uint64_t)a->offset();
	// ENG-2：编码失败不再 _exit 猝死（那会绕过统一失败出口——无 report/NDJSON/ROOT_KEY）。
	// 改为记入 errorHandler：assemble_aarch64 检查 isError() 后返回空 vector →
	// build_blob/weave 失败 → pipeline 降级（enabled=false+reason）/ main 走 fail()。
	if (!aarch64_asm_adrp_x(a, x1, cur_abs_addr, (uint64_t)m_huawei_extra.kti_addr)) {
		std::cout << "[发生错误] aarch64_asm_adrp_x failed (kti base=0x" << std::hex
		          << m_huawei_kti_calc_base << ", kti=0x" << m_huawei_extra.kti_addr << ")"
		          << std::endl;
		if (auto* eh = a->errorHandler()) eh->handleError(kErrorInvalidState,
		                                                  "adrp_x failed (huawei kti)",
		                                                  a);
		a->mov(x1, 0);   // 占位维持指令流形状（结果会因 errorHandler 置错被上层丢弃）
		return;
	}
	uint32_t lo12 = (uint32_t)(m_huawei_extra.kti_addr & 0xFFF);
	a->ldr(x1, ptr(x1, lo12));
	a->add(x, x, x1);
}
