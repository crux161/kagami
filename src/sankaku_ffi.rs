pub const VIDEO_CODEC_HEVC: u8 = 0x01;
#[allow(dead_code)]
pub const AUDIO_CODEC_OPUS: u8 = 0x03;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SankakuStreamStats {
    pub bitrate_bps: u32,
    pub packet_loss_ratio: f32,
    pub jitter_us: u32,
    pub latency_ms: u32,
    pub width: u32,
    pub height: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SankakuTelemetry {
    pub tx_stats: SankakuStreamStats,
    pub rx_stats: SankakuStreamStats,
    pub path_rtt_ms: u64,
    pub udp_tx_dropped: u64,
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct VideoFrame {
    pub timestamp_us: u64,
    pub keyframe: bool,
    pub codec: u8,
    pub width: u32,
    pub height: u32,
    pub payload: Vec<u8>,
}

impl VideoFrame {
    pub fn nal_with_codec(payload: Vec<u8>, timestamp_us: u64, keyframe: bool, codec: u8) -> Self {
        Self {
            timestamp_us,
            keyframe,
            codec,
            width: 0,
            height: 0,
            payload,
        }
    }

    pub fn with_dimensions(mut self, width: u32, height: u32) -> Self {
        self.width = width;
        self.height = height;
        self
    }
}

unsafe extern "C" {
    pub fn init();
}
