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
 * 48 位 Root 密钥的本地保险箱。
 *
 * 硬性要求（对应旧版缺陷 ⑧"release 构建 SKKeyProvider 恒 null"）：
 *  - **不得** 依赖 BuildConfig.DEBUG / debug 源集；本文件在 debug/release 行为完全一致。
 *  - 密钥用 Android Keystore 中不可导出的 AES-256 密钥加密后存 SharedPreferences；
 *    取不到、解不开时**显式报错**并允许重建，绝不返回 null 让上层静默失败。
 *
 * 密钥格式遵循 SPROOT-DOC-C1 §2.2：48 字符 [A-Za-z0-9]，前 47 字符写入内核，
 * 前 16 字符即 /data 隐藏目录名前缀。
 */
class SecretStore private constructor(private val context: Context) {

    companion object {
        private const val TAG = "SecretStore"
        private const val PREFS = "sproot_secret"
        private const val KEY_CIPHERTEXT = "root_key_ciphertext"
        private const val KEY_IV = "root_key_iv"
        private const val KEYSTORE = "AndroidKeyStore"
        private const val KEY_ALIAS = "sproot_root_key_v1"
        const val KEY_LENGTH = 48
        private const val KEY_CHARSET = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789"

        @Volatile
        private var instance: SecretStore? = null

        fun get(context: Context): SecretStore =
            instance ?: synchronized(this) {
                instance ?: SecretStore(context.applicationContext).also { instance = it }
            }

        /** 生成 48 字符随机密钥（SecureRandom，字符集与 C1 契约一致） */
        fun generateKey(): String {
            val rnd = SecureRandom()
            val sb = StringBuilder(KEY_LENGTH)
            repeat(KEY_LENGTH) { sb.append(KEY_CHARSET[rnd.nextInt(KEY_CHARSET.length)]) }
            return sb.toString()
        }

        fun isValidKey(key: String): Boolean =
            key.length == KEY_LENGTH && key.all { it in KEY_CHARSET }
    }

    private val prefs = context.getSharedPreferences(PREFS, Context.MODE_PRIVATE)

    /** 返回已有密钥；没有则生成并持久化。任何异常向上抛，由调用方展示完整错误。 */
    @Synchronized
    fun getOrCreateRootKey(): String {
        readRootKey()?.let { return it }
        val fresh = generateKey()
        saveRootKey(fresh)
        AppLog.info(TAG, "已生成新的 48 位 Root 密钥（隐藏目录前缀=${fresh.take(16)}）")
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
            String(plain, Charsets.US_ASCII)
        }.onFailure {
            AppLog.error(TAG, "读取本地 Root 密钥失败（将需要重新生成）", it)
        }.getOrNull()
    }

    @Synchronized
    fun saveRootKey(key: String) {
        require(isValidKey(key)) { "密钥必须是 48 位 [A-Za-z0-9]，实际长度=${key.length}" }
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
