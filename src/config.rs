//! Seaded: `%APPDATA%\iview\config.toml` (käsitsi loetav ja kirjutatav).
//! `IVIEW_CONFIG` annab katseks muu faili — katsekäivitus ei tohi kasutaja
//! seadeid (akna kohta) üle kirjutada.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

pub const CONFIG_ENV: &str = "IVIEW_CONFIG";

macro_rules! choice {
    ($(#[$doc:meta])* $name:ident { $($(#[$vdoc:meta])* $variant:ident),+ $(,)? } default $def:ident) => {
        $(#[$doc])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($(#[$vdoc])* $variant),+ }
        impl Default for $name {
            fn default() -> Self { $name::$def }
        }
        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];
            pub fn index(self) -> usize { Self::ALL.iter().position(|v| *v == self).unwrap_or(0) }
            pub fn nth(i: usize) -> Self { Self::ALL.get(i).copied().unwrap_or_default() }
        }
    };
}

choice!(/// Mis vaade pilt avanedes saab.
    OpenView { Fit, OneToOne, Last } default Fit);
choice!(/// Aken pildi järgi (nagu IrfanView' „Display options"): kehtib igale
    /// avatud pildile. F vahetab `ToScreen` ↔ `ImageSize`; Shift+F, Shift+O,
    /// Shift+W valivad otse.
    WindowFit {
        /// Aken jääb, pilt mahutatakse aknasse (Shift+W).
        Off,
        /// Suur pilt vähendatakse ekraanile, väike 1:1; aken pildi mõõtu (Shift+F).
        BigToScreen,
        /// Pilt ekraanile (ka väike suuremaks); aken pildi mõõtu (F).
        ToScreen,
        /// Pilt 1:1, aken pildi mõõtu, kuni ekraan lubab (Shift+O, F).
        ImageSize,
    } default Off);
choice!(/// Hiire ratas ilma Ctrl-ita.
    WheelMode { Pictures, Zoom } default Pictures);
choice!(/// + − samm.
    ZoomStep { Steps, Ten, TwentyFive } default Steps);
choice!(/// Pildi tagune.
    Background { Dark, Black, Gray } default Dark);
choice!(/// Läbipaistev ala (PNG, WebP).
    Transparent { Background, Checker } default Background);
choice!(/// Ctrl+R originaalist suuremaks.
    Enlarge { Never, Warn, Free } default Warn);
choice!(/// Salvestamata muudatus teisele pildile minnes või sulgedes.
    Unsaved { Drop, Ask } default Drop);
choice!(/// Kausta lõpus.
    AtEnd { Stop, Wrap, NextFolder } default Stop);
choice!(/// RAW-failid: kaamera JPEG konteinerist või Windowsi koodek (toorpilt).
    RawSource { CameraJpeg, Codec } default CameraJpeg);
choice!(/// Täisekraani tekst.
    FullText { Hidden, Shown, Flash } default Shown);
choice!(/// Täisekraani teksti koht.
    Corner { TopLeft, TopRight, BottomLeft, BottomRight } default TopLeft);
choice!(/// Täisekraani kirja suurus.
    TextSize { Small, Medium, Large } default Medium);
choice!(/// Kausta piltide järjekord.
    SortBy { Name, Modified, Size } default Name);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    // ---- avamine ----
    /// Laiendid, mis kaustas pildina loendisse lähevad (tühikuga).
    pub exts: String,
    pub raw: RawSource,
    /// Programm avaneb kohe täisekraanil; üks Esc sulgeb.
    pub open_fullscreen: bool,
    /// Teine avamine (Explorerist topeltklõps) läheb juba lahti olevasse
    /// aknasse — protsessi, graafikat ja akent uuesti ei looda.
    pub single_instance: bool,
    pub sort: SortBy,
    // ---- vaatamine ----
    pub open_view: OpenView,
    pub window_fit: WindowFit,
    pub wheel: WheelMode,
    pub zoom_step: ZoomStep,
    pub background: Background,
    pub transparent: Transparent,
    pub full_path_title: bool,
    /// Mahutades ka väike pilt suuremaks.
    pub enlarge_small: bool,
    /// Pööra EXIF-i järgi (väljas: nagu fail salvestas).
    pub exif_rotate: bool,
    pub hints: bool,
    pub meta_bar: bool,
    // ---- vähendamine ----
    /// Kiirvalikud pikslites tühikuga.
    pub quick_sides: String,
    pub enlarge: Enlarge,
    /// Viimati kasutatud pikem külg.
    pub last_long_side: u32,
    /// Valiku suhe (Shift+C; Ctrl+lohistus hoiab seda): „3:2", „2.370:1";
    /// tühi = vaba.
    pub sel_ratio: String,
    // ---- salvestamine ----
    /// Nimemall ilma laiendita; kaust on alati foto oma.
    pub save_template: String,
    pub quality_from_original: bool,
    /// Kvaliteet, kui originaalilt ei saa (ja viimati kasutatu).
    pub quality: u8,
    pub goto_saved: bool,
    pub keep_exif: bool,
    // ---- failid ----
    pub ask_delete: bool,
    /// Kaasfailide laiendid („xmp thm"): F2 ja Del võtavad kaasa.
    pub sidecars: String,
    /// Pildiredaktor (F4); tühi = Windowsi „Muuda" (nt Paint).
    pub editor: String,
    // ---- liikumine ----
    pub unsaved: Unsaved,
    pub at_end: AtEnd,
    // ---- täisekraan ----
    pub text: FullText,
    pub text_line1: String,
    pub text_line2: String,
    pub text_corner: Corner,
    pub text_size: TextSize,
    pub font_size: f32,
    /// „Avanedes: nagu eelmine": F-i olek eelmisel sulgemisel.
    pub last_one_to_one: bool,
    pub last_fill: bool,
    // ---- aken ----
    /// Akna koht füüsilistes pikslites (vt `platform::Placement`); `None`:
    /// esimene käivitus, aken keskele vaikimisi mõõdus.
    pub window: Option<crate::platform::Placement>,
}

/// Vaikimisi laiendid: tavalised pildivormingud ja levinud RAW-d (RAW
/// avaneb kaamera JPEG-ist, koodekit pole vaja).
pub const DEFAULT_EXTS: &str = "jpg jpeg jpe jfif png gif bmp webp tif tiff heic heif avif ico tga pnm ppm pgm svg dng crw cr2 cr3 nef nrw arw orf rw2 raf pef srw";
pub const DEFAULT_QUICK_SIDES: &str = "1080 1600 1920 2048 3840";
pub const DEFAULT_SAVE_TEMPLATE: &str = "{name}_{long_side}";
pub const DEFAULT_SIDECARS: &str = "xmp thm";
pub const DEFAULT_TEXT_1: &str = "{folder} › {name}  {index} / {total}";
pub const DEFAULT_TEXT_2: &str = "{camera} · {focal} · {aperture} · {exposure} · ISO {iso} · {taken:%Y-%m-%d %H:%M}";

impl Default for Config {
    fn default() -> Self {
        Self {
            exts: DEFAULT_EXTS.to_owned(),
            raw: RawSource::default(),
            open_fullscreen: false,
            single_instance: true,
            sort: SortBy::default(),
            open_view: OpenView::default(),
            window_fit: WindowFit::default(),
            wheel: WheelMode::default(),
            zoom_step: ZoomStep::default(),
            background: Background::default(),
            transparent: Transparent::default(),
            full_path_title: false,
            enlarge_small: false,
            exif_rotate: true,
            hints: true,
            meta_bar: true,
            quick_sides: DEFAULT_QUICK_SIDES.to_owned(),
            enlarge: Enlarge::default(),
            last_long_side: 2048,
            sel_ratio: String::new(),
            save_template: DEFAULT_SAVE_TEMPLATE.to_owned(),
            quality_from_original: true,
            quality: 90,
            goto_saved: true,
            keep_exif: true,
            ask_delete: true,
            sidecars: DEFAULT_SIDECARS.to_owned(),
            editor: String::new(),
            unsaved: Unsaved::default(),
            at_end: AtEnd::default(),
            text: FullText::default(),
            text_line1: DEFAULT_TEXT_1.to_owned(),
            text_line2: DEFAULT_TEXT_2.to_owned(),
            text_corner: Corner::default(),
            text_size: TextSize::default(),
            font_size: 14.0,
            last_one_to_one: false,
            last_fill: false,
            window: None,
        }
    }
}

impl Config {
    /// Seadete fail: `IVIEW_CONFIG` või `%APPDATA%\iview\config.toml`.
    pub fn path() -> Option<PathBuf> {
        if let Some(p) = std::env::var_os(CONFIG_ENV).filter(|p| !p.is_empty()) {
            return Some(PathBuf::from(p));
        }
        let base = std::env::var_os("APPDATA").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
        Some(base.join("iview").join("config.toml"))
    }

    /// Loeb seaded; puudub või vigane → vaikimisi (vigane fail jääb alles
    /// `.bad` nime all, et kasutaja töö ei kaoks).
    pub fn load() -> Self {
        let Some(path) = Self::path() else { return Self::default() };
        let Ok(text) = std::fs::read_to_string(&path) else { return Self::default() };
        match toml::from_str::<Config>(&text) {
            Ok(mut c) => {
                c.tidy();
                c
            }
            Err(_) => {
                let _ = std::fs::copy(&path, path.with_extension("toml.bad"));
                Self::default()
            }
        }
    }

    /// Kirjutab seaded (ajutine fail ja ümbernimetus — poolikut faili ei jää).
    pub fn save(&self) -> Result<(), String> {
        let path = Self::path().ok_or("no settings folder")?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| e.to_string())?;
        let temp = path.with_extension("toml.part");
        std::fs::write(&temp, text).map_err(|e| e.to_string())?;
        std::fs::rename(&temp, &path).map_err(|e| e.to_string())
    }

    /// Laiendid väikeste tähtedega, ilma punktita.
    pub fn ext_list(&self) -> Vec<String> {
        exts_of(&self.exts)
    }

    /// Kiirvalikud pikslites, kasvavalt.
    pub fn quick_sides(&self) -> Vec<u32> {
        let mut v: Vec<u32> = self.quick_sides.split([' ', ',', ';']).filter_map(|w| w.trim().parse().ok()).filter(|&n: &u32| n >= 16).collect();
        v.sort_unstable();
        v.dedup();
        v
    }

    /// Kaasfailide laiendid.
    pub fn sidecar_exts(&self) -> Vec<String> {
        exts_of(&self.sidecars)
    }

    /// Seade, mis mõjutab dekoodreid: RAW-i allikas. Käivitusel ja salvestamisel.
    pub fn apply(&self) {
        crate::raw::set_prefer_codec(self.raw == RawSource::Codec);
    }

    /// Korrastab vabad väljad (laiendid, kiirvalikud).
    pub fn tidy(&mut self) {
        self.exts = exts_of(&self.exts).join(" ");
        if self.exts.is_empty() {
            self.exts = DEFAULT_EXTS.to_owned();
        }
        self.sidecars = exts_of(&self.sidecars).join(" ");
        self.quick_sides = self.quick_sides().iter().map(u32::to_string).collect::<Vec<_>>().join(" ");
        if self.quick_sides.is_empty() {
            self.quick_sides = DEFAULT_QUICK_SIDES.to_owned();
        }
        if self.save_template.trim().is_empty() {
            self.save_template = DEFAULT_SAVE_TEMPLATE.to_owned();
        }
        self.quality = self.quality.clamp(1, 100);
        self.font_size = self.font_size.clamp(10.0, 24.0);
    }
}

/// „jpg, .PNG crw" → ["jpg", "png", "crw"] (koma või tühik vahel, punkt ees
/// lubatud, kordused välja).
pub fn exts_of(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for word in text.split([' ', ',', ';', '\t', '\n']) {
        let ext = word.trim().trim_start_matches('*').trim_start_matches('.').to_ascii_lowercase();
        if !ext.is_empty() && !out.contains(&ext) {
            out.push(ext);
        }
    }
    out
}

pub fn has_ext(path: &Path, exts: &[String]) -> bool {
    path.extension().is_some_and(|e| {
        let e = e.to_string_lossy().to_ascii_lowercase();
        exts.contains(&e)
    })
}

/// Faili kaasfailid, mis on olemas: sama nimi, kaasfaili laiend (väike või
/// suur täht). F2 ja Del võtavad need kaasa.
pub fn sidecars(path: &Path, exts: &[String]) -> Vec<PathBuf> {
    let Some(stem) = path.file_stem() else { return Vec::new() };
    let parent = path.parent().unwrap_or(Path::new(""));
    let mut out: Vec<PathBuf> = Vec::new();
    let same = |a: &Path, b: &Path| a.to_string_lossy().eq_ignore_ascii_case(&b.to_string_lossy());
    for ext in exts {
        // Windowsi kettal on „.xmp" ja „.XMP" sama fail: üks kord.
        for e in [ext.clone(), ext.to_ascii_uppercase()] {
            let candidate = parent.join(format!("{}.{e}", stem.to_string_lossy()));
            if !same(&candidate, path) && candidate.is_file() && !out.iter().any(|o| same(o, &candidate)) {
                out.push(candidate);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Loetelus on koma ja punkt lubatud, suurtähed ei loe, kordused välja.
    #[test]
    fn extension_list_is_forgiving() {
        assert_eq!(exts_of("jpg, .PNG crw;*.dng  jpg"), ["jpg", "png", "crw", "dng"]);
        assert!(exts_of("  ").is_empty());
        assert!(has_ext(Path::new("C:\\pildid\\IMG_0001.JPG"), &exts_of("jpg")));
        assert!(!has_ext(Path::new("C:\\pildid\\laiendita"), &exts_of("jpg")));
    }

    /// Kiirvalikud: korrastatud, kasvavalt, kordusteta; tühi → vaikimisi.
    #[test]
    fn quick_sides_are_tidied() {
        let mut c = Config { quick_sides: "2048, 1080 1080 x 1920".into(), ..Default::default() };
        assert_eq!(c.quick_sides(), [1080, 1920, 2048]);
        c.tidy();
        assert_eq!(c.quick_sides, "1080 1920 2048");
        c.quick_sides = "x".into();
        c.tidy();
        assert_eq!(c.quick_sides, DEFAULT_QUICK_SIDES);
    }

    /// Osaline seadistus loeb sisse, ülejäänu vaikimisi; tagasi kirjutades
    /// valikud väikeste tähtedega.
    #[test]
    fn partial_config_loads() {
        let c: Config = toml::from_str("single_instance = false\nexts = \"jpg\"\n[window]\nx = 10\ny = 20\nw = 900\nh = 700\nmaximized = true\n").unwrap();
        assert!(!c.single_instance && c.exts == "jpg");
        assert_eq!(c.window, Some(crate::platform::Placement { x: 10, y: 20, w: 900, h: 700, maximized: true }));
        assert_eq!(c.quality, 90);
        let back = toml::to_string(&c).unwrap();
        assert!(back.contains("raw = \"camera_jpeg\""));
    }

    #[test]
    fn sidecars_next_to_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let cr3 = dir.path().join("IMG_0001.CR3");
        std::fs::write(&cr3, b"x").unwrap();
        std::fs::write(dir.path().join("IMG_0001.xmp"), b"x").unwrap();
        std::fs::write(dir.path().join("IMG_0002.xmp"), b"x").unwrap();
        let found = sidecars(&cr3, &exts_of("xmp thm"));
        assert_eq!(found, vec![dir.path().join("IMG_0001.xmp")]);
    }
}
