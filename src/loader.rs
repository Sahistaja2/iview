//! Piltide ettelaadija: kaks taustalõime
//! pakivad soovitud pildid lahti, valmis pildid lähevad kaadri algul
//! tekstuuriks. Järjekord asendub iga liigutusega, nii et vahele jäetud
//! pilte ei pakita. Iga pilt pakitakse soovitud mõõtu (`side`, pikem külg
//! pikslites); kui sama pilti soovitakse hiljem suuremana (1:1), pakitakse
//! uuesti ja vana tekstuur jääb seni näha.
//!
//! Lõimed käivituvad enne akent (`start` ei vaja egui konteksti): esimene
//! pilt pakitakse lahti samal ajal, kui Windows akent ja graafikat loob.
//! Kontekst antakse hiljem (`attach`), siis valmis pildid tekstuuriks.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};

use eframe::egui;

/// Lahti pakitud pilt, algse pildi mõõt (vaadates, EXIF-i pööre sees) ja
/// liikuva pildi (GIF, WebP) kaadrid.
type Decoded = (Result<egui::ColorImage, String>, Option<(u32, u32)>, Option<crate::animation::Frames>);

#[derive(Default)]
struct Queue {
    /// Pööra EXIF-i järgi; väljas: nagu fail salvestas.
    no_orientation: bool,
    /// Lahti pakkida, tähtsuse järjekorras (praegune ees).
    wanted: Vec<(PathBuf, u32)>,
    /// Pakkimisel: tee → edenemine (laadimise riba).
    busy: HashMap<PathBuf, Arc<crate::wic::Progress>>,
    /// Praegune pilt (`wanted` esimene): kuni see on töös, ei võta teine
    /// lõim suurt naabrit — kaks 500 MB panoraami samalt kettalt korraga
    /// teeks praeguse kaks korda aeglasemaks.
    current: Option<PathBuf>,
    done: Vec<(PathBuf, u32, Decoded)>,
    stop: bool,
}

struct Shared {
    queue: Mutex<Queue>,
    signal: Condvar,
    /// Akna kontekst (pärast `attach`): valmis pilt tellib kaadri.
    ctx: Mutex<Option<egui::Context>>,
}

impl Shared {
    fn request(&self, wanted: &[(PathBuf, u32)]) {
        let mut q = self.queue.lock().unwrap_or_else(|e| e.into_inner());
        q.current = wanted.first().map(|(p, _)| p.clone());
        q.wanted = wanted.iter().filter(|(p, _)| !q.busy.contains_key(p)).cloned().collect();
        drop(q);
        self.signal.notify_all();
    }
}

/// `Loader::request` ilma laadijata endata (laadija kolib aknasse).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Clone)]
pub struct LoaderHandle {
    shared: Arc<Shared>,
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
impl LoaderHandle {
    pub fn request(&self, wanted: &[(PathBuf, u32)]) {
        self.shared.request(wanted);
    }
}

/// Valmis pilt: tekstuur ja algmõõt; `Err`: ei õnnestunud avada (põhjus).
pub type Ready = Result<(egui::TextureHandle, Option<(u32, u32)>), String>;

pub struct Loader {
    shared: Arc<Shared>,
    /// Tee → (mis mõõtu pakiti, tulemus).
    cache: HashMap<PathBuf, (u32, Ready)>,
    /// Mälus olevad pildid (Ctrl+V): failita, ei visata välja.
    pinned: HashMap<PathBuf, Ready>,
    /// Liikuvad pildid: kaadrid; praegusel vahetub `cache`-i tekstuur aja
    /// järgi. `playing`: pilt, mis eelmisel kaadril mängis (tagasi tulles
    /// algab otsast).
    animations: HashMap<PathBuf, crate::animation::Animation>,
    playing: Option<PathBuf>,
}

impl Loader {
    /// Lõimed tööle (aknata).
    pub fn start() -> Self {
        let shared = Arc::new(Shared { queue: Mutex::new(Queue::default()), signal: Condvar::new(), ctx: Mutex::new(None) });
        for n in 0..2 {
            let shared = shared.clone();
            let _ = std::thread::Builder::new().name(format!("decode-{n}")).spawn(move || worker(&shared));
        }
        Self { shared, cache: HashMap::new(), pinned: HashMap::new(), animations: HashMap::new(), playing: None }
    }

    /// Aken on olemas: valmis pildid tellivad kaadri.
    pub fn attach(&self, ctx: &egui::Context) {
        *self.shared.ctx.lock().unwrap_or_else(|e| e.into_inner()) = Some(ctx.clone());
        ctx.request_repaint();
    }

    /// Ainult järjekord (enne akent): need pildid lahti.
    pub fn request(&self, wanted: &[(PathBuf, u32)]) {
        self.shared.request(wanted);
    }

    /// Järjekorra käepide teisele lõimele või süsteemi sündmusele (macOS:
    /// Finderi fail saabub enne akent, `macos_open`).
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn handle(&self) -> LoaderHandle {
        LoaderHandle { shared: self.shared.clone() }
    }

    /// Kaadri algul: valmis pildid tekstuuriks, liiga kaugele jäänud välja,
    /// puuduvad (või liiga väiksena pakitud) järjekorda. `wanted`: teed
    /// tähtsuse järjekorras koos soovitud mõõduga.
    pub fn update(&mut self, ctx: &egui::Context, wanted: &[(PathBuf, u32)]) {
        let mut q = self.shared.queue.lock().unwrap_or_else(|e| e.into_inner());
        for (path, side, (image, dims, frames)) in std::mem::take(&mut q.done) {
            if !wanted.iter().any(|(p, _)| *p == path) {
                continue;
            }
            match frames {
                Some(frames) => {
                    let animation = crate::animation::Animation::new(ctx, &format!("pilt:{}", path.display()), frames);
                    self.animations.insert(path.clone(), animation);
                }
                None => {
                    self.animations.remove(&path);
                }
            }
            let ready = image.map(|image| (ctx.load_texture(format!("pilt:{}", path.display()), image, egui::TextureOptions::LINEAR), dims));
            self.cache.insert(path, (side, ready));
        }
        self.cache.retain(|p, _| wanted.iter().any(|(w, _)| w == p));
        self.animations.retain(|p, _| wanted.iter().any(|(w, _)| w == p));
        q.current = wanted.first().map(|(p, _)| p.clone());
        q.wanted = wanted.iter().filter(|(p, side)| self.needs(p, *side) && !q.busy.contains_key(p)).cloned().collect();
        drop(q);
        self.shared.signal.notify_all();
        self.play(ctx, wanted.first().map(|(p, _)| p.as_path()));
    }

    /// Praeguse pildi kaader aja järgi `cache`-i (`get` annab selle).
    fn play(&mut self, ctx: &egui::Context, current: Option<&Path>) {
        let animation = current.and_then(|p| self.animations.get_mut(p));
        if let (Some(animation), Some(path)) = (animation, current) {
            if self.playing.as_deref() != Some(path) {
                animation.restart();
            }
            if let Some((_, Ok((texture, _)))) = self.cache.get_mut(path) {
                *texture = animation.frame(ctx).clone();
            }
        }
        self.playing = current.map(Path::to_path_buf);
    }

    /// Pole pakitud, või on pakitud väiksemana kui soovitud ja pilt ise on
    /// suurem (uuesti pakkimine annab teravama).
    fn needs(&self, path: &Path, side: u32) -> bool {
        if self.pinned.contains_key(path) {
            return false;
        }
        match self.cache.get(path) {
            None => true,
            // Vähemalt kaheksandiku võrra suurem: ümardusest tulev 2560 → 2561
            // ei vääri uut pakkimist.
            Some((had, Ok((_, dims)))) => *had + *had / 8 < side && dims.is_some_and(|(w, h)| w.max(h) > *had),
            Some((_, Err(_))) => false,
        }
    }

    /// Pööre enne esimest pilti (`main`, aknata): ainult järjekorra seade.
    pub fn set_orientation_now(&self, apply: bool) {
        self.shared.queue.lock().unwrap_or_else(|e| e.into_inner()).no_orientation = !apply;
    }

    /// EXIF-i pööre: `false` näitab pilti nii, nagu fail salvestas.
    pub fn set_orientation(&mut self, apply: bool) {
        let mut q = self.shared.queue.lock().unwrap_or_else(|e| e.into_inner());
        if q.no_orientation == !apply {
            return;
        }
        q.no_orientation = !apply;
        drop(q);
        self.cache.clear();
        self.animations.clear();
    }

    /// Pakkimise edenemine (loetud, kokku baiti), kui pilt on parajasti töös.
    pub fn progress(&self, path: &Path) -> Option<(u64, u64)> {
        let q = self.shared.queue.lock().unwrap_or_else(|e| e.into_inner());
        let p = q.busy.get(path)?;
        Some((p.read.load(std::sync::atomic::Ordering::Relaxed), p.total.load(std::sync::atomic::Ordering::Relaxed)))
    }

    /// Liikuv pilt seisma või edasi (G); `false`: pilt ei liigu.
    pub fn toggle_pause(&mut self, path: &Path) -> bool {
        self.animations.get_mut(path).map(crate::animation::Animation::toggle_pause).is_some()
    }

    pub fn get(&self, path: &Path) -> Option<&Ready> {
        self.pinned.get(path).or_else(|| self.cache.get(path).map(|(_, r)| r))
    }

    /// Fail muutus kettal (F5): järgmine kaader pakib uuesti.
    pub fn forget(&mut self, path: &Path) {
        self.cache.remove(path);
        self.animations.remove(path);
    }

    /// Mälus olev pilt `path` nime all (faili pole): tekstuur kohe, pikem
    /// külg kuni `MAX_PINNED`; algmõõt jääb pildi mahutamiseks.
    pub fn pin(&mut self, ctx: &egui::Context, path: PathBuf, image: &image::DynamicImage) {
        let dims = (image.width(), image.height());
        let small;
        let shown = if dims.0.max(dims.1) > MAX_PINNED {
            small = image.resize(MAX_PINNED, MAX_PINNED, image::imageops::FilterType::Triangle);
            &small
        } else {
            image
        };
        let rgba = shown.to_rgba8();
        let color = egui::ColorImage::from_rgba_unmultiplied([rgba.width() as usize, rgba.height() as usize], rgba.as_raw());
        let texture = ctx.load_texture(format!("pilt:{}", path.display()), color, egui::TextureOptions::LINEAR);
        self.pinned.insert(path, Ok((texture, Some(dims))));
    }

    /// Mälus olev pilt loendist ära.
    pub fn unpin(&mut self, path: &Path) {
        self.pinned.remove(path);
    }

    /// Fail sai uue nime: valmis pilt jääb alles.
    pub fn renamed(&mut self, from: &Path, to: &Path) {
        if let Some(ready) = self.cache.remove(from) {
            self.cache.insert(to.to_path_buf(), ready);
        }
        if let Some(animation) = self.animations.remove(from) {
            self.animations.insert(to.to_path_buf(), animation);
        }
        if self.playing.as_deref() == Some(from) {
            self.playing = Some(to.to_path_buf());
        }
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        self.shared.queue.lock().unwrap_or_else(|e| e.into_inner()).stop = true;
        self.shared.signal.notify_all();
    }
}

/// Mälus oleva pildi tekstuuri pikem külg (`Loader::pin`).
const MAX_PINNED: u32 = 4096;

/// Naaber, mille võib võtta, kui praegune pilt on veel töös: väike fail.
const SMALL_NEIGHBOUR: u64 = 32 * 1024 * 1024;

fn current_busy(q: &Queue) -> bool {
    q.current.as_ref().is_some_and(|c| q.busy.contains_key(c))
}

/// Järjekorrast esimene, mida tohib praegu võtta: praegune pilt alati;
/// muud ainult siis, kui praegune pole töös või fail on teadaolevalt
/// väike (`sizes`).
fn pickable(q: &Queue, sizes: &HashMap<PathBuf, u64>) -> Option<usize> {
    let current_busy = current_busy(q);
    q.wanted.iter().position(|(p, _)| q.current.as_ref() == Some(p) || !current_busy || sizes.get(p).is_some_and(|&s| s <= SMALL_NEIGHBOUR))
}

fn worker(shared: &Shared) {
    // Naabrite failisuurused: loetakse luku väljas — magav võrguketas
    // hoidis luku all ka UI-lõime (`update`) kinni.
    let mut sizes: HashMap<PathBuf, u64> = HashMap::new();
    loop {
        let (path, side, no_orientation, progress) = {
            let mut q = shared.queue.lock().unwrap_or_else(|e| e.into_inner());
            let pick = loop {
                if q.stop {
                    return;
                }
                if let Some(i) = pickable(&q, &sizes) {
                    break i;
                }
                let unknown: Vec<PathBuf> = if current_busy(&q) { q.wanted.iter().map(|(p, _)| p).filter(|p| !sizes.contains_key(*p)).cloned().collect() } else { Vec::new() };
                if !unknown.is_empty() {
                    drop(q);
                    for p in unknown {
                        let size = std::fs::metadata(&p).map_or(0, |m| m.len());
                        sizes.insert(p, size);
                    }
                    q = shared.queue.lock().unwrap_or_else(|e| e.into_inner());
                    continue;
                }
                q = shared.signal.wait(q).unwrap_or_else(|e| e.into_inner());
            };
            let (path, side) = q.wanted.remove(pick);
            let progress = Arc::new(crate::wic::Progress::default());
            q.busy.insert(path.clone(), progress.clone());
            (path, side, q.no_orientation, progress)
        };
        let started = std::time::Instant::now();
        let picture = crate::decode::decode_with(&path, side, !no_orientation, Some(progress));
        let (image, dims) = match picture {
            Ok(p) => (Ok(p.image), p.dims.or_else(|| crate::decode::dimensions(&path))),
            Err(e) => (Err(e), None),
        };
        let frames = if image.is_ok() { crate::animation::decode(&path, side) } else { None };
        crate::startup::decoded(&path, started.elapsed());
        let mut q = shared.queue.lock().unwrap_or_else(|e| e.into_inner());
        q.busy.remove(&path);
        if q.stop {
            return;
        }
        q.done.push((path, side, (image, dims, frames)));
        drop(q);
        // Praegune valmis: ootav lõim võib nüüd suure naabri võtta.
        shared.signal.notify_all();
        if let Some(ctx) = shared.ctx.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
            ctx.request_repaint();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Praegune pilt võetakse alati; suur naaber alles siis, kui praegune
    /// pole enam töös; väike naaber kohe.
    #[test]
    fn current_first_big_neighbours_wait() {
        let a = PathBuf::from("a.jpg");
        let b = PathBuf::from("b.tif");
        let c = PathBuf::from("c.jpg");
        let mut q = Queue { current: Some(a.clone()), wanted: vec![(b.clone(), 100), (c.clone(), 100)], ..Queue::default() };
        q.busy.insert(a.clone(), Arc::default());
        let mut sizes = HashMap::new();
        sizes.insert(b.clone(), 600 * 1024 * 1024);
        assert_eq!(pickable(&q, &sizes), None);
        sizes.insert(c.clone(), 5 * 1024 * 1024);
        assert_eq!(pickable(&q, &sizes), Some(1));
        q.busy.clear();
        assert_eq!(pickable(&q, &sizes), Some(0));
    }
}
