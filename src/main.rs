//! iview — kiire pildivaatur.
//!
//! Käivituse järjekord on kiiruse pärast selline (iga samm on mõõdetud,
//! `startup.log` seadete kõrval, `IVIEW_TIMING` korral ka konsooli):
//! 1. üks protsess: kui vaatur juba käib, saab see faili ja see käivitus
//!    lõpeb kohe — teist protsessi, graafikat ega akent ei looda;
//! 2. esimese pildi lahtipakkimine algab enne akent, kohe akna mõõtu;
//! 3. samal ajal: kausta loend, graafikadraiveri soojendus, kirjafail;
//! 4. aken luuakse peidetuna ja tuleb nähtavale (kate maha) alles siis,
//!    kui pilt on joonistatud.

// Konsooliakent ei avata väljalaskeversioonis.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod animation;
mod app;
mod cli;
mod clip_image;
mod config;
mod cr3;
mod decode;
mod exif;
mod folder;
mod icon_art;
mod jpeg;
#[cfg(target_os = "macos")]
mod macos_open;
mod loader;
mod platform;
mod raw;
mod save;
mod settings;
mod startup;
mod template;
mod wic;

use std::path::PathBuf;

use eframe::egui;

/// Akna ikoon. Macis on see ka Dockis, seepärast Apple'i ruudustikus (tühi serv).
fn app_icon() -> std::sync::Arc<egui::IconData> {
    #[cfg(target_os = "macos")]
    let (size, inset) = (256, icon_art::MACOS_INSET);
    #[cfg(not(target_os = "macos"))]
    let (size, inset) = (64, 0.0);
    std::sync::Arc::new(egui::IconData { rgba: icon_art::icon_rgba_inset(size, inset), width: size, height: size })
}

/// `--export-iconset KAUST`: Maci ikoonikomplekt (`iconutil -c icns` jaoks),
/// sama joonistus mis programmil endal. Kasutab `tools/macos-app.sh`.
fn export_iconset(dir: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    for base in [16u32, 32, 128, 256, 512] {
        for scale in [1u32, 2] {
            let size = base * scale;
            let name = if scale == 1 { format!("icon_{base}x{base}.png") } else { format!("icon_{base}x{base}@2x.png") };
            let image = image::RgbaImage::from_raw(size, size, icon_art::icon_rgba_inset(size, icon_art::MACOS_INSET)).ok_or("icon pixels")?;
            image.save(dir.join(&name)).map_err(|e| format!("{name}: {e}"))?;
        }
    }
    Ok(())
}

/// Esimese pildi mõõt enne akent: maksimeeritud või täisekraanil ekraani
/// mõõt, muidu salvestatud akna mõõt (füüsilistes pikslites). Sama mõõt,
/// mida aken hiljem soovib — teist, teravamat lahtipakkimist ei tule.
fn first_side(config: &config::Config, fullscreen: bool) -> u32 {
    let placement = config.window.filter(platform::placement_visible);
    let at = placement.map(|p| (p.x + p.w / 2, p.y + p.h / 2));
    let screen = platform::screen_px(at).map_or(1920, |(w, h)| w.max(h));
    let side = match placement {
        Some(p) if !p.maximized && !fullscreen => p.w.max(p.h) as u32,
        None if !fullscreen => (screen * 3 / 5).max(1100),
        _ => screen,
    };
    side.clamp(640, 4096)
}

/// Windowsis eelistame DirectX 12-t ja integreeritud graafikat
/// (Vulkan NVIDIA-l hakkis akna liigutamisel, eraldi kaardi
/// äratamine võtab aega; pildivaaturile piisab integreeritud graafikast).
/// `WGPU_BACKEND` kirjutab üle.
fn prefer_dx12(options: &mut eframe::NativeOptions) {
    #[cfg(windows)]
    {
        use eframe::egui_wgpu::{WgpuSetup, WgpuSetupCreateNew};
        use eframe::wgpu;
        if std::env::var_os("WGPU_BACKEND").is_some() {
            return;
        }
        let mut setup = WgpuSetupCreateNew::without_display_handle();
        setup.instance_descriptor.backends = wgpu::Backends::DX12;
        setup.native_adapter_selector = Some(std::sync::Arc::new(move |adapters, surface| {
            startup::mark("graphics adapters");
            let usable = |a: &&wgpu::Adapter| surface.is_none_or(|s| a.is_surface_supported(s));
            let rank = |a: &wgpu::Adapter| match a.get_info().device_type {
                wgpu::DeviceType::IntegratedGpu => 0,
                wgpu::DeviceType::DiscreteGpu => 1,
                wgpu::DeviceType::VirtualGpu | wgpu::DeviceType::Other => 2,
                wgpu::DeviceType::Cpu => 3,
            };
            adapters.iter().filter(usable).min_by_key(|a| rank(a)).cloned().ok_or_else(|| "no suitable graphics adapter".to_owned())
        }));
        options.wgpu_options.wgpu_setup = WgpuSetup::CreateNew(setup);
    }
    #[cfg(not(windows))]
    let _ = options;
}

fn main() -> eframe::Result<()> {
    startup::begin();
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let cli = match cli::parse(&args) {
        Ok(cli) => cli,
        Err(e) => {
            cli::print(&format!("{e}\n\n{}", cli::USAGE), true);
            std::process::exit(2);
        }
    };
    if cli.help {
        cli::print(cli::USAGE, false);
        return Ok(());
    }
    if let Some(dir) = &cli.export_iconset {
        return match export_iconset(dir) {
            Ok(()) => {
                cli::print(&format!("Icon set written to {}", dir.display()), false);
                Ok(())
            }
            Err(e) => {
                cli::print(&e, true);
                std::process::exit(1);
            }
        };
    }
    if cli.version {
        cli::print(&format!("iview {}", env!("CARGO_PKG_VERSION")), false);
        return Ok(());
    }
    // Valikud keskkonnamuutujateks enne, kui ükski lõim neid loeb.
    // SAFETY: siin on veel üks lõim (midagi pole käivitatud).
    unsafe {
        if let Some(c) = &cli.config {
            std::env::set_var(config::CONFIG_ENV, std::path::absolute(c).unwrap_or_else(|_| c.clone()));
        }
        if cli.timing {
            std::env::set_var(startup::TIMING_ENV, "1");
        }
    }
    let cwd = std::env::current_dir().unwrap_or_default();
    let path = match cli.path.as_deref().map(|raw| cli::resolve(raw, &cwd)) {
        None => None,
        Some(Ok(p)) => Some(p),
        Some(Err(e)) => {
            cli::print(&format!("{e}\n(iview --help shows the usage)"), true);
            std::process::exit(1);
        }
    };
    startup::mark(format!("command line: {args:?} → {path:?}"));
    platform::dpi_aware();
    let config = config::Config::load();
    config.apply();
    startup::mark("settings read");
    let fullscreen = cli.fullscreen || config.open_fullscreen;
    if config.single_instance && !cli.new_window && matches!(platform::claim_instance(path.as_deref()), platform::Claim::Forwarded) {
        return Ok(());
    }
    startup::mark("single instance: this process is the viewer");

    // Esimene pilt lahti kohe — aken ja graafika valmivad samal ajal.
    let loader = loader::Loader::start();
    loader.set_orientation_now(config.exif_rotate);
    startup::warm_gpu();
    // macOS: Finderist avatud fail tuleb Apple Eventina (mitte argumendina)
    // käivitumise ajal, enne akent — siis kohe lahti pakkimisse.
    #[cfg(target_os = "macos")]
    {
        macos_open::install_early();
        if path.is_none() {
            let (handle, side) = (loader.handle(), first_side(&config, fullscreen));
            macos_open::set_early(Some(Box::new(move |p: PathBuf| {
                if p.is_file() {
                    handle.request(&[(p, side)]);
                }
            })));
        }
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let listing = match path.as_ref() {
        // Fail: see kohe lahti; kausta loend taustal.
        Some(p) if p.is_file() => {
            loader.request(&[(p.clone(), first_side(&config, fullscreen))]);
            let dir = p.parent().map(std::path::Path::to_path_buf).unwrap_or_default();
            let (exts, sort) = (config.ext_list(), config.sort);
            let _ = std::thread::Builder::new().name("folder-list".into()).spawn(move || {
                let items = folder::list(&dir, &exts, sort);
                startup::mark(format!("folder listed: {} pictures", items.len()));
                let _ = tx.send(items);
            });
            Some(rx)
        }
        // Kaust: loend kohe (ühe päringuga, millisekundid), esimene pilt lahti.
        Some(p) => {
            let items = folder::list(p, &config.ext_list(), config.sort);
            startup::mark(format!("folder listed: {} pictures", items.len()));
            if let Some(first) = items.first() {
                loader.request(&[(first.path.clone(), first_side(&config, fullscreen))]);
            }
            let _ = tx.send(items);
            Some(rx)
        }
        None => None,
    };
    // Süsteemi kiri (Segoe UI) taustal; egui oma kirjas pole kõiki märke.
    let font = std::thread::Builder::new()
        .name("font".into())
        .spawn(|| {
            let windir = std::env::var_os("WINDIR").map(PathBuf::from)?;
            std::fs::read(windir.join("Fonts").join("segoeui.ttf")).ok()
        })
        .ok();

    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([1100.0, 800.0])
        .with_min_inner_size([480.0, 360.0])
        .with_title(app::NAME)
        .with_icon(app_icon())
        .with_app_id("iview")
        .with_drag_and_drop(true)
        // Peidus, kuni pilt on joonistatud (vt `App::birth`).
        .with_visible(false);
    let mut options = eframe::NativeOptions { viewport, ..Default::default() };
    prefer_dx12(&mut options);
    startup::mark("window prepared");
    let start = app::Start { config, loader, path, listing, font, fullscreen };
    eframe::run_native("iview", options, Box::new(|cc| Ok(Box::new(app::App::new(cc, start)))))
}
