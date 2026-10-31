// Root build: plugin versions only. The Rust side is built by
// scripts/build-apk.sh, which drops libmetal_dash.so into app/src/main/jniLibs.
plugins {
    id("com.android.application") version "8.7.3" apply false
}
