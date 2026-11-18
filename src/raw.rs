//! RAW-failid ilma Windowsi koodekita: kaamera enda JPEG konteinerist, nagu
//! IrfanView „load embedded preview". CR3 (`cr3`) ja TIFF-põhised RAW-d
//! (CR2, NEF, ARW, DNG, ORF, RW2, RAF, PEF …), kus täissuuruses JPEG on
//! mõnes IFD-s (`JPEGInterchangeFormat` või JPEG-pakitud ribad). Üks
//! sissepääs kõigile: pisipildid, pildiaken, vaatur, vähendaja ja
//! omadused küsivad siit; `set_prefer_codec` (pildiakna seade „RAW-failid:
//! Windowsi koodek") laseb koodeki olemasolul toorpildi lahti pakkida.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

/// Konteinerist leitud JPEG: baidid ja EXIF-i pööre (1–8).
pub struct Embedded {
    pub jpeg: Vec<u8>,
    pub orientation: u16,
}

/// TIFF-põhised RAW-laiendid (CR3 eraldi).
const TIFF_RAW: &[&str] = &["cr2", "nef", "nrw", "arw", "srf", "sr2", "dng", "orf", "rw2", "raf", "pef", "srw", "3fr", "iiq", "erf", "mrw", "mos", "rwl"];

static PREFER_CODEC: AtomicBool = AtomicBool::new(false);

/// Seade „RAW-failid: Windowsi koodek": kui koodek on, pakitakse toorpilt
/// lahti; kaamera JPEG jääb varuks.
pub fn set_prefer_codec(on: bool) {
    PREFER_CODEC.store(on, Ordering::Relaxed);
}

pub fn prefers_codec() -> bool {
    PREFER_CODEC.load(Ordering::Relaxed)
}

fn ext_of(path: &Path) -> String {
    path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default()
}

/// Kas laiend on RAW (CR3 või TIFF-põhine)?
pub fn is_raw(path: &Path) -> bool {
    let ext = ext_of(path);
    ext == "cr3" || TIFF_RAW.contains(&ext.as_str())
}

/// Kaamera JPEG RAW-ist. `None`: pole RAW, JPEG-i ei leitud, või seade
/// eelistab koodekit ja Windows oskab faili ise.
pub fn embedded(path: &Path) -> Option<Embedded> {
    if !is_raw(path) {
        return None;
    }
    if prefers_codec() && crate::wic::dimensions(path).is_some() {
        return None;
    }
    if crate::cr3::is_cr3(path) {
        return crate::cr3::embedded(path);
    }
    tiff_preview(path)
}

/// Mõõdud nagu pilti vaadates (pööre arvestatud), ilma lahti pakkimata:
/// CR3-l rajalt, TIFF-põhisel RAW-il kaamera JPEG-i päisest.
pub fn dimensions(path: &Path) -> Option<(u32, u32)> {
    if !is_raw(path) || (prefers_codec() && crate::wic::dimensions(path).is_some()) {
        return None;
    }
    if crate::cr3::is_cr3(path) {
        return crate::cr3::dimensions(path);
    }
    let e = tiff_preview(path)?;
    let (w, h) = crate::jpeg::dimensions(&e.jpeg)?;
    Some(if e.orientation >= 5 { (h, w) } else { (w, h) })
}

/// TIFF-i (ka pakkimata panoraam) pildiandmete maht: IFD0 ribade või
/// paanide baidid (`StripByteCounts` 0x117 / `TileByteCounts` 0x145) pluss
/// esimese riba nihe. Laadimise riba tervik — faili suurus eksitaks, kui
/// failis on peale pildi muud (Photoshopi kihid), mida dekooder ei loe.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn tiff_image_bytes(path: &Path) -> Option<u64> {
    let file = std::fs::File::open(path).ok()?;
    let mut t = Tiff::open(std::io::BufReader::new(file))?;
    let ifd0 = t.u32_at(4)?;
    let (entries, _) = t.ifd(u64::from(ifd0))?;
    let counts = entries.iter().copied().find(|e| e.tag == 0x117 || e.tag == 0x145)?;
    let offsets = entries.iter().copied().find(|e| e.tag == 0x111 || e.tag == 0x144)?;
    let sum = t.sum(counts)?;
    let start = u64::from(t.first(offsets)?);
    Some(start + sum)
}

/// TIFF-i laius ja kõrgus IFD0-st (pöördeta, nagu failis).
pub fn tiff_size(path: &Path) -> Option<(u32, u32)> {
    let file = std::fs::File::open(path).ok()?;
    let mut t = Tiff::open(std::io::BufReader::new(file))?;
    let ifd0 = t.u32_at(4)?;
    let (entries, _) = t.ifd(u64::from(ifd0))?;
    let get = |tag: u16| entries.iter().copied().find(|e| e.tag == tag);
    let w = t.first(get(0x100)?)?;
    let h = t.first(get(0x101)?)?;
    (w > 0 && h > 0).then_some((w, h))
}

/// Pakkimata TIFF-i kiire eelvaade: loeb ainult iga `k`-nda rea (ja selle
/// naabri, keskmistatult), mitte kogu faili — 600 MB panoraami puhul on
/// ketas ainus pidur ja k = 5 juures loetakse 2/5. Ribadena, 8 või 16
/// bitti, hall / RGB / RGBA, planaarne kuju 1. `None`: muu kuju (pakitud,
/// CMYK, paanid, planaarne) või vähendamist pole vaja → tavaline dekood.
/// Tagastab (laius, kõrgus, RGB, EXIF-i pööre).
pub fn tiff_rows_preview(path: &Path, max_side: u32, progress: Option<&crate::wic::Progress>) -> Option<(u32, u32, Vec<u8>, u16)> {
    use std::sync::atomic::Ordering::Relaxed;
    let file = std::fs::File::open(path).ok()?;
    let mut t = Tiff::open(std::io::BufReader::with_capacity(64 * 1024, file))?;
    let ifd0 = t.u32_at(4)?;
    let (entries, _) = t.ifd(u64::from(ifd0))?;
    let get = |tag: u16| entries.iter().copied().find(|e| e.tag == tag);
    let width = t.first(get(0x100)?)?;
    let height = t.first(get(0x101)?)?;
    let bits = get(0x102).and_then(|e| t.first(e)).unwrap_or(1);
    let compression = get(0x103).and_then(|e| t.first(e)).unwrap_or(1);
    let photometric = get(0x106).and_then(|e| t.first(e)).unwrap_or(1);
    let samples = get(0x115).and_then(|e| t.first(e)).unwrap_or(1);
    let planar = get(0x11C).and_then(|e| t.first(e)).unwrap_or(1);
    let rows_per_strip = get(0x116).and_then(|e| t.first(e)).unwrap_or(height).max(1);
    let orientation = get(0x112).and_then(|e| t.first(e)).map_or(1, |o| o as u16);
    let bytes_per_sample = match bits {
        8 => 1usize,
        16 => 2,
        _ => return None,
    };
    if compression != 1 || planar != 1 || width == 0 || height == 0 || !(0..=2).contains(&photometric) || !matches!(samples, 1 | 3 | 4) {
        return None;
    }
    if photometric == 2 && samples == 1 {
        return None;
    }
    let k = (width.max(height) / max_side.max(1)) as usize;
    if k < 2 {
        return None;
    }
    let offsets = t.list(get(0x111)?)?;
    let row_bytes = width as usize * samples as usize * bytes_per_sample;
    let (ow, oh) = ((width as usize / k).max(1), (height as usize / k).max(1));
    if let Some(p) = progress {
        p.total.store((oh * 2 * row_bytes) as u64, Relaxed);
        p.read.store(0, Relaxed);
    }
    let le = t.le;
    let mut reader = t.r;
    let mut row0 = vec![0u8; row_bytes];
    let mut row1 = vec![0u8; row_bytes];
    let mut out = Vec::with_capacity(ow * oh * 3);
    let mut read_row = |r: usize, buf: &mut [u8]| -> Option<()> {
        let strip = r / rows_per_strip as usize;
        let at = u64::from(*offsets.get(strip)?) + ((r % rows_per_strip as usize) * row_bytes) as u64;
        reader.seek(SeekFrom::Start(at)).ok()?;
        reader.read_exact(buf).ok()
    };
    // Proov: baidist (8 bitti) või kahest baidist (16, kõrgem bait).
    let sample = |buf: &[u8], i: usize| -> u32 {
        if bytes_per_sample == 1 {
            u32::from(buf[i])
        } else if le {
            u32::from(buf[i * 2 + 1])
        } else {
            u32::from(buf[i * 2])
        }
    };
    let sp = samples as usize;
    for j in 0..oh {
        let r0 = j * k;
        let r1 = (r0 + 1).min(height as usize - 1);
        read_row(r0, &mut row0)?;
        read_row(r1, &mut row1)?;
        if let Some(p) = progress {
            p.read.fetch_add((2 * row_bytes) as u64, Relaxed);
        }
        for x in 0..ow {
            let x0 = x * k;
            let mut acc = [0u32; 3];
            for px in x0..x0 + k {
                for (c, a) in acc.iter_mut().enumerate() {
                    let i = px * sp + if sp >= 3 { c } else { 0 };
                    *a += sample(&row0, i) + sample(&row1, i);
                }
            }
            let n = (2 * k) as u32;
            for a in acc {
                let v = (a / n) as u8;
                out.push(if photometric == 0 { 255 - v } else { v });
            }
        }
    }
    Some((ow as u32, oh as u32, out, orientation))
}

/// TIFF-konteineri lugeja: väikesed lugemised otse failist (IFD-d on
/// alguses, JPEG võib olla kaugel).
struct Tiff<R> {
    r: R,
    le: bool,
}

fn rd16(le: bool, b: &[u8]) -> u16 {
    if le { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) }
}

fn rd32(le: bool, b: &[u8]) -> u32 {
    if le { u32::from_le_bytes([b[0], b[1], b[2], b[3]]) } else { u32::from_be_bytes([b[0], b[1], b[2], b[3]]) }
}

/// IFD kirje: silt, tüüp, arv, väärtus või nihe.
#[derive(Clone, Copy)]
struct Entry {
    tag: u16,
    kind: u16,
    count: u32,
    raw: u32,
    raw16: u16,
}

impl<R: Read + Seek> Tiff<R> {
    fn open(mut r: R) -> Option<Self> {
        let mut head = [0u8; 4];
        r.seek(SeekFrom::Start(0)).ok()?;
        r.read_exact(&mut head).ok()?;
        let le = match &head {
            b"II*\0" => true,
            b"MM\0*" => false,
            // Olympus ORF („IIRO"), Panasonic RW2 („IIU\0"): sama ehitus.
            b"IIRO" | b"IIRS" | b"IIU\0" => true,
            _ => return None,
        };
        Some(Self { r, le })
    }

    fn bytes(&mut self, at: u64, n: usize) -> Option<Vec<u8>> {
        let mut buf = vec![0u8; n];
        self.r.seek(SeekFrom::Start(at)).ok()?;
        self.r.read_exact(&mut buf).ok()?;
        Some(buf)
    }

    fn u32_at(&mut self, at: u64) -> Option<u32> {
        let b = self.bytes(at, 4)?;
        Some(rd32(self.le, &b))
    }

    /// IFD kirjed ja järgmise IFD nihe.
    fn ifd(&mut self, at: u64) -> Option<(Vec<Entry>, u32)> {
        let le = self.le;
        let n = rd16(le, &self.bytes(at, 2)?) as usize;
        if n == 0 || n > 1024 {
            return None;
        }
        let buf = self.bytes(at + 2, n * 12 + 4)?;
        let entries = buf[..n * 12]
            .chunks(12)
            .map(|e| Entry { tag: rd16(le, &e[0..2]), kind: rd16(le, &e[2..4]), count: rd32(le, &e[4..8]), raw: rd32(le, &e[8..12]), raw16: rd16(le, &e[8..10]) })
            .collect();
        let next = rd32(le, &buf[n * 12..]);
        Some((entries, next))
    }

    /// Esimene väärtus (SHORT või LONG, ka mitmesest).
    fn first(&mut self, e: Entry) -> Option<u32> {
        match (e.kind, e.count) {
            (3, 1) => Some(u32::from(e.raw16)),
            (4, 1) => Some(e.raw),
            (3, _) => {
                let b = self.bytes(u64::from(e.raw), 2)?;
                Some(u32::from(rd16(self.le, &b)))
            }
            (4, _) => self.u32_at(u64::from(e.raw)),
            _ => None,
        }
    }

    /// SHORT- või LONG-massiivi kõik väärtused (ribade nihked; tuhandeid).
    fn list(&mut self, e: Entry) -> Option<Vec<u32>> {
        let count = e.count as usize;
        let size = match e.kind {
            3 => 2,
            4 => 4,
            _ => return None,
        };
        if count * size <= 4 {
            return self.first(e).map(|v| vec![v]);
        }
        if count > 1 << 20 {
            return None;
        }
        let le = self.le;
        let buf = self.bytes(u64::from(e.raw), count * size)?;
        Some(buf.chunks(size).map(|c| if size == 2 { u32::from(rd16(le, c)) } else { rd32(le, c) }).collect())
    }

    /// SHORT- või LONG-massiivi summa (ribade baidid; tuhandeid kirjeid).
    #[cfg_attr(not(windows), allow(dead_code))]
    fn sum(&mut self, e: Entry) -> Option<u64> {
        let count = e.count as usize;
        let size = match e.kind {
            3 => 2,
            4 => 4,
            _ => return None,
        };
        if count * size <= 4 {
            return self.first(e).map(u64::from);
        }
        if count > 1 << 20 {
            return None;
        }
        let le = self.le;
        let buf = self.bytes(u64::from(e.raw), count * size)?;
        Some(buf.chunks(size).map(|c| if size == 2 { u64::from(rd16(le, c)) } else { u64::from(rd32(le, c)) }).sum())
    }

    /// LONG-massiivi kõik väärtused (alam-IFD-de nihked, ribade nihked).
    fn all(&mut self, e: Entry) -> Vec<u32> {
        let count = e.count as usize;
        if e.kind != 4 || count == 0 || count > 64 {
            return self.first(e).into_iter().collect();
        }
        if count == 1 {
            return vec![e.raw];
        }
        let le = self.le;
        let Some(buf) = self.bytes(u64::from(e.raw), count * 4) else { return Vec::new() };
        buf.chunks(4).map(|c| rd32(le, c)).collect()
    }
}

/// Suurim JPEG TIFF-konteineri IFD-dest (IFD0, selle alam-IFD-d, IFD1 …):
/// `JPEGInterchangeFormat` (0x201/0x202) või JPEG-pakitud (6, 7) üks riba
/// (0x111/0x117). Pööre IFD0-st.
fn tiff_preview(path: &Path) -> Option<Embedded> {
    let file = std::fs::File::open(path).ok()?;
    let mut t = Tiff::open(std::io::BufReader::new(file))?;
    let first = u64::from(t.u32_at(4)?);
    let mut queue = vec![first];
    let mut seen = Vec::new();
    let mut candidates: Vec<(u64, u32)> = Vec::new();
    let mut orientation = 1u16;
    while let Some(at) = queue.pop() {
        if seen.contains(&at) || seen.len() > 32 || at < 8 {
            continue;
        }
        seen.push(at);
        let Some((entries, next)) = t.ifd(at) else { continue };
        let find = |tag: u16| entries.iter().find(|e| e.tag == tag).copied();
        if at == first
            && let Some(e) = find(0x112)
            && let Some(o) = t.first(e)
            && (1..=8).contains(&o)
        {
            orientation = o as u16;
        }
        let mut candidate = |t: &mut Tiff<_>, off: Option<Entry>, len: Option<Entry>| {
            if let (Some(o), Some(l)) = (off, len)
                && let (Some(o), Some(l)) = (t.first(o), t.first(l))
                && l > 1024
            {
                candidates.push((u64::from(o), l));
            }
        };
        candidate(&mut t, find(0x201), find(0x202));
        let compression = find(0x103).and_then(|e| t.first(e));
        let strips = find(0x111).map(|e| t.all(e).len()).unwrap_or(0);
        if matches!(compression, Some(6 | 7)) && strips == 1 {
            candidate(&mut t, find(0x111), find(0x117));
        }
        // Panasonic RW2: JPEG IFD0-s baitidena (JpgFromRaw, 0x2E: nihe ja pikkus).
        if let Some(e) = find(0x2E).filter(|e| e.kind == 7 && e.count > 1024) {
            candidates.push((u64::from(e.raw), e.count));
        }
        if let Some(e) = find(0x14A) {
            queue.extend(t.all(e).into_iter().map(u64::from));
        }
        if next != 0 {
            queue.push(u64::from(next));
        }
    }
    // Päris (baseline / progressive) JPEG suurima piksliarvuga: CR2 hoiab
    // toorpilti ennast kadudeta JPEG-ina (SOF3), see ei ole eelvaade.
    let mut best: Option<(u64, u32, u64)> = None;
    for (at, len) in candidates {
        let Some(head) = t.bytes(at, (len as usize).min(64 * 1024)) else { continue };
        let Some((w, h)) = crate::jpeg::dimensions(&head) else { continue };
        let pixels = u64::from(w) * u64::from(h);
        if best.is_none_or(|(_, _, p)| pixels > p) {
            best = Some((at, len, pixels));
        }
    }
    let (at, len, _) = best?;
    let jpeg = t.bytes(at, len as usize)?;
    Some(Embedded { jpeg, orientation })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Väike TIFF: IFD0 pööre 6 ja alam-IFD (0x14A) JPEG-pakitud ribaga,
    /// IFD1 väikese JPEGInterchangeFormat-pisipildiga — suurem võidab.
    fn tiff() -> Vec<u8> {
        let mut v = b"II*\0".to_vec();
        v.extend_from_slice(&8u32.to_le_bytes());
        let entry = |tag: u16, kind: u16, count: u32, value: u32| {
            let mut e = tag.to_le_bytes().to_vec();
            e.extend_from_slice(&kind.to_le_bytes());
            e.extend_from_slice(&count.to_le_bytes());
            e.extend_from_slice(&value.to_le_bytes());
            e
        };
        // Paigutus: IFD0 @8 (2 kirjet = 2+24+4 = 30) → 38; IFD1 @38 (2 kirjet) → 68;
        // alam-IFD @68 (3 kirjet = 2+36+4 = 42) → 110; väike JPEG @110 (1100 B); suur JPEG @1210 (3000 B).
        let small_at = 110u32;
        let big_at = 1210u32;
        let ifd = |entries: Vec<Vec<u8>>, next: u32| {
            let mut b = (entries.len() as u16).to_le_bytes().to_vec();
            for e in entries {
                b.extend(e);
            }
            b.extend_from_slice(&next.to_le_bytes());
            b
        };
        v.extend(ifd(vec![entry(0x112, 3, 1, 6), entry(0x14A, 4, 1, 68)], 38));
        v.extend(ifd(vec![entry(0x201, 4, 1, small_at), entry(0x202, 4, 1, 1100)], 0));
        v.extend(ifd(vec![entry(0x103, 3, 1, 6), entry(0x111, 4, 1, big_at), entry(0x117, 4, 1, 3000)], 0));
        assert_eq!(v.len(), small_at as usize);
        let mut small = vec![0xFF, 0xD8, 0xFF, 0xD9];
        small.resize(1100, 0);
        v.extend(small);
        // Suur: päris SOF0 (100 × 80), väike: ainult SOI + EOI (ei loe).
        let mut big = vec![0xFF, 0xD8, 0xFF, 0xC0, 0x00, 0x0B, 0x08, 0x00, 0x50, 0x00, 0x64, 0x01, 0x01, 0x11, 0x00];
        big.resize(3000, 1);
        v.extend(big);
        v
    }

    /// Päris fail: `IVIEW_RAW_FILE=… cargo test --release embedded_from_env -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn embedded_from_env() {
        let path = std::path::PathBuf::from(std::env::var("IVIEW_RAW_FILE").expect("IVIEW_RAW_FILE"));
        let started = std::time::Instant::now();
        let e = embedded(&path).expect("kaamera JPEG");
        let dims = crate::jpeg::dimensions(&e.jpeg);
        let q = crate::jpeg::quality(&e.jpeg);
        eprintln!("{}: JPEG {} B, {dims:?}, pööre {}, kvaliteet {q:?}, {} ms", path.display(), e.jpeg.len(), e.orientation, started.elapsed().as_millis());
        assert!(dims.is_some());
    }

    #[test]
    fn picks_the_largest_jpeg_and_reads_orientation() {
        let data = tiff();
        let dir = std::env::temp_dir().join(format!("iview-raw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("katse.nef");
        std::fs::write(&path, &data).unwrap();
        let e = tiff_preview(&path).unwrap();
        assert_eq!(e.orientation, 6);
        assert_eq!(e.jpeg.len(), 3000);
        assert_eq!(crate::jpeg::dimensions(&e.jpeg), Some((100, 80)));
        assert!(is_raw(&path) && !is_raw(Path::new("a.jpg")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
