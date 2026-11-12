// patch_rc_virtualize.cpp — rc 虚拟化（APatch 路线）的实现
//
// 见 patch_rc_virtualize.h 顶部说明与 docs/NEW-DOC-02 §11。
// clean-room：机制理解自 KernelPatch/APatch（GPL-2.0-or-later），实现为本项目自研。
#include "patch_rc_virtualize.h"
#include "patch_kernel_root.h"
#include "analyze/base_func.h"
#include "3rdparty/aarch64_asm_helper.h"
#include "3rdparty/aarch64_asm_reg_protect_guard.h"

#include <cstring>
#include <algorithm>

using namespace asmjit;
using namespace asmjit::a64;
using namespace asmjit::a64::Predicate;

// ---------------------------------------------------------------- 常量
const char* const RC_VIRT_ORIG_PATHS[2] = {
	"/system/etc/init/hw/init.rc",              // 28 B（> 替换串 19 B，可原地覆写）
	"/vendor/etc/init/hw/init.target.rc",       // 34 B
};
const char* const RC_VIRT_REPLACE_PATH = "/dev/user_init.rc";   // 19 B

// init 的 `openat` syscall 入口（arm64）：x0 = struct pt_regs*
// 结构：regs[31] 在偏移 0，故 regs[1] = +8（filename 用户指针），regs[2] = +16（flags）
static constexpr int32_t PTREGS_OFF_REG1 = 8;
static constexpr int32_t PTREGS_OFF_REG2 = 16;

static constexpr uint32_t O_WRONLY_CREAT_TRUNC = 0x241;   // O_WRONLY(1)|O_CREAT(0x40)|O_TRUNC(0x200)
static constexpr uint32_t FILE_MODE_0600 = 0x180;
static constexpr uint64_t ERR_PTR_THRESHOLD = 0xFFFFFFFFFFFFF001ULL;  // IS_ERR: >= (unsigned long)-4095
static constexpr size_t PATH_BUF_BYTES = 256;
static constexpr size_t PATH_BUF_MAX_COPY = 256;

// 栈帧布局（相对 sp）
static constexpr int32_t FRAME_BYTES = 0x1A0;
static constexpr int32_t OFF_SAVE_A = 0x00;   // x0,x1
static constexpr int32_t OFF_SAVE_B = 0x10;   // x2,x3
static constexpr int32_t OFF_SAVE_C = 0x20;   // x4,x5
static constexpr int32_t OFF_CALLEE1 = 0x30;  // x19,x20
static constexpr int32_t OFF_CALLEE2 = 0x40;  // x21,x22
static constexpr int32_t OFF_CALLEE3 = 0x50;  // x23,x24
static constexpr int32_t OFF_POS = 0x60;      // loff_t
static constexpr int32_t OFF_POS2 = 0x68;     // loff_t
static constexpr int32_t OFF_PATHBUF = 0x80;  // 256 B 用户 path 拷贝缓冲

// ---------------------------------------------------------------- 纯逻辑层
std::string rc_virt_template() {
	return
		"\n"
		"on post-fs-data\n"
		"    mkdir /data/adb/sproot 0700 root root\n"
		// Zygisk 启动链关键行：init 在 post-fs-data 阶段**阻塞**执行本命令
		// （exec 是 init 的阻塞原语），从而保证所有模块的 post-fs-data.sh
		// （含 NeoZygisk 启动 zygiskd、Mountify 建立 overlay）在 zygote 启动**之前**
		// 全部完成。缺这一行 → zygiskd 晚于 zygote 启动 → 注入必然失败。
		"    exec -- /data/adb/apd post-fs-data\n"
		"service sproot-apd /data/adb/apd --superkey-file /data/adb/sproot/key daemon\n"
		"    class late_start\n"
		"    user root\n"
		"    group root\n"
		"    oneshot\n"
		"    disabled\n"
		"    override\n"
		"on property:sys.boot_completed=1\n"
		"    start sproot-apd\n";
}

size_t rc_virt_template_len() { return rc_virt_template().size(); }

std::vector<const char*> rc_virt_required_symbols() {
	// 顺序即自检输出顺序；A11 断言要求「数量下限 + 逐个在场」，防被删空。
	return {
		"__arm64_sys_openat",
		"strncpy_from_user",
		"strcmp",
		"filp_open",
		"kernel_read",
		"kernel_write",
		"filp_close",
		"vmalloc",
		"vfree",
		"__arch_copy_to_user",
	};
}

std::vector<RcVirtCave> rc_virt_scan_caves(const std::vector<char>& buf, size_t lo, size_t hi,
                                           size_t min_size, size_t max_out,
                                           const std::vector<RcVirtCave>& excluded) {
	std::vector<RcVirtCave> out;
	if (min_size == 0 || lo >= hi || hi > buf.size() || max_out == 0) return out;
	// 区间相交判定（ENG-1）：[a0,a1) 与 [b0,b1) 相交 ⇔ a0 < b1 && b0 < a1
	auto overlaps_any = [&](size_t s, size_t e) {
		for (const auto& x : excluded) {
			if (s < x.offset + x.size && x.offset < e) return true;
		}
		return false;
	};
	size_t i = lo;
	while (i < hi && out.size() < max_out) {
		if (static_cast<unsigned char>(buf[i]) != 0) { ++i; continue; }
		size_t j = i;
		while (j < hi && static_cast<unsigned char>(buf[j]) == 0) ++j;
		// 起点必须是 4 字节对齐（指令取指要求）
		size_t start = (i + 3) & ~static_cast<size_t>(3);
		if (start < j && (j - start) >= min_size && !overlaps_any(start, j))
			out.push_back({ start, j - start });
		i = j;
	}
	return out;
}

RcVirtDataLayout rc_virt_data_layout() {
	RcVirtDataLayout l;
	// 不变量：替换串必须严格短于每个原路径串（我们是原地覆写用户 path，不申请新用户内存）
	for (const char* p : RC_VIRT_ORIG_PATHS) {
		if (strlen(RC_VIRT_REPLACE_PATH) >= strlen(p)) {
			l.ok = false;
			l.reason = "replacement path is not shorter than ";
			l.reason += p;
			return l;
		}
	}
	const std::string tmpl = rc_virt_template();
	auto put = [&](const char* s, size_t& out_off) {
		out_off = l.data_len;
		l.data_len += strlen(s) + 1;
		l.data_len = (l.data_len + 3) & ~static_cast<size_t>(3);   // 4 字节对齐（ADR 要求）
	};
	put(RC_VIRT_ORIG_PATHS[0], l.off_orig[0]);
	put(RC_VIRT_ORIG_PATHS[1], l.off_orig[1]);
	put(RC_VIRT_REPLACE_PATH, l.off_replace);
	put(tmpl.c_str(), l.off_template);
	l.ok = true;
	return l;
}

// ---------------------------------------------------------------- 数据区构造
static std::vector<char> build_data(const RcVirtDataLayout& l) {
	std::vector<char> d(l.data_len, 0);
	auto put = [&](size_t off, const char* s) { memcpy(d.data() + off, s, strlen(s) + 1); };
	put(l.off_orig[0], RC_VIRT_ORIG_PATHS[0]);
	put(l.off_orig[1], RC_VIRT_ORIG_PATHS[1]);
	put(l.off_replace, RC_VIRT_REPLACE_PATH);
	const std::string tmpl = rc_virt_template();
	put(l.off_template, tmpl.c_str());
	return d;
}

// ---------------------------------------------------------------- 生成器
PatchRcVirtualize::PatchRcVirtualize(const PatchBase& patch_base, const KernelSymbolOffset& sym)
	: PatchBase(patch_base), m_sym(sym) {}
PatchRcVirtualize::~PatchRcVirtualize() {}

bool PatchRcVirtualize::precheck(size_t cred_offset, RcVirtInfo& info,
                                 const std::vector<RcVirtCave>& occupied) {
	info.mode = "rc";
	info.reason.clear();
	info.caves.clear();
	info.symbols.clear();

	// kernel_read/kernel_write 在 4.14 改为 (file, buf, count, pos) 签名；旧签名不敢硬上。
	if (m_kernel_ver_parser.is_kernel_version_less("4.14.0")) {
		info.reason = "kernel < 4.14 (kernel_read/kernel_write signature not supported)";
		return false;
	}
	if (cred_offset == 0) {
		info.reason = "task_struct cred offset unavailable";
		return false;
	}
	if (!m_sym._stext || !m_sym._etext || m_sym._etext <= m_sym._stext) {
		info.reason = "_stext/_etext symbol missing (cannot bound .text cave scan)";
		return false;
	}
	// 符号齐全性：逐项检查并回填 report（缺失时明确写出缺哪个）
	struct SymReq { const char* name; const SymbolRegion* region; size_t addr; };
	const SymReq reqs[] = {
		{ "__arm64_sys_openat", &m_sym.openat_sys, 0 },
		{ "strncpy_from_user", &m_sym.strncpy_from_user_fn, 0 },
		{ "strcmp", &m_sym.str_cmp, 0 },
		{ "filp_open", &m_sym.filp_open_fn, 0 },
		{ "kernel_read", &m_sym.kernel_read_fn, 0 },
		{ "kernel_write", &m_sym.kernel_write_fn, 0 },
		{ "filp_close", &m_sym.filp_close_fn, 0 },
		{ "vmalloc", &m_sym.vmalloc_fn, 0 },
		{ "vfree", &m_sym.vfree_fn, 0 },
		{ "__arch_copy_to_user", &m_sym.copy_to_user_fn, 0 },
	};
	for (const auto& r : reqs) {
		const bool ok = r.region ? static_cast<bool>(*r.region) : (r.addr != 0);
		if (!ok) {
			info.reason = std::string("symbol missing: ") + r.name;
			return false;
		}
		info.symbols.push_back({ r.name, r.region ? r.region->offset : r.addr });
	}
	const RcVirtDataLayout layout = rc_virt_data_layout();
	if (!layout.ok) { info.reason = "data layout unsafe: " + layout.reason; return false; }

	// 扫描 .text 内的零填充洞（铁律：不硬编码地址）。
	// occupied = 本 run 已被其它补丁占用的区间（ENG-1）：相交的零区不算合法洞，
	// 否则 <6.1 分支会把 rc blob 织进 [0x200,0x200+0x800) 的硬编码 hook 区 → 互踩变砖。
	info.caves = rc_virt_scan_caves(m_file_buf, m_sym._stext, m_sym._etext,
	                                RC_VIRT_MIN_CAVE_BYTES, RC_VIRT_MAX_REPORT_CAVES, occupied);
	if (info.caves.empty()) {
		info.reason = "no zero-filled cave >= " + std::to_string(RC_VIRT_MIN_CAVE_BYTES) + " B in .text";
		return false;
	}
	// 校验：每个洞在 [_text,_etext) 内且逐字节为 0（防止扫描器 bug 把非零区当洞）
	for (const auto& c : info.caves) {
		if (c.offset < m_sym._stext || c.offset + c.size > m_sym._etext) {
			info.reason = "cave out of .text bounds";
			return false;
		}
		for (size_t i = c.offset; i < c.offset + c.size; ++i) {
			if (static_cast<unsigned char>(m_file_buf[i]) != 0) {
				info.reason = "cave is not zero-filled (scanner bug)";
				return false;
			}
		}
	}
	return true;
}

std::vector<uint8_t> PatchRcVirtualize::build_blob(size_t blob_base, size_t hook_addr,
                                                   size_t cred_offset, size_t cred_uid_offset,
                                                   size_t& restore_insn_offset, bool* ok) {
	const RcVirtDataLayout layout = rc_virt_data_layout();
	const std::vector<char> data = build_data(layout);
	const std::string tmpl = rc_virt_template();
	restore_insn_offset = 0;
	if (ok) *ok = false;

	// 华为内核：emit_get_current 需要 ADRP 的"当前指令绝对地址"，必须以 blob 实际
	// 落位地址为基（ENG-2）。此前拷贝构造不携带 m_huawei_kti_calc_base 且这里从不
	// 设置 → page_delta 超出 ±4GB → emit_huawei_kti_add _exit 猝死（绕过统一失败出口）。
	if (is_huawei()) update_huawei_kti_calc_base(blob_base);

	const size_t sym_strncpy = m_sym.strncpy_from_user_fn.offset;
	const size_t sym_strcmp = m_sym.str_cmp.offset;
	const size_t sym_filp_open = m_sym.filp_open_fn.offset;
	const size_t sym_filp_close = m_sym.filp_close_fn.offset;
	const size_t sym_kernel_read = m_sym.kernel_read_fn.offset;
	const size_t sym_kernel_write = m_sym.kernel_write_fn.offset;
	const size_t sym_vmalloc = m_sym.vmalloc_fn.offset;
	const size_t sym_vfree = m_sym.vfree_fn.offset;
	const size_t sym_copy_to_user = m_sym.copy_to_user_fn.offset;

	std::vector<uint8_t> bytes = assemble_aarch64([&](Assembler* a) {
		Label label_ret = a->newLabel();
		Label label_match = a->newLabel();
		Label label_close_dev = a->newLabel();
		Label label_free_close_orig = a->newLabel();
		Label label_close_orig = a->newLabel();

		// ---- 数据区（blob 开头；ADR 用负偏移引用）----
		a->embed(reinterpret_cast<const uint8_t*>(data.data()), data.size());
		// ---- 被替换的原首指令槽位（entry 跳到这里，先执行原指令）----
		restore_insn_offset = a->offset();
		a->mov(x0, x0);   // 占位，稍后用原指令覆盖

		// ---- 帧 + 保存寄存器 ----
		a->sub(sp, sp, Imm(FRAME_BYTES));
		a->stp(x0, x1, ptr(sp, OFF_SAVE_A));
		a->stp(x2, x3, ptr(sp, OFF_SAVE_B));
		a->stp(x4, x5, ptr(sp, OFF_SAVE_C));
		a->stp(x19, x20, ptr(sp, OFF_CALLEE1));
		a->stp(x21, x22, ptr(sp, OFF_CALLEE2));
		a->stp(x23, x24, ptr(sp, OFF_CALLEE3));
		a->mov(x19, x0);   // x19 = pt_regs*

		// ---- 过滤 1：只处理 uid==0（init）；非 root 的 openat 只多 5~6 条指令 ----
		emit_get_current(a, x9);
		a->ldr(x9, ptr(x9, static_cast<int32_t>(cred_offset)));
		a->ldr(w10, ptr(x9, static_cast<int32_t>(cred_uid_offset)));
		a->cbnz(w10, label_ret);
		// ---- 过滤 2：只看只读打开（rc 解析）----
		a->ldr(w10, ptr(x19, PTREGS_OFF_REG2));
		a->and_(w10, w10, Imm(3));   // O_ACCMODE
		a->cbnz(w10, label_ret);

		// ---- 拷贝用户 path 到栈缓冲 ----
		a->add(x0, sp, Imm(OFF_PATHBUF));
		a->ldr(x1, ptr(x19, PTREGS_OFF_REG1));
		a->mov(x2, Imm(static_cast<int64_t>(PATH_BUF_MAX_COPY)));
		emit_safe_bl(a, blob_base, sym_strncpy);
		a->cmp(x0, Imm(0));
		a->b(CondCode::kLT, label_ret);      // 拷贝失败（负 errno / 缺页）→ 不处理

		// ---- 与 2 条原 rc 路径比对 ----
		aarch64_asm_adr_x(a, x0, static_cast<int32_t>(layout.off_orig[0]) - static_cast<int32_t>(a->offset()));
		a->add(x1, sp, Imm(OFF_PATHBUF));
		emit_safe_bl(a, blob_base, sym_strcmp);
		a->cbz(x0, label_match);
		aarch64_asm_adr_x(a, x0, static_cast<int32_t>(layout.off_orig[1]) - static_cast<int32_t>(a->offset()));
		a->add(x1, sp, Imm(OFF_PATHBUF));
		emit_safe_bl(a, blob_base, sym_strcmp);
		a->cbz(x0, label_match);
		a->b(label_ret);

		// ---- 命中：打开原 rc ----
		a->bind(label_match);
		a->add(x0, sp, Imm(OFF_PATHBUF));
		a->mov(x1, Imm(0));      // O_RDONLY
		a->mov(x2, Imm(0));
		emit_safe_bl(a, blob_base, sym_filp_open);
		a->mov(x9, Imm(static_cast<uint64_t>(ERR_PTR_THRESHOLD)));
		a->cmp(x0, x9);
		a->b(CondCode::kCS, label_ret);          // IS_ERR → 静默放弃
		a->mov(x21, x0);                         // x21 = orig file

		// ---- vmalloc 缓冲 ----
		a->mov(x0, Imm(static_cast<int64_t>(RC_VIRT_READ_MAX_BYTES)));
		emit_safe_bl(a, blob_base, sym_vmalloc);
		a->cbz(x0, label_close_orig);
		a->mov(x23, x0);

		// ---- 读取原 rc ----
		a->str(xzr, ptr(sp, OFF_POS));
		a->mov(x0, x21);
		a->mov(x1, x23);
		a->mov(x2, Imm(static_cast<int64_t>(RC_VIRT_READ_MAX_BYTES)));
		a->add(x3, sp, Imm(OFF_POS));
		emit_safe_bl(a, blob_base, sym_kernel_read);
		a->cmp(x0, Imm(0));
		a->b(CondCode::kLE, label_free_close_orig);
		// 安全闸：若读到上限（可能被截断）→ 放弃重定向。绝不把截断过的 rc 交给 init
		// （截断会破坏 init 解析 → 开机失败，属于最高等级事故）。
		a->cmp(x0, Imm(static_cast<int64_t>(RC_VIRT_READ_MAX_BYTES)));
		a->b(CondCode::kGE, label_free_close_orig);
		a->mov(x24, x0);                         // x24 = 读到的字节数

		// ---- 打开/创建 /dev/user_init.rc ----
		aarch64_asm_adr_x(a, x0, static_cast<int32_t>(layout.off_replace) - static_cast<int32_t>(a->offset()));
		a->mov(w1, Imm(static_cast<int32_t>(O_WRONLY_CREAT_TRUNC)));
		a->mov(w2, Imm(static_cast<int32_t>(FILE_MODE_0600)));
		emit_safe_bl(a, blob_base, sym_filp_open);
		a->mov(x9, Imm(static_cast<uint64_t>(ERR_PTR_THRESHOLD)));
		a->cmp(x0, x9);
		a->b(CondCode::kCS, label_free_close_orig);
		a->mov(x22, x0);                         // x22 = new file

		// ---- 写「原 rc 全文」 ----
		a->str(xzr, ptr(sp, OFF_POS2));
		a->mov(x0, x22);
		a->mov(x1, x23);
		a->mov(x2, x24);
		a->add(x3, sp, Imm(OFF_POS2));
		emit_safe_bl(a, blob_base, sym_kernel_write);

		// ---- 追加模板 ----
		a->mov(x0, x22);
		aarch64_asm_adr_x(a, x1, static_cast<int32_t>(layout.off_template) - static_cast<int32_t>(a->offset()));
		a->mov(x2, Imm(static_cast<int64_t>(tmpl.size())));
		a->add(x3, sp, Imm(OFF_POS2));
		emit_safe_bl(a, blob_base, sym_kernel_write);
		a->b(label_close_dev);

		// ---- 清理：关闭 new / vfree / 关闭 orig ----
		a->bind(label_close_dev);
		a->mov(x0, x22);
		emit_safe_bl(a, blob_base, sym_filp_close);
		a->bind(label_free_close_orig);
		a->mov(x0, x23);
		emit_safe_bl(a, blob_base, sym_vfree);
		a->bind(label_close_orig);
		a->mov(x0, x21);
		emit_safe_bl(a, blob_base, sym_filp_close);

		// ---- 原地覆写用户 path：/dev/user_init.rc（短于原串，安全）----
		a->ldr(x0, ptr(x19, PTREGS_OFF_REG1));
		aarch64_asm_adr_x(a, x1, static_cast<int32_t>(layout.off_replace) - static_cast<int32_t>(a->offset()));
		a->mov(x2, Imm(static_cast<int64_t>(strlen(RC_VIRT_REPLACE_PATH) + 1)));
		emit_safe_bl(a, blob_base, sym_copy_to_user);

		// ---- 恢复并跳回 ----
		a->bind(label_ret);
		a->ldp(x23, x24, ptr(sp, OFF_CALLEE3));
		a->ldp(x21, x22, ptr(sp, OFF_CALLEE2));
		a->ldp(x19, x20, ptr(sp, OFF_CALLEE1));
		a->ldp(x4, x5, ptr(sp, OFF_SAVE_C));
		a->ldp(x2, x3, ptr(sp, OFF_SAVE_B));
		a->ldp(x0, x1, ptr(sp, OFF_SAVE_A));
		a->add(sp, sp, Imm(FRAME_BYTES));
		aarch64_asm_b(a, static_cast<int32_t>((hook_addr + 4) - (blob_base + a->offset())));
	});
	if (bytes.empty()) return {};
	if (restore_insn_offset + 4 > bytes.size()) return {};
	uint32_t orig_insn = 0;
	memcpy(&orig_insn, m_file_buf.data() + hook_addr, sizeof(orig_insn));
	memcpy(bytes.data() + restore_insn_offset, &orig_insn, sizeof(orig_insn));
	if (ok) *ok = true;
	return bytes;
}

bool PatchRcVirtualize::weave(size_t cred_offset, size_t cred_uid_offset, const RcVirtInfo& info,
                              std::vector<patch_bytes_data>& vec_out, RcVirtInfo& out_info) {
	out_info = info;
	RcVirtInfo cand = info;
	const size_t hook_addr = skip_pac_bti_at_func_start(static_cast<uint32_t>(m_sym.openat_sys.offset));
	for (const auto& cave : info.caves) {
		size_t restore_off = 0;
		bool blob_ok = false;
		std::vector<uint8_t> blob = build_blob(cave.offset, hook_addr, cred_offset, cred_uid_offset,
		                                       restore_off, &blob_ok);
		if (!blob_ok || blob.empty()) { cand.reason = "blob generation failed"; out_info = cand; return false; }
		if (blob.size() > cave.size) continue;   // 洞放不下 → 试下一个
		// ENG-1 防线（weave 侧兜底断言）：落位前确认 cave 与 vec_out 已占用的每个
		// [write_addr, write_addr+len) 都不相交。precheck 的洞扫描已排除，这里是
		// 独立的第二道闸（调用方若没传 occupied 也不会静默互踩）。
		bool overlaps_existing = false;
		for (const auto& p : vec_out) {
			const size_t plen = p.str_bytes.length() / 2;
			if (cave.offset < p.write_addr + plen && p.write_addr < cave.offset + blob.size()) {
				overlaps_existing = true;
				break;
			}
		}
		if (overlaps_existing) continue;         // 相交 → 试下一个洞（绝不覆盖活补丁）
		// 入口跳转生成失败（±128MB 越界等）→ 该洞作废，试下一个（ENG-4）：
		// 绝不"blob 已写、入口没跳"地留下 enabled=true 的半成品（report 谎报）。
		const size_t before_jump = vec_out.size();
		const size_t jump_bytes = patch_jump(hook_addr, cave.offset + restore_off, vec_out);
		if (jump_bytes == 0 || vec_out.size() != before_jump + 1) {
			vec_out.resize(before_jump);         // 回滚（保险：patch_jump 失败本就不 push）
			continue;
		}
		const std::string hex = bytes2hex(blob.data(), blob.size());
		vec_out.push_back({ hex, cave.offset });
		out_info = cand;
		out_info.enabled = true;
		out_info.cave_offset = cave.offset;
		out_info.cave_size = cave.size;
		out_info.shellcode_bytes = blob.size();
		out_info.template_bytes = rc_virt_template_len();
		out_info.hook_addr = hook_addr;
		std::cout << "[rc_virtualize] cave=0x" << std::hex << cave.offset << " size=" << std::dec
		          << cave.size << " blob=" << blob.size() << " hook=0x" << std::hex << hook_addr
		          << std::dec << std::endl;
		return true;
	}
	cand.reason = "no cave large enough for generated blob";
	out_info = cand;
	return false;
}
