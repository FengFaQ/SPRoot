package me.sproot.patch

/**
 * 修补引擎 JNI 门面（与 engine/android/jni/skroot_patch_jni.cpp 的 ABI 严格对应）。
 *
 * 符号（nm -D --defined-only libskpatchjni.so 实测）：
 * ```
 * Java_me_sproot_patch_NativeEngine_patch
 * Java_me_sproot_patch_NativeEngine_engineVersion
 * ```
 *
 * 职责边界：本类只做「Java 参数 → 引擎调用」的桥接，不含任何修补逻辑；
 * 参数语义与 CLI 契约 SPROOT-DOC-C1 §1 一一对应：
 *  - inPath   = --in（已解压 arm64 内核 Image）
 *  - outPath  = --out
 *  - key      = --key；null/空 → --gen-key
 *  - jsonSink = --json（NDJSON 进度落盘，Java 侧轮询）
 *  - workdir  = --workdir
 * 引擎内部固定追加 --quiet（stdout 在 Android 上不可见），结果一律走 json/report 文件。
 */
object NativeEngine {

    /**
     * 阻塞式修补。
     * @return SPROOT-DOC-C1 §3 退出码（0=成功）
     */
    @JvmStatic
    external fun patch(
        inPath: String,
        outPath: String,
        key: String?,
        jsonSink: String?,
        workdir: String?,
    ): Int

    /** 引擎版本字符串（与 DOC-C1 §2.3 engine_version 一致） */
    @JvmStatic
    external fun engineVersion(): String
}
