use deckcraft_engine::cmd::figure;

use super::{Color32, Point, Rect, SlideApp, Stroke, Tokens, Xf, pos2, theme, vec2};

/// The crop saved with the current slide (Crop Set): the slide outside it dimmed, a frame around
/// it, and its size and resolution. It is for the editor only: slide shows and exports don't draw it.
pub(super) fn draw_crop_frame(app: &SlideApp, painter: &egui::Painter, xf: Xf, slide: Rect, t: &Tokens) {
    let Some(st) = app.session.active() else { return };
    let Some(sl) = st.doc.slides.get(st.selection.slide) else { return };
    let settings = figure::settings_of(sl);
    let Some(c) = settings.crop else { return };
    let (a, b) = (xf.to_screen(Point::new(c.x0, c.y0)), xf.to_screen(Point::new(c.x1, c.y1)));
    let f = Rect::from_two_pos(a, b).intersect(slide);
    if f.width() < 1.0 || f.height() < 1.0 {
        return;
    }
    let dim = Color32::from_black_alpha(90);
    for part in [
        Rect::from_min_max(slide.min, pos2(slide.max.x, f.top())),
        Rect::from_min_max(pos2(slide.min.x, f.bottom()), slide.max),
        Rect::from_min_max(pos2(slide.min.x, f.top()), pos2(f.left(), f.bottom())),
        Rect::from_min_max(pos2(f.right(), f.top()), pos2(slide.max.x, f.bottom())),
    ] {
        painter.rect_filled(part, 0.0, dim);
    }
    painter.rect_stroke(f, 0.0, Stroke::new(1.5, t.accent), egui::StrokeKind::Outside);
    let dpi = settings.dpi.unwrap_or(figure::DEFAULT_DPI);
    let label = format!("Crop {:.2} × {:.2} in · {dpi:.0} dpi", c.width() / 72.0, c.height() / 72.0);
    let pos = pos2(f.left() + 4.0, f.top() + 3.0);
    let galley = painter.layout_no_wrap(label, theme::font(11.0), Color32::WHITE);
    let bg = Rect::from_min_size(pos, galley.size() + vec2(8.0, 4.0)).translate(vec2(-2.0, -1.0));
    painter.rect_filled(bg, 2.0, t.accent);
    painter.galley(pos + vec2(2.0, 1.0), galley, Color32::WHITE);
}
