use crate::types::ScreenCaptureInfo;
use base64::Engine;

#[cfg(target_os = "windows")]
pub fn capture_screen_for_overlay() -> Result<ScreenCaptureInfo, String> {
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::UI::WindowsAndMessaging::*;

    unsafe {
        let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let mut width = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let mut height = GetSystemMetrics(SM_CYVIRTUALSCREEN);

        if width <= 0 || height <= 0 {
            width = GetSystemMetrics(SM_CXSCREEN);
            height = GetSystemMetrics(SM_CYSCREEN);
        }

        if width <= 0 || height <= 0 {
            return Err("Invalid screen dimensions".to_string());
        }

        let hdc_screen = GetDC(None);
        if hdc_screen.is_invalid() {
            return Err("Failed to get screen DC".to_string());
        }

        let hdc_mem = CreateCompatibleDC(Some(hdc_screen));
        if hdc_mem.is_invalid() {
            let _ = ReleaseDC(None, hdc_screen);
            return Err("Failed to create compatible DC".to_string());
        }

        let hbitmap = CreateCompatibleBitmap(hdc_screen, width, height);
        if hbitmap.is_invalid() {
            let _ = DeleteDC(hdc_mem);
            let _ = ReleaseDC(None, hdc_screen);
            return Err("Failed to create compatible bitmap".to_string());
        }

        let old_bitmap = SelectObject(hdc_mem, hbitmap.into());

        // CAPTUREBLT (0x40000000) includes hardware overlays, translucent & layered windows
        let bitblt_ok = BitBlt(
            hdc_mem,
            0,
            0,
            width,
            height,
            Some(hdc_screen),
            x,
            y,
            SRCCOPY | ROP_CODE(0x40000000),
        );

        let _ = SelectObject(hdc_mem, old_bitmap);

        if !bitblt_ok.is_ok() {
            let _ = DeleteObject(hbitmap.into());
            let _ = DeleteDC(hdc_mem);
            let _ = ReleaseDC(None, hdc_screen);
            return Err("Failed to BitBlt screen".to_string());
        }

        let mut bmi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width,
                biHeight: -height, // top-down DIB
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                biSizeImage: (width * height * 4) as u32,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            },
            bmiColors: [RGBQUAD::default()],
        };

        let mut bgra_pixels = vec![0u8; (width * height * 4) as usize];
        let lines = GetDIBits(
            hdc_screen,
            hbitmap,
            0,
            height as u32,
            Some(bgra_pixels.as_mut_ptr() as *mut _),
            &mut bmi,
            DIB_RGB_COLORS,
        );

        let _ = DeleteObject(hbitmap.into());
        let _ = DeleteDC(hdc_mem);
        let _ = ReleaseDC(None, hdc_screen);

        if lines == 0 {
            return Err("Failed to read DIBits".to_string());
        }

        let width_u32 = width as u32;
        let height_u32 = height as u32;

        // Encode as fast PNG
        let mut png_data = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png_data, width_u32, height_u32);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_compression(png::Compression::Fast);
            let mut writer = encoder.write_header().map_err(|e| format!("PNG header error: {}", e))?;

            // Convert BGRA to RGBA
            let mut rgba_pixels = vec![0u8; bgra_pixels.len()];
            for i in (0..bgra_pixels.len()).step_by(4) {
                rgba_pixels[i] = bgra_pixels[i + 2];     // R
                rgba_pixels[i + 1] = bgra_pixels[i + 1]; // G
                rgba_pixels[i + 2] = bgra_pixels[i];     // B
                rgba_pixels[i + 3] = 255;                // A (fully opaque)
            }

            writer.write_image_data(&rgba_pixels).map_err(|e| format!("PNG data error: {}", e))?;
        }

        let b64 = base64::engine::general_purpose::STANDARD.encode(&png_data);
        let image_data_url = format!("data:image/png;base64,{}", b64);

        Ok(ScreenCaptureInfo {
            image_data_url,
            physical_width: width_u32,
            physical_height: height_u32,
            scale_factor: 1.0,
        })
    }
}

#[cfg(target_os = "macos")]
pub fn capture_screen_for_overlay() -> Result<ScreenCaptureInfo, String> {
    use core_graphics::display::CGDisplay;

    let main_display = CGDisplay::main();
    let bounds = main_display.bounds();
    let image = main_display.image().ok_or("Failed to capture macOS display image")?;

    let width = image.width() as u32;
    let height = image.height() as u32;
    let raw_data = image.data();
    let bytes = raw_data.bytes();

    let mut png_data = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_data, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().map_err(|e| format!("PNG header error: {}", e))?;
        writer.write_image_data(bytes).map_err(|e| format!("PNG data error: {}", e))?;
    }

    let b64 = base64::engine::general_purpose::STANDARD.encode(&png_data);
    let image_data_url = format!("data:image/png;base64,{}", b64);

    let scale_factor = if bounds.size.width > 0.0 {
        width as f64 / bounds.size.width
    } else {
        1.0
    };

    Ok(ScreenCaptureInfo {
        image_data_url,
        physical_width: width,
        physical_height: height,
        scale_factor,
    })
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos")))]
pub fn capture_screen_for_overlay() -> Result<ScreenCaptureInfo, String> {
    Err("Screen capture not supported on this platform".to_string())
}
