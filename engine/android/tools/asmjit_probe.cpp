// asmjit_probe.cpp — 用 asmjit 生成若干 AArch64 指令字节，作为 LLVM 汇编器的
// **独立对照输入**（asm oracle）。仅测试用，不参与引擎产物。
//
// 背景（lead 2026-11 指示，借鉴 Skroot-Next 的文本汇编路线做 oracle）：
// 引擎的核心跳板全部由 asmjit 生成；asmjit 版本/参数若变化，字节可能悄悄改变，
// 导致"修补成功但内核跑飞"。这里用 NDK 自带 LLVM 汇编器汇编同一批助记符，
// 再用 llvm-objdump 取出机器码，与 asmjit 输出逐字节比对（engine/android/tools/asm_oracle.sh）。
//
// 用法：asmjit_probe <name>   → 打印该指令的 4 字节小端十六进制（如 "1f2003d5"）
//       asmjit_probe --list   → 列出所有 case 名
#include <asmjit/core.h>
#include <asmjit/a64.h>

#include <cstdio>
#include <cstring>
#include <string>
#include <vector>

using namespace asmjit;

namespace {

struct Case {
    const char* name;
    const char* asm_text;   // 供 LLVM 汇编器使用的等价助记符
    void (*emit)(a64::Assembler* a);
};

void emit_nop(a64::Assembler* a) { a->nop(); }
void emit_mov_x0_x0(a64::Assembler* a) { a->mov(a64::x0, a64::x0); }
void emit_add_x0_x1_16(a64::Assembler* a) { a->add(a64::x0, a64::x1, Imm(16)); }
void emit_sub_sp_80(a64::Assembler* a) { a->sub(a64::sp, a64::sp, Imm(80)); }
void emit_cmp_x11_x12(a64::Assembler* a) { a->cmp(a64::x11, a64::x12); }
void emit_ldrb_w14_post1(a64::Assembler* a) {
    a->ldrb(a64::w14, a64::ptr(a64::x11).post(1));
}
void emit_str_xzr_post8(a64::Assembler* a) { a->str(a64::xzr, a64::ptr(a64::x14).post(8)); }
void emit_cbnz_w15_8(a64::Assembler* a) { a->cbnz(a64::w15, Imm(8)); }
void emit_movk_x0_1234_l16(a64::Assembler* a) { a->movk(a64::x0, Imm(0x1234), Imm(16)); }
void emit_ret(a64::Assembler* a) { a->ret(a64::x30); }
// 注：MRS/系统寄存器 case 暂不纳入 oracle（asmjit 的 a64::SysReg 在部分构建配置下
// 不对外暴露，编译期依赖 host/target 宏；引擎侧用它没问题，但 probe 保持最小可移植）。

const std::vector<Case>& cases() {
    static const std::vector<Case> c = {
        {"nop", "nop", emit_nop},
        {"mov_x0_x0", "mov x0, x0", emit_mov_x0_x0},
        {"add_x0_x1_16", "add x0, x1, #16", emit_add_x0_x1_16},
        {"sub_sp_sp_80", "sub sp, sp, #80", emit_sub_sp_80},
        {"cmp_x11_x12", "cmp x11, x12", emit_cmp_x11_x12},
        {"ldrb_w14_post1", "ldrb w14, [x11], #1", emit_ldrb_w14_post1},
        {"str_xzr_post8", "str xzr, [x14], #8", emit_str_xzr_post8},
        {"cbnz_w15_8", "cbnz w15, . + 8", emit_cbnz_w15_8},
        {"movk_x0_1234_l16", "movk x0, #0x1234, lsl #16", emit_movk_x0_1234_l16},
        {"ret", "ret", emit_ret},
    };
    return c;
}

}  // namespace

int main(int argc, char** argv) {
    if (argc < 2) {
        std::fprintf(stderr, "usage: %s <case|--list>\n", argv[0]);
        return 2;
    }
    if (std::strcmp(argv[1], "--list") == 0) {
        for (auto& c : cases()) std::printf("%s\t%s\n", c.name, c.asm_text);
        return 0;
    }
    for (auto& c : cases()) {
        if (std::strcmp(argv[1], c.name) != 0) continue;
        a64::Assembler a;
        c.emit(&a);
        a.finalize();
        // 取 asmjit 实际生成字节：finalize 后从 assembler 的 code buffer 读
        const uint8_t* buf = a.bufferData();
        size_t n = a.offset();
        std::string hex;
        char t[8];
        for (size_t i = 0; i < n && i < 4; ++i) {
            std::snprintf(t, sizeof(t), "%02x", static_cast<unsigned char>(buf[i]));
            hex += t;
        }
        std::printf("%s\n", hex.c_str());
        return 0;
    }
    std::fprintf(stderr, "unknown case: %s\n", argv[1]);
    return 2;
}
