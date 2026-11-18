//! Canon CR3: kaamera enda JPEG konteinerist (nagu IrfanView näitab).
//!
//! CR3 on ISO BMFF (MP4-laadne) konteiner: `moov` kirjeldab neli rada,
//! millest esimene on täissuuruses JPEG (`CRAW` kirje, vorming 0), teised
//! Canoni CRX-pakitud toorpilt ja metaandmed. JPEG-i koht ja pikkus on
//! raja `co64` ja `stsz` kastides. Toorpilti ennast lahti ei pakita — ilma
//! Windowsi RAW-koodekita seda ei oska —, aga JPEG on sama kaader ja tuleb
//! ekraanile millisekunditega. Pööre on `CMT1` (IFD0) sildis 274.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::exif::boxes;
pub use crate::raw::Embedded;

/// JPEG-raja koht failis: mõõdud, nihe, pikkus ja pööre.
struct Track {
    width: u32,
    height: u32,
    offset: u64,
    len: usize,
    orientation: u16,
}

/// Rohkem `moov`-i ei loe: päris failil on see ~64 KB.
const MAX_MOOV: u64 = 8 * 1024 * 1024;
/// Üle selle JPEG ei ole kaamera eelvaade.
const MAX_JPEG: usize = 64 * 1024 * 1024;

/// Kas laiend ütleb CR3?
pub fn is_cr3(path: &Path) -> bool {
    path.extension().is_some_and(|e| e.eq_ignore_ascii_case("cr3"))
}

/// Kaamera JPEG CR3-st. `None`: pole CR3 või JPEG-rada puudub.
pub fn embedded(path: &Path) -> Option<Embedded> {
    let mut file = std::fs::File::open(path).ok()?;
    let track = track(&mut file)?;
    file.seek(SeekFrom::Start(track.offset)).ok()?;
    let mut jpeg = vec![0u8; track.len];
    file.read_exact(&mut jpeg).ok()?;
    if jpeg.len() < 4 || jpeg[..2] != [0xFF, 0xD8] {
        return None;
    }
    Some(Embedded { jpeg, orientation: track.orientation })
}

/// Mõõdud nagu pilti vaadates (pööre arvestatud); loeb ainult `moov`-i.
pub fn dimensions(path: &Path) -> Option<(u32, u32)> {
    let mut file = std::fs::File::open(path).ok()?;
    let t = track(&mut file)?;
    Some(if t.orientation >= 5 { (t.height, t.width) } else { (t.width, t.height) })
}

fn track<R: Read + Seek>(file: &mut R) -> Option<Track> {
    parse(&head_from(file)?)
}

/// Faili algus (`ftyp` + terve `moov`), kust Canoni osad ja rajad tulevad.
pub(crate) fn head(path: &Path) -> Option<Vec<u8>> {
    head_from(&mut std::fs::File::open(path).ok()?)
}

pub(crate) fn head_from<R: Read + Seek>(file: &mut R) -> Option<Vec<u8>> {
    let mut head = [0u8; 12];
    file.read_exact(&mut head).ok()?;
    if &head[4..12] != b"ftypcrx " {
        return None;
    }
    file.seek(SeekFrom::Start(0)).ok()?;
    // `ftyp` ja terve `moov`: moov-i pikkus on tema päises.
    let ftyp = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as u64;
    let mut moov_head = [0u8; 8];
    file.seek(SeekFrom::Start(ftyp)).ok()?;
    file.read_exact(&mut moov_head).ok()?;
    if &moov_head[4..8] != b"moov" {
        return None;
    }
    let moov = u32::from_be_bytes([moov_head[0], moov_head[1], moov_head[2], moov_head[3]]) as u64;
    if !(8..=MAX_MOOV).contains(&moov) {
        return None;
    }
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut data = vec![0u8; (ftyp + moov) as usize];
    file.read_exact(&mut data).ok()?;
    Some(data)
}

pub(crate) const CANON_UUID: [u8; 16] = [0x85, 0xc0, 0xb6, 0x87, 0x82, 0x0f, 0x11, 0xe0, 0x81, 0x11, 0xf4, 0xce, 0x46, 0x2b, 0x6a, 0x48];

/// Canoni `uuid` kasti osa (`CMT1` IFD0, `CMT2` Exif IFD, `CMT4` GPS)
/// `moov`-ist: TIFF-plokk nagu failis.
pub(crate) fn canon_part<'a>(data: &'a [u8], name: &[u8; 4]) -> Option<&'a [u8]> {
    let (_, ms, me) = boxes(data, 0, data.len()).into_iter().find(|(k, ..)| k == b"moov")?;
    let (_, us, ue) = boxes(data, ms, me).into_iter().find(|(k, s, e)| k == b"uuid" && e - s >= 16 && data[*s..s + 16] == CANON_UUID)?;
    boxes(data, us + 16, ue).into_iter().find(|(k, ..)| k == name).map(|(_, s, e)| &data[s..e])
}

/// JPEG-rada `moov`-ist: esimene `trak`, mille `stsd` kirje on `CRAW`
/// vormingus 0 (JPEG); pööre Canoni `uuid` → `CMT1`.
fn parse(data: &[u8]) -> Option<Track> {
    let (_, ms, me) = boxes(data, 0, data.len()).into_iter().find(|(k, ..)| k == b"moov")?;
    let moov = boxes(data, ms, me);
    let orientation = canon_part(data, b"CMT1").and_then(tiff_orientation).unwrap_or(1);
    let child = |list: &[([u8; 4], usize, usize)], name: &[u8; 4]| list.iter().find(|(k, ..)| k == name).map(|(_, s, e)| (*s, *e));
    for (k, ts, te) in &moov {
        if k != b"trak" {
            continue;
        }
        let Some((ms, me)) = child(&boxes(data, *ts, *te), b"mdia") else { continue };
        let Some((is, ie)) = child(&boxes(data, ms, me), b"minf") else { continue };
        let Some((bs, be)) = child(&boxes(data, is, ie), b"stbl") else { continue };
        let stbl = boxes(data, bs, be);
        let (Some(sd), Some(sz), Some(co)) = (child(&stbl, b"stsd"), child(&stbl, b"stsz"), child(&stbl, b"co64").or_else(|| child(&stbl, b"stco")))
        else {
            continue;
        };
        // stsd: versioon+lipud 4, kirjete arv 4, kirje: pikkus 4, tüüp 4,
        // VisualSampleEntry (laius ja kõrgus kirje baidil 32), CRAW-i
        // vorming baidil 88 (0 = JPEG, 1 = CRX).
        let entry = sd.0 + 8;
        if sd.1 < entry + 90 || &data[entry + 4..entry + 8] != b"CRAW" {
            continue;
        }
        let u16_at = |i: usize| u16::from_be_bytes([data[i], data[i + 1]]) as u32;
        if u16_at(entry + 88) != 0 {
            continue;
        }
        let (width, height) = (u16_at(entry + 32), u16_at(entry + 34));
        // stsz: versioon+lipud 4, ühine pikkus 4, arv 4, [pikkused …].
        if sz.1 < sz.0 + 12 {
            continue;
        }
        let u32_at = |i: usize| u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]);
        let common = u32_at(sz.0 + 4);
        let len = if common != 0 { common } else if sz.1 >= sz.0 + 16 { u32_at(sz.0 + 12) } else { 0 } as usize;
        // co64 / stco: versioon+lipud 4, arv 4, esimene nihe (u64 / u32).
        let offset = if co.1 >= co.0 + 16 && child(&stbl, b"co64").is_some() {
            u64::from_be_bytes(data[co.0 + 8..co.0 + 16].try_into().ok()?)
        } else if co.1 >= co.0 + 12 {
            u32_at(co.0 + 8) as u64
        } else {
            continue;
        };
        if len == 0 || len > MAX_JPEG || width == 0 || height == 0 {
            continue;
        }
        return Some(Track { width, height, offset, len, orientation });
    }
    None
}

/// TIFF-i IFD0 silt 274 (pööre), `None` kui puudub.
pub(crate) fn tiff_orientation(t: &[u8]) -> Option<u16> {
    let le = match t.get(..4)? {
        b"II*\0" => true,
        b"MM\0*" => false,
        _ => return None,
    };
    let u16_at = |i: usize| t.get(i..i + 2).map(|b| if le { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) });
    let u32_at = |i: usize| t.get(i..i + 4).map(|b| if le { u32::from_le_bytes([b[0], b[1], b[2], b[3]]) } else { u32::from_be_bytes([b[0], b[1], b[2], b[3]]) });
    let ifd = u32_at(4)? as usize;
    let n = u16_at(ifd)? as usize;
    (0..n.min(512)).map(|i| ifd + 2 + i * 12).find(|&e| u16_at(e) == Some(274)).and_then(|e| u16_at(e + 8)).filter(|o| (1..=8).contains(o))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        v.extend_from_slice(kind);
        v.extend_from_slice(body);
        v
    }

    /// Väike CR3: ftyp, moov → uuid (CMT1 pöördega 6), trak → … → stsd CRAW
    /// (JPEG, 6000×4000), stsz (üks pikkus), co64 (nihe) ja mdat JPEG-iga.
    fn file(orientation: u16) -> Vec<u8> {
        let mut cmt1 = b"II*\0\x08\0\0\0\x01\0".to_vec();
        cmt1.extend_from_slice(&[0x12, 0x01, 3, 0, 1, 0, 0, 0]);
        cmt1.extend_from_slice(&orientation.to_le_bytes());
        cmt1.extend_from_slice(&[0, 0, 0, 0, 0, 0]);
        let mut uuid = vec![0x85, 0xc0, 0xb6, 0x87, 0x82, 0x0f, 0x11, 0xe0, 0x81, 0x11, 0xf4, 0xce, 0x46, 0x2b, 0x6a, 0x48];
        uuid.extend(bx(b"CMT1", &cmt1));
        // Kirje sisu (pärast 8-baidist päist): laius ja kõrgus baidil 24,
        // vorming baidil 80 (kirje algusest 32 ja 88).
        let mut entry = vec![0u8; 90];
        entry[24..26].copy_from_slice(&6000u16.to_be_bytes());
        entry[26..28].copy_from_slice(&4000u16.to_be_bytes());
        entry[80..82].copy_from_slice(&0u16.to_be_bytes());
        let mut craw = ((entry.len() + 8) as u32).to_be_bytes().to_vec();
        craw.extend_from_slice(b"CRAW");
        craw.extend(entry);
        let mut stsd = vec![0, 0, 0, 0, 0, 0, 0, 1];
        stsd.extend(craw);
        let jpeg = [0xFF, 0xD8, 0xFF, 0xD9];
        let mut stsz = vec![0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
        stsz.extend_from_slice(&(jpeg.len() as u32).to_be_bytes());
        let ftyp = bx(b"ftyp", b"crx \0\0\0\0");
        // Nihe sõltub moov-i pikkusest: koostame kaks korda.
        let build = |offset: u64| {
            let mut co64 = vec![0, 0, 0, 0, 0, 0, 0, 1];
            co64.extend_from_slice(&offset.to_be_bytes());
            let stbl = bx(b"stbl", &[bx(b"stsd", &stsd), bx(b"stsz", &stsz), bx(b"co64", &co64)].concat());
            let trak = bx(b"trak", &bx(b"mdia", &bx(b"minf", &stbl)));
            bx(b"moov", &[bx(b"uuid", &uuid), trak].concat())
        };
        let moov_len = build(0).len();
        let offset = (ftyp.len() + moov_len + 8) as u64;
        [ftyp, build(offset), bx(b"mdat", &jpeg)].concat()
    }

    #[test]
    fn finds_the_jpeg_track_and_orientation() {
        let data = file(6);
        let mut cursor = std::io::Cursor::new(&data);
        let t = track(&mut cursor).unwrap();
        assert_eq!((t.width, t.height, t.orientation, t.len), (6000, 4000, 6, 4));
        assert_eq!(&data[t.offset as usize..t.offset as usize + 4], &[0xFF, 0xD8, 0xFF, 0xD9]);
        let mut cursor = std::io::Cursor::new(file(1));
        assert_eq!(track(&mut cursor).unwrap().orientation, 1);
    }

    #[test]
    fn rejects_other_files() {
        let mut cursor = std::io::Cursor::new(b"\xFF\xD8\xFF\xE0 not a cr3 at all".to_vec());
        assert!(track(&mut cursor).is_none());
        let mut cursor = std::io::Cursor::new(b"\0\0\0\x10ftypcrx \0\0\0\0".to_vec());
        assert!(track(&mut cursor).is_none());
    }
}
