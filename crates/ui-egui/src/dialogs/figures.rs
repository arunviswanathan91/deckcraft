//! Export Slides as Figures: every slide becomes its own TIFF, PNG, JPEG, SVG or PDF at its own
//! resolution, optionally cropped (publication figures). The work is `file.exportSlides`, run one
//! slide a frame so a progress bar can show. Crops and per-slide resolutions are set in Crop Set.

use deckcraft_engine::cmd::figure::{self, MAX_DPI, MIN_DPI, Plan};

use super::{Dialog, SlideApp, Tokens, Ui, Value, buttons, check, json};

const FORMATS: [(&str, &str); 5] = [
    ("tiff", "TIFF (.tif): lossless, for journals"),
    ("png", "PNG (.png): lossless"),
    ("jpeg", "JPEG (.jpg)"),
    ("pdf", "PDF (.pdf): one slide per file"),
    ("svg", "SVG (.svg): the picture in a vector wrapper"),
];
const DPI_PRESETS: [&str; 6] = ["72", "150", "300", "600", "1200", "2400"];

/// An export under way: the figures still to write, one per frame.
#[derive(Clone, Debug)]
pub struct Job {
    pub plans: Vec<Plan>,
    pub params: Value,
    /// The folder the figures go to (the desktop); `None` hands each to the browser as a download.
    pub dir: Option<String>,
    pub stem: String,
    pub done: Vec<Value>,
    /// The next frame writes the next figure (the frame before shows what is being worked on).
    pub pending: bool,
    pub failed: Option<String>,
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

/// The `file.exportSlides` params (without `dir`) the dialog asks for. Each slide's own crop and
/// resolution, saved by Crop Set, win over the default resolution here.
pub(super) fn request(d: &Dialog, count: usize, current: usize) -> Result<Value, String> {
    let slides = chosen_slides(d, count, current)?;
    let preset = d.fields.get("fdpi").map_or("300", String::as_str);
    let dpi: f64 = if preset == "custom" {
        let v = d.fields.get("fdpiCustom").and_then(|t| t.trim().parse::<f64>().ok()).filter(|v| (MIN_DPI..=MAX_DPI).contains(v));
        v.ok_or_else(|| format!("Resolution: enter {MIN_DPI} to {MAX_DPI} dpi"))?
    } else {
        preset.parse().unwrap_or(300.0)
    };
    Ok(json!({
        "format": d.fields.get("ffmt").map_or("tiff", String::as_str),
        "dpi": dpi,
        "slides": slides,
        "useSlideSettings": true,
        "transparentBackground": d.fields.get("ftransparent").is_some_and(|v| v == "1"),
    }))
}

/// A dialog opened again from Crop Set gets back what it had.
fn restore(d: &mut Dialog) {
    if d.fields.contains_key("restored") {
        return;
    }
    d.fields.insert("restored".into(), "1".into());
    if let Some(f) = d.params.get("fields").and_then(Value::as_object) {
        for (k, v) in f {
            if let Some(v) = v.as_str() {
                d.fields.insert(k.clone(), v.to_string());
            }
        }
    }
}

pub(super) fn figures_dialog(app: &mut SlideApp, ui: &mut Ui, d: &mut Dialog) -> bool {
    restore(d);
    if d.job.is_some() {
        return progress(app, ui, d);
    }
    let Some((count, current, title)) = app.session.active().map(|s| (s.doc.slides.len(), s.selection.slide, s.title())) else {
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
        ui.label(egui::RichText::new(format!("{MIN_DPI:.0}–{MAX_DPI:.0}; for slides without their own")).color(Tokens::get(ui.ctx()).text_dim));
    });
    if matches!(fmt.as_str(), "png" | "tiff") {
        check(ui, d, "Transparent background", "ftransparent", false);
    }

    ui.add_space(4.0);
    ui.separator();
    let cropped = app.session.active().map_or(0, |s| s.doc.slides.iter().filter(|sl| figure::settings_of(sl).crop.is_some()).count());
    let mut open_crop_set = false;
    ui.horizontal(|ui| {
        if ui.button("Crop Set…").on_hover_text("Choose the area and resolution of each slide").clicked() {
            open_crop_set = true;
        }
        ui.label(egui::RichText::new(format!("{cropped} of {count} slides have a crop")).color(Tokens::get(ui.ctx()).text_dim));
    });
    if open_crop_set {
        let mut c = Dialog::new("cropSet");
        c.params = json!({"slide": current, "returnId": "figures", "returnParams": {"fields": d.fields}});
        app.dialog = Some(c);
        return true;
    }

    // What will be written, slide by slide, or why not.
    let req = request(d, count, current);
    let plans = req.as_ref().map_err(Clone::clone).and_then(|r| {
        let doc = app.session.active().map(|s| s.doc.clone()).ok_or("no presentation")?;
        figure::plan(&doc, current, r).map_err(|e| e.to_string())
    });
    let t = Tokens::get(ui.ctx());
    match &plans {
        Ok(list) => {
            egui::ScrollArea::vertical().max_height(120.0).id_salt("fig_plan").show(ui, |ui| {
                for p in list {
                    let full = (p.region.width() - app_slide_width(app)).abs() < 0.01 && (p.region.height() - app_slide_height(app)).abs() < 0.01;
                    ui.label(
                        egui::RichText::new(format!(
                            "Slide {}: {} × {} px at {:.0} dpi{}",
                            p.slide + 1,
                            p.width,
                            p.height,
                            p.dpi,
                            if full { "" } else { ", cropped" }
                        ))
                        .color(t.text_dim),
                    );
                }
            });
        }
        Err(e) => {
            ui.label(egui::RichText::new(e).color(t.text_dim));
        }
    }
    let (ok, cancel) = buttons(ui, "Export");
    if ok {
        let (Ok(params), Ok(plans)) = (req, plans) else {
            app.set_status("Fix the settings in the dialog first");
            return false;
        };
        start(app, d, &title, params, plans);
        return false;
    }
    cancel
}

fn app_slide_width(app: &SlideApp) -> f64 {
    app.session.active().map_or(0.0, |s| s.doc.slide_size.width)
}

fn app_slide_height(app: &SlideApp) -> f64 {
    app.session.active().map_or(0.0, |s| s.doc.slide_size.height)
}

/// Asks where to put the files (the desktop) and starts the job the progress screen runs.
fn start(app: &mut SlideApp, d: &mut Dialog, title: &str, params: Value, plans: Vec<Plan>) {
    let (mut dir, mut stem) = (None, title.to_string());
    if let Some(pick) = app.services.pick_save.as_mut() {
        let ext = figure::Format::parse(params["format"].as_str().unwrap_or("tiff")).map_or("tif", figure::Format::extension);
        let Some(path) = pick(&format!("{}.{ext}", figure::file_stem(title))) else { return };
        let p = std::path::Path::new(&path);
        dir = Some(p.parent().map(|x| x.to_string_lossy().to_string()).filter(|x| !x.is_empty()).unwrap_or_else(|| ".".into()));
        stem = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or(stem);
    }
    d.job = Some(Job { plans, params, dir, stem, done: Vec::new(), pending: false, failed: None });
}

/// The progress screen: which slide is being drawn and how many are done. True when finished.
fn progress(app: &mut SlideApp, ui: &mut Ui, d: &mut Dialog) -> bool {
    let t = Tokens::get(ui.ctx());
    let Some(job) = d.job.as_mut() else { return true };
    let (done, total) = (job.done.len(), job.plans.len());
    if let Some(e) = &job.failed {
        ui.label(egui::RichText::new(format!("The export stopped: {e}")).color(t.text));
        ui.label(egui::RichText::new(format!("{done} of {total} figures were written.")).color(t.text_dim));
        let (_, close) = buttons(ui, "Close");
        return close;
    }
    if done == total {
        let n = total;
        app.set_status(match &job.dir {
            Some(dir) => format!("Exported {n} figure{} to {dir}", if n == 1 { "" } else { "s" }),
            None => format!("Exported {n} figure{}", if n == 1 { "" } else { "s" }),
        });
        return true;
    }
    let next = job.plans.get(done).map(|p| p.slide + 1).unwrap_or(0);
    ui.label(format!("Exporting slide {next} ({} of {total})…", done + 1));
    ui.add_space(4.0);
    ui.add(egui::ProgressBar::new(done as f32 / total.max(1) as f32).desired_width(420.0).show_percentage());
    ui.label(egui::RichText::new("Large figures take a few seconds each.").color(t.text_dim));
    let (_, cancel) = buttons(ui, "Cancel export");
    if cancel {
        app.set_status(format!("Export cancelled: {done} of {total} figures were written"));
        return true;
    }
    ui.ctx().request_repaint();
    if !job.pending {
        job.pending = true;
        return false;
    }
    job.pending = false;
    let Some(doc) = app.session.active().map(|s| s.doc.clone()) else { return true };
    let Some(plan) = job.plans.get(done).cloned() else { return true };
    let result = match &job.dir {
        Some(dir) => figure::write_figure(&doc, &plan, &job.params, dir, &job.stem).map_err(|e| e.to_string()),
        None => {
            let bytes = figure::render_figure(&doc, &plan, &job.params);
            let name = figure::figure_name(&job.stem, doc.slides.len(), &plan);
            if app.download_file(&name, bytes) {
                Ok(json!({"slide": plan.slide}))
            } else {
                Err("the browser did not accept the download".to_string())
            }
        }
    };
    match (result, d.job.as_mut()) {
        (Ok(v), Some(job)) => job.done.push(v),
        (Err(e), Some(job)) => job.failed = Some(e),
        _ => {}
    }
    false
}
