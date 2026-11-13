#pragma once
#include <iostream>
#include <filesystem>
#include "analyze/base_func.h"
#include "analyze/aarch64_insn.h"

#define ROOT_KEY_LEN 48
#define FOLDER_HEAD_ROOT_KEY_LEN 16

// R1：IF_EXIT 不再 exit(0)（上游遗留：Linux 上 system("pause") 立即失败 → exit(0)
// = **rc 0 伪成功**，绕过 fail() 的 E-code 体系；"hook 符号全缺席"会静默变成
// "成功但无产物"，与用户"刷错旧镜像"的症状同构，无法排障）。
// 现语义：条件为真时记录到 g_patch_abort_reason 并**不中断**，由调用方在
// patch 结束后检查（PATCH_AND_CONSUME 的 0 长度本身就会让管线判断失败）。
// 所有旧 IF_EXIT 调用点语义保持"报告异常"，退出码统一由 skroot_patch_main 的 fail() 决定。
#include <string>
extern std::string g_patch_abort_reason;
#define IF_EXIT(cond) do { if (cond) { std::string __r = "[ERROR] Patch empty addr!"; printf("%s\n", __r.c_str()); if (g_patch_abort_reason.empty()) g_patch_abort_reason = __r; } } while (0)

#define PATCH_AND_CONSUME(region, call_expr) \
    do { \
        size_t __sz = (call_expr); \
        IF_EXIT(!__sz); \
        IF_EXIT(!region.offset); \
        (region).consume(__sz); \
    } while (0)

struct patch_bytes_data {
	std::string str_bytes;
	size_t write_addr = 0;
};

static size_t patch_ret_cmd(const std::vector<char>& file_buf, size_t start, std::vector<patch_bytes_data>& vec_out_patch_bytes_data) {
	if (start == 0) return 0;
	// ENG-7：符号地址贴近缓冲末尾时读首指令会越界（最多 3 字节）→ 直接拒绝。
	if (start + sizeof(uint32_t) > file_buf.size()) return 0;
	uint32_t insn = *(uint32_t*)&file_buf[start];
	patch_bytes_data blob;
	if (aarch64_insn_is_paciaz(insn) || aarch64_insn_is_paciasp(insn)
		|| aarch64_insn_is_pacibz(insn) || aarch64_insn_is_pacibsp(insn) || aarch64_insn_is_bti(insn)) blob = { "DF2403D5C0035FD6", start };
	else blob = { "C0035FD6", start };
	vec_out_patch_bytes_data.push_back(blob);
	return blob.str_bytes.length() / 2;
}

static size_t patch_ret_1_cmd(const std::vector<char>& file_buf, size_t start, std::vector<patch_bytes_data>& vec_out_patch_bytes_data) {
	if (start == 0) return 0;
	if (start + sizeof(uint32_t) > file_buf.size()) return 0;   // ENG-7
	uint32_t insn = *(uint32_t*)&file_buf[start];
	patch_bytes_data blob;
	if (aarch64_insn_is_paciaz(insn) || aarch64_insn_is_paciasp(insn)
		|| aarch64_insn_is_pacibz(insn) || aarch64_insn_is_pacibsp(insn) || aarch64_insn_is_bti(insn)) blob = { "DF2403D5200080D2C0035FD6", start };
	else blob = { "200080D2C0035FD6", start };
	vec_out_patch_bytes_data.push_back(blob);
	return blob.str_bytes.length() / 2;
}

static size_t patch_ret_0_cmd(const std::vector<char>& file_buf, size_t start, std::vector<patch_bytes_data>& vec_out_patch_bytes_data) {
	if (start == 0) return 0;
	if (start + sizeof(uint32_t) > file_buf.size()) return 0;   // ENG-7
	uint32_t insn = *(uint32_t*)&file_buf[start];
	patch_bytes_data blob;
	if (aarch64_insn_is_paciaz(insn) || aarch64_insn_is_paciasp(insn)
		|| aarch64_insn_is_pacibz(insn) || aarch64_insn_is_pacibsp(insn) || aarch64_insn_is_bti(insn)) blob = { "DF2403D5E0031F2AC0035FD6", start };
	else blob = { "E0031F2AC0035FD6", start };
	vec_out_patch_bytes_data.push_back(blob);
	return blob.str_bytes.length() / 2;
}

static size_t patch_data(const std::vector<char>& file_buf, size_t start, void* buf, size_t buf_size, std::vector<patch_bytes_data>& vec_out_patch_bytes_data) {
	if (start == 0) return 0;
	if (start + buf_size > file_buf.size()) return 0;   // ENG-7：数据写入越界同样拒绝
	std::string str_bytes = bytes2hex((const unsigned char*)buf, buf_size);
	vec_out_patch_bytes_data.push_back({ str_bytes, start });
	return buf_size;
}
