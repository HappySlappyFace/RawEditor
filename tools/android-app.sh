#!/usr/bin/env bash
# Build and run the Android app (android/, with crates/raw-editor-android).
#
#   tools/android-app.sh build             the Rust library + the debug APK
#                                          (profile `quick`, no LTO: seconds to
#                                          rebuild; RELEASE=1 for the full build)
#   tools/android-app.sh install           build, then install on the phone over adb
#   tools/android-app.sh photo <file.raw>  copy a RAW into the app's folder (it opens the first)
#   tools/android-app.sh run               start the app
#   tools/android-app.sh log               follow its log (logcat, tag RawEditor)
#
# Needs what tools/android-probe.sh needs (rustup with aarch64-linux-android,
# the NDK), plus a JDK 17 and the Android SDK (ANDROID_HOME, else ~/Android/Sdk).
set -euo pipefail

cd "$(dirname "$0")/.."
TARGET=aarch64-linux-android
API=${ANDROID_API:-28}
PACKAGE=dev.raweditor
PROFILE=$([[ -n "${RELEASE:-}" ]] && echo release || echo quick)
LIB=target/android/$TARGET/$PROFILE/libraweditor.so
JNI=android/app/src/main/jniLibs/arm64-v8a
APK=android/app/build/outputs/apk/debug/app-debug.apk
PHOTOS=/sdcard/Android/data/$PACKAGE/files

find_ndk() {
    if [[ -n "${ANDROID_NDK_HOME:-}" ]]; then echo "$ANDROID_NDK_HOME"; return; fi
    local d
    for d in ~/Android/Sdk/ndk/* ~/Android/android-ndk-*; do [[ -d $d/toolchains ]] && echo "$d" && return; done
    echo "No Android NDK found: set ANDROID_NDK_HOME" >&2
    exit 1
}

build() {
    local cargo=~/.cargo/bin/cargo ndk bin
    [[ -x $cargo ]] || { echo "rustup's cargo not found at $cargo" >&2; exit 1; }
    ndk=$(find_ndk)
    bin=$ndk/toolchains/llvm/prebuilt/linux-x86_64/bin
    # max-page-size: Android 15+ wants native code loadable on 16 KB pages
    # (and warns about any library that isn't); NDK r27 still links for 4 KB.
    CARGO_TARGET_DIR=target/android \
    CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=$bin/$TARGET$API-clang \
    CC_aarch64_linux_android=$bin/$TARGET$API-clang \
    CXX_aarch64_linux_android=$bin/$TARGET$API-clang++ \
    AR_aarch64_linux_android=$bin/llvm-ar \
    RUSTC=~/.cargo/bin/rustc \
    CARGO_TARGET_AARCH64_LINUX_ANDROID_RUSTFLAGS="-C link-arg=-Wl,-z,max-page-size=16384" \
        "$cargo" build --profile $PROFILE -p raw-editor-android --target $TARGET

    mkdir -p $JNI
    cp "$LIB" $JNI/
    # ONNX Runtime links libc++ dynamically; apps ship it themselves.
    cp "$ndk/toolchains/llvm/prebuilt/linux-x86_64/sysroot/usr/lib/$TARGET/libc++_shared.so" $JNI/

    export ANDROID_HOME=${ANDROID_HOME:-$HOME/Android/Sdk}
    [[ -f android/local.properties ]] || echo "sdk.dir=$ANDROID_HOME" > android/local.properties
    (cd android && ./gradlew --quiet assembleDebug)
    echo "Built $APK ($(du -h $APK | cut -f1))"
}

case "${1:-}" in
    build) build ;;
    install) build; adb install -r "$APK" ;;
    photo)
        photo=${2:?which RAW file?}
        adb shell mkdir -p $PHOTOS
        adb push "$photo" $PHOTOS/ ;;
    run) adb shell am start -n $PACKAGE/.MainActivity ;;
    log) adb logcat -s RawEditor:V AndroidRuntime:E ;;
    *) sed -n '2,12p' "$0"; exit 2 ;;
esac
