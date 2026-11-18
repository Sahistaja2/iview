//! macOS: Finderist avatud failid (topeltklõps, „Open With", `open -a
//! iview fail`, faili lohistamine Docki ikoonile).
//!
//! Finder ei anna faili käsurea argumendina, vaid saadab Apple Eventi
//! „open documents" (`aevt`/`odoc`). winit-i rakenduse delegaat seda ei
//! käsitle, seepärast paigaldame oma käsitleja. Kui iview juba töötab,
//! saadab macOS sündmuse samale protsessile — „üks aken" tuleb siit.
//!
//! Ajastus: AppKit paigaldab oma käsitlejad käivitumise lõpus
//! (`finishLaunching`) ja käivitusfaili sündmus saabub kohe pärast seda,
//! enne `applicationDidFinishLaunching`-ut (enne eframe'i akent). Apple
//! soovitab oma käsitleja paigaldada `applicationWillFinishLaunching` ajal.
//! Paigaldame kolm korda — viimane võidab ja ükski ei sega:
//! 1. kohe `main`-is (kui AppKit oma enam üle ei kirjuta);
//! 2. teate `NSApplicationWillFinishLaunchingNotification` ajal (Apple'i
//!    soovitatud koht; CoreFoundationi kohalik teadetekeskus on sama mis
//!    `NSNotificationCenter.defaultCenter`);
//! 3. akna loomisel — hiljemalt sealt jõuavad kohale järgmised avamised.
//!
//! Käivitusfail: tee läheb järjekorda (`platform::push_incoming`, aken võtab
//! selle esimesel kaadril) ja laadija hakkab seda kohe lahti pakkima,
//! samal ajal kui aken ja graafika valmivad (`set_early`).

use std::ffi::{c_char, c_long, c_void};
use std::os::unix::ffi::OsStringExt;
use std::path::PathBuf;
use std::sync::Mutex;

#[repr(C)]
struct AEDesc {
    descriptor_type: u32,
    data_handle: *mut c_void,
}

type OSErr = i16;
type AEEventHandler = extern "C" fn(event: *const AEDesc, reply: *mut AEDesc, refcon: *mut c_void) -> OSErr;
type CFNotificationCallback = extern "C" fn(center: *mut c_void, observer: *mut c_void, name: *const c_void, object: *const c_void, user_info: *const c_void);

#[link(name = "CoreServices", kind = "framework")]
unsafe extern "C" {
    fn AEInstallEventHandler(class: u32, id: u32, handler: AEEventHandler, refcon: *mut c_void, is_sys_handler: u8) -> OSErr;
    fn AEGetParamDesc(event: *const AEDesc, keyword: u32, desired_type: u32, result: *mut AEDesc) -> OSErr;
    fn AECountItems(list: *const AEDesc, count: *mut c_long) -> OSErr;
    fn AEGetNthPtr(list: *const AEDesc, index: c_long, desired_type: u32, keyword: *mut u32, type_code: *mut u32, data: *mut c_void, maximum: c_long, actual: *mut c_long) -> OSErr;
    fn AEDisposeDesc(desc: *mut AEDesc) -> OSErr;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFNotificationCenterGetLocalCenter() -> *mut c_void;
    fn CFNotificationCenterAddObserver(center: *mut c_void, observer: *const c_void, callback: CFNotificationCallback, name: *const c_void, object: *const c_void, suspension: isize);
    fn CFStringCreateWithCString(alloc: *const c_void, text: *const c_char, encoding: u32) -> *const c_void;
}

const fn code(c: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*c)
}
const CORE_EVENT_CLASS: u32 = code(b"aevt");
const OPEN_DOCUMENTS: u32 = code(b"odoc");
const DIRECT_OBJECT: u32 = code(b"----");
const TYPE_LIST: u32 = code(b"list");
const TYPE_FILE_URL: u32 = code(b"furl");
const UTF8: u32 = 0x0800_0100;
/// `CFNotificationSuspensionBehaviorDeliverImmediately`.
const DELIVER_IMMEDIATELY: isize = 4;

/// Käivitusfaili kohene lahtipakkimine (laadija ja esimese pildi mõõt).
type Early = Box<dyn Fn(PathBuf) + Send>;
static EARLY: Mutex<Option<Early>> = Mutex::new(None);

/// `main`: kuhu esimene fail kohe lahti pakkida. Akna loomisel `None`
/// (edasi tegeleb aken ise).
pub fn set_early(early: Option<Early>) {
    *EARLY.lock().unwrap_or_else(|e| e.into_inner()) = early;
}

/// Käsitleja paika (vt mooduli kirjeldust: kutsutakse mitu korda).
pub fn install() {
    // SAFETY: funktsiooniviit elab programmi lõpuni; refcon puudub.
    unsafe {
        AEInstallEventHandler(CORE_EVENT_CLASS, OPEN_DOCUMENTS, open_documents, std::ptr::null_mut(), 0);
    }
}

/// `main`: käsitleja kohe ja uuesti AppKit'i `WillFinishLaunching` teate ajal.
pub fn install_early() {
    install();
    // SAFETY: CFString jääb alles (teade viitab sellele); vaatleja on
    // staatiline aadress, mis elab programmi lõpuni.
    unsafe {
        static OBSERVER: u8 = 0;
        let name = CFStringCreateWithCString(std::ptr::null(), c"NSApplicationWillFinishLaunchingNotification".as_ptr(), UTF8);
        if !name.is_null() {
            CFNotificationCenterAddObserver(CFNotificationCenterGetLocalCenter(), std::ptr::addr_of!(OBSERVER).cast(), will_finish_launching, name, std::ptr::null(), DELIVER_IMMEDIATELY);
        }
    }
}

extern "C" fn will_finish_launching(_center: *mut c_void, _observer: *mut c_void, _name: *const c_void, _object: *const c_void, _info: *const c_void) {
    install();
    crate::startup::mark("macOS: open-documents handler installed (will finish launching)");
}

/// „Open documents": iga fail järjekorda; esimene ka kohe lahti pakkimisse.
extern "C" fn open_documents(event: *const AEDesc, _reply: *mut AEDesc, _refcon: *mut c_void) -> OSErr {
    let paths = std::panic::catch_unwind(|| files_of(event)).unwrap_or_default();
    crate::startup::mark(format!("macOS: open documents ({} files)", paths.len()));
    if let Some(first) = paths.first()
        && let Some(early) = EARLY.lock().unwrap_or_else(|e| e.into_inner()).as_ref()
    {
        early(first.clone());
    }
    // Mitu faili korraga: aken näitab viimast (nagu teise käivituse puhul).
    for path in paths {
        crate::platform::push_incoming(path);
    }
    0
}

/// Sündmuse failid (`furl` loendist).
fn files_of(event: *const AEDesc) -> Vec<PathBuf> {
    let mut out = Vec::new();
    // SAFETY: `event` on AppKit'i antud kehtiv sündmus kogu kutse ajal;
    // loend vabastatakse; puhver on piisav ja tegelik pikkus kontrollitakse.
    unsafe {
        let mut list = AEDesc { descriptor_type: 0, data_handle: std::ptr::null_mut() };
        if AEGetParamDesc(event, DIRECT_OBJECT, TYPE_LIST, &mut list) != 0 {
            return out;
        }
        let mut count: c_long = 0;
        if AECountItems(&list, &mut count) == 0 {
            for i in 1..=count {
                let mut buf = vec![0u8; 4096];
                let (mut keyword, mut kind, mut actual) = (0u32, 0u32, 0 as c_long);
                let err = AEGetNthPtr(&list, i, TYPE_FILE_URL, &mut keyword, &mut kind, buf.as_mut_ptr().cast(), buf.len() as c_long, &mut actual);
                if err != 0 || actual <= 0 || actual as usize > buf.len() {
                    continue;
                }
                buf.truncate(actual as usize);
                let url = String::from_utf8_lossy(&buf).into_owned();
                if let Some(bytes) = crate::platform::file_url_bytes(&url) {
                    out.push(PathBuf::from(std::ffi::OsString::from_vec(bytes)));
                }
            }
        }
        AEDisposeDesc(&mut list);
    }
    out
}
