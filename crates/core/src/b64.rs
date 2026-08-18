//! Base64 encode/decode of files: file -> data URL text, and back.

use anyhow::{Context as _, Result, bail};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct B64Outcome {
    pub out_path: PathBuf,
    pub in_size: u64,
    pub out_size: u64,
}

/// Read `input` and write a sibling `<name>.b64.txt` containing a
/// `data:<mime>;base64,...` string. Never overwrites.
pub fn encode_file(input: &Path) -> Result<B64Outcome> {
    let bytes = std::fs::read(input).with_context(|| format!("reading {}", input.display()))?;
    let mime = mime_for_extension(input);
    let data_url = format!("data:{mime};base64,{}", STANDARD.encode(&bytes));

    let name = input
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("encoded");
    let out_path = unique_sibling(input, &format!("{name}.b64"), "txt");
    std::fs::write(&out_path, &data_url)
        .with_context(|| format!("writing {}", out_path.display()))?;
    Ok(B64Outcome {
        out_path,
        in_size: bytes.len() as u64,
        out_size: data_url.len() as u64,
    })
}

/// Read a text file holding base64 (bare or as a data URL), decode, sniff the
/// payload type, and write a sibling `<stem>-decoded.<ext>`. Never overwrites.
pub fn decode_file(input: &Path) -> Result<B64Outcome> {
    let text =
        std::fs::read_to_string(input).with_context(|| format!("reading {}", input.display()))?;
    let in_size = text.len() as u64;
    let trimmed = text.trim();
    let payload = match trimmed.strip_prefix("data:") {
        Some(rest) => rest
            .split_once(',')
            .map(|(_, data)| data)
            .context("data URL has no comma")?,
        None => trimmed,
    };
    let cleaned: String = payload.chars().filter(|c| !c.is_whitespace()).collect();
    if cleaned.is_empty() {
        bail!("file contains no base64 data");
    }
    let bytes = STANDARD
        .decode(cleaned.as_bytes())
        .context("invalid base64 data")?;

    let ext = sniff_extension(&bytes);
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("decoded");
    let out_path = unique_sibling(input, &format!("{stem}-decoded"), ext);
    std::fs::write(&out_path, &bytes).with_context(|| format!("writing {}", out_path.display()))?;
    Ok(B64Outcome {
        out_path,
        in_size,
        out_size: bytes.len() as u64,
    })
}

fn mime_for_extension(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("bmp") => "image/bmp",
        Some("ico") => "image/x-icon",
        Some("svg") => "image/svg+xml",
        Some("tif") | Some("tiff") => "image/tiff",
        Some("pdf") => "application/pdf",
        Some("zip") => "application/zip",
        Some("json") => "application/json",
        Some("txt") => "text/plain",
        Some("html") => "text/html",
        Some("css") => "text/css",
        Some("js") => "text/javascript",
        Some("xml") => "application/xml",
        Some("mp4") => "video/mp4",
        Some("webm") => "video/webm",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        _ => "application/octet-stream",
    }
}

/// Guess an extension from magic bytes; falls back to "bin".
fn sniff_extension(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        "png"
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        "jpg"
    } else if bytes.starts_with(b"GIF8") {
        "gif"
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "webp"
    } else if bytes.starts_with(b"%PDF") {
        "pdf"
    } else if bytes.starts_with(&[b'P', b'K', 0x03, 0x04]) {
        "zip"
    } else {
        "bin"
    }
}

/// Sibling `<base>.<ext>`, appending `-2`, `-3`, ... until it doesn't exist.
fn unique_sibling(input: &Path, base: &str, ext: &str) -> PathBuf {
    let sibling = |name: String| input.with_file_name(name);
    let mut candidate = sibling(format!("{base}.{ext}"));
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

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "konvrt-b64-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn encodes_to_data_url() {
        let dir = temp_dir();
        let input = dir.join("hello.png");
        // Not a real PNG; mime comes from the extension.
        std::fs::write(&input, b"hello").unwrap();
        let outcome = encode_file(&input).unwrap();
        assert_eq!(outcome.out_path, dir.join("hello.png.b64.txt"));
        let text = std::fs::read_to_string(&outcome.out_path).unwrap();
        assert_eq!(text, "data:image/png;base64,aGVsbG8=");
        // Never overwrites: a second run picks a new name.
        let second = encode_file(&input).unwrap();
        assert_eq!(second.out_path, dir.join("hello.png.b64-2.txt"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn decodes_data_url_and_sniffs_png() {
        let dir = temp_dir();
        let png = [0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n', 1, 2, 3];
        let input = dir.join("img.b64.txt");
        std::fs::write(
            &input,
            format!("data:image/png;base64,{}", STANDARD.encode(png)),
        )
        .unwrap();
        let outcome = decode_file(&input).unwrap();
        assert_eq!(outcome.out_path, dir.join("img.b64-decoded.png"));
        assert_eq!(std::fs::read(&outcome.out_path).unwrap(), png);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn decodes_bare_base64_with_whitespace() {
        let dir = temp_dir();
        let input = dir.join("raw.txt");
        std::fs::write(&input, "JVBE\nRi0x\n").unwrap(); // "%PDF-1"
        let outcome = decode_file(&input).unwrap();
        assert_eq!(outcome.out_path, dir.join("raw-decoded.pdf"));
        assert_eq!(std::fs::read(&outcome.out_path).unwrap(), b"%PDF-1");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rejects_invalid_base64() {
        let dir = temp_dir();
        let input = dir.join("bad.txt");
        std::fs::write(&input, "not*valid*base64!").unwrap();
        assert!(decode_file(&input).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sniffs_magic_bytes() {
        assert_eq!(sniff_extension(&[0xff, 0xd8, 0xff, 0xe0]), "jpg");
        assert_eq!(sniff_extension(b"GIF89a"), "gif");
        assert_eq!(sniff_extension(b"RIFF\x00\x00\x00\x00WEBPVP8 "), "webp");
        assert_eq!(sniff_extension(b"PK\x03\x04rest"), "zip");
        assert_eq!(sniff_extension(b"plain text"), "bin");
    }
}
