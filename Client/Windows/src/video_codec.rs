//! H.264 for camera and screen (OpenH264, built from source).
//!
//! Wire format of a video frame (`VideoFrame.data`):
//! `[frame number u32 LE][H.264 Annex-B access unit]`. The frame number lets
//! a viewer notice frames the server dropped (slow connection): it then asks
//! the sender for a keyframe instead of showing broken pictures until the next
//! periodic one (every `KEYFRAME_FRAMES`).

use anyhow::{anyhow, Context, Result};
use eframe::egui;
use openh264::decoder::Decoder;
use openh264::encoder::{BitRate, Encoder, EncoderConfig, FrameRate, IntraFramePeriod, UsageType};
use openh264::formats::{RgbSliceU8, YUVBuffer, YUVSource};
use openh264::OpenH264API;

/// Frames per second we send.
pub const FPS: f32 = 25.0;
/// A full picture at least every 4 s (late viewers, lost frames).
pub const KEYFRAME_FRAMES: u32 = 100;
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

/// One outgoing stream. The OpenH264 encoder is (re)created per resolution
/// (a resized window changes it).
pub struct H264Encoder {
    bitrate: u32,
    screen: bool,
    enc: Option<(Encoder, (usize, usize))>,
    force_keyframe: bool,
    frame_no: u32,
}

impl H264Encoder {
    pub fn new(bitrate_bps: u32, screen: bool) -> Self {
        Self { bitrate: bitrate_bps, screen, enc: None, force_keyframe: true, frame_no: 0 }
    }

    /// The next frame becomes a full picture (a viewer asked or just joined).
    pub fn request_keyframe(&mut self) {
        self.force_keyframe = true;
    }

    fn encoder(&mut self, dims: (usize, usize)) -> Result<&mut Encoder> {
        if self.enc.as_ref().map(|(_, d)| *d) != Some(dims) {
            let config = EncoderConfig::new()
                .bitrate(BitRate::from_bps(self.bitrate))
                .max_frame_rate(FrameRate::from_hz(FPS))
                .usage_type(if self.screen { UsageType::ScreenContentRealTime } else { UsageType::CameraVideoRealTime })
                .intra_frame_period(IntraFramePeriod::from_num_frames(KEYFRAME_FRAMES));
            let enc = Encoder::with_api_config(OpenH264API::from_source(), config).context("H.264 encoder")?;
            self.enc = Some((enc, dims));
            self.force_keyframe = true; // a new encoder starts with one anyway
        }
        Ok(&mut self.enc.as_mut().expect("just set").0)
    }

    /// Encodes one RGB picture (even size) into a wire frame.
    pub fn encode(&mut self, rgb: &image::RgbImage) -> Result<Vec<u8>> {
        let dims = (rgb.width() as usize, rgb.height() as usize);
        let keyframe = std::mem::take(&mut self.force_keyframe);
        self.frame_no = self.frame_no.wrapping_add(1);
        let frame_no = self.frame_no;
        let enc = self.encoder(dims)?;
        if keyframe {
            enc.force_intra_frame();
        }
        let yuv = YUVBuffer::from_rgb_source(RgbSliceU8::new(rgb.as_raw(), dims));
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
