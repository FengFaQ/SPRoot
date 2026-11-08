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

    /** AVB footer 固定长度（64 B，位于镜像最后 64 字节） */
    const val AVB_FOOTER_SIZE = 64

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

    /**
     * AVB footer 结构（Android AVB 定长 64 B，位于镜像最后 64 字节）。
     *
     * 注意：AVB 的整数字段全部是**大端（big-endian）**（avbtool 用
     * `be32toh`/`be64toh` 序列化，见 avb_footer 校验函数）：
     * ```
     * offset  0  magic[4] = "AVBf"
     * offset  4  version_major       (u32, BE)
     * offset  8  version_minor       (u32, BE)
     * offset 12  original_image_size (u64, BE)
     * offset 20  vbmeta_offset       (u64, BE)
     * offset 28  vbmeta_size         (u64, BE)
     * offset 36  reserved[28]
     * ```
     * **vbmeta_offset 是分区镜像内的绝对偏移**（不是「距文件尾的距离」）：
     * avbtool 的 `extract_vbmeta_image()` 直接 `image.seek(footer.vbmeta_offset)`，
     * `resize_image()` 也以 `vbmeta_offset + vbmeta_size` 作为 vbmeta 末端，footer 永远在
     * 最后一个 block。实测魅族 21 boot.img：`vbmeta_offset=33,898,496`、`vbmeta_size=2,496`、
     * 文件长 100,663,296 → vbmeta 实际就在 33,898,496（紧接原始镜像尾部），
     * 其后是分区零填充，最后一 64 B 才是 footer。因此内嵌 vbmeta 区段 =
     * `[vbmeta_offset, vbmeta_offset + vbmeta_size)`。
     */
    data class AvbFooter(
        val magic: String,
        val versionMajor: Long,
        val versionMinor: Long,
        val originalImageSize: Long,
        val vbmetaOffset: Long,
        val vbmetaSize: Long,
        val fileLength: Long,
    ) {
        /** 字段非法原因（null = 可定位）；AVB-3 降级 WARN 时必须写明原因，不得静默跳过。 */
        fun vbmetaProblem(): String? {
            if (vbmetaOffset <= 0L) return "vbmeta_offset=$vbmetaOffset（非法：为 0 或负数）"
            if (vbmetaSize <= 0L) return "vbmeta_size=$vbmetaSize（非法：为 0）"
            val end = vbmetaOffset + vbmetaSize
            if (vbmetaOffset < 0L || end > fileLength) {
                return "vbmeta 区段越界（offset=$vbmetaOffset, size=$vbmetaSize, len=$fileLength）"
            }
            if (end > fileLength - Elf.AVB_FOOTER_SIZE) {
                return "vbmeta 区段与 AVB footer 重叠（end=$end, footerStart=${fileLength - Elf.AVB_FOOTER_SIZE}）"
            }
            return null
        }

        /** vbmeta 区段在分区镜像内的绝对起点；非法时返回 null（原因见 [vbmetaProblem]） */
        fun vbmetaStart(): Long? {
            if (vbmetaProblem() != null) return null
            return vbmetaOffset
        }
    }

    fun readAvbFooter(file: File): AvbFooter? {
        val len = file.length()
        if (len < 64L) return null
        val buf = ByteArray(64)
        val ok = runCatching {
            RandomAccessFile(file, "r").use { raf ->
                raf.seek(len - 64L)
                raf.read(buf) == 64
            }
        }.getOrDefault(false)
        if (!ok) return null
        val magic = String(buf, 0, 4, Charsets.US_ASCII)
        if (magic != "AVBf") return null

        fun u32be(off: Int): Long =
            ((buf[off].toLong() and 0xFF) shl 24) or
                ((buf[off + 1].toLong() and 0xFF) shl 16) or
                ((buf[off + 2].toLong() and 0xFF) shl 8) or
                (buf[off + 3].toLong() and 0xFF)

        fun u64be(off: Int): Long = (u32be(off) shl 32) or u32be(off + 4)

        return AvbFooter(
            magic = magic,
            versionMajor = u32be(4),
            versionMinor = u32be(8),
            originalImageSize = u64be(12),
            vbmetaOffset = u64be(20),
            vbmetaSize = u64be(28),
            fileLength = len,
        )
    }

    /**
     * 比对两个文件中同一区段是否逐字节相同。
     * @return null = 相同；否则返回**首个差异的文件绝对偏移**
     */
    fun firstDiffOffset(a: File, b: File, offset: Long, size: Long): Long? {
        if (offset < 0L || size <= 0L) return offset
        return RandomAccessFile(a, "r").use { ra ->
            RandomAccessFile(b, "r").use { rb ->
                if (ra.length() < offset + size || rb.length() < offset + size) {
                    offset
                } else {
                    ra.seek(offset)
                    rb.seek(offset)
                    val bufA = ByteArray(64 * 1024)
                    val bufB = ByteArray(64 * 1024)
                    var consumed = 0L
                    var diff: Long? = null
                    while (consumed < size && diff == null) {
                        val want = minOf(size - consumed, bufA.size.toLong()).toInt()
                        val readA = ra.read(bufA, 0, want)
                        val readB = rb.read(bufB, 0, want)
                        if (readA <= 0 || readA != readB) {
                            diff = offset + consumed
                        } else {
                            var i = 0
                            while (i < readA) {
                                if (bufA[i] != bufB[i]) {
                                    diff = offset + consumed + i
                                    break
                                }
                                i++
                            }
                            consumed += readA
                        }
                    }
                    diff
                }
            }
        }
    }
}
