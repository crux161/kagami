#[cfg(target_os = "macos")]
pub mod avfoundation;

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct MediaTrack {
    pub id: String,
    pub kind: TrackKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[allow(dead_code)]
pub enum TrackKind {
    Video,
    Audio,
}

#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct MediaPacket {
    pub track_id: String,
    pub timestamp_us: u64,
    pub keyframe: bool,
    pub codec: u8,
    pub data: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone)]
#[allow(dead_code)]
pub struct PreviewFrame {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub data: Vec<u8>,
    pub timestamp_us: u64,
}

#[allow(dead_code)]
pub trait NezumiProducer {
    fn tracks(&self) -> Vec<MediaTrack>;
    fn start_reading(&mut self) -> Result<(), String>;
    fn next_packet(&mut self) -> Option<MediaPacket>;
    fn next_preview_frame(&mut self) -> Option<PreviewFrame>;
    fn stop(&mut self);
}

#[cfg(not(target_os = "macos"))]
pub struct StubProducer;

#[cfg(not(target_os = "macos"))]
impl NezumiProducer for StubProducer {
    fn tracks(&self) -> Vec<MediaTrack> {
        Vec::new()
    }
    fn start_reading(&mut self) -> Result<(), String> {
        Err("no capture backend available on this platform".to_owned())
    }
    fn next_packet(&mut self) -> Option<MediaPacket> {
        None
    }
    fn next_preview_frame(&mut self) -> Option<PreviewFrame> {
        None
    }
    fn stop(&mut self) {}
}
