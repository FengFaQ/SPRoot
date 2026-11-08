package com.sproot.manager.core

/**
 * 模块子命令退出码（sys-guard 定稿，NEW-DOC-05 §3.1.2）
 *
 * 0 成功｜2 用法｜110 输入不存在｜111 zip 损坏｜112 module.prop 缺失/字段非法｜
 * 113 不安全 zip（zip-slip/符号链接）｜114 超限（zip>512MiB / 解压>1GiB / 条目>10万）｜
 * 115 磁盘不足｜116 id 冲突｜120 安装脚本失败｜121 解包/IO（含 booting 门禁）｜
 * 122 state 写入失败｜124 超时｜125 提权失败｜126/127 部署类。
 *
 * UI 铁律：原始码与服务端 `error` 字段原样展示，映射只做「补充解释」，不覆盖原文。
 */
object ModuleExitCode {
    const val OK = 0
    const val E_USAGE = 2
    const val E_NOT_FOUND = 110
    const val E_ZIP_CORRUPT = 111
    const val E_PROPFILE_INVALID = 112
    const val E_ZIP_UNSAFE = 113
    const val E_LIMIT_EXCEEDED = 114
    const val E_NO_SPACE = 115
    const val E_ID_CONFLICT = 116
    const val E_INSTALL_SCRIPT = 120
    const val E_UNPACK_IO = 121
    const val E_STATE_WRITE = 122
    const val E_TIMEOUT = 124
    const val E_ESCALATION = 125
    const val E_NOT_EXECUTABLE = 126
    const val E_NOT_DEPLOYED = 127

    /** 码 → (可能原因, 建议动作)；未知码返回 null，由关键字回退逻辑处理 */
    fun describe(code: Int): Pair<String, String>? = when (code) {
        E_USAGE -> "AP 用法错误（2）" to "客户端/契约 bug：上报原始输出，勿重试"
        E_NOT_FOUND -> "目标模块不存在（110）" to "刷新列表确认模块 ID；若已被手动删除，重启 AP 后重试"
        E_ZIP_CORRUPT -> "zip 损坏（111）" to "重新下载模块包并校验 sha256；确认不是改扩展名的文件"
        E_PROPFILE_INVALID -> "module.prop 缺失或字段非法（112）" to "确认 zip 根目录直接含 module.prop，且 id/name/version 合法（id 只含字母数字与 _-）"
        E_ZIP_UNSAFE -> "不安全的 zip（113，zip-slip/符号链接）" to "该模块包含越界路径或符号链接，已拒绝安装；请更换可信来源的包"
        E_LIMIT_EXCEEDED -> "超出体积/条目上限（114）" to "上限：zip≤512MiB、解压≤1GiB、条目≤10 万；请精简模块或选择更小版本"
        E_NO_SPACE -> "磁盘空间不足（115）" to "清理 /data（预留解压后 2 倍体积）后重试"
        E_ID_CONFLICT -> "模块 ID 冲突（116）" to "先卸载同 ID 模块，或修改包内 module.prop 的 id"
        E_INSTALL_SCRIPT -> "模块安装脚本执行失败（120）" to "这是模块自身脚本错误：查看原始 stderr/日志；可联系模块作者或换版本"
        E_UNPACK_IO -> "解包/IO 失败（121，含 booting 门禁）" to "确认设备已完整启动后重试；若刚刷机请等待系统就绪"
        E_STATE_WRITE -> "state 写入失败（122）" to "AP 状态文件不可写：到「设置 → AP 部署」重新部署，或检查 /data 权限"
        E_TIMEOUT -> "操作超时（124）" to "大模块可能较慢：确认空间后重试；持续超时上报日志"
        E_ESCALATION -> "密钥无效或被内核静默拒绝（125）" to "重新「修补 → 刷入」；本 App 不自动重试"
        E_NOT_EXECUTABLE -> "AP 不可执行（126）" to "「设置 → AP 部署」重新部署"
        E_NOT_DEPLOYED -> "AP 不存在（127）" to "先完成 AP 部署"
        else -> null
    }
}
