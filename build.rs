fn main() {
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let fast_jpeg = std::env::var("CARGO_FEATURE_FAST_JPEG").is_ok();

    if fast_jpeg {
        let simd = match arch.as_str() {
            "x86_64" => "SSE4.2 / AVX2 auto-detected at runtime",
            "aarch64" => "NEON auto-detected at runtime",
            "x86" => "SSE2 auto-detected at runtime",
            other => {
                println!(
                    "cargo:warning=fast-jpeg: no SIMD path for target '{}', using scalar fallback",
                    other
                );
                "scalar only"
            }
        };
        println!("cargo:warning=fast-jpeg enabled ({simd})");
    }

    // Re-run only if the feature set or target changes, not on every source edit.
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_FAST_JPEG");
    println!("cargo:rerun-if-env-changed=CARGO_CFG_TARGET_ARCH");
}
