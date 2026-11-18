//! Windowsi pildidekooder (WIC). JPEG-i pakib see kohe vähendatult lahti
//! (DCT-skaleerimine dekoodri sees), mis ekraani mõõtu fotol on mitu korda
//! kiirem kui täissuuruses lahtipakkimine. Oskab ka vorminguid, mille
//! koodek on Windowsis (HEIC, RAW-d jm).

use std::path::Path;

/// Lahti pakitud pilt: laius, kõrgus, RGB baidid ja EXIF-i pööre (1–8).
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub rgb: Vec<u8>,
    pub orientation: u16,
    /// Algse kaadri mõõt failis (pöördeta).
    pub full: (u32, u32),
}

/// Pakib pildi lahti nii, et pikem külg on kuni `max_side`. `None`: WIC ei
/// oska seda faili (siis proovib kutsuja ise).
#[cfg(windows)]
pub fn decode(path: &Path, max_side: u32) -> Option<Decoded> {
    decode_err(path, max_side, None).ok()
}

/// Lahtipakkimise edenemine: loetud baite (kaugeim loetud koht) ja faili
/// suurus. Dekooder loeb faili meie voo (`FileStream`) kaudu, nii et
/// pildiaken saab riba näidata.
#[derive(Default)]
pub struct Progress {
    pub read: std::sync::atomic::AtomicU64,
    pub total: std::sync::atomic::AtomicU64,
}

/// Fail dekoodrile IStream-ina: loeb puhverdatult (1 MB) ja märgib
/// edenemise. Ainult lugemine; kirjutamine, lukud ja koopia pole toetatud.
#[cfg(windows)]
#[windows_core::implement(windows::Win32::System::Com::IStream)]
struct FileStream {
    inner: std::sync::Mutex<StreamInner>,
    len: u64,
    progress: Option<std::sync::Arc<Progress>>,
}

/// Oma puhver: otsing muudab ainult kohta (dekooder otsib iga rea kohta
/// kolm korda — BufReader tühjendas iga kord puhvri ja luges sama lõigu
/// ~20 korda uuesti); loetakse ainult siis, kui küsitud koht pole puhvris.
#[cfg(windows)]
struct StreamInner {
    file: std::fs::File,
    pos: u64,
    buf: Vec<u8>,
    buf_start: u64,
    buf_len: usize,
}

#[cfg(windows)]
const STREAM_BUF: usize = 4 << 20;

#[cfg(windows)]
impl StreamInner {
    /// Loeb `out` täis alates `pos`-ist (või faili lõpuni); tagastab baite.
    fn read_at(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        use std::io::{Read, Seek, SeekFrom};
        let mut done = 0;
        while done < out.len() {
            let in_buf = self.pos >= self.buf_start && self.pos < self.buf_start + self.buf_len as u64;
            if in_buf {
                let from = (self.pos - self.buf_start) as usize;
                let n = (self.buf_len - from).min(out.len() - done);
                out[done..done + n].copy_from_slice(&self.buf[from..from + n]);
                done += n;
                self.pos += n as u64;
                continue;
            }
            self.file.seek(SeekFrom::Start(self.pos))?;
            if out.len() - done >= STREAM_BUF {
                // Suur tükk otse sihtpuhvrisse.
                let n = self.file.read(&mut out[done..])?;
                if n == 0 {
                    break;
                }
                done += n;
                self.pos += n as u64;
                continue;
            }
            let n = self.file.read(&mut self.buf[..])?;
            if n == 0 {
                break;
            }
            self.buf_start = self.pos;
            self.buf_len = n;
        }
        Ok(done)
    }
}

#[cfg(windows)]
impl windows::Win32::System::Com::ISequentialStream_Impl for FileStream_Impl {
    fn Read(&self, pv: *mut core::ffi::c_void, cb: u32, pcbread: *mut u32) -> windows_core::HRESULT {
        use windows::Win32::Foundation::{E_FAIL, S_FALSE, S_OK};
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        // SAFETY: WIC annab `cb` baiti kirjutatavat puhvrit.
        let buf = unsafe { std::slice::from_raw_parts_mut(pv.cast::<u8>(), cb as usize) };
        let done = match inner.read_at(buf) {
            Ok(n) => n,
            Err(_) => return E_FAIL,
        };
        if !pcbread.is_null() {
            // SAFETY: kehtiv väljundviit.
            unsafe { *pcbread = done as u32 };
        }
        if let Some(p) = &self.progress {
            p.read.fetch_max(inner.pos, std::sync::atomic::Ordering::Relaxed);
        }
        if done < buf.len() { S_FALSE } else { S_OK }
    }

    fn Write(&self, _pv: *const core::ffi::c_void, _cb: u32, _pcbwritten: *mut u32) -> windows_core::HRESULT {
        windows::Win32::Foundation::E_NOTIMPL
    }
}

#[cfg(windows)]
impl windows::Win32::System::Com::IStream_Impl for FileStream_Impl {
    fn Seek(&self, dlibmove: i64, dworigin: windows::Win32::System::Com::STREAM_SEEK, plibnewposition: *mut u64) -> windows_core::Result<()> {
        use windows::Win32::System::Com::{STREAM_SEEK_CUR, STREAM_SEEK_END, STREAM_SEEK_SET};
        let mut inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let base = match dworigin {
            STREAM_SEEK_SET => 0i64,
            STREAM_SEEK_CUR => inner.pos as i64,
            STREAM_SEEK_END => self.len as i64,
            _ => return Err(windows::Win32::Foundation::E_INVALIDARG.into()),
        };
        let target = base.saturating_add(dlibmove).max(0) as u64;
        inner.pos = target;
        if !plibnewposition.is_null() {
            // SAFETY: kehtiv väljundviit.
            unsafe { *plibnewposition = target };
        }
        Ok(())
    }

    fn SetSize(&self, _libnewsize: u64) -> windows_core::Result<()> {
        Err(windows::Win32::Foundation::E_NOTIMPL.into())
    }

    fn CopyTo(&self, _pstm: windows_core::Ref<windows::Win32::System::Com::IStream>, _cb: u64, _pcbread: *mut u64, _pcbwritten: *mut u64) -> windows_core::Result<()> {
        Err(windows::Win32::Foundation::E_NOTIMPL.into())
    }

    fn Commit(&self, _grfcommitflags: &windows::Win32::System::Com::STGC) -> windows_core::Result<()> {
        Ok(())
    }

    fn Revert(&self) -> windows_core::Result<()> {
        Ok(())
    }

    fn LockRegion(&self, _liboffset: u64, _cb: u64, _dwlocktype: &windows::Win32::System::Com::LOCKTYPE) -> windows_core::Result<()> {
        Err(windows::Win32::Foundation::E_NOTIMPL.into())
    }

    fn UnlockRegion(&self, _liboffset: u64, _cb: u64, _dwlocktype: u32) -> windows_core::Result<()> {
        Err(windows::Win32::Foundation::E_NOTIMPL.into())
    }

    fn Stat(&self, pstatstg: *mut windows::Win32::System::Com::STATSTG, _grfstatflag: &windows::Win32::System::Com::STATFLAG) -> windows_core::Result<()> {
        use windows::Win32::System::Com::{STATSTG, STGM_READ, STGTY_STREAM};
        if pstatstg.is_null() {
            return Err(windows::Win32::Foundation::E_POINTER.into());
        }
        // SAFETY: kehtiv väljundviit; nime ei anta (STATFLAG_NONAME).
        unsafe {
            *pstatstg = STATSTG { r#type: STGTY_STREAM.0 as u32, cbSize: self.len, grfMode: STGM_READ, ..Default::default() };
        }
        Ok(())
    }

    fn Clone(&self) -> windows_core::Result<windows::Win32::System::Com::IStream> {
        Err(windows::Win32::Foundation::E_NOTIMPL.into())
    }
}

/// Nagu `decode`; vea korral Windowsi veateade (nt koodek puudub).
/// `progress`: edenemine riba jaoks (fail loetakse oma voo kaudu).
#[cfg(windows)]
pub fn decode_err(path: &Path, max_side: u32, progress: Option<std::sync::Arc<Progress>>) -> Result<Decoded, String> {
    use windows::core::Interface;
    use windows::Win32::Graphics::Imaging::{CLSID_WICImagingFactory, IWICBitmapSource, IWICImagingFactory, WICDecodeMetadataCacheOnDemand};
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, IStream, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};

    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let len = file.metadata().map_err(|e| e.to_string())?.len();
    if let Some(p) = &progress {
        // TIFF: riba tervik on pildiandmete maht, mitte fail (vt `raw`).
        let total = crate::raw::tiff_image_bytes(path).filter(|b| *b <= len).unwrap_or(len);
        p.total.store(total, std::sync::atomic::Ordering::Relaxed);
    }
    let stream: IStream = FileStream { inner: std::sync::Mutex::new(StreamInner { file, pos: 0, buf: vec![0; STREAM_BUF], buf_start: 0, buf_len: 0 }), len, progress }.into();
    // SAFETY: COM selles (taustal olevas) lõimes; objektid vabastavad end ise,
    // puhver on täpselt `stride * height` baiti.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let factory: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).map_err(|e| e.message())?;
        let decoder = factory.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand).map_err(|e| e.message())?;
        let frame = decoder.GetFrame(0).map_err(|e| e.message())?;
        let (mut w, mut h) = (0u32, 0u32);
        frame.GetSize(&mut w, &mut h).map_err(|e| e.message())?;
        if w == 0 || h == 0 {
            return Err("0 × 0".to_string());
        }
        let orientation = orientation(&frame).unwrap_or(1);
        let source: IWICBitmapSource = frame.cast().map_err(|e| e.message())?;
        let (width, height, rgb) = scaled(&factory, &source, w, h, max_side).ok_or_else(|| "WIC scaler".to_string())?;
        Ok(Decoded { width, height, rgb, orientation, full: (w, h) })
    }
}

#[cfg(not(windows))]
pub fn decode_err(_path: &Path, _max_side: u32, _progress: Option<std::sync::Arc<Progress>>) -> Result<Decoded, String> {
    Err("no Windows decoder".to_string())
}

/// Pilt mälus olevast failist (nt CR3 sees olev kaamera JPEG), muidu nagu
/// `decode`. Pööre EXIF-ist, kui on; CR3 JPEG-il seda pole (1).
#[cfg(windows)]
pub fn decode_bytes(data: &[u8], max_side: u32) -> Option<Decoded> {
    use windows::core::Interface;
    use windows::Win32::Graphics::Imaging::{CLSID_WICImagingFactory, IWICImagingFactory, WICDecodeMetadataCacheOnDemand};
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};

    // SAFETY: nagu `decode`; mäluvoog viitab `data`-le ainult selle kutse ajal.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let factory: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let stream = factory.CreateStream().ok()?;
        stream.InitializeFromMemory(data).ok()?;
        let decoder = factory.CreateDecoderFromStream(&stream, std::ptr::null(), WICDecodeMetadataCacheOnDemand).ok()?;
        let frame = decoder.GetFrame(0).ok()?;
        let (mut w, mut h) = (0u32, 0u32);
        frame.GetSize(&mut w, &mut h).ok()?;
        if w == 0 || h == 0 {
            return None;
        }
        let orientation = orientation(&frame).unwrap_or(1);
        let (width, height, rgb) = scaled(&factory, &frame.cast().ok()?, w, h, max_side)?;
        Some(Decoded { width, height, rgb, orientation, full: (w, h) })
    }
}

#[cfg(not(windows))]
pub fn decode_bytes(_data: &[u8], _max_side: u32) -> Option<Decoded> {
    None
}

/// Allikas (`w`×`h`) vähendatult RGB-ks: pikem külg kuni `max_side`.
/// Vähendaja kasutab JPEG-i dekoodri enda vähendamist (IWICBitmapSourceTransform).
#[cfg(windows)]
unsafe fn scaled(
    factory: &windows::Win32::Graphics::Imaging::IWICImagingFactory,
    source: &windows::Win32::Graphics::Imaging::IWICBitmapSource,
    w: u32,
    h: u32,
    max_side: u32,
) -> Option<(u32, u32, Vec<u8>)> {
    use windows::Win32::Graphics::Imaging::{
        GUID_WICPixelFormat24bppRGB, WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant, WICBitmapPaletteTypeCustom,
    };
    // SAFETY: COM objektid vabastavad end ise; puhver on täpselt `stride * height` baiti.
    unsafe {
        let scale = (max_side as f64 / w.max(h) as f64).min(1.0);
        let (dw, dh) = (((w as f64 * scale).round() as u32).max(1), ((h as f64 * scale).round() as u32).max(1));
        let scaler = factory.CreateBitmapScaler().ok()?;
        scaler.Initialize(source, dw, dh, WICBitmapInterpolationModeFant).ok()?;
        let converter = factory.CreateFormatConverter().ok()?;
        converter
            .Initialize(&scaler, &GUID_WICPixelFormat24bppRGB, WICBitmapDitherTypeNone, None, 0.0, WICBitmapPaletteTypeCustom)
            .ok()?;
        let stride = (dw * 3).div_ceil(4) * 4;
        let mut buffer = vec![0u8; (stride * dh) as usize];
        converter.CopyPixels(std::ptr::null(), stride, &mut buffer).ok()?;
        let rgb = if stride == dw * 3 {
            buffer
        } else {
            buffer.chunks_exact(stride as usize).flat_map(|row| &row[..(dw * 3) as usize]).copied().collect()
        };
        Some((dw, dh, rgb))
    }
}

#[cfg(not(windows))]
pub fn decode(_path: &Path, _max_side: u32) -> Option<Decoded> {
    None
}

/// Pildi mõõtmed (laius, kõrgus) nagu pilti vaadates — EXIF-i pööre
/// arvestatud. Loeb ainult päist, pilti lahti ei paki.
#[cfg(windows)]
pub fn dimensions(path: &Path) -> Option<(u32, u32)> {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::GENERIC_READ;
    use windows::Win32::Graphics::Imaging::{CLSID_WICImagingFactory, IWICImagingFactory, WICDecodeMetadataCacheOnDemand};
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED};
    // SAFETY: COM selles (taustal olevas) lõimes; objektid vabastavad end ise.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let factory: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let decoder = factory
            .CreateDecoderFromFilename(&HSTRING::from(path.as_os_str()), None, GENERIC_READ, WICDecodeMetadataCacheOnDemand)
            .ok()?;
        let frame = decoder.GetFrame(0).ok()?;
        let (mut w, mut h) = (0u32, 0u32);
        frame.GetSize(&mut w, &mut h).ok()?;
        if w == 0 || h == 0 {
            return None;
        }
        // Pööre 5–8: külili, laius ja kõrgus vahetuvad.
        Some(if orientation(&frame).is_some_and(|o| o >= 5) { (h, w) } else { (w, h) })
    }
}

#[cfg(not(windows))]
pub fn dimensions(_path: &Path) -> Option<(u32, u32)> {
    None
}

/// EXIF-i pööre (JPEG: APP1 → IFD0 → 274; TIFF/RAW: IFD0 → 274).
#[cfg(windows)]
unsafe fn orientation(frame: &windows::Win32::Graphics::Imaging::IWICBitmapFrameDecode) -> Option<u16> {
    use windows::core::w;
    use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PROPVARIANT};
    // SAFETY: PROPVARIANT puhastatakse; loeme ainult VT_UI2 (18) väärtust.
    unsafe {
        let reader = frame.GetMetadataQueryReader().ok()?;
        for query in [w!("/app1/ifd/{ushort=274}"), w!("/ifd/{ushort=274}")] {
            let mut value = PROPVARIANT::default();
            if reader.GetMetadataByName(query, &mut value).is_ok() {
                let inner = &value.Anonymous.Anonymous;
                let found = (inner.vt.0 == 18).then(|| inner.Anonymous.uiVal);
                let _ = PropVariantClear(&mut value);
                if found.is_some() {
                    return found;
                }
            }
        }
        None
    }
}
