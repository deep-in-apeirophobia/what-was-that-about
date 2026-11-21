//! Small platform queries for the system panel.

use std::sync::OnceLock;

use wwta_core::types::GpuDevice;

pub fn gpu_devices_cached() -> Vec<GpuDevice> {
    static DEVICES: OnceLock<Vec<GpuDevice>> = OnceLock::new();
    DEVICES
        .get_or_init(wwta_core::asr::whisper::gpu_devices)
        .clone()
}

#[cfg(target_os = "linux")]
pub fn total_memory() -> u64 {
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("MemTotal:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|kb| kb.parse::<u64>().ok())
        })
        .map(|kb| kb * 1024)
        .unwrap_or(0)
}

#[cfg(windows)]
pub fn total_memory() -> u64 {
    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }
    extern "system" {
        fn GlobalMemoryStatusEx(buf: *mut MemoryStatusEx) -> i32;
    }
    let mut m = MemoryStatusEx {
        length: std::mem::size_of::<MemoryStatusEx>() as u32,
        memory_load: 0,
        total_phys: 0,
        avail_phys: 0,
        total_page_file: 0,
        avail_page_file: 0,
        total_virtual: 0,
        avail_virtual: 0,
        avail_extended_virtual: 0,
    };
    // SAFETY: plain Win32 call with a correctly sized, initialized struct.
    if unsafe { GlobalMemoryStatusEx(&mut m) } != 0 {
        m.total_phys
    } else {
        0
    }
}

#[cfg(target_os = "macos")]
pub fn total_memory() -> u64 {
    std::process::Command::new("sysctl")
        .args(["-n", "hw.memsize"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0)
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
pub fn total_memory() -> u64 {
    0
}
