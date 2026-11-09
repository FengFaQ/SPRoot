import java.util.Properties

// SPRoot 管理器 —— app 模块
//
// 版本矩阵（实证来源见 gradle/libs.versions.toml 注释）：
//   AGP 9.4.1 / Kotlin 2.4.20 / Compose BOM 2026.08.00 (=1.12.0) / miuix 0.9.4
//   compileSdk 37（miuix AAR 要求 minCompileSdk=37）/ build-tools 37.0.0 / minSdk 26 / JDK 21
plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

// ---------------------------------------------------------------------------
// 签名（方案 A：keystore 入 private 仓库；同时兼容环境变量，便于 fork/CI 覆盖）
// 查找顺序：newapp/keystore/keystore.properties > 环境变量 > 无（release 产出 unsigned，仍可编译）
// ---------------------------------------------------------------------------
val keystoreDir = rootProject.layout.projectDirectory.dir("keystore")
val keystorePropsFile = keystoreDir.file("keystore.properties").asFile
val keystoreProps = Properties().apply {
    if (keystorePropsFile.isFile) keystorePropsFile.inputStream().use { load(it) }
}

fun signingValue(props: Properties, key: String, env: String): String? =
    props.getProperty(key)?.takeIf { it.isNotBlank() } ?: System.getenv(env)?.takeIf { it.isNotBlank() }

val signingStoreFile: File? = signingValue(keystoreProps, "storeFile", "SPROOT_KEYSTORE")?.let { path ->
    val candidate = File(path)
    when {
        candidate.isAbsolute -> candidate
        // keystore.properties 里的 "keystore/xxx.jks" 是相对 newapp/ 的
        rootProject.layout.projectDirectory.file(path).asFile.isFile ->
            rootProject.layout.projectDirectory.file(path).asFile
        else -> keystoreDir.file(path).asFile
    }
}
val signingStorePassword = signingValue(keystoreProps, "storePassword", "SPROOT_KEYSTORE_PASSWORD")
val signingKeyAlias = signingValue(keystoreProps, "keyAlias", "SPROOT_KEY_ALIAS")
val signingKeyPassword = signingValue(keystoreProps, "keyPassword", "SPROOT_KEY_PASSWORD")
val hasSigningKey = signingStoreFile?.isFile == true && !signingStorePassword.isNullOrBlank() &&
    !signingKeyAlias.isNullOrBlank() && !signingKeyPassword.isNullOrBlank()

android {
    namespace = "com.sproot.manager"

    compileSdk {
        version = release(37) {
            minorApiLevel = 0
        }
    }
    buildToolsVersion = "37.0.0"

    defaultConfig {
        applicationId = "com.sproot.manager"
        minSdk = 26
        targetSdk = 37
        versionCode = 1
        versionName = "0.1.0"
        // 只交付 arm64：SK 修补引擎与 magiskboot 均只交叉编译 aarch64
        ndk {
            abiFilters += listOf("arm64-v8a")
        }
    }

    if (hasSigningKey) {
        signingConfigs {
            register("sproot") {
                storeFile = signingStoreFile
                storePassword = signingStorePassword
                keyAlias = signingKeyAlias
                keyPassword = signingKeyPassword
                enableV3Signing = true
            }
        }
    }

    buildTypes {
        debug {
            applicationIdSuffix = ".debug"
            versionNameSuffix = "-debug"
        }
        release {
            // 暂不混淆：修补/日志路径需要可读堆栈；R8 规则后续单独评估
            isMinifyEnabled = false
            if (hasSigningKey) {
                signingConfig = signingConfigs.getByName("sproot")
            }
        }
    }

    buildFeatures {
        compose = true
        buildConfig = true
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    packaging {
        jniLibs {
            // 必须解压：libmagiskboot.so 需是 nativeLibraryDir 下的真实可执行文件，
            // 才能被 ProcessBuilder 直接 exec（不是 filesDir —— 规避旧版缺陷 ⑨）。
            useLegacyPackaging = true
        }
        resources {
            excludes += listOf("/META-INF/{AL2.0,LGPL2.1}", "META-INF/*.version")
        }
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    }
}

// miuix 的 ui/nav/preference 都会传递 material3-window-size-class（CMP 的 alpha 版），
// 其 runtime 变体会把 material3 约束抬到 alpha，造成编译期/运行期版本不一致。
// miuix 源码对 WindowSizeClass 零引用（docs/research/miuix-integration.md §1.3），全局排除最稳。
configurations.configureEach {
    exclude(group = "org.jetbrains.compose.material3", module = "material3-window-size-class")
    exclude(group = "org.jetbrains.compose.material3", module = "material3-window-size-class-android")
}

dependencies {
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.compose.runtime)
    implementation(libs.androidx.compose.foundation)
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.activity.compose)

    // miuix 五模块（照抄 KernelSU 的 miuix 实现所需的最小集）
    //  ui        —— 基础组件（Scaffold/TopAppBar/Card/NavigationBar/NavigationRail…）
    //  icons     —— MiuixIcons
    //  nav       —— NavDisplay/backstack（关于页等子页面转场）
    //  preference—— 设置页的 SwitchPreference/ArrowPreference/DropdownPreference
    //  blur      —— TopAppBar/底栏毛玻璃；minSdk 33，用 manifest tools:overrideLibrary + isRenderEffectSupported() 守卫
    implementation(libs.miuix.ui)
    implementation(libs.miuix.icons)
    implementation(libs.miuix.nav)
    implementation(libs.miuix.preference)
    implementation(libs.miuix.blur)
}
