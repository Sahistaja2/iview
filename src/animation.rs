//! Liikuv pilt (GIF, WebP): kaadrid lahti taustalõimes, tekstuurid
//! kaadri kaupa; näidatav kaader valitakse aja järgi ja järgmine joonistus
//! tellitakse kaadri vahetuse ajaks. Laadija (`loader`) hoiab neid.

use std::path::Path;
use std::time::{Duration, Instant};

use eframe::egui;

/// Lahti pakitud kaadrid ja iga kaadri kestus, nagu failis kirjas.
#[derive(Default)]
pub struct Frames {
    pub images: Vec<(egui::ColorImage, Duration)>,
    /// Mitu korda läbi mängida (failist); `None`: lõputult.
    pub plays: Option<u32>,
}

/// Kõigi kaadrite kokku lubatud maht mälus; suuremad kaadrid
/// vähendatakse, kuni mahuvad.
const MAX_BYTES: usize = 400 * 1024 * 1024;

/// GIF / WebP vähemalt kahe kaadriga: kaadrid (pikem külg kuni
/// `max_side`, kokku kuni `MAX_BYTES`); muu fail või üks kaader: `None`.
pub fn decode(path: &Path, max_side: u32) -> Option<Frames> {
    use image::AnimationDecoder;
    let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let reader = || std::fs::File::open(path).ok().map(std::io::BufReader::new);
    let frames = match ext.as_str() {
        "gif" => image::codecs::gif::GifDecoder::new(reader()?).ok()?.into_frames(),
        "webp" => {
            let decoder = image::codecs::webp::WebPDecoder::new(reader()?).ok()?;
            if !decoder.has_animation() {
                return None;
            }
            decoder.into_frames()
        }
        _ => return None,
    };
    let mut buffers: Vec<(image::RgbaImage, Duration)> = Vec::new();
    let mut side = if max_side > 0 { max_side } else { u32::MAX };
    let mut bytes = 0;
    for frame in frames {
        let frame = frame.ok()?;
        let (n, d) = frame.delay().numer_denom_ms();
        let ms = n.checked_div(d).unwrap_or(0);
        // Nii nagu failis; 0 („kohe järgmine") on GIF-i väikseim samm 10 ms
        // (brauserite 100 ms asendust ei tehta).
        let delay = Duration::from_millis(ms.max(10) as u64);
        let buffer = fit(frame.into_buffer(), side);
        bytes += buffer.as_raw().len();
        buffers.push((buffer, delay));
        // Ei mahu (nt 248 kaadrit 667×1000 = 660 MB): kõik väiksemaks.
        while bytes > MAX_BYTES {
            side = buffers[0].0.width().max(buffers[0].0.height()) * 3 / 4;
            if side < 32 {
                return None;
            }
            for (b, _) in &mut buffers {
                *b = fit(std::mem::take(b), side);
            }
            bytes = buffers.iter().map(|(b, _)| b.as_raw().len()).sum();
        }
    }
    let images: Vec<_> = buffers
        .into_iter()
        .map(|(b, delay)| (egui::ColorImage::from_rgba_unmultiplied([b.width() as usize, b.height() as usize], b.as_raw()), delay))
        .collect();
    (images.len() > 1).then(|| Frames { images, plays: plays(path, &ext) })
}

/// Pikem külg kuni `side`.
fn fit(buffer: image::RgbaImage, side: u32) -> image::RgbaImage {
    let (w, h) = buffer.dimensions();
    if w.max(h) <= side {
        return buffer;
    }
    let f = side as f32 / w.max(h) as f32;
    let (nw, nh) = (((w as f32 * f).round() as u32).max(1), ((h as f32 * f).round() as u32).max(1));
    image::imageops::resize(&buffer, nw, nh, image::imageops::FilterType::Triangle)
}

/// Korduste arv failist. GIF: NETSCAPE-laiendita üks kord, laiendiga
/// „kordusi N" → N + 1 korda, 0 → lõputult. WebP: N korda, 0 → lõputult.
fn plays(path: &Path, ext: &str) -> Option<u32> {
    let reader = std::io::BufReader::new(std::fs::File::open(path).ok()?);
    if ext == "gif" {
        let mut decoder = gif::DecodeOptions::new().read_info(reader).ok()?;
        // Laiend on enne esimest kaadrit.
        let _ = decoder.next_frame_info();
        match decoder.repeat() {
            gif::Repeat::Infinite => None,
            gif::Repeat::Finite(n) => Some(u32::from(n) + 1),
        }
    } else {
        match image_webp::WebPDecoder::new(reader).ok()?.loop_count() {
            image_webp::LoopCount::Forever => None,
            image_webp::LoopCount::Times(n) => Some(u32::from(n.get())),
        }
    }
}

/// Tekstuurideks laaditud kaadrid, korduste arv ja algushetk.
pub struct Animation {
    frames: Vec<(egui::TextureHandle, Duration)>,
    total: Duration,
    plays: Option<u32>,
    start: Instant,
    /// Peatatud (G): aeg seisab sellel hetkel.
    paused_at: Option<Instant>,
}

impl Animation {
    pub fn new(ctx: &egui::Context, name: &str, frames: Frames) -> Self {
        let Frames { images, plays } = frames;
        let total = images.iter().map(|(_, d)| *d).sum();
        let frames = images.into_iter().enumerate().map(|(i, (image, delay))| (ctx.load_texture(format!("{name}#{i}"), image, egui::TextureOptions::LINEAR), delay)).collect();
        Self { frames, total, plays, start: Instant::now(), paused_at: None }
    }

    /// Algusest uuesti (pildile tagasi tulles), peatus maha.
    pub fn restart(&mut self) {
        self.start = Instant::now();
        self.paused_at = None;
    }

    /// Peata või jätka (G, nagu IrfanView): jätkates sealt, kus jäi.
    pub fn toggle_pause(&mut self) {
        match self.paused_at.take() {
            Some(at) => self.start += at.elapsed(),
            None => self.paused_at = Some(Instant::now()),
        }
    }

    /// Praegune kaader; järgmine joonistus kaadri vahetuseks. Kordused
    /// läbi: viimane kaader jääb.
    pub fn frame(&self, ctx: &egui::Context) -> &egui::TextureHandle {
        let total = self.total.as_nanos().max(1);
        let elapsed = self.paused_at.unwrap_or_else(Instant::now).duration_since(self.start).as_nanos();
        if self.plays.is_some_and(|n| elapsed >= total * u128::from(n)) {
            return &self.frames[self.frames.len() - 1].0;
        }
        let mut t = Duration::from_nanos((elapsed % total) as u64);
        for (texture, delay) in &self.frames {
            if t < *delay {
                if self.paused_at.is_none() {
                    ctx.request_repaint_after(*delay - t);
                }
                return texture;
            }
            t -= *delay;
        }
        if self.paused_at.is_none() {
            ctx.request_repaint();
        }
        &self.frames[0].0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gif(path: &Path, frames: &[[u8; 4]], delay_ms: u32) {
        gif_repeat(path, frames, delay_ms, Some(image::codecs::gif::Repeat::Infinite));
    }

    fn gif_repeat(path: &Path, frames: &[[u8; 4]], delay_ms: u32, repeat: Option<image::codecs::gif::Repeat>) {
        let file = std::fs::File::create(path).unwrap();
        let mut encoder = image::codecs::gif::GifEncoder::new(file);
        if let Some(repeat) = repeat {
            encoder.set_repeat(repeat).unwrap();
        }
        for color in frames {
            let buffer = image::RgbaImage::from_pixel(8, 6, image::Rgba(*color));
            encoder.encode_frame(image::Frame::from_parts(buffer, 0, 0, image::Delay::from_numer_denom_ms(delay_ms, 1))).unwrap();
        }
    }

    /// G peatab: kaader ei vahetu; jätkates sealt, kus jäi.
    #[test]
    fn pause_holds_the_frame() {
        let ctx = egui::Context::default();
        // Teine kaader pikk: aeglane masin ei jõua tsükliga algusesse tagasi.
        let frames = [([255, 0, 0, 255], 40), ([0, 0, 255, 255], 10_000)];
        let images = frames.iter().map(|(c, ms)| (egui::ColorImage::from_rgba_unmultiplied([1, 1], c), Duration::from_millis(*ms))).collect();
        let mut animation = Animation::new(&ctx, "katse", Frames { images, plays: None });
        animation.toggle_pause();
        let held = animation.frame(&ctx).id();
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(animation.frame(&ctx).id(), held);
        animation.toggle_pause();
        assert_eq!(animation.frame(&ctx).id(), held, "jätkab samast kaadrist");
        std::thread::sleep(Duration::from_millis(50));
        assert_ne!(animation.frame(&ctx).id(), held);
    }

    /// Mitme kaadriga GIF annab kaadrid ja kestused (0 ms → 100 ms nagu
    /// brauseris); ühe kaadriga ja muud failid jäävad liikumatuks.
    #[test]
    fn animated_gif_gives_frames() {
        let dir = tempfile::tempdir().unwrap();
        let moving = dir.path().join("liikuv.gif");
        gif(&moving, &[[255, 0, 0, 255], [0, 0, 255, 255], [0, 255, 0, 255]], 70);
        let frames = decode(&moving, 0).expect("kaadrid");
        assert_eq!(frames.images.len(), 3);
        assert_eq!(frames.images[0].0.size, [8, 6]);
        assert_eq!(frames.images[1].1, Duration::from_millis(70));
        assert_eq!(frames.images[1].0.pixels[0], egui::Color32::from_rgb(0, 0, 255));
        assert_eq!(frames.plays, None);

        let fast = dir.path().join("kiire.gif");
        gif(&fast, &[[0, 0, 0, 255], [255, 255, 255, 255]], 0);
        let fast = decode(&fast, 4).unwrap();
        assert_eq!(fast.images[0].1, Duration::from_millis(10));
        assert_eq!(fast.images[0].0.size, [4, 3]);

        // Korduste arv failist: laiendita üks kord, „kordusi 2" → 3 korda.
        let once = dir.path().join("korra.gif");
        gif_repeat(&once, &[[0, 0, 0, 255], [255, 255, 255, 255]], 50, None);
        assert_eq!(decode(&once, 0).unwrap().plays, Some(1));
        let three = dir.path().join("kolm.gif");
        gif_repeat(&three, &[[0, 0, 0, 255], [255, 255, 255, 255]], 50, Some(image::codecs::gif::Repeat::Finite(2)));
        assert_eq!(decode(&three, 0).unwrap().plays, Some(3));

        let still = dir.path().join("paigal.gif");
        gif(&still, &[[0, 0, 0, 255]], 50);
        assert!(decode(&still, 0).is_none());
        assert!(decode(&dir.path().join("pole.png"), 0).is_none());
    }
}

