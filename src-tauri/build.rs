fn main() {
    // Delay-load the Vulkan loader on Windows so the app starts on machines without GPU
    // drivers; Shenava keeps working and Whisper explains that Vulkan is missing.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os == "windows" && target_env == "msvc" {
        println!("cargo:rustc-link-arg-bins=/DELAYLOAD:vulkan-1.dll");
        println!("cargo:rustc-link-lib=delayimp");
    }
    // whisper.cpp's Metal backend guards newer APIs with @available, which becomes a call into
    // compiler-rt (___isPlatformVersionAtLeast) when the deployment target (11.0, set by
    // `tauri build`) is older than the SDK. rustc links with -nodefaultlibs, so add it here.
    if target_os == "macos" {
        match clang_rt_osx() {
            Some(lib) => println!("cargo:rustc-link-arg-bins={}", lib.display()),
            None => println!("cargo:warning=libclang_rt.osx.a not found; linking may fail"),
        }
    }
    tauri_build::build()
}

fn clang_rt_osx() -> Option<std::path::PathBuf> {
    let out = std::process::Command::new("cc")
        .arg("-print-resource-dir")
        .output()
        .ok()?;
    let dir = String::from_utf8(out.stdout).ok()?;
    let lib = std::path::Path::new(dir.trim()).join("lib/darwin/libclang_rt.osx.a");
    lib.exists().then_some(lib)
}
