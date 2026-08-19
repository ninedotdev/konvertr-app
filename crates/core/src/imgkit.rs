//! Image batch kit: social-size presets (resize + smart crop), ASCII art, and
//! palette extraction. Pure logic; the UI runs it on a background thread.

use anyhow::{Context as _, Result};
use image::imageops::FilterType;
use image::{DynamicImage, GenericImageView};
use std::path::{Path, PathBuf};

/// A named output size. `Custom` carries the user's own dimensions.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Preset {
    OgImage,
    Square,
    Story,
    YoutubeThumb,
    XHeader,
    Width1920,
    Width1280,
    Half,
}

impl Preset {
    pub const ALL: [Preset; 8] = [
        Preset::OgImage,
        Preset::Square,
        Preset::Story,
        Preset::YoutubeThumb,
        Preset::XHeader,
        Preset::Width1920,
        Preset::Width1280,
        Preset::Half,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Preset::OgImage => "og image 1200×630",
            Preset::Square => "square 1080",
            Preset::Story => "story 1080×1920",
            Preset::YoutubeThumb => "yt thumb 1280×720",
            Preset::XHeader => "x header 1500×500",
            Preset::Width1920 => "max width 1920",
            Preset::Width1280 => "max width 1280",
            Preset::Half => "half size",
        }
    }

    /// Short tag used in the output file name.
    pub fn slug(self) -> &'static str {
        match self {
            Preset::OgImage => "og",
            Preset::Square => "square",
            Preset::Story => "story",
            Preset::YoutubeThumb => "thumb",
            Preset::XHeader => "header",
            Preset::Width1920 => "w1920",
            Preset::Width1280 => "w1280",
            Preset::Half => "half",
        }
    }

    /// Fixed-size presets crop to fill; width-bound ones only scale down.
    fn target(self, src: (u32, u32)) -> Target {
        match self {
            Preset::OgImage => Target::Exact(1200, 630),
            Preset::Square => Target::Exact(1080, 1080),
            Preset::Story => Target::Exact(1080, 1920),
            Preset::YoutubeThumb => Target::Exact(1280, 720),
            Preset::XHeader => Target::Exact(1500, 500),
            Preset::Width1920 => Target::MaxWidth(1920),
            Preset::Width1280 => Target::MaxWidth(1280),
            Preset::Half => Target::Scale(src.0.div_ceil(2).max(1), src.1.div_ceil(2).max(1)),
        }
    }
}

enum Target {
    Exact(u32, u32),
    MaxWidth(u32),
    Scale(u32, u32),
}

/// How an exact-size preset fills its frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fit {
    /// Scale to cover, then center-crop the overflow.
    Crop,
    /// Scale to fit inside, padding the rest.
    Pad,
}

impl Fit {
    pub const ALL: [Fit; 2] = [Fit::Crop, Fit::Pad];

    pub fn label(self) -> &'static str {
        match self {
            Fit::Crop => "crop",
            Fit::Pad => "pad",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ImgOutcome {
    pub out_path: PathBuf,
    pub in_size: u64,
    pub out_size: u64,
    pub width: u32,
    pub height: u32,
}

/// Geometry for a cover-crop: the scaled size and the top-left crop offset.
/// Pure so the framing math is testable without decoding anything.
pub fn cover_geometry(src: (u32, u32), dst: (u32, u32)) -> ((u32, u32), (u32, u32)) {
    let (sw, sh) = (src.0.max(1), src.1.max(1));
    let (dw, dh) = (dst.0.max(1), dst.1.max(1));
    let scale = (dw as f64 / sw as f64).max(dh as f64 / sh as f64);
    let w = ((sw as f64 * scale).round() as u32).max(dw);
    let h = ((sh as f64 * scale).round() as u32).max(dh);
    ((w, h), ((w - dw) / 2, (h - dh) / 2))
}

fn render(img: &DynamicImage, preset: Preset, fit: Fit) -> DynamicImage {
    match preset.target(img.dimensions()) {
        Target::Scale(w, h) => img.resize_exact(w, h, FilterType::Lanczos3),
        Target::MaxWidth(max) => {
            if img.width() <= max {
                img.clone()
            } else {
                let h = (img.height() as f64 * max as f64 / img.width() as f64).round() as u32;
                img.resize_exact(max, h.max(1), FilterType::Lanczos3)
            }
        }
        Target::Exact(dw, dh) => match fit {
            Fit::Crop => {
                let ((w, h), (x, y)) = cover_geometry(img.dimensions(), (dw, dh));
                img.resize_exact(w, h, FilterType::Lanczos3)
                    .crop_imm(x, y, dw, dh)
            }
            Fit::Pad => {
                let scaled = img.resize(dw, dh, FilterType::Lanczos3);
                let mut canvas = image::RgbaImage::new(dw, dh);
                let x = (dw - scaled.width()) / 2;
                let y = (dh - scaled.height()) / 2;
                image::imageops::overlay(&mut canvas, &scaled.to_rgba8(), x as i64, y as i64);
                DynamicImage::ImageRgba8(canvas)
            }
        },
    }
}

/// Resize `input` into `preset`, writing `<stem>-<preset>.<ext>` beside it.
pub fn resize_file(input: &Path, preset: Preset, fit: Fit) -> Result<ImgOutcome> {
    let in_size = std::fs::metadata(input).map(|m| m.len()).unwrap_or(0);
    let img = crate::decode_image(input)?;
    let out = render(&img, preset, fit);

    // Keep the input's format when we can write it (avif goes through ravif,
    // not the image crate); anything else lands as png.
    let format = input
        .extension()
        .and_then(|e| e.to_str())
        .and_then(crate::OutputFormat::from_extension)
        .unwrap_or(crate::OutputFormat::Png);

    let (w, h) = out.dimensions();
    let encoded = crate::encode(
        &out,
        &crate::ConvertRequest {
            format,
            quality: 0.9,
        },
    )?;
    let out_path = sibling(
        input,
        &format!("{}-{}", stem(input), preset.slug()),
        format.extension(),
    );
    std::fs::write(&out_path, &encoded)
        .with_context(|| format!("writing {}", out_path.display()))?;
    Ok(ImgOutcome {
        out_path,
        in_size,
        out_size: encoded.len() as u64,
        width: w,
        height: h,
    })
}

/// Luminance ramp, darkest to lightest.
const RAMP: &[u8] = b"@%#*+=-:. ";

/// Render `img` as ASCII art `cols` characters wide. Rows are halved because
/// terminal cells are about twice as tall as they are wide.
pub fn to_ascii(img: &DynamicImage, cols: u32, invert: bool) -> String {
    let cols = cols.clamp(16, 400);
    let (w, h) = img.dimensions();
    let rows = ((cols as f64 * h as f64 / w.max(1) as f64) * 0.5).round() as u32;
    let small = img
        .resize_exact(cols, rows.max(1), FilterType::Triangle)
        .to_luma8();
    let mut out = String::with_capacity((cols as usize + 1) * rows as usize);
    for y in 0..small.height() {
        for x in 0..small.width() {
            let mut l = small.get_pixel(x, y)[0] as usize;
            if invert {
                l = 255 - l;
            }
            let ix = l * (RAMP.len() - 1) / 255;
            out.push(RAMP[ix] as char);
        }
        out.push('\n');
    }
    out
}

pub fn ascii_from_file(input: &Path, cols: u32, invert: bool) -> Result<String> {
    Ok(to_ascii(&crate::decode_image(input)?, cols, invert))
}

/// Extract `k` dominant colours (deterministic k-means, no rng: centroids are
/// seeded evenly across the luminance-sorted sample).
pub fn palette(img: &DynamicImage, k: usize) -> Vec<[u8; 3]> {
    let k = k.clamp(1, 12);
    let small = img.resize(120, 120, FilterType::Triangle).to_rgb8();
    let mut pixels: Vec<[f64; 3]> = small
        .pixels()
        .map(|p| [p[0] as f64, p[1] as f64, p[2] as f64])
        .collect();
    if pixels.is_empty() {
        return Vec::new();
    }
    pixels.sort_by(|a, b| luma(a).partial_cmp(&luma(b)).unwrap());

    let mut centroids: Vec<[f64; 3]> = (0..k)
        .map(|i| pixels[i * (pixels.len() - 1) / k.max(1).min(pixels.len()).max(1) % pixels.len()])
        .collect();

    for _ in 0..12 {
        let mut sums = vec![[0.0f64; 3]; k];
        let mut counts = vec![0usize; k];
        for p in &pixels {
            let ci = nearest(p, &centroids);
            for c in 0..3 {
                sums[ci][c] += p[c];
            }
            counts[ci] += 1;
        }
        for i in 0..k {
            if counts[i] > 0 {
                for c in 0..3 {
                    centroids[i][c] = sums[i][c] / counts[i] as f64;
                }
            }
        }
    }

    // Report in order of coverage, most-used first.
    let mut counts = vec![0usize; k];
    for p in &pixels {
        counts[nearest(p, &centroids)] += 1;
    }
    let mut idx: Vec<usize> = (0..k).collect();
    idx.sort_by(|a, b| counts[*b].cmp(&counts[*a]));
    idx.into_iter()
        .filter(|i| counts[*i] > 0)
        .map(|i| {
            [
                centroids[i][0].round().clamp(0.0, 255.0) as u8,
                centroids[i][1].round().clamp(0.0, 255.0) as u8,
                centroids[i][2].round().clamp(0.0, 255.0) as u8,
            ]
        })
        .collect()
}

pub fn palette_from_file(input: &Path, k: usize) -> Result<Vec<[u8; 3]>> {
    Ok(palette(&crate::decode_image(input)?, k))
}

pub fn hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

fn luma(p: &[f64; 3]) -> f64 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

fn nearest(p: &[f64; 3], centroids: &[[f64; 3]]) -> usize {
    let mut best = 0;
    let mut best_d = f64::MAX;
    for (i, c) in centroids.iter().enumerate() {
        let d = (p[0] - c[0]).powi(2) + (p[1] - c[1]).powi(2) + (p[2] - c[2]).powi(2);
        if d < best_d {
            best_d = d;
            best = i;
        }
    }
    best
}

fn stem(path: &Path) -> String {
    path.file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("image")
        .to_string()
}

/// `<base>.<ext>` beside `input`, never overwriting an existing file.
fn sibling(input: &Path, base: &str, ext: &str) -> PathBuf {
    let mut candidate = input.with_file_name(format!("{base}.{ext}"));
    let mut n = 2;
    while candidate.exists() {
        candidate = input.with_file_name(format!("{base}-{n}.{ext}"));
        n += 1;
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(w: u32, h: u32) -> DynamicImage {
        let mut buf = image::RgbImage::new(w, h);
        for (x, y, p) in buf.enumerate_pixels_mut() {
            *p = image::Rgb([(x * 255 / w.max(1)) as u8, (y * 255 / h.max(1)) as u8, 90]);
        }
        DynamicImage::ImageRgb8(buf)
    }

    #[test]
    fn cover_geometry_fills_and_centers() {
        // 1000x1000 into 1200x630: scale by width, crop the tall overflow.
        let ((w, h), (x, y)) = cover_geometry((1000, 1000), (1200, 630));
        assert_eq!((w, h), (1200, 1200));
        assert_eq!(x, 0);
        assert_eq!(y, (1200 - 630) / 2);
        // Wide source into a square: crop the sides.
        let ((w, h), (x, y)) = cover_geometry((2000, 500), (1080, 1080));
        assert_eq!(h, 1080);
        assert!(w >= 1080);
        assert_eq!(y, 0);
        assert!(x > 0);
    }

    #[test]
    fn presets_produce_expected_dimensions() {
        let src = img(800, 600);
        assert_eq!(
            render(&src, Preset::OgImage, Fit::Crop).dimensions(),
            (1200, 630)
        );
        assert_eq!(
            render(&src, Preset::Story, Fit::Pad).dimensions(),
            (1080, 1920)
        );
        assert_eq!(
            render(&src, Preset::Half, Fit::Crop).dimensions(),
            (400, 300)
        );
        // Width-bound presets never upscale.
        assert_eq!(
            render(&src, Preset::Width1920, Fit::Crop).dimensions(),
            (800, 600)
        );
        assert_eq!(
            render(&src, Preset::Width1280, Fit::Crop).dimensions(),
            (800, 600)
        );
        assert_eq!(
            render(&img(2560, 1440), Preset::Width1280, Fit::Crop).dimensions(),
            (1280, 720)
        );
    }

    #[test]
    fn ascii_has_requested_width_and_ramp_chars() {
        let art = to_ascii(&img(200, 100), 40, false);
        let lines: Vec<&str> = art.lines().collect();
        assert!(!lines.is_empty());
        assert!(lines.iter().all(|l| l.chars().count() == 40));
        assert!(art.chars().all(|c| c == '\n' || RAMP.contains(&(c as u8))));
    }

    #[test]
    fn ascii_invert_flips_the_ramp() {
        let mut white = image::RgbImage::new(20, 20);
        for p in white.pixels_mut() {
            *p = image::Rgb([255, 255, 255]);
        }
        let white = DynamicImage::ImageRgb8(white);
        assert!(to_ascii(&white, 16, false).starts_with(' '));
        assert!(to_ascii(&white, 16, true).starts_with('@'));
    }

    #[test]
    fn palette_finds_the_planted_colours() {
        let mut buf = image::RgbImage::new(60, 20);
        for (x, _, p) in buf.enumerate_pixels_mut() {
            *p = match x / 20 {
                0 => image::Rgb([255, 0, 0]),
                1 => image::Rgb([0, 255, 0]),
                _ => image::Rgb([0, 0, 255]),
            };
        }
        let found = palette(&DynamicImage::ImageRgb8(buf), 3);
        assert_eq!(found.len(), 3);
        for target in [[255u8, 0, 0], [0, 255, 0], [0, 0, 255]] {
            assert!(
                found.iter().any(|c| {
                    let d = |a: u8, b: u8| (a as i32 - b as i32).abs();
                    d(c[0], target[0]) + d(c[1], target[1]) + d(c[2], target[2]) < 60
                }),
                "missing {target:?} in {found:?}"
            );
        }
    }

    #[test]
    fn hex_formats_lowercase_padded() {
        assert_eq!(hex([0, 17, 255]), "#0011ff");
    }
}

#[cfg(test)]
mod file_tests {
    use super::*;
    use image::ImageFormat;

    fn tmp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("konvrt-imgkit-{tag}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_sample(dir: &Path, name: &str, format: ImageFormat) -> PathBuf {
        let mut buf = image::RgbaImage::new(300, 200);
        for (x, y, p) in buf.enumerate_pixels_mut() {
            *p = image::Rgba([(x % 255) as u8, (y % 255) as u8, 120, 255]);
        }
        let img = DynamicImage::ImageRgba8(buf);
        let bytes = crate::write_with_format(&img, format).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn resizes_real_files_of_every_input_kind() {
        let dir = tmp_dir("resize");
        for (name, format) in [
            ("a.png", ImageFormat::Png),
            ("a.jpg", ImageFormat::Jpeg),
            ("a.webp", ImageFormat::WebP),
            ("a.bmp", ImageFormat::Bmp),
            ("a.gif", ImageFormat::Gif),
            ("a.tiff", ImageFormat::Tiff),
        ] {
            let src = write_sample(&dir, name, format);
            for fit in Fit::ALL {
                let out = resize_file(&src, Preset::OgImage, fit)
                    .unwrap_or_else(|e| panic!("{name} {fit:?}: {e:#}"));
                assert!(out.out_path.exists(), "{name}: no output written");
                assert_eq!((out.width, out.height), (1200, 630), "{name} {fit:?}");
                assert!(out.out_size > 0, "{name}: empty output");
            }
        }
    }

    /// AVIF decoding rides on macOS's `sips`; other platforms reject it.
    #[test]
    #[cfg(target_os = "macos")]
    fn accepts_avif_input() {
        let dir = tmp_dir("avif");
        let src = write_sample(&dir, "seed.png", ImageFormat::Png);
        // Round-trip through our own AVIF encoder, then read it back in.
        let avif = crate::convert_bytes(
            &std::fs::read(&src).unwrap(),
            &crate::ConvertRequest {
                format: crate::OutputFormat::Avif,
                quality: 0.8,
            },
        )
        .unwrap();
        let avif_path = dir.join("a.avif");
        std::fs::write(&avif_path, avif).unwrap();

        assert!(
            crate::is_supported_input(&avif_path),
            "avif must be accepted"
        );
        let out = resize_file(&avif_path, Preset::Square, Fit::Crop).unwrap();
        assert_eq!((out.width, out.height), (1080, 1080));
        assert!(!ascii_from_file(&avif_path, 40, false).unwrap().is_empty());
    }

    #[test]
    fn ascii_and_palette_read_real_files() {
        let dir = tmp_dir("read");
        let src = write_sample(&dir, "b.png", ImageFormat::Png);
        let art = ascii_from_file(&src, 60, false).unwrap();
        assert!(art.lines().count() > 4);
        assert_eq!(palette_from_file(&src, 5).unwrap().len(), 5);
    }
}
