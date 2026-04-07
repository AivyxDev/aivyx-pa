//! Screen OCR — extract text from screen regions using Tesseract.
//!
//! The "last resort" for reading text from applications that don't expose
//! accessibility data (games, remote desktops, poorly-built Electron apps).
//! Takes a screenshot of a region, pipes it to `tesseract` for OCR.
//!
//! Requires: `tesseract` CLI installed (e.g., `sudo apt install tesseract-ocr`).

use aivyx_core::{AivyxError, Result};

/// Maximum OCR region area (4K screen = ~8M pixels). Prevents accidental
/// full-screen OCR which would be slow and return too much text.
const MAX_OCR_PIXELS: u64 = 4_000_000;

/// OCR a region of the screen and return the recognized text.
///
/// `region` is in grim geometry format: `"x,y widthxheight"` (e.g., `"100,200 800x600"`).
/// If `language` is provided, it's passed to tesseract's `-l` flag (e.g., `"eng"`, `"deu"`).
pub async fn ocr_region(
    region: &str,
    language: Option<&str>,
) -> Result<String> {
    // Validate region format.
    validate_region(region)?;

    // Step 1: Screenshot the region to a temp file (grim → PNG).
    let screenshot_data = capture_region(region).await?;

    // Step 2: Pipe screenshot data to tesseract.
    let text = run_tesseract(&screenshot_data, language).await?;

    Ok(text)
}

/// OCR from existing screenshot data (base64-decoded PNG/JPEG bytes).
pub async fn ocr_image_data(
    data: &[u8],
    language: Option<&str>,
) -> Result<String> {
    run_tesseract(data, language).await
}

/// Validate region format and size constraints.
fn validate_region(region: &str) -> Result<()> {
    // Expected: "x,y widthxheight"
    let parts: Vec<&str> = region.split_whitespace().collect();
    if parts.len() != 2 {
        return Err(AivyxError::Validation(format!(
            "Region must be in 'x,y widthxheight' format (e.g., '100,200 800x600'). Got: '{region}'"
        )));
    }

    // Parse origin.
    let origin: Vec<&str> = parts[0].split(',').collect();
    if origin.len() != 2 {
        return Err(AivyxError::Validation(format!(
            "Region origin must be 'x,y'. Got: '{}'", parts[0]
        )));
    }
    for coord in &origin {
        coord.parse::<i32>().map_err(|_| {
            AivyxError::Validation(format!("Invalid coordinate: '{coord}'"))
        })?;
    }

    // Parse dimensions.
    let dims: Vec<&str> = parts[1].split('x').collect();
    if dims.len() != 2 {
        return Err(AivyxError::Validation(format!(
            "Region dimensions must be 'widthxheight'. Got: '{}'", parts[1]
        )));
    }

    let width: u64 = dims[0].parse().map_err(|_| {
        AivyxError::Validation(format!("Invalid width: '{}'", dims[0]))
    })?;
    let height: u64 = dims[1].parse().map_err(|_| {
        AivyxError::Validation(format!("Invalid height: '{}'", dims[1]))
    })?;

    if width == 0 || height == 0 {
        return Err(AivyxError::Validation(
            "Region width and height must be positive".into(),
        ));
    }

    if width * height > MAX_OCR_PIXELS {
        return Err(AivyxError::Validation(format!(
            "Region too large ({} pixels, max {MAX_OCR_PIXELS}). Use a smaller region.",
            width * height
        )));
    }

    Ok(())
}

/// Capture a screen region using grim (Wayland) or import (X11).
async fn capture_region(region: &str) -> Result<Vec<u8>> {
    // Try grim first.
    let grim_result = tokio::process::Command::new("grim")
        .args(["-g", region, "-t", "png", "-"])
        .output()
        .await;

    if let Ok(output) = grim_result {
        if output.status.success() && !output.stdout.is_empty() {
            return Ok(output.stdout);
        }
    }

    // Fallback: import (ImageMagick) — crop from full screenshot.
    // import doesn't support the same geometry format, so use grim's format
    // to construct an ImageMagick geometry string: WxH+X+Y
    let (x, y, w, h) = parse_region_coords(region)?;
    let im_geom = format!("{w}x{h}+{x}+{y}");

    let output = tokio::process::Command::new("import")
        .args(["-window", "root", "-crop", &im_geom, "png:-"])
        .output()
        .await
        .map_err(|e| AivyxError::Other(format!(
            "Neither grim nor import available for screenshots: {e}. \
             Install grim (Wayland) or imagemagick (X11)."
        )))?;

    if !output.status.success() {
        return Err(AivyxError::Other(format!(
            "import failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    Ok(output.stdout)
}

/// Parse "x,y widthxheight" into (x, y, w, h).
fn parse_region_coords(region: &str) -> Result<(i32, i32, u32, u32)> {
    let parts: Vec<&str> = region.split_whitespace().collect();
    let origin: Vec<&str> = parts[0].split(',').collect();
    let dims: Vec<&str> = parts[1].split('x').collect();

    Ok((
        origin[0].parse().unwrap_or(0),
        origin[1].parse().unwrap_or(0),
        dims[0].parse().unwrap_or(0),
        dims[1].parse().unwrap_or(0),
    ))
}

/// Run tesseract on image data, returning recognized text.
async fn run_tesseract(
    image_data: &[u8],
    language: Option<&str>,
) -> Result<String> {
    use tokio::io::AsyncWriteExt;

    let mut args = vec![
        "stdin".to_string(),    // read from stdin
        "stdout".to_string(),   // write to stdout
    ];

    if let Some(lang) = language {
        args.push("-l".to_string());
        args.push(lang.to_string());
    }

    // --psm 3 = fully automatic page segmentation (default, works well for screen regions).
    args.push("--psm".to_string());
    args.push("3".to_string());

    let mut child = tokio::process::Command::new("tesseract")
        .args(&args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AivyxError::Other(
                    "tesseract is not installed. Install it for screen OCR \
                     (e.g., sudo apt install tesseract-ocr)"
                        .into(),
                )
            } else {
                AivyxError::Other(format!("tesseract failed to start: {e}"))
            }
        })?;

    // Write image data to stdin.
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(image_data).await.map_err(|e| {
            AivyxError::Other(format!("Failed to write to tesseract stdin: {e}"))
        })?;
        // Drop stdin to close the pipe and signal EOF.
    }

    let output = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        child.wait_with_output(),
    )
    .await
    .map_err(|_| AivyxError::Other("tesseract timed out (30s)".into()))?
    .map_err(|e| AivyxError::Other(format!("tesseract failed: {e}")))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AivyxError::Other(format!("tesseract error: {stderr}")));
    }

    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(text)
}

// ── Tests ──────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_region_good() {
        assert!(validate_region("100,200 800x600").is_ok());
        assert!(validate_region("0,0 1920x1080").is_ok());
        assert!(validate_region("500,300 400x200").is_ok());
    }

    #[test]
    fn validate_region_bad_format() {
        assert!(validate_region("").is_err());
        assert!(validate_region("100,200").is_err());
        assert!(validate_region("100 200 800 600").is_err());
        assert!(validate_region("abc,def 800x600").is_err());
    }

    #[test]
    fn validate_region_zero_dims() {
        assert!(validate_region("0,0 0x600").is_err());
        assert!(validate_region("0,0 800x0").is_err());
    }

    #[test]
    fn validate_region_too_large() {
        // 3000x2000 = 6M pixels > 4M limit
        assert!(validate_region("0,0 3000x2000").is_err());
    }

    #[test]
    fn validate_region_within_limit() {
        // 2000x1500 = 3M pixels < 4M limit
        assert!(validate_region("0,0 2000x1500").is_ok());
    }

    #[test]
    fn parse_region_coords_basic() {
        let (x, y, w, h) = parse_region_coords("100,200 800x600").unwrap();
        assert_eq!((x, y, w, h), (100, 200, 800, 600));
    }
}
