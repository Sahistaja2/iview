//! JPEG-i päis ilma lahti pakkimata: mõõdud (SOF) ja kvaliteedi hinnang
//! (DQT kvantimistabel libjpeg-i skaala järgi, nagu IrfanView „Image →
//! Information" seda näitab). Kasutavad pildiaken (Ctrl+S kvaliteet
//! originaalilt) ja RAW-i kaamera-JPEG (`raw`).

use std::path::Path;

/// Standardne heledustabel (libjpeg, kvaliteet 50), zigzag-järjekorras
/// nagu failis.
const STD_LUMA: [u16; 64] = [
    16, 11, 12, 14, 12, 10, 16, 14, 13, 14, 18, 17, 16, 19, 24, 40, 26, 24, 22, 22, 24, 49, 35, 37, 29, 40, 58, 51, 61, 60, 57, 51, 56, 55, 64, 72, 92, 78, 64, 68, 87, 69, 55, 56, 80, 109, 81, 87, 95, 98, 103, 104, 103, 62, 77, 113, 121, 112, 100, 120, 92, 101, 103, 99,
];

/// Lõigud päisest: (marker, sisu). Lõpeb SOS-i (pildiandmed) või faili
/// lõpuga.
fn segments(data: &[u8]) -> impl Iterator<Item = (u8, &[u8])> {
    let mut i = 2usize;
    std::iter::from_fn(move || {
        if data.len() < 2 || data[..2] != [0xFF, 0xD8] {
            return None;
        }
        loop {
            while i < data.len() && data[i] == 0xFF {
                i += 1;
            }
            if i + 3 > data.len() {
                return None;
            }
            let marker = data[i];
            // SOS: edasi on pildiandmed; iseseisvad markerid (RSTn, TEM) ilma pikkuseta.
            if marker == 0xDA || marker == 0xD9 {
                return None;
            }
            if (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
                i += 1;
                continue;
            }
            let len = u16::from_be_bytes([data[i + 1], data[i + 2]]) as usize;
            if len < 2 || i + 1 + len > data.len() {
                return None;
            }
            let body = &data[i + 3..i + 1 + len];
            i += 1 + len;
            return Some((marker, body));
        }
    })
}

/// Laius ja kõrgus SOF-markerist (ilma pöördeta, nagu failis). Kadudeta
/// JPEG (SOF3, 7, 11, 15 — RAW-ide toorandmed) ei loe: seda vaatur ei näita.
pub fn dimensions(data: &[u8]) -> Option<(u32, u32)> {
    segments(data)
        .find(|(m, body)| matches!(m, 0xC0..=0xC2 | 0xC5 | 0xC6 | 0xC9 | 0xCA | 0xCD | 0xCE) && body.len() >= 5)
        .map(|(_, b)| (u16::from_be_bytes([b[3], b[4]]) as u32, u16::from_be_bytes([b[1], b[2]]) as u32))
        .filter(|&(w, h)| w > 0 && h > 0)
}

/// Kvaliteet 1–100 heledustabeli järgi: libjpeg skaleerib standardtabelit
/// teguriga `200 − 2q` (q ≥ 50) või `5000 / q` (q < 50); siit tegur tagasi
/// kvaliteediks. Kaamerad ja Photoshop ei kasuta täpselt sama tabelit,
/// sellepärast on see hinnang (nagu IrfanView'il).
pub fn quality(data: &[u8]) -> Option<u8> {
    let (_, dqt) = segments(data).find(|(m, _)| *m == 0xDB)?;
    // Esimene (heledus-) tabel: 1 bait täpsus ja number, siis 64 väärtust.
    let precision16 = dqt.first()? >> 4 != 0;
    let table: Vec<u32> = if precision16 {
        dqt.get(1..129)?.chunks(2).map(|b| u32::from(u16::from_be_bytes([b[0], b[1]]))).collect()
    } else {
        dqt.get(1..65)?.iter().map(|&b| u32::from(b)).collect()
    };
    let sum: u32 = table.iter().sum();
    let std_sum: u32 = STD_LUMA.iter().map(|&v| u32::from(v)).sum();
    if sum == 0 {
        return Some(100);
    }
    // Skaleerimistegur protsentides (100 = standardtabel = kvaliteet 50).
    let scale = (f64::from(sum) * 100.0 / f64::from(std_sum)).max(1.0);
    let q = if scale <= 100.0 { (200.0 - scale) / 2.0 } else { 5000.0 / scale };
    Some(q.round().clamp(1.0, 100.0) as u8)
}

/// Faili kvaliteet: JPEG-ist päis, RAW-ist kaamera JPEG; muu (PNG, TIFF)
/// `None`.
pub fn quality_of(path: &Path) -> Option<u8> {
    if let Some(e) = crate::raw::embedded(path) {
        return quality(&e.jpeg);
    }
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    if !matches!(ext.as_str(), "jpg" | "jpeg" | "jpe" | "jfif") {
        return None;
    }
    let head = read_head(path, 256 * 1024)?;
    quality(&head)
}

/// Faili algus (kuni `limit` baiti).
fn read_head(path: &Path, limit: u64) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut buf = Vec::new();
    std::fs::File::open(path).ok()?.take(limit).read_to_end(&mut buf).ok()?;
    Some(buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sünteetiline päis: SOI, DQT (standardtabel × tegur), SOF0, SOS.
    fn jpeg(scale_pct: u32, w: u16, h: u16) -> Vec<u8> {
        let mut v = vec![0xFF, 0xD8];
        v.extend_from_slice(&[0xFF, 0xDB, 0x00, 0x43, 0x00]);
        for &q in &STD_LUMA {
            v.push(((u32::from(q) * scale_pct + 50) / 100).clamp(1, 255) as u8);
        }
        v.extend_from_slice(&[0xFF, 0xC0, 0x00, 0x0B, 0x08]);
        v.extend_from_slice(&h.to_be_bytes());
        v.extend_from_slice(&w.to_be_bytes());
        v.extend_from_slice(&[0x01, 0x01, 0x11, 0x00]);
        v.extend_from_slice(&[0xFF, 0xDA, 0x00, 0x08, 0x01, 0x01, 0x00, 0x00, 0x3F, 0x00, 0x12, 0x34]);
        v
    }

    #[test]
    fn quality_from_quantisation_table() {
        // Tegur 100 % = standardtabel = kvaliteet 50; 20 % → 90; 8 % → 96.
        assert_eq!(quality(&jpeg(100, 10, 10)), Some(50));
        assert_eq!(quality(&jpeg(20, 10, 10)), Some(90));
        assert_eq!(quality(&jpeg(8, 10, 10)), Some(96));
        assert_eq!(quality(&jpeg(250, 10, 10)), Some(20));
        assert_eq!(quality(b"not a jpeg"), None);
    }

    #[test]
    fn dimensions_from_sof() {
        assert_eq!(dimensions(&jpeg(100, 6000, 4000)), Some((6000, 4000)));
        assert_eq!(dimensions(&[0xFF, 0xD8, 0xFF, 0xD9]), None);
    }
}
