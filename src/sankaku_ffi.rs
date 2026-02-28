#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SankakuTelemetry {
    pub bitrate_bps: u32,
    pub packet_loss_ratio: f32,
    pub jitter_us: u32,
}

unsafe extern "C" {
    pub fn init();
}
