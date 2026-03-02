#[cfg(target_os = "windows")]
unsafe extern "C" {
    pub fn init();
}

#[cfg(not(target_os = "windows"))]
pub unsafe fn init() {}
