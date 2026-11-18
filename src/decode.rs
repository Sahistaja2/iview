//! Lahtipakkimine: üks sissepääs kõigile vormingutele, alati kohe soovitud
//! mõõtu (pikem külg `max_side`), mitte täissuuruses ja siis vähendades.
//!
//! Kiirus tuleb sellest, mida *ei* tehta:
//! - JPEG: Windowsi dekooder (WIC) vähendab DCT-astmes — 24 MP foto
//!   ekraani mõõtu on mitu korda kiirem kui täissuuruses lahti pakkida;
//! - RAW (CR3, CR2, NEF, ARW, DNG …): kaamera enda täissuuruses JPEG
//!   konteinerist, toorpilti ei puudutata;
//! - pakkimata TIFF (panoraamid, sadu MB): loetakse ainult iga k-s rida;
//! - muu `image`-iga ja vähendus SIMD-iga (`fast_image_resize`);
//! - läbipaistvuseta pilt läheb tekstuuriks otse RGB-st (üks koopia vähem).

use std::path::Path;

use eframe::egui;

/// Lahti pakitud pilt ja algse pildi mõõt nagu vaadates (pööre arvestatud;
/// pöördeta vaates nagu failis). Mõõt tuleb dekoodrist endast — faili ei
/// avata päise pärast teist korda.
pub struct Picture {
    pub image: egui::ColorImage,
    pub dims: Option<(u32, u32)>,
}

/// Pööre 5–8: külili, laius ja kõrgus vahetuvad.
fn turned((w, h): (u32, u32), orientation: u16, on: bool) -> (u32, u32) {
    if on && orientation >= 5 { (h, w) } else { (w, h) }
}

/// Pakib pildi lahti nii, et pikem külg on kuni `max_side`. `orientation_on`:
/// pööra EXIF-i järgi. `progress`: riba jaoks (fail loetakse oma voo kaudu).
pub fn decode_with(path: &Path, max_side: u32, orientation_on: bool, progress: Option<std::sync::Arc<crate::wic::Progress>>) -> Result<Picture, String> {
    use image::ImageDecoder;
    let ext = ext_of(path);
    if matches!(ext.as_str(), "svg" | "svgz") {
        let image = decode_svg(path, max_side).ok_or_else(|| "Could not read the SVG".to_string())?;
        let dims = Some((image.size[0] as u32, image.size[1] as u32));
        return Ok(Picture { image, dims });
    }
    // RAW: kaamera täissuuruses JPEG konteinerist (`raw`); pööre on
    // konteineri EXIF-is.
    if let Some(e) = crate::raw::embedded(path) {
        let (w, h, rgb) = match crate::wic::decode_bytes(&e.jpeg, max_side) {
            Some(d) => (d.width, d.height, d.rgb),
            None => {
                let image = image::load_from_memory_with_format(&e.jpeg, image::ImageFormat::Jpeg).map_err(|e| format!("image: {e}"))?;
                let image = resize_fit(image, max_side).ok_or_else(resize_failed)?.into_rgb8();
                (image.width(), image.height(), image.into_raw())
            }
        };
        let dims = crate::jpeg::dimensions(&e.jpeg).map(|d| turned(d, e.orientation, orientation_on));
        let mut image = image::DynamicImage::ImageRgb8(image::RgbImage::from_raw(w, h, rgb).ok_or_else(resize_failed)?);
        if orientation_on && let Some(o) = image::metadata::Orientation::from_exif(e.orientation as u8) {
            image.apply_orientation(o);
        }
        return Ok(Picture { image: to_color_image(image), dims });
    }
    // Pakkimata TIFF: ainult vajalikud read kettalt.
    if matches!(ext.as_str(), "tif" | "tiff")
        && let Some((w, h, rgb, orientation)) = crate::raw::tiff_rows_preview(path, max_side, progress.as_deref())
    {
        let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_raw(w, h, rgb).ok_or_else(resize_failed)?);
        let mut image = resize_fit(image, max_side).ok_or_else(resize_failed)?;
        if orientation_on && let Some(o) = image::metadata::Orientation::from_exif(orientation as u8) {
            image.apply_orientation(o);
        }
        let dims = crate::raw::tiff_size(path).map(|d| turned(d, orientation, orientation_on));
        return Ok(Picture { image: to_color_image(image), dims });
    }
    // JPEG, HEIC, AVIF ja TIFF Windowsi dekoodriga: vähendab dekoodri sees,
    // oskab HEIC-i ja TIFF-i kõiki kujusid (JPEG-pakitud, CMYK, 16 bitti).
    // Kui WIC ei saa, proovib `image`. Läbipaistvusega vormingud jäävad
    // `image`-ile (WIC-i RGB kaotaks läbipaistvuse).
    let wic = matches!(ext.as_str(), "jpg" | "jpeg" | "jpe" | "jfif" | "heic" | "heif" | "avif" | "tif" | "tiff");
    let mut wic_err = None;
    let direct = if wic {
        match crate::wic::decode_err(path, max_side, progress) {
            Ok(d) => Some(d),
            Err(e) => {
                wic_err = Some(e);
                None
            }
        }
    } else {
        None
    };
    let (image, orientation, dims) = match direct {
        Some(d) => (
            image::DynamicImage::ImageRgb8(image::RgbImage::from_raw(d.width, d.height, d.rgb).ok_or_else(resize_failed)?),
            image::metadata::Orientation::from_exif(d.orientation as u8).unwrap_or(image::metadata::Orientation::NoTransforms),
            turned(d.full, d.orientation, orientation_on),
        ),
        None => {
            let by_image = (|| -> Result<_, image::ImageError> {
                let mut reader = image::ImageReader::open(path)?.with_guessed_format()?;
                // Suur pilt (panoraam): `image`-i vaikimisi mälupiir (512 MB) on väike.
                reader.no_limits();
                let mut decoder = reader.into_decoder()?;
                let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
                let image = image::DynamicImage::from_decoder(decoder)?;
                use image::metadata::Orientation as O;
                let quarter = matches!(orientation, O::Rotate90 | O::Rotate270 | O::Rotate90FlipH | O::Rotate270FlipH);
                let dims = if orientation_on && quarter { (image.height(), image.width()) } else { (image.width(), image.height()) };
                Ok((image, orientation, dims))
            })();
            match by_image {
                Ok(v) => v,
                Err(e) => {
                    return Err(match wic_err {
                        Some(w) => format!("Windows: {w} · image: {e}"),
                        None => format!("image: {e}"),
                    });
                }
            }
        }
    };
    let mut image = resize_fit(image, max_side).ok_or_else(resize_failed)?;
    if orientation_on {
        image.apply_orientation(orientation);
    }
    Ok(Picture { image: to_color_image(image), dims: Some(dims) })
}

/// Pildi mõõdud nagu vaadates (EXIF-i pööre arvestatud), ilma lahti
/// pakkimata: RAW-ist konteinerist, muu WIC-i või `image`-i päisest.
pub fn dimensions(path: &Path) -> Option<(u32, u32)> {
    if matches!(ext_of(path).as_str(), "svg" | "svgz") {
        return None;
    }
    crate::raw::dimensions(path).or_else(|| crate::wic::dimensions(path)).or_else(|| image::image_dimensions(path).ok())
}

/// Ilma läbipaistvuseta pilt otse RGB-st: üks täissuuruses koopia vähem.
fn to_color_image(image: image::DynamicImage) -> egui::ColorImage {
    let size = [image.width() as usize, image.height() as usize];
    match image {
        image::DynamicImage::ImageRgb8(rgb) => egui::ColorImage::from_rgb(size, rgb.as_raw()),
        other => egui::ColorImage::from_rgba_unmultiplied(size, other.to_rgba8().as_raw()),
    }
}

fn ext_of(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

fn resize_failed() -> String {
    "Resizing failed".to_string()
}

/// SVG (ka gzip-iga SVGZ) pildiks. Vektor joonistatakse kohe `max_side`
/// kasti mõõtu — ka väikese joonise puhul suuremaks, et oleks terav. Tekst
/// jääb välja: fontide teek (fontdb) pole kaasas.
fn decode_svg(path: &Path, max_side: u32) -> Option<egui::ColorImage> {
    let data = std::fs::read(path).ok()?;
    let options = resvg::usvg::Options { resources_dir: path.parent().map(Path::to_path_buf), ..Default::default() };
    let tree = resvg::usvg::Tree::from_data(&data, &options).ok()?;
    let size = tree.size();
    let side = max_side.min(8192);
    let scale = side as f32 / size.width().max(size.height()).max(1.0);
    let (w, h) = (((size.width() * scale).round() as u32).max(1), ((size.height() * scale).round() as u32).max(1));
    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h)?;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    Some(egui::ColorImage::from_rgba_premultiplied([w as usize, h as usize], pixmap.data()))
}

/// Vähendab pildi `max_side` kasti (bilineaarne, SIMD); väiksemat ei muudeta.
pub fn resize_fit(image: image::DynamicImage, max_side: u32) -> Option<image::DynamicImage> {
    use fast_image_resize::{FilterType, PixelType, ResizeAlg, ResizeOptions, Resizer, images::Image};
    let (w, h) = (image.width(), image.height());
    if w <= max_side && h <= max_side {
        return Some(image);
    }
    let scale = max_side as f64 / w.max(h) as f64;
    let (dw, dh) = (((w as f64 * scale).round() as u32).max(1), ((h as f64 * scale).round() as u32).max(1));
    let alpha = image.color().has_alpha();
    let (buffer, pixel) = if alpha { (image.into_rgba8().into_raw(), PixelType::U8x4) } else { (image.into_rgb8().into_raw(), PixelType::U8x3) };
    let src = Image::from_vec_u8(w, h, buffer, pixel).ok()?;
    let mut dst = Image::new(dw, dh, pixel);
    Resizer::new().resize(&src, &mut dst, &ResizeOptions::new().resize_alg(ResizeAlg::Convolution(FilterType::Bilinear))).ok()?;
    let raw = dst.into_vec();
    Some(if alpha {
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_raw(dw, dh, raw)?)
    } else {
        image::DynamicImage::ImageRgb8(image::RgbImage::from_raw(dw, dh, raw)?)
    })
}

/// Täissuuruses (või kuni `max_side`) ja pööratud pilt salvestamiseks ja
/// lõikelauale: JPEG-i dekooder vähendab ise, RAW-ist kaamera JPEG.
pub fn decode_full(source: &Path, max_side: u32) -> Result<image::DynamicImage, String> {
    use image::ImageDecoder;
    if let Some(e) = crate::raw::embedded(source) {
        let mut image = match crate::wic::decode_bytes(&e.jpeg, max_side) {
            Some(d) => image::DynamicImage::ImageRgb8(image::RgbImage::from_raw(d.width, d.height, d.rgb).ok_or("pixels")?),
            None => image::load_from_memory_with_format(&e.jpeg, image::ImageFormat::Jpeg).map_err(|e| e.to_string())?,
        };
        if let Some(o) = image::metadata::Orientation::from_exif(e.orientation as u8) {
            image.apply_orientation(o);
        }
        return Ok(image);
    }
    let wic = matches!(ext_of(source).as_str(), "jpg" | "jpeg" | "jpe" | "jfif" | "heic" | "heif" | "avif" | "tif" | "tiff");
    let direct = if wic { crate::wic::decode(source, max_side) } else { None };
    let (mut image, orientation) = match direct {
        Some(d) => (
            image::DynamicImage::ImageRgb8(image::RgbImage::from_raw(d.width, d.height, d.rgb).ok_or("pixels")?),
            image::metadata::Orientation::from_exif(d.orientation as u8).unwrap_or(image::metadata::Orientation::NoTransforms),
        ),
        None => {
            let mut reader = image::ImageReader::open(source).map_err(|e| e.to_string())?.with_guessed_format().map_err(|e| e.to_string())?;
            reader.no_limits();
            let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
            let orientation = decoder.orientation().unwrap_or(image::metadata::Orientation::NoTransforms);
            (image::DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?, orientation)
        }
    };
    image.apply_orientation(orientation);
    Ok(image)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lai panoraam vähendatakse kasti; väiksemat ei suurendata.
    #[test]
    fn resize_fits_the_box_and_keeps_small() {
        let wide = image::DynamicImage::ImageRgb8(image::RgbImage::new(4000, 1000));
        let out = resize_fit(wide, 1000).unwrap();
        assert_eq!((out.width(), out.height()), (1000, 250));
        let small = image::DynamicImage::ImageRgba8(image::RgbaImage::new(30, 20));
        let out = resize_fit(small, 1000).unwrap();
        assert_eq!((out.width(), out.height()), (30, 20));
    }

    /// PNG `image`-iga: läbipaistvus jääb alles, mõõt kasti.
    #[test]
    fn png_decodes_to_the_requested_side() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.png");
        image::RgbaImage::from_pixel(400, 200, image::Rgba([10, 20, 30, 128])).save(&path).unwrap();
        let img = decode_with(&path, 100, true, None).unwrap();
        assert_eq!(img.image.size, [100, 50]);
        assert_eq!(img.dims, Some((400, 200)));
        assert_eq!(dimensions(&path), Some((400, 200)));
        assert!(decode_with(&dir.path().join("pole.png"), 100, true, None).is_err());
    }

    /// Mõõt: WIC (ainult Windows) vs `image` + vähendus (Mac, Linux):
    /// `IVIEW_BENCH_FILE=… cargo test --release bench_jpeg_paths -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn bench_jpeg_paths() {
        let path = std::path::PathBuf::from(std::env::var("IVIEW_BENCH_FILE").expect("IVIEW_BENCH_FILE"));
        for side in [1536u32, 2560] {
            for _ in 0..3 {
                let t = std::time::Instant::now();
                let w = crate::wic::decode_err(&path, side, None).map(|d| (d.width, d.height));
                let wic = t.elapsed().as_millis();
                let t = std::time::Instant::now();
                let img = image::ImageReader::open(&path).unwrap().with_guessed_format().unwrap().decode().unwrap();
                let full = t.elapsed().as_millis();
                let small = resize_fit(img, side).unwrap();
                let both = t.elapsed().as_millis();
                eprintln!("külg {side}: WIC {wic} ms {w:?} · image {full} ms + vähendus = {both} ms ({}×{})", small.width(), small.height());
            }
        }
    }

    /// JPEG läbib WIC-i (Windowsis) või `image`-i ja tuleb kasti mõõtu.
    #[test]
    fn jpeg_decodes_scaled() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.jpg");
        image::RgbImage::from_pixel(1600, 1200, image::Rgb([200, 100, 50])).save(&path).unwrap();
        let img = decode_with(&path, 400, true, None).unwrap();
        assert_eq!(img.image.size, [400, 300]);
        assert_eq!(img.dims, Some((1600, 1200)));
        let full = decode_full(&path, u32::MAX).unwrap();
        assert_eq!((full.width(), full.height()), (1600, 1200));
    }
}
