//! Metadata cleaner: strips Exif/XMP/IPTC/comments from JPEG, PNG and WebP
//! losslessly — the compressed image data is copied byte-for-byte, never
//! re-encoded, so pixels (and ICC color) are untouched.

use anyhow::{Context as _, Result, ensure};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CleanFormat {
    Jpeg,
    Png,
    WebP,
}

impl CleanFormat {
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "jpg" | "jpeg" => Some(CleanFormat::Jpeg),
            "png" => Some(CleanFormat::Png),
            "webp" => Some(CleanFormat::WebP),
            _ => None,
        }
    }
}

pub fn is_supported_input(path: &Path) -> bool {
    detect_format(path).is_some()
}

pub fn detect_format(path: &Path) -> Option<CleanFormat> {
    path.extension()
        .and_then(|e| e.to_str())
        .and_then(CleanFormat::from_extension)
}

#[derive(Clone, Debug)]
pub struct CleanReport {
    pub out_path: PathBuf,
    pub in_size: u64,
    pub out_size: u64,
    /// Names of the stripped segments ("Exif", "XMP", "IPTC", "comment", …).
    pub removed: Vec<String>,
}

/// Read `input`, strip metadata, write `<stem>-clean.<ext>` next to it.
/// Never overwrites.
pub fn clean_file(input: &Path) -> Result<CleanReport> {
    let format = detect_format(input)
        .with_context(|| format!("unsupported input format: {}", input.display()))?;
    let bytes = std::fs::read(input).with_context(|| format!("reading {}", input.display()))?;
    let (out, removed) = clean_bytes(&bytes, format)?;
    let out_path = output_path(input);
    std::fs::write(&out_path, &out).with_context(|| format!("writing {}", out_path.display()))?;
    Ok(CleanReport {
        out_path,
        in_size: bytes.len() as u64,
        out_size: out.len() as u64,
        removed,
    })
}

pub fn clean_bytes(input: &[u8], format: CleanFormat) -> Result<(Vec<u8>, Vec<String>)> {
    match format {
        CleanFormat::Jpeg => clean_jpeg(input),
        CleanFormat::Png => clean_png(input),
        CleanFormat::WebP => clean_webp(input),
    }
}

fn output_path(input: &Path) -> PathBuf {
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("cleaned");
    let ext = input
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default();
    let sibling = |name: String| input.with_file_name(name);

    let mut candidate = sibling(format!("{stem}-clean.{ext}"));
    let mut n = 2;
    while candidate.exists() {
        candidate = sibling(format!("{stem}-clean-{n}.{ext}"));
        n += 1;
    }
    candidate
}

fn note(removed: &mut Vec<String>, name: &str) {
    if !removed.iter().any(|r| r == name) {
        removed.push(name.to_string());
    }
}

// Copies every segment except APP1 (Exif/XMP), APP13 (IPTC) and COM. APP0
// (JFIF) and APP2 (ICC) stay so color doesn't shift.
fn clean_jpeg(input: &[u8]) -> Result<(Vec<u8>, Vec<String>)> {
    ensure!(
        input.len() >= 4 && input[0..2] == [0xFF, 0xD8],
        "not a JPEG"
    );
    let mut out = vec![0xFF, 0xD8];
    let mut removed = Vec::new();
    let mut i = 2;
    loop {
        ensure!(i + 1 < input.len(), "truncated JPEG");
        ensure!(input[i] == 0xFF, "corrupt JPEG segment at byte {i}");
        let marker = input[i + 1];
        match marker {
            // Fill byte before a marker.
            0xFF => i += 1,
            // EOI before any scan (degenerate but valid).
            0xD9 => {
                out.extend_from_slice(&[0xFF, 0xD9]);
                break;
            }
            // SOS: entropy-coded data follows; copy through EOI verbatim.
            0xDA => {
                out.extend_from_slice(&input[i..]);
                break;
            }
            // Standalone markers without a length field.
            0x01 | 0xD0..=0xD7 => {
                out.extend_from_slice(&[0xFF, marker]);
                i += 2;
            }
            _ => {
                ensure!(i + 3 < input.len(), "truncated JPEG segment");
                let seg_len = u16::from_be_bytes([input[i + 2], input[i + 3]]) as usize;
                ensure!(
                    seg_len >= 2 && i + 2 + seg_len <= input.len(),
                    "bad JPEG segment length"
                );
                let data = &input[i + 4..i + 2 + seg_len];
                let strip = match marker {
                    0xE1 => {
                        note(&mut removed, app1_name(data));
                        true
                    }
                    0xED => {
                        note(&mut removed, "IPTC");
                        true
                    }
                    0xFE => {
                        note(&mut removed, "comment");
                        true
                    }
                    _ => false,
                };
                if !strip {
                    out.extend_from_slice(&input[i..i + 2 + seg_len]);
                }
                i += 2 + seg_len;
            }
        }
    }
    Ok((out, removed))
}

fn app1_name(data: &[u8]) -> &'static str {
    if data.starts_with(b"Exif\0") {
        "Exif"
    } else if data.starts_with(b"http://ns.adobe.com/xap/") {
        "XMP"
    } else {
        "APP1"
    }
}

// Copies signature + chunks with their original CRCs, dropping
// tEXt/zTXt/iTXt/eXIf/tIME.
const PNG_SIG: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

fn clean_png(input: &[u8]) -> Result<(Vec<u8>, Vec<String>)> {
    ensure!(input.starts_with(&PNG_SIG), "not a PNG");
    let mut out = PNG_SIG.to_vec();
    let mut removed = Vec::new();
    let mut i = 8;
    while i + 12 <= input.len() {
        let clen =
            u32::from_be_bytes([input[i], input[i + 1], input[i + 2], input[i + 3]]) as usize;
        let ctype: [u8; 4] = input[i + 4..i + 8].try_into().unwrap();
        let total = 12 + clen;
        ensure!(i + total <= input.len(), "truncated PNG chunk");
        if matches!(&ctype, b"tEXt" | b"zTXt" | b"iTXt" | b"eXIf" | b"tIME") {
            note(&mut removed, std::str::from_utf8(&ctype).unwrap_or("text"));
        } else {
            out.extend_from_slice(&input[i..i + total]);
        }
        i += total;
        if &ctype == b"IEND" {
            break;
        }
    }
    Ok((out, removed))
}

// RIFF chunks minus EXIF / "XMP "; the RIFF size is recomputed and the VP8X
// flag bits for the removed chunks cleared.
fn clean_webp(input: &[u8]) -> Result<(Vec<u8>, Vec<String>)> {
    ensure!(
        input.len() >= 12 && &input[0..4] == b"RIFF" && &input[8..12] == b"WEBP",
        "not a WebP"
    );

    // First pass: find which metadata chunks exist.
    let mut has_exif = false;
    let mut has_xmp = false;
    let mut i = 12;
    while i + 8 <= input.len() {
        let fourcc = &input[i..i + 4];
        let size =
            u32::from_le_bytes([input[i + 4], input[i + 5], input[i + 6], input[i + 7]]) as usize;
        let padded = size + (size & 1);
        ensure!(i + 8 + padded <= input.len(), "truncated WebP chunk");
        if fourcc == b"EXIF" {
            has_exif = true;
        } else if fourcc == b"XMP " {
            has_xmp = true;
        }
        i += 8 + padded;
    }

    let mut removed = Vec::new();
    if has_exif {
        note(&mut removed, "Exif");
    }
    if has_xmp {
        note(&mut removed, "XMP");
    }

    // Second pass: copy every chunk except the stripped ones.
    let mut body = Vec::with_capacity(input.len());
    let mut i = 12;
    while i + 8 <= input.len() {
        let fourcc = &input[i..i + 4];
        let size =
            u32::from_le_bytes([input[i + 4], input[i + 5], input[i + 6], input[i + 7]]) as usize;
        let padded = size + (size & 1);
        if fourcc == b"EXIF" || fourcc == b"XMP " {
            i += 8 + padded;
            continue;
        }
        let start = body.len();
        body.extend_from_slice(&input[i..i + 8 + padded]);
        // VP8X flags byte: ...ICC|Alpha|EXIF|XMP|Anim..; clear the bits for
        // the chunks we dropped so the header stays consistent.
        if fourcc == b"VP8X" && size >= 1 {
            if has_exif {
                body[start + 8] &= !0x08;
            }
            if has_xmp {
                body[start + 8] &= !0x04;
            }
        }
        i += 8 + padded;
    }

    let mut out = Vec::with_capacity(body.len() + 12);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((body.len() as u32 + 4).to_le_bytes()));
    out.extend_from_slice(b"WEBP");
    out.extend_from_slice(&body);
    Ok((out, removed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::jpeg::JpegEncoder;
    use image::{DynamicImage, ImageFormat};
    use std::io::Cursor;

    fn tiny_image() -> DynamicImage {
        let mut img = image::RgbImage::new(4, 4);
        for (x, y, px) in img.enumerate_pixels_mut() {
            *px = image::Rgb([(x * 60) as u8, (y * 60) as u8, 128]);
        }
        DynamicImage::ImageRgb8(img)
    }

    fn tiny_jpeg() -> Vec<u8> {
        let mut buf = Vec::new();
        let enc = JpegEncoder::new_with_quality(Cursor::new(&mut buf), 90);
        tiny_image().to_rgb8().write_with_encoder(enc).unwrap();
        buf
    }

    fn with_app1_exif(jpeg: &[u8]) -> Vec<u8> {
        let payload = b"Exif\0\0fake-tiff-data-with-gps";
        let mut out = jpeg[0..2].to_vec();
        out.extend_from_slice(&[0xFF, 0xE1]);
        out.extend_from_slice(&((payload.len() as u16 + 2).to_be_bytes()));
        out.extend_from_slice(payload);
        out.extend_from_slice(&jpeg[2..]);
        out
    }

    #[test]
    fn jpeg_strips_exif_and_still_decodes() {
        let tagged = with_app1_exif(&tiny_jpeg());
        assert!(tagged.windows(2).any(|w| w == [0xFF, 0xE1]));
        let (clean, removed) = clean_bytes(&tagged, CleanFormat::Jpeg).unwrap();
        assert_eq!(removed, vec!["Exif"]);
        // No APP1 marker survives in the header (entropy data can contain
        // any bytes, so only scan up to SOS).
        let sos = clean.windows(2).position(|w| w == [0xFF, 0xDA]).unwrap();
        assert!(!clean[..sos].windows(2).any(|w| w == [0xFF, 0xE1]));
        let decoded = image::load_from_memory(&clean).unwrap();
        assert_eq!(decoded.width(), 4);
    }

    #[test]
    fn jpeg_without_metadata_is_untouched() {
        let plain = tiny_jpeg();
        let (clean, removed) = clean_bytes(&plain, CleanFormat::Jpeg).unwrap();
        assert!(removed.is_empty());
        assert_eq!(clean, plain);
    }

    #[test]
    fn png_strips_text_chunks_and_still_decodes() {
        let mut png = Vec::new();
        tiny_image()
            .write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
            .unwrap();
        // Splice a tEXt chunk after IHDR (8-byte sig + 25-byte IHDR chunk).
        let mut tagged = png[..33].to_vec();
        let text = b"Comment\0made by someone at some GPS location";
        tagged.extend_from_slice(&(text.len() as u32).to_be_bytes());
        tagged.extend_from_slice(b"tEXt");
        tagged.extend_from_slice(text);
        tagged.extend_from_slice(&[0, 0, 0, 0]); // CRC never checked: chunk is dropped
        tagged.extend_from_slice(&png[33..]);

        let (clean, removed) = clean_bytes(&tagged, CleanFormat::Png).unwrap();
        assert_eq!(removed, vec!["tEXt"]);
        assert_eq!(clean, png);
        let decoded = image::load_from_memory(&clean).unwrap();
        assert_eq!(decoded.width(), 4);
    }

    #[test]
    fn webp_strips_exif_chunk_and_fixes_riff_size() {
        // Synthetic RIFF: a fake image chunk plus EXIF + XMP metadata.
        let mut chunks = Vec::new();
        for (fourcc, data) in [
            (b"VP8 ", &b"fake-bitstream"[..]),
            (b"EXIF", &b"gps-coords"[..]),
            (b"XMP ", &b"<xmp/>"[..]),
        ] {
            chunks.extend_from_slice(fourcc);
            chunks.extend_from_slice(&(data.len() as u32).to_le_bytes());
            chunks.extend_from_slice(data);
            if data.len() % 2 == 1 {
                chunks.push(0);
            }
        }
        let mut webp = Vec::new();
        webp.extend_from_slice(b"RIFF");
        webp.extend_from_slice(&((chunks.len() as u32 + 4).to_le_bytes()));
        webp.extend_from_slice(b"WEBP");
        webp.extend_from_slice(&chunks);

        let (clean, removed) = clean_bytes(&webp, CleanFormat::WebP).unwrap();
        assert_eq!(removed, vec!["Exif", "XMP"]);
        assert!(!clean.windows(4).any(|w| w == b"EXIF"));
        assert!(!clean.windows(4).any(|w| w == b"XMP "));
        let riff_size = u32::from_le_bytes([clean[4], clean[5], clean[6], clean[7]]) as usize;
        assert_eq!(riff_size + 8, clean.len());
    }

    #[test]
    fn detects_formats() {
        assert_eq!(CleanFormat::from_extension("JPG"), Some(CleanFormat::Jpeg));
        assert!(is_supported_input(Path::new("a/photo.webp")));
        assert!(!is_supported_input(Path::new("a/movie.mp4")));
    }
}
