// skroot_patch_main.cpp — SPRoot 修补引擎主程序
// 契约依据: SPROOT-DOC-C1 v1.0（资产 a_7103b902143367a9）
// 机制依据: SPROOT-DOC-A1（SKRoot 上游 master@943382cd 源码解析）
//
// 与上游 patch_kernel_root.cpp 的差异（全部为 C1 契约要求）:
//   1. 全参数化零交互（上游为 cin 交互式）
//   2. 显式 --out 输出（上游原地回写输入文件）
//   3. 失败不落盘铁律（非 0 退出码绝不产生 --out 文件）
//   4. NDJSON 进度流 + report.json（含 hook_table / hidden_dir_prefix）
//   5. 退出码 12 枚举（C1 §3）
//   6. report.json 是密钥权威源；stdout ROOT_KEY= 末行为兜底（客户端会签口径）

#include "patch_pipeline.h"
#include "bootimg.h"
#include "spr_sha256.h"
#include "analyze/base_func.h"
#include "analyze/symbol_analyze.h"
#include "analyze/init_cred_searcher.h"

#include "spr_json.hpp"

#include <algorithm>
#include <cctype>
#include <chrono>
#include <cstring>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <sstream>
#include <string>
#include <vector>

namespace fs = std::filesystem;
using spr::J;

// ---------------------------------------------------------------- 退出码（C1 §3）
enum ExitCode : int {
    OK                    = 0,
    E_USAGE               = 101,
    E_INPUT               = 102,
    E_KALLSYMS            = 201,
    E_SYMBOL_MISSING      = 202,
    E_OFFSET_DERIVE       = 210,
    E_INIT_CRED_NOT_FOUND = 211,
    E_NO_HOOK_SPACE       = 220,
    E_UNSUPPORTED_VERSION = 230,
    E_WRITE               = 240,
    E_INTERNAL            = 250,
};

// ---------------------------------------------------------------- 引擎上下文
struct EngineOptions {
    std::string in_path;
    std::string out_path;
    std::string report_path;   // <out>.report.json
    std::string workdir;       // <out>.work/（失败时保留诊断）
    std::string json_sink;     // --json NDJSON 落盘（可空）
    std::string key;
    bool key_from_user = false;
};

static std::ofstream g_ndjson;
static bool g_quiet = false;

static long long now_ms() {
    using namespace std::chrono;
    return duration_cast<milliseconds>(system_clock::now().time_since_epoch()).count();
}

static void emit_line(std::string line, bool to_stdout_only = false) {
    if (g_ndjson.is_open() && !to_stdout_only) { g_ndjson << line << "\n"; g_ndjson.flush(); }
    if (!g_quiet) std::cout << line << std::endl;
}

static void emit_progress(const char* phase, int pct, const std::string& detail) {
    J ev; ev.set("ts", now_ms()).set("event", "progress").set("phase", phase).set("pct", pct).set("detail", detail);
    emit_line(ev.dump());
}

static void emit_result(int code, J extra) {
    J ev; ev.set("ts", now_ms()).set("event", "result").set("code", code);
    // extra 的字段平铺进 result 事件
    if (cJSON* e = extra.raw()) {
        cJSON* child = e->child;
        while (child) {
            cJSON* dup = cJSON_Duplicate(child, 1);
            cJSON_AddItemToObject(ev.raw(), child->string, dup);
            child = child->next;
        }
    }
    emit_line(ev.dump());
}

// ---------------------------------------------------------------- 密钥
static bool valid_key(const std::string& k) {
    if (k.size() != 48) return false;
    for (char c : k) {
        unsigned char u = static_cast<unsigned char>(c);
        if (!(u >= '0' && u <= '9') && !(u >= 'A' && u <= 'Z') && !(u >= 'a' && u <= 'z')) return false;
    }
    return true;
}

// ---------------------------------------------------------------- report / 原子落盘
static bool write_file_atomic(const std::string& path, const std::string& content, std::string& err) {
    std::string tmp = path + ".tmp";
    {
        std::ofstream f(tmp, std::ios::binary | std::ios::trunc);
        if (!f) { err = "cannot create " + tmp; return false; }
        f << content;
        if (!f.good()) { f.close(); fs::remove(tmp); err = "write failed " + tmp; return false; }
    }
    std::error_code ec;
    fs::rename(tmp, path, ec);
    if (ec) { fs::remove(tmp); err = ec.message(); return false; }
    return true;
}


// ---------------------------------------------------------------- report 构造
struct EngineState {
    std::string kernel_version = "unknown";
    std::vector<char> file_buf;
    size_t input_size = 0;
    size_t output_size = 0;
    std::string input_sha256;
    std::string output_sha256;
    J hook_table = J::array();
    std::vector<std::string> errors;
    size_t cfi_patch_count = 0;
    size_t huawei_patch_count = 0;
};

static J build_report(const EngineOptions& opt, const EngineState& st, const std::string& root_key) {
    // C1 §2.3：input/output 均含 path/sha256/size（qa 门禁用其做等长与产物一致性断言）
    J input; input.set("path", opt.in_path).set("sha256", st.input_sha256)
              .set("size", static_cast<long long>(st.input_size));
    J output; output.set("path", opt.out_path).set("sha256", st.output_sha256)
                .set("size", static_cast<long long>(st.output_size));
    J key_spec;
    key_spec.set("charset", "A-Za-z0-9").set("length", 48).set("stored_len", 47)
            .set("source", opt.key_from_user ? "user" : "gen");
    J errors = J::array();
    for (auto& e : st.errors) errors.push(e);
    J rep;
    // hook_table 必须原样内联为数组（旧写法把 cJSON* 隐式转成 bool → "hook_table": true，qa 会判失败）
    J hooks = J::array();
    if (cJSON* arr = st.hook_table.raw()) {
        const cJSON* it = nullptr;
        cJSON_ArrayForEach(it, arr) { hooks.push(J(cJSON_Duplicate(it, 1))); }
    }
    rep.set("engine_version", "1.0.0")
       .set("upstream_baseline", "943382cd")
       .set("input", std::move(input))
       .set("output", std::move(output))
       .set("kernel_version", st.kernel_version)
       .set("key_spec", std::move(key_spec))
       .set("hook_table", std::move(hooks))
       .set("hidden_dir_prefix", root_key.substr(0, 16))
       .set("errors", std::move(errors));
    return rep;
}

// ---------------------------------------------------------------- bootimg 模式
// 内置 boot.img 解析/重打包（不依赖 magiskboot/kptools，见 bootimg.h 决策说明）
// 用法：
//   skroot-patch --bootimg info           --in boot.img
//   skroot-patch --bootimg extract-kernel --in boot.img --out kernel.raw
//   skroot-patch --bootimg unpack         --in boot.img --outdir DIR
//   skroot-patch --bootimg repack         --in boot.img --kernel kernel.patched --out new-boot.img [--strip-avb]
static int bootimg_cli(const std::string& action, const std::string& in_path,
                       const std::string& out_path, const std::string& kernel_path,
                       const std::string& outdir, bool strip_avb, bool quiet) {
    std::vector<char> buf = read_file_buf(in_path);
    if (buf.empty()) {
        std::cerr << "E102: fail to read boot image: " << in_path << "\n";
        return E_INPUT;
    }
    bootimg::Info info;
    if (!bootimg::read_info(buf, info)) {
        std::cerr << "E102: " << info.error << "\n";
        return E_INPUT;
    }
    if (!quiet) std::cout << bootimg::describe(info);

    auto write_out = [&](const std::string& path, const std::vector<char>& data) -> int {
        std::string err;
        std::string content(data.data(), data.size());
        if (!write_file_atomic(path, content, err)) {
            std::cerr << "E240: " << err << "\n";
            return E_WRITE;
        }
        if (!quiet) std::cout << "wrote " << path << " (" << data.size() << " bytes)\n";
        return OK;
    };

    if (action == "info") return OK;

    if (action == "extract-kernel") {
        std::vector<char> kernel;
        std::string err;
        if (!bootimg::extract_kernel(buf, info, kernel, err)) {
            std::cerr << "E230: " << err << "\n";
            return E_UNSUPPORTED_VERSION;
        }
        return write_out(out_path, kernel);
    }

    if (action == "unpack") {
        if (outdir.empty()) { std::cerr << "E101: --outdir required for unpack\n"; return E_USAGE; }
        std::error_code ec; fs::create_directories(outdir, ec);
        std::vector<char> kernel;
        std::string err;
        if (!bootimg::extract_kernel(buf, info, kernel, err)) {
            std::cerr << "E230: " << err << "\n";
            return E_UNSUPPORTED_VERSION;
        }
        int rc = write_out(outdir + "/kernel", kernel);
        if (rc != OK) return rc;
        return write_out(outdir + "/bootimg.info.txt",
                         std::vector<char>(bootimg::describe(info).begin(), bootimg::describe(info).end()));
    }

    if (action == "repack") {
        std::vector<char> patched = read_file_buf(kernel_path);
        if (patched.empty()) {
            std::cerr << "E102: fail to read patched kernel: " << kernel_path << "\n";
            return E_INPUT;
        }
        std::vector<char> out;
        std::string err;
        if (!bootimg::repack(buf, info, patched, strip_avb, out, err)) {
            std::cerr << "E240: " << err << "\n";
            return E_WRITE;
        }
        return write_out(out_path, out);
    }

    std::cerr << "E101: unknown --bootimg action: " << action
              << " (info|extract-kernel|unpack|repack)\n";
    return E_USAGE;
}

// ---------------------------------------------------------------- 主流程
// 引擎唯一逻辑实现。extern "C" 以便被 JNI 包装（engine/android/jni/
// skroot_patch_jni.cpp → libskpatchjni.so）直接调用，避免 -Dmain 改名造成的
// C++ 名字修饰不匹配；CLI 产物（libskrootpatch.so）由文末 main() 转发。
extern "C" int spr_engine_run(int argc, char* argv[]) {
    EngineOptions opt;
    bool have_in = false, have_out = false, have_key = false;

    // bootimg 模式提前分流（不进 patch 主流程）：--bootimg <action>
    {
        std::string bm_action, bm_in, bm_out, bm_kernel, bm_outdir;
        bool bm = false, bm_strip = false, bm_quiet = false;
        for (int i = 1; i < argc; ++i) {
            std::string a = argv[i];
            auto nxt = [&]() -> std::string { return (i + 1 < argc) ? argv[++i] : std::string(); };
            if (a == "--bootimg") { bm = true; bm_action = nxt(); }
            else if (a == "--in") bm_in = nxt();
            else if (a == "--out") bm_out = nxt();
            else if (a == "--kernel") bm_kernel = nxt();
            else if (a == "--outdir") bm_outdir = nxt();
            else if (a == "--strip-avb") bm_strip = true;
            else if (a == "--quiet") bm_quiet = true;
        }
        if (bm) {
            if (bm_action.empty()) { std::cerr << "E101: --bootimg requires an action\n"; return E_USAGE; }
            return bootimg_cli(bm_action, bm_in, bm_out, bm_kernel, bm_outdir, bm_strip, bm_quiet);
        }
    }

    for (int i = 1; i < argc; ++i) {
        std::string a = argv[i];
        auto need = [&](const char* name) -> const char* {
            if (i + 1 >= argc) { std::cerr << "missing value for " << name << "\n"; exit(E_USAGE); }
            return argv[++i];
        };
        if (a == "--in") { opt.in_path = need("--in"); have_in = true; }
        else if (a == "--out") { opt.out_path = need("--out"); have_out = true; }
        else if (a == "--key") { opt.key = need("--key"); opt.key_from_user = true; have_key = true; }
        else if (a == "--gen-key") { have_key = true; }
        else if (a == "--json") { opt.json_sink = need("--json"); }
        else if (a == "--workdir") { opt.workdir = need("--workdir"); }
        else if (a == "--quiet") { g_quiet = true; }
        else if (a == "--help" || a == "-h") {
            std::cout << "SPRoot patch engine v1.0.0 (contract SPROOT-DOC-C1 v1.0)\n"
                      << "usage: skroot-patch --in <kernel_image> --out <patched_kernel> (--gen-key|--key <48ch>)\n"
                      << "       [--json <ndjson_sink>] [--workdir <dir>] [--quiet]\n";
            return OK;
        }
        else { std::cerr << "unknown arg: " << a << "\n"; return E_USAGE; }
    }

    if (!have_in || !have_out || !have_key) {
        std::cerr << "E101: missing required args (--in/--out/--gen-key|--key)\n";
        J e; e.set("error", "E101: missing required args");
        emit_result(E_USAGE, std::move(e));
        return E_USAGE;
    }
    if (opt.key_from_user && !valid_key(opt.key)) {
        J e; e.set("error", "E101: --key must match ^[A-Za-z0-9]{48}$");
        emit_result(E_USAGE, std::move(e));
        return E_USAGE;
    }
    {
        std::string ext = fs::path(opt.in_path).extension().string();
        std::transform(ext.begin(), ext.end(), ext.begin(), [](unsigned char c){ return std::tolower(c); });
        if (fs::exists(opt.in_path) && (ext == ".img" || ext == ".gz")) {
            J e; e.set("error", "E102: input looks like boot image/compressed kernel; unpack & decompress first");
            emit_result(E_INPUT, std::move(e));
            return E_INPUT;
        }
    }

    opt.report_path = opt.out_path + ".report.json";
    if (opt.workdir.empty()) opt.workdir = opt.out_path + ".work";
    if (!opt.json_sink.empty()) g_ndjson.open(opt.json_sink, std::ios::binary | std::ios::trunc);

    EngineState st;
    std::string root_key = opt.key_from_user ? opt.key : generate_random_str(48);

    int final_code = E_INTERNAL;
    std::string fail_err, fail_detail;
    PatchKernelOffset off{};
    KernelSymbolOffset sym{};
    std::vector<patch_bytes_data> vec_patch;
    PatchKernelResult pr{};

    // 统一失败出口：删 --out、写 report、emit result、ROOT_KEY 兜底行
    auto fail = [&](int code, const char* err, const std::string& detail) -> int {
        final_code = code;
        fail_err = err; fail_detail = detail;
        st.errors.push_back(std::string(err) + (detail.empty() ? "" : " | " + detail));
        {   // 失败不落盘铁律（C1 §3）：删除已产出的 --out
            std::error_code ec; fs::remove(opt.out_path, ec);
        }
        J rep = build_report(opt, st, root_key);
        std::string rerr;
        if (!write_file_atomic(opt.report_path, rep.dump(2), rerr))
            st.errors.push_back("report write failed: " + rerr);
        J extra; extra.set("error", std::string(err) + (detail.empty() ? "" : " | " + detail));
        emit_result(code, std::move(extra));
        std::cout << "ROOT_KEY=" << root_key << std::endl;   // 兜底通道
        if (g_ndjson.is_open()) g_ndjson.close();
        return code;
    };

    // 1) 读入
    emit_progress("read_input", 0, "path=" + opt.in_path);
    st.file_buf = read_file_buf(opt.in_path);
    st.input_size = st.file_buf.size();
    st.input_sha256 = spr::sha256_hex(st.file_buf);
    if (st.file_buf.empty()) return fail(E_INPUT, "E102: fail to open/read input", opt.in_path);
    emit_progress("read_input", 5, "size=" + std::to_string(st.input_size));

    // 2) kallsyms
    emit_progress("parse_kallsyms", 5, "...");
    {
        SymbolAnalyze symbol_analyze(st.file_buf);
        if (!symbol_analyze.analyze_kernel_symbol())
            return fail(E_KALLSYMS, "E201: failed to analyze kernel symbols", "CONFIG_KALLSYMS missing or unsupported layout");
        sym = symbol_analyze.get_symbol_offset();
        KernelVersionParser kv(st.file_buf);
        {   // C1 §2.3：kernel_version 必须是真实版本串（qa/审计依赖），解析失败才回落 unknown
            std::string ver = kv.get_kernel_version();
            if (!ver.empty()) st.kernel_version = ver;
            std::string tag = kv.get_kernel_tag();
            emit_progress("parse_kallsyms", 15, "symbols parsed, version=" + st.kernel_version +
                                                     (tag.empty() ? "" : ", tag=" + tag));
        }
    }

    // 3) 关键符号必选集（C1 §4 / A1 §3）
    emit_progress("derive_offsets", 20, "checking required symbols");
    {
        bool has_execve = sym.do_execve.offset || sym.do_execveat.offset || sym.do_execveat_common.offset ||
                          sym.do_execve_common.offset || sym.__do_execve_file.offset;
        if (!has_execve) return fail(E_SYMBOL_MISSING, "E202: no do_execve family symbol", "-");
        if (!sym.filldir64.offset) return fail(E_SYMBOL_MISSING, "E202: filldir64 missing", "-");
        if (!sym.avc_denied.offset) return fail(E_SYMBOL_MISSING, "E202: avc_denied missing", "-");
        if (!sym.audit_log_start.offset) return fail(E_SYMBOL_MISSING, "E202: audit_log_start missing", "-");
        if (!sym.sys_getuid.offset) return fail(E_SYMBOL_MISSING, "E202: sys_getuid missing", "-");
        if (!sym.prctl_get_seccomp.offset) return fail(E_SYMBOL_MISSING, "E202: prctl_get_seccomp missing", "-");
        // >=6.1 分支判定复用上游逻辑：patch_kernel_handler 内部按 die/drm 符号是否存在选择路径，
        // 此处只预警：若 <6.1 空间（cfi_check 或 0x200~0x300 空隙）也不可用则必失败。
    }

    // 4) 偏移推导（上游逻辑复用，错误码 E210）
    {
        if (!parser_cred_offset(st.file_buf, sym.sys_getuid, off.cred_offset))
            return fail(E_OFFSET_DERIVE, "E210: cred offset derivation failed", "anchor=sys_getuid");
        if (!parse_cred_uid_offset(st.file_buf, sym.sys_getuid, off.cred_offset, off.cred_uid_offset))
            return fail(E_OFFSET_DERIVE, "E210: cred uid offset derivation failed", "anchor=sys_getuid");
        if (!parser_seccomp_offset(st.file_buf, sym.prctl_get_seccomp, off.seccomp_offset))
            return fail(E_OFFSET_DERIVE, "E210: seccomp offset derivation failed", "anchor=prctl_get_seccomp");
        parser_huawei_kti_addr(st.file_buf, sym.kti_randomize_init, off.huawei_kti_addr);
        std::ostringstream d;
        d << "cred=0x" << std::hex << off.cred_offset << ",uid=0x" << off.cred_uid_offset
          << ",seccomp=0x" << off.seccomp_offset;
        emit_progress("derive_offsets", 30, d.str());
    }

    // 5) init_cred
    emit_progress("locate_init_cred", 35, "...");
    {
        InitCredSearcher ics(st.file_buf, off.cred_uid_offset);
        if (!ics.init()) return fail(E_INIT_CRED_NOT_FOUND, "E211: init_cred pattern not found", "-");
    }

    // 6) CFI / 华为 bypass（计数供 report）
    emit_progress("patch_cfi_huawei", 40, "...");
    {
        size_t before = vec_patch.size();
        cfi_bypass(st.file_buf, sym, vec_patch);
        st.cfi_patch_count = vec_patch.size() - before;
        before = vec_patch.size();
        huawei_bypass(st.file_buf, sym, vec_patch);
        st.huawei_patch_count = vec_patch.size() - before;
    }

    // 7) 五组 hook
    emit_progress("patch_hooks", 50, "...");
    {
        pr = patch_kernel_handler(st.file_buf, off, sym, vec_patch);
        if (!pr.patched)
            return fail(E_NO_HOOK_SPACE, "E220: failed to find hook start addr / not enough kernel space", "-");
        J hooks; hooks.set("name", "hooks_embedded").set("blob_count", static_cast<int>(vec_patch.size()))
                      .set("root_key_region", "0x" + [&]{ std::ostringstream o; o << std::hex << pr.root_key_start; return o.str(); }());
        st.hook_table.push(std::move(hooks));
    }

    // 8) 密钥内嵌（47 字符 + NUL，写入 handler 选定的 root_key_start，与上游等价，A1 §4.1）
    emit_progress("embed_key", 80, "len=48");
    {
        std::string write_key = root_key;
        write_key.erase(write_key.size() - 1);
        patch_data(st.file_buf, pr.root_key_start, (void*)write_key.c_str(), write_key.length() + 1, vec_patch);
    }

    // 9) 应用补丁并写输出（临时文件 + rename）
    emit_progress("write_output", 95, "...");
    {
        std::vector<char> patched = st.file_buf;
        for (auto& item : vec_patch) {
            std::vector<uint8_t> bytes(item.str_bytes.length() / 2);
            hex2bytes((uint8_t*)item.str_bytes.c_str(), bytes.data());
            if (item.write_addr + bytes.size() > patched.size())
                return fail(E_INTERNAL, "E250: patch out of range", "addr overflow");
            std::memcpy(patched.data() + item.write_addr, bytes.data(), bytes.size());
        }
        std::string werr;
        std::string content(patched.data(), patched.size());
        st.output_size = patched.size();
        st.output_sha256 = spr::sha256_hex(patched);
        if (!write_file_atomic(opt.out_path, content, werr))
            return fail(E_WRITE, "E240: output write failed", werr);
    }

    // 10) 成功：report + stdout 兜底行 + result 事件
    final_code = OK;
    {
        J rep = build_report(opt, st, root_key);
        std::string rerr;
        write_file_atomic(opt.report_path, rep.dump(2), rerr);   // 失败仅记日志，不影响主产物
        std::cout << "ROOT_KEY=" << root_key << std::endl;       // 客户端会签口径：兜底读取源
        J extra;
        extra.set("root_key", root_key).set("report_ref", opt.report_path).set("hook_count", static_cast<int>(vec_patch.size()));
        emit_result(OK, std::move(extra));
        std::error_code ec; fs::remove_all(opt.workdir, ec);     // 成功清理 workdir
    }

    if (g_ndjson.is_open()) g_ndjson.close();
    return final_code;
}

// CLI 入口：独立 ELF 形态（libskrootpatch.so 实为静态可执行文件）的唯一 main。
// JNI 形态不链接本函数（构建时定义 SPROOT_NO_CLI_MAIN，由 libskpatchjni.so 调 spr_engine_run）。
#ifndef SPROOT_NO_CLI_MAIN
int main(int argc, char* argv[]) { return spr_engine_run(argc, argv); }
#endif
