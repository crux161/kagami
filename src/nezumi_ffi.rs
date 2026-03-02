#[cfg(any(target_os = "windows", target_os = "macos"))]
unsafe extern "C" {
    pub fn init();
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub unsafe fn init() {}
