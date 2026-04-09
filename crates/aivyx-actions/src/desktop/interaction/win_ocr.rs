#![allow(unsafe_op_in_unsafe_fn, unused_imports, unreachable_code, unused_variables, dead_code, clippy::all)]
//! Windows OCR backend — Windows.Media.Ocr WinRT API.
//!
//! Provides screen text recognition using the built-in Windows OCR engine
//! (Windows.Media.Ocr namespace). Requires Windows 10+. This is the Windows
//! equivalent of `screen_ocr.rs` (tesseract) on Linux.
//!
//! Flow: capture screen region → create SoftwareBitmap → OcrEngine.RecognizeAsync
//!
//! Unlike tesseract, the Windows OCR engine supports 25+ languages out of the
//! box and uses the system's installed language packs.

use aivyx_core::{AivyxError, Result};

/// Perform OCR on a region of the screen. Returns recognized text.
///
/// Coordinates are absolute screen pixels. Captures via BitBlt then
/// feeds the bitmap to Windows.Media.Ocr.
pub async fn ocr_region(x: i32, y: i32, w: i32, h: i32) -> Result<String> {
    #[cfg(target_os = "windows")]
    {
        use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
        use windows::Media::Ocr::OcrEngine;

        // Validate dimensions.
        if w <= 0 || h <= 0 {
            return Err(AivyxError::Validation(
                "OCR region must have positive dimensions".into(),
            ));
        }
        if (w * h * 4) as usize > super::MAX_SCREENSHOT_BYTES {
            return Err(AivyxError::Other("OCR region too large".into()));
        }

        // Capture the region.
        let pixels = capture_screen_region(x, y, w, h)?;

        // Create SoftwareBitmap from pixel data.
        let bitmap = SoftwareBitmap::Create(BitmapPixelFormat::Bgra8, w, h)
            .map_err(|e| AivyxError::Other(format!("SoftwareBitmap::Create: {e}")))?;

        // Copy pixels into the bitmap buffer.
        let buffer = bitmap
            .LockBuffer(windows::Graphics::Imaging::BitmapBufferAccessMode::Write)
            .map_err(|e| AivyxError::Other(format!("LockBuffer: {e}")))?;

        let reference = buffer
            .CreateReference()
            .map_err(|e| AivyxError::Other(format!("CreateReference: {e}")))?;

        // Use IMemoryBufferByteAccess to copy pixel data.
        use windows::Win32::System::WinRT::IMemoryBufferByteAccess;
        use windows::core::Interface;
        let byte_access: IMemoryBufferByteAccess = reference
            .cast()
            .map_err(|e| AivyxError::Other(format!("IMemoryBufferByteAccess: {e}")))?;

        let mut data_ptr = std::ptr::null_mut();
        let mut capacity = 0u32;
        unsafe {
            byte_access
                .GetBuffer(&mut data_ptr, &mut capacity)
                .map_err(|e| AivyxError::Other(format!("GetBuffer: {e}")))?;
            std::ptr::copy_nonoverlapping(
                pixels.as_ptr(),
                data_ptr,
                pixels.len().min(capacity as usize),
            );
        }
        drop(reference);
        drop(buffer);

        // Run OCR.
        let engine = OcrEngine::TryCreateFromUserProfileLanguages()
            .map_err(|e| AivyxError::Other(format!("OcrEngine: {e}")))?;

        let result = engine
            .RecognizeAsync(&bitmap)
            .map_err(|e| AivyxError::Other(format!("RecognizeAsync: {e}")))?
            .get()
            .map_err(|e| AivyxError::Other(format!("OCR result: {e}")))?;

        let text = result
            .Text()
            .map_err(|e| AivyxError::Other(format!("OCR text: {e}")))?
            .to_string();

        Ok(text)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (x, y, w, h);
        Err(AivyxError::Other(
            "win_ocr: only available on Windows".into(),
        ))
    }
}

/// Perform OCR on the entire screen. Returns recognized text.
pub async fn ocr_screen() -> Result<String> {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
        let w = unsafe { GetSystemMetrics(SM_CXSCREEN) };
        let h = unsafe { GetSystemMetrics(SM_CYSCREEN) };
        ocr_region(0, 0, w, h).await
    }
    #[cfg(not(target_os = "windows"))]
    {
        Err(AivyxError::Other(
            "win_ocr: only available on Windows".into(),
        ))
    }
}

/// Capture raw BGRA pixel data from a screen region via BitBlt.
#[cfg(target_os = "windows")]
fn capture_screen_region(x: i32, y: i32, w: i32, h: i32) -> Result<Vec<u8>> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC,
        DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, GetDIBits, ReleaseDC, SRCCOPY, SelectObject,
    };

    unsafe {
        let hdc_screen = GetDC(HWND(std::ptr::null_mut()));
        let hdc_mem = CreateCompatibleDC(hdc_screen);
        let hbitmap = CreateCompatibleBitmap(hdc_screen, w, h);
        let old = SelectObject(hdc_mem, hbitmap);

        BitBlt(hdc_mem, 0, 0, w, h, hdc_screen, x, y, SRCCOPY)
            .map_err(|e| AivyxError::Other(format!("BitBlt: {e}")))?;

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

        SelectObject(hdc_mem, old);
        DeleteObject(hbitmap);
        DeleteDC(hdc_mem);
        ReleaseDC(HWND(std::ptr::null_mut()), hdc_screen);

        Ok(pixels)
    }
}

// ── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ocr_on_non_windows() {
        #[cfg(not(target_os = "windows"))]
        {
            let rt = tokio::runtime::Runtime::new().unwrap();
            assert!(rt.block_on(ocr_region(0, 0, 100, 100)).is_err());
            assert!(rt.block_on(ocr_screen()).is_err());
        }
    }
}
