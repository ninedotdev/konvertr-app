//! konvrt-core: pure conversion logic, no UI dependencies.
//! Everything here is synchronous and `Send`; the UI runs it on a background
//! executor.

pub mod audio;
pub mod b64;
pub mod clean;
pub mod color;
pub mod data;
pub mod devutils;
pub mod hash;
pub mod icons;
pub mod imgkit;
pub mod pdf;
pub mod svg;
pub mod textkit;
pub mod video;
pub mod vstudio;
pub mod yoinks;

use anyhow::{Context as _, Result};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use image::{DynamicImage, ImageFormat};
use std::io::Cursor;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OutputFormat {
    Avif,
    Bmp,
    Gif,
    Icns,
    Ico,
    Jpeg,
    Png,
    Tiff,
    WebP,
}

impl OutputFormat {
    pub const ALL: [OutputFormat; 9] = [
        OutputFormat::Avif,
        OutputFormat::Bmp,
        OutputFormat::Gif,
        OutputFormat::Icns,
        OutputFormat::Ico,
        OutputFormat::Jpeg,
        OutputFormat::Png,
        OutputFormat::Tiff,
        OutputFormat::WebP,
    ];

    pub fn label(self) -> &'static str {
        match self {
            OutputFormat::Avif => "AVIF",
            OutputFormat::Bmp => "BMP",
            OutputFormat::Gif => "GIF",
            OutputFormat::Icns => "ICNS",
            OutputFormat::Ico => "ICO",
            OutputFormat::Jpeg => "JPEG",
            OutputFormat::Png => "PNG",
            OutputFormat::Tiff => "TIFF",
            OutputFormat::WebP => "WebP",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            OutputFormat::Avif => "avif",
            OutputFormat::Bmp => "bmp",
            OutputFormat::Gif => "gif",
            OutputFormat::Icns => "icns",
            OutputFormat::Ico => "ico",
            OutputFormat::Jpeg => "jpg",
            OutputFormat::Png => "png",
            OutputFormat::Tiff => "tiff",
            OutputFormat::WebP => "webp",
        }
    }

    /// The format an extension maps to, when we can encode it.
    pub fn from_extension(ext: &str) -> Option<OutputFormat> {
        Some(match ext.to_ascii_lowercase().as_str() {
            "avif" => OutputFormat::Avif,
            "bmp" => OutputFormat::Bmp,
            "gif" => OutputFormat::Gif,
            "icns" => OutputFormat::Icns,
            "ico" => OutputFormat::Ico,
            "jpg" | "jpeg" => OutputFormat::Jpeg,
            "png" => OutputFormat::Png,
            "tif" | "tiff" => OutputFormat::Tiff,
            "webp" => OutputFormat::WebP,
            _ => return None,
        })
    }

    /// Formats whose encoder takes a quality knob.
    pub fn supports_quality(self) -> bool {
        matches!(
            self,
            OutputFormat::Jpeg | OutputFormat::WebP | OutputFormat::Avif
        )
    }
}

/// Input extensions the image converter accepts. avif/heic/heif decode through
/// [`decode_image`]'s macOS fallback.
pub const INPUT_EXTENSIONS: [&str; 12] = [
    "png", "jpg", "jpeg", "webp", "gif", "bmp", "tif", "tiff", "ico", "avif", "heic", "heif",
];

pub fn is_supported_input(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| INPUT_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

#[derive(Clone, Copy, Debug)]
pub struct ConvertRequest {
    pub format: OutputFormat,
    /// 0.0..=1.0; only used when `format.supports_quality()`.
    pub quality: f32,
}

#[derive(Clone, Debug)]
pub struct ConvertOutcome {
    pub out_path: PathBuf,
    pub in_size: u64,
    pub out_size: u64,
}

/// Decode any accepted input. The `image` crate handles most formats; AVIF and
/// HEIC go through macOS's `sips`, which reads both natively.
pub fn decode_image(input: &Path) -> Result<DynamicImage> {
    let bytes = std::fs::read(input).with_context(|| format!("reading {}", input.display()))?;
    if let Ok(img) = image::load_from_memory(&bytes) {
        return Ok(img);
    }
    decode_via_sips(input).with_context(|| format!("could not decode {}", input.display()))
}

#[cfg(target_os = "macos")]
fn decode_via_sips(input: &Path) -> Result<DynamicImage> {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let tmp = std::env::temp_dir().join(format!(
        "konvrt-decode-{}-{}.png",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    let status = std::process::Command::new("sips")
        .args(["-s", "format", "png"])
        .arg(input)
        .arg("--out")
        .arg(&tmp)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .context("running sips")?;
    if !status.success() {
        anyhow::bail!("sips could not read this file");
    }
    let img = image::open(&tmp).context("decoding the converted file");
    let _ = std::fs::remove_file(&tmp);
    img
}

#[cfg(not(target_os = "macos"))]
fn decode_via_sips(_input: &Path) -> Result<DynamicImage> {
    anyhow::bail!("unsupported image format")
}

/// Read `input`, convert, and write the result next to it. Never overwrites.
pub fn convert_file(input: &Path, req: &ConvertRequest) -> Result<ConvertOutcome> {
    let in_size = std::fs::metadata(input).map(|m| m.len()).unwrap_or(0);
    let out = encode(&decode_image(input)?, req)?;
    let out_path = output_path(input, req.format);
    std::fs::write(&out_path, &out).with_context(|| format!("writing {}", out_path.display()))?;
    Ok(ConvertOutcome {
        out_path,
        in_size,
        out_size: out.len() as u64,
    })
}

pub fn convert_bytes(input: &[u8], req: &ConvertRequest) -> Result<Vec<u8>> {
    let img = image::load_from_memory(input).context("could not decode image")?;
    encode(&img, req)
}

pub fn encode(img: &DynamicImage, req: &ConvertRequest) -> Result<Vec<u8>> {
    let q = req.quality.clamp(0.1, 1.0);
    match req.format {
        OutputFormat::Png => write_with_format(img, ImageFormat::Png),
        OutputFormat::Gif => write_with_format(img, ImageFormat::Gif),
        OutputFormat::Tiff => {
            write_with_format(&DynamicImage::ImageRgba8(img.to_rgba8()), ImageFormat::Tiff)
        }
        OutputFormat::Bmp => write_with_format(&flatten_white(img), ImageFormat::Bmp),
        OutputFormat::Jpeg => {
            let rgb = flatten_white(img).to_rgb8();
            let mut buf = Vec::new();
            let enc = JpegEncoder::new_with_quality(Cursor::new(&mut buf), (q * 100.0) as u8);
            rgb.write_with_encoder(enc).context("encoding jpeg")?;
            Ok(buf)
        }
        OutputFormat::WebP => {
            let rgba = img.to_rgba8();
            let (w, h) = rgba.dimensions();
            let enc = webp::Encoder::from_rgba(rgba.as_raw(), w, h);
            Ok(enc.encode(q * 100.0).to_vec())
        }
        OutputFormat::Avif => encode_avif(img, q),
        OutputFormat::Ico => encode_ico(img),
        OutputFormat::Icns => encode_icns(img),
    }
}

pub fn write_with_format(img: &DynamicImage, format: ImageFormat) -> Result<Vec<u8>> {
    let mut buf = Cursor::new(Vec::new());
    img.write_to(&mut buf, format)
        .with_context(|| format!("encoding {format:?}"))?;
    Ok(buf.into_inner())
}

/// Composite over white and drop alpha (jpeg/bmp can't carry transparency).
fn flatten_white(img: &DynamicImage) -> DynamicImage {
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let mut out = image::RgbImage::new(w, h);
    for (x, y, px) in rgba.enumerate_pixels() {
        let a = px[3] as u32;
        let blend = |c: u8| ((c as u32 * a + 255 * (255 - a)) / 255) as u8;
        out.put_pixel(x, y, image::Rgb([blend(px[0]), blend(px[1]), blend(px[2])]));
    }
    DynamicImage::ImageRgb8(out)
}

fn encode_avif(img: &DynamicImage, q: f32) -> Result<Vec<u8>> {
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let pixels: Vec<rgb::RGBA8> = rgba
        .pixels()
        .map(|p| rgb::RGBA8::new(p[0], p[1], p[2], p[3]))
        .collect();
    let encoded = ravif::Encoder::new()
        .with_quality(q * 100.0)
        .with_speed(6)
        .encode_rgba(ravif::Img::new(pixels.as_slice(), w as usize, h as usize))
        .context("encoding avif")?;
    Ok(encoded.avif_file)
}

/// ICO capped at 256px (format limit); PNG-compressed payload.
pub fn encode_ico(img: &DynamicImage) -> Result<Vec<u8>> {
    let img = if img.width() > 256 || img.height() > 256 {
        img.resize(256, 256, FilterType::Lanczos3)
    } else {
        img.clone()
    };
    write_with_format(&DynamicImage::ImageRgba8(img.to_rgba8()), ImageFormat::Ico)
}

/// Apple ICNS container: "icns" magic + big-endian (type, length, PNG) chunks.
pub fn encode_icns(img: &DynamicImage) -> Result<Vec<u8>> {
    const TYPES: [(&[u8; 4], u32); 4] = [
        (b"ic07", 128),
        (b"ic08", 256),
        (b"ic09", 512),
        (b"ic10", 1024),
    ];
    let mut body = Vec::new();
    for (tag, size) in TYPES {
        let resized = img.resize_exact(size, size, FilterType::Lanczos3);
        let png = write_with_format(&resized, ImageFormat::Png)?;
        body.extend_from_slice(tag.as_slice());
        body.extend_from_slice(&((png.len() as u32 + 8).to_be_bytes()));
        body.extend_from_slice(&png);
    }
    let mut out = Vec::with_capacity(body.len() + 8);
    out.extend_from_slice(b"icns");
    out.extend_from_slice(&((body.len() as u32 + 8).to_be_bytes()));
    out.extend_from_slice(&body);
    Ok(out)
}

/// Sibling path for the converted file; never collides with the input or an
/// existing file (appends `-konverted`, then `-2`, `-3`, ...).
pub fn output_path(input: &Path, format: OutputFormat) -> PathBuf {
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("converted");
    let ext = format.extension();
    let sibling = |name: String| input.with_file_name(name);

    let mut candidate = sibling(format!("{stem}.{ext}"));
    if candidate == input {
        candidate = sibling(format!("{stem}-konverted.{ext}"));
    }
    let base = candidate
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(stem)
        .to_string();
    let mut n = 2;
    while candidate.exists() {
        candidate = sibling(format!("{base}-{n}.{ext}"));
        n += 1;
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        let mut img = image::RgbaImage::new(8, 8);
        for (x, y, px) in img.enumerate_pixels_mut() {
            *px = image::Rgba([(x * 30) as u8, (y * 30) as u8, 128, 200]);
        }
        write_with_format(&DynamicImage::ImageRgba8(img), ImageFormat::Png).unwrap()
    }

    #[test]
    fn converts_to_every_format() {
        let png = sample();
        for format in OutputFormat::ALL {
            let out = convert_bytes(
                &png,
                &ConvertRequest {
                    format,
                    quality: 0.8,
                },
            )
            .unwrap_or_else(|e| panic!("{}: {e:#}", format.label()));
            assert!(!out.is_empty(), "{} produced no bytes", format.label());
        }
    }

    #[test]
    fn jpeg_roundtrips_and_flattens_alpha() {
        let out = convert_bytes(
            &sample(),
            &ConvertRequest {
                format: OutputFormat::Jpeg,
                quality: 0.9,
            },
        )
        .unwrap();
        let decoded = image::load_from_memory(&out).unwrap();
        assert_eq!(decoded.width(), 8);
        assert_eq!(decoded.color().channel_count(), 3);
    }

    #[test]
    fn icns_has_magic_and_length() {
        let out = convert_bytes(
            &sample(),
            &ConvertRequest {
                format: OutputFormat::Icns,
                quality: 1.0,
            },
        )
        .unwrap();
        assert_eq!(&out[0..4], b"icns");
        let len = u32::from_be_bytes([out[4], out[5], out[6], out[7]]);
        assert_eq!(len as usize, out.len());
    }

    #[test]
    fn output_path_avoids_input_collision() {
        let p = Path::new("/nope/photo.png");
        assert_eq!(
            output_path(p, OutputFormat::Png),
            Path::new("/nope/photo-konverted.png")
        );
        assert_eq!(
            output_path(p, OutputFormat::WebP),
            Path::new("/nope/photo.webp")
        );
    }

    #[test]
    fn detects_supported_inputs() {
        assert!(is_supported_input(Path::new("a/b/IMG.JPEG")));
        assert!(!is_supported_input(Path::new("a/b/movie.mp4")));
        assert!(!is_supported_input(Path::new("noext")));
    }
}
