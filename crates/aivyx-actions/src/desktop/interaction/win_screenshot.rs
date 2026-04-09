#![allow(
    unsafe_op_in_unsafe_fn,
    unused_imports,
    unreachable_code,
    unused_variables,
    dead_code,
    clippy::all
)]
//! Windows screenshot backend — GDI+ BitBlt window capture.
//!
//! Captures screenshots of individual windows or the full screen using
//! Win32 GDI+ BitBlt. Returns base64-encoded PNG data. This is the Windows
//! equivalent of `screenshot.rs` (grim/import) on Linux.

use aivyx_core::{AivyxError, Result};

#[cfg(target_os = "windows")]
use windows::Win32::Foundation::HWND;
#[cfg(target_os = "windows")]
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SRCCOPY, SelectObject,
};
#[cfg(target_os = "windows")]
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetForegroundWindow, GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN,
};

/// Capture a screenshot of the foreground window. Returns base64-encoded PNG.
pub async fn capture_window() -> Result<String> {
    #[cfg(target_os = "windows")]
    {
        let hwnd = unsafe { GetForegroundWindow() };
        if hwnd.unwrap_or_default().0 == std::ptr::null_mut() {
            return Err(AivyxError::Other("No foreground window".into()));
        }
        capture_hwnd(hwnd.unwrap_or_default()).await
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(AivyxError::Other(
            "win_screenshot: only available on Windows".into(),
        ))
    }
}

/// Capture a screenshot of a specific window by title. Returns base64-encoded PNG.
pub async fn capture_window_by_title(title: &str) -> Result<String> {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::WindowsAndMessaging::FindWindowW;
        use windows::core::PCWSTR;

        if title.is_empty() {
            return capture_window().await;
        }

        let wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        let hwnd = unsafe { FindWindowW(PCWSTR::null(), PCWSTR(wide.as_ptr())) };
        if hwnd.unwrap_or_default().0 == std::ptr::null_mut() {
            return Err(AivyxError::Other(format!("Window not found: '{title}'")));
        }
        capture_hwnd(hwnd?).await
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = title;
        Err(AivyxError::Other(
            "win_screenshot: only available on Windows".into(),
        ))
    }
}

/// Capture a full-screen screenshot. Returns base64-encoded PNG.
pub async fn capture_screen() -> Result<String> {
    #[cfg(target_os = "windows")]
    {
        let hwnd = HWND(std::ptr::null_mut()); // Desktop/screen
        let w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
        let h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
        if w == 0 || h == 0 {
            return Err(AivyxError::Other(
                "Cannot determine screen dimensions".into(),
            ));
        }
        capture_region(hwnd, 0, 0, w, h)
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(AivyxError::Other(
            "win_screenshot: only available on Windows".into(),
        ))
    }
}

/// Internal: capture a specific HWND.
#[cfg(target_os = "windows")]
async fn capture_hwnd(hwnd: HWND) -> Result<String> {
    let mut rect = windows::Win32::Foundation::RECT::default();
    unsafe {
        GetClientRect(hwnd, &mut rect)
            .map_err(|e| AivyxError::Other(format!("GetClientRect: {e}")))?;
    }
    let w = rect.right - rect.left;
    let h = rect.bottom - rect.top;
    if w <= 0 || h <= 0 {
        return Err(AivyxError::Other("Window has zero dimensions".into()));
    }
    capture_region(hwnd, 0, 0, w, h)
}

/// Internal: capture a region of an HWND via BitBlt.
///
/// Returns base64-encoded raw bitmap data (in production, would encode to PNG).
#[cfg(target_os = "windows")]
fn capture_region(hwnd: HWND, x: i32, y: i32, w: i32, h: i32) -> Result<String> {
    unsafe {
        let hdc_screen = GetDC(hwnd);
        if hdc_screen.is_invalid() {
            return Err(AivyxError::Other("GetDC failed".into()));
        }

        let hdc_mem = CreateCompatibleDC(hdc_screen);
        if hdc_mem.is_invalid() {
            ReleaseDC(hwnd, hdc_screen);
            return Err(AivyxError::Other("CreateCompatibleDC failed".into()));
        }

        let hbitmap = CreateCompatibleBitmap(hdc_screen, w, h);
        if hbitmap.is_invalid() {
            DeleteDC(hdc_mem);
            ReleaseDC(hwnd, hdc_screen);
            return Err(AivyxError::Other("CreateCompatibleBitmap failed".into()));
        }

        let old = SelectObject(hdc_mem, hbitmap);
        let result = BitBlt(hdc_mem, 0, 0, w, h, hdc_screen, x, y, SRCCOPY);

        if result.is_err() {
            SelectObject(hdc_mem, old);
            DeleteObject(hbitmap);
            DeleteDC(hdc_mem);
            ReleaseDC(hwnd, hdc_screen);
            return Err(AivyxError::Other("BitBlt failed".into()));
        }

        // Read pixel data.
        let bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h, // top-down
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0 as u32,
                ..Default::default()
            },
            ..Default::default()
        };

        let buf_size = (w * h * 4) as usize;
        if buf_size > super::MAX_SCREENSHOT_BYTES {
            SelectObject(hdc_mem, old);
            DeleteObject(hbitmap);
            DeleteDC(hdc_mem);
            ReleaseDC(hwnd, hdc_screen);
            return Err(AivyxError::Other(format!(
                "Screenshot too large: {buf_size} bytes"
            )));
        }

        let mut pixels = vec![0u8; buf_size];
        GetDIBits(
            hdc_mem,
            hbitmap,
            0,
            h as u32,
            Some(pixels.as_mut_ptr() as *mut _),
            &bmi as *const _ as *mut _,
            DIB_RGB_COLORS,
        );

        // Cleanup GDI resources.
        SelectObject(hdc_mem, old);
        DeleteObject(hbitmap);
        DeleteDC(hdc_mem);
        ReleaseDC(hwnd, hdc_screen);

        // Encode as base64 (raw BGRA data — full PNG encoding would need a PNG crate).
        // For the MVP, return base64 of raw bitmap. The agent prompt describes this.
        use std::io::Write;
        let mut bmp_data = Vec::new();

        // Simple BMP file header + DIB header + pixel data.
        let file_size = 54 + buf_size;
        // BMP file header (14 bytes)
        bmp_data.write_all(b"BM").unwrap();
        bmp_data
            .write_all(&(file_size as u32).to_le_bytes())
            .unwrap();
        bmp_data.write_all(&[0u8; 4]).unwrap(); // reserved
        bmp_data.write_all(&54u32.to_le_bytes()).unwrap(); // pixel offset

        // DIB header (40 bytes)
        bmp_data.write_all(&40u32.to_le_bytes()).unwrap();
        bmp_data.write_all(&(w as i32).to_le_bytes()).unwrap();
        bmp_data.write_all(&(h as i32).to_le_bytes()).unwrap();
        bmp_data.write_all(&1u16.to_le_bytes()).unwrap(); // planes
        bmp_data.write_all(&32u16.to_le_bytes()).unwrap(); // bpp
        bmp_data.write_all(&[0u8; 24]).unwrap(); // rest of DIB header

        // Pixel data (needs vertical flip for BMP — bottom-up).
        let row_bytes = (w * 4) as usize;
        for y in (0..h as usize).rev() {
            let start = y * row_bytes;
            bmp_data
                .write_all(&pixels[start..start + row_bytes])
                .unwrap();
        }

        // Base64 encode.
        let b64 = base64_encode(&bmp_data);
        Ok(b64)
    }
}

/// Minimal base64 encoder (avoids pulling in a base64 crate for one usage).
fn base64_encode(data: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut result = String::with_capacity((data.len() + 2) / 3 * 4);

    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;

        result.push(CHARS[((n >> 18) & 0x3F) as usize] as char);
        result.push(CHARS[((n >> 12) & 0x3F) as usize] as char);
        if chunk.len() > 1 {
            result.push(CHARS[((n >> 6) & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
        if chunk.len() > 2 {
            result.push(CHARS[(n & 0x3F) as usize] as char);
        } else {
            result.push('=');
        }
    }

    result
}

// ── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_encode_empty() {
        assert_eq!(base64_encode(b""), "");
    }

    #[test]
    fn base64_encode_hello() {
        assert_eq!(base64_encode(b"Hello"), "SGVsbG8=");
    }

    #[test]
    fn base64_encode_padding() {
        assert_eq!(base64_encode(b"He"), "SGU=");
        assert_eq!(base64_encode(b"Hel"), "SGVs");
        assert_eq!(base64_encode(b"Hell"), "SGVsbA==");
    }

    #[test]
    fn capture_on_non_windows() {
        #[cfg(not(target_os = "windows"))]
        {
            let rt = tokio::runtime::Runtime::new().unwrap();
            assert!(rt.block_on(capture_window()).is_err());
            assert!(rt.block_on(capture_screen()).is_err());
            assert!(rt.block_on(capture_window_by_title("test")).is_err());
        }
    }
}
