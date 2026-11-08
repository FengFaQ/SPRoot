package com.sproot.manager.core

import android.content.Context
import me.sproot.patch.NativeEngine
import java.io.File

/**
 * SK 修补引擎（aarch64 JNI 形态）。
 *
 * ## 为什么是 .so + JNI，而不是「可执行文件」
 * 旧版根因已确证：APK 内根本没有 `skroot-patch` 可执行文件（jniLibs 为空），
 * 而且产物是宿主 x86-64 glibc ELF；即便放进去，Android 10+ 的 untrusted_app 域
 * 也不允许 exec app 数据目录（W^X/app_data_file neverallow，NEW-DOC-03 根因 R2）。
 *
 * 新方案：patcher 的 CI artifact `skroot-engine-arm64` 内含
 *  - `libskpatchjni.so`（JNI 形态，进程内调用；本类使用）
 *  - `libskrootpatch.so`（静态链接 CLI 形态，保留但不使用）
 *  - `magiskboot`（v31.1 aarch64，打入 jniLibs 供 exec）
 *
 * ## JNI ABI（引擎提供，非本项目臆造）
 * 见 `me/sproot/patch/NativeEngine.kt`：
 * ```
 * Java_me_sproot_patch_NativeEngine_patch(JNIEnv*, jclass, jstring in, jstring out,
 *                                         jstring key, jstring json, jstring workdir) -> jint
 * Java_me_sproot_patch_NativeEngine_engineVersion(JNIEnv*, jclass) -> jstring
 * ```
 * 进度**只走 `jsonSink` 的 NDJSON 文件轮询**（契约 §2.3），不依赖 stdout（JNI 下不可见）。
 *
 * ## 失败语义
 * 引擎缺失时 [ensureLoaded] 返回**可读错误**而不是抛异常——UI 必须把它当成
 * 一等公民的失败态展示（这就是旧版"一闪即退"的反面）。
 * 失败时引擎不得留下 out 文件（C1 §3 铁律），Kotlin 侧仍二次校验。
 */
object PatchEngine {

    /** 实际打进 APK 的文件名：libskpatchjni.so */
    const val LIB_NAME = "skpatchjni"
    const val LIB_FILE_NAME = "lib$LIB_NAME.so"

    /** 引擎 ABI 版本（与 patcher 侧一致；变化时同步本文档） */
    const val JNI_BINDING_VERSION = 2

    @Volatile
    private var loadFailure: String? = null
    @Volatile
    private var loaded = false
    @Volatile
    private var lastReportFile: File? = null
    @Volatile
    private var lastAttemptedKey: String? = null

    data class EngineStatus(
        val path: String,
        val present: Boolean,
        val executable: Boolean,
        val aarch64: Boolean,
        val loaded: Boolean,
        val version: String?,
        val detail: String,
    )

    fun libraryFile(context: Context): File =
        File(context.applicationInfo.nativeLibraryDir, LIB_FILE_NAME)

    /** @return null 表示加载成功；否则返回完整的失败原因（含绝对路径与排查建议）。 */
    @Synchronized
    fun ensureLoaded(): String? {
        if (loaded) return null
        loadFailure?.let { return it }
        val failure: String? = try {
            System.loadLibrary(LIB_NAME)
            loaded = true
            null
        } catch (t: Throwable) {
            "加载 lib$LIB_NAME.so 失败：${t.javaClass.simpleName}: ${t.message}\n" +
                "常见原因：①APK 内缺 arm64-v8a/$LIB_FILE_NAME（CI 未拉到 skroot-engine-arm64 artifact）；" +
                "②ABI 不匹配（非 arm64 设备）；③符号缺失（引擎 JNI 入口应为 " +
                "Java_me_sproot_patch_NativeEngine_patch / _engineVersion）。"
        }
        loadFailure = failure
        return failure
    }

    fun version(): String? =
        if (ensureLoaded() == null) runCatching { NativeEngine.engineVersion() }.getOrNull() else null

    fun status(context: Context): EngineStatus {
        val file = libraryFile(context)
        val present = file.isFile
        val executable = present && file.canExecute()
        val aarch64 = present && Elf.isAArch64(file.absolutePath)
        val failure = ensureLoaded()
        val versionValue = if (failure == null) version() else null
        val detail = when {
            !present -> "APK 内缺少 arm64-v8a/$LIB_FILE_NAME（CI 未拉到 skroot-engine-arm64 artifact）"
            !aarch64 -> "文件存在但不是 AArch64 ELF（可能打入了错误架构产物）"
            failure != null -> failure
            versionValue == null -> "已加载，但 engineVersion() 未返回版本（ABI 不符）"
            else -> "已加载，引擎版本 $versionValue，JNI ABI v$JNI_BINDING_VERSION"
        }
        return EngineStatus(
            path = file.absolutePath,
            present = present,
            executable = executable,
            aarch64 = aarch64,
            loaded = failure == null,
            version = versionValue,
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
    ): Int {
        ensureLoaded()?.let { throw IllegalStateException(it) }
        lastReportFile = File(outFile.absolutePath + ".report.json")
        lastAttemptedKey = if (genKey) null else key
        return NativeEngine.patch(
            inPath = inFile.absolutePath,
            outPath = outFile.absolutePath,
            key = if (genKey) null else key,
            jsonSink = jsonFile.absolutePath,
            workdir = workDir.absolutePath,
        )
    }

    /** 引擎最后错误：优先 report.json 的 errors，其次原始 report 前 2KB */
    fun lastError(): String? {
        val report = lastReportFile?.takeIf { it.isFile } ?: return null
        val text = runCatching { report.readText() }.getOrNull() ?: return null
        return extractField(text, "errors")
            ?: text.take(2000)
    }

    /** 引擎最后生成的 Root 密钥（report.json 的 root_key 字段） */
    fun lastRootKey(): String? {
        val report = lastReportFile?.takeIf { it.isFile } ?: return null
        val text = runCatching { report.readText() }.getOrNull() ?: return null
        return MiniJson.string(text, "root_key") ?: lastAttemptedKey
    }

    /** 从 JSON 里粗略取数组/字符串字段（避免为报告引入序列化依赖） */
    private fun extractField(json: String, key: String): String? {
        val idx = json.indexOf("\"$key\"")
        if (idx < 0) return null
        val start = json.indexOf('[', idx).let { if (it < 0) json.indexOf(':', idx) else it }
        if (start < 0) return null
        val end = json.indexOf(']', start).let { if (it < 0) json.length else it }
        if (end <= start) return null
        val raw = json.substring(start, (end + 1).coerceAtMost(json.length)).trim()
        return raw.takeIf { it.isNotEmpty() && it != "[]" }
    }
}
