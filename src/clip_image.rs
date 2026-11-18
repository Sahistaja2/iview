//! Pilt lõikelaua kujul ja tagasi (pildiakna Ctrl+C / Ctrl+V). Windowsi
//! lõikelaual on pilt kahel kujul: PNG (läbipaistvus alles; brauserid,
//! Snipping Tool) ja DIB — BMP-fail ilma 14-baidise päiseta, mida
//! mõistavad kõik programmid.

use image::{DynamicImage, ImageFormat, RgbaImage};

/// PNG baidid (läbipaistvus alles).
pub fn to_png(image: &RgbaImage) -> Result<Vec<u8>, String> {
    let mut out = std::io::Cursor::new(Vec::new());
    image.write_to(&mut out, ImageFormat::Png).map_err(|e| e.to_string())?;
    Ok(out.into_inner())
}

/// DIB (32 bitti, read alt üles). Läbipaistvus valgele: DIB-i neljanda
/// baidi loevad programmid eri moodi, läbipaistev must oleks paljudes must.
pub fn to_dib(image: &RgbaImage) -> Vec<u8> {
    let (w, h) = image.dimensions();
    let size = w as usize * h as usize * 4;
    let mut out = Vec::with_capacity(40 + size);
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(h as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // BI_RGB
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(&[0; 16]); // eraldusvõime, palett
    for y in (0..h).rev() {
        for x in 0..w {
            let [r, g, b, a] = image.get_pixel(x, y).0;
            let over_white = |c: u8| ((u32::from(c) * u32::from(a) + 255 * (255 - u32::from(a)) + 127) / 255) as u8;
            out.extend_from_slice(&[over_white(b), over_white(g), over_white(r), 255]);
        }
    }
    out
}

/// DIB lõikelaualt pildiks: ette BMP-faili päis ja `image` loeb selle.
pub fn from_dib(dib: &[u8]) -> Result<DynamicImage, String> {
    let u32_at = |i: usize| dib.get(i..i + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    let (Some(header), Some(compression), Some(used)) = (u32_at(0), u32_at(16), u32_at(32)) else {
        return Err("the clipboard picture is damaged".into());
    };
    let bits = dib.get(14..16).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]));
    let colors = if used > 0 { used } else if bits <= 8 { 1 << bits } else { 0 };
    // BI_BITFIELDS (3) lühikese päisega: kolm maski päise järel.
    let masks = if header == 40 && compression == 3 { 12 } else { 0 };
    let offset = 14 + header + masks + colors * 4;
    let mut bmp = Vec::with_capacity(14 + dib.len());
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&((14 + dib.len()) as u32).to_le_bytes());
    bmp.extend_from_slice(&[0; 4]);
    bmp.extend_from_slice(&offset.to_le_bytes());
    bmp.extend_from_slice(dib);
    let image = image::load_from_memory_with_format(&bmp, ImageFormat::Bmp).map_err(|e| e.to_string())?;
    // 32-bitine DIB, mille neljas bait on kõikjal 0 (ekraanipildid): see
    // pole läbipaistvus, pilt on läbinisti nähtav.
    Ok(match image {
        DynamicImage::ImageRgba8(mut rgba) if rgba.pixels().all(|p| p.0[3] == 0) => {
            rgba.pixels_mut().for_each(|p| p.0[3] = 255);
            DynamicImage::ImageRgba8(rgba)
        }
        other => other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> RgbaImage {
        RgbaImage::from_fn(5, 3, |x, y| image::Rgba([(x * 50) as u8, (y * 80) as u8, 200, 255]))
    }

    #[test]
    fn dib_round_trip_keeps_pixels_and_orientation() {
        let image = sample();
        let back = from_dib(&to_dib(&image)).unwrap().to_rgba8();
        assert_eq!(back.dimensions(), (5, 3));
        assert_eq!(back, image);
    }

    #[test]
    fn transparent_pixels_go_onto_white_in_dib() {
        let image = RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 0, 0]));
        let back = from_dib(&to_dib(&image)).unwrap().to_rgba8();
        assert_eq!(back.get_pixel(0, 0).0, [255, 255, 255, 255]);
    }

    /// Ekraanipildi DIB: 32 bitti, neljas bait 0 — pilt jääb nähtavaks.
    #[test]
    fn zero_alpha_dib_is_opaque() {
        let mut dib = to_dib(&sample());
        for px in dib[40..].chunks_mut(4) {
            px[3] = 0;
        }
        let back = from_dib(&dib).unwrap().to_rgba8();
        assert!(back.pixels().all(|p| p.0[3] == 255));
        assert_eq!(back.get_pixel(1, 2).0, [50, 160, 200, 255]);
    }

    #[test]
    fn png_round_trip() {
        let image = sample();
        let back = image::load_from_memory(&to_png(&image).unwrap()).unwrap().to_rgba8();
        assert_eq!(back, image);
    }

    #[test]
    fn short_dib_is_an_error() {
        assert!(from_dib(&[1, 2, 3]).is_err());
    }
}
