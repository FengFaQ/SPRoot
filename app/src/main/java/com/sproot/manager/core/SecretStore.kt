package com.sproot.manager.core

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import android.util.Log
import java.io.File
import java.security.KeyStore
import java.security.SecureRandom
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * Root 密钥的本地保险箱。
 *
 * 硬性要求（对应旧版缺陷 ⑧"release 构建 SKKeyProvider 恒 null"）：
 *  - **不得** 依赖 BuildConfig.DEBUG / debug 源集；本文件在 debug/release 行为完全一致。
 *  - 密钥用 Android Keystore 中不可导出的 AES-256 密钥加密后存 SharedPreferences；
 *    取不到、解不开时**显式报错**并允许重建，绝不返回 null 让上层静默失败。
 *
 * ## 密钥长度口径（两个 ABI 必须区分，混用会导致提权静默失败）
 *
 *  - **内核 ABI = 47 字符 + NUL**：引擎 `embed_key` 写进内核的是 `key[0:47] + '\0'`；
 *    apd 写 `/data/adb/sproot/key` 的也是 47 字节纯文本（`defs::SPROOT_KEY_FILE`）；
 *    内核拿这 47 字符做比对。
 *  - **引擎 `--key` 入参 ABI = 48 字符**：引擎先 `valid_key(size==48)`，再
 *    `write_key.erase(size-1)` 后写入 47+NUL（engine/src/skroot_patch_main.cpp）。
 *    即引擎要求 48 字符入参并**丢弃第 48 位**。
 *
 * 因此本项目统一口径：**本地存储 / apd 传输 / 内核比对一律 47 字符**；
 * 只在调用引擎时用 [engineKeyOf] 把 47 补到 48（末位填充位会被引擎丢弃）。
 * 展示与日志都写「47 字符（内核 ABI 47+NUL）」，不再出现"48 位密钥"的误导口径。
 */
class SecretStore private constructor(private val context: Context) {

    companion object {
        private const val TAG = "SecretStore"
        private const val PREFS = "sproot_secret"
        private const val KEY_CIPHERTEXT = "root_key_ciphertext"
        private const val KEY_IV = "root_key_iv"
        private const val KEYSTORE = "AndroidKeyStore"
        private const val KEY_ALIAS = "sproot_root_key_v1"
        /** 内核 ABI：47 字符 + NUL（存储/传输/内核比对口径） */
        const val KEY_LENGTH = 47

        /** 引擎 `--key` 入参 ABI：48 字符（引擎会丢弃第 48 位） */
        const val ENGINE_KEY_LENGTH = 48

        /** 补齐引擎入参用的填充位（必被引擎丢弃，不参与内核密钥） */
        private const val ENGINE_PAD = 'A'

        private const val KEY_CHARSET = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789"

        /** 本次进程内发生过"旧格式 48 字符密钥 → 自动迁移为 47"（用于一次性提示） */
        @Volatile
        var migratedFromLegacy: Boolean = false
            private set

        @Volatile
        private var instance: SecretStore? = null

        fun get(context: Context): SecretStore =
            instance ?: synchronized(this) {
                instance ?: SecretStore(context.applicationContext).also { instance = it }
            }

        /** 生成 47 字符随机内核密钥（SecureRandom；字符集与 C1 契约一致） */
        fun generateKey(): String {
            val rnd = SecureRandom()
            val sb = StringBuilder(KEY_LENGTH)
            repeat(KEY_LENGTH) { sb.append(KEY_CHARSET[rnd.nextInt(KEY_CHARSET.length)]) }
            return sb.toString()
        }

        fun isValidKey(key: String): Boolean =
            key.length == KEY_LENGTH && key.all { it in KEY_CHARSET }

        /**
         * 归一化为**内核密钥**：只接受 47 字符 [A-Za-z0-9]。
         *
         * 人类要求（2026-11-10）：**一机一份、固定不变**，不做"历史 48 位兼容"；
         * 48 字符旧值由 [SecretStore.readRootKey] 一次性迁移（自动重新生成 47 字符）。
         */
        fun kernelKeyOf(raw: String): String {
            val trimmed = raw.trim()
            require(isValidKey(trimmed)) {
                "Root 密钥必须是 47 字符 [A-Za-z0-9]（内核 ABI 47+NUL），实际长度=${trimmed.length}"
            }
            return trimmed
        }

        /** 引擎 `--key` 入参：47 字符补到 48（末位填充位被引擎丢弃） */
        fun engineKeyOf(raw: String): String =
            kernelKeyOf(raw).padEnd(ENGINE_KEY_LENGTH, ENGINE_PAD)

        /**
         * 把**引擎侧可能返回的**密钥归一到内核 ABI 的 47 字符。
         *
         * 为什么需要它（2026-11-11 真机事故）：引擎 `report.root_key` 回传的是
         * **调引擎时传入的 48 字符**（47 字符 + 1 位填充）。客户端拿到它再交给
         * [kernelKeyOf] 会因长度 48 触发 `require(isValidKey)` 抛异常 —— 表现为
         * 「产物已成功导出，但状态机置 ERROR」的假失败。
         *
         * 归一规则（与引擎 `embed_key_bytes()` 逐字节口径一致：内核只存前 47 位）：
         * - 48 字符且前 47 位合法 → 取前 47（第 48 位是填充位，本就该丢）
         * - 47 字符合法 → 原样返回
         * - 其它 → 交给 [kernelKeyOf] 报错（不做静默猜测）
         */
        fun normalizeEngineKey(raw: String): String {
            val trimmed = raw.trim()
            if (trimmed.length == ENGINE_KEY_LENGTH) {
                val head47 = trimmed.substring(0, ENGINE_KEY_LENGTH - 1)
                if (isValidKey(head47)) return head47
            }
            return kernelKeyOf(trimmed)
        }

        /**
         * 密钥指纹：sha256(密钥前 16 字符) 的 lowercase hex。
         *
         * 口径必须与 apd `version --json` 的 `key_prefix_sha256`（cli.rs print_version：
         * `sha256_hex_stdin(key 前 16 字符)`）**逐字节一致**——两边字符串直接比较，
         * 供 KeyMatch 三方对账（修补时密钥 A / 当前密钥 B / 已部署 key 文件 C）。
         * 只含 sha256 摘要、不含密钥本体或片段，写日志/持久化无泄露面
         * （与 apd 契约 "never the key or any fragment" 对齐）。
         */
        fun fingerprintOf(key: String): String {
            val digest = java.security.MessageDigest.getInstance("SHA-256")
                .digest(key.take(16).toByteArray(Charsets.UTF_8))
            return digest.joinToString("") { "%02x".format(it) }
        }
    }

    private val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    /**
     * 进程内缓存（人类 2026-11-10 真机反馈：一次启动读了 5 次密钥、warn 5 次）。
     * 密钥一机一份固定不变，读一次即可；[saveRootKey]/[rotate]/[clear] 会同步更新缓存。
     */
    @Volatile
    private var cachedRootKey: String? = null

    /** 返回已有密钥；没有则生成并持久化。任何异常向上抛，由调用方展示完整错误。 */
    @Synchronized
    fun getOrCreateRootKey(): String {
        cachedRootKey?.let { return it }
        readRootKey()?.let { return it }
        val fresh = generateKey()
        saveRootKey(fresh)
        AppLog.info(TAG, "已生成新的 47 字符 Root 密钥（内核 ABI 47+NUL；隐藏目录前缀=${fresh.take(16)}；指纹=${fingerprintOf(fresh)}）")
        return fresh
    }

    /**
     * 读取内核密钥（**只认 47 字符**；进程内缓存）。
     *
     * - 合法 47 字符 → 直接返回并缓存；
     * - 旧格式 48 字符 → **一次性自动迁移**：重新生成 47 字符并标记 [migratedFromLegacy]
     *   （人类要求"密钥一机一份固定不变"，不再取前 47 位凑合）；
     * - 其它长度/解密失败 → 清除密文并返回 null（调用方 getOrCreate 会重新生成）。
     */
    @Synchronized
    fun readRootKey(): String? {
        cachedRootKey?.let { return it }
        val cipherText = prefs.getString(KEY_CIPHERTEXT, null) ?: return null
        val iv = prefs.getString(KEY_IV, null) ?: return null
        val raw = runCatching {
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(
                Cipher.DECRYPT_MODE,
                secretKey(createIfMissing = true),
                GCMParameterSpec(128, Base64.decode(iv, Base64.NO_WRAP)),
            )
            val plain = cipher.doFinal(Base64.decode(cipherText, Base64.NO_WRAP))
            String(plain, Charsets.US_ASCII).trim()
        }.onFailure {
            AppLog.error(TAG, "读取本地 Root 密钥失败（将重新生成）", it)
        }.getOrNull() ?: return null

        return when {
            isValidKey(raw) -> raw.also { cachedRootKey = it }
            raw.length == ENGINE_KEY_LENGTH && raw.all { it in KEY_CHARSET } -> migrateLegacy(raw)
            else -> {
                AppLog.warn(TAG, "本地密钥格式无效（长度=${raw.length}）→ 清除并重新生成")
                clearCiphertext()
                null
            }
        }
    }

    /** 旧格式（48 字符）一次性迁移：重新生成 47 字符、写回、标记（不再每次启动警告）。 */
    private fun migrateLegacy(old: String): String {
        val fresh = generateKey()
        saveRootKey(fresh)
        migratedFromLegacy = true
        // 一次性标记：以后启动不再重复提示（人类要求：只提示一次）
        prefs.edit().putBoolean("key_migrated_v47", true).apply()
        AppLog.warn(
            TAG,
            "检测到旧格式密钥（48 字符），已重新生成 47 字符密钥；" +
                "若你已修补过镜像，请重新修补并刷入（旧镜像内嵌的是旧密钥；新密钥指纹=${fingerprintOf(fresh)}）",
        )
        return fresh
    }

    @Synchronized
    fun saveRootKey(raw: String) {
        val key = kernelKeyOf(raw)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, secretKey(createIfMissing = true))
        val encrypted = cipher.doFinal(key.toByteArray(Charsets.US_ASCII))
        prefs.edit()
            .putString(KEY_CIPHERTEXT, Base64.encodeToString(encrypted, Base64.NO_WRAP))
            .putString(KEY_IV, Base64.encodeToString(cipher.iv, Base64.NO_WRAP))
            .apply()
        cachedRootKey = key
    }

    @Synchronized
    fun rotate(): String {
        val fresh = generateKey()
        saveRootKey(fresh)
        AppLog.warn(TAG, "Root 密钥已轮换；旧密钥修补的内核将无法提权，必须重新修补并刷入（新密钥指纹=${fingerprintOf(fresh)}）")
        return fresh
    }

    fun clear() {
        prefs.edit().clear().apply()
        cachedRootKey = null
    }

    private fun clearCiphertext() {
        prefs.edit().remove(KEY_CIPHERTEXT).remove(KEY_IV).apply()
        cachedRootKey = null
    }

    /** 导出为可分享的明文文件（用户显式操作时才调用；UI 默认脱敏展示） */
    fun exportToFile(target: File, key: String) {
        target.parentFile?.mkdirs()
        target.writeText(key + "\n")
    }

    private fun secretKey(createIfMissing: Boolean): SecretKey {
        val keyStore = KeyStore.getInstance(KEYSTORE).apply { load(null) }
        (keyStore.getEntry(KEY_ALIAS, null) as? KeyStore.SecretKeyEntry)?.let { return it.secretKey }
        if (!createIfMissing) throw IllegalStateException("Keystore 中不存在 $KEY_ALIAS")
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, KEYSTORE)
        generator.init(
            KeyGenParameterSpec.Builder(
                KEY_ALIAS,
                KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
            )
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .setUserAuthenticationRequired(false)
                .build(),
        )
        val created = generator.generateKey()
        Log.i(TAG, "created keystore key $KEY_ALIAS")
        return created
    }
}
