//! Built-in imshow-style viewer for `sb topic listen --show`.
//!
//! [`ImageView`] shows `sensor_msgs/Image`- and `CompressedImage`-shaped
//! frames in a `minifb` window. Pure Rust, no system library at link time: the
//! X11 / Wayland client libraries are loaded at runtime, so a headless box gets
//! an `Err` from [`ImageView::open`] instead of a startup failure.

use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use image::{ImageFormat, RgbImage};
use minifb::{Key, ScaleMode, Window, WindowOptions};
use serde_json::{Map, Value};

/// Window size used until the first frame supplies the real dimensions.
const PLACEHOLDER_SIZE: (usize, usize) = (320, 240);

/// Minimum spacing between window updates (about 60 Hz). A faster frame is
/// converted and presented by the next due call. Nothing here sleeps.
const MIN_UPDATE_INTERVAL: Duration = Duration::from_millis(16);

const SUPPORTED_ENCODINGS: &str = "mono8, mono16, rgb8, bgr8, rgba8, bgra8, 8UC1, 8UC3, 16UC1";

/// Raw pixel layouts accepted by [`ImageView::show_raw`], named after ROS
/// `sensor_msgs/image_encodings`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PixelFormat {
    Mono8,
    Mono16,
    Rgb8,
    Bgr8,
    Rgba8,
    Bgra8,
}

impl PixelFormat {
    fn parse(encoding: &str) -> Option<Self> {
        match encoding.trim().to_ascii_lowercase().as_str() {
            "mono8" | "8uc1" => Some(Self::Mono8),
            "mono16" | "16uc1" => Some(Self::Mono16),
            "rgb8" => Some(Self::Rgb8),
            "bgr8" | "8uc3" => Some(Self::Bgr8),
            "rgba8" => Some(Self::Rgba8),
            "bgra8" => Some(Self::Bgra8),
            _ => None,
        }
    }

    fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Mono8 => 1,
            Self::Mono16 => 2,
            Self::Rgb8 | Self::Bgr8 => 3,
            Self::Rgba8 | Self::Bgra8 => 4,
        }
    }
}

/// One image window plus the frame it is showing.
///
/// The window is resizable and keeps the frame's aspect ratio. When the frame
/// dimensions change, the window is recreated at the new size. Keep the view
/// on the thread that opened it; dropping it closes the window.
pub struct ImageView {
    title: String,
    window: Window,
    /// Current frame in minifb's `0RGB` layout, `width * height` pixels.
    buffer: Vec<u32>,
    width: usize,
    height: usize,
    /// Time of the last window update; `None` makes the next call update.
    last_update: Option<Instant>,
    /// `buffer` holds a frame that has not been presented yet.
    pending: bool,
}

impl ImageView {
    /// Open a window titled `title`, black until the first frame arrives.
    ///
    /// Returns `Err` ("no display available for --show") when no X11 or
    /// Wayland display can be reached, for example on a headless system.
    pub fn open(title: &str) -> Result<Self> {
        let (width, height) = PLACEHOLDER_SIZE;
        let window = new_window(title, width, height)
            .map_err(|e| anyhow!("no display available for --show ({e})"))?;
        let mut view = Self {
            title: title.to_owned(),
            window,
            buffer: vec![0; width * height],
            width,
            height,
            last_update: None,
            pending: true,
        };
        view.refresh()?;
        Ok(view)
    }

    /// Show one raw frame.
    ///
    /// `encoding` follows ROS `image_encodings`: `mono8`, `mono16` (shown as
    /// `value >> 8`, little-endian), `rgb8`, `bgr8`, `rgba8`, `bgra8`, `8UC1`,
    /// `8UC3` (as `bgr8`) and `16UC1` (as `mono16`). `step` is bytes per row;
    /// `0` means tightly packed. Returns `Ok(false)` once the user closed the
    /// window or pressed Escape or `q`, `Ok(true)` otherwise. An unknown encoding or
    /// a `data` slice too short for `width`, `height` and `step` is an `Err`
    /// that names the problem; the window keeps the previous frame.
    pub fn show_raw(
        &mut self,
        width: u32,
        height: u32,
        encoding: &str,
        step: u32,
        data: &[u8],
    ) -> Result<bool> {
        let width = usize::try_from(width)?;
        let height = usize::try_from(height)?;
        let step = usize::try_from(step)?;
        convert_raw(width, height, encoding, step, data, &mut self.buffer)?;
        self.commit(width, height)
    }

    /// Decode a compressed frame and show it.
    ///
    /// `format` is the `sensor_msgs/CompressedImage` string (`"jpeg"`,
    /// `"png"`, `"bgr8; jpeg compressed bgr8"`, ...). The payload's magic
    /// bytes take precedence; `format` is the fallback hint. JPEG and PNG are
    /// supported. Return semantics match [`ImageView::show_raw`].
    pub fn show_compressed(&mut self, format: &str, data: &[u8]) -> Result<bool> {
        let rgb = decode_compressed(format, data)?;
        let width = usize::try_from(rgb.width())?;
        let height = usize::try_from(rgb.height())?;
        if width == 0 || height == 0 {
            bail!("compressed {format:?} frame decoded to an empty {width}x{height} image");
        }
        self.buffer.clear();
        self.buffer
            .extend(rgb.pixels().map(|p| pack(p[0], p[1], p[2])));
        self.commit(width, height)
    }

    /// Process window events without a new frame, so the window stays
    /// responsive between samples. Also presents a frame that an earlier
    /// call had to hold back. Returns `Ok(false)` once the window is closed.
    pub fn pump(&mut self) -> Result<bool> {
        self.refresh()
    }

    /// Adopt the frame now in `buffer`, recreating the window on a size change.
    fn commit(&mut self, width: usize, height: usize) -> Result<bool> {
        self.pending = true;
        if (width, height) != (self.width, self.height) {
            self.width = width;
            self.height = height;
            self.window = new_window(&self.title, width, height)
                .map_err(|e| anyhow!("cannot reopen image window at {width}x{height} ({e})"))?;
            self.last_update = None;
        }
        self.refresh()
    }

    /// Update an open window if it is due: present the pending frame, or just
    /// poll events. Never blocks.
    fn refresh(&mut self) -> Result<bool> {
        let due = self
            .last_update
            .is_none_or(|t| t.elapsed() >= MIN_UPDATE_INTERVAL);
        if due && self.window.is_open() {
            if self.pending {
                self.window
                    .update_with_buffer(&self.buffer, self.width, self.height)
                    .map_err(|e| anyhow!("image window update failed ({e})"))?;
                self.pending = false;
            } else {
                self.window.update();
            }
            self.last_update = Some(Instant::now());
        }
        Ok(self.window.is_open()
            && !self.window.is_key_down(Key::Escape)
            && !self.window.is_key_down(Key::Q))
    }
}

/// True when a decoded JSON object looks like an image message: numeric
/// `width` and `height`, a string `encoding`, and a `data` field of any shape.
pub fn looks_like_image(obj: &Map<String, Value>) -> bool {
    obj.get("width").is_some_and(Value::is_number)
        && obj.get("height").is_some_and(Value::is_number)
        && obj.get("encoding").is_some_and(Value::is_string)
        && obj.contains_key("data")
}

/// True when the object looks like `sensor_msgs/CompressedImage`: a string
/// `format`, a `data` field of any shape, and no `width`.
pub fn looks_like_compressed_image(obj: &Map<String, Value>) -> bool {
    obj.get("format").is_some_and(Value::is_string)
        && obj.contains_key("data")
        && !obj.contains_key("width")
}

/// Create a resizable, aspect-preserving window with minifb's own frame
/// limiter off (it sleeps inside `update`; [`ImageView::refresh`] throttles).
fn new_window(title: &str, width: usize, height: usize) -> minifb::Result<Window> {
    let opts = WindowOptions {
        resize: true,
        scale_mode: ScaleMode::AspectRatioStretch,
        ..WindowOptions::default()
    };
    let mut window = Window::new(title, width, height, opts)?;
    window.set_target_fps(0);
    Ok(window)
}

/// Pack 8-bit channels into minifb's `0RGB` pixel.
fn pack(r: u8, g: u8, b: u8) -> u32 {
    (u32::from(r) << 16) | (u32::from(g) << 8) | u32::from(b)
}

/// Convert one ROS-encoded raw frame into `out` as `0RGB` pixels.
///
/// Everything is validated before `out` is touched, so on `Err` it still
/// holds the previous frame.
fn convert_raw(
    width: usize,
    height: usize,
    encoding: &str,
    step: usize,
    data: &[u8],
    out: &mut Vec<u32>,
) -> Result<()> {
    let format = PixelFormat::parse(encoding).ok_or_else(|| {
        anyhow!("unsupported image encoding {encoding:?} (supported: {SUPPORTED_ENCODINGS})")
    })?;
    if width == 0 || height == 0 {
        bail!("empty {width}x{height} {encoding} frame");
    }
    let bpp = format.bytes_per_pixel();
    let row_bytes = width
        .checked_mul(bpp)
        .context("image row size overflows usize")?;
    let stride = if step == 0 { row_bytes } else { step };
    if stride < row_bytes {
        bail!("{encoding} step {stride} is less than width {width} x {bpp} bytes = {row_bytes}");
    }
    // The last row need not carry its padding.
    let needed = stride
        .checked_mul(height - 1)
        .and_then(|n| n.checked_add(row_bytes))
        .context("image size overflows usize")?;
    if data.len() < needed {
        bail!(
            "{width}x{height} {encoding} frame (step {stride}) needs {needed} bytes, got {}",
            data.len()
        );
    }

    out.clear();
    out.reserve(width * height);
    for row in data.chunks(stride).take(height) {
        let row = &row[..row_bytes];
        match format {
            PixelFormat::Mono8 => out.extend(row.iter().map(|&v| pack(v, v, v))),
            // Little-endian u16: `value >> 8` is the high (second) byte.
            PixelFormat::Mono16 => out.extend(row.chunks_exact(2).map(|p| pack(p[1], p[1], p[1]))),
            PixelFormat::Rgb8 => out.extend(row.chunks_exact(3).map(|p| pack(p[0], p[1], p[2]))),
            PixelFormat::Bgr8 => out.extend(row.chunks_exact(3).map(|p| pack(p[2], p[1], p[0]))),
            PixelFormat::Rgba8 => out.extend(row.chunks_exact(4).map(|p| pack(p[0], p[1], p[2]))),
            PixelFormat::Bgra8 => out.extend(row.chunks_exact(4).map(|p| pack(p[2], p[1], p[0]))),
        }
    }
    Ok(())
}

/// Decode a `sensor_msgs/CompressedImage` payload to 8-bit RGB.
fn decode_compressed(format: &str, data: &[u8]) -> Result<RgbImage> {
    let hint = format.to_ascii_lowercase();
    let image_format = image::guess_format(data)
        .ok()
        .or_else(|| {
            if hint.contains("png") {
                Some(ImageFormat::Png)
            } else if hint.contains("jpeg") || hint.contains("jpg") {
                Some(ImageFormat::Jpeg)
            } else {
                None
            }
        })
        .ok_or_else(|| {
            anyhow!("unsupported compressed image format {format:?} (supported: jpeg, png)")
        })?;
    let decoded = image::load_from_memory_with_format(data, image_format)
        .with_context(|| format!("cannot decode {format:?} frame ({} bytes)", data.len()))?;
    Ok(decoded.into_rgb8())
}
