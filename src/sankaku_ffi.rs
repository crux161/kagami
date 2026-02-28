pub const VIDEO_CODEC_HEVC: u8 = 0x01;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SankakuStreamStats {
    pub bitrate_bps: u32,
    pub packet_loss_ratio: f32,
    pub jitter_us: u32,
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SankakuTelemetry {
    pub tx_stats: SankakuStreamStats,
    pub rx_stats: SankakuStreamStats,
}

#[derive(Clone, Debug)]
pub struct VideoFrame {
    pub timestamp_us: u64,
    pub keyframe: bool,
    pub codec: u8,
    pub width: u32,
    pub height: u32,
    pub payload: Vec<u8>,
}

unsafe extern "C" {
    pub fn init();
}
