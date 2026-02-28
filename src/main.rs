mod nezumi_ffi;
mod sankaku_ffi;

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use sankaku_ffi::SankakuTelemetry;

slint::include_modules!();

trait MediaProvider {
    fn connect(&mut self, addr: &str) -> Result<(), String>;
    fn poll_frame(&mut self) -> Option<slint::Image>;
    fn get_telemetry(&self) -> SankakuTelemetry;
}

struct MockSankaku {
    connected_addr: Option<String>,
    frame_buffer: slint::SharedPixelBuffer<slint::Rgb8Pixel>,
    frame_counter: u32,
    telemetry: SankakuTelemetry,
    rng_state: u64,
}

impl MockSankaku {
    fn new() -> Self {
        let mut provider = Self {
            connected_addr: None,
            frame_buffer: slint::SharedPixelBuffer::<slint::Rgb8Pixel>::new(640, 480),
            frame_counter: 0,
            telemetry: SankakuTelemetry::default(),
            rng_state: 0x5A17_2D3C_4B91_08EF,
        };
        provider.refresh_telemetry();
        provider.render_frame();
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

    fn refresh_telemetry(&mut self) {
        let bitrate_floor = if self.connected_addr.is_some() {
            1_650_000
        } else {
            1_500_000
        };
        let bitrate_ceiling = if self.connected_addr.is_some() {
            2_450_000
        } else {
            2_250_000
        };
        let jitter_min = if self.connected_addr.is_some() {
            4_000
        } else {
            6_000
        };
        let jitter_max = if self.connected_addr.is_some() {
            22_000
        } else {
            28_000
        };

        self.telemetry = SankakuTelemetry {
            bitrate_bps: self.range_u32(bitrate_floor, bitrate_ceiling),
            packet_loss_ratio: self.range_f32(0.0015, 0.0240),
            jitter_us: self.range_u32(jitter_min, jitter_max),
        };
    }

    fn fold_address(addr: &str) -> u64 {
        let mut hash = 1469598103934665603u64;
        for byte in addr.bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(1099511628211);
        }
        hash
    }

    fn render_frame(&mut self) {
        const WIDTH: u32 = 640;
        const HEIGHT: u32 = 480;

        let phase = self.frame_counter;
        let live_boost = if self.connected_addr.is_some() {
            42u8
        } else {
            0u8
        };
        let buffer = self.frame_buffer.make_mut_slice();

        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let index = (y * WIDTH + x) as usize;
                let sweep = ((x + phase * 6) % WIDTH) as u8;
                let pulse = ((y + phase * 3) % HEIGHT) as u8;
                let stripes = if ((x / 48) + (phase / 6)) % 2 == 0 {
                    18u8
                } else {
                    0u8
                };
                let reticle = if self.connected_addr.is_some() && (x + phase * 8) % WIDTH < 96 {
                    28u8
                } else {
                    0u8
                };

                buffer[index] = slint::Rgb8Pixel {
                    r: 14u8.saturating_add(sweep / 7).saturating_add(stripes / 2),
                    g: 32u8
                        .saturating_add(pulse / 5)
                        .saturating_add(live_boost / 2)
                        .saturating_add(reticle / 2),
                    b: 64u8
                        .saturating_add(sweep / 4)
                        .saturating_add(stripes)
                        .saturating_add(live_boost)
                        .saturating_add(reticle),
                };
            }
        }

        self.frame_counter = self.frame_counter.wrapping_add(1);
    }
}

impl MediaProvider for MockSankaku {
    fn connect(&mut self, addr: &str) -> Result<(), String> {
        let trimmed = addr.trim();
        if trimmed.is_empty() {
            return Err("address cannot be empty".to_owned());
        }

        self.connected_addr = Some(trimmed.to_owned());
        self.rng_state ^= Self::fold_address(trimmed);
        self.refresh_telemetry();
        self.render_frame();
        Ok(())
    }

    fn poll_frame(&mut self) -> Option<slint::Image> {
        self.refresh_telemetry();
        self.render_frame();
        Some(slint::Image::from_rgb8(self.frame_buffer.clone()))
    }

    fn get_telemetry(&self) -> SankakuTelemetry {
        self.telemetry
    }
}

fn refresh_window(window: &MainWindow, provider: &mut dyn MediaProvider) {
    if let Some(frame) = provider.poll_frame() {
        window.set_video_frame(frame);
    }

    let telemetry = provider.get_telemetry();
    window.set_current_bitrate(format!("{:.2}", telemetry.bitrate_bps as f32 / 1_000_000.0).into());
    window.set_loss_ratio(format!("{:.2}", telemetry.packet_loss_ratio * 100.0).into());
    window.set_jitter(format!("{:.1}", telemetry.jitter_us as f32 / 1_000.0).into());
}

fn main() -> Result<(), slint::PlatformError> {
    unsafe {
        sankaku_ffi::init();
        nezumi_ffi::init();
    }

    let main_window = MainWindow::new()?;
    let media_provider: Rc<RefCell<Box<dyn MediaProvider>>> =
        Rc::new(RefCell::new(Box::new(MockSankaku::new())));

    {
        let mut provider = media_provider.borrow_mut();
        refresh_window(&main_window, provider.as_mut());
    }

    let connect_provider = Rc::clone(&media_provider);
    let connect_window = main_window.as_weak();
    main_window.on_connect_requested(move |addr| {
        let Some(window) = connect_window.upgrade() else {
            return;
        };
        let mut provider = connect_provider.borrow_mut();
        if provider.connect(addr.as_str()).is_ok() {
            refresh_window(&window, provider.as_mut());
        }
    });

    let frame_timer = slint::Timer::default();
    let timer_provider = Rc::clone(&media_provider);
    let timer_window = main_window.as_weak();
    frame_timer.start(
        slint::TimerMode::Repeated,
        Duration::from_millis(16),
        move || {
            let Some(window) = timer_window.upgrade() else {
                return;
            };
            let mut provider = timer_provider.borrow_mut();
            refresh_window(&window, provider.as_mut());
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
