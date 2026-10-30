//! A rectangle of a slide at any resolution (publication figures: 300–2400 dpi, cropped).
//!
//! One render is limited to [`MAX_SIDE`] pixels a side, so a region is drawn in tiles, each with a
//! margin of extra pixels that is thrown away, so blurs and shadows next to a tile's edge see the
//! shapes beyond it, and copied into one image.

use super::opts::RenderOpts;
use super::raster::Image;
use super::slide::render_slide;
use super::{Presentation, Rect};

/// Core side of a tile, in pixels.
const TILE_CORE: u32 = 4096;
/// Extra pixels drawn around a tile's core and discarded.
const TILE_MARGIN: u32 = 96;

/// The pixels of `region` (slide points) of slide `index` drawn at `scale` pixels per point, or
/// `None` when the region is empty, the scale is not usable or the image would not fit in memory
/// (more than `max_pixels` pixels). The image is `round(width × scale)` by `round(height × scale)`
/// and `transparent` leaves the slide background out.
pub fn render_region(pres: &Presentation, index: usize, region: Rect, scale: f64, transparent: bool, max_pixels: u64) -> Option<Image> {
    if !(scale.is_finite() && scale > 0.0) || !(region.x0.is_finite() && region.y0.is_finite() && region.x1.is_finite() && region.y1.is_finite()) {
        return None;
    }
    let ox = (region.x0 * scale).round();
    let oy = (region.y0 * scale).round();
    let w = (region.width() * scale).round().max(1.0);
    let h = (region.height() * scale).round().max(1.0);
    if !(ox.abs() < 1e9 && oy.abs() < 1e9 && w < 1e9 && h < 1e9) || (w * h) as u64 > max_pixels {
        return None;
    }
    let (ox, oy, w, h) = (ox as i64, oy as i64, w as u32, h as u32);
    let mut out = Image { width: w, height: h, pixels: vec![0; (w as usize).checked_mul(h as usize)?.checked_mul(4)?] };
    let mut ty = 0;
    while ty < h {
        let ch = TILE_CORE.min(h - ty);
        let mut tx = 0;
        while tx < w {
            let cw = TILE_CORE.min(w - tx);
            // The margin is clipped at the output's edges, where nothing more is needed.
            let (ml, mt) = (TILE_MARGIN.min(tx), TILE_MARGIN.min(ty));
            let (mr, mb) = (TILE_MARGIN.min(w - tx - cw), TILE_MARGIN.min(h - ty - ch));
            let (tw, th) = (ml + cw + mr, mt + ch + mb);
            let opts = RenderOpts {
                scale,
                size: Some((tw, th)),
                offset: (-((ox + i64::from(tx) - i64::from(ml)) as f64), -((oy + i64::from(ty) - i64::from(mt)) as f64)),
                transparent_background: transparent,
                ..Default::default()
            };
            let tile = render_slide(pres, index, &opts);
            if tile.width != tw || tile.height != th {
                return None;
            }
            for row in 0..ch {
                let src = ((mt + row) as usize * tw as usize + ml as usize) * 4;
                let dst = ((ty + row) as usize * w as usize + tx as usize) * 4;
                let n = cw as usize * 4;
                out.pixels.get_mut(dst..dst + n)?.copy_from_slice(tile.pixels.get(src..src + n)?);
            }
            tx += cw;
        }
        ty += ch;
    }
    Some(out)
}
