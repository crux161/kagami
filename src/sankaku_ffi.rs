#![allow(dead_code)]

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
use std::ptr;

pub const VIDEO_CODEC_HEVC: u8 = 0x01;
#[allow(dead_code)]
pub const AUDIO_CODEC_OPUS: u8 = 0x03;
pub const SANKAKU_FRAME_FLAG_KEYFRAME: u32 = 0x0000_0001;

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

#[repr(C)]
pub struct SankakuStreamHandle {
    _private: [u8; 0],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SankakuQuicHandleKind {
    Invalid = 0,
    Connection = 1,
    Endpoint = 2,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SankakuQuicHandle {
    pub kind: SankakuQuicHandleKind,
    pub handle: u64,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SankakuFrameKind {
    Keyframe = 0,
    Delta = 1,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SankakuVideoFrame {
    pub data: *const u8,
    pub len: usize,
    pub pts_us: u64,
    pub dts_us: u64,
    pub codec: u8,
    pub kind: SankakuFrameKind,
    pub flags: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct SankakuInboundFrame {
    pub data: *const u8,
    pub len: usize,
    pub session_id: u64,
    pub stream_id: u32,
    pub frame_index: u64,
    pub pts_us: u64,
    pub dts_us: u64,
    pub codec: u8,
    pub kind: SankakuFrameKind,
    pub flags: u32,
    pub packet_loss_ratio: f32,
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
unsafe extern "C" {
    pub fn init();
    pub fn sankaku_stream_create(quic_handle: SankakuQuicHandle) -> *mut SankakuStreamHandle;
    pub fn sankaku_stream_destroy(handle: *mut SankakuStreamHandle);
    pub fn sankaku_stream_send_frame(
        handle: *mut SankakuStreamHandle,
        frame: *const SankakuVideoFrame,
    ) -> i32;
    pub fn sankaku_stream_poll_frame(
        handle: *mut SankakuStreamHandle,
        out_frame: *mut *mut SankakuInboundFrame,
    ) -> i32;
    pub fn sankaku_frame_free(frame: *mut SankakuInboundFrame);
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub unsafe fn init() {}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub unsafe fn sankaku_stream_create(_quic_handle: SankakuQuicHandle) -> *mut SankakuStreamHandle {
    ptr::null_mut()
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub unsafe fn sankaku_stream_destroy(_handle: *mut SankakuStreamHandle) {}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub unsafe fn sankaku_stream_send_frame(
    _handle: *mut SankakuStreamHandle,
    _frame: *const SankakuVideoFrame,
) -> i32 {
    -1
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub unsafe fn sankaku_stream_poll_frame(
    _handle: *mut SankakuStreamHandle,
    out_frame: *mut *mut SankakuInboundFrame,
) -> i32 {
    if !out_frame.is_null() {
        unsafe {
            *out_frame = ptr::null_mut();
        }
    }
    -1
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub unsafe fn sankaku_frame_free(_frame: *mut SankakuInboundFrame) {}
