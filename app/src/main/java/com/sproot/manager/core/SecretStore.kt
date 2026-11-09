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

        /** 检测到历史 48 字符密钥（已按前 47 位使用），设置页据此提示重新生成 */
        @Volatile
        var legacy48Detected: Boolean = false
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
         * 归一化为**内核密钥**（47 字符）。
         *
         * 兼容历史数据：读到 48 字符旧密钥时取前 47 位并 warning（旧引擎写内核时也是前 47 位，
         * 因此大概率仍可提权；但旧镜像里具体嵌了哪 47 位取决于当时写入逻辑，建议重新生成 + 重刷）。
         */
        fun kernelKeyOf(raw: String): String {
            val trimmed = raw.trim()
            return when {
                trimmed.length == KEY_LENGTH && trimmed.all { it in KEY_CHARSET } -> trimmed
                trimmed.length == ENGINE_KEY_LENGTH && trimmed.all { it in KEY_CHARSET } -> {
                    legacy48Detected = true
                    AppLog.warn(
                        TAG,
                        "检测到历史 48 字符 Root 密钥（内核 ABI 只需 47 字符）→ 取前 47 位使用；" +
                            "建议重新生成密钥并重新修补/刷入镜像，避免旧镜像内嵌密钥与新密钥不一致",
                    )
                    trimmed.take(KEY_LENGTH)
                }
                else -> throw IllegalArgumentException(
                    "Root 密钥必须是 47 字符 [A-Za-z0-9]（内核 ABI 47+NUL），实际长度=${trimmed.length}",
                )
            }
        }

        /** 引擎 `--key` 入参：47 字符补到 48（末位填充位被引擎丢弃） */
        fun engineKeyOf(raw: String): String =
            kernelKeyOf(raw).padEnd(ENGINE_KEY_LENGTH, ENGINE_PAD)
    }

    private val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    /** 返回已有密钥；没有则生成并持久化。任何异常向上抛，由调用方展示完整错误。 */
    @Synchronized
    fun getOrCreateRootKey(): String {
        readRootKey()?.let { return it }
        val fresh = generateKey()
        saveRootKey(fresh)
        AppLog.info(TAG, "已生成新的 47 字符 Root 密钥（内核 ABI 47+NUL；隐藏目录前缀=${fresh.take(16)}）")
        return fresh
    }

    @Synchronized
    fun readRootKey(): String? {
        val cipherText = prefs.getString(KEY_CIPHERTEXT, null) ?: return null
        val iv = prefs.getString(KEY_IV, null) ?: return null
        return runCatching {
            val cipher = Cipher.getInstance("AES/GCM/NoPadding")
            cipher.init(
                Cipher.DECRYPT_MODE,
                secretKey(createIfMissing = true),
                GCMParameterSpec(128, Base64.decode(iv, Base64.NO_WRAP)),
            )
            val plain = cipher.doFinal(Base64.decode(cipherText, Base64.NO_WRAP))
            // 归一化为 47 字符内核密钥（历史 48 字符自动迁移并 warning）
            kernelKeyOf(String(plain, Charsets.US_ASCII))
        }.onFailure {
            AppLog.error(TAG, "读取本地 Root 密钥失败（将需要重新生成）", it)
        }.getOrNull()
    }

    @Synchronized
    fun saveRootKey(raw: String) {
        // 49+ 直接硬失败；48 字符旧值 warning 后取前 47（与 apd-dev 的 key_material 策略一致）
        val key = kernelKeyOf(raw)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, secretKey(createIfMissing = true))
        val encrypted = cipher.doFinal(key.toByteArray(Charsets.US_ASCII))
        prefs.edit()
            .putString(KEY_CIPHERTEXT, Base64.encodeToString(encrypted, Base64.NO_WRAP))
            .putString(KEY_IV, Base64.encodeToString(cipher.iv, Base64.NO_WRAP))
            .apply()
    }

    @Synchronized
    fun rotate(): String {
        val fresh = generateKey()
        saveRootKey(fresh)
        AppLog.warn(TAG, "Root 密钥已轮换；旧密钥修补的内核将无法提权，必须重新修补并刷入")
        return fresh
    }

    fun clear() {
        prefs.edit().clear().apply()
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
