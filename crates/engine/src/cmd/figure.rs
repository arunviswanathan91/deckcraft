//! Figure export: each slide becomes its own file (a publication figure), not a page of a document.
//!
//! `file.exportSlides` writes one PNG, JPEG, TIFF, SVG or PDF per slide at its own resolution
//! (72–4800 dpi) and, optionally, cropped to a rectangle of the slide. The resolution and crop of
//! a slide can be saved with it (`slide.exportSettings`, kept in the slide's tags, so they survive
//! saving as .deckcraft or .pptx) and are used by later exports.
//!
//! Every number is hostile: sizes are checked before anything is drawn or written, so a request
//! can't exhaust memory, and nothing is written unless every slide of the request can be.

use std::sync::Arc;

use deckcraft_model::{Presentation, Slide};
use kurbo::Rect;
use serde_json::{Value, json};

use super::{CommandSpec, bad, base64_encode, bool_or, cmd, has_doc, opt_usize_param, str_param};
use crate::{EngineError, Result, Session};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            noundo "file.exportSlides",
            "Export Slides as Figures…",
            ["File"],
            None,
            "{dir, name?: file name stem (the title), format?: png|jpeg|tiff|svg|pdf (png), dpi?: 72-4800 (300), slides?: [index] or a range text such as 1-3 or \"all\" (the current slide), crop?: {x, y, width, height} in slide points, useSlideSettings?: bool (true: saved dpi/crop/format of a slide win over the ones here), overrides?: [{slide: index, dpi?, crop?, format?}] (win over everything), quality?: 1-100 (jpeg 95), transparentBackground?: bool (png and tiff), textLayer?: bool (pdf; true)} → {files: [{slide, path, format, dpi, width, height, bytes}]}: one file per slide",
            has_doc,
            export_slides
        ),
        cmd!(
            "slide.exportSettings",
            "Slide Export Settings",
            [],
            None,
            "{slide?: index, dpi?: 72-4800 or null, crop?: {x, y, width, height} in slide points or null, format?: png|jpeg|tiff|svg|pdf or null} (null clears; absent keeps) → settings: the resolution, crop and format this slide is exported with, saved with the slide",
            has_doc,
            set_settings
        ),
        cmd!(
            query "slide.getExportSettings",
            "Get Slide Export Settings",
            [],
            None,
            "{} → {slides: [{slide, dpi?, crop?: {x, y, width, height}, format?}], slideWidth, slideHeight} in points",
            has_doc,
            get_settings
        ),
    ]
}

pub const TAG_DPI: &str = "deckcraft.export.dpi";
pub const TAG_CROP: &str = "deckcraft.export.crop";
pub const TAG_FORMAT: &str = "deckcraft.export.format";

pub const DEFAULT_DPI: f64 = 300.0;
pub const MIN_DPI: f64 = 72.0;
pub const MAX_DPI: f64 = 4800.0;
/// Longest side of a figure: JPEG, the narrowest format, stops at 65,535 pixels.
pub const MAX_SIDE_PX: f64 = 60_000.0;
/// Most pixels of one figure. An RGBA figure takes four bytes a pixel in memory while it is drawn
/// and encoded: 200 million is 800 MB, a 13.33 × 7.5 in slide at 1400 dpi.
pub const MAX_PIXELS: f64 = 200_000_000.0;
pub const MAX_FIGURES: usize = 500;

/// The file formats a figure can have.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Png,
    Jpeg,
    Tiff,
    Svg,
    Pdf,
}

impl Format {
    pub fn parse(text: &str) -> Option<Format> {
        Some(match text.trim().to_ascii_lowercase().as_str() {
            "png" => Format::Png,
            "jpeg" | "jpg" => Format::Jpeg,
            "tiff" | "tif" => Format::Tiff,
            "svg" => Format::Svg,
            "pdf" => Format::Pdf,
            _ => return None,
        })
    }
    pub fn id(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpeg => "jpeg",
            Format::Tiff => "tiff",
            Format::Svg => "svg",
            Format::Pdf => "pdf",
        }
    }
    pub fn extension(self) -> &'static str {
        match self {
            Format::Png => "png",
            Format::Jpeg => "jpg",
            Format::Tiff => "tif",
            Format::Svg => "svg",
            Format::Pdf => "pdf",
        }
    }
}

/// What is saved with a slide for exporting it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SlideSettings {
    pub dpi: Option<f64>,
    /// In slide points.
    pub crop: Option<Rect>,
    pub format: Option<Format>,
}

fn tag<'a>(slide: &'a Slide, name: &str) -> Option<&'a str> {
    slide.tags.iter().find(|(k, _)| k == name).map(|(_, v)| v.as_str())
}

fn parse_crop(text: &str) -> Option<Rect> {
    let v: Vec<f64> = text.split(',').map(|t| t.trim().parse::<f64>().ok().filter(|x| x.is_finite())).collect::<Option<_>>()?;
    let [x, y, w, h] = <[f64; 4]>::try_from(v).ok()?;
    (w > 0.0 && h > 0.0).then(|| Rect::new(x, y, x + w, y + h))
}

fn crop_text(r: Rect) -> String {
    format!("{},{},{},{}", r.x0, r.y0, r.width(), r.height())
}

/// The export settings saved with `slide`; anything unreadable counts as not set.
pub fn settings_of(slide: &Slide) -> SlideSettings {
    SlideSettings {
        dpi: tag(slide, TAG_DPI).and_then(|t| t.trim().parse::<f64>().ok()).filter(|d| (MIN_DPI..=MAX_DPI).contains(d)),
        crop: tag(slide, TAG_CROP).and_then(parse_crop),
        format: tag(slide, TAG_FORMAT).and_then(Format::parse),
    }
}

fn put_tag(slide: &mut Slide, name: &str, value: Option<String>) {
    slide.tags.retain(|(k, _)| k != name);
    if let Some(v) = value {
        slide.tags.push((name.to_string(), v));
    }
}

/// A rectangle given as `{x, y, width, height}` (slide points).
fn rect_param(v: &Value, cmd: &str) -> Result<Rect> {
    let num = |k: &str| v.get(k).and_then(Value::as_f64).filter(|x| x.is_finite());
    match (num("x"), num("y"), num("width"), num("height")) {
        (Some(x), Some(y), Some(w), Some(h)) if w > 0.0 && h > 0.0 => Ok(Rect::new(x, y, x + w, y + h)),
        _ => Err(bad(cmd, "a crop is {x, y, width, height} in slide points, with a positive width and height")),
    }
}

fn rect_json(r: Rect) -> Value {
    json!({"x": r.x0, "y": r.y0, "width": r.width(), "height": r.height()})
}

fn check_dpi(dpi: f64, cmd: &str) -> Result<f64> {
    if (MIN_DPI..=MAX_DPI).contains(&dpi) { Ok(dpi) } else { Err(bad(cmd, format!("dpi must be {MIN_DPI} to {MAX_DPI}"))) }
}

fn settings_json(i: usize, s: &SlideSettings) -> Value {
    let mut v = json!({"slide": i});
    if let Some(d) = s.dpi {
        v["dpi"] = json!(d);
    }
    if let Some(c) = s.crop {
        v["crop"] = rect_json(c);
    }
    if let Some(f) = s.format {
        v["format"] = json!(f.id());
    }
    v
}

fn get_settings(s: &mut Session, _p: &Value) -> Result<Value> {
    let doc = &s.doc()?.doc;
    let list: Vec<Value> = doc.slides.iter().enumerate().map(|(i, sl)| settings_json(i, &settings_of(sl))).collect();
    Ok(json!({"slides": list, "slideWidth": doc.slide_size.width, "slideHeight": doc.slide_size.height}))
}

fn set_settings(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "slide.exportSettings";
    let index = match opt_usize_param(p, "slide", C)? {
        Some(i) => i,
        None => s.doc()?.selection.slide,
    };
    // Parsed before the edit, so a bad value changes nothing.
    let dpi = match p.get("dpi") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(v) => Some(Some(check_dpi(v.as_f64().unwrap_or(f64::NAN), C)?)),
    };
    let crop = match p.get("crop") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(v) => Some(Some(rect_param(v, C)?)),
    };
    let format = match p.get("format") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(v) => Some(Some(v.as_str().and_then(Format::parse).ok_or_else(|| bad(C, "format is one of png, jpeg, tiff, svg, pdf"))?)),
    };
    s.edit(|doc, _| {
        let slide = doc.slides.get_mut(index).ok_or_else(|| bad(C, format!("no slide {index}")))?;
        let slide = Arc::make_mut(slide);
        if let Some(d) = dpi {
            put_tag(slide, TAG_DPI, d.map(|d| d.to_string()));
        }
        if let Some(c) = crop {
            put_tag(slide, TAG_CROP, c.map(crop_text));
        }
        if let Some(f) = format {
            put_tag(slide, TAG_FORMAT, f.map(|f| f.id().to_string()));
        }
        Ok(settings_json(index, &settings_of(slide)))
    })
}

/// One slide's figure, planned and checked.
#[derive(Clone, Debug)]
pub struct Plan {
    pub slide: usize,
    pub format: Format,
    pub dpi: f64,
    /// The part of the slide drawn, in points (the whole slide when not cropped).
    pub region: Rect,
    pub width: u32,
    pub height: u32,
}

/// Pixel size of `region` at `dpi`, or why it can't be made.
pub fn figure_size(slide: usize, region: Rect, dpi: f64) -> Result<(u32, u32)> {
    let k = dpi / 72.0;
    let (w, h) = ((region.width() * k).round().max(1.0), (region.height() * k).round().max(1.0));
    if w > MAX_SIDE_PX || h > MAX_SIDE_PX || w * h > MAX_PIXELS {
        return Err(bad(
            "file.exportSlides",
            format!(
                "slide {} would be {w:.0} × {h:.0} pixels ({:.0} megapixels) at {dpi:.0} dpi; the limit is {:.0} megapixels. Lower the dpi or crop the slide",
                slide + 1,
                w * h / 1e6,
                MAX_PIXELS / 1e6
            ),
        ));
    }
    Ok((w as u32, h as u32))
}

fn slides_param(p: &Value, doc: &Presentation, current: usize) -> Result<Vec<usize>> {
    let n = doc.slides.len();
    let list = match p.get("slides") {
        None | Some(Value::Null) => vec![current],
        Some(Value::String(t)) if t.trim().eq_ignore_ascii_case("all") => (0..n).collect(),
        Some(Value::String(t)) => super::file::parse_slide_range(t, n)?,
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| {
                v.as_u64().and_then(|i| usize::try_from(i).ok()).ok_or_else(|| bad("file.exportSlides", "slides: a list of slide indices (from 0)"))
            })
            .collect::<Result<_>>()?,
        Some(_) => return Err(bad("file.exportSlides", "slides: a list of indices, a range such as \"1-3,5\", or \"all\"")),
    };
    if let Some(i) = list.iter().find(|i| **i >= n) {
        return Err(bad("file.exportSlides", format!("no slide {i}")));
    }
    if list.is_empty() || list.len() > MAX_FIGURES {
        return Err(bad("file.exportSlides", format!("choose 1 to {MAX_FIGURES} slides")));
    }
    Ok(list)
}

/// The figures `p` asks for, each checked.
pub fn plan(doc: &Presentation, current: usize, p: &Value) -> Result<Vec<Plan>> {
    const C: &str = "file.exportSlides";
    let default_format = match str_param(p, "format") {
        None => Format::Png,
        Some(f) => Format::parse(f).ok_or_else(|| bad(C, format!("unknown format `{f}`: png, jpeg, tiff, svg or pdf")))?,
    };
    let default_dpi = match p.get("dpi") {
        None => DEFAULT_DPI,
        Some(v) => check_dpi(v.as_f64().unwrap_or(f64::NAN), C)?,
    };
    let default_crop = match p.get("crop") {
        None | Some(Value::Null) => None,
        Some(v) => Some(rect_param(v, C)?),
    };
    let use_saved = bool_or(p, "useSlideSettings", true);
    let overrides = p.get("overrides").and_then(Value::as_array).cloned().unwrap_or_default();
    let full = Rect::new(0.0, 0.0, doc.slide_size.width.max(1.0), doc.slide_size.height.max(1.0));
    let mut out = Vec::new();
    for i in slides_param(p, doc, current)? {
        let saved = if use_saved { doc.slides.get(i).map(|s| settings_of(s)).unwrap_or_default() } else { SlideSettings::default() };
        let (mut format, mut dpi, mut crop) = (saved.format.unwrap_or(default_format), saved.dpi.unwrap_or(default_dpi), saved.crop.or(default_crop));
        for o in overrides.iter().filter(|o| o.get("slide").and_then(Value::as_u64) == Some(i as u64)) {
            if let Some(d) = o.get("dpi").and_then(Value::as_f64) {
                dpi = check_dpi(d, C)?;
            }
            match o.get("crop") {
                Some(Value::Null) => crop = None,
                Some(v) => crop = Some(rect_param(v, C)?),
                None => {}
            }
            if let Some(f) = o.get("format").and_then(Value::as_str) {
                format = Format::parse(f).ok_or_else(|| bad(C, format!("unknown format `{f}`")))?;
            }
        }
        let region = crop.map_or(full, |c| c.intersect(full));
        if !(region.width() >= 1.0 && region.height() >= 1.0) {
            return Err(bad(C, format!("the crop of slide {} lies outside the slide or is smaller than a point", i + 1)));
        }
        let (width, height) = figure_size(i, region, dpi)?;
        out.push(Plan { slide: i, format, dpi, region, width, height });
    }
    Ok(out)
}

/// A file name part from a title: letters, digits, space, `-` and `_` stay.
pub fn file_stem(name: &str) -> String {
    let s: String = name.trim().chars().map(|c| if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_') { c } else { '_' }).collect();
    let s = s.trim().chars().take(80).collect::<String>();
    if s.is_empty() { "Slide".into() } else { s }
}

/// The file name of a figure: `<stem>_slide03.tif`, the number padded to the deck's size.
pub fn figure_name(stem: &str, slides: usize, pl: &Plan) -> String {
    let digits = slides.to_string().len().max(2);
    format!("{}_slide{:0digits$}.{}", file_stem(stem), pl.slide + 1, pl.format.extension())
}

/// The figure's bytes.
pub fn render_figure(doc: &Presentation, pl: &Plan, p: &Value) -> Result<Vec<u8>> {
    let transparent = bool_or(p, "transparentBackground", false) && matches!(pl.format, Format::Png | Format::Tiff);
    let max = MAX_PIXELS as u64;
    let fail = || EngineError::Other(format!("slide {} could not be drawn at {:.0} dpi", pl.slide + 1, pl.dpi));
    if pl.format == Format::Pdf {
        let opts = deckcraft_pdf::FigureOptions { dpi: pl.dpi, crop: Some(pl.region), text_layer: bool_or(p, "textLayer", true), max_pixels: max };
        return deckcraft_pdf::export_figure(doc, pl.slide, &opts).map_err(|e| EngineError::Other(e.to_string()));
    }
    let img = deckcraft_render::render_region(doc, pl.slide, pl.region, pl.dpi / 72.0, transparent, max).ok_or_else(fail)?;
    let bytes = match pl.format {
        Format::Png => img.into_png_at(pl.dpi, !transparent),
        Format::Jpeg => {
            let q = p.get("quality").and_then(Value::as_u64).map_or(95, |q| q.clamp(1, 100) as u8);
            img.into_jpeg_at(q, Some(pl.dpi))
        }
        Format::Tiff => img.into_tiff_at(pl.dpi, !transparent),
        Format::Svg => {
            let png = img.into_png_at(pl.dpi, true);
            let (pw, ph) = (pl.region.width(), pl.region.height());
            let title = doc.slides.get(pl.slide).map(|s| s.title()).unwrap_or_default();
            let title = title.lines().next().unwrap_or("").replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;");
            let svg = format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"{w:.4}in\" height=\"{h:.4}in\" viewBox=\"0 0 {pw:.3} {ph:.3}\">\n<title>{title}</title>\n<image x=\"0\" y=\"0\" width=\"{pw:.3}\" height=\"{ph:.3}\" preserveAspectRatio=\"none\" xlink:href=\"data:image/png;base64,{data}\"/>\n</svg>\n",
                w = pw / 72.0,
                h = ph / 72.0,
                data = base64_encode(&png),
            );
            svg.into_bytes()
        }
        Format::Pdf => Vec::new(),
    };
    if bytes.is_empty() { Err(fail()) } else { Ok(bytes) }
}

fn export_slides(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "file.exportSlides";
    let dir = str_param(p, "dir").ok_or_else(|| bad(C, "missing `dir`"))?.trim_end_matches(['/', '\\']).to_string();
    if dir.is_empty() {
        return Err(bad(C, "missing `dir`"));
    }
    let st = s.doc()?;
    let plans = plan(&st.doc, st.selection.slide, p)?;
    let stem = str_param(p, "name").map(String::from).unwrap_or_else(|| st.title());
    let mut files = Vec::new();
    for (k, pl) in plans.iter().enumerate() {
        let bytes = render_figure(&st.doc, pl, p)?;
        let path = format!("{dir}/{}", figure_name(&stem, st.doc.slides.len(), pl));
        crate::io::write_file(&path, &bytes)
            .map_err(|e| EngineError::Other(if k == 0 { e } else { format!("{e} ({k} of {} figures were written)", plans.len()) }))?;
        files.push(json!({"slide": pl.slide, "path": path, "format": pl.format.id(), "dpi": pl.dpi, "width": pl.width, "height": pl.height, "bytes": bytes.len()}));
    }
    Ok(json!({"files": files}))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use serde_json::json;

    use super::*;

    fn deck(n: usize) -> Session {
        let mut s = Session::new();
        s.execute("file.new", &json!({"blank": true})).unwrap();
        while s.doc().unwrap().doc.slides.len() < n {
            s.execute("slide.new", &json!({})).unwrap();
        }
        for (i, hex) in ["#E02020", "#20A040", "#2040E0"].iter().enumerate().take(n) {
            s.execute("design.background", &json!({"color": hex, "index": i})).unwrap();
        }
        s
    }

    fn dir(name: &str) -> String {
        let d = std::env::temp_dir().join(format!("deckcraft-figure-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d.to_string_lossy().to_string()
    }

    fn export(s: &mut Session, dir: &str, extra: Value) -> Value {
        let mut p = json!({"dir": dir, "name": "Fig"});
        p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
        s.execute("file.exportSlides", &p).unwrap()
    }

    #[test]
    fn every_slide_is_its_own_file_at_its_own_dpi() {
        let mut s = deck(3);
        s.execute("slide.exportSettings", &json!({"slide": 1, "dpi": 600})).unwrap();
        let d = dir("each");
        let r = export(&mut s, &d, json!({"slides": "all", "format": "png", "dpi": 150}));
        let files = r["files"].as_array().unwrap();
        assert_eq!(files.len(), 3);
        let size = |i: usize| (files[i]["width"].as_u64().unwrap(), files[i]["height"].as_u64().unwrap());
        // 13.333 × 7.5 in.
        assert_eq!(size(0), (2000, 1125));
        assert_eq!(size(1), (8000, 4500), "slide 2 keeps its saved 600 dpi");
        let png = std::fs::read(files[1]["path"].as_str().unwrap()).unwrap();
        assert_eq!(deckcraft_render::image_dpi(&png), Some((600.0, 600.0)));
        let img = image::load_from_memory(&png).unwrap().to_rgb8();
        assert_eq!((img.width(), img.height()), (8000, 4500));
        assert_eq!(img.get_pixel(10, 10).0, [0x20, 0xA0, 0x40], "slide 2 is green");
        assert!(files[0]["path"].as_str().unwrap().ends_with("Fig_slide01.png"));
    }

    #[test]
    fn a_crop_exports_only_that_area() {
        let mut s = deck(1);
        let d = dir("crop");
        let r = export(&mut s, &d, json!({"format": "png", "dpi": 300, "crop": {"x": 96, "y": 72, "width": 288, "height": 144}}));
        let f = &r["files"][0];
        assert_eq!((f["width"].as_u64(), f["height"].as_u64()), (Some(1200), Some(600)), "4 × 2 in at 300 dpi");
        let img = image::open(f["path"].as_str().unwrap()).unwrap();
        assert_eq!((img.width(), img.height()), (1200, 600));
    }

    #[test]
    fn tiff_jpeg_svg_and_pdf_are_written_with_their_resolution() {
        let mut s = deck(1);
        let d = dir("formats");
        let crop = json!({"x": 0, "y": 0, "width": 144, "height": 72});
        let mut out = std::collections::HashMap::new();
        for f in ["tiff", "jpeg", "svg", "pdf"] {
            let r = export(&mut s, &d, json!({"format": f, "dpi": 600, "crop": crop}));
            out.insert(f, std::fs::read(r["files"][0]["path"].as_str().unwrap()).unwrap());
        }
        let tif = &out["tiff"];
        assert!(tif.starts_with(b"II*\0") || tif.starts_with(b"MM\0*"));
        let img = image::load_from_memory_with_format(tif, image::ImageFormat::Tiff).unwrap();
        assert_eq!((img.width(), img.height()), (1200, 600));
        // The resolution is in the file, as 600/1 inch.
        let mut dec = tiff::decoder::Decoder::new(std::io::Cursor::new(tif)).unwrap();
        let xres = dec.get_tag(tiff::tags::Tag::XResolution).unwrap();
        assert_eq!(xres, tiff::decoder::ifd::Value::Rational(600, 1));
        assert_eq!(deckcraft_render::image_dpi(&out["jpeg"]), Some((600.0, 600.0)));
        let svg = String::from_utf8(out["svg"].clone()).unwrap();
        assert!(svg.contains("width=\"2.0000in\"") && svg.contains("viewBox=\"0 0 144.000 72.000\"") && svg.contains("data:image/png;base64,"));
        assert!(out["pdf"].starts_with(b"%PDF-"));
        let pdf = String::from_utf8_lossy(&out["pdf"]).to_string();
        assert!(pdf.contains("/MediaBox [0 0 144 72]") || pdf.contains("/MediaBox[0 0 144 72]"), "page is the crop: {}", &pdf[..pdf.len().min(600)]);
    }

    #[test]
    fn saved_settings_round_trip_and_clear() {
        let mut s = deck(2);
        s.execute(
            "slide.exportSettings",
            &json!({"slide": 1, "dpi": 1200, "format": "tiff", "crop": {"x": 10, "y": 20, "width": 100, "height": 50}}),
        )
        .unwrap();
        let g = s.execute("slide.getExportSettings", &json!({})).unwrap();
        assert_eq!(
            g["slides"][1],
            json!({"slide": 1, "dpi": 1200.0, "crop": {"x": 10.0, "y": 20.0, "width": 100.0, "height": 50.0}, "format": "tiff"})
        );
        assert_eq!(g["slides"][0], json!({"slide": 0}));
        // The pptx round trip keeps the tags.
        let bytes = deckcraft_pptx::export(&s.doc().unwrap().doc).unwrap();
        let back = deckcraft_pptx::import(&bytes).unwrap();
        assert_eq!(settings_of(&back.slides[1]).dpi, Some(1200.0));
        s.execute("slide.exportSettings", &json!({"slide": 1, "dpi": null, "crop": null, "format": null})).unwrap();
        assert_eq!(settings_of(&s.doc().unwrap().doc.slides[1]), SlideSettings::default());
        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(settings_of(&s.doc().unwrap().doc.slides[1]).dpi, Some(1200.0));
    }

    #[test]
    fn overrides_win_and_bad_requests_write_nothing() {
        let mut s = deck(2);
        s.execute("slide.exportSettings", &json!({"slide": 0, "dpi": 600})).unwrap();
        let d = dir("override");
        let r = export(&mut s, &d, json!({"slides": [0, 1], "overrides": [{"slide": 0, "dpi": 72}], "dpi": 144}));
        assert_eq!(r["files"][0]["dpi"], 72.0);
        assert_eq!(r["files"][1]["dpi"], 144.0);
        let ignore = export(&mut s, &d, json!({"slides": [0], "useSlideSettings": false, "dpi": 100}));
        assert_eq!(ignore["files"][0]["dpi"], 100.0);

        let bad_dir = dir("bad");
        for p in [
            json!({"dpi": 10}),
            json!({"dpi": 99999}),
            json!({"dpi": null}),
            json!({"dpi": "high"}),
            json!({"format": "bmp"}),
            json!({"slides": [5]}),
            json!({"slides": "9-12"}),
            json!({"crop": {"x": 0, "y": 0, "width": -1, "height": 5}}),
            json!({"crop": {"x": 9000, "y": 0, "width": 50, "height": 50}}),
            json!({"slides": [0, 1], "overrides": [{"slide": 1, "dpi": 4800}]}),
            json!({"dpi": 4800}),
        ] {
            let mut full = json!({"dir": bad_dir});
            full.as_object_mut().unwrap().extend(p.as_object().unwrap().clone());
            assert!(s.execute("file.exportSlides", &full).is_err(), "{p}");
        }
        assert_eq!(std::fs::read_dir(&bad_dir).unwrap().count(), 0, "nothing written");
        assert!(s.execute("slide.exportSettings", &json!({"dpi": 5})).is_err());
        assert!(s.execute("slide.exportSettings", &json!({"slide": 9, "dpi": 300})).is_err());
    }

    #[test]
    fn a_very_high_dpi_is_drawn_in_tiles_that_match_one_render() {
        let mut s = deck(1);
        crate::sample::open_sample(&mut s).unwrap();
        let doc = s.doc().unwrap().doc.clone();
        // 5 px/pt = 360 dpi: 4800 × 2700 px, two tiles across.
        let whole = deckcraft_render::render_slide(&doc, 0, &deckcraft_render::RenderOpts { scale: 5.0, ..Default::default() });
        let full = Rect::new(0.0, 0.0, doc.slide_size.width, doc.slide_size.height);
        let tiled = deckcraft_render::render_region(&doc, 0, full, 5.0, false, 100_000_000).unwrap();
        assert_eq!((tiled.width, tiled.height), (whole.width, whole.height));
        let differing = whole.pixels.iter().zip(&tiled.pixels).filter(|(a, b)| a != b).count();
        assert_eq!(differing, 0, "tiles seam-free");
        // A crop is the same pixels as that part of the whole.
        let part = deckcraft_render::render_region(&doc, 0, Rect::new(100.0, 50.0, 300.0, 150.0), 5.0, false, 100_000_000).unwrap();
        assert_eq!((part.width, part.height), (1000, 500));
        for (x, y) in [(0u32, 0u32), (500, 250), (999, 499), (13, 400)] {
            assert_eq!(part.pixel(x, y), whole.pixel(500 + x, 250 + y));
        }
    }

    #[test]
    fn pdf_figure_keeps_text_only_inside_the_crop() {
        let mut s = Session::new();
        crate::sample::open_sample(&mut s).unwrap();
        let doc = s.doc().unwrap().doc.clone();
        let whole =
            deckcraft_pdf::export_figure(&doc, 0, &deckcraft_pdf::FigureOptions { dpi: 100.0, crop: None, text_layer: true, max_pixels: 50_000_000 })
                .unwrap();
        let none = deckcraft_pdf::export_figure(
            &doc,
            0,
            &deckcraft_pdf::FigureOptions { dpi: 100.0, crop: Some(Rect::new(0.0, 0.0, 4.0, 4.0)), text_layer: true, max_pixels: 50_000_000 },
        )
        .unwrap();
        assert!(whole.len() > none.len());
        assert!(!String::from_utf8_lossy(&none).contains("/Font"), "no text inside a 4 pt corner");
    }

    /// Run by hand (`cargo test -p deckcraft-engine --lib -- --ignored full_slide_at_1200`): the
    /// whole sample slide at 1200 dpi, 16000 × 9000 pixels, in every format.
    #[test]
    #[ignore]
    fn full_slide_at_1200_dpi() {
        let mut s = Session::new();
        crate::sample::open_sample(&mut s).unwrap();
        let d = dir("big");
        for f in ["png", "tiff", "jpeg", "pdf", "svg"] {
            let t = std::time::Instant::now();
            let r = export(&mut s, &d, json!({"format": f, "dpi": 1200}));
            eprintln!(
                "{f}: {:?} {} bytes in {:.1}s",
                (r["files"][0]["width"].as_u64(), r["files"][0]["height"].as_u64()),
                r["files"][0]["bytes"],
                t.elapsed().as_secs_f64()
            );
        }
    }
}
