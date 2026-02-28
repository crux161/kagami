mod discovery;
mod nezumi_ffi;
mod sankaku_ffi;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use discovery::{DiscoveryEngine, Peer, DEFAULT_MEDIA_PORT};
use sankaku_ffi::{SankakuStreamStats, SankakuTelemetry, VideoFrame, VIDEO_CODEC_HEVC};

slint::include_modules!();

trait MediaProvider {
    fn connect(&mut self, addr: &str) -> Result<(), String>;
    fn poll_frame(&mut self) -> Option<slint::Image>;
    fn poll_local_preview(&mut self) -> Option<slint::Image>;
    fn capture_local_frame(&mut self) -> Option<VideoFrame>;
    fn broadcast_frame(&mut self, frame: VideoFrame);
    fn get_telemetry(&self) -> SankakuTelemetry;
    fn set_audio_muted(&mut self, muted: bool);
    fn set_video_enabled(&mut self, enabled: bool);
}

struct SankakuStreamProvider {
    connected_addr: Option<String>,
    audio_muted: bool,
    video_enabled: bool,
    local_frame_buffer: slint::SharedPixelBuffer<slint::Rgb8Pixel>,
    remote_frame_buffer: slint::SharedPixelBuffer<slint::Rgb8Pixel>,
    capture_tick: u32,
    remote_tick: u32,
    telemetry: SankakuTelemetry,
    rng_state: u64,
}

impl SankakuStreamProvider {
    fn new() -> Self {
        let mut provider = Self {
            connected_addr: None,
            audio_muted: false,
            video_enabled: true,
            local_frame_buffer: slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(320, 180),
            remote_frame_buffer: slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(1280, 720),
            capture_tick: 0,
            remote_tick: 0,
            telemetry: SankakuTelemetry::default(),
            rng_state: 0x5A17_2D3C_4B91_08EF,
        };
        provider.render_waiting_remote();
        provider.render_local_disabled_frame();
        provider
    }

    fn next_u32(&mut self) -> u32 {
        self.rng_state = self
            .rng_state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1);
        (self.rng_state >> 32) as u32
    }

    fn next_unit_f32(&mut self) -> f32 {
        self.next_u32() as f32 / u32::MAX as f32
    }

    fn range_u32(&mut self, min: u32, max: u32) -> u32 {
        min + self.next_u32() % (max - min + 1)
    }

    fn range_f32(&mut self, min: f32, max: f32) -> f32 {
        min + self.next_unit_f32() * (max - min)
    }

    fn render_local_capture_frame(&mut self) {
        let phase = self.capture_tick;
        let muted_overlay = if self.audio_muted { 10u8 } else { 0u8 };
        let width = self.local_frame_buffer.width();
        let height = self.local_frame_buffer.height();
        let pixels = self.local_frame_buffer.make_mut_slice();

        for y in 0..height {
            for x in 0..width {
                let index = (y * width + x) as usize;
                let wave = ((x + phase * 5) % width) as u8;
                let scan = ((y * 255) / height) as u8;
                let focus = if (x + phase * 3) % width < width / 4 {
                    26u8
                } else {
                    0u8
                };

                pixels[index] = slint::Rgb8Pixel {
                    r: 22u8.saturating_add(scan / 5).saturating_add(muted_overlay),
                    g: 54u8
                        .saturating_add(wave / 4)
                        .saturating_add(focus / 2)
                        .saturating_add(muted_overlay),
                    b: 88u8.saturating_add(wave / 3).saturating_add(focus),
                };
            }
        }

        self.capture_tick = self.capture_tick.wrapping_add(1);
    }

    fn render_local_disabled_frame(&mut self) {
        let pixels = self.local_frame_buffer.make_mut_slice();
        for pixel in pixels {
            *pixel = slint::Rgb8Pixel {
                r: 18,
                g: 18,
                b: 24,
            };
        }
    }

    fn render_waiting_remote(&mut self) {
        let phase = self.remote_tick;
        let width = self.remote_frame_buffer.width();
        let height = self.remote_frame_buffer.height();
        let pixels = self.remote_frame_buffer.make_mut_slice();

        for y in 0..height {
            for x in 0..width {
                let index = (y * width + x) as usize;
                let horizon = ((y * 255) / height) as u8;
                let beacon = if (x + phase * 12) % width < width / 8 {
                    36u8
                } else {
                    0u8
                };
                let grid = if x % 120 < 3 || y % 90 < 3 { 12u8 } else { 0u8 };

                pixels[index] = slint::Rgb8Pixel {
                    r: 8u8.saturating_add(horizon / 8).saturating_add(grid),
                    g: 18u8.saturating_add(beacon / 3).saturating_add(grid / 2),
                    b: 34u8.saturating_add(horizon / 3).saturating_add(beacon),
                };
            }
        }

        self.telemetry.rx_stats = SankakuStreamStats {
            bitrate_bps: 0,
            packet_loss_ratio: 0.0,
            jitter_us: 0,
            width: 0,
            height: 0,
        };
        self.remote_tick = self.remote_tick.wrapping_add(1);
    }

    fn mirror_remote_from_frame(&mut self, frame: &VideoFrame) {
        let width = self.remote_frame_buffer.width() as usize;
        let height = self.remote_frame_buffer.height() as usize;
        let bytes = self.remote_frame_buffer.make_mut_bytes();

        for y in 0..height {
            for x in 0..width {
                let remote_index = (y * width + x) * 3;
                let source_x = x * frame.width as usize / width.max(1);
                let source_y = y * frame.height as usize / height.max(1);
                let source_index = (source_y * frame.width as usize + source_x) * 3;
                let source = frame.payload.get(source_index..source_index + 3);

                if let Some(source) = source {
                    bytes[remote_index] = source[2].saturating_add(8);
                    bytes[remote_index + 1] = source[1].saturating_add(6);
                    bytes[remote_index + 2] = source[0];
                }
            }
        }

        self.telemetry.rx_stats = SankakuStreamStats {
            bitrate_bps: self.range_u32(1_250_000, 2_350_000),
            packet_loss_ratio: self.range_f32(0.0010, 0.0180),
            jitter_us: self.range_u32(4_000, 24_000),
            width: frame.width,
            height: frame.height,
        };
    }

    fn timestamp_us() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as u64
    }
}

impl MediaProvider for SankakuStreamProvider {
    fn connect(&mut self, addr: &str) -> Result<(), String> {
        let trimmed = addr.trim();
        if trimmed.is_empty() {
            return Err("address cannot be empty".to_owned());
        }

        self.connected_addr = Some(trimmed.to_owned());
        self.rng_state ^= trimmed.bytes().fold(0u64, |hash, byte| {
            hash.wrapping_mul(16777619) ^ u64::from(byte)
        });
        Ok(())
    }

    fn poll_frame(&mut self) -> Option<slint::Image> {
        if self.connected_addr.is_none() {
            self.render_waiting_remote();
        } else {
            self.remote_tick = self.remote_tick.wrapping_add(1);
            self.telemetry.rx_stats.jitter_us = self.range_u32(5_000, 22_000);
            self.telemetry.rx_stats.packet_loss_ratio = self.range_f32(0.0010, 0.0140);
        }

        Some(slint::Image::from_rgb8(self.remote_frame_buffer.clone()))
    }

    fn poll_local_preview(&mut self) -> Option<slint::Image> {
        Some(slint::Image::from_rgb8(self.local_frame_buffer.clone()))
    }

    fn capture_local_frame(&mut self) -> Option<VideoFrame> {
        if !self.video_enabled {
            self.telemetry.tx_stats = SankakuStreamStats {
                bitrate_bps: 0,
                packet_loss_ratio: 0.0,
                jitter_us: 0,
                width: 0,
                height: 0,
            };
            self.render_local_disabled_frame();
            return None;
        }

        self.render_local_capture_frame();
        let payload = self.local_frame_buffer.as_bytes().to_vec();

        Some(VideoFrame {
            timestamp_us: Self::timestamp_us(),
            keyframe: self.capture_tick % 60 == 0,
            codec: VIDEO_CODEC_HEVC,
            width: self.local_frame_buffer.width(),
            height: self.local_frame_buffer.height(),
            payload,
        })
    }

    fn broadcast_frame(&mut self, frame: VideoFrame) {
        let frames_per_second = 30u32;
        let keyframe_overhead = if frame.keyframe { 1.12 } else { 1.0 };
        let bitrate_bps = ((frame
            .payload
            .len()
            .saturating_mul(frames_per_second as usize)
            .saturating_mul(8) as f32)
            * keyframe_overhead) as u32;
        self.remote_tick = self
            .remote_tick
            .wrapping_add((frame.timestamp_us as u32 & 0xF).max(1));
        self.telemetry.tx_stats = SankakuStreamStats {
            bitrate_bps,
            packet_loss_ratio: self.range_f32(0.0005, 0.0090),
            jitter_us: self.range_u32(2_000, 8_000),
            width: frame.width,
            height: frame.height,
        };

        if self.connected_addr.is_some() && frame.codec == VIDEO_CODEC_HEVC {
            self.mirror_remote_from_frame(&frame);
        }
    }

    fn get_telemetry(&self) -> SankakuTelemetry {
        self.telemetry
    }

    fn set_audio_muted(&mut self, muted: bool) {
        self.audio_muted = muted;
    }

    fn set_video_enabled(&mut self, enabled: bool) {
        self.video_enabled = enabled;
        if !enabled {
            self.render_local_disabled_frame();
        }
    }
}

fn update_peer_model(
    window: &MainWindow,
    peer_model: &slint::VecModel<slint::SharedString>,
    peer_cache: &RefCell<Vec<Peer>>,
    peers: Vec<Peer>,
) {
    let labels = peers
        .iter()
        .map(|peer| slint::SharedString::from(peer.label()))
        .collect::<Vec<_>>();
    peer_model.set_vec(labels);
    *peer_cache.borrow_mut() = peers;

    let selected = window.get_selected_peer_index();
    let count = peer_cache.borrow().len() as i32;
    if count == 0 {
        window.set_selected_peer_index(-1);
    } else if selected < 0 || selected >= count {
        window.set_selected_peer_index(0);
    }
}

fn apply_telemetry(window: &MainWindow, telemetry: SankakuTelemetry) {
    window.set_tx_bitrate(
        format!("{:.2}", telemetry.tx_stats.bitrate_bps as f32 / 1_000_000.0).into(),
    );
    window.set_tx_loss(format!("{:.2}", telemetry.tx_stats.packet_loss_ratio * 100.0).into());
    window.set_rx_jitter(format!("{:.1}", telemetry.rx_stats.jitter_us as f32 / 1_000.0).into());
    window.set_rx_resolution(
        if telemetry.rx_stats.width == 0 || telemetry.rx_stats.height == 0 {
            "Waiting".into()
        } else {
            format!("{}x{}", telemetry.rx_stats.width, telemetry.rx_stats.height).into()
        },
    );
}

fn main() -> Result<(), slint::PlatformError> {
    unsafe {
        sankaku_ffi::init();
        nezumi_ffi::init();
    }

    let mut discovery_engine = DiscoveryEngine::new(
        DEFAULT_MEDIA_PORT,
        vec![
            "Kagami-Full-Duplex".to_owned(),
            "Kagami-Audio".to_owned(),
            "Kagami-Video".to_owned(),
        ],
    );
    let _ = discovery_engine.start();
    let discovery_engine = Rc::new(discovery_engine);

    let main_window = MainWindow::new()?;
    let media_provider: Rc<RefCell<Box<dyn MediaProvider>>> =
        Rc::new(RefCell::new(Box::new(SankakuStreamProvider::new())));
    let peer_cache = Rc::new(RefCell::new(Vec::<Peer>::new()));
    let peer_model = Rc::new(slint::VecModel::<slint::SharedString>::default());
    main_window.set_peers(peer_model.clone().into());

    {
        let mut provider = media_provider.borrow_mut();
        if let Some(remote) = provider.poll_frame() {
            main_window.set_remote_video_frame(remote);
        }
        if let Some(local) = provider.poll_local_preview() {
            main_window.set_local_video_frame(local);
        }
        apply_telemetry(&main_window, provider.get_telemetry());
    }
    update_peer_model(
        &main_window,
        peer_model.as_ref(),
        peer_cache.as_ref(),
        discovery_engine.peers_snapshot(),
    );

    let connect_provider = Rc::clone(&media_provider);
    let connect_window = main_window.as_weak();
    let connect_peer_cache = Rc::clone(&peer_cache);
    main_window.on_connect_requested(move || {
        let Some(window) = connect_window.upgrade() else {
            return;
        };

        let selected = window.get_selected_peer_index();
        if selected < 0 {
            return;
        }
        let peers = connect_peer_cache.borrow();
        let Some(peer) = peers.get(selected as usize) else {
            return;
        };

        let mut provider = connect_provider.borrow_mut();
        if provider.connect(&peer.addr.to_string()).is_ok() {
            apply_telemetry(&window, provider.get_telemetry());
        }
    });

    let audio_provider = Rc::clone(&media_provider);
    main_window.on_audio_muted_toggled(move |muted| {
        audio_provider.borrow_mut().set_audio_muted(muted);
    });

    let video_provider = Rc::clone(&media_provider);
    main_window.on_video_stopped_toggled(move |stopped| {
        video_provider.borrow_mut().set_video_enabled(!stopped);
    });

    let capture_timer = slint::Timer::default();
    let capture_provider = Rc::clone(&media_provider);
    let capture_window = main_window.as_weak();
    capture_timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(33),
        move || {
            let Some(window) = capture_window.upgrade() else {
                return;
            };
            let mut provider = capture_provider.borrow_mut();
            if let Some(frame) = provider.capture_local_frame() {
                provider.broadcast_frame(frame);
            }
            if let Some(local) = provider.poll_local_preview() {
                window.set_local_video_frame(local);
            }
            apply_telemetry(&window, provider.get_telemetry());
        },
    );

    let render_timer = slint::Timer::default();
    let render_provider = Rc::clone(&media_provider);
    let render_window = main_window.as_weak();
    render_timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(16),
        move || {
            let Some(window) = render_window.upgrade() else {
                return;
            };
            let mut provider = render_provider.borrow_mut();
            if let Some(remote) = provider.poll_frame() {
                window.set_remote_video_frame(remote);
            }
            apply_telemetry(&window, provider.get_telemetry());
        },
    );

    let discovery_timer = slint::Timer::default();
    let discovery_window = main_window.as_weak();
    let discovery_model = Rc::clone(&peer_model);
    let discovery_cache = Rc::clone(&peer_cache);
    let discovery_engine_ref = Rc::clone(&discovery_engine);
    discovery_timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(750),
        move || {
            let Some(window) = discovery_window.upgrade() else {
                return;
            };
            update_peer_model(
                &window,
                discovery_model.as_ref(),
                discovery_cache.as_ref(),
                discovery_engine_ref.peers_snapshot(),
            );
        },
    );

    main_window.run()
}

#[cfg(test)]
mod tests {
    use super::{nezumi_ffi, sankaku_ffi};

    #[test]
    fn test_ffi_init() {
        unsafe {
            sankaku_ffi::init();
            nezumi_ffi::init();
        }
    }
}
