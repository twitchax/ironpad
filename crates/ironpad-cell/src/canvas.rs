//! Pixel-buffer canvas for bitmap image output.
//!
//! [`Canvas`] provides a simple 2D pixel buffer that renders as a BMP image
//! via a base64 data URL.  This avoids the overhead of SVG when drawing
//! per-pixel graphics (fractals, ray tracers, heatmaps, etc.).
//!
//! # Quick start
//!
//! ```ignore
//! let mut c = Canvas::new(800, 600);
//! c.set_pixel(10, 20, (255, 0, 0));
//! c  // returns CellOutput with an <img> tag
//! ```
//!
//! Or use the functional constructor:
//!
//! ```ignore
//! Canvas::from_fn(800, 600, |x, y| {
//!     let t = x as f64 / 800.0;
//!     ((t * 255.0) as u8, 0, 0)
//! })
//! ```

/// A 2D pixel buffer that renders as a bitmap `<img>` tag.
///
/// Pixels are stored as a flat `Vec<u8>` of RGB triplets in row-major order.
/// On conversion to [`CellOutput`](crate::CellOutput), the buffer is BMP-encoded,
/// base64-encoded, and wrapped in an `<img>` tag with a data URL.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Canvas {
    width: u32,
    height: u32,
    /// Flat RGB bytes: `[r0, g0, b0, r1, g1, b1, ...]`, row-major.
    pixels: Vec<u8>,
}

impl Canvas {
    /// Create a new canvas filled with black.
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            pixels: vec![0u8; (width as usize) * (height as usize) * 3],
        }
    }

    /// Create a canvas from a vector of `(r, g, b)` tuples.
    ///
    /// # Panics
    ///
    /// Panics if `pixels.len() != width * height`.
    pub fn from_pixels(width: u32, height: u32, pixels: Vec<(u8, u8, u8)>) -> Self {
        let expected = (width as usize) * (height as usize);
        assert_eq!(
            pixels.len(),
            expected,
            "Canvas::from_pixels: expected {expected} pixels, got {}",
            pixels.len()
        );

        let mut flat = Vec::with_capacity(expected * 3);
        for (r, g, b) in pixels {
            flat.push(r);
            flat.push(g);
            flat.push(b);
        }

        Self {
            width,
            height,
            pixels: flat,
        }
    }

    /// Create a canvas by evaluating a function at each pixel.
    ///
    /// The closure receives `(x, y)` coordinates (column, row) and returns
    /// an `(r, g, b)` color tuple.
    pub fn from_fn(width: u32, height: u32, mut f: impl FnMut(u32, u32) -> (u8, u8, u8)) -> Self {
        let count = (width as usize) * (height as usize);
        let mut flat = Vec::with_capacity(count * 3);

        for y in 0..height {
            for x in 0..width {
                let (r, g, b) = f(x, y);
                flat.push(r);
                flat.push(g);
                flat.push(b);
            }
        }

        Self {
            width,
            height,
            pixels: flat,
        }
    }

    /// Set the color of a single pixel.
    ///
    /// Does nothing if `(x, y)` is out of bounds.
    pub fn set_pixel(&mut self, x: u32, y: u32, color: (u8, u8, u8)) {
        if x >= self.width || y >= self.height {
            return;
        }

        let idx = ((y as usize) * (self.width as usize) + (x as usize)) * 3;
        self.pixels[idx] = color.0;
        self.pixels[idx + 1] = color.1;
        self.pixels[idx + 2] = color.2;
    }

    /// Get the color of a single pixel.
    ///
    /// Returns `(0, 0, 0)` if `(x, y)` is out of bounds.
    pub fn get_pixel(&self, x: u32, y: u32) -> (u8, u8, u8) {
        if x >= self.width || y >= self.height {
            return (0, 0, 0);
        }

        let idx = ((y as usize) * (self.width as usize) + (x as usize)) * 3;
        (self.pixels[idx], self.pixels[idx + 1], self.pixels[idx + 2])
    }

    /// Fill the entire canvas with a single color.
    pub fn fill(&mut self, color: (u8, u8, u8)) {
        for chunk in self.pixels.chunks_exact_mut(3) {
            chunk[0] = color.0;
            chunk[1] = color.1;
            chunk[2] = color.2;
        }
    }

    /// Canvas width in pixels.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Canvas height in pixels.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Raw RGB pixel bytes (flat, row-major).
    pub fn pixels(&self) -> &[u8] {
        &self.pixels
    }

    /// Consume the canvas and return the raw RGB pixel bytes.
    pub fn into_pixels(self) -> Vec<u8> {
        self.pixels
    }

    /// Encode the pixel buffer as a 24-bit BMP (top-down, no compression).
    #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    pub fn to_bmp(&self) -> Vec<u8> {
        let w = self.width as usize;
        let h = self.height as usize;

        let row_bytes = w * 3;
        let padding = (4 - row_bytes % 4) % 4;
        let pixel_data_size = (row_bytes + padding) * h;
        let file_size = 54 + pixel_data_size;

        let mut buf = Vec::with_capacity(file_size);

        // BMP file header (14 bytes).
        buf.extend_from_slice(b"BM");
        buf.extend_from_slice(&(file_size as u32).to_le_bytes());
        buf.extend_from_slice(&[0u8; 4]); // reserved
        buf.extend_from_slice(&54u32.to_le_bytes()); // pixel data offset

        // DIB header (BITMAPINFOHEADER, 40 bytes).
        buf.extend_from_slice(&40u32.to_le_bytes());
        buf.extend_from_slice(&(w as i32).to_le_bytes());
        buf.extend_from_slice(&(-(h as i32)).to_le_bytes()); // negative = top-down
        buf.extend_from_slice(&1u16.to_le_bytes()); // color planes
        buf.extend_from_slice(&24u16.to_le_bytes()); // bits per pixel
        buf.extend_from_slice(&[0u8; 24]); // compression + remaining fields

        // Pixel data (BGR order, row-major, with per-row padding).
        for row in 0..h {
            let row_start = row * w * 3;
            for col in 0..w {
                let idx = row_start + col * 3;
                buf.push(self.pixels[idx + 2]); // B
                buf.push(self.pixels[idx + 1]); // G
                buf.push(self.pixels[idx]); // R
            }
            buf.extend(std::iter::repeat_n(0u8, padding));
        }

        buf
    }

    /// Render the canvas as an HTML `<img>` tag with a base64 BMP data URL.
    pub fn to_html(&self) -> String {
        let bmp = self.to_bmp();
        let b64 = base64_encode(&bmp);

        format!(
            "<img src=\"data:image/bmp;base64,{b64}\" \
             width=\"{}\" height=\"{}\" \
             style=\"image-rendering: pixelated;\" />",
            self.width, self.height
        )
    }
}

// ── Animation ────────────────────────────────────────────────────────────────

/// A sequence of [`Canvas`] frames with a target frame rate.
///
/// On conversion to [`CellOutput`](crate::CellOutput), all frames' raw RGB
/// bytes are concatenated and base64-encoded into a single
/// [`DisplayPanel::Animation`](crate::DisplayPanel::Animation).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Animation {
    frames: Vec<Canvas>,
    fps: u32,
}

impl Animation {
    /// Create a new animation from a list of frames and a target FPS.
    pub fn new(frames: Vec<Canvas>, fps: u32) -> Self {
        Self { frames, fps }
    }

    /// The animation frames.
    pub fn frames(&self) -> &[Canvas] {
        &self.frames
    }

    /// Target frames per second.
    pub fn fps(&self) -> u32 {
        self.fps
    }
}

// ── Base64 encoding ──────────────────────────────────────────────────────────

/// Encode binary data as a base64 string (no external crate needed).
pub(crate) fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);

    for chunk in data.chunks(3) {
        let b0 = u32::from(chunk[0]);
        let b1 = if chunk.len() > 1 {
            u32::from(chunk[1])
        } else {
            0
        };
        let b2 = if chunk.len() > 2 {
            u32::from(chunk[2])
        } else {
            0
        };
        let triple = (b0 << 16) | (b1 << 8) | b2;

        out.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        out.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);

        if chunk.len() > 1 {
            out.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }

        if chunk.len() > 2 {
            out.push(CHARS[(triple & 0x3F) as usize] as char);
        } else {
            out.push('=');
        }
    }

    out
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_is_black() {
        let c = Canvas::new(2, 2);
        assert_eq!(c.get_pixel(0, 0), (0, 0, 0));
        assert_eq!(c.get_pixel(1, 1), (0, 0, 0));
    }

    #[test]
    fn set_and_get_pixel() {
        let mut c = Canvas::new(4, 4);
        c.set_pixel(2, 3, (255, 128, 64));
        assert_eq!(c.get_pixel(2, 3), (255, 128, 64));
        assert_eq!(c.get_pixel(0, 0), (0, 0, 0));
    }

    #[test]
    fn out_of_bounds_is_safe() {
        let mut c = Canvas::new(2, 2);
        c.set_pixel(5, 5, (255, 0, 0)); // no-op
        assert_eq!(c.get_pixel(5, 5), (0, 0, 0));
    }

    #[test]
    fn fill_sets_all_pixels() {
        let mut c = Canvas::new(3, 3);
        c.fill((10, 20, 30));

        for y in 0..3 {
            for x in 0..3 {
                assert_eq!(c.get_pixel(x, y), (10, 20, 30));
            }
        }
    }

    #[test]
    fn from_pixels_roundtrip() {
        let colors = vec![(255, 0, 0), (0, 255, 0), (0, 0, 255), (128, 128, 128)];
        let c = Canvas::from_pixels(2, 2, colors);
        assert_eq!(c.get_pixel(0, 0), (255, 0, 0));
        assert_eq!(c.get_pixel(1, 0), (0, 255, 0));
        assert_eq!(c.get_pixel(0, 1), (0, 0, 255));
        assert_eq!(c.get_pixel(1, 1), (128, 128, 128));
    }

    #[test]
    #[allow(clippy::cast_possible_truncation)]
    fn from_fn_fills_correctly() {
        let c = Canvas::from_fn(2, 2, |x, y| ((x * 100) as u8, (y * 100) as u8, 0));
        assert_eq!(c.get_pixel(0, 0), (0, 0, 0));
        assert_eq!(c.get_pixel(1, 0), (100, 0, 0));
        assert_eq!(c.get_pixel(0, 1), (0, 100, 0));
        assert_eq!(c.get_pixel(1, 1), (100, 100, 0));
    }

    #[test]
    fn bmp_has_correct_header() {
        let c = Canvas::new(2, 2);
        let bmp = c.to_bmp();

        // BMP magic bytes.
        assert_eq!(&bmp[0..2], b"BM");
        // DIB header size (40).
        assert_eq!(u32::from_le_bytes(bmp[14..18].try_into().unwrap()), 40);
        // Width = 2.
        assert_eq!(i32::from_le_bytes(bmp[18..22].try_into().unwrap()), 2);
        // Height = -2 (top-down).
        assert_eq!(i32::from_le_bytes(bmp[22..26].try_into().unwrap()), -2);
        // 24 bits per pixel.
        assert_eq!(u16::from_le_bytes(bmp[28..30].try_into().unwrap()), 24);
    }

    #[test]
    fn to_html_produces_img_tag() {
        let c = Canvas::new(4, 4);
        let html = c.to_html();
        assert!(html.starts_with("<img src=\"data:image/bmp;base64,"));
        assert!(html.contains("width=\"4\""));
        assert!(html.contains("height=\"4\""));
    }

    #[test]
    fn base64_roundtrip() {
        let data = b"Hello, World!";
        let encoded = base64_encode(data);
        assert_eq!(encoded, "SGVsbG8sIFdvcmxkIQ==");
    }

    #[test]
    fn bincode_round_trip() {
        let mut c = Canvas::new(3, 3);
        c.set_pixel(1, 1, (200, 100, 50));

        let bytes = bincode::serde::encode_to_vec(&c, bincode::config::standard())
            .expect("Canvas bincode serialize");
        let (decoded, _): (Canvas, _) =
            bincode::serde::decode_from_slice(&bytes, bincode::config::standard())
                .expect("Canvas bincode deserialize");

        assert_eq!(decoded.width(), c.width());
        assert_eq!(decoded.height(), c.height());
        assert_eq!(decoded.get_pixel(1, 1), (200, 100, 50));
        assert_eq!(decoded.get_pixel(0, 0), (0, 0, 0));
    }

    #[test]
    fn pixels_accessor() {
        let mut c = Canvas::new(1, 2);
        c.set_pixel(0, 0, (10, 20, 30));
        c.set_pixel(0, 1, (40, 50, 60));
        assert_eq!(c.pixels(), &[10, 20, 30, 40, 50, 60]);
    }

    #[test]
    fn into_pixels_consumes() {
        let mut c = Canvas::new(1, 1);
        c.set_pixel(0, 0, (7, 8, 9));
        let v = c.into_pixels();
        assert_eq!(v, vec![7, 8, 9]);
    }

    #[test]
    fn animation_accessors() {
        let f = Canvas::new(2, 2);
        let anim = Animation::new(vec![f.clone(), f], 12);
        assert_eq!(anim.fps(), 12);
        assert_eq!(anim.frames().len(), 2);
    }
}
