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

    // Windows' 1 MB main-thread stack (Linux/macOS: 8 MB) overflows in debug and release builds.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        const STACK_BYTES: u32 = 8 * 1024 * 1024;
        match std::env::var("CARGO_CFG_TARGET_ENV").as_deref() {
            Ok("msvc") => println!("cargo:rustc-link-arg-bins=/STACK:{STACK_BYTES}"),
            Ok("gnu") => println!("cargo:rustc-link-arg-bins=-Wl,--stack,{STACK_BYTES}"),
            _ => {}
        }
    }

    #[cfg(windows)]
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("assets/icon.ico");
        if let Err(e) = res.compile() {
            println!("cargo:warning=app icon not embedded: {e}");
        }
    }

    // Re-run only if the feature set, target or icon changes, not on every source edit.
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_FAST_JPEG");
    println!("cargo:rerun-if-env-changed=CARGO_CFG_TARGET_ARCH");
    println!("cargo:rerun-if-changed=assets/icon.ico");
}
