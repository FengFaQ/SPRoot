// spr_json.hpp — SPRoot 引擎薄 JSON 封装（基于已 vendor 的 cJSON）
// 仅覆盖 skroot_patch_main.cpp 所需子集：对象/数组/字符串/数值/嵌套 dump
#pragma once
#include <cJSON.h>
#include <string>
#include <cstdint>
#include <memory>
#include <stdexcept>

namespace spr {

struct JsonDeleter { void operator()(cJSON* p) const { if (p) cJSON_Delete(p); } };
using JsonPtr = std::unique_ptr<cJSON, JsonDeleter>;

// 轻量值包装：不拥有所有权时用 view 模式太复杂，这里统一"拥有"语义，
// 以 builder 风格 API 构造，dump 时序列化。
class J {
public:
    J() : ptr_(cJSON_CreateObject()) {}
    explicit J(cJSON* take) : ptr_(take) {}
    explicit J(const char* s) : ptr_(cJSON_CreateString(s)) {}
    explicit J(const std::string& s) : ptr_(cJSON_CreateString(s.c_str())) {}
    explicit J(double v) : ptr_(cJSON_CreateNumber(v)) {}
    explicit J(int v) : ptr_(cJSON_CreateNumber(v)) {}
    explicit J(bool v) : ptr_(cJSON_CreateBool(v ? 1 : 0)) {}

    static J array() { return J(cJSON_CreateArray()); }

    J(const J& o) = delete;
    J& operator=(const J&) = delete;
    J(J&& o) noexcept : ptr_(std::move(o.ptr_)) {}
    J& operator=(J&& o) noexcept { ptr_ = std::move(o.ptr_); return *this; }

    cJSON* raw() { return ptr_.get(); }
    cJSON* raw() const { return ptr_.get(); }
    cJSON* release() { return ptr_.release(); }

    J& set(const std::string& key, J v) {
        cJSON_AddItemToObject(ptr_.get(), key.c_str(), v.release());
        return *this;
    }
    J& set(const std::string& key, const std::string& v) { return set(key, J(v)); }
    J& set(const std::string& key, const char* v) { return set(key, J(v)); }
    J& set(const std::string& key, long long v) { return set(key, J(static_cast<double>(v))); }
    J& set(const std::string& key, int v) { return set(key, J(v)); }
    J& set(const std::string& key, bool v) { return set(key, J(v)); }

    // 向数组追加（本对象必须是数组）
    J& push(J v) {
        cJSON_AddItemToArray(ptr_.get(), v.release());
        return *this;
    }
    J& push(const std::string& v) { return push(J(v)); }

    std::string dump(int indent = 0) const {
        char* s = indent > 0 ? cJSON_Print(ptr_.get()) : cJSON_PrintUnformatted(ptr_.get());
        if (!s) throw std::runtime_error("cJSON print failed");
        std::string out(s);
        cJSON_free(s);
        return out;
    }

    bool is_null() const { return !ptr_; }

private:
    JsonPtr ptr_;
};

} // namespace spr
