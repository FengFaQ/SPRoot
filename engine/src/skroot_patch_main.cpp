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
//   6. report.json 是密钥权威源：顶层 `root_key` 为 48 字符明文（C1 §2.2），
//      与 `key_spec` 同源（同一份 root_key 派生，绝无第二份密钥）；
//      stdout 末行 `ROOT_KEY=<48字符>` 仅为兜底（客户端会签口径）。
//      ★ V-18：report.json 含明文密钥，禁止外发/归档（NEW-DOC-06 §V-18）。

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
    bool self_test_rcvirtual = false;    // --self-test-rcvirtual（rc 虚拟化纯逻辑/布局/洞扫描）
    // 可选启动器：off（默认）| rc（rc 虚拟化，见 NEW-DOC-02 §11）| umh（B 方案，未实现）
    std::string launcher_mode = "off";
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

// ---------------------------------------------------------------- report 字段取值（纯函数）
// 从 build_report() 产出的报告里读出**顶层 root_key**，按下游真实消费方的口径判可用性：
//   quality/static/verify_patch.py:163  root_key = report.get("root_key")，随后
//     :200 if root_key and root_key != key         → V3「防刷别人的密钥」
//     :209 else                                    → V6 SKIP（空断言！）
//     :711 key or report.get("root_key")           → 不传 --key 时的密钥来源
// 与 JSON 里取值的区别：**键存在但为 null / 空串**时 cJSON 取到空串，jsonschema 语义上
// 下游 Python 会得到 None 或 ""，两者都不满足 `if root_key` ⇒ 等同于"没有字段"。
// 因此这里把「缺失 / null / 空串」统一判为 false，避免自检只认"键存在"而放过空值。
// 纯函数（无 IO）：主流程 build_report 与 --self-test-keyabi 共用同一份判定。
static bool report_root_key_usable(const cJSON* rep, std::string* out = nullptr) {
    if (out) out->clear();
    if (!rep) return false;
    const cJSON* it = cJSON_GetObjectItem(rep, "root_key");
    if (!it || !cJSON_IsString(it) || !it->valuestring) return false;
    const std::string v = it->valuestring;
    if (v.empty()) return false;                       // 空串 ⇒ 下游 `if root_key` 为假
    if (out) *out = v;
    return true;
}

// ---------------------------------------------------------------- launcher.reason（纯函数）
// patch_rc_virtualize.h 声明「reason：未织入原因（enabled=false 时必填）」，但默认构造
// （mode="off"）的 reason 是**空串** ⇒ 默认路径下 report.json 会是
//   "launcher": {"mode":"off","enabled":false,"reason":""}
// 这正是 task-8 要根除的形态：enabled=false 却给不出理由 = 静默跳过。真机上「rc 没织入」
// 既有「默认关」也有「前置不满足」，两者若在 report 里长得一样，排障只能靠猜。
// 本函数把「关闭/未织入」永远变成一句可读原因；enabled=true 时保持原样（不需要理由）。
// 纯函数（无 IO/全局状态）：build_report 与 --self-test-rcvirtual 共用同一份判定。
static std::string launcher_reason_effective(const std::string& mode, bool enabled,
                                             const std::string& reason) {
    if (enabled) return reason;                        // 已织入：reason 语义上不适用，原样保留
    if (!reason.empty()) return reason;                // 已有明确原因（precheck/weave 填的）：绝不覆盖
    if (mode == "off" || mode.empty()) return "disabled_by_default (--launcher off)";
    return "not_embedded (launcher mode=" + mode + " produced no blob)";
}

// ---------------------------------------------------------------- 改动地址分类（纯函数）
// 背景（XV-2 暴露的真缺陷）：report 曾只给"每组首处落位" = vec_patch[g.first]，
// 而多数组的 vec_patch 顺序是 [shellcode@cave, jump@真实函数入口, ...]：
//   filldir64/compat_filldir/avc_denied/audit_log_start 的 patch_jump(入口,...) 排在
//   shellcode 之后 ⇒ g.first 指向 **cave**，真实函数入口被埋在 patches[1..]。
// 结果：审计者看到 14 组，但实测 diff 出 17 处函数入口改写，其中 3 处**无任何记录**。
// 修复思路：把每个改动地址按"改了什么"显式分类，并保证**全集**可枚举、可对账。
//   只做分类不做裁剪——任何一处 vec_patch 都必须落进某一类，绝不静默丢弃。
enum class PatchRole {
    CaveShellcode,   // 织入的 shellcode/数据块（写的是空区/cave，不属函数入口）
    EntryHook,       // 把真实函数入口首指令改成 b（审计最关心的一类）
    RetPatch,        // patch_ret_*：把函数首指令改成 ret（bypass 类）
    DataPatch,       // 纯数据写入（如 47+NUL 密钥、跳板数据区）
};

// 判断某处写入是不是"函数入口被改成跳转"：长度为 4 且新字节是一条 b/b.<cond> 指令。
// AArch64 分支指令编码：bits[31:26]=0b000101（b）或 0b010101（b.cond）。
static bool insn_is_branch(uint32_t insn) {
    const uint32_t op = insn >> 26;
    return op == 0x05u || op == 0x15u;   // 0b000101 / 0b010101
}

// 把 vec_patch 的大写 hex 串解成 32 位指令。
// 注意字节序：hex 串是**按内存顺序**写的（bytes2hex 逐字节输出），
// 所以前两个 hex 字符 = 最低地址字节 = 小端最低字节。不能用"整体当大端"读。
static uint32_t hex_first_insn_le(const std::string& hex) {
    if (hex.size() < 8) return 0;
    auto nib = [](char c) -> uint32_t {
        if (c >= '0' && c <= '9') return (uint32_t)(c - '0');
        if (c >= 'A' && c <= 'F') return (uint32_t)(c - 'A' + 10);
        if (c >= 'a' && c <= 'f') return (uint32_t)(c - 'a' + 10);
        return 0;
    };
    uint32_t v = 0;
    for (int byte_i = 0; byte_i < 4; ++byte_i) {
        // 第 byte_i 个字节（内存序）由第 2*byte_i、2*byte_i+1 个 hex 字符组成
        const uint32_t b = (nib(hex[2 * byte_i]) << 4) | nib(hex[2 * byte_i + 1]);
        v |= b << (8 * byte_i);                  // 小端拼装
    }
    return v;
}

// 由写入字节与长度判定角色（**纯函数，只看这一处写入自身**）。
// 刻意不依赖"从别处传进来的 cave 集合"：那种做法要靠调用方正确填充，
// 一旦漏填就会把 shellcode 误判成入口 hook，反而制造新的假证据。
// 判据全部来自这一处写入本身，因此对任何调用方都成立：
//   ① 长度 != 4      → 一定是 shellcode/数据块（aarch64 单条指令恒 4 字节）
//   ② 4 字节且是 b   → 入口被改成跳转（EntryHook）——这是审计最关心的一类
//   ③ 4 字节且是 ret → bypass 类补丁（RetPatch）
//   ④ 其余 4 字节    → 视为数据写入
static PatchRole classify_patch(size_t /*addr*/, size_t len, const std::string& new_hex) {
    if (len != 4) return PatchRole::CaveShellcode;   // 多字节块 = 织入的 shellcode/数据区
    const uint32_t v = hex_first_insn_le(new_hex);
    if (insn_is_branch(v)) return PatchRole::EntryHook;
    if (v == 0xD65F03C0u /* ret */) return PatchRole::RetPatch;
    return PatchRole::DataPatch;
}

static const char* patch_role_name(PatchRole r) {
    switch (r) {
        case PatchRole::CaveShellcode: return "cave_shellcode";
        case PatchRole::EntryHook:     return "entry_hook";
        case PatchRole::RetPatch:      return "ret_patch";
        case PatchRole::DataPatch:     return "data_patch";
    }
    return "unknown";
}

// ---------------------------------------------------------------- 期望组数 breakdown（纯函数）
// task-16：把「期望组数」从"一个数字"变成"可自解释的构成 + 具名证据"。
// 序列化集中在此一处，build_report 与 --self-test-groups 共用，避免两边字段集漂移。
// 字段语义与「如何用它把魅族21 的 15 组与基线 14 对账」见 patch_pipeline.h 里
// HookGroupsExpectedBreakdown 的注释（三步 SOP：先看 branch_base，再看
// huawei_symbols_detected 是否为空，最后看 launcher_rc）。
static J breakdown_to_json(const HookGroupsExpectedBreakdown& b) {
    J o;
    o.set("kernel_ge_6_1", b.kernel_ge_6_1)
     .set("branch_base", static_cast<long long>(b.branch_base))
     .set("huawei_bypass", static_cast<long long>(b.huawei_bypass))
     .set("huawei_symbols_detected", [&b]() -> J {
         J arr = J::array();
         for (const auto& n : b.huawei_symbols_detected) arr.push(n);
         return arr;   // 空数组 = 一个都没命中（不是"未知"）
     }())
     .set("huawei_bypass_source", b.huawei_bypass_source)
     .set("launcher_rc", static_cast<long long>(b.launcher_rc))
     .set("total", static_cast<long long>(b.total()))
     // 自洽标记：分项之和 == total，且 huawei_bypass 与其依据一致。false = 引擎自身算错。
     .set("consistent", b.consistent());
    return o;
}

// 从 report 的 breakdown 里**独立重算**一遍，供对账断言使用：
//   total == branch_base + huawei_bypass + launcher_rc
//   total == report.hook_groups_expected（两处数字必须一致）
//   huawei_bypass == (huawei_symbols_detected 非空 ? 1 : 0)，除非 source=="assumed"
// 与 breakdown_to_json 不同：这里从 cJSON 读回来再算，**故意不复用构建期对象**，
// 否则"写错"和"读错"会一起错、断言永远绿。
static bool breakdown_json_selfconsistent(const cJSON* rep, std::string* why = nullptr) {
    auto fail = [&](const char* m) { if (why) *why = m; return false; };
    if (!rep) return fail("report null");
    const cJSON* bd = cJSON_GetObjectItem(rep, "hook_groups_expected_breakdown");
    if (!bd) return fail("breakdown 字段缺失");
    auto num = [&](const char* k, long long dflt) -> long long {
        const cJSON* it = cJSON_GetObjectItem(bd, k);
        return (it && cJSON_IsNumber(it)) ? static_cast<long long>(it->valuedouble) : dflt;
    };
    auto str = [&](const char* k) -> std::string {
        const cJSON* it = cJSON_GetObjectItem(bd, k);
        return (it && cJSON_IsString(it) && it->valuestring) ? std::string(it->valuestring) : std::string();
    };
    const bool   ge61   = [&]{ const cJSON* it = cJSON_GetObjectItem(bd, "kernel_ge_6_1");
                               return it && cJSON_IsTrue(it); }();
    const long long base = num("branch_base", -1);
    const long long hw   = num("huawei_bypass", -1);
    const long long lrc  = num("launcher_rc", -1);
    const long long tot  = num("total", -1);
    if (base != (ge61 ? 14 : 11)) return fail("branch_base 与 kernel_ge_6_1 不符");
    if (hw < 0 || hw > 1)         return fail("huawei_bypass 应为 0/1");
    if (lrc < 0 || lrc > 1)       return fail("launcher_rc 应为 0/1");
    if (tot != base + hw + lrc)   return fail("total != branch_base + huawei_bypass + launcher_rc");

    const cJSON* arr = cJSON_GetObjectItem(bd, "huawei_symbols_detected");
    if (!arr || !cJSON_IsArray(arr)) return fail("huawei_symbols_detected 不是数组");
    const long long nhit = static_cast<long long>(cJSON_GetArraySize(arr));
    const std::string src = str("huawei_bypass_source");
    if (src == "detected") {
        if (nhit < 1) return fail("source=detected 却没有任何符号名");
        if (hw != 1)  return fail("huawei 符号命中却 huawei_bypass=0");
    } else if (src == "assumed") {
        if (hw != 1)  return fail("source=assumed 却 huawei_bypass=0");
    } else if (src == "none") {
        if (nhit != 0) return fail("source=none 却列出了符号名");
        if (hw != 0)   return fail("source=none 却 huawei_bypass=1");
    } else {
        return fail("huawei_bypass_source 取值非法");
    }
    // 跨字段一致性：breakdown.total 必须等于顶层 hook_groups_expected（单一来源）
    const cJSON* exp = cJSON_GetObjectItem(rep, "hook_groups_expected");
    const long long expv = (exp && cJSON_IsNumber(exp)) ? static_cast<long long>(exp->valuedouble) : -1;
    if (expv != tot) return fail("total != report.hook_groups_expected");
    const cJSON* cons = cJSON_GetObjectItem(bd, "consistent");
    if (!cons || !cJSON_IsTrue(cons)) return fail("breakdown.consistent 不为 true");
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
    // C1 §2.3：逐 hook 落位（非破坏性新增；旧 hook_table 聚合字段保留）
    std::vector<PatchGroupSpan> groups;
    size_t groups_expected = 0;                 // 期望组数（唯一来源 = breakdown.total()）
    // task-16：期望组数的**构成**（自解释用）。total() 就是上面的 groups_expected，
    // 两者不得各算一份（build_report 用同一对象序列化 + 写 expected，天然不会漂移）。
    HookGroupsExpectedBreakdown expected_breakdown;
    bool branch_ge_6_1 = true;
    bool has_huawei = false;
    std::string groups_expected_source = "kernel";  // kernel | assume
    std::vector<char> orig_buf;                 // 打补丁前的镜像（回填 orig_bytes）
    std::vector<size_t> patch_addrs;            // vec_patch[i].write_addr 镜像
    std::vector<size_t> patch_lens;             // 每处补丁字节数
    std::vector<std::string> patch_hex;          // 每处补丁 bytes（大写 hex）
    RcVirtInfo launcher;                         // rc 虚拟化结果（--launcher rc）
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
    // rc 虚拟化结果（--launcher rc；off 时 mode=off/enabled=false）。caves/symbols 供真机排障。
    // reason 经 launcher_reason_effective 归一：enabled=false 时**绝不写空串**（防"静默跳过"）。
    J launcher;
    launcher.set("mode", st.launcher.mode)
            .set("enabled", st.launcher.enabled)
            .set("reason", launcher_reason_effective(st.launcher.mode, st.launcher.enabled,
                                                     st.launcher.reason))
            .set("hook_addr", static_cast<long long>(st.launcher.hook_addr))
            .set("cave_offset", static_cast<long long>(st.launcher.cave_offset))
            .set("cave_size", static_cast<long long>(st.launcher.cave_size))
            .set("shellcode_bytes", static_cast<long long>(st.launcher.shellcode_bytes))
            .set("template_bytes", static_cast<long long>(st.launcher.template_bytes))
            // 运行期安全闸阈值（task-8）：init 打开 rc 时若读到 >= 该字节数，shellcode 判定
            // "可能被截断"→ 放弃重定向（截断 rc 会破坏 init 解析 → 开机失败）。
            // 该闸**在开机时才触发**，故 patch 期 report 无法给出 enabled:false/reason；
            // 这里把阈值常量写进 report，供真机排障时对照"rc 是否超过上限"这一最常见原因。
            .set("read_max_bytes", static_cast<long long>(RC_VIRT_READ_MAX_BYTES))
            .set("read_max_bytes_scope", "runtime");
    {
        J caves = J::array();
        for (const auto& c : st.launcher.caves) {
            J one;
            one.set("offset", static_cast<long long>(c.offset)).set("size", static_cast<long long>(c.size));
            caves.push(std::move(one));
        }
        launcher.set("caves", std::move(caves));
        J syms;
        for (const auto& p : st.launcher.symbols) syms.set(p.first, static_cast<long long>(p.second));
        launcher.set("symbols", std::move(syms));
    }
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
       // C1 §2.2 顶层 root_key = 密钥权威源（48 字符明文，与 key_spec 同源：同一份 root_key）。
       // ★ 修复（task-8）：此前本函数只写 key_spec 子对象，从不写顶层 root_key，导致
       //   quality/static/verify_patch.py:163 的 report.get("root_key") 恒为 None
       //   → 不传 --key 时 V2（密钥内嵌）/V3（root_key 与 --key 一致）/V6（前缀一致性）
       //   静默 SKIP = 空断言（密钥链等于没验）。
       // ★ 安全（V-18 / NEW-DOC-06 §V-18）：本字段含明文密钥 ⇒ report.json 禁止外发/归档。
       .set("root_key", root_key)
       .set("root_key_contains_secret", true)   // 机器可读的"禁止外发"标记
       .set("root_key_sensitivity", "contains_secret_do_not_distribute")
       .set("key_spec", std::move(key_spec))
       .set("launcher", std::move(launcher))
       .set("hook_table", std::move(hooks))
       .set("hooks", [&st]() -> J {
            // C1 §2.3 五组逐 hook 落位（非破坏性新增）：每组含 group/name/resolved/
            // patch_offset/orig_bytes/new_bytes/target_addr + patches[]（该组全部落位）
            J arr = J::array();
            for (const auto& g : st.groups) {
                J h;
                size_t n = (g.last > g.first) ? (g.last - g.first) : 0;
                h.set("group", g.group ? g.group : "").set("name", g.symbol ? g.symbol : "");
                // 失败路径保护：st.patch_addrs/lens/hex 只在「成功收尾前」才回填，
                // 而 fail() 也会构建 report。若不校验下标就会越界读（曾在本轮 launcher
                // 的 E202 失败路径上触发段错误）。缺失时不冒充"已落位"。
                const bool have_bytes = n > 0 && g.last <= st.patch_addrs.size()
                    && g.last <= st.patch_lens.size() && g.last <= st.patch_hex.size();
                if (!have_bytes) {
                    h.set("resolved", false)
                     .set("reason", n == 0 ? "no_patch_bytes_symbol_missing_or_not_used"
                                           : "patch_bytes_not_filled_yet_failure_path")
                     .set("hook_addr", std::string("")).set("trampoline", std::string(""))
                     .set("bytes", 0);
                } else {
                    h.set("resolved", true);
                    size_t off0 = 0, len0 = 0;
                    J patches = J::array();
                    J entry_hooks = J::array();   // 本组内"改写真实函数入口"的落位
                    for (size_t i = g.first; i < g.last; ++i) {
                        const size_t addr = st.patch_addrs[i];
                        const size_t len  = st.patch_lens[i];
                        const std::string hexu = hex_lower(hex_of_str(st.patch_hex[i]));
                        const PatchRole role = classify_patch(addr, len, st.patch_hex[i]);
                        if (off0 == 0) { off0 = addr; len0 = len; }
                        char ah[32]; std::snprintf(ah, sizeof(ah), "0x%zx", addr);
                        J pj;
                        pj.set("patch_offset", std::string(ah))
                          .set("orig_bytes", hex_of(st.orig_buf, addr, len))
                          .set("new_bytes", hexu)
                          .set("len", static_cast<long long>(len))
                          // 显式角色：让审计者不必猜这处是 shellcode、入口跳转还是 bypass
                          .set("role", patch_role_name(role))
                          // 兼容别名：XV-2 等外部工具按 offset/addr 收集地址，一并给出
                          .set("offset", std::string(ah))
                          .set("addr", std::string(ah));
                        if (role == PatchRole::EntryHook) {
                            J eh;
                            eh.set("patch_offset", std::string(ah))
                              .set("orig_bytes", hex_of(st.orig_buf, addr, len))
                              .set("new_bytes", hexu);
                            entry_hooks.push(std::move(eh));
                        }
                        patches.push(std::move(pj));
                    }
                    char addr_hex[32]; std::snprintf(addr_hex, sizeof(addr_hex), "0x%zx", off0);
                    h.set("patch_offset", std::string(addr_hex));
                    h.set("orig_bytes", hex_of(st.orig_buf, off0, len0));
                    h.set("new_bytes", hex_lower(hex_of_str(st.patch_hex[g.first])));
                    h.set("len", static_cast<long long>(len0));
                    // ---- 地址语义（本次修复的核心，务必读清）----------------------------
                    // 旧实现只有一个 patch_offset + 语义含混的 target_addr，导致审计者无法
                    // 回答"原内核里哪个函数入口被改了"。现在明确区分两个**成对**地址：
                    //   patched_site = 该组首处 vec_patch 的写入地址（多为 cave/shellcode 落位）
                    //   entry_hooks[] = 该组内所有"把真实函数入口首指令改成 b"的地址全集
                    // 二者可能相同（<6.1 分支 shellcode 就写在入口空区），也可能不同
                    // （>=6.1 分支 shellcode 在 cave、入口在别处）——正因如此必须都记。
                    h.set("hook_addr", std::string(addr_hex));   // 兼容既有 qa 断言：= patched_site
                    h.set("bytes", static_cast<long long>(len0));
                    h.set("patched_site", std::string(addr_hex));
                    // trampoline 语义澄清：= 本组 shellcode 入口（cave）地址；
                    // 与"被改写的函数入口"是两回事，后者见 entry_hooks[]。
                    h.set("trampoline", g.target_addr ? [&]{ std::ostringstream t; t << "0x" << std::hex << g.target_addr; return t.str(); }()
                                                      : std::string(""));
                    h.set("entry_hooks", std::move(entry_hooks));
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
       // hook_groups_expected 直接取 breakdown.total()：单一来源，避免"两处算数漂移"。
       // （st.groups_expected 也由同一 total() 赋值，见主流程；这里再取一次是为了
       //   让 build_report 在 fail() 路径上也不必依赖外部赋值顺序。）
       .set("hook_groups_expected", static_cast<long long>(st.expected_breakdown.total()))
       .set("hook_groups_expected_source", st.groups_expected_source)
       // ★ task-16：把「那个数字怎么来的」显式化，任何人拿 report 就能对账 14 vs 15，
       //   不必读引擎源码。字段含义与真机复核步骤见 patch_pipeline.h 的
       //   HookGroupsExpectedBreakdown 注释（含魅族21 15 组的三步判定 SOP）。
       .set("hook_groups_expected_breakdown", breakdown_to_json(st.expected_breakdown))
       .set("hook_branch", st.branch_ge_6_1 ? ">=6.1" : "<6.1")
       .set("hook_has_huawei_symbols", st.has_huawei)
       .set("hook_groups_resolved", [&st]() -> long long {
            long long n = 0;
            for (auto& g : st.groups) if (g.last > g.first) ++n;
            return n;
       }())
       // ---- 改动全集（本次修复的关键：report 必须能回答"镜像到底改了哪几处"）--------
       // all_patch_sites[] = vec_patch 里**每一处**写入的地址+角色，无遗漏、无去重丢失。
       //   外部审计（如 quality/crossverify/xv.py 的 XV-2）拿它做 diff 归因，
       //   就不会再出现"实测改了 17 处、report 只承认 14 处"的缺口。
       // attribution = 全集是否都被 hooks[] 覆盖：
       //   "complete"   每处都能在某个 hook 组的 patches[] 里找到（正常路径）
       //   "incomplete" 有写入没被任何组覆盖 ⇒ 明细见 unattributed_patches[]（绝不静默丢弃）
       .set("all_patch_sites", [&st]() -> J {
           J arr = J::array();
           for (size_t i = 0; i < st.patch_addrs.size(); ++i) {
               const size_t addr = st.patch_addrs[i];
               const size_t len  = i < st.patch_lens.size() ? st.patch_lens[i] : 0;
               const std::string hexu = i < st.patch_hex.size()
                   ? hex_lower(hex_of_str(st.patch_hex[i])) : std::string();
               char ah[32]; std::snprintf(ah, sizeof(ah), "0x%zx", addr);
               J o;
               o.set("patch_offset", std::string(ah))
                .set("offset", std::string(ah))            // xv.py 兼容别名
                .set("len", static_cast<long long>(len))
                .set("role", patch_role_name(classify_patch(addr, len,
                          i < st.patch_hex.size() ? st.patch_hex[i] : std::string())))
                .set("orig_bytes", hex_of(st.orig_buf, addr, len))
                .set("new_bytes", hexu);
               arr.push(std::move(o));
           }
           return arr;
       }())
       .set("patch_sites_count", static_cast<long long>(st.patch_addrs.size()))
       .set("entry_hook_count", [&st]() -> long long {
           long long n = 0;
           for (size_t i = 0; i < st.patch_addrs.size(); ++i) {
               const size_t len = i < st.patch_lens.size() ? st.patch_lens[i] : 0;
               if (classify_patch(st.patch_addrs[i], len,
                                  i < st.patch_hex.size() ? st.patch_hex[i] : std::string())
                   == PatchRole::EntryHook) ++n;
           }
           return n;
       }())
       // 期望值：每组至少贡献一处写入；与实际 sites 数对照可发现"织入但未记账"。
       .set("unattributed_patches", [&st]() -> J {
           // 先把 hooks[] 覆盖到的下标标出来（各组的 [first,last) 区间并集）
           std::vector<bool> covered(st.patch_addrs.size(), false);
           for (const auto& g : st.groups)
               for (size_t i = g.first; i < g.last && i < covered.size(); ++i) covered[i] = true;
           J arr = J::array();
           for (size_t i = 0; i < covered.size(); ++i) {
               if (covered[i]) continue;                     // 已被某组记账
               const size_t addr = st.patch_addrs[i];
               const size_t len  = i < st.patch_lens.size() ? st.patch_lens[i] : 0;
               char ah[32]; std::snprintf(ah, sizeof(ah), "0x%zx", addr);
               J o;
               o.set("patch_offset", std::string(ah))
                .set("offset", std::string(ah))
                .set("len", static_cast<long long>(len))
                .set("role", patch_role_name(classify_patch(addr, len,
                          i < st.patch_hex.size() ? st.patch_hex[i] : std::string())))
                .set("reason", "写入落在所有 hook 组的 [first,last) 区间之外："
                               "hooks[] 未记账，故在此显式列出（不静默丢弃）");
               arr.push(std::move(o));
           }
           return arr;
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

    // ⑥ report 顶层 root_key（task-8 修复：此字段此前**从未写过**，导致下游
    //    quality/static/verify_patch.py:163 恒取到 None → 不传 --key 时 V2/V3/V6 静默
    //    SKIP（空断言：密钥链等于没验）。铁律：每条新断言都必须配**负样本证明会 FAIL**，
    //    否则又是 AVB-3 那种"永远 PASS 的空断言"。故此处同时断言正/负两侧：
    //      正样本 ⇒ 自检必须判定"可用"；负样本 ⇒ 自检必须判定"不可用"。
    //    否则本用例本身退化成恒 PASS，下面每个 case 单列一行便于 CI 逐条 grep。
    {
        std::string got;
        const bool positive_ok = report_root_key_usable(rep.raw(), &got);
        check("report_root_key_present",     positive_ok);
        check("report_root_key_equals_key",  positive_ok && got == key);
        check("report_root_key_48_chars",    positive_ok && got.size() == kKeyInputChars);
        // 同源校验：顶层 root_key 与 key_spec 必须由**同一份** root_key 派生（长度不得漂移）
        check("report_root_key_matches_key_spec_len",
              positive_ok && ks_num("input_len") == static_cast<long long>(got.size()));
        // V-18：明文密钥字段必须自带"禁止外发"标记（NEW-DOC-06 §V-18 要求标注）
        check("report_root_key_marked_secret", [&]{
            cJSON* m = cJSON_GetObjectItem(rep.raw(), "root_key_contains_secret");
            return m && cJSON_IsTrue(m);
        }());

        // ---- 负样本：复制正样本后逐项破坏，判定函数必须返回 false ----
        // 否则 report_root_key_present 就是"永远 PASS"的空断言（真实回归正是"字段没写"）。
        // 每条单列一个 KEYCASE，便于 CI 逐条 grep，也便于人类看到它确实会变红。
        auto mutated_unusable = [&](const char* cname, cJSON* replacement) {
            cJSON* m = cJSON_Duplicate(rep.raw(), 1);
            cJSON_ReplaceItemInObject(m, "root_key", replacement);   // 接管所有权
            const bool unusable = !report_root_key_usable(m);
            cJSON_Delete(m);
            check(cname, unusable);
        };
        {   // ① 删掉字段：正是修复前的真实状态（下游 report.get 恒 None）⇒ 必须判不可用
            cJSON* m = cJSON_Duplicate(rep.raw(), 1);
            cJSON_DeleteItemFromObject(m, "root_key");
            const bool unusable = !report_root_key_usable(m);
            cJSON_Delete(m);
            check("neg_missing_root_key_unusable", unusable);
        }
        // ② 字段为 null（JSON 语义上存在；下游 Python 取到 None）⇒ 必须判不可用
        mutated_unusable("neg_null_root_key_unusable", cJSON_CreateNull());
        // ③ 字段为空串（下游 `if root_key` 为假 ⇒ V2/V6 仍会 SKIP）⇒ 必须判不可用
        mutated_unusable("neg_empty_root_key_unusable", cJSON_CreateString(""));
        // ④ 字段存在但不是密钥本身：判定须"可用"，且与 CLI 密钥**不等**——
        //    这是下游 V3「防刷别人的密钥」能变红的前提（若引擎把 key 抄错，此处必须能发现）。
        {
            cJSON* m = cJSON_Duplicate(rep.raw(), 1);
            cJSON_ReplaceItemInObject(m, "root_key", cJSON_CreateString(std::string(48, 'Z').c_str()));
            std::string other;
            const bool usable = report_root_key_usable(m, &other);
            cJSON_Delete(m);
            check("neg_wrong_root_key_detected", usable && other != key);
        }
        // ⑤ 非字符串类型（数字）：下游取到 int，不是密钥串 ⇒ 必须判不可用
        mutated_unusable("neg_nonstring_root_key_unusable", cJSON_CreateNumber(12345));
    }

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

// 隐藏自检（--self-test-rcvirtual）：rc 虚拟化的**纯逻辑层**（模板/布局/符号表/洞扫描/组数矩阵）。
// 不需要内核镜像 ⇒ CI 每次必跑（A11）。真内核相关部分（符号解析/实际织入）由 Tier B B11 覆盖。
static int self_test_rcvirtual_contract() {
    int failures = 0, total = 0;
    auto check = [&](const char* name, bool ok, const std::string& detail = "") {
        ++total; if (!ok) ++failures;
        std::cout << "RCVCASE " << name << (ok ? " PASS" : " FAIL");
        if (!ok && !detail.empty()) std::cout << " (" << detail << ")";
        std::cout << "\n";
    };
    const std::string tmpl = rc_virt_template();
    auto has = [&](const char* needle) { return tmpl.find(needle) != std::string::npos; };

    // ① 模板：自启链语义（与 apd/assets/init.sproot.rc 等价的关键行）
    check("template_post_fs_data", has("on post-fs-data"));
    check("template_service_line",
          has("service sproot-apd /data/adb/apd --superkey-file /data/adb/sproot/key daemon"));
    // ② clap 顶层参数序：--superkey-file 必须在子命令 daemon 之前（§6.8 教训 #2）
    {
        const size_t p_flag = tmpl.find("--superkey-file");
        const size_t p_cmd = tmpl.rfind(" daemon");
        check("template_top_level_arg_order", p_flag != std::string::npos &&
              p_cmd != std::string::npos && p_flag < p_cmd,
              "superkey-file must precede subcommand daemon");
    }
    // ③ 防 bootloop：oneshot + disabled（退出后 init 不 respawn）
    // ⑤b Zygisk 启动链：post-fs-data 阶段必须阻塞执行 apd post-fs-data
    //     （缺此行 → zygiskd 晚于 zygote → NeoZygisk 注入必然失败）
    check("template_post_fs_data_exec", has("exec -- /data/adb/apd post-fs-data"),
          "missing: exec -- /data/adb/apd post-fs-data (Zygisk injection needs zygiskd before zygote)");
    // 时机正确性：exec 必须出现在 "on post-fs-data" 段内，不得落在 boot_completed 段
    {
        const size_t p_trigger = tmpl.find("on post-fs-data");
        const size_t p_exec = tmpl.find("exec -- /data/adb/apd post-fs-data");
        const size_t p_boot = tmpl.find("on property:sys.boot_completed=1");
        check("template_exec_stage_order",
              p_trigger != std::string::npos && p_exec != std::string::npos &&
                  p_boot != std::string::npos && p_trigger < p_exec && p_exec < p_boot,
              "exec must sit inside the post-fs-data block, before boot_completed");
    }
    check("template_oneshot", has("oneshot"));
    check("template_disabled", has("disabled"));
    // ④ 触发点：boot_completed 之后 start
    check("template_boot_completed", has("on property:sys.boot_completed=1") && has("start sproot-apd"));
    // ⑤ 模板里不得出现密钥原文 / 不得异常膨胀
    check("template_no_key_material", tmpl.find("ROOT_KEY") == std::string::npos && tmpl.size() < 2048);

    // ⑥ 数据区布局
    const RcVirtDataLayout l = rc_virt_data_layout();
    check("layout_ok", l.ok, l.reason);
    check("layout_monotonic", l.off_orig[0] < l.off_orig[1] && l.off_orig[1] < l.off_replace &&
          l.off_replace < l.off_template && l.off_template < l.data_len);
    check("layout_aligned", (l.off_orig[0] % 4) == 0 && (l.off_orig[1] % 4) == 0 &&
          (l.off_replace % 4) == 0 && (l.off_template % 4) == 0 && (l.data_len % 4) == 0);
    // ⑦ 原地覆写安全性：替换串必须严格短于每个原路径（否则会越界写用户内存）
    {
        bool safe = true;
        for (const char* p : RC_VIRT_ORIG_PATHS)
            if (strlen(RC_VIRT_REPLACE_PATH) >= strlen(p)) safe = false;
        check("layout_inplace_overwrite_safe", safe);
    }

    // ⑧ 符号清单（防被删空 / 防漂移）
    const std::vector<const char*> req = rc_virt_required_symbols();
    const std::vector<std::string> expect = {
        "__arm64_sys_openat", "strncpy_from_user", "strcmp", "filp_open", "kernel_read",
        "kernel_write", "filp_close", "vmalloc", "vfree", "__arch_copy_to_user"
    };
    check("symbols_count", req.size() >= 10, "count=" + std::to_string(req.size()));
    {
        std::vector<std::string> got;
        for (auto n : req) got.push_back(n ? n : "");
        std::sort(got.begin(), got.end());
        auto uniq = got; uniq.erase(std::unique(uniq.begin(), uniq.end()), uniq.end());
        check("symbols_unique", uniq.size() == got.size());
        std::vector<std::string> exp_sorted = expect;
        std::sort(exp_sorted.begin(), exp_sorted.end());
        check("symbols_match_expected", got == exp_sorted);
    }

    // ⑨ 洞扫描（纯函数；合成缓冲，覆盖边界/分裂/对齐/上限）
    {
        std::vector<char> z(4096, 0);
        auto c1 = rc_virt_scan_caves(z, 0, z.size(), 1024);
        check("cave_scan_all_zero", c1.size() == 1 && c1[0].offset == 0 && c1[0].size == 4096);

        std::vector<char> z2(4096, 0);
        z2[2048] = 1;
        auto c2 = rc_virt_scan_caves(z2, 0, z2.size(), 1024);
        check("cave_scan_split_by_nonzero", c2.size() == 2 && c2[0].offset == 0 && c2[0].size == 2048 &&
              (c2[1].offset % 4) == 0 && c2[1].size >= 1024);
        // 分裂点落在 "洞内填 1 字节非零" → 必须不再是一整块（负样本语义）
        check("cave_scan_nonzero_breaks", !(c2.size() == 1 && c2[0].size == 4096));

        std::vector<char> z3(4096, 0);
        z3[0] = 0x7f;
        auto c3 = rc_virt_scan_caves(z3, 3, z3.size(), 1024);   // lo 非对齐 → 起点对齐到 4
        check("cave_scan_align_start", c3.size() == 1 && (c3[0].offset % 4) == 0 && c3[0].offset == 4);

        std::vector<char> z4(4096, 0);
        for (size_t i = 0; i < z4.size(); i += 1024) z4[i] = 0x11;   // 10 个 1023B 的洞
        auto c4 = rc_virt_scan_caves(z4, 0, z4.size(), 512, 3);
        check("cave_scan_max_out", c4.size() == 3, "got=" + std::to_string(c4.size()));

        std::vector<char> z5(512, 0);
        auto c5 = rc_virt_scan_caves(z5, 0, z5.size(), 1024);
        check("cave_scan_min_filter", c5.empty());

        auto c6 = rc_virt_scan_caves(z, 0, z.size() + 16, 1024);
        check("cave_scan_bounds_guard", c6.empty());
    }

    // ⑨b 读取上限（防截断）：必须 >= 256 KB（init.rc 实测 60–100 KB；截断=开机事故）
    check("read_limit_sane", RC_VIRT_READ_MAX_BYTES >= 256 * 1024, "RC_VIRT_READ_MAX_BYTES=" + std::to_string(RC_VIRT_READ_MAX_BYTES));
    check("min_cave_sane", RC_VIRT_MIN_CAVE_BYTES >= 2048 && RC_VIRT_MIN_CAVE_BYTES <= 65536,
          "RC_VIRT_MIN_CAVE_BYTES=" + std::to_string(RC_VIRT_MIN_CAVE_BYTES));

    // ⑨c enabled=false ⇒ reason 必须非空（防"静默跳过"；task-8 第二项）。
    //    512KB 读取上限是**运行期**安全闸（init 打开 rc 时才知道真实大小，patch 期无从得知），
    //    它不可能出现在 patch 期 report 里；但它属于同一类"降级要用显式原因说话"的语义，
    //    故此处把 patch 期可判定的部分锁死：任何 enabled=false 的组合都不得留下空 reason。
    check("reason_off_nonempty",
          !launcher_reason_effective("off", false, "").empty());
    check("reason_off_explicit",
          launcher_reason_effective("off", false, "").find("disabled_by_default") != std::string::npos);
    check("reason_rc_not_embedded_nonempty",
          !launcher_reason_effective("rc", false, "").empty());
    check("reason_preserves_precheck_text",   // precheck/weave 已填的原因必须原样保留
          launcher_reason_effective("rc", false, "symbol missing: strcmp") == "symbol missing: strcmp");
    // ★ 负样本：若把"空 reason"当合法（即 launcher_reason_effective 直接返回空串），
    //   上面三条正向断言必须能把它照出来 —— 这里显式断言"空串不是合法结果"，
    //   否则本组断言在实现退化成 return reason 时仍会 PASS（空断言）。
    check("neg_empty_reason_rejected",
          launcher_reason_effective("off", false, "") != "" &&
          launcher_reason_effective("rc", false, "") != "");
    // 已织入（enabled=true）时不得凭空编造理由（保持空串语义，由 shellcode_bytes 等字段自证）
    check("reason_enabled_untouched",
          launcher_reason_effective("rc", true, "").empty());

    // ⑩ 组数矩阵（默认 off 不变；rc on 时 +1）
    check("groups_off_61", hook_groups_expected_count(true, false, false) == 14);
    check("groups_off_510", hook_groups_expected_count(false, false, false) == 11);
    check("groups_rc_61", hook_groups_expected_count(true, false, true) == 15);
    check("groups_rc_61_huawei", hook_groups_expected_count(true, true, true) == 16);
    check("groups_rc_510", hook_groups_expected_count(false, false, true) == 12);
    check("groups_rc_510_huawei", hook_groups_expected_count(false, true, true) == 13);

    std::cout << "RCVRESULT " << (failures == 0 ? "PASS" : "FAIL")
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
        else if (a == "--self-test-rcvirtual") { opt.self_test_rcvirtual = true; }
        else if (a == "--launcher") { opt.launcher_mode = need("--launcher"); }
        else if (a == "--gen-key") { have_key = true; }
        else if (a == "--json") { opt.json_sink = need("--json"); }
        else if (a == "--workdir") { opt.workdir = need("--workdir"); opt.workdir_from_user = true; }
        else if (a == "--quiet") { g_quiet = true; }
        else if (a == "--help" || a == "-h") {
            std::cout << "SPRoot patch engine v1.0.0 (contract SPROOT-DOC-C1 v1.0)\n"
                      << "usage: skroot-patch --in <kernel_image> --out <patched_kernel> (--gen-key|--key <48ch>)\n"
                      << "       [--json <ndjson_sink>] [--workdir <dir>] [--launcher off|rc|umh] [--quiet]\n"
                      << "       --launcher rc: 用 rc 虚拟化接管 AP 自启（免 init_boot；默认 off，见 NEW-DOC-02 §11)\n";
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
        // --launcher rc 维度（A8 矩阵扩到 8 例：off 6 例 + rc 2 例）
        const bool lrc = (opt.launcher_mode == "rc");
        const size_t exp_launcher = hook_groups_expected_count(ge61, opt.assume_huawei, lrc);
        std::cout << "launcher=" << opt.launcher_mode
                  << " hook_groups_expected_with_launcher=" << exp_launcher << "\n";
        J extra;
        extra.set("branch_ge_6_1", ge61)
             .set("huawei", opt.assume_huawei)
             .set("hook_groups_expected", static_cast<long long>(exp))
             .set("launcher", opt.launcher_mode)
             .set("hook_groups_expected_with_launcher", static_cast<long long>(exp_launcher));

        // ---- task-16：breakdown 对账矩阵（正样本 + 负样本，全部真跑）----
        // 铁律：每条断言都要有负样本证明它会红；否则又是"永远 PASS 的空断言"。
        // 用例名 GBREAK<name>，被 --na 以外的 CI 逐条 grep（见 verify_artifact.sh A8b）。
        {
            int fails = 0, cases = 0;
            auto chk = [&](const char* name, bool ok) {
                ++cases; if (!ok) ++fails;
                std::cout << "GBREAK " << name << (ok ? " PASS" : " FAIL") << "\n";
            };
            // 构造一个"命中 huawei"的 breakdown 进行对账（符号名走真实函数名）
            const std::vector<std::string> hit = { "hkip_check_uid_root" };
            const std::vector<std::string> none;

            // ① 正样本：>=6.1 + huawei 命中 → total 15，且 +1 可归因到 huawei
            //    这正是"魅族21 实测 15 组"的假定成因；若真机 report 长这样即为自证。
            {
                HookGroupsExpectedBreakdown b = hook_groups_expected_breakdown(true, hit, false);
                chk("pos_61_huawei_total15", b.total() == 15);
                chk("pos_61_huawei_branch14", b.branch_base == 14);
                chk("pos_61_huawei_bypass1", b.huawei_bypass == 1);
                chk("pos_61_huawei_source_detected", b.huawei_bypass_source == "detected");
                chk("pos_61_huawei_names_present", b.huawei_symbols_detected.size() == 1);
                chk("pos_61_huawei_consistent", b.consistent());
            }
            // ② 正样本：>=6.1 无 huawei → 14（基线），数组必须为空且 source=none
            {
                HookGroupsExpectedBreakdown b = hook_groups_expected_breakdown(true, none, false);
                chk("pos_61_nohuawei_total14", b.total() == 14);
                chk("pos_61_nohuawei_emptylist", b.huawei_symbols_detected.empty());
                chk("pos_61_nohuawei_source_none", b.huawei_bypass_source == "none");
                chk("pos_61_nohuawei_consistent", b.consistent());
            }
            // ③ 正样本：<6.1 分支基数 11（证明基数字不是写死 14）
            {
                HookGroupsExpectedBreakdown b = hook_groups_expected_breakdown(false, none, false);
                chk("pos_510_base11", b.total() == 11 && b.branch_base == 11);
            }
            // ④ 正样本：+launcher rc 与 +huawei 可叠加（14+1+1=16）
            {
                HookGroupsExpectedBreakdown b = hook_groups_expected_breakdown(true, hit, true);
                chk("pos_61_huawei_rc_total16", b.total() == 16 && b.launcher_rc == 1);
            }
            // ⑤ 正样本：--assume-huawei-symbols 的"假定"不得伪造符号名
            {
                HookGroupsExpectedBreakdown b = hook_groups_expected_breakdown(true, none, false, /*assumed*/true);
                chk("pos_assumed_total15", b.total() == 15);
                chk("pos_assumed_no_fake_names", b.huawei_symbols_detected.empty());
                chk("pos_assumed_source", b.huawei_bypass_source == "assumed");
                chk("pos_assumed_consistent", b.consistent());
            }
            // ⑤b total() 必须真的等于分项之和（防 total() 实现漂移成只返回 branch_base）。
            //     用**独立算式**逐个组合核验，而不是拿 total() 跟自己比。
            //     mutant 实测：把 total() 改成 return branch_base 时，只有这组会红。
            {
                const std::vector<std::string> combos[] = { none, hit };
                for (int gi = 0; gi < 2; ++gi) {
                    for (int li = 0; li < 2; ++li) {
                        for (int ai = 0; ai < 2; ++ai) {
                            HookGroupsExpectedBreakdown b = hook_groups_expected_breakdown(
                                true, combos[gi], li == 1, ai == 1);
                            const unsigned expect =
                                (unsigned)(14 + (b.huawei_bypass) + (b.launcher_rc));
                            chk("pos_total_is_sum_of_parts", b.total() == expect);
                        }
                    }
                }
            }
            // ⑤c hook_groups_expected_count() 与 breakdown.total() 必须同值
            //     （它是"有无 huawei"布尔口径，与带符号名的口径必须一致，否则两处算数漂移）
            {
                const bool hs[2] = { false, true };
                const bool lr[2] = { false, true };
                for (int hi = 0; hi < 2; ++hi)
                    for (int li = 0; li < 2; ++li) {
                        const size_t via_count = hook_groups_expected_count(true, hs[hi], lr[li]);
                        const size_t via_bd = hook_groups_expected_breakdown(
                            true, hs[hi] ? hit : none, lr[li], hs[hi]).total();
                        chk("pos_count_matches_breakdown", via_count == via_bd);
                    }
            }
            // ⑤d huawei_bypass 必须随符号命中而 +1（不是恒 0/恒 1）。
            //     mutant 实测：把 breakdown 里 huawei_bypass 恒置 0 时，这组与 ① 会红。
            chk("pos_huawei_bypass_follows_hit_1", hook_groups_expected_breakdown(true, hit, false).huawei_bypass == 1);
            chk("pos_huawei_bypass_follows_hit_0", hook_groups_expected_breakdown(true, none, false).huawei_bypass == 0);

            // ---- 负样本：从**已生成的真实 report JSON** 里改坏，再读回来判定必须红 ----
            // 用 EngineState 真实构造一份 report，保证测的是 build_report 的产物结构。
            auto make_report = [&]() -> J {
                EngineOptions o; o.key_from_user = true;
                EngineState s;
                s.branch_ge_6_1 = ge61;
                s.has_huawei = opt.assume_huawei;
                s.expected_breakdown = hook_groups_expected_breakdown(
                    ge61, opt.assume_huawei ? hit : none, opt.launcher_mode == "rc",
                    opt.assume_huawei);
                s.groups_expected = s.expected_breakdown.total();
                return build_report(o, s, std::string(48, 'A'));
            };
            J rep = make_report();
            chk("neg_baseline_ok", breakdown_json_selfconsistent(rep.raw()));

            auto mutated_bad = [&](const char* name, void (*mut)(cJSON*)) {
                cJSON* d = cJSON_Duplicate(rep.raw(), 1);
                mut(d);
                std::string why;
                const bool bad = !breakdown_json_selfconsistent(d, &why);
                cJSON_Delete(d);
                chk(name, bad);
            };
            // ⑥ 负样本：total 与 hook_groups_expected 不一致 → 必须红。
            //    构造要点（这是上一版被我自己抓到的漏洞）：只改**顶层** hook_groups_expected，
            //    保持 breakdown 内部算术完全自洽（total 仍 = base+bypass+launcher）。
            //    若连 total 一起改坏，算术规则会先报错，这条"跨字段一致"规则就没被测到 ——
            //    mutant 实测：删掉跨字段检查后用例仍全绿，说明它当时是假证据。
            mutated_bad("neg_total_mismatch_expected", [](cJSON* r) {
                cJSON* e = cJSON_GetObjectItem(r, "hook_groups_expected");
                const long long v = (e && cJSON_IsNumber(e)) ? (long long)e->valuedouble : 0;
                cJSON_ReplaceItemInObject(r, "hook_groups_expected", cJSON_CreateNumber((double)(v + 7)));
            });
            // ⑦ 负样本：huawei 命中但 huawei_bypass 写 0 → 必须红（task-16 明确要求）。
            //    构造要点：**保持算术自洽**（total 设为 branch_base+0+launcher_rc），
            //    使 branch_base+huawei_bypass+launcher_rc == total 仍成立，这样只有
            //    "命中却写 0" 这条规则能抓住它。否则会被前面的算术检查提前命中，
            //    断言就变成"看起来会红、其实被别的规则兜着"的假证据。
            //    total 用变量算，保证在 --assume-huawei-symbols 基线下也成立。
            {
                cJSON* d = cJSON_Duplicate(rep.raw(), 1);
                cJSON* bd = cJSON_GetObjectItem(d, "hook_groups_expected_breakdown");
                const cJSON* bj = cJSON_GetObjectItem(bd, "branch_base");
                const cJSON* lj = cJSON_GetObjectItem(bd, "launcher_rc");
                const long long base_v = (bj && cJSON_IsNumber(bj)) ? (long long)bj->valuedouble : 14;
                const long long lrc_v  = (lj && cJSON_IsNumber(lj)) ? (long long)lj->valuedouble : 0;
                cJSON_DeleteItemFromObject(bd, "huawei_symbols_detected");
                cJSON_AddItemToObject(bd, "huawei_symbols_detected",
                    cJSON_Parse("[\"hkip_check_uid_root\",\"hkip_check_gid_root\"]"));  // 命中
                cJSON_ReplaceItemInObject(bd, "huawei_bypass", cJSON_CreateNumber(0));   // 却写 0
                cJSON_ReplaceItemInObject(bd, "huawei_bypass_source", cJSON_CreateString("detected"));
                // 让算术检查通过，逼出 huawei 专用规则
                cJSON_ReplaceItemInObject(bd, "total", cJSON_CreateNumber((double)(base_v + lrc_v)));
                cJSON_ReplaceItemInObject(d, "hook_groups_expected",
                                          cJSON_CreateNumber((double)(base_v + lrc_v)));
                std::string why;
                const bool bad = !breakdown_json_selfconsistent(d, &why);
                cJSON_Delete(d);
                // 还要求"因预期原因"变红，而不是被算术规则顺手拦下
                chk("neg_huawei_hit_but_zero", bad && why.find("huawei") != std::string::npos);
            }
            // ⑧ 负样本：分项改了但 total 没跟着改（算术漂移）
            mutated_bad("neg_branch_base_wrong", [](cJSON* r) {
                cJSON* bd = cJSON_GetObjectItem(r, "hook_groups_expected_breakdown");
                cJSON_ReplaceItemInObject(bd, "branch_base", cJSON_CreateNumber(99));
            });
            // ⑨ 负样本：删掉 breakdown 字段 → 必须红（防字段被悄悄拿掉）
            mutated_bad("neg_breakdown_missing", [](cJSON* r) {
                cJSON_DeleteItemFromObject(r, "hook_groups_expected_breakdown");
            });
            // ⑩ 负样本：source=none 却仍列符号名（假证据）
            mutated_bad("neg_none_with_names", [](cJSON* r) {
                cJSON* bd = cJSON_GetObjectItem(r, "hook_groups_expected_breakdown");
                cJSON_ReplaceItemInObject(bd, "huawei_bypass_source", cJSON_CreateString("none"));
                cJSON_DeleteItemFromObject(bd, "huawei_symbols_detected");
                cJSON_AddItemToObject(bd, "huawei_symbols_detected",
                    cJSON_Parse("[\"hkip_check_uid_root\"]"));
            });
            std::cout << "GBREAKRESULT " << (fails == 0 ? "PASS" : "FAIL")
                      << " " << (cases - fails) << "/" << cases << " cases\n";
            extra.set("breakdown_matrix", fails == 0 ? "PASS" : "FAIL")
                 .set("breakdown_cases", cases);
            if (fails != 0) {
                emit_result(E_USAGE, std::move(extra));
                if (g_ndjson.is_open()) g_ndjson.close();
                return E_USAGE;
            }

            // ---- 地址记账矩阵（XV-2 缺陷回归）：report 必须如实列出每一处改动 ----
            // 缺陷现场：>=6.1 分支里 shellcode 在 cave、真实函数入口在别处；
            // 旧 report 只记 cave（patches[0]/patch_offset），入口被埋在 patches[1] 且
            // 键名不被采集器认识 ⇒ 审计者以为"没记的就是没改"。这组用例锁死修复。
            int afails = 0, acases = 0;
            auto achk = [&](const char* name, bool ok) {
                ++acases; if (!ok) ++afails;
                std::cout << "SITECASE " << name << (ok ? " PASS" : " FAIL") << "\n";
            };
            // 用真实字节：lead 实测的三处入口 b 跳转 + 各自 cave shellcode
            {
                EngineOptions o; o.key_from_user = true;
                EngineState s2; s2.branch_ge_6_1 = true;
                s2.expected_breakdown = hook_groups_expected_breakdown(true, {}, false);
                s2.orig_buf.assign(0x800000, 0); s2.file_buf = s2.orig_buf;
                s2.patch_addrs = { 0x24bd4,  0x38fcd4,
                                   0x85dee8, 0x3a1d88,
                                   0x85dfbc, 0x3a276c };
                s2.patch_hex   = { "AA",     "CC53F217",
                                   "AA",     "BC0BF217",
                                   "AA",     "4509F217" };
                s2.patch_lens  = { 180, 4, 180, 4, 108, 4 };
                s2.groups.push_back({ "do_execve",      "do_execveat_common",  0x24bd4,  0, 2 });
                s2.groups.push_back({ "filldir64",      "filldir64(core)",     0x85dee8, 2, 4 });
                s2.groups.push_back({ "compat_filldir", "compat_filldir(core)",0x85dfbc, 4, 6 });
                J r2 = build_report(o, s2, std::string(48, 'A'));

                // ① patch_sites_count 必须等于 vec_patch 总数（一处不漏）
                achk("sites_count_covers_all",
                     cJSON_GetObjectItem(r2.raw(), "patch_sites_count") &&
                     static_cast<size_t>(cJSON_GetObjectItem(r2.raw(), "patch_sites_count")->valuedouble) ==
                         s2.patch_addrs.size());
                // ② entry_hook_count 必须 == 3（三处真实入口）
                achk("entry_hook_count_3",
                     cJSON_GetObjectItem(r2.raw(), "entry_hook_count") &&
                     static_cast<int>(cJSON_GetObjectItem(r2.raw(), "entry_hook_count")->valuedouble) == 3);
                // ③ 每个入口都必须出现在某组的 entry_hooks[] 里（成对地址）
                {
                    const char* want[] = { "0x38fcd4", "0x3a1d88", "0x3a276c" };
                    cJSON* hooks = cJSON_GetObjectItem(r2.raw(), "hooks");
                    bool all_found = true;
                    for (const char* w : want) {
                        bool found = false;
                        const cJSON* hk = nullptr;
                        cJSON_ArrayForEach(hk, hooks) {
                            cJSON* ehs = cJSON_GetObjectItem(hk, "entry_hooks");
                            const cJSON* e = nullptr;
                            cJSON_ArrayForEach(e, ehs) {
                                cJSON* po = cJSON_GetObjectItem(e, "patch_offset");
                                if (po && cJSON_IsString(po) && std::string(po->valuestring) == w) found = true;
                            }
                        }
                        if (!found) all_found = false;
                    }
                    achk("all_entry_hooks_recorded", all_found);
                }
                // ④ patches[] 每项都要能被外部按 offset/addr 收集到（xv.py 兼容）
                {
                    cJSON* hooks = cJSON_GetObjectItem(r2.raw(), "hooks");
                    bool ok_compat = true;
                    const cJSON* hk = nullptr;
                    cJSON_ArrayForEach(hk, hooks) {
                        cJSON* ps = cJSON_GetObjectItem(hk, "patches");
                        const cJSON* p = nullptr;
                        cJSON_ArrayForEach(p, ps) {
                            if (!cJSON_GetObjectItem(p, "offset") || !cJSON_GetObjectItem(p, "addr"))
                                ok_compat = false;
                        }
                    }
                    achk("patches_have_offset_alias", ok_compat);
                }
                // ⑤ 正常路径不应有未归因写入
                achk("no_unattributed_when_fully_grouped", [&]{
                    cJSON* u = cJSON_GetObjectItem(r2.raw(), "unattributed_patches");
                    return u && cJSON_IsArray(u) && cJSON_GetArraySize(u) == 0;
                }());

                // ---- 负样本：把某组区间改小，让一处写入"漏账" ----
                // 期望：unattributed_patches 必须非空（显式列出），而不是静默丢掉。
                {
                    EngineState s3;   // EngineState 含 J（不可拷贝），故逐字段重建
                    s3.branch_ge_6_1 = true;
                    s3.expected_breakdown = s2.expected_breakdown;
                    s3.orig_buf = s2.orig_buf; s3.file_buf = s2.file_buf;
                    s3.patch_addrs = s2.patch_addrs;
                    s3.patch_hex = s2.patch_hex;
                    s3.patch_lens = s2.patch_lens;
                    s3.groups = s2.groups;
                    s3.groups[1].last = s3.groups[1].first + 1;  // filldir64 只记 cave，漏掉入口
                    J r3 = build_report(o, s3, std::string(48, 'A'));
                    cJSON* u = cJSON_GetObjectItem(r3.raw(), "unattributed_patches");
                    achk("neg_missing_entry_goes_unattributed",
                         u && cJSON_IsArray(u) && cJSON_GetArraySize(u) >= 1);
                    // 且漏账那处必须被如实标成 entry_hook 角色（而不是含糊的 data_patch）
                    bool role_ok = false;
                    const cJSON* it = nullptr;
                    cJSON_ArrayForEach(it, u) {
                        cJSON* ro = cJSON_GetObjectItem(it, "role");
                        cJSON* po = cJSON_GetObjectItem(it, "patch_offset");
                        if (ro && po && std::string(ro->valuestring) == "entry_hook" &&
                            std::string(po->valuestring) == "0x3a1d88") role_ok = true;
                    }
                    achk("neg_unattributed_marks_entry_hook", role_ok);
                }
                // ---- 负样本：整组丢失（模拟"织入了但完全没记账"）----
                {
                    EngineState s4;
                    s4.branch_ge_6_1 = true;
                    s4.expected_breakdown = s2.expected_breakdown;
                    s4.orig_buf = s2.orig_buf; s4.file_buf = s2.file_buf;
                    s4.patch_addrs = s2.patch_addrs;
                    s4.patch_hex = s2.patch_hex;
                    s4.patch_lens = s2.patch_lens;
                    s4.groups = s2.groups;
                    s4.groups.pop_back();          // 整个 compat_filldir 组消失
                    J r4 = build_report(o, s4, std::string(48, 'A'));
                    cJSON* u = cJSON_GetObjectItem(r4.raw(), "unattributed_patches");
                    achk("neg_dropped_group_becomes_unattributed",
                         u && cJSON_IsArray(u) && cJSON_GetArraySize(u) >= 2);
                }
                // ---- 负样本：分类器必须区分 b 跳转 vs 普通 4 字节写 ----
                // 若把 is_branch 判定写坏（恒真/恒假），这两个用例必须翻。
                achk("cls_b_is_entry_hook",
                     classify_patch(0, 4, "CC53F217") == PatchRole::EntryHook);
                achk("cls_non_branch_not_entry",
                     classify_patch(0, 4, "1F2003D5") != PatchRole::EntryHook);   // nop
                achk("cls_ret_is_ret_patch",
                     classify_patch(0, 4, "C0035FD6") == PatchRole::RetPatch);
                achk("cls_multibyte_is_shellcode",
                     classify_patch(0, 180, "AA") == PatchRole::CaveShellcode);
            }
            std::cout << "SITERESULT " << (afails == 0 ? "PASS" : "FAIL")
                      << " " << (acases - afails) << "/" << acases << " cases\n";
            extra.set("attribution_matrix", afails == 0 ? "PASS" : "FAIL")
                 .set("attribution_cases", acases);
            if (afails != 0) {
                emit_result(E_USAGE, std::move(extra));
                if (g_ndjson.is_open()) g_ndjson.close();
                return E_USAGE;
            }
        }
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

    // 隐藏自检：rc 虚拟化纯逻辑层（模板/布局/符号表/洞扫描/组数矩阵），CI 每次必跑（A11）
    if (opt.self_test_rcvirtual) {
        int rc = self_test_rcvirtual_contract();
        J extra;
        extra.set("rc_virtualize", rc == OK ? "PASS" : "FAIL");
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
    // --launcher 取值校验：off|rc|umh；umh（B 方案）尚未实现 → 显式 E101，绝不静默忽略
    if (opt.launcher_mode != "off" && opt.launcher_mode != "rc" && opt.launcher_mode != "umh") {
        std::string msg = "E101: --launcher must be one of off|rc|umh, got '" + opt.launcher_mode + "'";
        std::cerr << msg << "\n";
        J e; e.set("error", msg);
        emit_result(E_USAGE, std::move(e));
        return E_USAGE;
    }
    if (opt.launcher_mode == "umh") {
        std::string msg = "E101: --launcher umh is NOT implemented yet (B 方案，见 NEW-DOC-02 §9.4; "
                          "当前实现只有 off|rc)";
        std::cerr << msg << "\n";
        J e; e.set("error", msg);
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
            // 实际命中的华为符号名（不是布尔）：空数组 = 该内核无此保护路径。
            // 这是 task-16 的证据核心——report 里能指名道姓，才谈得上对账魅族21 的 15 组。
            std::vector<std::string> hw_syms = hook_huawei_symbols_detected(sym);
            st.has_huawei = !hw_syms.empty();
            bool hw_assumed = false;
            if (!opt.assume_kernel_version.empty()) {
                st.branch_ge_6_1 = !KernelVersionParser::version_less(opt.assume_kernel_version, "6.1.0");
                st.groups_expected_source = "assume";
            }
            if (opt.assume_huawei) {
                st.has_huawei = true;
                st.groups_expected_source = "assume";
                // --assume-huawei-symbols：真机没符号也当有。此时不能伪造符号名（那是假证据），
                // 只置 huawei_assumed，report 里 huawei_bypass_source="assumed" 明示其性质。
                if (hw_syms.empty()) hw_assumed = true;
            }
            st.expected_breakdown = hook_groups_expected_breakdown(
                st.branch_ge_6_1, hw_syms, opt.launcher_mode == "rc", hw_assumed);
            // 单一来源：期望值只来自 breakdown.total()，不再调用 hook_groups_expected_count()
            // 算第二遍（两处算数迟早漂移，这正是 14/15 疑云的温床）。
            st.groups_expected = st.expected_breakdown.total();
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
        LauncherOptions launcher_opt;
        launcher_opt.mode = opt.launcher_mode;
        pr = patch_kernel_handler(st.file_buf, off, sym, vec_patch, &st.groups, launcher_opt);
        if (!pr.patched)
            return fail(E_NO_HOOK_SPACE, "E220: failed to find hook start addr / not enough kernel space", "-");
        st.launcher = pr.launcher;
        // 显式 --launcher rc 但前置不满足（符号缺失/无洞/洞非法）→ E202 且不产出 --out（绝不静默降级）
        if (opt.launcher_mode == "rc" && !pr.launcher.enabled)
            return fail(E_SYMBOL_MISSING, "E202: --launcher rc preconditions not met",
                        pr.launcher.reason.empty() ? "unknown" : pr.launcher.reason);
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
