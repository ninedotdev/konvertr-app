//! Icon set generators: favicon, Tauri, Electron. Ported from konvertr's
//! favicon-generator.ts / tauri-icon-generator.ts / electron-icon-generator.ts;
//! same size lists, file names, and hand-rolled ICO/ICNS containers.

use anyhow::{Context as _, Result};
use image::imageops::FilterType;
use image::{DynamicImage, ImageFormat};
use std::path::{Path, PathBuf};

use crate::write_with_format;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IconSet {
    Favicon,
    Tauri,
    Electron,
    Xcode,
    ChromeExt,
}

impl IconSet {
    pub const ALL: [IconSet; 5] = [
        IconSet::Favicon,
        IconSet::Tauri,
        IconSet::Electron,
        IconSet::Xcode,
        IconSet::ChromeExt,
    ];

    pub fn label(self) -> &'static str {
        match self {
            IconSet::Favicon => "favicon",
            IconSet::Tauri => "tauri",
            IconSet::Electron => "electron",
            IconSet::Xcode => "xcode appicon",
            IconSet::ChromeExt => "chrome extension",
        }
    }

    /// Filesystem-safe folder name under the chosen output dir.
    fn dir_name(self) -> &'static str {
        match self {
            IconSet::Xcode => "xcode-appicon",
            IconSet::ChromeExt => "chrome-extension",
            other => other.label(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct GeneratedSet {
    pub files: Vec<PathBuf>,
}

// Favicon set (favicon-generator.ts).
const FAVICON_SIZES: [(u32, &str); 8] = [
    (16, "favicon"),
    (32, "favicon"),
    (48, "favicon"),
    (64, "favicon"),
    (128, "favicon"),
    (180, "apple-touch-icon"),
    (192, "android-chrome"),
    (512, "android-chrome"),
];
const FAVICON_ICO_SIZES: [u32; 3] = [16, 32, 48];

// Tauri set (tauri-icon-generator.ts): everything `tauri icon` generates.
const TAURI_ICONS: [(&str, u32); 40] = [
    // Desktop PNGs
    ("32x32.png", 32),
    ("128x128.png", 128),
    ("128x128@2x.png", 256),
    ("icon.png", 512),
    // Windows Store / AppX
    ("Square30x30Logo.png", 30),
    ("Square44x44Logo.png", 44),
    ("Square71x71Logo.png", 71),
    ("Square89x89Logo.png", 89),
    ("Square107x107Logo.png", 107),
    ("Square142x142Logo.png", 142),
    ("Square150x150Logo.png", 150),
    ("Square284x284Logo.png", 284),
    ("Square310x310Logo.png", 310),
    ("StoreLogo.png", 50),
    // Android
    ("mipmap-mdpi/ic_launcher.png", 48),
    ("mipmap-mdpi/ic_launcher_round.png", 48),
    ("mipmap-hdpi/ic_launcher.png", 72),
    ("mipmap-hdpi/ic_launcher_round.png", 72),
    ("mipmap-xhdpi/ic_launcher.png", 96),
    ("mipmap-xhdpi/ic_launcher_round.png", 96),
    ("mipmap-xxhdpi/ic_launcher.png", 144),
    ("mipmap-xxhdpi/ic_launcher_round.png", 144),
    ("mipmap-xxxhdpi/ic_launcher.png", 192),
    ("mipmap-xxxhdpi/ic_launcher_round.png", 192),
    ("playstore.png", 512),
    // iOS
    ("AppIcon-20x20@1x.png", 20),
    ("AppIcon-20x20@2x.png", 40),
    ("AppIcon-20x20@3x.png", 60),
    ("AppIcon-29x29@1x.png", 29),
    ("AppIcon-29x29@2x.png", 58),
    ("AppIcon-29x29@3x.png", 87),
    ("AppIcon-40x40@1x.png", 40),
    ("AppIcon-40x40@2x.png", 80),
    ("AppIcon-40x40@3x.png", 120),
    ("AppIcon-60x60@2x.png", 120),
    ("AppIcon-60x60@3x.png", 180),
    ("AppIcon-76x76@1x.png", 76),
    ("AppIcon-76x76@2x.png", 152),
    ("AppIcon-83.5x83.5@2x.png", 167),
    ("AppIcon-512@2x.png", 1024),
];
// appstore.png duplicates AppIcon-512@2x at 1024; listed separately because the
// const table above can't repeat names.
const TAURI_EXTRA: [(&str, u32); 1] = [("appstore.png", 1024)];

// Electron set (electron-icon-generator.ts).
const ELECTRON_ICONS: [(&str, u32); 14] = [
    // Linux PNGs
    ("16x16.png", 16),
    ("24x24.png", 24),
    ("32x32.png", 32),
    ("48x48.png", 48),
    ("64x64.png", 64),
    ("128x128.png", 128),
    ("256x256.png", 256),
    ("512x512.png", 512),
    ("1024x1024.png", 1024),
    // Tray icons
    ("tray-icon-16.png", 16),
    ("tray-icon-24.png", 24),
    ("tray-icon-32.png", 32),
    ("tray-icon@2x.png", 32),
    // Windows notification area
    ("icon.png", 256),
];

// Multi-layer ICO + ICNS chunk lists shared by tauri and electron.
const APP_ICO_SIZES: [u32; 6] = [16, 24, 32, 48, 64, 256];
const ICNS_ENTRIES: [(&[u8; 4], u32); 6] = [
    (b"is32", 16),
    (b"il32", 32),
    (b"ic07", 128),
    (b"ic08", 256),
    (b"ic09", 512),
    (b"ic10", 1024),
];

/// Decode `source`, generate every artifact of `set` into `out_dir/<set-name>/`
/// (suffixed `-2`, `-3`, ... if that folder already exists — never overwrites).
pub fn generate(source: &Path, set: IconSet, out_dir: &Path) -> Result<GeneratedSet> {
    let img = crate::decode_image(source)?;

    let dir = unique_set_dir(out_dir, set.dir_name());
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;

    let mut files = Vec::new();
    match set {
        IconSet::Favicon => generate_favicon(&img, &dir, &mut files)?,
        IconSet::Tauri => generate_app_set(&img, &dir, &TAURI_ICONS, &TAURI_EXTRA, &mut files)?,
        IconSet::Electron => generate_app_set(&img, &dir, &ELECTRON_ICONS, &[], &mut files)?,
        IconSet::Xcode => generate_xcode(&img, &dir, &mut files)?,
        IconSet::ChromeExt => generate_chrome_ext(&img, &dir, &mut files)?,
    }
    Ok(GeneratedSet { files })
}

/// (idiom, size, scale, platform, filename, pixels); "" means the field is
/// omitted. Filenames repeat across idioms when the pixels match.
const XCODE_ENTRIES: [(&str, &str, &str, &str, &str, u32); 35] = [
    // iPhone
    ("iphone", "20x20", "2x", "", "icon-20@2x.png", 40),
    ("iphone", "20x20", "3x", "", "icon-20@3x.png", 60),
    ("iphone", "29x29", "2x", "", "icon-29@2x.png", 58),
    ("iphone", "29x29", "3x", "", "icon-29@3x.png", 87),
    ("iphone", "38x38", "2x", "", "icon-38@2x.png", 76),
    ("iphone", "38x38", "3x", "", "icon-38@3x.png", 114),
    ("iphone", "40x40", "2x", "", "icon-40@2x.png", 80),
    ("iphone", "40x40", "3x", "", "icon-40@3x.png", 120),
    ("iphone", "60x60", "2x", "", "icon-60@2x.png", 120),
    ("iphone", "60x60", "3x", "", "icon-60@3x.png", 180),
    ("iphone", "64x64", "2x", "", "icon-64@2x.png", 128),
    ("iphone", "64x64", "3x", "", "icon-64@3x.png", 192),
    // iPad
    ("ipad", "20x20", "1x", "", "icon-20@1x.png", 20),
    ("ipad", "20x20", "2x", "", "icon-20@2x.png", 40),
    ("ipad", "29x29", "1x", "", "icon-29@1x.png", 29),
    ("ipad", "29x29", "2x", "", "icon-29@2x.png", 58),
    ("ipad", "40x40", "1x", "", "icon-40@1x.png", 40),
    ("ipad", "40x40", "2x", "", "icon-40@2x.png", 80),
    ("ipad", "68x68", "2x", "", "icon-68@2x.png", 136),
    ("ipad", "76x76", "1x", "", "icon-76@1x.png", 76),
    ("ipad", "76x76", "2x", "", "icon-76@2x.png", 152),
    ("ipad", "83.5x83.5", "2x", "", "icon-83.5@2x.png", 167),
    // App Store marketing + the modern single-size universal entry
    (
        "ios-marketing",
        "1024x1024",
        "1x",
        "",
        "icon-1024.png",
        1024,
    ),
    ("universal", "1024x1024", "", "ios", "icon-1024.png", 1024),
    // macOS
    ("mac", "16x16", "1x", "", "icon-mac-16@1x.png", 16),
    ("mac", "16x16", "2x", "", "icon-mac-16@2x.png", 32),
    ("mac", "32x32", "1x", "", "icon-mac-32@1x.png", 32),
    ("mac", "32x32", "2x", "", "icon-mac-32@2x.png", 64),
    ("mac", "128x128", "1x", "", "icon-mac-128@1x.png", 128),
    ("mac", "128x128", "2x", "", "icon-mac-128@2x.png", 256),
    ("mac", "256x256", "1x", "", "icon-mac-256@1x.png", 256),
    ("mac", "256x256", "2x", "", "icon-mac-256@2x.png", 512),
    ("mac", "512x512", "1x", "", "icon-mac-512@1x.png", 512),
    ("mac", "512x512", "2x", "", "icon-mac-512@2x.png", 1024),
    // watchOS companion marketing icon
    (
        "watch-marketing",
        "1024x1024",
        "1x",
        "",
        "icon-watch-1024.png",
        1024,
    ),
];

/// Swift/Xcode asset-catalog icon set, ready to drag into Assets.xcassets.
fn generate_xcode(img: &DynamicImage, dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let set_dir = "AppIcon.appiconset";

    let mut images = Vec::new();
    for (idiom, size, scale, platform, filename, _) in XCODE_ENTRIES {
        let mut entry = serde_json::json!({
            "filename": filename,
            "idiom": idiom,
            "size": size,
        });
        if !scale.is_empty() {
            entry["scale"] = serde_json::Value::String(scale.to_string());
        }
        if !platform.is_empty() {
            entry["platform"] = serde_json::Value::String(platform.to_string());
        }
        images.push(entry);
    }
    let contents = serde_json::json!({
        "images": images,
        "info": { "author": "xcode", "version": 1 },
    });
    let contents = serde_json::to_string_pretty(&contents).context("serializing Contents.json")?;
    write_file(
        dir,
        &format!("{set_dir}/Contents.json"),
        contents.as_bytes(),
        files,
    )?;

    // Write each referenced PNG once, even when several entries share it.
    let mut written: Vec<&str> = Vec::new();
    for (_, _, _, _, filename, px) in XCODE_ENTRIES {
        if written.contains(&filename) {
            continue;
        }
        written.push(filename);
        write_file(
            dir,
            &format!("{set_dir}/{filename}"),
            &resize_png(img, px)?,
            files,
        )?;
    }
    Ok(())
}

const CHROME_EXT_SIZES: [u32; 4] = [16, 32, 48, 128];

/// Browser-extension icons + the manifest v3 "icons" block to paste.
fn generate_chrome_ext(img: &DynamicImage, dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let mut icons = serde_json::Map::new();
    for size in CHROME_EXT_SIZES {
        let name = format!("icon{size}.png");
        write_file(dir, &name, &resize_png(img, size)?, files)?;
        icons.insert(size.to_string(), serde_json::Value::String(name));
    }
    let snippet = serde_json::json!({ "icons": icons });
    let snippet = serde_json::to_string_pretty(&snippet).context("serializing snippet.json")?;
    write_file(dir, "snippet.json", snippet.as_bytes(), files)?;
    Ok(())
}

fn generate_favicon(img: &DynamicImage, dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    write_file(
        dir,
        "favicon.ico",
        &build_ico(img, &FAVICON_ICO_SIZES)?,
        files,
    )?;
    for (size, purpose) in FAVICON_SIZES {
        let name = match purpose {
            "apple-touch-icon" => format!("apple-touch-icon-{size}x{size}.png"),
            "android-chrome" => format!("android-chrome-{size}x{size}.png"),
            _ => format!("favicon-{size}x{size}.png"),
        };
        write_file(dir, &name, &resize_png(img, size)?, files)?;
    }
    write_file(dir, "site.webmanifest", web_manifest()?.as_bytes(), files)?;
    write_file(dir, "snippet.html", html_snippet().as_bytes(), files)?;
    Ok(())
}

fn generate_app_set(
    img: &DynamicImage,
    dir: &Path,
    icons: &[(&str, u32)],
    extra: &[(&str, u32)],
    files: &mut Vec<PathBuf>,
) -> Result<()> {
    write_file(dir, "icon.ico", &build_ico(img, &APP_ICO_SIZES)?, files)?;
    write_file(dir, "icon.icns", &build_icns(img)?, files)?;
    for (name, size) in icons.iter().chain(extra) {
        write_file(dir, name, &resize_png(img, *size)?, files)?;
    }
    Ok(())
}

fn write_file(dir: &Path, rel: &str, bytes: &[u8], files: &mut Vec<PathBuf>) -> Result<()> {
    let path = dir.join(rel);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(&path, bytes).with_context(|| format!("writing {}", path.display()))?;
    files.push(path);
    Ok(())
}

fn resize_png(img: &DynamicImage, size: u32) -> Result<Vec<u8>> {
    let resized = img.resize_exact(size, size, FilterType::Lanczos3);
    write_with_format(
        &DynamicImage::ImageRgba8(resized.to_rgba8()),
        ImageFormat::Png,
    )
}

/// Multi-layer ICO: 6-byte header + 16-byte dir entry per image + PNG payloads.
fn build_ico(img: &DynamicImage, sizes: &[u32]) -> Result<Vec<u8>> {
    let pngs: Vec<Vec<u8>> = sizes
        .iter()
        .map(|&s| resize_png(img, s))
        .collect::<Result<_>>()?;

    let header_size = 6usize;
    let dir_size = 16 * pngs.len();
    let mut out =
        Vec::with_capacity(header_size + dir_size + pngs.iter().map(Vec::len).sum::<usize>());

    out.extend_from_slice(&0u16.to_le_bytes()); // reserved
    out.extend_from_slice(&1u16.to_le_bytes()); // type: 1 = icon
    out.extend_from_slice(&(pngs.len() as u16).to_le_bytes());

    let mut data_offset = (header_size + dir_size) as u32;
    for (size, png) in sizes.iter().zip(&pngs) {
        let dim = if *size < 256 { *size as u8 } else { 0 };
        out.push(dim); // width (0 = 256)
        out.push(dim); // height
        out.push(0); // color palette count
        out.push(0); // reserved
        out.extend_from_slice(&1u16.to_le_bytes()); // color planes
        out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        out.extend_from_slice(&(png.len() as u32).to_le_bytes());
        out.extend_from_slice(&data_offset.to_le_bytes());
        data_offset += png.len() as u32;
    }
    for png in &pngs {
        out.extend_from_slice(png);
    }
    Ok(out)
}

/// ICNS: "icns" magic + total length + big-endian (type, length, PNG) chunks,
/// with the exact type codes the web generators use.
fn build_icns(img: &DynamicImage) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    for (tag, size) in ICNS_ENTRIES {
        let png = resize_png(img, size)?;
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

fn web_manifest() -> Result<String> {
    let manifest = serde_json::json!({
        "name": "",
        "short_name": "",
        "icons": [
            { "src": "/android-chrome-192x192.png", "sizes": "192x192", "type": "image/png" },
            { "src": "/android-chrome-512x512.png", "sizes": "512x512", "type": "image/png" },
        ],
        "theme_color": "#ffffff",
        "background_color": "#ffffff",
        "display": "standalone",
    });
    serde_json::to_string_pretty(&manifest).context("serializing web manifest")
}

fn html_snippet() -> String {
    let mut lines = vec![r#"<link rel="icon" href="/favicon.ico" sizes="48x48">"#.to_string()];
    for (size, purpose) in FAVICON_SIZES {
        if purpose == "favicon" {
            lines.push(format!(
                r#"<link rel="icon" type="image/png" sizes="{size}x{size}" href="/favicon-{size}x{size}.png">"#
            ));
        }
    }
    lines.push(
        r#"<link rel="apple-touch-icon" sizes="180x180" href="/apple-touch-icon-180x180.png">"#
            .to_string(),
    );
    lines.push(r#"<link rel="manifest" href="/site.webmanifest">"#.to_string());
    lines.join("\n")
}

/// `out_dir/<name>`, appending `-2`, `-3`, ... until the folder doesn't exist.
fn unique_set_dir(out_dir: &Path, name: &str) -> PathBuf {
    let mut candidate = out_dir.join(name);
    let mut n = 2;
    while candidate.exists() {
        candidate = out_dir.join(format!("{name}-{n}"));
        n += 1;
    }
    candidate
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageFormat;

    fn setup() -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "konvrt-icons-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let mut img = image::RgbaImage::new(64, 64);
        for (x, y, px) in img.enumerate_pixels_mut() {
            *px = image::Rgba([(x * 4) as u8, (y * 4) as u8, 128, 255]);
        }
        let source = dir.join("source.png");
        img.save_with_format(&source, ImageFormat::Png).unwrap();
        (dir, source)
    }

    fn ico_layer_count(path: &Path) -> u16 {
        let bytes = std::fs::read(path).unwrap();
        assert_eq!(&bytes[0..4], &[0, 0, 1, 0], "ico header");
        u16::from_le_bytes([bytes[4], bytes[5]])
    }

    #[test]
    fn favicon_set_writes_expected_files() {
        let (dir, source) = setup();
        let result = generate(&source, IconSet::Favicon, &dir).unwrap();
        let base = dir.join("favicon");
        for name in [
            "favicon.ico",
            "favicon-16x16.png",
            "favicon-32x32.png",
            "favicon-48x48.png",
            "favicon-64x64.png",
            "favicon-128x128.png",
            "apple-touch-icon-180x180.png",
            "android-chrome-192x192.png",
            "android-chrome-512x512.png",
            "site.webmanifest",
            "snippet.html",
        ] {
            assert!(base.join(name).exists(), "missing {name}");
        }
        assert_eq!(result.files.len(), 11);
        assert_eq!(ico_layer_count(&base.join("favicon.ico")), 3);
        let manifest = std::fs::read_to_string(base.join("site.webmanifest")).unwrap();
        assert!(manifest.contains("android-chrome-512x512.png"));
        let snippet = std::fs::read_to_string(base.join("snippet.html")).unwrap();
        assert!(snippet.contains(r#"rel="apple-touch-icon""#));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn tauri_set_writes_subfolders_and_multilayer_ico() {
        let (dir, source) = setup();
        let result = generate(&source, IconSet::Tauri, &dir).unwrap();
        let base = dir.join("tauri");
        for name in [
            "icon.ico",
            "icon.icns",
            "32x32.png",
            "128x128@2x.png",
            "Square310x310Logo.png",
            "mipmap-xxxhdpi/ic_launcher.png",
            "AppIcon-83.5x83.5@2x.png",
            "appstore.png",
        ] {
            assert!(base.join(name).exists(), "missing {name}");
        }
        // ico + icns + 40 listed + appstore.png
        assert_eq!(result.files.len(), 43);
        assert_eq!(ico_layer_count(&base.join("icon.ico")), 6);
        let icns = std::fs::read(base.join("icon.icns")).unwrap();
        assert_eq!(&icns[0..4], b"icns");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn xcode_set_writes_valid_appiconset() {
        let (dir, source) = setup();
        let result = generate(&source, IconSet::Xcode, &dir).unwrap();
        let base = dir.join("xcode-appicon").join("AppIcon.appiconset");

        let contents: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(base.join("Contents.json")).unwrap())
                .unwrap();
        assert_eq!(contents["info"]["author"], "xcode");
        assert_eq!(contents["info"]["version"], 1);

        let images = contents["images"].as_array().unwrap();
        assert_eq!(images.len(), 35);
        // Every referenced filename exists on disk.
        for image in images {
            let filename = image["filename"].as_str().unwrap();
            assert!(base.join(filename).exists(), "missing {filename}");
            assert!(image["idiom"].is_string());
            assert!(image["size"].is_string());
        }
        // The modern single-size universal entry.
        assert!(images.iter().any(|i| i["idiom"] == "universal"
            && i["platform"] == "ios"
            && i["size"] == "1024x1024"
            && i["filename"] == "icon-1024.png"));
        // Classic per-scale fields come through correctly.
        assert!(images.iter().any(|i| i["idiom"] == "iphone"
            && i["size"] == "60x60"
            && i["scale"] == "3x"
            && i["filename"] == "icon-60@3x.png"));
        assert!(images.iter().any(|i| i["idiom"] == "ipad"
            && i["size"] == "83.5x83.5"
            && i["scale"] == "2x"
            && i["filename"] == "icon-83.5@2x.png"));

        // Pixel dimensions are real: 60pt@3x = 180px, 83.5pt@2x = 167px.
        let px180 = image::open(base.join("icon-60@3x.png")).unwrap();
        assert_eq!((px180.width(), px180.height()), (180, 180));
        let px167 = image::open(base.join("icon-83.5@2x.png")).unwrap();
        assert_eq!((px167.width(), px167.height()), (167, 167));

        // Contents.json + 31 unique PNGs (shared filenames written once).
        assert_eq!(result.files.len(), 32);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn chrome_ext_set_writes_icons_and_snippet() {
        let (dir, source) = setup();
        let result = generate(&source, IconSet::ChromeExt, &dir).unwrap();
        let base = dir.join("chrome-extension");
        for name in ["icon16.png", "icon32.png", "icon48.png", "icon128.png"] {
            assert!(base.join(name).exists(), "missing {name}");
        }
        let snippet: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(base.join("snippet.json")).unwrap())
                .unwrap();
        assert_eq!(snippet["icons"]["16"], "icon16.png");
        assert_eq!(snippet["icons"]["128"], "icon128.png");
        assert_eq!(result.files.len(), 5);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn electron_set_writes_expected_files_and_avoids_collision() {
        let (dir, source) = setup();
        let result = generate(&source, IconSet::Electron, &dir).unwrap();
        let base = dir.join("electron");
        for name in ["icon.ico", "icon.icns", "1024x1024.png", "tray-icon@2x.png"] {
            assert!(base.join(name).exists(), "missing {name}");
        }
        assert_eq!(result.files.len(), 16);
        // Second run must land in electron-2, not overwrite.
        let second = generate(&source, IconSet::Electron, &dir).unwrap();
        assert!(second.files[0].starts_with(dir.join("electron-2")));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
