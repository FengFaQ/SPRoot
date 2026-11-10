package com.sproot.manager.core

/** 日志级别（与 UI 配色一一对应） */
enum class LogLevel { DEBUG, INFO, WARN, ERROR }

/** 单条日志；timeMs 为 epoch millis */
data class LogEntry(
    val timeMs: Long,
    val level: LogLevel,
    val tag: String,
    val message: String,
)

/** 修补流水线阶段。UI 进度条直接映射该枚举，避免"闪一下就没"的观感。 */
enum class PatchStage(val title: String) {
    IDLE("就绪"),
    PREFLIGHT("环境自检"),
    COPY_INPUT("准备镜像"),
    UNPACK("解包 boot.img"),
    DECOMPRESS("解压内核"),
    PATCH_KERNEL("修补内核"),
    RECOMPRESS("回压内核"),
    REPACK("重打包 boot.img"),
    VERIFY("校验产物"),
    DONE("完成"),
    FAILED("失败"),
}

/** 修补模式（借鉴 APatch PatchMode，先落地前两种，后两种占位禁用） */
enum class PatchMode(val label: String, val description: String, val enabled: Boolean = true) {
    EXPORT_ONLY(
        "仅导出",
        "生成修补后的 boot.img 并保存到 Downloads/SPRoot/，不给刷入指引",
    ),
    EXPORT_AND_GUIDE(
        "导出并给出刷入指引",
        "生成产物 + 校验信息 + fastboot 命令模板 + AVB 风险提示",
    ),
    RESTORE_BACKUP(
        "从备份还原",
        "把修补前的 boot.img 还原到 Downloads/SPRoot/（在设置 → 备份中操作）",
        enabled = false,
    ),
    UNINSTALL_RESTORE(
        "卸载并还原内核",
        "需要已部署 AP；本阶段未开放",
        enabled = false,
    ),
}

/**
 * 修补页 UI 状态。
 *
 * 设计铁律（来自旧版失败根因）：
 *  - [errorTitle]/[errorDetail] 一旦写入，**不得**因阶段变化被清空；只有用户点"重置"才清。
 *  - [logPath] 始终指向落盘日志，失败后用户可复读/分享。
 */
data class PatchUiState(
    val stage: PatchStage = PatchStage.IDLE,
    val progress: Float = 0f,
    val phase: String = "",
    /** 引擎 NDJSON 自报的原始 phase（read_input…write_output）；空=还没收到事件 */
    val enginePhase: String = "",
    /** 引擎自报 pct（0..100）；-1=还没收到事件 */
    val enginePct: Int = -1,
    val running: Boolean = false,
    val mode: PatchMode = PatchMode.EXPORT_ONLY,
    val sourceUri: String? = null,
    val sourceName: String? = null,
    // P2 双镜像：init_boot.img（可选；ramdisk 的权威落点，D-12）
    val initBootSourceUri: String? = null,
    val initBootSourceName: String? = null,
    val initBootOutputPath: String? = null,
    val initBootOutputSha256: String? = null,
    val initBootBytes: Long = 0L,
    val workDir: String? = null,
    val outputPath: String? = null,
    val outputSha256: String? = null,
    val outputBytes: Long = 0L,
    val kernelBytes: Long = 0L,
    val kernelEqualLength: Boolean = false,
    val backupSha1: String? = null,
    // D6 AVB 断言结果
    val avbStatus: String = "",
    val avbFooterPresent: Boolean = false,
    val avbLengthEqual: Boolean = false,
    val avbVbmetaEqual: Boolean = false,
    // D1 开机自启注入结果
    val rcInjected: Boolean = false,
    val rcTarget: String = "",
    val rcReason: String = "",
    val rcSource: String = "",
    val reportPath: String? = null,
    val logPath: String? = null,
    val rootKey: String? = null,
    val errorCode: String? = null,
    val errorTitle: String? = null,
    val errorDetail: String? = null,
    val startedAt: Long = 0L,
    val finishedAt: Long = 0L,
) {
    val hasError: Boolean get() = errorTitle != null || errorDetail != null
}

/** 能力自检条目（入口自检 + 每次修补前必查） */
data class CapabilityItem(
    val key: String,
    val label: String,
    val ok: Boolean,
    val detail: String,
    val blocking: Boolean = true,
)

/**
 * 引擎 NDJSON 进度事件里的 `phase` → 中文（UI 直接展示，避免用户看到 read_input）。
 *
 * 引擎契约（engine/src/skroot_patch_main.cpp emit_progress）：
 * ```
 * read_input 0→5 → parse_kallsyms 5→15 → derive_offsets 20→30 → locate_init_cred 35
 * → patch_cfi_huawei 40 → patch_hooks 50 → embed_key 80 → write_output 95 → done 100
 * ```
 */
object EnginePhase {
    fun label(phase: String): String = when (phase) {
        "read_input" -> "读取内核"
        "parse_kallsyms" -> "解析 kallsyms 符号表"
        "derive_offsets" -> "推导 cred/uid/seccomp 偏移"
        "locate_init_cred" -> "定位 init_cred"
        "patch_cfi_huawei" -> "修补 CFI / 华为分支"
        "patch_hooks" -> "写入 hook 跳板"
        "embed_key" -> "内嵌 Root 密钥（47 字符 + NUL）"
        "write_output" -> "写出修补后内核"
        "done" -> "完成"
        else -> phase
    }
}

/** 修补引擎 JNI 返回码（与 SPROOT-DOC-C1 §3 退出码枚举一致） */
object EngineExitCode {
    const val OK = 0
    const val E_USAGE = 101
    const val E_INPUT = 102
    const val E_KALLSYMS = 201
    const val E_SYMBOL_MISSING = 202
    const val E_OFFSET_DERIVE = 210
    const val E_INIT_CRED_NOT_FOUND = 211
    const val E_NO_HOOK_SPACE = 220
    const val E_UNSUPPORTED_VERSION = 230
    const val E_WRITE = 240
    const val E_INTERNAL = 250

    /** 码 → 给人看的解释；未知码回退为通用描述，绝不吞掉原始码。 */
    fun describe(code: Int): String = when (code) {
        OK -> "成功"
        E_USAGE -> "E_USAGE($code)：调用参数错误（引擎实现或调用方 bug）"
        E_INPUT -> "E_INPUT($code)：输入内核不可读 / 仍是 .img/.gz / 非 arm64 Image"
        E_KALLSYMS -> "E_KALLSYMS($code)：kallsyms 解析失败，该内核暂不支持"
        E_SYMBOL_MISSING -> "E_SYMBOL_MISSING($code)：关键符号缺失，该内核暂不支持"
        E_OFFSET_DERIVE -> "E_OFFSET_DERIVE($code)：cred/uid/seccomp 偏移推导失败"
        E_INIT_CRED_NOT_FOUND -> "E_INIT_CRED_NOT_FOUND($code)：init_cred 模式搜索失败"
        E_NO_HOOK_SPACE -> "E_NO_HOOK_SPACE($code)：shellcode 空间不足（die/cfi 区域过小）"
        E_UNSUPPORTED_VERSION -> "E_UNSUPPORTED_VERSION($code)：内核版本超出 3.10~6.18 支持域"
        E_WRITE -> "E_WRITE($code)：输出写失败（磁盘/权限）"
        E_INTERNAL -> "E_INTERNAL($code)：引擎内部未分类错误"
        else -> "E_UNKNOWN($code)：引擎返回了契约外的退出码"
    }
}
