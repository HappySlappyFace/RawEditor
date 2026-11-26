#!/usr/bin/env bash
# Build the headless probe (crates/raw-editor-probe) for Android and run it on
# a phone over adb.
#
#   tools/android-probe.sh build            cross-compile for aarch64-linux-android
#   tools/android-probe.sh run <photo.raw>  push the probe and the photo, run it
#                                           (extra arguments go to the probe, e.g. --ticks 200)
#
# PROBE_FEATURES=profiling adds the per-render breakdown (CPU spans, GPU pass
# times) after the slider ticks.
#
# Needs: rustup with the aarch64-linux-android target (its cargo is used even
# when the system's Rust comes first on PATH), the Android NDK (ANDROID_NDK_HOME,
# else the newest under ~/Android/Sdk/ndk or ~/Android/ndk), and adb for `run`.
set -euo pipefail

cd "$(dirname "$0")/.."
TARGET=aarch64-linux-android
API=${ANDROID_API:-28}   # Android 9: Vulkan 1.1 is guaranteed from here
DEVICE_DIR=/data/local/tmp/raw-editor-probe
OUT=target/android/$TARGET/release/raw-editor-probe

find_ndk() {
    if [[ -n "${ANDROID_NDK_HOME:-}" ]]; then echo "$ANDROID_NDK_HOME"; return; fi
    local d
    for d in ~/Android/Sdk/ndk ~/Android/ndk; do
        if [[ -d $d ]]; then
            # A folder of versions (the SDK layout) or an unpacked NDK itself.
            if [[ -d $d/toolchains ]]; then echo "$d"; return; fi
            local newest
            newest=$(ls -1 "$d" 2>/dev/null | sort -V | tail -1)
            if [[ -n $newest ]]; then echo "$d/$newest"; return; fi
        fi
    done
    for d in ~/Android/android-ndk-*; do [[ -d $d ]] && echo "$d" && return; done
    echo "No Android NDK found: set ANDROID_NDK_HOME" >&2
    exit 1
}

build() {
    local cargo=~/.cargo/bin/cargo
    [[ -x $cargo ]] || { echo "rustup's cargo not found at $cargo" >&2; exit 1; }
    local ndk bin
    ndk=$(find_ndk)
    bin=$ndk/toolchains/llvm/prebuilt/linux-x86_64/bin
    [[ -d $bin ]] || { echo "Not an NDK: $ndk" >&2; exit 1; }
    echo "NDK $ndk, API $API"

    # Its own target folder: rustup's compiler differs from the system's, and
    # sharing one would rebuild the desktop's build scripts every switch.
    CARGO_TARGET_DIR=target/android \
    CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER=$bin/$TARGET$API-clang \
    CC_aarch64_linux_android=$bin/$TARGET$API-clang \
    CXX_aarch64_linux_android=$bin/$TARGET$API-clang++ \
    AR_aarch64_linux_android=$bin/llvm-ar \
    RUSTC=~/.cargo/bin/rustc \
        "$cargo" build --release -p raw-editor-probe --target $TARGET ${PROBE_FEATURES:+--features $PROBE_FEATURES}

    echo "Built $OUT ($(du -h "$OUT" | cut -f1))"
    echo "Shared libraries it needs:"
    "$bin/llvm-readelf" -d "$OUT" | grep NEEDED || true
}

run() {
    local photo=${1:?which RAW file?}
    shift
    [[ -f $OUT && -z ${PROBE_FEATURES:-} ]] || build
    command -v adb >/dev/null || { echo "adb not found" >&2; exit 1; }
    adb get-state >/dev/null
    echo "Device: $(adb shell getprop ro.product.manufacturer | tr -d '\r') $(adb shell getprop ro.product.model | tr -d '\r'), Android $(adb shell getprop ro.build.version.release | tr -d '\r'), $(adb shell getprop ro.soc.model | tr -d '\r')"

    adb shell mkdir -p $DEVICE_DIR
    adb push "$OUT" $DEVICE_DIR/ >/dev/null
    # libc++ is the one NDK library apps ship themselves; push it when linked.
    local extra="" prebuilt
    prebuilt=$(find_ndk)/toolchains/llvm/prebuilt/linux-x86_64
    if "$prebuilt/bin/llvm-readelf" -d "$OUT" | grep -q libc++_shared; then
        adb push "$prebuilt/sysroot/usr/lib/$TARGET/libc++_shared.so" $DEVICE_DIR/ >/dev/null
        extra="LD_LIBRARY_PATH=$DEVICE_DIR"
    fi
    local name
    name=$(basename "$photo")
    adb push "$photo" "$DEVICE_DIR/$name" >/dev/null
    adb shell "cd $DEVICE_DIR && chmod +x raw-editor-probe && $extra RUST_LOG=${RUST_LOG:-warn} ./raw-editor-probe '$name' --work $DEVICE_DIR/work $*"

    # The exported JPEG, to check by eye.
    mkdir -p target/android/out
    adb pull "$DEVICE_DIR/work/export/." target/android/out/ >/dev/null && echo "Export pulled to target/android/out/"
}

case "${1:-}" in
    build) build ;;
    run) shift; run "$@" ;;
    *) sed -n '2,12p' "$0"; exit 2 ;;
esac
