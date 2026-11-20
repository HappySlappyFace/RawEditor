fn main() {
    // ONNX Runtime's prebuilt macOS library has Objective-C `@available`
    // checks, which call `__isPlatformVersionAtLeast` from Clang's runtime
    // (libclang_rt.osx.a). rustc doesn't link that library, so link it here.
    // A link argument only reaches this package's own targets, so this one
    // covers the engine's test binaries; the app's build.rs does the same for
    // the app.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let resource_dir = std::process::Command::new("clang")
            .arg("--print-resource-dir")
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string());
        match resource_dir.map(|d| std::path::Path::new(&d).join("lib/darwin/libclang_rt.osx.a")) {
            Some(lib) if lib.exists() => println!("cargo:rustc-link-arg={}", lib.display()),
            _ => println!("cargo:warning=libclang_rt.osx.a not found; linking ONNX Runtime may fail"),
        }
    }

    println!("cargo:rerun-if-env-changed=CARGO_CFG_TARGET_OS");
}
