# SPRoot 修补引擎 (skroot-patch) 构建脚本
# 依据: SPROOT-DOC-C1 v1.0；引擎核心 vendor 自 SKRoot 上游 master@943382cd
# 3rdparty (asmjit/capstone/cJSON) 经 engine/src/3rdparty 引用上游原版
#
# 变量一律用 ?= ，便于 engine/android/build_arm64.sh 整体覆盖
# （CXX/CC/TARGET/OBJDIR/CXXFLAGS/CFLAGS/LDFLAGS/LIBS 均为可覆盖项）

CXX ?= g++
CC  ?= gcc
TARGET ?= skroot-patch
# 目标文件输出目录：本机构建用 "."（就地）；交叉编译用独立目录，避免
# arm64 与 x86-64 的 .o 互相覆盖造成脏产物
OBJDIR ?= .
LDFLAGS ?=

UPSTREAM_ENGINE := ../upstream/SKRoot-linuxKernelRoot/Lite_version/src/patch_kernel_root
UP3 := 3rdparty

INCLUDES := \
  -I. \
  -I$(UPSTREAM_ENGINE) \
  -I$(UP3)/capstone-5.0.7/include \
  -I$(UP3)/asmjit2-src/src \
  -I$(UP3) \
  -I$(UP3)/cJSON

DEFINES := \
  -DNDEBUG \
  -DCAPSTONE_HAS_ARM64 \
  -DCAPSTONE_USE_SYS_DYN_MEM

CXXFLAGS := -std=c++20 -O2 -Wall $(DEFINES) $(INCLUDES)
CFLAGS   := -O2 -Wall $(DEFINES) $(INCLUDES)
LIBS := -ldl -pthread

# SPRoot 层（新写）：CLI/pipeline + 内置 bootimg 解析/重打包
SPROOT_SRCS := \
  skroot_patch_main.cpp \
  patch_pipeline.cpp \
  bootimg.cpp

# 引擎核心（vendor 副本，已修上游 Linux 移植 bug: cstdint/cstring include）
ENGINE_SRCS := \
  analyze/kallsyms_lookup_name.cpp \
  analyze/kallsyms_lookup_name_4_6_0.cpp \
  analyze/kallsyms_lookup_name_6_1_0.cpp \
  analyze/kallsyms_lookup_name_6_1_42.cpp \
  analyze/kallsyms_lookup_name_6_1_60.cpp \
  analyze/kallsyms_lookup_name_6_4_0.cpp \
  analyze/kallsyms_lookup_name_6_12_0.cpp \
  analyze/init_cred_searcher.cpp \
  analyze/kernel_symbol_proxy.cpp \
  analyze/kernel_symbol_finder.cpp \
  analyze/kernel_version_parser.cpp \
  analyze/symbol_analyze.cpp \
  patch_current_avc_check.cpp \
  patch_avc_denied.cpp \
  patch_audit_log_start.cpp \
  patch_base.cpp \
  patch_do_execve.cpp \
  patch_filldir64.cpp \
  patch_compat_filldir.cpp

ASMJIT_SRCS := \
  $(UP3)/asmjit2-src/src/asmjit/arm/a64assembler.cpp \
  $(UP3)/asmjit2-src/src/asmjit/arm/a64builder.cpp \
  $(UP3)/asmjit2-src/src/asmjit/arm/a64compiler.cpp \
  $(UP3)/asmjit2-src/src/asmjit/arm/a64emithelper.cpp \
  $(UP3)/asmjit2-src/src/asmjit/arm/a64formatter.cpp \
  $(UP3)/asmjit2-src/src/asmjit/arm/a64func.cpp \
  $(UP3)/asmjit2-src/src/asmjit/arm/a64instapi.cpp \
  $(UP3)/asmjit2-src/src/asmjit/arm/a64instdb.cpp \
  $(UP3)/asmjit2-src/src/asmjit/arm/a64operand.cpp \
  $(UP3)/asmjit2-src/src/asmjit/arm/a64rapass.cpp \
  $(UP3)/asmjit2-src/src/asmjit/arm/armformatter.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/archtraits.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/assembler.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/builder.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/codeholder.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/codewriter.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/compiler.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/constpool.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/cpuinfo.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/emithelper.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/emitter.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/emitterutils.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/environment.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/errorhandler.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/formatter.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/func.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/funcargscontext.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/globals.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/inst.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/instdb.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/jitallocator.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/jitruntime.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/logger.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/operand.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/osutils.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/ralocal.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/rapass.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/rastack.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/string.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/support.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/target.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/type.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/virtmem.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/zone.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/zonehash.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/zonelist.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/zonestack.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/zonetree.cpp \
  $(UP3)/asmjit2-src/src/asmjit/core/zonevector.cpp \
  $(UP3)/asmjit2-src/src/asmjit/x86/x86assembler.cpp \
  $(UP3)/asmjit2-src/src/asmjit/x86/x86builder.cpp \
  $(UP3)/asmjit2-src/src/asmjit/x86/x86compiler.cpp \
  $(UP3)/asmjit2-src/src/asmjit/x86/x86emithelper.cpp \
  $(UP3)/asmjit2-src/src/asmjit/x86/x86formatter.cpp \
  $(UP3)/asmjit2-src/src/asmjit/x86/x86func.cpp \
  $(UP3)/asmjit2-src/src/asmjit/x86/x86instapi.cpp \
  $(UP3)/asmjit2-src/src/asmjit/x86/x86instdb.cpp \
  $(UP3)/asmjit2-src/src/asmjit/x86/x86operand.cpp \
  $(UP3)/asmjit2-src/src/asmjit/x86/x86rapass.cpp

CJSON_SRCS := $(UP3)/cJSON/cJSON.cpp

CAPSTONE_SRCS := \
  $(UP3)/capstone-5.0.7/src/arch/AArch64/AArch64BaseInfo.c \
  $(UP3)/capstone-5.0.7/src/arch/AArch64/AArch64Disassembler.c \
  $(UP3)/capstone-5.0.7/src/arch/AArch64/AArch64InstPrinter.c \
  $(UP3)/capstone-5.0.7/src/arch/AArch64/AArch64Mapping.c \
  $(UP3)/capstone-5.0.7/src/arch/AArch64/AArch64Module.c \
  $(UP3)/capstone-5.0.7/src/cs.c \
  $(UP3)/capstone-5.0.7/src/Mapping.c \
  $(UP3)/capstone-5.0.7/src/MCInst.c \
  $(UP3)/capstone-5.0.7/src/MCInstrDesc.c \
  $(UP3)/capstone-5.0.7/src/MCRegisterInfo.c \
  $(UP3)/capstone-5.0.7/src/SStream.c \
  $(UP3)/capstone-5.0.7/src/utils.c

ALL_SRCS := $(SPROOT_SRCS) $(ENGINE_SRCS) $(ASMJIT_SRCS) $(CJSON_SRCS)
SRCS_C   := $(CAPSTONE_SRCS)

OBJS := $(addprefix $(OBJDIR)/,$(ALL_SRCS:.cpp=.o) $(SRCS_C:.c=.o))

all: $(TARGET)

# 只编目标文件、不链接：给 engine/android/build_arm64.sh 的 JNI 形态复用
# （共享库需要自定义链接行 + 追加 JNI 包装对象，故不走这里的链接规则）
objs: $(OBJS)

$(TARGET): $(OBJS)
	$(CXX) -o $@ $(LDFLAGS) $(OBJS) $(LIBS)

$(OBJDIR)/%.o: %.cpp
	@mkdir -p $(dir $@)
	$(CXX) $(CXXFLAGS) -c $< -o $@

$(OBJDIR)/%.o: %.c
	@mkdir -p $(dir $@)
	$(CC) $(CFLAGS) -c $< -o $@

clean:
	rm -f $(TARGET)
	rm -rf $(OBJDIR)

.PHONY: all objs clean
