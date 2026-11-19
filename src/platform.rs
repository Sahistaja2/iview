//! Süsteemi poolt: üks protsess (teine avamine läheb olemasolevasse
//! aknasse), akna sünd ilma vilkumiseta (DWM-i kate), akna koht
//! füüsilistes pikslites, lõikelaud, redaktor. Iga funktsioonil on
//! `cfg(not(windows))` vaste — aus `None`/tühi.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, Ordering};

/// Akna käepide (HWND) eframe'i aknast.
pub fn window_id(frame: &eframe::Frame) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match frame.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

// ---------- üks protsess ----------

/// Teise käivituse toodud failid, mida aken pole veel võtnud.
static INCOMING: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
/// Akna kontekst: toodud fail tellib kaadri.
static CTX: Mutex<Option<eframe::egui::Context>> = Mutex::new(None);
/// Meie aken (teine käivitus toob selle ette); 0 — veel pole.
static WINDOW: AtomicIsize = AtomicIsize::new(0);

/// Kas käivitus jääb ellu (`Own`) või anti fail juba töötavale aknale.
pub enum Claim {
    /// See protsess on vaatur (ja võtab teiste käivituste failid vastu).
    Own,
    /// Fail anti töötavale vaaturile; see protsess lõpetab.
    #[cfg_attr(not(windows), allow(dead_code))]
    Forwarded,
}

/// Nimi seadete faili järgi: katseprofiil (`IVIEW_CONFIG`) ei saada faile
/// kasutaja aknasse ega võta kasutaja omi vastu.
#[cfg_attr(not(windows), allow(dead_code))]
fn instance_key() -> String {
    use std::hash::{Hash, Hasher};
    let path = crate::config::Config::path().map(|p| p.to_string_lossy().to_lowercase()).unwrap_or_default();
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    std::env::var("USERNAME").unwrap_or_default().to_lowercase().hash(&mut h);
    format!("{:016x}", h.finish())
}

/// Üks protsess: kui vaatur juba käib, saab see `path`-i (uus pilt samas
/// aknas, ette toodud) ja see käivitus lõpeb — protsessi, graafikat ja
/// akent uuesti ei looda. Muidu jääme ise kuulama.
#[cfg(windows)]
pub fn claim_instance(path: Option<&Path>) -> Claim {
    use windows::Win32::Foundation::{ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;
    use windows::core::HSTRING;
    let key = instance_key();
    // SAFETY: nimega objekt; käepide jääb protsessi lõpuni lahti (lukk).
    let Ok(_mutex) = (unsafe { CreateMutexW(None, false, &HSTRING::from(format!("Local\\iview:{key}"))) }) else { return Claim::Own };
    let running = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    let pipe = format!(r"\\.\pipe\iview-{key}");
    if !running {
        serve(pipe);
        return Claim::Own;
    }
    // Töötav vaatur võib alles käivituda (Explorer avab mitu faili korraga
    // mitme protsessina): toru ootame kuni 3 s.
    let started = std::time::Instant::now();
    while started.elapsed() < std::time::Duration::from_secs(3) {
        if let Some(window) = send(&pipe, path) {
            if window != 0 {
                bring_forward(window);
            }
            return Claim::Forwarded;
        }
        std::thread::sleep(std::time::Duration::from_millis(30));
    }
    Claim::Own
}

#[cfg(not(windows))]
pub fn claim_instance(_path: Option<&Path>) -> Claim {
    Claim::Own
}

/// Saadab tee torusse (UTF-8, reavahetusega) ja loeb vastuseks akna käepideme.
#[cfg(windows)]
fn send(pipe: &str, path: Option<&Path>) -> Option<isize> {
    use std::io::{Read, Write};
    let mut file = std::fs::OpenOptions::new().read(true).write(true).open(pipe).ok()?;
    let text = path.map(|p| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf()).to_string_lossy().into_owned()).unwrap_or_default();
    file.write_all(format!("{text}\n").as_bytes()).ok()?;
    let mut reply = [0u8; 8];
    file.read_exact(&mut reply).ok()?;
    Some(i64::from_le_bytes(reply) as isize)
}

/// Toru kuulaja omas lõimes: iga ühendus annab ühe tee.
#[cfg(windows)]
fn serve(pipe: String) {
    use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
    use windows::Win32::Storage::FileSystem::{FILE_FLAGS_AND_ATTRIBUTES, FlushFileBuffers, PIPE_ACCESS_DUPLEX, ReadFile, WriteFile};
    use windows::Win32::System::Pipes::{ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT};
    use windows::core::HSTRING;
    let _ = std::thread::Builder::new().name("instance-pipe".into()).spawn(move || {
        let name = HSTRING::from(pipe);
        loop {
            // SAFETY: toru käepide suletakse iga ühenduse lõpus; puhvrid on kohalikud.
            unsafe {
                let handle: HANDLE = CreateNamedPipeW(
                    &name,
                    FILE_FLAGS_AND_ATTRIBUTES(PIPE_ACCESS_DUPLEX.0),
                    PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                    PIPE_UNLIMITED_INSTANCES,
                    4096,
                    4096,
                    0,
                    None,
                );
                if handle == INVALID_HANDLE_VALUE {
                    return;
                }
                // Klient võis ühenduda enne `ConnectNamedPipe`-i — siis on viga, aga ühendus olemas.
                let _ = ConnectNamedPipe(handle, None);
                let mut text = Vec::new();
                let mut buf = [0u8; 4096];
                while !text.ends_with(b"\n") && text.len() < 64 * 1024 {
                    let mut n = 0u32;
                    if ReadFile(handle, Some(&mut buf), Some(&mut n), None).is_err() || n == 0 {
                        break;
                    }
                    text.extend_from_slice(&buf[..n as usize]);
                }
                let reply = (WINDOW.load(Ordering::SeqCst) as i64).to_le_bytes();
                let _ = WriteFile(handle, Some(&reply), None, None);
                let _ = FlushFileBuffers(handle);
                let _ = DisconnectNamedPipe(handle);
                let _ = CloseHandle(handle);
                let line = String::from_utf8_lossy(&text).trim().to_owned();
                if !line.is_empty() {
                    push_incoming(PathBuf::from(line));
                }
            }
        }
    });
}

/// Aken on olemas: teise käivituse fail tellib kaadri ja aken tuuakse ette.
pub fn publish_window(ctx: &eframe::egui::Context, window: Option<isize>) {
    *CTX.lock().unwrap_or_else(|e| e.into_inner()) = Some(ctx.clone());
    if let Some(w) = window {
        WINDOW.store(w, Ordering::SeqCst);
    }
}

/// Fail tuli väljast (teine käivitus, macOS-i Finder): järjekorda ja kaader.
#[cfg_attr(not(any(windows, target_os = "macos")), allow(dead_code))]
pub fn push_incoming(path: PathBuf) {
    INCOMING.lock().unwrap_or_else(|e| e.into_inner()).push(path);
    if let Some(ctx) = CTX.lock().unwrap_or_else(|e| e.into_inner()).as_ref() {
        ctx.request_repaint();
    }
}

/// `file:///Users/a/My%20Pictures/x.jpg` → tee baitidena (protsendid lahti,
/// `localhost` lubatud). `None`: pole `file:` URL.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn file_url_bytes(url: &str) -> Option<Vec<u8>> {
    let rest = url.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    if !rest.starts_with('/') {
        return None;
    }
    let b = rest.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok())
        {
            out.push(v);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    // Kausta URL lõpeb kaldkriipsuga.
    if out.len() > 1 && out.ends_with(b"/") {
        out.pop();
    }
    Some(out)
}

/// Teise käivituse toodud failid (viimane loeb).
pub fn take_incoming() -> Vec<PathBuf> {
    std::mem::take(&mut *INCOMING.lock().unwrap_or_else(|e| e.into_inner()))
}

/// Toob teise protsessi akna ette (minimeeritud — taastab). Äsja käivitatud
/// protsessil on Windowsis õigus esiplaani anda.
#[cfg(windows)]
fn bring_forward(window: isize) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{IsIconic, SW_RESTORE, SetForegroundWindow, ShowWindow};
    let hwnd = HWND(window as _);
    // SAFETY: käepide võib olla aegunud — siis kutsed lihtsalt ebaõnnestuvad.
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let _ = SetForegroundWindow(hwnd);
    }
}

// ---------- akna sünd ja koht ----------

/// Protsess ekraanipunkti-teadlikuks enne, kui winit seda teeb: nii annab
/// `screen_px` juba enne akent päris pikslid (esimese pildi mõõt).
#[cfg(windows)]
pub fn dpi_aware() {
    use windows::Win32::UI::HiDpi::{DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext};
    // SAFETY: protsessi seade; teist korda (winit) annab vea, mida eiratakse.
    let _ = unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
}

#[cfg(not(windows))]
pub fn dpi_aware() {}

/// Ekraani mõõt pikslites, millel on punkt `at` (füüsilised pikslid), või
/// põhiekraan.
#[cfg(windows)]
pub fn screen_px(at: Option<(i32, i32)>) -> Option<(u32, u32)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint};
    let (x, y) = at.unwrap_or((0, 0));
    // SAFETY: lihtsad päringud; struktuuri suurus on õige.
    unsafe {
        let monitor = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return None;
        }
        let r = info.rcMonitor;
        Some(((r.right - r.left).max(1) as u32, (r.bottom - r.top).max(1) as u32))
    }
}

#[cfg(not(windows))]
pub fn screen_px(_at: Option<(i32, i32)>) -> Option<(u32, u32)> {
    None
}

/// Mahutab akna praeguse ekraani tööalasse, arvestades aknaraami.
#[cfg(windows)]
pub fn resize_to_work_area(window: isize, client_size: impl FnOnce(eframe::egui::Vec2) -> eframe::egui::Vec2) {
    use eframe::egui::Vec2;
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow};
    use windows::Win32::UI::HiDpi::{AdjustWindowRectExForDpi, GetDpiForWindow};
    use windows::Win32::UI::WindowsAndMessaging::{GWL_EXSTYLE, GWL_STYLE, GetWindowLongW, IsZoomed, SW_RESTORE, SWP_NOACTIVATE, SWP_NOZORDER, SetWindowPos, ShowWindow, WINDOW_EX_STYLE, WINDOW_STYLE, WS_MAXIMIZE};

    let hwnd = HWND(window as _);
    // SAFETY: vaaturi aknakäepide ja õigesti algväärtustatud struktuurid.
    unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(monitor, &mut info).as_bool() {
            return;
        }
        let style = WINDOW_STYLE(GetWindowLongW(hwnd, GWL_STYLE) as u32 & !WS_MAXIMIZE.0);
        let ex_style = WINDOW_EX_STYLE(GetWindowLongW(hwnd, GWL_EXSTYLE) as u32);
        let mut frame = RECT::default();
        if AdjustWindowRectExForDpi(&mut frame, style, false, ex_style, GetDpiForWindow(hwnd)).is_err() {
            return;
        }

        let work = info.rcWork;
        let work_w = work.right - work.left;
        let work_h = work.bottom - work.top;
        let frame_w = frame.right - frame.left;
        let frame_h = frame.bottom - frame.top;
        let available = Vec2::new((work_w - frame_w) as f32, (work_h - frame_h) as f32);
        if available.min_elem() <= 0.0 {
            return;
        }
        let size = client_size(available).min(available);
        let w = size.x as i32 + frame_w;
        let h = size.y as i32 + frame_h;
        if IsZoomed(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let x = work.left + (work_w - w) / 2;
        let y = work.top + (work_h - h) / 2;
        let _ = SetWindowPos(hwnd, None, x, y, w, h, SWP_NOACTIVATE | SWP_NOZORDER);
    }
}

#[cfg(not(windows))]
pub fn resize_to_work_area(_window: isize, _client_size: impl FnOnce(eframe::egui::Vec2) -> eframe::egui::Vec2) {}

/// Akna tavaasend (taastatud olekus) füüsilistes pikslites ja kas aken on
/// maksimeeritud. Punktid ei sobi: eri skaalaga ekraanidel on igal ekraanil
/// oma punktid ja winit teisendab loomisel põhiekraani skaalaga.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Placement {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub maximized: bool,
}

#[cfg(windows)]
pub fn placement(window: isize) -> Option<Placement> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowPlacement, SW_SHOWMAXIMIZED, SW_SHOWMINIMIZED, WINDOWPLACEMENT, WPF_RESTORETOMAXIMIZED};
    let mut p = WINDOWPLACEMENT { length: std::mem::size_of::<WINDOWPLACEMENT>() as u32, ..Default::default() };
    // SAFETY: lihtne päring; struktuuri pikkus on seatud.
    unsafe { GetWindowPlacement(HWND(window as _), &mut p) }.ok()?;
    let r = p.rcNormalPosition;
    let maximized = p.showCmd == SW_SHOWMAXIMIZED.0 as u32 || (p.showCmd == SW_SHOWMINIMIZED.0 as u32 && p.flags.contains(WPF_RESTORETOMAXIMIZED));
    Some(Placement { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top, maximized })
}

#[cfg(not(windows))]
pub fn placement(_window: isize) -> Option<Placement> {
    None
}

/// Kas salvestatud asend on mõnel praegusel ekraanil (vähemalt nurk)?
#[cfg(windows)]
pub fn placement_visible(p: &Placement) -> bool {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONULL, MonitorFromPoint};
    // SAFETY: lihtne päring.
    !unsafe { MonitorFromPoint(POINT { x: p.x + 40, y: p.y + 20 }, MONITOR_DEFAULTTONULL) }.is_invalid() && p.w > 100 && p.h > 100
}

#[cfg(not(windows))]
pub fn placement_visible(_p: &Placement) -> bool {
    false
}

/// Peidetud aken salvestatud kohta ja mõõtu (enne nähtavaks tegemist,
/// ilma näitamata).
#[cfg(windows)]
pub fn place_hidden(window: isize, p: &Placement) {
    use windows::Win32::Foundation::{HWND, RECT};
    use windows::Win32::UI::WindowsAndMessaging::{SW_HIDE, SetWindowPlacement, WINDOWPLACEMENT};
    let wp = WINDOWPLACEMENT {
        length: std::mem::size_of::<WINDOWPLACEMENT>() as u32,
        showCmd: SW_HIDE.0 as u32,
        rcNormalPosition: RECT { left: p.x, top: p.y, right: p.x + p.w, bottom: p.y + p.h },
        ..Default::default()
    };
    // SAFETY: kehtiv käepide; struktuur on täidetud.
    let _ = unsafe { SetWindowPlacement(HWND(window as _), &wp) };
}

#[cfg(not(windows))]
pub fn place_hidden(_window: isize, _p: &Placement) {}

/// DWM-i kate: aken on Windowsi jaoks nähtav (saab mõõdu, joonistub), aga
/// ekraanil seda pole. Nii ei näe kasutaja tühja ega vale mõõduga akent.
#[cfg(windows)]
pub fn cloak(window: isize, on: bool) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{DWMWA_CLOAK, DwmSetWindowAttribute};
    use windows::core::BOOL;
    let value = BOOL(on as i32);
    // SAFETY: kehtiv aknakäepide; väärtus on BOOL ja suurus antakse õigesti.
    let _ = unsafe { DwmSetWindowAttribute(HWND(window as _), DWMWA_CLOAK, std::ptr::addr_of!(value).cast(), std::mem::size_of::<BOOL>() as u32) };
}

#[cfg(not(windows))]
pub fn cloak(_window: isize, _on: bool) {}

/// Windowsi akna animatsioonid (avamisel väike aken, mis suureks kasvab)
/// sisse või välja: avamisel välja — aken tekib kohe lõplikus mõõdus.
#[cfg(windows)]
pub fn transitions(window: isize, on: bool) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{DWMWA_TRANSITIONS_FORCEDISABLED, DwmSetWindowAttribute};
    use windows::core::BOOL;
    let value = BOOL(i32::from(!on));
    // SAFETY: kehtiv aknakäepide; väärtus on BOOL ja suurus antakse õigesti.
    let _ = unsafe { DwmSetWindowAttribute(HWND(window as _), DWMWA_TRANSITIONS_FORCEDISABLED, std::ptr::addr_of!(value).cast(), std::mem::size_of::<BOOL>() as u32) };
}

#[cfg(not(windows))]
pub fn transitions(_window: isize, _on: bool) {}

/// Tume tiitliriba (pildi taust on tume — hele riba pimestab).
#[cfg(windows)]
pub fn dark_title(window: isize) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
    use windows::core::BOOL;
    let value = BOOL(1);
    // SAFETY: kehtiv aknakäepide; väärtus on BOOL.
    let _ = unsafe { DwmSetWindowAttribute(HWND(window as _), DWMWA_USE_IMMERSIVE_DARK_MODE, std::ptr::addr_of!(value).cast(), std::mem::size_of::<BOOL>() as u32) };
}

#[cfg(not(windows))]
pub fn dark_title(_window: isize) {}

// ---------- klaviatuur, failid ----------

/// V-klahv all (Ctrl+V lõikelaua pildiga: egui ei anna sündmust, kui
/// lõikelaual pole teksti).
#[cfg(windows)]
pub fn v_key_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
    // SAFETY: lihtne olekupäring.
    unsafe { GetAsyncKeyState(0x56) < 0 }
}

#[cfg(not(windows))]
pub fn v_key_down() -> bool {
    false
}

#[cfg(windows)]
pub fn is_hidden(path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    std::fs::metadata(path).map(|m| m.file_attributes() & 2 != 0).unwrap_or(false)
}

#[cfg(not(windows))]
pub fn is_hidden(path: &Path) -> bool {
    path.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))
}

/// F4: pildiredaktor. Tühi seade: Windowsi „Muuda" (nt Paint), selle
/// puudumisel „Ava".
pub fn edit(path: &Path, editor: &str) -> Result<(), String> {
    let editor = editor.trim();
    if !editor.is_empty() {
        return std::process::Command::new(editor).arg(path).spawn().map(|_| ()).map_err(|e| format!("Cannot start the editor {editor}: {e}"));
    }
    shell_verb(path, "edit").or_else(|_| shell_verb(path, "open"))
}

#[cfg(windows)]
fn shell_verb(path: &Path, verb: &str) -> Result<(), String> {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::HSTRING;
    // SAFETY: nullidega lõppevad stringid elavad kutse lõpuni.
    let r = unsafe { ShellExecuteW(None, &HSTRING::from(verb), &HSTRING::from(path.as_os_str()), None, None, SW_SHOWNORMAL) };
    if r.0 as isize > 32 { Ok(()) } else { Err(format!("Windows has no “{verb}” action for this file")) }
}

#[cfg(not(windows))]
fn shell_verb(path: &Path, _verb: &str) -> Result<(), String> {
    std::process::Command::new("xdg-open").arg(path).spawn().map(|_| ()).map_err(|e| e.to_string())
}

// ---------- lõikelaud ----------

/// Pilt lõikelaual (Ctrl+V).
pub enum ClipboardImage {
    #[cfg_attr(not(windows), allow(dead_code))]
    Png(Vec<u8>),
    /// BMP ilma failipäiseta (`clip_image::from_dib`).
    #[cfg_attr(not(windows), allow(dead_code))]
    Dib(Vec<u8>),
}

/// Faililoend lõikelaua vormingutena (`CF_HDROP` ja „Preferred DropEffect").
#[cfg(windows)]
fn files_clip(paths: &[PathBuf]) -> Vec<(u32, Vec<u8>)> {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::POINT;
    use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
    use windows::Win32::UI::Shell::DROPFILES;
    use windows::core::w;

    const CF_HDROP: u32 = 15;
    let header = std::mem::size_of::<DROPFILES>();
    let df = DROPFILES { pFiles: header as u32, pt: POINT::default(), fNC: false.into(), fWide: true.into() };
    let mut files = vec![0u8; header];
    // SAFETY: puhver on täpselt DROPFILES-i suurune.
    unsafe { std::ptr::write_unaligned(files.as_mut_ptr() as *mut DROPFILES, df) };
    for p in paths {
        files.extend(p.as_os_str().encode_wide().chain(std::iter::once(0)).flat_map(u16::to_le_bytes));
    }
    files.extend([0, 0]);
    // Kopeerimisel COPY | LINK = 5, nagu Explorer.
    let effect = 5u32.to_le_bytes().to_vec();
    // SAFETY: konstantne nulliga lõppev nimi.
    let format = unsafe { RegisterClipboardFormatW(w!("Preferred DropEffect")) };
    vec![(CF_HDROP, files), (format, effect)]
}

/// Lõikelauale need vormingud (vana sisu läheb ära). Lõikelaud suletakse alati.
#[cfg(windows)]
fn set_clipboard(items: &[(u32, Vec<u8>)]) -> Result<(), String> {
    use windows::Win32::Foundation::{GlobalFree, HANDLE};
    use windows::Win32::System::DataExchange::{CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData};
    use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};

    // SAFETY: mälu on parajalt suur, lukustatakse kirjutamise ajaks; pärast
    // `SetClipboardData` kuulub see lõikelauale, ebaõnnestumisel vabastame.
    unsafe {
        OpenClipboard(None).map_err(|e| format!("cannot open the clipboard: {e}"))?;
        let result = (|| {
            EmptyClipboard().map_err(|e| e.to_string())?;
            for (format, bytes) in items {
                let memory = GlobalAlloc(GMEM_MOVEABLE, bytes.len()).map_err(|e| e.to_string())?;
                let ptr = GlobalLock(memory) as *mut u8;
                if ptr.is_null() {
                    let _ = GlobalFree(Some(memory));
                    return Err("cannot lock memory".to_string());
                }
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
                let _ = GlobalUnlock(memory);
                if let Err(e) = SetClipboardData(*format, Some(HANDLE(memory.0))) {
                    let _ = GlobalFree(Some(memory));
                    return Err(e.to_string());
                }
            }
            Ok(())
        })();
        let _ = CloseClipboard();
        result
    }
}

/// Pilt lõikelauale (Ctrl+C): PNG ja DIB (`clip_image`), ja kui `file` on
/// antud, ka fail ise — Explorerisse kleepides tuleb failikoopia.
#[cfg(windows)]
pub fn set_clipboard_image(png: Vec<u8>, dib: Vec<u8>, file: Option<&Path>) -> Result<(), String> {
    use windows::Win32::System::DataExchange::RegisterClipboardFormatW;
    use windows::core::w;
    const CF_DIB: u32 = 8;
    // SAFETY: konstantne nulliga lõppev nimi.
    let png_format = unsafe { RegisterClipboardFormatW(w!("PNG")) };
    let mut items = vec![(png_format, png), (CF_DIB, dib)];
    if let Some(file) = file {
        items.extend(files_clip(&[file.to_path_buf()]));
    }
    set_clipboard(&items)
}

#[cfg(not(windows))]
pub fn set_clipboard_image(_png: Vec<u8>, _dib: Vec<u8>, _file: Option<&Path>) -> Result<(), String> {
    Err("the clipboard is not supported on this platform".into())
}

/// Lõikelaua pilt: PNG, kui on (läbipaistvus alles), muidu DIB.
#[cfg(windows)]
pub fn clipboard_image() -> Option<ClipboardImage> {
    use windows::Win32::Foundation::HGLOBAL;
    use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW};
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    use windows::core::w;
    const CF_DIB: u32 = 8;
    const CF_DIBV5: u32 = 17;
    // SAFETY: andmed kopeeritakse lahtise lõikelaua ajal ega vabastata
    // (need kuuluvad lõikelauale).
    unsafe {
        let png = RegisterClipboardFormatW(w!("PNG"));
        let format = [png, CF_DIB, CF_DIBV5].into_iter().find(|f| IsClipboardFormatAvailable(*f).is_ok())?;
        OpenClipboard(None).ok()?;
        let bytes = (|| {
            let handle = HGLOBAL(GetClipboardData(format).ok()?.0);
            let p = GlobalLock(handle) as *const u8;
            if p.is_null() {
                return None;
            }
            let bytes = std::slice::from_raw_parts(p, GlobalSize(handle)).to_vec();
            let _ = GlobalUnlock(handle);
            Some(bytes)
        })();
        let _ = CloseClipboard();
        let bytes = bytes.filter(|b| !b.is_empty())?;
        Some(if format == png { ClipboardImage::Png(bytes) } else { ClipboardImage::Dib(bytes) })
    }
}

#[cfg(not(windows))]
pub fn clipboard_image() -> Option<ClipboardImage> {
    None
}

/// Lõikelaual olevad failid (Explorerist Ctrl+C).
#[cfg(windows)]
pub fn clipboard_files() -> Option<Vec<PathBuf>> {
    use std::os::windows::ffi::OsStringExt;
    use windows::Win32::System::DataExchange::{CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard};
    use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

    const CF_HDROP: u32 = 15;
    // SAFETY: lõikelaua andmeid loetakse ainult lahtise lõikelaua ajal ja
    // neid ei vabastata (need kuuluvad lõikelauale).
    unsafe {
        IsClipboardFormatAvailable(CF_HDROP).ok()?;
        OpenClipboard(None).ok()?;
        let result = (|| {
            let drop = HDROP(GetClipboardData(CF_HDROP).ok()?.0);
            let count = DragQueryFileW(drop, u32::MAX, None);
            let mut files = Vec::new();
            for i in 0..count {
                let len = DragQueryFileW(drop, i, None) as usize;
                let mut buf = vec![0u16; len + 1];
                DragQueryFileW(drop, i, Some(&mut buf));
                files.push(PathBuf::from(std::ffi::OsString::from_wide(&buf[..len])));
            }
            Some(files)
        })();
        let _ = CloseClipboard();
        result.filter(|files| !files.is_empty())
    }
}

#[cfg(not(windows))]
pub fn clipboard_files() -> Option<Vec<PathBuf>> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Finderi `furl`: protsendid lahti (ka UTF-8 täpitähed), `localhost`
    /// lubatud, kausta lõpukaldkriips ära; muu kui `file:` ei sobi.
    #[test]
    fn file_urls_become_paths() {
        assert_eq!(file_url_bytes("file:///Users/a/My%20Pictures/IMG_0001.jpg").unwrap(), b"/Users/a/My Pictures/IMG_0001.jpg");
        assert_eq!(file_url_bytes("file://localhost/tmp/x.png").unwrap(), b"/tmp/x.png");
        assert_eq!(String::from_utf8(file_url_bytes("file:///Users/a/P%C3%B5ld/").unwrap()).unwrap(), "/Users/a/Põld");
        assert_eq!(file_url_bytes("file:///a/100%25.jpg").unwrap(), b"/a/100%.jpg");
        assert_eq!(file_url_bytes("file:///a/%zz.jpg").unwrap(), b"/a/%zz.jpg");
        assert_eq!(file_url_bytes("file:///a/x%2").unwrap(), b"/a/x%2");
        assert!(file_url_bytes("https://example.com/a.jpg").is_none());
        assert_eq!(file_url_bytes("file:///").unwrap(), b"/");
    }
}
