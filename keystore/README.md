# SPRoot 签名 keystore（方案 A：入 private 仓库）

- 文件：sproot-release.jks（PKCS12，RSA 4096，有效期 ~30 年）
- DN：CN=FengFa, OU=SPRoot, O=FengFaQ_LeiZhe, C=CN
- keystore.properties 含明文口令，**仅限本 private 仓库**；一旦仓库转为 public 必须立即轮换密钥。
- 备份：原始密钥同时存于本机 /mnt/c/Users/Administrator/.ssh/sproot-release.jks，口令记录在 SPROOT_SIGNING.md。
- CI 用法（app/build.gradle.kts）：
```kotlin
val ksProps = java.util.Properties().apply { file("../keystore/keystore.properties").inputStream().use { load(it) } }
signingConfigs { create("release") {
    storeFile = file(ksProps.getProperty("storeFile"))
    storePassword = ksProps.getProperty("storePassword")
    keyAlias = ksProps.getProperty("keyAlias")
    keyPassword = ksProps.getProperty("keyPassword")
}}
```
- 校验：`apksigner verify --print-certs app-release.apk` 应显示 CN=FengFa
