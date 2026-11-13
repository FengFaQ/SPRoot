package com.sproot.manager.core

import android.content.Context
import java.io.File

/**
 * SK 修补引擎（aarch64 JNI 形态）。
 *
 * ## 为什么是 .so + JNI，而不是「可执行文件」
 * 旧版根因已确证：APK 内根本没有 `skroot-patch` 可执行文件（jniLibs 为空），
 * 而且产物是宿主 x86-64 glibc ELF；即便放进去，在 Android 上 exec 也需要额外
 * symlink/chmod 路径（旧版就在 filesDir 里 exec，违反 SELinux W^X）。
 * 新方案把引擎编译成 `libskrootpatch.so` 随 APK 分发，由 System.loadLibrary 加载，
 * 进程内调用，**不 exec、不落 filesDir、不拼 shell**。
 *
 * ## JNI 绑定契约（v1，供修补引擎组对齐）
 * 引擎侧需导出下列静态 native 方法（Kotlin `object` + `@JvmStatic external` 直接映射）：
 *
 * ```c
 * JNIEXPORT jstring JNICALL Java_com_sproot_manager_core_PatchEngine_nativeVersion(JNIEnv*, jclass);
 * JNIEXPORT jint    JNICALL Java_com_sproot_manager_core_PatchEngine_nativePatch(
 *         JNIEnv*, jclass, jstring inPath, jstring outPath, jstring key, jboolean genKey,
 *         jstring jsonPath, jstring workDir, jboolean verbose);
 * JNIEXPORT jstring JNICALL Java_com_sproot_manager_core_PatchEngine_nativeLastError(JNIEnv*, jclass);
 * JNIEXPORT jstring JNICALL Java_com_sproot_manager_core_PatchEngine_nativeLastRootKey(JNIEnv*, jclass);
 * ```
 *
 * 语义与 CLI 契约（SPROOT-DOC-C1 §1）一一对应：
 *  - `inPath`  = `--in`（必须是已解压的 arm64 内核 Image；压缩输入引擎应回 E_INPUT）
 *  - `outPath` = `--out`（失败时引擎**不得**留下 out 文件）
 *  - `key`     = `--key`；`genKey=true` 时忽略 key 走 `--gen-key`
 *  - `jsonPath`= `--json`（NDJSON 进度落盘，Kotlin 侧轮询读取，不依赖 stdout 管道）
 *  - 返回值    = 进程退出码枚举（EngineExitCode）
 *
 * 引擎缺失时 ensureLoaded() 返回**可读错误**而不是抛异常——UI 必须把它当成
 * 一等公民的失败态展示（这就是旧版"一闪即退"的反面）。
 */
object PatchEngine {

    const val LIB_NAME = "skrootpatch"
    const val LIB_FILE_NAME = "lib$LIB_NAME.so"

    /** 绑定契约版本；引擎侧实现变更须同步 +1 并在方案文档记录 */
    const val JNI_BINDING_VERSION = 1

    @Volatile
    private var loadState: String? = null // null = 未尝试；"" = 成功；其他 = 失败原因
    @Volatile
    private var loaded = false

    data class EngineStatus(
        val path: String,
        val present: Boolean,
        val executable: Boolean,
        val aarch64: Boolean,
        val loaded: Boolean,
        val version: String?,
        val detail: String,
    )

    // ------------------------------------------------------------------ JNI
    @JvmStatic
    external fun nativeVersion(): String

    @JvmStatic
    external fun nativePatch(
        inPath: String,
        outPath: String,
        key: String,
        genKey: Boolean,
        jsonPath: String,
        workDir: String,
        verbose: Boolean,
    ): Int

    @JvmStatic
    external fun nativeLastError(): String

    @JvmStatic
    external fun nativeLastRootKey(): String

    // -------------------------------------------------------------- helpers
    fun libraryFile(context: Context): File =
        File(context.applicationInfo.nativeLibraryDir, LIB_FILE_NAME)

    /** @return null 表示加载成功；否则返回完整的失败原因（含绝对路径与排查建议）。 */
    @Synchronized
    fun ensureLoaded(): String? {
        if (loaded) return null
        loadState?.let { if (!loaded) return it }
        val failure: String? = try {
            System.loadLibrary(LIB_NAME)
            loaded = true
            null
        } catch (t: Throwable) {
            "加载 lib$LIB_NAME.so 失败：${t.javaClass.simpleName}: ${t.message}\n" +
                "常见原因：①APK 内缺 arm64-v8a/$LIB_FILE_NAME；②ABI 不匹配（非 arm64 设备）；" +
                "③符号缺失（引擎未导出契约 v$JNI_BINDING_VERSION 的 JNI 函数）。"
        }
        loadState = failure ?: ""
        return failure
    }

    fun status(context: Context): EngineStatus {
        val file = libraryFile(context)
        val present = file.isFile
        val executable = present && file.canExecute()
        val aarch64 = present && Elf.isAarch64(file.absolutePath)
        val loadFailure = ensureLoaded()
        val version = if (loadFailure == null) {
            runCatching { nativeVersion() }.onFailure {
                AppLog.error("Engine", "nativeVersion() 调用失败", it)
            }.getOrNull()
        } else null
        val detail = when {
            !present -> "APK 内缺少 arm64-v8a/$LIB_FILE_NAME（构建时未打入 jniLibs）"
            !aarch64 -> "文件存在但不是 AArch64 ELF（可能打入了错误架构产物）"
            loadFailure != null -> loadFailure
            version == null -> "已加载，但 nativeVersion() 未返回版本号（绑定契约不符）"
            else -> "已加载，引擎版本 $version，绑定契约 v$JNI_BINDING_VERSION"
        }
        return EngineStatus(
            path = file.absolutePath,
            present = present,
            executable = executable,
            aarch64 = aarch64,
            loaded = loadFailure == null,
            version = version,
            detail = detail,
        )
    }

    /**
     * 调用修补引擎（阻塞直到完成）。调用方必须放在后台线程。
     * 成功返回 0；失败返回契约退出码。
     */
    fun patch(
        inFile: File,
        outFile: File,
        key: String,
        genKey: Boolean,
        jsonFile: File,
        workDir: File,
        verbose: Boolean,
    ): Int {
        ensureLoaded()?.let { throw IllegalStateException(it) }
        return nativePatch(
            inFile.absolutePath,
            outFile.absolutePath,
            key,
            genKey,
            jsonFile.absolutePath,
            workDir.absolutePath,
            verbose,
        )
    }

    fun lastError(): String? =
        if (loaded) runCatching { nativeLastError() }.getOrNull()?.takeIf { it.isNotBlank() } else null

    fun lastRootKey(): String? =
        if (loaded) runCatching { nativeLastRootKey() }.getOrNull()?.takeIf { it.isNotBlank() } else null
}
