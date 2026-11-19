//! Vaaturi aken. Pilt pakitakse kohe akna mõõtu lahti (JPEG-il
//! dekoodri sees, RAW-ist kaamera JPEG), naabrid ette liikumise suunas
//! (`loader`); nooltega liikudes vahetub ainult tekstuur. Mahutatud või
//! kindel suurendus; 1:1 ja suurendusel pakitakse täissuurus taustal.
//!
//! Muutmine ja salvestamine on eraldi sammud: Ctrl+R (pikem külg), R / L
//! (pööre), H / V (peegel) muudavad pilti aknas — faili ei puututa, Ctrl+Z
//! võtab tagasi — ja Ctrl+S kirjutab koopia (mall, sama kaust, JPEG). F2
//! nimi, Del prügikasti, F4 redaktor, Enter täisekraan tekstiga (mall),
//! Ctrl+C / Ctrl+V lõikelaud, P seaded.
//!
//! Aken sünnib peidetuna ja kaetuna (DWM cloak) ning tuleb nähtavale alles
//! siis, kui esimene pilt on joonistatud õiges mõõdus — tühja ega vale
//! mõõduga akent kasutaja ei näe.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use eframe::egui::{self, Align2, Color32, CornerRadius, FontId, Key, Modifiers, Rect, Sense, Stroke, Vec2};

use crate::config::{AtEnd, Background, Config, Enlarge, FullText, OpenView, Transparent, Unsaved, WheelMode, ZoomStep};
use crate::folder::{self, Item};
use crate::loader::Loader;
use crate::save::{Edit, fit_dims};
use crate::template;

pub const NAME: &str = "iview";
pub const MIN_WINDOW_SIZE: [f32; 2] = [480.0, 360.0];

/// Ette lahti pakitavaid pilte liikumise suunas ja tagasi.
const AHEAD: usize = 4;
const BEHIND: usize = 2;
/// Andmeriba pildi kohal.
const META_H: f32 = 32.0;
/// Täissuuruse pikim külg (graafikakaardi tekstuuri piir).
const SHARP_SIDE: u32 = 8192;
/// Suurendamise ülempiir: üle kahekordse originaali või 16 000 px ei tee.
const MAX_ENLARGE: f32 = 2.0;
const MAX_SIDE: u32 = 16_000;
/// Suurenduse astmed (+ −), 1.0 = üks pildipiksel = üks ekraanipiksel.
const ZOOM_STEPS: [f32; 17] = [0.05, 0.1, 0.15, 0.2, 0.25, 0.33, 0.5, 0.67, 0.75, 1.0, 1.5, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0];
const BOX_H: f32 = 38.0;
/// Ruum klahvireale pildi all.
const HINT_ROOM: f32 = 38.0;
/// Täisekraanil kaob kursor nii mitme sekundi pärast.
const CURSOR_HIDE: Duration = Duration::from_secs(2);
/// „3 s vahetusel": tekst pildi vahetudes nii kaua.
const FLASH: Duration = Duration::from_secs(3);
/// Kate maha hiljemalt nii kaua pärast nähtavaks tegemist (pilt veel
/// laadimas: siis „Laadin…" riba, mitte tühi ekraan).
const CLOAK_LIMIT: Duration = Duration::from_millis(600);

/// Tume palett (pildi taust on tume; heledad kastid pimestaksid).
#[derive(Clone, Copy)]
pub struct Palette {
    pub panel: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_faint: Color32,
    pub border: Color32,
    pub danger: Color32,
    pub accent: Color32,
}

pub const PALETTE: Palette = Palette {
    panel: Color32::from_rgb(0x24, 0x29, 0x31),
    text: Color32::from_rgb(0xEC, 0xEF, 0xF3),
    text_dim: Color32::from_rgb(0xAE, 0xB7, 0xC4),
    text_faint: Color32::from_rgb(0x6E, 0x77, 0x84),
    border: Color32::from_rgb(0x3C, 0x43, 0x4E),
    danger: Color32::from_rgb(0xC9, 0x3B, 0x3B),
    accent: Color32::from_rgb(0x4C, 0x8B, 0xF5),
};
const META: Color32 = Color32::from_rgb(0x15, 0x18, 0x1D);

/// F2: nimekast pildi all.
struct Rename {
    text: String,
    /// Esimene kaader: nimi märgitud.
    fresh: bool,
    error: Option<String>,
}

/// Ctrl+R: pikem külg.
struct ResizeBox {
    text: String,
    fresh: bool,
    error: Option<String>,
}

/// Ctrl+S: nimi ja kvaliteet.
struct SaveBox {
    name: String,
    quality: String,
    fresh: bool,
    error: Option<String>,
    /// Nimi on juba olemas (hoiatus, „ (2)" pakutud).
    exists: Option<String>,
}

/// Mida liikumine ootas, kui salvestamata muudatus küsib.
enum Pending {
    Cursor(usize),
    Close,
    Folder(bool),
    Open(PathBuf),
}

/// Aknas lahti olev küsimus või kast; klahvid lähevad sellele.
enum Overlay {
    Rename(Rename),
    AskDelete,
    Resize(ResizeBox),
    Save(SaveBox),
    AskUnsaved(Pending),
    Settings(Box<crate::settings::Draft>),
}

/// Pildi EXIF täisekraani teksti ja riba jaoks, üks kord faili kohta.
#[derive(Default)]
struct ExifInfo {
    meta: crate::exif::Meta,
    orientation: Option<u16>,
    /// `{exif:Silt}` väljad.
    fields: Vec<exif::Field>,
    quality: Option<u8>,
}

/// Salvestamise tulemus taustalõimest.
type Saved = Result<(PathBuf, u64), String>;

/// Lõikelaua töö tulemus taustalõimest (Ctrl+C / Ctrl+V).
enum ClipDone {
    Copied(Result<(), String>),
    /// Kleebitud pilt ja nimi (kui tuli failist).
    Pasted(Result<(image::DynamicImage, Option<String>), String>),
}

/// Akna sünd: peidus → nähtav katte all → kate maha.
#[derive(Clone, Copy, PartialEq)]
enum Birth {
    Hidden,
    Cloaked { since: Instant, fitting: u8 },
    Done,
}

/// Mida klahvid ja hiir aknalt paluvad (kaadri lõpus).
#[derive(Default)]
struct Out {
    close: bool,
    fit_window: Option<(Vec2, Vec2)>,
    /// Enter: täisekraan sisse / välja.
    fullscreen: Option<bool>,
    /// Kausta lõpus järgmisse (`true`) või eelmisse kausta.
    folder: Option<bool>,
    edit_file: Option<PathBuf>,
    open_dialog: bool,
    open: Option<PathBuf>,
}

/// Akna käivitusandmed `main`-ist.
pub struct Start {
    pub config: Config,
    pub loader: Loader,
    /// Käsurealt antud fail (või kaust).
    pub path: Option<PathBuf>,
    /// Kausta loend taustal (main alustas selle enne akent).
    pub listing: Option<mpsc::Receiver<Vec<Item>>>,
    /// Segoe UI baidid (loetud taustal).
    pub font: Option<std::thread::JoinHandle<Option<Vec<u8>>>>,
    /// Käsurealt `--fullscreen` (või seade).
    pub fullscreen: bool,
}

pub struct App {
    cfg: Config,
    /// Viimati kettale kirjutatud seaded (salvestus terve seadistuse
    /// võrdlusest, mitte käsitsi lipust).
    saved_cfg: Config,
    save_due: Option<Instant>,
    dir: PathBuf,
    items: Vec<Item>,
    cursor: usize,
    listing: Option<(PathBuf, mpsc::Receiver<Vec<Item>>)>,
    /// Viimane liikumise suund (+1 / −1): naabrid ette sinnapoole.
    direction: i8,
    /// `None`: mahutatud aknasse; `Some`: kindel suurendus, 1.0 = 1:1.
    zoom: Option<f32>,
    /// Mahutatud ka väikese pildi suurendusega — F kolmas olek.
    fill: bool,
    /// Pildi keskpunkti nihe ala keskpunktist (punktides).
    pan: Vec2,
    edit: Edit,
    undo: Vec<Edit>,
    overlay: Option<Overlay>,
    /// Teade riba paremas servas (viga punaselt, tehtu roheliselt).
    note: Option<(String, bool)>,
    loader: Loader,
    exif: HashMap<PathBuf, ExifInfo>,
    /// Taustal loetud EXIF ja teed, mille lugemine käib: riba ja
    /// täisekraani tekst ei loe faili UI-lõimes.
    exif_done: Arc<Mutex<Vec<(PathBuf, ExifInfo)>>>,
    exif_asked: HashSet<PathBuf>,
    saving: Option<mpsc::Receiver<Saved>>,
    clip: Option<mpsc::Receiver<ClipDone>>,
    /// V oli eelmisel kaadril all (Ctrl+V kord vajutuse kohta).
    paste_held: bool,
    /// Klahvirea kasti klõps: klahv järgmise kaadri sisendisse.
    pending_key: Option<(Modifiers, Key)>,
    fullscreen: bool,
    /// Avati täisekraanil (seade): üks Esc sulgeb akna.
    esc_closes: bool,
    pointer_moved: Instant,
    changed_at: Instant,
    /// Akna käepide ja sünni järk.
    hwnd: Option<isize>,
    birth: Birth,
    /// „Nagu eelmine": F-i olek eelmisel sulgemisel.
    last_title: String,
    /// Ajamõõt: pilt ekraanil (käivitus või pildi vahetus).
    timing: Option<(Instant, PathBuf)>,
    started: Instant,
    /// Katse (`IVIEW_KEYS`): klahvid, mis süstitakse ükshaaval sisendisse,
    /// kui pilt on ekraanil — sünteetilised klahvivajutused aknani ei jõua.
    test_keys: std::collections::VecDeque<egui::Event>,
    test_next: Instant,
}

/// `IVIEW_KEYS="Right Right R Ctrl+S 'uus_nimi Enter"` → sündmused
/// (`'` algusega sõna on sisestatud tekst).
fn parse_test_keys(text: &str) -> std::collections::VecDeque<egui::Event> {
    text.split_whitespace()
        .filter_map(|word| {
            if let Some(typed) = word.strip_prefix('\'') {
                return Some(egui::Event::Text(typed.to_owned()));
            }
            let mut mods = Modifiers::NONE;
            let mut name = word;
            while let Some((m, rest)) = name.split_once('+').filter(|(m, r)| !m.is_empty() && !r.is_empty()) {
                match m.to_ascii_lowercase().as_str() {
                    "ctrl" => mods |= Modifiers::COMMAND,
                    "shift" => mods |= Modifiers::SHIFT,
                    "alt" => mods |= Modifiers::ALT,
                    _ => return None,
                }
                name = rest;
            }
            Key::from_name(name).map(|key| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: mods })
        })
        .collect()
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, start: Start) -> Self {
        let ctx = &cc.egui_ctx;
        crate::startup::mark("window created");
        // Tume teema kindlalt (süsteemi hele teema kirjutaks `set_visuals` üle).
        ctx.set_theme(egui::Theme::Dark);
        if let Some(font) = start.font.and_then(|h| h.join().ok()).flatten() {
            let mut fonts = egui::FontDefinitions::default();
            fonts.font_data.insert("segoe".into(), Arc::new(egui::FontData::from_owned(font)));
            fonts.families.entry(egui::FontFamily::Proportional).or_default().insert(0, "segoe".into());
            ctx.set_fonts(fonts);
        }
        start.loader.attach(ctx);
        let mut loader = start.loader;
        loader.set_orientation(start.config.exif_rotate);
        let cfg = start.config;
        let (dir, items) = match &start.path {
            Some(p) if p.is_file() => (p.parent().map(Path::to_path_buf).unwrap_or_default(), vec![Item::file(p.clone())]),
            Some(p) => (p.clone(), Vec::new()),
            None => (PathBuf::new(), Vec::new()),
        };
        let (zoom, fill) = match cfg.open_view {
            OpenView::OneToOne => (Some(1.0), false),
            OpenView::Fit => (None, false),
            OpenView::Last => (cfg.last_one_to_one.then_some(1.0), cfg.last_fill && !cfg.last_one_to_one),
        };
        let now = Instant::now();
        let start_fullscreen = start.fullscreen;
        let mut app = Self {
            saved_cfg: cfg.clone(),
            save_due: None,
            listing: start.listing.map(|rx| (dir.clone(), rx)),
            dir,
            items,
            cursor: 0,
            direction: 1,
            zoom,
            fill,
            pan: Vec2::ZERO,
            edit: Edit::default(),
            undo: Vec::new(),
            overlay: None,
            note: None,
            loader,
            exif: HashMap::new(),
            exif_done: Arc::default(),
            exif_asked: HashSet::new(),
            saving: None,
            clip: None,
            paste_held: false,
            pending_key: None,
            fullscreen: false,
            esc_closes: false,
            pointer_moved: now,
            changed_at: now,
            hwnd: None,
            birth: Birth::Hidden,
            last_title: String::new(),
            timing: start.path.filter(|_| crate::startup::timing_on()).map(|p| (now, p)),
            started: now,
            test_keys: std::env::var("IVIEW_KEYS").map(|k| parse_test_keys(&k)).unwrap_or_default(),
            test_next: now,
            cfg,
        };
        // macOS: käsitleja veel kord (AppKit võis selle käivitumisel üle
        // kirjutada); edasi tegeleb failidega aken.
        #[cfg(target_os = "macos")]
        {
            crate::macos_open::install();
            crate::macos_open::set_early(None);
        }
        // Käsureal faili polnud, aga süsteem andis selle käivitumise ajal
        // (macOS: Finder; Windows: teine käivitus samal ajal).
        if app.items.is_empty()
            && app.listing.is_none()
            && let Some(path) = crate::platform::take_incoming().pop()
        {
            app.open_path(ctx, &path);
        }
        if start_fullscreen && (!app.items.is_empty() || app.listing.is_some()) {
            app.fullscreen = true;
            app.esc_closes = true;
        }
        app
    }

    fn current(&self) -> Option<&Item> {
        self.items.get(self.cursor)
    }

    fn current_path(&self) -> Option<PathBuf> {
        self.current().map(|i| i.path.clone())
    }

    /// Kursor uuele pildile: muudatus ja kastid nullist, F-i olek jääb.
    fn set_cursor(&mut self, to: usize) {
        let to = to.min(self.items.len().saturating_sub(1));
        if to != self.cursor {
            self.direction = if to > self.cursor { 1 } else { -1 };
            self.cursor = to;
            self.pan = Vec2::ZERO;
            // F-i olek (mahuta / 1:1 / täida) jääb; +/− vaba suurendus
            // läheb mahutatuks (teisel pildil teine mõõt).
            if self.zoom.is_some_and(|z| z != 1.0) {
                self.zoom = None;
            }
            self.edit = Edit::default();
            self.undo.clear();
            self.overlay = None;
            self.note = None;
            self.changed_at = Instant::now();
            if crate::startup::timing_on() {
                self.timing = self.current_path().map(|p| (Instant::now(), p));
            }
        }
    }

    /// Liikumine salvestamata muudatuse ja kausta lõpu reeglitega.
    fn go(&mut self, to: Option<usize>, forward: bool, out: &mut Out) {
        if self.items.is_empty() {
            return;
        }
        let last = self.items.len() - 1;
        let target = match to {
            Some(t) => Pending::Cursor(t.min(last)),
            None if forward && self.cursor >= last => match self.cfg.at_end {
                AtEnd::Stop => return,
                AtEnd::Wrap => Pending::Cursor(0),
                AtEnd::NextFolder => Pending::Folder(true),
            },
            None if !forward && self.cursor == 0 => match self.cfg.at_end {
                AtEnd::Stop => return,
                AtEnd::Wrap => Pending::Cursor(last),
                AtEnd::NextFolder => Pending::Folder(false),
            },
            None => Pending::Cursor(if forward { self.cursor + 1 } else { self.cursor - 1 }),
        };
        // Ringiga liikudes on suund see, kuhu vajutati.
        self.direction = if forward { 1 } else { -1 };
        self.ask_or(target, out);
    }

    fn ask_or(&mut self, target: Pending, out: &mut Out) {
        if !self.edit.is_empty() && self.cfg.unsaved == Unsaved::Ask {
            self.overlay = Some(Overlay::AskUnsaved(target));
            return;
        }
        self.proceed(target, out);
    }

    fn proceed(&mut self, target: Pending, out: &mut Out) {
        match target {
            Pending::Cursor(to) => {
                let dir = self.direction;
                self.set_cursor(to);
                self.direction = dir;
            }
            Pending::Close => out.close = true,
            Pending::Folder(forward) => out.folder = Some(forward),
            Pending::Open(path) => out.open = Some(path),
        }
    }

    /// Fail või kaust vaaturisse (teine käivitus, lohistamine, Ctrl+O):
    /// sama kausta fail — ainult kursor; muu kaust — uus loend taustal.
    fn open_path(&mut self, ctx: &egui::Context, path: &Path) {
        let path = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        if path.is_dir() {
            let items = folder::list(&path, &self.cfg.ext_list(), self.cfg.sort);
            if items.is_empty() {
                self.note = Some((format!("No pictures in this folder: {}", path.display()), true));
                return;
            }
            self.dir = path;
            self.items = items;
            self.listing = None;
            self.reset_view();
            self.cursor = 0;
        } else if path.is_file() {
            let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
            if dir == self.dir
                && let Some(i) = self.items.iter().position(|it| it.path == path)
            {
                self.set_cursor(i);
            } else {
                self.dir = dir.clone();
                self.items = vec![Item::file(path.clone())];
                self.cursor = 0;
                self.reset_view();
                let (exts, sort) = (self.cfg.ext_list(), self.cfg.sort);
                let (tx, rx) = mpsc::channel();
                let ctx = ctx.clone();
                let _ = std::thread::Builder::new().name("folder-list".into()).spawn(move || {
                    let _ = tx.send(folder::list(&dir, &exts, sort));
                    ctx.request_repaint();
                });
                self.listing = Some((self.dir.clone(), rx));
            }
        } else {
            self.note = Some((format!("File not found: {}", path.display()), true));
            return;
        }
        self.changed_at = Instant::now();
        if crate::startup::timing_on() {
            self.timing = self.current_path().map(|p| (Instant::now(), p));
        }
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }

    fn reset_view(&mut self) {
        self.pan = Vec2::ZERO;
        if self.zoom.is_some_and(|z| z != 1.0) {
            self.zoom = None;
        }
        self.edit = Edit::default();
        self.undo.clear();
        self.overlay = None;
        self.note = None;
    }

    /// Kausta loend taustalt valmis: kursor avatud failile (kui seda
    /// loendis pole — laiend pole loetelus —, jääb see loendisse).
    fn poll_listing(&mut self) {
        let Some((dir, rx)) = &self.listing else { return };
        let Ok(mut items) = rx.try_recv() else { return };
        let dir = dir.clone();
        self.listing = None;
        if dir != self.dir {
            return;
        }
        let current = self.current_path();
        if let Some(cur) = &current
            && !items.iter().any(|i| i.path == *cur)
            && cur.is_file()
        {
            items.push(Item::file(cur.clone()));
            folder::sort_items(&mut items, self.cfg.sort);
        }
        // Kleebitud (mälus olevad) pildid jäävad loendisse.
        let pasted: Vec<Item> = self.items.drain(..).filter(|i| i.clip.is_some()).collect();
        items.extend(pasted);
        self.cursor = current.and_then(|c| items.iter().position(|i| i.path == c)).unwrap_or(0);
        self.items = items;
    }

    /// Originaali mõõdud nagu failis (pööre arvestatud), kui pilt on loetud.
    fn orig_dims(&self) -> Option<(u32, u32)> {
        let path = &self.current()?.path;
        match self.loader.get(path) {
            Some(Ok((texture, dims))) => Some(dims.unwrap_or_else(|| {
                let s = texture.size();
                (s[0] as u32, s[1] as u32)
            })),
            _ => None,
        }
    }

    fn push_undo(&mut self) {
        self.undo.push(self.edit);
        if self.undo.len() > 32 {
            self.undo.remove(0);
        }
    }

    /// Joonistamiseks: EXIF, kui see on loetud; muidu alustab taustal
    /// lugemist ja annab `None` (kaader hiljem uuesti).
    fn exif_ready(&mut self, ctx: &egui::Context, path: &Path) -> Option<&mut ExifInfo> {
        for (p, info) in std::mem::take(&mut *self.exif_done.lock().unwrap_or_else(|e| e.into_inner())) {
            self.exif.entry(p).or_insert(info);
        }
        if !self.exif.contains_key(path) && self.exif_asked.insert(path.to_path_buf()) && path.is_file() {
            let (done, path, ctx) = (self.exif_done.clone(), path.to_path_buf(), ctx.clone());
            let _ = std::thread::Builder::new().name("exif".into()).spawn(move || {
                let info = ExifInfo {
                    meta: crate::exif::read(&path),
                    orientation: crate::exif::orientation(&path),
                    fields: crate::exif::fields(&path).map(|(f, _)| f).unwrap_or_default(),
                    quality: None,
                };
                done.lock().unwrap_or_else(|e| e.into_inner()).push((path, info));
                ctx.request_repaint();
            });
        }
        self.exif.get_mut(path)
    }

    /// Toimingule (nimi pildistamise ajast, salvestuse kvaliteet): loeb
    /// kohe, kui pole veel loetud.
    fn exif_info(&mut self, path: &Path) -> &mut ExifInfo {
        self.exif.entry(path.to_path_buf()).or_insert_with(|| ExifInfo { meta: crate::exif::read(path), orientation: crate::exif::orientation(path), fields: Vec::new(), quality: None })
    }

    /// Enter nimekastis: nimetab ümber (kaasfailid kaasa), jääb samale pildile.
    fn commit_rename(&mut self) {
        let sidecar_exts = self.cfg.sidecar_exts();
        let Some(Overlay::Rename(r)) = self.overlay.as_mut() else { return };
        let name = r.text.trim().to_owned();
        let Some(item) = self.items.get(self.cursor) else { return };
        if name.is_empty() || name == item.stem() {
            self.overlay = None;
            return;
        }
        if let Some(why) = template::invalid_name(&name) {
            r.error = Some(why.to_owned());
            return;
        }
        let target = item.path.with_file_name(format!("{name}{}", item.ext()));
        let same_file = target.to_string_lossy().to_lowercase() == item.path.to_string_lossy().to_lowercase();
        if target.exists() && !same_file {
            r.error = Some(format!("{name}{} already exists", item.ext()));
            return;
        }
        if let Err(e) = std::fs::rename(&item.path, &target) {
            r.error = Some(format!("{}: {e}", item.file_name()));
            return;
        }
        for side in crate::config::sidecars(&item.path, &sidecar_exts) {
            let ext = side.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
            let _ = std::fs::rename(&side, side.with_file_name(format!("{name}.{ext}")));
        }
        let old = item.path.clone();
        self.items[self.cursor].path = target.clone();
        self.loader.renamed(&old, &target);
        if let Some(info) = self.exif.remove(&old) {
            self.exif.insert(target, info);
        }
        self.overlay = None;
    }

    /// Del → Enter: prügikasti (kaasfailid kaasa); loendist välja, kursor
    /// järgmisele.
    fn delete_current(&mut self) {
        let Some(item) = self.current() else { return };
        let path = item.path.clone();
        if item.clip.is_some() {
            self.loader.unpin(&path);
            self.remove_current();
            return;
        }
        if let Err(e) = trash::delete(&path) {
            self.note = Some((format!("{}: {e}", path.display()), true));
            self.overlay = None;
            return;
        }
        for side in crate::config::sidecars(&path, &self.cfg.sidecar_exts()) {
            let _ = trash::delete(&side);
        }
        self.remove_current();
    }

    fn remove_current(&mut self) {
        self.items.remove(self.cursor);
        self.overlay = None;
        self.pan = Vec2::ZERO;
        self.edit = Edit::default();
        self.undo.clear();
        if !self.items.is_empty() {
            self.cursor = self.cursor.min(self.items.len() - 1);
        }
        self.changed_at = Instant::now();
    }

    // ---------- Ctrl+R ----------

    fn open_resize(&mut self) {
        self.overlay = Some(Overlay::Resize(ResizeBox { text: self.cfg.last_long_side.to_string(), fresh: true, error: None }));
    }

    /// Pikem külg kastist: suuremaks ainult hoiatusega (Ctrl+Enter või
    /// nupp) ja mitte üle kahekordse originaali. `force`: „Suurenda ikkagi".
    fn apply_resize(&mut self, force: bool) {
        let Some(orig) = self.orig_dims() else { return };
        let orig_side = orig.0.max(orig.1);
        let enlarge = self.cfg.enlarge;
        let Some(Overlay::Resize(r)) = self.overlay.as_mut() else { return };
        let side = match r.text.trim().parse::<u32>() {
            Ok(s) if s >= 16 => s,
            _ => {
                r.error = Some("Enter the long side in pixels".to_string());
                return;
            }
        };
        if side > orig_side {
            let limit = (((orig_side as f32) * MAX_ENLARGE) as u32).min(MAX_SIDE);
            if side > limit {
                r.error = Some(format!("Not beyond twice the original ({limit} px)"));
                return;
            }
            match enlarge {
                Enlarge::Never => {
                    r.error = Some(format!("Larger than the original ({orig_side} px) — enlarging is turned off"));
                    return;
                }
                Enlarge::Warn if !force => {
                    r.error = Some(format!("Larger than the original ({orig_side} px) — no sharpness is gained, only a bigger file"));
                    return;
                }
                _ => {}
            }
        }
        self.push_undo();
        self.edit.long_side = (side != orig_side).then_some(side);
        self.overlay = None;
        self.pan = Vec2::ZERO;
        self.cfg.last_long_side = side;
    }

    // ---------- Ctrl+S ----------

    /// Nimi mallist (`{name}_{long_side}`), vaba nimi „ (2)" lisaga, kvaliteet
    /// originaalilt või seadest.
    fn open_save(&mut self) {
        let Some(item) = self.current() else { return };
        let path = item.path.clone();
        let stem = item.stem();
        let in_memory = item.clip.is_some();
        let orig = self.orig_dims().unwrap_or((0, 0));
        let (w, h) = self.edit.dims(orig);
        let taken = if in_memory { None } else { self.exif_info(&path).meta.get(crate::exif::Field::Taken).and_then(template::parse_taken) };
        let mut lookup = |name: &str, arg: Option<&str>| -> Option<String> {
            match name {
                "name" => Some(template::slice_arg(&stem, arg)),
                "long_side" => Some(w.max(h).to_string()),
                "width" => Some(w.to_string()),
                "height" => Some(h.to_string()),
                "taken" => taken.map(|t| template::time_arg(t, arg.or(Some("%Y-%m-%d")))),
                _ => None,
            }
        };
        // Kleebitud pilt: nimi on juba uus — malli lisa (mõõt) pole vaja.
        let mut name = if in_memory { stem.clone() } else { template::render(&self.cfg.save_template, &mut lookup) };
        if name.is_empty() {
            name = format!("{stem}_{}", w.max(h));
        }
        let name = template::sanitize(&name);
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_else(|| self.dir.clone());
        let taken_name = |n: &str| dir.join(format!("{n}.jpg")).exists() || dir.join(format!("{n}.JPG")).exists();
        let mut exists = None;
        let free = if taken_name(&name) {
            exists = Some(format!("{name}.jpg"));
            (2..).map(|n| format!("{name} ({n})")).find(|c| !taken_name(c)).unwrap_or(name)
        } else {
            name
        };
        let quality = if self.cfg.quality_from_original && !in_memory {
            let fallback = self.cfg.quality;
            let info = self.exif_info(&path);
            if info.quality.is_none() {
                info.quality = crate::jpeg::quality_of(&path);
            }
            info.quality.unwrap_or(fallback)
        } else {
            self.cfg.quality
        };
        self.overlay = Some(Overlay::Save(SaveBox { name: free, quality: quality.to_string(), fresh: true, error: None, exists }));
    }

    /// Enter salvestuskastis: koopia taustal, aken jääb.
    fn start_save(&mut self) {
        let Some(item) = self.items.get(self.cursor) else { return };
        let source = item.path.clone();
        let dir = source.parent().map(Path::to_path_buf).unwrap_or_else(|| self.dir.clone());
        let modified = item.modified;
        let memory = item.clip.clone();
        let edit = self.edit;
        let keep_exif = self.cfg.keep_exif;
        let Some(Overlay::Save(s)) = self.overlay.as_mut() else { return };
        let name = s.name.trim().trim_end_matches(".jpg").trim_end_matches(".JPG").to_owned();
        if let Some(why) = template::invalid_name(&name) {
            s.error = Some(why.to_owned());
            return;
        }
        let Ok(quality) = s.quality.trim().parse::<u8>().map(|q| q.clamp(1, 100)) else {
            s.error = Some("Quality 1–100".to_string());
            return;
        };
        let output = dir.join(format!("{name}.jpg"));
        if output.to_string_lossy().eq_ignore_ascii_case(&source.to_string_lossy()) {
            s.error = Some("the output is the original file".to_string());
            return;
        }
        if output.exists() {
            s.error = Some(format!("{name}.jpg already exists"));
            return;
        }
        let (tx, rx) = mpsc::channel();
        let _ = std::thread::Builder::new().name("save".into()).spawn(move || {
            let result = crate::save::save_copy(&source, memory.as_deref(), &output, edit, quality, keep_exif, modified).map(|bytes| (output, bytes));
            let _ = tx.send(result);
        });
        self.saving = Some(rx);
        self.overlay = None;
        self.note = Some(("saving…".to_string(), false));
        self.cfg.quality = quality;
    }

    /// Taustalõim valmis: uus fail loendisse originaali järele; sinna, kui
    /// seade ütleb; riba ütleb, mis tehti.
    fn poll_save(&mut self) {
        let Some(result) = self.saving.as_ref().and_then(|rx| rx.try_recv().ok()) else { return };
        self.saving = None;
        match result {
            Ok((path, bytes)) => {
                let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                // Kleebitud pilt sai failiks: mälus olev kirje asendub.
                if self.current().is_some_and(|i| i.clip.is_some()) {
                    let old = self.items[self.cursor].path.clone();
                    self.loader.unpin(&old);
                    self.items[self.cursor] = Item::file(path);
                    self.edit = Edit::default();
                    self.undo.clear();
                    self.note = Some((format!("saved → {name} · {}", format_size(bytes)), false));
                    return;
                }
                let at = (self.cursor + 1).min(self.items.len());
                self.items.insert(at, Item::file(path));
                if self.cfg.goto_saved {
                    self.set_cursor(at);
                } else {
                    self.note = Some((format!("saved → {name} · {}", format_size(bytes)), false));
                }
            }
            Err(e) => self.note = Some((e, true)),
        }
    }

    // ---------- Ctrl+C / Ctrl+V ----------

    /// Ctrl+C: pilt lõikelauale nii, nagu see paistab (pööre, peegel, uus
    /// mõõt); muutmata failil ka fail ise (Explorerisse kleepides koopia).
    fn copy_to_clipboard(&mut self) {
        if self.clip.is_some() {
            return;
        }
        let Some(item) = self.current() else { return };
        let source = item.path.clone();
        let memory = item.clip.clone();
        let edit = self.edit;
        let file = (memory.is_none() && edit.is_empty()).then(|| source.clone());
        let (tx, rx) = mpsc::channel();
        let _ = std::thread::Builder::new().name("copy".into()).spawn(move || {
            let result = (|| {
                let image = crate::save::edited(&source, memory.as_deref(), edit)?;
                let rgba = image.to_rgba8();
                let png = crate::clip_image::to_png(&rgba)?;
                crate::platform::set_clipboard_image(png, crate::clip_image::to_dib(&rgba), file.as_deref())
            })();
            let _ = tx.send(ClipDone::Copied(result));
        });
        self.clip = Some(rx);
        self.note = Some(("copying…".to_string(), false));
    }

    /// Ctrl+V: lõikelaua pilt (või esimene pildifail) loendisse praeguse
    /// järele, ainult mälus. Ctrl+S salvestab selle selle kausta.
    fn paste_from_clipboard(&mut self) {
        if self.clip.is_some() {
            return;
        }
        enum From {
            Image(crate::platform::ClipboardImage),
            File(PathBuf),
        }
        let exts = self.cfg.ext_list();
        let from = crate::platform::clipboard_image()
            .map(From::Image)
            .or_else(|| crate::platform::clipboard_files()?.into_iter().find(|f| f.is_file() && crate::config::has_ext(f, &exts)).map(From::File));
        let Some(from) = from else {
            self.note = Some(("No picture on the clipboard".to_string(), true));
            return;
        };
        let (tx, rx) = mpsc::channel();
        let _ = std::thread::Builder::new().name("paste".into()).spawn(move || {
            let result = match from {
                From::Image(crate::platform::ClipboardImage::Png(bytes)) => image::load_from_memory_with_format(&bytes, image::ImageFormat::Png).map_err(|e| e.to_string()).map(|i| (i, None)),
                From::Image(crate::platform::ClipboardImage::Dib(bytes)) => crate::clip_image::from_dib(&bytes).map(|i| (i, None)),
                From::File(path) => crate::decode::decode_full(&path, u32::MAX).map(|i| (i, path.file_stem().map(|n| n.to_string_lossy().into_owned()))),
            };
            let _ = tx.send(ClipDone::Pasted(result));
        });
        self.clip = Some(rx);
    }

    /// Lõikelaua töö valmis.
    fn poll_clip(&mut self, ctx: &egui::Context, out: &mut Out) {
        let Some(done) = self.clip.as_ref().and_then(|rx| rx.try_recv().ok()) else { return };
        self.clip = None;
        match done {
            ClipDone::Copied(Ok(())) => self.note = Some(("copied to the clipboard".to_string(), false)),
            ClipDone::Copied(Err(e)) | ClipDone::Pasted(Err(e)) => self.note = Some((e, true)),
            ClipDone::Pasted(Ok((image, name))) => {
                let stem = name.unwrap_or_else(|| format!("Pasted picture {}", chrono::Local::now().format("%Y-%m-%d %H.%M.%S")));
                let path = self.unused_path(&format!("{stem}.jpg"));
                self.loader.pin(ctx, path.clone(), &image);
                let at = (self.cursor + 1).min(self.items.len());
                self.items.insert(at, Item { path, size: 0, modified: None, clip: Some(Arc::new(image)) });
                self.go(Some(at), true, out);
                self.note = Some(("pasted from the clipboard".to_string(), false));
            }
        }
    }

    /// Nimi kaustas, mida pole ei kettal ega loendis („nimi (2).jpg" …).
    fn unused_path(&self, name: &str) -> PathBuf {
        let dir = if self.dir.as_os_str().is_empty() { std::env::current_dir().unwrap_or_default() } else { self.dir.clone() };
        let first = dir.join(name);
        let free = |p: &PathBuf| !p.exists() && !self.items.iter().any(|i| i.path == *p);
        if free(&first) {
            return first;
        }
        let stem = first.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let ext = first.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        (2u32..).map(|n| dir.join(format!("{stem} ({n}){ext}"))).find(free).unwrap_or(first)
    }

    /// Soovitud pildid tähtsuse järjekorras: praegune (nii teravalt, kui
    /// ekraanil vaja), siis liikumise suunas `AHEAD` ja tagasi `BEHIND`
    /// (+1, +2, −1, +3, +4, −2); ringiga liikudes üle otsa.
    fn wanted(&self, current_side: u32, screen_side: u32) -> Vec<(PathBuf, u32)> {
        let Some(cur) = self.current() else { return Vec::new() };
        let mut out = vec![(cur.path.clone(), current_side)];
        for i in neighbour_order(self.cursor, self.items.len(), self.direction, self.cfg.at_end == AtEnd::Wrap) {
            out.push((self.items[i].path.clone(), screen_side));
        }
        out
    }

    /// Seaded kettale, kui midagi muutus (terve seadistuse võrdlus, mitte
    /// käsitsi lipp); muutuste reas kord sekundis.
    fn autosave(&mut self, ctx: &egui::Context) {
        if self.cfg == self.saved_cfg {
            self.save_due = None;
            return;
        }
        let due = *self.save_due.get_or_insert_with(|| Instant::now() + Duration::from_secs(1));
        if Instant::now() >= due {
            if self.cfg.save().is_ok() {
                self.saved_cfg = self.cfg.clone();
            }
            self.save_due = None;
        } else {
            ctx.request_repaint_after(due - Instant::now());
        }
    }

    /// Akna sünd: esimesel kaadril (peidus) koht paika, kate peale ja
    /// nähtavaks; kate maha, kui pilt on joonistatud akna lõplikus mõõdus.
    fn birth(&mut self, ctx: &egui::Context, frame: &eframe::Frame, image_shown: bool) {
        match self.birth {
            Birth::Hidden => {
                self.hwnd = crate::platform::window_id(frame);
                crate::platform::publish_window(ctx, self.hwnd);
                if let Some(w) = self.hwnd {
                    crate::platform::dark_title(w);
                    crate::platform::transitions(w, false);
                    crate::platform::cloak(w, true);
                    if let Some(p) = self.cfg.window.filter(crate::platform::placement_visible) {
                        crate::platform::place_hidden(w, &p);
                    }
                }
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(true));
                if self.cfg.window.is_some_and(|p| p.maximized) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
                }
                if self.fullscreen {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
                }
                self.birth = Birth::Cloaked { since: Instant::now(), fitting: 0 };
                crate::startup::mark("first frame (hidden)");
                ctx.request_repaint();
            }
            Birth::Cloaked { since, fitting } => {
                // Kas see kaader joonistati akna tegeliku mõõduga (maksimeeritud
                // aken saab mõõdu alles nähtavaks tehes)?
                // NB: `content_rect` väljaspool `input`-i sulundit — egui
                // konteksti lukk pesastatult jäi kinni (aken ei vastanud).
                let drawn = ctx.content_rect().size();
                let fits = ctx.input(|i| {
                    let v = i.viewport();
                    match v.inner_rect {
                        _ if v.minimized == Some(true) => false,
                        Some(inner) => (inner.size() - drawn).length() < 2.0,
                        None => false,
                    }
                });
                let fitting = if fits { fitting.saturating_add(1) } else { 0 };
                let ready = fitting >= 2 && (image_shown || self.items.is_empty() && self.listing.is_none());
                if ready || since.elapsed() >= CLOAK_LIMIT {
                    if let Some(w) = self.hwnd {
                        crate::platform::cloak(w, false);
                        crate::platform::transitions(w, true);
                    }
                    self.birth = Birth::Done;
                    crate::startup::finish(if ready { "window visible, picture on screen" } else { "window visible (picture still loading)" });
                } else {
                    self.birth = Birth::Cloaked { since, fitting };
                    ctx.request_repaint_after(Duration::from_millis(8));
                }
            }
            Birth::Done => {}
        }
    }
}

impl eframe::App for App {
    /// Katseklahvid (`IVIEW_KEYS`): üks iga 400 ms järel, kui pilt on ekraanil.
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        if self.test_keys.is_empty() {
            return;
        }
        let ready = self.birth == Birth::Done && self.current().is_none_or(|i| self.loader.get(&i.path).is_some()) && self.saving.is_none() && self.clip.is_none();
        if ready && Instant::now() >= self.test_next {
            if let Some(event) = self.test_keys.pop_front() {
                crate::startup::timing_line(&format!("test key {event:?}"));
                if let egui::Event::Key { key, modifiers, .. } = event {
                    raw_input.events.push(event);
                    raw_input.events.push(egui::Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers });
                } else {
                    raw_input.events.push(event);
                }
            }
            self.test_next = Instant::now() + Duration::from_millis(400);
        }
        let _ = ctx;
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        let c = stage_color(self.cfg.background);
        [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, 1.0]
    }

    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // Teise käivituse failid ja lohistatud failid.
        let mut incoming = crate::platform::take_incoming();
        incoming.extend(ctx.input(|i| i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).filter(|p| !p.as_os_str().is_empty()).collect::<Vec<_>>()));
        if let Some(path) = incoming.pop() {
            let mut out = Out::default();
            self.ask_or(Pending::Open(path), &mut out);
            if let Some(p) = out.open {
                self.open_path(&ctx, &p);
            }
        }
        self.poll_listing();
        let out = self.body(ui);
        let image_shown = self.current().is_some_and(|i| self.loader.get(&i.path).is_some());
        if self.birth != Birth::Done {
            self.birth(&ctx, frame, image_shown);
        }
        // Ajamõõt: avamisest (või vahetusest) pildini ekraanil.
        if let Some((since, path)) = &self.timing
            && self.loader.get(path).is_some()
            && self.birth == Birth::Done
        {
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            crate::startup::timing_line(&format!("picture on screen in {} ms ({} ms since the window was created): {name}", since.elapsed().as_millis(), self.started.elapsed().as_millis()));
            self.timing = None;
        }
        // Pealkiri.
        let title = match self.current() {
            Some(item) => {
                let shown = if self.cfg.full_path_title { item.path.display().to_string() } else { item.file_name() };
                format!("{shown}  {} / {} · {NAME}", self.cursor + 1, self.items.len())
            }
            None => NAME.to_owned(),
        };
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
        // Akna koht seadetesse (mitte täisekraanil ega minimeeritult).
        if self.birth == Birth::Done
            && !self.fullscreen
            && let Some(w) = self.hwnd
            && !ctx.input(|i| i.viewport().minimized == Some(true) || i.viewport().fullscreen == Some(true))
            && let Some(p) = crate::platform::placement(w)
        {
            self.cfg.window = Some(p);
        }
        // Kaadri lõpu palved.
        if let Some(on) = out.fullscreen {
            self.fullscreen = on;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(on));
            if !on {
                self.esc_closes = false;
            }
        }
        if let Some((natural, padding)) = out.fit_window
            && !self.fullscreen
            && let Some(window) = self.hwnd
        {
            let ppp = ctx.pixels_per_point();
            crate::platform::resize_to_work_area(window, |available| {
                let target = fitted_window_size(natural, padding, available / ppp);
                (target * ppp).ceil()
            });
        }
        if let Some(path) = out.edit_file
            && let Err(e) = crate::platform::edit(&path, &self.cfg.editor)
        {
            self.note = Some((e, true));
        }
        if out.open_dialog {
            let mut dialog = rfd::FileDialog::new().set_title("Open picture");
            if !self.dir.as_os_str().is_empty() {
                dialog = dialog.set_directory(&self.dir);
            }
            let exts = self.cfg.ext_list();
            dialog = dialog.add_filter("Pictures", &exts.iter().map(String::as_str).collect::<Vec<_>>()).add_filter("All files", &["*"]);
            if let Some(path) = dialog.pick_file() {
                let mut o = Out::default();
                self.ask_or(Pending::Open(path), &mut o);
                if let Some(p) = o.open {
                    self.open_path(&ctx, &p);
                }
            }
        }
        if let Some(path) = out.open {
            self.open_path(&ctx, &path);
        }
        if let Some(forward) = out.folder {
            match folder::sibling_folder(&self.dir, forward, &self.cfg.ext_list(), self.cfg.sort) {
                Some((_, file)) => self.open_path(&ctx, &file),
                None => self.note = Some(("No more folders with pictures".to_string(), false)),
            }
        }
        if out.close {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        self.autosave(&ctx);
        if !self.test_keys.is_empty() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    fn on_exit(&mut self) {
        self.cfg.last_one_to_one = self.zoom == Some(1.0);
        self.cfg.last_fill = self.zoom.is_none() && self.fill;
        if self.cfg != self.saved_cfg {
            let _ = self.cfg.save();
        }
    }
}

// ---------- joonistamine ja klahvid ----------

impl App {
    /// Klahvid ja joonistus akna sees.
    fn body(&mut self, ui: &mut egui::Ui) -> Out {
        let ctx = ui.ctx().clone();
        let ctx = &ctx;
        let palette = PALETTE;
        let font_size = self.cfg.font_size;
        let mut out = Out::default();
        self.poll_save();
        self.poll_clip(ctx, &mut out);
        if self.clip.is_some() || self.saving.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        // Seaded: oma aken pildi kohal, klahvid lähevad sinna.
        if let Some(Overlay::Settings(_)) = self.overlay {
            self.paint_stage(ui, &palette, font_size, None);
            let Some(Overlay::Settings(draft)) = self.overlay.as_mut() else { return out };
            match crate::settings::show(ctx, &palette, draft) {
                Some(true) => {
                    let mut new = draft.config.clone();
                    new.tidy();
                    new.window = self.cfg.window;
                    new.last_long_side = self.cfg.last_long_side;
                    new.quality = if new.quality != self.cfg.quality { new.quality } else { self.cfg.quality };
                    let orientation_changed = new.exif_rotate != self.cfg.exif_rotate;
                    let list_changed = new.exts != self.cfg.exts || new.sort != self.cfg.sort;
                    let raw_changed = new.raw != self.cfg.raw;
                    self.cfg = new;
                    self.cfg.apply();
                    if orientation_changed {
                        self.loader.set_orientation(self.cfg.exif_rotate);
                    }
                    if raw_changed && let Some(p) = self.current_path() {
                        self.loader.forget(&p);
                    }
                    if list_changed && let Some(p) = self.current_path() {
                        self.dir = PathBuf::new();
                        self.open_path(ctx, &p);
                    }
                    self.overlay = None;
                }
                Some(false) => self.overlay = None,
                None => {}
            }
            return out;
        }
        // Klaviatuur on akna oma: egui fookus nupul paneks nooled nuppe liigutama.
        release_widget_focus(ctx);
        if let Some((modifiers, key)) = self.pending_key.take() {
            ctx.input_mut(|i| i.events.push(egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers }));
        }
        let none = Modifiers::NONE;
        let ctrl = Modifiers::COMMAND;
        let key = |m: Modifiers, k: Key| ctx.input_mut(|i| i.consume_key(m, k));
        let before = self.cursor;
        let last = self.items.len().saturating_sub(1);
        let (stage, area) = self.stage_rects(ctx);
        let ppp = ctx.pixels_per_point();
        let natural = self.natural_size(ppp);
        let shown_scale = self.zoom.unwrap_or_else(|| natural.map_or(1.0, |n| fit_scale(n, area.size(), self.cfg.enlarge_small || self.fill)));
        // Ctrl+C tuleb eguist kopeerimise sündmusena; Ctrl+V-st, kui lõikelaual
        // on pilt (mitte tekst), ei tule midagi — V-klahv küsitakse otse.
        let copy_now = ctx.input(|i| !i.modifiers.shift && i.events.iter().any(|e| matches!(e, egui::Event::Copy)));
        let v_down = ctx.input(|i| i.focused && i.modifiers.command && !i.modifiers.shift && !i.modifiers.alt) && crate::platform::v_key_down();
        let paste_now = (v_down && !self.paste_held) || ctx.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Paste(_))));
        self.paste_held = v_down;
        let has_item = !self.items.is_empty();
        let in_memory = self.current().is_some_and(|i| i.clip.is_some());
        let save_first = ("Save it first (Ctrl+S)".to_string(), true);

        if ctx.input(|i| i.pointer.delta() != Vec2::ZERO || i.pointer.any_down()) {
            self.pointer_moved = Instant::now();
        }
        // Suurendatud: nooled nihutavad pilti noole suunas seni, kuni klahv
        // all; pilti vahetavad siis PgUp/PgDn, tühik, Home/End.
        if self.overlay.is_none() && self.zoom.is_some() {
            self.pan += arrow_pan(ctx);
        }

        match self.overlay.as_mut() {
            Some(Overlay::AskDelete) => {
                if key(none, Key::Enter) || key(none, Key::Y) {
                    self.delete_current();
                } else if key(none, Key::Escape) || key(none, Key::N) || key(none, Key::Delete) {
                    self.overlay = None;
                }
            }
            Some(Overlay::Rename(_)) => {
                if key(none, Key::Enter) {
                    self.commit_rename();
                } else if key(none, Key::Escape) {
                    self.overlay = None;
                }
            }
            Some(Overlay::Resize(_)) => {
                if key(ctrl, Key::Enter) {
                    self.apply_resize(true);
                } else if key(none, Key::Enter) {
                    self.apply_resize(false);
                } else if key(none, Key::Escape) {
                    self.overlay = None;
                }
            }
            Some(Overlay::Save(_)) => {
                if key(none, Key::Enter) {
                    self.start_save();
                } else if key(none, Key::Escape) {
                    self.overlay = None;
                }
            }
            Some(Overlay::AskUnsaved(_)) => {
                if key(none, Key::Enter) {
                    if let Some(Overlay::AskUnsaved(target)) = self.overlay.take() {
                        self.proceed(target, &mut out);
                    }
                } else if key(none, Key::Escape) {
                    self.overlay = None;
                } else if key(ctrl, Key::S) {
                    self.open_save();
                }
            }
            Some(Overlay::Settings(_)) => {}
            None => {
                if key(none, Key::Escape) {
                    // Täisekraanilt kõigepealt tagasi, siis kinni.
                    if self.fullscreen && !self.esc_closes {
                        out.fullscreen = Some(false);
                    } else {
                        self.ask_or(Pending::Close, &mut out);
                    }
                } else if key(none, Key::Enter) || key(none, Key::F11) {
                    out.fullscreen = Some(!self.fullscreen);
                } else if key(none, Key::ArrowRight) || key(none, Key::PageDown) || key(none, Key::Space) {
                    self.go(None, true, &mut out);
                } else if key(none, Key::ArrowLeft) || key(none, Key::PageUp) || key(none, Key::Backspace) {
                    self.go(None, false, &mut out);
                } else if key(none, Key::Home) {
                    self.go(Some(0), false, &mut out);
                } else if key(none, Key::End) {
                    self.go(Some(last), true, &mut out);
                } else if key(none, Key::Plus) || key(none, Key::Equals) || key(Modifiers::SHIFT, Key::Plus) || key(Modifiers::SHIFT, Key::Equals) {
                    self.zoom = Some(zoom_step(shown_scale, true, self.cfg.zoom_step));
                } else if key(none, Key::Minus) {
                    self.zoom = Some(zoom_step(shown_scale, false, self.cfg.zoom_step));
                } else if key(none, Key::Num1) {
                    self.zoom = Some(1.0);
                } else if key(none, Key::Num0) {
                    self.zoom = None;
                    self.fill = false;
                    self.pan = Vec2::ZERO;
                } else if key(none, Key::F) {
                    if let Some(natural) = natural {
                        self.pan = Vec2::ZERO;
                        if self.zoom.is_none() && self.fill {
                            self.zoom = Some(1.0);
                            self.fill = false;
                        } else {
                            self.zoom = None;
                            self.fill = true;
                            out.fit_window = Some((natural, ctx.content_rect().size() - area.size()));
                        }
                    }
                } else if key(ctrl, Key::O) {
                    out.open_dialog = true;
                } else if key(none, Key::P) {
                    self.overlay = Some(Overlay::Settings(Box::new(crate::settings::Draft::new(self.cfg.clone()))));
                } else if !has_item {
                    // Ülejäänud klahvid vajavad pilti.
                } else if key(ctrl, Key::R) {
                    if self.orig_dims().is_some() {
                        self.open_resize();
                    }
                } else if key(ctrl, Key::S) {
                    if self.orig_dims().is_some() && self.saving.is_none() {
                        self.open_save();
                    }
                } else if key(ctrl, Key::Z) {
                    if let Some(prev) = self.undo.pop() {
                        self.edit = prev;
                        self.pan = Vec2::ZERO;
                    }
                } else if copy_now || key(ctrl, Key::C) {
                    self.copy_to_clipboard();
                } else if key(none, Key::F2) {
                    if in_memory {
                        self.note = Some(save_first.clone());
                    } else if let Some(item) = self.current() {
                        self.overlay = Some(Overlay::Rename(Rename { text: item.stem(), fresh: true, error: None }));
                    }
                } else if key(none, Key::Delete) {
                    if self.cfg.ask_delete && !in_memory {
                        self.overlay = Some(Overlay::AskDelete);
                    } else {
                        self.delete_current();
                    }
                } else if key(none, Key::F4) {
                    if in_memory {
                        self.note = Some(save_first.clone());
                    } else {
                        out.edit_file = self.current_path();
                    }
                } else if key(none, Key::F5) {
                    // Uuesti kettalt (fail muutus teises programmis).
                    if let Some(p) = self.current_path() {
                        self.loader.forget(&p);
                        self.exif.remove(&p);
                        self.exif_asked.remove(&p);
                    }
                }
            }
        }
        // Ctrl+V ka tühjas aknas.
        if self.overlay.is_none() && (paste_now || (!has_item && key(ctrl, Key::V))) {
            self.paste_from_clipboard();
        }
        // Pööre ja peegel: eraldi, sest `consume_key` tarbib ühe korraga.
        if self.overlay.is_none() && has_item {
            let edits = [(Key::R, 0u8), (Key::L, 1), (Key::H, 2), (Key::V, 3)];
            for (k, what) in edits {
                if ctx.input_mut(|i| i.consume_key(none, k)) {
                    self.push_undo();
                    self.edit = match what {
                        0 => self.edit.rotate(true),
                        1 => self.edit.rotate(false),
                        2 => self.edit.flip_h(),
                        _ => self.edit.flip_v(),
                    };
                    self.pan = Vec2::ZERO;
                }
            }
        }
        // Ratas: eelmine / järgmine (või suurendus, seade järgi); Ctrl+ratas
        // teist. Puuteplaadi kahe näpu liigutus: alast suurem pilt liigub
        // näppudega kaasa, pilti ei vahetata.
        let (mut scroll, swipe, zoom_delta, pointer) = ctx.input(|i| {
            let (mut scroll, mut swipe) = (0.0, Vec2::ZERO);
            for e in &i.events {
                if let egui::Event::MouseWheel { unit, delta, modifiers, .. } = e
                    && !modifiers.command
                    && !modifiers.ctrl
                {
                    if *unit == egui::MouseWheelUnit::Point {
                        swipe += *delta;
                    } else {
                        scroll += delta.y;
                    }
                }
            }
            (scroll, swipe, i.zoom_delta(), i.pointer.latest_pos())
        });
        let overflows = natural.is_some_and(|n| {
            let shown = n * shown_scale;
            shown.x > area.width() + 0.5 || shown.y > area.height() + 0.5
        });
        if overflows && self.overlay.is_none() {
            self.pan += swipe;
        } else {
            scroll += swipe.y;
        }
        if self.overlay.is_none() && has_item {
            let (pictures, zoom) = match self.cfg.wheel {
                WheelMode::Pictures => (scroll, zoom_delta),
                WheelMode::Zoom => {
                    let z = if scroll > 0.0 {
                        1.1
                    } else if scroll < 0.0 {
                        1.0 / 1.1
                    } else {
                        1.0
                    };
                    let p = if (zoom_delta - 1.0).abs() > 0.001 { if zoom_delta > 1.0 { 1.0 } else { -1.0 } } else { 0.0 };
                    (p, z)
                }
            };
            if pictures < 0.0 {
                self.go(None, true, &mut out);
            } else if pictures > 0.0 {
                self.go(None, false, &mut out);
            }
            if (zoom - 1.0).abs() > 0.001 {
                let new = (shown_scale * zoom).clamp(ZOOM_STEPS[0], ZOOM_STEPS[ZOOM_STEPS.len() - 1]);
                let from_center = pointer.map_or(Vec2::ZERO, |p| p - area.center());
                self.pan = zoom_at(self.pan, from_center, shown_scale, new);
                self.zoom = Some(new);
            }
        }
        let _ = before;

        // ---------- ettelaadimine ----------
        let screen_side = ((stage.size().max_elem() * ppp).ceil() as u32).clamp(640, 4096);
        let natural = self.natural_size(ppp);
        let scale = self.zoom.unwrap_or_else(|| natural.map_or(1.0, |n| fit_scale(n, area.size(), self.cfg.enlarge_small || self.fill)));
        let shown = natural.map(|n| n * scale);
        // Praegune pilt nii teravalt, kui ekraanil vaja (1:1 → täissuurus).
        let current_side = shown.map_or(screen_side, |s| ((s.max_elem() * ppp).ceil() as u32).clamp(screen_side, SHARP_SIDE));
        let wanted = self.wanted(current_side, screen_side);
        self.loader.update(ctx, &wanted);
        // Tekstuur võis just saabuda: mõõt ja skaala uuesti, muidu joonistuks
        // see kaader skaalaga 1,0 — 1:1 vilksatus enne mahutamist.
        let scale = if natural.is_none() { self.zoom.unwrap_or_else(|| self.natural_size(ppp).map_or(1.0, |n| fit_scale(n, area.size(), self.cfg.enlarge_small || self.fill))) } else { scale };
        self.paint_stage(ui, &palette, font_size, Some(scale));
        out
    }

    /// Kogu ala, pildi ala (serv ja klahvirea ruum maha).
    fn stage_rects(&self, ctx: &egui::Context) -> (Rect, Rect) {
        let rect = ctx.content_rect();
        let meta_h = if self.fullscreen || !self.cfg.meta_bar || self.items.is_empty() { 0.0 } else { META_H };
        let stage = Rect::from_min_max(egui::pos2(rect.left(), rect.top() + meta_h), rect.max);
        let area = if self.fullscreen { stage } else { picture_area(stage, self.cfg.hints) };
        (stage, area)
    }

    /// Pildi mõõt punktides nii, et 1:1 on üks pildipiksel ekraanipikslis;
    /// muudatus (pikem külg, pööre) sees.
    fn natural_size(&self, ppp: f32) -> Option<Vec2> {
        let (w, h) = self.edit.dims(self.orig_dims()?);
        Some(Vec2::new(w as f32, h as f32) / ppp)
    }

    /// Riba, pilt, tekst ja kastid. `scale`: `None` — arvuta ise (seadete ajal).
    fn paint_stage(&mut self, ui: &mut egui::Ui, palette: &Palette, font_size: f32, scale: Option<f32>) {
        let ctx = ui.ctx().clone();
        let (stage, area) = self.stage_rects(&ctx);
        let ppp = ctx.pixels_per_point();
        let scale = scale.unwrap_or_else(|| self.zoom.unwrap_or_else(|| self.natural_size(ppp).map_or(1.0, |n| fit_scale(n, area.size(), self.cfg.enlarge_small || self.fill))));
        let fill = stage_color(self.cfg.background);
        let settings_open = matches!(self.overlay, Some(Overlay::Settings(_)));
        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(fill)).show(ui, |ui| {
            let rect = ctx.content_rect();
            ui.set_clip_rect(rect);
            ui.allocate_rect(rect, Sense::hover());
            if self.items.is_empty() {
                self.empty_screen(ui, palette, font_size, rect);
                return;
            }
            if stage.top() > rect.top() {
                let meta = Rect::from_min_size(rect.min, Vec2::new(rect.width(), META_H));
                self.meta_bar(ui, palette, font_size, meta, scale);
            }
            self.image_stage(ui, palette, font_size, stage, area, scale);
            if self.fullscreen {
                self.fullscreen_text(ui, font_size, stage, scale);
                if self.pointer_moved.elapsed() > CURSOR_HIDE && !settings_open {
                    ctx.set_cursor_icon(egui::CursorIcon::None);
                } else {
                    ctx.request_repaint_after(CURSOR_HIDE);
                }
            }
            let file_name = self.current().map(Item::file_name).unwrap_or_default();
            match self.overlay.as_ref() {
                Some(Overlay::AskDelete) => ask_bar(ui, palette, font_size, stage, format!("Move to the Recycle Bin: {file_name}?  Enter yes · Esc no")),
                Some(Overlay::AskUnsaved(_)) => ask_bar(ui, palette, font_size, stage, "The change is not saved — Enter continue without it · Esc stay · Ctrl+S save".to_string()),
                Some(Overlay::Rename(_)) => self.rename_box(ui, palette, font_size, stage),
                Some(Overlay::Resize(_)) => self.resize_box(ui, palette, font_size, stage),
                Some(Overlay::Save(_)) => self.save_box(ui, palette, font_size, stage),
                Some(Overlay::Settings(_)) => {}
                None if !self.fullscreen && self.cfg.hints => {
                    if let Some(key) = hints(ui, font_size, stage, self.zoom.is_some()) {
                        self.pending_key = Some(key);
                        ctx.request_repaint();
                    }
                }
                None => {}
            }
        });
    }

    /// Tühi aken (programm avati ilma failita või kaust sai tühjaks).
    fn empty_screen(&mut self, ui: &mut egui::Ui, palette: &Palette, font_size: f32, rect: Rect) {
        let p = ui.painter();
        let c = rect.center();
        if self.listing.is_some() {
            p.text(c, Align2::CENTER_CENTER, "Reading the folder…", FontId::proportional(font_size * 1.3), palette.text_faint);
            ui.ctx().request_repaint_after(Duration::from_millis(50));
            return;
        }
        p.text(c - Vec2::new(0.0, font_size * 1.2), Align2::CENTER_CENTER, "Ctrl+O open a picture · drop a picture here · Ctrl+V paste", FontId::proportional(font_size * 1.2), palette.text_dim);
        p.text(c + Vec2::new(0.0, font_size * 0.9), Align2::CENTER_CENTER, "P settings · Esc close", FontId::proportional(font_size), palette.text_faint);
        if let Some((note, bad)) = &self.note {
            p.text(c + Vec2::new(0.0, font_size * 3.0), Align2::CENTER_CENTER, note, FontId::proportional(font_size), if *bad { palette.danger } else { palette.text_dim });
        }
    }

    fn meta_bar(&mut self, ui: &mut egui::Ui, palette: &Palette, font_size: f32, rect: Rect, scale: f32) {
        let painter = ui.painter().clone();
        painter.rect_filled(rect, CornerRadius::ZERO, META);
        painter.hline(rect.x_range(), rect.bottom() - 0.5, Stroke::new(1.0, Color32::from_rgb(0x2A, 0x2F, 0x37)));
        let y = rect.center().y;
        let white = Color32::WHITE;
        let dim = palette.text_dim;
        let faint = Color32::from_rgb(0x4A, 0x51, 0x5C);
        let yellow = Color32::from_rgb(0xF2, 0xC1, 0x4E);
        let green = Color32::from_rgb(0x8F, 0xD4, 0x7A);
        let small = FontId::proportional((font_size - 1.5).round());
        // Paremal: suurendus (ja teade).
        let (zoom_text, zoom_color) = match &self.note {
            Some((note, bad)) => (note.clone(), if *bad { palette.danger } else { green }),
            None => {
                let pct = format!("{} %", (scale * 100.0).round().max(1.0));
                match (self.zoom, self.fill) {
                    (None, false) => (format!("fit · {pct}"), dim),
                    (None, true) => (format!("filled · {pct}"), dim),
                    _ => (pct, white),
                }
            }
        };
        let z = painter.text(egui::pos2(rect.right() - 14.0, y), Align2::RIGHT_CENTER, zoom_text, small.clone(), zoom_color);
        let Some(item) = self.current() else { return };
        let path = item.path.clone();
        let (size, modified, in_memory, name_text) = (item.size, item.modified, item.clip.is_some(), item.file_name());
        let mut x = rect.left() + 14.0;
        let name_room = (z.left() - 24.0 - x - 260.0).max(80.0);
        let name = elide_middle(ui.ctx(), &name_text, &FontId::monospace(font_size), name_room);
        let r = painter.text(egui::pos2(x, y), Align2::LEFT_CENTER, name, FontId::monospace(font_size), white);
        x = r.right();
        let mut parts: Vec<(String, Color32)> = Vec::new();
        if let Some((w, h)) = self.orig_dims() {
            if self.edit.is_empty() {
                parts.push((format!("{w} × {h}"), dim));
            } else {
                let (ew, eh) = self.edit.dims((w, h));
                parts.push((format!("{w} × {h} → {ew} × {eh}"), dim));
            }
        }
        if self.cfg.exif_rotate
            && !in_memory
            && let Some(o) = self.exif_ready(ui.ctx(), &path).and_then(|i| i.orientation)
            && o != 1
        {
            let what = match o {
                3 => "EXIF 180°".to_owned(),
                6 => "EXIF 90°".to_owned(),
                8 => "EXIF 270°".to_owned(),
                _ => "EXIF mirrored".to_owned(),
            };
            parts.push((what, Color32::from_rgb(0x7E, 0x8A, 0x9A)));
        }
        if in_memory {
            parts.push(("from the clipboard · not saved · Ctrl+S".to_string(), yellow));
        } else {
            parts.push((format_size(size), dim));
        }
        if let Some(m) = modified {
            parts.push((chrono::DateTime::<chrono::Local>::from(m).format("%Y-%m-%d %H:%M").to_string(), dim));
        }
        if !self.edit.is_empty() {
            let mut desc = self.edit.describe();
            desc.push("not saved · Ctrl+S".to_string());
            parts.push((desc.join(" · "), yellow));
        }
        for (part, color) in parts {
            let dot = painter.text(egui::pos2(x + 10.0, y), Align2::LEFT_CENTER, "·", small.clone(), faint);
            let r = painter.text(egui::pos2(dot.right() + 10.0, y), Align2::LEFT_CENTER, part, small.clone(), color);
            x = r.right();
            if x > z.left() - 24.0 {
                break;
            }
        }
    }

    /// Pilt keskel (nihkega, kui alast suurem). Lohistamine nihutab,
    /// topeltklõps vahetab mahutatud ja 1:1.
    fn image_stage(&mut self, ui: &mut egui::Ui, palette: &Palette, font_size: f32, stage: Rect, area: Rect, scale: f32) {
        let response = ui.interact(stage, egui::Id::new("picture-stage"), Sense::click_and_drag());
        let ppp = ui.ctx().pixels_per_point();
        let Some(item) = self.current() else { return };
        let (path, item_size) = (item.path.clone(), item.size);
        match self.loader.get(&path) {
            Some(Ok((texture, _))) => {
                let texture = texture.id();
                let Some(natural) = self.natural_size(ppp) else { return };
                let shown_size = natural * scale;
                if response.dragged() {
                    self.pan += response.drag_delta();
                }
                if response.double_clicked() {
                    if self.zoom.is_some() {
                        self.zoom = None;
                        self.fill = false;
                        self.pan = Vec2::ZERO;
                    } else {
                        let from_center = response.interact_pointer_pos().map_or(Vec2::ZERO, |p| p - area.center());
                        self.pan = zoom_at(self.pan, from_center, scale, 1.0);
                        self.zoom = Some(1.0);
                    }
                }
                self.pan = clamp_pan(self.pan, shown_size, area.size());
                let shown = Rect::from_center_size(area.center() + self.pan, shown_size);
                let painter = ui.painter().with_clip_rect(stage);
                if self.cfg.transparent == Transparent::Checker && has_alpha(&path) {
                    checkerboard(&painter, shown.intersect(stage));
                }
                painter.add(transformed_image(texture, shown, self.edit));
            }
            Some(Err(reason)) => {
                let reason = reason.clone();
                let p = ui.painter();
                p.text(area.center() - Vec2::new(0.0, font_size * 0.9), Align2::CENTER_CENTER, "Could not open the picture", FontId::proportional(font_size * 1.2), palette.text_dim);
                p.text(area.center() + Vec2::new(0.0, font_size * 0.9), Align2::CENTER_CENTER, reason, FontId::proportional(font_size * 0.95), palette.text_faint);
            }
            None => {
                // Pilt tuleb: loetud / kokku ja riba (dekooder loeb faili meie
                // voo kaudu, vt `wic::Progress`).
                let (read, total) = match self.loader.progress(&path) {
                    Some((r, t)) if t > 0 => (r.min(t), t),
                    _ => (0, item_size),
                };
                let size = if read > 0 { format!("{} / {}", format_size(read), format_size(total)) } else { format_size(total) };
                let p = ui.painter();
                p.text(area.center() - Vec2::new(0.0, font_size * 0.9), Align2::CENTER_CENTER, format!("Loading {size}…"), FontId::proportional(font_size * 1.3), palette.text_faint);
                let w = (area.width() * 0.3).clamp(160.0, 420.0);
                let bar = Rect::from_center_size(area.center() + Vec2::new(0.0, font_size * 0.9), Vec2::new(w, 6.0));
                p.rect_filled(bar, CornerRadius::same(3), palette.border);
                if total > 0 && read > 0 {
                    let done = Rect::from_min_size(bar.min, Vec2::new(bar.width() * (read as f32 / total as f32), bar.height()));
                    p.rect_filled(done, CornerRadius::same(3), palette.accent);
                }
                ui.ctx().request_repaint_after(Duration::from_millis(50));
            }
        }
    }

    /// F2: nimekast keskel all, laiend hallina, nimi märgitud.
    fn rename_box(&mut self, ui: &mut egui::Ui, palette: &Palette, font_size: f32, stage: Rect) {
        let ext = self.current().map(Item::ext).unwrap_or_default();
        let Some(Overlay::Rename(r)) = self.overlay.as_mut() else { return };
        let name_size = (font_size * 1.15).round();
        let small = FontId::proportional((font_size - 1.5).round());
        let width = (stage.width() - 48.0).min(480.0);
        let field = light_box(ui, palette, stage, Vec2::new(width, BOX_H), r.error.is_some());
        let ext_g = ui.painter().layout_no_wrap(ext, small, palette.text_dim);
        let ext_w = ext_g.size().x;
        ui.painter().galley(egui::pos2(field.right() - 12.0 - ext_w, field.center().y - ext_g.size().y / 2.0), ext_g, palette.text_dim);
        let inner = Rect::from_min_max(field.min + Vec2::new(12.0, 2.0), egui::pos2(field.right() - 12.0 - ext_w - 8.0, field.bottom() - 2.0));
        let before = r.text.clone();
        box_edit(ui, palette, inner, egui::Id::new("picture-rename"), &mut r.text, &mut r.fresh, FontId::proportional(name_size));
        if r.text != before {
            r.error = None;
        }
        under_box(ui, palette, font_size, field, r.error.as_deref(), "Enter rename · Esc cancel");
    }

    /// Ctrl+R: pikem külg, kiirvalikud ja ½; → tulemuse mõõt; Enter vähendab.
    fn resize_box(&mut self, ui: &mut egui::Ui, palette: &Palette, font_size: f32, stage: Rect) {
        let orig = self.orig_dims().unwrap_or((0, 0));
        let orig_side = orig.0.max(orig.1);
        let quick = self.cfg.quick_sides();
        let enlarge = self.cfg.enlarge;
        let mut force = false;
        {
            let Some(Overlay::Resize(r)) = self.overlay.as_mut() else { return };
            let small = FontId::proportional((font_size - 1.5).round());
            let width = (stage.width() - 48.0).min(600.0);
            let field = light_box(ui, palette, stage, Vec2::new(width, 74.0), r.error.is_some());
            let painter = ui.painter().clone();
            let y1 = field.top() + 22.0;
            let title = painter.text(egui::pos2(field.left() + 14.0, y1), Align2::LEFT_CENTER, "Resize", FontId::proportional(font_size), palette.text);
            let num = Rect::from_min_size(egui::pos2(title.right() + 14.0, y1 - 14.0), Vec2::new(72.0, 28.0));
            painter.rect(num, CornerRadius::same(3), palette.panel, Stroke::new(1.0, palette.border), egui::StrokeKind::Inside);
            let before = r.text.clone();
            box_edit(ui, palette, num.shrink2(Vec2::new(8.0, 2.0)), egui::Id::new("picture-resize"), &mut r.text, &mut r.fresh, FontId::proportional(font_size));
            if r.text != before {
                r.error = None;
            }
            let px = painter.text(egui::pos2(num.right() + 6.0, y1), Align2::LEFT_CENTER, "px", small.clone(), palette.text_dim);
            // Kiirvalikud nuppudena (sama kõrgus, 3 px nurk).
            let current: u32 = r.text.trim().parse().unwrap_or(0);
            let mut labels: Vec<(String, Option<u32>)> = quick.iter().map(|q| (q.to_string(), Some(*q))).collect();
            labels.push(("½".to_owned(), None));
            let mut x = px.right() + 12.0;
            for (i, (label, value)) in labels.iter().enumerate() {
                let g = painter.layout_no_wrap(label.clone(), small.clone(), palette.text);
                let b = Rect::from_min_size(egui::pos2(x, y1 - 12.0), Vec2::new(g.size().x + 14.0, 24.0));
                if b.right() > field.right() - 10.0 {
                    break;
                }
                let resp = ui.interact(b, egui::Id::new(("resize-quick", i)), Sense::click());
                let selected = *value == Some(current);
                let fill = if selected {
                    palette.accent
                } else if resp.hovered() {
                    palette.border
                } else {
                    palette.panel
                };
                painter.rect(b, CornerRadius::same(3), fill, Stroke::new(1.0, palette.border), egui::StrokeKind::Inside);
                painter.galley(b.center() - g.size() / 2.0, g, palette.text);
                if resp.clicked() {
                    r.text = value.unwrap_or((orig_side / 2).max(16)).to_string();
                    r.error = None;
                }
                x = b.right() + 4.0;
            }
            let y2 = field.bottom() - 20.0;
            let side: u32 = r.text.trim().parse().unwrap_or(0);
            let result = if side >= 16 && orig_side > 0 {
                let (w, h) = fit_dims(orig, side);
                format!("→ {w} × {h}")
            } else {
                String::new()
            };
            painter.text(egui::pos2(field.left() + 14.0, y2), Align2::LEFT_CENTER, result, small, palette.text_dim);
            let too_big = side > orig_side && enlarge == Enlarge::Warn && side <= ((orig_side as f32 * MAX_ENLARGE) as u32).min(MAX_SIDE);
            if too_big {
                let b = Rect::from_min_size(egui::pos2(field.right() - 150.0, y2 - 13.0), Vec2::new(136.0, 26.0));
                let resp = ui.interact(b, egui::Id::new("resize-force"), Sense::click());
                painter.rect(b, CornerRadius::same(3), if resp.hovered() { palette.danger } else { palette.panel }, Stroke::new(1.0, palette.danger), egui::StrokeKind::Inside);
                painter.text(b.center(), Align2::CENTER_CENTER, "Enlarge anyway", FontId::proportional((font_size - 1.5).round()), palette.text);
                force = resp.clicked();
            }
            under_box(ui, palette, font_size, field, r.error.as_deref(), "Enter resize · Ctrl+Enter enlarge · Esc cancel");
        }
        if force {
            self.apply_resize(true);
        }
    }

    /// Ctrl+S: nimi (märgitud), .jpg, kaust, kvaliteet; Enter salvestab.
    fn save_box(&mut self, ui: &mut egui::Ui, palette: &Palette, font_size: f32, stage: Rect) {
        let dir = self.current().and_then(|i| i.path.parent()).map(|p| p.display().to_string()).unwrap_or_default();
        let orig = self.orig_dims().unwrap_or((0, 0));
        let (w, h) = self.edit.dims(orig);
        let Some(Overlay::Save(s)) = self.overlay.as_mut() else { return };
        let small = FontId::proportional((font_size - 1.5).round());
        let width = (stage.width() - 48.0).min(600.0);
        let field = light_box(ui, palette, stage, Vec2::new(width, 104.0), s.error.is_some());
        let painter = ui.painter().clone();
        let y1 = field.top() + 24.0;
        let title = painter.text(egui::pos2(field.left() + 14.0, y1), Align2::LEFT_CENTER, "Save", FontId::proportional(font_size), palette.text);
        painter.text(egui::pos2(field.right() - 14.0, y1), Align2::RIGHT_CENTER, format!("{w} × {h}"), small.clone(), palette.text_dim);
        let ext_g = painter.layout_no_wrap(".jpg".to_owned(), small.clone(), palette.text_dim);
        let name = Rect::from_min_size(egui::pos2(title.right() + 14.0, y1 - 15.0), Vec2::new(field.right() - 14.0 - 80.0 - title.right() - 14.0, 30.0));
        painter.rect(name, CornerRadius::same(3), palette.panel, Stroke::new(1.0, palette.border), egui::StrokeKind::Inside);
        painter.galley(egui::pos2(name.right() - 8.0 - ext_g.size().x, name.center().y - ext_g.size().y / 2.0), ext_g.clone(), palette.text_dim);
        let before = s.name.clone();
        box_edit(ui, palette, Rect::from_min_max(name.min + Vec2::new(8.0, 2.0), egui::pos2(name.right() - 12.0 - ext_g.size().x, name.bottom() - 2.0)), egui::Id::new("picture-save-name"), &mut s.name, &mut s.fresh, FontId::monospace(font_size));
        if s.name != before {
            s.error = None;
            s.exists = None;
        }
        let y2 = field.top() + 60.0;
        let mono_small = FontId::monospace((font_size - 1.5).round());
        let folder_g = elide_middle(ui.ctx(), &dir, &mono_small, width - 220.0);
        painter.text(egui::pos2(field.left() + 14.0, y2), Align2::LEFT_CENTER, folder_g, mono_small, palette.text_dim);
        let y3 = field.top() + 86.0;
        let q_label = painter.text(egui::pos2(field.left() + 14.0, y3), Align2::LEFT_CENTER, "Quality", small.clone(), palette.text);
        let q = Rect::from_min_size(egui::pos2(q_label.right() + 10.0, y3 - 12.0), Vec2::new(52.0, 24.0));
        painter.rect(q, CornerRadius::same(3), palette.panel, Stroke::new(1.0, palette.border), egui::StrokeKind::Inside);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(q.shrink2(Vec2::new(6.0, 1.0))).layout(egui::Layout::left_to_right(egui::Align::Center)));
        egui::TextEdit::singleline(&mut s.quality).id(egui::Id::new("picture-save-quality")).frame(egui::Frame::NONE).font(FontId::proportional(font_size)).text_color(palette.text).vertical_align(egui::Align::Center).desired_width(q.width() - 12.0).show(&mut child);
        painter.text(egui::pos2(q.right() + 8.0, y3), Align2::LEFT_CENTER, "JPEG", small, palette.text_dim);
        let under = match (&s.error, &s.exists) {
            (Some(e), _) => Some(e.clone()),
            (None, Some(name)) => Some(format!("{name} already exists")),
            _ => None,
        };
        under_box(ui, palette, font_size, field, under.as_deref(), "Enter save · Esc cancel");
    }

    /// Täisekraani tekst nurgas: kaks mallirida, valge, tumedal ribal.
    fn fullscreen_text(&mut self, ui: &mut egui::Ui, font_size: f32, stage: Rect, scale: f32) {
        let show = match self.cfg.text {
            FullText::Hidden => false,
            FullText::Shown => true,
            FullText::Flash => {
                if self.changed_at.elapsed() < FLASH {
                    ui.ctx().request_repaint_after(Duration::from_millis(200));
                    true
                } else {
                    false
                }
            }
        };
        if !show {
            return;
        }
        let templates = [self.cfg.text_line1.clone(), self.cfg.text_line2.clone()];
        let lines: Vec<String> = templates.iter().map(|t| self.render_line(ui.ctx(), t, scale)).filter(|l| !l.is_empty()).collect();
        if lines.is_empty() {
            return;
        }
        let size = match self.cfg.text_size {
            crate::config::TextSize::Small => font_size,
            crate::config::TextSize::Medium => font_size * 1.25,
            crate::config::TextSize::Large => font_size * 1.7,
        };
        let font = FontId::proportional(size.round());
        let painter = ui.painter();
        let margin = 18.0;
        let second = Color32::from_rgb(0xC9, 0xD1, 0xDC);
        let mut galleys: Vec<_> = lines.iter().map(|l| painter.layout_no_wrap(l.clone(), font.clone(), Color32::WHITE)).collect();
        // Kaks rida ühte ritta, kui mahuvad: esimene valge, teine tuhmim.
        if galleys.len() == 2 && galleys[0].size().x + galleys[1].size().x + 40.0 <= stage.width() - 2.0 * margin {
            let mut job = egui::text::LayoutJob::default();
            job.append(&lines[0], 0.0, egui::TextFormat::simple(font.clone(), Color32::WHITE));
            job.append("·", 12.0, egui::TextFormat::simple(font.clone(), Color32::from_rgb(0x8A, 0x93, 0xA1)));
            job.append(&lines[1], 12.0, egui::TextFormat::simple(font.clone(), second));
            galleys = vec![painter.layout_job(job)];
        }
        let total_h: f32 = galleys.iter().map(|g| g.size().y).sum::<f32>() + 4.0 * (galleys.len() as f32 - 1.0);
        let max_w = galleys.iter().map(|g| g.size().x).fold(0.0, f32::max);
        use crate::config::Corner;
        let (left, top) = match self.cfg.text_corner {
            Corner::TopLeft => (stage.left() + margin, stage.top() + margin),
            Corner::TopRight => (stage.right() - margin - max_w, stage.top() + margin),
            Corner::BottomLeft => (stage.left() + margin, stage.bottom() - margin - total_h),
            Corner::BottomRight => (stage.right() - margin - max_w, stage.bottom() - margin - total_h),
        };
        // Poolläbipaistev tume riba teksti all: valge kiri ainult varjuga
        // kadus heleda taeva peale ära.
        let band = Rect::from_min_size(egui::pos2(left - 10.0, top - 5.0), Vec2::new(max_w + 20.0, total_h + 10.0));
        painter.rect_filled(band, CornerRadius::same(3), Color32::from_black_alpha(130));
        let mut y = top;
        let right = matches!(self.cfg.text_corner, Corner::TopRight | Corner::BottomRight);
        for (i, g) in galleys.into_iter().enumerate() {
            let x = if right { left + max_w - g.size().x } else { left };
            let color = if i == 0 { Color32::WHITE } else { second };
            painter.galley_with_override_text_color(egui::pos2(x, y) + Vec2::new(0.0, 1.0), g.clone(), Color32::from_black_alpha(120));
            painter.galley(egui::pos2(x, y), g.clone(), color);
            y += g.size().y + 4.0;
        }
    }

    /// Mallirida pildi väärtustega (`template::render`).
    fn render_line(&mut self, ctx: &egui::Context, line: &str, scale: f32) -> String {
        use crate::exif::Field;
        let Some(item) = self.current() else { return String::new() };
        let path = item.path.clone();
        let stem = item.stem();
        let ext = item.ext().trim_start_matches('.').to_owned();
        let size = item.size;
        let modified = item.modified;
        let in_memory = item.clip.is_some();
        let (index, total) = (self.cursor + 1, self.items.len());
        let dims = self.orig_dims().map(|d| self.edit.dims(d));
        let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        let base = template::folder_vars(&path);
        // Kuni EXIF taustal loeb: väljad tühjad.
        let mut waiting = ExifInfo::default();
        let info = if in_memory { &mut waiting } else { self.exif_ready(ctx, &path).unwrap_or(&mut waiting) };
        let meta = info.meta.clone();
        let taken = meta.get(Field::Taken).and_then(template::parse_taken);
        let fields = &info.fields;
        let tag = |t: &str| -> Option<String> {
            let f = fields.iter().find(|f| f.ifd_num == exif::In::PRIMARY && f.tag.to_string().eq_ignore_ascii_case(t.trim()))?;
            let text = f.display_value().to_string().trim().trim_matches('"').trim().to_owned();
            (!text.is_empty()).then_some(text)
        };
        let mut lookup = |name: &str, arg: Option<&str>| -> Option<String> {
            let dims_or = |f: &dyn Fn(u32, u32) -> String| dims.map(|(w, h)| f(w, h));
            match name {
                "name" => Some(template::slice_arg(&stem, arg)),
                "ext" => Some(ext.clone()),
                "folder" => Some(template::slice_arg(&base.folder, arg)),
                "path" => Some(dir.display().to_string()),
                "date_folder" => Some(base.date_folder.clone()).filter(|s| !s.is_empty()),
                "title" => Some(base.title.clone()).filter(|s| !s.is_empty()),
                "index" => Some(index.to_string()),
                "total" => Some(total.to_string()),
                "width" => dims_or(&|w, _| w.to_string()),
                "height" => dims_or(&|_, h| h.to_string()),
                "size" => (!in_memory).then(|| format_size(size)),
                "mp" => dims_or(&|w, h| format!("{:.0}", f64::from(w) * f64::from(h) / 1e6)),
                "ratio" => dims_or(&|w, h| template::ratio_text(w, h)),
                "zoom" => Some(format!("{} %", (scale * 100.0).round().max(1.0))),
                "modified" => modified.map(|m| template::time_arg(chrono::DateTime::<chrono::Local>::from(m).naive_local(), arg)),
                "now" => Some(template::time_arg(chrono::Local::now().naive_local(), arg.or(Some("%H:%M")))),
                "taken" => taken.map(|t| template::time_arg(t, arg)),
                "camera" => match (meta.get(Field::Make), meta.get(Field::Model)) {
                    (Some(make), Some(model)) if !model.to_lowercase().contains(&make.to_lowercase()) => Some(format!("{make} {model}")),
                    (_, Some(model)) => Some(model.to_owned()),
                    (Some(make), None) => Some(make.to_owned()),
                    _ => None,
                },
                "lens" => meta.get(Field::Lens).map(str::to_owned),
                "focal" => meta.get(Field::FocalLength).map(|f| format!("{f} mm")),
                "focal35" => meta.get(Field::FocalLength35).map(|f| format!("{f} mm")),
                "aperture" => meta.get(Field::FNumber).map(|f| format!("f/{f}")),
                "exposure" => meta.get(Field::Exposure).map(|e| format!("{} s", e.replacen('-', "/", 1))),
                "iso" => meta.get(Field::Iso).map(str::to_owned),
                "position" => meta.get(Field::Position).map(str::to_owned),
                "altitude" => meta.get(Field::Altitude).map(|a| format!("{a} m")),
                "yaw" => meta.get(Field::CameraYaw).map(|a| format!("{a}°")),
                "pitch" => meta.get(Field::CameraPitch).map(|a| format!("{a}°")),
                "comment" => tag("ImageDescription").or_else(|| tag("UserComment")),
                "exif" => arg.and_then(tag),
                _ => None,
            }
        };
        template::render(line, &mut lookup)
    }
}

/// Naabrite indeksid ettelaadimiseks: liikumise suunas kaks, siis üks
/// tagasi jne (+1, +2, −1, +3, +4, −2); `wrap`: üle otsa ringiga.
/// Praegust ega kordusi ei ole.
fn neighbour_order(cursor: usize, n: usize, direction: i8, wrap: bool) -> Vec<usize> {
    let dir: isize = if direction < 0 { -1 } else { 1 };
    let mut deltas: Vec<isize> = Vec::new();
    let (mut a, mut b) = (1, 1);
    while a <= AHEAD || b <= BEHIND {
        for _ in 0..2 {
            if a <= AHEAD {
                deltas.push(dir * a as isize);
                a += 1;
            }
        }
        if b <= BEHIND {
            deltas.push(-dir * b as isize);
            b += 1;
        }
    }
    let mut out: Vec<usize> = Vec::new();
    for d in deltas {
        let i = cursor as isize + d;
        let i = if wrap && n > 0 {
            i.rem_euclid(n as isize)
        } else if (0..n as isize).contains(&i) {
            i
        } else {
            continue;
        };
        let i = i as usize;
        if i != cursor && !out.contains(&i) {
            out.push(i);
        }
    }
    out
}

/// Pildi ala: serv ja all ruum klahvireale (mahutatud pilt ei lähe selle alla).
fn picture_area(stage: Rect, hints: bool) -> Rect {
    let mut area = stage.shrink(12.0);
    if hints {
        area.max.y -= HINT_ROOM;
    }
    area
}

/// Mahutamise suurendus: pilt alasse, väiksemaks; suuremaks ainult seadega.
fn fit_scale(natural: Vec2, area: Vec2, enlarge: bool) -> f32 {
    if natural.x <= 0.0 || natural.y <= 0.0 {
        return 1.0;
    }
    let s = (area.x / natural.x).min(area.y / natural.y);
    if enlarge { s } else { s.min(1.0) }
}

fn fitted_window_size(natural: Vec2, padding: Vec2, available: Vec2) -> Vec2 {
    let scale = fit_scale(natural, (available - padding).max(Vec2::ZERO), true);
    (natural * scale + padding).max(Vec2::from(MIN_WINDOW_SIZE)).min(available)
}

/// Järgmine aste üles või alla praegusest (mis ei pruugi olla astmel);
/// seade „10 %" / „25 %" korrutab.
fn zoom_step(current: f32, up: bool, step: ZoomStep) -> f32 {
    const EPS: f32 = 0.001;
    let (lo, hi) = (ZOOM_STEPS[0], ZOOM_STEPS[ZOOM_STEPS.len() - 1]);
    match step {
        ZoomStep::Steps => {
            if up {
                ZOOM_STEPS.iter().copied().find(|&s| s > current + EPS).unwrap_or(hi)
            } else {
                ZOOM_STEPS.iter().rev().copied().find(|&s| s < current - EPS).unwrap_or(lo)
            }
        }
        ZoomStep::Ten | ZoomStep::TwentyFive => {
            let f = if step == ZoomStep::Ten { 1.1 } else { 1.25 };
            (if up { current * f } else { current / f }).clamp(lo, hi)
        }
    }
}

/// Nihe nii, et pilt ei lahkuks alast: alast suurem pilt katab ala,
/// väiksem on keskel.
fn clamp_pan(pan: Vec2, shown: Vec2, area: Vec2) -> Vec2 {
    let lim = ((shown - area) / 2.0).max(Vec2::ZERO);
    pan.clamp(-lim, lim)
}

/// Allhoitud noolte nihe selles kaadris: vaade liigub noole suunas nagu
/// kerides; noolevajutused tarbitakse, et pilti ei vahetataks.
fn arrow_pan(ctx: &egui::Context) -> Vec2 {
    const SPEED: f32 = 900.0;
    let arrows = [(Key::ArrowLeft, Vec2::new(-1.0, 0.0)), (Key::ArrowRight, Vec2::new(1.0, 0.0)), (Key::ArrowUp, Vec2::new(0.0, -1.0)), (Key::ArrowDown, Vec2::new(0.0, 1.0))];
    let (dir, dt) = ctx.input_mut(|i| {
        let mut dir = Vec2::ZERO;
        for (k, d) in arrows {
            while i.consume_key(Modifiers::NONE, k) {}
            if i.key_down(k) && i.modifiers.is_none() {
                dir += d;
            }
        }
        (dir, i.stable_dt.min(0.1))
    });
    if dir == Vec2::ZERO {
        return Vec2::ZERO;
    }
    ctx.request_repaint();
    -dir * SPEED * dt
}

/// Suurendus kursori kohal: kursori all olev pildipunkt jääb paigale.
fn zoom_at(pan: Vec2, cursor_from_center: Vec2, old: f32, new: f32) -> Vec2 {
    let o = cursor_from_center - pan;
    cursor_from_center - o * (new / old)
}

fn stage_color(bg: Background) -> Color32 {
    match bg {
        Background::Dark => Color32::from_rgb(0x1E, 0x22, 0x28),
        Background::Black => Color32::BLACK,
        Background::Gray => Color32::from_rgb(0x6A, 0x6E, 0x74),
    }
}

/// Vormingud, millel võib olla läbipaistvus.
fn has_alpha(path: &Path) -> bool {
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    matches!(ext.as_str(), "png" | "webp" | "gif" | "tif" | "tiff" | "svg" | "svgz" | "avif" | "ico" | "tga")
}

/// Hall-valge ruudustik läbipaistva ala all.
fn checkerboard(painter: &egui::Painter, rect: Rect) {
    const CELL: f32 = 12.0;
    let (a, b) = (Color32::from_rgb(0xC8, 0xC8, 0xC8), Color32::from_rgb(0xF4, 0xF4, 0xF4));
    painter.rect_filled(rect, CornerRadius::ZERO, b);
    let cols = (rect.width() / CELL).ceil() as i32;
    let rows = (rect.height() / CELL).ceil() as i32;
    for row in 0..rows {
        for col in 0..cols {
            if (row + col) % 2 == 0 {
                let cell = Rect::from_min_size(rect.min + Vec2::new(col as f32 * CELL, row as f32 * CELL), Vec2::splat(CELL)).intersect(rect);
                painter.rect_filled(cell, CornerRadius::ZERO, a);
            }
        }
    }
}

/// Pilt ristkülikus muudatusega: ekraani nurk saab tekstuuri nurga
/// pöörde võrra tagasi ringiga ja peegli korral vastasservalt.
fn transformed_image(texture: egui::TextureId, rect: Rect, edit: Edit) -> egui::Shape {
    let uv = [egui::pos2(0.0, 0.0), egui::pos2(1.0, 0.0), egui::pos2(1.0, 1.0), egui::pos2(0.0, 1.0)];
    let corners = [rect.left_top(), rect.right_top(), rect.right_bottom(), rect.left_bottom()];
    let mut mesh = egui::Mesh::with_texture(texture);
    for (i, pos) in corners.into_iter().enumerate() {
        let mut t = uv[(i + 4 - usize::from(edit.rot % 4)) % 4];
        if edit.flip_h {
            t.x = 1.0 - t.x;
        }
        mesh.vertices.push(egui::epaint::Vertex { pos, uv: t, color: Color32::WHITE });
    }
    mesh.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    egui::Shape::mesh(mesh)
}

/// Klahvirida pildi all: iga käsk oma kastis (klahvimärk + silt), ühes
/// reas. Kui kõik ei mahu: sildid ära; siis vähemtähtsad kastid tähtsuse
/// järjekorras. Kast on nupp: klõps teeb sama mis klahv.
fn hints(ui: &mut egui::Ui, font_size: f32, stage: Rect, zoomed: bool) -> Option<(Modifiers, Key)> {
    struct Entry {
        keys: &'static [(&'static str, Modifiers, Key)],
        label: &'static str,
        rank: u8,
    }
    // Suurendatult nooled nihutavad pilti; pildivahetus on siis PgUp/PgDn.
    let browse: &'static [(&'static str, Modifiers, Key)] =
        if zoomed { &[("PgUp", Modifiers::NONE, Key::PageUp), ("PgDn", Modifiers::NONE, Key::PageDown)] } else { &[("←", Modifiers::NONE, Key::ArrowLeft), ("→", Modifiers::NONE, Key::ArrowRight)] };
    let entries = [
        Entry { keys: browse, label: "previous / next", rank: 0 },
        Entry { keys: &[("+", Modifiers::NONE, Key::Plus), ("−", Modifiers::NONE, Key::Minus)], label: "zoom", rank: 0 },
        Entry { keys: &[("F", Modifiers::NONE, Key::F)], label: "fit to screen / 100%", rank: 0 },
        Entry { keys: &[("Enter", Modifiers::NONE, Key::Enter)], label: "full screen", rank: 0 },
        Entry { keys: &[("R", Modifiers::NONE, Key::R), ("L", Modifiers::NONE, Key::L)], label: "rotate", rank: 1 },
        Entry { keys: &[("H", Modifiers::NONE, Key::H), ("V", Modifiers::NONE, Key::V)], label: "mirror", rank: 1 },
        Entry { keys: &[("Ctrl+R", Modifiers::COMMAND, Key::R)], label: "resize", rank: 1 },
        Entry { keys: &[("Ctrl+S", Modifiers::COMMAND, Key::S)], label: "save", rank: 1 },
        Entry { keys: &[("F2", Modifiers::NONE, Key::F2)], label: "rename", rank: 1 },
        Entry { keys: &[("Del", Modifiers::NONE, Key::Delete)], label: "delete", rank: 1 },
        Entry { keys: &[("Ctrl+O", Modifiers::COMMAND, Key::O)], label: "open", rank: 2 },
        Entry { keys: &[("P", Modifiers::NONE, Key::P)], label: "settings", rank: 2 },
        Entry { keys: &[("Esc", Modifiers::NONE, Key::Escape)], label: "close", rank: 2 },
    ];
    const H: f32 = 22.0;
    const GAP: f32 = 5.0;
    const PAD: f32 = 5.0;
    let caps = (font_size - 2.5).round();
    let font = FontId::proportional(caps);
    let key_font = FontId::monospace(caps);
    let painter = ui.painter().clone();
    let key_w = |k: &str| painter.layout_no_wrap(k.to_owned(), key_font.clone(), Color32::WHITE).size().x + 8.0;
    let label_w = |l: &str| painter.layout_no_wrap(l.to_owned(), font.clone(), Color32::WHITE).size().x;
    let box_w = |e: &Entry, label_rank: i8| {
        let keys: f32 = e.keys.iter().map(|(k, _, _)| key_w(k)).sum::<f32>() + 3.0 * (e.keys.len() as f32 - 1.0);
        PAD + keys + if (e.rank as i8) <= label_rank { 5.0 + label_w(e.label) } else { 0.0 } + PAD
    };
    // Astmed: (sildid tähtsuseni, kastid tähtsuseni).
    let tiers: [(i8, u8); 6] = [(2, 2), (1, 2), (0, 2), (-1, 2), (-1, 1), (-1, 0)];
    let (label_rank, max_rank) = tiers.into_iter().find(|&(label_rank, max_rank)| {
        let shown: Vec<&Entry> = entries.iter().filter(|e| e.rank <= max_rank).collect();
        let total: f32 = shown.iter().map(|e| box_w(e, label_rank)).sum::<f32>() + GAP * (shown.len() as f32 - 1.0);
        total <= stage.width() - 24.0
    })?;
    let shown: Vec<&Entry> = entries.iter().filter(|e| e.rank <= max_rank).collect();
    let total: f32 = shown.iter().map(|e| box_w(e, label_rank)).sum::<f32>() + GAP * (shown.len() as f32 - 1.0);
    let mut x = (stage.center().x - total / 2.0).round();
    let y = stage.bottom() - 8.0 - H;
    let mut pressed = None;
    for (n, e) in shown.iter().enumerate() {
        let w = box_w(e, label_rank);
        let rect = Rect::from_min_size(egui::pos2(x, y), Vec2::new(w, H));
        let response = ui.interact(rect, egui::Id::new(("picture-hint", n)), Sense::click());
        let fill = if response.hovered() { Color32::from_rgba_unmultiplied(16, 20, 28, 215) } else { Color32::from_rgba_unmultiplied(16, 20, 28, 150) };
        painter.rect_filled(rect, CornerRadius::same(3), fill);
        let mut kx = rect.left() + PAD;
        for (i, (k, m, key)) in e.keys.iter().enumerate() {
            let kw = key_w(k);
            let badge = Rect::from_min_size(egui::pos2(kx, rect.top() + 3.0), Vec2::new(kw, H - 6.0));
            let badge_response = ui.interact(badge, egui::Id::new(("picture-hint-key", n, i)), Sense::click());
            let badge_fill = if badge_response.hovered() { Color32::from_white_alpha(70) } else { Color32::from_white_alpha(38) };
            painter.rect_filled(badge, CornerRadius::same(3), badge_fill);
            painter.text(badge.center(), Align2::CENTER_CENTER, *k, key_font.clone(), Color32::WHITE);
            if badge_response.clicked() {
                pressed = Some((*m, *key));
            }
            kx += kw + 3.0;
        }
        if (e.rank as i8) <= label_rank {
            painter.text(egui::pos2(kx + 2.0, rect.center().y), Align2::LEFT_CENTER, e.label, font.clone(), Color32::from_rgb(0xC3, 0xCB, 0xD6));
        }
        if response.clicked() && pressed.is_none() {
            let (_, m, key) = e.keys[0];
            pressed = Some((m, key));
        }
        x += w + GAP;
    }
    pressed
}

/// Küsimus all nagu klahvirida, Enter / Esc.
fn ask_bar(ui: &mut egui::Ui, palette: &Palette, font_size: f32, stage: Rect, text: String) {
    let font = FontId::proportional(font_size);
    let galley = ui.painter().layout_no_wrap(text, font, Color32::WHITE);
    let band = Rect::from_center_size(egui::pos2(stage.center().x, stage.bottom() - 26.0), galley.size() + Vec2::new(28.0, 14.0));
    ui.painter().rect(band, CornerRadius::same(4), palette.danger, Stroke::NONE, egui::StrokeKind::Inside);
    ui.painter().galley(band.min + Vec2::new(14.0, 7.0), galley, Color32::WHITE);
}

/// Kast pildi peal (F2 nimi, Ctrl+R, Ctrl+S): vari ja raam; sisu
/// joonistab kutsuja. Tagastab sisu ala.
fn light_box(ui: &mut egui::Ui, palette: &Palette, stage: Rect, size: Vec2, error: bool) -> Rect {
    let field = Rect::from_center_size(egui::pos2(stage.center().x, stage.bottom() - 16.0 - 22.0 - size.y / 2.0), size);
    let border = if error { palette.danger } else { palette.accent };
    let shadow = egui::epaint::Shadow { offset: [0, 6], blur: 20, spread: 0, color: Color32::from_black_alpha(115) };
    ui.painter().add(shadow.as_shape(field, CornerRadius::same(4)));
    ui.painter().rect(field, CornerRadius::same(4), palette.panel, Stroke::new(2.0, border), egui::StrokeKind::Inside);
    field
}

/// Rida kasti all: viga punaselt või klahvid tuhmilt.
fn under_box(ui: &mut egui::Ui, palette: &Palette, font_size: f32, field: Rect, error: Option<&str>, keys: &str) {
    let small = FontId::proportional((font_size - 1.5).round());
    let (text, color) = match error {
        Some(e) => (e.to_owned(), Color32::WHITE),
        None => (keys.to_owned(), Color32::from_rgb(0xC3, 0xCB, 0xD6)),
    };
    let galley = ui.painter().layout_no_wrap(text, small, color);
    let band = Rect::from_center_size(egui::pos2(field.center().x, field.bottom() + 8.0 + 11.0), galley.size() + Vec2::new(20.0, 6.0));
    let fill = if error.is_some() { palette.danger } else { Color32::from_black_alpha(115) };
    ui.painter().rect_filled(band, CornerRadius::same(3), fill);
    ui.painter().galley(band.min + Vec2::new(10.0, 3.0), galley, color);
}

/// Tekstiväli kasti sees ilma raamita; `fresh`: kogu tekst märgitud.
fn box_edit(ui: &mut egui::Ui, palette: &Palette, inner: Rect, id: egui::Id, text: &mut String, fresh: &mut bool, font: FontId) {
    let mut edit = ui.new_child(egui::UiBuilder::new().max_rect(inner).layout(egui::Layout::left_to_right(egui::Align::Center)));
    let out = egui::TextEdit::singleline(text).id(id).frame(egui::Frame::NONE).font(font).text_color(palette.text).vertical_align(egui::Align::Center).desired_width(inner.width()).show(&mut edit);
    if *fresh {
        let mut state = out.state.clone();
        let end = egui::text::CCursor::new(text.chars().count());
        state.cursor.set_char_range(Some(egui::text::CCursorRange::two(egui::text::CCursor::new(0), end)));
        state.store(ui.ctx(), id);
        *fresh = false;
    }
    out.response.request_focus();
}

/// Fookus nupult ära (tekstiväli jääb): klahvid on akna omad.
fn release_widget_focus(ctx: &egui::Context) {
    let Some(id) = ctx.memory(|m| m.focused()) else { return };
    if egui::TextEdit::load_state(ctx, id).is_none() {
        ctx.memory_mut(|m| m.surrender_focus(id));
    }
}

/// Tekst laiusesse, keskelt lühendatud („IMG_00…0123.jpg").
fn elide_middle(ctx: &egui::Context, text: &str, font: &FontId, width: f32) -> String {
    let fits = |s: &str| ctx.fonts_mut(|f| f.layout_no_wrap(s.to_owned(), font.clone(), Color32::WHITE).size().x) <= width;
    if fits(text) {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut keep = chars.len();
    while keep > 2 {
        keep -= 1;
        let head = keep / 2;
        let tail = keep - head;
        let s: String = chars[..head].iter().chain(['…'].iter()).chain(chars[chars.len() - tail..].iter()).collect();
        if fits(&s) {
            return s;
        }
    }
    "…".to_owned()
}

/// Faili suurus inimesele: „8,4 MB".
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 { format!("{bytes} B") } else if v < 10.0 { format!("{v:.1} {}", UNITS[u]) } else { format!("{v:.0} {}", UNITS[u]) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fitted_window_uses_the_limiting_screen_dimension() {
        let available = Vec2::new(1900.0, 1000.0);
        let padding = Vec2::new(24.0, 94.0);
        let wide = fitted_window_size(Vec2::new(4000.0, 1000.0), padding, available);
        let tall = fitted_window_size(Vec2::new(1000.0, 4000.0), padding, available);
        assert_eq!(wide, Vec2::new(1900.0, 563.0));
        assert_eq!(tall, Vec2::new(480.0, 1000.0));
    }

    /// Mahutamine: väiksemaks, mitte suuremaks (seadeta); + − liiguvad
    /// astmeid pidi ka vahepealsest väärtusest, protsendiga korrutades.
    #[test]
    fn fit_shrinks_but_never_enlarges() {
        assert_eq!(fit_scale(Vec2::new(6000.0, 4000.0), Vec2::new(1200.0, 1000.0), false), 0.2);
        assert_eq!(fit_scale(Vec2::new(300.0, 200.0), Vec2::new(1200.0, 1000.0), false), 1.0);
        assert_eq!(fit_scale(Vec2::new(300.0, 200.0), Vec2::new(1200.0, 1000.0), true), 4.0);
        assert_eq!(zoom_step(0.2, true, ZoomStep::Steps), 0.25);
        assert_eq!(zoom_step(0.22, true, ZoomStep::Steps), 0.25);
        assert_eq!(zoom_step(0.22, false, ZoomStep::Steps), 0.2);
        assert_eq!(zoom_step(1.0, true, ZoomStep::Steps), 1.5);
        assert_eq!(zoom_step(12.0, true, ZoomStep::Steps), 12.0);
        assert_eq!(zoom_step(0.05, false, ZoomStep::Steps), 0.05);
        assert!((zoom_step(1.0, true, ZoomStep::Ten) - 1.1).abs() < 1e-5);
        assert!((zoom_step(1.0, false, ZoomStep::TwentyFive) - 0.8).abs() < 1e-5);
    }

    /// Nihe ei vii pilti alast välja; väiksem pilt püsib keskel. Suurendus
    /// kursori kohal jätab kursori all oleva punkti paigale.
    #[test]
    fn pan_stays_inside_and_zoom_keeps_the_point_under_the_cursor() {
        let area = Vec2::new(1000.0, 800.0);
        assert_eq!(clamp_pan(Vec2::new(500.0, -500.0), Vec2::new(1400.0, 1000.0), area), Vec2::new(200.0, -100.0));
        assert_eq!(clamp_pan(Vec2::new(50.0, 50.0), Vec2::new(400.0, 300.0), area), Vec2::ZERO);
        assert_eq!(zoom_at(Vec2::ZERO, Vec2::new(100.0, 0.0), 1.0, 2.0), Vec2::new(-100.0, 0.0));
        assert_eq!(zoom_at(Vec2::new(30.0, 0.0), Vec2::ZERO, 1.0, 2.0), Vec2::new(60.0, 0.0));
    }

    #[test]
    fn test_keys_parse() {
        let key = |modifiers, key| egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers };
        let k = parse_test_keys("Right ctrl+S Shift+Plus nonsense 'uus Enter");
        assert_eq!(k, [key(Modifiers::NONE, Key::ArrowRight), key(Modifiers::COMMAND, Key::S), key(Modifiers::SHIFT, Key::Plus), egui::Event::Text("uus".into()), key(Modifiers::NONE, Key::Enter)]);
    }

    /// Ette laaditakse liikumise suunas rohkem; otsas ei minda üle (ringiga
    /// minnakse); väikeses kaustas kordusi ega praegust pole.
    #[test]
    fn neighbours_follow_the_direction() {
        assert_eq!(neighbour_order(10, 100, 1, false), [11, 12, 9, 13, 14, 8]);
        assert_eq!(neighbour_order(10, 100, -1, false), [9, 8, 11, 7, 6, 12]);
        assert_eq!(neighbour_order(0, 100, 1, false), [1, 2, 3, 4]);
        assert_eq!(neighbour_order(0, 100, -1, true), [99, 98, 1, 97, 96, 2]);
        assert_eq!(neighbour_order(0, 3, 1, true), [1, 2]);
        assert!(neighbour_order(0, 1, 1, true).is_empty());
    }

    #[test]
    fn sizes_for_people() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(8_808_038), "8.4 MB");
        assert_eq!(format_size(25 * 1024 * 1024), "25 MB");
    }
}
