// skroot_patch_jni.cpp — libskpatchjni.so 的 JNI 入口（DOC-C1 契约的进程内调用形态）
//
// 背景（见 docs/NEW-DOC-02 §4）：Android 10+ 的 untrusted_app 域不允许 exec
// app 数据目录（W^X / app_data_file neverallow）。旧链路把 jniLibs 里的 ELF
// symlink 进 app 私有目录再 exec，必然 EACCES（NEW-DOC-03 根因 R2）。
// 因此提供两条集成形态：
//   A. 独立 ELF：libskrootpatch.so（静态链接可执行文件），直接以
//      nativeLibraryDir 下的真实路径 exec，不经 symlink。崩溃隔离好。
//   B. JNI 内存内：libskpatchjni.so，进程内调用 CLI 主函数，完全绕过 exec
//      限制；进度仍走 --json NDJSON 文件（契约 §2.3），Java 侧轮询即可。
//
// 本文件只做 argv 拼装 + 调用，不含任何修补逻辑（逻辑唯一实现在
// engine/src/skroot_patch_main.cpp，JNI 版通过 -Dmain=skroot_patch_cli_main
// 重命名其入口符号，避免共享库导出 main 造成链接冲突）。

#include <jni.h>
#include <string>
#include <vector>

// 由 skroot_patch_main.cpp 提供（extern "C"，CLI 与 JNI 共用同一实现）
extern "C" int spr_engine_run(int argc, char** argv);

namespace {

std::string toStd(JNIEnv* env, jstring s) {
    if (s == nullptr) return {};
    const char* c = env->GetStringUTFChars(s, nullptr);
    std::string out = c ? c : "";
    if (c) env->ReleaseStringUTFChars(s, c);
    return out;
}

}  // namespace

extern "C" JNIEXPORT jint JNICALL
Java_me_sproot_patch_NativeEngine_patch(
    JNIEnv* env, jclass /*clazz*/,
    jstring inPath, jstring outPath, jstring key /*nullable*/,
    jstring jsonSink /*nullable*/, jstring workdir /*nullable*/) {

    std::vector<std::string> owned;
    owned.push_back("skroot-patch");
    owned.push_back("--in");
    owned.push_back(toStd(env, inPath));
    owned.push_back("--out");
    owned.push_back(toStd(env, outPath));

    const std::string keyStr = toStd(env, key);
    if (!keyStr.empty()) {
        owned.push_back("--key");
        owned.push_back(keyStr);
    } else {
        owned.push_back("--gen-key");
    }

    const std::string jsonStr = toStd(env, jsonSink);
    if (!jsonStr.empty()) {
        owned.push_back("--json");
        owned.push_back(jsonStr);
    }
    const std::string workStr = toStd(env, workdir);
    if (!workStr.empty()) {
        owned.push_back("--workdir");
        owned.push_back(workStr);
    }
    // JNI 形态下 stdout 不可见（Android 上被重定向到 /dev/null），
    // 结构化结果一律走 --json + <out>.report.json，因此加 --quiet。
    owned.push_back("--quiet");

    std::vector<char*> argv;
    argv.reserve(owned.size() + 1);
    for (auto& s : owned) argv.push_back(const_cast<char*>(s.c_str()));
    argv.push_back(nullptr);

    return static_cast<jint>(
        spr_engine_run(static_cast<int>(owned.size()), argv.data()));
}

// 引擎版本（Java 侧做兼容性检查用；与 DOC-C1 §2.3 engine_version 一致）
extern "C" JNIEXPORT jstring JNICALL
Java_me_sproot_patch_NativeEngine_engineVersion(JNIEnv* env, jclass /*clazz*/) {
    return env->NewStringUTF("1.0.0");
}
