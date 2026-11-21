fn main() {
    // Delay-load the Vulkan loader on Windows so the app starts on machines without GPU
    // drivers; Shenava keeps working and Whisper explains that Vulkan is missing.
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let target_env = std::env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();
    if target_os == "windows" && target_env == "msvc" {
        println!("cargo:rustc-link-arg-bins=/DELAYLOAD:vulkan-1.dll");
        println!("cargo:rustc-link-lib=delayimp");
    }
    tauri_build::build()
}
