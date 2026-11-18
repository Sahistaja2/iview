//! Ctrl+S: koopia kettale. Originaal
//! täissuuruses lahti (RAW-ist kaamera JPEG), peegel ja pööre, pikem külg
//! Lanczos3-ga (SIMD), EXIF ja värviprofiil kaasa, EXIF-i pööre nulli
//! (pikslid on juba õiget pidi), faili aeg originaalilt. Kirjutatakse
//! `.part` faili ja see saab lõpliku nime alles valmis kujul.

use std::path::Path;
use std::time::SystemTime;

/// Pildi muudatus aknas (faili ei puututa): peegel (enne), pööre
/// veerandpöördeid päripäeva (pärast), pikem külg.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Edit {
    pub flip_h: bool,
    pub rot: u8,
    pub long_side: Option<u32>,
}

impl Edit {
    pub fn is_empty(self) -> bool {
        self == Edit::default()
    }

    /// Peegel ekraanil horisontaalselt: F ∘ R^k ∘ F^f = R^(−k) ∘ F^(f+1).
    pub fn flip_h(self) -> Self {
        Edit { flip_h: !self.flip_h, rot: (4 - self.rot) % 4, ..self }
    }

    /// Peegel vertikaalselt = pööre 180° ja horisontaalne peegel.
    pub fn flip_v(self) -> Self {
        Edit { flip_h: !self.flip_h, rot: (6 - self.rot) % 4, ..self }
    }

    pub fn rotate(self, clockwise: bool) -> Self {
        Edit { rot: (self.rot + if clockwise { 1 } else { 3 }) % 4, ..self }
    }

    /// Mõõdud pärast muudatust (pikem külg, pööre) originaali mõõtudest.
    pub fn dims(self, orig: (u32, u32)) -> (u32, u32) {
        let (w, h) = match self.long_side {
            Some(side) => fit_dims(orig, side),
            None => orig,
        };
        if self.rot % 2 == 1 { (h, w) } else { (w, h) }
    }

    /// Pikslitele: peegel enne, pööre pärast — sama järjekord mis ekraanil.
    pub fn apply(self, mut image: image::DynamicImage) -> image::DynamicImage {
        if self.flip_h {
            image = image.fliph();
        }
        match self.rot % 4 {
            1 => image.rotate90(),
            2 => image.rotate180(),
            3 => image.rotate270(),
            _ => image,
        }
    }

    /// Riba tekst: „90° · mirrored".
    pub fn describe(self) -> Vec<String> {
        let mut parts = Vec::new();
        if self.rot != 0 {
            parts.push(format!("{}°", u32::from(self.rot) * 90));
        }
        if self.flip_h {
            parts.push("mirrored".to_string());
        }
        parts
    }
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
    let decode_side = edit.long_side.map_or(u32::MAX, |s| s.saturating_mul(2));
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
    let (exif, icc) = if keep_exif && memory.is_none() { jpeg_metadata(source) } else { (None, None) };
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
        assert_eq!(e, Edit { flip_h: true, rot: 3, long_side: None });
        assert_eq!(Edit::default().flip_h().flip_h(), Edit::default());
        assert_eq!(Edit::default().flip_v(), Edit { flip_h: true, rot: 2, long_side: None });
        assert_eq!(Edit::default().flip_v().flip_v(), Edit::default());
        let r = Edit { long_side: Some(2048), rot: 1, flip_h: false };
        assert_eq!(r.dims((6000, 4000)), (1365, 2048));
        assert_eq!(fit_dims((6000, 4000), 3000), (3000, 2000));
        // Pikslitel: 2 × 1 pilt, peegel ja pööre.
        let mut img = image::RgbImage::new(2, 1);
        img.put_pixel(0, 0, image::Rgb([255, 0, 0]));
        img.put_pixel(1, 0, image::Rgb([0, 0, 255]));
        let out = Edit { flip_h: true, rot: 1, long_side: None }.apply(image::DynamicImage::ImageRgb8(img)).into_rgb8();
        // Peegel: sinine, punane; 90° päripäeva: ülemine = sinine.
        assert_eq!(out.dimensions(), (1, 2));
        assert_eq!(out.get_pixel(0, 0).0, [0, 0, 255]);
    }

    /// Koopia: pööratud ja vähendatud, aeg originaalilt, `.part` faili ei jää.
    #[test]
    fn saved_copy_is_rotated_resized_and_dated() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.jpg");
        image::RgbImage::from_pixel(800, 400, image::Rgb([90, 120, 200])).save(&src).unwrap();
        let out = dir.path().join("a_200.jpg");
        let when = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_600_000_000);
        let edit = Edit { rot: 1, long_side: Some(200), flip_h: false };
        let bytes = save_copy(&src, None, &out, edit, 85, true, Some(when)).unwrap();
        assert!(bytes > 0);
        let back = image::open(&out).unwrap();
        assert_eq!((back.width(), back.height()), (100, 200));
        assert_eq!(std::fs::metadata(&out).unwrap().modified().unwrap(), when);
        assert!(!dir.path().join("a_200.jpg.part").exists());
    }
}
