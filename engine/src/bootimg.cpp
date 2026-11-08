// bootimg.cpp — SPRoot 内置 boot.img 解析/重打包实现（header v3/v4，见 bootimg.h）
//
// 字段布局依据 AOSP system/tools/mkbootimg/include/bootimg/bootimg.h：
//   v3: magic[8] + kernel_size + ramdisk_size + os_version + header_size +
//       reserved[4] + header_version + cmdline[1536]            = 1580B，page 4096
//   v4: v3 + signature_size(u32)                                 = 1584B
//   kernel 段起始 = page_size（header 占 1 页），ramdisk 紧随 kernel 页对齐之后
// AVB footer（镜像尾 64B，大端）：
//   magic[4]="AVBf" + version_major + version_minor + original_image_size(u64) +
//   vbmeta_offset(u64) + vbmeta_size(u64) + reserved[28]
// 注：魅族 21 实测 vbmeta_offset == original_image_size（vbmeta 紧接原始镜像尾部），
//     文件末尾 66 MB 为 fastboot 分区填充（全 0），保留原样即可。
#include "bootimg.h"

#include <cstring>
#include <cstdio>
#include <algorithm>

namespace bootimg {
namespace {

constexpr uint32_t kPageSize = 4096;
constexpr uint64_t kAvbFooterSize = 64;

uint32_t rd_u32_le(const char* p) {
    return static_cast<uint32_t>(static_cast<uint8_t>(p[0])) |
           (static_cast<uint32_t>(static_cast<uint8_t>(p[1])) << 8) |
           (static_cast<uint32_t>(static_cast<uint8_t>(p[2])) << 16) |
           (static_cast<uint32_t>(static_cast<uint8_t>(p[3])) << 24);
}

uint32_t rd_u32_be(const char* p) {
    return (static_cast<uint32_t>(static_cast<uint8_t>(p[0])) << 24) |
           (static_cast<uint32_t>(static_cast<uint8_t>(p[1])) << 16) |
           (static_cast<uint32_t>(static_cast<uint8_t>(p[2])) << 8) |
           static_cast<uint32_t>(static_cast<uint8_t>(p[3]));
}

uint64_t rd_u64_be(const char* p) {
    uint64_t v = 0;
    for (int i = 0; i < 8; ++i) v = (v << 8) | static_cast<uint8_t>(p[i]);
    return v;
}

uint64_t align_up(uint64_t v, uint64_t a) { return (v + a - 1) / a * a; }

}  // namespace

const char* compression_name(Compression c) {
    switch (c) {
        case Compression::Raw: return "raw";
        case Compression::Gzip: return "gzip";
        case Compression::Lz4Legacy: return "lz4(legacy)";
        case Compression::Lz4Frame: return "lz4(frame)";
        default: return "unknown";
    }
}

bool read_info(const std::vector<char>& buf, Info& info) {
    info = Info{};
    if (buf.size() < 64) { info.error = "file too small (<64B)"; return false; }
    if (std::memcmp(buf.data(), "ANDROID!", 8) != 0) {
        info.error = "not an Android boot image (magic != ANDROID!)";
        return false;
    }
    info.page_size = kPageSize;   // v3/v4 固定 4096
    info.kernel_size = rd_u32_le(buf.data() + 8);
    info.ramdisk_size = rd_u32_le(buf.data() + 12);
    info.header_size = rd_u32_le(buf.data() + 20);
    info.header_version = rd_u32_le(buf.data() + 40);
    if (info.header_version < 3) {
        info.error = "only boot header v3/v4 supported (got v" +
                     std::to_string(info.header_version) + ")";
        return false;
    }
    if (info.header_version == 4) {
        info.signature_size = rd_u32_le(buf.data() + 1580);
    }
    info.kernel_offset = info.page_size;
    if (info.kernel_size == 0) { info.error = "kernel_size == 0"; return false; }
    if (info.kernel_offset + info.kernel_size > buf.size()) {
        info.error = "kernel_size exceeds file size";
        return false;
    }
    info.ramdisk_offset = info.kernel_offset + align_up(info.kernel_size, info.page_size);

    // kernel 压缩格式识别（前 4 字节）
    const char* k = buf.data() + info.kernel_offset;
    if (static_cast<uint8_t>(k[0]) == 0x1f && static_cast<uint8_t>(k[1]) == 0x8b) {
        info.compression = Compression::Gzip;
    } else if (rd_u32_le(k) == 0x184c2102u) {
        info.compression = Compression::Lz4Legacy;
    } else if (rd_u32_le(k) == 0x184d2204u) {
        info.compression = Compression::Lz4Frame;
    } else if (static_cast<uint8_t>(k[0]) == 0x4d && static_cast<uint8_t>(k[1]) == 0x5a) {
        // ARM64 Image header 以 "MZ"（code0 = b <...>）开头
        info.compression = Compression::Raw;
    } else {
        // 其余一律按 raw 处理（不同 SoC 的 Image 头可能不同，但 SK 引擎只需要
        // 完整 Image 字节；压缩才必须解压）
        info.compression = Compression::Raw;
    }

    // AVB footer（大端 64B）
    if (buf.size() >= kAvbFooterSize) {
        const char* f = buf.data() + buf.size() - kAvbFooterSize;
        if (std::memcmp(f, "AVBf", 4) == 0) {
            info.avb_footer = true;
            info.original_image_size = rd_u64_be(f + 12);
            info.vbmeta_offset = rd_u64_be(f + 20);
            info.vbmeta_size = rd_u64_be(f + 28);
        }
    }
    info.valid = true;
    return true;
}

bool extract_kernel(const std::vector<char>& buf, const Info& info,
                    std::vector<char>& kernel, std::string& err) {
    if (!info.valid) { err = "boot image info invalid"; return false; }
    if (info.compression != Compression::Raw) {
        err = std::string("kernel is ") + compression_name(info.compression) +
              "-compressed; SPRoot 引擎只接受未压缩 arm64 Image —— 需先解压"
              "（gzip: `gzip -dc`；lz4: `lz4 -d`），或等待内置解压支持";
        return false;
    }
    kernel.assign(buf.begin() + static_cast<long>(info.kernel_offset),
                  buf.begin() + static_cast<long>(info.kernel_offset + info.kernel_size));
    return true;
}

bool repack(const std::vector<char>& orig, const Info& info,
            const std::vector<char>& new_kernel, bool strip_avb,
            std::vector<char>& out, std::string& err) {
    if (!info.valid) { err = "boot image info invalid"; return false; }
    if (new_kernel.size() != info.kernel_size) {
        // SK 修补是等长字节 patch（C1 §2.1）；长度变化说明上游出错，拒绝落盘
        err = "kernel size mismatch: expected " + std::to_string(info.kernel_size) +
              ", got " + std::to_string(new_kernel.size()) +
              "（SK 修补必须等长，检查是否用了别的引擎）";
        return false;
    }
    out = orig;
    std::memcpy(out.data() + info.kernel_offset, new_kernel.data(), new_kernel.size());

    if (strip_avb && info.avb_footer) {
        // 清零 [original_image_size, EOF)：去 AVBf footer、内嵌 vbmeta 与分区填充
        uint64_t keep = info.original_image_size;
        if (keep == 0 || keep > out.size()) keep = out.size() - kAvbFooterSize;
        out.resize(static_cast<size_t>(keep));
        // 同时清掉末尾 64B（resize 已截断，若 original_image_size 不含 padding 亦安全）
    }
    return true;
}

std::string describe(const Info& i) {
    char b[1024];
    std::snprintf(b, sizeof(b),
                  "header_version=%u header_size=%u page_size=%u\n"
                  "kernel: offset=%llu size=%llu compression=%s\n"
                  "ramdisk: offset=%llu size=%llu\n"
                  "signature_size=%llu\n"
                  "avb: footer=%s original_image_size=%llu vbmeta_offset=%llu vbmeta_size=%llu\n",
                  i.header_version, i.header_size, i.page_size,
                  static_cast<unsigned long long>(i.kernel_offset),
                  static_cast<unsigned long long>(i.kernel_size),
                  compression_name(i.compression),
                  static_cast<unsigned long long>(i.ramdisk_offset),
                  static_cast<unsigned long long>(i.ramdisk_size),
                  static_cast<unsigned long long>(i.signature_size),
                  i.avb_footer ? "yes" : "no",
                  static_cast<unsigned long long>(i.original_image_size),
                  static_cast<unsigned long long>(i.vbmeta_offset),
                  static_cast<unsigned long long>(i.vbmeta_size));
    return b;
}

}  // namespace bootimg
