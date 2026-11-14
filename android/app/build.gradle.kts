plugins {
    id("com.android.application")
}

android {
    namespace = "com.metalvj.dash"
    compileSdk = 35
    buildToolsVersion = "35.0.0"

    defaultConfig {
        applicationId = "com.metalvj.dash"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"

        // The ABI cargo-ndk built: scripts/build-apk.sh passes -PfwAbi
        // (arm64-v8a for phones, x86_64 for the emulator).
        ndk {
            abiFilters += listOf(findProperty("fwAbi")?.toString() ?: "arm64-v8a")
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    // libmetal_dash.so must sit on disk, not inside the APK, for NativeActivity's
    // dlopen. Set here, not via the deprecated manifest `extractNativeLibs`.
    packaging {
        jniLibs {
            useLegacyPackaging = true
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}
