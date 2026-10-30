use super::*;

/// Largest pixmap side we render (vello_cpu uses u16 sizes).
pub const MAX_SIDE: u32 = 16_000;

/// A rendered image: premultiplied RGBA8.
#[derive(Clone, Debug, Default)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

impl Image {
    /// Straight (unpremultiplied) RGBA.
    pub fn to_straight(&self) -> Vec<u8> {
        let mut out = self.pixels.clone();
        for px in out.as_chunks_mut::<4>().0 {
            let a = px.get(3).copied().unwrap_or(255) as u32;
            if a != 0 && a != 255 {
                for c in px.iter_mut().take(3) {
                    *c = ((*c as u32 * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
        out
    }
    /// One pixel as straight (unpremultiplied) RGBA; `None` outside the image.
    pub fn straight_at(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = (y as usize).checked_mul(self.width as usize)?.checked_add(x as usize)?.checked_mul(4)?;
        let [r, g, b, a] = <[u8; 4]>::try_from(self.pixels.get(i..i.checked_add(4)?)?).ok()?;
        if a == 0 || a == 255 {
            return Some([r, g, b, a]);
        }
        let unpremultiply = |c: u8| ((c as u32 * 255 + a as u32 / 2) / a as u32).min(255) as u8;
        Some([unpremultiply(r), unpremultiply(g), unpremultiply(b), a])
    }
    pub fn to_png(&self) -> Vec<u8> {
        let mut px = self.to_straight();
        px.resize(self.width as usize * self.height as usize * 4, 0);
        let Some(img) = image::RgbaImage::from_raw(self.width, self.height, px) else { return Vec::new() };
        let mut buf = Vec::new();
        if let Err(e) = img.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png) {
            log::error!("PNG encode: {e}");
            return Vec::new();
        }
        buf
    }

    /// PNG of this image whose `pHYs` chunk says `dpi` dots per inch.
    ///
    /// Slide space is points, 72 per inch, so a render at `scale` pixels per point is
    /// [`dpi_for_scale`]`(scale)`. Without the chunk, Preview treats the pixels as 72 dpi and a
    /// Retina (2×) export opens at twice the slide's size.
    pub fn to_png_at(&self, dpi: f64) -> Vec<u8> {
        images::stamp_png_dpi(&self.to_png(), dpi)
    }

    pub fn to_jpeg(&self, quality: u8) -> Vec<u8> {
        self.to_jpeg_at(quality, None)
    }

    /// JPEG of this image. `dpi` is stored as JFIF density in dots per inch when it is in range.
    pub fn to_jpeg_at(&self, quality: u8, dpi: Option<f64>) -> Vec<u8> {
        let straight = self.to_straight();
        let rgb: Vec<u8> = straight.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
        let Some(img) = image::RgbImage::from_raw(self.width, self.height, rgb) else { return Vec::new() };
        let mut buf = Vec::new();
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality.clamp(1, 100));
        if let Some(d) = dpi.and_then(dpi_u16) {
            enc.set_pixel_density(image::codecs::jpeg::PixelDensity::dpi(d));
        }
        if let Err(e) = img.write_with_encoder(enc) {
            log::error!("JPEG encode: {e}");
            return Vec::new();
        }
        buf
    }
    /// This image with its pixels unpremultiplied in place (no second copy: publication-size
    /// figures run to hundreds of megabytes). `opaque` drops the transparency by compositing onto
    /// white, for formats without an alpha channel.
    fn into_straight(mut self, opaque: bool) -> Image {
        for px in self.pixels.as_chunks_mut::<4>().0 {
            let a = u32::from(px[3]);
            if opaque {
                // Premultiplied over white: the colour plus the white that shows through.
                let k = 255 - px[3];
                for c in px.iter_mut().take(3) {
                    *c = c.saturating_add(k);
                }
                px[3] = 255;
            } else if a != 0 && a != 255 {
                for c in px.iter_mut().take(3) {
                    *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
                }
            }
        }
        self
    }

    /// [`Image::to_png_at`] consuming the image; `opaque` flattens it onto white (an alpha channel
    /// is kept otherwise).
    pub fn into_png_at(self, dpi: f64, opaque: bool) -> Vec<u8> {
        let img = self.into_straight(opaque);
        let (w, h) = (img.width, img.height);
        let mut buf = Vec::new();
        let res = if opaque {
            let rgb: Vec<u8> = img.pixels.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
            drop(img);
            match image::RgbImage::from_raw(w, h, rgb) {
                Some(i) => i.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png),
                None => return Vec::new(),
            }
        } else {
            match image::RgbaImage::from_raw(w, h, img.pixels) {
                Some(i) => i.write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png),
                None => return Vec::new(),
            }
        };
        if let Err(e) = res {
            log::error!("PNG encode: {e}");
            return Vec::new();
        }
        images::stamp_png_dpi(&buf, dpi)
    }

    /// [`Image::to_jpeg_at`] consuming the image (transparency flattened onto white).
    pub fn into_jpeg_at(self, quality: u8, dpi: Option<f64>) -> Vec<u8> {
        let img = self.into_straight(true);
        let (w, h) = (img.width, img.height);
        let rgb: Vec<u8> = img.pixels.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
        drop(img);
        let Some(img) = image::RgbImage::from_raw(w, h, rgb) else { return Vec::new() };
        let mut buf = Vec::new();
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, quality.clamp(1, 100));
        if let Some(d) = dpi.and_then(dpi_u16) {
            enc.set_pixel_density(image::codecs::jpeg::PixelDensity::dpi(d));
        }
        if let Err(e) = img.write_with_encoder(enc) {
            log::error!("JPEG encode: {e}");
            return Vec::new();
        }
        buf
    }

    /// A TIFF of this image with LZW compression, recording `dpi` (inch units) in its
    /// resolution tags, the way journals ask for figures. RGB, or RGBA when `opaque` is false.
    pub fn into_tiff_at(self, dpi: f64, opaque: bool) -> Vec<u8> {
        use tiff::encoder::{Compression, Rational, TiffEncoder, colortype};
        use tiff::tags::ResolutionUnit;
        let img = self.into_straight(opaque);
        let (w, h) = (img.width, img.height);
        let res = Rational { n: dpi.clamp(1.0, 100_000.0).round() as u32, d: 1 };
        let mut buf = std::io::Cursor::new(Vec::new());
        let done = (|| -> Result<(), tiff::TiffError> {
            let mut enc = TiffEncoder::new(&mut buf)?.with_compression(Compression::Lzw);
            if opaque {
                let rgb: Vec<u8> = img.pixels.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect();
                drop(img);
                let mut im = enc.new_image::<colortype::RGB8>(w, h)?;
                im.resolution(ResolutionUnit::Inch, res);
                im.write_data(&rgb)
            } else {
                let mut im = enc.new_image::<colortype::RGBA8>(w, h)?;
                im.resolution(ResolutionUnit::Inch, res);
                im.write_data(&img.pixels)
            }
        })();
        match done {
            Ok(()) => buf.into_inner(),
            Err(e) => {
                log::error!("TIFF encode: {e}");
                Vec::new()
            }
        }
    }

    /// Pixel at (x, y) as straight RGBA.
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let i = (y as usize * self.width as usize + x as usize) * 4;
        let p = self.pixels.get(i..i + 4).unwrap_or(&[0, 0, 0, 0]);
        let a = p.get(3).copied().unwrap_or(0) as u32;
        let un = |c: u8| (c as u32 * 255 + a / 2).checked_div(a).map(|v| v.min(255) as u8).unwrap_or(0);
        [un(p[0]), un(p[1]), un(p[2]), a as u8]
    }
}

/// Dots per inch for a slide drawn at `pixels_per_point`.
///
/// A point is 1/72 inch. The default picture export uses 2 pixels per point, which is 144 dpi:
/// the same pixels, at the slide's real size, on a Retina display.
pub fn dpi_for_scale(pixels_per_point: f64) -> f64 {
    if !pixels_per_point.is_finite() || pixels_per_point <= 0.0 {
        return 72.0;
    }
    (pixels_per_point * 72.0).clamp(1.0, 10_000.0)
}

fn dpi_u16(dpi: f64) -> Option<u16> {
    if !dpi.is_finite() || !(1.0..=f64::from(u16::MAX)).contains(&dpi) {
        return None;
    }
    Some(dpi.round() as u16)
}

/// Resolution a PNG (`pHYs`) or JPEG (JFIF) records, in whole dots per inch.
pub fn image_dpi(bytes: &[u8]) -> Option<(f64, f64)> {
    images::dpi(bytes)
}
