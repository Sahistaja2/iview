//! Ctrl+S: koopia kettale. Originaal
//! täissuuruses lahti (RAW-ist kaamera JPEG), peegel ja pööre, pikem külg
//! Lanczos3-ga (SIMD), EXIF ja värviprofiil kaasa, EXIF-i pööre nulli
//! (pikslid on juba õiget pidi), faili aeg originaalilt. Kirjutatakse
//! `.part` faili ja see saab lõpliku nime alles valmis kujul.

use std::path::Path;
use std::time::SystemTime;

/// Pildi muudatus aknas (faili ei puututa): peegel (enne), pööre
/// veerandpöördeid päripäeva (pärast), lõige, pikem külg.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Edit {
    pub flip_h: bool,
    pub rot: u8,
    pub crop: Option<Crop>,
    pub long_side: Option<u32>,
}

/// Lõige peeglitud ja pööratud (ekraanil paistva) pildi pikslites; `base`:
/// selle pildi mõõt lõikamata. Pööre ja peegel teisendavad lõiget kaasa.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Crop {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub base: (u32, u32),
}

impl Crop {
    fn rotated(self, clockwise: bool) -> Self {
        let (bw, bh) = self.base;
        let (x, y) = if clockwise { (bh - (self.y + self.h), self.x) } else { (self.y, bw - (self.x + self.w)) };
        Crop { x, y, w: self.h, h: self.w, base: (bh, bw) }
    }

    fn flipped_h(self) -> Self {
        Crop { x: self.base.0 - (self.x + self.w), ..self }
    }

    fn flipped_v(self) -> Self {
        Crop { y: self.base.1 - (self.y + self.h), ..self }
    }

    /// Sama osa pildil mõõduga `dims` (vähendatult loetud pildil teine
    /// mõõt kui `base`): x, y, laius, kõrgus — vähemalt 1 piksel, pildi sees.
    fn on(self, (w, h): (u32, u32)) -> (u32, u32, u32, u32) {
        if (w, h) == self.base {
            return (self.x, self.y, self.w, self.h);
        }
        let axis = |pos: u32, len: u32, base: u32, size: u32| {
            let s = f64::from(size) / f64::from(base.max(1));
            let a = ((f64::from(pos) * s).round() as u32).min(size.saturating_sub(1));
            let b = ((f64::from(pos + len) * s).round() as u32).clamp(a + 1, size.max(a + 1));
            (a, b - a)
        };
        let (x, cw) = axis(self.x, self.w, self.base.0, w);
        let (y, ch) = axis(self.y, self.h, self.base.1, h);
        (x, y, cw, ch)
    }
}

impl Edit {
    pub fn is_empty(self) -> bool {
        self == Edit::default()
    }

    /// Peegel ekraanil horisontaalselt: F ∘ R^k ∘ F^f = R^(−k) ∘ F^(f+1).
    pub fn flip_h(self) -> Self {
        Edit { flip_h: !self.flip_h, rot: (4 - self.rot) % 4, crop: self.crop.map(Crop::flipped_h), ..self }
    }

    /// Peegel vertikaalselt = pööre 180° ja horisontaalne peegel.
    pub fn flip_v(self) -> Self {
        Edit { flip_h: !self.flip_h, rot: (6 - self.rot) % 4, crop: self.crop.map(Crop::flipped_v), ..self }
    }

    pub fn rotate(self, clockwise: bool) -> Self {
        Edit { rot: (self.rot + if clockwise { 1 } else { 3 }) % 4, crop: self.crop.map(|c| c.rotated(clockwise)), ..self }
    }

    /// Mõõdud pärast muudatust (pööre, lõige, pikem külg) originaali mõõtudest.
    pub fn dims(self, orig: (u32, u32)) -> (u32, u32) {
        let turned = if self.rot % 2 == 1 { (orig.1, orig.0) } else { orig };
        let (w, h) = match self.crop {
            Some(c) => {
                let (_, _, w, h) = c.on(turned);
                (w, h)
            }
            None => turned,
        };
        match self.long_side {
            Some(side) => fit_dims((w, h), side),
            None => (w, h),
        }
    }

    /// Valik (osad 0..1 praegu paistvast pildist: x0, y0, x1, y1) uueks
    /// lõikeks. Vähendatud pilt jääb samasse mõõtkavasse (pikem külg
    /// väheneb lõikega kaasa).
    pub fn cropped(self, orig: (u32, u32), sel: [f32; 4]) -> Self {
        let base = if self.rot % 2 == 1 { (orig.1, orig.0) } else { orig };
        let (cx, cy, cw, ch) = self.crop.map_or((0, 0, base.0, base.1), |c| c.on(base));
        let at = |f: f32, len: u32| (f64::from(f.clamp(0.0, 1.0)) * f64::from(len)).round() as u32;
        let x0 = (cx + at(sel[0], cw)).min(base.0.saturating_sub(1));
        let y0 = (cy + at(sel[1], ch)).min(base.1.saturating_sub(1));
        let w = (cx + at(sel[2], cw)).saturating_sub(x0).clamp(1, base.0 - x0);
        let h = (cy + at(sel[3], ch)).saturating_sub(y0).clamp(1, base.1 - y0);
        let long_side = self.long_side.map(|side| ((f64::from(side) * f64::from(w.max(h)) / f64::from(cw.max(ch).max(1))).round() as u32).max(1));
        Edit { crop: Some(Crop { x: x0, y: y0, w, h, base }), long_side, ..self }
    }

    /// Paistev osa lõikamata pildist (osad 0..1: x0, y0, x1, y1).
    pub fn crop_fractions(self) -> [f32; 4] {
        match self.crop {
            Some(c) => {
                let (bw, bh) = (c.base.0.max(1) as f32, c.base.1.max(1) as f32);
                [c.x as f32 / bw, c.y as f32 / bh, (c.x + c.w) as f32 / bw, (c.y + c.h) as f32 / bh]
            }
            None => [0.0, 0.0, 1.0, 1.0],
        }
    }

    /// Mitu korda suurem on lõikamata pilt paistvast osast (tekstuur peab
    /// olema nii palju teravam).
    pub fn crop_magnification(self) -> f32 {
        let [x0, y0, x1, y1] = self.crop_fractions();
        (1.0 / (x1 - x0).max(1e-4)).max(1.0 / (y1 - y0).max(1e-4))
    }

    /// Ekraanil paistva pildi punkt (osad 0..1) → koht failis loetud pildil
    /// (tekstuuril): lõige, pööre ja peegel tagasi.
    pub fn source_point(self, u: f32, v: f32) -> (f32, f32) {
        let [x0, y0, x1, y1] = self.crop_fractions();
        let (u, v) = (x0 + u * (x1 - x0), y0 + v * (y1 - y0));
        let (mut s, t) = match self.rot % 4 {
            1 => (v, 1.0 - u),
            2 => (1.0 - u, 1.0 - v),
            3 => (1.0 - v, u),
            _ => (u, v),
        };
        if self.flip_h {
            s = 1.0 - s;
        }
        (s, t)
    }

    /// Pikslitele: peegel enne, pööre pärast — sama järjekord mis ekraanil —,
    /// siis lõige.
    pub fn apply(self, mut image: image::DynamicImage) -> image::DynamicImage {
        if self.flip_h {
            image = image.fliph();
        }
        let image = match self.rot % 4 {
            1 => image.rotate90(),
            2 => image.rotate180(),
            3 => image.rotate270(),
            _ => image,
        };
        match self.crop {
            Some(c) => {
                let (x, y, w, h) = c.on((image.width(), image.height()));
                image.crop_imm(x, y, w, h)
            }
            None => image,
        }
    }

    /// Riba tekst: „90° · mirrored · cropped".
    pub fn describe(self) -> Vec<String> {
        let mut parts = Vec::new();
        if self.rot != 0 {
            parts.push(format!("{}°", u32::from(self.rot) * 90));
        }
        if self.flip_h {
            parts.push("mirrored".to_string());
        }
        if self.crop.is_some() {
            parts.push("cropped".to_string());
        }
        parts
    }
}

/// Osa pildist (osad 0..1: x0, y0, x1, y1), vähemalt 1 piksel.
pub fn crop_part(image: image::DynamicImage, part: [f32; 4]) -> image::DynamicImage {
    let (w, h) = (image.width(), image.height());
    let crop = Edit::default().cropped((w, h), part).crop.unwrap_or(Crop { x: 0, y: 0, w, h, base: (w, h) });
    image.crop_imm(crop.x, crop.y, crop.w, crop.h)
}

pub fn paste_part(image: image::DynamicImage, pasted: image::DynamicImage, part: [f32; 4]) -> image::DynamicImage {
    let dims = (image.width(), image.height());
    let crop = Edit::default().cropped(dims, part).crop.expect("cropped sets the bounds");
    let pasted = pasted.resize_exact(crop.w, crop.h, image::imageops::FilterType::Lanczos3).into_rgba8();
    let mut rgba = image.into_rgba8();
    image::imageops::overlay(&mut rgba, &pasted, i64::from(crop.x), i64::from(crop.y));
    image::DynamicImage::ImageRgba8(rgba)
}

/// Pikem külg `side` kasti, proportsioon jääb (ka suuremaks, kui küsitakse).
pub fn fit_dims((w, h): (u32, u32), side: u32) -> (u32, u32) {
    if w == 0 || h == 0 {
        return (w, h);
    }
    let scale = f64::from(side) / f64::from(w.max(h));
    (((f64::from(w) * scale).round() as u32).max(1), ((f64::from(h) * scale).round() as u32).max(1))
}

/// Pilt muudatusega: lahti (pikem külg kuni kahekordne soovitud — dekooder
/// vähendab kiiresti, Lanczos teeb lõpu), peegel, pööre, pikem külg.
pub fn edited(source: &Path, memory: Option<&image::DynamicImage>, edit: Edit) -> Result<image::DynamicImage, String> {
    // Lõikega täissuuruses: väike osa vähendatud pildist oleks udune.
    let decode_side = match (edit.long_side, edit.crop) {
        (Some(s), None) => s.saturating_mul(2),
        _ => u32::MAX,
    };
    let image = match memory {
        Some(image) => image.clone(),
        None => crate::decode::decode_full(source, decode_side)?,
    };
    let image = Edit { long_side: None, ..edit }.apply(image);
    Ok(match edit.long_side {
        Some(side) => image::DynamicImage::ImageRgb8(resize(image, side)?),
        None => image,
    })
}

/// Koopia kettale; tagastab baitide arvu.
pub fn save_copy(source: &Path, memory: Option<&image::DynamicImage>, output: &Path, edit: Edit, quality: u8, keep_exif: bool, modified: Option<SystemTime>) -> Result<u64, String> {
    let image = edited(source, memory, edit)?;
    let rgb = image.into_rgb8();
    let (exif, icc) = if keep_exif { jpeg_metadata(source) } else { (None, None) };
    write_jpeg(&rgb, output, quality, exif, icc, modified)
}

/// Lanczos3 (SIMD) `long_side` kasti, proportsioon jääb (ka suuremaks).
pub fn resize(image: image::DynamicImage, long_side: u32) -> Result<image::RgbImage, String> {
    use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer, images::Image};
    let rgb = image.into_rgb8();
    let (w, h) = rgb.dimensions();
    let (dw, dh) = fit_dims((w, h), long_side.max(16));
    if (dw, dh) == (w, h) {
        return Ok(rgb);
    }
    let src = Image::from_vec_u8(w, h, rgb.into_raw(), PixelType::U8x3).map_err(|e| e.to_string())?;
    let mut dst = Image::new(dw, dh, PixelType::U8x3);
    Resizer::new().resize(&src, &mut dst, &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Lanczos3))).map_err(|e| e.to_string())?;
    image::RgbImage::from_raw(dw, dh, dst.into_vec()).ok_or_else(|| "pixels".to_owned())
}

/// Allika EXIF (pööre nullitud — pikslid on juba õiget pidi) ja
/// värviprofiil: JPEG-ist plokk tervikuna, TIFF-ist, DNG-st, CR3-st ja
/// teistest konteineritest valikuliselt (`container_metadata`).
pub fn jpeg_metadata(source: &Path) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    use image::ImageDecoder;
    let ext = source.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if !matches!(ext.as_str(), "jpg" | "jpeg" | "jpe" | "jfif") {
        return container_metadata(source);
    }
    let Ok(file) = std::fs::File::open(source) else { return (None, None) };
    let Ok(mut decoder) = image::codecs::jpeg::JpegDecoder::new(std::io::BufReader::new(file)) else { return (None, None) };
    let mut exif = decoder.exif_metadata().ok().flatten();
    if let Some(chunk) = exif.as_mut() {
        let _ = image::metadata::Orientation::remove_from_exif_chunk(chunk);
    }
    (exif, decoder.icc_profile().ok().flatten())
}

/// TIFF, DNG, HEIC jt: EXIF kamadak-exifiga, ainult see, mis JPEG-is
/// mõtet omab — pildistamise andmed (ilma tootja kinnise MakerNote'ita,
/// mille sisemised viited kopeerimisel katki läheksid), GPS ning IFD0-st
/// kaamera, aeg, autor, autoriõigus, kirjeldus. Pööre jääb välja.
/// Värviprofiil TIFF-i sildist 34675.
fn container_metadata(source: &Path) -> (Option<Vec<u8>>, Option<Vec<u8>>) {
    use exif::experimental::Writer;
    use exif::{Context, In, Tag, Value};
    const TIFF_KEEP: &[Tag] = &[Tag::Make, Tag::Model, Tag::DateTime, Tag::Artist, Tag::Copyright, Tag::ImageDescription, Tag::Software];
    let Some((fields, little)) = crate::exif::fields(source) else { return (None, None) };
    let icc = fields.iter().find(|f| f.ifd_num == In::PRIMARY && f.tag == Tag(Context::Tiff, 34675)).and_then(|f| match &f.value {
        Value::Undefined(bytes, _) | Value::Byte(bytes) => Some(bytes.clone()),
        _ => None,
    });
    let mut writer = Writer::new();
    let mut any = false;
    for f in fields.iter().filter(|f| f.ifd_num == In::PRIMARY) {
        let empty = matches!(&f.value, Value::Ascii(v) if v.iter().all(|s| s.iter().all(|&b| b == 0 || b == b' ')));
        let keep = !empty
            && match f.tag.context() {
                Context::Tiff => TIFF_KEEP.contains(&f.tag),
                Context::Exif => f.tag != Tag::MakerNote,
                Context::Gps => true,
                _ => false,
            };
        if keep {
            writer.push_field(f);
            any = true;
        }
    }
    if !any {
        return (None, icc);
    }
    let mut out = std::io::Cursor::new(Vec::new());
    match writer.write(&mut out, little) {
        Ok(()) => (Some(out.into_inner()), icc),
        Err(_) => (None, icc),
    }
}

/// JPEG kettale: `.part` fail, EXIF ja värviprofiil kaasa, faili aeg
/// (`modified`) originaalilt, valmis fail lõpliku nime alla — poolik fail
/// ei jää kunagi lõpliku nime alla. Tagastab baitide arvu.
pub fn write_jpeg(rgb: &image::RgbImage, output: &Path, quality: u8, exif: Option<Vec<u8>>, icc: Option<Vec<u8>>, modified: Option<SystemTime>) -> Result<u64, String> {
    use image::ImageEncoder;
    let (width, height) = rgb.dimensions();
    let file_name = output.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let temp = output.with_file_name(format!("{file_name}.part"));
    let written = (|| -> Result<u64, String> {
        let file = std::fs::File::create(&temp).map_err(|e| e.to_string())?;
        let mut writer = std::io::BufWriter::new(file);
        let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut writer, quality.clamp(1, 100));
        if let Some(exif) = exif {
            let _ = encoder.set_exif_metadata(exif);
        }
        if let Some(icc) = icc {
            let _ = encoder.set_icc_profile(icc);
        }
        encoder.write_image(rgb.as_raw(), width, height, image::ExtendedColorType::Rgb8).map_err(|e| e.to_string())?;
        let file = writer.into_inner().map_err(|e| e.into_error().to_string())?;
        if let Some(modified) = modified {
            file.set_modified(modified).map_err(|e| e.to_string())?;
        }
        Ok(file.metadata().map_err(|e| e.to_string())?.len())
    })();
    let bytes = match written {
        Ok(b) => b,
        Err(e) => {
            let _ = std::fs::remove_file(&temp);
            return Err(e);
        }
    };
    if let Err(e) = std::fs::rename(&temp, output) {
        let _ = std::fs::remove_file(&temp);
        return Err(format!("{}: {e}", output.display()));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pööre ja peegel: H pärast R-i on sama mis R vastupidi pärast H-d;
    /// V = 180° + H; kaks korda H on algseis; mõõdud pöörduvad.
    #[test]
    fn edits_compose_like_on_screen() {
        let e = Edit::default().rotate(true).flip_h();
        assert_eq!(e, Edit { flip_h: true, rot: 3, ..Default::default() });
        assert_eq!(Edit::default().flip_h().flip_h(), Edit::default());
        assert_eq!(Edit::default().flip_v(), Edit { flip_h: true, rot: 2, ..Default::default() });
        assert_eq!(Edit::default().flip_v().flip_v(), Edit::default());
        let r = Edit { long_side: Some(2048), rot: 1, ..Default::default() };
        assert_eq!(r.dims((6000, 4000)), (1365, 2048));
        assert_eq!(fit_dims((6000, 4000), 3000), (3000, 2000));
        // Pikslitel: 2 × 1 pilt, peegel ja pööre.
        let mut img = image::RgbImage::new(2, 1);
        img.put_pixel(0, 0, image::Rgb([255, 0, 0]));
        img.put_pixel(1, 0, image::Rgb([0, 0, 255]));
        let out = Edit { flip_h: true, rot: 1, ..Default::default() }.apply(image::DynamicImage::ImageRgb8(img)).into_rgb8();
        // Peegel: sinine, punane; 90° päripäeva: ülemine = sinine.
        assert_eq!(out.dimensions(), (1, 2));
        assert_eq!(out.get_pixel(0, 0).0, [0, 0, 255]);
    }

    /// 5 × 3 pilt, iga piksel oma värviga (järjekorranumber).
    fn numbered() -> image::DynamicImage {
        image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(5, 3, |x, y| image::Rgb([(y * 5 + x) as u8, 0, 0])))
    }

    /// Lõige liigub pöörde ja peegliga kaasa: pööre pärast lõiget = lõigatud
    /// pildi pööre; mõõt klapib pikslitega; ekraani punkt viib õigesse
    /// failipikslisse (nii joonistab aken).
    #[test]
    fn crop_follows_rotation_and_mirror() {
        let img = numbered();
        let base = Edit::default().rotate(true).cropped((5, 3), [0.34, 0.2, 1.0, 0.8]);
        let cut = base.apply(img.clone());
        assert_eq!((cut.width(), cut.height()), base.dims((5, 3)));
        assert_eq!(base.crop.map(|c| (c.w, c.h)), Some((2, 3)));
        type Same = fn(&image::DynamicImage) -> image::DynamicImage;
        let cases: [(Edit, Same); 4] = [
            (base.rotate(true), |i| i.rotate90()),
            (base.rotate(false), |i| i.rotate270()),
            (base.flip_h(), |i| i.fliph()),
            (base.flip_v(), |i| i.flipv()),
        ];
        for (edit, same) in cases {
            let out = edit.apply(img.clone());
            assert_eq!(out.to_rgb8(), same(&cut).to_rgb8(), "{edit:?}");
            assert_eq!((out.width(), out.height()), edit.dims((5, 3)));
            let src = img.to_rgb8();
            let shown = out.to_rgb8();
            for (x, y, px) in shown.enumerate_pixels() {
                let (s, t) = edit.source_point((x as f32 + 0.5) / out.width() as f32, (y as f32 + 0.5) / out.height() as f32);
                let at = src.get_pixel((s * 5.0) as u32, (t * 3.0) as u32);
                assert_eq!(at, px, "{edit:?} ({x}, {y})");
            }
        }
    }

    /// Lõige vähendatud pildil hoiab mõõtkava; teine lõige on esimese sees;
    /// valiku osa pildist.
    #[test]
    fn crop_keeps_scale_and_nests() {
        let e = Edit { long_side: Some(3000), ..Default::default() }.cropped((6000, 4000), [0.0, 0.0, 0.5, 0.5]);
        assert_eq!(e.crop, Some(Crop { x: 0, y: 0, w: 3000, h: 2000, base: (6000, 4000) }));
        assert_eq!(e.dims((6000, 4000)), (1500, 1000));
        let again = e.cropped((6000, 4000), [0.5, 0.5, 1.0, 1.0]);
        assert_eq!(again.crop, Some(Crop { x: 1500, y: 1000, w: 1500, h: 1000, base: (6000, 4000) }));
        assert_eq!(again.dims((6000, 4000)), (750, 500));
        assert!((again.crop_magnification() - 4.0).abs() < 1e-4);
        let part = crop_part(numbered(), [0.2, 1.0 / 3.0, 0.6, 1.0]).to_rgb8();
        assert_eq!(part.dimensions(), (2, 2));
        assert_eq!(part.get_pixel(0, 0).0[0], 6);
    }

    #[test]
    fn paste_scales_to_selection_without_changing_outside_pixels() {
        let base = image::Rgba([24, 48, 120, 255]);
        let patch = image::Rgba([200, 150, 100, 255]);
        let image = image::RgbaImage::from_pixel(8, 4, base).into();
        let pasted = image::RgbaImage::from_pixel(1, 1, patch).into();
        let out = paste_part(image, pasted, [0.25, 0.25, 0.75, 0.75]).into_rgba8();
        assert_eq!(out.dimensions(), (8, 4));
        for (x, y, pixel) in out.enumerate_pixels() {
            let expected = if (2..6).contains(&x) && (1..3).contains(&y) { patch } else { base };
            assert_eq!(*pixel, expected, "pixel at ({x}, {y})");
        }
    }

    /// Koopia: pööratud ja vähendatud, aeg originaalilt, `.part` faili ei jää.
    #[test]
    fn saved_copy_is_rotated_resized_and_dated() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.jpg");
        image::RgbImage::from_pixel(800, 400, image::Rgb([90, 120, 200])).save(&src).unwrap();
        let out = dir.path().join("a_200.jpg");
        let when = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
        let edit = Edit { rot: 1, long_side: Some(200), ..Default::default() };
        let bytes = save_copy(&src, None, &out, edit, 85, true, Some(when)).unwrap();
        assert!(bytes > 0);
        let back = image::open(&out).unwrap();
        assert_eq!((back.width(), back.height()), (100, 200));
        assert_eq!(std::fs::metadata(&out).unwrap().modified().unwrap(), when);
        assert!(!dir.path().join("a_200.jpg.part").exists());
    }
}
