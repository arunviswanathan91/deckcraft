//! Crop Set: a print-setup style window for choosing, slide by slide, the area of the slide that
//! is exported as a figure and the resolution it is exported at. The slide is shown as it looks;
//! drag a frame over it (or type inches). Saving keeps the crops with the slides
//! (`slide.exportSettings`), where the canvas shows them and Export Slides as Figures uses them.

use deckcraft_engine::cmd::figure::{self, DEFAULT_DPI, MAX_DPI, MIN_DPI};
use egui::{Color32, Pos2, Rect, Sense, Stroke, StrokeKind, pos2, vec2};

use super::{Dialog, SlideApp, Tokens, Ui, Value, json};

/// The crop fields of a slide: left, top, width, height (inches).
const KEYS: [&str; 4] = ["l", "t", "w", "h"];
/// Width of the slide preview, in points of the window.
const PREVIEW_W: f32 = 520.0;
const MIN_SIDE: f64 = 0.1;
const HANDLE: f32 = 8.0;

fn num(d: &Dialog, key: &str) -> Option<f64> {
    d.fields.get(key).and_then(|t| t.trim().trim_end_matches(['"', ' ']).parse::<f64>().ok()).filter(|v| v.is_finite())
}

/// The crop of slide `i` as (left, top, width, height) in inches, when all four are set.
fn crop_in(d: &Dialog, i: usize) -> Option<[f64; 4]> {
    let v: Vec<f64> = KEYS.iter().map(|k| num(d, &format!("c{i}{k}"))).collect::<Option<_>>()?;
    let a = <[f64; 4]>::try_from(v).ok()?;
    (a[2] > 0.0 && a[3] > 0.0).then_some(a)
}

fn set_crop(d: &mut Dialog, i: usize, c: Option<[f64; 4]>) {
    for (n, k) in KEYS.iter().enumerate() {
        let key = format!("c{i}{k}");
        match c {
            Some(a) => d.fields.insert(key, format!("{}", (a[n] * 100.0).round() / 100.0)),
            None => d.fields.remove(&key),
        };
    }
}

/// The fields hold what is saved with the slides when the window opens.
fn seed(app: &SlideApp, d: &mut Dialog) {
    if d.fields.contains_key("seeded") {
        return;
    }
    d.fields.insert("seeded".into(), "1".into());
    let current = app.session.active().map_or(0, |s| s.selection.slide);
    d.fields.insert("sel".into(), d.params.get("slide").and_then(Value::as_u64).map_or(current, |v| v as usize).to_string());
    let Some(doc) = app.session.active().map(|s| s.doc.clone()) else { return };
    for (i, sl) in doc.slides.iter().enumerate() {
        let s = figure::settings_of(sl);
        if let Some(dpi) = s.dpi {
            d.fields.insert(format!("dpi{i}"), format!("{dpi}"));
        }
        if let Some(c) = s.crop {
            set_crop(d, i, Some([c.x0 / 72.0, c.y0 / 72.0, c.width() / 72.0, c.height() / 72.0]));
        }
    }
}

/// What the frame is grabbed by.
#[derive(Clone, Copy, PartialEq)]
enum Grab {
    New,
    Move,
    Edges { l: bool, r: bool, t: bool, b: bool },
}

fn grab_at(frame: Option<Rect>, p: Pos2) -> Grab {
    let Some(f) = frame else { return Grab::New };
    let near = |a: f32, b: f32| (a - b).abs() <= HANDLE;
    let inside_x = p.x >= f.left() - HANDLE && p.x <= f.right() + HANDLE;
    let inside_y = p.y >= f.top() - HANDLE && p.y <= f.bottom() + HANDLE;
    let (l, r) = (near(p.x, f.left()) && inside_y, near(p.x, f.right()) && inside_y);
    let (t, b) = (near(p.y, f.top()) && inside_x, near(p.y, f.bottom()) && inside_x);
    if l || r || t || b {
        Grab::Edges { l, r, t, b }
    } else if f.contains(p) {
        Grab::Move
    } else {
        Grab::New
    }
}

/// The preview of slide `i` as a texture (kept while the same slide is shown).
fn preview(app: &SlideApp, ui: &Ui, d: &mut Dialog, i: usize) -> Option<egui::TextureId> {
    if let Some(super::SlideTexture(k, tex)) = &d.texture
        && *k == i
    {
        return Some(tex.id());
    }
    let doc = app.session.active()?.doc.clone();
    let scale = f64::from(PREVIEW_W) * 2.0 / doc.slide_size.width.max(1.0);
    let img = deckcraft_render::render_slide(&doc, i, &deckcraft_render::RenderOpts { scale, ..Default::default() });
    if img.width == 0 || img.height == 0 {
        return None;
    }
    let color = egui::ColorImage::from_rgba_premultiplied([img.width as usize, img.height as usize], &img.pixels);
    let tex = ui.ctx().load_texture(format!("cropset-{i}"), color, egui::TextureOptions::LINEAR);
    let id = tex.id();
    d.texture = Some(super::SlideTexture(i, tex));
    Some(id)
}

/// The slide with its crop frame; dragging draws, moves or resizes the frame.
fn sketch(app: &SlideApp, ui: &mut Ui, d: &mut Dialog, i: usize, (sw, sh): (f64, f64)) {
    let t = Tokens::get(ui.ctx());
    let (w_in, h_in) = (sw / 72.0, sh / 72.0);
    let height = (f64::from(PREVIEW_W) * sh / sw.max(1.0)) as f32;
    let (resp, painter) = ui.allocate_painter(vec2(PREVIEW_W, height.clamp(40.0, 420.0)), Sense::click_and_drag());
    let r = resp.rect;
    painter.rect_filled(r, 0.0, Color32::WHITE);
    if let Some(id) = preview(app, ui, d, i) {
        painter.image(id, r, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), Color32::WHITE);
    }
    let to_screen = |x: f64, y: f64| pos2(r.left() + (x / w_in) as f32 * r.width(), r.top() + (y / h_in) as f32 * r.height());
    let to_in = |p: Pos2| {
        (
            (f64::from(p.x - r.left()) / f64::from(r.width()) * w_in).clamp(0.0, w_in),
            (f64::from(p.y - r.top()) / f64::from(r.height()) * h_in).clamp(0.0, h_in),
        )
    };
    let frame_of = |c: [f64; 4]| Rect::from_min_max(to_screen(c[0], c[1]), to_screen(c[0] + c[2], c[1] + c[3])).intersect(r);
    let frame = crop_in(d, i).map(frame_of);

    if resp.drag_started()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let (x, y) = to_in(p);
        let grab = grab_at(frame, p);
        let tag = match grab {
            Grab::New => "n".to_string(),
            Grab::Move => "m".to_string(),
            Grab::Edges { l, r, t, b } => format!("e{}{}{}{}", u8::from(l), u8::from(r), u8::from(t), u8::from(b)),
        };
        d.fields.insert("grab".into(), tag);
        d.fields.insert("gx".into(), x.to_string());
        d.fields.insert("gy".into(), y.to_string());
        let orig = crop_in(d, i).unwrap_or([x, y, 0.0, 0.0]);
        d.fields.insert("orig".into(), orig.iter().map(f64::to_string).collect::<Vec<_>>().join(","));
    }
    if resp.dragged()
        && let (Some(p), Some(grab)) = (resp.interact_pointer_pos(), d.fields.get("grab").cloned())
    {
        let (x, y) = to_in(p);
        let o: Vec<f64> = d.fields.get("orig").map(|s| s.split(',').filter_map(|v| v.parse().ok()).collect()).unwrap_or_default();
        let (gx, gy) = (num(d, "gx").unwrap_or(x), num(d, "gy").unwrap_or(y));
        if let [ol, ot, ow, oh] = o[..] {
            let (mut l, mut tp, mut rr, mut bb) = (ol, ot, ol + ow, ot + oh);
            match grab.as_str() {
                "n" => (l, tp, rr, bb) = (gx.min(x), gy.min(y), gx.max(x), gy.max(y)),
                "m" => {
                    let dx = (x - gx).clamp(-ol, w_in - ol - ow);
                    let dy = (y - gy).clamp(-ot, h_in - ot - oh);
                    (l, tp, rr, bb) = (ol + dx, ot + dy, ol + dx + ow, ot + dy + oh);
                }
                e => {
                    let f: Vec<bool> = e.chars().skip(1).map(|c| c == '1').collect();
                    if let [el, er, et, eb] = f[..] {
                        if el {
                            l = x.min(rr - MIN_SIDE);
                        }
                        if er {
                            rr = x.max(l + MIN_SIDE);
                        }
                        if et {
                            tp = y.min(bb - MIN_SIDE);
                        }
                        if eb {
                            bb = y.max(tp + MIN_SIDE);
                        }
                    }
                }
            }
            if rr - l > 0.0 && bb - tp > 0.0 {
                set_crop(d, i, Some([l, tp, rr - l, bb - tp]));
            }
        }
    }

    if let Some(c) = crop_in(d, i) {
        let f = frame_of(c);
        let dim = Color32::from_black_alpha(120);
        for part in [
            Rect::from_min_max(r.min, pos2(r.right(), f.top())),
            Rect::from_min_max(pos2(r.left(), f.bottom()), r.max),
            Rect::from_min_max(pos2(r.left(), f.top()), pos2(f.left(), f.bottom())),
            Rect::from_min_max(pos2(f.right(), f.top()), pos2(r.right(), f.bottom())),
        ] {
            painter.rect_filled(part, 0.0, dim);
        }
        painter.rect_stroke(f, 0.0, Stroke::new(2.0, t.accent), StrokeKind::Middle);
        for p in
            [f.left_top(), f.center_top(), f.right_top(), f.left_center(), f.right_center(), f.left_bottom(), f.center_bottom(), f.right_bottom()]
        {
            let h = Rect::from_center_size(p, vec2(HANDLE, HANDLE));
            painter.rect_filled(h, 1.0, Color32::WHITE);
            painter.rect_stroke(h, 1.0, Stroke::new(1.0, t.accent), StrokeKind::Middle);
        }
    }
    painter.rect_stroke(r, 0.0, Stroke::new(1.0, t.text_dim), StrokeKind::Inside);
}

fn dpi_of(d: &Dialog, i: usize) -> Option<f64> {
    num(d, &format!("dpi{i}")).filter(|v| (MIN_DPI..=MAX_DPI).contains(v))
}

/// The settings of every slide as `slide.exportSettings` entries, or why they can't be saved.
pub(super) fn entries(d: &Dialog, count: usize) -> Result<Vec<Value>, String> {
    (0..count)
        .map(|i| {
            if d.fields.get(&format!("dpi{i}")).is_some_and(|t| !t.trim().is_empty()) && dpi_of(d, i).is_none() {
                return Err(format!("Slide {}: resolution is {MIN_DPI} to {MAX_DPI} dpi", i + 1));
            }
            let partial = KEYS.iter().filter(|k| num(d, &format!("c{i}{k}")).is_some()).count();
            if partial != 0 && crop_in(d, i).is_none() {
                return Err(format!("Slide {}: the crop needs a Left, Top, Width and Height, with a width and height over 0", i + 1));
            }
            let crop = crop_in(d, i).map(|c| json!({"x": c[0] * 72.0, "y": c[1] * 72.0, "width": c[2] * 72.0, "height": c[3] * 72.0}));
            Ok(json!({"slide": i, "dpi": dpi_of(d, i), "crop": crop}))
        })
        .collect()
}

pub(super) fn crop_set_dialog(app: &mut SlideApp, ui: &mut Ui, d: &mut Dialog) -> bool {
    seed(app, d);
    let Some((count, size, titles)) = app.session.active().map(|s| {
        let titles: Vec<String> = s.doc.slides.iter().map(|sl| sl.title().lines().next().unwrap_or("").trim().to_string()).collect();
        (s.doc.slides.len(), (s.doc.slide_size.width, s.doc.slide_size.height), titles)
    }) else {
        return true;
    };
    let sel = num(d, "sel").map_or(0, |v| v as usize).min(count.saturating_sub(1));
    let t = Tokens::get(ui.ctx());
    ui.label("Choose the part of each slide to export and the resolution it is exported at. The resolution applies to the cropped area.");
    ui.add_space(4.0);
    ui.horizontal_top(|ui| {
        ui.vertical(|ui| {
            ui.set_width(150.0);
            egui::ScrollArea::vertical().max_height(400.0).id_salt("crop_slides").show(ui, |ui| {
                for i in 0..count {
                    let mark = if crop_in(d, i).is_some() { "  ✂" } else { "" };
                    let title = titles.get(i).map(String::as_str).unwrap_or("");
                    let label = format!("{}  {}{mark}", i + 1, if title.is_empty() { "(no title)" } else { title });
                    if ui.selectable_label(i == sel, egui::RichText::new(label)).clicked() {
                        d.fields.insert("sel".into(), i.to_string());
                    }
                }
            });
        });
        ui.vertical(|ui| {
            sketch(app, ui, d, sel, size);
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                for (k, label) in KEYS.iter().zip(["Left", "Top", "Width", "Height"]) {
                    ui.label(label);
                    let mut v = d.fields.get(&format!("c{sel}{k}")).cloned().unwrap_or_default();
                    ui.add(egui::TextEdit::singleline(&mut v).desired_width(48.0));
                    if v.trim().is_empty() {
                        d.fields.remove(&format!("c{sel}{k}"));
                    } else {
                        d.fields.insert(format!("c{sel}{k}"), v);
                    }
                }
                ui.label("inches");
            });
            ui.horizontal(|ui| {
                ui.label("Resolution");
                let mut v = d.fields.get(&format!("dpi{sel}")).cloned().unwrap_or_default();
                ui.add(egui::TextEdit::singleline(&mut v).desired_width(60.0).hint_text("default"));
                d.fields.insert(format!("dpi{sel}"), v);
                ui.label("dpi");
                if ui.button("Full slide").on_hover_text("Remove this slide's crop").clicked() {
                    set_crop(d, sel, None);
                }
                if ui.button("Copy to all slides").on_hover_text("Give every slide this crop and resolution").clicked() {
                    let (c, dpi) = (crop_in(d, sel), d.fields.get(&format!("dpi{sel}")).cloned());
                    for i in 0..count {
                        set_crop(d, i, c);
                        match &dpi {
                            Some(v) => d.fields.insert(format!("dpi{i}"), v.clone()),
                            None => d.fields.remove(&format!("dpi{i}")),
                        };
                    }
                }
            });
            // What this slide becomes: the pixels of the cropped area, not of the whole slide.
            let c = crop_in(d, sel).unwrap_or([0.0, 0.0, size.0 / 72.0, size.1 / 72.0]);
            let dpi = dpi_of(d, sel).unwrap_or(DEFAULT_DPI);
            let region = deckcraft_geom::Rect::new(c[0] * 72.0, c[1] * 72.0, (c[0] + c[2]) * 72.0, (c[1] + c[3]) * 72.0);
            let text = match figure::figure_size(sel, region.intersect(deckcraft_geom::Rect::new(0.0, 0.0, size.0, size.1)), dpi) {
                Ok((w, h)) => format!("Slide {}: {:.2} × {:.2} in at {dpi:.0} dpi = {w} × {h} pixels", sel + 1, c[2], c[3]),
                Err(e) => e.to_string(),
            };
            ui.label(egui::RichText::new(text).color(t.text_dim));
            ui.label(
                egui::RichText::new(format!(
                    "Slide size {:.2} × {:.2} in. Blank resolution: the default chosen when exporting ({DEFAULT_DPI:.0} dpi).",
                    size.0 / 72.0,
                    size.1 / 72.0
                ))
                .color(t.text_dim),
            );
        });
    });
    let list = entries(d, count);
    if let Err(e) = &list {
        ui.label(egui::RichText::new(e).color(t.text_dim));
    }
    let (ok, cancel) = super::buttons(ui, "Save");
    if ok {
        let Ok(list) = list else {
            app.set_status("Fix the crop settings first");
            return false;
        };
        let changed: Vec<Value> = app
            .session
            .active()
            .map(|s| {
                list.iter()
                    .filter(|e| {
                        let i = e["slide"].as_u64().unwrap_or(0) as usize;
                        let old = s.doc.slides.get(i).map(|sl| figure::settings_of(sl)).unwrap_or_default();
                        let new_dpi = e["dpi"].as_f64();
                        let new_crop = e.get("crop").filter(|c| c.is_object()).map(|c| {
                            let g = |k: &str| c.get(k).and_then(Value::as_f64).unwrap_or(f64::NAN);
                            [g("x"), g("y"), g("width"), g("height")]
                        });
                        let old_crop = old.crop.map(|c| [c.x0, c.y0, c.width(), c.height()]);
                        let same_crop = match (new_crop, old_crop) {
                            (None, None) => true,
                            (Some(a), Some(b)) => a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6),
                            _ => false,
                        };
                        new_dpi != old.dpi || !same_crop
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if !changed.is_empty() && app.run("slide.exportSettings", json!({"settings": changed})).is_err() {
            return false;
        }
        app.set_status("Crop set saved with the slides");
        return true;
    }
    cancel
}
