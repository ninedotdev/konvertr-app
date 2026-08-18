//! PDF suite: merge / split / extract / rotate with lopdf, images→PDF with
//! printpdf. Pure Rust, no external binaries.

use anyhow::{Context as _, Result, bail};
use lopdf::{Document, Object, ObjectId};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct PdfOutcome {
    pub out_path: PathBuf,
    pub in_size: u64,
    pub out_size: u64,
    pub pages: usize,
}

#[derive(Clone, Debug)]
pub enum SplitMode {
    EveryPage,
    /// 1-based inclusive page ranges.
    Ranges(Vec<(usize, usize)>),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PageSize {
    A4,
    Letter,
    /// Page sized to each image (at 96 dpi) plus margins.
    FitImage,
}

#[derive(Clone, Copy, Debug)]
pub struct ImagePdfOptions {
    pub page: PageSize,
    pub margin_mm: f32,
    /// Rotate the page to landscape when the image is wider than tall.
    pub landscape_auto: bool,
}

impl Default for ImagePdfOptions {
    fn default() -> Self {
        Self {
            page: PageSize::A4,
            margin_mm: 10.0,
            landscape_auto: true,
        }
    }
}

/// Number of pages in a PDF.
pub fn page_count(path: &Path) -> Result<usize> {
    let doc = Document::load(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(doc.get_pages().len())
}

/// `out` if free, else `-2`, `-3`, ... before the extension. Never overwrites.
fn unique_target(out: &Path) -> PathBuf {
    if !out.exists() {
        return out.to_path_buf();
    }
    let stem = out.file_stem().and_then(|s| s.to_str()).unwrap_or("output");
    let ext = out.extension().and_then(|e| e.to_str()).unwrap_or("pdf");
    let mut n = 2;
    loop {
        let candidate = out.with_file_name(format!("{stem}-{n}.{ext}"));
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
    }
}

fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path).map(|m| m.len()).unwrap_or(0)
}

/// Concatenate `inputs` in order into one PDF (lopdf's standard merge
/// pattern: renumber ids per document, rebuild the Pages tree + Catalog).
pub fn merge(inputs: &[PathBuf], out: &Path) -> Result<PdfOutcome> {
    if inputs.len() < 2 {
        bail!("merging needs at least two PDFs");
    }
    let in_size: u64 = inputs.iter().map(|p| file_size(p)).sum();

    let mut max_id = 1;
    // Vec, not a map: Kids order is the merge order, and a page's object id
    // isn't guaranteed to ascend with its page number.
    let mut all_pages: Vec<(ObjectId, Object)> = Vec::new();
    let mut all_objects: BTreeMap<ObjectId, Object> = BTreeMap::new();
    let mut document = Document::with_version("1.5");

    for path in inputs {
        let mut doc =
            Document::load(path).with_context(|| format!("reading {}", path.display()))?;
        doc.renumber_objects_with(max_id);
        max_id = doc.max_id + 1;
        for (_, object_id) in doc.get_pages() {
            let object = doc
                .get_object(object_id)
                .with_context(|| format!("broken page tree in {}", path.display()))?
                .to_owned();
            all_pages.push((object_id, object));
        }
        all_objects.extend(doc.objects);
    }

    let mut catalog: Option<(ObjectId, Object)> = None;
    let mut pages_root: Option<(ObjectId, Object)> = None;
    for (object_id, object) in all_objects {
        match object.type_name().unwrap_or("") {
            "Catalog" => {
                let id = catalog.as_ref().map(|(id, _)| *id).unwrap_or(object_id);
                catalog = Some((id, object));
            }
            "Pages" => {
                if let Ok(dict) = object.as_dict() {
                    let mut dict = dict.clone();
                    if let Some((_, old)) = &pages_root
                        && let Ok(old_dict) = old.as_dict()
                    {
                        dict.extend(old_dict);
                    }
                    let id = pages_root.as_ref().map(|(id, _)| *id).unwrap_or(object_id);
                    pages_root = Some((id, Object::Dictionary(dict)));
                }
            }
            // Pages are re-inserted below; outlines aren't merged.
            "Page" | "Outlines" | "Outline" => {}
            _ => {
                document.objects.insert(object_id, object);
            }
        }
    }

    let (pages_id, pages_object) = pages_root.context("no Pages root found in the inputs")?;
    let (catalog_id, catalog_object) = catalog.context("no Catalog found in the inputs")?;

    for (object_id, object) in &all_pages {
        if let Ok(dict) = object.as_dict() {
            let mut dict = dict.clone();
            dict.set("Parent", pages_id);
            document
                .objects
                .insert(*object_id, Object::Dictionary(dict));
        }
    }

    let mut pages_dict = pages_object.as_dict().cloned().unwrap_or_default();
    pages_dict.set("Count", all_pages.len() as u32);
    pages_dict.set(
        "Kids",
        all_pages
            .iter()
            .map(|(id, _)| Object::Reference(*id))
            .collect::<Vec<_>>(),
    );
    document
        .objects
        .insert(pages_id, Object::Dictionary(pages_dict));

    let mut catalog_dict = catalog_object.as_dict().cloned().unwrap_or_default();
    catalog_dict.set("Pages", pages_id);
    catalog_dict.remove(b"Outlines");
    document
        .objects
        .insert(catalog_id, Object::Dictionary(catalog_dict));

    document.trailer.set("Root", catalog_id);
    document.max_id = document.objects.len() as u32;
    document.renumber_objects();

    let pages = all_pages.len();
    let out_path = unique_target(out);
    document
        .save(&out_path)
        .with_context(|| format!("writing {}", out_path.display()))?;
    Ok(PdfOutcome {
        out_size: file_size(&out_path),
        out_path,
        in_size,
        pages,
    })
}

/// Load `input` keeping only the 1-based pages in `keep` (order preserved).
fn load_with_pages(input: &Path, keep: &[usize]) -> Result<(Document, usize)> {
    let mut doc = Document::load(input).with_context(|| format!("reading {}", input.display()))?;
    let total = doc.get_pages().len();
    let delete: Vec<u32> = (1..=total)
        .filter(|p| !keep.contains(p))
        .map(|p| p as u32)
        .collect();
    if delete.len() == total {
        bail!("no pages selected");
    }
    doc.delete_pages(&delete);
    doc.prune_objects();
    doc.renumber_objects();
    Ok((doc, total - delete.len()))
}

/// Split into `<stem>-p1.pdf` / `<stem>-p3-7.pdf` files under `out_dir`.
pub fn split(input: &Path, mode: SplitMode, out_dir: &Path) -> Result<Vec<PathBuf>> {
    let total = page_count(input)?;
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("split");
    let ranges: Vec<(usize, usize)> = match mode {
        SplitMode::EveryPage => (1..=total).map(|p| (p, p)).collect(),
        SplitMode::Ranges(ranges) => ranges,
    };
    if ranges.is_empty() {
        bail!("no pages selected");
    }

    let mut outputs = Vec::new();
    for (start, end) in ranges {
        if start < 1 || end > total || start > end {
            bail!("range {start}-{end} is outside 1-{total}");
        }
        let keep: Vec<usize> = (start..=end).collect();
        let (mut doc, _) = load_with_pages(input, &keep)?;
        let name = if start == end {
            format!("{stem}-p{start}.pdf")
        } else {
            format!("{stem}-p{start}-{end}.pdf")
        };
        let out_path = unique_target(&out_dir.join(name));
        doc.save(&out_path)
            .with_context(|| format!("writing {}", out_path.display()))?;
        outputs.push(out_path);
    }
    Ok(outputs)
}

/// One output containing only the pages in `ranges` (1-based inclusive).
pub fn extract_pages(input: &Path, ranges: &[(usize, usize)], out: &Path) -> Result<PdfOutcome> {
    let mut keep: Vec<usize> = Vec::new();
    for &(start, end) in ranges {
        for p in start..=end {
            if !keep.contains(&p) {
                keep.push(p);
            }
        }
    }
    if keep.is_empty() {
        bail!("no pages selected");
    }
    let (mut doc, pages) = load_with_pages(input, &keep)?;
    let out_path = unique_target(out);
    doc.save(&out_path)
        .with_context(|| format!("writing {}", out_path.display()))?;
    Ok(PdfOutcome {
        out_size: file_size(&out_path),
        out_path,
        in_size: file_size(input),
        pages,
    })
}

/// Add `degrees` (90/180/270) to /Rotate on the given 1-based pages (all
/// pages when None), normalized modulo 360.
pub fn rotate(
    input: &Path,
    degrees: i64,
    pages: Option<&[usize]>,
    out: &Path,
) -> Result<PdfOutcome> {
    if degrees % 90 != 0 {
        bail!("rotation must be a multiple of 90");
    }
    let mut doc = Document::load(input).with_context(|| format!("reading {}", input.display()))?;
    let page_map = doc.get_pages();
    let total = page_map.len();
    for (page_no, page_id) in page_map {
        if let Some(pages) = pages
            && !pages.contains(&(page_no as usize))
        {
            continue;
        }
        let dict = doc
            .get_object_mut(page_id)
            .and_then(|obj| obj.as_dict_mut())
            .context("broken page object")?;
        let current = dict.get(b"Rotate").and_then(|o| o.as_i64()).unwrap_or(0);
        dict.set("Rotate", (current + degrees).rem_euclid(360));
    }
    let out_path = unique_target(out);
    doc.save(&out_path)
        .with_context(|| format!("writing {}", out_path.display()))?;
    Ok(PdfOutcome {
        out_size: file_size(&out_path),
        out_path,
        in_size: file_size(input),
        pages: total,
    })
}

/// One image per page, aspect-preserving fit inside the margins. Handles
/// png/jpeg/webp (anything the `image` crate decodes); alpha is flattened
/// over white.
pub fn images_to_pdf(images: &[PathBuf], opts: ImagePdfOptions, out: &Path) -> Result<PdfOutcome> {
    use printpdf::{
        ColorBits, ColorSpace, Image, ImageTransform, ImageXObject, Mm, PdfDocument, Px,
    };

    if images.is_empty() {
        bail!("no images given");
    }
    let in_size: u64 = images.iter().map(|p| file_size(p)).sum();
    let margin = opts.margin_mm.max(0.0);

    // Decode everything first so a bad file fails before we build the doc.
    let mut decoded = Vec::new();
    for path in images {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let img = image::load_from_memory(&bytes)
            .with_context(|| format!("could not decode {}", path.display()))?;
        decoded.push(flatten_white_rgb(&img));
    }

    let page_dims = |w_px: u32, h_px: u32| -> (f32, f32) {
        let (mut pw, mut ph) = match opts.page {
            PageSize::A4 => (210.0, 297.0),
            PageSize::Letter => (215.9, 279.4),
            PageSize::FitImage => {
                // Image at 96 dpi plus the margins.
                let to_mm = |px: u32| px as f32 * 25.4 / 96.0;
                return (to_mm(w_px) + 2.0 * margin, to_mm(h_px) + 2.0 * margin);
            }
        };
        if opts.landscape_auto && w_px > h_px {
            std::mem::swap(&mut pw, &mut ph);
        }
        (pw, ph)
    };

    let first = &decoded[0];
    let (pw, ph) = page_dims(first.width(), first.height());
    let (doc, mut page_idx, mut layer_idx) = PdfDocument::new("konvrt", Mm(pw), Mm(ph), "Layer 1");

    for (ix, rgb) in decoded.iter().enumerate() {
        let (pw, ph) = page_dims(rgb.width(), rgb.height());
        if ix > 0 {
            let (p, l) = doc.add_page(Mm(pw), Mm(ph), "Layer 1");
            page_idx = p;
            layer_idx = l;
        }
        let layer = doc.get_page(page_idx).get_layer(layer_idx);

        let (w_px, h_px) = (rgb.width(), rgb.height());
        // Aspect-preserving fit into the content box.
        let (cw, ch) = (pw - 2.0 * margin, ph - 2.0 * margin);
        if cw <= 0.0 || ch <= 0.0 {
            bail!("margins larger than the page");
        }
        let aspect = w_px as f32 / h_px as f32;
        let mut target_w = cw;
        let mut target_h = target_w / aspect;
        if target_h > ch {
            target_h = ch;
            target_w = target_h * aspect;
        }
        // printpdf sizes images as px / dpi; uniform scale keeps the aspect.
        const DPI: f32 = 300.0;
        let natural_w = w_px as f32 * 25.4 / DPI;
        let scale = target_w / natural_w;

        let xobject = ImageXObject {
            width: Px(w_px as usize),
            height: Px(h_px as usize),
            color_space: ColorSpace::Rgb,
            bits_per_component: ColorBits::Bit8,
            interpolate: true,
            image_data: rgb.as_raw().clone(),
            image_filter: None,
            smask: None,
            clipping_bbox: None,
        };
        Image::from(xobject).add_to_layer(
            layer,
            ImageTransform {
                translate_x: Some(Mm((pw - target_w) / 2.0)),
                translate_y: Some(Mm((ph - target_h) / 2.0)),
                rotate: None,
                scale_x: Some(scale),
                scale_y: Some(scale),
                dpi: Some(DPI),
            },
        );
    }

    let bytes = doc
        .save_to_bytes()
        .map_err(|e| anyhow::anyhow!("building pdf: {e}"))?;
    let out_path = unique_target(out);
    std::fs::write(&out_path, &bytes).with_context(|| format!("writing {}", out_path.display()))?;
    Ok(PdfOutcome {
        out_path,
        in_size,
        out_size: bytes.len() as u64,
        pages: images.len(),
    })
}

/// Composite over white and drop alpha (PDF page images carry no
/// transparency here).
fn flatten_white_rgb(img: &image::DynamicImage) -> image::RgbImage {
    let rgba = img.to_rgba8();
    let (w, h) = rgba.dimensions();
    let mut out = image::RgbImage::new(w, h);
    for (x, y, px) in rgba.enumerate_pixels() {
        let a = px[3] as u32;
        let blend = |c: u8| ((c as u32 * a + 255 * (255 - a)) / 255) as u8;
        out.put_pixel(x, y, image::Rgb([blend(px[0]), blend(px[1]), blend(px[2])]));
    }
    out
}

/// Parse a print-dialog range string like "1-3,5,8-" against `page_count`.
/// "8-" runs to the last page, "-3" from the first; ends past the last page
/// are clamped. Reversed ranges, zero, and pages past the end are errors.
pub fn parse_ranges(input: &str, page_count: usize) -> Result<Vec<(usize, usize)>> {
    if page_count == 0 {
        bail!("document has no pages");
    }
    let mut ranges = Vec::new();
    for part in input.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (start, end) = match part.split_once('-') {
            None => {
                let p: usize = part.parse().with_context(|| format!("bad page '{part}'"))?;
                (p, p)
            }
            Some((a, b)) => {
                let a = a.trim();
                let b = b.trim();
                let start = if a.is_empty() {
                    1
                } else {
                    a.parse().with_context(|| format!("bad range '{part}'"))?
                };
                let end = if b.is_empty() {
                    page_count
                } else {
                    b.parse().with_context(|| format!("bad range '{part}'"))?
                };
                (start, end)
            }
        };
        if start == 0 || end == 0 {
            bail!("pages are numbered from 1");
        }
        if start > page_count {
            bail!("page {start} is past the last page ({page_count})");
        }
        let end = end.min(page_count);
        if start > end {
            bail!("range '{part}' is reversed");
        }
        ranges.push((start, end));
    }
    if ranges.is_empty() {
        bail!("no pages selected");
    }
    Ok(ranges)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lopdf::Stream;
    use lopdf::content::{Content, Operation};
    use lopdf::dictionary;

    /// Minimal valid n-page PDF built with lopdf.
    fn fixture(pages: usize) -> Document {
        let mut doc = Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let font_id = doc.add_object(dictionary! {
            "Type" => "Font", "Subtype" => "Type1", "BaseFont" => "Courier",
        });
        let resources_id = doc.add_object(dictionary! {
            "Font" => dictionary! { "F1" => font_id },
        });
        let mut kids = Vec::new();
        for i in 0..pages {
            let content = Content {
                operations: vec![
                    Operation::new("BT", vec![]),
                    Operation::new("Tf", vec!["F1".into(), 48.into()]),
                    Operation::new("Td", vec![100.into(), 600.into()]),
                    Operation::new(
                        "Tj",
                        vec![Object::string_literal(format!("Page {}", i + 1))],
                    ),
                    Operation::new("ET", vec![]),
                ],
            };
            let content_id = doc.add_object(Stream::new(dictionary! {}, content.encode().unwrap()));
            let page_id = doc.add_object(dictionary! {
                "Type" => "Page",
                "Parent" => pages_id,
                "Contents" => content_id,
                "Resources" => resources_id,
                "MediaBox" => vec![0.into(), 0.into(), 595.into(), 842.into()],
            });
            kids.push(page_id.into());
        }
        doc.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages", "Kids" => kids, "Count" => pages as u32,
            }),
        );
        let catalog_id = doc.add_object(dictionary! { "Type" => "Catalog", "Pages" => pages_id });
        doc.trailer.set("Root", catalog_id);
        doc
    }

    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "konvrt-pdf-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn save_fixture(dir: &Path, name: &str, pages: usize) -> PathBuf {
        let path = dir.join(name);
        fixture(pages).save(&path).unwrap();
        path
    }

    #[test]
    fn merge_sums_page_counts_and_reloads() {
        let dir = temp_dir();
        let a = save_fixture(&dir, "a.pdf", 2);
        let b = save_fixture(&dir, "b.pdf", 3);
        let outcome = merge(&[a, b], &dir.join("merged.pdf")).unwrap();
        assert_eq!(outcome.pages, 5);
        let reloaded = Document::load(&outcome.out_path).unwrap();
        assert_eq!(reloaded.get_pages().len(), 5);
        // Never overwrite.
        let second = merge(
            &[
                save_fixture(&dir, "c.pdf", 1),
                save_fixture(&dir, "d.pdf", 1),
            ],
            &dir.join("merged.pdf"),
        )
        .unwrap();
        assert_eq!(second.out_path, dir.join("merged-2.pdf"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn merge_rejects_single_input() {
        let dir = temp_dir();
        let a = save_fixture(&dir, "one.pdf", 1);
        assert!(merge(&[a], &dir.join("x.pdf")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn split_every_page_yields_single_page_files() {
        let dir = temp_dir();
        let input = save_fixture(&dir, "doc.pdf", 3);
        let outputs = split(&input, SplitMode::EveryPage, &dir).unwrap();
        assert_eq!(outputs.len(), 3);
        assert_eq!(outputs[0], dir.join("doc-p1.pdf"));
        for out in &outputs {
            assert_eq!(Document::load(out).unwrap().get_pages().len(), 1);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn split_ranges_names_and_counts() {
        let dir = temp_dir();
        let input = save_fixture(&dir, "doc.pdf", 5);
        let outputs = split(
            &input,
            SplitMode::Ranges(vec![(1, 2), (3, 3), (4, 5)]),
            &dir,
        )
        .unwrap();
        assert_eq!(
            outputs,
            vec![
                dir.join("doc-p1-2.pdf"),
                dir.join("doc-p3.pdf"),
                dir.join("doc-p4-5.pdf"),
            ]
        );
        assert_eq!(Document::load(&outputs[0]).unwrap().get_pages().len(), 2);
        assert_eq!(Document::load(&outputs[1]).unwrap().get_pages().len(), 1);
        assert!(split(&input, SplitMode::Ranges(vec![(4, 9)]), &dir).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn extract_keeps_selected_pages() {
        let dir = temp_dir();
        let input = save_fixture(&dir, "doc.pdf", 5);
        let outcome = extract_pages(&input, &[(2, 3), (5, 5)], &dir.join("out.pdf")).unwrap();
        assert_eq!(outcome.pages, 3);
        assert_eq!(
            Document::load(&outcome.out_path).unwrap().get_pages().len(),
            3
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rotate_sets_normalized_rotate_values() {
        let dir = temp_dir();
        let input = save_fixture(&dir, "doc.pdf", 2);
        let outcome = rotate(&input, 450, None, &dir.join("rot.pdf")).unwrap();
        let doc = Document::load(&outcome.out_path).unwrap();
        for (_, page_id) in doc.get_pages() {
            let dict = doc.get_dictionary(page_id).unwrap();
            assert_eq!(dict.get(b"Rotate").unwrap().as_i64().unwrap(), 90);
        }
        // Only page 2, on top of the existing 90.
        let outcome2 = rotate(&outcome.out_path, 180, Some(&[2]), &dir.join("rot2.pdf")).unwrap();
        let doc2 = Document::load(&outcome2.out_path).unwrap();
        let rotations: Vec<i64> = doc2
            .get_pages()
            .into_iter()
            .map(|(_, id)| {
                doc2.get_dictionary(id)
                    .unwrap()
                    .get(b"Rotate")
                    .unwrap()
                    .as_i64()
                    .unwrap()
            })
            .collect();
        assert_eq!(rotations, vec![90, 270]);
        assert!(rotate(&input, 45, None, &dir.join("bad.pdf")).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn images_to_pdf_builds_loadable_pdf() {
        let dir = temp_dir();
        let mut img = image::RgbaImage::new(64, 32);
        for px in img.pixels_mut() {
            *px = image::Rgba([200, 100, 50, 255]);
        }
        let png = dir.join("scan.png");
        img.save_with_format(&png, image::ImageFormat::Png).unwrap();

        let outcome = images_to_pdf(
            &[png.clone()],
            ImagePdfOptions::default(),
            &dir.join("scan.pdf"),
        )
        .unwrap();
        assert_eq!(outcome.pages, 1);
        let doc = Document::load(&outcome.out_path).unwrap();
        assert_eq!(doc.get_pages().len(), 1);

        // Two images → two pages, FitImage page size.
        let opts = ImagePdfOptions {
            page: PageSize::FitImage,
            ..ImagePdfOptions::default()
        };
        let outcome2 = images_to_pdf(&[png.clone(), png], opts, &dir.join("scan-fit.pdf")).unwrap();
        assert_eq!(
            Document::load(&outcome2.out_path)
                .unwrap()
                .get_pages()
                .len(),
            2
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn parse_ranges_table() {
        assert_eq!(
            parse_ranges("1-3,5,8-", 10).unwrap(),
            vec![(1, 3), (5, 5), (8, 10)]
        );
        assert_eq!(parse_ranges("-3", 10).unwrap(), vec![(1, 3)]);
        assert_eq!(
            parse_ranges(" 2 - 4 , 6 ", 10).unwrap(),
            vec![(2, 4), (6, 6)]
        );
        // End clamped to the page count.
        assert_eq!(parse_ranges("8-99", 10).unwrap(), vec![(8, 10)]);
        // Errors: reversed, zero, past the end, empty, garbage.
        assert!(parse_ranges("7-3", 10).is_err());
        assert!(parse_ranges("0-2", 10).is_err());
        assert!(parse_ranges("11", 10).is_err());
        assert!(parse_ranges("", 10).is_err());
        assert!(parse_ranges(",,", 10).is_err());
        assert!(parse_ranges("abc", 10).is_err());
        assert!(parse_ranges("1-2", 0).is_err());
    }
}
