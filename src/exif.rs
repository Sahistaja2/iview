//! Pildi andmed (EXIF) failinime ja veeru jaoks: fookuskaugus, kaamera,
//! objektiiv, pildistamise aeg …
//!
//! Loetakse ainult faili algus: DNG, NEF, ARW, TIFF ja JPEG hoiavad need
//! andmed esimestes kilobaitides. Windowsi omaduste süsteem oli RAW-failide
//! puhul liiga aeglane (teeb lahti terve dekoodri). Kui algusest ei piisa
//! (andmed on kaugemal), loetakse rohkem — kuni 8 MB.

use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Väli, mida saab maskis kasutada: `[exif:FocalLength]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Field {
    FocalLength,
    FocalLength35,
    Make,
    Model,
    Lens,
    Taken,
    TakenDay,
    TakenMonth,
    Iso,
    FNumber,
    Exposure,
    Size,
    /// Drooni asend (XMP `drone-dji`): kõrgus stardist (m), kaamera suund
    /// (kompassi järgi, °), kaamera kalle (°). GPS-asukoht („lat,lon") ka
    /// tavalisest EXIF-i GPS-plokist (telefonid, GPS-iga kaamerad).
    Altitude,
    CameraYaw,
    CameraPitch,
    Position,
}

/// Ühe faili loetud väärtused (failinimes kasutatav kuju).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Meta {
    values: Vec<(Field, String)>,
}

impl Meta {
    pub fn get(&self, field: Field) -> Option<&str> {
        self.values.iter().find(|(f, _)| *f == field).map(|(_, v)| v.as_str())
    }

    /// Väärtus failinimesse: Windowsis keelatud märgid (`/` objektiivi
    /// nimes „24.0 mm f/2.8") sidekriipsuks.
    fn set(&mut self, field: Field, value: String) {
        let value: String = value
            .trim()
            .chars()
            .map(|c| if matches!(c, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*') || c.is_control() { '-' } else { c })
            .collect();
        if value.is_empty() {
            return;
        }
        // Pildistamise aeg annab ka päeva ja kuu (jaotamiseks).
        if field == Field::Taken && value.len() >= 8 && value[..8].chars().all(|c| c.is_ascii_digit()) {
            self.values.push((Field::TakenDay, format!("{}-{}-{}", &value[..4], &value[4..6], &value[6..8])));
            self.values.push((Field::TakenMonth, format!("{}-{}", &value[..4], &value[4..6])));
        }
        self.values.push((field, value));
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Ühe väärtusega (testid).
    #[cfg(test)]
    pub fn with(field: Field, value: &str) -> Self {
        Self::with_all(&[(field, value)])
    }

    /// Mitme väärtusega (testid).
    #[cfg(test)]
    pub fn with_all(values: &[(Field, &str)]) -> Self {
        let mut m = Meta::default();
        for (field, value) in values {
            m.set(*field, (*value).to_owned());
        }
        m
    }
}

/// Faili pildi andmed; pole pilt või andmeid pole → tühi. Mõõdud, kui
/// EXIF-is neid pole, pildi päisest (JPEG, PNG, WebP …; pilti lahti ei
/// pakita).
pub fn read(path: &Path) -> Meta {
    let mut m = read_exif(path);
    if m.get(Field::Size).is_none()
        && let Ok((w, h)) = image::image_dimensions(path)
    {
        m.set(Field::Size, format!("{w}x{h}"));
    }
    m
}

fn read_exif(path: &Path) -> Meta {
    let Ok(mut file) = std::fs::File::open(path) else { return Meta::default() };
    let mut head = [0u8; 12];
    if file.read_exact(&mut head).is_err() {
        return Meta::default();
    }
    // TIFF-konteiner (DNG, NEF, ARW, CR2, TIFF): EXIF on faili alguses, aga
    // fail võib olla 100 MB ja viidata lõpuni — tervikparser nuriseks
    // lõigatud puhvril. Loeme IFD-d nihete kaupa otse failist.
    if &head[..4] == b"II*\0" || &head[..4] == b"MM\0*" {
        return tiff::read(&mut file).unwrap_or_default();
    }
    // Canon CR3 (ISO BMFF, „ftyp crx "): EXIF on alguses `moov` kastis.
    if &head[4..12] == b"ftypcrx " {
        let mut buf = Vec::new();
        if file.seek(SeekFrom::Start(0)).is_ok() && (&mut file).take(1024 * 1024).read_to_end(&mut buf).is_ok() {
            return cr3(&buf).unwrap_or_default();
        }
        return Meta::default();
    }
    // JPEG jm: EXIF (APP1) on alguses; 1 MB, vajadusel 8 MB.
    for limit in [1024 * 1024u64, 8 * 1024 * 1024] {
        if file.seek(SeekFrom::Start(0)).is_err() {
            break;
        }
        let mut buf = Vec::new();
        if (&mut file).take(limit).read_to_end(&mut buf).is_err() {
            break;
        }
        let whole = (buf.len() as u64) < limit;
        if let Ok(data) = exif::Reader::new().read_from_container(&mut Cursor::new(&buf)) {
            let mut m = from_exif(&data);
            drone(&buf, &mut m);
            return m;
        }
        if whole {
            break;
        }
    }
    Meta::default()
}

/// Kõik EXIF-i väljad kamadak-exifi kujul koos baidijärjega: JPEG, TIFF,
/// DNG, HEIC jt konteinerist otse; CR3-st Canoni osadest (`CMT1` IFD0,
/// `CMT2` Exif IFD, `CMT4` GPS), mis on eraldi TIFF-id — sildid saavad
/// õige konteksti, et neid saaks JPEG-i EXIF-iks kirjutada. Kasutavad
/// vähendaja (EXIF tulemusse) ja pildiakna täisekraani tekst
/// (`{exif:Silt}`).
pub fn fields(path: &Path) -> Option<(Vec<exif::Field>, bool)> {
    use exif::{Context, In, Tag};
    if crate::cr3::is_cr3(path) {
        let head = crate::cr3::head(path)?;
        let mut out = Vec::new();
        let mut little = true;
        for (name, ctx) in [(b"CMT1", Context::Tiff), (b"CMT2", Context::Exif), (b"CMT4", Context::Gps)] {
            let Some(part) = crate::cr3::canon_part(&head, name) else { continue };
            let Ok(e) = exif::Reader::new().read_raw(part.to_vec()) else { continue };
            little = e.little_endian();
            for f in e.fields().filter(|f| f.ifd_num == In::PRIMARY) {
                out.push(exif::Field { tag: Tag(ctx, f.tag.number()), ifd_num: In::PRIMARY, value: f.value.clone() });
            }
        }
        return (!out.is_empty()).then_some((out, little));
    }
    let mut file = std::fs::File::open(path).ok()?;
    // Suur TIFF (panoraam, sadu MB): kamadak-exif loeks terve faili mällu —
    // proovime ainult algust; kui IFD on faili lõpus, jääb EXIF lugemata.
    let mut head = [0u8; 4];
    let tiff = file.read_exact(&mut head).is_ok() && (&head == b"II*\0" || &head == b"MM\0*");
    file.seek(SeekFrom::Start(0)).ok()?;
    let e = if tiff && file.metadata().ok()?.len() > 64 * 1024 * 1024 {
        let mut buf = Vec::new();
        (&mut file).take(8 * 1024 * 1024).read_to_end(&mut buf).ok()?;
        exif::Reader::new().read_from_container(&mut Cursor::new(&buf)).ok()?
    } else {
        exif::Reader::new().read_from_container(&mut std::io::BufReader::new(file)).ok()?
    };
    let little = e.little_endian();
    Some((e.fields().cloned().collect(), little))
}

/// EXIF-i pööre (1–8; 1 = õiget pidi), kui failis on; CR3-st Canoni osast.
pub fn orientation(path: &Path) -> Option<u16> {
    if crate::cr3::is_cr3(path) {
        return crate::cr3::canon_part(&crate::cr3::head(path)?, b"CMT1").and_then(crate::cr3::tiff_orientation);
    }
    // TIFF-konteiner: IFD0 nihke kaupa failist — kamadak-exif loeks terve
    // faili mällu (600 MB panoraam blokeeris UI-lõime pool sekundit).
    if let Ok(mut file) = std::fs::File::open(path) {
        let mut head = [0u8; 4];
        if file.read_exact(&mut head).is_ok() && (&head == b"II*\0" || &head == b"MM\0*") {
            return file.seek(SeekFrom::Start(0)).ok().and_then(|_| tiff::orientation(&mut file));
        }
    }
    let (fields, _) = fields(path)?;
    let f = fields.iter().find(|f| f.ifd_num == exif::In::PRIMARY && f.tag == exif::Tag::Orientation)?;
    f.value.get_uint(0).map(|v| v as u16).filter(|v| (1..=8).contains(v))
}

/// ISO BMFF kastid lõigus: (tüüp, sisu algus, lõpp).
pub(crate) fn boxes(data: &[u8], from: usize, to: usize) -> Vec<([u8; 4], usize, usize)> {
    let mut out = Vec::new();
    let mut i = from;
    while i + 8 <= to {
        let size = u32::from_be_bytes([data[i], data[i + 1], data[i + 2], data[i + 3]]) as usize;
        let kind = [data[i + 4], data[i + 5], data[i + 6], data[i + 7]];
        let (head, size) = match size {
            1 if i + 16 <= to => (16, u64::from_be_bytes(data[i + 8..i + 16].try_into().unwrap_or_default()) as usize),
            0 => (8, to - i),
            n => (8, n),
        };
        if size < head || i + size > to {
            // Lõigatud puhver: viimane kast ulatub kaugemale.
            out.push((kind, i + head, to));
            break;
        }
        out.push((kind, i + head, i + size));
        i += size;
    }
    out
}

/// Canon CR3: `moov` → Canoni `uuid` → `CMT1` (IFD0) ja `CMT2` (Exif IFD).
fn cr3(data: &[u8]) -> Option<Meta> {
    const CANON: [u8; 16] = [0x85, 0xc0, 0xb6, 0x87, 0x82, 0x0f, 0x11, 0xe0, 0x81, 0x11, 0xf4, 0xce, 0x46, 0x2b, 0x6a, 0x48];
    let (_, ms, me) = boxes(data, 0, data.len()).into_iter().find(|(k, ..)| k == b"moov")?;
    let (_, us, ue) = boxes(data, ms, me).into_iter().find(|(k, s, e)| k == b"uuid" && e - s >= 16 && data[*s..s + 16] == CANON)?;
    let parts = boxes(data, us + 16, ue);
    let part = |name: &[u8; 4]| parts.iter().find(|(k, ..)| k == name).map(|(_, s, e)| &data[*s..*e]);
    Some(tiff::read_cr3(part(b"CMT1")?, part(b"CMT2").unwrap_or_default(), part(b"CMT4").unwrap_or_default()))
}

/// TIFF-konteineri EXIF nihete kaupa: IFD0 (Make, Model, DateTime, mõõtmed,
/// Exif IFD viit) → Exif IFD (fookuskaugus, objektiiv, aeg, ISO, ava, säri).
mod tiff {
    use std::io::{Read, Seek, SeekFrom};

    use super::{number, taken, Field, Meta};

    struct Entry {
        tag: u16,
        kind: u16,
        count: u32,
        raw: [u8; 4],
    }

    struct Tiff {
        le: bool,
    }

    impl Tiff {
        fn u16(&self, b: &[u8]) -> u16 {
            if self.le { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) }
        }

        fn u32(&self, b: &[u8]) -> u32 {
            if self.le { u32::from_le_bytes([b[0], b[1], b[2], b[3]]) } else { u32::from_be_bytes([b[0], b[1], b[2], b[3]]) }
        }

        fn ifd<R: Read + Seek>(&self, r: &mut R, offset: u64) -> Option<Vec<Entry>> {
            r.seek(SeekFrom::Start(offset)).ok()?;
            let mut n = [0u8; 2];
            r.read_exact(&mut n).ok()?;
            let n = self.u16(&n) as usize;
            if n == 0 || n > 4096 {
                return None;
            }
            let mut buf = vec![0u8; n * 12];
            r.read_exact(&mut buf).ok()?;
            Some(
                buf.chunks(12)
                    .map(|e| Entry { tag: self.u16(&e[0..2]), kind: self.u16(&e[2..4]), count: self.u32(&e[4..8]), raw: [e[8], e[9], e[10], e[11]] })
                    .collect(),
            )
        }

        fn uint(&self, e: &Entry) -> Option<u32> {
            match e.kind {
                3 => Some(u32::from(self.u16(&e.raw[..2]))),
                4 => Some(self.u32(&e.raw)),
                _ => None,
            }
        }

        fn text<R: Read + Seek>(&self, r: &mut R, e: &Entry) -> Option<String> {
            if e.kind != 2 {
                return None;
            }
            let n = e.count as usize;
            let bytes = if n <= 4 {
                e.raw[..n].to_vec()
            } else {
                if n > 4096 {
                    return None;
                }
                r.seek(SeekFrom::Start(u64::from(self.u32(&e.raw)))).ok()?;
                let mut buf = vec![0u8; n];
                r.read_exact(&mut buf).ok()?;
                buf
            };
            Some(String::from_utf8_lossy(&bytes).trim_matches(|c: char| c == '\0' || c.is_whitespace()).to_owned())
        }

        /// Kolm murdu järjest (GPS kraadid, minutid, sekundid).
        fn rational3<R: Read + Seek>(&self, r: &mut R, e: &Entry) -> Option<[(u32, u32); 3]> {
            if e.kind != 5 || e.count < 3 {
                return None;
            }
            r.seek(SeekFrom::Start(u64::from(self.u32(&e.raw)))).ok()?;
            let mut buf = [0u8; 24];
            r.read_exact(&mut buf).ok()?;
            let part = |i: usize| (self.u32(&buf[i * 8..i * 8 + 4]), self.u32(&buf[i * 8 + 4..i * 8 + 8]));
            Some([part(0), part(1), part(2)])
        }

        fn rational<R: Read + Seek>(&self, r: &mut R, e: &Entry) -> Option<(u32, u32)> {
            if e.kind != 5 {
                return None;
            }
            r.seek(SeekFrom::Start(u64::from(self.u32(&e.raw)))).ok()?;
            let mut buf = [0u8; 8];
            r.read_exact(&mut buf).ok()?;
            let (n, d) = (self.u32(&buf[..4]), self.u32(&buf[4..]));
            (d != 0).then_some((n, d))
        }
    }

    /// TIFF päis ja esimene IFD.
    /// EXIF-i pööre (1–8) IFD0-st.
    pub fn orientation<R: Read + Seek>(r: &mut R) -> Option<u16> {
        let (t, ifd0) = open(r)?;
        let i = find(&ifd0, 0x0112)?;
        t.uint(&ifd0[i]).map(|v| v as u16).filter(|v| (1..=8).contains(v))
    }

    fn open<R: Read + Seek>(r: &mut R) -> Option<(Tiff, Vec<Entry>)> {
        let mut hdr = [0u8; 8];
        r.seek(SeekFrom::Start(0)).ok()?;
        r.read_exact(&mut hdr).ok()?;
        let t = Tiff { le: &hdr[..2] == b"II" };
        let ifd = t.ifd(r, u64::from(t.u32(&hdr[4..8])))?;
        Some((t, ifd))
    }

    fn find(ifd: &[Entry], tag: u16) -> Option<usize> {
        ifd.iter().position(|e| e.tag == tag)
    }

    /// IFD0: tootja ja mudel.
    fn camera<R: Read + Seek>(t: &Tiff, r: &mut R, ifd0: &[Entry], m: &mut Meta) {
        if let Some(i) = find(ifd0, 0x010F) && let Some(v) = t.text(r, &ifd0[i]) {
            m.set(Field::Make, v);
        }
        if let Some(i) = find(ifd0, 0x0110) && let Some(v) = t.text(r, &ifd0[i]) {
            m.set(Field::Model, v);
        }
    }

    /// Exif IFD: fookuskaugus, objektiiv, aeg, ISO, ava, säri.
    fn exif_fields<R: Read + Seek>(t: &Tiff, r: &mut R, exif: &[Entry], m: &mut Meta) {
        if let Some(i) = find(exif, 0x920A) && let Some((n, d)) = t.rational(r, &exif[i]) {
            m.set(Field::FocalLength, number(f64::from(n) / f64::from(d)));
        }
        if let Some(v) = find(exif, 0xA405).and_then(|i| t.uint(&exif[i])).filter(|v| *v > 0) {
            m.set(Field::FocalLength35, v.to_string());
        }
        if let Some(i) = find(exif, 0xA434) && let Some(v) = t.text(r, &exif[i]) {
            m.set(Field::Lens, v);
        }
        if let Some(v) = find(exif, 0x9003).and_then(|i| t.text(r, &exif[i])) {
            m.set(Field::Taken, taken(&v));
        }
        if let Some(v) = find(exif, 0x8827).and_then(|i| t.uint(&exif[i])) {
            m.set(Field::Iso, v.to_string());
        }
        if let Some(i) = find(exif, 0x829D) && let Some((n, d)) = t.rational(r, &exif[i]) {
            m.set(Field::FNumber, number(f64::from(n) / f64::from(d)));
        }
        if let Some(i) = find(exif, 0x829A) && let Some((n, d)) = t.rational(r, &exif[i]) {
            let text = if n > 0 && n < d { format!("1-{}", (f64::from(d) / f64::from(n)).round()) } else { number(f64::from(n) / f64::from(d)) };
            m.set(Field::Exposure, text);
        }
    }

    /// GPS IFD: laius (1, 2) ja pikkus (3, 4).
    fn gps<R: Read + Seek>(t: &Tiff, r: &mut R, gps: &[Entry], m: &mut Meta) {
        let deg = |r: &mut R, tag: u16| find(gps, tag).and_then(|i| t.rational3(r, &gps[i])).and_then(super::dms);
        let (Some(lat), Some(lon)) = (deg(r, 2), deg(r, 4)) else { return };
        let lat_ref = find(gps, 1).and_then(|i| t.text(r, &gps[i]));
        let lon_ref = find(gps, 3).and_then(|i| t.text(r, &gps[i]));
        super::set_position(m, lat, lat_ref.as_deref(), lon, lon_ref.as_deref());
    }

    /// Pildistamise aeg puudub Exif IFD-s → IFD0 DateTime.
    fn taken_fallback<R: Read + Seek>(t: &Tiff, r: &mut R, ifd0: &[Entry], m: &mut Meta) {
        if m.get(Field::Taken).is_none() && let Some(v) = find(ifd0, 0x0132).and_then(|i| t.text(r, &ifd0[i])) {
            m.set(Field::Taken, taken(&v));
        }
    }

    fn dims(t: &Tiff, ifd: &[Entry], a: u16, b: u16) -> Option<(u32, u32)> {
        Some((find(ifd, a).and_then(|i| t.uint(&ifd[i]))?, find(ifd, b).and_then(|i| t.uint(&ifd[i]))?))
    }

    pub fn read<R: Read + Seek>(r: &mut R) -> Option<Meta> {
        let (t, ifd0) = open(r)?;
        let mut m = Meta::default();
        camera(&t, r, &ifd0, &mut m);
        let exif = find(&ifd0, 0x8769).and_then(|i| t.uint(&ifd0[i])).and_then(|off| t.ifd(r, u64::from(off))).unwrap_or_default();
        exif_fields(&t, r, &exif, &mut m);
        taken_fallback(&t, r, &ifd0, &mut m);
        // DNG: IFD0 on pisipilt, põhipilt on esimeses alam-IFD-s (SubIFDs).
        let sub = find(&ifd0, 0x014A).and_then(|i| {
            let e = &ifd0[i];
            let off = if e.count <= 1 {
                t.uint(e)
            } else {
                r.seek(SeekFrom::Start(u64::from(t.u32(&e.raw)))).ok()?;
                let mut b = [0u8; 4];
                r.read_exact(&mut b).ok()?;
                Some(t.u32(&b))
            }?;
            t.ifd(r, u64::from(off))
        });
        let size = dims(&t, &exif, 0xA002, 0xA003)
            .or_else(|| sub.as_deref().and_then(|s| dims(&t, s, 0x0100, 0x0101)))
            .or_else(|| dims(&t, &ifd0, 0x0100, 0x0101));
        if let Some((w, h)) = size {
            m.set(Field::Size, format!("{w}x{h}"));
        }
        if let Some(g) = find(&ifd0, 0x8825).and_then(|i| t.uint(&ifd0[i])).and_then(|off| t.ifd(r, u64::from(off))) {
            gps(&t, r, &g, &mut m);
        }
        // XMP (silt 700): võib olla faili lõpus (Lightroom kirjutab ümber).
        if let Some(i) = find(&ifd0, 0x02BC) {
            let e = &ifd0[i];
            if matches!(e.kind, 1 | 7) && e.count > 4 && e.count <= 4 * 1024 * 1024 && r.seek(SeekFrom::Start(u64::from(t.u32(&e.raw)))).is_ok() {
                let mut xmp = vec![0u8; e.count as usize];
                if r.read_exact(&mut xmp).is_ok() {
                    super::drone(&xmp, &mut m);
                }
            }
        }
        Some(m)
    }

    /// Canon CR3: EXIF on eraldi TIFF-idena (`CMT1` = IFD0 tootja ja
    /// mudeliga, `CMT2` = Exif IFD, `CMT4` = GPS IFD); kumbki nihked enda
    /// alguse suhtes.
    pub fn read_cr3(cmt1: &[u8], cmt2: &[u8], cmt4: &[u8]) -> Meta {
        let mut m = Meta::default();
        let mut r1 = std::io::Cursor::new(cmt1);
        let first = open(&mut r1);
        if let Some((t, ifd0)) = &first {
            camera(t, &mut r1, ifd0, &mut m);
        }
        let mut r2 = std::io::Cursor::new(cmt2);
        if let Some((t, exif)) = open(&mut r2) {
            exif_fields(&t, &mut r2, &exif, &mut m);
            if let Some((w, h)) = dims(&t, &exif, 0xA002, 0xA003) {
                m.set(Field::Size, format!("{w}x{h}"));
            }
        }
        let mut r4 = std::io::Cursor::new(cmt4);
        if let Some((t, g)) = open(&mut r4) {
            gps(&t, &mut r4, &g, &mut m);
        }
        if let Some((t, ifd0)) = &first {
            taken_fallback(t, &mut r1, ifd0, &mut m);
            if m.get(Field::Size).is_none() && let Some((w, h)) = dims(t, ifd0, 0x0100, 0x0101) {
                m.set(Field::Size, format!("{w}x{h}"));
            }
        }
        m
    }

    #[cfg(test)]
    mod tests {
        use super::super::Field;
        use std::io::Cursor;

        /// Väike TIFF (II): IFD0 Model + Exif IFD viit; Exif IFD fookuskaugus
        /// (ratsionaal nihke taga), ISO ja aeg. Lugeja ei vaata midagi muud.
        #[test]
        fn reads_focal_length_from_tiff_ifds() {
            let mut f: Vec<u8> = b"II*\0".to_vec();
            f.extend(8u32.to_le_bytes());
            // IFD0 @8: 2 kirjet (Model inline, ExifIFD), next 0.
            f.extend(2u16.to_le_bytes());
            f.extend(0x0110u16.to_le_bytes()); f.extend(2u16.to_le_bytes()); f.extend(4u32.to_le_bytes()); f.extend(b"M3T\0");
            let exif_off: u32 = 8 + 2 + 2 * 12 + 4;
            f.extend(0x8769u16.to_le_bytes()); f.extend(4u16.to_le_bytes()); f.extend(1u32.to_le_bytes()); f.extend(exif_off.to_le_bytes());
            f.extend(0u32.to_le_bytes());
            assert_eq!(f.len() as u32, exif_off);
            // Exif IFD: 3 kirjet (FocalLength → nihe, ISO inline, DateTimeOriginal → nihe).
            let data_off = exif_off + 2 + 3 * 12 + 4;
            f.extend(3u16.to_le_bytes());
            f.extend(0x829Au16.to_le_bytes()); f.extend(5u16.to_le_bytes()); f.extend(1u32.to_le_bytes()); f.extend((data_off + 8).to_le_bytes());
            f.extend(0x8827u16.to_le_bytes()); f.extend(3u16.to_le_bytes()); f.extend(1u32.to_le_bytes()); f.extend(100u16.to_le_bytes()); f.extend(0u16.to_le_bytes());
            f.extend(0x920Au16.to_le_bytes()); f.extend(5u16.to_le_bytes()); f.extend(1u32.to_le_bytes()); f.extend(data_off.to_le_bytes());
            f.extend(0u32.to_le_bytes());
            assert_eq!(f.len() as u32, data_off);
            f.extend(18990u32.to_le_bytes()); f.extend(1000u32.to_le_bytes()); // 18.99 → „19"
            f.extend(1u32.to_le_bytes()); f.extend(250u32.to_le_bytes()); // 1/250
            let m = super::read(&mut Cursor::new(f)).unwrap();
            assert_eq!(m.get(Field::Model), Some("M3T"));
            assert_eq!(m.get(Field::FocalLength), Some("19"));
            assert_eq!(m.get(Field::Iso), Some("100"));
            assert_eq!(m.get(Field::Exposure), Some("1-250"));
            assert_eq!(m.get(Field::Taken), None);
        }
    }
}

fn from_exif(data: &exif::Exif) -> Meta {
    use exif::{In, Tag, Value};
    let field = |tag| data.get_field(tag, In::PRIMARY);
    let text = |tag| -> Option<String> {
        match &field(tag)?.value {
            Value::Ascii(parts) => parts
                .first()
                .map(|p| String::from_utf8_lossy(p).trim_matches(|c: char| c == '\0' || c.is_whitespace()).to_owned()),
            _ => None,
        }
    };
    let rational = |tag| -> Option<(u32, u32)> {
        match &field(tag)?.value {
            Value::Rational(v) => v.first().filter(|r| r.denom != 0).map(|r| (r.num, r.denom)),
            _ => None,
        }
    };
    let uint = |tag| field(tag).and_then(|f| f.value.get_uint(0));

    let mut m = Meta::default();
    if let Some((n, d)) = rational(Tag::FocalLength) {
        m.set(Field::FocalLength, number(n as f64 / d as f64));
    }
    if let Some(v) = uint(Tag::FocalLengthIn35mmFilm).filter(|v| *v > 0) {
        m.set(Field::FocalLength35, v.to_string());
    }
    if let Some(v) = text(Tag::Make) {
        m.set(Field::Make, v);
    }
    if let Some(v) = text(Tag::Model) {
        m.set(Field::Model, v);
    }
    if let Some(v) = text(Tag::LensModel) {
        m.set(Field::Lens, v);
    }
    if let Some(v) = text(Tag::DateTimeOriginal).or_else(|| text(Tag::DateTime)) {
        m.set(Field::Taken, taken(&v));
    }
    if let Some(v) = uint(Tag::PhotographicSensitivity) {
        m.set(Field::Iso, v.to_string());
    }
    if let Some((n, d)) = rational(Tag::FNumber) {
        m.set(Field::FNumber, number(n as f64 / d as f64));
    }
    if let Some((n, d)) = rational(Tag::ExposureTime) {
        // 1/250 s → „1-250" (kaldkriips pole failinimes lubatud).
        let text = if n > 0 && n < d { format!("1-{}", (d as f64 / n as f64).round()) } else { number(n as f64 / d as f64) };
        m.set(Field::Exposure, text);
    }
    if let (Some(w), Some(h)) = (uint(Tag::PixelXDimension), uint(Tag::PixelYDimension)) {
        m.set(Field::Size, format!("{w}x{h}"));
    }
    let degrees = |tag| -> Option<f64> {
        match &field(tag)?.value {
            Value::Rational(v) if v.len() >= 3 => Some(dms([(v[0].num, v[0].denom), (v[1].num, v[1].denom), (v[2].num, v[2].denom)])?),
            _ => None,
        }
    };
    if let (Some(lat), Some(lon)) = (degrees(Tag::GPSLatitude), degrees(Tag::GPSLongitude)) {
        set_position(&mut m, lat, text(Tag::GPSLatitudeRef).as_deref(), lon, text(Tag::GPSLongitudeRef).as_deref());
    }
    m
}

/// Kraadid, minutid, sekundid (murdudena) → kraadid.
fn dms(parts: [(u32, u32); 3]) -> Option<f64> {
    let mut out = 0.0;
    for (k, (n, d)) in parts.into_iter().enumerate() {
        if d == 0 {
            return None;
        }
        out += f64::from(n) / f64::from(d) / [1.0, 60.0, 3600.0][k];
    }
    Some(out)
}

/// GPS-asukoht „lat,lon" (lõuna / lääs miinusega); 0,0 = asukohta pole.
fn set_position(m: &mut Meta, lat: f64, lat_ref: Option<&str>, lon: f64, lon_ref: Option<&str>) {
    if m.get(Field::Position).is_some() || (lat == 0.0 && lon == 0.0) {
        return;
    }
    let lat = if lat_ref.is_some_and(|r| r.trim().eq_ignore_ascii_case("S")) { -lat } else { lat };
    let lon = if lon_ref.is_some_and(|r| r.trim().eq_ignore_ascii_case("W")) { -lon } else { lon };
    m.set(Field::Position, format!("{lat:.6},{lon:.6}"));
}

/// Drooni asend XMP-st (DJI `drone-dji:` väljad); teistel failidel neid pole.
fn drone(data: &[u8], m: &mut Meta) {
    const NS: &[u8] = b"drone-dji:";
    let Some(start) = data.windows(NS.len()).position(|w| w == NS) else { return };
    let text = String::from_utf8_lossy(&data[start..data.len().min(start + 32 * 1024)]);
    let get = |key: &str| xmp_number(&text, &format!("drone-dji:{key}"));
    if let Some(v) = get("RelativeAltitude") {
        m.set(Field::Altitude, format!("{v:.1}"));
    }
    if let Some(v) = get("GimbalYawDegree") {
        m.set(Field::CameraYaw, format!("{v:.1}"));
    }
    if let Some(v) = get("GimbalPitchDegree") {
        m.set(Field::CameraPitch, format!("{v:.1}"));
    }
    if let (Some(lat), Some(lon)) = (get("GpsLatitude"), get("GpsLongitude")) {
        set_position(m, lat, None, lon, None);
    }
}

/// XMP väli arvuna: `tag="+71.70"` (atribuut) või `<tag>71.70</tag>`.
fn xmp_number(text: &str, tag: &str) -> Option<f64> {
    let mut from = 0;
    while let Some(i) = text[from..].find(tag) {
        let rest = text[from + i + tag.len()..].trim_start();
        from += i + tag.len();
        let value = if let Some(r) = rest.strip_prefix('=') {
            let r = r.trim_start();
            let quote = r.chars().next().filter(|c| matches!(c, '"' | '\''))?;
            let r = &r[1..];
            &r[..r.find(quote)?]
        } else if let Some(r) = rest.strip_prefix('>') {
            &r[..r.find('<')?]
        } else {
            continue;
        };
        return value.trim().parse().ok();
    }
    None
}

/// Arv failinimesse: täisarv ilma komakohata („19"), muidu üks koht („4.5").
fn number(value: f64) -> String {
    if (value - value.round()).abs() < 0.05 { format!("{:.0}", value) } else { format!("{:.1}", value) }
}

/// „2026:10:16 17:48:08" → „20261016-174808".
fn taken(text: &str) -> String {
    let digits: String = text.chars().filter(char::is_ascii_digit).collect();
    if digits.len() >= 14 { format!("{}-{}", &digits[..8], &digits[8..14]) } else { digits }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_and_dates_for_file_names() {
        assert_eq!(number(19.0), "19");
        assert_eq!(number(4.5), "4.5");
        assert_eq!(number(30.99), "31");
        assert_eq!(Meta::with(Field::Lens, "24.0 mm f/2.8").get(Field::Lens), Some("24.0 mm f-2.8"));
        assert_eq!(taken("2026:10:16 17:48:08"), "20261016-174808");
    }

    /// Fail ilma EXIF-ita: tühi, mitte viga.
    #[test]
    fn drone_pose_from_xmp() {
        // DJI DNG: atribuudid; mõnel failil elemendid. `GpsLatitudeRef` ei sega.
        let xmp = br#"<rdf:Description xmlns:drone-dji="http://www.dji.com/drone-dji/1.0/"
            drone-dji:GpsLatitudeRef="N" drone-dji:GpsLatitude="+58.227523280"
            drone-dji:GpsLongitude="+26.382926971" drone-dji:RelativeAltitude="+51.700"
            drone-dji:GimbalYawDegree="-71.70"><drone-dji:GimbalPitchDegree>-7.40</drone-dji:GimbalPitchDegree>"#;
        let mut m = Meta::default();
        drone(xmp, &mut m);
        assert_eq!(m.get(Field::Altitude), Some("51.7"));
        assert_eq!(m.get(Field::CameraYaw), Some("-71.7"));
        assert_eq!(m.get(Field::CameraPitch), Some("-7.4"));
        assert_eq!(m.get(Field::Position), Some("58.227523,26.382927"));
        let mut none = Meta::default();
        drone(b"<x:xmpmeta>tiff:Make=\"Canon\"</x:xmpmeta>", &mut none);
        assert!(none.is_empty());
    }

    /// Väike CR3: ftyp, moov → Canoni uuid → CMT1 (Make, Model) ja CMT2
    /// (ISO, säri). Kast kasti sees, TIFF-id nihetega enda alguse suhtes.
    #[test]
    fn canon_cr3_boxes() {
        fn tiff(entries: &[(u16, u16, u32, Vec<u8>)]) -> Vec<u8> {
            // II, IFD @8; pikad väärtused IFD järel.
            let mut out = b"II*\0".to_vec();
            out.extend(8u32.to_le_bytes());
            let mut data_off = 8 + 2 + entries.len() as u32 * 12 + 4;
            let mut data: Vec<u8> = Vec::new();
            out.extend((entries.len() as u16).to_le_bytes());
            for (tag, kind, count, value) in entries {
                out.extend(tag.to_le_bytes());
                out.extend(kind.to_le_bytes());
                out.extend(count.to_le_bytes());
                if value.len() <= 4 {
                    let mut v = value.clone();
                    v.resize(4, 0);
                    out.extend(v);
                } else {
                    out.extend(data_off.to_le_bytes());
                    data_off += value.len() as u32;
                    data.extend(value);
                }
            }
            out.extend(0u32.to_le_bytes());
            out.extend(data);
            out
        }
        fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
            let mut out = ((body.len() + 8) as u32).to_be_bytes().to_vec();
            out.extend(kind);
            out.extend(body);
            out
        }
        let cmt1 = tiff(&[(0x010F, 2, 6, b"Canon\0".to_vec()), (0x0110, 2, 15, b"Canon EOS R6m2\0".to_vec())]);
        let mut exposure = 1u32.to_le_bytes().to_vec();
        exposure.extend(500u32.to_le_bytes());
        let cmt2 = tiff(&[(0x829A, 5, 1, exposure), (0x8827, 3, 1, 100u16.to_le_bytes().to_vec())]);
        let mut uuid = vec![0x85, 0xc0, 0xb6, 0x87, 0x82, 0x0f, 0x11, 0xe0, 0x81, 0x11, 0xf4, 0xce, 0x46, 0x2b, 0x6a, 0x48];
        uuid.extend(bx(b"CNCV", b"CanonCR3"));
        uuid.extend(bx(b"CMT1", &cmt1));
        uuid.extend(bx(b"CMT2", &cmt2));
        // GPS: 58° 22' 48" N, 26° 43' 12" E.
        let deg = |d: u32, m: u32, s: u32| [d, 1, m, 1, s, 1].iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>();
        let cmt4 = tiff(&[(1, 2, 2, b"N\0".to_vec()), (2, 5, 3, deg(58, 22, 48)), (3, 2, 2, b"E\0".to_vec()), (4, 5, 3, deg(26, 43, 12))]);
        uuid.extend(bx(b"CMT4", &cmt4));
        let mut file = bx(b"ftyp", b"crx \0\0\0\x01crx isom");
        file.extend(bx(b"moov", &bx(b"uuid", &uuid)));
        let m = cr3(&file).unwrap();
        assert_eq!(m.get(Field::Make), Some("Canon"));
        assert_eq!(m.get(Field::Model), Some("Canon EOS R6m2"));
        assert_eq!(m.get(Field::Iso), Some("100"));
        assert_eq!(m.get(Field::Exposure), Some("1-500"));
        assert_eq!(m.get(Field::Position), Some("58.380000,26.720000"));
        assert!(cr3(b"\0\0\0\x10ftypcrx \0\0\0\0").is_none());
    }

    #[test]
    fn plain_file_has_no_data() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("a.txt");
        std::fs::write(&path, "tere").unwrap();
        assert!(read(&path).is_empty());
    }
}
