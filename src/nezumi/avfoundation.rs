use std::ffi::c_void;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObjectProtocol, ProtocolObject};
use objc2::{define_class, msg_send, AllocAnyThread, DefinedClass};
use objc2_av_foundation::{
    AVCaptureConnection, AVCaptureDevice, AVCaptureDeviceInput, AVCaptureOutput,
    AVCaptureSession, AVCaptureSessionPreset1280x720, AVCaptureVideoDataOutput,
    AVCaptureVideoDataOutputSampleBufferDelegate, AVMediaTypeVideo,
};
use objc2_core_media::CMSampleBuffer;
use objc2_foundation::{NSDictionary, NSNumber, NSObject, NSString};

use super::{MediaPacket, MediaTrack, NezumiProducer, PreviewFrame, TrackKind};

const HEVC_CODEC_ID: u8 = 0x01;

// CoreVideo pixel format constants
const K_CV_PIXEL_FORMAT_TYPE_32BGRA: u32 = 0x42475241; // 'BGRA'

// VideoToolbox / CoreMedia / CoreVideo C API types and functions.
// The objc2 ecosystem wraps ObjC classes but VTCompressionSession is a
// CoreFoundation / C-function API, so we bind the handful of symbols we need.
type CFTypeRef = *const c_void;
type CFAllocatorRef = *const c_void;
type CFDictionaryRef = *const c_void;
type CFStringRef = *const c_void;
type CFBooleanRef = *const c_void;
type CFNumberRef = *const c_void;
type CVPixelBufferRef = *const c_void;
type CVImageBufferRef = CVPixelBufferRef;
type CMSampleBufferRef = *const c_void;
type CMBlockBufferRef = *const c_void;
type VTCompressionSessionRef = *mut c_void;
type CMTime = CMTimeRepr;
type OSStatus = i32;

#[repr(C)]
#[derive(Clone, Copy)]
struct CMTimeRepr {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

const K_CM_TIME_FLAGS_VALID: u32 = 1;

fn cm_time(value: i64, timescale: i32) -> CMTimeRepr {
    CMTimeRepr {
        value,
        timescale,
        flags: K_CM_TIME_FLAGS_VALID,
        epoch: 0,
    }
}

#[allow(dead_code)]
const K_CM_TIME_INVALID: CMTimeRepr = CMTimeRepr {
    value: 0,
    timescale: 0,
    flags: 0,
    epoch: 0,
};

type VTCompressionOutputCallback = unsafe extern "C" fn(
    output_callback_ref_con: *mut c_void,
    source_frame_ref_con: *mut c_void,
    status: OSStatus,
    info_flags: u32,
    sample_buffer: CMSampleBufferRef,
);

unsafe extern "C" {
    // CoreFoundation
    fn CFRelease(cf: CFTypeRef);

    // CoreMedia
    fn CMSampleBufferGetImageBuffer(sbuf: CMSampleBufferRef) -> CVImageBufferRef;
    fn CMSampleBufferGetDataBuffer(sbuf: CMSampleBufferRef) -> CMBlockBufferRef;
    fn CMBlockBufferGetDataLength(block: CMBlockBufferRef) -> usize;
    fn CMBlockBufferCopyDataBytes(
        block: CMBlockBufferRef,
        offset_to_data: usize,
        data_length: usize,
        destination: *mut u8,
    ) -> OSStatus;
    fn CMSampleBufferGetSampleAttachmentsArray(
        sbuf: CMSampleBufferRef,
        create_if_necessary: u8,
    ) -> CFTypeRef;

    // CoreVideo
    fn CVPixelBufferLockBaseAddress(pixel_buffer: CVPixelBufferRef, flags: u64) -> OSStatus;
    fn CVPixelBufferUnlockBaseAddress(pixel_buffer: CVPixelBufferRef, flags: u64) -> OSStatus;
    fn CVPixelBufferGetBaseAddress(pixel_buffer: CVPixelBufferRef) -> *const u8;
    fn CVPixelBufferGetWidth(pixel_buffer: CVPixelBufferRef) -> usize;
    fn CVPixelBufferGetHeight(pixel_buffer: CVPixelBufferRef) -> usize;
    fn CVPixelBufferGetBytesPerRow(pixel_buffer: CVPixelBufferRef) -> usize;

    // VideoToolbox
    fn VTCompressionSessionCreate(
        allocator: CFAllocatorRef,
        width: i32,
        height: i32,
        codec_type: u32,
        encoder_specification: CFDictionaryRef,
        source_image_buffer_attributes: CFDictionaryRef,
        compressed_data_allocator: CFAllocatorRef,
        output_callback: VTCompressionOutputCallback,
        output_callback_ref_con: *mut c_void,
        compression_session_out: *mut VTCompressionSessionRef,
    ) -> OSStatus;
    fn VTCompressionSessionEncodeFrame(
        session: VTCompressionSessionRef,
        image_buffer: CVImageBufferRef,
        presentation_time_stamp: CMTime,
        duration: CMTime,
        frame_properties: CFDictionaryRef,
        source_frame_ref_con: *mut c_void,
        info_flags_out: *mut u32,
    ) -> OSStatus;
    fn VTSessionSetProperty(
        session: CFTypeRef,
        property_key: CFStringRef,
        property_value: CFTypeRef,
    ) -> OSStatus;
    fn VTCompressionSessionPrepareToEncodeFrames(session: VTCompressionSessionRef) -> OSStatus;
    fn VTCompressionSessionInvalidate(session: VTCompressionSessionRef);

    // CoreFoundation helpers
    static kCFBooleanTrue: CFBooleanRef;
    static kCFAllocatorDefault: CFAllocatorRef;
    static kCMSampleAttachmentKey_NotSync: CFStringRef;

    // VideoToolbox property keys
    static kVTCompressionPropertyKey_RealTime: CFStringRef;
    static kVTCompressionPropertyKey_ProfileLevel: CFStringRef;
    static kVTCompressionPropertyKey_MaxKeyFrameInterval: CFStringRef;
    static kVTCompressionPropertyKey_ExpectedFrameRate: CFStringRef;
    static kVTCompressionPropertyKey_AverageBitRate: CFStringRef;
    static kVTProfileLevel_HEVC_Main_AutoLevel: CFStringRef;

    // CoreFoundation number
    fn CFNumberCreate(
        allocator: CFAllocatorRef,
        the_type: i64,
        value_ptr: *const c_void,
    ) -> CFNumberRef;

    // CFArray
    fn CFArrayGetCount(the_array: CFTypeRef) -> isize;
    fn CFArrayGetValueAtIndex(the_array: CFTypeRef, idx: isize) -> CFTypeRef;

    // CFDictionary
    fn CFDictionaryGetValue(the_dict: CFDictionaryRef, key: CFTypeRef) -> CFTypeRef;
    fn CFBooleanGetValue(boolean: CFBooleanRef) -> u8;

    // Pixel format dictionary
    #[allow(dead_code)]
    fn CVPixelBufferGetPixelFormatType(pixel_buffer: CVPixelBufferRef) -> u32;
}

// kCFNumberSInt32Type = 3, kCFNumberFloat64Type = 13
const K_CF_NUMBER_SINT32_TYPE: i64 = 3;
const K_CF_NUMBER_FLOAT64_TYPE: i64 = 13;

// kCMVideoCodecType_HEVC = 'hvc1' = 0x68766331
const K_CM_VIDEO_CODEC_TYPE_HEVC: u32 = 0x6876_6331;

// CVPixelBuffer lock flags
const K_CV_PIXEL_BUFFER_LOCK_READ_ONLY: u64 = 0x0000_0001;

struct EncoderContext {
    packet_tx: mpsc::Sender<MediaPacket>,
    width: u32,
    height: u32,
}

unsafe extern "C" fn vt_output_callback(
    output_callback_ref_con: *mut c_void,
    _source_frame_ref_con: *mut c_void,
    status: OSStatus,
    _info_flags: u32,
    sample_buffer: CMSampleBufferRef,
) {
    if status != 0 || sample_buffer.is_null() {
        return;
    }

    let ctx = &*(output_callback_ref_con as *const EncoderContext);

    let keyframe = is_keyframe(sample_buffer);

    let block_buf = CMSampleBufferGetDataBuffer(sample_buffer);
    if block_buf.is_null() {
        return;
    }

    let data_len = CMBlockBufferGetDataLength(block_buf);
    if data_len == 0 {
        return;
    }

    let mut nal_data = vec![0u8; data_len];
    let copy_status =
        CMBlockBufferCopyDataBytes(block_buf, 0, data_len, nal_data.as_mut_ptr());
    if copy_status != 0 {
        return;
    }

    let timestamp_us = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros() as u64;

    let packet = MediaPacket {
        track_id: "video0".to_owned(),
        timestamp_us,
        keyframe,
        codec: HEVC_CODEC_ID,
        data: nal_data,
        width: ctx.width,
        height: ctx.height,
    };

    let _ = ctx.packet_tx.send(packet);
}

unsafe fn is_keyframe(sample_buffer: CMSampleBufferRef) -> bool {
    let attachments = CMSampleBufferGetSampleAttachmentsArray(sample_buffer, 0);
    if attachments.is_null() {
        return true;
    }
    let count = CFArrayGetCount(attachments);
    if count == 0 {
        return true;
    }
    let dict = CFArrayGetValueAtIndex(attachments, 0);
    if dict.is_null() {
        return true;
    }
    let not_sync_value = CFDictionaryGetValue(dict, kCMSampleAttachmentKey_NotSync);
    if not_sync_value.is_null() {
        return true;
    }
    CFBooleanGetValue(not_sync_value) == 0
}

fn bgra_to_rgb8(bgra: &[u8], width: u32, height: u32, stride: usize) -> Vec<u8> {
    let mut rgb = Vec::with_capacity((width * height * 3) as usize);
    for y in 0..height as usize {
        let row_start = y * stride;
        for x in 0..width as usize {
            let px = row_start + x * 4;
            if px + 2 < bgra.len() {
                rgb.push(bgra[px + 2]); // R
                rgb.push(bgra[px + 1]); // G
                rgb.push(bgra[px]);     // B
            } else {
                rgb.extend_from_slice(&[0, 0, 0]);
            }
        }
    }
    rgb
}

// Shared state passed into the delegate via Arc<Mutex<..>>
#[allow(dead_code)]
struct DelegateState {
    vt_session: VTCompressionSessionRef,
    preview_tx: mpsc::Sender<PreviewFrame>,
    frame_count: u64,
    width: u32,
    height: u32,
}

// SAFETY: VTCompressionSessionRef is used only from the serial dispatch queue
// that AVCaptureVideoDataOutput is configured with, so access is serialized.
unsafe impl Send for DelegateState {}

struct DelegateIvars {
    state: Arc<Mutex<DelegateState>>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[name = "KagamiCaptureDelegate"]
    #[ivars = DelegateIvars]
    struct KagamiCaptureDelegate;

    unsafe impl NSObjectProtocol for KagamiCaptureDelegate {}

    unsafe impl AVCaptureVideoDataOutputSampleBufferDelegate for KagamiCaptureDelegate {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        unsafe fn capture_output_did_output(
            &self,
            _output: &AVCaptureOutput,
            sample_buffer: &CMSampleBuffer,
            _connection: &AVCaptureConnection,
        ) {
            self.handle_sample_buffer(sample_buffer);
        }

        #[unsafe(method(captureOutput:didDropSampleBuffer:fromConnection:))]
        unsafe fn capture_output_did_drop(
            &self,
            _output: &AVCaptureOutput,
            _sample_buffer: &CMSampleBuffer,
            _connection: &AVCaptureConnection,
        ) {
            log::debug!("AVCaptureVideoDataOutput dropped a frame");
        }
    }
);

impl KagamiCaptureDelegate {
    fn new(state: Arc<Mutex<DelegateState>>) -> Retained<Self> {
        let this = Self::alloc();
        let this = this.set_ivars(DelegateIvars { state });
        unsafe { msg_send![super(this), init] }
    }

    fn handle_sample_buffer(&self, sample_buffer: &CMSampleBuffer) {
        let sb_ref = sample_buffer as *const CMSampleBuffer as CMSampleBufferRef;

        let state = match self.ivars().state.lock() {
            Ok(s) => s,
            Err(_) => return,
        };

        unsafe {
            let pixel_buffer = CMSampleBufferGetImageBuffer(sb_ref);
            if pixel_buffer.is_null() {
                return;
            }

            // Extract preview frame (BGRA → RGB8)
            let lock_status =
                CVPixelBufferLockBaseAddress(pixel_buffer, K_CV_PIXEL_BUFFER_LOCK_READ_ONLY);
            if lock_status == 0 {
                let base = CVPixelBufferGetBaseAddress(pixel_buffer);
                let w = CVPixelBufferGetWidth(pixel_buffer) as u32;
                let h = CVPixelBufferGetHeight(pixel_buffer) as u32;
                let stride = CVPixelBufferGetBytesPerRow(pixel_buffer);

                if !base.is_null() {
                    let total_bytes = stride * h as usize;
                    let bgra_slice = std::slice::from_raw_parts(base, total_bytes);
                    let rgb_data = bgra_to_rgb8(bgra_slice, w, h, stride);

                    let timestamp_us = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_micros() as u64;

                    let _ = state.preview_tx.send(PreviewFrame {
                        width: w,
                        height: h,
                        stride: w * 3,
                        data: rgb_data,
                        timestamp_us,
                    });
                }

                CVPixelBufferUnlockBaseAddress(pixel_buffer, K_CV_PIXEL_BUFFER_LOCK_READ_ONLY);
            }

            // Feed to VT encoder
            if !state.vt_session.is_null() {
                let pts = cm_time(state.frame_count as i64, 30);
                let dur = cm_time(1, 30);
                let mut info_flags: u32 = 0;
                VTCompressionSessionEncodeFrame(
                    state.vt_session,
                    pixel_buffer,
                    pts,
                    dur,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    &mut info_flags,
                );
            }
        }

        drop(state);

        if let Ok(mut state) = self.ivars().state.lock() {
            state.frame_count += 1;
        }
    }
}

pub struct AvFoundationProducer {
    capture_session: Option<Retained<AVCaptureSession>>,
    _delegate: Option<Retained<KagamiCaptureDelegate>>,
    vt_session: VTCompressionSessionRef,
    encoder_ctx: Option<Box<EncoderContext>>,
    packet_rx: mpsc::Receiver<MediaPacket>,
    preview_rx: mpsc::Receiver<PreviewFrame>,
    width: u32,
    height: u32,
    running: bool,
}

// SAFETY: AvFoundationProducer is only used from the main Slint thread.
// The AVCaptureSession and VTCompressionSession are started/stopped from
// the main thread; callbacks run on a serial dispatch queue.
unsafe impl Send for AvFoundationProducer {}

impl AvFoundationProducer {
    pub fn new(width: u32, height: u32) -> Result<Self, String> {
        let (packet_tx, packet_rx) = mpsc::channel();
        let (preview_tx, preview_rx) = mpsc::channel();

        // --- Create VTCompressionSession (H.265) ---
        let encoder_ctx = Box::new(EncoderContext {
            packet_tx,
            width,
            height,
        });

        let mut vt_session: VTCompressionSessionRef = std::ptr::null_mut();
        let status = unsafe {
            VTCompressionSessionCreate(
                kCFAllocatorDefault,
                width as i32,
                height as i32,
                K_CM_VIDEO_CODEC_TYPE_HEVC,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                vt_output_callback,
                &*encoder_ctx as *const EncoderContext as *mut c_void,
                &mut vt_session,
            )
        };

        if status != 0 || vt_session.is_null() {
            return Err(format!(
                "VTCompressionSessionCreate failed with status {status}"
            ));
        }

        unsafe {
            VTSessionSetProperty(
                vt_session,
                kVTCompressionPropertyKey_RealTime,
                kCFBooleanTrue,
            );
            VTSessionSetProperty(
                vt_session,
                kVTCompressionPropertyKey_ProfileLevel,
                kVTProfileLevel_HEVC_Main_AutoLevel,
            );

            let max_kfi: i32 = 60;
            let cf_max_kfi = CFNumberCreate(
                kCFAllocatorDefault,
                K_CF_NUMBER_SINT32_TYPE,
                &max_kfi as *const i32 as *const c_void,
            );
            VTSessionSetProperty(
                vt_session,
                kVTCompressionPropertyKey_MaxKeyFrameInterval,
                cf_max_kfi,
            );
            CFRelease(cf_max_kfi);

            let frame_rate: f64 = 30.0;
            let cf_frame_rate = CFNumberCreate(
                kCFAllocatorDefault,
                K_CF_NUMBER_FLOAT64_TYPE,
                &frame_rate as *const f64 as *const c_void,
            );
            VTSessionSetProperty(
                vt_session,
                kVTCompressionPropertyKey_ExpectedFrameRate,
                cf_frame_rate,
            );
            CFRelease(cf_frame_rate);

            let bitrate: i32 = 2_000_000;
            let cf_bitrate = CFNumberCreate(
                kCFAllocatorDefault,
                K_CF_NUMBER_SINT32_TYPE,
                &bitrate as *const i32 as *const c_void,
            );
            VTSessionSetProperty(
                vt_session,
                kVTCompressionPropertyKey_AverageBitRate,
                cf_bitrate,
            );
            CFRelease(cf_bitrate);

            VTCompressionSessionPrepareToEncodeFrames(vt_session);
        }

        // --- Create AVCaptureSession ---
        let capture_session = unsafe {
            let session = AVCaptureSession::new();
            session.setSessionPreset(AVCaptureSessionPreset1280x720);
            session
        };

        let media_type = unsafe { AVMediaTypeVideo }
            .expect("AVMediaTypeVideo symbol should be available");
        let device = unsafe {
            AVCaptureDevice::defaultDeviceWithMediaType(media_type)
        }
        .ok_or("no video capture device found")?;

        let input = unsafe {
            AVCaptureDeviceInput::deviceInputWithDevice_error(&device)
        }
        .map_err(|e| format!("failed to create capture input: {e}"))?;

        unsafe {
            if capture_session.canAddInput(&input) == true {
                capture_session.addInput(&input);
            } else {
                return Err("cannot add video input to capture session".to_owned());
            }
        }

        let video_output = unsafe { AVCaptureVideoDataOutput::new() };

        // Request BGRA pixel format for easy preview conversion
        let pixel_format_key = NSString::from_str("PixelFormatType");
        let pixel_format_value = NSNumber::new_u32(K_CV_PIXEL_FORMAT_TYPE_32BGRA);
        let video_settings = NSDictionary::from_slices(
            &[&*pixel_format_key],
            &[&*pixel_format_value as &AnyObject],
        );
        unsafe {
            video_output.setVideoSettings(Some(&video_settings));
        }

        let delegate_state = Arc::new(Mutex::new(DelegateState {
            vt_session,
            preview_tx,
            frame_count: 0,
            width,
            height,
        }));

        let delegate = KagamiCaptureDelegate::new(Arc::clone(&delegate_state));

        let queue = dispatch2::DispatchQueue::new(
            "com.izakaya.kagami.capture",
            None,
        );

        unsafe {
            video_output.setSampleBufferDelegate_queue(
                Some(ProtocolObject::from_ref(&*delegate)),
                Some(&queue),
            );
        }

        unsafe {
            if capture_session.canAddOutput(&video_output) == true {
                capture_session.addOutput(&video_output);
            } else {
                VTCompressionSessionInvalidate(vt_session);
                CFRelease(vt_session);
                return Err("cannot add video output to capture session".to_owned());
            }
        }

        Ok(Self {
            capture_session: Some(capture_session),
            _delegate: Some(delegate),
            vt_session,
            encoder_ctx: Some(encoder_ctx),
            packet_rx,
            preview_rx,
            width,
            height,
            running: false,
        })
    }
}

impl NezumiProducer for AvFoundationProducer {
    fn tracks(&self) -> Vec<MediaTrack> {
        vec![MediaTrack {
            id: "video0".to_owned(),
            kind: TrackKind::Video,
        }]
    }

    fn start_reading(&mut self) -> Result<(), String> {
        if self.running {
            return Ok(());
        }
        if let Some(ref session) = self.capture_session {
            unsafe {
                session.startRunning();
            }
            self.running = true;
            log::info!(
                "AVCaptureSession started ({}x{}, HEVC encoding)",
                self.width,
                self.height
            );
            Ok(())
        } else {
            Err("capture session not initialized".to_owned())
        }
    }

    fn next_packet(&mut self) -> Option<MediaPacket> {
        self.packet_rx.try_recv().ok()
    }

    fn next_preview_frame(&mut self) -> Option<PreviewFrame> {
        // Drain to the most recent frame so the preview stays current
        let mut latest = None;
        while let Ok(frame) = self.preview_rx.try_recv() {
            latest = Some(frame);
        }
        latest
    }

    fn stop(&mut self) {
        if let Some(ref session) = self.capture_session {
            if self.running {
                unsafe {
                    session.stopRunning();
                }
                self.running = false;
                log::info!("AVCaptureSession stopped");
            }
        }
    }
}

impl Drop for AvFoundationProducer {
    fn drop(&mut self) {
        self.stop();
        if !self.vt_session.is_null() {
            unsafe {
                VTCompressionSessionInvalidate(self.vt_session);
                CFRelease(self.vt_session);
            }
            self.vt_session = std::ptr::null_mut();
        }
        // Drop encoder_ctx so the callback refcon becomes invalid only
        // after the VT session is invalidated (no more callbacks).
        self.encoder_ctx.take();
    }
}
