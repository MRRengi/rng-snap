//! Video side of the recorder: capture, constant-rate clock, GPU scale and
//! the Media Foundation sink writer (which also receives the audio chunks).

use super::audio::{self, Chunk, RATE};
use super::{Control, Params, Summary, log};
use crate::geom::Rect;
use crate::image::Frame;
use crate::videoplan::Codec;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use windows::Foundation::TimeSpan;
use windows::Graphics::Capture::{Direct3D11CaptureFramePool, GraphicsCaptureItem, GraphicsCaptureSession};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Threading::*;
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;
use windows::core::{HSTRING, Interface};

const RING: usize = 8;

fn w<T>(r: windows::core::Result<T>, what: &str) -> Result<T, String> {
    r.map_err(|e| format!("{what}: {e}"))
}

/// Refresh rate of a monitor in Hz (60 if unknown).
pub fn refresh_rate(monitor: isize) -> u32 {
    unsafe {
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if !GetMonitorInfoW(HMONITOR(monitor as *mut _), &mut info as *mut _ as *mut MONITORINFO).as_bool() {
            return 60;
        }
        let mut dm = DEVMODEW { dmSize: size_of::<DEVMODEW>() as u16, ..Default::default() };
        if EnumDisplaySettingsW(windows::core::PCWSTR(info.szDevice.as_ptr()), ENUM_CURRENT_SETTINGS, &mut dm).as_bool() && dm.dmDisplayFrequency > 1 {
            dm.dmDisplayFrequency
        } else {
            60
        }
    }
}

/// True when a hardware HEVC encoder is present.
pub fn hevc_available() -> bool {
    unsafe {
        let _ = MFStartup(MF_VERSION, MFSTARTUP_LITE);
        let info = MFT_REGISTER_TYPE_INFO { guidMajorType: MFMediaType_Video, guidSubtype: MFVideoFormat_HEVC };
        let mut pp: *mut Option<IMFActivate> = std::ptr::null_mut();
        let mut n = 0u32;
        let ok = MFTEnumEx(MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER, None, Some(&info), &mut pp, &mut n).is_ok();
        if !pp.is_null() {
            for i in 0..n as usize {
                std::ptr::drop_in_place(pp.add(i));
            }
            CoTaskMemFree(Some(pp as *const _));
        }
        ok && n > 0
    }
}

fn texture(dev: &ID3D11Device, tw: u32, h: u32, bind: D3D11_BIND_FLAG, usage: D3D11_USAGE, cpu: u32) -> Result<ID3D11Texture2D, String> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: tw,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: usage,
        BindFlags: bind.0 as u32,
        CPUAccessFlags: cpu,
        MiscFlags: 0,
    };
    let mut t = None;
    unsafe { w(dev.CreateTexture2D(&desc, None, Some(&mut t)), "texture")? };
    t.ok_or_else(|| "texture".into())
}

fn device() -> Result<ID3D11Device, String> {
    let flags = D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT;
    for driver in [D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP] {
        let mut dev = None;
        let ok = unsafe { D3D11CreateDevice(None, driver, HMODULE::default(), flags, None, D3D11_SDK_VERSION, Some(&mut dev), None, None) };
        if ok.is_ok() {
            if let Some(d) = dev {
                return Ok(d);
            }
        }
    }
    Err("no Direct3D 11 device".into())
}

fn video_type(subtype: &windows::core::GUID, w_: u32, h: u32, fps: u32) -> Result<IMFMediaType, String> {
    unsafe {
        let t = w(MFCreateMediaType(), "media type")?;
        w(t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video), "major")?;
        w(t.SetGUID(&MF_MT_SUBTYPE, subtype), "subtype")?;
        w(t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32), "interlace")?;
        w(t.SetUINT64(&MF_MT_FRAME_SIZE, (w_ as u64) << 32 | h as u64), "size")?;
        w(t.SetUINT64(&MF_MT_FRAME_RATE, (fps as u64) << 32 | 1), "rate")?;
        w(t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, 1u64 << 32 | 1), "par")?;
        Ok(t)
    }
}

fn audio_type(aac: bool) -> Result<IMFMediaType, String> {
    unsafe {
        let t = w(MFCreateMediaType(), "media type")?;
        w(t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio), "major")?;
        w(t.SetGUID(&MF_MT_SUBTYPE, if aac { &MFAudioFormat_AAC } else { &MFAudioFormat_PCM }), "subtype")?;
        w(t.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16), "bits")?;
        w(t.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, RATE), "rate")?;
        w(t.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2), "channels")?;
        if aac {
            w(t.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 24_000), "aac bitrate")?;
        } else {
            w(t.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 4), "align")?;
            w(t.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, RATE * 4), "bytes")?;
        }
        Ok(t)
    }
}

pub fn run(p: Params, ctrl: Arc<Control>, ready: mpsc::Sender<Result<(), String>>) -> Result<Summary, String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _ = MFStartup(MF_VERSION, 0);
    }
    let r = record(&p, &ctrl, &ready);
    if let Err(e) = &r {
        log(&format!("recording failed: {e}"));
        let _ = ready.send(Err(e.clone()));
        let _ = std::fs::remove_file(&p.path);
    }
    unsafe {
        let _ = MFShutdown();
    }
    r
}

fn record(p: &Params, ctrl: &Arc<Control>, ready: &mpsc::Sender<Result<(), String>>) -> Result<Summary, String> {
    let plan = p.plan;
    let (sw, sh) = (p.region.width() as u32, p.region.height() as u32);
    let (ow, oh) = (plan.out_w, plan.out_h);
    let fps = plan.fps as i64;

    // Audio first: we only add an audio stream if a device actually opened.
    let (chunk_tx, chunk_rx) = mpsc::channel::<Chunk>();
    let mut audio_thread = None;
    let mut has_audio = false;
    if p.system_audio || p.mic {
        let (atx, arx) = mpsc::channel::<bool>();
        let (sys, mic, c) = (p.system_audio, p.mic, ctrl.clone());
        audio_thread = Some(std::thread::spawn(move || audio::run(sys, mic, c, atx, chunk_tx)));
        has_audio = arx.recv_timeout(Duration::from_secs(3)).unwrap_or(false);
    } else {
        drop(chunk_tx);
    }

    let dev = device()?;
    let ctx = unsafe { w(dev.GetImmediateContext(), "context")? };
    if let Ok(mt) = dev.cast::<ID3D11Multithread>() {
        unsafe {
            let _ = mt.SetMultithreadProtected(true);
        }
    }
    let dxgi: IDXGIDevice = w(dev.cast(), "dxgi device")?;
    let winrt_dev: IDirect3DDevice = unsafe { w(CreateDirect3D11DeviceFromDXGIDevice(&dxgi), "winrt device")? }.cast().map_err(|e| e.to_string())?;

    // Capture the monitor.
    let interop = w(windows::core::factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>(), "capture interop")?;
    let item: GraphicsCaptureItem = unsafe { w(interop.CreateForMonitor(HMONITOR(p.monitor as *mut _)), "capture item")? };
    let size = w(item.Size(), "item size")?;
    let pool = w(Direct3D11CaptureFramePool::CreateFreeThreaded(&winrt_dev, DirectXPixelFormat::B8G8R8A8UIntNormalized, 3, size), "frame pool")?;
    let session: GraphicsCaptureSession = w(pool.CreateCaptureSession(&item), "session")?;
    let _ = session.SetIsCursorCaptureEnabled(true);
    // Windows 11 draws a yellow frame around captured screens unless the app
    // asked for borderless capture first (granted to desktop apps).
    if let Ok(op) = windows::Graphics::Capture::GraphicsCaptureAccess::RequestAccessAsync(windows::Graphics::Capture::GraphicsCaptureAccessKind::Borderless) {
        let _ = op.join();
    }
    if let Err(e) = session.SetIsBorderRequired(false) {
        log(&format!("border stays: {e}"));
    }
    let _ = session.SetMinUpdateInterval(TimeSpan { Duration: 10_000 });
    w(session.StartCapture(), "start capture")?;

    // Region inside the captured texture.
    let lx = (p.region.left - p.monitor_rect.left).max(0) as u32;
    let ly = (p.region.top - p.monitor_rect.top).max(0) as u32;
    let src_box = D3D11_BOX {
        left: lx,
        top: ly,
        front: 0,
        right: (lx + sw).min(size.Width as u32),
        bottom: (ly + sh).min(size.Height as u32),
        back: 1,
    };
    let bind = D3D11_BIND_FLAG(D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0);
    let latest = texture(&dev, sw, sh, bind, D3D11_USAGE_DEFAULT, 0)?;
    let ring: Vec<ID3D11Texture2D> = (0..RING).map(|_| texture(&dev, ow, oh, bind, D3D11_USAGE_DEFAULT, 0)).collect::<Result<_, _>>()?;

    // GPU scaler (only when the output is smaller than the region).
    let scaling = (ow, oh) != (sw, sh);
    let mut scaler = None;
    if scaling {
        unsafe {
            let f: ID2D1Factory1 = w(D2D1CreateFactory(D2D1_FACTORY_TYPE_MULTI_THREADED, None), "d2d factory")?;
            let d2dev = w(f.CreateDevice(&dxgi), "d2d device")?;
            let dc = w(d2dev.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE), "d2d context")?;
            let props = |opts: D2D1_BITMAP_OPTIONS| D2D1_BITMAP_PROPERTIES1 {
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_IGNORE },
                dpiX: 96.0,
                dpiY: 96.0,
                bitmapOptions: opts,
                colorContext: std::mem::ManuallyDrop::new(None),
            };
            let src_surface: IDXGISurface = w(latest.cast(), "surface")?;
            let src = w(dc.CreateBitmapFromDxgiSurface(&src_surface, Some(&props(D2D1_BITMAP_OPTIONS_NONE))), "src bitmap")?;
            let mut targets = Vec::new();
            for t in &ring {
                let s: IDXGISurface = w(t.cast(), "surface")?;
                targets.push(w(dc.CreateBitmapFromDxgiSurface(&s, Some(&props(D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW))), "target bitmap")?);
            }
            scaler = Some((dc, src, targets));
        }
    }

    // Sink writer.
    let mut token = 0u32;
    let mut mgr = None;
    unsafe { w(MFCreateDXGIDeviceManager(&mut token, &mut mgr), "device manager")? };
    let mgr = mgr.ok_or("device manager")?;
    unsafe { w(mgr.ResetDevice(&dev, token), "reset device")? };
    let writer = unsafe {
        let mut attrs = None;
        w(MFCreateAttributes(&mut attrs, 4), "attributes")?;
        let attrs = attrs.ok_or("attributes")?;
        w(attrs.SetUINT32(&MF_READWRITE_ENABLE_HARDWARE_TRANSFORMS, 1), "hw")?;
        w(attrs.SetUnknown(&MF_SINK_WRITER_D3D_MANAGER, &mgr), "d3d manager")?;
        w(attrs.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MPEG4), "container")?;
        let _ = std::fs::create_dir_all(p.path.parent().unwrap_or(std::path::Path::new(".")));
        w(MFCreateSinkWriterFromURL(&HSTRING::from(p.path.as_os_str()), None, &attrs), "sink writer")?
    };
    let subtype = if plan.codec == Codec::Hevc { MFVideoFormat_HEVC } else { MFVideoFormat_H264 };
    let out_t = video_type(&subtype, ow, oh, plan.fps)?;
    unsafe {
        w(out_t.SetUINT32(&MF_MT_AVG_BITRATE, plan.bitrate), "bitrate")?;
        if plan.codec == Codec::H264 {
            let _ = out_t.SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32);
        }
    }
    let vs = unsafe { w(writer.AddStream(&out_t), "add video stream")? };
    let in_t = video_type(&MFVideoFormat_RGB32, ow, oh, plan.fps)?;
    unsafe {
        let _ = in_t.SetUINT32(&MF_MT_DEFAULT_STRIDE, ow * 4);
        let _ = in_t.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1);
        w(writer.SetInputMediaType(vs, &in_t, None), "video input type")?;
    }
    let mut astream = None;
    if has_audio {
        unsafe {
            match writer.AddStream(&audio_type(true)?) {
                Ok(i) => match writer.SetInputMediaType(i, &audio_type(false)?, None) {
                    Ok(()) => astream = Some(i),
                    Err(e) => log(&format!("audio input type: {e}")),
                },
                Err(e) => log(&format!("audio stream: {e}")),
            }
        }
    }
    unsafe { w(writer.BeginWriting(), "begin writing")? };

    // First frame (WGC sends one right away; give it a second).
    let grab = |pool: &Direct3D11CaptureFramePool| -> bool {
        let mut got = None;
        while let Ok(f) = pool.TryGetNextFrame() {
            got = Some(f);
        }
        let Some(f) = got else { return false };
        let ok = (|| -> windows::core::Result<()> {
            let access: IDirect3DDxgiInterfaceAccess = f.Surface()?.cast()?;
            let tex: ID3D11Texture2D = unsafe { access.GetInterface()? };
            unsafe { ctx.CopySubresourceRegion(&latest, 0, 0, 0, 0, &tex, 0, Some(&src_box)) };
            Ok(())
        })()
        .is_ok();
        let _ = f.Close();
        ok
    };
    let t0 = Instant::now();
    while !grab(&pool) && t0.elapsed() < Duration::from_secs(1) {
        std::thread::sleep(Duration::from_millis(2));
    }
    let thumb = read_back(&dev, &ctx, &latest, sw, sh);

    let start = audio::now_100ns();
    ctrl.start_100ns.store(start, Ordering::Release);
    let _ = ready.send(Ok(()));

    let timer = unsafe { CreateWaitableTimerExW(None, None, CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, TIMER_ALL_ACCESS.0) }.ok();
    let mut last_idx: i64 = -1;
    let mut frames = 0u64;
    let mut slot = 0usize;
    let mut fail = None;
    let write_audio = |c: Chunk| {
        if let Some(i) = astream {
            if let Err(e) = write_pcm(&writer, i, c) {
                log(&format!("audio write: {e}"));
            }
        }
    };
    loop {
        if ctrl.stop.load(Ordering::Acquire) {
            break;
        }
        grab(&pool);
        while let Ok(c) = chunk_rx.try_recv() {
            write_audio(c);
        }
        let paused = ctrl.paused.load(Ordering::Acquire);
        let paused_total = ctrl.paused_total_100ns.load(Ordering::Acquire);
        let now = audio::now_100ns();
        if !paused {
            let el = now - start - paused_total;
            let idx = (el as i128 * fps as i128 / 10_000_000) as i64;
            if idx > last_idx {
                let target = &ring[slot];
                match &scaler {
                    Some((dc, src, targets)) => unsafe {
                        dc.SetTarget(&targets[slot]);
                        dc.BeginDraw();
                        dc.DrawBitmap(src, Some(&D2D_RECT_F { left: 0.0, top: 0.0, right: ow as f32, bottom: oh as f32 }), 1.0, D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, None, None);
                        let _ = dc.EndDraw(None, None);
                    },
                    None => unsafe { ctx.CopyResource(target, &latest) },
                }
                let t = (idx as i128 * 10_000_000 / fps as i128) as i64;
                let d = ((idx + 1) as i128 * 10_000_000 / fps as i128) as i64 - t;
                if let Err(e) = write_frame(&writer, vs, target, t, d) {
                    fail = Some(e);
                    break;
                }
                slot = (slot + 1) % RING;
                last_idx = idx;
                frames += 1;
            }
            let next = start + paused_total + ((last_idx + 1) as i128 * 10_000_000 / fps as i128) as i64;
            let due = next - audio::now_100ns();
            if due > 0 {
                match &timer {
                    Some(t) => unsafe {
                        let rel = -due;
                        let _ = SetWaitableTimer(*t, &rel, 0, None, None, false);
                        WaitForSingleObject(*t, 100);
                    },
                    None => std::thread::sleep(Duration::from_nanos(due as u64 * 100)),
                }
            }
        } else {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
    let _ = session.Close();
    let _ = pool.Close();
    if let Some(t) = audio_thread.take() {
        let _ = t.join();
    }
    while let Ok(c) = chunk_rx.try_recv() {
        write_audio(c);
    }
    if let Some(t) = timer {
        unsafe {
            let _ = CloseHandle(t);
        }
    }
    let fin = unsafe { writer.Finalize() };
    drop(scaler);
    if let Some(e) = fail {
        return Err(e);
    }
    w(fin, "finalize")?;
    if ctrl.discard.load(Ordering::Acquire) {
        let _ = std::fs::remove_file(&p.path);
    }
    let seconds = (last_idx + 1).max(0) as f64 / fps as f64;
    Ok(Summary { path: p.path.clone(), seconds, frames, plan, thumb, audio: astream.is_some() })
}

fn write_frame(writer: &IMFSinkWriter, stream: u32, tex: &ID3D11Texture2D, t: i64, d: i64) -> Result<(), String> {
    unsafe {
        let buf = w(MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, tex, 0, false), "surface buffer")?;
        if let Ok(b2) = buf.cast::<IMF2DBuffer>() {
            if let Ok(len) = b2.GetContiguousLength() {
                let _ = buf.SetCurrentLength(len);
            }
        }
        let s = w(MFCreateSample(), "sample")?;
        w(s.AddBuffer(&buf), "add buffer")?;
        w(s.SetSampleTime(t), "time")?;
        w(s.SetSampleDuration(d), "duration")?;
        w(writer.WriteSample(stream, &s), "write video")
    }
}

fn write_pcm(writer: &IMFSinkWriter, stream: u32, c: Chunk) -> Result<(), String> {
    let bytes = c.pcm.len() * 2;
    if bytes == 0 {
        return Ok(());
    }
    unsafe {
        let buf = w(MFCreateMemoryBuffer(bytes as u32), "audio buffer")?;
        let mut ptr = std::ptr::null_mut();
        w(buf.Lock(&mut ptr, None, None), "lock")?;
        std::ptr::copy_nonoverlapping(c.pcm.as_ptr() as *const u8, ptr, bytes);
        let _ = buf.Unlock();
        w(buf.SetCurrentLength(bytes as u32), "length")?;
        let s = w(MFCreateSample(), "sample")?;
        w(s.AddBuffer(&buf), "add")?;
        let frames = (c.pcm.len() / 2) as i64;
        w(s.SetSampleTime(c.pos as i64 * 10_000_000 / RATE as i64), "time")?;
        w(s.SetSampleDuration(frames * 10_000_000 / RATE as i64), "duration")?;
        w(writer.WriteSample(stream, &s), "write audio")
    }
}

fn read_back(dev: &ID3D11Device, ctx: &ID3D11DeviceContext, tex: &ID3D11Texture2D, w_: u32, h: u32) -> Option<Frame> {
    let staging = texture(dev, w_, h, D3D11_BIND_FLAG(0), D3D11_USAGE_STAGING, D3D11_CPU_ACCESS_READ.0 as u32).ok()?;
    unsafe {
        ctx.CopyResource(&staging, tex);
        let mut m = D3D11_MAPPED_SUBRESOURCE::default();
        ctx.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut m)).ok()?;
        let mut pixels = Vec::with_capacity((w_ * h * 4) as usize);
        for y in 0..h as usize {
            let row = std::slice::from_raw_parts((m.pData as *const u8).add(y * m.RowPitch as usize), w_ as usize * 4);
            pixels.extend_from_slice(row);
        }
        ctx.Unmap(&staging, 0);
        Some(Frame { rect: Rect::from_xywh(0, 0, w_ as i32, h as i32), pixels })
    }
}
