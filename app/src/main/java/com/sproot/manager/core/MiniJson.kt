package com.sproot.manager.core

/**
 * 极简 JSON 取值器。
 *
 * 为什么不引 kotlinx.serialization：修补引擎的 NDJSON 事件流是**契约固定**的扁平对象
 * （见 SPROOT-DOC-C1 §2.3），一个 100 行的手写解析器就够；少一个编译插件 = CI 少一类失败。
 * 解析失败一律返回 null，由调用方记录"原始行"原文，绝不静默丢弃。
 */
object MiniJson {

    fun string(json: String, key: String): String? {
        val idx = indexOfKey(json, key) ?: return null
        var i = skipWs(json, idx)
        if (i >= json.length || json[i] != ':') return null
        i = skipWs(json, i + 1)
        if (i >= json.length) return null
        return if (json[i] == '"') readString(json, i + 1) else readBare(json, i)
    }

    fun int(json: String, key: String): Int? = number(json, key)?.toInt()

    fun long(json: String, key: String): Long? = number(json, key)?.toLong()

    fun double(json: String, key: String): Double? = number(json, key)

    private fun number(json: String, key: String): Double? {
        val idx = indexOfKey(json, key) ?: return null
        var i = skipWs(json, idx)
        if (i >= json.length || json[i] != ':') return null
        i = skipWs(json, i + 1)
        val raw = readBare(json, i) ?: return null
        return raw.toDoubleOrNull()
    }

    /** 找不到 key 返回 null；找到返回 value 起始下标前的冒号位置探测点 */
    private fun indexOfKey(json: String, key: String): Int? {
        val needle = "\"$key\""
        var from = 0
        while (true) {
            val p = json.indexOf(needle, from)
            if (p < 0) return null
            // 必须是对象键：前面跳过空白后应为 '{' 或 ','
            var j = p - 1
            while (j >= 0 && json[j].isWhitespace()) j--
            if (j < 0 || json[j] == '{' || json[j] == ',') return p + needle.length
            from = p + needle.length
        }
    }

    private fun skipWs(s: String, start: Int): Int {
        var i = start
        while (i < s.length && s[i].isWhitespace()) i++
        return i
    }

    private fun readString(s: String, start: Int): String? {
        val sb = StringBuilder()
        var i = start
        while (i < s.length) {
            val c = s[i]
            when {
                c == '"' -> return sb.toString()
                c == '\\' && i + 1 < s.length -> {
                    when (val n = s[i + 1]) {
                        'n' -> sb.append('\n')
                        't' -> sb.append('\t')
                        'r' -> sb.append('\r')
                        'b' -> sb.append('\b')
                        'f' -> sb.append('\u000C')
                        '"' -> sb.append('"')
                        '\\' -> sb.append('\\')
                        '/' -> sb.append('/')
                        'u' -> {
                            if (i + 5 < s.length) {
                                val hex = s.substring(i + 2, i + 6)
                                hex.toIntOrNull(16)?.let { sb.append(it.toChar()) }
                                i += 4
                            }
                        }
                        else -> sb.append(n)
                    }
                    i += 2
                }
                else -> {
                    sb.append(c)
                    i++
                }
            }
        }
        return null
    }

    private fun readBare(s: String, start: Int): String? {
        var i = start
        while (i < s.length && s[i] != ',' && s[i] != '}' && s[i] != ']' &&
            !s[i].isWhitespace() && s[i] != '"'
        ) i++
        if (i == start) return null
        val raw = s.substring(start, i)
        return raw.ifBlank { null }
    }

    /**
     * 解析「对象数组」为 Map 列表（本项目够用的最小实现）。
     *
     * 用于 `apd module list --json` 与 `apd selftest --json`：
     * - 支持 `[{...},{...}]`；
     * - 每个对象只取字符串/数字/布尔标量字段（嵌套对象忽略，不报错）；
     * - 解析失败返回空列表，由调用方把**原始输出**完整展示（绝不静默丢数据）。
     */
    fun arrayOfObjects(json: String?): List<Map<String, String>> {
        if (json.isNullOrBlank()) return emptyList()
        val start = json.indexOf('[')
        if (start < 0) return emptyList()
        val result = mutableListOf<Map<String, String>>()
        var i = start + 1
        while (i < json.length) {
            when {
                json[i] == ']' -> return result
                json[i] == '{' -> {
                    val end = findObjectEnd(json, i)
                    if (end < 0) return result
                    result += parseFlatObject(json.substring(i + 1, end))
                    i = end + 1
                }
                else -> i++
            }
        }
        return result
    }

    /** 解析单个扁平对象（也用于 `apd selftest --json` 这类顶层对象） */
    fun objectOf(json: String?): Map<String, String> {
        if (json.isNullOrBlank()) return emptyMap()
        val start = json.indexOf('{')
        if (start < 0) return emptyMap()
        val end = findObjectEnd(json, start)
        if (end < 0) return emptyMap()
        return parseFlatObject(json.substring(start + 1, end))
    }

    private fun findObjectEnd(s: String, openIndex: Int): Int {
        var depth = 0
        var inString = false
        var escaped = false
        for (i in openIndex until s.length) {
            val c = s[i]
            when {
                escaped -> escaped = false
                inString && c == '\\' -> escaped = true
                c == '"' -> inString = !inString
                inString -> Unit
                c == '{' -> depth++
                c == '}' -> {
                    depth--
                    if (depth == 0) return i
                }
            }
        }
        return -1
    }

    private fun parseFlatObject(body: String): Map<String, String> {
        val map = mutableMapOf<String, String>()
        val pairs = splitTopLevel(body)
        pairs.forEach { pair ->
            val idx = pair.indexOf(':')
            if (idx <= 0) return@forEach
            val key = pair.substring(0, idx).trim().trim('"')
            val rawValue = pair.substring(idx + 1).trim()
            if (rawValue.startsWith("{") || rawValue.startsWith("[")) return@forEach
            map[key] = when {
                rawValue.startsWith("\"") -> readString(rawValue, 1) ?: ""
                else -> rawValue.trim('"')
            }
        }
        return map
    }

    private fun splitTopLevel(body: String): List<String> {
        val out = mutableListOf<String>()
        var inString = false
        var escaped = false
        var depth = 0
        val sb = StringBuilder()
        body.forEach { c ->
            when {
                escaped -> { sb.append(c); escaped = false }
                inString && c == '\\' -> { sb.append(c); escaped = true }
                c == '"' -> { inString = !inString; sb.append(c) }
                inString -> sb.append(c)
                c == '{' || c == '[' -> { depth++; sb.append(c) }
                c == '}' || c == ']' -> { depth--; sb.append(c) }
                c == ',' && depth == 0 -> { out += sb.toString(); sb.clear() }
                else -> sb.append(c)
            }
        }
        if (sb.isNotBlank()) out += sb.toString()
        return out
    }
}
