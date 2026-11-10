#![cfg(windows)]
use sha2::{Digest, Sha256};
use std::{
    collections::VecDeque,
    fs,
    io::{Read, Write},
    net::TcpStream,
    sync::{
        Arc, Condvar, LazyLock, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::*,
    Graphics::Gdi::*,
    System::{LibraryLoader::*, Threading::*},
    UI::{Controls::*, Input::KeyboardAndMouse::*, WindowsAndMessaging::*},
};
const FRAME: u32 = 0x42535446;
const INPUT: u32 = 0x42535449;
const FRAME_READY: u32 = WM_APP + 1;
const RESIZE_TIMER: usize = 1;
type ReadBuffer = unsafe extern "C" fn(u32, i32, i32, i32, i32, u32, u32, *mut core::ffi::c_void);
#[derive(Default)]
struct Pixels {
    bytes: Vec<u8>,
    width: u32,
    height: u32,
    sequence: u32,
    ready: Option<Instant>,
}
struct View {
    window: AtomicUsize,
    pixels: Mutex<Arc<Pixels>>,
    frame_pending: AtomicBool,
    touching: AtomicBool,
    width: AtomicUsize,
    height: AtomicUsize,
    paints: AtomicUsize,
    presented: AtomicUsize,
    presented_sequence: AtomicUsize,
    paint_us: AtomicUsize,
    paint_max_us: AtomicUsize,
    paint_age_us: AtomicUsize,
}
static VIEWS: LazyLock<[View; 2]> = LazyLock::new(|| {
    std::array::from_fn(|_| View {
        window: AtomicUsize::new(0),
        pixels: Mutex::new(Arc::new(Pixels::default())),
        frame_pending: AtomicBool::new(false),
        touching: AtomicBool::new(false),
        width: AtomicUsize::new(540),
        height: AtomicUsize::new(960),
        paints: AtomicUsize::new(0),
        presented: AtomicUsize::new(0),
        presented_sequence: AtomicUsize::new(0),
        paint_us: AtomicUsize::new(0),
        paint_max_us: AtomicUsize::new(0),
        paint_age_us: AtomicUsize::new(0),
    })
});
struct QueuedInput {
    packet: [u32; 7],
    queued: Instant,
}
static INPUTS: LazyLock<(Mutex<VecDeque<QueuedInput>>, Condvar)> =
    LazyLock::new(|| (Mutex::new(VecDeque::new()), Condvar::new()));
static MODULE: AtomicUsize = AtomicUsize::new(0);
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}
fn log(message: &str) {
    let root = std::env::var_os("LOCALAPPDATA")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("MinimalBlueStack/logs");
    let _ = fs::create_dir_all(&root);
    if let Ok(mut file) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(root.join("native-render.log"))
    {
        let _ = writeln!(file, "pid={} {message}", std::process::id());
    }
}
fn queue(input: [u32; 7]) {
    enqueue(&mut INPUTS.0.lock().unwrap(), input);
    INPUTS.1.notify_one();
}
fn enqueue(queue: &mut VecDeque<QueuedInput>, packet: [u32; 7]) {
    if packet[1] == 1 && packet[3] == 2 {
        if let Some(last) = queue.back_mut().filter(|last| {
            last.packet[1] == 1 && last.packet[2] == packet[2] && last.packet[3] == 2
        }) {
            *last = QueuedInput {
                packet,
                queued: Instant::now(),
            };
            return;
        }
    }
    if queue.len() >= 256 {
        queue.clear();
        for slot in 0..2 {
            queue.push_back(QueuedInput {
                packet: [INPUT, 1, slot, 3, 0, 0, 0],
                queued: Instant::now(),
            });
        }
    }
    queue.push_back(QueuedInput {
        packet,
        queued: Instant::now(),
    });
}
fn publish_frame(view: &View, pixels: Pixels) -> Vec<u8> {
    let previous = std::mem::replace(&mut *view.pixels.lock().unwrap(), Arc::new(pixels));
    Arc::try_unwrap(previous)
        .map(|pixels| pixels.bytes)
        .unwrap_or_default()
}
unsafe fn stretch_pixels(dc: HDC, area: &RECT, pixels: &Pixels) {
    unsafe {
        let mut bitmap: BITMAPINFO = std::mem::zeroed();
        bitmap.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bitmap.bmiHeader.biWidth = pixels.width as i32;
        bitmap.bmiHeader.biHeight = -(pixels.height as i32);
        bitmap.bmiHeader.biPlanes = 1;
        bitmap.bmiHeader.biBitCount = 32;
        bitmap.bmiHeader.biCompression = BI_RGB;
        StretchDIBits(
            dc,
            0,
            0,
            area.right,
            area.bottom,
            0,
            0,
            pixels.width as i32,
            pixels.height as i32,
            pixels.bytes.as_ptr() as _,
            &bitmap,
            DIB_RGB_COLORS,
            SRCCOPY,
        );
    }
}
unsafe fn paint_pixels(dc: HDC, area: &RECT, pixels: &Pixels) {
    unsafe {
        SetStretchBltMode(dc, COLORONCOLOR);
        stretch_pixels(dc, area, pixels);
    }
}
unsafe extern "system" fn dispatcher_proc(
    window: HWND,
    message: u32,
    wp: usize,
    lp: isize,
) -> isize {
    unsafe {
        if message != FRAME_READY || wp >= 2 {
            return DefWindowProcW(window, message, wp, lp);
        }
        let view = &VIEWS[wp];
        view.frame_pending.store(false, Ordering::Release);
        let mut target = view.window.load(Ordering::Acquire) as HWND;
        if target.is_null() {
            target = CreateWindowExW(
                0,
                wide("BlueStacksNativeGameWindow").as_ptr(),
                wide(&format!("BlueStacks Native Display {}", wp + 1)).as_ptr(),
                WS_OVERLAPPEDWINDOW,
                60 + wp as i32 * 530,
                70,
                500,
                850,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                MODULE.load(Ordering::Relaxed) as HINSTANCE,
                (wp + 1) as _,
            );
            if !target.is_null() {
                SetPropW(target, wide("BstNativeGameSlot").as_ptr(), (wp + 1) as _);
                view.window.store(target as usize, Ordering::Release);
            }
        }
        if !target.is_null() {
            InvalidateRect(target, std::ptr::null(), 0);
        }
        0
    }
}
unsafe extern "system" fn window_proc(window: HWND, message: u32, wp: usize, lp: isize) -> isize {
    unsafe {
        if message == WM_NCCREATE {
            let create = &*(lp as *const CREATESTRUCTW);
            SetWindowLongPtrW(window, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        let stored = GetWindowLongPtrW(window, GWLP_USERDATA) as usize;
        if !(1..=2).contains(&stored) {
            return DefWindowProcW(window, message, wp, lp);
        }
        let slot = stored - 1;
        let view = &VIEWS[slot];
        match message {
            WM_PAINT => {
                let started = Instant::now();
                let mut paint: PAINTSTRUCT = std::mem::zeroed();
                let dc = BeginPaint(window, &mut paint);
                let mut area: RECT = std::mem::zeroed();
                GetClientRect(window, &mut area);
                if area.right <= 0
                    || area.bottom <= 0
                    || IsIconic(window) != 0
                    || IsWindowVisible(window) == 0
                {
                    EndPaint(window, &paint);
                    return 0;
                }
                let mut back_dc = std::ptr::null_mut();
                let buffer =
                    BeginBufferedPaint(dc, &area, BPBF_TOPDOWNDIB, std::ptr::null(), &mut back_dc);
                let paint_dc = if buffer == 0 { dc } else { back_dc };
                let pixels = view.pixels.lock().unwrap().clone();
                if !pixels.bytes.is_empty() {
                    paint_pixels(paint_dc, &area, &pixels);
                }
                if buffer != 0 {
                    EndBufferedPaint(buffer, 1);
                }
                GdiFlush();
                EndPaint(window, &paint);
                if !pixels.bytes.is_empty() {
                    let elapsed = started.elapsed().as_micros() as usize;
                    view.paints.fetch_add(1, Ordering::Relaxed);
                    view.paint_us.fetch_add(elapsed, Ordering::Relaxed);
                    view.paint_max_us.fetch_max(elapsed, Ordering::Relaxed);
                    view.paint_age_us.fetch_add(
                        pixels
                            .ready
                            .map_or(0, |ready| ready.elapsed().as_micros() as usize),
                        Ordering::Relaxed,
                    );
                    if view
                        .presented_sequence
                        .swap(pixels.sequence as usize, Ordering::Relaxed)
                        != pixels.sequence as usize
                    {
                        view.presented.fetch_add(1, Ordering::Relaxed);
                    }
                }
                0
            }
            WM_SIZE => {
                InvalidateRect(window, std::ptr::null(), 0);
                0
            }
            WM_ENTERSIZEMOVE => {
                SetTimer(window, RESIZE_TIMER, 16, None);
                0
            }
            WM_TIMER if wp == RESIZE_TIMER => {
                if IsIconic(window) == 0 {
                    InvalidateRect(window, std::ptr::null(), 0);
                    UpdateWindow(window);
                }
                0
            }
            WM_EXITSIZEMOVE => {
                KillTimer(window, RESIZE_TIMER);
                InvalidateRect(window, std::ptr::null(), 0);
                UpdateWindow(window);
                0
            }
            WM_ERASEBKGND => 1,
            WM_LBUTTONDOWN | WM_LBUTTONUP | WM_MOUSEMOVE => {
                if message == WM_MOUSEMOVE && wp & 1 == 0 {
                    return 0;
                }
                if message == WM_LBUTTONDOWN {
                    view.touching.store(true, Ordering::Relaxed);
                    SetCapture(window);
                }
                if message == WM_LBUTTONUP {
                    view.touching.store(false, Ordering::Relaxed);
                    ReleaseCapture();
                }
                let mut area: RECT = std::mem::zeroed();
                GetClientRect(window, &mut area);
                let width = view.width.load(Ordering::Relaxed) as u32;
                let height = view.height.load(Ordering::Relaxed) as u32;
                let x = (lp as u16 as i16 as i32).max(0) as u32;
                let y = ((lp >> 16) as u16 as i16 as i32).max(0) as u32;
                let input = [
                    INPUT,
                    1,
                    slot as u32,
                    if message == WM_LBUTTONDOWN {
                        0
                    } else if message == WM_LBUTTONUP {
                        1
                    } else {
                        2
                    },
                    (x * width / (area.right.max(1) as u32)).min(width.saturating_sub(1)),
                    (y * height / (area.bottom.max(1) as u32)).min(height.saturating_sub(1)),
                    0,
                ];
                queue(input);
                0
            }
            WM_KEYDOWN if wp == 27 => {
                queue([INPUT, 2, slot as u32, 0, 0, 0, 4]);
                0
            }
            WM_CAPTURECHANGED | WM_CANCELMODE | WM_KILLFOCUS => {
                if view.touching.swap(false, Ordering::Relaxed) {
                    queue([INPUT, 1, slot as u32, 3, 0, 0, 0]);
                }
                DefWindowProcW(window, message, wp, lp)
            }
            WM_CLOSE => {
                ShowWindow(window, SW_HIDE);
                0
            }
            _ => DefWindowProcW(window, message, wp, lp),
        }
    }
}
fn connection(reader: ReadBuffer, dispatcher: usize) {
    loop {
        let result = (|| -> std::io::Result<()> {
            let mut socket = TcpStream::connect("127.0.0.1:57015")?;
            socket.set_read_timeout(Some(Duration::from_secs(5)))?;
            socket.set_write_timeout(Some(Duration::from_secs(5)))?;
            socket.set_nodelay(true)?;
            let mut first = [true; 2];
            let mut reusable = Vec::new();
            let mut interval = Instant::now();
            let mut frames = [0u64; 2];
            let mut read_us = [0u128; 2];
            let mut max_us = [0u128; 2];
            loop {
                let mut bytes = [0u8; 24];
                socket.read_exact(&mut bytes)?;
                let frame: Vec<u32> = bytes
                    .chunks_exact(4)
                    .map(|v| u32::from_le_bytes(v.try_into().unwrap()))
                    .collect();
                let slot = frame[1] as usize;
                if frame[0] != FRAME
                    || slot >= 2
                    || frame[2] == 0
                    || !(1..=2048).contains(&frame[3])
                    || !(1..=2048).contains(&frame[4])
                {
                    return Err(std::io::Error::other("Invalid frame metadata"));
                }
                reusable.resize((frame[3] * frame[4] * 4) as usize, 0);
                let started = Instant::now();
                unsafe {
                    reader(
                        frame[2],
                        0,
                        0,
                        frame[3] as i32,
                        frame[4] as i32,
                        0x1908,
                        0x1401,
                        reusable.as_mut_ptr() as _,
                    );
                }
                let elapsed = started.elapsed().as_micros();
                frames[slot] += 1;
                read_us[slot] += elapsed;
                max_us[slot] = max_us[slot].max(elapsed);
                for pixel in reusable.chunks_exact_mut(4) {
                    pixel.swap(0, 2);
                }
                let view = &VIEWS[slot];
                view.width.store(frame[3] as usize, Ordering::Relaxed);
                view.height.store(frame[4] as usize, Ordering::Relaxed);
                reusable = publish_frame(
                    view,
                    Pixels {
                        bytes: std::mem::take(&mut reusable),
                        width: frame[3],
                        height: frame[4],
                        sequence: frame[5],
                        ready: Some(Instant::now()),
                    },
                );
                if !view.frame_pending.swap(true, Ordering::AcqRel) {
                    unsafe {
                        if PostMessageW(dispatcher as HWND, FRAME_READY, slot, 0) == 0 {
                            view.frame_pending.store(false, Ordering::Release);
                        }
                    }
                }
                let ack = [INPUT, 0, 0, 0, 0, 0, 0]
                    .into_iter()
                    .flat_map(u32::to_le_bytes)
                    .collect::<Vec<_>>();
                socket.write_all(&ack)?;
                if first[slot] {
                    log(&format!("Rust native frame delivered slot={slot}"));
                    first[slot] = false;
                }
                if interval.elapsed() >= Duration::from_secs(5) {
                    for index in 0..2 {
                        if frames[index] > 0 {
                            log(&format!(
                                "Frame stats slot={index} fps={:.1} readback_avg_ms={:.2} readback_max_ms={:.2}",
                                frames[index] as f64 / interval.elapsed().as_secs_f64(),
                                read_us[index] as f64 / frames[index] as f64 / 1000.,
                                max_us[index] as f64 / 1000.
                            ));
                            let view = &VIEWS[index];
                            let paints = view.paints.swap(0, Ordering::Relaxed);
                            let shown = view.presented.swap(0, Ordering::Relaxed);
                            let paint_us = view.paint_us.swap(0, Ordering::Relaxed);
                            let maximum = view.paint_max_us.swap(0, Ordering::Relaxed);
                            let age = view.paint_age_us.swap(0, Ordering::Relaxed);
                            log(&format!(
                                "Presentation stats slot={index} fps={:.1} paints={paints} paint_avg_ms={:.2} paint_max_ms={:.2} frame_age_avg_ms={:.2}",
                                shown as f64 / interval.elapsed().as_secs_f64(),
                                paint_us as f64 / paints.max(1) as f64 / 1000.,
                                maximum as f64 / 1000.,
                                age as f64 / paints.max(1) as f64 / 1000.
                            ));
                        }
                    }
                    frames = [0; 2];
                    read_us = [0; 2];
                    max_us = [0; 2];
                    interval = Instant::now();
                }
            }
        })();
        if let Err(error) = result {
            if error.kind() != std::io::ErrorKind::ConnectionRefused {
                log(&format!("Display connection: {error}"));
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}
fn input_connection() {
    let mut sequence = 0u32;
    loop {
        let result = (|| -> std::io::Result<()> {
            let mut socket = TcpStream::connect("127.0.0.1:57016")?;
            socket.set_nodelay(true)?;
            socket.set_read_timeout(Some(Duration::from_secs(2)))?;
            socket.set_write_timeout(Some(Duration::from_secs(2)))?;
            {
                let mut queue = INPUTS.0.lock().unwrap();
                queue.clear();
                for slot in 0..2 {
                    enqueue(&mut queue, [INPUT, 1, slot, 3, 0, 0, 0]);
                }
            }
            log("Independent input channel connected");
            let mut interval = Instant::now();
            let mut count = 0u64;
            let mut total = 0u128;
            let mut maximum = 0u128;
            let mut queue_max = 0u128;
            let mut events = 0u64;
            let mut event_us = 0u128;
            loop {
                let mut queue = INPUTS.0.lock().unwrap();
                if queue.is_empty() {
                    queue = INPUTS
                        .1
                        .wait_timeout_while(queue, Duration::from_millis(500), |queue| {
                            queue.is_empty()
                        })
                        .unwrap()
                        .0;
                }
                let item = queue.pop_front();
                drop(queue);
                let queued_us = item
                    .as_ref()
                    .map_or(0, |item| item.queued.elapsed().as_micros());
                let packet = item.map_or([INPUT, 3, 0, 0, 0, 0, 0], |item| item.packet);
                sequence = sequence.wrapping_add(1);
                let bytes = packet
                    .into_iter()
                    .chain([sequence])
                    .flat_map(u32::to_le_bytes)
                    .collect::<Vec<_>>();
                let started = Instant::now();
                socket.write_all(&bytes)?;
                let mut ack = [0; 8];
                socket.read_exact(&mut ack)?;
                if u32::from_le_bytes(ack[..4].try_into().unwrap()) != 0x42535441
                    || u32::from_le_bytes(ack[4..].try_into().unwrap()) != sequence
                {
                    return Err(std::io::Error::other("Invalid input acknowledgement"));
                }
                let elapsed = started.elapsed().as_micros();
                if packet[1] != 3 {
                    events += 1;
                    event_us += elapsed;
                }
                count += 1;
                total += elapsed;
                maximum = maximum.max(elapsed);
                queue_max = queue_max.max(queued_us);
                if interval.elapsed() >= Duration::from_secs(5) {
                    log(&format!(
                        "Input stats samples={count} rtt_avg_ms={:.2} rtt_max_ms={:.2} queue_max_ms={:.2} events={events} event_dispatch_avg_ms={:.2}",
                        total as f64 / count as f64 / 1000.,
                        maximum as f64 / 1000.,
                        queue_max as f64 / 1000.,
                        if events == 0 {
                            0.
                        } else {
                            event_us as f64 / events as f64 / 1000.
                        }
                    ));
                    count = 0;
                    total = 0;
                    maximum = 0;
                    queue_max = 0;
                    events = 0;
                    event_us = 0;
                    interval = Instant::now();
                }
            }
        })();
        if let Err(error) = result {
            if error.kind() != std::io::ErrorKind::ConnectionRefused {
                log(&format!("Input connection: {error}"));
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}
unsafe extern "system" fn render(_: *mut core::ffi::c_void) -> u32 {
    unsafe {
        BufferedPaintInit();
        let mut message: MSG = std::mem::zeroed();
        PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_NOREMOVE);
        let mut module = std::ptr::null_mut();
        let mut verified = false;
        for _ in 0..120 {
            module = GetModuleHandleW(wide("libOpenglRender.dll").as_ptr());
            if !module.is_null() && !verified {
                let mut path = vec![0u16; 32768];
                let n = GetModuleFileNameW(module, path.as_mut_ptr(), path.len() as u32);
                let path = String::from_utf16_lossy(&path[..n as usize]);
                let Ok(bytes) = fs::read(path) else {
                    log("Renderer cannot be read");
                    return 2;
                };
                if format!("{:x}", Sha256::digest(&bytes))
                    != "116bdb8cdbb9e980a462433d0db1a47bcb4b11946de5d8fe917de502ba330ee4"
                {
                    log("Unsupported renderer checksum");
                    return 2;
                }
                verified = true;
            }
            if verified
                && (module as *const u8)
                    .add(0xbd4b88)
                    .cast::<usize>()
                    .read_volatile()
                    != 0
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
        if !verified
            || (module as *const u8)
                .add(0xbd4b88)
                .cast::<usize>()
                .read_volatile()
                == 0
        {
            log("Renderer library is not ready");
            return 1;
        }
        let function = (module as *const u8).add(0x1ad540);
        if std::slice::from_raw_parts(function, 7) != [0x48, 0x83, 0xec, 0x58, 0x44, 0x8b, 0xd1] {
            log("Renderer API fingerprint differs");
            return 2;
        }
        let reader: ReadBuffer = std::mem::transmute(function);
        let class = wide("BlueStacksNativeGameWindow");
        let instance = MODULE.load(Ordering::Relaxed) as HINSTANCE;
        let mut info: WNDCLASSW = std::mem::zeroed();
        info.lpfnWndProc = Some(window_proc);
        info.style = CS_HREDRAW | CS_VREDRAW;
        info.hInstance = instance;
        info.lpszClassName = class.as_ptr();
        info.hCursor = LoadCursorW(std::ptr::null_mut(), 32512usize as _);
        info.hbrBackground = GetStockObject(BLACK_BRUSH) as _;
        if RegisterClassW(&info) == 0 && GetLastError() != 1410 {
            log("Native window class registration failed");
            return 3;
        }
        let dispatch_class = wide("MinimalBlueStackFrameDispatcher");
        let mut dispatch_info: WNDCLASSW = std::mem::zeroed();
        dispatch_info.lpfnWndProc = Some(dispatcher_proc);
        dispatch_info.hInstance = instance;
        dispatch_info.lpszClassName = dispatch_class.as_ptr();
        if RegisterClassW(&dispatch_info) == 0 && GetLastError() != 1410 {
            log("Frame dispatcher registration failed");
            return 3;
        }
        let dispatcher = CreateWindowExW(
            0,
            dispatch_class.as_ptr(),
            std::ptr::null(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        );
        if dispatcher.is_null() {
            log("Frame dispatcher creation failed");
            return 3;
        }
        let dispatch_handle = dispatcher as usize;
        std::thread::spawn(move || connection(reader, dispatch_handle));
        std::thread::spawn(input_connection);
        log("Rust renderer bridge initialized");
        while GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
        0
    }
}
#[unsafe(no_mangle)]
pub extern "C" fn NativeBridgeInitialize() -> u32 {
    1
}
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllMain(
    instance: HINSTANCE,
    reason: u32,
    _: *mut core::ffi::c_void,
) -> i32 {
    if reason == 1 {
        MODULE.store(instance as usize, Ordering::Relaxed);
        unsafe {
            DisableThreadLibraryCalls(instance);
            let thread = CreateThread(
                std::ptr::null(),
                0,
                Some(render),
                std::ptr::null(),
                0,
                std::ptr::null_mut(),
            );
            if !thread.is_null() {
                CloseHandle(thread);
            }
        }
    }
    1
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shrinking_color_frames_must_not_and_neighboring_pixel_colors() {
        unsafe {
            let dc = CreateCompatibleDC(std::ptr::null_mut());
            assert!(!dc.is_null());
            let mut info: BITMAPINFO = std::mem::zeroed();
            info.bmiHeader.biSize = 40;
            info.bmiHeader.biWidth = 31;
            info.bmiHeader.biHeight = -31;
            info.bmiHeader.biPlanes = 1;
            info.bmiHeader.biBitCount = 32;
            let mut bits = std::ptr::null_mut();
            let bitmap = CreateDIBSection(
                dc,
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                std::ptr::null_mut(),
                0,
            );
            assert!(!bitmap.is_null());
            let previous = SelectObject(dc, bitmap);
            let mut bytes = Vec::new();
            for y in 0..32 {
                for x in 0..32 {
                    let color = if (x + y) % 2 == 0 {
                        0x00ff0000u32
                    } else {
                        0x0000ffffu32
                    };
                    bytes.extend(color.to_le_bytes());
                }
            }
            let pixels = Pixels {
                bytes,
                width: 32,
                height: 32,
                ..Default::default()
            };
            let area = RECT {
                left: 0,
                top: 0,
                right: 31,
                bottom: 31,
            };
            SetStretchBltMode(dc, BLACKONWHITE);
            stretch_pixels(dc, &area, &pixels);
            GdiFlush();
            let output = std::slice::from_raw_parts(bits as *const u32, 31 * 31);
            assert!(
                output.iter().any(|pixel| pixel & 0x00ffffff == 0),
                "Old scaling must reproduce dark grid artifacts"
            );
            paint_pixels(dc, &area, &pixels);
            GdiFlush();
            assert!(
                output
                    .iter()
                    .all(|pixel| matches!(pixel & 0x00ffffff, 0x00ff0000 | 0x0000ffff)),
                "Color scaling must not invent dark/color grid pixels"
            );
            SelectObject(dc, previous);
            DeleteObject(bitmap);
            DeleteDC(dc);
        }
    }
    #[test]
    fn published_frames_cannot_mutate_an_in_progress_paint_snapshot() {
        let view = &VIEWS[1];
        publish_frame(
            view,
            Pixels {
                bytes: vec![7; 16],
                width: 2,
                height: 2,
                sequence: 1,
                ready: Some(Instant::now()),
            },
        );
        let painting = view.pixels.lock().unwrap().clone();
        for sequence in 2..1002 {
            let recycled = publish_frame(
                view,
                Pixels {
                    bytes: vec![sequence as u8; 16],
                    width: 2,
                    height: 2,
                    sequence,
                    ready: Some(Instant::now()),
                },
            );
            assert!(recycled.is_empty() || recycled.len() == 16);
        }
        assert_eq!(painting.bytes, vec![7; 16]);
        assert_eq!(painting.sequence, 1);
        assert_eq!(view.pixels.lock().unwrap().sequence, 1001);
    }
    #[test]
    fn standard_modal_dispatch_delivers_frame_notifications() {
        unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            MODULE.store(instance as usize, Ordering::Relaxed);
            let game_class = wide("BlueStacksNativeGameWindow");
            let mut info: WNDCLASSW = std::mem::zeroed();
            info.lpfnWndProc = Some(window_proc);
            info.hInstance = instance;
            info.lpszClassName = game_class.as_ptr();
            info.style = CS_HREDRAW | CS_VREDRAW;
            assert!(RegisterClassW(&info) != 0 || GetLastError() == 1410);
            let class = wide("MinimalBlueStackTestDispatcher");
            info.lpfnWndProc = Some(dispatcher_proc);
            info.lpszClassName = class.as_ptr();
            assert!(RegisterClassW(&info) != 0 || GetLastError() == 1410);
            let dispatcher = CreateWindowExW(
                0,
                class.as_ptr(),
                std::ptr::null(),
                0,
                0,
                0,
                0,
                0,
                HWND_MESSAGE,
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            );
            assert!(!dispatcher.is_null());
            VIEWS[0].frame_pending.store(true, Ordering::Release);
            assert_ne!(PostMessageW(dispatcher, FRAME_READY, 0, 0), 0);
            let mut message: MSG = std::mem::zeroed();
            // Only standard dispatch: no custom primary-loop WM_APP handler.
            for _ in 0..100 {
                if PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_REMOVE) == 0 {
                    break;
                }
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
            assert!(!VIEWS[0].frame_pending.load(Ordering::Acquire));
            let window = VIEWS[0].window.load(Ordering::Acquire) as HWND;
            assert!(!window.is_null());
            SendMessageW(window, WM_ENTERSIZEMOVE, 0, 0);
            SendMessageW(window, WM_EXITSIZEMOVE, 0, 0);
            DestroyWindow(window);
            DestroyWindow(dispatcher);
            VIEWS[0].window.store(0, Ordering::Release);
        }
    }
    #[test]
    fn drag_moves_coalesce_without_delaying_release() {
        let mut queue = VecDeque::new();
        enqueue(&mut queue, [INPUT, 1, 0, 0, 0, 0, 0]);
        for x in 0..2000 {
            enqueue(&mut queue, [INPUT, 1, 0, 2, x, 100, 0]);
        }
        enqueue(&mut queue, [INPUT, 1, 0, 1, 1999, 100, 0]);
        assert_eq!(queue.len(), 3);
        assert_eq!(queue[0].packet[3], 0);
        assert_eq!(queue[1].packet[4], 1999);
        assert_eq!(queue[2].packet[3], 1);
    }
    #[test]
    fn coalescing_preserves_other_windows_and_gesture_boundaries() {
        let packets = [
            [INPUT, 1, 0, 2, 1, 1, 0],
            [INPUT, 1, 1, 2, 2, 2, 0],
            [INPUT, 1, 0, 1, 3, 3, 0],
            [INPUT, 1, 0, 0, 4, 4, 0],
            [INPUT, 1, 0, 2, 5, 5, 0],
        ];
        let mut queue = VecDeque::new();
        for packet in packets {
            enqueue(&mut queue, packet);
        }
        assert_eq!(
            queue
                .into_iter()
                .map(|item| item.packet)
                .collect::<Vec<_>>(),
            packets
        );
    }
    #[test]
    fn overflow_cancels_stale_gestures_and_keeps_current_release() {
        let mut queue = VecDeque::new();
        for _ in 0..256 {
            enqueue(&mut queue, [INPUT, 1, 0, 0, 1, 1, 0]);
        }
        enqueue(&mut queue, [INPUT, 1, 0, 1, 1, 1, 0]);
        assert_eq!(queue.len(), 3);
        assert_eq!((queue[0].packet[2], queue[0].packet[3]), (0, 3));
        assert_eq!((queue[1].packet[2], queue[1].packet[3]), (1, 3));
        assert_eq!(queue[2].packet[3], 1);
    }
}
