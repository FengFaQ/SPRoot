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
    // workdir 所有权标记（真机事故修复，见 NEW-DOC-02 §8.1）：
    //   true  = 调用方在命令行**显式**传了 --workdir ⇒ 该目录及其内容生命周期归调用方，
    //           引擎绝不删除（不能靠"字符串是否等于默认值"来推断！调用方完全可能显式
    //           传入与默认值相同的路径，那也仍然是调用方的目录）。
    //   false = 调用方没传，workdir 是引擎自己拼出来的默认值 <out>.work。
    bool workdir_from_user = false;
    std::string json_sink;     // --json NDJSON 落盘（可空）
    std::string key;
    bool key_from_user = false;
    // 密钥 ABI（P0 防错，见 NEW-DOC-02 §7.6）：
    //   本引擎只实现 47 字符 + NUL（key[0..46] 写入跳板，第 48 字节恒 0）。
    //   Skroot-Next 用「48 字节无 NUL、按输入长度比较」，两套**不可混用**。
    //   因此显式要求 --key-abi=47nul（缺省即此值）；任何其它取值一律拒绝。
    std::string key_abi = "47nul";
    bool dump_symbols = false;
    // 隐藏测试参数（仅自检/CI 用，不面向用户）：强制按给定内核版本走分支选择逻辑
    std::string assume_kernel_version;   // --assume-kernel-version <ver>
    bool assume_huawei = false;          // --assume-huawei-symbols
    bool self_test_groups = false;       // --self-test-groups
    bool self_test_workdir = false;      // --self-test-workdir（workdir 所有权判定矩阵）
    bool self_test_keyabi = false;       // --self-test-keyabi（密钥 47+NUL ABI 闭环）
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


// ---------------------------------------------------------------- workdir 所有权
// 契约（SPROOT-DOC-C1 §3 补充 + NEW-DOC-02 §8.1）：
//   **显式传入 --workdir 时，该目录及其内容的所有权归调用方，引擎不得删除其中任何东西；
//     只有默认 workdir（<out>.work，引擎自己拼出来的）才由引擎在成功收尾时回收。**
//
// 真机事故（2026-11-09，魅族21 首跑）：客户端把 --out=kernel.patched 放进了显式传入的
// --workdir 内，引擎成功收尾时无条件 remove_all(workdir)，把产物一起删掉却仍返回 rc=0
// → 客户端只看到"引擎返回成功但没有产物"，修补在最后一步静默失败。
//
// child 是否位于 dir **之内**。必须按路径分隔符做边界判定：
//   实现若写成 child.compare(0, dir.size(), dir) == 0（纯前缀），
//   就会把 "/a/wx/k" 误判成 "/a/w" 的子路径（假阳性）。
static bool path_within_dir(const std::string& child, const std::string& dir) {
    if (child.empty() || dir.empty()) return false;
    std::string d = dir;
    while (d.size() > 1 && (d.back() == '/' || d.back() == '\\')) d.pop_back();   // 去尾部分隔符
    if (child.size() <= d.size()) return false;                                    // 同路径/更短 ⇒ 不在其内
    if (child.compare(0, d.size(), d) != 0) return false;
    const char sep = child[d.size()];
    return sep == '/' || sep == '\\';
}

// 成功收尾是否回收 workdir。**纯函数**（无 IO、无全局状态），主流程与
// --self-test-workdir 共用同一份判定，保证 CI 能对"事故现场"做真正的行为级回归
// （不依赖 qemu 跑真内核，见 verify_artifact.sh A9d）。
//   返回 true 当且仅当：
//     ① 调用方没显式传 --workdir（workdir_from_user == false）；
//     ② workdir 恰好是引擎默认值 <out>.work（不是别的什么目录）；
//     ③ 没有任何引擎产物（--out / <out>.report.json / --json）落在 workdir 内。
static bool should_reclaim_workdir(bool workdir_from_user,
                                   const std::string& workdir,
                                   const std::string& out_path,
                                   const std::string& report_path,
                                   const std::string& json_sink) {
    if (workdir_from_user) return false;                        // ① 显式传入 ⇒ 归调用方
    if (workdir != out_path + ".work") return false;             // ② 只认引擎自建的默认目录
    if (path_within_dir(out_path, workdir)) return false;        // ③ 产物兜底（默认值下理论不可达，防未来演进）
    if (path_within_dir(report_path, workdir)) return false;
    if (path_within_dir(json_sink, workdir)) return false;       // --json 落在默认 workdir 内 ⇒ 不删（否则丢事件流）
    return true;
}

// 隐藏自检（--self-test-workdir）：workdir 所有权判定矩阵。
// 无需内核镜像 / 无需密钥 / 无需 qemu 之外的任何输入 ⇒ CI 每次都能跑，
// 是 A9 从"静态 grep"升级为"行为断言"的关键（T-12 铁律：断言必须能被负样本打红）。
static int self_test_workdir_contract() {
    struct Case {
        const char* name;
        bool        from_user;   // 命令行是否显式出现 --workdir
        const char* workdir;     // 空 = 调用方没传（走默认值 <out>.work）
        const char* out;
        const char* json_sink;   // 空 = 无 --json
        bool        expect_reclaim;
    };
    // out_path 形如 /d/kernel.patched；report_path 恒为 out + ".report.json"
    const Case cases[] = {
        // 默认 workdir：引擎自建 ⇒ 成功收尾应回收（契约正向用例，防"干脆永不删"的偷懒实现）
        { "default_workdir_reclaimed",                 false, "",                    "/d/kernel.patched",        "",                                  true  },
        // ★ 真机事故现场：--out 在显式 --workdir 内 ⇒ 必须保留
        { "explicit_workdir_containing_out",           true,  "/d/patch",            "/d/patch/kernel.patched",  "/d/patch/ev.ndjson",                false },
        // 事故的"改名版"：客户端显式传入与默认值**字符串相同**的路径 ⇒ 仍归调用方（必须在解析时标记，不能靠字符串比对）
        { "explicit_workdir_equal_default_string",     true,  "/d/kernel.patched.work","/d/kernel.patched",       "",                                  false },
        // 显式 workdir，且产物在外部 ⇒ 保留（目录归调用方）
        { "explicit_workdir_product_outside",          true,  "/d/w",                "/d/kernel.patched",        "/d/ev.ndjson",                      false },
        // 默认 workdir，但 --json 事件流落在其中 ⇒ 不能删（否则丢 NDJSON；8a196f0 修复未覆盖的同类残留风险）
        { "default_workdir_json_sink_inside",          false, "",                    "/d/kernel.patched",        "/d/kernel.patched.work/ev.ndjson",  false },
        // 默认 workdir，--json 在外部的同名**前缀**目录（/d/kernel.patched.workx）⇒ 不算包含，仍回收（边界判定）
        { "default_workdir_json_prefix_sibling",       false, "",                    "/d/kernel.patched",        "/d/kernel.patched.workx/ev.ndjson", true  },
        // 显式 workdir 指向 /（极端输入）：产物在 / 下 ⇒ 保留（绝不删调用方的根）
        { "explicit_workdir_root",                     true,  "/",                   "/kernel.patched",          "/ev.ndjson",                        false },
    };
    struct DirCase { const char* child; const char* dir; bool expect; };
    const DirCase dircases[] = {
        { "/a/w/k",  "/a/w",   true  },
        { "/a/w/k",  "/a/w/",  true  },   // 尾部分隔符
        { "/a/w",    "/a/w",   false },   // 自身不算"之内"
        { "/a/wx/k", "/a/w",   false },   // ★ 纯前缀假阳性（无边界判定的实现会在此翻车）
        { "/a/k",    "/a",     true  },
        { "",        "/a",     false },
    };

    int failures = 0, total = 0;
    for (const auto& c : cases) {
        std::string wd = (c.workdir[0] == '\0') ? (std::string(c.out) + ".work") : std::string(c.workdir);
        std::string rep = std::string(c.out) + ".report.json";
        bool got = should_reclaim_workdir(c.from_user, wd, c.out, rep, c.json_sink);
        ++total;
        const bool ok = (got == c.expect_reclaim);
        if (!ok) ++failures;
        std::cout << "WDCASE " << c.name
                  << " explicit_workdir=" << (c.from_user ? "yes" : "no")
                  << " expect=" << (c.expect_reclaim ? "reclaim" : "keep")
                  << " got="    << (got ? "reclaim" : "keep")
                  << (ok ? " PASS" : " FAIL") << "\n";
    }
    for (const auto& c : dircases) {
        bool got = path_within_dir(c.child, c.dir);
        ++total;
        const bool ok = (got == c.expect);
        if (!ok) ++failures;
        std::cout << "WDDIR  path_within_dir(\"" << c.child << "\", \"" << c.dir << "\")"
                  << " expect=" << (c.expect ? "inside" : "outside")
                  << " got="    << (got ? "inside" : "outside")
                  << (ok ? " PASS" : " FAIL") << "\n";
    }
    std::cout << "WDRESULT " << (failures == 0 ? "PASS" : "FAIL")
              << " " << (total - failures) << "/" << total << " cases\n";
    return failures == 0 ? OK : 1;
}

// ---------------------------------------------------------------- 密钥 ABI（P0，提权链）
// **内核侧存储格式的唯一来源**：CLI 收到的 48 字符密钥，只把**前 47 字符**写进内核，
// 紧跟一个 NUL ⇒ 共 48 字节，第 48 字节恒为 0（与上游 SKRoot `write_key.erase(size()-1)`
// + `patch_data(..., len+1)` 逐字节等价）。
//
// 提权链（CALL v1.1 E1，必须全链一致）：
//   App 生成/保存 48 字符密钥 → 引擎写 key[0:47] + NUL → 内核存 47 字符 + NUL
//   → daemon 读 /data/adb/sproot/key（47 字符纯文本）→ escalation_token() 47 字符
//   → execve 传 47 字符 → 内核逐字节 strcmp 命中。
//
// ⚠️ 任何一环用 48 字符去比（或把 48 个可见字符塞进内核），都会**静默失配**
//    （普通 ENOENT，无日志、无计数）—— 与 Skroot-Next 的 48-no-NUL ABI 混用是同一类坑。
static constexpr size_t kKeyInputChars = 48;   // CLI 契约：^[A-Za-z0-9]{48}$
static constexpr size_t kKeyStoredChars = 47;  // 内核内嵌的可见字符数
static constexpr size_t kKeyStoredBytes = 48;  // 47 字符 + NUL

// 由 CLI 密钥派生内核内嵌字节。返回 false 只可能因为调用方没先做 48 字符校验（编程错误）。
static bool embed_key_bytes(const std::string& cli_key, std::string& out) {
    if (cli_key.size() < kKeyStoredChars) return false;
    out.assign(cli_key, 0, kKeyStoredChars);   // 取前 47 字符：第 48 字符**不写入内核**
    out.push_back('\0');                       // 第 48 字节恒为 NUL
    // 内联断言：格式必须恒为 48 字节且末字节 NUL（防将来改动悄悄破坏 ABI）
    return out.size() == kKeyStoredBytes &&
           static_cast<unsigned char>(out[kKeyStoredChars]) == 0;
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
    // C1 §2.3：逐 hook 落位（非破坏性新增；旧 hook_table 聚合字段保留）
    std::vector<PatchGroupSpan> groups;
    size_t groups_expected = 0;                 // 期望组数（唯一来源 hook_groups_expected_count）
    bool branch_ge_6_1 = true;
    bool has_huawei = false;
    std::string groups_expected_source = "kernel";  // kernel | assume
    std::vector<char> orig_buf;                 // 打补丁前的镜像（回填 orig_bytes）
    std::vector<size_t> patch_addrs;            // vec_patch[i].write_addr 镜像
    std::vector<size_t> patch_lens;             // 每处补丁字节数
    std::vector<std::string> patch_hex;          // 每处补丁 bytes（大写 hex）
};

// 十六进制小写化（vec_patch 内部为大写 hex；orig_bytes 由镜像读出，统一小写便于比对）
static std::string hex_lower(const std::string& s) {
    std::string o = s;
    for (auto& c : o) c = static_cast<char>(std::tolower(static_cast<unsigned char>(c)));
    return o;
}
static std::string hex_of_str(const std::string& s) { return s; }
static std::string hex_of(const std::vector<char>& buf, size_t off, size_t n) {
    static const char* d = "0123456789abcdef";
    std::string o;
    o.reserve(n * 2);
    for (size_t i = 0; i < n && off + i < buf.size(); ++i) {
        unsigned char b = static_cast<unsigned char>(buf[off + i]);
        o.push_back(d[b >> 4]);
        o.push_back(d[b & 0xf]);
    }
    return o;
}

static J build_report(const EngineOptions& opt, const EngineState& st, const std::string& root_key) {
    // C1 §2.3：input/output 均含 path/sha256/size（qa 门禁用其做等长与产物一致性断言）
    J input; input.set("path", opt.in_path).set("sha256", st.input_sha256)
              .set("size", static_cast<long long>(st.input_size));
    J output; output.set("path", opt.out_path).set("sha256", st.output_sha256)
                .set("size", static_cast<long long>(st.output_size));
    J key_spec;
    key_spec.set("charset", "A-Za-z0-9")
            .set("length", 48)            // CLI 输入长度（^[A-Za-z0-9]{48}$）
            .set("stored_len", 47)        // 内核内嵌的可见字符数
            .set("bytes", 48)             // 内核内嵌总字节数 = 47 字符 + NUL
            .set("last_byte_is_nul", true)
            .set("input_len", static_cast<int>(root_key.size()))
            .set("abi", "47+nul")         // P0：与 Skroot-Next 的 48 字节无 NUL ABI 不兼容
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
       .set("hooks", [&st]() -> J {
            // C1 §2.3 五组逐 hook 落位（非破坏性新增）：每组含 group/name/resolved/
            // patch_offset/orig_bytes/new_bytes/target_addr + patches[]（该组全部落位）
            J arr = J::array();
            for (const auto& g : st.groups) {
                J h;
                size_t n = (g.last > g.first) ? (g.last - g.first) : 0;
                h.set("group", g.group ? g.group : "").set("name", g.symbol ? g.symbol : "");
                if (n == 0) {
                    // 未解析组也输出统一字段（空值 + reason），便于 qa 按固定 schema 断言
                    h.set("resolved", false).set("reason", "no_patch_bytes_symbol_missing_or_not_used")
                     .set("hook_addr", std::string("")).set("trampoline", std::string(""))
                     .set("bytes", 0);
                } else {
                    h.set("resolved", true);
                    const auto& first = st.groups.empty() ? g : g;
                    size_t off0 = 0, len0 = 0;
                    // 首处落位（供 qa 快速核对；完整列表见 patches[]）
                    (void)first;
                    J patches = J::array();
                    for (size_t i = g.first; i < g.last; ++i) {
                        size_t addr = st.patch_addrs[i];
                        size_t len = st.patch_lens[i];
                        if (off0 == 0) { off0 = addr; len0 = len; }
                        J pj;
                        pj.set("patch_offset", "0x" + [&]{ std::ostringstream o; o << std::hex << addr; return o.str(); }())
                          .set("orig_bytes", hex_of(st.orig_buf, addr, len))
                          .set("new_bytes", hex_lower(hex_of_str(st.patch_hex[i])))
                          .set("len", static_cast<long long>(len));
                        patches.push(std::move(pj));
                    }
                    char addr_hex[32]; std::snprintf(addr_hex, sizeof(addr_hex), "0x%zx", off0);
                    h.set("patch_offset", std::string(addr_hex));
                    h.set("orig_bytes", hex_of(st.orig_buf, off0, len0));
                    h.set("new_bytes", hex_lower(hex_of_str(st.patch_hex[g.first])));
                    h.set("len", static_cast<long long>(len0));
                    // C1 §2.3 术语别名（qa V 组按 hook_addr/trampoline/bytes 断言，与上面字段同源）：
                    //   hook_addr = 该组首处补丁落位；trampoline = 作用/跳板地址（无则空串）；bytes = 首处字节数
                    h.set("hook_addr", std::string(addr_hex));
                    h.set("bytes", static_cast<long long>(len0));
                    h.set("trampoline", g.target_addr ? [&]{ std::ostringstream t; t << "0x" << std::hex << g.target_addr; return t.str(); }()
                                                      : std::string(""));
                    h.set("patches", std::move(patches));
                }
                if (g.target_addr) {
                    std::ostringstream t; t << "0x" << std::hex << g.target_addr;
                    h.set("target_addr", t.str());
                } else {
                    h.set("target_addr", nullptr);
                }
                arr.push(std::move(h));
            }
            return arr;
        }())
       .set("hook_groups_count", static_cast<long long>(st.groups.size()))
       .set("hook_groups_expected", static_cast<long long>(st.groups_expected))
       .set("hook_groups_expected_source", st.groups_expected_source)
       .set("hook_branch", st.branch_ge_6_1 ? ">=6.1" : "<6.1")
       .set("hook_has_huawei_symbols", st.has_huawei)
       .set("hook_groups_resolved", [&st]() -> long long {
            long long n = 0;
            for (auto& g : st.groups) if (g.last > g.first) ++n;
            return n;
       }())
       .set("hidden_dir_prefix", root_key.substr(0, 16))
       .set("errors", std::move(errors));
    return rep;
}

// 隐藏自检（--self-test-keyabi）：密钥 ABI 闭环验证（不需内核镜像，CI 每次必跑）。
// 背景（P0）：内核侧存的是 key[0:47] + NUL（48 字节）；用户态提权串必须是 **47 字符**。
// 若把 48 个可见字符写进内核（或写成 48 字节无 NUL），提权会**静默失败**（ENOENT，无日志）。
// 这里直接断言"引擎将要写进内核的那串字节"的样子，并校验 report.key_spec 的机器可断言字段。
static int self_test_keyabi_contract(const std::string& cli_key) {
    int failures = 0, total = 0;
    auto check = [&](const char* name, bool ok) {
        ++total; if (!ok) ++failures;
        std::cout << "KEYCASE " << name << (ok ? " PASS" : " FAIL") << "\n";
    };
    const std::string key = cli_key.empty() ? generate_random_str(kKeyInputChars) : cli_key;

    // ① 输入契约：^[A-Za-z0-9]{48}$
    check("input_is_48_alnum", valid_key(key));
    // ② 派生：前 47 字符 + NUL，共 48 字节，末字节为 0
    std::string embed;
    const bool derived = embed_key_bytes(key, embed);
    check("derivation_ok", derived);
    check("embed_is_48_bytes", embed.size() == kKeyStoredBytes);
    check("last_byte_nul", !embed.empty() && static_cast<unsigned char>(embed.back()) == 0);
    check("prefix_47_matches", embed.size() >= kKeyStoredChars &&
          embed.compare(0, kKeyStoredChars, key, 0, kKeyStoredChars) == 0);
    check("case_preserved", embed.size() >= kKeyStoredChars &&
          embed.compare(0, kKeyStoredChars, key, 0, kKeyStoredChars) == 0);
    // ③ 用户态提权串 = 47 字符（不含 NUL、不含被丢弃的第 48 字符）
    check("escalation_token_is_47", std::string(embed.c_str()).size() == kKeyStoredChars);
    // ④ 长度守卫：47/49/非 alnum/空 一律拒绝（CLI 层 E101，绝不静默截断或放行）
    check("reject_47_chars",   !valid_key(key.substr(0, kKeyStoredChars)));
    check("reject_49_chars",   !valid_key(key + "A"));
    check("reject_non_alnum",  !valid_key(std::string(kKeyInputChars, '-')));
    check("reject_empty",      !valid_key(""));

    // ⑤ report.key_spec：客户端/质测可机器断言的字段（Lead 要求的四个 + 既有字段）
    EngineOptions o; o.key_from_user = true;
    EngineState st;
    J rep = build_report(o, st, key);
    cJSON* ks = cJSON_GetObjectItem(rep.raw(), "key_spec");
    auto ks_str = [&](const char* k) -> std::string {
        if (!ks) return "";
        cJSON* it = cJSON_GetObjectItem(ks, k);
        return (it && cJSON_IsString(it)) ? std::string(it->valuestring) : std::string();
    };
    auto ks_num = [&](const char* k) -> long long {
        if (!ks) return -1;
        cJSON* it = cJSON_GetObjectItem(ks, k);
        return (it && cJSON_IsNumber(it)) ? static_cast<long long>(it->valuedouble) : -1;
    };
    auto ks_bool = [&](const char* k) -> bool {
        if (!ks) return false;
        cJSON* it = cJSON_GetObjectItem(ks, k);
        return it && cJSON_IsTrue(it);
    };
    check("spec_abi_47nul",        ks_str("abi") == "47+nul");
    check("spec_bytes_48",         ks_num("bytes") == 48);
    check("spec_last_byte_is_nul", ks_bool("last_byte_is_nul"));
    check("spec_input_len_48",     ks_num("input_len") == 48);
    check("spec_stored_len_47",    ks_num("stored_len") == 47);
    check("spec_length_48",        ks_num("length") == 48);

    // 人类/脚本可读的一行摘要（A10 用它断言、qa 可贴进报告）
    std::cout << "KEYSPEC abi=" << ks_str("abi")
              << " bytes=" << ks_num("bytes")
              << " last_byte_is_nul=" << (ks_bool("last_byte_is_nul") ? "true" : "false")
              << " input_len=" << ks_num("input_len")
              << " stored_len=" << ks_num("stored_len")
              << " embed_hex_len=" << (embed.size() * 2)
              << " embed_hex_tail=" << (embed.size() >= 2 ?
                     [&]{ char b[3]; std::snprintf(b, sizeof b, "%02x",
                         static_cast<unsigned char>(embed[embed.size()-1])); return std::string(b); }() : "-")
              << "\n";
    std::cout << "KEYRESULT " << (failures == 0 ? "PASS" : "FAIL")
              << " " << (total - failures) << "/" << total << " cases\n";
    return failures == 0 ? OK : 1;
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
        else if (a == "--key-abi") { opt.key_abi = need("--key-abi"); }
        else if (a == "--dump-symbols") { opt.dump_symbols = true; }
        else if (a == "--assume-kernel-version") { opt.assume_kernel_version = need("--assume-kernel-version"); }
        else if (a == "--assume-huawei-symbols") { opt.assume_huawei = true; }
        else if (a == "--self-test-groups") { opt.self_test_groups = true; }
        else if (a == "--self-test-workdir") { opt.self_test_workdir = true; }
        else if (a == "--self-test-keyabi") { opt.self_test_keyabi = true; }
        else if (a == "--gen-key") { have_key = true; }
        else if (a == "--json") { opt.json_sink = need("--json"); }
        else if (a == "--workdir") { opt.workdir = need("--workdir"); opt.workdir_from_user = true; }
        else if (a == "--quiet") { g_quiet = true; }
        else if (a == "--help" || a == "-h") {
            std::cout << "SPRoot patch engine v1.0.0 (contract SPROOT-DOC-C1 v1.0)\n"
                      << "usage: skroot-patch --in <kernel_image> --out <patched_kernel> (--gen-key|--key <48ch>)\n"
                      << "       [--json <ndjson_sink>] [--workdir <dir>] [--quiet]\n";
            return OK;
        }
        else { std::cerr << "unknown arg: " << a << "\n"; return E_USAGE; }
    }

    // 隐藏自检：期望组数分支逻辑覆盖（无需内核镜像、无需密钥；CI 用它保证 <6.1 分支正确）
    //   --self-test-groups [--assume-kernel-version 5.10.0] [--assume-huawei-symbols]
    if (opt.self_test_groups) {
        bool ge61 = true;
        if (!opt.assume_kernel_version.empty()) {
            ge61 = !KernelVersionParser::version_less(opt.assume_kernel_version, "6.1.0");
        }
        size_t exp = hook_groups_expected_count(ge61, opt.assume_huawei);
        std::cout << "branch=" << (ge61 ? ">=6.1" : "<6.1")
                  << " base=" << (ge61 ? 14 : 11)
                  << " huawei=" << (opt.assume_huawei ? "yes" : "no")
                  << " hook_groups_expected=" << exp << "\n";
        J extra;
        extra.set("branch_ge_6_1", ge61)
             .set("huawei", opt.assume_huawei)
             .set("hook_groups_expected", static_cast<long long>(exp));
        emit_result(OK, std::move(extra));
        if (g_ndjson.is_open()) g_ndjson.close();
        return OK;
    }

    // 隐藏自检：workdir 所有权判定矩阵（无需内核镜像/密钥；CI 用它做 A9d 行为级断言）
    //   --self-test-workdir
    if (opt.self_test_workdir) {
        int rc = self_test_workdir_contract();
        J extra;
        extra.set("workdir_contract", rc == OK ? "PASS" : "FAIL");
        emit_result(rc == OK ? OK : E_USAGE, std::move(extra));
        if (g_ndjson.is_open()) g_ndjson.close();
        return rc;
    }

    // 隐藏自检：密钥 ABI（47 字符 + NUL，48 字节）闭环，CI 每次必跑（A10）
    //   --self-test-keyabi [--key <48ch>]   （缺省随机生成一把 48 字符密钥）
    if (opt.self_test_keyabi) {
        int rc = self_test_keyabi_contract(opt.key_from_user ? opt.key : std::string());
        J extra;
        extra.set("key_abi", rc == OK ? "PASS" : "FAIL");
        emit_result(rc == OK ? OK : E_USAGE, std::move(extra));
        if (g_ndjson.is_open()) g_ndjson.close();
        return rc;
    }

    if (!have_in || !have_out || !have_key) {
        std::cerr << "E101: missing required args (--in/--out/--gen-key|--key)\n";
        J e; e.set("error", "E101: missing required args");
        emit_result(E_USAGE, std::move(e));
        return E_USAGE;
    }
    // 密钥 ABI 自检：拒绝任何非 47+NUL 的 ABI（防把 Skroot-Next 的 48 无 NUL 密钥套到本引擎产物上）
    {
        std::string normalized;
        for (char c : opt.key_abi) if (c != '+' && c != '-' && c != '_') normalized.push_back(
            static_cast<char>(std::tolower(static_cast<unsigned char>(c))));
        if (normalized != "47nul") {
            std::string msg = "E101: unsupported --key-abi '" + opt.key_abi +
                "'; SPRoot engine implements only 47+NUL (key[0..46] + NUL, 48th byte is 0). "
                "Skroot-Next uses 48-byte no-NUL ABI and is INCOMPATIBLE — do not mix keys or patchers.";
            std::cerr << msg << "\n";
            J e; e.set("error", msg);
            emit_result(E_USAGE, std::move(e));
            return E_USAGE;
        }
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
            std::string msg = "E102: input looks like boot image/compressed kernel (" + ext +
                "). Action: ./magiskboot unpack boot.img  # → kernel, then pass that raw kernel; "
                "gzip: gzip -dc kernel.gz > kernel; lz4: lz4 -d kernel.lz4 kernel";
            std::cerr << msg << "\n";
            J e; e.set("error", msg);
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
        // --dump-symbols：全量符号 dump（诊断用，对齐 Skroot-Next patch_kernel_root.cpp 的诊断能力）。
        // 只读取符号表、不落盘任何产物，便于"机型号不匹配/关键符号缺失"时快速定位。
        if (opt.dump_symbols) {
            auto all = symbol_analyze.get_all_symbols();
            std::vector<std::pair<std::string, uint64_t>> rows(all.begin(), all.end());
            std::sort(rows.begin(), rows.end(), [](const auto& a, const auto& b) {
                return a.second < b.second;
            });
            std::cout << "# SPRoot symbol dump (" << rows.size() << " symbols, base=0x"
                      << std::hex << sym._text << std::dec << ")\n";
            for (auto& r : rows) {
                char buf[128];
                std::snprintf(buf, sizeof(buf), "0x%08llx %s\n",
                              static_cast<unsigned long long>(r.second), r.first.c_str());
                std::cout << buf;
            }
            J extra; extra.set("symbols", static_cast<long long>(rows.size()))
                          .set("dump", "stdout");
            emit_result(OK, std::move(extra));
            if (g_ndjson.is_open()) g_ndjson.close();
            return OK;
        }

        {   // 期望组数（唯一来源）：分支判据与 patch_kernel_handler 一致（KV 版本比较）
            st.branch_ge_6_1 = !kv.is_kernel_version_less("6.1.0");
            st.has_huawei = hook_has_huawei_symbols(sym);
            if (!opt.assume_kernel_version.empty()) {
                st.branch_ge_6_1 = !KernelVersionParser::version_less(opt.assume_kernel_version, "6.1.0");
                st.groups_expected_source = "assume";
            }
            if (opt.assume_huawei) { st.has_huawei = true; st.groups_expected_source = "assume"; }
            st.groups_expected = hook_groups_expected_count(st.branch_ge_6_1, st.has_huawei);
        }
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
        st.orig_buf = st.file_buf;
        cfi_bypass(st.file_buf, sym, vec_patch);
        st.cfi_patch_count = vec_patch.size() - before;
        st.groups.push_back({ "cfi_bypass", "__cfi_check(family)", sym.__cfi_check.offset, before, vec_patch.size() });
        before = vec_patch.size();
        huawei_bypass(st.file_buf, sym, vec_patch);
        st.huawei_patch_count = vec_patch.size() - before;
        // huawei 符号不存在时**不产出该组**（caps 语义：该内核无此保护路径，属"不适用"而非"未解析"；
        // 若符号存在但未产出字节，则仍会 push 一组 resolved:false，便于发现异常）
        if (st.has_huawei) {
            st.groups.push_back({ "huawei_bypass", "hkip_check_(uid|gid|xid)_root", sym.hkip_check_uid_root, before, vec_patch.size() });
        }
    }

    // 7) 五组 hook
    emit_progress("patch_hooks", 50, "...");
    {
        st.orig_buf = st.file_buf;   // 备份：hooks[].orig_bytes 用原始字节
        pr = patch_kernel_handler(st.file_buf, off, sym, vec_patch, &st.groups);
        if (!pr.patched)
            return fail(E_NO_HOOK_SPACE, "E220: failed to find hook start addr / not enough kernel space", "-");
        J hooks; hooks.set("name", "hooks_embedded").set("blob_count", static_cast<int>(vec_patch.size()))
                      .set("root_key_region", "0x" + [&]{ std::ostringstream o; o << std::hex << pr.root_key_start; return o.str(); }());
        st.hook_table.push(std::move(hooks));
    }

    // 8) 密钥内嵌（47 字符 + NUL，写入 handler 选定的 root_key_start，与上游逐字节等价）
    //    详见 embed_key_bytes()：48 字符 CLI 密钥只写前 47 字符 + NUL（48 字节），
    //    第 48 字符丢弃；用户态提权串必须用同一 47 字符（CALL v1.1 E1）。
    size_t key_blob_first = vec_patch.size();
    emit_progress("embed_key", 80, "input=48 stored=47+NUL(48B)");
    {
        std::string embed;
        if (!embed_key_bytes(root_key, embed))
            return fail(E_INTERNAL, "E250: key ABI derivation failed",
                        "root_key shorter than 47 chars (valid_key should have rejected it)");
        patch_data(st.file_buf, pr.root_key_start, (void*)embed.data(), embed.size(), vec_patch);
        st.groups.push_back({ "embed_key", "root_key(47+NUL)", pr.root_key_start, key_blob_first, vec_patch.size() });
    }

    // 9) 应用补丁并写输出（临时文件 + rename）
    emit_progress("write_output", 95, "...");
    {
        st.patch_addrs.clear(); st.patch_lens.clear(); st.patch_hex.clear();
        for (auto& item : vec_patch) {
            st.patch_addrs.push_back(item.write_addr);
            st.patch_lens.push_back(item.str_bytes.size() / 2);
            st.patch_hex.push_back(item.str_bytes);
        }
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
        J extra;
        extra.set("root_key", root_key).set("report_ref", opt.report_path).set("hook_count", static_cast<int>(vec_patch.size()));
        emit_result(OK, std::move(extra));
        // ROOT_KEY 兜底行必须是 stdout 的**最后一行**（C1 §2.2：客户端会签口径）。
        // 旧顺序把它打印在 result 事件之前，末行实际是 NDJSON result —— 与失败分支
        // （emit_result → ROOT_KEY）也不一致；真机事故复盘时顺带校正。
        std::cout << "ROOT_KEY=" << root_key << std::endl;

        // 成功清理 workdir —— **只有引擎自建的默认 workdir 才回收**（判定见 should_reclaim_workdir）。
        // 契约（NEW-DOC-02 §8.1）：显式传入 --workdir ⇒ 目录及其内容归调用方，引擎不得删除其中
        // 任何内容；只有默认 workdir（<out>.work）由引擎回收。
        // 判定放在纯函数里、并被 --self-test-workdir 覆盖，避免再次出现"注释写了契约、
        // 代码却无条件删除"的静默回归（真机事故 2026-11-09）。
        const bool reclaim_workdir = should_reclaim_workdir(
            opt.workdir_from_user, opt.workdir, opt.out_path, opt.report_path, opt.json_sink);
        if (reclaim_workdir) {
            std::error_code ec; fs::remove_all(opt.workdir, ec);
        } else {
            // 诊断走 stderr：C1 §2.2 约定 stdout 末行必须是 ROOT_KEY=<48字符>（客户端会签兜底口径），
            // 这条提示若写 stdout 会把末行挤掉。
            if (!g_quiet) std::cerr << "[engine] workdir preserved (owned by caller): "
                                    << opt.workdir << "\n";
        }
    }

    if (g_ndjson.is_open()) g_ndjson.close();
    return final_code;
}

// CLI 入口：独立 ELF 形态（libskrootpatch.so 实为静态可执行文件）的唯一 main。
// JNI 形态不链接本函数（构建时定义 SPROOT_NO_CLI_MAIN，由 libskpatchjni.so 调 spr_engine_run）。
#ifndef SPROOT_NO_CLI_MAIN
int main(int argc, char* argv[]) { return spr_engine_run(argc, argv); }
#endif
