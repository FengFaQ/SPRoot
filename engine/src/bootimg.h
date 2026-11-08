// bootimg.h — SPRoot 内置 boot.img 解析/重打包（不依赖 magiskboot/kptools）
//
// 决策依据（lead 2026-11-08）：学 KernelSU 的 ksud 自研 bootimg 处理，去掉旧链路
// 「外部件缺失静默失败」的根因（见 NEW-DOC-03 R1/F1）。SK 只 patch kernel 段，
// **不需要改 ramdisk**（与 KSU 的 init 替换不同）。
//
// 支持范围：
//   - boot header v3 / v4（page_size 固定 4096；v4 追加 signature_size）
//   - kernel 段：**未压缩 raw arm64 Image**（魅族 21 实测为 raw，见 samples/meizu21）
//   - gzip / lz4 压缩内核：明确识别并返回可读错误（压缩内核需先解压；后续vendor
//     inflate/lz4 后开放，见 NEW-DOC-02「bootimg 与刷入链」）
//   - AVB：识别镜像尾部 AVBf footer 与内嵌 vbmeta；repack 支持保留（默认）或剥离
//
// 不做：ramdisk 修改、dtb/dtb1/dtb2、vendor_boot、v4 boot signature 重签。
#pragma once

#include <cstdint>
#include <string>
#include <vector>

namespace bootimg {

enum class Compression { Raw, Gzip, Lz4Legacy, Lz4Frame, Unknown };

struct Info {
    bool valid = false;
    uint32_t header_version = 0;   // 3 / 4
    uint32_t header_size = 0;
    uint32_t page_size = 4096;
    uint64_t kernel_size = 0;
    uint64_t ramdisk_size = 0;
    uint64_t kernel_offset = 0;    // = page_size（header 占 1 页）
    uint64_t ramdisk_offset = 0;
    uint64_t signature_size = 0;   // v4 才有
    Compression compression = Compression::Unknown;

    // AVB
    bool avb_footer = false;
    uint64_t original_image_size = 0;  // AVBf footer 记录的「原始镜像长度」
    uint64_t vbmeta_offset = 0;        // 内嵌 vbmeta 偏移（boot.img 尾部区）
    uint64_t vbmeta_size = 0;

    std::string error;
};

const char* compression_name(Compression c);

// 解析 boot header；失败时返回 false 并填 info.error
bool read_info(const std::vector<char>& buf, Info& info);

// 抽取 kernel 段（仅支持 Raw；压缩内核返回 false 且 error 说明格式）
bool extract_kernel(const std::vector<char>& buf, const Info& info,
                    std::vector<char>& kernel, std::string& err);

// 用新的 kernel 字节重打包 boot.img。
//   - 要求 new_kernel.size() == info.kernel_size（SK 修补是等长字节 patch）
//   - strip_avb=true：把 [original_image_size, EOF) 区间清零（去 AVB footer/内嵌
//     vbmeta），并把文件截到 original_image_size（刷入后交给 vbmeta 侧 disable-verification）
//   - strip_avb=false：保留原 AVB 区（hash 已失效，需配合 vbmeta 关闭校验）
bool repack(const std::vector<char>& orig, const Info& info,
            const std::vector<char>& new_kernel, bool strip_avb,
            std::vector<char>& out, std::string& err);

// 生成「可由人读的 info 文本」（tools 用，不参与 patch 主流程）
std::string describe(const Info& info);

}  // namespace bootimg
