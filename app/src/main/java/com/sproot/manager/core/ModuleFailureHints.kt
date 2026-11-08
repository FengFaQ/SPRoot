package com.sproot.manager.core

/**
 * 模块操作失败 → 中文可操作提示（延续 G-04：原始码/原始输出 + 可读文案一起给）。
 *
 * 输入是 **exit code + apd 的原始 stdout/stderr**（不依赖其 `--json` schema 是否定稿），
 * 通过关键字匹配给出「可能原因 + 建议动作」。匹配不到时给通用提示并保留原始输出，
 * **绝不置信度臆断**：每条提示都标注依据（命中的关键字或退出码）。
 */
object ModuleFailureHints {

    data class Hint(
        val cause: String,
        val action: String,
        val basis: String,
    )

    /**
     * @param operation 人类可读操作名（"安装"/"卸载"/"启用"/"禁用"/"读取列表"）
     */
    fun analyze(exitCode: Int, stdout: String, stderr: String, operation: String): List<Hint> {
        val text = (stdout + "\n" + stderr).lowercase()
        val hints = mutableListOf<Hint>()

        // ---- 先按退出码给确定性的提示（模块子命令码表 NEW-DOC-05 §3.1.2，优先级最高） ----
        ModuleExitCode.describe(exitCode)?.let { (cause, action) ->
            hints += Hint(cause = cause, action = action, basis = "exit=$exitCode（NEW-DOC-05 §3.1.2）")
            return hints
        }

        when (exitCode) {
            ApdClient.EXIT_ESCALATION -> hints += Hint(
                cause = "密钥无效或被内核静默拒绝（exit=125）",
                action = "该密钥与当前内核不匹配：请重新走一次「修补 → 刷入」，或到设置页确认密钥；本 App 不会自动重试",
                basis = "exit=125",
            )
            ApdClient.EXIT_NOT_EXECUTABLE -> hints += Hint(
                cause = "AP 不可执行（exit=126）",
                action = "到「设置 → AP 部署」重新部署 AP（首次部署或升级后需要）",
                basis = "exit=126",
            )
            ApdClient.EXIT_NOT_FOUND -> hints += Hint(
                cause = "找不到 AP 或目标命令（exit=127）",
                action = "先完成 AP 部署；若已部署仍报此错，说明设备上的 apd 与 APK 内置版本不一致，请重新部署",
                basis = "exit=127",
            )
            ApdClient.EXIT_TIMEOUT -> hints += Hint(
                cause = "AP 调用超时（exit=124）",
                action = "大模块安装可能较慢：确认存储空间充足后重试；若持续超时，请把日志上报（可能 AP 卡死）",
                basis = "exit=124",
            )
            ApdClient.EXIT_USAGE -> hints += Hint(
                cause = "AP 用法错误（exit=2）",
                action = "这是客户端/契约 bug：请把原始输出与操作名上报（不要重试）",
                basis = "exit=2",
            )
        }
        if (hints.isNotEmpty()) return hints

        // ---- 再按原始输出关键字匹配（schema 未定稿也能工作） ----
        fun hit(vararg keywords: String): String? =
            keywords.firstOrNull { text.contains(it.lowercase()) }

        hit("module.prop")?.let {
            hints += Hint(
                cause = "模块包缺少 module.prop（或格式不正确）",
                action = "确认 zip 根目录直接含 module.prop（不是套了一层文件夹）；Magisk/KSU 模块包可直接导入",
                basis = "输出含 \"$it\"",
            )
        }
        hit("already exists", "already installed", "duplicate", "conflict", "id 冲突", "id already")?.let {
            hints += Hint(
                cause = "模块 ID 冲突（同 ID 已安装）",
                action = "先卸载同 ID 模块，或修改模块包 module.prop 里的 id 后再安装",
                basis = "输出含 \"$it\"",
            )
        }
        hit("no space", "enospc", "not enough space", "空间不足")?.let {
            hints += Hint(
                cause = "存储空间不足",
                action = "清理 /data 空间（至少留出模块解压后 2 倍体积）后重试",
                basis = "输出含 \"$it\"",
            )
        }
        hit("permission denied", "eacces", "eperm", "operation not permitted")?.let {
            hints += Hint(
                cause = "权限不足（文件系统/SELinux）",
                action = "确认 AP 已正确部署且处于已修补内核；若刚刷机请重启后再试；仍失败请上报日志（含 avc denied）",
                basis = "输出含 \"$it\"",
            )
        }
        hit("avc: denied", "selinux")?.let {
            hints += Hint(
                cause = "SELinux 拒绝（avc denied）",
                action = "记录完整 avc 行并上报：可能是 AP 部署上下文不正确，需要重新部署或修补内核",
                basis = "输出含 \"$it\"",
            )
        }
        hit("end of central directory", "bad zip", "corrupt", "not a zip", "invalid zip", "unzip")?.let {
            hints += Hint(
                cause = "模块 zip 损坏或不是合法 zip",
                action = "重新下载模块包（校验 sha256）；确认不是只改扩展名的文件",
                basis = "输出含 \"$it\"",
            )
        }
        hit("not found", "no such file", "不存在")?.let {
            hints += Hint(
                cause = "$operation 目标不存在",
                action = "刷新模块列表确认 ID 正确；若模块已被手动删除，重启 AP 后重试",
                basis = "输出含 \"$it\"",
            )
        }

        if (hints.isEmpty()) {
            hints += Hint(
                cause = "未识别的失败（exit=$exitCode）",
                action = "点击「复制原始输出」把完整 stdout/stderr 发给开发者；本 App 已保留原始文本，不做猜测",
                basis = "exit=$exitCode / 无已知关键字",
            )
        }
        return hints
    }
}
