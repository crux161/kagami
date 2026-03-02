mod discovery;
mod nezumi;
mod nezumi_ffi;
mod sankaku_ffi;

use std::cell::RefCell;
use std::net::{Ipv4Addr, SocketAddr, UdpSocket};
use std::process;
use std::rc::Rc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use discovery::{DiscoveryManager, Peer};
use nezumi::NezumiProducer;
use sankaku_ffi::{SankakuStreamStats, SankakuTelemetry, VideoFrame, VIDEO_CODEC_HEVC};

slint::include_modules!();

const PREFERRED_RECEIVER_PORT: u16 = 9292;

trait MediaProvider {
    fn connect(&mut self, addr: &str) -> Result<(), String>;
    fn local_receiver_port(&self) -> u16;
    fn poll_frame(&mut self) -> Option<slint::Image>;
    fn poll_local_preview(&mut self) -> Option<slint::Image>;
    fn capture_local_frame(&mut self) -> Option<VideoFrame>;
    fn broadcast_frame(&mut self, frame: VideoFrame);
    fn get_telemetry(&self) -> SankakuTelemetry;
    fn set_audio_muted(&mut self, muted: bool);
    fn set_video_enabled(&mut self, enabled: bool);
}

// ---------------------------------------------------------------------------
// RealDuplexSession — backed by a NezumiProducer (AVFoundation on macOS)
// ---------------------------------------------------------------------------

#[allow(dead_code)]
struct RealDuplexSession {
    producer: Box<dyn nezumi::NezumiProducer>,
    connected_addr: Option<String>,
    audio_muted: bool,
    video_enabled: bool,
    receiver_socket: UdpSocket,
    local_preview_buffer: slint::SharedPixelBuffer<slint::Rgb8Pixel>,
    remote_frame_buffer: slint::SharedPixelBuffer<slint::Rgb8Pixel>,
    telemetry: SankakuTelemetry,
    capture_width: u32,
    capture_height: u32,
    frames_sent: u64,
}

#[allow(dead_code)]
impl RealDuplexSession {
    fn new(producer: Box<dyn nezumi::NezumiProducer>, width: u32, height: u32) -> Self {
        Self {
            producer,
            connected_addr: None,
            audio_muted: false,
            video_enabled: true,
            receiver_socket: bind_receiver_socket(),
            local_preview_buffer: slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(width, height),
            remote_frame_buffer: slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(1280, 720),
            telemetry: SankakuTelemetry::default(),
            capture_width: width,
            capture_height: height,
            frames_sent: 0,
        }
    }

    fn timestamp_us() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as u64
    }

    fn render_disabled_preview(&mut self) {
        let pixels = self.local_preview_buffer.make_mut_slice();
        for pixel in pixels {
            *pixel = slint::Rgb8Pixel {
                r: 18,
                g: 18,
                b: 24,
            };
        }
    }
}

impl MediaProvider for RealDuplexSession {
    fn connect(&mut self, addr: &str) -> Result<(), String> {
        let trimmed = addr.trim();
        if trimmed.is_empty() {
            return Err("address cannot be empty".to_owned());
        }
        self.connected_addr = Some(trimmed.to_owned());
        log::info!("RealDuplexSession connected to {trimmed}");
        Ok(())
    }

    fn local_receiver_port(&self) -> u16 {
        self.receiver_socket
            .local_addr()
            .expect("receiver socket should expose a local address")
            .port()
    }

    fn poll_frame(&mut self) -> Option<slint::Image> {
        // TODO: decode inbound H.265 from Sankaku when transport is wired
        Some(slint::Image::from_rgb8(self.remote_frame_buffer.clone()))
    }

    fn poll_local_preview(&mut self) -> Option<slint::Image> {
        if !self.video_enabled {
            return Some(slint::Image::from_rgb8(self.local_preview_buffer.clone()));
        }
        if let Some(frame) = self.producer.next_preview_frame() {
            let expected_len = (frame.width * frame.height * 3) as usize;
            if frame.data.len() >= expected_len {
                if frame.width != self.local_preview_buffer.width()
                    || frame.height != self.local_preview_buffer.height()
                {
                    self.local_preview_buffer =
                        slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(
                            frame.width,
                            frame.height,
                        );
                }
                let dest = self.local_preview_buffer.make_mut_slice();
                let src = &frame.data[..expected_len];
                // RGB8 bytes map directly to Rgb8Pixel layout
                let dest_bytes = unsafe {
                    std::slice::from_raw_parts_mut(
                        dest.as_mut_ptr() as *mut u8,
                        expected_len,
                    )
                };
                dest_bytes.copy_from_slice(src);
            }
        }
        Some(slint::Image::from_rgb8(self.local_preview_buffer.clone()))
    }

    fn capture_local_frame(&mut self) -> Option<VideoFrame> {
        if !self.video_enabled {
            self.telemetry.tx_stats = SankakuStreamStats::default();
            self.telemetry.udp_tx_dropped = 0;
            self.render_disabled_preview();
            return None;
        }

        let packet = self.producer.next_packet()?;
        self.frames_sent += 1;

        self.telemetry.tx_stats = SankakuStreamStats {
            bitrate_bps: (packet.data.len() as u32) * 8 * 30,
            packet_loss_ratio: 0.0,
            jitter_us: 0,
            latency_ms: 0,
            width: packet.width,
            height: packet.height,
        };

        Some(
            VideoFrame::nal_with_codec(
                packet.data,
                packet.timestamp_us,
                packet.keyframe,
                VIDEO_CODEC_HEVC,
            )
            .with_dimensions(packet.width, packet.height),
        )
    }

    fn broadcast_frame(&mut self, frame: VideoFrame) {
        if self.connected_addr.is_some() && frame.codec == VIDEO_CODEC_HEVC {
            log::trace!(
                "broadcast H.265 frame: {}B keyframe={}",
                frame.payload.len(),
                frame.keyframe
            );
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
            self.render_disabled_preview();
        }
    }
}

#[derive(Clone, Copy)]
struct AnimatedSquare {
    x: i32,
    y: i32,
    vx: i32,
    vy: i32,
    size: u32,
}

struct MockDuplexSession {
    connected_addr: Option<String>,
    audio_muted: bool,
    video_enabled: bool,
    receiver_socket: UdpSocket,
    local_frame_buffer: slint::SharedPixelBuffer<slint::Rgb8Pixel>,
    remote_frame_buffer: slint::SharedPixelBuffer<slint::Rgb8Pixel>,
    local_square: AnimatedSquare,
    remote_square: AnimatedSquare,
    local_phase: u32,
    remote_phase: u32,
    telemetry: SankakuTelemetry,
    rng_state: u64,
}

fn bind_receiver_socket() -> UdpSocket {
    UdpSocket::bind((Ipv4Addr::UNSPECIFIED, PREFERRED_RECEIVER_PORT))
        .or_else(|_| UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)))
        .expect("failed to bind local receiver socket")
}

fn local_instance_id() -> String {
    let host = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "kagami".to_owned())
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_owned();
    let host = if host.is_empty() {
        "kagami".to_owned()
    } else {
        host
    };

    format!(
        "{}-{:x}",
        host,
        process::id() as u64
            ^ SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64
    )
}

impl MockDuplexSession {
    fn new() -> Self {
        let receiver_socket = bind_receiver_socket();
        let mut session = Self {
            connected_addr: None,
            audio_muted: false,
            video_enabled: true,
            receiver_socket,
            local_frame_buffer: slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(320, 180),
            remote_frame_buffer: slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(1280, 720),
            local_square: AnimatedSquare {
                x: 18,
                y: 14,
                vx: 4,
                vy: 3,
                size: 54,
            },
            remote_square: AnimatedSquare {
                x: 96,
                y: 72,
                vx: 7,
                vy: 5,
                size: 108,
            },
            local_phase: 0,
            remote_phase: 0,
            telemetry: SankakuTelemetry::default(),
            rng_state: 0x5A17_2D3C_4B91_08EF,
        };
        session.render_local_frame();
        session.render_remote_frame();
        session
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

    fn advance_square(square: &mut AnimatedSquare, width: u32, height: u32) {
        let max_x = (width as i32 - square.size as i32).max(0);
        let max_y = (height as i32 - square.size as i32).max(0);

        square.x += square.vx;
        square.y += square.vy;

        if square.x <= 0 || square.x >= max_x {
            square.x = square.x.clamp(0, max_x);
            square.vx = -square.vx;
        }

        if square.y <= 0 || square.y >= max_y {
            square.y = square.y.clamp(0, max_y);
            square.vy = -square.vy;
        }
    }

    fn render_local_frame(&mut self) {
        let width = self.local_frame_buffer.width();
        let height = self.local_frame_buffer.height();
        Self::advance_square(&mut self.local_square, width, height);

        let square = self.local_square;
        let phase = self.local_phase;
        let accent = if self.audio_muted { 30u8 } else { 0u8 };
        let pixels = self.local_frame_buffer.make_mut_slice();

        for y in 0..height {
            for x in 0..width {
                let index = (y * width + x) as usize;
                let in_square = x >= square.x as u32
                    && x < square.x as u32 + square.size
                    && y >= square.y as u32
                    && y < square.y as u32 + square.size;
                let grid = if x % 20 == 0 || y % 20 == 0 { 8u8 } else { 0u8 };
                let scan = (((x + phase) % width.max(1)) * 255 / width.max(1)) as u8;
                let status_bar = if y < 16 { 18u8 } else { 0u8 };

                pixels[index] = if in_square {
                    slint::Rgb8Pixel {
                        r: 24u8.saturating_add(accent),
                        g: 104u8.saturating_add(scan / 8),
                        b: 232,
                    }
                } else {
                    slint::Rgb8Pixel {
                        r: 8u8.saturating_add(status_bar / 2),
                        g: 18u8.saturating_add(grid / 2),
                        b: 44u8.saturating_add(grid).saturating_add(scan / 10),
                    }
                };
            }
        }

        self.local_phase = self.local_phase.wrapping_add(1);
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

    fn render_remote_frame(&mut self) {
        let width = self.remote_frame_buffer.width();
        let height = self.remote_frame_buffer.height();
        Self::advance_square(&mut self.remote_square, width, height);

        let square = self.remote_square;
        let phase = self.remote_phase;
        let connected_bias = if self.connected_addr.is_some() {
            24u8
        } else {
            0u8
        };
        let pixels = self.remote_frame_buffer.make_mut_slice();

        for y in 0..height {
            for x in 0..width {
                let index = (y * width + x) as usize;
                let in_square = x >= square.x as u32
                    && x < square.x as u32 + square.size
                    && y >= square.y as u32
                    && y < square.y as u32 + square.size;
                let band = (((x / 6) + phase) % 255) as u8;
                let horizon = ((y * 255) / height.max(1)) as u8;
                let grid = if x % 80 < 2 || y % 60 < 2 { 10u8 } else { 0u8 };

                pixels[index] = if in_square {
                    slint::Rgb8Pixel {
                        r: 232,
                        g: 62u8.saturating_add(connected_bias / 2),
                        b: 52,
                    }
                } else {
                    slint::Rgb8Pixel {
                        r: 18u8.saturating_add(band / 8).saturating_add(grid / 2),
                        g: 18u8
                            .saturating_add(horizon / 7)
                            .saturating_add(connected_bias / 3),
                        b: 42u8.saturating_add(band / 3).saturating_add(grid),
                    }
                };
            }
        }

        self.remote_phase = self.remote_phase.wrapping_add(1);
    }

    fn timestamp_us() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as u64
    }
}

impl MediaProvider for MockDuplexSession {
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

    fn local_receiver_port(&self) -> u16 {
        self.receiver_socket
            .local_addr()
            .expect("receiver socket should expose a local address")
            .port()
    }

    fn poll_frame(&mut self) -> Option<slint::Image> {
        self.render_remote_frame();

        self.telemetry.rx_stats = if self.connected_addr.is_some() {
            SankakuStreamStats {
                bitrate_bps: self.range_u32(1_350_000, 2_250_000),
                packet_loss_ratio: self.range_f32(0.0010, 0.0140),
                jitter_us: self.range_u32(5_000, 22_000),
                latency_ms: self.range_u32(28, 96),
                width: self.remote_frame_buffer.width(),
                height: self.remote_frame_buffer.height(),
            }
        } else {
            SankakuStreamStats::default()
        };
        self.telemetry.path_rtt_ms = if self.connected_addr.is_some() {
            self.range_u32(24, 78) as u64
        } else {
            0
        };

        Some(slint::Image::from_rgb8(self.remote_frame_buffer.clone()))
    }

    fn poll_local_preview(&mut self) -> Option<slint::Image> {
        Some(slint::Image::from_rgb8(self.local_frame_buffer.clone()))
    }

    fn capture_local_frame(&mut self) -> Option<VideoFrame> {
        if !self.video_enabled {
            self.telemetry.tx_stats = SankakuStreamStats::default();
            self.telemetry.udp_tx_dropped = 0;
            self.render_local_disabled_frame();
            return None;
        }

        self.render_local_frame();
        let payload = self.local_frame_buffer.as_bytes().to_vec();

        Some(
            VideoFrame::nal_with_codec(
                payload,
                Self::timestamp_us(),
                self.local_phase % 45 == 0,
                VIDEO_CODEC_HEVC,
            )
            .with_dimensions(
                self.local_frame_buffer.width(),
                self.local_frame_buffer.height(),
            ),
        )
    }

    fn broadcast_frame(&mut self, frame: VideoFrame) {
        if self.connected_addr.is_some() && frame.codec == VIDEO_CODEC_HEVC {
            self.telemetry.tx_stats = SankakuStreamStats {
                bitrate_bps: self.range_u32(1_550_000, 2_550_000),
                packet_loss_ratio: self.range_f32(0.0005, 0.0090),
                jitter_us: self.range_u32(2_000, 8_000),
                latency_ms: 0,
                width: frame.width,
                height: frame.height,
            };
            self.telemetry.udp_tx_dropped = self
                .telemetry
                .udp_tx_dropped
                .saturating_add(u64::from(self.range_u32(0, 2)));
        } else {
            self.telemetry.tx_stats = SankakuStreamStats::default();
            self.telemetry.udp_tx_dropped = 0;
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

struct ReceiverLoopState {
    bound_port: u16,
    accepted_peer: Option<SocketAddr>,
}

struct SenderLoopState {
    target_peer: Option<SocketAddr>,
}

struct SessionManager {
    media_provider: Box<dyn MediaProvider>,
    discovery_manager: Option<DiscoveryManager>,
    connected_peer: Option<Peer>,
    receiver_loop: ReceiverLoopState,
    sender_loop: SenderLoopState,
}

impl SessionManager {
    fn new(media_provider: Box<dyn MediaProvider>) -> Self {
        let bound_port = media_provider.local_receiver_port();
        let discovery_manager = DiscoveryManager::new(
            bound_port,
            local_instance_id(),
            vec![
                "Kagami-Full-Duplex".to_owned(),
                "Kagami-Audio".to_owned(),
                "Kagami-Video".to_owned(),
            ],
        )
        .ok();

        Self {
            media_provider,
            discovery_manager,
            connected_peer: None,
            receiver_loop: ReceiverLoopState {
                bound_port,
                accepted_peer: None,
            },
            sender_loop: SenderLoopState { target_peer: None },
        }
    }

    fn peers_snapshot(&self) -> Vec<Peer> {
        self.discovery_manager
            .as_ref()
            .map_or_else(Vec::new, DiscoveryManager::peers_snapshot)
    }

    fn connect_peer(&mut self, peer: Peer) -> Result<(), String> {
        self.media_provider.connect(&peer.addr.to_string())?;
        self.sender_loop.target_peer = Some(peer.addr);
        self.receiver_loop.accepted_peer = Some(peer.addr);
        self.connected_peer = Some(peer);
        Ok(())
    }

    fn poll_remote_frame(&mut self) -> Option<slint::Image> {
        let _receiver_port = self.receiver_loop.bound_port;
        let _accepted_peer = self.receiver_loop.accepted_peer;
        self.media_provider.poll_frame()
    }

    fn poll_local_preview(&mut self) -> Option<slint::Image> {
        self.media_provider.poll_local_preview()
    }

    fn capture_and_broadcast(&mut self) {
        if let Some(frame) = self.media_provider.capture_local_frame() {
            self.media_provider.broadcast_frame(frame);
        }
    }

    fn telemetry(&self) -> SankakuTelemetry {
        self.media_provider.get_telemetry()
    }

    fn set_audio_muted(&mut self, muted: bool) {
        self.media_provider.set_audio_muted(muted);
    }

    fn set_video_enabled(&mut self, enabled: bool) {
        self.media_provider.set_video_enabled(enabled);
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
    window.set_upload_bitrate(
        format!("{:.2}", telemetry.tx_stats.bitrate_bps as f32 / 1_000_000.0).into(),
    );
    window.set_upload_loss(format!("{:.2}", telemetry.tx_stats.packet_loss_ratio * 100.0).into());
    window.set_download_jitter(
        format!("{:.1}", telemetry.rx_stats.jitter_us as f32 / 1_000.0).into(),
    );
    window.set_download_latency(if telemetry.rx_stats.latency_ms == 0 {
        "Waiting".into()
    } else {
        format!("{}ms", telemetry.rx_stats.latency_ms).into()
    });
    window.set_path_rtt_ms(if telemetry.path_rtt_ms == 0 {
        "Waiting".into()
    } else {
        format!("{}ms", telemetry.path_rtt_ms).into()
    });
    window.set_udp_tx_dropped(telemetry.udp_tx_dropped.to_string().into());
}

fn create_media_provider() -> Box<dyn MediaProvider> {
    #[cfg(target_os = "macos")]
    {
        const CAPTURE_W: u32 = 1280;
        const CAPTURE_H: u32 = 720;

        match nezumi::avfoundation::AvFoundationProducer::new(CAPTURE_W, CAPTURE_H) {
            Ok(mut producer) => {
                if let Err(e) = producer.start_reading() {
                    log::warn!("AVFoundation start_reading failed: {e} — falling back to mock");
                    return Box::new(MockDuplexSession::new());
                }
                log::info!("using AVFoundation capture at {CAPTURE_W}x{CAPTURE_H}");
                return Box::new(RealDuplexSession::new(
                    Box::new(producer),
                    CAPTURE_W,
                    CAPTURE_H,
                ));
            }
            Err(e) => {
                log::warn!("AVFoundation init failed: {e} — falling back to mock");
            }
        }
    }
    Box::new(MockDuplexSession::new())
}

fn main() -> Result<(), slint::PlatformError> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    unsafe {
        sankaku_ffi::init();
        nezumi_ffi::init();
    }

    let main_window = MainWindow::new()?;
    let session_manager = Rc::new(RefCell::new(SessionManager::new(
        create_media_provider(),
    )));
    let peer_cache = Rc::new(RefCell::new(Vec::<Peer>::new()));
    let peer_model = Rc::new(slint::VecModel::<slint::SharedString>::default());
    main_window.set_discovered_peers(peer_model.clone().into());
    main_window.set_selected_peer_index(-1);

    {
        let mut session = session_manager.borrow_mut();
        session.capture_and_broadcast();
        if let Some(remote) = session.poll_remote_frame() {
            main_window.set_remote_video_frame(remote);
        }
        if let Some(local) = session.poll_local_preview() {
            main_window.set_local_video_frame(local);
        }
        apply_telemetry(&main_window, session.telemetry());
    }
    update_peer_model(
        &main_window,
        peer_model.as_ref(),
        peer_cache.as_ref(),
        session_manager.borrow().peers_snapshot(),
    );

    let connect_session = Rc::clone(&session_manager);
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

        let mut session = connect_session.borrow_mut();
        if session.connect_peer(peer.clone()).is_ok() {
            apply_telemetry(&window, session.telemetry());
        }
    });

    let audio_session = Rc::clone(&session_manager);
    main_window.on_audio_muted_toggled(move |muted| {
        audio_session.borrow_mut().set_audio_muted(muted);
    });

    let video_session = Rc::clone(&session_manager);
    main_window.on_video_stopped_toggled(move |stopped| {
        video_session.borrow_mut().set_video_enabled(!stopped);
    });

    let media_timer = slint::Timer::default();
    let media_session = Rc::clone(&session_manager);
    let media_window = main_window.as_weak();
    media_timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(16),
        move || {
            let Some(window) = media_window.upgrade() else {
                return;
            };
            let mut session = media_session.borrow_mut();
            session.capture_and_broadcast();
            if let Some(local) = session.poll_local_preview() {
                window.set_local_video_frame(local);
            }
            if let Some(remote) = session.poll_remote_frame() {
                window.set_remote_video_frame(remote);
            }
            apply_telemetry(&window, session.telemetry());
        },
    );

    let discovery_timer = slint::Timer::default();
    let discovery_window = main_window.as_weak();
    let discovery_model = Rc::clone(&peer_model);
    let discovery_cache = Rc::clone(&peer_cache);
    let discovery_session = Rc::clone(&session_manager);
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
                discovery_session.borrow().peers_snapshot(),
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
