//! Export Slides as Figures: every slide becomes its own TIFF, PNG, JPEG, SVG or PDF at its own
//! resolution, optionally cropped (publication figures). The work is `file.exportSlides`.

use deckcraft_engine::cmd::figure::{self, MAX_DPI, MIN_DPI};

use super::{Dialog, SlideApp, Tokens, Ui, Value, buttons, check, json, vec2};

const FORMATS: [(&str, &str); 5] = [
    ("tiff", "TIFF (.tif): lossless, for journals"),
    ("png", "PNG (.png): lossless"),
    ("jpeg", "JPEG (.jpg)"),
    ("pdf", "PDF (.pdf): one slide per file"),
    ("svg", "SVG (.svg): the picture in a vector wrapper"),
];
const DPI_PRESETS: [&str; 6] = ["72", "150", "300", "600", "1200", "2400"];
/// The four crop fields of the dialog (`c`) and of a slide row (`c<i>`).
const CROP_KEYS: [&str; 4] = ["l", "t", "w", "h"];

/// Inches typed in a field; `None` when it is blank or not a number.
fn inches(d: &Dialog, key: &str) -> Option<f64> {
    d.fields.get(key).and_then(|t| t.trim().trim_end_matches(['"', ' ']).parse::<f64>().ok()).filter(|v| v.is_finite())
}

/// The crop in the fields `<prefix>l`…`<prefix>h` as `{x,y,width,height}` in points: `Ok(None)`
/// when all four are blank, an error when only some are filled in or the size is not positive.
fn crop_of(d: &Dialog, prefix: &str, what: &str) -> Result<Option<Value>, String> {
    let v: Vec<Option<f64>> = CROP_KEYS.iter().map(|k| inches(d, &format!("{prefix}{k}"))).collect();
    if v.iter().all(Option::is_none) {
        return Ok(None);
    }
    let [Some(x), Some(y), Some(w), Some(h)] = v[..] else {
        return Err(format!("{what}: fill in all of Left, Top, Width and Height, or clear them"));
    };
    if w <= 0.0 || h <= 0.0 {
        return Err(format!("{what}: the width and height must be more than 0"));
    }
    Ok(Some(json!({"x": x * 72.0, "y": y * 72.0, "width": w * 72.0, "height": h * 72.0})))
}

fn dpi_of(d: &Dialog, key: &str, what: &str) -> Result<Option<f64>, String> {
    let Some(t) = d.fields.get(key).map(|t| t.trim()).filter(|t| !t.is_empty()) else { return Ok(None) };
    match t.parse::<f64>() {
        Ok(v) if (MIN_DPI..=MAX_DPI).contains(&v) => Ok(Some(v)),
        _ => Err(format!("{what}: resolution is {MIN_DPI} to {MAX_DPI} dpi")),
    }
}

/// The slides the dialog is set to export (0-based).
fn chosen_slides(d: &Dialog, count: usize, current: usize) -> Result<Vec<usize>, String> {
    match d.fields.get("fslides").map(String::as_str).unwrap_or("current") {
        "all" => Ok((0..count).collect()),
        "range" => {
            deckcraft_engine::cmd::file::parse_slide_range(d.fields.get("frange").map_or("", String::as_str), count).map_err(|e| e.to_string())
        }
        _ => Ok(vec![current.min(count.saturating_sub(1))]),
    }
}

/// What the dialog asks for: the `file.exportSlides` params without `dir`, and per chosen slide the
/// resolution and crop in its row (`slide.exportSettings` values, for saving them).
pub(super) struct Request {
    pub params: Value,
    pub rows: Vec<(usize, Option<f64>, Option<Value>)>,
}

pub(super) fn request(d: &Dialog, count: usize, current: usize) -> Result<Request, String> {
    let slides = chosen_slides(d, count, current)?;
    let preset = d.fields.get("fdpi").map_or("300", String::as_str);
    let dpi = if preset == "custom" {
        dpi_of(d, "fdpiCustom", "Resolution")?.ok_or("Resolution: enter a number")?
    } else {
        preset.parse().unwrap_or(300.0)
    };
    let crop = if d.fields.get("fcrop").is_some_and(|v| v == "1") { crop_of(d, "c", "Crop")? } else { None };
    let mut overrides = Vec::new();
    let mut rows = Vec::new();
    for &i in &slides {
        let what = format!("Slide {}", i + 1);
        let (row_dpi, row_crop) = (dpi_of(d, &format!("dpi{i}"), &what)?, crop_of(d, &format!("c{i}"), &what)?);
        let mut o = json!({"slide": i});
        if let Some(v) = row_dpi {
            o["dpi"] = json!(v);
        }
        if let Some(c) = &row_crop {
            o["crop"] = c.clone();
        }
        overrides.push(o);
        rows.push((i, row_dpi, row_crop));
    }
    let mut params = json!({
        "format": d.fields.get("ffmt").map_or("tiff", String::as_str),
        "dpi": dpi,
        "slides": slides,
        "useSlideSettings": false,
        "overrides": overrides,
        "transparentBackground": d.fields.get("ftransparent").is_some_and(|v| v == "1"),
    });
    if let Some(c) = crop {
        params["crop"] = c;
    }
    Ok(Request { params, rows })
}

/// Rows start with what is saved on each slide.
fn seed(app: &SlideApp, d: &mut Dialog) {
    if d.fields.contains_key("seeded") {
        return;
    }
    d.fields.insert("seeded".into(), "1".into());
    let Some(doc) = app.session.active().map(|s| s.doc.clone()) else { return };
    let show = |v: f64| format!("{}", (v / 72.0 * 1000.0).round() / 1000.0);
    for (i, sl) in doc.slides.iter().enumerate() {
        let s = figure::settings_of(sl);
        if let Some(dpi) = s.dpi {
            d.fields.insert(format!("dpi{i}"), format!("{dpi}"));
        }
        if let Some(c) = s.crop {
            for (k, v) in CROP_KEYS.iter().zip([c.x0, c.y0, c.width(), c.height()]) {
                d.fields.insert(format!("c{i}{k}"), show(v));
            }
        }
    }
}

/// A sketch of the slide where dragging draws the crop rectangle (fields `cl`…`ch`, inches).
fn crop_sketch(ui: &mut Ui, d: &mut Dialog, (sw, sh): (f64, f64)) {
    let t = Tokens::get(ui.ctx());
    let width = 240.0_f32;
    let height = (f64::from(width) * sh / sw.max(1.0)) as f32;
    let (resp, painter) = ui.allocate_painter(vec2(width, height.clamp(20.0, 200.0)), egui::Sense::click_and_drag());
    let r = resp.rect;
    painter.rect_filled(r, 2.0, egui::Color32::WHITE);
    painter.rect_stroke(r, 2.0, egui::Stroke::new(1.0, t.text_dim), egui::StrokeKind::Inside);
    let to_in = |p: egui::Pos2| {
        (
            (f64::from(p.x - r.left()) / f64::from(r.width()) * sw / 72.0).clamp(0.0, sw / 72.0),
            (f64::from(p.y - r.top()) / f64::from(r.height()) * sh / 72.0).clamp(0.0, sh / 72.0),
        )
    };
    let snap = |v: f64| (v * 100.0).round() / 100.0;
    if resp.drag_started()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let (x, y) = to_in(p);
        d.fields.insert("dragx".into(), x.to_string());
        d.fields.insert("dragy".into(), y.to_string());
    }
    if resp.dragged()
        && let (Some(p), Some(x0), Some(y0)) = (resp.interact_pointer_pos(), inches(d, "dragx"), inches(d, "dragy"))
    {
        let (x, y) = to_in(p);
        for (k, v) in CROP_KEYS.iter().zip([x0.min(x), y0.min(y), (x - x0).abs(), (y - y0).abs()]) {
            d.fields.insert(format!("c{k}"), snap(v).to_string());
        }
        d.fields.insert("fcrop".into(), "1".into());
    }
    if let Ok(Some(c)) = crop_of(d, "c", "Crop") {
        let g = |k: &str| c.get(k).and_then(Value::as_f64).unwrap_or(0.0);
        let to_px = |x: f64, y: f64| egui::pos2(r.left() + (x / sw) as f32 * r.width(), r.top() + (y / sh) as f32 * r.height());
        let area = egui::Rect::from_two_pos(to_px(g("x"), g("y")), to_px(g("x") + g("width"), g("y") + g("height"))).intersect(r);
        painter.rect_filled(area, 0.0, t.accent.gamma_multiply(0.3));
        painter.rect_stroke(area, 0.0, egui::Stroke::new(1.5, t.accent), egui::StrokeKind::Inside);
    }
}

pub(super) fn figures_dialog(app: &mut SlideApp, ui: &mut Ui, d: &mut Dialog) -> bool {
    seed(app, d);
    let Some((count, current, size, title)) =
        app.session.active().map(|s| (s.doc.slides.len(), s.selection.slide, (s.doc.slide_size.width, s.doc.slide_size.height), s.title()))
    else {
        return true;
    };
    ui.label("Each chosen slide is written to its own file, at its own resolution.");
    ui.add_space(4.0);
    let fmt = d.get("ffmt", "tiff");
    ui.horizontal(|ui| {
        let l = ui.label("File type:");
        let text = FORMATS.iter().find(|f| f.0 == fmt).map_or("", |f| f.1);
        let combo = craft_ui::ComboBox::from_id_salt("fig_format").width(300.0).selected_text(text).show_ui(ui, |ui| {
            for (k, label) in FORMATS {
                if ui.selectable_label(fmt == k, label).clicked() {
                    d.fields.insert("ffmt".into(), k.into());
                }
            }
        });
        let _ = combo.response.labelled_by(l.id);
    });
    ui.horizontal(|ui| {
        let which = d.get("fslides", "current");
        for (k, label) in [("current", "Current slide"), ("all", "All slides"), ("range", "Slides:")] {
            if ui.radio(which == k, label).clicked() {
                d.fields.insert("fslides".into(), k.into());
            }
        }
        if which == "range" {
            let mut r = d.get("frange", "1");
            ui.add(egui::TextEdit::singleline(&mut r).desired_width(90.0).hint_text("1-3, 5"));
            d.fields.insert("frange".into(), r);
        }
    });
    let preset = d.get("fdpi", "300");
    ui.horizontal(|ui| {
        let l = ui.label("Resolution (dpi):");
        let combo = craft_ui::ComboBox::from_id_salt("fig_dpi")
            .width(110.0)
            .selected_text(if preset == "custom" { "Custom".to_string() } else { preset.clone() })
            .show_ui(ui, |ui| {
                for k in DPI_PRESETS.iter().copied().chain(["custom"]) {
                    if ui.selectable_label(preset == k, if k == "custom" { "Custom" } else { k }).clicked() {
                        d.fields.insert("fdpi".into(), k.into());
                    }
                }
            });
        let _ = combo.response.labelled_by(l.id);
        if preset == "custom" {
            let mut v = d.get("fdpiCustom", "1200");
            ui.add(egui::TextEdit::singleline(&mut v).desired_width(70.0));
            d.fields.insert("fdpiCustom".into(), v);
        }
        ui.label(egui::RichText::new(format!("{MIN_DPI:.0}–{MAX_DPI:.0}")).color(Tokens::get(ui.ctx()).text_dim));
    });
    if matches!(fmt.as_str(), "png" | "tiff") {
        check(ui, d, "Transparent background", "ftransparent", false);
    }

    ui.add_space(4.0);
    ui.separator();
    let cropping = check(ui, d, "Crop the slide and export only that area (drag on the sketch, or type inches)", "fcrop", false);
    if cropping {
        ui.horizontal(|ui| {
            crop_sketch(ui, d, size);
            ui.vertical(|ui| {
                for (k, label) in CROP_KEYS.iter().zip(["Left", "Top", "Width", "Height"]) {
                    ui.horizontal(|ui| {
                        ui.add_sized(vec2(50.0, 20.0), egui::Label::new(label));
                        let mut v = d.get(&format!("c{k}"), "");
                        ui.add(egui::TextEdit::singleline(&mut v).desired_width(60.0));
                        d.fields.insert(format!("c{k}"), v);
                    });
                }
                ui.label(egui::RichText::new(format!("Slide: {:.2} × {:.2} in", size.0 / 72.0, size.1 / 72.0)).color(Tokens::get(ui.ctx()).text_dim));
            });
        });
    }

    let req = request(d, count, current);
    if let Ok(r) = &req
        && let Some(slides) = r.params.get("slides").and_then(Value::as_array)
        && slides.len() > 1
    {
        ui.add_space(4.0);
        ui.separator();
        ui.label("Settings for each slide (blank: use the settings above). Crop is Left, Top, Width, Height in inches.");
        egui::ScrollArea::vertical().max_height(150.0).id_salt("fig_rows").show(ui, |ui| {
            for s in slides.iter().filter_map(Value::as_u64).map(|s| s as usize) {
                ui.horizontal(|ui| {
                    ui.add_sized(vec2(56.0, 20.0), egui::Label::new(format!("Slide {}", s + 1)));
                    let mut v = d.get(&format!("dpi{s}"), "");
                    ui.add(egui::TextEdit::singleline(&mut v).desired_width(48.0).hint_text("dpi"));
                    d.fields.insert(format!("dpi{s}"), v);
                    for k in CROP_KEYS {
                        let mut v = d.get(&format!("c{s}{k}"), "");
                        ui.add(egui::TextEdit::singleline(&mut v).desired_width(40.0).hint_text(k));
                        d.fields.insert(format!("c{s}{k}"), v);
                    }
                });
            }
        });
    } else if let Ok(r) = &req
        && let Some(s) = r.params["slides"].get(0).and_then(Value::as_u64)
    {
        ui.add_space(4.0);
        ui.separator();
        ui.horizontal(|ui| {
            ui.label(format!("Slide {} only: set its own resolution and crop", s + 1));
        });
        ui.horizontal(|ui| {
            let s = s as usize;
            let mut v = d.get(&format!("dpi{s}"), "");
            ui.add(egui::TextEdit::singleline(&mut v).desired_width(48.0).hint_text("dpi"));
            d.fields.insert(format!("dpi{s}"), v);
            for k in CROP_KEYS {
                let mut v = d.get(&format!("c{s}{k}"), "");
                ui.add(egui::TextEdit::singleline(&mut v).desired_width(40.0).hint_text(k));
                d.fields.insert(format!("c{s}{k}"), v);
            }
        });
    }
    check(ui, d, "Remember these resolutions and crops with the slides", "fsave", true);

    // What it will make, or why not.
    let plans = req.as_ref().map_err(Clone::clone).and_then(|r| {
        let doc = app.session.active().map(|s| s.doc.clone()).ok_or("no presentation")?;
        figure::plan(&doc, current, &r.params).map_err(|e| e.to_string())
    });
    let t = Tokens::get(ui.ctx());
    match &plans {
        Ok(p) => {
            let sizes: Vec<String> = p.iter().take(3).map(|p| format!("slide {}: {} × {} px", p.slide + 1, p.width, p.height)).collect();
            let more = if p.len() > 3 { format!(" … {} files", p.len()) } else { String::new() };
            ui.label(egui::RichText::new(format!("{}{more}", sizes.join("; "))).color(t.text_dim));
        }
        Err(e) => {
            ui.label(egui::RichText::new(e).color(t.text_dim));
        }
    }
    let (ok, cancel) = buttons(ui, "Export");
    if ok {
        let (Ok(req), Ok(_)) = (req, plans) else {
            app.set_status("Fix the settings in the dialog first");
            return false;
        };
        return export(app, d, &title, req) || cancel;
    }
    cancel
}

/// Saves the row settings with the slides, then writes the figures. True when the dialog is done.
fn export(app: &mut SlideApp, d: &Dialog, title: &str, req: Request) -> bool {
    if d.fields.get("fsave").is_some_and(|v| v == "1") {
        let saved: Vec<_> = app
            .session
            .active()
            .map(|s| req.rows.iter().map(|(i, ..)| s.doc.slides.get(*i).map(|sl| figure::settings_of(sl))).collect())
            .unwrap_or_default();
        for ((i, dpi, crop), old) in req.rows.iter().zip(saved) {
            let old = old.unwrap_or_default();
            let same_crop = match (&crop, old.crop) {
                (None, None) => true,
                (Some(c), Some(o)) => {
                    let g = |k: &str| c.get(k).and_then(Value::as_f64).unwrap_or(f64::NAN);
                    [g("x"), g("y"), g("width"), g("height")].iter().zip([o.x0, o.y0, o.width(), o.height()]).all(|(a, b)| (a - b).abs() < 1e-6)
                }
                _ => false,
            };
            if *dpi != old.dpi || !same_crop {
                let _ = app.run("slide.exportSettings", json!({"slide": i, "dpi": dpi, "crop": crop}));
            }
        }
    }
    let Some(doc) = app.session.active().map(|s| s.doc.clone()) else { return true };
    let current = app.session.active().map_or(0, |s| s.selection.slide);
    let mut params = req.params;
    if let Some(pick) = app.services.pick_save.as_mut() {
        let ext = figure::Format::parse(params["format"].as_str().unwrap_or("tiff")).map_or("tif", figure::Format::extension);
        let Some(path) = pick(&format!("{}.{ext}", figure::file_stem(title))) else { return false };
        let p = std::path::Path::new(&path);
        let dir = p.parent().map(|x| x.to_string_lossy().to_string()).filter(|x| !x.is_empty()).unwrap_or_else(|| ".".into());
        params["dir"] = json!(dir);
        params["name"] = json!(p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| title.to_string()));
        return match app.run("file.exportSlides", params) {
            Ok(r) => {
                let n = r["files"].as_array().map_or(0, Vec::len);
                app.set_status(format!("Exported {n} figure{} to {dir}", if n == 1 { "" } else { "s" }));
                true
            }
            Err(_) => false,
        };
    }
    // The web: one download per slide.
    let plans = match figure::plan(&doc, current, &params) {
        Ok(p) => p,
        Err(e) => {
            app.set_status(e.to_string());
            return false;
        }
    };
    let mut any = false;
    for pl in &plans {
        let bytes = figure::render_figure(&doc, pl, &params);
        any = app.download_file(&figure::figure_name(title, doc.slides.len(), pl), bytes) || any;
    }
    any
}
