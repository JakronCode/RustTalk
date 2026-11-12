//! H.264 for camera and screen (OpenH264, built from source).
//!
//! Wire format of a video frame (`VideoFrame.data`):
//! `[frame number u32 LE][H.264 Annex-B access unit]`. The frame number lets
//! a viewer notice frames the server dropped (slow connection): it then asks
//! the sender for a keyframe instead of showing broken pictures until the next
//! periodic one (about every 4 s).
//!
//! Speed: OpenH264 is built with its SIMD assembly (needs `nasm` at build
//! time, see README) and RGB is turned into YUV with the SIMD path - together
//! ~15 ms instead of ~80 ms per 1080p frame.

use anyhow::{anyhow, Context, Result};
use eframe::egui;
use openh264::decoder::Decoder;
use openh264::encoder::{BitRate, Complexity, Encoder, EncoderConfig, FrameRate, IntraFramePeriod, QpRange, RateControlMode, UsageType};
use openh264::formats::{RgbSliceU8, RgbaSliceU8, YUVBuffer, YUVSource};
use openh264::OpenH264API;

/// Frames per second we send.
pub const FPS: f32 = 30.0;
/// Bytes in front of the H.264 data.
pub const FRAME_HEADER: usize = 4;

/// Scales `img` to fit `max_w` x `max_h` with even width/height (H.264 4:2:0).
pub fn fit_even(img: image::DynamicImage, max_w: u32, max_h: u32) -> image::RgbImage {
    let img = if img.width() > max_w || img.height() > max_h { img.thumbnail(max_w, max_h) } else { img };
    let rgb = img.to_rgb8();
    let (w, h) = (rgb.width() & !1, rgb.height() & !1);
    if (w, h) == (rgb.width(), rgb.height()) {
        rgb
    } else {
        image::imageops::crop_imm(&rgb, 0, 0, w.max(2), h.max(2)).to_image()
    }
}

/// Encoder settings beyond bitrate and content type.
#[derive(Debug, Clone, Copy)]
pub struct Tuning {
    pub fps: f32,
    pub rate_control: RateControlMode,
    /// Drop frames when over budget (stutters) - or keep every frame.
    pub skip_frames: bool,
    pub complexity: Complexity,
    /// Upper bound of the quantizer: lower = never blurrier than this.
    pub max_qp: u8,
    /// Encoder threads (0 = OpenH264 decides).
    pub threads: u16,
}

impl Tuning {
    /// OpenH264 defaults (what v0.9.0 and earlier sent, at 25 fps).
    pub const DEFAULT: Tuning = Tuning { fps: 25.0, rate_control: RateControlMode::Quality, skip_frames: true, complexity: Complexity::Medium, max_qp: 51, threads: 0 };

    /// What we send: every frame (no skipping = no stutter), steady bit
    /// rate, and never blurrier than QP 36 (screen text) / 40 (camera).
    pub const fn stream(screen: bool) -> Tuning {
        Tuning {
            fps: FPS,
            rate_control: RateControlMode::Bitrate,
            skip_frames: false,
            complexity: Complexity::Medium,
            max_qp: if screen { 36 } else { 40 },
            threads: 0,
        }
    }
}

/// Like [`fit_even`], but fast (SIMD, Catmull-Rom) and without changing the
/// pixel format: RGB stays RGB, RGBA stays RGBA (no extra conversion pass).
pub fn fit_even_fast(img: image::DynamicImage, max_w: u32, max_h: u32) -> image::DynamicImage {
    use fast_image_resize as fr;
    use image::DynamicImage::{ImageRgb8, ImageRgba8};
    let img = match img {
        ImageRgb8(_) | ImageRgba8(_) => img,
        other => ImageRgba8(other.to_rgba8()),
    };
    let (w, h) = (img.width(), img.height());
    let scale = (max_w as f64 / w as f64).min(max_h as f64 / h as f64).min(1.0);
    let tw = (((w as f64 * scale).round() as u32) & !1).max(2);
    let th = (((h as f64 * scale).round() as u32) & !1).max(2);
    if (tw, th) == (w, h) {
        return img;
    }
    if scale >= 1.0 {
        // Only an odd edge: crop one pixel instead of scaling.
        return match img {
            ImageRgb8(i) => ImageRgb8(image::imageops::crop_imm(&i, 0, 0, tw, th).to_image()),
            ImageRgba8(i) => ImageRgba8(image::imageops::crop_imm(&i, 0, 0, tw, th).to_image()),
            _ => unreachable!(),
        };
    }
    let (pixel, raw) = match &img {
        ImageRgb8(i) => (fr::PixelType::U8x3, i.as_raw().as_slice()),
        ImageRgba8(i) => (fr::PixelType::U8x4, i.as_raw().as_slice()),
        _ => unreachable!(),
    };
    let resized = fr::images::ImageRef::new(w, h, raw, pixel).ok().and_then(|src| {
        let mut dst = fr::images::Image::new(tw, th, pixel);
        let options = fr::ResizeOptions::new().resize_alg(fr::ResizeAlg::Convolution(fr::FilterType::CatmullRom)).use_alpha(false);
        fr::Resizer::new().resize(&src, &mut dst, &options).ok()?;
        Some(dst.into_vec())
    });
    match (resized, pixel) {
        (Some(buf), fr::PixelType::U8x3) => image::RgbImage::from_raw(tw, th, buf).map(ImageRgb8),
        (Some(buf), _) => image::RgbaImage::from_raw(tw, th, buf).map(ImageRgba8),
        (None, _) => None,
    }
    .unwrap_or_else(|| image::DynamicImage::ImageRgb8(fit_even(img, max_w, max_h)))
}

/// One outgoing stream. The OpenH264 encoder is (re)created per resolution
/// (a resized window changes it).
pub struct H264Encoder {
    bitrate: u32,
    screen: bool,
    tuning: Tuning,
    enc: Option<(Encoder, (usize, usize))>,
    force_keyframe: bool,
    frame_no: u32,
}

impl H264Encoder {
    pub fn new(bitrate_bps: u32, screen: bool) -> Self {
        Self::with_tuning(bitrate_bps, screen, Tuning::DEFAULT)
    }

    pub fn with_tuning(bitrate_bps: u32, screen: bool, tuning: Tuning) -> Self {
        Self { bitrate: bitrate_bps, screen, tuning, enc: None, force_keyframe: true, frame_no: 0 }
    }

    /// The next frame becomes a full picture (a viewer asked or just joined).
    pub fn request_keyframe(&mut self) {
        self.force_keyframe = true;
    }

    fn encoder(&mut self, dims: (usize, usize)) -> Result<&mut Encoder> {
        if self.enc.as_ref().map(|(_, d)| *d) != Some(dims) {
            let t = self.tuning;
            let config = EncoderConfig::new()
                .bitrate(BitRate::from_bps(self.bitrate))
                .max_frame_rate(FrameRate::from_hz(t.fps))
                .usage_type(if self.screen { UsageType::ScreenContentRealTime } else { UsageType::CameraVideoRealTime })
                .rate_control_mode(t.rate_control)
                .skip_frames(t.skip_frames)
                .complexity(t.complexity)
                .qp(QpRange::new(0, t.max_qp))
                .num_threads(t.threads)
                // A full picture about every 4 s, whatever the frame rate.
                .intra_frame_period(IntraFramePeriod::from_num_frames((t.fps * 4.0) as u32));
            let enc = Encoder::with_api_config(OpenH264API::from_source(), config).context("H.264 encoder")?;
            self.enc = Some((enc, dims));
            self.force_keyframe = true; // a new encoder starts with one anyway
        }
        Ok(&mut self.enc.as_mut().expect("just set").0)
    }

    /// Encodes one RGB picture (even size) into a wire frame.
    pub fn encode(&mut self, rgb: &image::RgbImage) -> Result<Vec<u8>> {
        let dims = (rgb.width() as usize, rgb.height() as usize);
        self.encode_yuv(dims, YUVBuffer::from_rgb8_source(RgbSliceU8::new(rgb.as_raw(), dims)))
    }

    /// Encodes an RGB or RGBA picture (even size) - RGBA goes to YUV directly.
    pub fn encode_image(&mut self, img: &image::DynamicImage) -> Result<Vec<u8>> {
        match img {
            image::DynamicImage::ImageRgb8(rgb) => self.encode(rgb),
            image::DynamicImage::ImageRgba8(rgba) => {
                let dims = (rgba.width() as usize, rgba.height() as usize);
                self.encode_yuv(dims, YUVBuffer::from_rgba8_source(RgbaSliceU8::new(rgba.as_raw(), dims)))
            }
            other => self.encode(&other.to_rgb8()),
        }
    }

    fn encode_yuv(&mut self, dims: (usize, usize), yuv: YUVBuffer) -> Result<Vec<u8>> {
        let keyframe = std::mem::take(&mut self.force_keyframe);
        self.frame_no = self.frame_no.wrapping_add(1);
        let frame_no = self.frame_no;
        let enc = self.encoder(dims)?;
        if keyframe {
            enc.force_intra_frame();
        }
        let bits = enc.encode(&yuv).map_err(|e| anyhow!("H.264 encode: {e}"))?;
        let mut out = Vec::with_capacity(FRAME_HEADER + 64 * 1024);
        out.extend_from_slice(&frame_no.to_le_bytes());
        bits.write_vec(&mut out);
        Ok(out)
    }
}

/// What a decoder made of one wire frame.
pub enum Decoded {
    /// A new picture.
    Picture(egui::ColorImage),
    /// Nothing to show yet (waiting for a keyframe).
    Nothing,
}

/// One incoming stream.
pub struct H264Decoder {
    dec: Decoder,
    last_frame_no: Option<u32>,
    /// True until the first picture after a (re)start or a gap was decoded.
    pub need_keyframe: bool,
}

impl H264Decoder {
    pub fn new() -> Result<Self> {
        Ok(Self { dec: Decoder::new().context("H.264 decoder")?, last_frame_no: None, need_keyframe: true })
    }

    /// Decodes a wire frame. A gap in the frame numbers sets `need_keyframe`.
    pub fn decode(&mut self, frame: &[u8]) -> Result<Decoded> {
        if frame.len() <= FRAME_HEADER {
            return Ok(Decoded::Nothing);
        }
        let no = u32::from_le_bytes(frame[..FRAME_HEADER].try_into().expect("4 bytes"));
        if let Some(last) = self.last_frame_no {
            if no != last.wrapping_add(1) {
                self.need_keyframe = true; // frames were dropped on the way
            }
        }
        self.last_frame_no = Some(no);
        let decoded = self.dec.decode(&frame[FRAME_HEADER..]).map_err(|e| {
            self.need_keyframe = true;
            anyhow!("H.264 decode: {e}")
        })?;
        let Some(yuv) = decoded else { return Ok(Decoded::Nothing) };
        let (w, h) = yuv.dimensions();
        let mut rgba = vec![0u8; w * h * 4];
        yuv.write_rgba8(&mut rgba);
        Ok(Decoded::Picture(egui::ColorImage::from_rgba_unmultiplied([w, h], &rgba)))
    }

    /// Call when a picture came out after a keyframe was requested.
    pub fn got_picture(&mut self) {
        self.need_keyframe = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Moving test picture (gradient + a moving box).
    fn picture(w: u32, h: u32, t: u32) -> image::RgbImage {
        image::RgbImage::from_fn(w, h, |x, y| {
            let in_box = (x + 400 - t * 8) % 400 < 60 && y > h / 3 && y < h / 2;
            if in_box {
                image::Rgb([250, 250, 250])
            } else {
                image::Rgb([(x * 255 / w) as u8, (y * 255 / h) as u8, 90])
            }
        })
    }

    /// Quality report (needs a screen): one screenshot, kept in memory only,
    /// scrolled 6 px per frame like reading a page. Prints PSNR (higher =
    /// closer to the original), data rate, encode time and dropped frames.
    /// cargo test -p rusttalk-client quality_report -- --ignored --nocapture
    #[cfg(windows)]
    #[test]
    #[ignore]
    fn quality_report() {
        let mon = xcap::Monitor::all().unwrap().into_iter().next().expect("a screen");
        let shot = image::DynamicImage::ImageRgba8(mon.capture_image().unwrap());
        let base = fit_even(shot, 1920, 1080);
        let (w, h) = (base.width(), base.height());
        let frames: Vec<image::RgbImage> = (0..75u32)
            .map(|t| image::RgbImage::from_fn(w, h, |x, y| *base.get_pixel(x, (y + t * 6) % h)))
            .collect();
        println!("screen {w}x{h}, {} frames", frames.len());
        let t0 = std::time::Instant::now();
        for f in frames.iter().take(20) {
            let _ = YUVBuffer::from_rgb8_source(RgbSliceU8::new(f.as_raw(), (w as usize, h as usize)));
        }
        println!("RGB->YUV alone: {:.1} ms/frame", t0.elapsed().as_secs_f64() * 1000.0 / 20.0);
        println!("{}", report_line("v0.9.0 settings, 4 Mbit", &frames, 4_000_000, true, Tuning::DEFAULT));
        println!("{}", report_line("now (stream tuning), 6 Mbit", &frames, 6_000_000, true, Tuning::stream(true)));
    }

    /// Encodes + decodes `frames`, returns one line of numbers.
    #[cfg(windows)]
    fn report_line(name: &str, frames: &[image::RgbImage], bitrate: u32, screen: bool, t: Tuning) -> String {
        let psnr = |a: &image::RgbImage, b: &egui::ColorImage| {
            let mut se = 0f64;
            for (pa, pb) in a.pixels().zip(b.pixels.iter()) {
                for (ca, cb) in [(pa[0], pb.r()), (pa[1], pb.g()), (pa[2], pb.b())] {
                    let d = ca as f64 - cb as f64;
                    se += d * d;
                }
            }
            let mse = se / (a.width() * a.height() * 3) as f64;
            10.0 * (255.0f64 * 255.0 / mse.max(1e-9)).log10()
        };
        let mut enc = H264Encoder::with_tuning(bitrate, screen, t);
        let mut dec = H264Decoder::new().unwrap();
        let (mut bytes, mut pics, mut sum, mut worst, mut skipped) = (0usize, 0, 0f64, f64::MAX, 0);
        let mut enc_time = std::time::Duration::ZERO;
        for (i, f) in frames.iter().enumerate() {
            let t0 = std::time::Instant::now();
            let wire = enc.encode(f).unwrap();
            enc_time += t0.elapsed();
            bytes += wire.len();
            match dec.decode(&wire) {
                Ok(Decoded::Picture(img)) => {
                    let p = psnr(f, &img);
                    pics += 1;
                    sum += p;
                    worst = worst.min(p);
                }
                _ if i > 0 => skipped += 1,
                _ => {}
            }
        }
        let secs = frames.len() as f64 / t.fps as f64;
        format!(
            "{name:<36} PSNR avg {:5.1} dB, worst {:5.1} | {:5.2} Mbit/s | encode {:5.1} ms/frame | skipped {skipped}",
            sum / pics.max(1) as f64,
            worst,
            bytes as f64 * 8.0 / secs / 1e6,
            enc_time.as_secs_f64() * 1000.0 / frames.len() as f64
        )
    }

    #[test]
    fn fast_fit_keeps_format_aspect_and_even_sizes() {
        let rgba = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(2560, 1440, image::Rgba([10, 20, 30, 255])));
        let out = fit_even_fast(rgba, 1920, 1080);
        assert!(matches!(out, image::DynamicImage::ImageRgba8(_)), "RGBA stays RGBA");
        assert_eq!((out.width(), out.height()), (1920, 1080));
        assert_eq!(out.as_rgba8().unwrap().get_pixel(500, 500).0, [10, 20, 30, 255], "colors survive");
        let rgb = image::DynamicImage::ImageRgb8(image::RgbImage::new(1281, 721));
        let out = fit_even_fast(rgb, 1280, 720);
        assert!(matches!(out, image::DynamicImage::ImageRgb8(_)));
        assert!(out.width() % 2 == 0 && out.height() % 2 == 0 && out.width() <= 1280 && out.height() <= 720);
        let small = fit_even_fast(image::DynamicImage::ImageRgba8(image::RgbaImage::new(641, 361)), 1920, 1080);
        assert_eq!((small.width(), small.height()), (640, 360), "odd edge cropped, not scaled");
        // RGBA encodes and decodes like RGB.
        let mut enc = H264Encoder::new(1_000_000, true);
        let mut dec = H264Decoder::new().unwrap();
        let pic = fit_even_fast(image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(640, 360, image::Rgba([200, 40, 40, 255]))), 640, 360);
        match dec.decode(&enc.encode_image(&pic).unwrap()).unwrap() {
            Decoded::Picture(img) => {
                let p = img.pixels[360 * 100 + 100];
                assert!((p.r() as i32 - 200).abs() < 12 && (p.g() as i32 - 40).abs() < 12, "{p:?}");
            }
            Decoded::Nothing => panic!("keyframe expected"),
        }
    }

    #[test]
    fn even_sizes_for_h264() {
        let odd = image::DynamicImage::ImageRgb8(image::RgbImage::new(1921, 1081));
        let r = fit_even(odd, 1920, 1080);
        assert!(r.width() % 2 == 0 && r.height() % 2 == 0);
        assert!(r.width() <= 1920 && r.height() <= 1080);
        let small = fit_even(image::DynamicImage::ImageRgb8(image::RgbImage::new(641, 361)), 1920, 1080);
        assert_eq!((small.width(), small.height()), (640, 360), "odd edge cropped, not scaled");
    }

    #[test]
    fn encode_decode_roundtrip_is_small_and_close() {
        let mut enc = H264Encoder::new(2_000_000, true);
        let mut dec = H264Decoder::new().unwrap();
        let (w, h) = (640u32, 360u32);
        let mut total = 0usize;
        let mut last = None;
        for t in 0..25 {
            let frame = enc.encode(&picture(w, h, t)).unwrap();
            total += frame.len();
            if let Decoded::Picture(img) = dec.decode(&frame).unwrap() {
                assert_eq!(img.size, [w as usize, h as usize]);
                last = Some((img, t));
            }
        }
        let (img, t) = last.expect("pictures decoded");
        // Compare a pixel of the gradient with the source (lossy but close).
        let src = picture(w, h, t);
        let p = img.pixels[(300 * w + 100) as usize];
        let s = src.get_pixel(100, 300);
        assert!((p.r() as i32 - s[0] as i32).abs() < 30 && (p.g() as i32 - s[1] as i32).abs() < 30, "{p:?} vs {s:?}");
        // 1 s of 640x360 at 2 Mbit/s is well below MJPEG (~25 x 30 KB).
        assert!(total < 400_000, "too big: {total} bytes for 25 frames");
    }

    #[test]
    fn late_viewer_needs_and_gets_a_keyframe() {
        let mut enc = H264Encoder::new(1_000_000, false);
        let frames: Vec<Vec<u8>> = (0..10).map(|t| enc.encode(&picture(320, 180, t)).unwrap()).collect();
        // A viewer joining at frame 5 sees nothing usable (no keyframe yet)...
        let mut dec = H264Decoder::new().unwrap();
        let mut pictures = 0;
        for f in &frames[5..] {
            if matches!(dec.decode(f), Ok(Decoded::Picture(_))) {
                pictures += 1;
            }
        }
        assert!(dec.need_keyframe, "still waiting for a full picture");
        // ...until the sender forces one.
        enc.request_keyframe();
        let key = enc.encode(&picture(320, 180, 11)).unwrap();
        assert!(matches!(dec.decode(&key).unwrap(), Decoded::Picture(_)), "keyframe decodes ({pictures} before)");
    }

    #[test]
    fn dropped_frames_are_noticed() {
        let mut enc = H264Encoder::new(1_000_000, false);
        let mut dec = H264Decoder::new().unwrap();
        let f: Vec<Vec<u8>> = (0..4).map(|t| enc.encode(&picture(320, 180, t)).unwrap()).collect();
        dec.decode(&f[0]).unwrap();
        dec.got_picture();
        assert!(!dec.need_keyframe);
        let _ = dec.decode(&f[2]); // frame 1 lost
        assert!(dec.need_keyframe, "gap detected");
    }

    #[test]
    fn new_resolution_restarts_with_a_keyframe() {
        let mut enc = H264Encoder::new(1_000_000, true);
        let mut dec = H264Decoder::new().unwrap();
        for t in 0..3 {
            dec.decode(&enc.encode(&picture(320, 180, t)).unwrap()).unwrap();
        }
        let bigger = enc.encode(&picture(640, 360, 3)).unwrap();
        match dec.decode(&bigger).unwrap() {
            Decoded::Picture(img) => assert_eq!(img.size, [640, 360]),
            Decoded::Nothing => panic!("window resize must give a decodable keyframe"),
        }
    }
}
