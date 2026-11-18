// Programmi ikoon koodist: sinine ruut, selles „foto" (taevas, mäed,
// päike). Sama joonistus läheb exe-faili (`build.rs`, ICO) ja aknale
// (`main.rs`) — eraldi pildifaili pole. Kaasatakse `include!`-iga, seepärast
// ilma moodulidokumentatsioonita.
//
// macOS joonistab rakenduste ikoonid oma ruudustikus: 1024 px lõuendil on
// ruut 824 px, ümber tühi serv.
// Täies laiuses ikoon paistaks Dockis teistest suurem.

/// Maci ikooni tühi serv lõuendi osana (ruut ≈ 822 / 1024 px).
#[allow(dead_code)]
pub const MACOS_INSET: f32 = 0.082;

/// RGBA pikslid `size` × `size`, servad silutud (4 × 4 proovi pikslis).
#[allow(dead_code)]
pub fn icon_rgba(size: u32) -> Vec<u8> {
    icon_rgba_inset(size, 0.0)
}

/// Nagu `icon_rgba`, ümber tühi serv `inset` (lõuendi osana, igal küljel).
pub fn icon_rgba_inset(size: u32, inset: f32) -> Vec<u8> {
    let s = size as f32;
    let inner = (1.0 - 2.0 * inset).max(0.01);
    let mut out = vec![0u8; (size * size * 4) as usize];
    // Ümarnurkne ristkülik: kas punkt on sees.
    let rounded = |x: f32, y: f32, l: f32, t: f32, r: f32, b: f32, rad: f32| -> bool {
        if x < l || x > r || y < t || y > b {
            return false;
        }
        let cx = x.clamp(l + rad, r - rad);
        let cy = y.clamp(t + rad, b - rad);
        (x - cx).powi(2) + (y - cy).powi(2) <= rad * rad
    };
    // Punkt kolmnurgas (märgiga pindalad).
    let tri = |x: f32, y: f32, a: (f32, f32), b: (f32, f32), c: (f32, f32)| -> bool {
        let d = |p: (f32, f32), q: (f32, f32)| (x - q.0) * (p.1 - q.1) - (p.0 - q.0) * (y - q.1);
        let (d1, d2, d3) = (d(a, b), d(b, c), d(c, a));
        let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
        let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
        !(neg && pos)
    };
    let color = |x: f32, y: f32| -> Option<[f32; 3]> {
        // Taust: sinine ümarruut.
        if !rounded(x, y, 0.02, 0.02, 0.98, 0.98, 0.2) {
            return None;
        }
        let blue = [0.18, 0.43, 0.86];
        // Foto: valge raam.
        let (l, t, r, b) = (0.16, 0.22, 0.84, 0.80);
        if !rounded(x, y, l, t, r, b, 0.05) {
            return Some(blue);
        }
        let (il, it, ir, ib) = (l + 0.05, t + 0.05, r - 0.05, b - 0.05);
        if x < il || x > ir || y < it || y > ib {
            return Some([1.0, 1.0, 1.0]);
        }
        // Päike.
        if (x - 0.63).powi(2) + (y - 0.38).powi(2) <= 0.065f32.powi(2) {
            return Some([1.0, 0.79, 0.24]);
        }
        // Mäed.
        if tri(x, y, (il - 0.02, ib + 0.01), (0.40, 0.42), (0.66, ib + 0.01)) {
            return Some([0.12, 0.23, 0.43]);
        }
        if tri(x, y, (0.46, ib + 0.01), (0.66, 0.53), (ir + 0.06, ib + 0.01)) {
            return Some([0.20, 0.36, 0.62]);
        }
        // Taevas.
        Some([0.81, 0.89, 1.0])
    };
    const N: u32 = 4;
    for py in 0..size {
        for px in 0..size {
            let (mut acc, mut cover) = ([0.0f32; 3], 0.0f32);
            for sy in 0..N {
                for sx in 0..N {
                    let x = ((px as f32 + (sx as f32 + 0.5) / N as f32) / s - inset) / inner;
                    let y = ((py as f32 + (sy as f32 + 0.5) / N as f32) / s - inset) / inner;
                    if let Some(c) = color(x, y) {
                        for k in 0..3 {
                            acc[k] += c[k];
                        }
                        cover += 1.0;
                    }
                }
            }
            let i = ((py * size + px) * 4) as usize;
            if cover > 0.0 {
                for k in 0..3 {
                    out[i + k] = (acc[k] / cover * 255.0).round() as u8;
                }
                out[i + 3] = (cover / (N * N) as f32 * 255.0).round() as u8;
            }
        }
    }
    out
}
