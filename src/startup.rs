//! Käivituse ajamõõt: iga etapi aeg protsessi
//! loomisest. `startup.log` (seadete kõrval, iga käivitusega üle
//! kirjutatud) näitab, mis etapp viivitas; `IVIEW_TIMING` korral ka
//! standardveaväljundisse. Mõõdab ka pildi lahtipakkimise aega.

use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const TIMING_ENV: &str = "IVIEW_TIMING";

struct Marks {
    /// `main` algus ja protsessi loomisest selleni kulunud aeg (Windows
    /// laeb programmi, viirusetõrje kontrollib seda).
    start: Option<(Instant, Duration)>,
    list: Vec<(String, Duration)>,
    written: bool,
}

static MARKS: Mutex<Marks> = Mutex::new(Marks { start: None, list: Vec::new(), written: false });

pub fn timing_on() -> bool {
    std::env::var_os(TIMING_ENV).is_some()
}

/// Ajamõõdu rida: standardveaväljundisse ja (väljalaskel pole konsooli)
/// faili `timing.log` seadete kõrval.
pub fn timing_line(text: &str) {
    if !timing_on() {
        return;
    }
    eprintln!("TIMING {text}");
    if let Some(path) = crate::config::Config::path().map(|p| p.with_file_name("timing.log")) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let _ = writeln!(f, "{} {text}", chrono::Local::now().format("%H:%M:%S%.3f"));
        }
    }
}

/// Esimene märk `main`-i alguses.
pub fn begin() {
    if let Ok(mut m) = MARKS.lock() {
        m.start = Some((Instant::now(), before_main()));
    }
}

/// Etapp lõppes.
pub fn mark(label: impl Into<String>) {
    if let Ok(mut m) = MARKS.lock()
        && let Some((start, before)) = m.start
        && !m.written
    {
        m.list.push((label.into(), before + start.elapsed()));
    }
}

/// Pilt pakiti lahti (laadija lõimes): käivituse ajal logisse, `IVIEW_TIMING`
/// korral iga pilt standardveaväljundisse.
pub fn decoded(path: &Path, took: Duration) {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    timing_line(&format!("decoded in {} ms: {name}", took.as_millis()));
    mark(format!("decoded ({} ms): {name}", took.as_millis()));
}

/// Graafikadraiveri soojendus: DirectX 12 adapterite läbikäimine laeb ja
/// käivitab draiveri (NVIDIA-l 250–350 ms). Teises lõimes kohe alguses,
/// samal ajal kui pilt pakitakse lahti ja aken luuakse; eframe'i enda
/// läbikäimine võtab siis ~5 ms. Tulemus hoitakse alles, kuni aken on
/// nähtav — nii jääb draiver vahepeal laetuks.
#[cfg(windows)]
pub fn warm_gpu() {
    use eframe::wgpu;
    if std::env::var_os("WGPU_BACKEND").is_some() {
        return;
    }
    let _ = std::thread::Builder::new().name("gpu-warm".into()).spawn(|| {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::DX12;
        let instance = wgpu::Instance::new(desc);
        let adapters = pollster::block_on(instance.enumerate_adapters(wgpu::Backends::DX12));
        let names: Vec<String> = adapters.iter().map(|a| a.get_info()).map(|i| format!("{} ({:?})", i.name, i.device_type)).collect();
        if let Ok(mut warm) = WARM.lock() {
            *warm = Some((instance, adapters));
        }
        mark(format!("graphics driver warmed up (in the background): {}", names.join(", ")));
    });
}

#[cfg(not(windows))]
pub fn warm_gpu() {}

#[cfg(windows)]
static WARM: Mutex<Option<(eframe::wgpu::Instance, Vec<eframe::wgpu::Adapter>)>> = Mutex::new(None);

/// Aken valmis: kirjad faili (ainult esimene kord).
pub fn finish(label: &str) {
    mark(label.to_owned());
    #[cfg(windows)]
    if let Ok(mut warm) = WARM.lock() {
        warm.take();
    }
    let Ok(mut m) = MARKS.lock() else { return };
    if m.written {
        return;
    }
    m.written = true;
    let text = report(&m);
    if timing_on() {
        eprint!("{text}");
    }
    if let Some(path) = crate::config::Config::path().map(|p| p.with_file_name("startup.log")) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, text);
    }
}

fn report(m: &Marks) -> String {
    let mut text = format!("{}\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"));
    let mut prev = Duration::ZERO;
    if let Some((_, before)) = m.start {
        text += &format!("{:>6} ms  {:>+6} ms  process → main\n", before.as_millis(), before.as_millis());
        prev = before;
    }
    let mut list = m.list.clone();
    list.sort_by_key(|(_, at)| *at);
    for (label, at) in &list {
        text += &format!("{:>6} ms  {:>+6} ms  {label}\n", at.as_millis(), at.saturating_sub(prev).as_millis());
        prev = *at;
    }
    text
}

/// Protsessi loomisest praeguseni.
#[cfg(windows)]
fn before_main() -> Duration {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let (mut created, mut a, mut b, mut c) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
    // SAFETY: oma protsessi pseudokäepide, väljundid on kohalikud muutujad.
    if unsafe { GetProcessTimes(GetCurrentProcess(), &mut created, &mut a, &mut b, &mut c) }.is_err() {
        return Duration::ZERO;
    }
    // FILETIME: 100 ns sammud alates 1601; UNIX-ajast 11 644 473 600 s varem.
    let created = (u64::from(created.dwHighDateTime) << 32 | u64::from(created.dwLowDateTime)) / 10;
    let Ok(now) = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) else { return Duration::ZERO };
    let now = now.as_micros() as u64 + 11_644_473_600_000_000;
    Duration::from_micros(now.saturating_sub(created))
}

#[cfg(not(windows))]
fn before_main() -> Duration {
    Duration::ZERO
}
