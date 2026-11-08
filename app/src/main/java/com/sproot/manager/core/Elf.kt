package com.sproot.manager.core

import java.io.File
import java.io.RandomAccessFile

/**
 * ELF / 压缩格式探测。
 *
 * 全部走「读文件头字节」的静态判断，不 exec、不依赖 file(1)。
 * 用途：①入口能力自检（引擎与 magiskboot 是否 AArch64 ELF）②内核是否仍被压缩。
 */
object Elf {

    /** AArch64 的 ELF e_machine 值 */
    const val EM_AARCH64 = 183
    const val EM_ARM = 40
    const val EM_X86_64 = 62

    data class Header(
        val isElf: Boolean,
        val is64Bit: Boolean,
        val machine: Int,
        val machineName: String,
    )

    fun readHeader(path: String): Header? = runCatching {
        RandomAccessFile(path, "r").use { raf ->
            val buf = ByteArray(20)
            val read = raf.read(buf)
            if (read < 20) return null
            val isElf = buf[0] == 0x7F.toByte() &&
                buf[1] == 'E'.code.toByte() &&
                buf[2] == 'L'.code.toByte() &&
                buf[3] == 'F'.code.toByte()
            val is64 = buf[4].toInt() == 2
            // e_machine 位于 offset 18，小端 16 位
            val machine = (buf[18].toInt() and 0xFF) or ((buf[19].toInt() and 0xFF) shl 8)
            Header(isElf, is64, machine, machineName(machine))
        }
    }.getOrNull()

    fun isAarch64(path: String): Boolean {
        val h = readHeader(path) ?: return false
        return h.isElf && h.is64Bit && h.machine == EM_AARCH64
    }

    fun machineName(machine: Int): String = when (machine) {
        EM_AARCH64 -> "AArch64(arm64)"
        EM_ARM -> "ARM(32)"
        EM_X86_64 -> "x86-64"
        3 -> "x86"
        else -> "e_machine=$machine"
    }

    /** @return 压缩格式名（gzip/lz4/zstd/xz/bzip2/lzop），未压缩返回 null */
    fun compressionFormat(file: File): String? = runCatching {
        RandomAccessFile(file, "r").use { raf ->
            val buf = ByteArray(8)
            if (raf.read(buf) < 4) return null
            fun b(i: Int) = buf[i].toInt() and 0xFF
            when {
                b(0) == 0x1F && b(1) == 0x8B -> "gzip"
                b(0) == 0x04 && b(1) == 0x22 && b(2) == 0x4D && b(3) == 0x18 -> "lz4"
                b(0) == 0x02 && b(1) == 0x21 && b(2) == 0x4C && b(3) == 0x18 -> "lz4_legacy"
                b(0) == 0x28 && b(1) == 0xB5 && b(2) == 0x2F && b(3) == 0xFD -> "zstd"
                b(0) == 0xFD && b(1) == 0x37 && b(2) == 0x7A && b(3) == 0x58 -> "xz"
                b(0) == 0x42 && b(1) == 0x5A && b(2) == 0x68 -> "bzip2"
                b(0) == 0x89 && b(1) == 0x4C && b(2) == 0x5A && b(3) == 0x4F -> "lzop"
                else -> null
            }
        }
    }.getOrNull()

    /** Android boot image 魔数（boot_img_hdr_v0：offset 0 处 8 字节 "ANDROID!"） */
    fun isAndroidBootImage(file: File): Boolean = runCatching {
        RandomAccessFile(file, "r").use { raf ->
            val buf = ByteArray(8)
            if (raf.read(buf) < 8) return false
            String(buf, Charsets.US_ASCII) == "ANDROID!"
        }
    }.getOrDefault(false)

    /**
     * 检测 AVB footer（boot.img 尾部 64 字节结构，magic "AVBf"）。
     * 用途：重打包后确认 AVB footer 仍保留（不触碰 AVB 密码学，但需要它还在原位）。
     */
    fun hasAvbFooter(file: File): Boolean = runCatching {
        if (file.length() < 64) return false
        RandomAccessFile(file, "r").use { raf ->
            raf.seek(file.length() - 64)
            val buf = ByteArray(64)
            if (raf.read(buf) < 64) return false
            String(buf, 0, 4, Charsets.US_ASCII) == "AVBf"
        }
    }.getOrDefault(false)
}
