use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, DeleteDC, ReleaseDC,
    CreateCompatibleBitmap, SelectObject, DeleteObject,
    BitBlt, GetDIBits, GetWindowDC,
    BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    SRCCOPY, CAPTUREBLT,
    HDC, HBITMAP, HGDIOBJ, HMONITOR,
    BITMAP, GetObjectW,
    EnumDisplayMonitors,
    GetMonitorInfoW, MONITORINFO,
};
use windows::Win32::Foundation::{GetLastError, HWND, RECT, LPARAM, BOOL};
use windows::Win32::UI::WindowsAndMessaging::{
    GetDesktopWindow, GetWindowRect,
};

/// Holds raw pixel data from a single monitor capture, plus dimensions.
#[derive(Clone)]
pub struct CapturedImage {
    pub data: Vec<u8>,
    pub width: i32,
    pub height: i32,
    /// Index of the monitor (0-based, in enumeration order)
    pub monitor_index: usize,
}

pub struct ScreenCapture;

impl ScreenCapture {
    pub fn new() -> Self {
        ScreenCapture
    }

    /// Capture all monitors into individual memory buffers (no disk I/O).
    /// Returns a Vec of CapturedImage, one per monitor.
    /// Falls back to desktop window capture if monitor enumeration fails.
    pub fn capture_all(&self) -> Result<Vec<CapturedImage>, String> {
        unsafe {
            let monitors = enumerate_monitors()?;

            if monitors.is_empty() {
                // Fallback: capture the entire virtual desktop
                let img = capture_desktop()?;
                return Ok(vec![img]);
            }

            let mut images = Vec::with_capacity(monitors.len());
            for (idx, rect) in monitors.iter().enumerate() {
                match capture_rect(*rect, idx) {
                    Ok(img) => images.push(img),
                    Err(e) => {
                        eprintln!(
                            "[auto_ocr_win] Failed to capture monitor {} (rect {:?}): {}",
                            idx, rect, e
                        );
                        // Continue capturing remaining monitors
                    }
                }
            }

            if images.is_empty() {
                // All monitors failed, fall back to desktop capture
                let img = capture_desktop()?;
                return Ok(vec![img]);
            }

            Ok(images)
        }
    }
}

/// Monitor info collected during enumeration.
#[derive(Clone, Copy, Debug)]
struct MonitorRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

/// Enumerate all display monitors and collect their screen-space rectangles.
unsafe fn enumerate_monitors() -> Result<Vec<MonitorRect>, String> {
    let mut collected: Vec<MonitorRect> = Vec::new();
    let collected_ptr: *mut Vec<MonitorRect> = &mut collected;

    let enum_result = EnumDisplayMonitors(
        HDC::default(),
        None,
        Some(monitor_enum_proc),
        LPARAM(collected_ptr as isize),
    );
    if enum_result.0 == 0 {
        return Err(format!("EnumDisplayMonitors failed (error code: {:?})", GetLastError()));
    }

    if collected.is_empty() {
        return capture_desktop_rect().map(|r| vec![r]);
    }

    Ok(collected)
}

/// Fallback: get the desktop window rectangle as a single MonitorRect.
unsafe fn capture_desktop_rect() -> Result<MonitorRect, String> {
    let desktop_hwnd: HWND = GetDesktopWindow();
    let mut rect = RECT::default();
    if GetWindowRect(desktop_hwnd, &mut rect).is_err() {
        return Err("GetWindowRect failed".to_string());
    }
    Ok(MonitorRect {
        left: rect.left,
        top: rect.top,
        right: rect.right,
        bottom: rect.bottom,
    })
}

/// Callback for EnumDisplayMonitors — collects MonitorRect into a Vec via raw pointer.
unsafe extern "system" fn monitor_enum_proc(
    hmonitor: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    let collected: &mut Vec<MonitorRect> = &mut *(lparam.0 as *mut Vec<MonitorRect>);

    let mut monitor_info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        rcMonitor: RECT::default(),
        rcWork: RECT::default(),
        dwFlags: 0,
    };

    let mi_result = GetMonitorInfoW(hmonitor, &mut monitor_info);
    if mi_result.0 != 0 {
        let rc = monitor_info.rcMonitor;
        collected.push(MonitorRect {
            left: rc.left,
            top: rc.top,
            right: rc.right,
            bottom: rc.bottom,
        });
    }
    BOOL(1) // Continue enumeration
}

/// Capture the entire virtual desktop as a fallback.
unsafe fn capture_desktop() -> Result<CapturedImage, String> {
    let rect = capture_desktop_rect()?;
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;

    if width <= 0 || height <= 0 {
        return Err(format!("capture_desktop: invalid dimensions {}x{}", width, height));
    }

    let desktop_hwnd: HWND = GetDesktopWindow();
    let desktop_dc: HDC = GetWindowDC(desktop_hwnd);
    if desktop_dc.is_invalid() {
        return Err("capture_desktop: GetWindowDC failed".to_string());
    }

    let pixel_data = capture_rect_inner(desktop_dc, 0, 0, width, height)?;

    let _ = ReleaseDC(desktop_hwnd, desktop_dc);

    Ok(CapturedImage {
        data: pixel_data,
        width,
        height,
        monitor_index: 0,
    })
}

/// Capture a specific rectangle (in screen coordinates) from the desktop.
unsafe fn capture_rect(rect: MonitorRect, monitor_index: usize) -> Result<CapturedImage, String> {
    let x = rect.left;
    let y = rect.top;
    let width = rect.right - rect.left;
    let height = rect.bottom - rect.top;

    if width <= 0 || height <= 0 {
        return Err(format!(
            "Monitor {} has invalid dimensions: {}x{} at ({},{})",
            monitor_index, width, height, x, y
        ));
    }

    let desktop_hwnd: HWND = GetDesktopWindow();
    let desktop_dc: HDC = GetWindowDC(desktop_hwnd);
    if desktop_dc.is_invalid() {
        return Err(format!(
            "capture_rect: GetWindowDC failed for monitor {}",
            monitor_index
        ));
    }

    let pixel_data = capture_rect_inner(desktop_dc, x, y, width, height)?;

    let _ = ReleaseDC(desktop_hwnd, desktop_dc);

    Ok(CapturedImage {
        data: pixel_data,
        width,
        height,
        monitor_index,
    })
}

/// Low-level bitmap capture for a rectangle region.
unsafe fn capture_rect_inner(
    desktop_dc: HDC,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) -> Result<Vec<u8>, String> {
    let capture_dc = CreateCompatibleDC(desktop_dc);
    if capture_dc.is_invalid() {
        return Err("CreateCompatibleDC failed".to_string());
    }

    let bitmap: HBITMAP = CreateCompatibleBitmap(desktop_dc, width, height);
    if bitmap.is_invalid() {
        let _ = DeleteDC(capture_dc);
        return Err("CreateCompatibleBitmap failed".to_string());
    }

    let old_obj = SelectObject(capture_dc, bitmap);

    let result = BitBlt(
        capture_dc,
        0,
        0,
        width,
        height,
        desktop_dc,
        x,
        y,
        SRCCOPY | CAPTUREBLT,
    );

    if result.is_err() {
        let _ = SelectObject(capture_dc, old_obj);
        let _ = DeleteObject(bitmap);
        let _ = DeleteDC(capture_dc);
        return Err("BitBlt failed".to_string());
    }

    let pixel_data = get_bitmap_data(capture_dc, bitmap, width, height)?;

    let _ = SelectObject(capture_dc, old_obj);
    let _ = DeleteObject(bitmap);
    let _ = DeleteDC(capture_dc);

    Ok(pixel_data)
}

/// Extract raw BGRA pixel data from a GDI bitmap into a Vec<u8>.
unsafe fn get_bitmap_data(
    dc: HDC,
    bitmap: HBITMAP,
    _width: i32,
    _height: i32,
) -> Result<Vec<u8>, String> {
    let mut bmp = BITMAP::default();
    let ret = GetObjectW(
        HGDIOBJ::from(bitmap),
        std::mem::size_of::<BITMAP>() as i32,
        Some(&mut bmp as *mut _ as *mut _),
    );
    if ret == 0 {
        return Err("GetObjectW failed".to_string());
    }

    let bm_width = bmp.bmWidth;
    let bm_height = bmp.bmHeight;
    let bits_per_pixel = bmp.bmBitsPixel as u16;

    let mut bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: bm_width,
            biHeight: -bm_height, // Negative for top-down DIB
            biPlanes: 1,
            biBitCount: bits_per_pixel,
            biCompression: BI_RGB.0,
            biSizeImage: 0,
            biXPelsPerMeter: 0,
            biYPelsPerMeter: 0,
            biClrUsed: 0,
            biClrImportant: 0,
        },
        bmiColors: [std::mem::zeroed(); 1],
    };

    let row_size = ((bm_width * bits_per_pixel as i32 + 31) / 32) * 4;
    let data_size = (row_size * bm_height) as usize;

    if row_size <= 0 || bm_height <= 0 {
        return Err(format!("Invalid bitmap dimensions: {}x{}, row_size={}", bm_width, bm_height, row_size));
    }

    let mut data: Vec<u8> = vec![0u8; data_size];

    let result = GetDIBits(
        dc,
        bitmap,
        0,
        bm_height as u32,
        Some(data.as_mut_ptr() as *mut _),
        &mut bi,
        DIB_RGB_COLORS,
    );

    if result == 0 {
        return Err(format!("GetDIBits failed: {:?}", GetLastError()));
    }

    Ok(data)
}